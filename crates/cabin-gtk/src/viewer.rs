//! The viewer pane: it shows a desktop, or it explains why it does not.
//!
//! The pane owns what it is showing, rather than being written to from
//! several places. That matters because sessions outlive the moment they are
//! replaced: a session thread that fails after the user has moved on would
//! otherwise put its own error over whatever took its place. Every session
//! carries a generation, and a session that is no longer the current one is
//! ignored. See DESIGN.md section 16.1.

use crate::display::CabinDisplay;
use adw::prelude::*;
use cabin_core::Desktop;
use cabin_rdp::{ConnectionParams, Credentials, SessionEvent};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

const PAGE_EMPTY: &str = "empty";
const PAGE_DESKTOP: &str = "desktop";

/// Asks the window to act on a desktop. The window owns the Podman handle,
/// so the pane routes these back rather than doing the work itself.
type DesktopCallback = RefCell<Option<Box<dyn Fn(Desktop)>>>;

/// What the pane is currently showing.
struct Shown {
    desktop: Desktop,
    /// True while a session thread is running for this desktop.
    live: bool,
}

/// The viewer pane and everything it is showing.
pub struct Viewer {
    /// The pane itself, for the split view to hold.
    pub content: adw::ToolbarView,
    /// Shows and hides the desktop list. The window binds it to the split
    /// view, so the pane does not need to know how the window is laid out.
    pub sidebar_toggle: gtk::ToggleButton,
    display: CabinDisplay,
    stack: gtk::Stack,
    placeholder: adw::StatusPage,
    /// The action offered under the placeholder text, when there is one.
    action: gtk::Button,
    /// The handler currently connected to `action`, so it can be replaced.
    action_handler: Cell<Option<gtk::glib::SignalHandlerId>>,
    /// Opens the settings for whatever is shown. Insensitive when nothing is.
    settings_button: gtk::Button,
    shown: RefCell<Option<Shown>>,
    /// Increments on every attach and detach, so a session can tell whether it
    /// is still the one on screen.
    generation: Cell<u64>,
    on_open: DesktopCallback,
    on_settings: DesktopCallback,
}

/// The fixed sizes offered in the display menu.
const RESOLUTIONS: &[(u32, u32)] = &[(1280, 720), (1920, 1080), (2560, 1440), (3840, 2160)];

impl Viewer {
    pub fn new() -> Rc<Self> {
        let display = CabinDisplay::new();
        display.set_hexpand(true);
        display.set_vexpand(true);

        let action = gtk::Button::builder()
            .halign(gtk::Align::Center)
            .css_classes(vec!["pill", "suggested-action"])
            .visible(false)
            .build();

        let placeholder = adw::StatusPage::builder()
            .icon_name("video-display-symbolic")
            .title("No desktop connected")
            .description("Choose a desktop from the sidebar to connect to it.")
            .child(&action)
            .build();

        let stack = gtk::Stack::new();
        stack.add_named(&placeholder, Some(PAGE_EMPTY));
        stack.add_named(&display, Some(PAGE_DESKTOP));
        stack.set_visible_child_name(PAGE_EMPTY);

        let header = adw::HeaderBar::new();
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&stack));

        let sidebar_toggle = gtk::ToggleButton::builder()
            .icon_name("sidebar-show-symbolic")
            // Matches active(true). The window updates it on notify::active,
            // and setting the same value again emits no notify.
            .tooltip_text("Hide the desktop list")
            .active(true)
            .build();
        header.pack_start(&sidebar_toggle);

        let settings_button = gtk::Button::builder()
            .icon_name("emblem-system-symbolic")
            .tooltip_text("Settings for this desktop")
            .sensitive(false)
            .build();

        let viewer = Rc::new(Self {
            content: toolbar,
            sidebar_toggle,
            display,
            stack,
            placeholder,
            action,
            action_handler: Cell::new(None),
            settings_button,
            shown: RefCell::new(None),
            generation: Cell::new(0),
            on_open: RefCell::new(None),
            on_settings: RefCell::new(None),
        });
        viewer.build_header(&header);
        viewer
    }

    /// Fills the pane's header bar.
    ///
    /// Settings live here as well as in the sidebar row menu. A per-row
    /// overflow menu is easy to miss, and every comparable application puts
    /// the settings for the machine you are looking at above the machine.
    fn build_header(self: &Rc<Self>, header: &adw::HeaderBar) {
        let weak = Rc::downgrade(self);
        self.settings_button.connect_clicked(move |_| {
            let Some(viewer) = weak.upgrade() else { return };
            let Some(desktop) = viewer.shown_desktop() else {
                return;
            };
            let callback = viewer.on_settings.borrow();
            if let Some(callback) = callback.as_ref() {
                callback(desktop);
            }
        });
        header.pack_end(&self.settings_button);
        header.pack_end(&self.build_display_menu());
        header.pack_end(&fullscreen_button());
    }

    /// Opens the settings for the desktop the pane is showing.
    pub fn connect_settings(&self, callback: impl Fn(Desktop) + 'static) {
        *self.on_settings.borrow_mut() = Some(Box::new(callback));
    }

    fn build_display_menu(&self) -> gtk::MenuButton {
        let popover = gtk::Popover::new();
        let box_ = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .margin_top(8)
            .margin_bottom(8)
            .margin_start(8)
            .margin_end(8)
            .build();

        let state = crate::state::State::new(crate::APP_ID);

        // Choosing a resolution larger than the pane letterboxes it, which
        // makes the desktop smaller rather than larger and reads as the
        // setting having done nothing. Offer to bring the window with it.
        let match_window = gtk::CheckButton::with_label("Resize window to match");
        match_window.set_active(state.resize_window_to_resolution());
        match_window.set_tooltip_text(Some(
            "Make the Cabin window the size it needs to show the resolution without scaling.",
        ));

        let fit = gtk::CheckButton::with_label("Fit to window");
        fit.set_active(true);
        box_.append(&fit);
        box_.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

        for &(width, height) in RESOLUTIONS {
            let button = gtk::CheckButton::with_label(&format!("{width} x {height}"));
            button.set_group(Some(&fit));
            let display = self.display.clone();
            let match_window = match_window.clone();
            button.connect_toggled(move |b| {
                if !b.is_active() {
                    return;
                }
                display.set_fixed_resolution(width, height);
                if match_window.is_active() {
                    resize_window_for(&display, width, height);
                }
            });
            box_.append(&button);
        }

        box_.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        box_.append(&match_window);

        let display_for_match = self.display.clone();
        match_window.connect_toggled(move |b| {
            crate::state::State::new(crate::APP_ID).set_resize_window_to_resolution(b.is_active());
            // Act on it at once when a fixed resolution is already chosen,
            // rather than waiting for the user to pick the same one again.
            if b.is_active() {
                if let Some((width, height)) = display_for_match.fixed_resolution() {
                    resize_window_for(&display_for_match, width, height);
                }
            }
        });

        let display = self.display.clone();
        fit.connect_toggled(move |b| {
            if b.is_active() {
                display.set_fit_to_window(true);
            }
        });

        popover.set_child(Some(&box_));
        gtk::MenuButton::builder()
            .icon_name("video-display-symbolic")
            .tooltip_text("Display")
            .popover(&popover)
            .build()
    }

    /// Called when the pane needs a desktop opened, which needs its password.
    pub fn connect_open(&self, callback: impl Fn(Desktop) + 'static) {
        *self.on_open.borrow_mut() = Some(Box::new(callback));
    }

    /// The desktop the pane is showing, connected or not.
    pub fn shown_desktop(&self) -> Option<Desktop> {
        self.shown.borrow().as_ref().map(|s| s.desktop.clone())
    }

    /// True when a live session is attached for `id`.
    pub fn is_connected_to(&self, id: &str) -> bool {
        self.shown
            .borrow()
            .as_ref()
            .is_some_and(|s| s.live && s.desktop.id == id)
    }

    /// Ends any session and returns the pane to its opening state.
    pub fn clear(&self) {
        self.detach();
        *self.shown.borrow_mut() = None;
        self.set_placeholder(
            "video-display-symbolic",
            "No desktop connected",
            "Choose a desktop from the sidebar to connect to it.",
            None,
        );
    }

    /// The pane when no desktop exists at all.
    pub fn show_no_desktops(&self) {
        self.detach();
        *self.shown.borrow_mut() = None;
        self.set_placeholder(
            "computer-symbolic",
            "No desktops",
            "Create a desktop to get started.",
            None,
        );
    }

    /// The pane for a desktop that is not running.
    pub fn show_stopped(self: &Rc<Self>, desktop: &Desktop) {
        self.detach();
        *self.shown.borrow_mut() = Some(Shown {
            desktop: desktop.clone(),
            live: false,
        });
        self.set_placeholder(
            "media-playback-stop-symbolic",
            &format!("{} is not running", desktop.name),
            "Start it from the sidebar to connect.",
            None,
        );
    }

    /// The pane for a session that ended or never started. Offers to retry,
    /// because the usual cause is a guest that is still booting.
    fn show_failure(self: &Rc<Self>, desktop: &Desktop, reason: &str) {
        self.set_placeholder(
            "dialog-warning-symbolic",
            &format!("{} disconnected", desktop.name),
            reason,
            Some("Reconnect"),
        );
        let weak = Rc::downgrade(self);
        let desktop = desktop.clone();
        self.set_action(move || {
            if let Some(viewer) = weak.upgrade() {
                viewer.open(desktop.clone());
            }
        });
    }

    /// The pane for a desktop whose certificate is not the recorded one.
    ///
    /// Reconnecting would only fail again, because it checks the same pin.
    /// So the pane offers to trust the new certificate instead, and asks
    /// first. The session never does this by itself.
    fn show_certificate_changed(self: &Rc<Self>, desktop: &Desktop, seen: &str) {
        self.set_placeholder(
            "security-low-symbolic",
            &format!("{} presented a new certificate", desktop.name),
            "This desktop presented a different security certificate than last \
             time. That is expected if the desktop was rebuilt from scratch. If it \
             was not, something may be intercepting the connection.",
            Some("Trust New Certificate"),
        );
        let weak = Rc::downgrade(self);
        let desktop = desktop.clone();
        let seen = seen.to_string();
        self.set_action(move || {
            if let Some(viewer) = weak.upgrade() {
                viewer.confirm_new_certificate(&desktop, &seen);
            }
        });
    }

    /// Asks before trusting a changed certificate, then reconnects.
    fn confirm_new_certificate(self: &Rc<Self>, desktop: &Desktop, seen: &str) {
        let dialog = adw::AlertDialog::builder()
            .heading("Trust the New Certificate?")
            .body(format!(
                "Trust it only if you know why {} changed, for example because \
                 it was rebuilt.\n\nIt now presents:\n{seen}",
                desktop.name
            ))
            .build();
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("trust", "Trust");
        dialog.set_response_appearance("trust", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");

        let weak = Rc::downgrade(self);
        let desktop = desktop.clone();
        let seen = seen.to_string();
        dialog.connect_response(None, move |_, response| {
            if response != "trust" {
                return;
            }
            let Some(viewer) = weak.upgrade() else {
                return;
            };
            // Record the fingerprint the user saw, rather than forgetting the
            // pin. Forgetting would accept whatever the next connection
            // presents, which need not be the certificate shown here.
            tracing::info!("trusting a new certificate for {}", desktop.name);
            crate::pinning::record(&desktop.name, &seen);
            viewer.open(desktop.clone());
        });
        dialog.present(Some(&self.content));
    }

    fn set_action(&self, on_click: impl Fn() + 'static) {
        // A button keeps every handler ever connected, and this button is
        // reused, so an old handler would reopen a stale desktop.
        if let Some(id) = self.action_handler.take() {
            self.action.disconnect(id);
        }
        let id = self.action.connect_clicked(move |_| on_click());
        self.action_handler.set(Some(id));
    }

    /// Routes an open request back to the window.
    fn open(self: &Rc<Self>, desktop: Desktop) {
        if let Some(callback) = self.on_open.borrow().as_ref() {
            callback(desktop);
        }
    }

    /// Reconciles the pane with the desktops Podman reports.
    ///
    /// This is what makes a deleted or stopped desktop clear the pane. Without
    /// it the pane keeps whatever it last showed, so deleting the desktop
    /// leaves its disconnection notice behind with nothing to explain it.
    pub fn reconcile(self: &Rc<Self>, desktops: &[Desktop]) {
        let Some(shown) = self.shown.borrow().as_ref().map(|s| s.desktop.clone()) else {
            // Nothing shown. Say so in the way that fits the list.
            if desktops.is_empty() {
                self.show_no_desktops();
            }
            return;
        };

        let Some(current) = desktops.iter().find(|d| d.id == shown.id) else {
            // The desktop was deleted, or recreated under a new id.
            if let Some(replacement) = desktops.iter().find(|d| d.name == shown.name) {
                let replacement = replacement.clone();
                self.detach();
                self.open(replacement);
                return;
            }
            if desktops.is_empty() {
                self.show_no_desktops();
            } else {
                self.clear();
            }
            return;
        };

        if !current.state.is_running() {
            self.show_stopped(current);
            return;
        }

        // Running again after being stopped. Reconnect on that edge only, so
        // a session that failed for its own reasons is not retried in a loop.
        let was_stopped = !self
            .shown
            .borrow()
            .as_ref()
            .is_some_and(|s| s.desktop.state.is_running());
        if was_stopped {
            // Record the running state first. Opening fetches the password
            // before it attaches, and starting a desktop sends several Podman
            // events, so a later list would otherwise see "stopped" again and
            // open the same desktop a second time.
            *self.shown.borrow_mut() = Some(Shown {
                desktop: current.clone(),
                live: false,
            });
            self.open(current.clone());
        }
    }

    /// Connects to `desktop` and draws it.
    pub fn attach(self: &Rc<Self>, desktop: &Desktop, password: String) {
        let Some(port) = desktop.rdp_port else {
            // Detach first, so the session this replaces can no longer write
            // to the pane, and the pane names the desktop it now shows.
            self.detach();
            *self.shown.borrow_mut() = Some(Shown {
                desktop: desktop.clone(),
                live: false,
            });
            self.show_failure(desktop, "The desktop publishes no RDP port.");
            return;
        };

        self.detach();
        let generation = self.generation.get();
        *self.shown.borrow_mut() = Some(Shown {
            desktop: desktop.clone(),
            live: true,
        });
        self.set_placeholder(
            "content-loading-symbolic",
            &format!("Connecting to {}", desktop.name),
            "Starting the remote desktop session.",
            None,
        );

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
            // None on a first connection, which records whatever is presented.
            expected_fingerprint: crate::pinning::recorded(&desktop.name),
        };

        let (command_tx, command_rx) = std::sync::mpsc::channel();
        // One frame at most waits for the interface. A stale frame has no
        // value, so the session should wait rather than build a backlog.
        let (event_tx, event_rx) = async_channel::bounded(1);

        let name = desktop.name.clone();
        std::thread::Builder::new()
            .name(format!("cabin-rdp-{name}"))
            .spawn(move || cabin_rdp::run_session(params, command_rx, event_tx))
            .expect("could not start the session thread");

        // Attaching the sender also states the pane's size, so the desktop
        // fits the window on its first connection rather than only after the
        // user resizes it.
        self.display.set_command_sender(command_tx);

        let weak = Rc::downgrade(self);
        let desktop = desktop.clone();
        gtk::glib::spawn_future_local(async move {
            let mut showing = false;
            while let Ok(event) = event_rx.recv().await {
                let Some(viewer) = weak.upgrade() else { return };
                // A session that is no longer on screen must not write to it.
                if viewer.generation.get() != generation {
                    return;
                }
                match event {
                    SessionEvent::Connecting { attempt, waited } => {
                        // Say nothing for the first couple of seconds. A
                        // desktop that starts promptly should not flash a
                        // message about being slow.
                        if waited >= 2 {
                            viewer.set_placeholder(
                                "content-loading-symbolic",
                                &format!("Starting {}", desktop.name),
                                "The desktop is still coming up. This takes a \
                                 moment on the first start.",
                                None,
                            );
                        }
                        tracing::debug!("connect attempt {attempt} after {waited}s");
                    }
                    SessionEvent::Connected {
                        width,
                        height,
                        fingerprint,
                    } => {
                        tracing::info!("session connected at {width}x{height}");
                        // Record on the first connection, so every later one
                        // is checked against it.
                        if let Some(fingerprint) = fingerprint {
                            if crate::pinning::recorded(&desktop.name).is_none() {
                                tracing::info!("recording certificate for {}", desktop.name);
                                crate::pinning::record(&desktop.name, &fingerprint);
                            }
                        }
                    }
                    SessionEvent::Frame(frame) => {
                        // Switch on the first frame, not on Connected. A
                        // connected session with nothing drawn yet still looks
                        // broken.
                        if !showing {
                            showing = true;
                            viewer.stack.set_visible_child_name(PAGE_DESKTOP);
                            viewer.display.grab_focus();
                        }
                        viewer.display.set_frame(frame);
                    }
                    SessionEvent::Resized { width, height } => {
                        tracing::debug!("guest resized to {width}x{height}");
                    }
                    SessionEvent::Disconnected { reason } => {
                        tracing::info!("session ended: {reason}");
                        viewer.session_ended(&desktop, &reason);
                        return;
                    }
                    SessionEvent::CertificateChanged { seen } => {
                        tracing::warn!("{} presented a new certificate", desktop.name);
                        viewer.mark_ended();
                        viewer.show_certificate_changed(&desktop, &seen);
                        return;
                    }
                    SessionEvent::Error(message) => {
                        tracing::error!("session failed: {message}");
                        viewer.session_ended(&desktop, &message);
                        return;
                    }
                    SessionEvent::ClipboardOffer { .. } => {}
                }
            }
        });
    }

    /// Records that the session for `desktop` has gone, and says why.
    fn session_ended(self: &Rc<Self>, desktop: &Desktop, reason: &str) {
        self.mark_ended();
        self.show_failure(desktop, reason);
    }

    /// Records that the session on screen has stopped running.
    fn mark_ended(&self) {
        if let Some(shown) = self.shown.borrow_mut().as_mut() {
            shown.live = false;
        }
        self.display.clear_command_sender();
    }

    /// Ends the current session, if there is one, and invalidates it.
    fn detach(&self) {
        // Every session compares against this, so bumping it first means the
        // one being replaced can no longer touch the pane.
        self.generation.set(self.generation.get() + 1);
        self.display.disconnect_session();
    }

    /// Keeps the header in step with what the pane shows.
    fn update_header(&self) {
        self.settings_button
            .set_sensitive(self.shown.borrow().is_some());
    }

    fn set_placeholder(&self, icon: &str, title: &str, description: &str, action: Option<&str>) {
        self.update_header();
        self.placeholder.set_icon_name(Some(icon));
        self.placeholder.set_title(title);
        self.placeholder.set_description(Some(description));
        match action {
            Some(label) => {
                self.action.set_label(label);
                self.action.set_visible(true);
            }
            None => self.action.set_visible(false),
        }
        self.stack.set_visible_child_name(PAGE_EMPTY);
    }
}

fn fullscreen_button() -> gtk::Button {
    let button = gtk::Button::builder()
        .icon_name("view-fullscreen-symbolic")
        .tooltip_text("Fullscreen")
        .build();
    button.connect_clicked(|b| {
        if let Some(window) = b.root().and_downcast::<gtk::Window>() {
            window.set_fullscreened(!window.is_fullscreen());
        }
    });
    button
}

/// Resizes the window so the pane is exactly `width` by `height`.
///
/// The difference between the window and the pane is measured rather than
/// assumed, because it depends on the header bar, whether the desktop list is
/// showing, and how wide it is.
fn resize_window_for(display: &CabinDisplay, width: u32, height: u32) {
    let Some(window) = display.root().and_downcast::<gtk::Window>() else {
        return;
    };
    if display.width() <= 0 || display.height() <= 0 {
        // Not laid out yet, so there is no chrome to measure.
        return;
    }

    let chrome_w = window.width() - display.width();
    let chrome_h = window.height() - display.height();
    let mut wanted_w = width as i32 + chrome_w;
    let mut wanted_h = height as i32 + chrome_h;

    // A window larger than the screen cannot be shown, and asking for one
    // leaves it wedged against the edges at a size nobody chose.
    if let Some((max_w, max_h)) = work_area(&window) {
        wanted_w = wanted_w.min(max_w);
        wanted_h = wanted_h.min(max_h);
        if wanted_w < width as i32 + chrome_w || wanted_h < height as i32 + chrome_h {
            tracing::info!("{width}x{height} does not fit this screen, so the pane will scale");
        }
    }

    // A maximised or fullscreen window ignores a size request, so leave those
    // states first. Otherwise the setting silently does nothing.
    window.unfullscreen();
    window.unmaximize();
    window.set_default_size(wanted_w, wanted_h);
}

/// The usable area of the monitor the window is on.
fn work_area(window: &gtk::Window) -> Option<(i32, i32)> {
    let surface = window.surface()?;
    let display = WidgetExt::display(window);
    let monitor = display.monitor_at_surface(&surface)?;
    let area = monitor.geometry();
    Some((area.width(), area.height()))
}
