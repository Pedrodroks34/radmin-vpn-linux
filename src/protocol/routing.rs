use crate::core::types::MacAddress;
use crate::protocol::tlv::MacRouteTable;

pub use crate::protocol::tlv::FramePriority;

#[derive(Debug, Clone)]
pub enum RouteDecision {
    Unicast(u64),

    Group(Vec<u64>),

    NoRoute,
}

#[derive(Debug, Default)]
pub struct FrameRouter {
    table: MacRouteTable,
}

impl FrameRouter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_peer(&mut self, mac: MacAddress, handle_id: u64) {
        self.table.register(mac, handle_id);
    }

    pub fn release_handle(&mut self, handle_id: u64) {
        self.table.release_handle(handle_id);
    }

    pub fn route(&self, frame: &crate::core::types::EthernetFrame) -> RouteDecision {
        let dst = match frame.dst_mac() {
            Some(m) => m,
            None => return RouteDecision::NoRoute,
        };

        if dst.is_group() {
            let handles = self.table.active_handles();
            if handles.is_empty() {
                RouteDecision::NoRoute
            } else {
                RouteDecision::Group(handles)
            }
        } else {
            match self.table.lookup(&dst) {
                Some(handle_id) => RouteDecision::Unicast(handle_id),
                None => RouteDecision::NoRoute,
            }
        }
    }

    pub fn active_count(&self) -> usize {
        self.table.active_count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_unicast() {
        let mut router = FrameRouter::new();
        let mac = MacAddress::new([2, 2, 3, 4, 5, 6]);
        router.register_peer(mac, 42);

        let frame = crate::core::types::EthernetFrame::new(vec![
            2, 2, 3, 4, 5, 6, 0x02, 0x50, 0xde, 0xad, 0xbe, 0xef, 0x08, 0x00,
        ]);

        match router.route(&frame) {
            RouteDecision::Unicast(h) => assert_eq!(h, 42),
            _ => panic!("expected unicast route"),
        }
    }

    #[test]
    fn route_group() {
        let mut router = FrameRouter::new();
        let mac1 = MacAddress::new([1, 2, 3, 4, 5, 6]);
        let mac2 = MacAddress::new([7, 8, 9, 10, 11, 12]);
        router.register_peer(mac1, 1);
        router.register_peer(mac2, 2);

        let frame = crate::core::types::EthernetFrame::new(vec![
            0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x02, 0x50, 0xde, 0xad, 0xbe, 0xef, 0x08, 0x00,
        ]);

        match router.route(&frame) {
            RouteDecision::Group(handles) => {
                assert_eq!(handles.len(), 2);
                assert!(handles.contains(&1));
                assert!(handles.contains(&2));
            }
            _ => panic!("expected group route"),
        }
    }

    #[test]
    fn route_no_route() {
        let router = FrameRouter::new();
        let frame = crate::core::types::EthernetFrame::new(vec![
            1, 2, 3, 4, 5, 6, 0x02, 0x50, 0xde, 0xad, 0xbe, 0xef, 0x08, 0x00,
        ]);

        match router.route(&frame) {
            RouteDecision::NoRoute => {}
            _ => panic!("expected no route"),
        }
    }
}
