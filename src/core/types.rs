use crate::core::constants::ETH_FRAME_MIN;
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MacAddress(pub [u8; 6]);

impl MacAddress {
    pub const fn new(bytes: [u8; 6]) -> Self {
        Self(bytes)
    }

    pub const fn bytes(&self) -> [u8; 6] {
        self.0
    }

    pub fn is_broadcast(&self) -> bool {
        self.0 == [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]
    }

    pub fn is_group(&self) -> bool {
        (self.0[0] & 0x01) != 0
    }

    pub fn is_locally_administered(&self) -> bool {
        (self.0[0] & 0x02) != 0
    }

    pub fn parse(s: &str) -> Result<Self, MacAddressError> {
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() != 6 {
            return Err(MacAddressError::InvalidFormat);
        }
        let mut bytes = [0u8; 6];
        for (i, part) in parts.iter().enumerate() {
            bytes[i] = u8::from_str_radix(part, 16).map_err(|_| MacAddressError::InvalidFormat)?;
        }
        Ok(Self(bytes))
    }
}

impl fmt::Display for MacAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            self.0[0], self.0[1], self.0[2], self.0[3], self.0[4], self.0[5]
        )
    }
}

impl From<[u8; 6]> for MacAddress {
    fn from(bytes: [u8; 6]) -> Self {
        Self(bytes)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum MacAddressError {
    #[error("invalid MAC address format")]
    InvalidFormat,
}

#[derive(Clone, Debug)]
pub struct EthernetFrame {
    pub data: Vec<u8>,
}

impl EthernetFrame {
    pub fn new(data: Vec<u8>) -> Self {
        Self { data }
    }

    pub fn dst_mac(&self) -> Option<MacAddress> {
        if self.data.len() < 6 {
            return None;
        }
        let mut bytes = [0u8; 6];
        bytes.copy_from_slice(&self.data[0..6]);
        Some(MacAddress(bytes))
    }

    pub fn src_mac(&self) -> Option<MacAddress> {
        if self.data.len() < 12 {
            return None;
        }
        let mut bytes = [0u8; 6];
        bytes.copy_from_slice(&self.data[6..12]);
        Some(MacAddress(bytes))
    }

    pub fn ethertype(&self) -> Option<u16> {
        if self.data.len() < 14 {
            return None;
        }
        Some(u16::from_be_bytes([self.data[12], self.data[13]]))
    }

    pub fn is_ipv4(&self) -> bool {
        self.ethertype() == Some(0x0800)
    }

    pub fn is_arp(&self) -> bool {
        self.ethertype() == Some(0x0806)
    }

    pub fn is_ipv6(&self) -> bool {
        self.ethertype() == Some(0x86dd)
    }

    pub fn src_ipv4(&self) -> Option<[u8; 4]> {
        if self.data.len() < 30 || !self.is_ipv4() {
            return None;
        }
        let mut bytes = [0u8; 4];
        bytes.copy_from_slice(&self.data[26..30]);
        Some(bytes)
    }

    pub fn dst_ipv4(&self) -> Option<[u8; 4]> {
        if self.data.len() < 34 || !self.is_ipv4() {
            return None;
        }
        let mut bytes = [0u8; 4];
        bytes.copy_from_slice(&self.data[30..34]);
        Some(bytes)
    }

    pub fn ip_protocol(&self) -> Option<u8> {
        if self.data.len() < 24 || !self.is_ipv4() {
            return None;
        }
        Some(self.data[23])
    }

    pub fn is_ipv4_icmp(&self) -> bool {
        self.is_ipv4() && self.ip_protocol() == Some(1)
    }

    pub fn is_ipv4_tcp(&self) -> bool {
        self.is_ipv4() && self.ip_protocol() == Some(6)
    }

    pub fn is_ipv4_udp(&self) -> bool {
        self.is_ipv4() && self.ip_protocol() == Some(17)
    }

    pub fn ipv4_ihl(&self) -> Option<usize> {
        if self.data.len() < 15 || !self.is_ipv4() {
            return None;
        }
        let ihl = (self.data[14] & 0x0F) as usize * 4;
        if ihl < 20 {
            return None;
        }
        Some(ihl)
    }

    pub fn is_minecraft_mcast(&self) -> bool {
        if !self.is_ipv4_udp() {
            return false;
        }
        let dst = match self.dst_ipv4() {
            Some(d) => d,
            None => return false,
        };
        if dst != [224, 0, 2, 60] {
            return false;
        }

        let ihl = match self.ipv4_ihl() {
            Some(i) => i,
            None => return false,
        };
        if self.data.len() < 14 + ihl + 8 {
            return false;
        }
        let udp_hdr = &self.data[14 + ihl..14 + ihl + 8];
        let _dst_port = u16::from_be_bytes([udp_hdr[2], udp_hdr[3]]);
        true
    }
}

impl fmt::Display for EthernetFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "EthernetFrame(len={})", self.data.len())
    }
}

#[derive(Clone, Debug)]
pub struct TlvFrame {
    pub dest_mac_prefix: u32,

    pub padded_len: u32,

    pub data: Vec<u8>,
}

impl TlvFrame {
    pub fn encode(frame: &EthernetFrame, setup_mode: u32) -> Vec<u8> {
        let frame_len = frame.data.len();
        let padded_len = if frame_len < ETH_FRAME_MIN {
            ETH_FRAME_MIN
        } else {
            frame_len
        };

        let header_size = if setup_mode >= 2 { 8 } else { 4 };
        let mut buf = Vec::with_capacity(header_size + padded_len);

        if setup_mode >= 2 {
            let mac_prefix = if frame.data.len() >= 4 {
                let raw = u32::from_le_bytes([
                    frame.data[0],
                    frame.data[1],
                    frame.data[2],
                    frame.data[3],
                ]);
                raw.swap_bytes()
            } else {
                0
            };
            buf.extend_from_slice(&mac_prefix.to_le_bytes());
        }

        buf.extend_from_slice(&(padded_len as u32).to_le_bytes());
        buf.extend_from_slice(&frame.data);
        if frame_len < ETH_FRAME_MIN {
            buf.extend(std::iter::repeat_n(0u8, ETH_FRAME_MIN - frame_len));
        }

        buf
    }

    pub fn decode(buf: &[u8], setup_mode: u32) -> Option<(EthernetFrame, usize)> {
        let header_size = if setup_mode >= 2 { 8 } else { 4 };
        if buf.len() < header_size {
            return None;
        }

        let offset = if setup_mode >= 2 { 4 } else { 0 };
        let padded_len = u32::from_le_bytes([
            buf[offset],
            buf[offset + 1],
            buf[offset + 2],
            buf[offset + 3],
        ]) as usize;

        let needed = header_size + padded_len;
        if buf.len() < needed {
            return None;
        }

        let frame_data = &buf[header_size..header_size + frame_len_min(padded_len)];

        let real_len = padded_len.min(frame_data.len());

        Some((EthernetFrame::new(frame_data[..real_len].to_vec()), needed))
    }
}

fn frame_len_min(padded_len: usize) -> usize {
    padded_len
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FilterConfig {
    pub block_ips_enabled: bool,
    pub blocked_ips: Vec<String>,
    pub block_macs_enabled: bool,
    pub blocked_macs: Vec<String>,
    pub block_broadcast_enabled: bool,
    pub broadcast_block_ips: Vec<String>,
}

impl FilterConfig {
    pub fn is_empty(&self) -> bool {
        !self.block_ips_enabled && !self.block_macs_enabled && !self.block_broadcast_enabled
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServiceState {
    Stopped,
    Starting,
    Running,
    Stopping,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildStatus {
    Running,
    Exited(i32),
    Signaled(i32),
    Crashed,
}

pub type RvpnResult<T> = Result<T, RvpnError>;

#[derive(Debug, thiserror::Error)]
pub enum RvpnError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("MAC address error: {0}")]
    MacAddress(#[from] MacAddressError),

    #[error("TAP device error: {0}")]
    Tap(String),

    #[error("bridge error: {0}")]
    Bridge(String),

    #[error("driver shim error: {0}")]
    Driver(String),

    #[error("Wine error: {0}")]
    Wine(String),

    #[error("registry error: {0}")]
    Registry(String),

    #[error("firewall error: {0}")]
    Firewall(String),

    #[error("route error: {0}")]
    Route(String),

    #[error("service error: {0}")]
    Service(String),

    #[error("configuration error: {0}")]
    Config(String),

    #[error("protocol error: {0}")]
    Protocol(String),

    #[error("not initialized: {0}")]
    NotInitialized(String),

    #[error("permission denied: {0}")]
    PermissionDenied(String),

    #[error("not supported: {0}")]
    NotSupported(String),

    #[error("timeout: {0}")]
    Timeout(String),
}
