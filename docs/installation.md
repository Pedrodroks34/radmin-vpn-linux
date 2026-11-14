# Installation

This rewrite provides a Rust `radmin-vpn` binary and a systemd unit that owns the TAP interface for the lifetime of the service. The proprietary Radmin VPN Windows binaries and Wine remain external dependencies.

## Pre-built packages

Tagged releases on GitHub publish ready-to-install packages (built by CI):

| Format | Distros | File |
|--------|---------|------|
| Debian | Debian, Ubuntu | `radmin-vpn-linux_<version>_amd64.deb` |
| RPM | Fedora, RHEL, openSUSE | `radmin-vpn-linux-<version>-1.x86_64.rpm` |
| Pacman | Arch, CachyOS | `radmin-vpn-linux-<version>-1-x86_64.pkg.tar.zst` |
| Tarball | any | `radmin-vpn-linux-<version>-x86_64.tar.gz` (with `install.sh`/`uninstall.sh`) |

The service is a **user** unit (`/usr/lib/systemd/user/radmin-vpn.service`), so it runs as whatever desktop user enables it — no accounts to create or match. Enable it with `systemctl --user`, not `systemctl`:

```sh
systemctl --user daemon-reload
systemctl --user enable --now radmin-vpn.service
# optional, for boot-time start before login:
loginctl enable-linger "$USER"
```

And it controls the TAP interface lifecycle for that user.

## Dependencies

Common requirements:

- Linux with TUN/TAP (`/dev/net/tun`) — enable it early; on minimal kernels `modprobe tun` may be required
- nftables **or** iptables (firewall backend auto-detected; on hardened hosts a one-off manual rule may be needed — see [systemd.md](systemd.md#privilege-model-file-capabilities))
- libcap (`setcap`, applied by the package postinst / `make install` — no network tool is needed for routes/addresses)
- Wine (required to run the proprietary Radmin service/GUI)
- Rust toolchain (`rustup` recommended for cross-compilation targets)
- GTK4 development files **only** if you want the optional GTK4 filter UI (`--features gui`)

### Arch Linux / CachyOS

```sh
sudo pacman -S --needed rust nftables wine curl
# For cross-compilation:
sudo pacman -S --needed rustup mingw-w64-gcc
rustup target add x86_64-pc-windows-gnu
```

### Debian / Ubuntu

```sh
sudo apt-get update
sudo apt-get install -y cargo rustc nftables wine curl
# For cross-compilation:
sudo apt-get install -y rustup gcc-mingw-w64-x86-64
rustup target add x86_64-pc-windows-gnu
```

### Fedora

```sh
sudo dnf install -y rust cargo nftables wine curl
# For cross-compilation:
sudo dnf install -y rustup mingw64-gcc
rustup target add x86_64-pc-windows-gnu
```

## Build

```sh
cargo build --release --workspace
```

The main binary is `target/release/radmin-vpn`. The workspace also builds the Wine-scoped LD_PRELOAD shims (`librvpn_dnsfix.so`, `librvpn_reuseport.so`).

## Install (manual)

```sh
sudo install -Dm755 target/release/radmin-vpn /usr/bin/radmin-vpn
sudo install -Dm644 packaging/systemd/radmin-vpn.service /usr/lib/systemd/user/radmin-vpn.service
# The binary carries file caps so the user service can manage the TAP and routes:
setcap 'cap_net_admin,cap_net_raw+ep' /usr/bin/radmin-vpn
# The runtime dir is created by the unit's RuntimeDirectory=
# on every start (with the correct owner); no tmpfiles.d snippet is needed.
# Optional LD_PRELOAD shims (recommended for Wine/Radmin compatibility):
sudo install -Dm755 target/release/librvpn_dnsfix.so /usr/lib/radmin-vpn/rvpn_dnsfix.so
sudo install -Dm755 target/release/librvpn_reuseport.so /usr/lib/radmin-vpn/rvpn_reuseport.so

systemctl --user daemon-reload
```

## Enable the service

```sh
systemctl --user enable --now radmin-vpn.service
systemctl --user status radmin-vpn.service
journalctl --user -u radmin-vpn.service -f
```

The service owns the TAP interface and records ownership in `$HOME/.local/state/radmin-vpn/tap.state`. Stopping the Radmin GUI does **not** remove the interface; stop the service instead (`systemctl --user stop radmin-vpn.service`).

## Graphical session and the Radmin GUI

The service runs the Radmin service (`RvControlSvc.exe`) without a display, but it
injects the graphical session environment (`DISPLAY`/`XAUTHORITY`, or
`WAYLAND_DISPLAY`/`XDG_RUNTIME_DIR`) into Wine processes when one is detected by
reading `/proc` of same-uid processes (e.g. plasmashell, kwin, Xwayland). This
makes the Wine session's desktop come up with a working driver, so windows like
`RvRvpnGui.exe` open normally — even when the service (re)starts after you have
logged in.

At boot before you log in there is no session to detect: the service still runs
the VPN headless (the VPN works), but the Wine desktop is created without a
display driver and the GUI will fail with `winediag:nodrv_CreateWindow`. After
logging in, restart the service once (`systemctl --user restart radmin-vpn.service`)
to recreate the Wine session with your session's display; from then on the GUI
works normally.

`Restart=always` with the child monitor: if `RvControlSvc.exe` dies, the service
requests shutdown so systemd can restart it cleanly instead of running dead.

## Verify

```sh
# Service state and logs
systemctl --user status radmin-vpn.service
journalctl --user -u radmin-vpn.service --no-pager -n 50

# Interface
ip addr show radminvpn0   # after RvControlSvc registers, the Radmin IPv4 (and ULA IPv6) appears
ip route show             # 26.0.0.0/8 dev radminvpn0 plus 224.0.0.0/4 and 255.255.255.255/32

# What applies the address: the Radmin service calls netsh on every start;
# /usr/lib/radmin-vpn/netsh64.exe (installed as system32/netsh.exe) is selected
# via the HKCU\Software\Wine\DllOverrides "netsh.exe"="native" override that
# `configure_registry` writes, writes /tmp/radmin_netsh_cmd, and the Rust
# netsh relay applies it to radminvpn0. If the address never shows up, check
# /tmp/radmin_netsh_all.log and `journalctl --user -u radmin-vpn.service | grep -i relay`.

# Lock/ownership
ls -l "$XDG_RUNTIME_DIR/radmin-vpn/service.lock"
cat "$HOME/.local/state/radmin-vpn/tap.state"

# Reload behavior (best-effort graceful restart)
systemctl --user reload radmin-vpn.service
journalctl --user -u radmin-vpn.service --no-pager -n 20

# Stop and cleanup
systemctl --user stop radmin-vpn.service
ip link show radminvpn0 || echo "TAP removed"
```

## Uninstall

```sh
systemctl --user disable --now radmin-vpn.service
sudo rm -f /usr/lib/systemd/user/radmin-vpn.service /usr/lib/systemd/system/radmin-vpn.service
sudo rm -rf /usr/lib/radmin-vpn "$XDG_RUNTIME_DIR/radmin-vpn" "$HOME/.local/state/radmin-vpn"
sudo systemctl daemon-reload
```

Packages newer than 1.2.2 remove the stale system-wide unit and `/usr/lib/tmpfiles.d/radmin-vpn.conf` automatically on install/upgrade (via postinst / post_install / tarball installer).

## Windows PE cross-build notes

The Rust modules under `src/protocol/` and `src/driver/` are structured for reuse in a Windows PE build.

```sh
rustup target add x86_64-pc-windows-gnu
# i686 for 32-bit components, if needed:
rustup target add i686-pc-windows-gnu

# Ensure the MinGW cross-compiler is on PATH (e.g. x86_64-w64-mingw32-gcc).
cargo build --target x86_64-pc-windows-gnu --release
```

This workspace does not currently ship a single-command Windows PE build pipeline; the commands above are the correct starting point and must be run on a host with the matching MinGW toolchain and DDK headers.
