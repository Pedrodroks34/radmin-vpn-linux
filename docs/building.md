# Building from source

Requirements:

- Rust (stable, with `rustfmt` and `clippy` for development)
- TUN/TAP kernel support (`modprobe tun`)
- nftables or iptables
- Wine (to run the proprietary Radmin service/GUI)
- iproute2

Arch Linux:

```bash
sudo pacman -S --needed rust nftables wine iproute2 curl
```

Debian/Ubuntu:

```bash
sudo apt-get install -y cargo rustc nftables wine iproute2 curl
```

Fedora:

```bash
sudo dnf install -y rust cargo nftables wine iproute2 curl
```

Build:

```bash
cargo build --release --workspace
```

Install:

```bash
sudo make install
sudo systemctl daemon-reload
sudo systemctl enable --now radmin-vpn.service
```

Run directly:

```bash
sudo /usr/local/bin/radmin-vpn --json-logs service
```

The service owns the TAP interface and ships notify/watchdog integration. See [systemd.md](systemd.md).

## Windows PE artifacts

Windows components are not validated as part of this workspace build. They require a Windows cross target and DDK-compatible headers. The Rust modules under `src/protocol/` and `src/driver/` are structured so the same protocol logic can be reused in a Windows PE build.

## Tests

```bash
cargo test --all-features
cargo clippy --all-targets --all-features
cargo fmt --check
```
