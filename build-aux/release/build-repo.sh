#!/usr/bin/env bash
# Builds Cabin into a signed Flatpak (OSTree) repository.
#
# Usage: build-repo.sh REPO_DIR [--unsigned] [--no-mirror]
#
# 1. Mirrors the live repository from GitHub Pages, so the new commit has a parent and
#    clients can update with a static delta. --no-mirror starts an empty repository.
# 2. Exports a new commit with flatpak-builder. The builder signs it.
# 3. Updates the summary and the AppStream data, makes the static deltas and prunes old
#    commits. Signatures must exist before the deltas: a delta pull does not read the
#    detached signature of a commit, so deltas made first fail with "no signatures found".
# 4. Makes sure that each ref is signed and that a delta pull verifies with the public key.
#
# Run it in the CI image (see in-container.sh). It needs flatpak-builder, ostree and
# the GNOME SDK. The Rust SDK extension comes from Flathub, because the image does not
# have it. The Cabin build itself is offline: Cargo reads cargo-sources.json.
set -euo pipefail
# shellcheck source=build-aux/release/common.sh
source "$(dirname "${BASH_SOURCE[0]}")/common.sh"

REPO="${1:?usage: build-repo.sh REPO_DIR [--unsigned] [--no-mirror]}"
shift
SIGNED=1
MIRROR=1
for arg in "$@"; do
    case "$arg" in
        --unsigned) SIGNED=0 ;;
        --no-mirror) MIRROR=0 ;;
        *) die "unknown option $arg" ;;
    esac
done

GPG_ARGS=()
if [[ "$SIGNED" == 1 ]]; then
    import_signing_key
fi

mkdir -p "$REPO"
if [[ ! -f "$REPO/config" ]]; then
    ostree init --mode=archive-z2 --repo="$REPO"
fi
live_status=404
if [[ "$MIRROR" == 1 ]]; then
    live_status="$(curl -sS -o /dev/null -w '%{http_code}' --retry 3 --retry-all-errors \
        "$PAGES_URL/repo/config")" || die "cannot reach $PAGES_URL"
fi
# Only a 404 means that no live repository exists. Any other error must stop the build,
# because a new history replaces the live one at the deploy.
if [[ "$live_status" == 200 ]]; then
    echo "Mirroring the last $HISTORY_DEPTH commits from $PAGES_URL/repo/"
    ostree remote add --repo="$REPO" --if-not-exists --gpg-import="$PUBLIC_KEY" live "$PAGES_URL/repo/"
    ostree pull --repo="$REPO" --mirror --depth="$HISTORY_DEPTH" live
    ostree remote delete --repo="$REPO" live
elif [[ "$live_status" == 404 ]]; then
    echo "No live repository: the new commit has no parent."
else
    die "HTTP $live_status for $PAGES_URL/repo/config"
fi

# The image has the GNOME SDK but not the SDK extensions in the manifest (Rust). Do not
# use --install-deps-from: it also updates the SDK in the image, and that fails with
# "Invalid cross-device link" on the image's overlay file system. An extension's branch
# is the Freedesktop SDK version that the GNOME SDK names, not the GNOME version.
sdk="$(awk '$1 == "sdk:" {print $2}' "$MANIFEST")"
sdk_branch="$(awk '$1 == "runtime-version:" {gsub(/["\047]/, "", $2); print $2}' "$MANIFEST")"
extension_branch="$(flatpak info --show-metadata "$sdk//$sdk_branch" | awk -F' = ' '
    /^\[Extension org\.freedesktop\.Sdk\.Extension\]$/ {found = 1; next}
    /^\[/ {found = 0}
    found && $1 == "version" {print $2}')"
[[ -n "$extension_branch" ]] || die "$sdk//$sdk_branch names no SDK extension version"
mapfile -t extensions < <(sed -n '/^sdk-extensions:/,/^[^ ]/ s/^  - //p' "$MANIFEST")
if [[ "${#extensions[@]}" -gt 0 ]]; then
    flatpak remote-add --user --if-not-exists flathub "$RUNTIME_REPO"
    flatpak install --user --noninteractive -y flathub "${extensions[@]/%///$extension_branch}"
fi

# The manifest copies the source folder into the build, and skips _release. The build
# folder is not _flatpak, because "just flatpak" builds there.
BUILD_DIR="${BUILD_DIR:-$SOURCE_DIR/_release/flatpak}"
mkdir -p "$BUILD_DIR"
flatpak-builder \
    --force-clean \
    --disable-rofiles-fuse \
    --state-dir="$BUILD_DIR/state" \
    --repo="$REPO" \
    --default-branch="$BRANCH" \
    --subject="$APP_TITLE $(app_version)" \
    "${GPG_ARGS[@]}" \
    "$BUILD_DIR/build" "$MANIFEST"

# The debug extension is 10 MB per commit, and GitHub Pages allows 1 GB per site.
ostree refs --repo="$REPO" --delete "runtime/$APP_ID.Debug/x86_64/$BRANCH" 2> /dev/null || true

update_args=(
    --title="$APP_TITLE"
    --homepage="$HOMEPAGE"
    --default-branch="$BRANCH"
    --prune
    --prune-depth="$HISTORY_DEPTH"
)
if [[ "$SIGNED" == 1 ]]; then
    update_args+=(--gpg-import="$PUBLIC_KEY" "${GPG_ARGS[@]}")
fi
rm -rf "$REPO/deltas" "$REPO/delta-indexes"
flatpak build-update-repo "${update_args[@]}" --generate-static-deltas "$REPO"

echo "Refs:"
ostree refs --repo="$REPO" | sort
if [[ "$SIGNED" == 1 ]]; then
    for ref in $(ostree refs --repo="$REPO"); do
        commit="$(ostree rev-parse --repo="$REPO" "$ref")"
        ostree show --repo="$REPO" --print-detached-metadata-key=ostree.gpgsigs "$commit" > /dev/null \
            || die "$ref ($commit) is not signed"
    done
    [[ -s "$REPO/summary.sig" ]] || die "the summary is not signed"
    "$RELEASE_DIR/verify-repo.sh" "$REPO"
fi
