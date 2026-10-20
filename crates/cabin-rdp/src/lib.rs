//! The RDP client side of Cabin.
//!
//! This crate owns the connection to a guest and produces frames. It knows
//! nothing about GTK, so it can be tested without a display.
//!
//! # How it works
//!
//! The session runs on FreeRDP 3, through the C shim in `cabin-freerdp-sys`.
//! See DESIGN.md sections 14 and 15 for why. [`run_session`] is the entry
//! point. It blocks its thread until the session ends, so the caller gives it
//! a thread of its own and talks to it through the channels in [`session`].
//!
//! # Required capabilities
//!
//! The milestone 1 prototype established that `gnome-remote-desktop` refuses a
//! plain TLS handshake and answers `HYBRID_REQUIRED_BY_SERVER`. The client
//! must therefore negotiate CredSSP with NTLM. See DESIGN.md section 12.4.

pub mod client;
pub mod frame;
pub mod session;

pub use client::{run_session, ConnectionParams};
pub use frame::{Frame, PixelFormat, Rect};
pub use session::{Credentials, SessionCommand, SessionEvent};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not connect to {address}")]
    Connect {
        address: String,
        #[source]
        source: std::io::Error,
    },
    #[error("the server rejected the credentials")]
    AuthenticationFailed,

    #[error(
        "this desktop presented a different security certificate than last time.\n\n\
         That is expected if the desktop was rebuilt from scratch. If it was not, \
         something may be intercepting the connection.\n\nIt now presents:\n{seen}"
    )]
    CertificateChanged { seen: String },
    #[error("the server requires NLA, which the client did not offer")]
    NlaRequired,
    #[error("the RDP session ended: {0}")]
    Closed(String),
    #[error("not implemented yet: {0}")]
    Unimplemented(&'static str),
}

pub type Result<T> = std::result::Result<T, Error>;
