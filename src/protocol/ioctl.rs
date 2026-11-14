use crate::core::constants::*;
use crate::core::types::MacAddress;
use crate::core::types::RvpnError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum RvpnIoctl {
    Version = 0,
    Status = 1,
    Connect = 2,
    SetLink = 3,
    Setup = 4,
    PeerMac = 5,
    SetMac = 6,
    Unknown(u32),
}

impl From<u32> for RvpnIoctl {
    fn from(code: u32) -> Self {
        match code {
            IOCTL_RVPN_VERSION => RvpnIoctl::Version,
            IOCTL_RVPN_STATUS => RvpnIoctl::Status,
            IOCTL_RVPN_CONNECT => RvpnIoctl::Connect,
            IOCTL_RVPN_SETLINK => RvpnIoctl::SetLink,
            IOCTL_RVPN_SETUP => RvpnIoctl::Setup,
            IOCTL_RVPN_PEERMAC => RvpnIoctl::PeerMac,
            IOCTL_RVPN_SETMAC => RvpnIoctl::SetMac,
            c => RvpnIoctl::Unknown(c),
        }
    }
}

impl RvpnIoctl {
    pub fn code(self) -> u32 {
        match self {
            RvpnIoctl::Version => IOCTL_RVPN_VERSION,
            RvpnIoctl::Status => IOCTL_RVPN_STATUS,
            RvpnIoctl::Connect => IOCTL_RVPN_CONNECT,
            RvpnIoctl::SetLink => IOCTL_RVPN_SETLINK,
            RvpnIoctl::Setup => IOCTL_RVPN_SETUP,
            RvpnIoctl::PeerMac => IOCTL_RVPN_PEERMAC,
            RvpnIoctl::SetMac => IOCTL_RVPN_SETMAC,
            RvpnIoctl::Unknown(c) => c,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            RvpnIoctl::Version => "IOCTL_RVPN_VERSION",
            RvpnIoctl::Status => "IOCTL_RVPN_STATUS",
            RvpnIoctl::Connect => "IOCTL_RVPN_CONNECT",
            RvpnIoctl::SetLink => "IOCTL_RVPN_SETLINK",
            RvpnIoctl::Setup => "IOCTL_RVPN_SETUP",
            RvpnIoctl::PeerMac => "IOCTL_RVPN_PEERMAC",
            RvpnIoctl::SetMac => "IOCTL_RVPN_SETMAC",
            RvpnIoctl::Unknown(c) => format!("IOCTL_UNKNOWN(0x{c:08x})").leak(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct VersionResponse {
    pub status: u32,
    pub mac: [u8; 6],
}

impl VersionResponse {
    pub fn success(mac: [u8; 6]) -> Self {
        Self { status: 0, mac }
    }

    pub fn mismatch() -> Self {
        Self {
            status: 1,
            mac: [0; 6],
        }
    }

    pub fn encode(&self) -> [u8; 12] {
        let mut buf = [0u8; 12];
        buf[0..4].copy_from_slice(&self.status.to_le_bytes());
        buf[4..10].copy_from_slice(&self.mac);

        buf
    }
}

#[derive(Debug, Clone)]
pub struct StatusResponse {
    pub status: u32,
}

impl StatusResponse {
    pub fn connected() -> Self {
        Self {
            status: STATUS_MEDIA_CONNECTED,
        }
    }

    pub fn encode(&self) -> [u8; 4] {
        self.status.to_le_bytes()
    }
}

#[derive(Debug, Clone)]
pub struct ConnectResponse {
    pub media_connected: u8,
}

impl ConnectResponse {
    pub fn connected() -> Self {
        Self {
            media_connected: CONNECT_MEDIA_CONNECTED,
        }
    }

    pub fn encode(&self) -> [u8; 1] {
        [self.media_connected]
    }
}

#[derive(Debug, Clone)]
pub struct SetLinkResponse;

impl SetLinkResponse {
    pub fn encode() -> [u8; 0xB8] {
        [0u8; 0xB8]
    }
}

#[derive(Debug, Clone)]
pub struct SetMacResponse;

impl SetMacResponse {
    pub fn encode(&self) -> [u8; 1] {
        [0u8; 1]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum SetupMode {
    Legacy = 0,
    Mode1 = 1,
    TlvMode = 2,
}

impl From<u32> for SetupMode {
    fn from(mode: u32) -> Self {
        if mode >= 2 {
            SetupMode::TlvMode
        } else {
            SetupMode::Legacy
        }
    }
}

#[derive(Debug, Clone)]
pub struct PeerMacEntry {
    pub mac: MacAddress,

    pub handle_id: u64,
}

impl PeerMacEntry {
    pub fn new(mac: MacAddress, handle_id: u64) -> Self {
        Self { mac, handle_id }
    }
}

#[derive(Debug, Clone)]
pub enum IoctlResponse {
    Version(VersionResponse),
    Status(StatusResponse),
    Connect(ConnectResponse),
    SetLink(SetLinkResponse),
    SetupAck,
    PeerMacAck,
    SetMac(SetMacResponse),
    UnknownAck,
}

impl IoctlResponse {
    pub fn encode(&self) -> Vec<u8> {
        match self {
            IoctlResponse::Version(r) => r.encode().to_vec(),
            IoctlResponse::Status(r) => r.encode().to_vec(),
            IoctlResponse::Connect(r) => r.encode().to_vec(),
            IoctlResponse::SetLink(_) => SetLinkResponse::encode().to_vec(),
            IoctlResponse::SetupAck => vec![],
            IoctlResponse::PeerMacAck => vec![],
            IoctlResponse::SetMac(r) => r.encode().to_vec(),
            IoctlResponse::UnknownAck => vec![],
        }
    }

    pub fn info_len(&self) -> u32 {
        match self {
            IoctlResponse::Version(_) => 12,
            IoctlResponse::Status(_) => 4,
            IoctlResponse::Connect(_) => 1,
            IoctlResponse::SetLink(_) => 0xB8,
            IoctlResponse::SetupAck => 0,
            IoctlResponse::PeerMacAck => 0,
            IoctlResponse::SetMac(_) => 1,
            IoctlResponse::UnknownAck => 0,
        }
    }
}

pub fn process_ioctl(
    ioctl: RvpnIoctl,
    input_buffer: &[u8],
    output_buffer_len: u32,
    adapter_mac: &[u8; 6],
    setup_mode: &mut SetupMode,
) -> Result<IoctlResponse, RvpnError> {
    match ioctl {
        RvpnIoctl::Version => {
            let req_ver = if input_buffer.len() >= 4 {
                u32::from_le_bytes([
                    input_buffer[0],
                    input_buffer[1],
                    input_buffer[2],
                    input_buffer[3],
                ])
            } else {
                0
            };

            if output_buffer_len >= 12 {
                if req_ver == 4 {
                    let mut mac = [0u8; 6];
                    mac.copy_from_slice(adapter_mac);
                    Ok(IoctlResponse::Version(VersionResponse::success(mac)))
                } else {
                    Ok(IoctlResponse::Version(VersionResponse::mismatch()))
                }
            } else {
                Err(RvpnError::Protocol(
                    "IOCTL_RVPN_VERSION: output buffer too small".to_string(),
                ))
            }
        }
        RvpnIoctl::Status => {
            if output_buffer_len >= 4 {
                Ok(IoctlResponse::Status(StatusResponse::connected()))
            } else {
                Err(RvpnError::Protocol(
                    "IOCTL_RVPN_STATUS: output buffer too small".to_string(),
                ))
            }
        }
        RvpnIoctl::Connect => {
            if output_buffer_len >= 1 {
                Ok(IoctlResponse::Connect(ConnectResponse::connected()))
            } else {
                Err(RvpnError::Protocol(
                    "IOCTL_RVPN_CONNECT: output buffer too small".to_string(),
                ))
            }
        }
        RvpnIoctl::SetLink => Ok(IoctlResponse::SetLink(SetLinkResponse)),
        RvpnIoctl::Setup => {
            if input_buffer.len() >= 4 {
                let mode = u32::from_le_bytes([
                    input_buffer[0],
                    input_buffer[1],
                    input_buffer[2],
                    input_buffer[3],
                ]);
                *setup_mode = SetupMode::from(mode);
                Ok(IoctlResponse::SetupAck)
            } else {
                Err(RvpnError::Protocol(
                    "IOCTL_RVPN_SETUP: input buffer too small".to_string(),
                ))
            }
        }
        RvpnIoctl::PeerMac => {
            if input_buffer.len() >= 6 {
                let mac_bytes = [
                    input_buffer[0],
                    input_buffer[1],
                    input_buffer[2],
                    input_buffer[3],
                    input_buffer[4],
                    input_buffer[5],
                ];
                let _mac = MacAddress::new(mac_bytes);

                Ok(IoctlResponse::PeerMacAck)
            } else {
                Err(RvpnError::Protocol(
                    "IOCTL_RVPN_PEERMAC: input buffer too small".to_string(),
                ))
            }
        }
        RvpnIoctl::SetMac => Ok(IoctlResponse::SetMac(SetMacResponse)),
        RvpnIoctl::Unknown(_c) => Ok(IoctlResponse::UnknownAck),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_response_encode() {
        let resp = VersionResponse::success([0x02, 0x50, 0xde, 0xad, 0xbe, 0xef]);
        let buf = resp.encode();
        assert_eq!(buf.len(), 12);
        assert_eq!(&buf[0..4], &0u32.to_le_bytes());
        assert_eq!(&buf[4..10], &[0x02, 0x50, 0xde, 0xad, 0xbe, 0xef]);
    }

    #[test]
    fn status_response_encode() {
        let resp = StatusResponse::connected();
        let buf = resp.encode();
        assert_eq!(buf.len(), 4);
        assert_eq!(buf, 1u32.to_le_bytes());
    }

    #[test]
    fn connect_response_encode() {
        let resp = ConnectResponse::connected();
        let buf = resp.encode();
        assert_eq!(buf.len(), 1);
        assert_eq!(buf[0], 0x20);
    }

    #[test]
    fn setlink_response_encode() {
        let buf = SetLinkResponse::encode();
        assert_eq!(buf.len(), 0xB8);
    }

    #[test]
    fn process_ioctl_version_ok() {
        let mut mode = SetupMode::Legacy;
        let mac = [0x02, 0x50, 0xde, 0xad, 0xbe, 0xef];
        let resp =
            process_ioctl(RvpnIoctl::Version, &4u32.to_le_bytes(), 12, &mac, &mut mode).unwrap();
        matches!(resp, IoctlResponse::Version(v) if v.status == 0);
    }

    #[test]
    fn process_ioctl_version_mismatch() {
        let mut mode = SetupMode::Legacy;
        let mac = [0x02, 0x50, 0xde, 0xad, 0xbe, 0xef];
        let resp = process_ioctl(
            RvpnIoctl::Version,
            &99u32.to_le_bytes(),
            12,
            &mac,
            &mut mode,
        )
        .unwrap();
        matches!(resp, IoctlResponse::Version(v) if v.status == 1);
    }

    #[test]
    fn ioctl_from_code() {
        assert_eq!(RvpnIoctl::from(IOCTL_RVPN_VERSION), RvpnIoctl::Version);
        assert_eq!(RvpnIoctl::from(IOCTL_RVPN_STATUS), RvpnIoctl::Status);
        assert_eq!(RvpnIoctl::from(0xDEADBEEF), RvpnIoctl::Unknown(0xDEADBEEF));
    }
}
