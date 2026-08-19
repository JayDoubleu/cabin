//! The set of guest recipes, and the images they build.
//!
//! A recipe is a build context: a Containerfile and the files beside it. Cabin
//! ships one, the base. A user can add their own, and each builds an image
//! that a desktop can be created from. See DESIGN.md section 21.

use crate::guestdir;
use std::path::PathBuf;

/// The tag the base recipe builds. Desktops use this unless told otherwise.
pub const BASE_TAG: &str = "localhost/cabin-guest:f44";

/// Where a custom recipe's image is published, given its name.
///
/// Names are kept in the tag so an image can be traced back to the recipe that
/// made it, and so `podman images` is readable without Cabin.
pub fn tag_for(name: &str) -> String {
    format!("localhost/cabin-guest-{name}:latest")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The recipe Cabin ships, or the user's copy of it.
    Base,
    /// One the user added.
    Custom,
}

#[derive(Debug, Clone)]
pub struct Recipe {
    /// What the user calls it.
    pub name: String,
    /// The build context.
    pub dir: PathBuf,
    /// The image tag it builds.
    pub tag: String,
    pub kind: Kind,
}

impl Recipe {
    /// True when the recipe's files can be changed.
    ///
    /// Only the shipped base cannot, because it lives inside the installation.
    pub fn editable(&self) -> bool {
        self.kind == Kind::Custom || guestdir::has_custom()
    }
}

/// Where custom recipes live.
fn custom_root() -> PathBuf {
    gtk::glib::user_config_dir().join("cabin").join("images")
}

/// The base recipe: the shipped tree, or the user's copy of it.
pub fn base() -> Option<Recipe> {
    guestdir::active().map(|(dir, source)| Recipe {
        name: match source {
            guestdir::Source::Shipped => "Cabin guest".to_string(),
            guestdir::Source::Custom => "Cabin guest (yours)".to_string(),
        },
        dir,
        tag: BASE_TAG.to_string(),
        kind: Kind::Base,
    })
}

/// Every recipe the user has added, by name.
pub fn custom() -> Vec<Recipe> {
    let root = custom_root();
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut recipes: Vec<Recipe> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().join("Containerfile").is_file())
        .map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            Recipe {
                tag: tag_for(&name),
                dir: e.path(),
                name,
                kind: Kind::Custom,
            }
        })
        .collect();
    recipes.sort_by(|a, b| a.name.cmp(&b.name));
    recipes
}

/// The base first, then the user's, which is the order they were made in.
pub fn all() -> Vec<Recipe> {
    let mut recipes: Vec<Recipe> = base().into_iter().collect();
    recipes.extend(custom());
    recipes
}

/// Whether a name can be used for a new recipe.
///
/// The name becomes part of an image tag, so Podman's rules apply: lower case,
/// and only characters a tag may hold. Rejecting here means the failure is
/// about the name rather than about a request the user never saw.
pub fn check_name(name: &str, taken: &[String]) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Give the recipe a name.".to_string());
    }
    if name.len() > 40 {
        return Err("That name is too long.".to_string());
    }
    if !name.chars().next().is_some_and(|c| c.is_ascii_lowercase()) {
        return Err("The name must start with a lower case letter.".to_string());
    }
    if let Some(bad) = name
        .chars()
        .find(|c| !c.is_ascii_lowercase() && !c.is_ascii_digit() && !matches!(c, '-' | '_'))
    {
        if bad.is_ascii_uppercase() {
            return Err("An image name cannot hold capitals.".to_string());
        }
        return Err(format!("The name cannot contain '{bad}'."));
    }
    if taken.iter().any(|t| t == name) {
        return Err(format!("A recipe called {name} already exists."));
    }
    Ok(())
}

/// The recipe a new custom image starts from.
///
/// It builds on the base rather than repeating it. Repeating it would mean
/// every custom image rebuilt the whole guest, taking minutes and another
/// gigabyte and a half, and it would drift from the base as Cabin updates.
/// Building on it takes seconds and keeps only the difference.
fn starter(name: &str) -> String {
    format!(
        "# The {name} guest image.\n\
         #\n\
         # This builds on the image Cabin ships, so only what you add below is\n\
         # rebuilt. Everything already in the desktop stays as it is.\n\
         FROM {BASE_TAG}\n\
         \n\
         LABEL io.github.jaydoubleu.cabin.guest=\"true\"\n\
         \n\
         # Add what you need. For example:\n\
         #\n\
         # RUN dnf -y install --setopt=install_weak_deps=False \\\\\n\
         #         firefox code \\\\\n\
         #     && dnf clean all\n"
    )
}

/// Creates a recipe, ready to build.
pub fn create(name: &str) -> Result<Recipe, String> {
    let name = name.trim().to_string();
    let dir = custom_root().join(&name);
    if dir.exists() {
        return Err(format!("{} already exists", dir.display()));
    }
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    std::fs::write(dir.join("Containerfile"), starter(&name))
        .map_err(|e| format!("could not write the recipe: {e}"))?;
    Ok(Recipe {
        tag: tag_for(&name),
        dir,
        name,
        kind: Kind::Custom,
    })
}

/// Deletes a recipe's files. The image it built, if any, is left alone.
pub fn delete(name: &str) -> Result<(), String> {
    let dir = custom_root().join(name);
    if !dir.exists() {
        return Ok(());
    }
    // The path is built from a name that was checked, but a recipe is a
    // directory of the user's files and deleting the wrong one is not
    // recoverable, so refuse anything that is not where it should be.
    if dir.parent() != Some(custom_root().as_path()) {
        return Err("that recipe is not in the recipes folder".to_string());
    }
    std::fs::remove_dir_all(&dir).map_err(|e| format!("could not remove {}: {e}", dir.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_custom_recipe_builds_its_own_tag() {
        assert_eq!(tag_for("dev"), "localhost/cabin-guest-dev:latest");
        assert_ne!(tag_for("dev"), BASE_TAG);
    }

    #[test]
    fn accepts_a_name_podman_can_put_in_a_tag() {
        assert!(check_name("dev", &[]).is_ok());
        assert!(check_name("dev-tools", &[]).is_ok());
        assert!(check_name("py3_11", &[]).is_ok());
    }

    #[test]
    fn rejects_a_name_podman_would_refuse() {
        // A tag cannot hold capitals, and the failure is otherwise reported
        // as a bad request rather than as a bad name.
        assert!(check_name("DevTools", &[]).is_err());
        assert!(check_name("1st", &[]).is_err());
        assert!(check_name("with space", &[]).is_err());
        assert!(check_name("", &[]).is_err());
    }

    #[test]
    fn rejects_a_name_already_used() {
        assert!(check_name("dev", &["dev".to_string()]).is_err());
    }

    #[test]
    fn the_starter_builds_on_the_base_rather_than_repeating_it() {
        // Repeating the base would cost minutes and a gigabyte and a half per
        // image, and would drift from the base as Cabin updates.
        let text = starter("dev");
        assert!(text.contains(&format!("FROM {BASE_TAG}")));
        assert!(
            !text.contains("registry.fedoraproject.org"),
            "a custom recipe should not start from the distribution image"
        );
    }

    #[test]
    fn the_starter_carries_the_label_so_the_manager_lists_it() {
        assert!(starter("dev").contains("io.github.jaydoubleu.cabin.guest"));
    }

    #[test]
    fn creating_and_deleting_a_recipe_round_trips() {
        let name = format!("probe{}", std::process::id());
        // The name has to pass the same check the dialogue applies, or the
        // test would prove something the interface cannot reach.
        assert!(check_name(&name, &[]).is_ok(), "{name}");

        let recipe = create(&name).expect("create");
        assert!(recipe.dir.join("Containerfile").is_file());
        assert_eq!(recipe.tag, tag_for(&name));
        assert!(custom().iter().any(|r| r.name == name));

        delete(&name).expect("delete");
        assert!(!recipe.dir.exists());
        assert!(!custom().iter().any(|r| r.name == name));
    }

    #[test]
    fn the_base_is_listed_first() {
        // The list is offered as choices, and the base is the one that works
        // without anything else being built.
        let all = all();
        if let Some(first) = all.first() {
            assert_eq!(first.kind, Kind::Base);
        }
    }
}
