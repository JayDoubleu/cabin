//! Proves that deleting a desktop removes its files, and that only deleting
//! does.
//!
//!     just podman-live
//!
//! The recipe runs each live Podman test file in turn, because they share
//! the host Podman and the guest image.
//!
//! The home and state volumes are named after the desktop. If a delete left
//! them, a new desktop with the same name would silently take over the old
//! files and the old certificate.

use cabin_core::{DesktopConfig, GraphicsMode};
use cabin_podman::Podman;
use std::process::Command;

/// Runs a podman command against the same podman the client talks to.
///
/// Inside a Fedora toolbox the bare `podman` binary is the container's own,
/// with its own storage, and it cannot join the host user namespace. The
/// desktops live on the host, so the command has to go there.
fn host_podman(args: &[&str]) -> std::process::Output {
    if std::path::Path::new("/run/.toolboxenv").exists() {
        let mut full = vec!["--host", "podman"];
        full.extend_from_slice(args);
        Command::new("flatpak-spawn").args(&full).output()
    } else {
        Command::new("podman").args(args).output()
    }
    .expect("could not run podman")
}

fn volume_exists(name: &str) -> bool {
    host_podman(&["volume", "exists", name]).status.success()
}

const NAME: &str = "cabin-delete-test";

#[tokio::test]
#[ignore = "needs a running podman.socket and the cabin-guest image"]
async fn deleting_a_desktop_removes_both_volumes() {
    let podman = Podman::new();
    let _ = podman.remove(NAME, true).await;

    let mut config = DesktopConfig::new(NAME);
    config.graphics = GraphicsMode::Software;
    config.memory_mb = Some(2048);
    let id = podman
        .create_desktop(&config, 13987, &[], &[], "delete-test")
        .await
        .expect("create");

    let home = cabin_core::home_volume_name(NAME);
    let state = cabin_core::state_volume_name(NAME);
    assert!(volume_exists(&home), "the home volume should exist");
    assert!(volume_exists(&state), "the state volume should exist");

    podman.delete_desktop(&id, NAME).await.expect("delete");

    assert!(!volume_exists(&home), "{home} was left behind");
    assert!(!volume_exists(&state), "{state} was left behind");
    assert!(
        !podman
            .list_desktops()
            .await
            .expect("list")
            .iter()
            .any(|d| d.config.name == NAME),
        "the desktop should be gone"
    );
}

#[tokio::test]
#[ignore = "needs a running podman.socket"]
async fn removing_a_volume_that_is_gone_is_not_an_error() {
    // A delete that stopped part way must be able to run again.
    let podman = Podman::new();
    podman
        .remove_volume("cabin-delete-test-never-existed")
        .await
        .expect("a missing volume is already removed");
}
