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
#
# The guest runs as container_engine_t, the same type the app uses. Under the
# default container_t every image decode fails and the session shows blank
# squares in place of icons. See DESIGN.md section 22. Never use label=disable.
#
# Connect with clipboard sharing turned off. A client that advertises the
# cliprdr channel makes gnome-remote-desktop mount a FUSE filesystem, the mount
# needs CAP_SYS_ADMIN, and the daemon calls abort() when it fails. Passing
# /dev/fuse does not change that. See DESIGN.md section 15.2.
guest-test password="testpass" port="13389":
    #!/usr/bin/env bash
    set -euo pipefail
    {{podman}} rm -f cabin-test >/dev/null 2>&1 || true
    {{podman}} run -d --name cabin-test --systemd=always \
        --security-opt label=type:container_engine_t \
        --publish 127.0.0.1:{{port}}:3389 \
        --env CABIN_PASSWORD={{password}} \
        --memory 4g --shm-size 512m \
        {{guest_image}}
    echo "Waiting for the desktop session and the RDP port ..."
    # systemd finishing is not enough. gnome-remote-desktop only listens once
    # GNOME Shell is up, so wait for the port itself. A cold start can take a
    # while, so the limit is generous, but a stopped container ends the wait.
    bound=no
    for _ in $(seq 180); do
        running=$({{podman}} container inspect -f '{{{{.State.Running}}' cabin-test)
        if [ "$running" != "true" ]; then
            code=$({{podman}} container inspect -f '{{{{.State.ExitCode}}' cabin-test)
            logs=$({{podman}} logs cabin-test 2>&1 || true)
            echo "The guest stopped with exit code $code."
            [ -n "$logs" ] && printf '%s\n' "$logs"
            if [ "$code" = "255" ] && [ -z "$logs" ]; then
                # Keep this in step with diagnose_early_exit in cabin-podman.
                echo "The most likely cause is that this computer has run out of"
                echo "inotify instances, which every desktop needs. Raise the limit with:"
                echo "    sudo sysctl -w fs.inotify.max_user_instances=1024"
                echo "To keep it after a restart, put fs.inotify.max_user_instances=1024"
                echo "in /etc/sysctl.d/99-cabin.conf"
            fi
            exit 1
        fi
        # Capture the listing before the test. With pipefail, "grep -q" can
        # exit before podman finishes writing, podman then dies of SIGPIPE,
        # and a bound port reads as unbound. The pattern also stops a port
        # such as 33890 from matching.
        sockets=$({{podman}} exec cabin-test ss -Htln 2>/dev/null || true)
        if grep -qE ':3389([[:space:]]|$)' <<<"$sockets"; then
            bound=yes
            break
        fi
        sleep 1
    done
    if [ "$bound" != "yes" ]; then
        echo "The guest is running, but port 3389 never bound."
        echo "Run 'just guest-shell' and check 'journalctl' in the guest."
        exit 1
    fi
    {{podman}} exec cabin-test sudo -u cabin XDG_RUNTIME_DIR=/run/user/1000 \
        systemctl --user list-units 'cabin*' 'gnome-remote*' \
        --no-legend --no-pager || true
    just guest-probe {{port}}
    echo ""
    echo "Connect an RDP client to 127.0.0.1:{{port}} with clipboard sharing off."
    echo "  username: cabin"
    echo "  password: {{password}}"
    echo "The TLS certificate is self-signed, so the client warns on first connect."

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
# rofiles-fuse can fail to mount inside a toolbox, so it is off.
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
    if ! command -v dnf >/dev/null; then
        echo "freerdp-sysroot needs dnf, so it works on Fedora only."
        echo "On other distributions, install the FreeRDP 3 development package"
        echo "instead, for example freerdp3-dev on Debian and Ubuntu or freerdp on Arch."
        exit 1
    fi
    dest="$(eval echo {{dest}})"
    work="$(mktemp -d)"
    trap 'rm -rf "$work"' EXIT
    echo "Downloading FreeRDP 3 packages ..."
    ( cd "$work" && dnf download --quiet freerdp-devel freerdp-libs libwinpr libwinpr-devel \
        sso-mib-libs uriparser )
    shopt -s nullglob
    rpms=( "$work"/*"$(uname -m)".rpm )
    if [ "${#rpms[@]}" -eq 0 ]; then
        echo "dnf downloaded no packages for $(uname -m)." >&2
        exit 1
    fi
    mkdir -p "$dest"
    for r in "${rpms[@]}"; do
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

# Run every live Podman test. Each binary runs on its own, because they share
# the host Podman and the guest image. The nested Podman test is skipped
# because nested Podman does not work yet (DESIGN.md section 15.17).
podman-live:
    cargo test -p cabin-podman --test live_podman -- --ignored --test-threads=1
    cargo test -p cabin-podman --test live_recreate -- --ignored --test-threads=1 --skip nested_podman_can_run_a_container
    cargo test -p cabin-podman --test live_delete -- --ignored --test-threads=1
    cargo test -p cabin-podman --test live_overlay -- --ignored --test-threads=1
    cargo test -p cabin-podman --test live_selinux -- --ignored --test-threads=1
    cargo test -p cabin-podman --test live_session_health -- --ignored --test-threads=1

# Run the live guest image tests. This rebuilds {{guest_image}} and takes
# several minutes, so it is apart from podman-live.
guest-image-live:
    cargo test -p cabin-podman --test live_images -- --ignored --test-threads=1

# Check the host for everything Cabin needs. Changes nothing.
doctor:
    @build-aux/doctor.sh
