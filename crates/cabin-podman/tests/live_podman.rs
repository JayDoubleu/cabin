//! Integration tests against a real Podman socket.
//!
//! These are ignored by default, because they need a running
//! podman.socket and the cabin-guest image. Run them with:
//!
//!     just podman-live
//!
//! Do not run every ignored test in this crate at once. That also runs
//! `make_demo`, which replaces any container called `demo`, and
//! `live_images`, which rebuilds the guest image.
//!
//! Every test cleans up after itself, and uses a name that no real desktop
//! would use.

use cabin_core::{DesktopConfig, GraphicsMode, Mount, MountMode, State};
use cabin_podman::Podman;
use std::process::Command;

/// Runs the host Podman, as rule 17 in CLAUDE.md asks.
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

/// Runs `podman exec` in a desktop and returns what it printed. A failed
/// command fails the test here, so that it does not compare an empty string.
fn exec_in(name: &str, command: &[&str]) -> String {
    let mut args = vec!["exec", name];
    args.extend_from_slice(command);
    let output = host_podman(&args);
    assert!(
        output.status.success(),
        "podman exec {command:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// Reads the UTC offset on the host, where the desktop runs. Inside a
/// toolbox this goes through `flatpak-spawn`, as `host_podman` does.
fn host_utc_offset() -> String {
    let output = if std::path::Path::new("/run/.toolboxenv").exists() {
        Command::new("flatpak-spawn")
            .args(["--host", "date", "+%z"])
            .output()
    } else {
        Command::new("date").arg("+%z").output()
    }
    .expect("could not run date on the host");
    assert!(output.status.success(), "date failed on the host");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

const TEST_NAME: &str = "cabin-integration-test";
const TEST_PORT: u16 = 13999;

async fn cleanup(podman: &Podman) {
    // force, because the container may be running from a failed earlier run.
    // delete_desktop also takes the named volumes, which a bare remove
    // leaves behind on every run.
    let _ = podman.delete_desktop(TEST_NAME, TEST_NAME).await;
}

fn test_config() -> DesktopConfig {
    let mut config = DesktopConfig::new(TEST_NAME);
    config.memory_mb = Some(2048);
    // Software rendering keeps the test independent of the host GPU.
    config.graphics = GraphicsMode::Software;
    config
}

#[tokio::test]
#[ignore = "needs a running podman.socket"]
async fn socket_answers() {
    let podman = Podman::new();
    podman.ping().await.expect(
        "podman.socket should answer; enable it with systemctl --user enable --now podman.socket",
    );
}

#[tokio::test]
#[ignore = "needs a running podman.socket and the cabin-guest image"]
async fn create_list_start_stop_remove() {
    let podman = Podman::new();
    cleanup(&podman).await;

    let config = test_config();
    let id = podman
        .create_desktop(&config, TEST_PORT, &[], &[], "integration-test-password")
        .await
        .expect("create should succeed");
    assert!(!id.is_empty());

    // The new desktop must appear in the list, which proves the labels
    // round-trip through Podman.
    let desktops = podman.list_desktops().await.expect("list should succeed");
    let found = desktops
        .iter()
        .find(|d| d.config.name == TEST_NAME)
        .expect("the created desktop should be listed");
    assert_eq!(found.state, State::Created);
    assert_eq!(found.config.memory_mb, Some(2048));
    assert_eq!(found.config.graphics, GraphicsMode::Software);

    podman.start(&id).await.expect("start should succeed");
    let desktops = podman.list_desktops().await.unwrap();
    let running = desktops
        .iter()
        .find(|d| d.config.name == TEST_NAME)
        .expect("still listed after start");
    assert!(running.state.is_running(), "state was {:?}", running.state);
    assert_eq!(
        running.rdp_port,
        Some(TEST_PORT),
        "the RDP port mapping should be reported"
    );
    // The viewer compares this to see a restart that a batch of events hid.
    assert!(
        running.started_at > 0,
        "started_at was {}",
        running.started_at
    );

    // A guest in UTC shows a different hour from the host on its clock.
    let guest_zone = exec_in(TEST_NAME, &["date", "+%z"]);
    let host_zone = host_utc_offset();
    assert_eq!(
        guest_zone, host_zone,
        "the guest must use the time zone of the host"
    );

    // The prompt in the guest's terminal shows the host name.
    let hostname = exec_in(TEST_NAME, &["cat", "/proc/sys/kernel/hostname"]);
    assert_eq!(hostname, TEST_NAME, "the guest is named after the desktop");

    podman.stop(&id, 20).await.expect("stop should succeed");
    podman
        .remove(&id, false)
        .await
        .expect("remove should succeed");

    let desktops = podman.list_desktops().await.unwrap();
    assert!(
        !desktops.iter().any(|d| d.config.name == TEST_NAME),
        "the desktop should be gone after removal"
    );
    // The bare remove keeps the named volumes, as a recreate needs.
    cleanup(&podman).await;
}

#[tokio::test]
#[ignore = "needs a running podman.socket and the cabin-guest image"]
async fn mounts_and_config_survive_a_round_trip() {
    let podman = Podman::new();
    cleanup(&podman).await;

    let mut config = test_config();
    config.mounts.push(Mount {
        host_path: "/tmp".to_string(),
        guest_path: "/mnt/host-tmp".to_string(),
        mode: MountMode::ReadOnly,
        relabel: false,
        kind: cabin_core::MountKind::Directory,
    });
    config.width = 2560;
    config.height = 1440;

    let id = podman
        .create_desktop(&config, TEST_PORT, &[], &[], "integration-test-password")
        .await
        .expect("create should succeed");

    let desktops = podman.list_desktops().await.unwrap();
    let found = desktops
        .iter()
        .find(|d| d.config.name == TEST_NAME)
        .expect("listed");

    assert_eq!(found.config.mounts.len(), 1);
    assert_eq!(found.config.mounts[0].guest_path, "/mnt/host-tmp");
    assert_eq!(found.config.mounts[0].mode, MountMode::ReadOnly);
    assert_eq!(found.config.width, 2560);
    assert_eq!(found.config.height, 1440);

    podman
        .delete_desktop(&id, TEST_NAME)
        .await
        .expect("delete should succeed");
}

#[tokio::test]
#[ignore = "needs a running podman.socket"]
async fn unmanaged_containers_are_not_listed() {
    // Containers that Cabin did not create must never appear. The guest test
    // container from "just guest-test" is exactly such a container.
    let podman = Podman::new();
    let desktops = podman.list_desktops().await.expect("list should succeed");
    assert!(
        !desktops.iter().any(|d| d.name.contains("cabin-test")),
        "an unlabelled container leaked into the desktop list"
    );
}
