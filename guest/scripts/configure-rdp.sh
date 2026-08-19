#!/usr/bin/bash
# Applies the RDP configuration, then the unit that starts the daemon runs.
#
# Note: gnome-remote-desktop requires NLA (CredSSP). It rejects a plain TLS
# handshake with HYBRID_REQUIRED_BY_SERVER, so any client must speak CredSSP.
set -euo pipefail

# "Init TPM credentials failed ... using GKeyFile as fallback" is expected in a
# container and is not an error. Drop it so real failures are visible.
grd() { grdctl --headless "$@" 2>&1 | grep -v 'Init TPM credentials failed' || true; }

grd rdp set-tls-cert "${CABIN_TLS_CERT}"
grd rdp set-tls-key  "${CABIN_TLS_KEY}"
grd rdp set-port     "${CABIN_RDP_PORT}"
grd rdp disable-port-negotiation
grd rdp set-credentials "${CABIN_USER}" "${CABIN_PASSWORD}"
grd rdp disable-view-only
grd rdp enable
