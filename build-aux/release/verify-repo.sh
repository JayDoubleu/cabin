#!/usr/bin/env bash
# Pulls the app and the AppStream data from a repository the way a client does: with
# static deltas and GPG verification. A pull of single objects can succeed while a
# delta pull fails, so this script must not disable the deltas.
#
# Usage: verify-repo.sh REPO_DIR_OR_URL
set -euo pipefail
# shellcheck source=build-aux/release/common.sh
source "$(dirname "${BASH_SOURCE[0]}")/common.sh"

SOURCE="${1:?usage: verify-repo.sh REPO_DIR_OR_URL}"
if [[ "$SOURCE" != *://* ]]; then
    SOURCE="file://$(cd "$SOURCE" && pwd -P)"
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
ostree init --mode=bare-user-only --repo="$WORK/repo"
ostree remote add --repo="$WORK/repo" --gpg-import="$PUBLIC_KEY" check "$SOURCE"
for ref in "app/$APP_ID/x86_64/$BRANCH" appstream2/x86_64; do
    echo "Pull $ref from $SOURCE"
    ostree pull --repo="$WORK/repo" --depth=0 check "$ref"
done
echo "The signed delta pull works."
