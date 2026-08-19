//! Proves a customised recipe is the one that gets built.
//!
//! The overlay is only worth having if the build uses it, and nothing else
//! checks that: the packing is unit tested and the build is tested against the
//! shipped tree, but the join between them is not.
//!
//!     cargo test -p cabin-podman --test live_overlay -- --ignored --nocapture

use cabin_podman::{images::build_context, Podman};

const TAG: &str = "localhost/cabin-overlay-test:latest";

#[tokio::test]
#[ignore = "needs a running podman.socket"]
async fn a_customised_recipe_is_what_gets_built() {
    let podman = Podman::new();
    let dir = std::env::temp_dir().join(format!("cabin-overlay-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("scripts")).expect("make the context");

    // Small enough to build in seconds, and shaped like the real one: a
    // Containerfile that copies a file from beside it.
    std::fs::write(
        dir.join("Containerfile"),
        "FROM registry.fedoraproject.org/fedora:44\n\
         LABEL io.github.jaydoubleu.cabin.guest=\"true\"\n\
         COPY scripts/ /usr/libexec/cabin/\n\
         RUN echo overlay-marker > /etc/cabin-overlay\n",
    )
    .expect("write the recipe");
    std::fs::write(dir.join("scripts").join("hello.sh"), "#!/bin/sh\necho hi\n")
        .expect("write a script");

    let context = build_context(&dir).expect("pack");
    let mut failed = None;
    podman
        .build_image(TAG, context, |line| {
            if line.to_lowercase().contains("error") {
                failed = Some(line.to_string());
            }
        })
        .await
        .unwrap_or_else(|e| panic!("the build failed: {e}"));
    std::fs::remove_dir_all(&dir).ok();

    assert!(failed.is_none(), "build reported: {failed:?}");
    assert!(
        podman.image_exists(TAG).await.expect("query"),
        "the customised recipe should have produced an image"
    );

    // The subdirectory has to arrive too. A context that packed only the
    // top level would build a guest with no scripts and fail much later.
    let out = std::process::Command::new("flatpak-spawn")
        .args([
            "--host",
            "podman",
            "run",
            "--rm",
            TAG,
            "sh",
            "-c",
            "cat /etc/cabin-overlay && ls /usr/libexec/cabin/",
        ])
        .output()
        .expect("run the built image");
    let text = String::from_utf8_lossy(&out.stdout);
    println!("image contains: {}", text.trim());
    assert!(
        text.contains("overlay-marker"),
        "the edit did not reach the image"
    );
    assert!(text.contains("hello.sh"), "the subdirectory was not packed");

    podman.remove_image(TAG, true).await.ok();
}
