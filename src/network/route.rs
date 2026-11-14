use crate::core::constants::*;
use crate::core::types::RvpnError;
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
        let mut cmd = std::process::Command::new("ip");
        cmd.args(["route", "replace", destination, "dev", &self.device]);
        if let Some(m) = metric {
            cmd.args(["metric", &m.to_string()]);
        }

        let status = cmd
            .status()
            .map_err(|e| RvpnError::Route(format!("ip route replace failed: {e}")))?;

        if status.success() {
            let route = ManagedRoute {
                destination: destination.to_string(),
                device: self.device.clone(),
                metric,
            };
            self.routes.insert(route);
            Ok(())
        } else {
            Err(RvpnError::Route(format!(
                "ip route replace {destination} dev {} failed",
                self.device
            )))
        }
    }

    pub fn append_route(
        &mut self,
        destination: &str,
        metric: Option<u32>,
    ) -> Result<(), RvpnError> {
        let mut cmd = std::process::Command::new("ip");
        cmd.args(["route", "append", destination, "dev", &self.device]);
        if let Some(m) = metric {
            cmd.args(["metric", &m.to_string()]);
        }

        let status = cmd
            .status()
            .map_err(|e| RvpnError::Route(format!("ip route append failed: {e}")))?;

        if status.success() {
            let route = ManagedRoute {
                destination: destination.to_string(),
                device: self.device.clone(),
                metric,
            };
            self.routes.insert(route);
            Ok(())
        } else {
            Ok(())
        }
    }

    pub fn remove_route(&mut self, destination: &str) -> Result<(), RvpnError> {
        let status = std::process::Command::new("ip")
            .args(["route", "del", destination, "dev", &self.device])
            .status()
            .map_err(|e| RvpnError::Route(format!("ip route del failed: {e}")))?;

        self.routes.retain(|r| r.destination != destination);

        if status.success() {
            Ok(())
        } else {
            Err(RvpnError::Route(format!(
                "ip route del {destination} dev {} failed",
                self.device
            )))
        }
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
