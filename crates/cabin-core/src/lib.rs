//! Domain model for Cabin.
//!
//! This crate performs no I/O. It defines what a desktop is, and it converts
//! between that model and the container labels that hold it. Podman is the
//! only store of desktop state, so these labels are the serialisation format.

pub mod config;
pub mod labels;
pub mod mount;

pub use config::{DesktopConfig, GraphicsMode, SocketPolicy};
pub use mount::{Mount, MountMode};

/// The prefix on every label that Cabin writes.
pub const LABEL_PREFIX: &str = "io.github.jaydoubleu.cabin";

/// Marks a container as a Cabin desktop. Used to filter `podman ps`.
pub const LABEL_MANAGED: &str = "io.github.jaydoubleu.cabin.managed";

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
#[derive(Debug, Clone)]
pub struct Desktop {
    pub id: String,
    pub name: String,
    pub state: State,
    pub config: DesktopConfig,
    /// The host port that the guest's RDP port maps to, when running.
    pub rdp_port: Option<u16>,
}
