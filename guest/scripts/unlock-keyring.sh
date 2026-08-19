#!/usr/bin/bash
# gnome-remote-desktop stores its headless RDP credentials with libsecret.
# The keyring must therefore be unlocked before grdctl can write them.
set -euo pipefail

CONTROL="${XDG_RUNTIME_DIR}/keyring/control"
if [ -S "${CONTROL}" ]; then
    exit 0
fi

printf '%s' "${CABIN_PASSWORD}" | \
    gnome-keyring-daemon --daemonize --unlock --components=secrets > /run/cabin/keyring.env 2>/dev/null || true

# Export the daemon's addresses into the systemd user environment so that
# later units in the session can reach the secret service.
if [ -s /run/cabin/keyring.env ]; then
    while IFS= read -r line; do
        case "${line}" in
            *=*) systemctl --user set-environment "${line}" ;;
        esac
    done < /run/cabin/keyring.env
fi
