#!/usr/bin/bash
# Runs GNOME Shell on the headless mutter backend. The virtual monitor size is
# only the starting size: an RDP client that supports the Display Control
# channel resizes it after it connects.
set -euo pipefail

export XDG_SESSION_TYPE=wayland
export XDG_CURRENT_DESKTOP=GNOME
export XDG_SESSION_DESKTOP=gnome
export GNOME_SHELL_SESSION_MODE=user

# Software rendering is the fallback when no render node is passed in.
if [ ! -e /dev/dri/renderD128 ]; then
    export LIBGL_ALWAYS_SOFTWARE=1
    export GALLIUM_DRIVER=llvmpipe
fi

exec /usr/bin/gnome-shell \
    --headless \
    --virtual-monitor "${CABIN_WIDTH}x${CABIN_HEIGHT}"
