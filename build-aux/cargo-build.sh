#!/usr/bin/env bash
# Wrapper that meson calls to build the Rust binary and place it where meson
# expects. meson cannot express environment variables and shell operators
# inside custom_target, so the logic lives here.
set -euo pipefail

SOURCE_ROOT="$1"
BUILD_ROOT="$2"
PROFILE="$3"     # debug or release
APP_ID="$4"
OUTPUT="$5"

export CARGO_TARGET_DIR="${BUILD_ROOT}/target"
export CABIN_APP_ID="${APP_ID}"

# The Flatpak build runs offline with vendored sources, so honour an existing
# CARGO_HOME instead of overwriting it.
export CARGO_HOME="${CARGO_HOME:-${BUILD_ROOT}/cargo-home}"

args=(build --manifest-path "${SOURCE_ROOT}/Cargo.toml")
if [ "${PROFILE}" = "release" ]; then
    args+=(--release)
fi
if [ -n "${CABIN_CARGO_OFFLINE:-}" ]; then
    args+=(--offline)
fi

cargo "${args[@]}"
cp "${CARGO_TARGET_DIR}/${PROFILE}/cabin" "${OUTPUT}"
