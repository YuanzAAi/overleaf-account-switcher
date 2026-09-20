#!/bin/sh
set -eu
parent_pid="$1"
target="$2"
stage="$3"
install_dir="$(dirname "$target")"
case "$target" in /*.app) ;; *) exit 1 ;; esac
case "$stage" in "$install_dir"/.overleaf-update-*) ;; *) exit 1 ;; esac
test "$(dirname "$stage")" = "$install_dir"
test ! -L "$stage"
previous="$stage/previous.app"
moved=0
installed=0
parent_exited=0

rollback() {
    trap - EXIT
    recovered=1
    if [ "$installed" -eq 1 ]; then
        mv "$target" "$stage/rejected.app" || recovered=0
    fi
    if [ "$moved" -eq 1 ]; then
        mv "$previous" "$target" || recovered=0
    fi
    if [ "$parent_exited" -eq 1 ] && [ "$recovered" -eq 1 ]; then
        /usr/bin/open "$target" || true
    fi
    /usr/bin/osascript -e 'display alert "Overleaf Account Switcher" message "The update could not be completed. Your account data is unchanged."' || true
    if [ "$recovered" -eq 1 ]; then rm -rf "$stage"; fi
}
trap rollback EXIT
attempt=0
while kill -0 "$parent_pid" 2>/dev/null; do
    attempt=$((attempt + 1))
    test "$attempt" -lt 90
    sleep 1
done
parent_exited=1
/usr/bin/ditto -x -k "$stage/payload.zip" "$stage/unpacked"
source="$stage/unpacked/Overleaf 账号控制台.app"
test -f "$source/Contents/MacOS/overleaf-desktop"
/usr/bin/codesign --verify --deep --strict "$source"
mv "$target" "$previous"
moved=1
mv "$source" "$target"
installed=1
/usr/bin/open "$target"
trap - EXIT
rm -rf "$stage"
