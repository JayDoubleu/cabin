#!/bin/sh
# Takes the screenshots in docs/screenshots from the installed Cabin Flatpak.
#
# Run this on the host, not in a toolbox. From a toolbox, run it through
# "flatpak-spawn --host". It needs gnome-shell 50, the Cabin Flatpak in the
# user installation, and the guest image. The script makes the demo desktops
# fedora-dev, web-project and sandbox on the real Podman of this user and
# deletes them again. It stops if one of them exists already.
#
#   build-aux/screenshots/take.sh [OUT]
#
# The images go to OUT, by default docs/screenshots in this repository.
# Cabin's output and the work shots go to $WORK, by default
# ~/.cache/cabin-shots. Look at every image before you commit it.
set -e
HERE=$(dirname "$(readlink -f "$0")")
OUT=$(readlink -f "${1:-$HERE/../../docs/screenshots}")
WORK=${WORK:-$HOME/.cache/cabin-shots}
export CABIN_SESSION="${CABIN_SESSION:-$HOME/.cache/cabin-shot-session}"
# Larger than the window, so that the window is not maximised and keeps its
# shadow and its rounded corners.
export CABIN_SESSION_SIZE="${CABIN_SESSION_SIZE:-1600x1000}"

rm -rf "$WORK"
mkdir -p "$WORK" "$OUT"
"$HERE/session.sh" up > "$WORK/session.log" 2>&1 &
for _ in $(seq 1 90); do
    [ -e "$CABIN_SESSION/env" ] && break
    sleep 0.5
done
if [ ! -e "$CABIN_SESSION/env" ]; then
    echo "The private session did not start. See $WORK/session.log." >&2
    exit 1
fi

status=0
"$HERE/session.sh" run python3 "$HERE/shots.py" "$OUT" "$WORK" || status=$?
"$HERE/session.sh" down
wait
echo "The screenshots are in $OUT. Cabin's output is in $WORK."
exit $status
