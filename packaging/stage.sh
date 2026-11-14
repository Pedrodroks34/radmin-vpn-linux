#!/bin/sh
set -eu

DEST="${1:?usage: stage.sh <destdir>}"
SRC_ROOT="${STAGE_ROOT:-$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)}"

install -Dm755 "$SRC_ROOT/target/release/radmin-vpn" "$DEST/usr/bin/radmin-vpn"

install -Dm0644 "$SRC_ROOT/packaging/systemd/radmin-vpn.service" \
  "$DEST/usr/lib/systemd/system/radmin-vpn.service"
install -Dm0644 "$SRC_ROOT/packaging/tmpfiles.d/radmin-vpn.conf" \
  "$DEST/usr/lib/tmpfiles.d/radmin-vpn.conf"

install -d -m 0755 "$DEST/usr/lib/radmin-vpn"
for f in rvpnnetmp.sys adapter_hook.dll rvpn_launcher.exe netsh.exe netsh64.exe drvinst.exe; do
  install -Dm0644 "$SRC_ROOT/build/$f" "$DEST/usr/lib/radmin-vpn/$f"
done

install -Dm0755 "$SRC_ROOT/target/release/librvpn_dnsfix.so" \
  "$DEST/usr/lib/radmin-vpn/rvpn_dnsfix.so"
install -Dm0755 "$SRC_ROOT/target/release/librvpn_reuseport.so" \
  "$DEST/usr/lib/radmin-vpn/rvpn_reuseport.so"

install -d -m 0755 "$DEST/usr/share/doc/radmin-vpn-linux"
install -Dm0644 "$SRC_ROOT/README.md" "$DEST/usr/share/doc/radmin-vpn-linux/README.md"
install -Dm0644 "$SRC_ROOT/LICENSE" "$DEST/usr/share/doc/radmin-vpn-linux/LICENSE"
for f in installation systemd headless architecture-rust building; do
  if [ -f "$SRC_ROOT/docs/$f.md" ]; then
    install -Dm0644 "$SRC_ROOT/docs/$f.md" "$DEST/usr/share/doc/radmin-vpn-linux/$f.md"
  fi
done
