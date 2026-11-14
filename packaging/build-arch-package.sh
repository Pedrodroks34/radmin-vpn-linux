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

PREP="$(mktemp -d)"
trap 'rm -rf "$PREP" 2>/dev/null || true' EXIT

cp -a "$ROOT/packaging" "$PREP/pkg"
sed -i "s/@PKGVER@/$VERSION/" "$PREP/pkg/arch/PKGBUILD"
mkdir -p "$OUT"
chmod -R u+rwX "$PREP"

image="${ARCHLINUX_IMAGE:-archlinux:latest}"

if command -v docker >/dev/null 2>&1; then
  echo "building Arch package with docker ($image)"
  docker run --rm \
    -e STAGE_ROOT=/src \
    -e PACKAGER="Pedrodroks34 <droks5804@gmail.com>" \
    -v "$ROOT:/src:Z" \
    -v "$PREP:/work:Z" \
    -w /work/pkg/arch \
    "$image" \
    bash -lc '
      set -eu
      pacman -Sy --noconfirm --needed base-devel sudo make
      useradd -m builder 2>/dev/null || true
      chmod -R a+rwX /work
      chown -R builder:builder /work
      su builder -s /bin/bash -c "STAGE_ROOT=/src PACKAGER=\"$PACKAGER\" makepkg -f --noconfirm --skipinteg --nodeps"
    '
  cp "$PREP"/pkg/arch/*.pkg.tar.zst "$OUT"/
  docker run --rm -v "$PREP:/work:Z" "$image" sh -c 'rm -rf /work/* /work/.[!.]*'
else
  if ! command -v makepkg >/dev/null 2>&1; then
    echo "error: docker or makepkg required to build the .pkg.tar.zst" >&2
    exit 1
  fi
  echo "building Arch package with native makepkg"
  (cd "$PREP/pkg/arch" && STAGE_ROOT="$ROOT" PACKAGER="${PACKAGER:-radmin-vpn-linux CI}" makepkg -f --noconfirm --skipinteg --nodeps)
  cp "$PREP"/pkg/arch/*.pkg.tar.zst "$OUT"/
fi

rm -rf "$PREP" 2>/dev/null || true

echo "built Arch package in $OUT"
ls -l "$OUT"
