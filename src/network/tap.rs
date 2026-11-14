use crate::core::constants::*;
use crate::core::types::MacAddress;
use crate::core::types::RvpnError;
use nix::errno::Errno;
use nix::unistd::close;
use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::{AsRawFd, RawFd};
use std::path::PathBuf;
use tracing::{debug, info, warn};

const TUNSETIFF: libc::c_ulong = 0x4004_54ca;
const TUNSETPERSIST: libc::c_ulong = 0x4004_54cb;

const SIOCSIFHWADDR: libc::c_ulong = 0x8924;
const SIOCSIFMTU: libc::c_ulong = 0x8922;
const SIOCGIFFLAGS: libc::c_ulong = 0x8913;
const SIOCSIFFLAGS: libc::c_ulong = 0x8914;
const SIOCSIFNAME: libc::c_ulong = 0x8923;
const SIOCADDMULTI: libc::c_ulong = 0x8931;

const IFF_UP: libc::c_short = 0x0001;
const IFF_TAP: libc::c_uint = 0x0002;
const IFF_NO_PI: libc::c_uint = 0x1000;

pub const RUNTIME_DIR: &str = "/run/radmin-vpn";

pub const STATE_DIR: &str = "/var/lib/radmin-vpn";

pub const TAP_STATE_FILE: &str = "/var/lib/radmin-vpn/tap.state";

fn tap_state_file() -> PathBuf {
    std::env::var_os("RVPN_TAP_STATE_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(TAP_STATE_FILE))
}

fn write_state(name: &str) -> std::io::Result<()> {
    let path = tap_state_file();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut f = std::fs::File::create(&path)?;
    writeln!(f, "{name}")?;
    f.sync_all()
}

fn read_state() -> Option<String> {
    std::fs::read_to_string(tap_state_file())
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn owned_interface() -> Option<String> {
    read_state()
}

pub fn clear_owned_interface() {
    let _ = std::fs::remove_file(tap_state_file());
}

pub fn destroy_owned_interface(name: &str) -> Result<bool, RvpnError> {
    let recorded = read_state();
    if recorded.as_deref() != Some(name) {
        debug!("interface {name} not recorded as owned; skipping delete");
        return Ok(false);
    }
    if !iface_exists(name) {
        clear_owned_interface();
        return Ok(false);
    }

    let status = std::process::Command::new("ip")
        .args(["link", "delete", name])
        .status()
        .map_err(|e| RvpnError::Tap(format!("ip link delete failed: {e}")))?;

    if status.success() {
        clear_owned_interface();
        info!("Deleted owned TAP interface {name}");
        Ok(true)
    } else {
        Err(RvpnError::Tap(format!("ip link delete {name} failed")))
    }
}

fn iface_exists(name: &str) -> bool {
    std::path::Path::new(&format!("/sys/class/net/{name}")).exists()
}

fn set_ifr_name(ifr: &mut libc::ifreq, name: &str) {
    let bytes = name.as_bytes();
    let len = bytes.len().min(libc::IFNAMSIZ - 1);
    let as_i8: &[i8] = unsafe { std::slice::from_raw_parts(bytes.as_ptr() as *const i8, len) };
    ifr.ifr_name[..len].copy_from_slice(as_i8);
}

struct IoctlSocket(RawFd);

impl IoctlSocket {
    fn raw(&self) -> RawFd {
        self.0
    }
}

impl Drop for IoctlSocket {
    fn drop(&mut self) {
        let _ = close(self.0);
    }
}

pub struct TapInterface {
    name: String,
    fd: Option<RawFd>,
    mac: MacAddress,
    mtu: u16,
    owned: bool,
}

impl TapInterface {
    pub fn is_available() -> bool {
        std::path::Path::new("/dev/net/tun").exists()
    }

    pub fn has_cap_net_admin() -> bool {
        let Ok(status) = std::fs::read_to_string("/proc/self/status") else {
            return false;
        };
        let Some(line) = status.lines().find(|l| l.starts_with("CapEff:")) else {
            return false;
        };
        let Ok(bits) = u64::from_str_radix(line.trim_start_matches("CapEff:").trim(), 16) else {
            return false;
        };

        bits & (1 << 12) != 0
    }

    pub fn create(
        name: &str,
        mac: MacAddress,
        mtu: u16,
        create: bool,
        persist: bool,
    ) -> Result<Self, RvpnError> {
        if name.is_empty() || name.len() >= libc::IFNAMSIZ {
            return Err(RvpnError::Tap(format!("invalid TAP name: {name:?}")));
        }

        let existed_before = iface_exists(name);
        let recorded = read_state();
        let recorded_owned = recorded.as_deref() == Some(name);
        let owned = if create {
            match (recorded_owned, existed_before) {
                (true, true) => true,
                (false, false) => true,
                (true, false) => true,
                (false, true) => {
                    return Err(RvpnError::Tap(format!(
                        "interface {name} already exists and is not owned by radmin-vpn; \
                         refusing to modify it"
                    )));
                }
            }
        } else {
            if !existed_before {
                return Err(RvpnError::Tap(format!("interface {name} does not exist")));
            }

            recorded_owned
        };

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_CLOEXEC)
            .open("/dev/net/tun")
            .map_err(|e| RvpnError::Tap(format!("cannot open /dev/net/tun: {e}")))?;

        let mut ifr: libc::ifreq = unsafe { std::mem::zeroed() };
        ifr.ifr_ifru.ifru_flags = (IFF_TAP | IFF_NO_PI) as i16;
        set_ifr_name(&mut ifr, name);

        let ret = unsafe { libc::ioctl(file.as_raw_fd(), TUNSETIFF, &ifr) };
        if ret < 0 {
            return Err(RvpnError::Tap(format!(
                "TUNSETIFF failed for {name}: {}",
                Errno::last()
            )));
        }

        if persist {
            let ret = unsafe { libc::ioctl(file.as_raw_fd(), TUNSETPERSIST, 1 as libc::c_int) };
            if ret < 0 {
                warn!("TUNSETPERSIST failed for {name}: {}", Errno::last());
            } else {
                debug!("TUNSETPERSIST set on {name}");
            }
        }

        if owned && create {
            write_state(name)
                .map_err(|e| RvpnError::Tap(format!("cannot record TAP ownership: {e}")))?;
        }

        let fd = file.as_raw_fd();
        std::mem::forget(file);

        let fd_check = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        if fd_check < 0 {
            let err = std::io::Error::last_os_error();
            let _ = unsafe { libc::close(fd) };
            return Err(RvpnError::Tap(format!(
                "TAP fd {fd} invalid after TUNSETIFF: {err}"
            )));
        }

        info!("TAP device {name} ready (fd={fd}, owned={owned})");

        Ok(Self {
            name: name.to_string(),
            fd: Some(fd),
            mac,
            mtu,
            owned,
        })
    }

    pub fn set_name(&mut self, new_name: &str) -> Result<(), RvpnError> {
        let socket = self.socket_fd()?;
        let fd = socket.raw();
        let mut ifr: libc::ifreq = unsafe { std::mem::zeroed() };
        set_ifr_name(&mut ifr, &self.name);
        let bytes = new_name.as_bytes();
        let len = bytes.len().min(libc::IFNAMSIZ - 1);
        let as_i8: &[i8] = unsafe { std::slice::from_raw_parts(bytes.as_ptr() as *const i8, len) };
        unsafe {
            ifr.ifr_ifru.ifru_newname[..len].copy_from_slice(as_i8);
        }

        let ret = unsafe { libc::ioctl(fd, SIOCSIFNAME, &ifr) };
        if ret < 0 {
            return Err(RvpnError::Tap(format!(
                "SIOCSIFNAME {new_name}: {}",
                Errno::last()
            )));
        }
        self.name = new_name.to_string();
        Ok(())
    }

    pub fn set_mac(&self, mac: MacAddress) -> Result<(), RvpnError> {
        let socket = self.socket_fd()?;
        let fd = socket.raw();
        let mut ifr: libc::ifreq = unsafe { std::mem::zeroed() };
        set_ifr_name(&mut ifr, &self.name);
        let mut sa: libc::sockaddr = unsafe { std::mem::zeroed() };
        sa.sa_family = libc::ARPHRD_ETHER as libc::sa_family_t;
        ifr.ifr_ifru.ifru_hwaddr = sa;
        unsafe {
            for (dst, src) in ifr.ifr_ifru.ifru_hwaddr.sa_data.iter_mut().zip(mac.bytes()) {
                *dst = src as libc::c_char;
            }
        }

        let ret = unsafe { libc::ioctl(fd, SIOCSIFHWADDR, &ifr) };
        if ret < 0 {
            let err = Errno::last();

            if err != Errno::EADDRNOTAVAIL {
                return Err(RvpnError::Tap(format!("SIOCSIFHWADDR {mac}: {err}")));
            }
        }
        Ok(())
    }

    pub fn set_mtu(&self, mtu: u16) -> Result<(), RvpnError> {
        let socket = self.socket_fd()?;
        let fd = socket.raw();
        let mut ifr: libc::ifreq = unsafe { std::mem::zeroed() };
        set_ifr_name(&mut ifr, &self.name);
        ifr.ifr_ifru.ifru_mtu = mtu as libc::c_int;

        let ret = unsafe { libc::ioctl(fd, SIOCSIFMTU, &ifr) };
        if ret < 0 {
            return Err(RvpnError::Tap(format!(
                "SIOCSIFMTU {mtu}: {}",
                Errno::last()
            )));
        }
        Ok(())
    }

    pub fn set_up(&self) -> Result<(), RvpnError> {
        self.change_flags(|flags| flags | IFF_UP)
    }

    pub fn set_multicast(&self) -> Result<(), RvpnError> {
        Ok(())
    }

    pub fn set_allmulticast(&self) -> Result<(), RvpnError> {
        Ok(())
    }

    fn change_flags<F: FnOnce(libc::c_short) -> libc::c_short>(
        &self,
        change: F,
    ) -> Result<(), RvpnError> {
        let socket = self.socket_fd()?;
        let fd = socket.raw();
        let mut ifr: libc::ifreq = unsafe { std::mem::zeroed() };
        set_ifr_name(&mut ifr, &self.name);
        let ret = unsafe { libc::ioctl(fd, SIOCGIFFLAGS, &mut ifr) };
        if ret < 0 {
            return Err(RvpnError::Tap(format!(
                "SIOCGIFFLAGS {}: {}",
                self.name,
                Errno::last()
            )));
        }
        let flags = unsafe { ifr.ifr_ifru.ifru_flags };
        ifr.ifr_ifru.ifru_flags = change(flags);
        let ret = unsafe { libc::ioctl(fd, SIOCSIFFLAGS, &ifr) };
        if ret < 0 {
            return Err(RvpnError::Tap(format!(
                "SIOCSIFFLAGS {}: {}",
                self.name,
                Errno::last()
            )));
        }
        Ok(())
    }

    pub fn add_maddr(&self, addr: &str) -> Result<(), RvpnError> {
        let mac = MacAddress::parse(addr)
            .map_err(|e| RvpnError::Tap(format!("invalid multicast MAC {addr}: {e}")))?;
        let socket = self.socket_fd()?;
        let fd = socket.raw();
        let mut ifr: libc::ifreq = unsafe { std::mem::zeroed() };
        set_ifr_name(&mut ifr, &self.name);
        let mut sa: libc::sockaddr = unsafe { std::mem::zeroed() };
        sa.sa_family = libc::ARPHRD_ETHER as libc::sa_family_t;
        for (dst, src) in sa.sa_data.iter_mut().zip(mac.bytes()) {
            *dst = src as libc::c_char;
        }
        ifr.ifr_ifru.ifru_hwaddr = sa;
        let ret = unsafe { libc::ioctl(fd, SIOCADDMULTI, &ifr) };
        if ret < 0 {
            let err = Errno::last();
            if err != Errno::EADDRINUSE {
                warn!("SIOCADDMULTI {addr} on {}: {err}", self.name);
            }
        }
        Ok(())
    }

    pub fn set_sysctl(&self, key: &str, value: &str) -> Result<(), RvpnError> {
        let path = format!("/proc/sys/net/ipv4/conf/{}/{key}", self.name);
        match std::fs::write(&path, value) {
            Ok(()) => Ok(()),
            Err(e) => {
                warn!("sysctl {path}={value} failed (non-fatal): {e}");
                Ok(())
            }
        }
    }

    pub fn nmcli_unmanaged(&self) -> Result<(), RvpnError> {
        match std::process::Command::new("nmcli")
            .args(["device", "set", &self.name, "managed", "no"])
            .status()
        {
            Ok(s) if s.success() => Ok(()),
            Ok(_) => {
                warn!("nmcli device set managed no failed (non-fatal)");
                Ok(())
            }
            Err(e) => {
                debug!("nmcli not available: {e}");
                Ok(())
            }
        }
    }

    fn socket_fd(&self) -> Result<IoctlSocket, RvpnError> {
        let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0) };
        if fd < 0 {
            return Err(RvpnError::Tap(format!(
                "socket(AF_INET) failed: {}",
                std::io::Error::last_os_error()
            )));
        }
        Ok(IoctlSocket(fd))
    }

    pub fn read_frame(&self, buf: &mut [u8]) -> Result<usize, RvpnError> {
        let fd = self
            .fd
            .ok_or_else(|| RvpnError::Tap("no TAP fd".to_string()))?;
        let ret = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
        if ret < 0 {
            return Err(RvpnError::Tap(format!(
                "read TAP failed: {}",
                std::io::Error::last_os_error()
            )));
        }
        Ok(ret as usize)
    }

    pub fn write_frame(&self, data: &[u8]) -> Result<(), RvpnError> {
        let fd = self
            .fd
            .ok_or_else(|| RvpnError::Tap("no TAP fd".to_string()))?;
        let ret = unsafe { libc::write(fd, data.as_ptr() as *const libc::c_void, data.len()) };
        if ret < 0 {
            return Err(RvpnError::Tap(format!(
                "write TAP failed: {}",
                std::io::Error::last_os_error()
            )));
        }
        Ok(())
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn mac(&self) -> MacAddress {
        self.mac
    }

    pub fn mtu(&self) -> u16 {
        self.mtu
    }

    pub fn owned(&self) -> bool {
        self.owned
    }

    pub fn fd(&self) -> RawFd {
        self.fd.unwrap_or(-1)
    }

    pub fn exists(&self) -> bool {
        iface_exists(&self.name)
    }

    pub fn destroy(&mut self) -> Result<(), RvpnError> {
        if !self.owned {
            debug!("Not destroying {} - not owned", self.name);
            return Ok(());
        }
        if !self.exists() {
            clear_owned_interface();
            return Ok(());
        }

        let status = std::process::Command::new("ip")
            .args(["link", "delete", &self.name])
            .status()
            .map_err(|e| RvpnError::Tap(format!("ip link delete failed: {e}")))?;

        if status.success() {
            info!("Deleted TAP interface {}", self.name);
            self.owned = false;
            clear_owned_interface();
            Ok(())
        } else {
            Err(RvpnError::Tap(format!(
                "ip link delete {} failed",
                self.name
            )))
        }
    }
}

impl Drop for TapInterface {
    fn drop(&mut self) {
        if let Some(fd) = self.fd.take() {
            let _ = close(fd);
        }
    }
}

pub fn configure_tap(
    name: &str,
    mac: MacAddress,
    mtu: u16,
    create: bool,
) -> Result<TapInterface, RvpnError> {
    let tap = TapInterface::create(name, mac, mtu, create, true)?;

    tap.set_mac(mac)?;
    tap.set_mtu(mtu)?;
    tap.set_up()?;
    tap.add_maddr(MINECRAFT_MCAST_MAC)?;
    tap.set_sysctl("rp_filter", "0")?;
    tap.set_sysctl("accept_local", "1")?;
    tap.nmcli_unmanaged()?;
    Ok(tap)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tap_availability_check() {
        let _ = TapInterface::is_available();
        let _ = TapInterface::has_cap_net_admin();
    }

    #[test]
    fn ownership_state_roundtrip() {
        let unique = format!("radmin-vpn-test-tap-{}-{}.state", std::process::id(), 1);
        let temp_path = std::env::temp_dir().join(unique);
        std::env::set_var("RVPN_TAP_STATE_FILE", &temp_path);

        write_state("rvpn-test0").expect("state file should be writable in temp dir");
        assert_eq!(owned_interface().as_deref(), Some("rvpn-test0"));
        clear_owned_interface();
        assert_eq!(owned_interface(), None);

        std::env::remove_var("RVPN_TAP_STATE_FILE");
        let _ = std::fs::remove_file(&temp_path);
    }

    #[test]
    fn mac_address_parse() {
        let mac = MacAddress::parse("02:50:de:ad:be:ef").unwrap();
        assert_eq!(mac.bytes(), [0x02, 0x50, 0xde, 0xad, 0xbe, 0xef]);
        assert!(!mac.is_broadcast());
        assert!(!mac.is_group());
        assert!(mac.is_locally_administered());

        let bcast = MacAddress::parse("ff:ff:ff:ff:ff:ff").unwrap();
        assert!(bcast.is_broadcast());
        assert!(bcast.is_group());

        let mcast = MacAddress::parse("01:00:5e:00:00:01").unwrap();
        assert!(mcast.is_group());
    }

    #[test]
    fn mac_address_display() {
        let mac = MacAddress::new([0x02, 0x50, 0xde, 0xad, 0xbe, 0xef]);
        assert_eq!(mac.to_string(), "02:50:de:ad:be:ef");
    }
}
