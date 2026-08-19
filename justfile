# Cabin task runner. Run "just" to list tasks.
#
# Desktops must run on the host, not nested inside a toolbox. A toolbox has its
# own podman with its own storage, so a guest built there would be invisible to
# the host and would run nested. Detect the toolbox and reach the host podman.
podman := if path_exists("/run/.toolboxenv") == "true" { "flatpak-spawn --host podman" } else { "podman" }

# cabin-freerdp-sys links FreeRDP 3. If freerdp-devel is not installed system
# wide, "just freerdp-sysroot" builds a local one and these point the build at
# it. When the sysroot is absent, pkg-config falls back to the system.
sysroot := env_var_or_default("CABIN_FREERDP_SYSROOT", env_var("HOME") / ".local/freerdp-sysroot")
have_sysroot := path_exists(sysroot / "usr/lib64/pkgconfig")

export PKG_CONFIG_PATH := if have_sysroot == "true" { sysroot / "usr/lib64/pkgconfig" } else { env_var_or_default("PKG_CONFIG_PATH", "") }
export PKG_CONFIG_SYSROOT_DIR := if have_sysroot == "true" { sysroot } else { env_var_or_default("PKG_CONFIG_SYSROOT_DIR", "") }
export LD_LIBRARY_PATH := if have_sysroot == "true" { sysroot / "usr/lib64" } else { env_var_or_default("LD_LIBRARY_PATH", "") }

app_id := "io.github.jaydoubleu.Cabin"
manifest := "build-aux/" + app_id + ".yaml"
guest_image := "localhost/cabin-guest:f44"

default:
    @just --list

# --- native development ---

# Build the Rust workspace.
build:
    cargo build

# Compile the settings schema for a native run.
#
# GLib aborts when a schema is missing, so without this the application exits
# immediately with no output. The Flatpak installs the schema properly; a
# native run has to be pointed at a compiled copy.
_schemas:
    @mkdir -p target/glib-schemas
    @cp data/cabin.gschema.xml target/glib-schemas/{{app_id}}.gschema.xml
    @glib-compile-schemas target/glib-schemas

# Run the app natively, without the Flatpak sandbox.
run: _schemas
    GSETTINGS_SCHEMA_DIR=target/glib-schemas RUST_LOG=cabin=debug cargo run -p cabin-gtk

# Run every test.
test:
    cargo test

# Check formatting and lints.
lint:
    cargo fmt --all --check
    cargo clippy --all-targets -- -D warnings

fmt:
    cargo fmt --all

# --- guest image ---

# Build the guest desktop image.
guest:
    {{podman}} build -t {{guest_image}} -f guest/Containerfile guest/

# Start a throwaway guest and report whether RDP came up.
guest-test password="testpass" port="13389":
    -{{podman}} rm -f cabin-test
    @# /dev/fuse is not optional. gnome-remote-desktop mounts a FUSE
    @# filesystem for clipboard redirection as soon as a client advertises
    @# the cliprdr channel. Without the device the mount fails and the daemon
    @# calls abort(), which drops the connection. See DESIGN.md section 14.
    {{podman}} run -d --name cabin-test --systemd=always \
        --publish 127.0.0.1:{{port}}:3389 \
        --env CABIN_PASSWORD={{password}} \
        --device /dev/fuse \
        --memory 4g --shm-size 512m \
        {{guest_image}}
    @echo "Waiting for the desktop session and the RDP port ..."
    @# systemd finishing is not enough. gnome-remote-desktop only listens once
    @# GNOME Shell is up, so wait for the port itself.
    @until {{podman}} exec cabin-test ss -tln 2>/dev/null | grep -q ':3389'; \
        do sleep 1; done
    @{{podman}} exec cabin-test sudo -u cabin XDG_RUNTIME_DIR=/run/user/1000 \
        systemctl --user list-units 'cabin*' 'gnome-remote*' \
        --no-legend --no-pager || true
    @just guest-probe {{port}}
    @echo ""
    @echo "Connect an RDP client to 127.0.0.1:{{port}}"
    @echo "  username: cabin"
    @echo "  password: {{password}}"
    @echo "The TLS certificate is self-signed, so the client warns on first connect."

# Check that the guest is really speaking RDP.
guest-probe port="13389":
    python3 build-aux/rdp-probe.py {{port}}

guest-shell:
    {{podman}} exec -it cabin-test bash

guest-clean:
    -{{podman}} rm -f cabin-test

# --- flatpak ---

# Regenerate the vendored cargo sources. Run after any Cargo.lock change.
flatpak-sources:
    uv run --with aiohttp --with tomlkit \
        python build-aux/flatpak-cargo-generator.py Cargo.lock -o cargo-sources.json

# Build and install the Flatpak.
# rofiles-fuse cannot mount under /mnt/data from inside a toolbox, so it is off.
flatpak:
    flatpak-builder --user --force-clean --disable-rofiles-fuse --install \
        _flatpak {{manifest}}

flatpak-run:
    flatpak run --env=RUST_LOG=cabin=debug {{app_id}}

clean:
    cargo clean
    rm -rf _build _flatpak .flatpak-builder

# Create a Cabin-managed demo desktop, so the app has something to list.
demo:
    cargo test -p cabin-podman --test make_demo -- --ignored --nocapture

# --- FreeRDP ---

# Build a local FreeRDP sysroot without root, for building cabin-freerdp-sys.
# Use this when freerdp-devel is not installed system wide.
freerdp-sysroot dest="~/.local/freerdp-sysroot":
    #!/usr/bin/env bash
    set -euo pipefail
    dest="$(eval echo {{dest}})"
    work="$(mktemp -d)"
    trap 'rm -rf "$work"' EXIT
    echo "Downloading FreeRDP 3 packages ..."
    ( cd "$work" && dnf download --quiet freerdp-devel freerdp-libs libwinpr libwinpr-devel \
        sso-mib-libs uriparser )
    mkdir -p "$dest"
    for r in "$work"/*x86_64.rpm; do
        ( cd "$dest" && rpm2cpio "$r" | cpio -idm --quiet )
    done
    echo
    echo "Sysroot ready at $dest"
    echo "Export these before building:"
    echo "  export PKG_CONFIG_PATH=$dest/usr/lib64/pkgconfig"
    echo "  export PKG_CONFIG_SYSROOT_DIR=$dest"
    echo "  export LD_LIBRARY_PATH=$dest/usr/lib64"

# Run one live test by name, for iterating.
rdp-one name:
    cargo test -p cabin-rdp --test live_input {{name}} -- --ignored --nocapture

# Run every live test that needs a running guest.
rdp-live:
    cargo test -p cabin-freerdp-sys --test live_connect -- --ignored --nocapture
    cargo test -p cabin-rdp --test live_session -- --ignored --nocapture --test-threads=1
    cargo test -p cabin-rdp --test live_input -- --ignored --nocapture --test-threads=1

# Check the host for everything Cabin needs. Changes nothing.
doctor:
    @build-aux/doctor.sh
