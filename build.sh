#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
if [[ "$(uname -s)" != Linux ]]; then echo 'Run build.sh on Linux; Windows uses build.ps1.' >&2; exit 1; fi
command -v cargo >/dev/null || { echo 'Install Rust using rustup first.' >&2; exit 1; }
cargo build --release
./target/release/muisland --check-env
if [[ "${1:-}" == --deb ]]; then
  command -v dpkg-deb >/dev/null || { echo 'dpkg-deb is required for Debian packages.' >&2; exit 1; }
  package="$(mktemp -d)"
  trap 'rm -rf -- "$package"' EXIT
  install -Dm755 target/release/muisland "$package/usr/bin/muisland"
  install -Dm644 linux/muisland.desktop "$package/usr/share/applications/muisland.desktop"
  mkdir -p "$package/DEBIAN" dist
  architecture="$(dpkg --print-architecture)"
  cat > "$package/DEBIAN/control" <<EOF
Package: muisland
Version: 0.4.0
Architecture: $architecture
Maintainer: Muisland contributors
Depends: python3 (>= 3.10), python3-gi, python3-gi-cairo, python3-cryptography, gir1.2-gtk-3.0, gir1.2-webkit2-4.1, libsecret-tools
Section: utils
Priority: optional
Description: Floating Claude and Codex account and usage companion
 GTK/WebKitGTK interface with encrypted local profile snapshots.
EOF
  dpkg-deb --root-owner-group --build "$package" "dist/muisland_0.4.0_${architecture}.deb"
  echo "Created dist/muisland_0.4.0_${architecture}.deb"
else
  echo 'Built target/release/muisland. Run it, or use --preview for sample accounts.'
fi
