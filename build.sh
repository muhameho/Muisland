#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
if [[ "$(uname -s)" != Linux ]]; then echo 'Run build.sh on Linux; Windows uses build.ps1.' >&2; exit 1; fi
command -v cargo >/dev/null || { echo 'Install Rust using rustup first.' >&2; exit 1; }
cargo build --release --locked
./target/release/muisland --check-env
version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)"
stage="$(mktemp -d)"
trap 'rm -rf -- "$stage"' EXIT
mkdir -p dist
if [[ "${1:-}" == --deb ]]; then
  command -v dpkg-deb >/dev/null || { echo 'dpkg-deb is required for Debian packages.' >&2; exit 1; }
  install -Dm755 target/release/muisland "$stage/usr/bin/muisland"
  install -Dm644 linux/muisland.desktop "$stage/usr/share/applications/io.github.muhameho.Muisland.desktop"
  install -Dm644 linux/muisland.svg "$stage/usr/share/icons/hicolor/scalable/apps/io.github.muhameho.Muisland.svg"
  mkdir -p "$stage/DEBIAN"
  architecture="$(dpkg --print-architecture)"
  cat > "$stage/DEBIAN/control" <<EOF
Package: muisland
Version: $version
Architecture: $architecture
Maintainer: Muisland contributors
Depends: python3 (>= 3.10), python3-gi, python3-gi-cairo, python3-cryptography, gir1.2-gtk-3.0, gir1.2-webkit2-4.1, libsecret-tools
Recommends: gnome-keyring, gir1.2-gtklayershell-0.1, xwayland
Section: utils
Priority: optional
Description: Floating Claude and Codex account and usage companion
 GTK/WebKitGTK interface with encrypted local profile snapshots.
EOF
  dpkg-deb --root-owner-group --build "$stage" "dist/muisland_${version}_${architecture}.deb"
elif [[ "${1:-}" == --tar ]]; then
  folder="muisland-${version}-linux-$(uname -m)"
  mkdir -p "$stage/$folder"
  install -m755 target/release/muisland "$stage/$folder/muisland"
  install -m755 linux/install.sh "$stage/$folder/install.sh"
  install -m644 linux/muisland.desktop linux/muisland.svg README.md "$stage/$folder/"
  tar -C "$stage" -czf "dist/${folder}.tar.gz" "$folder"
elif [[ -n "${1:-}" ]]; then
  echo 'Usage: bash build.sh [--deb|--tar]' >&2; exit 1
else
  echo 'Built target/release/muisland. Run it, or use --preview for sample accounts.'
fi
