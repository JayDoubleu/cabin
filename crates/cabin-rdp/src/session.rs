//! The message contract between the RDP session and the user interface.
//!
//! The session runs on its own thread with its own Tokio runtime. It never
//! touches GTK types. Everything crosses through these two enums, over bounded
//! channels, so a slow interface applies back pressure instead of growing a
//! queue. See DESIGN.md section 6.2.

use crate::frame::Frame;
use tokio::sync::mpsc;

/// How many frames may wait for the interface before the session blocks.
/// One is deliberate: a stale frame has no value, so the session should wait
/// rather than build a backlog.
const FRAME_QUEUE: usize = 1;
const COMMAND_QUEUE: usize = 32;

#[derive(Debug, Clone)]
pub struct Credentials {
    pub username: String,
    pub password: String,
    /// gnome-remote-desktop ignores the domain, but CredSSP needs the field.
    pub domain: String,
}

/// Sent from the interface to the session.
#[derive(Debug, Clone)]
pub enum SessionCommand {
    /// Ask the guest to change its monitor size. Sent over the Display Control
    /// channel. The interface debounces these, so the session does not.
    Resize {
        width: u32,
        height: u32,
    },
    /// A PC/AT scancode. `extended` selects the 0xE0 prefixed set, which
    /// carries the arrow keys, the Super keys and the right-hand modifiers.
    KeyPress {
        scancode: u16,
        extended: bool,
        pressed: bool,
    },
    PointerMove {
        x: u32,
        y: u32,
    },
    /// A button event carries the pointer position, because RDP has no
    /// separate notion of "where the pointer already is": every pointer PDU
    /// states its own coordinates.
    PointerButton {
        x: u32,
        y: u32,
        button: u8,
        pressed: bool,
    },
    PointerScroll {
        horizontal: i16,
        vertical: i16,
    },
    ClipboardOffer {
        text: String,
    },
    Disconnect,
}

/// Sent from the session to the interface.
#[derive(Debug)]
pub enum SessionEvent {
    Connected {
        width: u32,
        height: u32,
        /// The certificate the guest presented, to be recorded on a first
        /// connection.
        fingerprint: Option<String>,
    },
    Frame(Frame),
    /// The guest changed its own resolution, which happens after a resize
    /// request is honoured.
    Resized {
        width: u32,
        height: u32,
    },
    ClipboardOffer {
        text: String,
    },
    Disconnected {
        reason: String,
    },
    Error(String),
}

/// The interface's end of a session.
pub struct SessionHandle {
    commands: mpsc::Sender<SessionCommand>,
    events: mpsc::Receiver<SessionEvent>,
}

impl SessionHandle {
    /// Builds a connected pair. The returned halves belong to the session
    /// thread.
    pub fn channel() -> (
        Self,
        mpsc::Receiver<SessionCommand>,
        mpsc::Sender<SessionEvent>,
    ) {
        let (command_tx, command_rx) = mpsc::channel(COMMAND_QUEUE);
        let (event_tx, event_rx) = mpsc::channel(FRAME_QUEUE);
        (
            Self {
                commands: command_tx,
                events: event_rx,
            },
            command_rx,
            event_tx,
        )
    }

    /// Sends a command. Returns false when the session has already ended.
    pub fn send(&self, command: SessionCommand) -> bool {
        self.commands.try_send(command).is_ok()
    }

    pub async fn next_event(&mut self) -> Option<SessionEvent> {
        self.events.recv().await
    }
}

/// PC/AT scancodes for the keys Cabin sends itself.
///
/// RDP carries scancodes, not keysyms, so these are protocol constants rather
/// than anything platform specific.
pub mod scancode {
    /// Escape. Closes the Activities overview.
    pub const ESCAPE: u16 = 0x01;
    /// The left Super key. Toggles the Activities overview. Extended.
    pub const LEFT_SUPER: u16 = 0x5B;
}

/// Rounds a size for the Display Control channel.
///
/// Several RDP codecs need even dimensions, and the protocol has its own
/// bounds. See DESIGN.md section 6.4.
pub fn clamp_resolution(width: u32, height: u32) -> (u32, u32) {
    const MIN: u32 = 200;
    const MAX: u32 = 8192;
    let round_even = |v: u32| v & !1;
    (
        round_even(width.clamp(MIN, MAX)),
        round_even(height.clamp(MIN, MAX)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounds_odd_sizes_down_to_even() {
        assert_eq!(clamp_resolution(1921, 1081), (1920, 1080));
    }

    #[test]
    fn clamps_to_bounds() {
        assert_eq!(clamp_resolution(10, 10), (200, 200));
        assert_eq!(clamp_resolution(99_999, 99_999), (8192, 8192));
    }

    #[test]
    fn leaves_common_sizes_alone() {
        for (w, h) in [(1280, 720), (1920, 1080), (2560, 1440), (3840, 2160)] {
            assert_eq!(clamp_resolution(w, h), (w, h));
        }
    }

    #[tokio::test]
    async fn commands_reach_the_session() {
        let (handle, mut rx, _events) = SessionHandle::channel();
        assert!(handle.send(SessionCommand::Disconnect));
        assert!(matches!(rx.recv().await, Some(SessionCommand::Disconnect)));
    }

    #[tokio::test]
    async fn sending_fails_once_the_session_is_gone() {
        let (handle, rx, _events) = SessionHandle::channel();
        drop(rx);
        assert!(!handle.send(SessionCommand::Disconnect));
    }
}
