//! The message contract between the RDP session and the user interface.
//!
//! The session runs on a plain thread of its own, in `run_session`, and never
//! touches GTK types. Everything crosses through these two enums.
//!
//! - Commands go to the session over a `std::sync::mpsc` channel, which has
//!   no bound. Input is small and the session drains it on every poll.
//! - Events come back over an `async_channel`, and the caller chooses the
//!   bound. The viewer uses a bound of one, so a slow interface applies back
//!   pressure on frames instead of growing a queue. See DESIGN.md section 6.2.

use crate::frame::Frame;

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
    /// Wheel rotation in RDP units, where 120 is one notch. Positive
    /// vertical is away from the user. The session splits a large value into
    /// several events, so any value is safe to send.
    PointerScroll {
        horizontal: i32,
        vertical: i32,
    },
    ClipboardOffer {
        text: String,
    },
    Disconnect,
}

/// Sent from the session to the interface.
#[derive(Debug)]
pub enum SessionEvent {
    /// The session is still trying to reach the guest.
    ///
    /// A desktop that has just started does not answer for a few seconds, so
    /// the session retries rather than reporting a failure the user has to
    /// clear by hand. See DESIGN.md section 17.1.
    Connecting {
        attempt: u32,
        /// How long the session has been trying, in seconds.
        waited: u64,
    },
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
    /// The guest presented a certificate that does not match the one recorded
    /// for it. The session does not retry this, because only the user can
    /// decide whether the change is expected. `seen` is the new fingerprint.
    CertificateChanged {
        seen: String,
    },
    Error(String),
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

/// Wheel rotation, from what GTK reports to what RDP carries.
pub mod wheel {
    /// One wheel notch, in RDP units.
    pub const NOTCH: i32 = 120;

    /// How far a touchpad moves, in pixels, for one notch.
    ///
    /// A touchpad reports pixels, not notches. Sending one notch per pixel
    /// scrolled a page for a small finger movement.
    pub const PIXELS_PER_NOTCH: f64 = 12.0;

    /// The largest rotation one RDP wheel event can carry.
    ///
    /// The field is nine bit two's complement, so the range is -256 to 255.
    /// Cabin uses the same magnitude in both directions.
    pub const MAX_STEP: i32 = 255;

    /// The largest rotation taken from one GTK event.
    ///
    /// A fast fling can report a very large delta. Without a limit, one event
    /// could become thousands of RDP events.
    pub const MAX_PER_EVENT: i32 = NOTCH * 20;

    /// What a GTK scroll delta counts.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Unit {
        /// Wheel notches. A mouse wheel reports this.
        Wheel,
        /// Surface pixels. A touchpad reports this.
        Pixels,
    }

    /// Turns a stream of fractional deltas into whole RDP units.
    ///
    /// A touchpad reports many small deltas. Each one alone is less than one
    /// RDP unit, so rounding each one drops all of them. The accumulator keeps
    /// the part that was not sent and adds it to the next delta.
    #[derive(Debug, Clone, Copy, Default, PartialEq)]
    pub struct Accumulator {
        remainder: f64,
    }

    impl Accumulator {
        /// Adds a GTK delta and returns the whole RDP units to send now.
        pub fn feed(&mut self, delta: f64, unit: Unit) -> i32 {
            if !delta.is_finite() {
                return 0;
            }
            let units = match unit {
                Unit::Wheel => delta * NOTCH as f64,
                Unit::Pixels => delta / PIXELS_PER_NOTCH * NOTCH as f64,
            };
            let total = self.remainder + units;
            let whole = total.trunc();
            self.remainder = total - whole;
            let limit = MAX_PER_EVENT as f64;
            whole.clamp(-limit, limit) as i32
        }

        /// Forgets any part of a unit that was not sent.
        pub fn reset(&mut self) {
            self.remainder = 0.0;
        }
    }

    /// Splits a rotation into RDP wheel events and encodes each one.
    ///
    /// Each value is the rotation field of one pointer event, ready to be
    /// combined with the wheel flag. A negative step is nine bit two's
    /// complement, so the encoding sets the sign bit itself. The guest decodes
    /// it that way. An earlier version sent the sign and the magnitude, and
    /// the guest read a scroll of -120 as -136, and -1 as -255.
    pub fn encode(delta: i32) -> Vec<u32> {
        let mut steps = Vec::new();
        let mut left = delta;
        while left != 0 {
            let step = left.clamp(-MAX_STEP, MAX_STEP);
            steps.push((step as u32) & cabin_freerdp_sys::WHEEL_ROTATION_MASK);
            left -= step;
        }
        steps
    }
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
}

#[cfg(test)]
mod wheel_tests {
    use super::wheel::*;
    use cabin_freerdp_sys::PTR_FLAGS_WHEEL_NEGATIVE;

    /// Decodes a step the way the guest does, to check the round trip.
    fn decode(step: u32) -> i32 {
        if step & PTR_FLAGS_WHEEL_NEGATIVE != 0 {
            step as i32 - 0x200
        } else {
            step as i32
        }
    }

    #[test]
    fn one_notch_up_is_positive() {
        assert_eq!(encode(120), vec![0x078]);
    }

    #[test]
    fn one_notch_down_is_twos_complement() {
        // FreeRDP's own clients send this value for one notch down.
        assert_eq!(encode(-120), vec![0x188]);
    }

    #[test]
    fn the_smallest_step_down_is_all_ones() {
        assert_eq!(encode(-1), vec![0x1FF]);
    }

    #[test]
    fn only_a_negative_step_sets_the_sign_bit() {
        for delta in [-1, -120, -255] {
            assert_ne!(encode(delta)[0] & PTR_FLAGS_WHEEL_NEGATIVE, 0, "{delta}");
        }
        for delta in [1, 120, 255] {
            assert_eq!(encode(delta)[0] & PTR_FLAGS_WHEEL_NEGATIVE, 0, "{delta}");
        }
    }

    #[test]
    fn a_large_rotation_splits_without_wrapping() {
        assert_eq!(encode(300), vec![255, 45]);
        let steps = encode(-600);
        assert_eq!(steps.len(), 3);
        assert_eq!(steps.iter().map(|s| decode(*s)).sum::<i32>(), -600);
    }

    #[test]
    fn nothing_is_sent_for_no_rotation() {
        assert!(encode(0).is_empty());
    }

    #[test]
    fn every_value_round_trips() {
        for delta in -1000..=1000 {
            let sum: i32 = encode(delta).iter().map(|s| decode(*s)).sum();
            assert_eq!(sum, delta);
        }
    }

    #[test]
    fn a_wheel_notch_is_120_units() {
        let mut acc = Accumulator::default();
        assert_eq!(acc.feed(1.0, Unit::Wheel), 120);
        assert_eq!(acc.feed(-1.0, Unit::Wheel), -120);
    }

    #[test]
    fn touchpad_pixels_are_scaled_to_notches() {
        let mut acc = Accumulator::default();
        assert_eq!(acc.feed(PIXELS_PER_NOTCH, Unit::Pixels), NOTCH);
    }

    #[test]
    fn small_deltas_add_up_instead_of_vanishing() {
        let mut acc = Accumulator::default();
        // A hundredth of a notch is 1.2 units. Rounded one at a time, a whole
        // notch of these would be lost.
        let sent: i32 = (0..100).map(|_| acc.feed(0.01, Unit::Wheel)).sum();
        assert!((119..=120).contains(&sent), "sent {sent}");
    }

    #[test]
    fn the_sign_is_kept() {
        let mut acc = Accumulator::default();
        assert!(acc.feed(-3.0, Unit::Pixels) < 0);
        let mut acc = Accumulator::default();
        assert!(acc.feed(3.0, Unit::Pixels) > 0);
    }

    #[test]
    fn a_huge_delta_is_limited() {
        let mut acc = Accumulator::default();
        assert_eq!(acc.feed(1.0e9, Unit::Pixels), MAX_PER_EVENT);
        assert_eq!(acc.feed(f64::NAN, Unit::Wheel), 0);
    }

    #[test]
    fn reset_drops_the_remainder() {
        let mut acc = Accumulator::default();
        acc.feed(0.005, Unit::Wheel);
        acc.reset();
        assert_eq!(acc, Accumulator::default());
    }
}
