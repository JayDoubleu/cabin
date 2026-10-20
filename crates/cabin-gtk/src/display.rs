//! `CabinDisplay`: the widget that shows a guest desktop.
//!
//! Frames arrive from the session thread as packed BGRA and become a
//! `GdkMemoryTexture`. The widget letterboxes, so the guest keeps its aspect
//! ratio in any window shape.
//!
//! The resize debounce matters, because without it dragging a window edge
//! sends a resize request per frame and the guest re-lays out continuously.
//! See DESIGN.md section 6.4.

use cabin_rdp::session::wheel;
use cabin_rdp::Frame;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use std::cell::{Cell, RefCell};

/// How long the window size must settle before Cabin asks the guest to resize.
const RESIZE_DEBOUNCE_MS: u32 = 300;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct CabinDisplay {
        /// The most recent frame, ready to draw.
        pub texture: RefCell<Option<gtk::gdk::MemoryTexture>>,
        /// Sends commands to the session thread, when one is attached.
        pub commands: RefCell<Option<std::sync::mpsc::Sender<cabin_rdp::SessionCommand>>>,
        /// The guest framebuffer size. Zero until the first frame arrives.
        pub guest_size: Cell<(u32, u32)>,
        /// True when the guest should follow the window size.
        pub fit_to_window: Cell<bool>,
        pub pending_resize: RefCell<Option<glib::SourceId>>,
        /// The last size actually sent to the guest, so an identical request
        /// is not sent twice.
        pub last_requested: Cell<Option<(u32, u32)>>,
        /// Set when a fixed resolution is chosen instead of fit.
        pub fixed_size: Cell<Option<(u32, u32)>>,
        /// The keys the guest holds down, so they can be released when the
        /// display loses focus.
        pub held_keys: RefCell<crate::keymap::HeldKeys>,
        /// The part of a wheel unit not yet sent, for each axis.
        pub scroll_x: Cell<wheel::Accumulator>,
        pub scroll_y: Cell<wheel::Accumulator>,
        /// The window whose activation is watched, and the handler, so the
        /// handler can be removed when the widget leaves that window.
        pub active_watch: RefCell<Option<(glib::WeakRef<gtk::Window>, glib::SignalHandlerId)>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for CabinDisplay {
        const NAME: &'static str = "CabinDisplay";
        type Type = super::CabinDisplay;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for CabinDisplay {
        fn constructed(&self) {
            self.parent_constructed();
            self.fit_to_window.set(true);
            let widget = self.obj();
            // The widget must take focus, or key events go to the sidebar.
            widget.set_focusable(true);
            widget.set_can_focus(true);
            widget.attach_input_controllers();
        }

        fn dispose(&self) {
            if let Some(id) = self.pending_resize.take() {
                id.remove();
            }
        }
    }

    impl WidgetImpl for CabinDisplay {
        fn realize(&self) {
            self.parent_realize();
            self.obj().watch_window_activation();
        }

        fn unrealize(&self) {
            if let Some((window, id)) = self.active_watch.take() {
                if let Some(window) = window.upgrade() {
                    window.disconnect(id);
                }
            }
            self.parent_unrealize();
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            self.parent_size_allocate(width, height, baseline);
            if self.fit_to_window.get() {
                self.obj().schedule_resize(width, height);
            }
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let widget = self.obj();
            let width = widget.width() as f32;
            let height = widget.height() as f32;

            // The letterbox bars. Painting them every time avoids leaving
            // stale pixels when the aspect ratio changes.
            let bounds = gtk::graphene::Rect::new(0.0, 0.0, width, height);
            snapshot.append_color(&gtk::gdk::RGBA::new(0.09, 0.09, 0.11, 1.0), &bounds);

            let texture = self.texture.borrow();
            let Some(texture) = texture.as_ref() else {
                return;
            };

            let (guest_w, guest_h) = self.guest_size.get();
            if guest_w == 0 || guest_h == 0 {
                return;
            }

            // Keep the guest's aspect ratio and centre it.
            let scale = (width / guest_w as f32).min(height / guest_h as f32);
            let drawn_w = guest_w as f32 * scale;
            let drawn_h = guest_h as f32 * scale;
            let area = gtk::graphene::Rect::new(
                (width - drawn_w) / 2.0,
                (height - drawn_h) / 2.0,
                drawn_w,
                drawn_h,
            );
            snapshot.append_texture(texture, &area);
        }
    }
}

glib::wrapper! {
    pub struct CabinDisplay(ObjectSubclass<imp::CabinDisplay>)
        @extends gtk::Widget;
}

impl Default for CabinDisplay {
    fn default() -> Self {
        Self::new()
    }
}

impl CabinDisplay {
    pub fn new() -> Self {
        glib::Object::new()
    }

    /// Makes the guest follow the window size.
    pub fn set_fit_to_window(&self, fit: bool) {
        let imp = self.imp();
        imp.fit_to_window.set(fit);
        if fit {
            imp.fixed_size.set(None);
            self.schedule_resize(self.width(), self.height());
        }
    }

    /// Pins the guest to one resolution and stops following the window.
    pub fn set_fixed_resolution(&self, width: u32, height: u32) {
        let imp = self.imp();
        imp.fit_to_window.set(false);
        imp.fixed_size.set(Some((width, height)));
        self.cancel_pending_resize();
        self.request_guest_resize(width, height);
    }

    /// The fixed resolution in force, if the pane is not following the window.
    pub fn fixed_resolution(&self) -> Option<(u32, u32)> {
        self.imp().fixed_size.get()
    }

    pub fn guest_size(&self) -> (u32, u32) {
        self.imp().guest_size.get()
    }

    /// Attaches a session, so the widget can send input and resize requests.
    ///
    /// A new session starts at the size the desktop was created with, which is
    /// almost never the size of this widget. So the widget states its size at
    /// once instead of waiting for the user to drag a window edge. The session
    /// holds the request until the Display Control channel attaches.
    pub fn set_command_sender(&self, sender: std::sync::mpsc::Sender<cabin_rdp::SessionCommand>) {
        let imp = self.imp();
        imp.commands.replace(Some(sender));
        // The previous session's beliefs do not apply to this one.
        imp.last_requested.set(None);
        self.request_current_size();
    }

    /// Detaches the session, so no further input is sent.
    pub fn clear_command_sender(&self) {
        // Release first, while the sender still exists. A new session then
        // starts with no keys down, as the guest does.
        self.release_held_keys();
        let imp = self.imp();
        imp.commands.replace(None);
        imp.last_requested.set(None);
        imp.scroll_x.set(wheel::Accumulator::default());
        imp.scroll_y.set(wheel::Accumulator::default());
    }

    /// Ends the session cleanly, so its thread stops rather than being left to
    /// discover the disconnection on its own.
    pub fn disconnect_session(&self) {
        self.cancel_pending_resize();
        // Before the Disconnect, or the session ends without them.
        self.release_held_keys();
        self.send(cabin_rdp::SessionCommand::Disconnect);
        self.clear_command_sender();
        self.imp().texture.replace(None);
        self.imp().guest_size.set((0, 0));
        self.queue_draw();
    }

    /// Asks the guest to match the size this widget already has.
    fn request_current_size(&self) {
        let imp = self.imp();
        let (width, height) = match imp.fixed_size.get() {
            Some(size) => size,
            None if imp.fit_to_window.get() => (self.width() as u32, self.height() as u32),
            None => return,
        };
        if width == 0 || height == 0 {
            // Not allocated yet. size_allocate will schedule one.
            return;
        }
        self.request_guest_resize(width, height);
    }

    /// Shows a new frame.
    ///
    /// The frame is packed BGRA with no row padding, which is what
    /// `GdkMemoryTexture` expects when given a stride of width * 4.
    pub fn set_frame(&self, frame: Frame) {
        debug_assert!(
            frame.is_consistent(),
            "frame size does not match its buffer"
        );

        let imp = self.imp();
        let bytes = gtk::glib::Bytes::from_owned(frame.data);
        let texture = gtk::gdk::MemoryTexture::new(
            frame.width as i32,
            frame.height as i32,
            gtk::gdk::MemoryFormat::B8g8r8a8,
            &bytes,
            frame.width as usize * 4,
        );

        let previous = imp.guest_size.replace((frame.width, frame.height));
        imp.texture.replace(Some(texture));

        // A guest-driven size change means the fit request was granted, or the
        // guest resized itself. Either way the letterbox needs recomputing.
        if previous != (frame.width, frame.height) {
            self.queue_resize();
        }
        self.queue_draw();
    }

    /// Attaches the keyboard, pointer and scroll controllers.
    ///
    /// Every handler converts to guest coordinates first and drops the event
    /// when the pointer is over a letterbox bar, so the guest never sees a
    /// click that came from outside its screen.
    fn attach_input_controllers(&self) {
        let keys = gtk::EventControllerKey::new();
        let widget = self.clone();
        keys.connect_key_pressed(move |_, _, keycode, _| {
            widget.send_key(keycode as u16, true);
            gtk::glib::Propagation::Stop
        });
        let widget = self.clone();
        keys.connect_key_released(move |_, _, keycode, _| {
            widget.send_key(keycode as u16, false);
        });
        self.add_controller(keys);

        // GTK sends no release for a key lifted after focus leaves, so the
        // guest would keep it held. Release everything on the way out.
        let focus = gtk::EventControllerFocus::new();
        let widget = self.clone();
        focus.connect_leave(move |_| widget.release_held_keys());
        self.add_controller(focus);

        let motion = gtk::EventControllerMotion::new();
        let widget = self.clone();
        motion.connect_motion(move |_, x, y| {
            if let Some((gx, gy)) = widget.widget_to_guest(x, y) {
                widget.send(cabin_rdp::SessionCommand::PointerMove { x: gx, y: gy });
            }
        });
        self.add_controller(motion);

        let click = gtk::GestureClick::new();
        // 0 means "any button", so middle and right arrive too.
        click.set_button(0);
        let widget = self.clone();
        click.connect_pressed(move |gesture, _, x, y| {
            widget.grab_focus();
            widget.send_button(gesture.current_button() as u8, x, y, true);
        });
        let widget = self.clone();
        click.connect_released(move |gesture, _, x, y| {
            widget.send_button(gesture.current_button() as u8, x, y, false);
        });
        self.add_controller(click);

        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
        let widget = self.clone();
        scroll.connect_scroll(move |controller, dx, dy| {
            // A mouse wheel reports notches and a touchpad reports pixels, so
            // the unit decides the scale. See cabin_rdp::session::wheel.
            let unit = match controller.unit() {
                gtk::gdk::ScrollUnit::Surface => wheel::Unit::Pixels,
                _ => wheel::Unit::Wheel,
            };
            let imp = widget.imp();
            let mut x = imp.scroll_x.get();
            let mut y = imp.scroll_y.get();
            // RDP's vertical sign is the opposite of GTK's.
            let horizontal = x.feed(dx, unit);
            let vertical = y.feed(-dy, unit);
            imp.scroll_x.set(x);
            imp.scroll_y.set(y);
            if horizontal != 0 || vertical != 0 {
                widget.send(cabin_rdp::SessionCommand::PointerScroll {
                    horizontal,
                    vertical,
                });
            }
            gtk::glib::Propagation::Stop
        });
        self.add_controller(scroll);
    }

    fn send_key(&self, hardware_keycode: u16, pressed: bool) {
        // An unmapped key is dropped rather than guessed at, so the guest never
        // receives a scancode that means something else.
        let Some((scancode, extended)) = crate::keymap::to_scancode(hardware_keycode) else {
            tracing::trace!("no scancode for keycode {hardware_keycode}");
            return;
        };
        if self.send(cabin_rdp::SessionCommand::KeyPress {
            scancode,
            extended,
            pressed,
        }) {
            self.imp()
                .held_keys
                .borrow_mut()
                .record(scancode, extended, pressed);
        }
    }

    /// Releases in the guest every key it believes is held.
    fn release_held_keys(&self) {
        let keys = self.imp().held_keys.borrow_mut().take_all();
        for (scancode, extended) in keys {
            self.send(cabin_rdp::SessionCommand::KeyPress {
                scancode,
                extended,
                pressed: false,
            });
        }
    }

    /// Releases held keys when the window stops being the active one.
    ///
    /// Focus leave alone is not enough. When the whole window loses
    /// activation, for example on Alt+Tab, the focus widget inside it may
    /// not get a leave event.
    fn watch_window_activation(&self) {
        let imp = self.imp();
        if let Some((window, id)) = imp.active_watch.take() {
            if let Some(window) = window.upgrade() {
                window.disconnect(id);
            }
        }
        let Some(window) = self.root().and_downcast::<gtk::Window>() else {
            return;
        };
        let weak = self.downgrade();
        let id = window.connect_is_active_notify(move |window| {
            if window.is_active() {
                return;
            }
            if let Some(widget) = weak.upgrade() {
                widget.release_held_keys();
            }
        });
        imp.active_watch.replace(Some((window.downgrade(), id)));
    }

    fn send_button(&self, button: u8, x: f64, y: f64, pressed: bool) {
        let Some((gx, gy)) = self.widget_to_guest(x, y) else {
            return;
        };
        self.send(cabin_rdp::SessionCommand::PointerButton {
            x: gx,
            y: gy,
            button,
            pressed,
        });
    }

    /// Returns false when no session is attached, or the session has gone.
    fn send(&self, command: cabin_rdp::SessionCommand) -> bool {
        match self.imp().commands.borrow().as_ref() {
            Some(sender) => sender.send(command).is_ok(),
            None => false,
        }
    }

    fn cancel_pending_resize(&self) {
        if let Some(id) = self.imp().pending_resize.take() {
            id.remove();
        }
    }

    /// Starts or restarts the debounce timer.
    fn schedule_resize(&self, width: i32, height: i32) {
        if width <= 0 || height <= 0 {
            return;
        }
        self.cancel_pending_resize();
        let widget = self.clone();
        let id = glib::timeout_add_local_once(
            std::time::Duration::from_millis(RESIZE_DEBOUNCE_MS as u64),
            move || {
                widget.imp().pending_resize.replace(None);
                widget.request_guest_resize(width as u32, height as u32);
            },
        );
        self.imp().pending_resize.replace(Some(id));
    }

    fn request_guest_resize(&self, width: u32, height: u32) {
        let (width, height) = cabin_rdp::session::clamp_resolution(width, height);
        let imp = self.imp();

        // One window drag produces several size-allocate calls, so the debounce
        // can expire more than once on the same final size. Sending it twice
        // costs a Display Control round trip and a guest re-layout for nothing.
        if !should_send_resize(imp.last_requested.get(), (width, height)) {
            return;
        }

        // The guest rounds odd sizes down and may grant something else, so the
        // granted size is read back from the next frame rather than assumed.
        if !self.send(cabin_rdp::SessionCommand::Resize { width, height }) {
            // No session took it. Recording it anyway would suppress the
            // repeat, and the desktop would never reach this size.
            return;
        }
        imp.last_requested.set(Some((width, height)));
        tracing::debug!("guest resize requested: {width}x{height}");
    }

    /// Maps a point in widget space to guest framebuffer space.
    ///
    /// The widget letterboxes, so the transform removes the offset before it
    /// scales. Returns `None` when the point falls in the letterbox bars.
    pub fn widget_to_guest(&self, x: f64, y: f64) -> Option<(u32, u32)> {
        let (guest_w, guest_h) = self.guest_size();
        if guest_w == 0 || guest_h == 0 {
            return None;
        }
        letterbox_map(
            x,
            y,
            self.width() as f64,
            self.height() as f64,
            guest_w,
            guest_h,
        )
    }
}

/// True when a resize request is worth sending.
///
/// Only an exact repeat of the previous request is suppressed. Returning to an
/// earlier size after visiting another one is still sent, because the guest may
/// have been left somewhere else.
pub fn should_send_resize(last: Option<(u32, u32)>, next: (u32, u32)) -> bool {
    last != Some(next)
}

/// Pure geometry, split out so it can be tested with no widget.
pub fn letterbox_map(
    x: f64,
    y: f64,
    widget_w: f64,
    widget_h: f64,
    guest_w: u32,
    guest_h: u32,
) -> Option<(u32, u32)> {
    if widget_w <= 0.0 || widget_h <= 0.0 || guest_w == 0 || guest_h == 0 {
        return None;
    }
    let scale = (widget_w / guest_w as f64).min(widget_h / guest_h as f64);
    let drawn_w = guest_w as f64 * scale;
    let drawn_h = guest_h as f64 * scale;
    let offset_x = (widget_w - drawn_w) / 2.0;
    let offset_y = (widget_h - drawn_h) / 2.0;

    let gx = (x - offset_x) / scale;
    let gy = (y - offset_y) / scale;
    if gx < 0.0 || gy < 0.0 || gx >= guest_w as f64 || gy >= guest_h as f64 {
        return None;
    }
    Some((gx as u32, gy as u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_centre_to_centre() {
        // 16:9 guest in a 16:9 widget: no letterbox.
        let p = letterbox_map(960.0, 540.0, 1920.0, 1080.0, 1920, 1080).unwrap();
        assert_eq!(p, (960, 540));
    }

    #[test]
    fn accounts_for_horizontal_bars() {
        // 16:9 guest in a 4:3 widget: bars top and bottom.
        // scale = min(1600/1600, 1200/900) = 1.0, drawn 1600x900, offset_y=150.
        let p = letterbox_map(800.0, 600.0, 1600.0, 1200.0, 1600, 900).unwrap();
        assert_eq!(p, (800, 450));
    }

    #[test]
    fn rejects_points_in_the_bars() {
        assert!(letterbox_map(800.0, 10.0, 1600.0, 1200.0, 1600, 900).is_none());
    }

    #[test]
    fn rejects_before_the_first_frame() {
        assert!(letterbox_map(1.0, 1.0, 100.0, 100.0, 0, 0).is_none());
    }

    #[test]
    fn suppresses_an_identical_repeat_request() {
        assert!(!should_send_resize(Some((1380, 604)), (1380, 604)));
    }

    #[test]
    fn sends_the_first_request() {
        assert!(should_send_resize(None, (1920, 1080)));
    }

    #[test]
    fn sends_a_changed_size() {
        assert!(should_send_resize(Some((1380, 604)), (1130, 804)));
    }

    #[test]
    fn sends_a_return_to_an_earlier_size() {
        // A then B then A must reach the guest, because after B the guest is
        // no longer at A.
        let mut last = None;
        for size in [(800, 600), (1024, 768), (800, 600)] {
            assert!(should_send_resize(last, size), "{size:?} should be sent");
            last = Some(size);
        }
    }

    #[test]
    fn scales_down_a_large_guest() {
        // 1920x1080 guest shown in a 960x540 widget.
        let p = letterbox_map(480.0, 270.0, 960.0, 540.0, 1920, 1080).unwrap();
        assert_eq!(p, (960, 540));
    }
}
