//! Interop test against a real Cabin guest.
//!
//! This answers open question 4 in DESIGN.md: does IronRDP interoperate with
//! gnome-remote-desktop? CredSSP is where RDP implementations most often
//! disagree, and the guest makes it mandatory.
//!
//! Start a guest first:
//!     just guest-test
//!
//! Then:
//!     cargo test -p cabin-rdp -- --ignored --nocapture

use cabin_rdp::{connect, ConnectionParams, Credentials};

fn params() -> ConnectionParams {
    ConnectionParams {
        host: "127.0.0.1".to_string(),
        port: 13389,
        credentials: Credentials {
            username: "cabin".to_string(),
            password: "testpass".to_string(),
            domain: String::new(),
        },
        width: 1920,
        height: 1080,
    }
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "ironrdp=trace".into()),
        )
        .try_init();
}

#[tokio::test]
#[ignore = "needs a running guest from 'just guest-test'"]
async fn connects_with_credssp() {
    init_tracing();
    let connection = connect(&params())
        .await
        .expect("the connection should succeed");
    let size = connection.result.desktop_size;
    println!("connected, desktop size {}x{}", size.width, size.height);
    assert!(size.width > 0 && size.height > 0);
}

#[tokio::test]
#[ignore = "needs a running guest from 'just guest-test'"]
async fn rejects_a_wrong_password() {
    let mut p = params();
    p.credentials.password = "definitely-not-the-password".to_string();
    match connect(&p).await {
        Ok(_) => panic!("a wrong password must not connect"),
        Err(e) => println!("rejected as expected: {e}"),
    }
}
