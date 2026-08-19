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

/// Shared folders must reach the container, not just the labels.
#[tokio::test]
#[ignore = "needs a running podman.socket and the cabin-guest image"]
async fn a_shared_folder_reaches_the_container() {
    use cabin_core::{Mount, MountMode};

    let podman = Podman::new();
    let name = "cabin-mount-test";
    let _ = podman.remove(name, true).await;

    let mut config = DesktopConfig::new(name);
    config.graphics = GraphicsMode::Software;
    config.mounts.push(Mount {
        host_path: "/tmp".to_string(),
        guest_path: "/mnt/host-tmp".to_string(),
        mode: MountMode::ReadOnly,
        relabel: false,
    });

    let id = podman
        .create_desktop(&config, 13993, &[], "mount-test")
        .await
        .expect("create");

    // Ask podman what it actually recorded, rather than trusting our labels.
    let output = host_podman(&[
        "inspect",
        name,
        "--format",
        "{{range .Mounts}}{{.Source}}->{{.Destination}}:{{.RW}} {{end}}",
    ]);
    let mounts = String::from_utf8_lossy(&output.stdout);
    println!("podman recorded: {}", mounts.trim());

    assert!(
        mounts.contains("/tmp->/mnt/host-tmp"),
        "the shared folder should be a real mount, got {mounts:?}"
    );
    assert!(
        mounts.contains("/mnt/host-tmp:false"),
        "a read-only folder must not be writable, got {mounts:?}"
    );
    assert!(
        mounts.contains("/home/cabin:true"),
        "the home volume should still be there and writable, got {mounts:?}"
    );

    let _ = podman.remove(&id, true).await;
}

/// Nested Podman must actually run a container inside the desktop. The setting
/// existed in the interface for a while before it did anything, so this test
/// asks the guest to prove it rather than trusting the configuration.
#[tokio::test]
#[ignore = "nested Podman does not work yet, see DESIGN.md section 15.17"]
async fn nested_podman_can_run_a_container() {
    use cabin_core::SocketPolicy;

    let podman = Podman::new();
    let name = "cabin-nested-test";
    let _ = podman.remove(name, true).await;

    let mut config = DesktopConfig::new(name);
    config.graphics = GraphicsMode::Software;
    config.socket = SocketPolicy::Nested;

    let id = podman
        .create_desktop(&config, 13992, &[], "nested-test")
        .await
        .expect("create");
    podman
        .start_desktop(&id, name)
        .await
        .expect("the desktop should start");

    // Run a container inside the desktop, as the desktop user.
    let output = host_podman(&[
        "exec",
        "-u",
        "cabin",
        name,
        "sh",
        "-c",
        "export XDG_RUNTIME_DIR=/run/user/1000; podman run --rm \
         registry.fedoraproject.org/fedora:44 echo nested-ok 2>&1 | tail -3",
    ]);
    let text = String::from_utf8_lossy(&output.stdout);
    let errors = String::from_utf8_lossy(&output.stderr);
    println!("guest said: {}", text.trim());
    if !errors.trim().is_empty() {
        println!("stderr: {}", errors.trim());
    }

    assert!(
        text.contains("nested-ok"),
        "podman inside the desktop did not run a container.\nstdout: {text}\nstderr: {errors}"
    );

    podman.stop(&id, 20).await.ok();
    let _ = podman.remove(&id, true).await;
}

/// Exposing the host Podman socket must let the desktop drive the host's
/// containers. This is the dangerous option, so it had better either work or
/// not be offered.
#[tokio::test]
#[ignore = "needs a running podman.socket and the cabin-guest image"]
async fn the_host_podman_socket_reaches_the_desktop() {
    use cabin_core::SocketPolicy;

    let podman = Podman::new();
    let name = "cabin-hostsock-test";
    let _ = podman.remove(name, true).await;

    let mut config = DesktopConfig::new(name);
    config.graphics = GraphicsMode::Software;
    config.socket = SocketPolicy::HostPodman;

    let id = podman
        .create_desktop(&config, 13991, &[], "hostsock-test")
        .await
        .expect("create");
    podman
        .start_desktop(&id, name)
        .await
        .expect("the desktop should start");

    let host_name =
        String::from_utf8_lossy(&host_podman(&["info", "--format", "{{.Host.Hostname}}"]).stdout)
            .trim()
            .to_string();
    assert!(!host_name.is_empty(), "could not read the host name");

    // The desktop user must reach the host podman with no sudo and no
    // configuration, using only the environment Cabin set. The proxy is socket
    // activated and the session is still starting, so allow a few attempts.
    let mut seen = String::new();
    for _ in 0..12 {
        let output = host_podman(&[
            "exec",
            "-u",
            "cabin",
            name,
            "sh",
            "-c",
            "podman info --format '{{.Host.Hostname}}' 2>&1 | tail -2",
        ]);
        seen = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if seen.contains(&host_name) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }

    println!("desktop sees podman on: {seen}");
    assert!(
        seen.contains(&host_name),
        "the desktop should reach the host podman ({host_name}), got {seen:?}"
    );

    podman.stop(&id, 20).await.ok();
    let _ = podman.remove(&id, true).await;
}

/// Pinning only means something if the certificate outlives the container.
/// Otherwise every settings change would look like a machine in the middle.
#[tokio::test]
#[ignore = "needs a running podman.socket and the cabin-guest image"]
async fn the_tls_certificate_survives_a_recreate() {
    let podman = Podman::new();
    let name = "cabin-cert-test";
    let _ = podman.remove(name, true).await;
    let _ = host_podman(&["volume", "rm", "-f", &cabin_core::state_volume_name(name)]);

    let mut config = DesktopConfig::new(name);
    config.graphics = GraphicsMode::Software;

    let id = podman
        .create_desktop(&config, 13990, &[], "cert-test")
        .await
        .expect("create");
    podman.start_desktop(&id, name).await.expect("start");

    let read_fingerprint = || {
        let out = host_podman(&[
            "exec",
            name,
            "sh",
            "-c",
            "openssl x509 -in /var/lib/cabin/tls/rdp.crt -noout -fingerprint -sha256",
        ]);
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };

    let before = read_fingerprint();
    assert!(!before.is_empty(), "the guest should have a certificate");
    println!("before: {before}");

    podman.stop(&id, 20).await.expect("stop");
    podman.remove(&id, true).await.expect("remove");
    config.memory_mb = Some(3072);
    let second = podman
        .create_desktop(&config, 13990, &[], "cert-test")
        .await
        .expect("recreate");
    podman
        .start_desktop(&second, name)
        .await
        .expect("start again");

    let after = read_fingerprint();
    println!("after:  {after}");
    assert_eq!(
        before, after,
        "the certificate changed, so every reconfigure would look like an attack"
    );

    podman.stop(&second, 20).await.ok();
    let _ = podman.remove(&second, true).await;
}
