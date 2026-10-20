#!/usr/bin/env bash
# Installs Cabin from the .flatpakref of a site, as a user does, and checks the result.
# With --update, it updates an earlier install instead.
#
# Usage: verify-install.sh EXPECTED_VERSION [--update]
#
# PAGES_URL selects the site. FLATPAK_USER_DIR selects the installation, so the check
# does not touch your own apps. The GNOME runtime must be installed or reachable.
set -euo pipefail
# shellcheck source=build-aux/release/common.sh
source "$(dirname "${BASH_SOURCE[0]}")/common.sh"

EXPECTED="${1:?usage: verify-install.sh EXPECTED_VERSION [--update]}"
if [[ "${2:-}" == "--update" ]]; then
    flatpak update --user --noninteractive -y "$APP_ID"
else
    flatpak install --user --noninteractive -y --from "$PAGES_URL/$APP_ID.flatpakref"
fi

info="$(flatpak info --user "$APP_ID")"
echo "$info"
origin="$(flatpak info --user --show-origin "$APP_ID")"
version="$(awk '$1 == "Version:" {print $2}' <<< "$info")"
url="$(flatpak remotes --user --columns=name,url | awk -v r="$origin" '$1 == r {print $2}')"

[[ "$origin" == "$REMOTE_NAME" ]] || die "the origin is $origin, not $REMOTE_NAME"
[[ "$version" == "$EXPECTED" ]] || die "the version is $version, not $EXPECTED"
[[ "$url" == "$PAGES_URL/repo/" ]] || die "the remote URL is $url, not $PAGES_URL/repo/"
gpg_verify="$(flatpak remotes --user --columns=name,options | awk -v r="$origin" '$1 == r {print $2}')"
[[ "$gpg_verify" != *no-gpg-verify* ]] || die "the remote does not verify signatures"
echo "Installed $APP_ID $version from $url with signature checks."
