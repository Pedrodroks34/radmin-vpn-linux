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

The unit file runs the service as `User=pedrodroks` (the machine this project is developed on). On other machines create that user, or better, override `User=` with a drop-in before enabling:

```sh
sudo systemctl edit radmin-vpn.service   # systemd opens an editor
# or non-interactively:
sudo mkdir -p /etc/systemd/system/radmin-vpn.service.d
printf '[Service]\nUser=%s\n' "$USER" | sudo tee /etc/systemd/system/radmin-vpn.service.d/user.conf
sudo systemctl daemon-reload
sudo systemctl enable --now radmin-vpn.service
```

## Dependencies

Common requirements:

- Linux with TUN/TAP (`/dev/net/tun`) — enable it early; on minimal kernels `modprobe tun` may be required
- nftables **or** iptables (firewall backend auto-detected)
- Wine (required to run the proprietary Radmin service/GUI)
- `iproute2`
- Rust toolchain (`rustup` recommended for cross-compilation targets)
- GTK4 development files **only** if you want the optional GTK4 filter UI (`--features gui`)

### Arch Linux / CachyOS

```sh
sudo pacman -S --needed rust nftables wine iproute2 curl
# For cross-compilation:
sudo pacman -S --needed rustup mingw-w64-gcc
rustup target add x86_64-pc-windows-gnu
```

### Debian / Ubuntu

```sh
sudo apt-get update
sudo apt-get install -y cargo rustc nftables wine iproute2 curl
# For cross-compilation:
sudo apt-get install -y rustup gcc-mingw-w64-x86-64
rustup target add x86_64-pc-windows-gnu
```

### Fedora

```sh
sudo dnf install -y rust cargo nftables wine iproute2 curl
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
sudo install -Dm755 target/release/radmin-vpn /usr/lib/radmin-vpn/radmin-vpn
sudo install -Dm644 packaging/systemd/radmin-vpn.service /usr/lib/systemd/system/radmin-vpn.service
sudo install -Dm644 packaging/tmpfiles.d/radmin-vpn.conf /usr/lib/tmpfiles.d/radmin-vpn.conf
sudo systemd-tmpfiles --create /usr/lib/tmpfiles.d/radmin-vpn.conf

# Optional LD_PRELOAD shims (recommended for Wine/Radmin compatibility):
sudo install -Dm755 target/release/librvpn_dnsfix.so /usr/lib/radmin-vpn/rvpn_dnsfix.so
sudo install -Dm755 target/release/librvpn_reuseport.so /usr/lib/radmin-vpn/rvpn_reuseport.so

sudo systemctl daemon-reload
```

## Enable the service

```sh
sudo systemctl enable --now radmin-vpn.service
systemctl status radmin-vpn.service
journalctl -u radmin-vpn.service -f
```

The service owns the TAP interface and records ownership in `/var/lib/radmin-vpn/tap.state`. Stopping the Radmin GUI does **not** remove the interface; stop the service instead.

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
logging in, restart the service once (`sudo systemctl restart radmin-vpn.service`)
to recreate the Wine session with your session's display; from then on the GUI
works normally.

`Restart=always` with the child monitor: if `RvControlSvc.exe` dies, the service
requests shutdown so systemd can restart it cleanly instead of running dead.

## Verify

```sh
# Service state and logs
systemctl status radmin-vpn.service
journalctl -u radmin-vpn.service --no-pager -n 50

# Interface
ip addr show radminvpn0   # after RvControlSvc registers, the Radmin IPv4 (and ULA IPv6) appears
ip route show             # 26.0.0.0/8 dev radminvpn0 plus 224.0.0.0/4 and 255.255.255.255/32

# What applies the address: the Radmin service calls netsh on every start;
# /usr/lib/radmin-vpn/netsh64.exe (installed as system32/netsh.exe) is selected
# via the HKCU\Software\Wine\DllOverrides "netsh.exe"="native" override that
# `configure_registry` writes, writes /tmp/radmin_netsh_cmd, and the Rust
# netsh relay applies it to radminvpn0. If the address never shows up, check
# /tmp/radmin_netsh_all.log and `journalctl -u radmin-vpn.service | grep -i relay`.

# Lock/ownership
ls -l /run/radmin-vpn/service.lock
cat /var/lib/radmin-vpn/tap.state

# Reload behavior (best-effort graceful restart)
systemctl reload radmin-vpn.service
journalctl -u radmin-vpn.service --no-pager -n 20

# Stop and cleanup
sudo systemctl stop radmin-vpn.service
ip link show radminvpn0 || echo "TAP removed"
```

## Uninstall

```sh
sudo systemctl disable --now radmin-vpn.service
sudo rm -f /usr/lib/systemd/system/radmin-vpn.service
sudo rm -f /usr/lib/tmpfiles.d/radmin-vpn.conf
sudo rm -rf /usr/lib/radmin-vpn /run/radmin-vpn
sudo rm -f /run/radmin-vpn/service.lock
sudo systemctl daemon-reload
```

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
