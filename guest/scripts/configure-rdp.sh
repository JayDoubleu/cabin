#!/usr/bin/bash
# Applies the RDP configuration before the daemon starts.
#
# gnome-remote-desktop requires NLA (CredSSP). It refuses a plain TLS handshake
# with HYBRID_REQUIRED_BY_SERVER. See DESIGN.md section 12.4.
#
# The two tools split the work, and the split is not obvious:
#
#  - "grdctl --headless" silently ignores the plain toggles. "grdctl --headless
#    rdp disable-view-only" exits 0 and leaves view-only set to true. So the
#    settings are written with gsettings instead.
#  - "grdctl --headless rdp enable" is NOT a settings write. It is the call
#    that makes the daemon serve. The daemon runs with the "enable" key still
#    reading false, and writing that key with gsettings does not make it bind.
#  - Credentials go through grdctl, because they live in a secret store rather
#    than in the settings schema.
#
# See DESIGN.md section 14.
set -euo pipefail

SCHEMA=org.gnome.desktop.remote-desktop.rdp

set_checked() {
    local key="$1" value="$2" actual
    gsettings set "${SCHEMA}" "${key}" "${value}"
    actual="$(gsettings get "${SCHEMA}" "${key}")"
    # gsettings returns strings quoted and uint16 with a type prefix, so
    # compare loosely rather than for exact equality.
    case "${actual}" in
        *"${value#\'}"*) : ;;
        *)
            echo "setting ${key} did not take: wanted ${value}, got ${actual}" >&2
            return 1
            ;;
    esac
}

set_checked tls-cert "'${CABIN_TLS_CERT}'"
set_checked tls-key  "'${CABIN_TLS_KEY}'"
set_checked port     "uint16 ${CABIN_RDP_PORT}"
set_checked negotiate-port false
# Without this the session ignores every key press and every pointer move.
set_checked view-only false

# The daemon reads credentials from a secret store. In a container there is no
# TPM, so it falls back to a keyfile, which works. Filter that one warning.
grdctl --headless rdp set-credentials "${CABIN_USER}" "${CABIN_PASSWORD}" 2>&1 \
    | grep -v 'Init TPM credentials failed' || true
if [ "${PIPESTATUS[0]}" -ne 0 ]; then
    echo "grdctl set-credentials failed" >&2
    exit 1
fi

# This must come last, and it must be grdctl rather than a gsettings write.
# "grdctl --headless rdp enable" is the operative call: it tells the daemon to
# start serving. Setting the "enable" key alone does nothing, and the daemon
# serves happily while that key still reads false. Replacing this call with a
# gsettings write stopped the server binding at all.
grdctl --headless rdp enable 2>&1 | grep -v 'Init TPM credentials failed' || true
if [ "${PIPESTATUS[0]}" -ne 0 ]; then
    echo "grdctl rdp enable failed" >&2
    exit 1
fi

echo "RDP configured: port ${CABIN_RDP_PORT}, view-only $(gsettings get ${SCHEMA} view-only)"
