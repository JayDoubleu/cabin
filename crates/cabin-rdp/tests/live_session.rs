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

/// How long gnome-remote-desktop needs to recycle a headless session.
///
/// The guest serves one session at a time. A connection that arrives while the
/// previous one is tearing down is not queued: it is rejected part way through
/// the capabilities exchange with ERRINFO_LOGOFF_BY_USER, which reads like a
/// client fault and is not one. See DESIGN.md section 16.3.
const SESSION_SETTLE: Duration = Duration::from_secs(4);

/// Serialises the tests in this file and leaves the guest time to recover.
///
/// Cargo runs tests in one binary on several threads by default, so without
/// this the tests fight over the guest's single session.
fn guest_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::thread::sleep(SESSION_SETTLE);
    guard
}

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
    let _guest = guest_lock();
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
    let _guest = guest_lock();
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

/// A resize asked for before the session has a Display Control channel must
/// still reach the guest.
///
/// This is the exact ordering the application produces. The widget is
/// allocated its size the moment it is attached, which is well before DRDYNVC
/// negotiates the disp channel, so the first request is always early. An
/// earlier version dropped it, and the desktop then stayed at its created size
/// until the user dragged a window edge. See DESIGN.md section 16.2.
#[test]
#[ignore = "needs a running guest from 'just guest-test'"]
fn a_resize_sent_before_the_channel_is_ready_still_arrives() {
    const WANTED: (u32, u32) = (1280, 800);

    let _guest = guest_lock();
    let (command_tx, command_rx) = std::sync::mpsc::channel();
    let (event_tx, event_rx) = async_channel::bounded(1);

    // Queued before the thread starts, so it is waiting in the channel when
    // the session begins polling. This cannot arrive any earlier.
    command_tx
        .send(SessionCommand::Resize {
            width: WANTED.0,
            height: WANTED.1,
        })
        .expect("the command should queue");

    let handle = std::thread::spawn(move || run_session(params(13389), command_rx, event_tx));

    let mut last = (0, 0);
    let mut granted = false;
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        match event_rx.recv_blocking() {
            Ok(SessionEvent::Frame(frame)) => {
                last = (frame.width, frame.height);
                if last == WANTED {
                    granted = true;
                    break;
                }
            }
            Ok(SessionEvent::Disconnected { .. }) | Ok(SessionEvent::Error(_)) | Err(_) => break,
            Ok(_) => {}
        }
    }

    let _ = command_tx.send(SessionCommand::Disconnect);
    drop(event_rx);
    let _ = handle.join();

    // Put the guest back, so a later run of the other tests still sees 1920x1080.
    println!("last frame {last:?}");
    assert!(
        granted,
        "the guest never reached {WANTED:?}, the last frame was {last:?}"
    );
}
