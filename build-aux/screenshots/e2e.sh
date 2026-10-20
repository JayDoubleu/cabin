#!/bin/sh
# Tests the installed Cabin Flatpak from end to end, through real input.
#
# Run this on the host, not in a toolbox. From a toolbox, run it through
# "flatpak-spawn --host". It needs gnome-shell 50, the Cabin Flatpak in the
# user installation, and the guest image. The test makes desktops called
# e2e-one and e2e-two on the real Podman of this user and deletes them again.
#
#   OUT=/some/folder build-aux/screenshots/e2e.sh
#
# The screenshots, Cabin's output and results.json go to $OUT, by default
# ~/.cache/cabin-e2e. The exit code is 0 only when every check passed.
set -e
HERE=$(dirname "$(readlink -f "$0")")
OUT=${OUT:-$HOME/.cache/cabin-e2e}
export CABIN_SESSION="${CABIN_SESSION:-$HOME/.cache/cabin-e2e-session}"

rm -rf "$OUT"
mkdir -p "$OUT"
"$HERE/session.sh" up > "$OUT/session.log" 2>&1 &
for _ in $(seq 1 90); do
    [ -e "$CABIN_SESSION/env" ] && break
    sleep 0.5
done
if [ ! -e "$CABIN_SESSION/env" ]; then
    echo "The private session did not start. See $OUT/session.log." >&2
    exit 1
fi

status=0
"$HERE/session.sh" run python3 "$HERE/e2e.py" "$OUT" || status=$?
"$HERE/session.sh" down
wait
echo "The screenshots and results.json are in $OUT."
exit $status
