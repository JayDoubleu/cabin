//! A safe wrapper over the FreeRDP shim, and the session thread.
//!
//! FreeRDP is not thread safe, so every call into the shim happens on one
//! owned thread. The interface talks to that thread through channels, and
//! never touches a FreeRDP pointer itself.

use crate::frame::{Frame, PixelFormat};
use crate::session::{Credentials, SessionCommand, SessionEvent};
use crate::{Error, Result};
use cabin_freerdp_sys as sys;
use std::ffi::CString;
use std::time::Duration;

/// Where to connect, and as whom.
#[derive(Debug, Clone)]
pub struct ConnectionParams {
    pub host: String,
    pub port: u16,
    pub credentials: Credentials,
    pub width: u32,
    pub height: u32,
    /// The fingerprint recorded from an earlier connection, if any. `None`
    /// accepts and records whatever the guest presents.
    pub expected_fingerprint: Option<String>,
}

/// Owns a shim client and frees it on drop.
struct Client(*mut sys::cabin_rdp);

// The pointer is only ever used from the session thread that creates it. The
// thread owns the Client for its whole life, so moving it in at spawn time is
// the only cross-thread action.
unsafe impl Send for Client {}

impl Client {
    fn new() -> Result<Self> {
        // SAFETY: returns null on failure, checked here.
        let raw = unsafe { sys::cabin_rdp_new() };
        if raw.is_null() {
            return Err(Error::Closed("could not create a FreeRDP client".into()));
        }
        Ok(Self(raw))
    }

    fn configure(&self, params: &ConnectionParams) -> Result<()> {
        let host = cstring(&params.host)?;
        let user = cstring(&params.credentials.username)?;
        let password = cstring(&params.credentials.password)?;
        let domain = cstring(&params.credentials.domain)?;

        // SAFETY: every pointer is a live CString for the duration of the call.
        let rc = unsafe {
            sys::cabin_rdp_configure(
                self.0,
                host.as_ptr(),
                params.port as u32,
                user.as_ptr(),
                password.as_ptr(),
                domain.as_ptr(),
                params.width,
                params.height,
            )
        };
        if rc != sys::CABIN_OK {
            return Err(Error::Closed(format!(
                "could not apply settings, code {rc}"
            )));
        }
        Ok(())
    }

    fn expect_fingerprint(&self, fingerprint: Option<&str>) -> Result<()> {
        match fingerprint {
            Some(value) => {
                let value = cstring(value)?;
                // SAFETY: valid handle and a live CString for the call.
                unsafe { sys::cabin_rdp_expect_fingerprint(self.0, value.as_ptr()) };
            }
            // SAFETY: null means "no expectation yet", which the shim handles.
            None => unsafe { sys::cabin_rdp_expect_fingerprint(self.0, std::ptr::null()) },
        }
        Ok(())
    }

    fn seen_fingerprint(&self) -> Option<String> {
        // SAFETY: valid handle; the shim owns the returned string.
        let raw = unsafe { sys::cabin_rdp_seen_fingerprint(self.0) };
        if raw.is_null() {
            return None;
        }
        // SAFETY: the shim returns a NUL terminated string it owns.
        Some(
            unsafe { std::ffi::CStr::from_ptr(raw) }
                .to_string_lossy()
                .into_owned(),
        )
    }

    fn fingerprint_changed(&self) -> bool {
        // SAFETY: valid handle.
        (unsafe { sys::cabin_rdp_fingerprint_changed(self.0) }) == 1
    }

    fn connect(&self, address: &str) -> Result<()> {
        // SAFETY: valid handle.
        let rc = unsafe { sys::cabin_rdp_connect(self.0) };
        match rc {
            sys::CABIN_OK => Ok(()),
            sys::CABIN_ERR_AUTH => Err(Error::AuthenticationFailed),
            _ if self.fingerprint_changed() => Err(Error::CertificateChanged {
                seen: self.seen_fingerprint().unwrap_or_default(),
            }),
            _ => {
                // SAFETY: valid handle.
                let code = unsafe { sys::cabin_rdp_last_error(self.0) };
                Err(Error::Connect {
                    address: address.to_string(),
                    source: std::io::Error::other(format!("FreeRDP error {code:#010x}")),
                })
            }
        }
    }

    /// Returns false once the session has ended.
    fn poll(&self, timeout: Duration) -> Result<bool> {
        // SAFETY: valid handle.
        let rc = unsafe { sys::cabin_rdp_poll(self.0, timeout.as_millis() as u32) };
        match rc {
            1 => Ok(true),
            0 => Ok(false),
            other => Err(Error::Closed(format!("the session failed, code {other}"))),
        }
    }

    fn frame_count(&self) -> u64 {
        // SAFETY: valid handle.
        unsafe { sys::cabin_rdp_frame_count(self.0) }
    }

    /// Copies the current framebuffer.
    ///
    /// The copy is deliberate. The shim's buffer belongs to FreeRDP and is
    /// reused and reallocated as the session runs, so it cannot be handed to
    /// another thread by reference. The shim does the copy under FreeRDP's
    /// update lock and never hands out a pointer into that buffer.
    fn framebuffer(&self) -> Option<Frame> {
        let (mut width, mut height, mut stride) = (0u32, 0u32, 0u32);
        // SAFETY: valid handle and out pointers.
        let rc = unsafe {
            sys::cabin_rdp_framebuffer_info(self.0, &mut width, &mut height, &mut stride)
        };
        if rc != sys::CABIN_OK || width == 0 || height == 0 {
            return None;
        }

        // Check the description before trusting it.
        //
        // A guest that went from 1920 to 1152 wide once reported width 1152
        // with the old stride of 7680 still in place, which describes a
        // buffer half again as large as the one that exists. Copying on that
        // description read past the end of the allocation and aborted the
        // process. The cause was the drdynvc thread resizing the buffer while
        // this thread read it, and synchronous dynamic channels remove that
        // cause. The check stays, because a wrong description must never
        // reach a copy.
        let row_bytes = width as usize * 4;
        if !stride_matches(stride as usize, row_bytes) {
            tracing::debug!("skipping a frame: {width}x{height} cannot have a stride of {stride}");
            return None;
        }
        let Some(total) = row_bytes.checked_mul(height as usize) else {
            tracing::warn!("framebuffer size overflows: {width}x{height}");
            return None;
        };

        // The Frame is exactly width * 4 per row, so GdkTexture can take it
        // without a stride argument.
        let mut data = vec![0u8; total];
        // SAFETY: valid handle, and `data` holds the `total` bytes passed.
        let rc = unsafe {
            sys::cabin_rdp_copy_framebuffer(
                self.0,
                data.as_mut_ptr(),
                data.len(),
                width,
                height,
                stride,
            )
        };
        if rc != sys::CABIN_OK {
            // The framebuffer changed after it was described. The next frame
            // carries the new description.
            tracing::debug!("skipping a frame: the framebuffer changed during the copy");
            return None;
        }

        Some(Frame {
            width,
            height,
            format: PixelFormat::Bgra8888,
            data,
            damage: Vec::new(),
        })
    }

    /// True once the Display Control channel has attached.
    fn display_ready(&self) -> bool {
        // SAFETY: valid handle.
        unsafe { sys::cabin_rdp_display_ready(self.0) == 1 }
    }

    fn request_resize(&self, width: u32, height: u32) -> ResizeOutcome {
        // SAFETY: valid handle.
        let rc = unsafe { sys::cabin_rdp_request_resize(self.0, width, height) };
        match rc {
            sys::CABIN_OK => ResizeOutcome::Sent,
            sys::CABIN_ERR_NOT_READY => ResizeOutcome::NotReady,
            _ => ResizeOutcome::Refused,
        }
    }

    fn send_pointer(&self, x: u32, y: u32, flags: u32) {
        // SAFETY: valid handle.
        unsafe { sys::cabin_rdp_send_pointer(self.0, x, y, flags) };
    }

    fn send_key(&self, scancode: u16, extended: bool, pressed: bool) {
        // SAFETY: valid handle.
        unsafe {
            sys::cabin_rdp_send_key(self.0, scancode as u32, extended as i32, pressed as i32)
        };
    }

    /// Sends a wheel rotation, split into as many events as it needs.
    fn send_wheel(&self, delta: i32, horizontal: bool) {
        let axis = if horizontal {
            sys::PTR_FLAGS_HWHEEL
        } else {
            sys::PTR_FLAGS_WHEEL
        };
        for step in crate::session::wheel::encode(delta) {
            // SAFETY: valid handle. A wheel event carries no position.
            unsafe { sys::cabin_rdp_send_pointer(self.0, 0, 0, axis | step) };
        }
    }

    fn disconnect(&self) {
        // SAFETY: valid handle.
        unsafe { sys::cabin_rdp_disconnect(self.0) };
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        // SAFETY: the handle came from cabin_rdp_new and is freed once.
        unsafe { sys::cabin_rdp_free(self.0) };
    }
}

fn cstring(value: &str) -> Result<CString> {
    CString::new(value).map_err(|_| Error::Closed("a setting contained a NUL byte".into()))
}

/// What came of asking the guest to change its resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResizeOutcome {
    Sent,
    /// The Display Control channel is not attached yet.
    NotReady,
    Refused,
}

/// How long a poll waits for network activity. Short enough that queued
/// commands are acted on promptly, long enough not to spin.
const POLL_INTERVAL: Duration = Duration::from_millis(16);

/// Runs a session until it ends. Intended to be the body of a dedicated thread.
///
/// Returns when the guest disconnects, when a `Disconnect` command arrives, or
/// when the event receiver is dropped.
pub fn run_session(
    params: ConnectionParams,
    commands: std::sync::mpsc::Receiver<SessionCommand>,
    events: async_channel::Sender<SessionEvent>,
) {
    let began = std::time::Instant::now();
    let deadline = began + CONNECT_WINDOW;
    // Commands that arrived before there was a session to give them to.
    let mut held: Vec<SessionCommand> = Vec::new();
    // The size the interface last asked for. It is re-applied to every new
    // connection, because a request that went to an attempt that then died
    // would otherwise be lost, and the interface does not repeat itself.
    let mut desired_size: Option<(u32, u32)> = None;

    for attempt in 1u32.. {
        let client = match start(&params) {
            Ok(client) => client,
            Err(Error::CertificateChanged { seen }) => {
                // A typed event, so the interface can offer to trust the new
                // certificate without matching on the error text.
                let _ = events.send_blocking(SessionEvent::CertificateChanged { seen });
                return;
            }
            Err(e) => {
                if !is_transient(&e) || std::time::Instant::now() >= deadline {
                    report_giving_up(&events, attempt, began, &e.to_string());
                    return;
                }
                if !keep_waiting(&events, &commands, attempt, began, &mut held) {
                    return;
                }
                continue;
            }
        };

        match run_connected(
            &client,
            &params,
            &commands,
            &events,
            &mut held,
            &mut desired_size,
        ) {
            // Frames flowed. The session ran and has reported its own ending.
            Outcome::Ran => return,
            Outcome::Cancelled => return,
            // Connected, then dropped before drawing anything. A desktop that
            // is still starting does this: the RDP port is open while the
            // GNOME session behind it is not ready. Treat it as not up yet.
            Outcome::EndedEarly(reason) => {
                if std::time::Instant::now() >= deadline {
                    report_giving_up(&events, attempt, began, &reason);
                    return;
                }
                tracing::debug!("attempt {attempt} ended before any frame: {reason}");
                if !keep_waiting(&events, &commands, attempt, began, &mut held) {
                    return;
                }
            }
        }
    }
}

/// Tells the interface the attempt is over, and why.
fn report_giving_up(
    events: &async_channel::Sender<SessionEvent>,
    attempt: u32,
    began: std::time::Instant,
    reason: &str,
) {
    if attempt > 1 {
        tracing::warn!(
            "gave up after {attempt} attempts in {:?}: {reason}",
            began.elapsed()
        );
    }
    let _ = events.send_blocking(SessionEvent::Error(reason.to_string()));
}

/// Reports that the session is still trying, then waits before the next go.
///
/// Returns false when the session should stop, either because the interface
/// asked it to or because the interface has gone.
fn keep_waiting(
    events: &async_channel::Sender<SessionEvent>,
    commands: &std::sync::mpsc::Receiver<SessionCommand>,
    attempt: u32,
    began: std::time::Instant,
    held: &mut Vec<SessionCommand>,
) -> bool {
    if events
        .send_blocking(SessionEvent::Connecting {
            attempt,
            waited: began.elapsed().as_secs(),
        })
        .is_err()
    {
        return false;
    }
    // The user may switch desktops or close the window while this waits, so
    // the interval is spent watching for that rather than sleeping through it.
    !wait_or_cancel(commands, RETRY_INTERVAL, held)
}

/// What became of one connected session.
enum Outcome {
    /// Frames were delivered. A terminal event has already been sent.
    Ran,
    /// The interface asked the session to stop, or went away.
    Cancelled,
    /// The session ended before any frame arrived.
    EndedEarly(String),
}

/// Runs one connected session until it ends.
fn run_connected(
    client: &Client,
    params: &ConnectionParams,
    commands: &std::sync::mpsc::Receiver<SessionCommand>,
    events: &async_channel::Sender<SessionEvent>,
    held: &mut Vec<SessionCommand>,
    desired_size: &mut Option<(u32, u32)>,
) -> Outcome {
    let _ = events.send_blocking(SessionEvent::Connected {
        width: params.width,
        height: params.height,
        fingerprint: client.seen_fingerprint(),
    });

    let mut last_frame = 0u64;
    let mut drew_anything = false;
    // The size asked for before the Display Control channel was attached.
    // Without this the first request is lost: the widget is allocated its size
    // well before DRDYNVC negotiates disp, so the opening resize arrives too
    // early, and the interface will not repeat a request it believes it
    // already sent. The desktop then stays at its created size until the user
    // drags a window edge. See DESIGN.md section 16.2.
    let mut pending_resize: Option<(u32, u32)> = None;

    // Whatever arrived while the guest was still starting, then the size the
    // interface wants. Both go through the same path as any later command, so
    // a resize is held again if the channel is not attached yet.
    for command in held.drain(..) {
        remember_size(&command, desired_size);
        if !apply(client, command, &mut pending_resize) {
            client.disconnect();
            return finish(events, drew_anything, "closed by the application");
        }
    }
    if let Some((width, height)) = *desired_size {
        pending_resize = Some((width, height));
    }

    loop {
        while let Ok(command) = commands.try_recv() {
            remember_size(&command, desired_size);
            if !apply(client, command, &mut pending_resize) {
                client.disconnect();
                return finish(events, drew_anything, "closed by the application");
            }
        }

        if let Some((width, height)) = pending_resize {
            if client.display_ready() {
                pending_resize = None;
                match client.request_resize(width, height) {
                    ResizeOutcome::Sent => {
                        tracing::debug!("sent the held resize to {width}x{height}")
                    }
                    _ => tracing::debug!("the held resize to {width}x{height} was refused"),
                }
            }
        }

        match client.poll(POLL_INTERVAL) {
            Ok(true) => {}
            Ok(false) => return finish(events, drew_anything, "the guest ended the session"),
            Err(e) => {
                if !drew_anything {
                    return Outcome::EndedEarly(e.to_string());
                }
                let _ = events.send_blocking(SessionEvent::Error(e.to_string()));
                return Outcome::Ran;
            }
        }

        let count = client.frame_count();
        if count == last_frame {
            continue;
        }
        last_frame = count;

        if let Some(frame) = client.framebuffer() {
            drew_anything = true;
            // send_blocking applies back pressure: a slow interface makes the
            // session wait rather than build a queue of stale frames.
            if events.send_blocking(SessionEvent::Frame(frame)).is_err() {
                client.disconnect();
                return Outcome::Cancelled;
            }
        }
    }
}

/// Ends a session, reporting it only when it had something to show.
///
/// A session that never drew is reported as not yet up, so the caller can try
/// again instead of putting a failure in front of the user.
///
/// This does not call `disconnect` itself. The guest has already gone in every
/// case that reaches here, and the shim disconnects when the client is freed.
/// That matters, because FreeRDP's context free does not run PostDisconnect or
/// close the channels, so a session that was only freed leaked its GDI state
/// and its channel plugins. The shim records whether a disconnect is still
/// owed, so a session that `disconnect` already ended is not ended twice.
fn finish(
    events: &async_channel::Sender<SessionEvent>,
    drew_anything: bool,
    reason: &str,
) -> Outcome {
    if !drew_anything {
        return Outcome::EndedEarly(reason.to_string());
    }
    let _ = events.send_blocking(SessionEvent::Disconnected {
        reason: reason.to_string(),
    });
    Outcome::Ran
}

/// True when a stride really belongs to a row of `row_bytes`.
///
/// FreeRDP's GDI does not pad rows: `gdi_init` with no stride gives a primary
/// bitmap of exactly width * 4 bytes per row. So any other stride is one left
/// over from a different size. An earlier check allowed 64 bytes of padding,
/// and that still accepted a stale stride after a shrink of 16 pixels or
/// less, which read past the end of the new buffer.
fn stride_matches(stride: usize, row_bytes: usize) -> bool {
    stride == row_bytes
}

/// Keeps track of the size the interface wants, so a new attempt can restate
/// it.
fn remember_size(command: &SessionCommand, desired: &mut Option<(u32, u32)>) {
    if let SessionCommand::Resize { width, height } = command {
        *desired = Some(crate::session::clamp_resolution(*width, *height));
    }
}

/// How long to keep trying to reach a desktop that has just started.
///
/// A cold start binds the port about two and a half seconds in on the
/// development host. The window is far wider than that, because a loaded host
/// or a large image is slower, and waiting is cheaper than making the user
/// clear an error and press Reconnect.
const CONNECT_WINDOW: Duration = Duration::from_secs(90);

/// How long to wait between attempts.
const RETRY_INTERVAL: Duration = Duration::from_millis(1000);

/// Waits for `interval`, returning true if the session was told to stop.
///
/// Commands that are not `Disconnect` are put in `held` for the session to
/// apply once it is connected.
fn wait_or_cancel(
    commands: &std::sync::mpsc::Receiver<SessionCommand>,
    interval: Duration,
    held: &mut Vec<SessionCommand>,
) -> bool {
    use std::sync::mpsc::RecvTimeoutError;
    let deadline = std::time::Instant::now() + interval;
    loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return false;
        }
        match commands.recv_timeout(left) {
            Ok(SessionCommand::Disconnect) => return true,
            Ok(other) => held.push(other),
            Err(RecvTimeoutError::Timeout) => return false,
            Err(RecvTimeoutError::Disconnected) => return true,
        }
    }
}

/// True when the failure is one that starting up explains.
///
/// Only the connection itself is retried. A rejected password is not retried,
/// because the guest cannot report one by accident: `cabin-rdp.service`
/// configures the credentials and only then starts
/// `gnome-remote-desktop-headless.service`, so nothing is listening until the
/// password is in place. A changed certificate is never retried either, since
/// that is a decision for the user rather than a fault to wait out.
fn is_transient(error: &Error) -> bool {
    matches!(error, Error::Connect { .. })
}

fn start(params: &ConnectionParams) -> Result<Client> {
    let client = Client::new()?;
    client.configure(params)?;
    client.expect_fingerprint(params.expected_fingerprint.as_deref())?;
    client.connect(&format!("{}:{}", params.host, params.port))?;
    Ok(client)
}

/// Maps a GTK button number to an RDP button flag.
///
/// The two disagree, and only on the middle and right buttons:
///
/// | Button | GTK | RDP flag |
/// |---|---|---|
/// | left | 1 | `PTR_FLAGS_BUTTON1` |
/// | middle | 2 | `PTR_FLAGS_BUTTON3` |
/// | right | 3 | `PTR_FLAGS_BUTTON2` |
///
/// Because the left button agrees, a smoke test that only left-clicks passes
/// while paste-on-middle-click opens a context menu. The arms are written out
/// rather than computed, since the flag values are not a straight swap either.
fn button_flag(button: u8) -> Option<u32> {
    match button {
        1 => Some(sys::PTR_FLAGS_BUTTON1),
        2 => Some(sys::PTR_FLAGS_BUTTON3),
        3 => Some(sys::PTR_FLAGS_BUTTON2),
        _ => None,
    }
}

/// Applies one command. Returns false when the session should end.
///
/// A resize that cannot be sent yet is written to `pending_resize` rather than
/// dropped. See `run_session` for why that matters.
fn apply(
    client: &Client,
    command: SessionCommand,
    pending_resize: &mut Option<(u32, u32)>,
) -> bool {
    match command {
        SessionCommand::Disconnect => return false,
        SessionCommand::Resize { width, height } => {
            let size = crate::session::clamp_resolution(width, height);
            match client.request_resize(size.0, size.1) {
                ResizeOutcome::Sent => *pending_resize = None,
                ResizeOutcome::NotReady => *pending_resize = Some(size),
                ResizeOutcome::Refused => {
                    tracing::debug!("the guest refused a resize to {}x{}", size.0, size.1)
                }
            }
        }
        SessionCommand::PointerMove { x, y } => {
            client.send_pointer(x, y, sys::PTR_FLAGS_MOVE);
        }
        SessionCommand::PointerButton {
            x,
            y,
            button,
            pressed,
        } => {
            let Some(button_flag) = button_flag(button) else {
                return true;
            };
            let flags = button_flag | if pressed { sys::PTR_FLAGS_DOWN } else { 0 };
            // The coordinates matter. An earlier version sent (0, 0) here, so
            // every click landed in the top left corner regardless of where
            // the user clicked.
            client.send_pointer(x, y, flags);
        }
        SessionCommand::PointerScroll {
            horizontal,
            vertical,
        } => {
            if vertical != 0 {
                client.send_wheel(vertical, false);
            }
            if horizontal != 0 {
                client.send_wheel(horizontal, true);
            }
        }
        SessionCommand::KeyPress {
            scancode,
            extended,
            pressed,
        } => {
            client.send_key(scancode, extended, pressed);
        }
        SessionCommand::ClipboardOffer { .. } => {
            // Deliberately ignored. Advertising the clipboard crashes the guest
            // daemon. See DESIGN.md section 15.2.
        }
    }
    true
}

#[cfg(test)]
mod stride_tests {
    use super::stride_matches;

    #[test]
    fn an_exact_stride_is_accepted() {
        assert!(stride_matches(4608, 4608));
    }

    #[test]
    fn a_stride_left_over_from_a_wider_frame_is_rejected() {
        // The case that aborted the process: 1152 wide, still striding for
        // 1920. Reading on that description runs off the end of the buffer.
        assert!(!stride_matches(1920 * 4, 1152 * 4));
    }

    #[test]
    fn a_stride_left_over_from_a_slightly_wider_frame_is_rejected() {
        // A shrink of two pixels leaves a stride 8 bytes too long. That reads
        // 8 bytes past the end for every row but the last.
        assert!(!stride_matches(1922 * 4, 1920 * 4));
    }

    #[test]
    fn a_stride_shorter_than_the_row_is_rejected() {
        assert!(!stride_matches(4000, 4608));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_a_nul_byte_in_a_setting() {
        assert!(cstring("we\0ird").is_err());
        assert!(cstring("fine").is_ok());
    }

    /// The flag values, copied from freerdp/input.h with its own comments:
    ///   PTR_FLAGS_BUTTON1 0x1000 /* left */
    ///   PTR_FLAGS_BUTTON2 0x2000 /* right */
    ///   PTR_FLAGS_BUTTON3 0x4000 /* middle */
    #[test]
    fn button_flags_match_the_protocol() {
        assert_eq!(sys::PTR_FLAGS_BUTTON1, 0x1000, "BUTTON1 is left");
        assert_eq!(sys::PTR_FLAGS_BUTTON2, 0x2000, "BUTTON2 is right");
        assert_eq!(sys::PTR_FLAGS_BUTTON3, 0x4000, "BUTTON3 is middle");
    }

    #[test]
    fn left_click_maps_straight_through() {
        assert_eq!(button_flag(1), Some(sys::PTR_FLAGS_BUTTON1));
    }

    #[test]
    fn middle_and_right_are_transposed() {
        // This is the whole point of the function. GTK middle is 2, and RDP
        // middle is BUTTON3. Getting it wrong sends paste to the context menu.
        assert_eq!(
            button_flag(2),
            Some(sys::PTR_FLAGS_BUTTON3),
            "GTK 2 is middle"
        );
        assert_eq!(
            button_flag(3),
            Some(sys::PTR_FLAGS_BUTTON2),
            "GTK 3 is right"
        );
    }

    #[test]
    fn mapping_is_not_a_shift_of_the_button_number() {
        // 1 -> 0x1000, 2 -> 0x4000, 3 -> 0x2000. Any arithmetic scheme that
        // looks plausible gets middle and right the wrong way round.
        let naive = |b: u8| 0x1000u32 << (b - 1);
        assert_eq!(button_flag(1), Some(naive(1)));
        assert_ne!(
            button_flag(2),
            Some(naive(2)),
            "arithmetic would be wrong here"
        );
        assert_ne!(
            button_flag(3),
            Some(naive(3)),
            "arithmetic would be wrong here"
        );
    }

    #[test]
    fn unknown_buttons_are_dropped() {
        assert_eq!(button_flag(0), None);
        assert_eq!(button_flag(8), None);
    }
}
