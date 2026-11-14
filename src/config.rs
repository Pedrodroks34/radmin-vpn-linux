use crate::core::constants::*;
use crate::core::types::RvpnError;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

fn default_data_dir() -> PathBuf {
    std::env::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".local/share/radmin-vpn-linux")
}

fn default_wineprefix() -> PathBuf {
    default_data_dir().join("wineprefix")
}

fn resolve_build_dir() -> PathBuf {
    if Path::new("build").is_dir() {
        PathBuf::from("build")
    } else {
        PathBuf::from("/usr/lib/radmin-vpn")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    pub tap_dev: String,

    pub adapter_mac: Option<String>,

    pub vpn_ip: Option<String>,

    pub broadcast_routes: bool,

    pub firewall: bool,

    pub launch_gui: bool,

    pub no_ui: bool,

    pub fix_chat: bool,

    pub filter_ui: bool,

    pub installer: Option<PathBuf>,

    pub update: bool,

    pub wineprefix: PathBuf,

    pub build_dir: PathBuf,

    pub data_dir: PathBuf,

    pub radmin_version: String,

    pub radmin_sha256: Option<String>,

    pub tap_guid: Option<String>,

    pub log_level: String,

    pub json_logs: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            tap_dev: TAP_DEV_NAME.to_string(),
            adapter_mac: None,
            vpn_ip: None,
            broadcast_routes: true,
            firewall: true,
            launch_gui: true,
            no_ui: false,
            fix_chat: true,
            filter_ui: false,
            installer: None,
            update: false,
            wineprefix: default_wineprefix(),
            build_dir: PathBuf::from("build"),
            data_dir: default_data_dir(),
            radmin_version: DEFAULT_RADMIN_VERSION.to_string(),
            radmin_sha256: None,
            tap_guid: None,
            log_level: "info".to_string(),
            json_logs: false,
        }
    }
}

impl Config {
    pub fn from_env() -> Result<Self, RvpnError> {
        let mut cfg = Self::default();

        if std::env::var("RVPN_BUILD_DIR").is_err() {
            cfg.build_dir = resolve_build_dir();
        }

        if let Ok(v) = std::env::var("RVPN_TAP_DEV") {
            cfg.tap_dev = v;
        }
        if let Ok(v) = std::env::var("RVPN_ADAPTER_MAC") {
            cfg.adapter_mac = Some(v);
        }
        if let Ok(v) = std::env::var("RVPN_WINEPREFIX") {
            cfg.wineprefix = PathBuf::from(v);
        }
        if let Ok(v) = std::env::var("RVPN_BUILD_DIR") {
            cfg.build_dir = PathBuf::from(v);
        }
        if let Ok(v) = std::env::var("RVPN_DATA_DIR") {
            cfg.data_dir = PathBuf::from(v);
        }
        if let Ok(v) = std::env::var("RVPN_RADMIN_VERSION") {
            cfg.radmin_version = v;
        }
        if let Ok(v) = std::env::var("RVPN_LOG_LEVEL") {
            cfg.log_level = v;
        }
        if std::env::var("RVPN_JSON_LOGS").is_ok() {
            cfg.json_logs = true;
        }
        if std::env::var("RVPN_NO_UI").is_ok() {
            cfg.no_ui = true;
            cfg.launch_gui = false;
        }
        if std::env::var("RVPN_NO_FIREWALL").is_ok() {
            cfg.firewall = false;
        }
        if std::env::var("RVPN_NO_BROADCAST_ROUTES").is_ok() {
            cfg.broadcast_routes = false;
        }
        if std::env::var("RVPN_NO_FIX_CHAT").is_ok() {
            cfg.fix_chat = false;
        }
        if std::env::var("RVPN_UPDATE").is_ok() {
            cfg.update = true;
        }

        Ok(cfg)
    }

    pub fn radmin_dir(&self) -> PathBuf {
        self.wineprefix
            .join("drive_c/Program Files (x86)/Radmin VPN")
    }

    pub fn service_log(&self) -> PathBuf {
        self.wineprefix
            .join("drive_c/ProgramData/Famatech/Radmin VPN/service.log")
    }

    pub fn mac_file(&self) -> PathBuf {
        self.wineprefix.join("radmin_mac")
    }

    pub fn tap_guid_file(&self) -> PathBuf {
        self.wineprefix.join("radmin_guid")
    }
}

pub fn load_config_file(path: Option<&PathBuf>) -> Result<Config, RvpnError> {
    let mut cfg = Config::default();
    if let Some(path) = path {
        if path.exists() {
            let contents = std::fs::read_to_string(path)
                .map_err(|e| RvpnError::Config(format!("cannot read config file: {e}")))?;
            let file_cfg: Config = toml::from_str(&contents)
                .map_err(|e| RvpnError::Config(format!("cannot parse config file: {e}")))?;

            cfg = file_cfg;
        }
    }
    Ok(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_default() {
        let cfg = Config::default();
        assert_eq!(cfg.tap_dev, "radminvpn0");
        assert!(cfg.firewall);
        assert!(cfg.broadcast_routes);
        assert!(cfg.launch_gui);
        assert!(!cfg.no_ui);
    }

    #[test]
    fn config_from_env() {
        std::env::set_var("RVPN_TAP_DEV", "tap0");
        std::env::set_var("RVPN_NO_UI", "1");
        let cfg = Config::from_env().unwrap();
        assert_eq!(cfg.tap_dev, "tap0");
        assert!(cfg.no_ui);
        std::env::remove_var("RVPN_TAP_DEV");
        std::env::remove_var("RVPN_NO_UI");
    }

    #[test]
    fn config_radmin_dir() {
        let cfg = Config {
            wineprefix: PathBuf::from("/home/user/.local/share/radmin-vpn-linux/wineprefix"),
            ..Config::default()
        };
        let dir = cfg.radmin_dir();
        assert!(dir.to_string_lossy().contains("Radmin VPN"));
    }
}
