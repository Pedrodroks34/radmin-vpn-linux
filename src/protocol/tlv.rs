use crate::core::constants::ETH_FRAME_MIN;
use crate::core::types::RvpnError;
use crate::core::types::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FramePriority {
    High,
    Low,
}

pub fn encode_tlv_frame(frame: &EthernetFrame, setup_mode: u32) -> Vec<u8> {
    let frame_len = frame.data.len();
    let padded_len = if frame_len < ETH_FRAME_MIN {
        ETH_FRAME_MIN
    } else {
        frame_len
    };

    let _header_size = if setup_mode >= 2 { 8 } else { 4 };
    let mut buf = Vec::with_capacity(8 + padded_len);

    if setup_mode >= 2 {
        let mac_prefix = if frame.data.len() >= 4 {
            let raw =
                u32::from_le_bytes([frame.data[0], frame.data[1], frame.data[2], frame.data[3]]);
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

pub fn encode_tlv_frames(
    frames: &[EthernetFrame],
    setup_mode: u32,
    buf_capacity: usize,
) -> Vec<u8> {
    let mut buf = Vec::with_capacity(buf_capacity);
    let mut total = 0;

    for frame in frames {
        let encoded = encode_tlv_frame(frame, setup_mode);
        if total + encoded.len() > buf_capacity {
            break;
        }
        buf.extend_from_slice(&encoded);
        total += encoded.len();
    }

    buf
}

pub fn decode_tlv_frames(buf: &[u8], setup_mode: u32) -> Result<Vec<EthernetFrame>, RvpnError> {
    let mut frames = Vec::new();
    let mut offset = 0;
    let _header_size = if setup_mode >= 2 { 8 } else { 4 };

    while offset < buf.len() {
        if setup_mode >= 2 {
            if offset + 4 > buf.len() {
                break;
            }
            offset += 4;
        }

        if offset + 4 > buf.len() {
            break;
        }
        let frame_len = u32::from_le_bytes([
            buf[offset],
            buf[offset + 1],
            buf[offset + 2],
            buf[offset + 3],
        ]) as usize;
        offset += 4;

        if frame_len == 0 || frame_len > 1600 || offset + frame_len > buf.len() {
            break;
        }

        let frame_data = buf[offset..offset + frame_len].to_vec();
        frames.push(EthernetFrame::new(frame_data));
        offset += frame_len;
    }

    Ok(frames)
}

pub fn classify_frame_priority(frame: &EthernetFrame) -> FramePriority {
    let ethertype = match frame.ethertype() {
        Some(e) => e,
        None => return FramePriority::Low,
    };

    if ethertype == 0x0806 {
        return FramePriority::High;
    }

    if ethertype == 0x0800 {
        if frame.data.len() < 34 {
            return FramePriority::Low;
        }
        let proto = frame.data[23];
        match proto {
            6 | 1 => FramePriority::High,
            17 => {
                if frame.data[0] & 0x01 == 0 {
                    FramePriority::High
                } else {
                    FramePriority::Low
                }
            }
            _ => FramePriority::Low,
        }
    } else if ethertype == 0x86dd {
        if frame.data.len() < 54 {
            return FramePriority::Low;
        }
        let proto = frame.data[20];
        match proto {
            6 | 58 => FramePriority::High,
            17 => {
                if frame.data[0] & 0x01 == 0 {
                    FramePriority::High
                } else {
                    FramePriority::Low
                }
            }
            _ => FramePriority::Low,
        }
    } else {
        FramePriority::Low
    }
}

#[derive(Debug)]
pub struct MacRouteTable {
    entries: Vec<MacRouteEntry>,
    capacity: usize,
}

impl Default for MacRouteTable {
    fn default() -> Self {
        Self::new(256)
    }
}

#[derive(Debug, Clone)]
pub struct MacRouteEntry {
    pub mac: MacAddress,
    pub handle_id: u64,
    pub active: bool,
}

impl MacRouteTable {
    pub fn new(initial_capacity: usize) -> Self {
        Self {
            entries: Vec::with_capacity(initial_capacity),
            capacity: initial_capacity,
        }
    }

    pub fn register(&mut self, mac: MacAddress, handle_id: u64) -> bool {
        for entry in &mut self.entries {
            if entry.handle_id == handle_id {
                entry.mac = mac;
                entry.active = true;
                return true;
            }
        }

        for entry in &mut self.entries {
            if !entry.active {
                entry.mac = mac;
                entry.handle_id = handle_id;
                entry.active = true;
                return true;
            }
        }

        if self.entries.len() < self.capacity {
            self.entries.push(MacRouteEntry {
                mac,
                handle_id,
                active: true,
            });
            true
        } else {
            false
        }
    }

    pub fn release_handle(&mut self, handle_id: u64) -> usize {
        let mut freed = 0;
        for entry in &mut self.entries {
            if entry.handle_id == handle_id && entry.active {
                entry.active = false;
                freed += 1;
            }
        }

        self.entries.retain(|e| e.active);
        freed
    }

    pub fn lookup(&self, dst_mac: &MacAddress) -> Option<u64> {
        if dst_mac.is_group() {
            return None;
        }
        for entry in &self.entries {
            if entry.active && entry.mac == *dst_mac {
                return Some(entry.handle_id);
            }
        }
        None
    }

    pub fn active_handles(&self) -> Vec<u64> {
        self.entries
            .iter()
            .filter(|e| e.active)
            .map(|e| e.handle_id)
            .collect()
    }

    pub fn active_count(&self) -> usize {
        self.entries.iter().filter(|e| e.active).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tlv_encode_decode() {
        let frame = EthernetFrame::new(vec![
            0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x02, 0x50, 0xde, 0xad, 0xbe, 0xef, 0x08, 0x00, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ]);

        let encoded = encode_tlv_frame(&frame, 2);
        assert!(encoded.len() >= 8 + 60);

        let decoded = decode_tlv_frames(&encoded, 2).unwrap();
        assert_eq!(decoded.len(), 1);

        let mut expected = frame.data.clone();
        expected.resize(ETH_FRAME_MIN, 0);
        assert_eq!(decoded[0].data, expected);
    }

    #[test]
    fn tlv_legacy_mode() {
        let frame = EthernetFrame::new(vec![1, 2, 3, 4, 5, 6]);
        let encoded = encode_tlv_frame(&frame, 1);
        let decoded = decode_tlv_frames(&encoded, 1).unwrap();
        assert_eq!(decoded.len(), 1);
    }

    #[test]
    fn classify_priority() {
        let mut tcp_data = vec![
            0x02, 0x00, 0x00, 0x00, 0x00, 0x01, 0x02, 0x50, 0xde, 0xad, 0xbe, 0xef, 0x08, 0x00,
        ];

        tcp_data.resize(34, 0);
        tcp_data[14] = 0x45;
        tcp_data[23] = 6;
        let tcp_frame = EthernetFrame::new(tcp_data);
        assert_eq!(classify_frame_priority(&tcp_frame), FramePriority::High);

        let mut bcast_data = vec![
            0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x02, 0x50, 0xde, 0xad, 0xbe, 0xef, 0x08, 0x00,
        ];
        bcast_data.resize(34, 0);
        bcast_data[14] = 0x45;
        bcast_data[23] = 17;
        let bcast_frame = EthernetFrame::new(bcast_data);
        assert_eq!(classify_frame_priority(&bcast_frame), FramePriority::Low);

        let arp_frame = EthernetFrame::new(vec![
            0x02, 0x00, 0x00, 0x00, 0x00, 0x01, 0x02, 0x50, 0xde, 0xad, 0xbe, 0xef, 0x08, 0x06,
        ]);
        assert_eq!(classify_frame_priority(&arp_frame), FramePriority::High);
    }

    #[test]
    fn mac_route_table() {
        let mut table = MacRouteTable::new(16);
        let mac1 = MacAddress::new([2, 2, 3, 4, 5, 6]);
        let mac2 = MacAddress::new([8, 8, 9, 10, 11, 12]);

        assert!(table.register(mac1, 100));
        assert!(table.register(mac2, 200));

        assert_eq!(table.lookup(&mac1), Some(100));
        assert_eq!(table.lookup(&mac2), Some(200));
        assert_eq!(table.active_count(), 2);

        table.release_handle(100);
        assert_eq!(table.lookup(&mac1), None);
        assert_eq!(table.lookup(&mac2), Some(200));
        assert_eq!(table.active_count(), 1);

        let group_mac = MacAddress::new([0xff, 0xff, 0xff, 0xff, 0xff, 0xff]);
        assert_eq!(table.lookup(&group_mac), None);
    }
}
