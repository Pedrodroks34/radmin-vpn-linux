#!/bin/sh
set -eu

VERSION="${1:-}"
OUT="${2:-dist}"

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"

if [ -z "$VERSION" ]; then
  VERSION="$(git -C "$ROOT" describe --tags --abbrev=0 2>/dev/null || true)"
  VERSION="${VERSION#v}"
fi
if [ -z "$VERSION" ]; then
  VERSION="$(grep -m1 '^version' "$ROOT/Cargo.toml" | cut -d'"' -f2)"
fi
if [ -z "$VERSION" ]; then
  echo "error: could not determine package version" >&2
  exit 1
fi

NAME="radmin-vpn-linux"
mkdir -p "$OUT"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

sh "$ROOT/packaging/stage.sh" "$WORK/stage"

# ---------- .tar.gz portable bundle ----------
mkdir -p "$WORK/tar/$NAME-$VERSION-x86_64"
cp -a "$WORK/stage/." "$WORK/tar/$NAME-$VERSION-x86_64/"
cat > "$WORK/tar/$NAME-$VERSION-x86_64/install.sh" <<'EOF'
#!/bin/sh
set -eu
cd "$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"

PREFIX="${PREFIX:-/usr}"
SYSD="${SYSTEMDDIR:-/usr/lib/systemd/system}"
TMPF="${TMPFILESDIR:-/usr/lib/tmpfiles.d}"
DOC="${DOCDIR:-/usr/share/doc/radmin-vpn-linux}"

if [ "$PREFIX" = "/usr" ] && [ "$(id -u)" -ne 0 ]; then
  echo "this installer writes to /usr (root required); re-run with sudo or doas" >&2
  exit 1
fi

install -Dm755 usr/bin/radmin-vpn "$PREFIX/bin/radmin-vpn"
install -Dm0644 usr/lib/systemd/system/radmin-vpn.service "$SYSD/radmin-vpn.service"
install -Dm0644 usr/lib/tmpfiles.d/radmin-vpn.conf "$TMPF/radmin-vpn.conf"
install -d -m 0755 "$PREFIX/lib/radmin-vpn"
for f in usr/lib/radmin-vpn/*; do
  case "$f" in
    *.so) install -Dm0755 "$f" "$PREFIX/lib/radmin-vpn/$(basename "$f")" ;;
    *)    install -Dm0644 "$f" "$PREFIX/lib/radmin-vpn/$(basename "$f")" ;;
  esac
done
install -d -m 0755 "$DOC"
for f in README.md LICENSE; do
  [ -f "usr/share/doc/radmin-vpn-linux/$f" ] && install -Dm0644 "usr/share/doc/radmin-vpn-linux/$f" "$DOC/$f"
done

echo "installed into $PREFIX"
echo "enable the service with: systemctl daemon-reload && systemctl enable --now radmin-vpn.service"
EOF
cat > "$WORK/tar/$NAME-$VERSION-x86_64/uninstall.sh" <<'EOF'
#!/bin/sh
set -eu
cd "$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"

PREFIX="${PREFIX:-/usr}"
SYSD="${SYSTEMDDIR:-/usr/lib/systemd/system}"
TMPF="${TMPFILESDIR:-/usr/lib/tmpfiles.d}"
DOC="${DOCDIR:-/usr/share/doc/radmin-vpn-linux}"

if [ "$PREFIX" = "/usr" ] && [ "$(id -u)" -ne 0 ]; then
  echo "root required to uninstall from /usr" >&2
  exit 1
fi

systemctl disable --now radmin-vpn.service >/dev/null 2>&1 || true
rm -f "$PREFIX/bin/radmin-vpn" "$SYSD/radmin-vpn.service" "$TMPF/radmin-vpn.conf"
rm -rf "$PREFIX/lib/radmin-vpn" "$DOC"
systemctl daemon-reload
echo "uninstalled"
EOF
chmod 755 "$WORK/tar/$NAME-$VERSION-x86_64/install.sh" "$WORK/tar/$NAME-$VERSION-x86_64/uninstall.sh"
tar -C "$WORK/tar" -czf "$OUT/$NAME-$VERSION-x86_64.tar.gz" "$NAME-$VERSION-x86_64"

# ---------- .deb (dpkg-deb) ----------
DEBROOT="$WORK/deb"
mkdir -p "$DEBROOT"
cp -a "$WORK/stage/." "$DEBROOT/"
mkdir -p "$DEBROOT/DEBIAN"
cat > "$DEBROOT/DEBIAN/control" <<EOF
Package: $NAME
Version: $VERSION
Section: net
Priority: optional
Architecture: amd64
Maintainer: Pedrodroks34 <droks5804@gmail.com>
Depends: wine | wine64 | wine32, iproute2, iptables, systemd, libc6
Homepage: https://github.com/Pedrodroks34/radmin-vpn-linux
Description: Radmin VPN client for Linux (Wine + TAP bridge)
 Runs the proprietary Radmin VPN Windows service and GUI under Wine,
 with the Linux side (TAP adapter ownership, Ethernet bridge, netsh
 relay, firewall and routes, systemd lifecycle) implemented in Rust.
EOF
cat > "$DEBROOT/DEBIAN/postinst" <<'EOF'
#!/bin/sh
set -e
if [ -d /run/systemd/system ]; then
  systemctl daemon-reload
  systemctl enable radmin-vpn.service >/dev/null 2>&1 || true
fi
exit 0
EOF
cat > "$DEBROOT/DEBIAN/prerm" <<'EOF'
#!/bin/sh
set -e
if [ -d /run/systemd/system ]; then
  systemctl disable --now radmin-vpn.service >/dev/null 2>&1 || true
fi
exit 0
EOF
cat > "$DEBROOT/DEBIAN/postrm" <<'EOF'
#!/bin/sh
set -e
if [ -d /run/systemd/system ]; then
  systemctl daemon-reload
fi
exit 0
EOF
chmod 755 "$DEBROOT/DEBIAN/postinst" "$DEBROOT/DEBIAN/prerm" "$DEBROOT/DEBIAN/postrm"
if ! command -v dpkg-deb >/dev/null 2>&1; then
  echo "error: dpkg-deb not found (install dpkg-dev)" >&2
  exit 1
fi
dpkg-deb --root-owner-group --build "$DEBROOT" "$OUT/${NAME}_${VERSION}_amd64.deb"

# ---------- .rpm (rpmbuild) ----------
RPMTOP="$WORK/rpm"
mkdir -p "$RPMTOP/BUILD" "$RPMTOP/RPMS" "$RPMTOP/SOURCES" "$RPMTOP/SPECS" "$RPMTOP/SRPMS"
cp -a "$WORK/stage/." "$RPMTOP/BUILD/stage"
cat > "$RPMTOP/SPECS/$NAME.spec" <<EOF
Name:           $NAME
Version:        $VERSION
Release:        1%{?dist}
Summary:        Radmin VPN client for Linux (Wine + TAP bridge)
License:        GPL-3.0-only
URL:            https://github.com/Pedrodroks34/radmin-vpn-linux
BuildArch:      x86_64
Requires:       wine, iproute, iptables, systemd

%description
Runs the proprietary Radmin VPN Windows service and GUI under Wine,
with the Linux side (TAP adapter ownership, Ethernet bridge, netsh relay,
firewall and routes, systemd lifecycle) implemented in Rust.

%install
mkdir -p %{buildroot}
cp -a %{stagedir}/. %{buildroot}/

%files
/usr/bin/radmin-vpn
/usr/lib/radmin-vpn
/usr/lib/systemd/system/radmin-vpn.service
/usr/lib/tmpfiles.d/radmin-vpn.conf
/usr/share/doc/radmin-vpn-linux

%post
if [ -d /run/systemd/system ]; then
    systemctl daemon-reload || :
    systemctl enable radmin-vpn.service >/dev/null 2>&1 || :
fi

%preun
if [ "\$1" = "0" ] && [ -d /run/systemd/system ]; then
    systemctl disable --now radmin-vpn.service >/dev/null 2>&1 || :
fi

%postun
if [ -d /run/systemd/system ]; then
    systemctl daemon-reload || :
fi
EOF
if ! command -v rpmbuild >/dev/null 2>&1; then
  echo "error: rpmbuild not found (install rpm-build/rpm)" >&2
  exit 1
fi
rpmbuild -bb --define "_topdir $RPMTOP" --define "stagedir $RPMTOP/BUILD/stage" "$RPMTOP/SPECS/$NAME.spec"
mv "$RPMTOP"/RPMS/x86_64/*.rpm "$OUT/${NAME}-${VERSION}-1.x86_64.rpm"

echo "built:"
ls -l "$OUT"
