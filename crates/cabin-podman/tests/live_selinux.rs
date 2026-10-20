//! Proves a desktop is created with the SELinux label it needs.
//!
//!     just podman-live
//!
//! The recipe runs each live Podman test file in turn, because they share
//! the host Podman and the guest image.
//!
//! The default type cannot mount the devpts that Fedora's image decoder
//! sandbox needs, and a desktop with the wrong label starts, connects, and
//! then shows blank squares in place of every icon. Nothing logs a reason, so
//! only the label itself gives the fault away.

use cabin_core::{DesktopConfig, GraphicsMode, GUEST_SELINUX_TYPE};
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

const NAME: &str = "cabin-selinux-test";

#[tokio::test]
#[ignore = "needs a running podman.socket and the cabin-guest image"]
async fn a_desktop_runs_under_the_type_that_can_sandbox() {
    let podman = Podman::new();
    let _ = podman.remove(NAME, true).await;

    let mut config = DesktopConfig::new(NAME);
    config.graphics = GraphicsMode::Software;
    config.memory_mb = Some(2048);

    podman
        .create_desktop(&config, 13997, &[], &[], "selinux-test")
        .await
        .expect("create");

    let output = host_podman(&["inspect", NAME, "--format", "{{.ProcessLabel}}"]);
    let label = String::from_utf8_lossy(&output.stdout).trim().to_string();
    println!("process label: {label}");

    let _ = podman.remove(NAME, true).await;
    let _ = host_podman(&["volume", "rm", &cabin_core::home_volume_name(NAME)]);

    assert!(
        label.contains(GUEST_SELINUX_TYPE),
        "a desktop must run as {GUEST_SELINUX_TYPE}, got {label:?}"
    );
    // The type that Podman falls back to when the request does not take. It
    // reads as a working container and it cannot draw an icon.
    assert!(
        !label.contains("container_init_t"),
        "the label was not kept: {label:?}"
    );
    // Not label=disable, which also permits the mount and gives up both the
    // type and the per-container categories.
    assert!(!label.is_empty(), "the container should still be labelled");
    assert!(
        label.contains(":s0:c"),
        "the container should keep its own MCS categories: {label:?}"
    );
}
