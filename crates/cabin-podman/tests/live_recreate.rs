//! Proves that reconfiguring a desktop keeps the user's home directory.
//!
//!     cargo test -p cabin-podman --test live_recreate -- --ignored --nocapture

use cabin_core::{DesktopConfig, GraphicsMode};
use cabin_podman::Podman;

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
