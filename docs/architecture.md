# How it works

This document describes the Rust implementation. See [architecture-rust.md](architecture-rust.md) for the component mapping and constraints.

Radmin VPN's Windows service expects a virtual adapter driver and a small set of Win32/Wine compatibility shims. The Rust rewrite provides the Linux-side pieces natively and keeps the proprietary service/GUI unchanged.

```
Linux app ← TAP (radminvpn0) ← radmin-vpn (bridge) ← FIFOs ← driver shim / Wine
```

## Components

| Component | Role |
|---|---|
| `radmin-vpn` | Rust binary providing the service daemon, TAP ownership, Ethernet relay, firewall/routes, CLI, and systemd notify/watchdog integration. |
| `crates/rvpn-dnsfix` | `LD_PRELOAD` shim for private-address PTR lookups. |
| `crates/rvpn-reuseport` | `LD_PRELOAD` shim setting `SO_REUSEPORT` for TCP sockets. |
| Windows PE artifacts | `rvpnnetmp.sys`, `adapter_hook.dll`, `rvpn_launcher.exe`, `netsh.exe`, `netsh64.exe` remain external/cross-built Windows components. Their Rust protocol/shared logic is kept in `src/protocol/` and `src/driver/`. |

## Limitations

- One service instance at a time (exclusive lock in `$XDG_RUNTIME_DIR/radmin-vpn/service.lock`).
- The `26.0.0.0/8` route and broadcast/multicast routes apply system-wide only while the service runs and are removed on clean shutdown.
- Wine's missing PerfLib exports and Famatech's updater behavior remain external constraints.
