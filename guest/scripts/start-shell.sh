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

# Software rendering is the fallback when no render node is passed in. Cabin
# passes the chosen node at its host path, which can be any renderD number, so
# any render node counts. Never infer a driver from the node number.
if ! compgen -G '/dev/dri/renderD*' >/dev/null; then
    export LIBGL_ALWAYS_SOFTWARE=1
    export GALLIUM_DRIVER=llvmpipe
fi

# GNOME Shell tells a user once that the screen cannot lock without the GNOME
# display manager, and it records that in this file. No display manager runs
# here, so every new desktop showed the notice on its first start. Cabin shows
# the desktop in a window on a computer that has its own lock screen, so the
# notice tells the user nothing. The home is a volume, so the build cannot put
# the file there (CLAUDE.md rule 37).
mkdir -p "${XDG_DATA_HOME:-${HOME}/.local/share}/gnome-shell"
touch "${XDG_DATA_HOME:-${HOME}/.local/share}/gnome-shell/lock-warning-shown"

if [ -n "${CABIN_SHELL_MONITOR:-}" ]; then
    exec /usr/bin/gnome-shell --headless --virtual-monitor "${CABIN_SHELL_MONITOR}"
fi
exec /usr/bin/gnome-shell --headless
