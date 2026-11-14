use crate::core::constants::NETSH_CMD_FILE;
use crate::core::types::RvpnError;
use std::io::{Read, Seek, SeekFrom};
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
                let (ip, prefix) = crate::network::netlink::parse_ipv4_cidr(addr_cidr)?;
                crate::network::netlink::addr_add(dev, std::net::IpAddr::V4(ip), prefix)?;
            }
            ["ip", "-6", "addr", "add", addr_cidr, "dev", dev] if *dev == tap_dev => {
                let (ip, prefix) = crate::network::netlink::parse_ipv6_cidr(addr_cidr)?;
                crate::network::netlink::addr_add(dev, std::net::IpAddr::V6(ip), prefix)?;
            }
            ["ip", "link", "set", dev, "up"] if *dev == tap_dev => {
                crate::network::netlink::link_set_up(dev)?;
            }
            ["ip", "addr", "flush", "dev", dev] if *dev == tap_dev => {
                crate::network::netlink::addr_flush(dev)?;
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
