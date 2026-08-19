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
}

pub type Result<T> = std::result::Result<T, Error>;
