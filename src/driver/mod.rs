use crate::core::constants::*;
use crate::core::types::MacAddress;
use crate::core::types::RvpnError;
use crate::protocol::ioctl::*;
use crate::protocol::routing::FrameRouter;
use crate::protocol::tlv::*;

pub mod userspace;

#[cfg(target_os = "windows")]
mod pe_driver {

    use crate::core::constants::*;
    use crate::protocol::ioctl::*;
    use crate::protocol::routing::{FrameRouter, RouteDecision};
    use crate::protocol::tlv::*;
    use core::ptr::null_mut;

    pub type NTSTATUS = i32;
    pub type PVOID = *mut core::ffi::c_void;
    pub type HANDLE = *mut core::ffi::c_void;
    pub type PIRP = *mut core::ffi::c_void;
    pub type PDEVICE_OBJECT = *mut core::ffi::c_void;
    pub type PDRIVER_OBJECT = *mut core::ffi::c_void;
    pub type PFILE_OBJECT = *mut core::ffi::c_void;
    pub type PUNICODE_STRING = *mut core::ffi::c_void;
    pub type PIO_STACK_LOCATION = *mut core::ffi::c_void;
    pub type POBJECT_ATTRIBUTES = *mut core::ffi::c_void;
    pub type PIO_STATUS_BLOCK = *mut core::ffi::c_void;
    pub type KIRQL = u8;

    pub const STATUS_SUCCESS: NTSTATUS = 0;
    pub const STATUS_PENDING: NTSTATUS = 0x103;
    pub const STATUS_CANCELLED: NTSTATUS = 0xC0000120;
    pub const STATUS_DEVICE_NOT_CONNECTED: NTSTATUS = 0xC00000D9;
    pub const STATUS_BUFFER_TOO_SMALL: NTSTATUS = 0xC0000023;
    pub const STATUS_INSUFFICIENT_RESOURCES: NTSTATUS = 0xC000009A;
    pub const STATUS_END_OF_FILE: NTSTATUS = 0xC0000011;

    pub const IO_NO_INCREMENT: u8 = 0;
    pub const FILE_DEVICE_UNKNOWN: u32 = 0x00000022;
    pub const DO_DIRECT_IO: u32 = 0x00000004;
    pub const DO_DEVICE_INITIALIZING: u32 = 0x00000001;

    pub const IRP_MJ_CREATE: u8 = 0;
    pub const IRP_MJ_CLOSE: u8 = 2;
    pub const IRP_MJ_CLEANUP: u8 = 3;
    pub const IRP_MJ_DEVICE_CONTROL: u8 = 14;
    pub const IRP_MJ_READ: u8 = 3;
    pub const IRP_MJ_WRITE: u8 = 4;

    pub const OBJ_CASE_INSENSITIVE: u32 = 0x00000040;
    pub const OBJ_KERNEL_HANDLE: u32 = 0x00000200;
    pub const FILE_SYNCHRONOUS_IO_NONALERT: u32 = 0x00000020;
    pub const FILE_OPEN_IF: u32 = 0x00000003;
    pub const FILE_ATTRIBUTE_NORMAL: u32 = 0x00000080;
    pub const FILE_SHARE_READ: u32 = 0x00000001;
    pub const FILE_SHARE_WRITE: u32 = 0x00000002;

    pub const NonPagedPool: u32 = 0;

    pub const PEER_ROUTES_TAG: u32 = 0x72507652;

    pub const IRP_QUEUE_SIZE: usize = 1024;
    pub const RX_RING_SIZE: usize = 16;
    pub const RX_FRAME_MAX: usize = 1600;
    pub const PEER_ROUTES_INITIAL: usize = 16;
    pub const PEER_ROUTES_MAX: usize = 4096;

    #[no_mangle]
    pub extern "system" fn DriverEntry(
        _driver_object: PDRIVER_OBJECT,
        _registry_path: PUNICODE_STRING,
    ) -> NTSTATUS {
        STATUS_SUCCESS
    }
}

pub struct UserspaceDriver {
    adapter_mac: MacAddress,
    setup_mode: SetupModeState,
    router: FrameRouter,
    fifo_b2d: Option<std::fs::File>,
    fifo_d2b_high: Option<std::fs::File>,
    fifo_d2b_low: Option<std::fs::File>,
    control_socket: Option<std::os::unix::net::UnixListener>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupModeState {
    Legacy = 0,
    Tlv = 2,
}

impl UserspaceDriver {
    pub fn new(adapter_mac: MacAddress) -> Self {
        Self {
            adapter_mac,
            setup_mode: SetupModeState::Legacy,
            router: FrameRouter::new(),
            fifo_b2d: None,
            fifo_d2b_high: None,
            fifo_d2b_low: None,
            control_socket: None,
        }
    }

    pub fn open_fifos(&mut self) -> Result<(), RvpnError> {
        use std::os::unix::fs::OpenOptionsExt;

        self.fifo_b2d = Some(
            std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(FIFO_B2D)
                .map_err(|e| RvpnError::Driver(format!("open b2d: {e}")))?,
        );

        self.fifo_d2b_high = Some(
            std::fs::OpenOptions::new()
                .write(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(FIFO_D2B_HIGH)
                .map_err(|e| RvpnError::Driver(format!("open d2b_high: {e}")))?,
        );

        self.fifo_d2b_low = Some(
            std::fs::OpenOptions::new()
                .write(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(FIFO_D2B_LOW)
                .map_err(|e| RvpnError::Driver(format!("open d2b_low: {e}")))?,
        );

        Ok(())
    }

    pub fn setup_control_socket(&mut self, path: &str) -> Result<(), RvpnError> {
        let _ = std::fs::remove_file(path);
        let listener = std::os::unix::net::UnixListener::bind(path)
            .map_err(|e| RvpnError::Driver(format!("bind control socket: {e}")))?;
        self.control_socket = Some(listener);
        Ok(())
    }

    pub fn register_peer(&mut self, mac: MacAddress, handle_id: u64) {
        self.router.register_peer(mac, handle_id);
    }

    pub fn release_handle(&mut self, handle_id: u64) {
        self.router.release_handle(handle_id);
    }

    pub fn process_command(
        &mut self,
        cmd: u32,
        input: &[u8],
        output: &mut [u8],
    ) -> Result<u32, RvpnError> {
        let ioctl = RvpnIoctl::from(cmd);
        let response = process_ioctl(
            ioctl,
            input,
            output.len() as u32,
            &self.adapter_mac.bytes(),
            &mut self.setup_mode.into(),
        )?;

        let encoded = response.encode();
        if encoded.len() > output.len() {
            return Err(RvpnError::Driver("output buffer too small".to_string()));
        }
        output[..encoded.len()].copy_from_slice(&encoded);

        Ok(response.info_len())
    }

    pub fn read_frame_from_bridge(&mut self, buf: &mut [u8]) -> Result<usize, RvpnError> {
        let f = self
            .fifo_b2d
            .as_mut()
            .ok_or_else(|| RvpnError::Driver("b2d FIFO not open".to_string()))?;
        use std::io::Read;
        match f.read(buf) {
            Ok(0) => Err(RvpnError::Driver("EOF on b2d FIFO".to_string())),
            Ok(n) => Ok(n),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(0),
            Err(e) => Err(RvpnError::Driver(format!("read b2d: {e}"))),
        }
    }

    pub fn write_frame_to_bridge(
        &mut self,
        frame: &crate::core::types::EthernetFrame,
        priority: FramePriority,
    ) -> Result<(), RvpnError> {
        let fifo = match priority {
            FramePriority::High => self.fifo_d2b_high.as_mut(),
            FramePriority::Low => self.fifo_d2b_low.as_mut(),
        };

        let f = fifo.ok_or_else(|| RvpnError::Driver("d2b FIFO not open".to_string()))?;

        let len = frame.data.len() as u16;
        let mut combined = Vec::with_capacity(2 + len as usize);
        combined.push((len & 0xFF) as u8);
        combined.push(((len >> 8) & 0xFF) as u8);
        combined.extend_from_slice(&frame.data);

        // The d2b FIFO is opened O_NONBLOCK: drain what fits and report
        // WouldBlock instead of discarding already-sent bytes like
        // `write_all` does.
        use std::io::Write;
        let mut sent = 0usize;
        while sent < combined.len() {
            match f.write(&combined[sent..]) {
                Ok(0) => {
                    return Err(RvpnError::Driver(format!(
                        "write d2b: zero-length write after {sent}/{} bytes",
                        combined.len()
                    )))
                }
                Ok(n) => sent += n,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    return Err(RvpnError::Driver(format!(
                        "write d2b: would block after {sent}/{} bytes",
                        combined.len()
                    )))
                }
                Err(e) => {
                    return Err(RvpnError::Driver(format!("write d2b: {e}")));
                }
            }
        }

        Ok(())
    }

    pub fn adapter_mac(&self) -> MacAddress {
        self.adapter_mac
    }

    pub fn setup_mode(&self) -> SetupModeState {
        self.setup_mode
    }
}

impl SetupModeState {
    fn into(self) -> crate::protocol::ioctl::SetupMode {
        match self {
            SetupModeState::Legacy => crate::protocol::ioctl::SetupMode::Legacy,
            SetupModeState::Tlv => crate::protocol::ioctl::SetupMode::TlvMode,
        }
    }
}

impl From<crate::protocol::ioctl::SetupMode> for SetupModeState {
    fn from(m: crate::protocol::ioctl::SetupMode) -> Self {
        match m {
            crate::protocol::ioctl::SetupMode::TlvMode => SetupModeState::Tlv,
            _ => SetupModeState::Legacy,
        }
    }
}

impl Default for UserspaceDriver {
    fn default() -> Self {
        Self::new(MacAddress::new([0x02, 0x50, 0xDE, 0xAD, 0xBE, 0xEF]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn userspace_driver_default() {
        let drv = UserspaceDriver::default();
        assert_eq!(drv.setup_mode(), SetupModeState::Legacy);
    }

    #[test]
    fn process_command_version() {
        let mut drv = UserspaceDriver::new(MacAddress::new([0x02, 0x50, 0xDE, 0xAD, 0xBE, 0xEF]));
        let mut output = [0u8; 12];
        let info_len = drv
            .process_command(IOCTL_RVPN_VERSION, &4u32.to_le_bytes(), &mut output)
            .unwrap();
        assert_eq!(info_len, 12);
        assert_eq!(&output[0..4], &0u32.to_le_bytes());
    }

    #[test]
    fn process_command_status() {
        let mut drv = UserspaceDriver::default();
        let mut output = [0u8; 4];
        let info_len = drv
            .process_command(IOCTL_RVPN_STATUS, &[], &mut output)
            .unwrap();
        assert_eq!(info_len, 4);
        assert_eq!(output, 1u32.to_le_bytes());
    }
}
