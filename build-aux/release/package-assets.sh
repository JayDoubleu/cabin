#!/usr/bin/env bash
# Makes the files for a GitHub release.
#
# Usage: package-assets.sh REPO_DIR OUT_DIR
#
# Cabin-VERSION.flatpak   a single-file bundle. It knows the repository URL, so an
#                         install from the bundle also gets updates.
# SHA256SUMS
set -euo pipefail
# shellcheck source=build-aux/release/common.sh
source "$(dirname "${BASH_SOURCE[0]}")/common.sh"

REPO="${1:?usage: package-assets.sh REPO_DIR OUT_DIR}"
OUT="${2:?usage: package-assets.sh REPO_DIR OUT_DIR}"
VERSION="$(app_version)"

rm -rf "$OUT"
mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"

bundle_args=(--repo-url="$PAGES_URL/repo/" --runtime-repo="$RUNTIME_REPO")
# A bundle from an unsigned test build must not ask for signatures at the next update.
if [[ -s "$REPO/summary.sig" ]]; then
    bundle_args+=(--gpg-keys="$PUBLIC_KEY")
fi
flatpak build-bundle "${bundle_args[@]}" "$REPO" "$OUT/Cabin-$VERSION.flatpak" "$APP_ID" "$BRANCH"

(cd "$OUT" && sha256sum -- * > SHA256SUMS)
ls -la "$OUT"
