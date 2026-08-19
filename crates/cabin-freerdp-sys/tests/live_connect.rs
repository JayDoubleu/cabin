//! Connects to a real Cabin guest and proves pixels arrive.
//!
//! Start a guest first:
//!     just guest-clean && just guest-test
//!
//! Then:
//!     just rdp-live
use cabin_freerdp_sys::*;
use std::ffi::CString;

struct Client(*mut cabin_rdp);

impl Client {
    fn new() -> Self {
        // SAFETY: returns null or a valid handle, checked below.
        let raw = unsafe { cabin_rdp_new() };
        assert!(!raw.is_null(), "could not create a FreeRDP client");
        Self(raw)
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        // SAFETY: the handle came from cabin_rdp_new and is freed once.
        unsafe { cabin_rdp_free(self.0) };
    }
}

fn configure(client: &Client, password: &str) -> i32 {
    let host = CString::new("127.0.0.1").unwrap();
    let user = CString::new("cabin").unwrap();
    let pass = CString::new(password).unwrap();
    let domain = CString::new("").unwrap();
    // SAFETY: every pointer is a live CString for the duration of the call.
    unsafe {
        cabin_rdp_configure(
            client.0,
            host.as_ptr(),
            13389,
            user.as_ptr(),
            pass.as_ptr(),
            domain.as_ptr(),
            1920,
            1080,
        )
    }
}

#[test]
#[ignore = "needs a running guest from 'just guest-test'"]
#[ignore = "needs a running guest from 'just guest-test'"]
fn connects_and_receives_frames() {
    let client = Client::new();
    assert_eq!(configure(&client, "testpass"), CABIN_OK, "configure failed");

    // SAFETY: valid handle.
    let rc = unsafe { cabin_rdp_connect(client.0) };
    assert_eq!(
        rc,
        CABIN_OK,
        "connect failed with {rc}, FreeRDP error {:#010x}",
        unsafe { cabin_rdp_last_error(client.0) }
    );
    println!("connected");

    // Pump until the first frame arrives. The guest paints promptly once the
    // session is up, so a few seconds is generous.
    let mut frames = 0u64;
    for _ in 0..300 {
        // SAFETY: valid handle.
        let alive = unsafe { cabin_rdp_poll(client.0, 100) };
        assert!(alive >= 0, "poll reported error {alive}");
        if alive == 0 {
            break;
        }
        // SAFETY: valid handle.
        frames = unsafe { cabin_rdp_frame_count(client.0) };
        if frames > 0 {
            break;
        }
    }
    assert!(frames > 0, "no frame arrived");

    let (mut w, mut h, mut stride) = (0u32, 0u32, 0u32);
    // SAFETY: valid handle and out pointers.
    let buf = unsafe { cabin_rdp_framebuffer(client.0, &mut w, &mut h, &mut stride) };
    assert!(!buf.is_null(), "no framebuffer after {frames} frames");
    println!("frames={frames} size={w}x{h} stride={stride}");
    assert_eq!(w, 1920);
    assert_eq!(h, 1080);

    // SAFETY: the shim guarantees stride * height bytes are readable.
    let pixels = unsafe { std::slice::from_raw_parts(buf, (stride * h) as usize) };
    let non_black = pixels.chunks(4).filter(|p| p[0] | p[1] | p[2] != 0).count();
    println!("non-black pixels: {non_black}");
    assert!(
        non_black > 1000,
        "the framebuffer looks empty, so nothing was really painted"
    );

    // Dump the framebuffer so the result can be looked at, not just counted.
    // CABIN_DUMP is opt-in so the ordinary test run writes nothing.
    if let Ok(path) = std::env::var("CABIN_DUMP") {
        let mut raw = Vec::with_capacity(pixels.len());
        raw.extend_from_slice(&w.to_le_bytes());
        raw.extend_from_slice(&h.to_le_bytes());
        raw.extend_from_slice(&stride.to_le_bytes());
        raw.extend_from_slice(pixels);
        std::fs::write(&path, raw).expect("could not write the dump");
        println!("wrote {path}");
    }

    // SAFETY: valid handle.
    unsafe { cabin_rdp_disconnect(client.0) };
}
