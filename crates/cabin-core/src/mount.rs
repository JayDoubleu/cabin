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

/// A host directory made visible inside a desktop.
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
    fn renders_podman_volume() {
        let m = Mount {
            host_path: "/data".into(),
            guest_path: "/mnt/data".into(),
            mode: MountMode::ReadOnly,
            relabel: true,
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
        };
        assert_eq!(m.to_podman_volume(), "/data:/mnt/data:rw");
    }
}
