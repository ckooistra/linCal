#!/usr/bin/env bash
# Adds Universal Calendar to your application menu and to "Open with" for
# .ics files, using the build in this checkout. For running from source
# without installing a package; the deb, rpm and AppImage do this themselves.
#
#   scripts/install-desktop-entry.sh            # menu entry and .ics handler
#   scripts/install-desktop-entry.sh --default  # also make it the default app for .ics files
#   scripts/install-desktop-entry.sh --remove   # undo
#
# Everything goes to your user directories (~/.local/share); nothing needs root.
# It uses target/release/universal-calendar: build it first with
#   (cd apps/desktop && npx tauri build --no-bundle)
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
data="${XDG_DATA_HOME:-$HOME/.local/share}"
entry="$data/applications/universal-calendar.desktop"
icon="$data/icons/hicolor/256x256/apps/universal-calendar.png"
bin="$root/target/release/universal-calendar"

refresh() {
  command -v update-desktop-database >/dev/null && update-desktop-database "$data/applications" || true
  command -v kbuildsycoca6 >/dev/null && kbuildsycoca6 >/dev/null 2>&1 || true
}

if [[ "${1:-}" == "--remove" ]]; then
  rm -f "$entry" "$icon"
  refresh
  echo "Removed $entry"
  exit 0
fi

[[ -x "$bin" ]] || { echo "No build at $bin; run: (cd apps/desktop && npx tauri build --no-bundle)" >&2; exit 1; }

mkdir -p "$(dirname "$entry")" "$(dirname "$icon")"
cp "$root/apps/desktop/src-tauri/icons/256x256.png" "$icon"
cat >"$entry" <<EOF
[Desktop Entry]
Type=Application
Name=Universal Calendar
GenericName=Calendar
Comment=Fast, local-first calendar (built from $root)
Exec="$bin" %F
Icon=universal-calendar
Terminal=false
Categories=Office;Calendar;
MimeType=text/calendar;
StartupNotify=true
StartupWMClass=universal-calendar
EOF
refresh
echo "Installed $entry"

if [[ "${1:-}" == "--default" ]]; then
  xdg-mime default universal-calendar.desktop text/calendar
  echo "Universal Calendar now opens .ics files."
else
  echo "To open .ics files with it by default, run again with --default (or use Open With in the file manager)."
fi
