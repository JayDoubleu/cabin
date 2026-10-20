//! Cabin: run Fedora GNOME desktops in containers.

mod app;
mod create;
mod display;
mod graphics;
mod guestdir;
mod images;
mod keymap;
mod netaddr;
mod network;
mod pinning;
mod recipe;
mod recipes;
mod remote;
mod runtime;
mod settings;
mod shell;
mod sidebar;
mod state;
mod viewer;
mod window;

use adw::prelude::*;

/// Set by the build system. Falls back so `cargo run` works during
/// development.
const APP_ID: &str = match option_env!("CABIN_APP_ID") {
    Some(id) => id,
    None => "io.github.jaydoubleu.Cabin",
};

fn main() -> gtk::glib::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "cabin=info".into()),
        )
        .init();

    // A second launch normally hands over to the running instance and exits
    // with no output. A developer who runs a new build beside the installed
    // one needs it to start on its own. docs/HOST-SETUP.md describes this.
    let mut flags = gtk::gio::ApplicationFlags::empty();
    if std::env::var_os("CABIN_DEV_INSTANCE").is_some() {
        flags |= gtk::gio::ApplicationFlags::NON_UNIQUE;
    }
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(flags)
        .build();

    app.connect_activate(app::activate);
    app.run()
}
