#!/usr/bin/env bash
# Launch the release build, wait for it to render, and save a screenshot.
# Uses Xvfb when no display is available (headless CI / VMs).
#
#   scripts/screenshot.sh [output.png] [seconds-to-wait]
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
bin="$root/target/release/universal-calendar-desktop"
out="${1:-$root/docs/screenshots/month.png}"
wait_s="${2:-6}"
mkdir -p "$(dirname "$out")"
[[ -x "$bin" ]] || { echo "Build first: (cd apps/desktop && npx tauri build)" >&2; exit 1; }

capture() {
  "$bin" &
  local pid=$!
  sleep "$wait_s"
  import -window root "$out"
  kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
}

if [[ -z "${DISPLAY:-}" && -z "${WAYLAND_DISPLAY:-}" ]]; then
  # Re-run this script inside a virtual X server with software rendering and
  # a private D-Bus session (a headless login's bus may lack the desktop
  # portal, which stalls WebKitGTK start-up).
  WEBKIT_DISABLE_DMABUF_RENDERER=1 LIBGL_ALWAYS_SOFTWARE=1 \
    xvfb-run -a -s "-screen 0 1440x900x24" dbus-run-session -- "$0" "$out" "$wait_s"
  exit $?
else
  capture
  echo "Saved $out"
fi
