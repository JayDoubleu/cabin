//! Rust declarations for the Cabin FreeRDP shim.
//!
//! Cabin links FreeRDP rather than using a pure-Rust stack because
//! `gnome-remote-desktop` requires the MS-RDPEGFX graphics pipeline and closes
//! the connection without it. See DESIGN.md section 14.
//!
//! Every function here takes primitives or an opaque pointer, so no FreeRDP
//! struct layout is reproduced on the Rust side. The C shim owns all of that.
//! See DESIGN.md section 15 for why bindgen was rejected.

use std::os::raw::{c_char, c_int};

/// Opaque handle to a shim client. Created by [`cabin_rdp_new`].
#[repr(C)]
pub struct cabin_rdp {
    _private: [u8; 0],
}

pub const CABIN_OK: c_int = 0;
pub const CABIN_ERR_GENERIC: c_int = -1;
pub const CABIN_ERR_CONNECT: c_int = -2;
pub const CABIN_ERR_AUTH: c_int = -3;
pub const CABIN_ERR_DISCONNECTED: c_int = -4;

// Pointer button flags, from MS-RDPBCGR. Declared here rather than bound,
// because they are stable protocol constants.
pub const PTR_FLAGS_MOVE: u32 = 0x0800;
pub const PTR_FLAGS_DOWN: u32 = 0x8000;
pub const PTR_FLAGS_BUTTON1: u32 = 0x1000;
pub const PTR_FLAGS_BUTTON2: u32 = 0x2000;
pub const PTR_FLAGS_BUTTON3: u32 = 0x4000;

extern "C" {
    pub fn cabin_rdp_new() -> *mut cabin_rdp;
    pub fn cabin_rdp_free(client: *mut cabin_rdp);

    pub fn cabin_rdp_configure(
        client: *mut cabin_rdp,
        host: *const c_char,
        port: u32,
        username: *const c_char,
        password: *const c_char,
        domain: *const c_char,
        width: u32,
        height: u32,
    ) -> c_int;

    pub fn cabin_rdp_expect_fingerprint(client: *mut cabin_rdp, fingerprint: *const c_char);
    pub fn cabin_rdp_seen_fingerprint(client: *mut cabin_rdp) -> *const c_char;
    pub fn cabin_rdp_fingerprint_changed(client: *mut cabin_rdp) -> c_int;
    pub fn cabin_rdp_connect(client: *mut cabin_rdp) -> c_int;
    pub fn cabin_rdp_poll(client: *mut cabin_rdp, timeout_ms: u32) -> c_int;

    pub fn cabin_rdp_framebuffer(
        client: *mut cabin_rdp,
        width: *mut u32,
        height: *mut u32,
        stride: *mut u32,
    ) -> *const u8;

    pub fn cabin_rdp_frame_count(client: *mut cabin_rdp) -> u64;
    pub fn cabin_rdp_request_resize(client: *mut cabin_rdp, width: u32, height: u32) -> c_int;
    pub fn cabin_rdp_send_key(
        client: *mut cabin_rdp,
        scancode: u32,
        extended: c_int,
        down: c_int,
    ) -> c_int;
    pub fn cabin_rdp_send_unicode(client: *mut cabin_rdp, code: u16, down: c_int) -> c_int;
    pub fn cabin_rdp_send_pointer(client: *mut cabin_rdp, x: u32, y: u32, flags: u32) -> c_int;
    pub fn cabin_rdp_send_wheel(client: *mut cabin_rdp, delta: i32, horizontal: c_int) -> c_int;
    pub fn cabin_rdp_disconnect(client: *mut cabin_rdp);
    pub fn cabin_rdp_last_error(client: *mut cabin_rdp) -> u32;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Proves the shim compiles, links, and that FreeRDP allocates a client.
    #[test]
    fn creates_and_frees_a_client() {
        // SAFETY: the shim returns either null or a valid handle.
        let client = unsafe { cabin_rdp_new() };
        assert!(!client.is_null(), "could not create a FreeRDP client");
        // SAFETY: client came from cabin_rdp_new and is freed exactly once.
        unsafe { cabin_rdp_free(client) };
    }

    /// Freeing a null handle must be safe, because error paths rely on it.
    #[test]
    fn free_tolerates_null() {
        // SAFETY: the shim checks for null.
        unsafe { cabin_rdp_free(std::ptr::null_mut()) };
    }

    #[test]
    fn no_framebuffer_before_connecting() {
        // SAFETY: valid handle, and the shim returns null before the first frame.
        unsafe {
            let client = cabin_rdp_new();
            assert!(!client.is_null());
            let (mut w, mut h, mut s) = (0u32, 0u32, 0u32);
            let buf = cabin_rdp_framebuffer(client, &mut w, &mut h, &mut s);
            assert!(
                buf.is_null(),
                "there should be no framebuffer before connecting"
            );
            cabin_rdp_free(client);
        }
    }
}
