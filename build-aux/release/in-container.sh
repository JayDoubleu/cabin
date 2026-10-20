#!/usr/bin/env bash
# Runs a command in the CI image with podman, so a local release build is the same as CI.
#
# Usage: in-container.sh COMMAND [ARGS...]
#   build-aux/release/in-container.sh build-aux/release/build-repo.sh _release/repo --unsigned
#
# The source folder is mounted at the same path. GPG_PRIVATE_KEY_FILE, PAGES_URL and
# HISTORY_DEPTH go into the container. The key file is mounted read-only. CONTAINER_ARGS
# adds podman options, for example "--network=host" to reach a local test server.
#
# Keep IMAGE in step with the runtime-version in the manifest and with the workflows.
set -euo pipefail
# shellcheck source=build-aux/release/common.sh
source "$(dirname "${BASH_SOURCE[0]}")/common.sh"

IMAGE="${IMAGE:-ghcr.io/flathub-infra/flatpak-github-actions:gnome-51}"
PODMAN=(podman)
# Inside a toolbox, podman is the toolbox's own, with separate storage (CLAUDE.md rule 17).
if [[ -f /run/.toolboxenv ]]; then
    PODMAN=(flatpak-spawn --host podman)
fi

args=(run --rm --privileged --volume "$SOURCE_DIR:$SOURCE_DIR:z" --workdir "$SOURCE_DIR")
args+=(--env PAGES_URL="$PAGES_URL" --env HISTORY_DEPTH="$HISTORY_DEPTH")
if [[ -n "${GPG_PRIVATE_KEY_FILE:-}" ]]; then
    args+=(--volume "$GPG_PRIVATE_KEY_FILE:/run/signing-key.asc:ro,z" --env GPG_PRIVATE_KEY_FILE=/run/signing-key.asc)
fi
read -r -a extra <<< "${CONTAINER_ARGS:-}"
exec "${PODMAN[@]}" "${args[@]}" "${extra[@]}" "$IMAGE" "$@"
