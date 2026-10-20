#!/bin/sh
# Runs a private headless GNOME Shell for the test and screenshot scripts.
#
# Run this on the host, not in a toolbox. From a toolbox, prefix each command
# with "flatpak-spawn --host". It needs gnome-shell 50 and the Cabin Flatpak
# installed in the user installation.
#
#   session.sh up       Starts the session and waits until "down" stops it.
#   session.sh run CMD  Runs CMD in the session that "up" started.
#   session.sh down     Stops the session and removes its files.
#
# The session has its own home, XDG folders, D-Bus session bus and runtime
# folder, so nothing touches the real desktop or its settings. A private
# XDG_RUNTIME_DIR is necessary: else the document portal of the private session
# replaces the mount of the real session.
#
# Podman is the exception. Cabin reaches the real rootless Podman socket, so
# desktops made in the session are real desktops of this user. Give every test
# desktop a name that starts with "e2e-" or "shot-", and remove it at the end.
# drive.py starts Cabin behind podman-proxy.py, which hides every other
# desktop, so the session cannot open or change the user's own.
set -e
HERE=$(dirname "$(readlink -f "$0")")
STATE=${CABIN_SESSION:-$HOME/.cache/cabin-session}
SIZE=${CABIN_SESSION_SIZE:-1600x1000}
APP_ID=io.github.jaydoubleu.Cabin

up() {
    if [ -e "$STATE/env" ]; then
        echo "A session is already up. Run \"$0 down\" first." >&2
        exit 1
    fi
    rm -rf "$STATE"
    home=$STATE/home
    mkdir -p "$home/.local/share/gnome-shell/extensions" "$home/.config" "$home/.cache"
    cp -r "$HERE/cabin-shot@local" "$home/.local/share/gnome-shell/extensions/"
    # The private runtime folder sits inside the real one, on the same file
    # system, so that a hard link can carry the real Podman socket into it.
    # Flatpak binds xdg-run/podman of this folder into the sandbox at its host
    # path, where the proxy finds it. A symbolic link would point back into
    # the sandbox.
    real_run=${XDG_RUNTIME_DIR:-/run/user/$(id -u)}
    run=$(mktemp -d "$real_run/cabin-session.XXXXXX")
    echo "$run" > "$STATE/run"
    mkdir "$run/podman"
    ln "$real_run/podman/podman.sock" "$run/podman/podman.sock"
    # The inner script expands its variables in the private session.
    # shellcheck disable=SC2016
    env -u WAYLAND_DISPLAY -u DISPLAY HOME="$home" XDG_RUNTIME_DIR="$run" \
        XDG_DATA_HOME="$home/.local/share" XDG_CONFIG_HOME="$home/.config" \
        XDG_CACHE_HOME="$home/.cache" FLATPAK_USER_DIR="$HOME/.local/share/flatpak" \
        REAL_HOME="$HOME" REAL_RUNTIME_DIR="$real_run" \
        STATE="$STATE" SIZE="$SIZE" dbus-run-session -- sh -c '
        # D-Bus activation of the accessibility bus fails with "Permission
        # denied" in this session, and the registry on that bus asks systemd,
        # which this session does not have. Start both by hand. The driver
        # finds widgets through them, and GTK warns at start-up without them.
        /usr/libexec/at-spi-bus-launcher --launch-immediately > /dev/null 2>&1 &
        for i in $(seq 1 20); do
            gdbus call --session --dest org.a11y.Bus --object-path /org/a11y/bus \
                --method org.a11y.Bus.GetAddress > /dev/null 2>&1 && break
            sleep 0.25
        done
        /usr/libexec/at-spi2-registryd > /dev/null 2>&1 &
        gsettings set org.gnome.shell disable-user-extensions false
        gsettings set org.gnome.shell enabled-extensions "[\"cabin-shot@local\"]"
        gsettings set org.gnome.shell welcome-dialog-last-shown-version "999"
        gsettings set org.gnome.desktop.interface enable-animations false
        # The virtual pointer starts in the top left corner. A hot corner there
        # opens the overview and covers the window.
        gsettings set org.gnome.desktop.interface enable-hot-corners false
        gsettings set org.gnome.desktop.interface toolkit-accessibility true
        gsettings set org.gnome.desktop.session idle-delay 0
        gsettings set org.gnome.desktop.screensaver lock-enabled false
        gnome-shell --headless --virtual-monitor "$SIZE" --wayland --no-x11 \
            --wayland-display=wayland-0 > "$STATE/shell.log" 2>&1 &
        shell=$!
        ready=no
        for i in $(seq 1 60); do
            if gdbus introspect --session --dest io.github.jaydoubleu.CabinShot \
                --object-path /io/github/jaydoubleu/CabinShot > /dev/null 2>&1; then
                ready=yes
                break
            fi
            sleep 0.5
        done
        if [ $ready = no ]; then
            echo "The shot helper did not start. See $STATE/shell.log." >&2
            kill $shell
            exit 1
        fi
        gdbus call --session --dest io.github.jaydoubleu.CabinShot \
            --object-path /io/github/jaydoubleu/CabinShot \
            --method io.github.jaydoubleu.CabinShot.HideOverview > /dev/null
        {
            echo "export HOME=\"$HOME\""
            echo "export XDG_RUNTIME_DIR=\"$XDG_RUNTIME_DIR\""
            echo "export XDG_DATA_HOME=\"$XDG_DATA_HOME\""
            echo "export XDG_CONFIG_HOME=\"$XDG_CONFIG_HOME\""
            echo "export XDG_CACHE_HOME=\"$XDG_CACHE_HOME\""
            echo "export FLATPAK_USER_DIR=\"$FLATPAK_USER_DIR\""
            echo "export DBUS_SESSION_BUS_ADDRESS=\"$DBUS_SESSION_BUS_ADDRESS\""
            echo "export WAYLAND_DISPLAY=wayland-0"
            echo "export REAL_HOME=\"$REAL_HOME\""
            echo "export REAL_RUNTIME_DIR=\"$REAL_RUNTIME_DIR\""
            echo "unset DISPLAY"
        } > "$STATE/env"
        echo "The session is up."
        while [ ! -e "$STATE/stop" ]; do
            sleep 0.5
        done
        flatpak kill io.github.jaydoubleu.Cabin 2> /dev/null || true
        kill $shell
        wait $shell 2> /dev/null || true
    ' || true
    # The document portal and gvfs of the private session leave FUSE mounts.
    for mount in "$run/doc" "$run/gvfs"; do
        fusermount -u "$mount" 2> /dev/null || true
    done
    rm -rf "$run" 2> /dev/null || true
    rm -f "$STATE/env" "$STATE/stop" "$STATE/run"
}

run() {
    if [ ! -e "$STATE/env" ]; then
        echo "No session is up. Run \"$0 up\" first." >&2
        exit 1
    fi
    # shellcheck disable=SC1091
    . "$STATE/env"
    export CABIN_SESSION="$STATE" APP_ID
    exec "$@"
}

down() {
    if [ ! -e "$STATE/env" ]; then
        return
    fi
    touch "$STATE/stop"
    for _ in $(seq 1 40); do
        [ -e "$STATE/env" ] || break
        sleep 0.5
    done
    # Services of the private session can still write here for a moment.
    sleep 1
    rm -rf "${STATE:?}/home" 2> /dev/null || true
}

# The braces make the shell read the whole command before it runs, so an edit
# to this file while a session is up cannot change what the session runs.
{
    case "$1" in
        up) up ;;
        run) shift; run "$@" ;;
        down) down ;;
        *) echo "Usage: $0 up | run COMMAND... | down" >&2; exit 2 ;;
    esac
    exit
}
