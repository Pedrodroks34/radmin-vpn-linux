//! In-process rtnetlink operations used by the service.
//!
//! All network configuration for the VPN adapter (routes, L3 addresses,
//! interface state and deletion) goes through these helpers instead of the
//! `ip` userspace binary: the service holds `cap_net_admin` on its own file,
//! and file capabilities do not propagate to subprocesses.

use crate::core::types::RvpnError;
use futures::{StreamExt, TryStreamExt};
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// The rtnetlink operations are rare (service setup/teardown), so a fresh
/// current-thread tokio runtime per call is fine and avoids leaking
/// background connection tasks.
fn block_on<F: Future>(future: F) -> F::Output {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to create netlink runtime");
    rt.block_on(future)
}

fn map_err(err: rtnetlink::Error) -> RvpnError {
    RvpnError::Service(format!("netlink: {err}"))
}

fn errno_of(err: &rtnetlink::Error) -> Option<i32> {
    match err {
        rtnetlink::Error::NetlinkError(msg) => Some(msg.code.abs()),
        _ => None,
    }
}

fn is_not_found(err: &rtnetlink::Error) -> bool {
    errno_of(err).is_some_and(|e| e == libc::ESRCH || e == libc::EADDRNOTAVAIL || e == libc::ENOENT)
}

async fn new_conn_handle() -> Result<(tokio::task::JoinHandle<()>, rtnetlink::Handle), RvpnError> {
    let (connection, handle, _) = rtnetlink::new_connection()
        .map_err(|e| RvpnError::Service(format!("netlink connection: {e}")))?;
    let task = tokio::spawn(connection);
    Ok((task, handle))
}

/// Find the interface index of `name`, or `None` if it does not exist.
async fn link_index(handle: &rtnetlink::Handle, name: &str) -> Result<Option<u32>, RvpnError> {
    let mut stream = handle
        .link()
        .get()
        .set_name_filter(name.to_string())
        .execute();
    if let Some(message) = stream.try_next().await.map_err(map_err)? {
        return Ok(Some(message.header.index));
    }
    Ok(None)
}

/// Delete the interface `name`. Returns true if it existed and was deleted.
pub fn link_delete(name: &str) -> Result<bool, RvpnError> {
    block_on(async move {
        let (task, mut handle) = new_conn_handle().await?;
        let result = async {
            let Some(index) = link_index(&handle, name).await? else {
                return Ok(false);
            };
            // rtnetlink 0.7's LinkDelRequest hardcodes NLM_F_CREATE|NLM_F_EXCL
            // on the delete message, which the kernel rejects with
            // EOPNOTSUPP. Build a plain RTM_DELLINK instead.
            let mut message = rtnetlink::packet::LinkMessage::default();
            message.header.index = index;
            let mut req = rtnetlink::packet::NetlinkMessage::from(
                rtnetlink::packet::RtnlMessage::DelLink(message),
            );
            req.header.flags = rtnetlink::packet::NLM_F_REQUEST | rtnetlink::packet::NLM_F_ACK;
            let mut stream = handle.request(req).map_err(map_err)?;
            while let Some(message) = stream.next().await {
                if let rtnetlink::packet::NetlinkPayload::Error(err) = message.payload {
                    return Err(map_err(rtnetlink::Error::NetlinkError(err)));
                }
            }
            Ok(true)
        }
        .await;
        task.abort();
        result
    })
}

/// Bring the interface `name` up (`ip link set dev <name> up`).
pub fn link_set_up(name: &str) -> Result<(), RvpnError> {
    block_on(async move {
        let (task, handle) = new_conn_handle().await?;
        let result = async {
            let Some(index) = link_index(&handle, name).await? else {
                return Err(RvpnError::Service(format!("link {name} not found")));
            };
            let link = handle.link();
            link.set(index).up().execute().await.map_err(map_err)?;
            Ok(())
        }
        .await;
        task.abort();
        result
    })
}

/// Delete every L3 address assigned to `name` (`ip addr flush dev <name>`).
/// Unknown or already-removed addresses are tolerated.
pub fn addr_flush(name: &str) -> Result<(), RvpnError> {
    block_on(async move {
        let (task, handle) = new_conn_handle().await?;
        let result = async {
            let Some(index) = link_index(&handle, name).await? else {
                return Ok(());
            };
            let mut stream = handle
                .address()
                .get()
                .set_link_index_filter(index)
                .execute();
            let mut addresses = Vec::new();
            while let Some(message) = stream.try_next().await.map_err(map_err)? {
                addresses.push(message);
            }
            let address = handle.address();
            for message in addresses {
                if let Err(e) = address.del(message).execute().await {
                    if !is_not_found(&e) {
                        return Err(map_err(e));
                    }
                }
            }
            Ok(())
        }
        .await;
        task.abort();
        result
    })
}

/// Add an L3 address to `name`, ignoring "already exists" (the netsh relay
/// applies this on every RvControlSvc start, so idempotency matters).
pub fn addr_add(name: &str, address: IpAddr, prefix_len: u8) -> Result<(), RvpnError> {
    block_on(async move {
        let (task, handle) = new_conn_handle().await?;
        let result = async {
            let Some(index) = link_index(&handle, name).await? else {
                return Err(RvpnError::Service(format!("link {name} not found")));
            };
            match handle
                .address()
                .add(index, address, prefix_len)
                .execute()
                .await
            {
                Ok(()) => Ok(()),
                Err(e) if errno_of(&e) == Some(libc::EEXIST) => Ok(()),
                Err(e) => Err(map_err(e)),
            }
        }
        .await;
        task.abort();
        result
    })
}

/// Parse `a.b.c.d/prefix` into its octets and prefix length.
pub fn parse_ipv4_cidr(s: &str) -> Result<(Ipv4Addr, u8), RvpnError> {
    let (addr, prefix) = s
        .split_once('/')
        .ok_or_else(|| RvpnError::Service(format!("invalid CIDR: {s}")))?;
    let addr: Ipv4Addr = addr
        .parse()
        .map_err(|_| RvpnError::Service(format!("invalid IPv4 address: {addr}")))?;
    let prefix: u8 = prefix
        .parse()
        .map_err(|_| RvpnError::Service(format!("invalid prefix length: {s}")))?;
    if prefix > 32 {
        return Err(RvpnError::Service(format!(
            "prefix length out of range: {prefix}"
        )));
    }
    Ok((addr, prefix))
}

/// Route into `device` with create-or-ignore semantics (mirrors
/// `ip route replace` closely enough for a static VPN route).
pub fn route_add_v4(destination: &str, device: &str, metric: Option<u32>) -> Result<(), RvpnError> {
    let (addr, prefix) = parse_ipv4_cidr(destination)?;
    block_on(async move {
        let (task, handle) = new_conn_handle().await?;
        let result = async {
            let Some(index) = link_index(&handle, device).await? else {
                return Err(RvpnError::Service(format!("link {device} not found")));
            };
            let mut request = handle
                .route()
                .add()
                .v4()
                .destination_prefix(addr, prefix)
                .output_interface(index);
            if let Some(metric) = metric {
                use rtnetlink::packet::RouteMessage;
                let message: &mut RouteMessage = request.message_mut();
                message.nlas.push(route_priority(metric));
            }
            match request.execute().await {
                Ok(()) => Ok(()),
                Err(e) if errno_of(&e) == Some(libc::EEXIST) => Ok(()),
                Err(e) => Err(map_err(e)),
            }
        }
        .await;
        task.abort();
        result
    })
}

fn route_priority(metric: u32) -> rtnetlink::packet::nlas::route::Nla {
    rtnetlink::packet::nlas::route::Nla::Priority(metric)
}

/// Delete the IPv4 route `destination/prefix` pointing at `device`,
/// if it exists (`ip route del <destination> dev <device>`).
pub fn route_del_v4(destination: &str, device: &str) -> Result<(), RvpnError> {
    let (addr, prefix) = parse_ipv4_cidr(destination)?;
    block_on(async move {
        let (task, handle) = new_conn_handle().await?;
        let result = async {
            let Some(index) = link_index(&handle, device).await? else {
                return Ok(());
            };
            let dst_bytes = addr.octets().to_vec();
            let mut stream = handle.route().get(rtnetlink::IpVersion::V4).execute();
            let mut found = None;
            while let Some(message) = stream.try_next().await.map_err(map_err)? {
                let dst_ok = message.header.destination_prefix_length == prefix
                    && message.nlas.iter().any(|nla| match nla {
                        rtnetlink::packet::nlas::route::Nla::Destination(bytes) => {
                            bytes.as_slice() == dst_bytes.as_slice()
                        }
                        _ => false,
                    });
                let oif_ok = message.nlas.iter().any(|nla| match nla {
                    rtnetlink::packet::nlas::route::Nla::Oif(i) => *i == index,
                    _ => false,
                });
                // Only consider routes whose OIF matches; routes without an OIF
                // (e.g. default via) are left alone.
                if dst_ok && oif_ok {
                    found = Some(message);
                    break;
                }
            }
            let Some(message) = found else {
                return Ok(());
            };
            match handle.route().del(message).execute().await {
                Ok(()) => Ok(()),
                Err(e) if is_not_found(&e) => Ok(()),
                Err(e) => Err(map_err(e)),
            }
        }
        .await;
        task.abort();
        result
    })
}

/// Parse `a:b::c/prefix` into its address and prefix length.
pub fn parse_ipv6_cidr(s: &str) -> Result<(Ipv6Addr, u8), RvpnError> {
    let (addr, prefix) = s
        .split_once('/')
        .ok_or_else(|| RvpnError::Service(format!("invalid IPv6 CIDR: {s}")))?;
    let addr: Ipv6Addr = addr
        .parse()
        .map_err(|_| RvpnError::Service(format!("invalid IPv6 address: {addr}")))?;
    let prefix: u8 = prefix
        .parse()
        .map_err(|_| RvpnError::Service(format!("invalid prefix length: {s}")))?;
    if prefix > 128 {
        return Err(RvpnError::Service(format!(
            "prefix length out of range: {prefix}"
        )));
    }
    Ok((addr, prefix))
}
