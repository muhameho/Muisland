#!/usr/bin/env bash
# An isolated X server, authenticated with a disposable cookie. No host display.
set -euo pipefail
work="$(mktemp -d)"
trap 'if [[ -n "${server:-}" ]]; then kill "$server" 2>/dev/null || true; fi; rm -rf -- "$work"' EXIT
export DISPLAY=:99 XAUTHORITY="$work/authority"
cookie="$(python3 -c 'import secrets;print(secrets.token_hex(16))')"
touch "$XAUTHORITY"
chmod 600 "$XAUTHORITY"
xauth -f "$XAUTHORITY" add "$DISPLAY" . "$cookie"
Xvfb "$DISPLAY" -screen 0 1280x900x24 -nolisten tcp -auth "$XAUTHORITY" > "$work/server.log" 2>&1 &
server=$!
for attempt in $(seq 1 100); do
  [[ -S /tmp/.X11-unix/X99 ]] && break
  kill -0 "$server" || { cat "$work/server.log"; exit 1; }
  sleep 0.1
done
[[ -S /tmp/.X11-unix/X99 ]]
if [[ $# == 0 ]]; then set -- ./target/release/muisland --preview --smoke-test; fi
"$@"
