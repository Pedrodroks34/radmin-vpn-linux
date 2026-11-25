

#include <ntddk.h>

#define DEVICE_NAME     L"\\Device\\RVPNNETMP"
#define DOSDEVICE_NAME  L"\\DosDevices\\RVPNNETMP"


#define FIFO_B2D_PATH       L"\\??\\Z:\\tmp\\rvpn_b2d"
#define FIFO_D2B_PATH       L"\\??\\Z:\\tmp\\rvpn_d2b"
#define FIFO_D2B_HIGH_PATH  L"\\??\\Z:\\tmp\\rvpn_d2b_high"
#define FIFO_D2B_LOW_PATH   L"\\??\\Z:\\tmp\\rvpn_d2b_low"


#define MAC_FILE_PATH L"\\??\\Z:\\tmp\\rvpn_mac"
static UCHAR g_adapter_mac[6] = { 0x02, 0x50, 0xDE, 0xAD, 0xBE, 0xEF };
static BOOLEAN g_mac_loaded = FALSE;


#define ETH_FRAME_MIN 60

typedef struct _DEVICE_EXTENSION {
    BOOLEAN Connected;
    UCHAR   MacAddress[6];
    LONG    IoctlCount;
    LONG    StatusCount;
    ULONG   SetupMode;       
    HANDLE  FifoB2D;         
    HANDLE  FifoD2B;         
    HANDLE  FifoD2B_High;    
    HANDLE  FifoD2B_Low;     
} DEVICE_EXTENSION, *PDEVICE_EXTENSION;


static PDEVICE_OBJECT g_DeviceObject = NULL;



#define LOG_BUF_SIZE 4096

static HANDLE  g_log_file = NULL;
static KSPIN_LOCK g_log_lock;
static CHAR    g_log_buf[LOG_BUF_SIZE];
static ULONG   g_log_buf_pos = 0;
static ULONG   g_log_call_count = 0;


static void drv_log_flush_locked(void)
{
    if (!g_log_file || g_log_buf_pos == 0)
        return;

    IO_STATUS_BLOCK iosb;
    ZwWriteFile(g_log_file, NULL, NULL, NULL, &iosb,
                g_log_buf, g_log_buf_pos, NULL, NULL);
    g_log_buf_pos = 0;
}

static void drv_log(const char *msg)
{
    if (!g_log_file)
        return;

    ULONG msgLen = 0;
    const char *p = msg;
    while (*p++) msgLen++;

    KIRQL oldIrql;
    KeAcquireSpinLock(&g_log_lock, &oldIrql);

    
    if (g_log_buf_pos + msgLen + 2 > LOG_BUF_SIZE)
        drv_log_flush_locked();

    RtlCopyMemory(g_log_buf + g_log_buf_pos, msg, msgLen);
    g_log_buf_pos += msgLen;
    RtlCopyMemory(g_log_buf + g_log_buf_pos, "\r\n", 2);
    g_log_buf_pos += 2;

    
    if (++g_log_call_count % 8 == 0)
        drv_log_flush_locked();

    KeReleaseSpinLock(&g_log_lock, oldIrql);
}

static void hex32(char *out, ULONG val)
{
    static const char hx[] = "0123456789abcdef";
    out[0] = hx[(val >> 28) & 0xf]; out[1] = hx[(val >> 24) & 0xf];
    out[2] = hx[(val >> 20) & 0xf]; out[3] = hx[(val >> 16) & 0xf];
    out[4] = hx[(val >> 12) & 0xf]; out[5] = hx[(val >>  8) & 0xf];
    out[6] = hx[(val >>  4) & 0xf]; out[7] = hx[(val      ) & 0xf];
    out[8] = 0;
}

static void drv_log_ioctl(ULONG code, ULONG inLen, ULONG outLen, const UCHAR *data, ULONG dataLen)
{
    char buf[256];
    char *p = buf;
    const char *prefix = "IOCTL 0x";
    while (*prefix) *p++ = *prefix++;
    hex32(p, code); p += 8;
    *p++ = ' '; *p++ = 'i'; *p++ = 'n'; *p++ = '=';
    hex32(p, inLen); p += 8;
    *p++ = ' '; *p++ = 'o'; *p++ = 'u'; *p++ = 't'; *p++ = '=';
    hex32(p, outLen); p += 8;
    if (data && dataLen > 0) {
        *p++ = ' '; *p++ = 'd'; *p++ = '=';
        ULONG i;
        for (i = 0; i < dataLen && i < 32 && (p - buf) < 240; i++) {
            static const char hx2[] = "0123456789abcdef";
            *p++ = hx2[(data[i] >> 4) & 0xf];
            *p++ = hx2[data[i] & 0xf];
        }
    }
    *p = 0;
    drv_log(buf);
}



static HANDLE open_fifo(const WCHAR *path, ACCESS_MASK access)
{
    UNICODE_STRING uPath;
    OBJECT_ATTRIBUTES oa;
    IO_STATUS_BLOCK iosb;
    HANDLE h = NULL;
    NTSTATUS st;

    RtlInitUnicodeString(&uPath, path);
    InitializeObjectAttributes(&oa, &uPath, OBJ_CASE_INSENSITIVE, NULL, NULL);

    st = ZwCreateFile(&h, access | SYNCHRONIZE, &oa, &iosb,
                      NULL, FILE_ATTRIBUTE_NORMAL,
                      FILE_SHARE_READ | FILE_SHARE_WRITE,
                      FILE_OPEN, FILE_SYNCHRONOUS_IO_NONALERT, NULL, 0);
    if (!NT_SUCCESS(st)) {
        char buf[80] = "FIFO open failed: 0x";
        hex32(buf + 20, (ULONG)st);
        drv_log(buf);
        return NULL;
    }
    return h;
}


static int is_ipv4_icmp(const UCHAR *frame, USHORT len)
{
    if (len < 34) return 0;
    
    if (frame[12] != 0x08 || frame[13] != 0x00) return 0;
    
    return (frame[23] == 1);
}


static int classify_high_priority(const UCHAR *frame, USHORT len)
{
    if (len < 14) return 0;

    USHORT ethertype = ((USHORT)frame[12] << 8) | frame[13];

    
    if (ethertype == 0x0806)
        return 1;

    if (ethertype == 0x0800) {
        
        if (len < 34) return 0;
        UCHAR proto = frame[23];
        if (proto == 6 || proto == 1) {
            return 1;  
        }
        if (proto == 17) {
            
            return !(frame[0] & 0x01);
        }
        return 0;
    }

    if (ethertype == 0x86dd) {
        
        if (len < 54) return 0;
        UCHAR proto = frame[20];  
        if (proto == 6 || proto == 58) {
            return 1;  
        }
        if (proto == 17) {
            return !(frame[0] & 0x01);
        }
        return 0;
    }

    return 0;  
}

static volatile LONG g_icmp_rx = 0;   
static volatile LONG g_icmp_tx = 0;   




#define RX_RING_SIZE 16
#define RX_FRAME_MAX 1600

typedef struct _RX_RING {
    volatile ULONG write_idx;
    volatile ULONG read_idx;
    struct {
        USHORT len;
        UCHAR  data[RX_FRAME_MAX];
    } frames[RX_RING_SIZE];
} RX_RING;

static RX_RING g_rx_ring;



#define IRP_QUEUE_SIZE 1024

static struct {
    KSPIN_LOCK Lock;
    PIRP       Irps[IRP_QUEUE_SIZE];
    LONG       Head;   
    LONG       Tail;   
    LONG       Count;
    PFILE_OBJECT FileObjs[IRP_QUEUE_SIZE]; 
} g_irp_queue;


#define PEER_ROUTES_INITIAL 16
#define PEER_ROUTES_MAX     4096
#define PEER_ROUTES_TAG     'rPvR'  

typedef struct _PEER_ROUTE {
    PFILE_OBJECT fo;
    UCHAR mac[6];
} PEER_ROUTE;

static PEER_ROUTE *g_peer_routes = NULL;
static LONG        g_peer_routes_capacity = 0;  
static volatile LONG g_peer_route_count = 0;


static volatile LONG g_ring_enqueued = 0;
static volatile LONG g_ring_consumed = 0;
static volatile LONG g_ring_dropped  = 0;

static volatile LONG g_rx_cancel_ceded  = 0;
static volatile LONG g_irp_cancelled    = 0;
static volatile LONG g_queue_full_drops = 0;
static volatile LONG g_irp_completed    = 0;


static PIRP         g_compact_keep_irps[IRP_QUEUE_SIZE];
static PFILE_OBJECT g_compact_keep_fos[IRP_QUEUE_SIZE];
static PIRP         g_compact_picked[IRP_QUEUE_SIZE];
static PFILE_OBJECT g_compact_seen[IRP_QUEUE_SIZE];


static ULONG tlv_encode_frame(PUCHAR outBuf, ULONG bufRemain, const UCHAR *frame, USHORT frameLen, ULONG setupMode)
{
    ULONG paddedLen = (frameLen < ETH_FRAME_MIN) ? ETH_FRAME_MIN : (ULONG)frameLen;
    ULONG headerSize = (setupMode >= 2) ? 8 : 4;  
    ULONG needed = headerSize + paddedLen;

    if (needed > bufRemain)
        return 0;  

    ULONG pos = 0;

    
    if (setupMode >= 2) {
        ULONG macPrefix = 0;
        if (frameLen >= 4) {
            ULONG raw;
            RtlCopyMemory(&raw, frame, 4);
            macPrefix = (raw << 24) | ((raw & 0xff00) << 8) |
                        ((raw >> 8) & 0xff00) | (raw >> 24);
        }
        RtlCopyMemory(outBuf + pos, &macPrefix, 4);
        
        {
            static LONG rd_mac_log = 0;
            if (InterlockedIncrement(&rd_mac_log) <= 3) {
                char buf[80] = "RD mac=";
                static const char hx4[] = "0123456789abcdef";
                UCHAR *mb = (UCHAR *)&macPrefix;
                int j;
                for (j = 0; j < 4; j++) {
                    buf[7 + j*2] = hx4[(mb[j] >> 4) & 0xf];
                    buf[8 + j*2] = hx4[mb[j] & 0xf];
                }
                
                buf[15] = ' '; buf[16] = 'd'; buf[17] = 's'; buf[18] = 't'; buf[19] = '=';
                for (j = 0; j < 6 && j < frameLen; j++) {
                    buf[20 + j*2] = hx4[(frame[j] >> 4) & 0xf];
                    buf[21 + j*2] = hx4[frame[j] & 0xf];
                }
                buf[20 + (j < 6 ? j : 6)*2] = 0;
                drv_log(buf);
            }
        }
        pos += 4;
    }

    
    RtlCopyMemory(outBuf + pos, &paddedLen, 4);
    pos += 4;

    
    RtlCopyMemory(outBuf + pos, frame, frameLen);
    pos += frameLen;

    
    if (frameLen < ETH_FRAME_MIN) {
        RtlZeroMemory(outBuf + pos, ETH_FRAME_MIN - frameLen);
        pos += (ETH_FRAME_MIN - frameLen);
    }

    return pos;
}


static void complete_read_irp(PIRP irp, const UCHAR *frame, USHORT frameLen)
{
    PUCHAR outBuf = NULL;
    if (irp->MdlAddress)
        outBuf = (PUCHAR)MmGetSystemAddressForMdlSafe(irp->MdlAddress, NormalPagePriority);

    if (!outBuf || frameLen == 0) {
        irp->IoStatus.Status = STATUS_SUCCESS;
        irp->IoStatus.Information = 0;
        IoCompleteRequest(irp, IO_NO_INCREMENT);
        return;
    }

    PIO_STACK_LOCATION irpSp = IoGetCurrentIrpStackLocation(irp);
    ULONG bufLen = irpSp->Parameters.Read.Length;

    
    ULONG mode = 0;
    if (g_DeviceObject) {
        PDEVICE_EXTENSION ext = (PDEVICE_EXTENSION)g_DeviceObject->DeviceExtension;
        mode = ext->SetupMode;
    }

    ULONG written = tlv_encode_frame(outBuf, bufLen, frame, frameLen, mode);

    irp->IoStatus.Status = STATUS_SUCCESS;
    irp->IoStatus.Information = written;
    IoCompleteRequest(irp, IO_NO_INCREMENT);
}




static NTSTATUS fifo_read_exact(HANDLE fifo, void *buf, ULONG n)
{
    IO_STATUS_BLOCK iosb;
    ULONG got = 0;
    while (got < n) {
        NTSTATUS st = ZwReadFile(fifo, NULL, NULL, NULL, &iosb,
                                 (UCHAR *)buf + got, n - got, NULL, NULL);
        if (!NT_SUCCESS(st)) return st;
        if (iosb.Information == 0) return STATUS_END_OF_FILE;  
        got += (ULONG)iosb.Information;
    }
    return STATUS_SUCCESS;
}




static BOOLEAN dequeue_specific_irp_locked(PIRP irp)
{
    LONG keep_n = 0;
    BOOLEAN found = FALSE;
    for (LONG i = 0; i < g_irp_queue.Count; i++) {
        LONG idx = (g_irp_queue.Head + i) % IRP_QUEUE_SIZE;
        if (!found && g_irp_queue.Irps[idx] == irp) {
            found = TRUE;
        } else {
            g_compact_keep_irps[keep_n] = g_irp_queue.Irps[idx];
            g_compact_keep_fos[keep_n]  = g_irp_queue.FileObjs[idx];
            keep_n++;
        }
    }
    if (found) {
        for (LONG i = 0; i < keep_n; i++) {
            g_irp_queue.Irps[i]     = g_compact_keep_irps[i];
            g_irp_queue.FileObjs[i] = g_compact_keep_fos[i];
        }
        g_irp_queue.Head  = 0;
        g_irp_queue.Tail  = keep_n % IRP_QUEUE_SIZE;
        g_irp_queue.Count = keep_n;
    }
    return found;
}


static VOID NTAPI RvpnCancelRoutine(PDEVICE_OBJECT DeviceObject, PIRP Irp)
{
    (void)DeviceObject;
    
    IoReleaseCancelSpinLock(Irp->CancelIrql);

    KIRQL oldIrql;
    KeAcquireSpinLock(&g_irp_queue.Lock, &oldIrql);
    dequeue_specific_irp_locked(Irp);   
    KeReleaseSpinLock(&g_irp_queue.Lock, oldIrql);

    
    Irp->IoStatus.Status = STATUS_CANCELLED;
    Irp->IoStatus.Information = 0;
    IoCompleteRequest(Irp, IO_NO_INCREMENT);

    LONG c = InterlockedIncrement(&g_irp_cancelled);
    if (c <= 5 || (c % 100) == 0) {
        char buf[32] = "IRP cancelled #";
        hex32(buf + 15, (ULONG)c);
        buf[23] = 0;
        drv_log(buf);
    }
}


static BOOLEAN rx_claim_irp(PIRP irp)
{
    if (IoSetCancelRoutine(irp, NULL) != NULL)
        return TRUE;
    LONG c = InterlockedIncrement(&g_rx_cancel_ceded);
    if (c <= 5 || (c % 100) == 0) {
        char buf[32] = "RX cancel-ceded #";
        hex32(buf + 17, (ULONG)c);
        buf[25] = 0;
        drv_log(buf);
    }
    return FALSE;
}

static void __stdcall rx_thread_proc(PVOID context)
{
    HANDLE fifo = (HANDLE)context;
    IO_STATUS_BLOCK iosb;
    NTSTATUS st;
    LONG rx_count = 0;
    UCHAR frameBuf[RX_FRAME_MAX];

    drv_log("RX thread started");

    while (1) {
        USHORT frameLen = 0;

        st = fifo_read_exact(fifo, &frameLen, sizeof(frameLen));
        if (!NT_SUCCESS(st)) {
            char buf[96] = "RX exit: st=0x";
            hex32(buf + 14, (ULONG)st);
            char *p = buf + 22;
            *p++ = ' '; *p++ = 'n'; *p++ = '=';
            hex32(p, (ULONG)rx_count);
            drv_log(buf);
            break;
        }

        if (frameLen == 0 || frameLen > RX_FRAME_MAX) {
            UCHAR drain[64];
            USHORT rem = frameLen;
            while (rem > 0) {
                USHORT chunk = rem > 64 ? 64 : rem;
                ZwReadFile(fifo, NULL, NULL, NULL, &iosb, drain, chunk, NULL, NULL);
                if (iosb.Information == 0) break;
                rem -= (USHORT)iosb.Information;
            }
            continue;
        }

        st = fifo_read_exact(fifo, frameBuf, frameLen);
        if (!NT_SUCCESS(st))
            break;

        rx_count++;

        
        if (is_ipv4_icmp(frameBuf, frameLen)) {
            LONG n = InterlockedIncrement(&g_icmp_rx);
            if (n <= 3 || (n % 100) == 0) {
                char buf[64] = "ICMP-OUT #";
                hex32(buf + 10, (ULONG)n);
                buf[18] = ' '; buf[19] = 'l'; buf[20] = '=';
                hex32(buf + 21, (ULONG)frameLen);
                buf[29] = 0;
                drv_log(buf);
            }
        }

        
        {
            KIRQL oldIrql;
            KeAcquireSpinLock(&g_irp_queue.Lock, &oldIrql);
            if (g_irp_queue.Count > 0) {
                UCHAR *dstMac = frameBuf;  
                int is_group = (frameLen >= 1 && (dstMac[0] & 0x01));

                
                PFILE_OBJECT target_fo = NULL;
                if (!is_group && frameLen >= 6 && g_peer_routes) {
                    LONG rc = g_peer_route_count;
                    if (rc > g_peer_routes_capacity) rc = g_peer_routes_capacity;
                    for (LONG r = 0; r < rc; r++) {
                        if (g_peer_routes[r].fo != NULL &&
                            RtlCompareMemory(g_peer_routes[r].mac, dstMac, 6) == 6) {
                            target_fo = g_peer_routes[r].fo;
                            break;
                        }
                    }
                }

                if (is_group) {
                    
                    LONG picked_n = 0;
                    LONG seen_n = 0;
                    LONG keep_n = 0;

                    for (LONG i = 0; i < g_irp_queue.Count; i++) {
                        LONG idx = (g_irp_queue.Head + i) % IRP_QUEUE_SIZE;
                        PFILE_OBJECT fo = g_irp_queue.FileObjs[idx];
                        int already = 0;
                        for (LONG s = 0; s < seen_n; s++) {
                            if (g_compact_seen[s] == fo) { already = 1; break; }
                        }
                        if (!already && seen_n < IRP_QUEUE_SIZE) {
                            g_compact_seen[seen_n++] = fo;
                            g_compact_picked[picked_n++] = g_irp_queue.Irps[idx];
                        } else {
                            g_compact_keep_irps[keep_n] = g_irp_queue.Irps[idx];
                            g_compact_keep_fos[keep_n]  = fo;
                            keep_n++;
                        }
                    }
                    for (LONG i = 0; i < keep_n; i++) {
                        g_irp_queue.Irps[i]     = g_compact_keep_irps[i];
                        g_irp_queue.FileObjs[i] = g_compact_keep_fos[i];
                    }
                    g_irp_queue.Head  = 0;
                    g_irp_queue.Tail  = keep_n % IRP_QUEUE_SIZE;  
                    g_irp_queue.Count = keep_n;
                    KeReleaseSpinLock(&g_irp_queue.Lock, oldIrql);

                    
                    for (LONG i = 0; i < picked_n; i++) {
                        
                        if (rx_claim_irp(g_compact_picked[i])) {
                            complete_read_irp(g_compact_picked[i], frameBuf, frameLen);
                            InterlockedIncrement(&g_irp_completed);
                        }
                    }
                } else if (target_fo) {
                    
                    PIRP found = NULL;
                    LONG keep_n = 0;
                    for (LONG i = 0; i < g_irp_queue.Count; i++) {
                        LONG idx = (g_irp_queue.Head + i) % IRP_QUEUE_SIZE;
                        if (!found && g_irp_queue.FileObjs[idx] == target_fo) {
                            found = g_irp_queue.Irps[idx];
                        } else {
                            g_compact_keep_irps[keep_n] = g_irp_queue.Irps[idx];
                            g_compact_keep_fos[keep_n]  = g_irp_queue.FileObjs[idx];
                            keep_n++;
                        }
                    }
                    for (LONG i = 0; i < keep_n; i++) {
                        g_irp_queue.Irps[i]     = g_compact_keep_irps[i];
                        g_irp_queue.FileObjs[i] = g_compact_keep_fos[i];
                    }
                    g_irp_queue.Head  = 0;
                    g_irp_queue.Tail  = keep_n % IRP_QUEUE_SIZE;  
                    g_irp_queue.Count = keep_n;
                    KeReleaseSpinLock(&g_irp_queue.Lock, oldIrql);

                    if (found) {
                        if (rx_claim_irp(found)) {
                            complete_read_irp(found, frameBuf, frameLen);
                            InterlockedIncrement(&g_irp_completed);
                        }
                    } else {
                        static LONG udrop = 0;
                        if (InterlockedIncrement(&udrop) <= 3)
                            drv_log("RX unicast no IRP");
                    }
                } else {
                    
                    static LONG ndrop = 0;
                    if (InterlockedIncrement(&ndrop) <= 3)
                        drv_log("RX no route");
                    KeReleaseSpinLock(&g_irp_queue.Lock, oldIrql);
                }
                continue;
            }
            KeReleaseSpinLock(&g_irp_queue.Lock, oldIrql);
        }

        
        ULONG widx = g_rx_ring.write_idx;
        ULONG ridx = g_rx_ring.read_idx;
        if (widx - ridx >= RX_RING_SIZE) {
            InterlockedIncrement(&g_ring_dropped);
            static LONG drops = 0;
            if (InterlockedIncrement(&drops) <= 3)
                drv_log("RX ring full, drop");
            continue;
        }

        ULONG slot = widx % RX_RING_SIZE;
        RtlCopyMemory(g_rx_ring.frames[slot].data, frameBuf, frameLen);
        g_rx_ring.frames[slot].len = frameLen;
        InterlockedIncrement((volatile LONG *)&g_rx_ring.write_idx);
        InterlockedIncrement(&g_ring_enqueued);
    }

    
    {
        char buf[160] = "RX stats: irp=";
        hex32(buf + 14, (ULONG)g_irp_completed);
        char *p = buf + 22;
        *p++ = ' '; *p++ = 'r'; *p++ = 'n'; *p++ = 'g'; *p++ = '=';
        hex32(p, (ULONG)g_ring_enqueued);
        p += 8;
        *p++ = ' '; *p++ = 'c'; *p++ = 'o'; *p++ = 'n'; *p++ = '=';
        hex32(p, (ULONG)g_ring_consumed);
        p += 8;
        *p++ = ' '; *p++ = 'd'; *p++ = 'r'; *p++ = 'p'; *p++ = '=';
        hex32(p, (ULONG)g_ring_dropped);
        p += 8;
        *p++ = ' '; *p++ = 'r'; *p++ = 'x'; *p++ = '=';
        hex32(p, (ULONG)rx_count);
        p += 8;
        *p++ = ' '; *p++ = 'I'; *p++ = 'o'; *p++ = '=';
        hex32(p, (ULONG)g_icmp_rx);
        p += 8;
        *p++ = ' '; *p++ = 'I'; *p++ = 'i'; *p++ = '=';
        hex32(p, (ULONG)g_icmp_tx);
        *p = 0;
        drv_log(buf);
    }

    PsTerminateSystemThread(STATUS_SUCCESS);
}



static void load_adapter_mac(void)
{
    if (g_mac_loaded) return;
    UNICODE_STRING path;
    RtlInitUnicodeString(&path, MAC_FILE_PATH);
    OBJECT_ATTRIBUTES oa;
    InitializeObjectAttributes(&oa, &path, OBJ_CASE_INSENSITIVE | OBJ_KERNEL_HANDLE, NULL, NULL);
    IO_STATUS_BLOCK iosb;
    HANDLE hFile;
    NTSTATUS st = ZwOpenFile(&hFile, FILE_READ_DATA | SYNCHRONIZE, &oa, &iosb,
                              FILE_SHARE_READ, FILE_SYNCHRONOUS_IO_NONALERT);
    if (NT_SUCCESS(st)) {
        UCHAR buf[6];
        st = ZwReadFile(hFile, NULL, NULL, NULL, &iosb, buf, 6, NULL, NULL);
        if (NT_SUCCESS(st) && iosb.Information == 6) {
            RtlCopyMemory(g_adapter_mac, buf, 6);
            char msg[48] = "MAC loaded: ";
            for (int i = 0; i < 6; i++) {
                msg[12 + i*3] = "0123456789abcdef"[buf[i] >> 4];
                msg[13 + i*3] = "0123456789abcdef"[buf[i] & 0xf];
                msg[14 + i*3] = (i < 5) ? ':' : '\0';
            }
            drv_log(msg);
        }
        ZwClose(hFile);
    } else {
        drv_log("MAC file not found, using default");
    }
    g_mac_loaded = TRUE;
}

static NTSTATUS NTAPI DispatchCreate(PDEVICE_OBJECT DeviceObject, PIRP Irp)
{
    PDEVICE_EXTENSION ext = (PDEVICE_EXTENSION)DeviceObject->DeviceExtension;
    ext->Connected = TRUE;

    load_adapter_mac();

    if (!ext->FifoB2D) {
        ext->FifoB2D = open_fifo(FIFO_B2D_PATH, FILE_READ_DATA);
        if (ext->FifoB2D) {
            drv_log("FIFO b2d opened");
            HANDLE hThread;
            OBJECT_ATTRIBUTES oa2;
            InitializeObjectAttributes(&oa2, NULL, OBJ_KERNEL_HANDLE, NULL, NULL);
            if (NT_SUCCESS(PsCreateSystemThread(&hThread, THREAD_ALL_ACCESS, &oa2,
                                                 NULL, NULL, rx_thread_proc, ext->FifoB2D))) {
                ZwClose(hThread);
                drv_log("RX thread created");
            }
        }
    }
    if (!ext->FifoD2B_High) {
        ext->FifoD2B_High = open_fifo(FIFO_D2B_HIGH_PATH, FILE_WRITE_DATA);
        if (ext->FifoD2B_High) drv_log("FIFO d2b_high opened");
    }
    if (!ext->FifoD2B_Low) {
        ext->FifoD2B_Low  = open_fifo(FIFO_D2B_LOW_PATH,  FILE_WRITE_DATA);
        if (ext->FifoD2B_Low) drv_log("FIFO d2b_low opened");
    }

    Irp->IoStatus.Status = STATUS_SUCCESS;
    Irp->IoStatus.Information = 0;
    IoCompleteRequest(Irp, IO_NO_INCREMENT);
    return STATUS_SUCCESS;
}

static NTSTATUS NTAPI DispatchClose(PDEVICE_OBJECT DeviceObject, PIRP Irp)
{
    (void)DeviceObject;

    
    PIO_STACK_LOCATION sp = IoGetCurrentIrpStackLocation(Irp);
    PFILE_OBJECT fo = sp->FileObject;
    if (fo) {
        KIRQL oldIrql;
        LONG freed = 0;
        KeAcquireSpinLock(&g_irp_queue.Lock, &oldIrql);
        for (LONG i = 0; i < g_peer_route_count; i++) {
            if (g_peer_routes[i].fo == fo) {
                g_peer_routes[i].fo = NULL;
                RtlZeroMemory(g_peer_routes[i].mac, 6);
                freed++;
            }
        }
        while (g_peer_route_count > 0 &&
               g_peer_routes[g_peer_route_count - 1].fo == NULL)
            g_peer_route_count--;
        LONG wm = g_peer_route_count;
        KeReleaseSpinLock(&g_irp_queue.Lock, oldIrql);

        if (freed > 0) {
            char buf[48] = "CLOSE freed=";
            hex32(buf + 12, (ULONG)freed);
            buf[20] = ' '; buf[21] = 'w'; buf[22] = 'm'; buf[23] = '=';
            hex32(buf + 24, (ULONG)wm);
            buf[32] = 0;
            drv_log(buf);
        }
    }

    Irp->IoStatus.Status = STATUS_SUCCESS;
    Irp->IoStatus.Information = 0;
    IoCompleteRequest(Irp, IO_NO_INCREMENT);
    return STATUS_SUCCESS;
}


static NTSTATUS NTAPI DispatchCleanup(PDEVICE_OBJECT DeviceObject, PIRP Irp)
{
    (void)DeviceObject;
    PIO_STACK_LOCATION sp = IoGetCurrentIrpStackLocation(Irp);
    PFILE_OBJECT fo = sp->FileObject;

    
    PIRP cancel_local[IRP_QUEUE_SIZE];
    LONG cancel_n = 0;

    if (fo) {
        KIRQL oldIrql;
        KeAcquireSpinLock(&g_irp_queue.Lock, &oldIrql);

        LONG keep_n = 0;
        for (LONG i = 0; i < g_irp_queue.Count; i++) {
            LONG idx = (g_irp_queue.Head + i) % IRP_QUEUE_SIZE;
            if (g_irp_queue.FileObjs[idx] == fo) {
                if (cancel_n < IRP_QUEUE_SIZE)
                    cancel_local[cancel_n++] = g_irp_queue.Irps[idx];
            } else {
                g_compact_keep_irps[keep_n] = g_irp_queue.Irps[idx];
                g_compact_keep_fos[keep_n]  = g_irp_queue.FileObjs[idx];
                keep_n++;
            }
        }
        for (LONG i = 0; i < keep_n; i++) {
            g_irp_queue.Irps[i]     = g_compact_keep_irps[i];
            g_irp_queue.FileObjs[i] = g_compact_keep_fos[i];
        }
        g_irp_queue.Head  = 0;
        g_irp_queue.Tail  = keep_n % IRP_QUEUE_SIZE;  
        g_irp_queue.Count = keep_n;

        KeReleaseSpinLock(&g_irp_queue.Lock, oldIrql);
    }

    for (LONG i = 0; i < cancel_n; i++) {
        PIRP irp = cancel_local[i];
        
        if (rx_claim_irp(irp)) {
            irp->IoStatus.Status = STATUS_CANCELLED;
            irp->IoStatus.Information = 0;
            IoCompleteRequest(irp, IO_NO_INCREMENT);
        }
    }

    if (cancel_n > 0) {
        char buf[48] = "CLEANUP cancel=";
        hex32(buf + 15, (ULONG)cancel_n);
        buf[23] = 0;
        drv_log(buf);
    }

    Irp->IoStatus.Status = STATUS_SUCCESS;
    Irp->IoStatus.Information = 0;
    IoCompleteRequest(Irp, IO_NO_INCREMENT);
    return STATUS_SUCCESS;
}


#define IOCTL_RVPN_VERSION    0x0022c004  
#define IOCTL_RVPN_STATUS     0x00224018  
#define IOCTL_RVPN_CONNECT    0x00224020  
#define IOCTL_RVPN_SETLINK    0x00224010  
#define IOCTL_RVPN_SETUP      0x0022801c  
#define IOCTL_RVPN_PEERMAC    0x00228014  
#define IOCTL_RVPN_SETMAC     0x00228024  
#define IOCTL_RVPN_VERSION2   0x0022c004  

static NTSTATUS NTAPI DispatchDeviceControl(PDEVICE_OBJECT DeviceObject, PIRP Irp)
{
    PIO_STACK_LOCATION irpSp = IoGetCurrentIrpStackLocation(Irp);
    ULONG ioctl = irpSp->Parameters.DeviceIoControl.IoControlCode;
    ULONG inLen = irpSp->Parameters.DeviceIoControl.InputBufferLength;
    ULONG outLen = irpSp->Parameters.DeviceIoControl.OutputBufferLength;
    PVOID sysBuffer = Irp->AssociatedIrp.SystemBuffer;
    NTSTATUS status = STATUS_SUCCESS;
    ULONG info = 0;

    PDEVICE_EXTENSION dext = (PDEVICE_EXTENSION)DeviceObject->DeviceExtension;
    InterlockedIncrement(&dext->IoctlCount);

    if (ioctl == IOCTL_RVPN_STATUS) {
        LONG sc = InterlockedIncrement(&dext->StatusCount);
        if (sc <= 3 || (sc % 500) == 0)
            drv_log("IOCTL STATUS poll");
    } else {
        drv_log_ioctl(ioctl, inLen, outLen,
                      (const UCHAR *)sysBuffer, inLen);
    }

    switch (ioctl) {
    case IOCTL_RVPN_VERSION:
        
        if (outLen >= 12 && sysBuffer) {
            ULONG *inData = (ULONG *)sysBuffer;
            UCHAR *out = (UCHAR *)sysBuffer;
            ULONG reqVer = (inLen >= 4) ? inData[0] : 0;

            if (reqVer == 4) {
                
                ULONG st_val = 0;
                RtlCopyMemory(out, &st_val, 4);           
                RtlCopyMemory(out + 4, g_adapter_mac, 6); 
                RtlZeroMemory(out + 10, 2);                
                info = 12;
                drv_log("VERSION: returned MAC");
            } else {
                ULONG st_val = 1;
                RtlCopyMemory(out, &st_val, 4);
                RtlZeroMemory(out + 4, 8);
                info = 12;
            }
        }
        break;

    case IOCTL_RVPN_STATUS:
        
        if (outLen >= 4 && sysBuffer) {
            *((ULONG *)sysBuffer) = 1;
            info = 4;
        }
        break;

    case IOCTL_RVPN_CONNECT:
        
        if (outLen >= 1 && sysBuffer) {
            *((UCHAR *)sysBuffer) = 0x20;  
            info = 1;
        }
        break;

    case IOCTL_RVPN_SETLINK:
        
        if (outLen >= 0xB8 && sysBuffer) {
            RtlZeroMemory(sysBuffer, 0xB8);
            info = 0xB8;
        }
        break;

    case IOCTL_RVPN_SETMAC:
        
        if (outLen >= 1 && sysBuffer) {
            *((UCHAR *)sysBuffer) = 0;
            info = 1;
        }
        break;



    case IOCTL_RVPN_SETUP:
        if (inLen >= 4 && sysBuffer) {
            dext->SetupMode = *((ULONG *)sysBuffer);
            char buf[48] = "SETUP mode=";
            hex32(buf + 11, dext->SetupMode);
            drv_log(buf);
        }
        break;

    case IOCTL_RVPN_PEERMAC:
        if (inLen >= 6 && sysBuffer) {
            PIO_STACK_LOCATION sp2 = IoGetCurrentIrpStackLocation(Irp);
            PFILE_OBJECT fo = sp2->FileObject;
            KIRQL oldIrql;
            LONG assigned = -1;
            LONG grew_to = 0;

            
            for (;;) {
                KeAcquireSpinLock(&g_irp_queue.Lock, &oldIrql);
                for (LONG i = 0; i < g_peer_route_count; i++) {
                    if (g_peer_routes[i].fo == NULL) { assigned = i; break; }
                }
                if (assigned < 0 && g_peer_route_count < g_peer_routes_capacity)
                    assigned = g_peer_route_count++;

                if (assigned >= 0) {
                    g_peer_routes[assigned].fo = fo;
                    RtlCopyMemory(g_peer_routes[assigned].mac, sysBuffer, 6);
                    KeReleaseSpinLock(&g_irp_queue.Lock, oldIrql);
                    break;
                }

                
                LONG old_cap = g_peer_routes_capacity;
                KeReleaseSpinLock(&g_irp_queue.Lock, oldIrql);

                if (old_cap >= PEER_ROUTES_MAX) {
                    drv_log("PEERMAC table full (max capacity)");
                    break;
                }

                LONG new_cap = old_cap * 2;
                if (new_cap > PEER_ROUTES_MAX) new_cap = PEER_ROUTES_MAX;

                PEER_ROUTE *new_buf = (PEER_ROUTE *)ExAllocatePoolWithTag(
                    NonPagedPool, sizeof(PEER_ROUTE) * new_cap, PEER_ROUTES_TAG);
                if (!new_buf) {
                    drv_log("PEERMAC grow: alloc failed");
                    break;
                }
                RtlZeroMemory(new_buf, sizeof(PEER_ROUTE) * new_cap);

                PEER_ROUTE *old_buf = NULL;
                KeAcquireSpinLock(&g_irp_queue.Lock, &oldIrql);
                if (g_peer_routes_capacity == old_cap) {
                    RtlCopyMemory(new_buf, g_peer_routes,
                                  sizeof(PEER_ROUTE) * g_peer_routes_capacity);
                    old_buf = g_peer_routes;
                    g_peer_routes = new_buf;
                    g_peer_routes_capacity = new_cap;
                    grew_to = new_cap;
                } else {
                    
                    old_buf = new_buf;
                }
                KeReleaseSpinLock(&g_irp_queue.Lock, oldIrql);
                ExFreePoolWithTag(old_buf, PEER_ROUTES_TAG);
                
            }

            char buf[64] = "PEERMAC fo=";
            hex32(buf + 11, (ULONG)(ULONG_PTR)fo);
            buf[19] = ' ';
            for (int k = 0; k < 6; k++) {
                UCHAR b = ((UCHAR*)sysBuffer)[k];
                buf[20 + k*3] = "0123456789abcdef"[b >> 4];
                buf[21 + k*3] = "0123456789abcdef"[b & 0xf];
                buf[22 + k*3] = (k < 5) ? ':' : '\0';
            }
            drv_log(buf);
            if (grew_to) {
                char gb[48] = "PEERMAC grew cap=";
                hex32(gb + 17, (ULONG)grew_to);
                gb[25] = 0;
                drv_log(gb);
            }
        }
        break;

    default: {
        
        char ubuf[64] = "IOCTL_UNKNOWN 0x";
        hex32(ubuf + 16, ioctl);
        ubuf[24] = ' '; ubuf[25] = 'o'; ubuf[26] = '=';
        hex32(ubuf + 27, outLen);
        ubuf[35] = 0;
        drv_log(ubuf);
        break;
    }
    }

    Irp->IoStatus.Status = status;
    Irp->IoStatus.Information = info;
    IoCompleteRequest(Irp, IO_NO_INCREMENT);
    return status;
}


static NTSTATUS NTAPI DispatchRead(PDEVICE_OBJECT DeviceObject, PIRP Irp)
{
    PDEVICE_EXTENSION ext = (PDEVICE_EXTENSION)DeviceObject->DeviceExtension;

    if (!ext->FifoB2D) {
        Irp->IoStatus.Status = STATUS_DEVICE_NOT_CONNECTED;
        Irp->IoStatus.Information = 0;
        IoCompleteRequest(Irp, IO_NO_INCREMENT);
        return STATUS_DEVICE_NOT_CONNECTED;
    }

    ULONG ridx = g_rx_ring.read_idx;

    if (ridx < g_rx_ring.write_idx) {
        
        PUCHAR outBuf = NULL;
        if (Irp->MdlAddress)
            outBuf = (PUCHAR)MmGetSystemAddressForMdlSafe(Irp->MdlAddress, NormalPagePriority);

        if (!outBuf) {
            Irp->IoStatus.Status = STATUS_SUCCESS;
            Irp->IoStatus.Information = 0;
            IoCompleteRequest(Irp, IO_NO_INCREMENT);
            return STATUS_SUCCESS;
        }

        PIO_STACK_LOCATION irpSp = IoGetCurrentIrpStackLocation(Irp);
        ULONG bufLen = irpSp->Parameters.Read.Length;
        ULONG totalWritten = 0;
        ULONG framesReturned = 0;

        while (ridx < g_rx_ring.write_idx) {
            ULONG slot = ridx % RX_RING_SIZE;
            USHORT frameLen = g_rx_ring.frames[slot].len;

            ULONG written = tlv_encode_frame(outBuf + totalWritten,
                                              bufLen - totalWritten,
                                              g_rx_ring.frames[slot].data,
                                              frameLen, ext->SetupMode);
            if (written == 0)
                break;  

            totalWritten += written;
            InterlockedIncrement((volatile LONG *)&g_rx_ring.read_idx);
            InterlockedIncrement(&g_ring_consumed);
            ridx++;
            framesReturned++;
        }

        static LONG read_ok = 0;
        LONG rc = InterlockedIncrement(&read_ok);
        if (rc <= 10 || (rc % 100) == 0) {
            char buf[80] = "READ ring frames=";
            hex32(buf + 17, (ULONG)framesReturned);
            char *p = buf + 25;
            *p++ = ' '; *p++ = 'b'; *p++ = '=';
            hex32(p, totalWritten);
            drv_log(buf);
        }

        Irp->IoStatus.Status = STATUS_SUCCESS;
        Irp->IoStatus.Information = totalWritten;
        IoCompleteRequest(Irp, IO_NO_INCREMENT);
        return STATUS_SUCCESS;
    }

    
    IoMarkIrpPending(Irp);

    {
        KIRQL oldIrql;
        KeAcquireSpinLock(&g_irp_queue.Lock, &oldIrql);
        if (g_irp_queue.Count >= IRP_QUEUE_SIZE) {
            
            LONG qf = InterlockedIncrement(&g_queue_full_drops);
            PIRP oldIrp = g_irp_queue.Irps[g_irp_queue.Head];
            g_irp_queue.Head = (g_irp_queue.Head + 1) % IRP_QUEUE_SIZE;
            g_irp_queue.Count--;
            KeReleaseSpinLock(&g_irp_queue.Lock, oldIrql);
            if (qf <= 5 || (qf % 100) == 0) {
                char buf[32] = "IRP queue full #";
                hex32(buf + 16, (ULONG)qf);
                buf[24] = 0;
                drv_log(buf);
            }
            if (rx_claim_irp(oldIrp)) {
                oldIrp->IoStatus.Status = STATUS_CANCELLED;
                oldIrp->IoStatus.Information = 0;
                IoCompleteRequest(oldIrp, IO_NO_INCREMENT);
            }
            KeAcquireSpinLock(&g_irp_queue.Lock, &oldIrql);
        }
        {
            PIO_STACK_LOCATION sp = IoGetCurrentIrpStackLocation(Irp);
            g_irp_queue.FileObjs[g_irp_queue.Tail] = sp->FileObject;
        }
        g_irp_queue.Irps[g_irp_queue.Tail] = Irp;
        g_irp_queue.Tail = (g_irp_queue.Tail + 1) % IRP_QUEUE_SIZE;
        g_irp_queue.Count++;

        
        IoSetCancelRoutine(Irp, RvpnCancelRoutine);
        if (Irp->Cancel && IoSetCancelRoutine(Irp, NULL) != NULL) {
            dequeue_specific_irp_locked(Irp);
            KeReleaseSpinLock(&g_irp_queue.Lock, oldIrql);
            Irp->IoStatus.Status = STATUS_CANCELLED;
            Irp->IoStatus.Information = 0;
            IoCompleteRequest(Irp, IO_NO_INCREMENT);
            return STATUS_PENDING;
        }
        KeReleaseSpinLock(&g_irp_queue.Lock, oldIrql);
    }

    static LONG pend_count = 0;
    LONG pc = InterlockedIncrement(&pend_count);
    if (pc <= 5 || (pc % 500) == 0) {
        char buf[48] = "READ pend #";
        hex32(buf + 11, (ULONG)pc);
        drv_log(buf);
    }

    return STATUS_PENDING;
}


static NTSTATUS NTAPI DispatchWrite(PDEVICE_OBJECT DeviceObject, PIRP Irp)
{
    PDEVICE_EXTENSION ext = (PDEVICE_EXTENSION)DeviceObject->DeviceExtension;
    PIO_STACK_LOCATION irpSp = IoGetCurrentIrpStackLocation(Irp);
    ULONG length = irpSp->Parameters.Write.Length;
    IO_STATUS_BLOCK iosb;

    if ((!ext->FifoD2B_High && !ext->FifoD2B_Low) || length == 0) {
        Irp->IoStatus.Status = STATUS_SUCCESS;
        Irp->IoStatus.Information = length;
        IoCompleteRequest(Irp, IO_NO_INCREMENT);
        return STATUS_SUCCESS;
    }

    PUCHAR inBuf = NULL;
    if (Irp->MdlAddress)
        inBuf = (PUCHAR)MmGetSystemAddressForMdlSafe(Irp->MdlAddress, NormalPagePriority);

    if (!inBuf) {
        Irp->IoStatus.Status = STATUS_SUCCESS;
        Irp->IoStatus.Information = length;
        IoCompleteRequest(Irp, IO_NO_INCREMENT);
        return STATUS_SUCCESS;
    }

    
    ULONG offset = 0;
    ULONG mode = ext->SetupMode;
    LONG frameCount = 0;

    while (offset < length) {
        
        if (mode >= 2) {
            if (offset + 4 > length) break;
            
            if (frameCount < 3) {
                char buf[64] = "WR mac=";
                static const char hx3[] = "0123456789abcdef";
                int j;
                for (j = 0; j < 4; j++) {
                    buf[7 + j*2] = hx3[(inBuf[offset+j] >> 4) & 0xf];
                    buf[8 + j*2] = hx3[inBuf[offset+j] & 0xf];
                }
                buf[15] = 0;
                drv_log(buf);
            }
            offset += 4;
        }

        
        if (offset + 4 > length) break;
        ULONG frameLen = *((ULONG *)(inBuf + offset));
        offset += 4;

        
        if (frameLen == 0 || frameLen > RX_FRAME_MAX || offset + frameLen > length)
            break;

        
        if (is_ipv4_icmp(inBuf + offset, (USHORT)frameLen)) {
            LONG n = InterlockedIncrement(&g_icmp_tx);
            if (n <= 3 || (n % 100) == 0) {
                char buf[64] = "ICMP-IN  #";
                hex32(buf + 10, (ULONG)n);
                buf[18] = ' '; buf[19] = 'l'; buf[20] = '=';
                hex32(buf + 21, (ULONG)frameLen);
                buf[29] = 0;
                drv_log(buf);
            }
        }

        
        USHORT fifoLen = (USHORT)frameLen;
        HANDLE fifo = classify_high_priority(inBuf + offset, (USHORT)frameLen)
                       ? ext->FifoD2B_High
                       : ext->FifoD2B_Low;
        if (fifo) {
            UCHAR combined[2 + RX_FRAME_MAX];
            combined[0] = (UCHAR)(fifoLen & 0xFF);
            combined[1] = (UCHAR)(fifoLen >> 8);
            RtlCopyMemory(combined + 2, inBuf + offset, frameLen);
            ZwWriteFile(fifo, NULL, NULL, NULL, &iosb,
                        combined, 2 + frameLen, NULL, NULL);
        }

        offset += frameLen;
        frameCount++;
    }

    static LONG write_ok = 0;
    LONG wc = InterlockedIncrement(&write_ok);
    if (wc <= 10 || (wc % 100) == 0) {
        char buf[80] = "WRITE tlv frames=";
        hex32(buf + 17, (ULONG)frameCount);
        char *p = buf + 25;
        *p++ = ' '; *p++ = 'r'; *p++ = 'a'; *p++ = 'w'; *p++ = '=';
        hex32(p, length);
        drv_log(buf);
    }

    Irp->IoStatus.Status = STATUS_SUCCESS;
    Irp->IoStatus.Information = length;
    IoCompleteRequest(Irp, IO_NO_INCREMENT);
    return STATUS_SUCCESS;
}



static VOID NTAPI Unload(PDRIVER_OBJECT DriverObject)
{
    UNICODE_STRING dosDeviceName;
    RtlInitUnicodeString(&dosDeviceName, DOSDEVICE_NAME);
    IoDeleteSymbolicLink(&dosDeviceName);

    
    {
        KIRQL oldIrql;
        KeAcquireSpinLock(&g_irp_queue.Lock, &oldIrql);
        while (g_irp_queue.Count > 0) {
            PIRP irp = g_irp_queue.Irps[g_irp_queue.Head];
            g_irp_queue.Head = (g_irp_queue.Head + 1) % IRP_QUEUE_SIZE;
            g_irp_queue.Count--;
            KeReleaseSpinLock(&g_irp_queue.Lock, oldIrql);
            if (rx_claim_irp(irp)) {
                irp->IoStatus.Status = STATUS_CANCELLED;
                irp->IoStatus.Information = 0;
                IoCompleteRequest(irp, IO_NO_INCREMENT);
            }
            KeAcquireSpinLock(&g_irp_queue.Lock, &oldIrql);
        }
        KeReleaseSpinLock(&g_irp_queue.Lock, oldIrql);
    }

    if (DriverObject->DeviceObject) {
        PDEVICE_EXTENSION ext = (PDEVICE_EXTENSION)DriverObject->DeviceObject->DeviceExtension;
        if (ext->FifoB2D)     ZwClose(ext->FifoB2D);
        if (ext->FifoD2B)     ZwClose(ext->FifoD2B);
        if (ext->FifoD2B_High) ZwClose(ext->FifoD2B_High);
        if (ext->FifoD2B_Low)  ZwClose(ext->FifoD2B_Low);
        IoDeleteDevice(DriverObject->DeviceObject);
    }

    if (g_peer_routes) {
        ExFreePoolWithTag(g_peer_routes, PEER_ROUTES_TAG);
        g_peer_routes = NULL;
        g_peer_routes_capacity = 0;
    }

    
    {
        KIRQL oldIrql;
        KeAcquireSpinLock(&g_log_lock, &oldIrql);
        drv_log_flush_locked();
        KeReleaseSpinLock(&g_log_lock, oldIrql);
    }
    if (g_log_file) {
        ZwClose(g_log_file);
        g_log_file = NULL;
    }
}

NTSTATUS NTAPI DriverEntry(PDRIVER_OBJECT DriverObject, PUNICODE_STRING RegistryPath)
{
    NTSTATUS status;
    PDEVICE_OBJECT deviceObject = NULL;
    UNICODE_STRING deviceName, dosDeviceName;

    (void)RegistryPath;

    RtlInitUnicodeString(&deviceName, DEVICE_NAME);
    RtlInitUnicodeString(&dosDeviceName, DOSDEVICE_NAME);

    status = IoCreateDevice(
        DriverObject, sizeof(DEVICE_EXTENSION), &deviceName,
        FILE_DEVICE_UNKNOWN, 0, FALSE, &deviceObject);

    if (!NT_SUCCESS(status)) return status;

    status = IoCreateSymbolicLink(&dosDeviceName, &deviceName);
    if (!NT_SUCCESS(status)) {
        IoDeleteDevice(deviceObject);
        return status;
    }

    deviceObject->Flags |= DO_DIRECT_IO;
    deviceObject->Flags &= ~DO_DEVICE_INITIALIZING;

    RtlZeroMemory(deviceObject->DeviceExtension, sizeof(DEVICE_EXTENSION));
    RtlZeroMemory(&g_rx_ring, sizeof(g_rx_ring));
    g_DeviceObject = deviceObject;
    KeInitializeSpinLock(&g_irp_queue.Lock);
    KeInitializeSpinLock(&g_log_lock);

    
    {
        UNICODE_STRING fileName;
        
        RtlInitUnicodeString(&fileName, L"\\??\\unix\\tmp\\radmin_driver.log");
        OBJECT_ATTRIBUTES oa;
        InitializeObjectAttributes(&oa, &fileName, OBJ_CASE_INSENSITIVE, NULL, NULL);
        IO_STATUS_BLOCK iosb;
        
        ZwCreateFile(&g_log_file, FILE_APPEND_DATA | SYNCHRONIZE, &oa, &iosb,
                     NULL, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ | FILE_SHARE_WRITE,
                     FILE_OPEN_IF, FILE_SYNCHRONOUS_IO_NONALERT, NULL, 0);
    }

    
    g_peer_routes = (PEER_ROUTE *)ExAllocatePoolWithTag(
        NonPagedPool, sizeof(PEER_ROUTE) * PEER_ROUTES_INITIAL, PEER_ROUTES_TAG);
    if (!g_peer_routes) {
        IoDeleteSymbolicLink(&dosDeviceName);
        IoDeleteDevice(deviceObject);
        return STATUS_INSUFFICIENT_RESOURCES;
    }
    RtlZeroMemory(g_peer_routes, sizeof(PEER_ROUTE) * PEER_ROUTES_INITIAL);
    g_peer_routes_capacity = PEER_ROUTES_INITIAL;

    DriverObject->MajorFunction[IRP_MJ_CREATE]         = DispatchCreate;
    DriverObject->MajorFunction[IRP_MJ_CLOSE]          = DispatchClose;
    DriverObject->MajorFunction[IRP_MJ_CLEANUP]        = DispatchCleanup;
    DriverObject->MajorFunction[IRP_MJ_DEVICE_CONTROL] = DispatchDeviceControl;
    DriverObject->MajorFunction[IRP_MJ_READ]            = DispatchRead;
    DriverObject->MajorFunction[IRP_MJ_WRITE]           = DispatchWrite;
    DriverObject->DriverUnload = Unload;

    drv_log("Driver loaded");
    return STATUS_SUCCESS;
}
