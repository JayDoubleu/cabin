//! Remembers window geometry and which desktop was last in use.
//!
//! The keys are declared in the gschema. Reading them is what makes the
//! application feel like it was here before.

use gtk::gio;
use gtk::prelude::*;

/// Reads and writes the application's own settings.
pub struct State {
    settings: gio::Settings,
}

impl State {
    pub fn new(app_id: &str) -> Self {
        Self {
            settings: gio::Settings::new(app_id),
        }
    }

    /// Restores the window to the size it had when it was last closed.
    pub fn restore_window(&self, window: &impl IsA<gtk::Window>) {
        let window = window.as_ref();
        window.set_default_size(
            self.settings.int("window-width"),
            self.settings.int("window-height"),
        );
        if self.settings.boolean("window-maximized") {
            window.maximize();
        }
    }

    /// Saves the window geometry. Call this as the window closes.
    ///
    /// The maximised size is not recorded, or unmaximising after a restart
    /// would restore a window the size of the screen.
    pub fn save_window(&self, window: &impl IsA<gtk::Window>) {
        let window = window.as_ref();
        let maximised = window.is_maximized();
        if let Err(e) = self.settings.set_boolean("window-maximized", maximised) {
            tracing::warn!("could not save the window state: {e}");
            return;
        }
        if maximised {
            return;
        }
        let _ = self.settings.set_int("window-width", window.width());
        let _ = self.settings.set_int("window-height", window.height());
    }

    /// The desktop that was open when Cabin last closed, if any.
    pub fn last_desktop(&self) -> Option<String> {
        let name = self.settings.string("last-desktop").to_string();
        (!name.is_empty()).then_some(name)
    }

    pub fn set_last_desktop(&self, name: &str) {
        let _ = self.settings.set_string("last-desktop", name);
    }

    /// The image a new desktop starts from.
    pub fn default_image(&self) -> String {
        self.settings.string("default-image").to_string()
    }
}
