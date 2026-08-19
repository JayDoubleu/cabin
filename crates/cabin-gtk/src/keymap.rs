//! Turns GTK hardware keycodes into RDP scancodes.
//!
//! RDP carries PC/AT set 1 scancodes. GTK reports X11 hardware keycodes, which
//! on Linux are evdev codes offset by 8.
//!
//! The main block is easy, because evdev numbered its keys from the AT set 1
//! scancodes in the first place: evdev 1 is Escape and the AT scancode for
//! Escape is 0x01, and that identity holds up to the keypad. Above that the
//! keys are in the 0xE0 prefixed set and need a table.

/// X11 hardware keycodes are evdev codes plus this.
const EVDEV_OFFSET: u16 = 8;

/// The highest evdev code that maps straight through to its own scancode.
/// This is `KEY_KPDOT`, the last key of the original AT keyboard block.
const LAST_DIRECT: u16 = 83;

/// Keys in the 0xE0 prefixed set, as `(evdev code, scancode)`.
///
/// The scancode is the byte after the 0xE0 prefix, which is what FreeRDP wants
/// alongside its extended flag.
const EXTENDED: &[(u16, u16)] = &[
    (96, 0x1C),  // KEY_KPENTER
    (97, 0x1D),  // KEY_RIGHTCTRL
    (98, 0x35),  // KEY_KPSLASH
    (100, 0x38), // KEY_RIGHTALT
    (102, 0x47), // KEY_HOME
    (103, 0x48), // KEY_UP
    (104, 0x49), // KEY_PAGEUP
    (105, 0x4B), // KEY_LEFT
    (106, 0x4D), // KEY_RIGHT
    (107, 0x4F), // KEY_END
    (108, 0x50), // KEY_DOWN
    (109, 0x51), // KEY_PAGEDOWN
    (110, 0x52), // KEY_INSERT
    (111, 0x53), // KEY_DELETE
    (125, 0x5B), // KEY_LEFTMETA, the Super key
    (126, 0x5C), // KEY_RIGHTMETA
    (127, 0x5D), // KEY_COMPOSE, the menu key
];

/// Maps a GTK hardware keycode to `(scancode, extended)`.
///
/// Returns `None` for a key with no RDP equivalent, so the caller can drop it
/// rather than send something arbitrary.
pub fn to_scancode(hardware_keycode: u16) -> Option<(u16, bool)> {
    let evdev = hardware_keycode.checked_sub(EVDEV_OFFSET)?;
    if evdev == 0 {
        return None;
    }
    if evdev <= LAST_DIRECT {
        return Some((evdev, false));
    }
    EXTENDED
        .iter()
        .find(|(code, _)| *code == evdev)
        .map(|(_, scancode)| (*scancode, true))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_maps_to_one() {
        // evdev KEY_ESC is 1, so the X11 keycode is 9.
        assert_eq!(to_scancode(9), Some((0x01, false)));
    }

    #[test]
    fn the_digit_row_maps_straight_through() {
        // evdev KEY_1 is 2, KEY_0 is 11.
        assert_eq!(to_scancode(2 + EVDEV_OFFSET), Some((0x02, false)));
        assert_eq!(to_scancode(11 + EVDEV_OFFSET), Some((0x0B, false)));
    }

    #[test]
    fn letters_map_straight_through() {
        // evdev KEY_A is 30, KEY_Q is 16.
        assert_eq!(to_scancode(30 + EVDEV_OFFSET), Some((0x1E, false)));
        assert_eq!(to_scancode(16 + EVDEV_OFFSET), Some((0x10, false)));
    }

    #[test]
    fn super_is_extended() {
        // KEY_LEFTMETA. The keyboard test drives the guest with this one, so
        // getting it wrong would be noticed loudly.
        assert_eq!(to_scancode(125 + EVDEV_OFFSET), Some((0x5B, true)));
    }

    #[test]
    fn arrows_are_extended() {
        assert_eq!(to_scancode(103 + EVDEV_OFFSET), Some((0x48, true)));
        assert_eq!(to_scancode(108 + EVDEV_OFFSET), Some((0x50, true)));
    }

    #[test]
    fn the_boundary_key_is_still_direct() {
        assert_eq!(
            to_scancode(LAST_DIRECT + EVDEV_OFFSET),
            Some((LAST_DIRECT, false))
        );
    }

    #[test]
    fn unknown_keys_are_dropped_rather_than_guessed() {
        // Just past the direct block and not in the extended table.
        assert_eq!(to_scancode(90 + EVDEV_OFFSET), None);
        // Below the offset, which should never happen but must not panic.
        assert_eq!(to_scancode(0), None);
        assert_eq!(to_scancode(EVDEV_OFFSET), None);
    }

    #[test]
    fn no_duplicate_entries_in_the_extended_table() {
        let mut codes: Vec<u16> = EXTENDED.iter().map(|(c, _)| *c).collect();
        codes.sort_unstable();
        let before = codes.len();
        codes.dedup();
        assert_eq!(before, codes.len(), "an evdev code is listed twice");
    }
}
