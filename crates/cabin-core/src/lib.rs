//! Domain model for Cabin.
//!
//! This crate performs no I/O. It defines what a desktop is, and it converts
//! between that model and the container labels that hold it. Podman is the
//! only store of desktop state, so these labels are the serialisation format.

pub mod config;
pub mod labels;
pub mod mount;

pub use config::{
    check_name, DesktopConfig, Exposure, GraphicsMode, NetInterface, SocketPolicy,
    GUEST_SELINUX_TYPE,
};
pub use mount::{Mount, MountKind, MountMode};

/// The prefix on every label that Cabin writes.
pub const LABEL_PREFIX: &str = "io.github.jaydoubleu.cabin";

/// Marks a container as a Cabin desktop. Used to filter `podman ps`.
pub const LABEL_MANAGED: &str = "io.github.jaydoubleu.cabin.managed";

/// Where the guest user's home directory lives inside the container.
pub const GUEST_HOME: &str = "/home/cabin";

/// The named volume that holds a desktop's home directory.
///
/// Changing a mount, the graphics mode or a resource limit means recreating
/// the container. Without a volume the writable layer goes with it and the
/// user loses their files, so the home directory has to outlive the container.
pub fn home_volume_name(desktop_name: &str) -> String {
    format!("cabin-{desktop_name}-home")
}

/// Where a desktop keeps its own state, including its TLS certificate.
pub const GUEST_STATE: &str = "/var/lib/cabin";

/// The named volume that holds a desktop's state.
///
/// The TLS certificate lives here. Cabin pins its fingerprint, so if a
/// settings change regenerated the certificate, every reconfigure would look
/// exactly like a machine in the middle. The certificate has to outlive the
/// container for pinning to mean anything.
pub fn state_volume_name(desktop_name: &str) -> String {
    format!("cabin-{desktop_name}-state")
}

/// The run state of a desktop, as reported by Podman.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Created,
    Running,
    Paused,
    Stopping,
    Exited,
    Unknown,
}

impl State {
    pub fn from_podman(s: &str) -> Self {
        match s {
            "created" => Self::Created,
            "running" => Self::Running,
            "paused" => Self::Paused,
            "stopping" => Self::Stopping,
            "exited" | "stopped" => Self::Exited,
            _ => Self::Unknown,
        }
    }

    pub fn is_running(self) -> bool {
        matches!(self, Self::Running)
    }
}

/// A desktop, as Cabin sees it.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_and_home_are_separate_volumes() {
        // Sharing one volume would put the TLS key in the user's home
        // directory, where anything in the desktop could read it.
        assert_ne!(state_volume_name("a"), home_volume_name("a"));
    }

    #[test]
    fn home_volume_is_namespaced_per_desktop() {
        assert_eq!(home_volume_name("fedora-dev"), "cabin-fedora-dev-home");
        assert_ne!(home_volume_name("a"), home_volume_name("b"));
    }
}

#[derive(Debug, Clone)]
pub struct Desktop {
    pub id: String,
    pub name: String,
    pub state: State,
    pub config: DesktopConfig,
    /// The host port that the guest's RDP port maps to, when running.
    pub rdp_port: Option<u16>,
}
