#!/usr/bin/bash
# Prepares per-boot state before the user session starts.
#
# This runs on EVERY boot, not only the first. /run/cabin/env lives on a
# tmpfs, so a container restart wipes it. An earlier version guarded the whole
# script with ConditionPathExists on a marker in /var/lib, which survives a
# restart. The result was that a restarted desktop lost its environment file
# and the whole session failed to start. Only the TLS certificate is generated
# once.
#
# Configuration is read from the container environment through /proc/1/environ,
# because systemd does not pass the container environment down to services.
set -euo pipefail

env_of_pid1() {
    tr '\0' '\n' < /proc/1/environ | grep -E "^$1=" | head -n1 | cut -d= -f2- || true
}

CABIN_USER="$(env_of_pid1 CABIN_USER)";           CABIN_USER="${CABIN_USER:-cabin}"
CABIN_WIDTH="$(env_of_pid1 CABIN_WIDTH)";         CABIN_WIDTH="${CABIN_WIDTH:-1920}"
CABIN_HEIGHT="$(env_of_pid1 CABIN_HEIGHT)";       CABIN_HEIGHT="${CABIN_HEIGHT:-1080}"
CABIN_RDP_PORT="$(env_of_pid1 CABIN_RDP_PORT)";   CABIN_RDP_PORT="${CABIN_RDP_PORT:-3389}"
CABIN_PASSWORD="$(env_of_pid1 CABIN_PASSWORD)";   CABIN_PASSWORD="${CABIN_PASSWORD:-cabin}"
# When set, GNOME Shell creates its own virtual monitor. Leave it unset so
# that gnome-remote-desktop provides the only monitor. See section 14 of
# DESIGN.md: a shell-created monitor becomes the primary, and the client is
# then shown the other one, which is empty.
CABIN_SHELL_MONITOR="$(env_of_pid1 CABIN_SHELL_MONITOR)"
CABIN_NESTED_PODMAN="$(env_of_pid1 CABIN_NESTED_PODMAN)"

install -d -m 0755 /run/cabin
install -d -m 0700 -o "${CABIN_USER}" -g "${CABIN_USER}" /var/lib/cabin
install -d -m 0700 -o "${CABIN_USER}" -g "${CABIN_USER}" /var/lib/cabin/tls

# Applied every boot, so that changing CABIN_PASSWORD actually takes effect.
echo "${CABIN_USER}:${CABIN_PASSWORD}" | chpasswd

# Generated once. The client pins the fingerprint, so it must stay stable.
if [ ! -f /var/lib/cabin/tls/rdp.crt ]; then
    openssl req -new -newkey rsa:4096 -days 3650 -nodes -x509 \
        -subj "/C=GB/O=Cabin/CN=cabin" \
        -out /var/lib/cabin/tls/rdp.crt \
        -keyout /var/lib/cabin/tls/rdp.key >/dev/null 2>&1
    chown "${CABIN_USER}:${CABIN_USER}" /var/lib/cabin/tls/rdp.crt /var/lib/cabin/tls/rdp.key
    chmod 0600 /var/lib/cabin/tls/rdp.key
fi

# Rootless Podman inside a rootless container cannot mount fuse-overlayfs,
# because that needs CAP_SYS_ADMIN which the outer container does not have.
# The vfs driver needs no mount at all. It is slower and uses more disk, and it
# is the only driver that works here.
if [ -n "${CABIN_NESTED_PODMAN}" ]; then
    # Podman inside a rootless container cannot call newuidmap: writing a
    # uid_map for a child namespace needs privileges the outer container does
    # not have, and it fails with "Operation not permitted". Removing the
    # subordinate ID ranges puts Podman into single UID mode, where it never
    # calls newuidmap at all. Everything inside then runs as one user, which is
    # the only arrangement that works here.
    : > /etc/subuid
    : > /etc/subgid

    # fuse-overlayfs needs a mount, which needs CAP_SYS_ADMIN, which a rootless
    # container does not have. vfs needs no mount. It is slower and uses more
    # disk, and it is the only driver available. ignore_chown_errors goes with
    # single UID mode, where the usual ownership changes cannot be made.
    conf_dir="/home/${CABIN_USER}/.config/containers"
    install -d -m 0755 -o "${CABIN_USER}" -g "${CABIN_USER}" \
        "/home/${CABIN_USER}/.config" "${conf_dir}"
    cat > "${conf_dir}/storage.conf" <<'STORAGEEOF'
[storage]
driver = "vfs"

[storage.options]
ignore_chown_errors = "true"
STORAGEEOF
    chown "${CABIN_USER}:${CABIN_USER}" "${conf_dir}/storage.conf"
fi

cat > /run/cabin/env <<ENVEOF
CABIN_USER=${CABIN_USER}
CABIN_WIDTH=${CABIN_WIDTH}
CABIN_HEIGHT=${CABIN_HEIGHT}
CABIN_RDP_PORT=${CABIN_RDP_PORT}
CABIN_PASSWORD=${CABIN_PASSWORD}
CABIN_SHELL_MONITOR=${CABIN_SHELL_MONITOR}
CABIN_TLS_CERT=/var/lib/cabin/tls/rdp.crt
CABIN_TLS_KEY=/var/lib/cabin/tls/rdp.key
ENVEOF
chmod 0640 /run/cabin/env
chgrp "${CABIN_USER}" /run/cabin/env
