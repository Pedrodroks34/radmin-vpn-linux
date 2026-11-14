pub const TAP_DEV_NAME: &str = "radminvpn0";

pub const FIFO_B2D: &str = "/tmp/rvpn_b2d";
pub const FIFO_D2B_HIGH: &str = "/tmp/rvpn_d2b_high";
pub const FIFO_D2B_LOW: &str = "/tmp/rvpn_d2b_low";
pub const FIFO_D2B_LEGACY: &str = "/tmp/rvpn_d2b";

pub const NETSH_CMD_FILE: &str = "/tmp/radmin_netsh_cmd";
pub const NETSH_CMD_FILE_PROC: &str = "/tmp/radmin_netsh_cmd.proc";

pub const MAC_FILE: &str = "/tmp/rvpn_mac";

pub const DRIVER_LOG: &str = "/tmp/radmin_driver.log";

pub const FILTER_PATH: &str = "/tmp/rvpn_filters.json";

pub const DEFAULT_MTU: u16 = 1500;

pub const FRAME_MAX: usize = 1500 + 14 + 4;

pub const ETH_FRAME_MAX: usize = FRAME_MAX;

pub const RELAY_BUF_MAX: usize = 2048;

pub const ETH_FRAME_MIN: usize = 60;

pub const IOCTL_RVPN_VERSION: u32 = 0x0022c004;
pub const IOCTL_RVPN_STATUS: u32 = 0x00224018;
pub const IOCTL_RVPN_CONNECT: u32 = 0x00224020;
pub const IOCTL_RVPN_SETLINK: u32 = 0x00224010;
pub const IOCTL_RVPN_SETUP: u32 = 0x0022801c;
pub const IOCTL_RVPN_PEERMAC: u32 = 0x00228014;
pub const IOCTL_RVPN_SETMAC: u32 = 0x00228024;

pub const PEER_NETWORK: &str = "26.0.0.0/8";

pub const TAP_IP_PREFIX: u8 = 8;

pub const NETSH_RELAY_POLL_SECS: f64 = 0.1;

pub const SERVICE_READY_TIMEOUT_SECS: u64 = 30;

pub const DEFAULT_WINEPREFIX: &str = "$HOME/.local/share/radmin-vpn-linux/wineprefix";

pub const DEFAULT_BUILD_DIR: &str = "./build";

pub const DEFAULT_RADMIN_VERSION: &str = "2.1.4952.2";

pub const RADMIN_INSTALLER_URL_TEMPLATE: &str =
    "https://download.radmin-vpn.com/download/files/Radmin_VPN_{version}.exe";

pub const RADMIN_SHA256_PINNED: &str =
    "717a87141fd703bb58bb730d3282e31055bf4ea9c319177ade9b793974a7d5a4";

pub const WIN_DEVICE_NAME: &str = "\\\\.\\RVPNNETMP";

pub const NT_DEVICE_NAME: &str = "\\Device\\RVPNNETMP";

pub const NT_DOS_DEVICE_NAME: &str = "\\DosDevices\\RVPNNETMP";

pub const WINE_FIFO_B2D: &str = "\\??\\Z:\\tmp\\rvpn_b2d";
pub const WINE_FIFO_D2B_HIGH: &str = "\\??\\Z:\\tmp\\rvpn_d2b_high";
pub const WINE_FIFO_D2B_LOW: &str = "\\??\\Z:\\tmp\\rvpn_d2b_low";
pub const WINE_MAC_FILE: &str = "\\??\\Z:\\tmp\\rvpn_mac";

pub const REG_KEY_NET_CLASS: &str =
    "HKLM\\SYSTEM\\CurrentControlSet\\Control\\Class\\{4d36e972-e325-11ce-bfc1-08002be10318}\\0099";
pub const REG_KEY_NET_CONNECTION: &str =
    "HKLM\\SYSTEM\\CurrentControlSet\\Control\\Network\\{4d36e972-e325-11ce-bfc1-08002be10318}";
pub const REG_KEY_FAMATECH_FIREWALL: &str =
    "HKLM\\Software\\Wow6432Node\\Famatech\\RadminVPN\\1.0\\Firewall";
pub const REG_KEY_FAMATECH_REGISTRATION: &str =
    "HKLM\\SOFTWARE\\Famatech\\RadminVPN\\1.0\\Registration";
pub const REG_KEY_RVPNNETMP_SERVICE: &str = "HKLM\\SYSTEM\\CurrentControlSet\\Services\\rvpnnetmp";
pub const REG_KEY_RVCONTROLSVC_SERVICE: &str =
    "HKLM\\SYSTEM\\CurrentControlSet\\Services\\RvControlSvc";
pub const REG_KEY_WINE_DLLOVERRIDES: &str = "HKCU\\Software\\Wine\\DllOverrides";

pub const TAP_DESC_WIDE: &str = "radminvpn0";
pub const RADMIN_DESC_WIDE: &str = "Famatech Radmin VPN Ethernet Adapter";
pub const RADMIN_FRIENDLY_WIDE: &str = "Radmin VPN";

pub const DEFAULT_ROUTE_PROBE: &str = "8.8.8.8";

pub const MINECRAFT_MCAST_GROUP: &str = "224.0.2.60";

pub const MINECRAFT_MCAST_MAC: &str = "01:00:5e:00:02:3c";

pub const MINECRAFT_BCAST_VPN: &str = "26.255.255.255";
pub const MINECRAFT_BCAST_ALL: &str = "255.255.255.255";
pub const MINECRAFT_PORT: u16 = 4445;

pub const DEFAULT_TAP_GUID: &str = "{0000000B-0000-0000-0000-4E6574446576}";

pub const SYSTEMD_UNIT_NAME: &str = "radmin-vpn.service";

pub const FW_TAG: &str = "radmin-vpn-linux";

pub const STATUS_MEDIA_CONNECTED: u32 = 1;

pub const CONNECT_MEDIA_CONNECTED: u8 = 0x20;
