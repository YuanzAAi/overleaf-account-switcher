#!/bin/sh
set -eu
umask 077
test -s /run/secrets/keyring_password || { echo "A nonempty keyring_password secret is required" >&2; exit 1; }
mkdir -p "$XDG_RUNTIME_DIR" "$OVERLEAF_SWITCHER_TMP_DIR"
chmod 700 "$XDG_RUNTIME_DIR"
mkdir -p /tmp/.X11-unix
chmod 1777 /tmp/.X11-unix
if [ -z "${DBUS_SESSION_BUS_ADDRESS:-}" ]; then
    exec dbus-run-session -- "$0"
fi
eval "$(tr -d '\r\n' < /run/secrets/keyring_password | gnome-keyring-daemon --unlock --components=secrets)"
Xvfb "$DISPLAY" -screen 0 1280x900x24 -nolisten tcp &
display_pid=$!
attempt=0
until xdpyinfo >/dev/null 2>&1; do
    attempt=$((attempt + 1))
    test "$attempt" -lt 30 || { echo "Display failed to start" >&2; exit 1; }
    sleep 0.2
done
fluxbox >/dev/null 2>&1 &
wm_pid=$!
x11vnc -display "$DISPLAY" -localhost -rfbport 5900 -forever -shared -nopw -quiet &
vnc_pid=$!
websockify --web=/usr/share/novnc 6080 localhost:5900 &
web_pid=$!
overleaf-service-api &
service_pid=$!
trap 'kill "$service_pid" "$web_pid" "$vnc_pid" "$wm_pid" "$display_pid" 2>/dev/null || true; wait || true' EXIT
trap 'exit 0' TERM INT
wait "$service_pid"
