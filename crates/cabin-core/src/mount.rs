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
    ///
    /// This must stay pure and cheap. The settings dialogue calls it on every
    /// keystroke, because an alert dialogue closes on any response.
    pub fn check_guest_path(path: &str) -> Result<(), &'static str> {
        Self::normalise_guest_path(path).map(|_| ())
    }

    /// Like [`Mount::check_guest_path`], and also refuses a path that one of
    /// `others` already uses, or that the host socket uses.
    ///
    /// Podman refuses a create request with two mounts on one destination.
    /// The request fails only after the dialogue has closed, so the user
    /// loses the change. The dialogue has to refuse the path first.
    pub fn check_guest_path_among(path: &str, others: &[&str]) -> Result<(), &'static str> {
        let normalised = Self::normalise_guest_path(path)?;
        if normalised == crate::config::GUEST_HOST_SOCKET {
            return Err("That location holds the container socket.");
        }
        let taken = others
            .iter()
            .filter_map(|other| Self::normalise_guest_path(other).ok())
            .any(|other| other == normalised);
        if taken {
            return Err("Another shared folder already uses that location.");
        }
        Ok(())
    }

    /// Checks a guest path and returns it in the form Podman uses.
    ///
    /// Podman cleans a destination before it mounts it, so `//usr` and
    /// `/./etc` land on the same place as `/usr` and `/etc`. The checks have
    /// to compare the cleaned form, or those spellings pass.
    fn normalise_guest_path(path: &str) -> Result<String, &'static str> {
        let path = path.trim();
        if path.is_empty() {
            return Err("Give a location inside the desktop.");
        }
        if !path.starts_with('/') {
            return Err("The location must start with a slash.");
        }
        if path.contains(':') {
            // Podman separates the fields of a volume argument with colons.
            return Err("The location cannot contain a colon.");
        }
        let mut segments = Vec::new();
        for segment in path.split('/') {
            match segment {
                "" | "." => {}
                ".." => return Err("The location cannot contain '..'."),
                other => segments.push(other),
            }
        }
        if segments.is_empty() {
            return Err("The location cannot be the whole filesystem.");
        }
        let normalised = format!("/{}", segments.join("/"));

        // Overwriting these, or anything below them, would leave the desktop
        // unable to start, and the failure would look like a fault in Cabin
        // rather than a setting. /var/lib also holds the state volume.
        const RESERVED_TREES: &[&str] = &[
            "/bin",
            "/boot",
            "/dev",
            "/etc",
            "/lib",
            "/lib64",
            "/proc",
            "/sbin",
            "/sys",
            "/usr",
            "/var/lib",
            "/var/log",
            "/run/systemd",
            "/run/cabin",
            "/run/user",
        ];
        // These are refused only as themselves. Most of what lives below
        // /var and /run is harmless to cover, such as /var/www or /run/media,
        // and the parts that are not are in the list above. The home
        // directory is a named volume, so a share on it collides with the
        // volume, but a share below it is an ordinary nested mount.
        const RESERVED_EXACT: &[&str] = &["/var", "/run", crate::GUEST_HOME];
        let is_at_or_below = |root: &str| {
            normalised == root
                || normalised
                    .strip_prefix(root)
                    .is_some_and(|rest| rest.starts_with('/'))
        };
        if RESERVED_TREES.iter().any(|root| is_at_or_below(root))
            || RESERVED_EXACT.contains(&normalised.as_str())
        {
            return Err("That location belongs to the desktop's own system.");
        }
        Ok(normalised)
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
        for path in [
            "/usr",
            "/etc",
            "/dev",
            "/var/",
            "/usr/lib",
            "/etc/systemd",
            "/var/lib",
            "/var/lib/cabin",
            "/run/user/1000",
        ] {
            assert!(
                Mount::check_guest_path(path).is_err(),
                "{path} should be refused"
            );
        }
    }

    #[test]
    fn a_system_directory_spelt_another_way_is_still_refused() {
        // Podman cleans the destination, so these land on /usr and /etc.
        for path in ["//usr", "/./etc", "/usr/", "/usr//lib"] {
            assert!(
                Mount::check_guest_path(path).is_err(),
                "{path} should be refused"
            );
        }
    }

    #[test]
    fn rejects_a_parent_segment() {
        assert!(Mount::check_guest_path("/mnt/../etc").is_err());
        assert!(Mount::check_guest_path("/mnt/..").is_err());
    }

    #[test]
    fn rejects_the_home_volume_itself_but_not_below_it() {
        // The home directory is a named volume. A share on it collides with
        // the volume, but a share inside it is an ordinary nested mount.
        assert!(Mount::check_guest_path("/home/cabin").is_err());
        assert!(Mount::check_guest_path("/home/cabin/").is_err());
        assert!(Mount::check_guest_path("/home/cabin/work").is_ok());
        assert!(Mount::check_guest_path("/mnt/code").is_ok());
    }

    #[test]
    fn harmless_places_below_var_and_run_are_allowed() {
        assert!(Mount::check_guest_path("/var/www").is_ok());
        assert!(Mount::check_guest_path("/run/media/data").is_ok());
    }

    #[test]
    fn rejects_a_location_another_share_uses() {
        // Podman refuses two mounts on one destination at create time.
        assert!(Mount::check_guest_path_among("/mnt/code", &["/mnt/code"]).is_err());
        assert!(Mount::check_guest_path_among("/mnt/code/", &["/mnt//code"]).is_err());
        assert!(Mount::check_guest_path_among("/mnt/code", &["/mnt/data"]).is_ok());
        assert!(Mount::check_guest_path_among("/home/cabin/work", &["/mnt/code"]).is_ok());
    }

    #[test]
    fn rejects_the_host_socket_location() {
        let socket = crate::config::GUEST_HOST_SOCKET;
        assert!(Mount::check_guest_path_among(socket, &[]).is_err());
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
