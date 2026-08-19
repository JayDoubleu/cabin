#!/usr/bin/env bash
# Checks the host for everything Cabin needs, and says how to fix what is
# missing. Every check here exists because something actually went wrong.
#
# Read-only: it changes nothing and never asks for a password.
set -uo pipefail

pass=0
warn=0
fail=0

ok()   { printf '  \033[32mok\033[0m    %s\n' "$1"; pass=$((pass + 1)); }
note() { printf '  \033[33mnote\033[0m  %s\n' "$1"; warn=$((warn + 1)); }
bad()  { printf '  \033[31mfix\033[0m   %s\n' "$1"; fail=$((fail + 1)); }
fixup(){ printf '        %s\n' "$1"; }

# Cabin manages containers on the host. Inside a toolbox the local podman has
# its own storage, so every check has to go to the host.
if [ -f /run/.toolboxenv ]; then
    host() { flatpak-spawn --host "$@"; }
    printf '\nRunning inside a toolbox, checking the host through flatpak-spawn.\n'
else
    host() { "$@"; }
fi

printf '\nContainer runtime\n'
if host podman --version >/dev/null 2>&1; then
    ok "podman $(host podman --version 2>/dev/null | awk '{print $3}')"
else
    bad "podman is not installed"
    fixup "sudo dnf install podman"
fi

if host systemctl --user is-active podman.socket >/dev/null 2>&1; then
    ok "podman.socket is running"
else
    bad "podman.socket is not running, so Cabin cannot manage desktops"
    fixup "systemctl --user enable --now podman.socket"
fi

printf '\nKernel limits\n'
# Every desktop runs systemd, which needs several inotify instances. The
# default of 128 is reached after a handful of desktops, and the failure is
# silent: the container exits 255 with an empty log.
instances=$(host cat /proc/sys/fs/inotify/max_user_instances 2>/dev/null || echo 0)
if [ "${instances}" -ge 512 ]; then
    ok "fs.inotify.max_user_instances is ${instances}"
else
    bad "fs.inotify.max_user_instances is ${instances}, too low for several desktops"
    fixup "sudo sysctl -w fs.inotify.max_user_instances=1024"
fi

if host grep -rqs "max_user_instances" /etc/sysctl.conf /etc/sysctl.d/ 2>/dev/null; then
    ok "the inotify limit is set permanently"
else
    note "the inotify limit will reset when you restart"
    fixup "echo fs.inotify.max_user_instances=1024 | sudo tee /etc/sysctl.d/99-cabin.conf"
fi

printf '\nGraphics\n'
nodes=$(host sh -c 'ls /dev/dri/renderD* 2>/dev/null' | wc -l)
if [ "${nodes}" -gt 0 ]; then
    for node in $(host sh -c 'ls /dev/dri/renderD* 2>/dev/null'); do
        leaf=$(basename "${node}")
        driver=$(host sh -c "basename \$(readlink -f /sys/class/drm/${leaf}/device/driver 2>/dev/null)" 2>/dev/null)
        ok "${node} uses the ${driver:-unknown} driver"
    done
else
    note "no render node found, so desktops will use software rendering"
fi

printf '\nFlatpak\n'
if host flatpak info org.gnome.Platform//50 >/dev/null 2>&1; then
    ok "the GNOME 50 runtime is installed"
else
    bad "the GNOME 50 runtime is missing"
    fixup "flatpak install flathub org.gnome.Platform//50 org.gnome.Sdk//50"
fi
if host flatpak info org.freedesktop.Sdk.Extension.rust-stable//25.08 >/dev/null 2>&1; then
    ok "the Rust SDK extension is installed"
else
    note "the Rust SDK extension is missing, needed only to build the Flatpak"
    fixup "flatpak install flathub org.freedesktop.Sdk.Extension.rust-stable//25.08"
fi

printf '\nBuilding natively\n'
if pkg-config --exists freerdp-client3 2>/dev/null; then
    ok "FreeRDP $(pkg-config --modversion freerdp-client3) is available to the build"
else
    note "FreeRDP is not on the pkg-config path, needed only for a native build"
    fixup "just freerdp-sysroot   (no root needed), then export what it prints"
fi

printf '\nGuest image\n'
if host podman image exists localhost/cabin-guest:f44 2>/dev/null; then
    ok "the guest image is built"
else
    bad "the guest image is missing"
    fixup "just guest"
fi

printf '\n%d ok, %d to note, %d to fix\n\n' "${pass}" "${warn}" "${fail}"
[ "${fail}" -eq 0 ]
