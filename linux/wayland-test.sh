#!/usr/bin/env bash
# Isolated headless Sway session; no real app credentials or user desktop.
set -euo pipefail
runtime="$(mktemp -d)"
trap 'if [[ -n "${compositor:-}" ]]; then kill "$compositor" 2>/dev/null || true; fi; rm -rf -- "$runtime"' EXIT
export XDG_RUNTIME_DIR="$runtime" WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1
printf 'output * resolution 1280x900
' > "$runtime/sway.conf"
sway --unsupported-gpu -c "$runtime/sway.conf" > "$runtime/sway.log" 2>&1 &
compositor=$!
for attempt in $(seq 1 100); do
  endpoint="$(find "$runtime" -maxdepth 1 -type s -name 'wayland-*' -print -quit)"
  [[ -n "$endpoint" ]] && break
  sleep 0.1
done
[[ -n "${endpoint:-}" ]] || { cat "$runtime/sway.log"; exit 1; }
export WAYLAND_DISPLAY="$(basename "$endpoint")" GDK_BACKEND=wayland
unset DISPLAY
"${MUISLAND_TEST_BIN:-./target/release/muisland}" --preview --smoke-test
