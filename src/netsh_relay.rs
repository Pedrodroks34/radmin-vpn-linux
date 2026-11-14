use crate::core::constants::NETSH_CMD_FILE;
use crate::core::types::RvpnError;
use std::io::{Read, Seek, SeekFrom};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

const POLL_INTERVAL: Duration = Duration::from_millis(100);

pub fn spawn(tap_dev: &str) -> NetshRelayHandle {
    let stop = Arc::new(AtomicBool::new(false));
    let tap = tap_dev.to_string();
    let handle = std::thread::Builder::new()
        .name("rvpn-netsh-relay".to_string())
        .spawn({
            let stop = stop.clone();
            move || relay_loop(&tap, &stop)
        })
        .expect("failed to spawn netsh relay thread");
    NetshRelayHandle { stop, handle }
}

pub struct NetshRelayHandle {
    stop: Arc<AtomicBool>,
    handle: std::thread::JoinHandle<()>,
}

impl NetshRelayHandle {
    pub fn stop(self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = self.handle.join();
    }
}

fn relay_loop(tap_dev: &str, stop: &AtomicBool) {
    info!("netsh relay started for {tap_dev}");
    let mut offset: u64 = 0;

    while !stop.load(Ordering::Relaxed) {
        match std::fs::File::open(NETSH_CMD_FILE) {
            Ok(mut file) => {
                let len = file.metadata().map(|m| m.len()).unwrap_or(0);
                if len < offset {
                    offset = 0;
                }
                if len > offset {
                    let _ = file.seek(SeekFrom::Start(offset));
                    let mut buf = String::new();
                    if let Err(e) = file.read_to_string(&mut buf) {
                        warn!("netsh relay: read error: {e}");
                        offset = len;
                    } else {
                        for line in buf.lines() {
                            let line = line.trim();
                            if line.is_empty() {
                                continue;
                            }
                            if let Err(e) = execute_command(line, tap_dev) {
                                warn!("netsh relay: rejected command {line:?}: {e}");
                            }
                        }
                        offset = len;
                    }

                    if let Ok(f) = std::fs::File::create(NETSH_CMD_FILE) {
                        drop(f);
                        offset = 0;
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => warn!("netsh relay: open {NETSH_CMD_FILE}: {e}"),
        }

        std::thread::sleep(POLL_INTERVAL);
    }

    info!("netsh relay stopped");
}

fn execute_command(line: &str, tap_dev: &str) -> Result<(), RvpnError> {
    for part in line.split(';') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }

        let part = part.split("2>").next().unwrap_or(part).trim();

        let args = shell_words(part);
        if args.is_empty() {
            continue;
        }

        match args.as_slice() {
            ["ip", "addr", "add", addr_cidr, "dev", dev] if *dev == tap_dev => {
                let (ip, cidr) = parse_cidr(addr_cidr)?;
                run_ip(&["addr", "add", &format!("{ip}/{cidr}"), "dev", dev])?;
            }
            ["ip", "-6", "addr", "add", addr_cidr, "dev", dev] if *dev == tap_dev => {
                let (ip, cidr) = parse_ipv6_cidr(addr_cidr)?;
                run_ip(&["-6", "addr", "add", &format!("{ip}/{cidr}"), "dev", dev])?;
            }
            ["ip", "link", "set", dev, "up"] if *dev == tap_dev => {
                run_ip(&["link", "set", dev, "up"])?;
            }
            ["ip", "addr", "flush", "dev", dev] if *dev == tap_dev => {
                run_ip(&["addr", "flush", "dev", dev])?;
            }
            _ => {
                return Err(RvpnError::Service(format!("unsupported command: {part}")));
            }
        }
    }
    Ok(())
}

fn shell_words(s: &str) -> Vec<&str> {
    s.split_whitespace().collect()
}

fn parse_cidr(s: &str) -> Result<(String, u8), RvpnError> {
    let (ip, cidr) = s
        .split_once('/')
        .ok_or_else(|| RvpnError::Service(format!("invalid CIDR: {s}")))?;
    if !valid_ipv4(ip) {
        return Err(RvpnError::Service(format!("invalid IPv4: {ip}")));
    }
    let cidr: u8 = cidr
        .parse()
        .map_err(|_| RvpnError::Service(format!("invalid prefix length: {s}")))?;
    if cidr > 32 {
        return Err(RvpnError::Service(format!(
            "prefix length out of range: {cidr}"
        )));
    }
    Ok((ip.to_string(), cidr))
}

fn parse_ipv6_cidr(s: &str) -> Result<(String, u8), RvpnError> {
    let (ip, cidr) = s
        .split_once('/')
        .ok_or_else(|| RvpnError::Service(format!("invalid IPv6 CIDR: {s}")))?;
    if !valid_ipv6(ip) {
        return Err(RvpnError::Service(format!("invalid IPv6: {ip}")));
    }
    let cidr: u8 = cidr
        .parse()
        .map_err(|_| RvpnError::Service(format!("invalid prefix length: {s}")))?;
    if cidr > 128 {
        return Err(RvpnError::Service(format!(
            "prefix length out of range: {cidr}"
        )));
    }
    Ok((ip.to_string(), cidr))
}

fn valid_ipv4(s: &str) -> bool {
    let mut count = 0;
    for part in s.split('.') {
        if part.is_empty() || part.len() > 3 {
            return false;
        }
        if !part.chars().all(|c| c.is_ascii_digit()) {
            return false;
        }
        let octet: u16 = match part.parse() {
            Ok(v) => v,
            Err(_) => return false,
        };
        if octet > 255 {
            return false;
        }
        count += 1;
    }
    count == 4
}

fn valid_ipv6(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    s.split(':')
        .all(|part| part.is_empty() || part.chars().all(|c| c.is_ascii_hexdigit()))
}

fn run_ip(args: &[&str]) -> Result<(), RvpnError> {
    let output = Command::new("ip")
        .args(args)
        .output()
        .map_err(|e| RvpnError::Service(format!("ip {args:?} failed: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);

        if stderr.contains("File exists") {
            return Ok(());
        }
        return Err(RvpnError::Service(format!("ip {args:?} failed: {stderr}")));
    }
    Ok(())
}
