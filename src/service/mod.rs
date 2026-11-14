use crate::bridge::{Bridge, BridgeControl, BridgeStatsSnapshot};
use crate::config::Config;
use crate::core::constants::*;
use crate::core::types::{MacAddress, RvpnError, ServiceState};
use crate::firewall::FirewallManager;
use crate::logging::{notify_systemd, watchdog_ping};
use crate::netsh_relay::{spawn as spawn_netsh_relay, NetshRelayHandle};
use crate::network::route::RouteManager;
use crate::network::tap::{configure_tap, destroy_owned_interface};
use crate::wine::WineProcessManager;
use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;
use std::collections::HashMap;
use std::os::unix::fs::FileTypeExt;
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;
use std::process::Child;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;
use tracing::{error, info, warn};

pub struct ServiceLock {
    lock_file: std::fs::File,
}

impl ServiceLock {
    pub fn acquire() -> Result<Self, RvpnError> {
        let lock_path = PathBuf::from(SERVICE_LOCK_FILE);
        if let Some(parent) = lock_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| RvpnError::Service(format!("create runtime dir: {e}")))?;
        }

        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|e| RvpnError::Service(format!("open lock file: {e}")))?;

        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if rc == 0 {
            info!("Acquired exclusive service lock");
            Ok(Self { lock_file: file })
        } else {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EWOULDBLOCK) {
                Err(RvpnError::Service(
                    "another radmin-vpn service instance is running".to_string(),
                ))
            } else {
                Err(RvpnError::Service(format!("flock failed: {err}")))
            }
        }
    }
}

impl Drop for ServiceLock {
    fn drop(&mut self) {
        let _ = unsafe { libc::flock(self.lock_file.as_raw_fd(), libc::LOCK_UN) };
        let _ = std::fs::remove_file(SERVICE_LOCK_FILE);
    }
}

struct SupervisedChild {
    name: String,
    child: Option<Child>,
    critical: bool,
}

struct ServiceLifecycle {
    state: ServiceState,
    route_manager: Option<RouteManager>,
    firewall: Option<FirewallManager>,
    bridge_control: Option<BridgeControl>,
    bridge_thread: Option<JoinHandle<Result<BridgeStatsSnapshot, RvpnError>>>,
    driver_thread: Option<JoinHandle<Result<(), RvpnError>>>,
    driver_running: Option<Arc<AtomicBool>>,
    netsh_relay: Option<NetshRelayHandle>,
    children: Mutex<HashMap<u32, SupervisedChild>>,
    running: Arc<AtomicBool>,
}

impl ServiceLifecycle {
    fn new() -> Self {
        Self {
            state: ServiceState::Stopped,
            route_manager: None,
            firewall: None,
            bridge_control: None,
            bridge_thread: None,
            driver_thread: None,
            driver_running: None,
            netsh_relay: None,
            children: Mutex::new(HashMap::new()),
            running: Arc::new(AtomicBool::new(false)),
        }
    }
}

pub struct RadminVpnService {
    config: Config,
    lifecycle: Mutex<ServiceLifecycle>,
    wine_manager: WineProcessManager,
    _lock: Option<ServiceLock>,
}

impl RadminVpnService {
    pub fn new(config: Config) -> Result<Self, RvpnError> {
        Ok(Self {
            wine_manager: WineProcessManager::new(&config),
            config,
            lifecycle: Mutex::new(ServiceLifecycle::new()),
            _lock: None,
        })
    }

    pub fn run(&mut self) -> Result<(), RvpnError> {
        info!("Starting radmin-vpn service");
        self._lock = Some(ServiceLock::acquire()?);

        let adapter_mac = self
            .config
            .adapter_mac
            .as_ref()
            .map(|s| MacAddress::parse(s))
            .transpose()
            .map_err(|e| RvpnError::Config(format!("invalid adapter MAC: {e}")))?
            .unwrap_or_else(generate_random_mac);

        let tap = configure_tap(&self.config.tap_dev, adapter_mac, DEFAULT_MTU, true)?;

        let mut route_manager = RouteManager::new(&self.config.tap_dev);
        route_manager.setup_vpn_routes(self.config.broadcast_routes)?;

        let mut firewall = FirewallManager::new(&self.config.tap_dev);
        if self.config.firewall {
            firewall.open()?;
        }

        let tap_guid = self.config.tap_guid.as_deref().unwrap_or(DEFAULT_TAP_GUID);
        self.wine_manager.install_components()?;
        self.wine_manager.configure_registry(tap_guid)?;
        self.write_mac_file(adapter_mac)?;

        let mut bridge = Bridge::new(tap)?;
        let bridge_control = bridge.control();
        let bridge_thread = std::thread::Builder::new()
            .name("rvpn-bridge".to_string())
            .spawn(move || bridge.run())
            .map_err(|e| RvpnError::Service(format!("spawn bridge thread: {e}")))?;

        let relay = spawn_netsh_relay(&self.config.tap_dev);

        let driver_running = Arc::new(AtomicBool::new(true));
        let driver_thread = {
            let running = driver_running.clone();
            let mac = adapter_mac;
            let sock = format!("{}/rvpn.control", SERVICE_RUNTIME_DIR);
            let _ = std::fs::create_dir_all(SERVICE_RUNTIME_DIR);
            std::thread::Builder::new()
                .name("rvpn-driver".to_string())
                .spawn(move || crate::driver::userspace::run(mac, &sock, running))
                .ok()
        };

        {
            let mut lc = self.lifecycle.lock().unwrap();
            lc.state = ServiceState::Running;
            lc.route_manager = Some(route_manager);
            lc.firewall = Some(firewall);
            lc.bridge_control = Some(bridge_control);
            lc.bridge_thread = Some(bridge_thread);
            lc.driver_running = Some(driver_running);
            lc.driver_thread = driver_thread;
            lc.netsh_relay = Some(relay);
            lc.running.store(true, Ordering::SeqCst);
        }

        notify_systemd(true, false);
        info!("radmin-vpn service is ready");

        if !self.config.no_ui {
            if let Err(e) = self.start_radmin_processes() {
                warn!("failed to start Radmin processes: {e}");
            }
        }

        self.monitor_loop()?;
        self.stop()?;
        Ok(())
    }

    pub fn stop(&mut self) -> Result<(), RvpnError> {
        info!("Stopping radmin-vpn service");
        notify_systemd(false, true);

        let (
            bridge_control,
            bridge_thread,
            driver_running,
            driver_thread,
            netsh_relay,
            mut route_manager,
            mut firewall,
        ) = {
            let mut lc = self.lifecycle.lock().unwrap();
            lc.state = ServiceState::Stopping;
            lc.running.store(false, Ordering::SeqCst);
            (
                lc.bridge_control.take(),
                lc.bridge_thread.take(),
                lc.driver_running.take(),
                lc.driver_thread.take(),
                lc.netsh_relay.take(),
                lc.route_manager.take(),
                lc.firewall.take(),
            )
        };

        self.terminate_children()?;
        let _ = self.wine_manager.kill_wine();

        if let Some(r) = netsh_relay {
            r.stop();
        }

        if let Some(r) = driver_running {
            r.store(false, Ordering::SeqCst);
        }
        if let Some(t) = driver_thread {
            let _ = t.join();
        }

        if let Some(c) = bridge_control {
            c.stop();
        }
        if let Some(t) = bridge_thread {
            let _ = t.join();
        }

        if let Some(ref mut fw) = firewall {
            if let Err(e) = fw.close() {
                warn!("firewall close error: {e}");
            }
        }
        if let Some(ref mut rm) = route_manager {
            if let Err(e) = rm.remove_all() {
                warn!("route removal error: {e}");
            }
        }

        let _ = destroy_owned_interface(&self.config.tap_dev);

        self._lock = None;
        {
            let mut lc = self.lifecycle.lock().unwrap();
            lc.state = ServiceState::Stopped;
        }

        info!("radmin-vpn service stopped");
        Ok(())
    }

    pub fn cleanup(&mut self) -> Result<(), RvpnError> {
        info!("Cleaning up radmin-vpn service resources");

        for path in [FIFO_B2D, FIFO_D2B_HIGH, FIFO_D2B_LOW, FIFO_D2B_LEGACY] {
            if let Ok(meta) = std::fs::symlink_metadata(path) {
                if meta.file_type().is_fifo() {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
        for path in [NETSH_CMD_FILE, NETSH_CMD_FILE_PROC, MAC_FILE, FILTER_PATH] {
            let _ = std::fs::remove_file(path);
        }
        let _ = std::fs::remove_file(format!("{NETSH_CMD_FILE}.proc"));
        let _ = std::fs::remove_file(format!("{SERVICE_RUNTIME_DIR}/rvpn.control"));

        let _ = destroy_owned_interface(&self.config.tap_dev);

        info!("Cleanup complete");
        Ok(())
    }

    pub fn shutdown_handle(&self) -> Arc<AtomicBool> {
        self.lifecycle.lock().unwrap().running.clone()
    }

    pub fn request_shutdown(&self) {
        self.lifecycle
            .lock()
            .unwrap()
            .running
            .store(false, Ordering::SeqCst);
    }

    pub fn is_running(&self) -> bool {
        self.lifecycle
            .lock()
            .unwrap()
            .running
            .load(Ordering::SeqCst)
    }

    fn start_radmin_processes(&mut self) -> Result<(), RvpnError> {
        info!("Starting Radmin VPN processes");
        let reuseport_so = self.build_dir_for("rvpn_reuseport.so");
        let _ = self
            .wine_manager
            .boot_wineserver_with_reuseport(reuseport_so.as_deref());

        let dnsfix_so = self.build_dir_for("rvpn_dnsfix.so");
        let child = self.wine_manager.launch_service(dnsfix_so.as_deref())?;
        let pid = child.id();
        self.register_child(pid, "RvControlSvc", child, true);
        Ok(())
    }

    fn register_child(&self, pid: u32, name: &str, child: Child, critical: bool) {
        let lc = self.lifecycle.lock().unwrap();
        lc.children.lock().unwrap().insert(
            pid,
            SupervisedChild {
                name: name.to_string(),
                child: Some(child),
                critical,
            },
        );
    }

    fn build_dir_for(&self, name: &str) -> Option<PathBuf> {
        [
            self.config.build_dir.join(name),
            self.config.build_dir.join(format!("lib{name}")),
        ]
        .into_iter()
        .find(|path| path.exists())
    }

    fn write_mac_file(&self, mac: MacAddress) -> Result<(), RvpnError> {
        let mac_bytes = mac.bytes();
        let hex: String = mac_bytes
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .join(":");

        let mac_file = self.config.mac_file();
        if let Some(parent) = mac_file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(&mac_file, &hex)
            .map_err(|e| RvpnError::Service(format!("write MAC file: {e}")))?;

        std::fs::write(MAC_FILE, mac_bytes)
            .map_err(|e| RvpnError::Service(format!("write raw MAC file: {e}")))?;

        info!("Adapter MAC written: {mac}");
        Ok(())
    }

    fn terminate_children(&mut self) -> Result<(), RvpnError> {
        let child_pids: Vec<u32> = {
            let lc = self.lifecycle.lock().unwrap();
            let children = lc.children.lock().unwrap();
            children.keys().copied().collect()
        };

        for pid in &child_pids {
            let _ = signal::kill(Pid::from_raw(*pid as i32), Signal::SIGTERM);
        }
        std::thread::sleep(Duration::from_secs(2));
        for pid in &child_pids {
            let _ = signal::kill(Pid::from_raw(*pid as i32), Signal::SIGKILL);
        }
        Ok(())
    }

    fn monitor_loop(&mut self) -> Result<(), RvpnError> {
        info!("Entering monitor loop");
        while self.is_running() {
            self.check_children()?;
            watchdog_ping();
            std::thread::sleep(Duration::from_secs(1));
        }
        info!("Monitor loop exiting");
        Ok(())
    }

    fn check_children(&self) -> Result<(), RvpnError> {
        let lc = self.lifecycle.lock().unwrap();
        let mut children = lc.children.lock().unwrap();
        let mut any_critical_died = false;
        let dead_pids: Vec<u32> = children
            .iter_mut()
            .filter_map(|(pid, child)| {
                let exit_status = child
                    .child
                    .as_mut()
                    .and_then(|c| c.try_wait().ok())
                    .flatten();
                match exit_status {
                    Some(status) => {
                        if child.critical {
                            error!(
                                "Critical child {} ({pid}) exited with {status:?}; requesting shutdown so systemd can restart the unit",
                                child.name
                            );
                            any_critical_died = true;
                        } else {
                            warn!("Child {} ({pid}) exited with {status:?}", child.name);
                        }
                        Some(*pid)
                    }
                    None => None,
                }
            })
            .collect();
        for pid in dead_pids {
            children.remove(&pid);
        }
        drop(children);
        if any_critical_died {
            lc.running.store(false, Ordering::SeqCst);
        }
        Ok(())
    }
}

fn generate_random_mac() -> MacAddress {
    use std::time::SystemTime;
    let seed = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0) as u64;
    let mut bytes = [0x02, 0x50, 0xDE, 0xAD, 0xBE, 0xEF];
    bytes[1] = ((seed >> 8) & 0xFF) as u8;
    bytes[2] = ((seed >> 16) & 0xFF) as u8;
    bytes[3] = ((seed >> 24) & 0xFF) as u8;
    bytes[4] = ((seed >> 32) & 0xFF) as u8;
    bytes[5] = ((seed >> 40) & 0xFF) as u8;
    MacAddress::new(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_random_mac_test() {
        let mac = generate_random_mac();
        assert!(mac.is_locally_administered());
        assert!(!mac.is_group());
        assert!(!mac.is_broadcast());
    }
}
