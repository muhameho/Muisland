#!/usr/bin/env bash
# Runs only inside a fresh test DBus session; uses no real accounts.
set -euo pipefail
mkdir -p "${XDG_CACHE_HOME:-$HOME/.cache}"
export XDG_DATA_HOME="$(mktemp -d)" XDG_CONFIG_HOME="$(mktemp -d)"
trap 'rm -rf -- "$XDG_DATA_HOME" "$XDG_CONFIG_HOME"' EXIT
printf 'muisland-disposable-test-keyring' | gnome-keyring-daemon --unlock --components=secrets
PYTHONPATH=linux python3 - <<'PY'
import backend, pathlib
first = backend.Service(network=False)
first.handle('settings', {'theme': 2})
second = backend.Service(network=False)
assert second.settings['theme'] == 2
assert len(first.vault.key) == 32
assert first.vault.key == second.vault.key
assert b'"theme"' not in (first.vault.root/'settings.bin').read_bytes()
print('Real Secret Service encryption and restart: OK')
PY
