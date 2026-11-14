# Building from source

Requires Rust, make and a MinGW-w64 cross toolchain (with DDK headers for the Windows miniport driver shim).

```bash
make
```

This builds the Rust workspace (`cargo build --release --workspace`) and the Windows interoperability shims under `build/`:

- `rvpnnetmp.sys` — NDIS miniport (MinGW + DDK headers)
- `adapter_hook.dll` — adapter hook (MinGW)
- `rvpn_launcher.exe` — Wine launcher (MinGW)
- `netsh.exe` / `netsh64.exe` — netsh wrappers (MinGW)
- `drvinst.exe` — no-op driver-install stub (MinGW)

The Linux-side runtime (TAP bridge, service, firewall/routes, lifecycle) is Rust. The PE artifacts are interop shims required by the Wine/Radmin protocol and are the reason a plain `cargo build` is not enough to produce a working install.

## Windows PE artifacts

Built with MinGW: `i686-w64-mingw32-gcc` (x86) and `x86_64-w64-mingw32-gcc` (x86_64), with DDK headers at `/usr/x86_64-w64-mingw32/include/ddk` (Debian/Ubuntu) or `/usr/x86_64-w64-mingw32/sys-root/mingw/include/ddk` (Fedora/RHEL).

## Tests

```bash
cargo test --all-features
cargo clippy --all-targets --all-features
cargo fmt --check
```

## Packages

CI (`.github/workflows/build.yml`) builds and publishes distributable packages on tags `v*` (and as workflow artifacts otherwise):

- `radmin-vpn-linux_<version>_amd64.deb` (Debian/Ubuntu)
- `radmin-vpn-linux-<version>-1.x86_64.rpm` (Fedora/RHEL)
- `radmin-vpn-linux-<version>-1-x86_64.pkg.tar.zst` (Arch/CachyOS)
- `radmin-vpn-linux-<version>-x86_64.tar.gz` (portable bundle with `install.sh`/`uninstall.sh`)

To build them locally (requires `dpkg-dev`, `rpm-tools`/`rpm`, and either `docker` or native `makepkg` for the Arch package):

```bash
make
./packaging/build-packages.sh "" ./dist        # version = git describe --tags or Cargo.toml
./packaging/build-arch-package.sh "" ./dist
```
