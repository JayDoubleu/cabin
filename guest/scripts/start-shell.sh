#!/usr/bin/bash
# Runs GNOME Shell on the headless mutter backend.
#
# By default no virtual monitor is requested here. gnome-remote-desktop creates
# one for the RDP session, and that monitor is the one the client sees. If the
# shell also creates its own, that one becomes the primary, the top bar and new
# windows go to it, and the client is shown an empty second screen. See
# DESIGN.md section 14.
#
# Set CABIN_SHELL_MONITOR to a size such as 1920x1080 to restore the old
# behaviour for debugging.
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

if [ -n "${CABIN_SHELL_MONITOR:-}" ]; then
    exec /usr/bin/gnome-shell --headless --virtual-monitor "${CABIN_SHELL_MONITOR}"
fi
exec /usr/bin/gnome-shell --headless
