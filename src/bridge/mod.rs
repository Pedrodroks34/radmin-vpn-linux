use crate::core::constants::ETH_FRAME_MIN;
use crate::core::constants::*;
use crate::core::types::{EthernetFrame, MacAddress};
use crate::network::tap::TapInterface;
use nix::errno::Errno;
use nix::fcntl::{fcntl, OFlag, F_GETFL, F_SETFL};
use nix::sys::stat::Mode;
use nix::unistd::{close, read};
use std::collections::VecDeque;
use std::ffi::CString;
use std::os::unix::fs::FileTypeExt;
use std::os::unix::io::{AsRawFd, RawFd};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::{Duration, Instant};
use tracing::{debug, error, info, warn};

#[derive(Clone, Debug, Default)]
struct FilterConfig {
    block_ips_enabled: bool,
    blocked_ips: Vec<[u8; 4]>,
    block_macs_enabled: bool,
    blocked_macs: Vec<[u8; 6]>,
    block_broadcast_enabled: bool,
    broadcast_block_ips: Vec<[u8; 4]>,
}

const ARP_CACHE_SIZE: usize = 256;
const ARP_CACHE_TTL_SEC: i64 = 600;

#[derive(Debug, Default)]
pub struct BridgeStats {
    pub tap_to_drv: AtomicU64,
    pub drv_to_tap_high: AtomicU64,
    pub drv_to_tap_low: AtomicU64,
    pub drop_ip: AtomicU64,
    pub drop_mac: AtomicU64,
    pub drop_bcast: AtomicU64,
    pub arp_snooped: AtomicU64,
    pub arp_replies_sent: AtomicU64,
}

#[derive(Clone, Debug)]
struct ArpCacheEntry {
    last_seen: i64,
    valid: bool,
    ip: [u8; 4],
    mac: [u8; 6],
}

pub struct Bridge {
    tap: TapInterface,
    b2d_fd: Option<RawFd>,
    d2b_high_fd: Option<RawFd>,
    d2b_low_fd: Option<RawFd>,
    local_ip: Option<[u8; 4]>,
    stats: Arc<BridgeStats>,
    running: Arc<AtomicBool>,
    filters: Arc<RwLock<FilterConfig>>,
    arp_cache: Arc<RwLock<Vec<ArpCacheEntry>>>,
}

impl Bridge {
    pub fn new(tap: TapInterface) -> Result<Self, crate::RvpnError> {
        Ok(Self {
            tap,
            b2d_fd: None,
            d2b_high_fd: None,
            d2b_low_fd: None,
            local_ip: None,
            stats: Arc::new(BridgeStats::default()),
            running: Arc::new(AtomicBool::new(false)),
            filters: Arc::new(RwLock::new(FilterConfig::default())),
            arp_cache: Arc::new(RwLock::new(Vec::with_capacity(ARP_CACHE_SIZE))),
        })
    }

    pub fn create_fifos() -> Result<(), crate::RvpnError> {
        for path in [FIFO_B2D, FIFO_D2B_HIGH, FIFO_D2B_LOW] {
            match std::fs::symlink_metadata(path) {
                Ok(meta) if meta.file_type().is_fifo() => continue,
                Ok(_) => {
                    return Err(crate::RvpnError::Bridge(format!(
                        "{path} exists and is not a FIFO; refusing to replace it"
                    )));
                }
                Err(_) => {}
            }
            let c_path = CString::new(path)
                .map_err(|e| crate::RvpnError::Bridge(format!("CString {path}: {e}")))?;
            let mode = Mode::S_IRWXU.bits() | Mode::S_IRWXG.bits() | Mode::S_IRWXO.bits();
            let ret = unsafe { libc::mkfifo(c_path.as_ptr(), mode) };
            if ret != 0 {
                let err = Errno::last();
                return Err(crate::RvpnError::Bridge(format!("mkfifo {path}: {err}")));
            }
        }
        info!(
            "FIFOs created: {}, {}, {}",
            FIFO_B2D, FIFO_D2B_HIGH, FIFO_D2B_LOW
        );
        Ok(())
    }

    pub fn remove_fifos() -> Result<(), crate::RvpnError> {
        for path in [FIFO_B2D, FIFO_D2B_HIGH, FIFO_D2B_LOW] {
            if let Ok(meta) = std::fs::symlink_metadata(path) {
                if meta.file_type().is_fifo() {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
        Ok(())
    }

    fn detect_local_ip(&self) -> Option<[u8; 4]> {
        let socket = match std::net::UdpSocket::bind("0.0.0.0:0") {
            Ok(s) => s,
            Err(_) => return None,
        };

        let fd = socket.as_raw_fd();
        let mut ifr: libc::ifreq = unsafe { std::mem::zeroed() };
        let name_bytes = self.tap.name().as_bytes();
        let len = name_bytes.len().min(libc::IFNAMSIZ - 1);
        let name_bytes_i8: &[i8] =
            unsafe { std::slice::from_raw_parts(name_bytes.as_ptr() as *const i8, len) };
        ifr.ifr_name[..len].copy_from_slice(name_bytes_i8);

        let ret = unsafe { libc::ioctl(fd, libc::SIOCGIFADDR, &mut ifr) };
        if ret < 0 {
            return None;
        }

        unsafe {
            let addr_ptr = &ifr.ifr_ifru as *const _ as *const libc::sockaddr_in;
            let sin_addr = (*addr_ptr).sin_addr;
            Some(u32::from_le(sin_addr.s_addr).to_ne_bytes())
        }
    }

    pub fn run(&mut self) -> Result<BridgeStatsSnapshot, crate::RvpnError> {
        self.local_ip = self.detect_local_ip();
        if let Some(ip) = self.local_ip {
            info!("Local TAP IP: {}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]);
        }

        Self::create_fifos()?;

        info!("Opening FIFOs for frame relay");

        let d2b_high_fd = nix::fcntl::open(
            FIFO_D2B_HIGH,
            OFlag::O_RDWR | OFlag::O_NONBLOCK,
            Mode::S_IRWXU | Mode::S_IRWXG | Mode::S_IRWXO,
        )
        .map_err(|e| crate::RvpnError::Bridge(format!("open d2b_high: {e}")))?;

        let _ = unsafe { libc::fcntl(d2b_high_fd, libc::F_SETPIPE_SZ, 256 * 1024) };

        let d2b_low_fd = nix::fcntl::open(
            FIFO_D2B_LOW,
            OFlag::O_RDWR | OFlag::O_NONBLOCK,
            Mode::S_IRWXU | Mode::S_IRWXG | Mode::S_IRWXO,
        )
        .map_err(|e| crate::RvpnError::Bridge(format!("open d2b_low: {e}")))?;

        let _ = unsafe { libc::fcntl(d2b_low_fd, libc::F_SETPIPE_SZ, 1024 * 1024) };

        let b2d_fd = nix::fcntl::open(
            FIFO_B2D,
            OFlag::O_RDWR | OFlag::O_NONBLOCK,
            Mode::S_IRWXU | Mode::S_IRWXG | Mode::S_IRWXO,
        )
        .map_err(|e| crate::RvpnError::Bridge(format!("open b2d: {e}")))?;

        let flags = fcntl(b2d_fd, F_GETFL)
            .map_err(|e| crate::RvpnError::Bridge(format!("fcntl GETFL: {e}")))?;
        let flags = OFlag::from_bits_truncate(flags);
        let _ = fcntl(b2d_fd, F_SETFL(flags | OFlag::O_NONBLOCK));

        self.b2d_fd = Some(b2d_fd);
        self.d2b_high_fd = Some(d2b_high_fd);
        self.d2b_low_fd = Some(d2b_low_fd);
        self.running.store(true, Ordering::SeqCst);

        info!("FIFOs connected! Relaying frames (dual-thread).");

        let running = self.running.clone();
        let stats = self.stats.clone();
        let filters = self.filters.clone();
        let arp_cache = self.arp_cache.clone();
        let tap_fd = self.tap.fd();

        let b2d_fd_out = self.b2d_fd.unwrap();
        let d2b_high_fd_in = self.d2b_high_fd.unwrap();
        let d2b_low_fd_in = self.d2b_low_fd.unwrap();

        let tap_name = self.tap.name().to_string();
        let tap_mac = self.tap.mac();

        if let Ok(cfg) = load_filter_config() {
            *self.filters.write().unwrap() = cfg;
        }

        let running_out = running.clone();
        let stats_out = stats.clone();
        let filters_out = filters.clone();
        let arp_cache_out = arp_cache.clone();

        let out_thread = thread::spawn(move || {
            outgoing_thread(OutgoingContext {
                tap_fd,
                b2d_fd: b2d_fd_out,
                running: running_out,
                stats: stats_out,
                filters: filters_out,
                arp_cache: arp_cache_out,
                _tap_name: tap_name,
                _tap_mac: tap_mac,
            })
        });

        let in_thread = thread::spawn(move || {
            incoming_thread(
                d2b_high_fd_in,
                d2b_low_fd_in,
                running,
                stats,
                filters,
                arp_cache,
                tap_fd,
            )
        });

        let _ = out_thread.join();
        let _ = in_thread.join();

        self.running.store(false, Ordering::SeqCst);

        for fd in self
            .b2d_fd
            .iter()
            .chain(self.d2b_high_fd.iter())
            .chain(self.d2b_low_fd.iter())
        {
            let _ = close(*fd);
        }

        Ok(self.snapshot_stats())
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    pub fn control(&self) -> BridgeControl {
        BridgeControl {
            running: self.running.clone(),
        }
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    pub fn snapshot_stats(&self) -> BridgeStatsSnapshot {
        BridgeStatsSnapshot {
            tap_to_drv: self.stats.tap_to_drv.load(Ordering::Relaxed),
            drv_to_tap_high: self.stats.drv_to_tap_high.load(Ordering::Relaxed),
            drv_to_tap_low: self.stats.drv_to_tap_low.load(Ordering::Relaxed),
            drop_ip: self.stats.drop_ip.load(Ordering::Relaxed),
            drop_mac: self.stats.drop_mac.load(Ordering::Relaxed),
            drop_bcast: self.stats.drop_bcast.load(Ordering::Relaxed),
            arp_snooped: self.stats.arp_snooped.load(Ordering::Relaxed),
            arp_replies_sent: self.stats.arp_replies_sent.load(Ordering::Relaxed),
        }
    }

    pub fn load_filters(&self) -> Result<(), crate::RvpnError> {
        let config = load_filter_config()?;
        *self.filters.write().unwrap() = config;
        Ok(())
    }
}

#[derive(Debug, Clone, Default)]
pub struct BridgeStatsSnapshot {
    pub tap_to_drv: u64,
    pub drv_to_tap_high: u64,
    pub drv_to_tap_low: u64,
    pub drop_ip: u64,
    pub drop_mac: u64,
    pub drop_bcast: u64,
    pub arp_snooped: u64,
    pub arp_replies_sent: u64,
}

#[derive(Clone)]
pub struct BridgeControl {
    running: Arc<AtomicBool>,
}

impl BridgeControl {
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }
}

struct OutgoingContext {
    tap_fd: RawFd,
    b2d_fd: RawFd,
    running: Arc<AtomicBool>,
    stats: Arc<BridgeStats>,
    filters: Arc<RwLock<FilterConfig>>,
    arp_cache: Arc<RwLock<Vec<ArpCacheEntry>>>,
    _tap_name: String,
    _tap_mac: MacAddress,
}

fn outgoing_thread(ctx: OutgoingContext) {
    let OutgoingContext {
        tap_fd,
        b2d_fd,
        running,
        stats,
        filters,
        arp_cache,
        _tap_name,
        _tap_mac,
    } = ctx;

    let mut buf = [0u8; RELAY_BUF_MAX];
    let mut retry_ring: VecDeque<Vec<u8>> = VecDeque::with_capacity(8192);
    let mut last_filter_load = Instant::now();

    while running.load(Ordering::Relaxed) {
        if !retry_ring.is_empty() {
            match flush_retry_ring(b2d_fd, &mut retry_ring) {
                Ok(_) => {}
                Err(_) => {
                    error!("b2d write failed, shutting down outgoing thread");
                    running.store(false, Ordering::SeqCst);
                    break;
                }
            }
        }

        if last_filter_load.elapsed() > Duration::from_secs(1) {
            if let Ok(cfg) = load_filter_config() {
                *filters.write().unwrap() = cfg;
            }
            last_filter_load = Instant::now();
        }

        match wait_readable(tap_fd, Duration::from_secs(1)) {
            Ok(false) => continue,
            Err(e) => {
                if e.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                error!("TAP select error: {e}");
                running.store(false, Ordering::SeqCst);
                break;
            }
            Ok(true) => {}
        }

        let n = match read(tap_fd, &mut buf) {
            Ok(n) => n,
            Err(e) => {
                error!("read TAP error: {e}; stopping bridge");
                running.store(false, Ordering::SeqCst);
                break;
            }
        };
        if n == 0 {
            continue;
        }

        let len = n as u16;
        let frame = EthernetFrame::new(buf[..n].to_vec());

        {
            let mut cache = arp_cache.write().unwrap();
            if len >= 42 && frame.ethertype() == Some(0x0806) {
                let ip = [buf[28], buf[29], buf[30], buf[31]];
                let mac = [buf[22], buf[23], buf[24], buf[25], buf[26], buf[27]];
                arp_cache_update(&mut cache, ip, mac, &stats);
            }
        }

        {
            let filters_guard = filters.read().unwrap();
            if should_drop_frame(&frame, 0, &filters_guard) {
                stats.drop_mac.fetch_add(1, Ordering::Relaxed);
                continue;
            }
        }

        let mut combined = Vec::with_capacity(2 + n);
        combined.push((len & 0xFF) as u8);
        combined.push(((len >> 8) & 0xFF) as u8);
        combined.extend_from_slice(&buf[..n]);

        match write_exact_nonblock(b2d_fd, &combined) {
            Ok(()) => {
                stats.tap_to_drv.fetch_add(1, Ordering::Relaxed);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if retry_ring.len() < 8192 {
                    retry_ring.push_back(combined);
                } else {
                    warn!("retry ring full, dropped outgoing frame ({n} bytes)");
                }
            }
            Err(e) => {
                error!("b2d write failed: {e}");
                running.store(false, Ordering::SeqCst);
                break;
            }
        }

        replicate_mcast_to_bcast(b2d_fd, &frame, true);
    }

    running.store(false, Ordering::SeqCst);
}

fn incoming_thread(
    d2b_high_fd: RawFd,
    d2b_low_fd: RawFd,
    running: Arc<AtomicBool>,
    stats: Arc<BridgeStats>,
    filters: Arc<RwLock<FilterConfig>>,
    arp_cache: Arc<RwLock<Vec<ArpCacheEntry>>>,
    tap_fd: RawFd,
) {
    let mut buf = [0u8; RELAY_BUF_MAX];
    let maxfd = d2b_high_fd.max(d2b_low_fd);
    let mut last_low_attempt = Instant::now();
    let mut have_last_low_attempt = false;

    while running.load(Ordering::Relaxed) {
        use libc::{fd_set, select, timeval, FD_ISSET, FD_SET, FD_ZERO};

        let mut rfds: fd_set = unsafe { std::mem::zeroed() };
        unsafe { FD_ZERO(&mut rfds) };
        unsafe { FD_SET(d2b_high_fd, &mut rfds) };
        unsafe { FD_SET(d2b_low_fd, &mut rfds) };

        let mut tv = timeval {
            tv_sec: 0,
            tv_usec: 200000,
        };

        let ret = unsafe {
            select(
                maxfd + 1,
                &mut rfds,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut tv,
            )
        };
        if ret < 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            error!("incoming select error: {err}");
            running.store(false, Ordering::SeqCst);
            break;
        }
        if ret == 0 {
            if have_last_low_attempt && last_low_attempt.elapsed() >= Duration::from_micros(50000) {
                if let Ok(Some(_len)) = process_incoming_frame(
                    d2b_low_fd, &mut buf, &stats, &filters, &arp_cache, tap_fd,
                ) {
                    stats.drv_to_tap_low.fetch_add(1, Ordering::Relaxed);
                }
                last_low_attempt = Instant::now();
            }
            continue;
        }

        let high_ready = unsafe { FD_ISSET(d2b_high_fd, &rfds) };
        let _low_ready = unsafe { FD_ISSET(d2b_low_fd, &rfds) };

        if high_ready {
            while running.load(Ordering::Relaxed) {
                match process_incoming_frame(
                    d2b_high_fd,
                    &mut buf,
                    &stats,
                    &filters,
                    &arp_cache,
                    tap_fd,
                ) {
                    Ok(Some(_len)) => {
                        stats.drv_to_tap_high.fetch_add(1, Ordering::Relaxed);
                    }
                    Ok(None) => break,
                    Err(_) => {
                        running.store(false, Ordering::SeqCst);
                        break;
                    }
                }
            }
        }

        if !running.load(Ordering::Relaxed) {
            break;
        }

        let low_ready = unsafe { FD_ISSET(d2b_low_fd, &rfds) };
        if low_ready
            || (have_last_low_attempt && last_low_attempt.elapsed() >= Duration::from_micros(50000))
        {
            last_low_attempt = Instant::now();
            have_last_low_attempt = true;

            if let Ok(Some(_len)) =
                process_incoming_frame(d2b_low_fd, &mut buf, &stats, &filters, &arp_cache, tap_fd)
            {
                stats.drv_to_tap_low.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    running.store(false, Ordering::SeqCst);
}

fn process_incoming_frame(
    fd: RawFd,
    buf: &mut [u8],
    stats: &Arc<BridgeStats>,
    filters: &Arc<RwLock<FilterConfig>>,
    arp_cache: &Arc<RwLock<Vec<ArpCacheEntry>>>,
    tap_fd: RawFd,
) -> Result<Option<u16>, std::io::Error> {
    let mut len_buf = [0u8; 2];
    match read_exact(fd, &mut len_buf) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return Ok(None),
        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => return Ok(None),
        Err(e) => return Err(e),
    }

    let len = u16::from_le_bytes([len_buf[0], len_buf[1]]) as usize;
    if len > ETH_FRAME_MAX {
        error!("corrupt frame len {} > {}", len, ETH_FRAME_MAX);
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "corrupt frame",
        ));
    }

    match read_exact(fd, &mut buf[..len]) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return Ok(None),
        Err(e) => return Err(e),
    }

    let frame = EthernetFrame::new(buf[..len].to_vec());

    {
        let mut cache = arp_cache.write().unwrap();
        if len >= 34 && frame.ethertype() == Some(0x0800) {
            let ip = [buf[26], buf[27], buf[28], buf[29]];
            let mac = [buf[6], buf[7], buf[8], buf[9], buf[10], buf[11]];
            arp_cache_update(&mut cache, ip, mac, stats);
        }
        if len >= 42 && frame.ethertype() == Some(0x0806) {
            let ip = [buf[28], buf[29], buf[30], buf[31]];
            let mac = [buf[22], buf[23], buf[24], buf[25], buf[26], buf[27]];
            arp_cache_update(&mut cache, ip, mac, stats);
        }
    }

    {
        let filters_guard = filters.read().unwrap();
        if should_drop_frame(&frame, 1, &filters_guard) {
            stats.drop_ip.fetch_add(1, Ordering::Relaxed);
            return Ok(Some(len as u16));
        }
    }

    let ret = unsafe { libc::write(tap_fd, buf.as_ptr() as *const libc::c_void, len) };
    if ret < 0 {
        let err = std::io::Error::last_os_error();
        if err.kind() == std::io::ErrorKind::WouldBlock {
            return Ok(Some(len as u16));
        }
        return Err(err);
    }
    if ret as usize != len {
        warn!("short write to TAP");
        return Ok(Some(len as u16));
    }

    replicate_mcast_to_bcast(tap_fd, &frame, false);

    Ok(Some(len as u16))
}

fn write_exact_nonblock(fd: RawFd, data: &[u8]) -> Result<(), std::io::Error> {
    use std::io::Write;
    use std::os::unix::io::FromRawFd;
    let mut fd_file = unsafe { std::fs::File::from_raw_fd(fd) };
    let result = fd_file.write_all(data);

    std::mem::forget(fd_file);
    result
}

fn read_exact(fd: RawFd, buf: &mut [u8]) -> Result<(), std::io::Error> {
    let mut done = 0;
    while done < buf.len() {
        match read(fd, &mut buf[done..]) {
            Ok(0) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "EOF",
                ))
            }
            Ok(n) => done += n,
            Err(e) => {
                let io_err: std::io::Error = e.into();
                if io_err.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(io_err);
            }
        }
    }
    Ok(())
}

fn wait_readable(fd: RawFd, timeout: Duration) -> Result<bool, std::io::Error> {
    use libc::{fd_set, select, timeval, FD_ISSET, FD_SET, FD_ZERO};

    let mut rfds: fd_set = unsafe { std::mem::zeroed() };
    unsafe { FD_ZERO(&mut rfds) };
    unsafe { FD_SET(fd, &mut rfds) };

    let mut tv = timeval {
        tv_sec: timeout.as_secs() as libc::time_t,
        tv_usec: timeout.subsec_micros() as libc::suseconds_t,
    };

    let ret = unsafe {
        select(
            fd + 1,
            &mut rfds,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut tv,
        )
    };
    if ret < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { FD_ISSET(fd, &rfds) })
}

fn flush_retry_ring(fd: RawFd, ring: &mut VecDeque<Vec<u8>>) -> Result<(), std::io::Error> {
    while !ring.is_empty() {
        let entry = ring.front().unwrap().clone();
        match write_exact_nonblock(fd, &entry) {
            Ok(()) => {
                ring.pop_front();
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "retry flush failed",
                ))
            }
        }
    }
    Ok(())
}

fn should_drop_frame(frame: &EthernetFrame, direction: i32, cfg: &FilterConfig) -> bool {
    if frame.data.len() < 14 {
        return false;
    }

    if cfg.block_macs_enabled && !cfg.blocked_macs.is_empty() {
        let src = frame.src_mac().map(|m| m.bytes()).unwrap_or([0; 6]);
        let dst = frame.dst_mac().map(|m| m.bytes()).unwrap_or([0; 6]);
        if cfg.blocked_macs.iter().any(|m| *m == src || *m == dst) {
            return true;
        }
    }

    if frame.data.len() < 34 || frame.ethertype() != Some(0x0800) {
        return false;
    }

    if frame.data[14] >> 4 != 4 || (frame.data[14] & 0x0F) < 5 {
        return false;
    }

    let src_ip = [
        frame.data[26],
        frame.data[27],
        frame.data[28],
        frame.data[29],
    ];
    let dst_ip = [
        frame.data[30],
        frame.data[31],
        frame.data[32],
        frame.data[33],
    ];

    if cfg.block_ips_enabled
        && !cfg.blocked_ips.is_empty()
        && cfg.blocked_ips.iter().any(|m| *m == src_ip || *m == dst_ip)
    {
        return true;
    }

    if direction == 1 && cfg.block_broadcast_enabled && !cfg.broadcast_block_ips.is_empty() {
        if frame.data[23] != 17 {
            return false;
        }
        let ihl = (frame.data[14] & 0x0F) as usize * 4;
        if frame.data.len() < 14 + ihl + 8 {
            return false;
        }
        let udp_hdr = &frame.data[14 + ihl..14 + ihl + 8];
        let src_port = u16::from_be_bytes([udp_hdr[0], udp_hdr[1]]);
        let dst_port = u16::from_be_bytes([udp_hdr[2], udp_hdr[3]]);
        if (src_port == 4445 || dst_port == 4445)
            && cfg
                .broadcast_block_ips
                .iter()
                .any(|m| *m == src_ip || *m == dst_ip)
        {
            return true;
        }
    }

    false
}

fn arp_cache_update(
    cache: &mut Vec<ArpCacheEntry>,
    ip: [u8; 4],
    mac: [u8; 6],
    stats: &Arc<BridgeStats>,
) {
    let mut ts: libc::timespec = unsafe { std::mem::zeroed() };
    let now = unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    let now = if now == 0 { ts.tv_sec } else { 0 };

    for entry in cache.iter_mut() {
        if entry.valid && entry.ip == ip {
            entry.mac = mac;
            entry.last_seen = now;
            return;
        }
    }

    for entry in cache.iter_mut() {
        if !entry.valid || now - entry.last_seen > ARP_CACHE_TTL_SEC {
            entry.ip = ip;
            entry.mac = mac;
            entry.last_seen = now;
            entry.valid = true;
            stats.arp_snooped.fetch_add(1, Ordering::Relaxed);
            return;
        }
    }

    if cache.len() < ARP_CACHE_SIZE {
        cache.push(ArpCacheEntry {
            last_seen: now,
            valid: true,
            ip,
            mac,
        });
        stats.arp_snooped.fetch_add(1, Ordering::Relaxed);
        return;
    }

    if let Some(oldest_idx) = cache
        .iter()
        .enumerate()
        .min_by_key(|(_, e)| e.last_seen)
        .map(|(i, _)| i)
    {
        cache[oldest_idx] = ArpCacheEntry {
            last_seen: now,
            valid: true,
            ip,
            mac,
        };
        stats.arp_snooped.fetch_add(1, Ordering::Relaxed);
    }
}

fn replicate_mcast_to_bcast(fd: RawFd, frame: &EthernetFrame, is_fifo: bool) {
    if frame.data.len() < ETH_FRAME_MIN {
        return;
    }
    if frame.ethertype() != Some(0x0800) {
        return;
    }
    if frame.data[30] != 224 || frame.data[31] != 0 || frame.data[32] != 2 || frame.data[33] != 60 {
        return;
    }
    if frame.data[23] != 17 {
        return;
    }

    let ihl = (frame.data[14] & 0x0F) as usize * 4;
    let udp_csum_off = 14 + ihl + 6;

    let targets: [([u8; 4], &str); 2] = [
        ([26, 255, 255, 255], "26.255.255.255"),
        ([255, 255, 255, 255], "255.255.255.255"),
    ];

    for (target_ip, target_str) in &targets {
        let mut copy = frame.data.clone();
        copy[0..6].fill(0xFF);
        copy[30] = target_ip[0];
        copy[31] = target_ip[1];
        copy[32] = target_ip[2];
        copy[33] = target_ip[3];
        fix_ip_checksum(&mut copy);
        if udp_csum_off + 1 < copy.len() {
            copy[udp_csum_off] = 0;
            copy[udp_csum_off + 1] = 0;
        }

        if is_fifo {
            let len = copy.len() as u16;
            let mut combined = Vec::with_capacity(2 + copy.len());
            combined.push((len & 0xFF) as u8);
            combined.push(((len >> 8) & 0xFF) as u8);
            combined.extend_from_slice(&copy);
            let ret = unsafe {
                libc::write(fd, combined.as_ptr() as *const libc::c_void, combined.len())
            };
            if ret < 0 {
                break;
            }
        } else {
            let ret = unsafe { libc::write(fd, copy.as_ptr() as *const libc::c_void, copy.len()) };
            if ret < 0 {
                break;
            }
        }

        debug!(
            "replicated 224.0.2.60 → {} ({} bytes)",
            target_str,
            copy.len()
        );
    }
}

fn fix_ip_checksum(frame: &mut [u8]) {
    let ihl = (frame[14] & 0x0F) as usize * 4;
    frame[24] = 0;
    frame[25] = 0;
    let mut sum: u32 = 0;
    for i in (0..ihl).step_by(2) {
        sum += ((frame[14 + i] as u32) << 8) | (frame[15 + i] as u32);
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    let csum = !sum as u16;
    frame[24] = (csum >> 8) as u8;
    frame[25] = csum as u8;
}

fn load_filter_config() -> Result<FilterConfig, crate::RvpnError> {
    let path = std::path::Path::new(FILTER_PATH);
    if !path.exists() {
        return Ok(FilterConfig::default());
    }

    let contents = std::fs::read_to_string(path)
        .map_err(|e| crate::RvpnError::Bridge(format!("read filter config: {e}")))?;

    let block_ips_enabled = json_bool(&contents, "block_ips_enabled", false);
    let block_macs_enabled = json_bool(&contents, "block_macs_enabled", false);
    let block_broadcast_enabled = json_bool(&contents, "block_broadcast_enabled", false);

    let blocked_ips = json_str_array(&contents, "blocked_ips")
        .into_iter()
        .filter_map(|s| parse_ip(&s))
        .collect();

    let blocked_macs = json_str_array(&contents, "blocked_macs")
        .into_iter()
        .filter_map(|s| parse_mac(&s))
        .collect();

    let broadcast_block_ips = json_str_array(&contents, "broadcast_block_ips")
        .into_iter()
        .filter_map(|s| parse_ip(&s))
        .collect();

    Ok(FilterConfig {
        block_ips_enabled,
        blocked_ips,
        block_macs_enabled,
        blocked_macs,
        block_broadcast_enabled,
        broadcast_block_ips,
    })
}

fn json_find_key<'a>(json: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("\"{key}\"");
    let pos = json.find(&needle)?;
    let p = &json[pos + needle.len()..];
    let p = p.trim_start_matches([' ', '\t', ':', '\n', '\r']);
    Some(p)
}

fn json_bool(json: &str, key: &str, defval: bool) -> bool {
    json_find_key(json, key)
        .map(|p| {
            if p.starts_with("true") {
                true
            } else if p.starts_with("false") {
                false
            } else {
                defval
            }
        })
        .unwrap_or(defval)
}

fn json_str_array(json: &str, key: &str) -> Vec<String> {
    let mut result = Vec::new();
    let Some(p) = json_find_key(json, key) else {
        return result;
    };
    let bytes = p.as_bytes();
    if bytes.is_empty() || bytes[0] != b'[' {
        return result;
    }
    let mut i = 1;
    while i < bytes.len() {
        while i < bytes.len() && (bytes[i] as char).is_whitespace() || bytes[i] == b',' {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] == b']' {
            break;
        }
        if bytes[i] != b'"' {
            i += 1;
            continue;
        }
        i += 1;
        let start = i;
        while i < bytes.len() && bytes[i] != b'"' {
            i += 1;
        }
        if i < bytes.len() {
            result.push(String::from_utf8_lossy(&bytes[start..i]).to_string());
            i += 1;
        }
    }
    result
}

fn parse_ip(s: &str) -> Option<[u8; 4]> {
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() != 4 {
        return None;
    }
    let mut bytes = [0u8; 4];
    for (i, part) in parts.iter().enumerate() {
        let v: u8 = part.parse().ok()?;
        bytes[i] = v;
    }
    Some(bytes)
}

fn parse_mac(s: &str) -> Option<[u8; 6]> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 6 {
        return None;
    }
    let mut bytes = [0u8; 6];
    for (i, part) in parts.iter().enumerate() {
        let v = u8::from_str_radix(part, 16).ok()?;
        bytes[i] = v;
    }
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ip_test() {
        assert_eq!(parse_ip("192.168.1.1"), Some([192, 168, 1, 1]));
        assert_eq!(parse_ip("26.0.0.1"), Some([26, 0, 0, 1]));
        assert_eq!(parse_ip("999.1.1.1"), None);
    }

    #[test]
    fn parse_mac_test() {
        assert_eq!(
            parse_mac("02:50:de:ad:be:ef"),
            Some([0x02, 0x50, 0xde, 0xad, 0xbe, 0xef])
        );
        assert_eq!(parse_mac("ff:ff:ff:ff:ff:ff"), Some([0xFF; 6]));
        assert_eq!(parse_mac("invalid"), None);
    }

    #[test]
    fn fix_ip_checksum_test() {
        let mut frame = vec![
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x08, 0x00, 0x45, 0x00, 0x00, 0x28, 0, 0, 0, 0,
            0x40, 0x01, 0, 0, 192, 168, 1, 1, 10, 0, 0, 1,
        ];
        fix_ip_checksum(&mut frame);
        let csum = u16::from_be_bytes([frame[24], frame[25]]);
        assert_ne!(csum, 0);
    }

    #[test]
    fn read_write_exact_roundtrip() {
        let mut fds = [-1, -1];
        let rc = unsafe { libc::pipe(fds.as_mut_ptr()) };
        assert_eq!(rc, 0);
        let (read_fd, write_fd) = (fds[0], fds[1]);

        let payload = [0x34, 0x12, 0xaa, 0xbb, 0xcc];
        write_exact_nonblock(write_fd, &payload).unwrap();

        let mut buf = [0u8; 5];
        read_exact(read_fd, &mut buf).unwrap();
        assert_eq!(buf, payload);

        let _ = unsafe { libc::close(read_fd) };
        let _ = unsafe { libc::close(write_fd) };
    }

    #[test]
    fn flush_retry_ring_drains_when_pipe_writable() {
        let mut fds = [-1, -1];
        let rc = unsafe { libc::pipe(fds.as_mut_ptr()) };
        assert_eq!(rc, 0);
        let (read_fd, write_fd) = (fds[0], fds[1]);

        let ret = unsafe { libc::fcntl(write_fd, libc::F_SETFL, libc::O_NONBLOCK) };
        assert_eq!(ret, 0);

        let mut ring = VecDeque::new();
        ring.push_back(vec![0x01, 0x02, 0x03]);
        ring.push_back(vec![0x04, 0x05]);

        flush_retry_ring(write_fd, &mut ring).unwrap();
        assert!(ring.is_empty());

        let mut buf = [0u8; 5];
        let n = unsafe { libc::read(read_fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
        assert_eq!(n, 5);
        assert_eq!(buf, [0x01, 0x02, 0x03, 0x04, 0x05]);

        let _ = unsafe { libc::close(read_fd) };
        let _ = unsafe { libc::close(write_fd) };
    }

    #[test]
    fn should_drop_frame_respects_blocked_mac_and_ip() {
        let cfg = FilterConfig {
            block_macs_enabled: true,
            blocked_macs: vec![[0xde, 0xad, 0xbe, 0xef, 0x00, 0x01]],
            ..FilterConfig::default()
        };

        let frame = EthernetFrame::new(vec![
            0xde, 0xad, 0xbe, 0xef, 0x00, 0x01, 0x02, 0x50, 0xde, 0xad, 0xbe, 0xef, 0x08, 0x00,
            0x45, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 192, 168, 1, 1, 10, 0, 0, 1,
        ]);

        assert!(should_drop_frame(&frame, 0, &cfg));

        let cfg2 = FilterConfig {
            block_ips_enabled: true,
            blocked_ips: vec![[10, 0, 0, 1]],
            ..FilterConfig::default()
        };
        assert!(should_drop_frame(&frame, 0, &cfg2));
    }

    #[test]
    fn arp_cache_update_refreshes_entry() {
        let stats = Arc::new(BridgeStats::default());
        let mut cache = Vec::new();
        arp_cache_update(&mut cache, [10, 0, 0, 1], [1, 2, 3, 4, 5, 6], &stats);
        arp_cache_update(&mut cache, [10, 0, 0, 1], [6, 5, 4, 3, 2, 1], &stats);
        let entry = cache.iter().find(|e| e.ip == [10, 0, 0, 1]).unwrap();
        assert_eq!(entry.mac, [6, 5, 4, 3, 2, 1]);
        assert!(entry.valid);
    }

    #[test]
    fn should_drop_frame_test() {
        let cfg = FilterConfig::default();
        let frame = EthernetFrame::new(vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 0x08, 0x00]);
        assert!(!should_drop_frame(&frame, 0, &cfg));
    }
}
