#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
prefix="${XDG_DATA_HOME:-$HOME/.local/share}"
bindir="$HOME/.local/bin"
runtime="$prefix/muisland/runtime"
if [[ "${1:-}" == --uninstall ]]; then
  rm -f -- "$bindir/muisland" "$prefix/applications/io.github.muhameho.Muisland.desktop" "$prefix/icons/hicolor/scalable/apps/io.github.muhameho.Muisland.svg"
  rm -f -- "${XDG_CONFIG_HOME:-$HOME/.config}/autostart/io.github.muhameho.Muisland.desktop"
  for name in muisland app.py backend.py island.html VERSION; do rm -f -- "$runtime/$name"; done
  rmdir -- "$runtime" 2>/dev/null || true
  echo 'Removed the launcher and binary. Encrypted accounts and backups were kept.'
  exit 0
fi
[[ -x ./muisland ]] || { echo 'Run this installer from the extracted Muisland Linux archive.' >&2; exit 1; }
./muisland --check-env
if [[ -f ./app.py && -f ./backend.py && -f ./VERSION ]]; then
  mkdir -p "$runtime" "$bindir"
  install -m755 ./muisland "$runtime/muisland"
  install -m644 ./app.py ./backend.py ./island.html ./VERSION "$runtime/"
  ln -sfn -- "$runtime/muisland" "$bindir/muisland"
else
  # Replace an old source launcher symlink without following it.
  [[ ! -L "$bindir/muisland" ]] || rm -f -- "$bindir/muisland"
  install -Dm755 ./muisland "$bindir/muisland"
fi
install -Dm644 ./muisland.svg "$prefix/icons/hicolor/scalable/apps/io.github.muhameho.Muisland.svg"
mkdir -p "$prefix/applications"
# Desktop Exec quoting is not shell quoting. Escape its reserved characters.
python3 - "$bindir/muisland" ./muisland.desktop "$prefix/applications/io.github.muhameho.Muisland.desktop" <<'PY'
import pathlib, sys
binary, source, target = sys.argv[1:]
escaped = binary.replace('\\', '\\\\').replace('"', '\\"').replace('`', '\\`').replace('$', '\\$').replace('%', '%%')
text = pathlib.Path(source).read_text().replace('Exec=muisland', 'Exec="' + escaped + '"')
pathlib.Path(target).write_text(text)
PY
command -v update-desktop-database >/dev/null && update-desktop-database "$prefix/applications" || true
echo 'Installed for this user. Quit the old Muisland instance when idle, then open Muisland from your application launcher.'
echo 'Add ~/.local/bin to PATH to use terminal commands. Autostart uses the installed absolute path.'
