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
    /// another thread by reference.
    fn framebuffer(&self) -> Option<Frame> {
        let (mut width, mut height, mut stride) = (0u32, 0u32, 0u32);
        // SAFETY: valid handle and out pointers. Returns null before the first
        // frame, which is checked.
        let data =
            unsafe { sys::cabin_rdp_framebuffer(self.0, &mut width, &mut height, &mut stride) };
        if data.is_null() || width == 0 || height == 0 {
            return None;
        }

        // SAFETY: the shim guarantees stride * height readable bytes.
        let raw = unsafe { std::slice::from_raw_parts(data, (stride * height) as usize) };

        // Drop any row padding, so the Frame is exactly width * 4 per row and
        // GdkTexture can take it without a stride argument.
        let row_bytes = width as usize * 4;
        let mut packed = Vec::with_capacity(row_bytes * height as usize);
        for row in 0..height as usize {
            let start = row * stride as usize;
            packed.extend_from_slice(&raw[start..start + row_bytes]);
        }

        Some(Frame {
            width,
            height,
            format: PixelFormat::Bgra8888,
            data: packed,
            damage: Vec::new(),
        })
    }

    fn request_resize(&self, width: u32, height: u32) -> Result<()> {
        // SAFETY: valid handle.
        let rc = unsafe { sys::cabin_rdp_request_resize(self.0, width, height) };
        if rc != sys::CABIN_OK {
            return Err(Error::Closed(format!("resize refused, code {rc}")));
        }
        Ok(())
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

    fn send_wheel(&self, delta: i16, horizontal: bool) {
        // SAFETY: valid handle.
        unsafe { sys::cabin_rdp_send_wheel(self.0, delta as i32, horizontal as i32) };
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
    let client = match start(&params) {
        Ok(client) => client,
        Err(e) => {
            let _ = events.send_blocking(SessionEvent::Error(e.to_string()));
            return;
        }
    };

    let _ = events.send_blocking(SessionEvent::Connected {
        width: params.width,
        height: params.height,
        fingerprint: client.seen_fingerprint(),
    });

    let mut last_frame = 0u64;
    loop {
        while let Ok(command) = commands.try_recv() {
            if !apply(&client, command) {
                client.disconnect();
                let _ = events.send_blocking(SessionEvent::Disconnected {
                    reason: "closed by the application".into(),
                });
                return;
            }
        }

        match client.poll(POLL_INTERVAL) {
            Ok(true) => {}
            Ok(false) => {
                let _ = events.send_blocking(SessionEvent::Disconnected {
                    reason: "the guest ended the session".into(),
                });
                return;
            }
            Err(e) => {
                let _ = events.send_blocking(SessionEvent::Error(e.to_string()));
                return;
            }
        }

        let count = client.frame_count();
        if count == last_frame {
            continue;
        }
        last_frame = count;

        if let Some(frame) = client.framebuffer() {
            // send_blocking applies back pressure: a slow interface makes the
            // session wait rather than build a queue of stale frames.
            if events.send_blocking(SessionEvent::Frame(frame)).is_err() {
                client.disconnect();
                return;
            }
        }
    }
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
fn apply(client: &Client, command: SessionCommand) -> bool {
    match command {
        SessionCommand::Disconnect => return false,
        SessionCommand::Resize { width, height } => {
            let (width, height) = crate::session::clamp_resolution(width, height);
            if let Err(e) = client.request_resize(width, height) {
                tracing::debug!("resize refused: {e}");
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
