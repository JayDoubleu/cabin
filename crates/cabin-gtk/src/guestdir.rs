//! Finds the guest build context, and manages a customised copy of it.
//!
//! Cabin builds its guest image on the machine that runs it, so the `guest/`
//! tree has to be readable at run time. Two trees can exist: the one shipped
//! with the application, which is read-only, and a copy the user has taken and
//! changed. See DESIGN.md section 20.

use std::path::{Path, PathBuf};

/// Which recipe a build would use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The tree shipped with the application. Read-only.
    Shipped,
    /// A copy the user has taken and may have changed.
    Custom,
}

/// Records which shipped tree a custom copy was taken from.
///
/// Without it there is no way to tell a customised recipe that is current from
/// one taken three releases ago, and the second silently builds an old guest.
///
/// It lives beside the copy rather than inside it. Anything inside is part of
/// the build context and would be sent to Podman, and a build context should
/// hold what the build needs and nothing else.
fn origin_file() -> PathBuf {
    gtk::glib::user_config_dir()
        .join("cabin")
        .join("guest-origin")
}

/// Every place the shipped context may be, most specific first.
fn shipped_candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    // Set by hand, which is the escape hatch when the others are wrong.
    if let Ok(dir) = std::env::var("CABIN_GUEST_DIR") {
        paths.push(PathBuf::from(dir));
    }

    // Installed, including inside the Flatpak where the prefix is /app.
    paths.push(PathBuf::from("/app/share/cabin/guest"));
    paths.push(PathBuf::from("/usr/share/cabin/guest"));
    paths.push(PathBuf::from("/usr/local/share/cabin/guest"));

    // Beside the executable's prefix, for an install somewhere else.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(prefix) = exe.parent().and_then(|d| d.parent()) {
            paths.push(prefix.join("share/cabin/guest"));
        }
    }

    // The source tree, so `just run` works without installing anything.
    paths.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../guest"));

    paths
}

/// The tree shipped with the application, if one is readable.
///
/// A directory only counts when it holds a Containerfile. An empty or partly
/// installed directory would otherwise be chosen over a complete one further
/// down the list, and the build would fail for a reason that points nowhere.
pub fn shipped() -> Option<PathBuf> {
    shipped_candidates()
        .into_iter()
        .find(|path| path.join("Containerfile").is_file())
}

/// Where a customised copy lives.
///
/// Under the user's configuration directory, which the Flatpak can write
/// without asking for any filesystem permission. The shipped tree is inside
/// the installation and is read-only, so editing in place is not an option
/// even when it looks like one.
pub fn custom_dir() -> PathBuf {
    gtk::glib::user_config_dir().join("cabin").join("guest")
}

/// True when a customised copy exists.
pub fn has_custom() -> bool {
    custom_dir().join("Containerfile").is_file()
}

/// The tree a build would use, and where it came from.
pub fn active() -> Option<(PathBuf, Source)> {
    if has_custom() {
        return Some((custom_dir(), Source::Custom));
    }
    shipped().map(|dir| (dir, Source::Shipped))
}

/// Where Cabin looked, for a message when it found nothing.
pub fn searched() -> String {
    shipped_candidates()
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Copies the shipped tree so the user can change it.
///
/// The whole tree is copied, not only the Containerfile, because the
/// Containerfile copies the scripts and units beside it. A copy holding only
/// the one file would build nothing.
pub fn customise() -> Result<PathBuf, String> {
    let Some(source) = shipped() else {
        return Err(format!(
            "Cabin could not find the files it ships with. It looked in:\n{}",
            searched()
        ));
    };
    let target = custom_dir();
    if target.exists() {
        std::fs::remove_dir_all(&target)
            .map_err(|e| format!("could not replace the previous copy: {e}"))?;
    }
    copy_tree(&source, &target)?;
    write_origin(&source)?;
    Ok(target)
}

/// Deletes the customised copy, so builds go back to the shipped tree.
pub fn reset() -> Result<(), String> {
    let dir = custom_dir();
    if !dir.exists() {
        return Ok(());
    }
    std::fs::remove_dir_all(&dir)
        .map_err(|e| format!("could not remove {}: {e}", dir.display()))?;
    let _ = std::fs::remove_file(origin_file());
    Ok(())
}

/// True when the shipped tree has changed since the copy was taken.
///
/// An application update can bring a new guest recipe, and a copy taken before
/// it keeps building the old one. Nothing warns about that on its own.
pub fn shipped_has_moved_on() -> bool {
    let Some(source) = shipped() else {
        return false;
    };
    let recorded = std::fs::read_to_string(origin_file()).unwrap_or_default();
    let recorded = recorded.trim();
    if recorded.is_empty() {
        return false;
    }
    recorded != fingerprint(&source)
}

fn write_origin(source: &Path) -> Result<(), String> {
    let path = origin_file();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
    }
    std::fs::write(path, fingerprint(source))
        .map_err(|e| format!("could not record where the copy came from: {e}"))
}

/// A stable identifier for the contents of a tree.
///
/// The packing is already deterministic, so hashing it answers "has any of
/// this changed" without needing to walk the tree twice. This is a change
/// check, not a security check, so a short non-cryptographic hash is enough.
fn fingerprint(dir: &Path) -> String {
    let Ok(packed) = cabin_podman::images::build_context(dir) else {
        return String::new();
    };
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in &packed {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn copy_tree(source: &Path, target: &Path) -> Result<(), String> {
    std::fs::create_dir_all(target)
        .map_err(|e| format!("could not create {}: {e}", target.display()))?;
    let entries = std::fs::read_dir(source)
        .map_err(|e| format!("could not read {}: {e}", source.display()))?;
    for entry in entries.filter_map(|e| e.ok()) {
        let from = entry.path();
        let to = target.join(entry.file_name());
        let kind = entry
            .file_type()
            .map_err(|e| format!("could not read {}: {e}", from.display()))?;
        if kind.is_dir() {
            copy_tree(&from, &to)?;
        } else if kind.is_file() {
            std::fs::copy(&from, &to)
                .map_err(|e| format!("could not copy {}: {e}", from.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_shipped_context_in_the_source_tree() {
        // The repository is always a valid context, so this fails only if the
        // guest tree moves and this module is not updated with it.
        let found = shipped().expect("the source tree should provide a context");
        assert!(found.join("Containerfile").is_file());
    }

    #[test]
    fn the_installed_path_is_preferred_over_the_source_tree() {
        let paths = shipped_candidates();
        let installed = paths
            .iter()
            .position(|p| p.starts_with("/app/share"))
            .expect("the Flatpak path should be a candidate");
        let source = paths.len() - 1;
        assert!(
            installed < source,
            "an installed tree must win over the source tree"
        );
    }

    #[test]
    fn the_custom_copy_is_not_inside_the_installation() {
        // The shipped tree is read-only, in the Flatpak and in a system
        // install alike, so a copy kept beside it could never be saved.
        let custom = custom_dir();
        assert!(!custom.starts_with("/app/share"));
        assert!(!custom.starts_with("/usr"));
    }

    #[test]
    fn the_origin_marker_is_not_inside_the_build_context() {
        // Anything inside the copy is packed and sent to Podman. A build
        // context should hold what the build needs and nothing else.
        assert!(!origin_file().starts_with(custom_dir()));
    }

    /// Not a unit test of pure logic: it reports which recipe a build would
    /// use on this machine, which is the one thing that cannot be checked
    /// without looking at the filesystem.
    #[test]
    fn reports_the_active_recipe() {
        let (dir, source) = active().expect("some recipe should be found");
        println!("active recipe: {source:?} at {}", dir.display());
        assert!(dir.join("Containerfile").is_file());
        if has_custom() {
            assert_eq!(
                source,
                Source::Custom,
                "a copy must win over the shipped tree"
            );
        } else {
            assert_eq!(source, Source::Shipped);
        }
    }

    #[test]
    fn the_fingerprint_follows_the_contents() {
        let base = std::env::temp_dir().join(format!("cabin-fp-{}", std::process::id()));
        let dir = base.join("guest");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Containerfile"), b"FROM scratch\n").unwrap();
        let before = fingerprint(&dir);

        std::fs::write(dir.join("Containerfile"), b"FROM scratch\nRUN true\n").unwrap();
        let after = fingerprint(&dir);
        std::fs::remove_dir_all(&base).ok();

        assert_ne!(
            before, after,
            "a changed recipe must change the fingerprint"
        );
        assert!(!before.is_empty());
    }
}
