#!/usr/bin/env bash
# Installs the built packages on clean Linux distributions (Docker) and
# starts the app in each, as a check that the packages are complete:
#
#   deb       on Ubuntu 24.04 and Debian 12  (apt installs its dependencies)
#   rpm       on Fedora 41                    (dnf installs its dependencies)
#   AppImage  on Debian 12 without WebKit     (it must bring its own)
#
# In each container: install, validate the desktop entry and AppStream
# metadata, start the app in a virtual display, check it is still running
# after a while, and save a screenshot.
#
#   scripts/test-packages.sh [bundle-dir] [output-dir]
#
# bundle-dir defaults to target/release/bundle (build with `npx tauri build`);
# screenshots and logs go to output-dir (default: target/package-test).
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
bundle="${1:-$root/target/release/bundle}"
out="${2:-$root/target/package-test}"
mkdir -p "$out"

deb=$(ls "$bundle"/deb/*.deb 2>/dev/null | head -1)
rpm=$(ls "$bundle"/rpm/*.rpm 2>/dev/null | head -1)
appimage=$(ls "$bundle"/appimage/*.AppImage 2>/dev/null | head -1)
[[ -n "$deb$rpm$appimage" ]] || { echo "No packages in $bundle; run: (cd apps/desktop && npx tauri build)" >&2; exit 1; }

# Runs inside each container after installation: start the app (a fresh
# profile, passwords in memory), screenshot, and report.
read -r -d '' run_app <<'EOF' || true
set -e
export UC_DATA_DIR=/tmp/uc-data UC_CREDENTIALS=memory
export WEBKIT_DISABLE_DMABUF_RENDERER=1 LIBGL_ALWAYS_SOFTWARE=1
# WebKit's sandbox needs user namespaces, which containers usually lack.
export WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1
xvfb-run -a -s "-screen 0 1280x800x24" dbus-run-session -- sh -c '
  "$0" >/out/$NAME.log 2>&1 &
  pid=$!
  sleep 12
  if kill -0 $pid 2>/dev/null; then echo "running after 12 s"; else echo "EXITED"; fi
  import -window root /out/$NAME.png
  kill $pid 2>/dev/null || true
' "$APP"
EOF

check_metadata='
desktop-file-validate /usr/share/applications/universal-calendar.desktop && echo "desktop entry: valid"
appstreamcli validate --no-net /usr/share/metainfo/org.universalcalendar.desktop.metainfo.xml | tail -1
'

status=0
run() {
  local name=$1 image=$2 setup=$3
  echo "== $name ($image)"
  if docker run --rm -e NAME="$name" -v "$bundle:/bundle:ro" -v "$out:/out" "$image" bash -c "$setup
$run_app" 2>&1 | tail -6 | tee "$out/$name.result" | grep -q "running after"; then
    echo "   ok — screenshot: $out/$name.png"
  else
    echo "   FAILED — see $out/$name.result and $out/$name.log"
    status=1
  fi
}

apt_tools="xvfb xauth dbus imagemagick desktop-file-utils appstream"

if [[ -n "$deb" ]]; then
  for image in ubuntu:24.04 debian:12; do
    run "deb-${image/:/-}" "$image" "
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq >/dev/null
apt-get install -y -qq /bundle/deb/$(basename "$deb") $apt_tools >/dev/null
$check_metadata
export APP=universal-calendar"
  done
fi

if [[ -n "$rpm" ]]; then
  run "rpm-fedora-41" fedora:41 "
dnf install -y -q /bundle/rpm/$(basename "$rpm") xorg-x11-server-Xvfb xauth dbus-daemon ImageMagick desktop-file-utils appstream which >/dev/null
$check_metadata
export APP=universal-calendar"
fi

if [[ -n "$appimage" ]]; then
  # Only what the AppImage cannot bring: X, D-Bus, fonts, and tools for the test.
  run "appimage-debian-12" debian:12 "
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq >/dev/null
apt-get install -y -qq --no-install-recommends xvfb xauth dbus imagemagick fonts-dejavu-core >/dev/null
if ldconfig -p | grep -q libwebkit2gtk; then echo 'WebKit is installed: not a clean test'; exit 1; fi
cp /bundle/appimage/$(basename "$appimage") /tmp/app.AppImage
export APPIMAGE_EXTRACT_AND_RUN=1 APP=/tmp/app.AppImage"
fi

exit $status
