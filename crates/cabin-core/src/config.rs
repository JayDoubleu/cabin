use crate::mount::Mount;
use serde::{Deserialize, Serialize};

/// How a desktop renders. See DESIGN.md section 7.3.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum GraphicsMode {
    /// Cabin picks the first non-NVIDIA render node it finds.
    #[default]
    Automatic,
    /// No device is passed in. Mesa falls back to llvmpipe.
    Software,
    /// A specific DRM render node, for example `/dev/dri/renderD129`.
    RenderNode { path: String },
}

impl GraphicsMode {
    /// Resolves to the device path to pass to Podman, if any.
    ///
    /// `available` is the list of render nodes with their driver names, which
    /// the caller reads from sysfs.
    pub fn resolve<'a>(&'a self, available: &'a [(String, String)]) -> Option<&'a str> {
        match self {
            Self::Software => None,
            Self::RenderNode { path } => Some(path.as_str()),
            // NVIDIA needs the Container Device Interface and matching guest
            // drivers, so it is never chosen automatically.
            Self::Automatic => available
                .iter()
                .find(|(_, driver)| driver != "nvidia")
                .map(|(path, _)| path.as_str()),
        }
    }
}

/// Whether a desktop can reach a container runtime. See DESIGN.md section 7.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SocketPolicy {
    /// Nothing is exposed.
    #[default]
    None,
    /// The desktop runs its own rootless Podman. This is the safe choice.
    Nested,
    /// The host Podman socket is bind-mounted in. Dangerous.
    HostPodman,
    /// The host Docker socket is bind-mounted in. More dangerous: the Docker
    /// daemon runs as root, so this grants root on the host.
    HostDocker,
}

impl SocketPolicy {
    /// True when the user must confirm a warning before this applies.
    pub fn needs_warning(self) -> bool {
        matches!(self, Self::HostPodman | Self::HostDocker)
    }

    /// The warning text shown before the setting applies. Returns `None` when
    /// no warning is needed.
    pub fn warning(self) -> Option<&'static str> {
        match self {
            Self::HostPodman => Some(
                "Exposing the host Podman socket gives this desktop the same power \
                 over your computer as the account that runs it. A process in the \
                 desktop can start a privileged container and read or change any \
                 file. Only enable this for desktops you fully trust.",
            ),
            Self::HostDocker => Some(
                "Exposing the host Docker socket gives this desktop root access to \
                 your computer. The Docker daemon runs as root, so a process in the \
                 desktop can take full control of the host. Only enable this for \
                 desktops you fully trust.",
            ),
            _ => None,
        }
    }
}

/// Everything the user can configure about a desktop.
///
/// This is `PartialEq` but not `Eq`, because the CPU limit is a float.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DesktopConfig {
    pub name: String,
    pub image: String,
    #[serde(default)]
    pub mounts: Vec<Mount>,
    #[serde(default)]
    pub graphics: GraphicsMode,
    #[serde(default)]
    pub socket: SocketPolicy,
    /// Memory limit in megabytes. `None` means no limit.
    #[serde(default)]
    pub memory_mb: Option<u64>,
    /// CPU limit in whole cores. `None` means no limit.
    #[serde(default)]
    pub cpus: Option<f64>,
    /// Starting virtual monitor size. RDP resizes it after connecting.
    #[serde(default = "default_width")]
    pub width: u32,
    #[serde(default = "default_height")]
    pub height: u32,
    /// Listen beyond loopback. Off by default.
    #[serde(default)]
    pub expose_on_lan: bool,
}

fn default_width() -> u32 {
    1920
}
fn default_height() -> u32 {
    1080
}

impl DesktopConfig {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            image: "localhost/cabin-guest:f44".to_string(),
            mounts: Vec::new(),
            graphics: GraphicsMode::default(),
            socket: SocketPolicy::default(),
            memory_mb: Some(4096),
            cpus: None,
            width: default_width(),
            height: default_height(),
            expose_on_lan: false,
        }
    }

    /// The address that the RDP port publishes on.
    pub fn bind_address(&self) -> &'static str {
        if self.expose_on_lan {
            "0.0.0.0"
        } else {
            "127.0.0.1"
        }
    }

    /// Changing any of these needs the container to be recreated.
    pub fn needs_recreate(&self, other: &Self) -> bool {
        self.image != other.image
            || self.mounts != other.mounts
            || self.graphics != other.graphics
            || self.socket != other.socket
            || self.memory_mb != other.memory_mb
            || self.cpus != other.cpus
            || self.expose_on_lan != other.expose_on_lan
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nodes() -> Vec<(String, String)> {
        vec![
            ("/dev/dri/renderD128".into(), "nvidia".into()),
            ("/dev/dri/renderD129".into(), "i915".into()),
        ]
    }

    #[test]
    fn automatic_skips_nvidia() {
        let n = nodes();
        assert_eq!(
            GraphicsMode::Automatic.resolve(&n),
            Some("/dev/dri/renderD129")
        );
    }

    #[test]
    fn automatic_yields_nothing_when_only_nvidia() {
        let n = vec![("/dev/dri/renderD128".to_string(), "nvidia".to_string())];
        assert_eq!(GraphicsMode::Automatic.resolve(&n), None);
    }

    #[test]
    fn software_passes_no_device() {
        assert_eq!(GraphicsMode::Software.resolve(&nodes()), None);
    }

    #[test]
    fn host_sockets_warn_and_defaults_do_not() {
        assert!(SocketPolicy::HostDocker.needs_warning());
        assert!(SocketPolicy::HostPodman.needs_warning());
        assert!(!SocketPolicy::Nested.needs_warning());
        assert!(!SocketPolicy::None.needs_warning());
        assert!(SocketPolicy::None.warning().is_none());
    }

    #[test]
    fn default_is_loopback_only() {
        let c = DesktopConfig::new("test");
        assert_eq!(c.bind_address(), "127.0.0.1");
        assert_eq!(c.socket, SocketPolicy::None);
    }

    #[test]
    fn resolution_change_alone_does_not_recreate() {
        let a = DesktopConfig::new("test");
        let mut b = a.clone();
        b.width = 2560;
        assert!(!a.needs_recreate(&b));
    }

    #[test]
    fn mount_change_forces_recreate() {
        let a = DesktopConfig::new("test");
        let mut b = a.clone();
        b.mounts.push(Mount {
            host_path: "/data".into(),
            guest_path: "/mnt/data".into(),
            mode: crate::mount::MountMode::ReadOnly,
            relabel: true,
        });
        assert!(a.needs_recreate(&b));
    }
}
