//! `CabinDisplay`: the widget that shows a guest desktop.
//!
//! Frames arrive from the session thread as packed BGRA and become a
//! `GdkMemoryTexture`. The widget letterboxes, so the guest keeps its aspect
//! ratio in any window shape.
//!
//! The resize debounce matters, because without it dragging a window edge
//! sends a resize request per frame and the guest re-lays out continuously.
//! See DESIGN.md section 6.4.

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
        /// Set when a fixed resolution is chosen instead of fit.
        pub fixed_size: Cell<Option<(u32, u32)>>,
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
            self.obj().set_focusable(true);
        }

        fn dispose(&self) {
            if let Some(id) = self.pending_resize.take() {
                id.remove();
            }
        }
    }

    impl WidgetImpl for CabinDisplay {
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

    pub fn guest_size(&self) -> (u32, u32) {
        self.imp().guest_size.get()
    }

    /// Attaches a session, so the widget can send input and resize requests.
    pub fn set_command_sender(&self, sender: std::sync::mpsc::Sender<cabin_rdp::SessionCommand>) {
        self.imp().commands.replace(Some(sender));
    }

    /// Detaches the session, so no further input is sent.
    pub fn clear_command_sender(&self) {
        self.imp().commands.replace(None);
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

    fn send(&self, command: cabin_rdp::SessionCommand) {
        if let Some(sender) = self.imp().commands.borrow().as_ref() {
            let _ = sender.send(command);
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
        tracing::debug!("guest resize requested: {width}x{height}");
        // The guest rounds odd sizes down and may grant something else, so the
        // granted size is read back from the next frame rather than assumed.
        self.send(cabin_rdp::SessionCommand::Resize { width, height });
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
    fn scales_down_a_large_guest() {
        // 1920x1080 guest shown in a 960x540 widget.
        let p = letterbox_map(480.0, 270.0, 960.0, 540.0, 1920, 1080).unwrap();
        assert_eq!(p, (960, 540));
    }
}
