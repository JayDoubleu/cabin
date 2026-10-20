//! Not a test. A helper that leaves a demo desktop behind so the application
//! has something to list. It replaces any container called `demo`. Run with:
//!
//!     just demo
use cabin_core::{DesktopConfig, GraphicsMode};
use cabin_podman::Podman;

#[tokio::test]
#[ignore = "helper, not a test"]
async fn make_demo_desktop() {
    let podman = Podman::new();
    let _ = podman.remove("demo", true).await;
    let mut config = DesktopConfig::new("demo");
    config.memory_mb = Some(4096);
    config.graphics = GraphicsMode::Software;
    let id = podman
        .create_desktop(&config, 13390, &[], &[], "demopassword")
        .await
        .expect("create");
    println!("created demo desktop {id}");
}
