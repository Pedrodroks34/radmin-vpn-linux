# Radmin VPN for Linux

Run [Radmin VPN](https://www.radmin-vpn.com/) on Linux through Wine: join networks, see peers, host and join game servers, no Windows VM.

This repository has been rewritten from a collection of shell scripts and C/C++ sources into a Rust workspace. The proprietary Radmin VPN binaries and Wine remain external dependencies.

For the architecture map, see [docs/architecture-rust.md](docs/architecture-rust.md). For install and systemd details, see [docs/installation.md](docs/installation.md) and [docs/systemd.md](docs/systemd.md).

## Audit it yourself instead of claiming it's a RAT

It asks for root privileges to manage the VPN adapter, so be suspicious of it. Then check: every line of this wrapper is open source, and it takes less time than writing the accusation. (Radmin itself is Famatech's, downloaded from them and run unmodified.)

- The maintained components are Rust, in `src/`, `crates/`, `Cargo.toml` and `Cargo.lock`. Review at your own pace.
- The Rust build and test workflow is enforced in CI ([`.github/workflows/build.yml`](.github/workflows/build.yml)): `cargo fmt --check`, `cargo clippy --all-targets --all-features`, and `cargo test --all-features`.
- What needs elevated privileges: the `radminvpn0` adapter setup and configuration (ioctl-based where possible, netlink in-process where a subprocess cannot hold the privilege), the firewall rules (nft/iptables/firewalld, tagged for cleanup, best-effort), and the routes — granted narrowly through `setcap cap_net_admin,cap_net_raw=ep` on the binary itself, not via a root service. The netsh relay only accepts validated address strings and never passes them through a shell.
- What gets downloaded: the Radmin installer, from `download.radmin-vpn.com`, checked against a sha256 pinned in the installer module. Nothing else.
- What Radmin itself sends is Famatech's business, not ours.

## Build

```bash
cargo build --release --workspace
```

This produces the `radmin-vpn` binary and, when built, the Wine-scoped LD_PRELOAD shims (`librvpn_dnsfix.so`, `librvpn_reuseport.so`).

## Install

```bash
cargo build --release --workspace
sudo make install
radmin-vpn setup                    # creates the Wine prefix and installs Radmin VPN
systemctl --user enable --now radmin-vpn.service
```

See [docs/installation.md](docs/installation.md) for manual install steps and [docs/systemd.md](docs/systemd.md) for unit hardening details.

## Run

With the service:

```bash
systemctl --user start radmin-vpn.service
systemctl --user reload radmin-vpn.service   # graceful restart (see docs/systemd.md)
journalctl --user -u radmin-vpn.service -f
```

Directly:

```bash
sudo target/release/radmin-vpn --json-logs service
```

The service owns the TAP interface for its lifetime. Closing the Radmin GUI does not remove the adapter.

## Troubleshooting

When you open an issue, attach:

```bash
journalctl -u radmin-vpn.service -b --no-pager
```

Known limitations:

- GTK4-based filter UI is optional (`gui` Cargo feature) and requires GTK4 development files.
- Windows PE artifacts (`rvpnnetmp.sys`, `adapter_hook.dll`, `rvpn_launcher.exe`, `netsh.exe`, `netsh64.exe`) require a Windows cross-build (`x86_64-pc-windows-gnu`) and MinGW DDK headers; the Rust modules are structured so that target can reuse the shared protocol logic, but a validated PE build is not yet guaranteed by this workspace alone.
- Wine and Radmin behavior are affected by the chosen Wine release and Famatech's own updater behavior.

## License

GPL-3.0, see [LICENSE](LICENSE). In spirit this is public domain; the GPL is there because it explicitly protects reverse engineering for interoperability, which is what this project does.

Radmin VPN is proprietary software by Famatech Corp. This project ships interoperability tools only, no Famatech code.
