# Rust rewrite architecture

This document maps the original project-owned components to the Rust workspace.

## Component mapping

The original C/Python/shell sources were removed after the rewrite. This table maps the responsibilities to the Rust workspace that replaced them.

| Responsibility | Role | Rust workspace |
| --- | --- | --- |
| TAP bridge / Ethernet relay | TAP <-> FIFO Ethernet relay, filters, ARP cache, multicast replication | `src/bridge/mod.rs` |
| Wine kernel driver shim | IOCTLs, TLV framing, MAC routing, IRP/queue semantics | `src/driver/mod.rs` + `src/driver/userspace.rs` + `src/protocol/*` |
| Wine compatibility hooks | In-process compatibility surface / registry overrides | `src/hooks/mod.rs` + `src/wine/mod.rs` |
| netsh replacement | Wine `netsh` stub replacement writing commands for Linux execution | `src/cli/mod.rs` (`netsh-relay`); commands are argument-based, never shell-interpolated |
| Launcher | Inject hook DLL / launch service | `src/launcher/mod.rs` |
| Driver installer stub | No-op stand-in for the real NDIS driver installer | `src/installer/mod.rs` |
| DNS shim | `LD_PRELOAD` for private-address PTR lookups | `crates/rvpn-dnsfix` (`cdylib`) |
| Reuseport shim | `LD_PRELOAD` setting `SO_REUSEPORT` for wineserver sockets | `crates/rvpn-reuseport` (`cdylib`) |
| Filter UI | GTK4 filter UI | `src/cli/mod.rs` + optional `gui` Cargo feature |
| Orchestration | Setup, firewall, routes, netsh relay, diagnostics | `src/service/*`, `src/network/*`, `src/firewall/*`, `src/wine/*`, `src/installer/*`, `src/cli/*` |

## Runtime data flow

1. `radmin-vpn service` acquires the exclusive lock in `$XDG_RUNTIME_DIR/radmin-vpn/service.lock` (fallback `/run/radmin-vpn/service.lock`).
2. TAP interface is created or adopted through `/dev/net/tun` using `TUNSETIFF`/`TUNSETPERSIST` when the service created it, and is configured before use.
3. FIFOs are created/reused: `/tmp/rvpn_b2d`, `/tmp/rvpn_d2b_high`, `/tmp/rvpn_d2b_low`.
4. Bridge relays frames: TAP -> `b2d`, `d2b_high`/`d2b_low` -> TAP (high-priority drained first, low-pipe starvation guard).
5. Driver shim (userspace/native mode) and/or Wine-side components exchange framed packets and control messages according to the preserved protocol.
6. Wine/Radmin processes are launched with scoped environment (`LD_PRELOAD` shims only for the intended processes).
7. Firewall and routes are applied with managed tags/state so only service-created resources are removed.

## Preservation constraints

- FIFO names and `[u16 len][frame]` framing are preserved.
- TAP is owned by the service; GUI exit does not remove it.
- Pre-existing interfaces and unrelated firewall rules are never modified or deleted.
- External inputs (netsh command file, config, filter file) are parsed and dispatched without shell interpolation.

## Known limitations

- A true Windows NDIS/WDM driver binary (`rvpnnetmp.sys`) requires a Windows PE cross-build (`x86_64-pc-windows-gnu`) and DDK-compatible environment; producing and validating it is environment-dependent. The Rust protocol/logic is structured so the PE target can reuse it.
- GTK4-based filter UI requires GTK4 development/runtime libraries and is behind an optional Cargo feature.
- Wine and the proprietary Radmin VPN binaries remain external and are subject to Wine compatibility behavior.
