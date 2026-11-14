# NixOS installation and development guide

This project provides a Nix flake for building the Rust `radmin-vpn` binary and packaging the systemd unit.

## Temporary install / try it

```bash
nix run github:Pedrodroks34/radmin-vpn-linux
```

## Declarative install

Add the flake to your system configuration:

```nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs";
    radmin-vpn-linux.url = "github:Pedrodroks34/radmin-vpn-linux";
  };

  outputs = { self, nixpkgs, radmin-vpn-linux, ... }: {
    nixosConfigurations.your-host = nixpkgs.lib.nixosSystem {
      modules = [
        { nixpkgs.overlays = [ radmin-vpn-linux.overlays.default ]; }
      ];
    };
  };
}
```

Then add the package:

```nix
{ pkgs, ... }:
{
  environment.systemPackages = with pkgs; [ radmin-vpn ];
}
```

Rebuild with:

```bash
sudo nixos-rebuild switch --flake .
```

## Usage

The flake builds and installs the Rust `radmin-vpn` binary.

```bash
radmin-vpn --json-logs service
```

For a proper systemd-managed deployment, install/enable the bundled `radmin-vpn.service` unit. See [docs/installation.md](docs/installation.md) and [docs/systemd.md](docs/systemd.md).

## Development shell

```bash
git clone https://github.com/Pedrodroks34/radmin-vpn-linux.git
cd radmin-vpn-linux
nix develop
```

The dev shell provides Rust/cargo and the runtime dependencies needed for development. Build the workspace with:

```bash
cargo build --release --workspace
```

## Limitations (important)

The flake builds the Rust `radmin-vpn` binary and the systemd unit. It does **not** build the Windows interoperability shims (`rvpnnetmp.sys`, `adapter_hook.dll`, `rvpn_launcher.exe`, `netsh.exe`, `netsh64.exe`, `drvinst.exe`), which require a MinGW toolchain with DDK headers. The service **requires** those in `/usr/lib/radmin-vpn/` (it fails loudly without them), so the Nix package installs but the VPN service will not start until the shims are available — use `make install` or a `.deb`/`.rpm`/`.pkg.tar.zst` release, which ship them.

The flake is kept honest about this instead of silently producing a broken package.

## Note

The Rust rewrite is the maintained implementation. Legacy AppImage/shell packaging was removed; use the systemd unit and `make install`/`docs/installation.md` guidance instead.
