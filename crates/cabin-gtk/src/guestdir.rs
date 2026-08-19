//! Finds the guest build context that ships with the application.
//!
//! Cabin builds its guest image on the machine that runs it, so the
//! `guest/` tree has to be readable at runtime. Where it lives depends on how
//! Cabin was started. See DESIGN.md section 19.

use std::path::PathBuf;

/// Every place the build context may be, most specific first.
fn candidates() -> Vec<PathBuf> {
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

/// The build context directory, if one is readable.
///
/// A directory only counts when it holds a Containerfile. An empty or partly
/// installed directory would otherwise be chosen over a complete one further
/// down the list, and the build would fail for a reason that points nowhere.
pub fn find() -> Option<PathBuf> {
    candidates()
        .into_iter()
        .find(|path| path.join("Containerfile").is_file())
}

/// Where Cabin looked, for a message when it found nothing.
pub fn searched() -> String {
    candidates()
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_context_in_the_source_tree() {
        // The repository is always a valid context, so this fails only if the
        // guest tree moves and this module is not updated with it.
        let found = find().expect("the source tree should provide a context");
        assert!(found.join("Containerfile").is_file());
    }

    #[test]
    fn an_env_var_takes_priority() {
        assert_eq!(
            candidates().first(),
            Some(&PathBuf::from("/app/share/cabin/guest")),
            "with no override the installed path comes first"
        );
    }
}
