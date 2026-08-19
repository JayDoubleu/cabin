# Cabin task runner. Run "just" to list tasks.
app_id := "io.github.jaydoubleu.Cabin"
manifest := "build-aux/" + app_id + ".yaml"
guest_image := "localhost/cabin-guest:f44"

default:
    @just --list

# --- native development ---

# Build the Rust workspace.
build:
    cargo build

# Run the app natively, without the Flatpak sandbox.
run:
    RUST_LOG=cabin=debug cargo run -p cabin-gtk

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
    podman build -t {{guest_image}} -f guest/Containerfile guest/

# Start a throwaway guest and report whether RDP came up.
guest-test password="testpass" port="13389":
    -podman rm -f cabin-test
    podman run -d --name cabin-test --systemd=always \
        --publish 127.0.0.1:{{port}}:3389 \
        --env CABIN_PASSWORD={{password}} \
        --memory 4g --shm-size 512m \
        {{guest_image}}
    @echo "Waiting for the session ..."
    @until podman exec cabin-test systemctl is-system-running 2>/dev/null \
        | grep -qE 'running|degraded'; do sleep 1; done
    @podman exec cabin-test systemctl --user --machine=cabin@ \
        list-units 'cabin*' 'gnome-remote*' --no-legend --no-pager || true
    @echo "RDP should be on 127.0.0.1:{{port}} as user 'cabin'"

# Check that the guest is really speaking RDP.
guest-probe port="13389":
    python3 build-aux/rdp-probe.py {{port}}

guest-shell:
    podman exec -it cabin-test bash

guest-clean:
    -podman rm -f cabin-test

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
