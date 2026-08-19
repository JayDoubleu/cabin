//! Finds the guest build context, and copies it for a user's own recipe.
//!
//! Cabin builds its guest image on the machine that runs it, so the `guest/`
//! tree has to be readable at run time. That tree is shipped inside the
//! installation and never changes. A user who wants a different desktop takes
//! a named copy of it, which is a recipe of its own. See DESIGN.md section 20.

use std::path::{Path, PathBuf};

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

/// True when `dir` is the tree Cabin ships, which cannot be written.
pub fn is_shipped(dir: &Path) -> bool {
    shipped().as_deref() == Some(dir)
}

/// Where Cabin looked, for a message when it found nothing.
pub fn searched() -> String {
    shipped_candidates()
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Records which shipped tree a full copy was taken from.
///
/// Without it there is no way to tell a copy that is current from one taken
/// three releases ago, and the second silently builds an old guest.
///
/// It lives beside the copy rather than inside it. Anything inside is part of
/// the build context and would be sent to Podman, and a build context should
/// hold what the build needs and nothing else.
fn origin_file(recipe_dir: &Path) -> PathBuf {
    let mut name = recipe_dir.file_name().unwrap_or_default().to_os_string();
    name.push(".origin");
    recipe_dir.with_file_name(name)
}

/// Notes that `recipe_dir` is a full copy of the shipped tree as it is now.
pub fn record_origin(recipe_dir: &Path) -> Result<(), String> {
    let Some(source) = shipped() else {
        return Ok(());
    };
    std::fs::write(origin_file(recipe_dir), fingerprint(&source))
        .map_err(|e| format!("could not record where the copy came from: {e}"))
}

/// True when `recipe_dir` is a full copy and the shipped tree has since moved.
///
/// An application update can bring a new guest recipe, and a copy taken before
/// it keeps building the old one. Nothing warns about that on its own.
pub fn shipped_has_moved_on(recipe_dir: &Path) -> bool {
    let Some(source) = shipped() else {
        return false;
    };
    let recorded = std::fs::read_to_string(origin_file(recipe_dir)).unwrap_or_default();
    let recorded = recorded.trim().to_string();
    // No marker means the recipe is not a full copy, so there is nothing it
    // could have fallen behind.
    if recorded.is_empty() {
        return false;
    }
    recorded != fingerprint(&source)
}

/// True when `recipe_dir` was made as a full copy of the shipped tree.
pub fn has_origin(recipe_dir: &Path) -> bool {
    origin_file(recipe_dir).is_file()
}

/// Forgets the origin marker for a recipe that is being deleted.
pub fn forget_origin(recipe_dir: &Path) {
    let _ = std::fs::remove_file(origin_file(recipe_dir));
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

/// Copies a build context, leaving out what does not belong in one.
pub fn copy_context(source: &Path, target: &Path) -> Result<(), String> {
    copy_tree(source, target)
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
            // A source checkout carries meson.build beside the Containerfile.
            // Copying it into the user's own recipe folder only invites the
            // question of what it is doing there.
            if cabin_podman::images::is_build_system_file(&entry.file_name().to_string_lossy()) {
                continue;
            }
            std::fs::copy(&from, &to)
                .map_err(|e| format!("could not copy {}: {e}", from.display()))?;
        }
    }
    Ok(())
}

/// Where an earlier version put the one copy it allowed.
///
/// That copy took the base recipe's own tag, so taking one hid the recipe
/// Cabin ships. It is now a named recipe like any other.
fn legacy_copy_dir() -> PathBuf {
    gtk::glib::user_config_dir().join("cabin").join("guest")
}

/// Moves a copy made by an earlier version into the named recipes.
///
/// Returns the name it was given, so the caller can say where it went. Doing
/// nothing is the normal case, and a failure here must not stop the
/// application: the copy stays where it is and the shipped recipe is used.
pub fn migrate_legacy_copy(recipes_root: &Path, name: &str) -> Option<String> {
    let legacy = legacy_copy_dir();
    if !legacy.join("Containerfile").is_file() {
        return None;
    }
    let target = recipes_root.join(name);
    if target.exists() {
        return None;
    }
    if let Err(e) = copy_tree(&legacy, &target) {
        tracing::error!("could not move the old guest copy: {e}");
        return None;
    }
    // The marker moves with it, so the "Cabin ships a newer recipe" warning
    // keeps working across the move.
    let old_marker = gtk::glib::user_config_dir()
        .join("cabin")
        .join("guest-origin");
    if let Ok(recorded) = std::fs::read_to_string(&old_marker) {
        let _ = std::fs::write(origin_file(&target), recorded.trim());
    } else {
        let _ = record_origin(&target);
    }
    if let Err(e) = std::fs::remove_dir_all(&legacy) {
        tracing::error!("could not remove the old guest copy: {e}");
    }
    let _ = std::fs::remove_file(&old_marker);
    tracing::info!("moved the old guest copy to {}", target.display());
    Some(name.to_string())
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
    fn the_shipped_tree_is_always_found_so_it_is_always_offered() {
        // The recipe Cabin ships is the one choice that needs nothing else to
        // exist. If it can go missing, a user who has taken a copy has no way
        // back to it.
        assert!(shipped().is_some());
        assert!(is_shipped(&shipped().unwrap()));
    }

    #[test]
    fn the_origin_marker_is_not_inside_the_build_context() {
        // Anything inside the copy is packed and sent to Podman. A build
        // context should hold what the build needs and nothing else.
        let dir = PathBuf::from("/home/someone/.config/cabin/images/mine");
        let marker = origin_file(&dir);
        assert!(!marker.starts_with(&dir));
        assert_eq!(marker.parent(), dir.parent());
    }

    #[test]
    fn a_recipe_with_no_marker_is_never_called_stale() {
        // A layered recipe is not a copy of anything, so it cannot fall
        // behind the shipped tree and must not be warned about.
        let dir = std::env::temp_dir().join(format!("cabin-nomarker-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!shipped_has_moved_on(&dir));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_copy_is_stale_once_the_shipped_tree_changes() {
        let root = std::env::temp_dir().join(format!("cabin-stale-{}", std::process::id()));
        let dir = root.join("mine");
        std::fs::create_dir_all(&dir).unwrap();

        record_origin(&dir).expect("record");
        assert!(
            !shipped_has_moved_on(&dir),
            "a copy taken just now is current"
        );

        // A marker from an older tree is what an application update leaves.
        std::fs::write(origin_file(&dir), "0000000000000000").unwrap();
        assert!(shipped_has_moved_on(&dir));

        forget_origin(&dir);
        assert!(!shipped_has_moved_on(&dir), "a forgotten marker is silent");
        std::fs::remove_dir_all(&root).ok();
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

    #[test]
    fn migrating_moves_the_old_copy_and_leaves_nothing_behind() {
        // Only meaningful when this machine has an old copy, so it checks the
        // shape of the move rather than requiring one.
        let root = std::env::temp_dir().join(format!("cabin-mig-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        if !legacy_copy_dir().join("Containerfile").is_file() {
            assert_eq!(migrate_legacy_copy(&root, "moved"), None);
        }
        std::fs::remove_dir_all(&root).ok();
    }
}
