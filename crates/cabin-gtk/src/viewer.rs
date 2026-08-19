//! Starts an RDP session for a desktop and feeds its frames to the widget.

use crate::display::CabinDisplay;
use cabin_core::Desktop;
use cabin_rdp::{ConnectionParams, Credentials, SessionEvent};

/// What the viewer should be showing.
#[derive(Debug, Clone)]
pub enum State {
    Connecting,
    Showing,
    Failed(String),
}

/// Connects to `desktop` and drives `display` from the session.
///
/// The session runs on its own thread, because FreeRDP is not thread safe and
/// every call must come from the thread that created the client. Frames cross
/// back through a bounded channel, so a slow interface applies back pressure
/// rather than building a queue of stale frames.
pub fn attach(
    display: &CabinDisplay,
    desktop: &Desktop,
    password: String,
    on_state: impl Fn(State) + 'static,
) {
    let Some(port) = desktop.rdp_port else {
        tracing::warn!("{} has no published RDP port", desktop.name);
        on_state(State::Failed(format!(
            "{} publishes no RDP port",
            desktop.name
        )));
        return;
    };
    on_state(State::Connecting);

    let params = ConnectionParams {
        host: "127.0.0.1".to_string(),
        port,
        credentials: Credentials {
            username: "cabin".to_string(),
            password,
            domain: String::new(),
        },
        width: desktop.config.width,
        height: desktop.config.height,
    };

    let (command_tx, command_rx) = std::sync::mpsc::channel();
    let (event_tx, event_rx) = async_channel::bounded(1);

    display.set_command_sender(command_tx);

    let name = desktop.name.clone();
    std::thread::Builder::new()
        .name(format!("cabin-rdp-{name}"))
        .spawn(move || cabin_rdp::run_session(params, command_rx, event_tx))
        .expect("could not start the session thread");

    let display = display.clone();
    gtk::glib::spawn_future_local(async move {
        let mut showing = false;
        while let Ok(event) = event_rx.recv().await {
            match event {
                SessionEvent::Connected { width, height } => {
                    tracing::info!("session connected at {width}x{height}");
                }
                SessionEvent::Frame(frame) => {
                    // Switch on the first frame, not on Connected. A connected
                    // session with nothing drawn yet still looks broken.
                    if !showing {
                        showing = true;
                        on_state(State::Showing);
                    }
                    display.set_frame(frame);
                }
                SessionEvent::Resized { width, height } => {
                    tracing::debug!("guest resized to {width}x{height}");
                }
                SessionEvent::Disconnected { reason } => {
                    tracing::info!("session ended: {reason}");
                    on_state(State::Failed(reason));
                    break;
                }
                SessionEvent::Error(message) => {
                    tracing::error!("session failed: {message}");
                    on_state(State::Failed(message));
                    break;
                }
                SessionEvent::ClipboardOffer { .. } => {}
            }
        }
        // Stop sending input to a session that has gone.
        display.clear_command_sender();
    });
}
