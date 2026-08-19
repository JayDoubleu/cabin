#!/usr/bin/bash
# Prepares per-container state before the user session starts.
# Reads configuration from the container environment (PID 1) so that values
# passed with "podman run --env" reach the systemd user units.
set -euo pipefail

env_of_pid1() {
    tr '\0' '\n' < /proc/1/environ | grep -E "^$1=" | head -n1 | cut -d= -f2- || true
}

CABIN_USER="$(env_of_pid1 CABIN_USER)";       CABIN_USER="${CABIN_USER:-cabin}"
CABIN_WIDTH="$(env_of_pid1 CABIN_WIDTH)";     CABIN_WIDTH="${CABIN_WIDTH:-1920}"
CABIN_HEIGHT="$(env_of_pid1 CABIN_HEIGHT)";   CABIN_HEIGHT="${CABIN_HEIGHT:-1080}"
CABIN_RDP_PORT="$(env_of_pid1 CABIN_RDP_PORT)"; CABIN_RDP_PORT="${CABIN_RDP_PORT:-3389}"
CABIN_PASSWORD="$(env_of_pid1 CABIN_PASSWORD)"; CABIN_PASSWORD="${CABIN_PASSWORD:-cabin}"

install -d -m 0755 /run/cabin
install -d -m 0700 -o "${CABIN_USER}" -g "${CABIN_USER}" /var/lib/cabin
install -d -m 0700 -o "${CABIN_USER}" -g "${CABIN_USER}" /var/lib/cabin/tls

# Set the desktop account password. RDP authenticates against these
# credentials, and the GNOME lock screen uses the same password.
echo "${CABIN_USER}:${CABIN_PASSWORD}" | chpasswd

# A self-signed certificate is enough: the client pins it on first connect.
if [ ! -f /var/lib/cabin/tls/rdp.crt ]; then
    openssl req -new -newkey rsa:4096 -days 3650 -nodes -x509 \
        -subj "/C=GB/O=Cabin/CN=cabin" \
        -out /var/lib/cabin/tls/rdp.crt \
        -keyout /var/lib/cabin/tls/rdp.key >/dev/null 2>&1
    chown "${CABIN_USER}:${CABIN_USER}" /var/lib/cabin/tls/rdp.crt /var/lib/cabin/tls/rdp.key
    chmod 0600 /var/lib/cabin/tls/rdp.key
fi

cat > /run/cabin/env <<ENVEOF
CABIN_USER=${CABIN_USER}
CABIN_WIDTH=${CABIN_WIDTH}
CABIN_HEIGHT=${CABIN_HEIGHT}
CABIN_RDP_PORT=${CABIN_RDP_PORT}
CABIN_PASSWORD=${CABIN_PASSWORD}
CABIN_TLS_CERT=/var/lib/cabin/tls/rdp.crt
CABIN_TLS_KEY=/var/lib/cabin/tls/rdp.key
ENVEOF
chmod 0640 /run/cabin/env
chgrp "${CABIN_USER}" /run/cabin/env

touch /var/lib/cabin/.provisioned
