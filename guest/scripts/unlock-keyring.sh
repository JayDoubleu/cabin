#!/usr/bin/bash
# gnome-remote-desktop stores its headless RDP credentials with libsecret.
# The keyring must therefore be unlocked before grdctl can write them.
set -euo pipefail

CONTROL="${XDG_RUNTIME_DIR}/keyring/control"
if [ -S "${CONTROL}" ]; then
    exit 0
fi

# The output goes under XDG_RUNTIME_DIR because this runs as the desktop user.
# /run/cabin belongs to root, so the desktop user cannot create files in it.
# An earlier version wrote there: bash then could not open the file,
# never started the daemon, and the error was discarded, so no keyring ran.
#
# Errors are not hidden. If the daemon fails, the unit fails and the journal
# shows why.
ENV_FILE="${XDG_RUNTIME_DIR}/cabin-keyring.env"
printf '%s' "${CABIN_PASSWORD}" | \
    gnome-keyring-daemon --daemonize --unlock --components=secrets > "${ENV_FILE}"

# Export the daemon's addresses into the systemd user environment so that
# later units in the session can reach the secret service.
if [ -s "${ENV_FILE}" ]; then
    while IFS= read -r line; do
        case "${line}" in
            *=*) systemctl --user set-environment "${line}" ;;
        esac
    done < "${ENV_FILE}"
fi
