//! Exercises the whole session path: the thread, both channels, and the frames
//! the widget consumes.
//!
//! Needs a running guest:
//!     just guest-clean && just guest-test
//!     cargo test -p cabin-rdp -- --ignored --nocapture

use cabin_rdp::{
    run_session, ConnectionParams, Credentials, PixelFormat, SessionCommand, SessionEvent,
};
use std::time::Duration;

fn params(port: u16) -> ConnectionParams {
    ConnectionParams {
        host: "127.0.0.1".to_string(),
        port,
        credentials: Credentials {
            username: "cabin".to_string(),
            password: "testpass".to_string(),
            domain: String::new(),
        },
        width: 1920,
        height: 1080,
        // These tests connect to a throwaway guest, so there is nothing
        // recorded to check against.
        expected_fingerprint: None,
    }
}

/// Runs a session and returns the events it produced, stopping once `wanted`
/// frames have arrived or the session ends.
fn collect_frames(port: u16, wanted: usize) -> Vec<SessionEvent> {
    let (command_tx, command_rx) = std::sync::mpsc::channel();
    let (event_tx, event_rx) = async_channel::bounded(1);

    let handle = std::thread::spawn(move || run_session(params(port), command_rx, event_tx));

    let mut collected = Vec::new();
    let mut frames = 0;
    let deadline = std::time::Instant::now() + Duration::from_secs(30);

    while std::time::Instant::now() < deadline {
        match event_rx.recv_blocking() {
            Ok(event) => {
                if matches!(event, SessionEvent::Frame(_)) {
                    frames += 1;
                }
                let finished = matches!(
                    event,
                    SessionEvent::Disconnected { .. } | SessionEvent::Error(_)
                );
                collected.push(event);
                if finished || frames >= wanted {
                    break;
                }
            }
            Err(_) => break,
        }
    }

    let _ = command_tx.send(SessionCommand::Disconnect);
    drop(event_rx);
    let _ = handle.join();
    collected
}

#[test]
#[ignore = "needs a running guest from 'just guest-test'"]
fn session_thread_delivers_usable_frames() {
    let events = collect_frames(13389, 1);

    assert!(
        matches!(events.first(), Some(SessionEvent::Connected { .. })),
        "the first event should be Connected, got {:?}",
        events.first()
    );

    let frame = events
        .iter()
        .find_map(|e| match e {
            SessionEvent::Frame(f) => Some(f),
            _ => None,
        })
        .expect("at least one frame should arrive");

    println!("frame {}x{}", frame.width, frame.height);
    assert_eq!(frame.format, PixelFormat::Bgra8888);
    assert_eq!(frame.width, 1920);
    assert_eq!(frame.height, 1080);

    // The widget hands this straight to GdkMemoryTexture with a stride of
    // width * 4, so any row padding must already be gone.
    assert!(
        frame.is_consistent(),
        "the buffer must be packed, {} bytes for {}x{}",
        frame.data.len(),
        frame.width,
        frame.height
    );

    let painted = frame
        .data
        .chunks(4)
        .filter(|p| p[0] | p[1] | p[2] != 0)
        .count();
    println!("painted pixels: {painted}");
    assert!(painted > 1000, "the frame looks blank");
}

#[test]
#[ignore = "needs a running guest from 'just guest-test'"]
fn wrong_password_reports_an_authentication_failure() {
    let (command_tx, command_rx) = std::sync::mpsc::channel();
    let (event_tx, event_rx) = async_channel::bounded(1);

    let mut bad = params(13389);
    bad.credentials.password = "definitely-not-the-password".to_string();

    let handle = std::thread::spawn(move || run_session(bad, command_rx, event_tx));

    let event = event_rx.recv_blocking().expect("an event should arrive");
    match event {
        SessionEvent::Error(message) => {
            println!("rejected as expected: {message}");
            assert!(
                message.to_lowercase().contains("credential")
                    || message.to_lowercase().contains("authentication"),
                "the message should name the cause, got {message}"
            );
        }
        other => panic!("expected an error, got {other:?}"),
    }

    drop(command_tx);
    let _ = handle.join();
}
