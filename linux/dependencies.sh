#!/usr/bin/env bash
# Print by default. Install only when the user explicitly chooses --install.
set -euo pipefail
[[ -r /etc/os-release ]] || { echo 'No os-release found. See the Linux guide for runtime requirements.' >&2; exit 1; }
. /etc/os-release
case "${ID:-}" in
  ubuntu|debian|linuxmint|pop|elementary|zorin|kali|raspbian|neon)
    packages=(python3 python3-gi python3-gi-cairo python3-cryptography gir1.2-gtk-3.0 gir1.2-webkit2-4.1 libsecret-tools gnome-keyring gir1.2-gtklayershell-0.1 xwayland)
    command=(apt-get install -y "${packages[@]}"); update=(apt-get update) ;;
  fedora|nobara)
    command=(dnf install -y python3 python3-gobject python3-cairo python3-cryptography gtk3 webkit2gtk4.1 libsecret gnome-keyring gtk-layer-shell xorg-x11-server-Xwayland); update=() ;;
  arch|manjaro|endeavouros|cachyos|garuda)
    command=(pacman -Syu --needed --noconfirm python python-gobject python-cairo python-cryptography gtk3 webkit2gtk-4.1 libsecret gnome-keyring gtk-layer-shell xorg-xwayland); update=() ;;
  opensuse-tumbleweed|opensuse-leap|opensuse)
    command=(zypper --non-interactive install python3 python3-gobject python3-gobject-Gdk python3-cairo python3-cryptography typelib-1_0-Gtk-3_0 typelib-1_0-WebKit2-4_1 typelib-1_0-GtkLayerShell-0_1 libsecret-tools gnome-keyring xwayland); update=() ;;
  alpine)
    command=(apk add python3 py3-gobject3 py3-cairo py3-cryptography gtk+3.0 webkit2gtk-4.1 libsecret gnome-keyring gtk-layer-shell xwayland bash); update=() ;;
  *) echo "No automatic dependency recipe for ${ID:-this distribution}. Python 3.10+, GTK3, WebKitGTK4.1/4.0, PyGObject/Cairo, cryptography, secret-tool and a Secret Service provider are required. See README.md." >&2; exit 1 ;;
esac
if [[ "${1:-}" != --install ]]; then
  echo "Dependency recipe for ${PRETTY_NAME:-$ID}:"
  [[ ${#update[@]} == 0 ]] || { printf '  '; printf '%q ' "${update[@]}"; printf '\n'; }
  printf '  '; printf '%q ' "${command[@]}"; printf '\n'
  echo 'Run bash dependencies.sh --install to install these system packages.'
  exit 0
fi
admin=()
if [[ "$(id -u)" != 0 ]]; then
  command -v sudo >/dev/null || { echo 'Run this command as root or install sudo.' >&2; exit 1; }
  admin=(sudo)
fi
[[ ${#update[@]} == 0 ]] || "${admin[@]}" "${update[@]}"
"${admin[@]}" "${command[@]}"
