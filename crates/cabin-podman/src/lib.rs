//! A small Podman client that speaks the libpod REST API over the rootless
//! user socket.
//!
//! Cabin uses the libpod API rather than the Docker compatibility API, because
//! libpod exposes device requests, label filters, and health state directly.
//!
//! The HTTP client here is deliberately hand-written and minimal. Podman
//! answers on a UNIX socket, the request set is small and fixed, and avoiding a
//! full HTTP stack keeps the dependency tree small enough to vendor for a
//! Flatpak build without trouble.

pub mod client;
pub mod error;
pub mod http;
pub mod model;

pub use client::Podman;
pub use error::{Error, Result};
