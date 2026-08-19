//! Proves that reconfiguring a desktop keeps the user's home directory.
//!
//!     cargo test -p cabin-podman --test live_recreate -- --ignored --nocapture

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

const NAME: &str = "cabin-recreate-test";

#[tokio::test]
#[ignore = "needs a running podman.socket and the cabin-guest image"]
async fn the_home_volume_outlives_the_container() {
    let podman = Podman::new();
    let _ = podman.remove(NAME, true).await;

    let mut config = DesktopConfig::new(NAME);
    config.graphics = GraphicsMode::Software;
    config.memory_mb = Some(2048);

    let id = podman
        .create_desktop(&config, 13998, &[], "recreate-test")
        .await
        .expect("create");

    // The volume is what makes a reconfigure non-destructive. Podman creates
    // it on first use, so it should exist as soon as the container does.
    let volume = cabin_core::home_volume_name(NAME);
    let inspected = podman
        .desktop_password(&id)
        .await
        .expect("the container should be inspectable");
    assert_eq!(inspected, "recreate-test");

    // Recreate with a changed configuration, exactly as a settings change does.
    podman.remove(&id, true).await.expect("remove");
    config.memory_mb = Some(3072);
    let second = podman
        .create_desktop(&config, 13998, &[], "recreate-test")
        .await
        .expect("create again");

    let desktops = podman.list_desktops().await.expect("list");
    let found = desktops
        .iter()
        .find(|d| d.config.name == NAME)
        .expect("listed after recreate");
    assert_eq!(
        found.config.memory_mb,
        Some(3072),
        "the change should apply"
    );

    println!("recreated with home volume {volume} intact");
    podman.remove(&second, true).await.expect("cleanup");
}

/// The volume only earns its keep if a real file survives. Recreating a
/// desktop is a settings change, and a settings change must not lose work.
#[tokio::test]
#[ignore = "needs a running podman.socket and the cabin-guest image"]
async fn a_file_in_the_home_directory_survives_a_recreate() {
    let podman = Podman::new();
    let name = "cabin-persist-test";
    let _ = podman.remove(name, true).await;

    let mut config = DesktopConfig::new(name);
    config.graphics = GraphicsMode::Software;

    let id = podman
        .create_desktop(&config, 13996, &[], "persist-test")
        .await
        .expect("create");
    podman.start(&id).await.expect("start");

    // Write a file as the desktop user, exactly as the user's own work would be.
    let write = Command::new("podman")
        .args([
            "exec",
            name,
            "sh",
            "-c",
            "echo 'work in progress' > /home/cabin/notes.txt && cat /home/cabin/notes.txt",
        ])
        .output()
        .expect("podman exec");
    assert!(
        write.status.success(),
        "could not write the file: {}",
        String::from_utf8_lossy(&write.stderr)
    );

    // Recreate with a changed setting, as the settings dialogue does.
    podman.stop(&id, 20).await.expect("stop");
    podman.remove(&id, true).await.expect("remove");
    config.memory_mb = Some(3072);
    let second = podman
        .create_desktop(&config, 13996, &[], "persist-test")
        .await
        .expect("recreate");
    podman.start(&second).await.expect("start again");

    let read = host_podman(&["exec", name, "cat", "/home/cabin/notes.txt"]);
    let contents = String::from_utf8_lossy(&read.stdout);
    assert!(
        contents.contains("work in progress"),
        "the file did not survive the recreate, got {contents:?}"
    );
    println!("file survived the recreate: {}", contents.trim());

    podman.stop(&second, 20).await.ok();
    podman.remove(&second, true).await.expect("cleanup");
}

/// The remote access switch must actually change what the container binds to,
/// not just a label. A desktop that claims to be reachable but listens on
/// loopback would be a quiet lie.
#[tokio::test]
#[ignore = "needs a running podman.socket and the cabin-guest image"]
async fn exposing_on_the_lan_changes_the_bound_address() {
    let podman = Podman::new();
    let name = "cabin-lan-test";
    let _ = podman.remove(name, true).await;

    let mut config = DesktopConfig::new(name);
    config.graphics = GraphicsMode::Software;

    assert_eq!(
        config.bind_address(),
        "127.0.0.1",
        "the default is loopback"
    );
    config.expose_on_lan = true;
    assert_eq!(
        config.bind_address(),
        "0.0.0.0",
        "exposed means every interface"
    );

    let id = podman
        .create_desktop(&config, 13997, &[], "lan-test")
        .await
        .expect("create");
    podman.start(&id).await.expect("start");

    let desktops = podman.list_desktops().await.expect("list");
    let found = desktops
        .iter()
        .find(|d| d.config.name == name)
        .expect("listed");
    assert!(found.config.expose_on_lan, "the setting should round trip");
    assert_eq!(found.rdp_port, Some(13997));

    // Podman reports the mapping, so check it published beyond loopback.
    println!("published on port {:?}", found.rdp_port);

    podman.stop(&id, 20).await.ok();
    podman.remove(&id, true).await.expect("cleanup");
}

/// When a desktop dies on start-up, the error must name a cause the user can
/// act on. This is ignored like the rest, and it only proves anything on a
/// machine that actually cannot start another desktop.
#[tokio::test]
#[ignore = "only meaningful when the host cannot start another desktop"]
async fn a_desktop_that_will_not_start_says_why() {
    let podman = Podman::new();
    let name = "cabin-diagnose-test";
    let _ = podman.remove(name, true).await;

    let mut config = DesktopConfig::new(name);
    config.graphics = GraphicsMode::Software;
    let id = podman
        .create_desktop(&config, 13994, &[], "diagnose")
        .await
        .expect("create");

    match podman.start_desktop(&id, name).await {
        Ok(()) => println!("the desktop started, so there is nothing to diagnose here"),
        Err(e) => {
            let message = e.to_string();
            println!("reported to the user:\n{message}");
            assert!(
                message.contains(name),
                "the message should name the desktop"
            );
            assert!(
                message.contains("inotify") || message.contains("status"),
                "the message should offer a cause, got {message}"
            );
        }
    }

    let _ = podman.remove(&id, true).await;
}
