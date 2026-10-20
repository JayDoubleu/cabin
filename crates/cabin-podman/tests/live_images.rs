//! Exercises image management against the real Podman socket.
//!
//!     just guest-image-live
//!
//! This rebuilds the guest image, which takes several minutes, so
//! `just podman-live` leaves it out.

use cabin_podman::{images::build_context, Podman};

const TAG: &str = "localhost/cabin-guest:f44";

#[tokio::test]
#[ignore = "needs a running podman.socket"]
async fn reports_whether_an_image_is_present() {
    let podman = Podman::new();
    assert!(
        !podman
            .image_exists("localhost/definitely-not-here:0")
            .await
            .expect("the query should succeed"),
        "an absent image must report absent, not error"
    );
    // The guest image may or may not be built, so only the call is checked.
    let present = podman.image_exists(TAG).await.expect("query");
    println!("{TAG} present: {present}");
}

#[tokio::test]
#[ignore = "needs a running podman.socket"]
async fn lists_images_with_names_and_sizes() {
    let podman = Podman::new();
    let images = podman.list_images().await.expect("list");
    assert!(!images.is_empty(), "this host should have some images");
    for image in images.iter().take(5) {
        println!(
            "{:<50} {:>10} {}",
            image.display_name(),
            image.human_size(),
            image.short_id()
        );
        assert!(!image.id.is_empty());
    }
}

/// The whole path the application uses: pack the tree, send it, read the
/// output back. Podman's layer cache makes a rebuild quick.
#[tokio::test]
#[ignore = "needs a running podman.socket and rebuilds the guest image"]
async fn builds_the_guest_image_from_the_shipped_context() {
    let podman = Podman::new();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../guest");
    let context = build_context(&dir).expect("pack the context");
    println!("context is {} bytes", context.len());

    let mut lines = 0usize;
    let mut tail = String::new();
    podman
        .build_image(TAG, context, |line| {
            lines += 1;
            tail = line.trim_end().to_string();
            if lines.is_multiple_of(20) {
                println!("... {}", tail);
            }
        })
        .await
        .expect("the build should succeed");

    println!("{lines} lines of output, last: {tail}");
    assert!(lines > 0, "a build should report what it did");
    assert!(
        podman.image_exists(TAG).await.expect("query"),
        "the image should exist once the build reports success"
    );
}

#[tokio::test]
#[ignore = "needs a running podman.socket"]
async fn refuses_to_delete_an_image_a_container_uses() {
    // Podman protects an image that a container was made from. Cabin shows
    // that refusal rather than forcing past it.
    let podman = Podman::new();
    if !podman.image_exists(TAG).await.unwrap_or(false) {
        eprintln!("no guest image, skipping");
        return;
    }
    let desktops = podman.list_desktops().await.expect("list");
    if desktops.is_empty() {
        eprintln!("no desktops use the image, skipping");
        return;
    }
    match podman.remove_image(TAG, false).await {
        Ok(()) => panic!("the image was deleted while a desktop still uses it"),
        Err(e) => println!("refused as expected: {e}"),
    }
}
