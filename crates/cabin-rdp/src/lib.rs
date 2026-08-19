//! The RDP client side of Cabin.
//!
//! This crate owns the connection to a guest and produces frames. It knows
//! nothing about GTK, so it can be tested without a display.
//!
//! # Status
//!
//! The types and the message contract are defined and tested. The IronRDP
//! session itself is not wired up yet. That is milestone 4 in DESIGN.md.
//!
//! # Required capabilities
//!
//! The milestone 1 prototype established that `gnome-remote-desktop` refuses a
//! plain TLS handshake and answers `HYBRID_REQUIRED_BY_SERVER`. The client
//! must therefore negotiate CredSSP with NTLM. See DESIGN.md section 12.4.

pub mod frame;
pub mod session;

pub use frame::{Frame, PixelFormat, Rect};
pub use session::{Credentials, SessionCommand, SessionEvent, SessionHandle};

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
    #[error("the server requires NLA, which the client did not offer")]
    NlaRequired,
    #[error("the RDP session ended: {0}")]
    Closed(String),
    #[error("not implemented yet: {0}")]
    Unimplemented(&'static str),
}

pub type Result<T> = std::result::Result<T, Error>;
