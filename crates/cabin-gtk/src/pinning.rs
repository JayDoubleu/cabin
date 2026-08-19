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
fn key_for(desktop_name: &str) -> String {
    desktop_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
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
/// Used when the user decides a changed certificate is expected, and when a
/// desktop is deleted.
pub fn forget(desktop_name: &str) {
    let _ = std::fs::remove_file(store_dir().join(key_for(desktop_name)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_cannot_escape_the_store() {
        // A desktop is named by the user, so the name must never become a path.
        assert_eq!(key_for("../../etc/passwd"), "______etc_passwd");
        assert_eq!(key_for("a/b"), "a_b");
        assert!(!key_for("../x").contains('/'));
        assert!(!key_for("..").contains('.'));
    }

    #[test]
    fn ordinary_names_are_left_alone() {
        assert_eq!(key_for("fedora-dev"), "fedora-dev");
        assert_eq!(key_for("my_desktop2"), "my_desktop2");
    }

    #[test]
    fn nothing_is_recorded_for_an_unknown_desktop() {
        assert_eq!(recorded("a-desktop-that-was-never-connected-9d2f"), None);
    }
}
