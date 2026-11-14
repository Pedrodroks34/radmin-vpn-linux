use std::path::PathBuf;

pub fn runtime_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("RADMIN_VPN_RUNTIME_DIR") {
        return PathBuf::from(p);
    }
    if let Some(xdg) = std::env::var_os("XDG_RUNTIME_DIR") {
        return PathBuf::from(xdg).join("radmin-vpn");
    }
    PathBuf::from("/run/radmin-vpn")
}

pub fn state_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("RADMIN_VPN_STATE_DIR") {
        return PathBuf::from(p);
    }
    if let Some(xdg) = std::env::var_os("XDG_STATE_HOME") {
        return PathBuf::from(xdg).join("radmin-vpn");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".local/state/radmin-vpn");
    }
    PathBuf::from("/var/lib/radmin-vpn")
}

pub fn lock_file() -> PathBuf {
    runtime_dir().join("service.lock")
}

pub fn control_socket() -> PathBuf {
    runtime_dir().join("rvpn.control")
}

pub fn mac_state_file() -> PathBuf {
    state_dir().join("mac")
}

pub fn tap_state_file() -> PathBuf {
    state_dir().join("tap.state")
}

pub fn ensure_dir(path: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)
}
