{
  description = "Radmin VPN for Linux via Wine (Rust rewrite)";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, utils }:
    (utils.lib.eachSystem [ "x86_64-linux" ] (system:
      let
        pkgs = import nixpkgs { inherit system; };

        radmin = pkgs.rustPlatform.buildRustPackage rec {
          pname = "radmin-vpn";
          version = "1.0.0";
          src = self;
          cargoLock.lockFile = ./Cargo.lock;

          nativeBuildInputs = [
            pkgs.pkg-config
          ];

          buildInputs = [
            pkgs.nftables
            pkgs.iproute2
          ];

          installPhase = ''
            runHook preInstall

            # The derivation builds with --target <triple>, so cargo puts the
            # artifacts under target/<triple>/release, not target/release.
            targetDir=target/release
            for d in target/*-gnu/release target/*-musl/release; do
              if [ -x "$d/radmin-vpn" ]; then
                targetDir="$d"
                break
              fi
            done
            echo "installPhase: using cargo output directory: $targetDir"

            install -d $out/bin $out/lib/radmin-vpn $out/lib/systemd/system $out/lib/tmpfiles.d
            install -m755 "$targetDir/radmin-vpn" $out/bin/radmin-vpn

            if [ -f "$targetDir/librvpn_dnsfix.so" ]; then
              install -m644 "$targetDir/librvpn_dnsfix.so" $out/lib/radmin-vpn/rvpn_dnsfix.so
            fi
            if [ -f "$targetDir/librvpn_reuseport.so" ]; then
              install -m644 "$targetDir/librvpn_reuseport.so" $out/lib/radmin-vpn/rvpn_reuseport.so
            fi

            cp packaging/systemd/radmin-vpn.service $out/lib/systemd/system/radmin-vpn.service
            cp packaging/tmpfiles.d/radmin-vpn.conf $out/lib/tmpfiles.d/radmin-vpn.conf

            runHook postInstall
          '';
        };
      in
      {
        devShells.default = pkgs.mkShell {
          nativeBuildInputs = with pkgs; [
            cargo
            rustc
            rustfmt
            clippy
            pkg-config
            wineWow64Packages.stable
            nftables
            iproute2
          ];
        };

        packages.default = radmin;
      }))

    // {
      overlays.default = final: prev: {
        radmin-vpn = self.packages.${final.system}.default;
      };
    };
}
