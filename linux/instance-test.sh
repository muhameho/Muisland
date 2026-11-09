#!/usr/bin/env bash
set -euo pipefail
export MESA_SHADER_CACHE_DISABLE=true
export XDG_CACHE_HOME="$(mktemp -d)"
trap 'if [[ -n "${island:-}" ]]; then kill "$island" 2>/dev/null || true; fi; rm -rf -- "$XDG_CACHE_HOME" || true' EXIT
./target/release/muisland --preview --no-network > "$XDG_CACHE_HOME/instance.log" 2>&1 &
island=$!
for attempt in $(seq 1 100); do
  [[ -S "$XDG_CACHE_HOME/muisland/preview.sock" ]] && break
  kill -0 "$island" || { cat "$XDG_CACHE_HOME/instance.log"; exit 1; }
  sleep 0.1
done
[[ -S "$XDG_CACHE_HOME/muisland/preview.sock" ]]
./target/release/muisland --preview --show
./target/release/muisland --preview --reset-position
kill -0 "$island"
./target/release/muisland --preview --quit
wait "$island"
island=''
echo 'Single instance activation, recovery, and quit: OK'
