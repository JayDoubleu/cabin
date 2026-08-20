//! Proves a desktop boots able to draw icons and to list its applications.
//!
//!     cargo test -p cabin-podman --test live_session_health -- --ignored --nocapture
//!
//! Both faults this guards against look identical from outside: the desktop
//! starts, accepts a connection, and shows a session that is missing something
//! obvious. Neither reports a cause, and neither is visible from the container
//! state, so only the session's own log gives them away.

use cabin_core::{DesktopConfig, GraphicsMode};
use cabin_podman::Podman;
use std::process::Command;
use std::time::{Duration, Instant};

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

/// Runs a command inside the desktop as the session user.
fn in_session(command: &str) -> String {
    let output = host_podman(&[
        "exec",
        "-u",
        "cabin",
        "-e",
        "XDG_RUNTIME_DIR=/run/user/1000",
        NAME,
        "sh",
        "-c",
        command,
    ]);
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

const NAME: &str = "cabin-health-test";

#[tokio::test]
#[ignore = "needs a running podman.socket and the cabin-guest image"]
async fn a_desktop_boots_with_icons_and_applications() {
    let podman = Podman::new();
    let _ = podman.remove(NAME, true).await;

    let mut config = DesktopConfig::new(NAME);
    config.graphics = GraphicsMode::Software;
    config.memory_mb = Some(2048);
    let id = podman
        .create_desktop(&config, 13996, &[], &[], "health-test")
        .await
        .expect("create");
    podman.start_desktop(&id, NAME).await.expect("start");

    // The session takes a few seconds to reach the point where either fault
    // would have been logged, so wait for the shell rather than for the
    // container, which is running long before the session is.
    let deadline = Instant::now() + Duration::from_secs(90);
    while Instant::now() < deadline {
        if in_session("pgrep -c gnome-shell")
            .parse::<u32>()
            .unwrap_or(0)
            > 0
        {
            break;
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    // Give the shell time to run its own start-up, which is where both
    // failures are reported.
    std::thread::sleep(Duration::from_secs(10));

    let pixbuf = in_session(
        r#"journalctl --user --no-pager 2>/dev/null | grep -ci "could not load a pixbuf""#,
    );
    let parental =
        in_session(r#"journalctl --user --no-pager 2>/dev/null | grep -ci "parental controls""#);
    let polkit = in_session("systemctl is-active polkit.service");
    let shell_running = in_session("pgrep -c gnome-shell");
    let monospace = in_session("fc-match monospace");
    let apps = in_session(
        r#"for f in /usr/share/applications/*.desktop; do grep -q '^NoDisplay=true' "$f" || echo x; done | wc -l"#,
    );
    println!(
        "pixbuf errors={pixbuf} parental errors={parental} polkit={polkit} shell={shell_running} apps={apps} monospace={monospace}"
    );

    let _ = podman.remove(NAME, true).await;
    let _ = host_podman(&["volume", "rm", &cabin_core::home_volume_name(NAME)]);
    let _ = host_podman(&["volume", "rm", &cabin_core::state_volume_name(NAME)]);

    assert_ne!(shell_running, "0", "the session never started");

    // Every icon is an SVG, and the decoder sandbox cannot run under the
    // wrong SELinux type. The session then draws blank squares everywhere
    // while text still renders, which reads as a theme fault.
    assert_eq!(
        pixbuf, "0",
        "the session could not decode images, so it has no icons"
    );

    // GNOME Shell asks malcontent whether an application may be shown, and
    // malcontent needs polkit. Without it the dash, the application grid and
    // search are all empty while the desktop files are perfectly correct.
    assert_eq!(polkit, "active", "polkit must run or no application shows");
    assert_eq!(
        parental, "0",
        "the parental controls lookup failed, so the shell hides every application"
    );

    // Fedora's font packages are weak dependencies. Without one, fontconfig
    // answers "monospace" with a proportional face, and a terminal lays every
    // glyph on a grid the font does not fit. The text comes out spaced apart
    // and nothing reports it.
    assert!(
        monospace.to_lowercase().contains("mono"),
        "monospace resolved to a proportional font: {monospace:?}"
    );

    // A desktop with a usable set of applications is the point of it. A floor
    // rather than an exact count, so adding one to the recipe does not fail
    // the test.
    assert!(
        apps.parse::<u32>().unwrap_or(0) >= 10,
        "the desktop should carry a usable set of applications, found {apps}"
    );
}
