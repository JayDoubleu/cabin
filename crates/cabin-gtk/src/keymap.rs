//! Turns GTK hardware keycodes into RDP scancodes.
//!
//! RDP carries PC/AT set 1 scancodes. GTK reports X11 hardware keycodes, which
//! on Linux are evdev codes offset by 8.
//!
//! The main block is easy, because evdev numbered its keys from the AT set 1
//! scancodes in the first place: evdev 1 is Escape and the AT scancode for
//! Escape is 0x01, and that identity holds up to the keypad. Above that, the
//! identity does not hold, and each key needs a table entry. Most of those
//! keys are in the 0xE0 prefixed set, but some are plain set 1 keys with a
//! different number, such as F11, F12 and the ISO key beside left Shift. Each
//! kind has its own table.

use std::collections::BTreeSet;

/// X11 hardware keycodes are evdev codes plus this.
const EVDEV_OFFSET: u16 = 8;

/// The highest evdev code that maps straight through to its own scancode.
/// This is `KEY_KPDOT`, the last key of the original AT keyboard block.
const LAST_DIRECT: u16 = 83;

/// Plain set 1 keys above `LAST_DIRECT`, as `(evdev code, scancode)`.
///
/// These have no 0xE0 prefix, but their evdev code is not their scancode.
/// They were missing once, so F11, F12 and the UK backslash key did nothing.
const NON_EXTENDED: &[(u16, u16)] = &[
    (86, 0x56),  // KEY_102ND, the ISO key beside left Shift
    (87, 0x57),  // KEY_F11
    (88, 0x58),  // KEY_F12
    (89, 0x73),  // KEY_RO, on Japanese and Brazilian keyboards
    (117, 0x59), // KEY_KPEQUAL
    (124, 0x7D), // KEY_YEN, on Japanese keyboards
];

/// Keys in the 0xE0 prefixed set, as `(evdev code, scancode)`.
///
/// Pause (evdev 119) is not here on purpose. It sends the E1 1D 45 sequence,
/// which has no single extended scancode, so it is dropped rather than
/// guessed at.
///
/// The scancode is the byte after the 0xE0 prefix, which is what FreeRDP wants
/// alongside its extended flag.
const EXTENDED: &[(u16, u16)] = &[
    (96, 0x1C),  // KEY_KPENTER
    (97, 0x1D),  // KEY_RIGHTCTRL
    (98, 0x35),  // KEY_KPSLASH
    (99, 0x37),  // KEY_SYSRQ, the Print Screen key
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
    if let Some((_, scancode)) = NON_EXTENDED.iter().find(|(code, _)| *code == evdev) {
        return Some((*scancode, false));
    }
    EXTENDED
        .iter()
        .find(|(code, _)| *code == evdev)
        .map(|(_, scancode)| (*scancode, true))
}

/// The keys the guest believes are held down, as `(scancode, extended)`.
///
/// GTK sends no release for a key that is lifted after the display loses
/// focus. Without this record, Alt stays held in the guest after Alt+Tab
/// takes the user to another window, and every later click is an Alt click.
#[derive(Debug, Default)]
pub struct HeldKeys {
    keys: BTreeSet<(u16, bool)>,
}

impl HeldKeys {
    /// Records a key event that was sent to the guest. Key repeat sends a
    /// press for a key that is already held, which changes nothing.
    pub fn record(&mut self, scancode: u16, extended: bool, pressed: bool) {
        if pressed {
            self.keys.insert((scancode, extended));
        } else {
            self.keys.remove(&(scancode, extended));
        }
    }

    /// Returns every held key and forgets them all, so the caller can send a
    /// release for each one.
    pub fn take_all(&mut self) -> Vec<(u16, bool)> {
        std::mem::take(&mut self.keys).into_iter().collect()
    }
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

    #[test]
    fn no_evdev_code_is_in_both_tables_or_the_direct_block() {
        for (code, _) in NON_EXTENDED {
            assert!(*code > LAST_DIRECT, "{code} is already direct");
            assert!(
                !EXTENDED.iter().any(|(c, _)| c == code),
                "{code} is in both tables"
            );
        }
        for (code, _) in EXTENDED {
            assert!(*code > LAST_DIRECT, "{code} is already direct");
        }
    }

    #[test]
    fn f11_and_f12_are_plain_set_one_keys() {
        assert_eq!(to_scancode(87 + EVDEV_OFFSET), Some((0x57, false)));
        assert_eq!(to_scancode(88 + EVDEV_OFFSET), Some((0x58, false)));
    }

    #[test]
    fn the_iso_key_beside_left_shift_is_sent() {
        // The backslash key on a UK keyboard.
        assert_eq!(to_scancode(86 + EVDEV_OFFSET), Some((0x56, false)));
    }

    #[test]
    fn print_screen_is_extended() {
        assert_eq!(to_scancode(99 + EVDEV_OFFSET), Some((0x37, true)));
    }

    #[test]
    fn pause_is_dropped_rather_than_guessed() {
        assert_eq!(to_scancode(119 + EVDEV_OFFSET), None);
    }

    #[test]
    fn held_keys_are_released_once() {
        let mut held = HeldKeys::default();
        held.record(0x38, false, true); // left Alt down
        held.record(0x38, false, true); // key repeat
        held.record(0x5B, true, true); // Super down
        assert_eq!(held.take_all(), vec![(0x38, false), (0x5B, true)]);
        assert!(held.take_all().is_empty(), "a key was released twice");
    }

    #[test]
    fn a_released_key_is_not_held() {
        let mut held = HeldKeys::default();
        held.record(0x1E, false, true);
        held.record(0x1E, false, false);
        assert!(held.take_all().is_empty());
    }

    #[test]
    fn extended_and_plain_keys_with_one_scancode_are_separate() {
        // Left Ctrl is 0x1D and right Ctrl is extended 0x1D.
        let mut held = HeldKeys::default();
        held.record(0x1D, false, true);
        held.record(0x1D, true, true);
        held.record(0x1D, false, false);
        assert_eq!(held.take_all(), vec![(0x1D, true)]);
    }
}
