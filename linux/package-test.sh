#!/usr/bin/env bash
set -euo pipefail
work="$(mktemp -d)"
trap 'rm -rf -- "$work"' EXIT
archive="${1:-$(find dist -maxdepth 1 -name 'muisland-*-linux-*.tar.gz' -print -quit)}"
[[ -n "$archive" ]]
tar -xzf "$archive" -C "$work"
folder="$(find "$work" -mindepth 1 -maxdepth 1 -type d -print -quit)"
export HOME="$work/home with spaces"
export XDG_DATA_HOME="$HOME/.local/share" XDG_CONFIG_HOME="$HOME/.config"
mkdir -p "$HOME"
bash "$folder/install.sh"
"$HOME/.local/bin/muisland" --version
python3 - <<'PY'
import gi, os, tempfile
gi.require_version('Gio', '2.0')
from gi.repository import Gio, GLib
from pathlib import Path
home = Path.home()
desktop = Gio.DesktopAppInfo.new_from_filename(str(home / '.local/share/applications/io.github.muhameho.Muisland.desktop'))
assert desktop is not None
# Actually launch through the desktop parser. get_executable() on older GLib
# splits quoted commands at the first space, so it is not a launch assertion.
probe = home / 'probe.desktop'
source = home / '.local/share/applications/io.github.muhameho.Muisland.desktop'
probe.write_text(source.read_text().replace('--show', '--version'))
entry = Gio.DesktopAppInfo.new_from_filename(str(probe))
pids = []
with tempfile.TemporaryFile() as output:
    assert entry.launch_uris_as_manager_with_fds([], None, GLib.SpawnFlags.DO_NOT_REAP_CHILD, None, None, lambda app, pid, data: pids.append(pid), None, 0, output.fileno(), output.fileno())
    assert pids
    assert os.waitpid(pids[0], 0)[1] == 0
    output.seek(0)
    assert output.read().strip().startswith(b'Muisland ')
data = home / '.local/share/muisland'
data.mkdir(exist_ok=True)
(data/'kept.txt').write_text('disposable-profile-placeholder')
PY
bash "$folder/install.sh" --uninstall
[[ ! -e "$HOME/.local/bin/muisland" ]]
[[ -f "$HOME/.local/share/muisland/kept.txt" ]]
echo 'Portable install, desktop Exec paths with spaces, and profile-preserving uninstall: OK'
