#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(
        "cannot reach the Podman socket at {path}. \
         Start it with: systemctl --user enable --now podman.socket"
    )]
    SocketUnavailable {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("input or output failed while talking to Podman")]
    Io(#[from] std::io::Error),

    #[error("Podman returned {status}: {message}")]
    Api { status: u16, message: String },

    #[error("could not decode the Podman response")]
    Decode(#[from] serde_json::Error),

    #[error("the Podman response was malformed: {0}")]
    Protocol(String),

    #[error("no desktop named {0}")]
    NotFound(String),

    #[error("no password recorded for desktop {0}")]
    PasswordMissing(String),

    #[error("{name} started and stopped immediately.\n\n{likely_cause}")]
    StartedThenExited { name: String, likely_cause: String },

    #[error(
        "{name} could not start, because an address of a network chosen for \
         remote access has changed. Open the desktop settings and apply them \
         again.\n\n{message}"
    )]
    AddressGone { name: String, message: String },

    #[error("{0}")]
    Settings(String),

    #[error("{name} kept its old settings, because Podman refused the new ones.\n\n{message}")]
    RecreateRefused { name: String, message: String },

    #[error(
        "{name} has its new settings, but the container with the old settings \
         could not be removed. It is called {old_name}.\n\n{message}"
    )]
    OldContainerLeft {
        name: String,
        old_name: String,
        message: String,
    },

    /// A recreate made the new container and removed the old one, and then
    /// the start failed. The text is the start's own, because the reason,
    /// such as the inotify limit, is what the user acts on. The variant
    /// tells a caller that the new settings are in place.
    #[error("{0}")]
    StartAfterRecreate(Box<Error>),

    #[error(
        "The desktop was removed, but some of its files are still on this \
         computer, in the Podman volumes {volumes}.\n\n{message}"
    )]
    VolumesLeft { volumes: String, message: String },

    #[error(
        "There is no Podman socket at {path} to give the desktop. Start it \
         with: systemctl --user enable --now podman.socket"
    )]
    HostSocketMissing { path: String },
}

pub type Result<T> = std::result::Result<T, Error>;
