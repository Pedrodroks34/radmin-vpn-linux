

#include <winsock2.h>
#include <ws2tcpip.h>
#include <windows.h>
#include <iphlpapi.h>
#include <string.h>
#include <stdio.h>
#include <stdint.h>
#include <stdlib.h>
#include <ctype.h>

#define TAP_DESC     L"radminvpn0"
#define RADMIN_DESC  L"Famatech Radmin VPN Ethernet Adapter"
#define RADMIN_FRIENDLY L"Radmin VPN"



static void dbg(const char *msg)
{
    HANDLE f = CreateFileA("C:\\radmin_hook_debug.log",
        FILE_APPEND_DATA, FILE_SHARE_READ | FILE_SHARE_WRITE, NULL,
        OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL, NULL);
    if (f == INVALID_HANDLE_VALUE)
        return;
    {
        DWORD w;
        WriteFile(f, msg, strlen(msg), &w, NULL);
        WriteFile(f, "\r\n", 2, &w, NULL);
        CloseHandle(f);
    }
}



#define CRASH_LOG "C:\\radmin_crash.log"

static void crashlog(const char *msg)
{
    HANDLE f = CreateFileA(CRASH_LOG, FILE_APPEND_DATA,
        FILE_SHARE_READ | FILE_SHARE_WRITE, NULL,
        OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL, NULL);
    if (f == INVALID_HANDLE_VALUE)
        return;
    {
        DWORD w;
        WriteFile(f, msg, (DWORD)strlen(msg), &w, NULL);
        CloseHandle(f);
    }
}


static USHORT (WINAPI *p_CaptureStack)(ULONG, ULONG, PVOID *, PULONG) = NULL;


static void module_at(void *addr, char *out, size_t outsz)
{
    HMODULE hm = NULL;
    if (GetModuleHandleExA(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS |
            GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            (LPCSTR)addr, &hm) && hm) {
        char path[MAX_PATH];
        if (GetModuleFileNameA(hm, path, MAX_PATH)) {
            const char *base = strrchr(path, '\\');
            base = base ? base + 1 : path;
            snprintf(out, outsz, "%s+0x%lx", base,
                     (unsigned long)((uintptr_t)addr - (uintptr_t)hm));
            return;
        }
    }
    snprintf(out, outsz, "%p", addr);
}

static volatile LONG in_crash = 0;


static DWORD g_jmp_tls = TLS_OUT_OF_INDEXES;
static void longjmp_tramp(void);   

static LONG CALLBACK crash_handler(PEXCEPTION_POINTERS ep)
{
    DWORD code = ep->ExceptionRecord->ExceptionCode;

    
    if ((code & 0xC0000000) != 0xC0000000)
        return EXCEPTION_CONTINUE_SEARCH;
    if (code == 0xE06D7363u)
        return EXCEPTION_CONTINUE_SEARCH;

    
    if (code == 0xC0000005u && g_jmp_tls != TLS_OUT_OF_INDEXES &&
        TlsGetValue(g_jmp_tls)) {
        crashlog("[task-guard] recovered from fault in worker task "
                 "(task dropped, service alive)\r\n");
        ep->ContextRecord->Eip = (DWORD)(ULONG_PTR)&longjmp_tramp;
        return EXCEPTION_CONTINUE_EXECUTION;
    }

    
    if (InterlockedExchange(&in_crash, 1) != 0)
        return EXCEPTION_CONTINUE_SEARCH;

    {
        char buf[1024];
        char modbuf[MAX_PATH + 32];
        CONTEXT *c = ep->ContextRecord;
        void *pc = ep->ExceptionRecord->ExceptionAddress;
        SYSTEMTIME st;

        module_at(pc, modbuf, sizeof(modbuf));
        GetLocalTime(&st);
        snprintf(buf, sizeof(buf),
            "\r\n===== CRASH %04d-%02d-%02d %02d:%02d:%02d.%03d =====\r\n"
            "code=0x%08lx  pc=%p  (%s)\r\n",
            st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute,
            st.wSecond, st.wMilliseconds,
            (unsigned long)code, pc, modbuf);
        crashlog(buf);

        if (code == 0xC0000005u &&
            ep->ExceptionRecord->NumberParameters >= 2) {
            ULONG_PTR kind = ep->ExceptionRecord->ExceptionInformation[0];
            const char *op = kind == 0 ? "read" : kind == 1 ? "write" : "exec";
            snprintf(buf, sizeof(buf), "access-violation: %s @ %p\r\n",
                     op, (void *)ep->ExceptionRecord->ExceptionInformation[1]);
            crashlog(buf);
        }

#ifdef __i386__
        snprintf(buf, sizeof(buf),
            "EAX=%08lx EBX=%08lx ECX=%08lx EDX=%08lx\r\n"
            "ESI=%08lx EDI=%08lx EBP=%08lx ESP=%08lx\r\n"
            "EIP=%08lx EFL=%08lx\r\n",
            c->Eax, c->Ebx, c->Ecx, c->Edx,
            c->Esi, c->Edi, c->Ebp, c->Esp,
            c->Eip, c->EFlags);
        crashlog(buf);
#else
        (void)c;
#endif

        if (!p_CaptureStack) {
            HMODULE nt = GetModuleHandleA("ntdll.dll");
            if (nt)
                p_CaptureStack = (void *)GetProcAddress(nt,
                    "RtlCaptureStackBackTrace");
        }
        if (p_CaptureStack) {
            void *frames[48];
            USHORT i, n = p_CaptureStack(0, 48, frames, NULL);
            crashlog("backtrace (innermost first):\r\n");
            for (i = 0; i < n; i++) {
                module_at(frames[i], modbuf, sizeof(modbuf));
                snprintf(buf, sizeof(buf), "  #%02u  %p  %s\r\n",
                         i, frames[i], modbuf);
                crashlog(buf);
            }
        }
        crashlog("===== END CRASH =====\r\n");
    }

    in_crash = 0;
    
    return EXCEPTION_CONTINUE_SEARCH;
}



#define DISPATCH_RVA 0x788C0        
#define FREE_RVA     0xE5807        
#define COUNTER_RVA  0x15C778       

static BYTE *g_rvbase = NULL;       


static void longjmp_tramp(void)
{
    if (g_jmp_tls != TLS_OUT_OF_INDEXES) {
        void **e = (void **)TlsGetValue(g_jmp_tls);
        if (e)
            __builtin_longjmp(e, 1);            
    }
    for (;;)                                     
        Sleep(1000);
}


static unsigned __stdcall hook_dispatch(void **Block)
{
    if (Block) {
        void *fn  = Block[0];
        void *arg = Block[1];
        
        ((void (__cdecl *)(void *, unsigned))(g_rvbase + FREE_RVA))(Block, 8);
        if (fn) {
            void *env[5];                        
            void **prev;
            if (g_jmp_tls != TLS_OUT_OF_INDEXES) {
                prev = (void **)TlsGetValue(g_jmp_tls);
                TlsSetValue(g_jmp_tls, env);
                if (__builtin_setjmp(env) == 0)
                    ((void (__stdcall *)(void *))fn)(arg);  
                TlsSetValue(g_jmp_tls, prev);
            } else {
                ((void (__stdcall *)(void *))fn)(arg);
            }
        }
        
        _InterlockedDecrement((long *)(g_rvbase + COUNTER_RVA));
    }
    return 0;
}

#define GETINETWORK_RVA 0x67D00     

static void install_release_guard(void)
{
    
    static const BYTE expect[5] = { 0x55, 0x8B, 0xEC, 0x6A, 0xFF };
    HMODULE base = GetModuleHandleA(NULL);
    BYTE *t;
    DWORD old;

    if (!base) {
        dbg("task-guard: no module base");
        return;
    }
    g_rvbase = (BYTE *)base;

    
    {
        static const BYTE gi_expect[3] = { 0x56, 0x57, 0x8B };  
        BYTE *g = (BYTE *)base + GETINETWORK_RVA;
        DWORD go;
        if (memcmp(g, gi_expect, sizeof(gi_expect)) != 0) {
            dbg("GetINetwork stub: prologue mismatch, skip");
        } else if (!VirtualProtect(g, 3, PAGE_EXECUTE_READWRITE, &go)) {
            dbg("GetINetwork stub: VirtualProtect failed");
        } else {
            g[0] = 0x30; g[1] = 0xC0;            
            g[2] = 0xC3;                         
            VirtualProtect(g, 3, go, &go);
            FlushInstructionCache(GetCurrentProcess(), g, 3);
            dbg("GetINetwork stubbed at RvControlSvc+0x67D00 (always return 0)");
        }
    }

    
    t = (BYTE *)base + DISPATCH_RVA;
    if (memcmp(t, expect, sizeof(expect)) != 0) {
        dbg("task-guard: prologue mismatch -- wrong RvControlSvc, skip");
        return;
    }
    if (!VirtualProtect(t, 6, PAGE_EXECUTE_READWRITE, &old)) {
        dbg("task-guard: VirtualProtect failed");
        return;
    }
    
    t[0] = 0x68;
    *(DWORD *)(t + 1) = (DWORD)(ULONG_PTR)hook_dispatch;
    t[5] = 0xC3;
    VirtualProtect(t, 6, old, &old);
    FlushInstructionCache(GetCurrentProcess(), t, 6);
    dbg("task-guard installed at RvControlSvc+0x788C0");
}



static ULONG (WINAPI *real_GetAdaptersAddresses)(
    ULONG, ULONG, PVOID, PIP_ADAPTER_ADDRESSES, PULONG) = NULL;
static ULONG (WINAPI *real_GetAdaptersInfo)(PIP_ADAPTER_INFO, PULONG) = NULL;

static DWORD g_tap_ifindex = 0;   


static BOOL is_uplink_v4(DWORD s_addr_net)
{
    DWORD h = ntohl(s_addr_net);
    if (h == 0) return FALSE;
    if ((h & 0xFF000000u) == 0x7F000000u) return FALSE;   
    if ((h & 0xFFFF0000u) == 0xA9FE0000u) return FALSE;   
    if ((h & 0xFFC00000u) == 0x64400000u) return FALSE;   
    return TRUE;
}


static DWORD default_route_ifindex(void)
{
    if (!real_GetAdaptersAddresses) {
        
        DWORD idx = 0;
        if (GetBestInterface(inet_addr("8.8.8.8"), &idx) == NO_ERROR && idx)
            return idx;
        return 0;
    }

    ULONG size = 15 * 1024;
    IP_ADAPTER_ADDRESSES *buf = (IP_ADAPTER_ADDRESSES *)malloc(size);
    if (!buf) return 0;
    ULONG r = real_GetAdaptersAddresses(AF_INET, GAA_FLAG_INCLUDE_GATEWAYS,
                                        NULL, buf, &size);
    if (r == ERROR_BUFFER_OVERFLOW) {
        IP_ADAPTER_ADDRESSES *nb = (IP_ADAPTER_ADDRESSES *)realloc(buf, size);
        if (!nb) { free(buf); return 0; }
        buf = nb;
        r = real_GetAdaptersAddresses(AF_INET, GAA_FLAG_INCLUDE_GATEWAYS,
                                      NULL, buf, &size);
    }
    if (r != ERROR_SUCCESS) { free(buf); return 0; }

    
    DWORD best = 0;
    if (GetBestInterface(inet_addr("8.8.8.8"), &best) == NO_ERROR && best) {
        for (IP_ADAPTER_ADDRESSES *a = buf; a; a = a->Next) {
            if (a->IfIndex == best) {
                free(buf);
                dbg("uplink: GetBestInterface fast-path");
                return best;
            }
        }
    }

    
    DWORD found = 0;
    int n = 0;
    for (IP_ADAPTER_ADDRESSES *a = buf; a; a = a->Next) {
        if (a->OperStatus != IfOperStatusUp) continue;
        if (!a->FirstGatewayAddress) continue;
        BOOL has_v4 = FALSE;
        for (PIP_ADAPTER_UNICAST_ADDRESS u = a->FirstUnicastAddress; u; u = u->Next) {
            if (u->Address.lpSockaddr &&
                u->Address.lpSockaddr->sa_family == AF_INET &&
                is_uplink_v4(((struct sockaddr_in *)
                              u->Address.lpSockaddr)->sin_addr.s_addr)) {
                has_v4 = TRUE;
                break;
            }
        }
        if (!has_v4) continue;
        n++;
        found = a->IfIndex;
    }
    free(buf);

    if (n == 1) {
        dbg("uplink: gateway fallback (GetBestInterface unusable)");
        return found;
    }
    dbg(n == 0 ? "uplink: unresolved (no gateway+routable adapter) -> 0"
               : "uplink: ambiguous (>1 gateway adapter) -> 0");
    return 0;
}

static ULONG WINAPI hook_GetAdaptersAddresses(
    ULONG Family, ULONG Flags, PVOID Rsvd,
    PIP_ADAPTER_ADDRESSES Addrs, PULONG Size)
{
    if (!real_GetAdaptersAddresses) return ERROR_NOT_SUPPORTED;

    ULONG ret = real_GetAdaptersAddresses(Family, Flags, Rsvd, Addrs, Size);
    if (ret != ERROR_SUCCESS || !Addrs) return ret;

    DWORD best = default_route_ifindex();
    BOOL  best_ok = FALSE;

    
    for (PIP_ADAPTER_ADDRESSES cur = Addrs; cur; cur = cur->Next) {
        if (cur->Description && wcscmp(cur->Description, TAP_DESC) == 0) {
            
            cur->Description  = (WCHAR *)RADMIN_DESC;
            cur->FriendlyName = (WCHAR *)RADMIN_FRIENDLY;
            g_tap_ifindex = cur->IfIndex;
            continue;
        }
        if (best && cur->IfIndex == best)
            best_ok = TRUE;
    }

    if (!best || !best_ok) {
        dbg("filter(GAA): no default-route adapter resolved -- rename only");
        return ret;
    }

    
    int hidden = 0;
    for (PIP_ADAPTER_ADDRESSES cur = Addrs; cur; cur = cur->Next) {
        if (cur->IfIndex == best || cur->IfIndex == g_tap_ifindex)
            continue;
        cur->FirstUnicastAddress = NULL;
        hidden++;
    }
    {
        char buf[96];
        snprintf(buf, sizeof(buf),
            "filter(GAA): default-route if=%lu, TAP if=%lu, hid %d adapters",
            (unsigned long)best, (unsigned long)g_tap_ifindex, hidden);
        dbg(buf);
    }
    return ret;
}

static ULONG WINAPI hook_GetAdaptersInfo(PIP_ADAPTER_INFO Info, PULONG Size)
{
    if (!real_GetAdaptersInfo) return ERROR_NOT_SUPPORTED;

    ULONG ret = real_GetAdaptersInfo(Info, Size);
    if (ret != ERROR_SUCCESS || !Info) return ret;

    DWORD best = default_route_ifindex();
    BOOL  best_ok = FALSE;
    for (PIP_ADAPTER_INFO cur = Info; cur; cur = cur->Next)
        if (best && cur->Index == best) { best_ok = TRUE; break; }

    if (!best || !best_ok) {
        dbg("filter(GAI): no default-route adapter resolved -- skip");
        return ret;
    }

    int hidden = 0;
    for (PIP_ADAPTER_INFO cur = Info; cur; cur = cur->Next) {
        if (cur->Index == best || (g_tap_ifindex && cur->Index == g_tap_ifindex))
            continue;
        
        strcpy(cur->IpAddressList.IpAddress.String, "0.0.0.0");
        cur->IpAddressList.Next = NULL;
        hidden++;
    }
    {
        char buf[64];
        snprintf(buf, sizeof(buf), "filter(GAI): hid %d adapters", hidden);
        dbg(buf);
    }
    return ret;
}



#define WS2_ORD_GETHOSTBYNAME 52          
#define ALLOWED_MAX           32

static DWORD g_allowed_v4[ALLOWED_MAX];   
static int   g_allowed_n     = 0;
static BOOL  g_allowed_valid = FALSE;

static struct hostent *(WINAPI *real_gethostbyname)(const char *) = NULL;
static int (WINAPI *real_getaddrinfo)(const char *, const char *,
    const struct addrinfo *, struct addrinfo **) = NULL;
static int (WSAAPI *real_getnameinfo)(const SOCKADDR *, socklen_t,
    PCHAR, DWORD, PCHAR, DWORD, INT) = NULL;


static BOOL is_cgnat(DWORD s_addr_net)
{
    return (ntohl(s_addr_net) & 0xFFC00000u) == 0x64400000u;
}

static BOOL in_allowed_v4(DWORD s_addr_net)
{
    for (int i = 0; i < g_allowed_n; i++)
        if (g_allowed_v4[i] == s_addr_net) return TRUE;
    return FALSE;
}


static void build_allowed_v4(void)
{
    g_allowed_valid = FALSE;
    g_allowed_n = 0;

    if (!real_GetAdaptersAddresses) return;
    DWORD uplink = default_route_ifindex();
    if (!uplink) return;

    ULONG size = 15 * 1024;
    IP_ADAPTER_ADDRESSES *buf = (IP_ADAPTER_ADDRESSES *)malloc(size);
    if (!buf) return;
    ULONG r = real_GetAdaptersAddresses(AF_INET, 0, NULL, buf, &size);
    if (r == ERROR_BUFFER_OVERFLOW) {
        IP_ADAPTER_ADDRESSES *nb = (IP_ADAPTER_ADDRESSES *)realloc(buf, size);
        if (!nb) { free(buf); return; }
        buf = nb;
        r = real_GetAdaptersAddresses(AF_INET, 0, NULL, buf, &size);
    }
    if (r != ERROR_SUCCESS) { free(buf); return; }

    BOOL uplink_has_v4 = FALSE;
    for (IP_ADAPTER_ADDRESSES *a = buf; a; a = a->Next) {
        if (a->IfIndex != uplink && a->IfIndex != g_tap_ifindex) continue;
        for (PIP_ADAPTER_UNICAST_ADDRESS u = a->FirstUnicastAddress; u; u = u->Next) {
            if (!u->Address.lpSockaddr ||
                u->Address.lpSockaddr->sa_family != AF_INET) continue;
            DWORD ip = ((struct sockaddr_in *)u->Address.lpSockaddr)->sin_addr.s_addr;
            if (a->IfIndex == uplink) {
                uplink_has_v4 = TRUE;
                if (is_cgnat(ip)) { free(buf); return; }   
            }
            if (g_allowed_n < ALLOWED_MAX) g_allowed_v4[g_allowed_n++] = ip;
        }
    }
    free(buf);
    if (!uplink_has_v4) return;
    g_allowed_valid = TRUE;
}


static void norm_host(char *dst, size_t dstsz, const char *src)
{
    size_t i = 0;
    for (; src[i] && i + 1 < dstsz; i++)
        dst[i] = (char)tolower((unsigned char)src[i]);
    while (i && dst[i - 1] == '.') i--;
    dst[i] = 0;
}


static BOOL is_local_hostname(const char *name)
{
    if (!name || !*name) return FALSE;
    char host[256];
    if (gethostname(host, sizeof host) != 0) return FALSE;
    char a[256], b[256];
    norm_host(a, sizeof a, name);
    norm_host(b, sizeof b, host);
    if (_stricmp(a, b) == 0) return TRUE;
    char *da = strchr(a, '.'); if (da) *da = 0;    
    char *db = strchr(b, '.'); if (db) *db = 0;
    return _stricmp(a, b) == 0;
}


static void log_ghbn(const char *name, BOOL local, struct hostent *h)
{
    char b[512];
    int n = snprintf(b, sizeof b, "gethostbyname('%s') local=%d ->",
                     name ? name : "(null)", local ? 1 : 0);
    if (h && h->h_addrtype == AF_INET && h->h_addr_list && h->h_addr_list[0]) {
        for (char **rd = h->h_addr_list; *rd && n < (int)sizeof b - 20; rd++) {
            struct in_addr a; a.s_addr = *(DWORD *)*rd;
            n += snprintf(b + n, sizeof b - n, " %s", inet_ntoa(a));
        }
    } else {
        snprintf(b + n, sizeof b - n, " (no A records)");
    }
    dbg(b);
}

static struct hostent *WINAPI hook_gethostbyname(const char *name)
{
    struct hostent *h = real_gethostbyname ? real_gethostbyname(name) : NULL;
    if (!h || h->h_addrtype != AF_INET || !h->h_addr_list) return h;

    BOOL local = is_local_hostname(name);
    log_ghbn(name, local, h);
    if (!local) return h;                       

    
    build_allowed_v4();
    if (!g_allowed_valid) {
        h->h_addr_list[0] = NULL;
        dbg("filter(gethostbyname): allow-set unavailable -> emptied local candidates");
        return h;
    }

    char **rd = h->h_addr_list, **wr = h->h_addr_list;
    int kept = 0, dropped = 0;
    for (; *rd; rd++) {
        if (in_allowed_v4(*(DWORD *)*rd)) { *wr++ = *rd; kept++; }
        else dropped++;
    }
    *wr = NULL;
    {
        char b[96];
        snprintf(b, sizeof b, "filter(gethostbyname): kept %d, dropped %d (allow-set=%d)",
                 kept, dropped, g_allowed_n);
        dbg(b);
    }
    return h;
}

static int WINAPI hook_getaddrinfo(const char *node, const char *service,
    const struct addrinfo *hints, struct addrinfo **res)
{
    int rc = real_getaddrinfo ? real_getaddrinfo(node, service, hints, res)
                              : EAI_FAIL;
    if (rc != 0 || !res || !*res) return rc;
    BOOL local = is_local_hostname(node);
    {
        char b[160];
        snprintf(b, sizeof b, "getaddrinfo('%s') local=%d", node ? node : "(null)", local ? 1 : 0);
        dbg(b);
    }
    if (!local) return rc;                          
    build_allowed_v4();
    if (!g_allowed_valid) return rc;

    
    int keep = 0;
    for (struct addrinfo *c = *res; c; c = c->ai_next)
        if (c->ai_family == AF_INET && c->ai_addr &&
            in_allowed_v4(((struct sockaddr_in *)c->ai_addr)->sin_addr.s_addr))
            keep++;
    if (keep == 0) return rc;

    
    struct addrinfo *cur = *res, *prev = NULL;
    int dropped = 0;
    while (cur) {
        BOOL ok = (cur->ai_family == AF_INET && cur->ai_addr &&
                   in_allowed_v4(((struct sockaddr_in *)cur->ai_addr)->sin_addr.s_addr));
        struct addrinfo *next = cur->ai_next;
        if (ok) {
            prev = cur;
        } else {
            if (prev) prev->ai_next = next; else *res = next;
            cur->ai_next = NULL;                     
            dropped++;
        }
        cur = next;
    }
    if (dropped) {
        char b[96];
        snprintf(b, sizeof b, "filter(getaddrinfo): kept %d, dropped %d local addr(s)",
                 keep, dropped);
        dbg(b);
    }
    return rc;
}


static BOOL is_private_v4(DWORD s_addr_net)
{
    DWORD h = ntohl(s_addr_net);
    if ((h & 0xFF000000u) == 0x7F000000u) return TRUE;   
    if ((h & 0xFF000000u) == 0x0A000000u) return TRUE;   
    if ((h & 0xFFF00000u) == 0xAC100000u) return TRUE;   
    if ((h & 0xFFFF0000u) == 0xC0A80000u) return TRUE;   
    if ((h & 0xFFFF0000u) == 0xA9FE0000u) return TRUE;   
    if ((h & 0xFFC00000u) == 0x64400000u) return TRUE;   
    return FALSE;
}

static int WSAAPI hook_getnameinfo(const SOCKADDR *sa, socklen_t salen,
    PCHAR host, DWORD hostlen, PCHAR serv, DWORD servlen, INT flags)
{
    if (sa && sa->sa_family == AF_INET &&
        salen >= (socklen_t)sizeof(struct sockaddr_in)) {
        const struct sockaddr_in *si = (const struct sockaddr_in *)sa;
        if (is_private_v4(si->sin_addr.s_addr)) {
            char ip[16];
            lstrcpynA(ip, inet_ntoa(si->sin_addr), sizeof ip);
            if (host && hostlen) lstrcpynA(host, ip, hostlen);
            if (serv && servlen) {
                char pb[8];
                snprintf(pb, sizeof pb, "%u", (unsigned)ntohs(si->sin_port));
                lstrcpynA(serv, pb, servlen);
            }
            { char b[64];
              snprintf(b, sizeof b, "getnameinfo: short-circuit PTR %s", ip);
              dbg(b); }
            return 0;
        }
        
        { char b[64];
          snprintf(b, sizeof b, "getnameinfo: passthrough %s",
                   inet_ntoa(si->sin_addr));
          dbg(b); }
    }
    return real_getnameinfo
        ? real_getnameinfo(sa, salen, host, hostlen, serv, servlen, flags)
        : EAI_FAIL;
}



typedef int (WSAAPI *gni_a_t)(const SOCKADDR *, socklen_t,
    PCHAR, DWORD, PCHAR, DWORD, INT);
typedef int (WSAAPI *gni_w_t)(const SOCKADDR *, socklen_t,
    PWCHAR, DWORD, PWCHAR, DWORD, INT);

static gni_w_t real_GetNameInfoW = NULL;

static int WSAAPI hook_GetNameInfoW(const SOCKADDR *sa, socklen_t salen,
    PWCHAR host, DWORD hostlen, PWCHAR serv, DWORD servlen, INT flags)
{
    if (sa && sa->sa_family == AF_INET &&
        salen >= (socklen_t)sizeof(struct sockaddr_in)) {
        const struct sockaddr_in *si = (const struct sockaddr_in *)sa;
        if (is_private_v4(si->sin_addr.s_addr)) {
            char ip[16], pb[8];
            lstrcpynA(ip, inet_ntoa(si->sin_addr), sizeof ip);
            snprintf(pb, sizeof pb, "%u", (unsigned)ntohs(si->sin_port));
            if (host && hostlen)
                MultiByteToWideChar(CP_ACP, 0, ip, -1, host, hostlen);
            if (serv && servlen)
                MultiByteToWideChar(CP_ACP, 0, pb, -1, serv, servlen);
            { char b[80];
              snprintf(b, sizeof b, "GetNameInfoW: short-circuit PTR %s", ip);
              dbg(b); }
            return 0;
        }
        { char b[64];
          snprintf(b, sizeof b, "GetNameInfoW: passthrough %s",
                   inet_ntoa(si->sin_addr));
          dbg(b); }
    }
    return real_GetNameInfoW
        ? real_GetNameInfoW(sa, salen, host, hostlen, serv, servlen, flags)
        : EAI_FAIL;
}



static ULONG WINAPI perf_stub_ul(HANDLE prov, void *inst, ULONG id, ULONG val)
{
    (void)prov; (void)inst; (void)id; (void)val;
    return ERROR_SUCCESS;
}

static ULONG WINAPI perf_stub_ull(HANDLE prov, void *inst, ULONG id,
                                  ULONGLONG val)
{
    (void)prov; (void)inst; (void)id; (void)val;
    return ERROR_SUCCESS;
}


static FARPROC perf_fill_in(const char *name)
{
    if (strcmp(name, "PerfIncrementULongCounterValue") == 0 ||
        strcmp(name, "PerfDecrementULongCounterValue") == 0)
        return (FARPROC)perf_stub_ul;
    if (strcmp(name, "PerfIncrementULongLongCounterValue") == 0 ||
        strcmp(name, "PerfDecrementULongLongCounterValue") == 0)
        return (FARPROC)perf_stub_ull;
    return NULL;
}

static FARPROC (WINAPI *real_GetProcAddress)(HMODULE, LPCSTR) = NULL;

static FARPROC WINAPI hook_GetProcAddress(HMODULE mod, LPCSTR name)
{
    FARPROC p = real_GetProcAddress ? real_GetProcAddress(mod, name) : NULL;

    
    if (!name || ((DWORD_PTR)name >> 16) == 0)
        return p;

    
    if (!p && strncmp(name, "Perf", 4) == 0) {
        FARPROC stub = perf_fill_in(name);
        if (stub) {
            char buf[176];
            snprintf(buf, sizeof(buf), "GetProcAddress: %s absent from Wine "
                     "-> no-op stub (perflib counterset)", name);
            dbg(buf);
            return stub;
        }
    }
    if (!p)
        return p;
    if (mod != GetModuleHandleA("ws2_32.dll"))
        return p;

    if (strcmp(name, "GetNameInfoW") == 0) {
        real_GetNameInfoW = (gni_w_t)p;
        dbg("GetProcAddress: handed out hooked GetNameInfoW");
        return (FARPROC)hook_GetNameInfoW;
    }
    if (strcmp(name, "getnameinfo") == 0) {
        real_getnameinfo = (gni_a_t)p;
        dbg("GetProcAddress: handed out hooked getnameinfo");
        return (FARPROC)hook_getnameinfo;
    }
    return p;
}



static LONG (WINAPI *real_RegSetKeySecurity)(HKEY hKey, SECURITY_INFORMATION si,
    PSECURITY_DESCRIPTOR psd) = NULL;

static LONG WINAPI hook_RegSetKeySecurity(HKEY hKey, SECURITY_INFORMATION si,
    PSECURITY_DESCRIPTOR psd)
{
    (void)hKey; (void)si; (void)psd;
    dbg("RegSetKeySecurity: blocked (Wine SYSTEM SID workaround)");
    return ERROR_SUCCESS;
}




static BOOL hook_import(HMODULE mod, const char *dll, const char *fn, WORD ord,
                        void *newfn, void **saved)
{
    if (!mod) return FALSE;
    PIMAGE_DOS_HEADER dos = (PIMAGE_DOS_HEADER)mod;
    if (dos->e_magic != IMAGE_DOS_SIGNATURE) return FALSE;
    PIMAGE_NT_HEADERS nt = (PIMAGE_NT_HEADERS)((BYTE *)mod + dos->e_lfanew);
    if (nt->Signature != IMAGE_NT_SIGNATURE) return FALSE;
    DWORD rva = nt->OptionalHeader.DataDirectory[IMAGE_DIRECTORY_ENTRY_IMPORT].VirtualAddress;
    if (!rva) return FALSE;

    for (PIMAGE_IMPORT_DESCRIPTOR imp =
            (PIMAGE_IMPORT_DESCRIPTOR)((BYTE *)mod + rva); imp->Name; imp++) {
        if (_stricmp((char *)mod + imp->Name, dll) != 0)
            continue;
        PIMAGE_THUNK_DATA orig  = (PIMAGE_THUNK_DATA)((BYTE *)mod + imp->OriginalFirstThunk);
        PIMAGE_THUNK_DATA thunk = (PIMAGE_THUNK_DATA)((BYTE *)mod + imp->FirstThunk);
        for (; orig->u1.AddressOfData; orig++, thunk++) {
            BOOL match;
            if (orig->u1.Ordinal & IMAGE_ORDINAL_FLAG) {
                
                match = (fn == NULL) &&
                        (IMAGE_ORDINAL(orig->u1.Ordinal) == ord);
            } else {
                if (fn == NULL) continue;
                PIMAGE_IMPORT_BY_NAME bn =
                    (PIMAGE_IMPORT_BY_NAME)((BYTE *)mod + orig->u1.AddressOfData);
                match = (strcmp(bn->Name, fn) == 0);
            }
            if (!match) continue;
            if (saved && !*saved) *saved = (void *)thunk->u1.Function;
            DWORD old;
            if (!VirtualProtect(&thunk->u1.Function, sizeof(DWORD_PTR), PAGE_READWRITE, &old))
                return FALSE;
            thunk->u1.Function = (DWORD_PTR)newfn;
            VirtualProtect(&thunk->u1.Function, sizeof(DWORD_PTR), old, &old);
            return TRUE;
        }
    }
    return FALSE;
}


static void hook_iphlpapi(HMODULE mod, const char *tag)
{
    char buf[64];
    if (hook_import(mod, "IPHLPAPI.DLL", "GetAdaptersAddresses", 0,
                    hook_GetAdaptersAddresses, (void **)&real_GetAdaptersAddresses)) {
        snprintf(buf, sizeof(buf), "hooked GetAdaptersAddresses (%s)", tag); dbg(buf);
    }
    if (hook_import(mod, "IPHLPAPI.DLL", "GetAdaptersInfo", 0,
                    hook_GetAdaptersInfo, (void **)&real_GetAdaptersInfo)) {
        snprintf(buf, sizeof(buf), "hooked GetAdaptersInfo (%s)", tag); dbg(buf);
    }
}


static void hook_ws2_32(HMODULE mod, const char *tag)
{
    char buf[64];
    if (hook_import(mod, "WS2_32.DLL", NULL, WS2_ORD_GETHOSTBYNAME,
                    hook_gethostbyname, (void **)&real_gethostbyname)) {
        snprintf(buf, sizeof(buf), "hooked gethostbyname (ord 52) (%s)", tag); dbg(buf);
    }
    if (hook_import(mod, "WS2_32.DLL", "getaddrinfo", 0,
                    hook_getaddrinfo, (void **)&real_getaddrinfo)) {
        snprintf(buf, sizeof(buf), "hooked getaddrinfo (%s)", tag); dbg(buf);
    }
    if (hook_import(mod, "WS2_32.DLL", "getnameinfo", 0,
                    hook_getnameinfo, (void **)&real_getnameinfo)) {
        snprintf(buf, sizeof(buf), "hooked getnameinfo (%s)", tag); dbg(buf);
    }
}



static volatile LONG g_rol_patched = 0;

static void try_patch_rol(void)
{
    if (g_rol_patched) return;
    HMODULE rol = GetModuleHandleW(L"RvROLClient.dll");
    if (!rol) return;
    if (InterlockedExchange(&g_rol_patched, 1) != 0) return;   
    hook_iphlpapi(rol, "RvROLClient.dll");
    hook_ws2_32(rol, "RvROLClient.dll");
    
    if (hook_import(rol, "KERNEL32.dll", "GetProcAddress", 0,
                    hook_GetProcAddress, (void **)&real_GetProcAddress))
        dbg("hooked GetProcAddress (RvROLClient.dll)");
    dbg("RvROLClient.dll IAT patched");
}

static HMODULE (WINAPI *real_LoadLibraryW)(LPCWSTR) = NULL;
static HMODULE (WINAPI *real_LoadLibraryA)(LPCSTR) = NULL;
static HMODULE (WINAPI *real_LoadLibraryExW)(LPCWSTR, HANDLE, DWORD) = NULL;

static HMODULE WINAPI hook_LoadLibraryW(LPCWSTR name)
{
    HMODULE h = real_LoadLibraryW ? real_LoadLibraryW(name) : NULL;
    try_patch_rol();
    return h;
}
static HMODULE WINAPI hook_LoadLibraryA(LPCSTR name)
{
    HMODULE h = real_LoadLibraryA ? real_LoadLibraryA(name) : NULL;
    try_patch_rol();
    return h;
}
static HMODULE WINAPI hook_LoadLibraryExW(LPCWSTR name, HANDLE file, DWORD flags)
{
    HMODULE h = real_LoadLibraryExW ? real_LoadLibraryExW(name, file, flags) : NULL;
    try_patch_rol();
    return h;
}


static DWORD WINAPI rol_watch_thread(LPVOID unused)
{
    (void)unused;
    for (int i = 0; i < 600 && !g_rol_patched; i++) {
        try_patch_rol();
        Sleep(100);
    }
    return 0;
}



__declspec(dllexport) int AdapterHookInit(void) { return 1; }



BOOL WINAPI DllMain(HINSTANCE inst, DWORD reason, LPVOID reserved)
{
    (void)inst; (void)reserved;
    if (reason == DLL_PROCESS_ATTACH) {
        dbg("adapter_hook.dll loaded");

        
        AddVectoredExceptionHandler(1, crash_handler);
        dbg("crash handler installed");

        
        g_jmp_tls = TlsAlloc();
        install_release_guard();

        HMODULE exe = GetModuleHandle(NULL);
        if (!exe) {
            dbg("GetModuleHandle(NULL) returned NULL");
            return TRUE;
        }

        
        hook_iphlpapi(exe, "RvControlSvc.exe");
        hook_ws2_32(exe, "RvControlSvc.exe");
        hook_import(exe, "ADVAPI32.DLL", "RegSetKeySecurity", 0,
                    hook_RegSetKeySecurity,  (void **)&real_RegSetKeySecurity);
        hook_import(exe, "KERNEL32.dll", "LoadLibraryW", 0,
                    hook_LoadLibraryW,       (void **)&real_LoadLibraryW);
        hook_import(exe, "KERNEL32.dll", "LoadLibraryA", 0,
                    hook_LoadLibraryA,       (void **)&real_LoadLibraryA);
        hook_import(exe, "KERNEL32.dll", "LoadLibraryExW", 0,
                    hook_LoadLibraryExW,     (void **)&real_LoadLibraryExW);
        hook_import(exe, "KERNEL32.dll", "GetProcAddress", 0,
                    hook_GetProcAddress,     (void **)&real_GetProcAddress);

        
        try_patch_rol();
        { HANDLE t = CreateThread(NULL, 0, rol_watch_thread, NULL, 0, NULL);
          if (t) CloseHandle(t); }

        if (real_GetAdaptersAddresses && real_RegSetKeySecurity)
            dbg("core IAT hooks installed");
        else
            dbg("WARNING: some core IAT hooks failed");
    }
    return TRUE;
}
