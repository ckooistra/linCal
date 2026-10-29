#!/usr/bin/env bash
# Throw-away local Radicale CalDAV server for development and tests.
#
#   scripts/dev-radicale.sh start [dir]   # starts on http://127.0.0.1:5232, user "test" / "test"
#   scripts/dev-radicale.sh stop [dir]
#
# Needs Radicale (GPL-3): python3 -m pip install radicale, or set RADICALE
# to its executable. Data lives in [dir] (default: a temp directory).
set -euo pipefail
cmd="${1:-start}"
dir="${2:-${TMPDIR:-/tmp}/uc-radicale}"
radicale="${RADICALE:-$HOME/.local/share/uc-dev/radicale-venv/bin/radicale}"
port="${RADICALE_PORT:-5232}"
base="http://127.0.0.1:$port"

case "$cmd" in
  start)
    mkdir -p "$dir/collections"
    printf 'test:test\n' > "$dir/users"
    cat > "$dir/config" <<CONF
[server]
hosts = 127.0.0.1:$port
[auth]
type = htpasswd
htpasswd_filename = $dir/users
htpasswd_encryption = plain
[storage]
filesystem_folder = $dir/collections
[logging]
level = warning
CONF
    "$radicale" --config "$dir/config" > "$dir/radicale.log" 2>&1 &
    echo $! > "$dir/pid"
    for _ in $(seq 50); do
      curl -s -o /dev/null "$base/" && break
      sleep 0.1
    done
    # Create the test calendar (ignored if it already exists).
    curl -s -o /dev/null -u test:test -X MKCALENDAR "$base/test/uc-test/" \
      -H 'Content-Type: application/xml' --data-binary \
      '<?xml version="1.0"?><c:mkcalendar xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:a="http://apple.com/ns/ical/"><d:set><d:prop><d:displayname>UC Test</d:displayname><a:calendar-color>#2e7d32ff</a:calendar-color><c:supported-calendar-component-set><c:comp name="VEVENT"/></c:supported-calendar-component-set></d:prop></d:set></c:mkcalendar>'
    echo "Radicale running at $base (user test/test), data in $dir"
    ;;
  stop)
    [[ -f "$dir/pid" ]] && kill "$(cat "$dir/pid")" 2>/dev/null || true
    rm -f "$dir/pid"
    ;;
  *) echo "usage: $0 start|stop [dir]" >&2; exit 2 ;;
esac
