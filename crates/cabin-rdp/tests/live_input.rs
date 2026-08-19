//! Proves that input reaches the guest and that the frame path reports the
//! result, using Cabin's own client rather than a third party one.
//!
//! Needs a running guest:
//!     just guest-clean && just guest-test
//!     just rdp-live
//!
//! # Why the Activities overview, and not typed text
//!
//! A host session measured both against a real guest. Typed text changes about
//! 0.0013 of the frame, while toggling the overview changes 0.399. Text is
//! three orders of magnitude weaker, and it is hostage to the font, theme and
//! scaling, so it makes a brittle gate. The overview is an aggregate over the
//! whole framebuffer and survives a codec change.
//!
//! The measured noise floor of an idle guest is exactly zero: consecutive
//! frames are bit identical. The one periodic change is the top bar clock,
//! worth about twenty pixels once a minute, which is why the round-trip
//! tolerance is set well above that.

use cabin_rdp::{
    run_session, session::scancode, ConnectionParams, Credentials, Frame, SessionCommand,
    SessionEvent,
};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

/// Observed 0.399 when the overview opens. A fortieth of that is still far
/// above anything the guest does on its own.
const CHANGED: f64 = 0.05;
/// Observed 0.0000101 returning to the starting state, and the clock tick is
/// worth about 0.00001. This sits two orders of magnitude above both.
const UNCHANGED: f64 = 0.001;
/// A popover covers far less of the screen than the overview does. The
/// calendar measured 0.0123, so this keeps a sixfold margin below the signal
/// while staying two hundred times above the once-a-minute clock tick.
const POPOVER_CHANGED: f64 = 0.002;

struct Session {
    commands: Sender<SessionCommand>,
    events: async_channel::Receiver<SessionEvent>,
    latest: Option<Frame>,
}

impl Session {
    fn open(port: u16) -> Self {
        let (commands, command_rx) = std::sync::mpsc::channel();
        let (event_tx, events) = async_channel::bounded(1);
        let params = ConnectionParams {
            host: "127.0.0.1".to_string(),
            port,
            credentials: Credentials {
                username: "cabin".to_string(),
                password: "testpass".to_string(),
                domain: String::new(),
            },
            width: 1920,
            height: 1080,
            // A throwaway guest, so there is nothing recorded to check against.
            expected_fingerprint: None,
        };
        std::thread::spawn(move || run_session(params, command_rx, event_tx));
        Self {
            commands,
            events,
            latest: None,
        }
    }

    /// Drains frames until none arrives for `quiet`, then returns the last one.
    ///
    /// The frame channel holds a single item, so the session blocks until this
    /// drains it. Waiting for quiet also lets an animation finish before the
    /// frame is compared.
    fn settle(&mut self, quiet: Duration) -> Frame {
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut last_change = Instant::now();

        while Instant::now() < deadline {
            match self.events.try_recv() {
                Ok(SessionEvent::Frame(frame)) => {
                    self.latest = Some(frame);
                    last_change = Instant::now();
                }
                Ok(SessionEvent::Error(message)) => panic!("session failed: {message}"),
                Ok(SessionEvent::Disconnected { reason }) => panic!("session ended: {reason}"),
                Ok(_) => {}
                Err(_) => {
                    if self.latest.is_some() && last_change.elapsed() >= quiet {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        }
        self.latest.clone().expect("no frame arrived")
    }

    fn move_to(&self, x: u32, y: u32) {
        self.commands
            .send(SessionCommand::PointerMove { x, y })
            .expect("the session should accept a move");
    }

    fn click(&self, x: u32, y: u32, button: u8) {
        // Move first. Some targets only react once the pointer is over them.
        self.commands
            .send(SessionCommand::PointerMove { x, y })
            .expect("the session should accept a move");
        for pressed in [true, false] {
            self.commands
                .send(SessionCommand::PointerButton {
                    x,
                    y,
                    button,
                    pressed,
                })
                .expect("the session should accept a button");
        }
    }

    fn tap(&self, scancode: u16, extended: bool) {
        for pressed in [true, false] {
            self.commands
                .send(SessionCommand::KeyPress {
                    scancode,
                    extended,
                    pressed,
                })
                .expect("the session should accept a key");
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.commands.send(SessionCommand::Disconnect);
    }
}

/// The fraction of pixels that differ noticeably between two frames.
///
/// Compares luminance rather than raw bytes, so a codec that shifts a channel
/// by one does not register as a change.
fn changed_fraction(a: &Frame, b: &Frame) -> f64 {
    assert_eq!((a.width, a.height), (b.width, b.height), "size changed");
    let luma = |p: &[u8]| 0.114 * p[0] as f64 + 0.587 * p[1] as f64 + 0.299 * p[2] as f64;
    let differing = a
        .data
        .chunks_exact(4)
        .zip(b.data.chunks_exact(4))
        .filter(|(x, y)| (luma(x) - luma(y)).abs() > 12.75) // 5% of full scale
        .count();
    differing as f64 / (a.width * a.height) as f64
}

#[test]
#[ignore = "needs a running guest from 'just guest-test'"]
fn super_key_toggles_the_overview_and_returns() {
    let mut session = Session::open(13389);
    // The overview close animation runs for over a second. At 700 ms the frame
    // is captured mid-animation and the round trip reads about 0.025 rather
    // than 0. That looks exactly like a real failure, so do not shorten this.
    let quiet = Duration::from_millis(1500);

    let start = session.settle(quiet);
    println!("baseline {}x{}", start.width, start.height);

    // Negative control. Escape with the overview already closed must do
    // nothing. If this registers a change, something unrelated is repainting
    // and the rest of the test would be meaningless.
    session.tap(scancode::ESCAPE, false);
    let after_escape = session.settle(quiet);
    let idle = changed_fraction(&start, &after_escape);
    println!("negative control, escape as a no-op: {idle:.6}");
    assert!(
        idle < UNCHANGED,
        "an inert keystroke changed {idle:.6} of the frame, so the guest is not quiescent"
    );

    // Open the overview.
    session.tap(scancode::LEFT_SUPER, true);
    let opened = session.settle(quiet);
    let opening = changed_fraction(&after_escape, &opened);
    println!("overview opened: {opening:.6}");
    assert!(
        opening > CHANGED,
        "the overview did not open, so input is not reaching the guest ({opening:.6})"
    );

    // Close it again.
    session.tap(scancode::LEFT_SUPER, true);
    let closed = session.settle(quiet);
    let closing = changed_fraction(&opened, &closed);
    println!("overview closed: {closing:.6}");
    assert!(
        closing > CHANGED,
        "the overview did not close ({closing:.6})"
    );

    // The round trip is the real assertion. It proves the guest tracked a
    // state change both ways, rather than repainting once for some other
    // reason.
    let round_trip = changed_fraction(&after_escape, &closed);
    println!("round trip back to the start: {round_trip:.6}");
    assert!(
        round_trip < UNCHANGED,
        "the guest did not return to its starting state ({round_trip:.6})"
    );
}

/// Proves a click reaches the guest at the coordinates it was sent to.
///
/// The target is the top bar clock, which sits at the centre of the top edge
/// and opens the calendar popover. That makes the test a position check as
/// well as a button check: a click sent to the wrong place, for instance the
/// (0, 0) that an earlier version of the code always sent, changes nothing.
#[test]
#[ignore = "needs a running guest from 'just guest-test'"]
fn clicking_the_clock_opens_the_calendar() {
    let mut session = Session::open(13389);
    let quiet = Duration::from_millis(1500);

    // A freshly booted guest sits in the Activities overview, so close it
    // first. Without this the click lands on the overview instead of the top
    // bar and the test measures the wrong thing entirely.
    session.tap(scancode::ESCAPE, false);
    let _ = session.settle(quiet);

    // Park the pointer somewhere inert. The top bar highlights whatever the
    // pointer is over, so a baseline taken with the pointer on the clock would
    // differ from the final frame by that highlight alone.
    session.move_to(40, 400);
    let start = session.settle(quiet);
    let (width, _) = (start.width, start.height);

    // The clock is centred horizontally and sits inside the top bar.
    session.click(width / 2, 14, 1);
    let opened = session.settle(quiet);
    let opening = changed_fraction(&start, &opened);
    println!("calendar opened: {opening:.6}");
    assert!(
        opening > POPOVER_CHANGED,
        "clicking the clock changed {opening:.6}, so the click did not land"
    );

    // Click the clock again to close it. Using a second click rather than
    // Escape keeps this a pure pointer test, and it exercises the toggle in
    // both directions.
    session.click(width / 2, 14, 1);
    // Move off the clock again, so the comparison is not defeated by the
    // hover highlight the pointer leaves behind.
    session.move_to(40, 400);
    let closed = session.settle(quiet);
    let round_trip = changed_fraction(&start, &closed);
    println!("round trip after a second click: {round_trip:.6}");
    assert!(
        round_trip < UNCHANGED,
        "the calendar did not close cleanly ({round_trip:.6})"
    );
}
