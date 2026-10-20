//! Remembers each desktop's TLS certificate fingerprint.
//!
//! Every desktop signs its own certificate, so there is no authority to check
//! it against. Pinning is what makes the connection mean anything: the first
//! connection records the fingerprint, and a later one that presents a
//! different certificate is refused rather than quietly accepted.
//!
//! The store lives in the application's own data directory, which needs no
//! extra sandbox permission. Desktops are keyed by name, because a container
//! id changes whenever a desktop is reconfigured.

use std::path::PathBuf;

fn store_dir() -> PathBuf {
    gtk::glib::user_data_dir()
        .join("cabin")
        .join("fingerprints")
}

/// A file name that cannot escape the store, whatever the desktop is called.
///
/// The map must give each name its own file. Desktop names may hold '.', so
/// mapping it to '_' gave "my.desk" and "my_desk" one shared pin, and each
/// desktop then refused the other's certificate. Every byte outside the plain
/// set is written as '%' and two hex digits, and '%' itself is outside the
/// set, so no two names give the same key.
///
/// A leading '.' is also encoded. Cabin's own name check already rejects one,
/// but a container made outside Cabin carries whatever name its labels give,
/// and "." or ".." must never become a file name.
fn key_for(desktop_name: &str) -> String {
    let mut key = String::with_capacity(desktop_name.len());
    for (index, byte) in desktop_name.bytes().enumerate() {
        let plain = byte.is_ascii_alphanumeric()
            || byte == b'-'
            || byte == b'_'
            || (byte == b'.' && index > 0);
        if plain {
            key.push(char::from(byte));
        } else {
            key.push_str(&format!("%{byte:02X}"));
        }
    }
    key
}

/// The fingerprint recorded for a desktop, if this is not the first time.
pub fn recorded(desktop_name: &str) -> Option<String> {
    let path = store_dir().join(key_for(desktop_name));
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Records the fingerprint seen on a first connection.
///
/// Also records a changed fingerprint that the user confirmed. That pins the
/// exact certificate the user saw, not whatever the next connection presents.
pub fn record(desktop_name: &str, fingerprint: &str) {
    let dir = store_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!("could not create the fingerprint store: {e}");
        return;
    }
    let path = dir.join(key_for(desktop_name));
    if let Err(e) = std::fs::write(&path, fingerprint) {
        tracing::warn!("could not record the fingerprint: {e}");
    }
}

/// Forgets a desktop's fingerprint, so the next connection records a new one.
///
/// Only the deletion of a desktop uses this. A desktop made later with the
/// same name has a new certificate, and must not inherit the old pin.
pub fn forget(desktop_name: &str) {
    let _ = std::fs::remove_file(store_dir().join(key_for(desktop_name)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_cannot_escape_the_store() {
        // A desktop is named by the user, so the name must never become a path.
        for name in ["../../etc/passwd", "a/b", "../x", "..", ".", ".hidden"] {
            let key = key_for(name);
            assert!(!key.contains('/'), "{name} gave {key}");
            assert!(!key.starts_with('.'), "{name} gave {key}");
        }
        assert_eq!(key_for("a/b"), "a%2Fb");
    }

    #[test]
    fn ordinary_names_are_left_alone() {
        // The earlier map kept letters, digits, '-' and '_', and wrote every
        // other character as '_'. A name made only of the kept characters has
        // the same key as before, so its recorded pin is still found.
        assert_eq!(key_for("fedora-dev"), "fedora-dev");
        assert_eq!(key_for("my_desktop2"), "my_desktop2");
    }

    #[test]
    fn a_name_with_a_dot_has_a_new_key() {
        // The earlier map gave "work.dev" the key "work_dev". The new key
        // differs, so a desktop with a '.' in its name records its pin again
        // on the next connection. The old file is not removed, because
        // "work_dev" is also the key of a desktop called "work_dev", and
        // removing it could delete that desktop's pin.
        assert_eq!(key_for("work.dev"), "work.dev");
        assert_ne!(key_for("work.dev"), "work_dev");
    }

    #[test]
    fn different_names_never_share_a_pin() {
        // Each desktop has its own certificate, so a shared pin makes one of
        // them refuse the other's certificate as an attack.
        assert_ne!(key_for("my.desk"), key_for("my_desk"));
        assert_ne!(key_for("a%2Fb"), key_for("a/b"));
        assert_ne!(key_for("a b"), key_for("a_b"));
    }

    #[test]
    fn nothing_is_recorded_for_an_unknown_desktop() {
        assert_eq!(recorded("a-desktop-that-was-never-connected-9d2f"), None);
    }
}
