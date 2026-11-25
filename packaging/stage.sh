#!/bin/sh
set -eu

DEST="${1:?usage: stage.sh <destdir>}"
SRC_ROOT="${STAGE_ROOT:-$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)}"

install -Dm755 "$SRC_ROOT/target/release/radmin-vpn" "$DEST/usr/bin/radmin-vpn"

install -Dm755 "$SRC_ROOT/packaging/radmin-gui" "$DEST/usr/lib/radmin-vpn/radmin-gui"
install -Dm0644 "$SRC_ROOT/packaging/radmin-vpn.desktop" "$DEST/usr/share/applications/radmin-vpn.desktop"
for sz in 16 32 48; do
  install -Dm0644 "$SRC_ROOT/packaging/icons/hicolor/${sz}x${sz}/apps/radmin-vpn.png" \
    "$DEST/usr/share/icons/hicolor/${sz}x${sz}/apps/radmin-vpn.png"
done

install -Dm0644 "$SRC_ROOT/packaging/systemd/radmin-vpn.service" \
  "$DEST/usr/lib/systemd/user/radmin-vpn.service"

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
