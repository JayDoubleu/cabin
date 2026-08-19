use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MountMode {
    ReadWrite,
    ReadOnly,
}

impl MountMode {
    /// The option string that Podman expects.
    pub fn as_podman_option(self) -> &'static str {
        match self {
            Self::ReadWrite => "rw",
            Self::ReadOnly => "ro",
        }
    }
}

/// Whether a share is a whole directory or a single file.
///
/// Podman bind mounts either, but they do not behave alike, so the kind is
/// recorded rather than guessed from the path. A path that no longer exists
/// would otherwise change kind between one start and the next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum MountKind {
    #[default]
    Directory,
    File,
}

/// A host directory or file made visible inside a desktop.
///
/// The host path comes from the file chooser portal. Cabin never browses the
/// filesystem itself, so it needs no broad filesystem permission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mount {
    pub host_path: String,
    pub guest_path: String,
    pub mode: MountMode,
    /// Relabel for SELinux. Needed on Fedora when the host path is not already
    /// labelled for container use.
    #[serde(default = "default_true")]
    pub relabel: bool,
    #[serde(default)]
    pub kind: MountKind,
}

fn default_true() -> bool {
    true
}

impl Mount {
    /// Suggests a guest path from the host path, so the user rarely types one.
    pub fn suggest_guest_path(host_path: &str) -> String {
        let base = host_path
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or("mount");
        format!("/mnt/{base}")
    }

    /// Whether a guest path can be used as written.
    ///
    /// Podman takes the path as given, so a relative one silently produces a
    /// mount nobody can find, and an empty one fails at create time with a
    /// message about the request rather than about the setting.
    pub fn check_guest_path(path: &str) -> Result<(), &'static str> {
        let path = path.trim();
        if path.is_empty() {
            return Err("Give a location inside the desktop.");
        }
        if !path.starts_with('/') {
            return Err("The location must start with a slash.");
        }
        if path == "/" {
            return Err("The location cannot be the whole filesystem.");
        }
        if path.contains(':') {
            // Podman separates the fields of a volume argument with colons.
            return Err("The location cannot contain a colon.");
        }
        // Overwriting these would leave the desktop unable to start, and the
        // failure would look like a fault in Cabin rather than a setting.
        const RESERVED: &[&str] = &[
            "/bin", "/boot", "/dev", "/etc", "/lib", "/lib64", "/proc", "/run", "/sbin", "/sys",
            "/usr", "/var",
        ];
        if RESERVED.contains(&path.trim_end_matches('/')) {
            return Err("That location belongs to the desktop's own system.");
        }
        Ok(())
    }

    /// Renders the `--volume` argument that Podman expects.
    pub fn to_podman_volume(&self) -> String {
        let mut opts = vec![self.mode.as_podman_option().to_string()];
        if self.relabel {
            opts.push("z".to_string());
        }
        format!("{}:{}:{}", self.host_path, self.guest_path, opts.join(","))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggests_basename_as_guest_path() {
        assert_eq!(Mount::suggest_guest_path("/home/jay/code"), "/mnt/code");
        assert_eq!(Mount::suggest_guest_path("/home/jay/code/"), "/mnt/code");
    }

    #[test]
    fn falls_back_when_no_basename() {
        assert_eq!(Mount::suggest_guest_path("/"), "/mnt/mount");
    }

    #[test]
    fn accepts_a_sensible_guest_path() {
        assert!(Mount::check_guest_path("/mnt/code").is_ok());
        assert!(Mount::check_guest_path("/home/cabin/work").is_ok());
    }

    #[test]
    fn rejects_a_guest_path_podman_would_take_literally() {
        // Podman does not resolve these, it just uses them.
        assert!(Mount::check_guest_path("").is_err());
        assert!(Mount::check_guest_path("mnt/code").is_err());
        assert!(Mount::check_guest_path("/").is_err());
    }

    #[test]
    fn rejects_a_colon_because_podman_splits_on_it() {
        assert!(Mount::check_guest_path("/mnt/a:b").is_err());
    }

    #[test]
    fn rejects_the_guests_own_system_directories() {
        // Mounting over these leaves a desktop that cannot start, and the
        // failure does not point at the setting that caused it.
        for path in ["/usr", "/etc", "/dev", "/var/"] {
            assert!(
                Mount::check_guest_path(path).is_err(),
                "{path} should be refused"
            );
        }
    }

    #[test]
    fn a_file_share_renders_the_same_volume_argument() {
        // Podman takes a file source exactly as it takes a directory.
        let m = Mount {
            host_path: "/home/jay/.gitconfig".into(),
            guest_path: "/home/cabin/.gitconfig".into(),
            mode: MountMode::ReadOnly,
            relabel: true,
            kind: MountKind::File,
        };
        assert_eq!(
            m.to_podman_volume(),
            "/home/jay/.gitconfig:/home/cabin/.gitconfig:ro,z"
        );
    }

    #[test]
    fn renders_podman_volume() {
        let m = Mount {
            host_path: "/data".into(),
            guest_path: "/mnt/data".into(),
            mode: MountMode::ReadOnly,
            relabel: true,
            kind: MountKind::Directory,
        };
        assert_eq!(m.to_podman_volume(), "/data:/mnt/data:ro,z");
    }

    #[test]
    fn omits_relabel_when_disabled() {
        let m = Mount {
            host_path: "/data".into(),
            guest_path: "/mnt/data".into(),
            mode: MountMode::ReadWrite,
            relabel: false,
            kind: MountKind::Directory,
        };
        assert_eq!(m.to_podman_volume(), "/data:/mnt/data:rw");
    }
}
