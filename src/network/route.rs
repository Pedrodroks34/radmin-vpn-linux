use crate::core::constants::*;
use crate::core::types::RvpnError;
use crate::network::netlink;
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ManagedRoute {
    pub destination: String,
    pub device: String,
    pub metric: Option<u32>,
}

#[derive(Debug, Default)]
pub struct RouteManager {
    device: String,
    routes: HashSet<ManagedRoute>,
}

impl RouteManager {
    pub fn new(device: &str) -> Self {
        Self {
            device: device.to_string(),
            routes: HashSet::new(),
        }
    }

    pub fn add_route(&mut self, destination: &str, metric: Option<u32>) -> Result<(), RvpnError> {
        netlink::route_add_v4(destination, &self.device, metric)?;

        let route = ManagedRoute {
            destination: destination.to_string(),
            device: self.device.clone(),
            metric,
        };
        self.routes.insert(route);
        Ok(())
    }

    pub fn append_route(
        &mut self,
        destination: &str,
        metric: Option<u32>,
    ) -> Result<(), RvpnError> {
        // Best-effort: the broadcast routes are not required for the VPN to
        // work (peer unicast still flows), so a pre-existing or rejected
        // broadcast route must not abort startup.
        if netlink::route_add_v4(destination, &self.device, metric).is_ok() {
            let route = ManagedRoute {
                destination: destination.to_string(),
                device: self.device.clone(),
                metric,
            };
            self.routes.insert(route);
        }
        Ok(())
    }

    pub fn remove_route(&mut self, destination: &str) -> Result<(), RvpnError> {
        netlink::route_del_v4(destination, &self.device)?;

        self.routes.retain(|r| r.destination != destination);
        Ok(())
    }

    pub fn setup_vpn_routes(&mut self, broadcast: bool) -> Result<(), RvpnError> {
        self.add_route(PEER_NETWORK, None)?;

        if broadcast {
            self.append_route("255.255.255.255/32", Some(0))?;
            self.append_route("224.0.0.0/4", Some(0))?;
        }

        Ok(())
    }

    pub fn remove_all(&mut self) -> Result<(), RvpnError> {
        let mut errors = Vec::new();
        let routes: Vec<ManagedRoute> = self.routes.clone().into_iter().collect();

        for route in &routes {
            if let Err(e) = self.remove_route(&route.destination) {
                errors.push(e.to_string());
            }
        }

        self.routes.clear();

        if errors.is_empty() {
            Ok(())
        } else {
            Err(RvpnError::Route(format!(
                "failed to remove some routes: {}",
                errors.join(", ")
            )))
        }
    }

    pub fn routes(&self) -> Vec<ManagedRoute> {
        self.routes.iter().cloned().collect()
    }

    pub fn device(&self) -> &str {
        &self.device
    }
}

impl Drop for RouteManager {
    fn drop(&mut self) {
        if !self.routes.is_empty() {
            let _ = self.remove_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_manager_default() {
        let rm = RouteManager::new("tap0");
        assert_eq!(rm.device(), "tap0");
        assert!(rm.routes().is_empty());
    }

    #[test]
    fn managed_route_clone() {
        let r = ManagedRoute {
            destination: "26.0.0.0/8".to_string(),
            device: "radminvpn0".to_string(),
            metric: None,
        };
        let r2 = r.clone();
        assert_eq!(r, r2);
    }
}
