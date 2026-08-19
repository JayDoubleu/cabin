//! The main window: a sidebar of desktops beside the viewer.
//!
//! See the layout sketch in DESIGN.md section 5.3.

use crate::display::CabinDisplay;
use crate::sidebar::Sidebar;
use adw::prelude::*;
use cabin_podman::Podman;
use std::sync::Arc;

/// The fixed sizes offered in the display menu.
const RESOLUTIONS: &[(u32, u32)] = &[(1280, 720), (1920, 1080), (2560, 1440), (3840, 2160)];

/// Builds the main window.
pub fn build(app: &adw::Application) -> adw::ApplicationWindow {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Cabin")
        .default_width(1280)
        .default_height(800)
        .build();

    let split = adw::NavigationSplitView::builder()
        .min_sidebar_width(220.0)
        .max_sidebar_width(320.0)
        .build();

    let podman = Arc::new(Podman::new());
    let sidebar = Sidebar::new(podman);

    let sidebar_toolbar = adw::ToolbarView::new();
    sidebar_toolbar.add_top_bar(&adw::HeaderBar::new());
    sidebar_toolbar.set_content(Some(&sidebar.widget));
    let sidebar_page = adw::NavigationPage::builder()
        .title("Cabin")
        .child(&sidebar_toolbar)
        .build();

    // Selecting a desktop will open a session in milestone 4. For now it
    // reports what it would connect to, which is enough to verify the list
    // carries the right port and state.
    sidebar.connect_selected(|desktop| {
        tracing::info!(
            "selected {} ({:?}), rdp port {:?}",
            desktop.name,
            desktop.state,
            desktop.rdp_port
        );
    });

    split.set_sidebar(Some(&sidebar_page));
    split.set_content(Some(&build_content()));
    window.set_content(Some(&split));

    // Podman is reachable or it is not. Tell the user at start-up rather
    // than failing later with a less obvious message.
    check_podman(&window);
    report_render_nodes();

    window
}

fn build_content() -> adw::NavigationPage {
    let display = CabinDisplay::new();
    display.set_hexpand(true);
    display.set_vexpand(true);

    let header = adw::HeaderBar::new();
    header.pack_end(&build_display_menu(&display));
    header.pack_end(&fullscreen_button());

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&display));

    adw::NavigationPage::builder()
        .title("Desktop")
        .child(&toolbar)
        .build()
}

fn build_display_menu(display: &CabinDisplay) -> gtk::MenuButton {
    let popover = gtk::Popover::new();
    let box_ = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .margin_top(8)
        .margin_bottom(8)
        .margin_start(8)
        .margin_end(8)
        .build();

    let fit = gtk::CheckButton::with_label("Fit to window");
    fit.set_active(true);
    box_.append(&fit);
    box_.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

    for &(width, height) in RESOLUTIONS {
        let button = gtk::CheckButton::with_label(&format!("{width} x {height}"));
        button.set_group(Some(&fit));
        let display = display.clone();
        button.connect_toggled(move |b| {
            if b.is_active() {
                display.set_fixed_resolution(width, height);
            }
        });
        box_.append(&button);
    }

    let display_for_fit = display.clone();
    fit.connect_toggled(move |b| {
        if b.is_active() {
            display_for_fit.set_fit_to_window(true);
        }
    });

    popover.set_child(Some(&box_));
    gtk::MenuButton::builder()
        .icon_name("video-display-symbolic")
        .tooltip_text("Display")
        .popover(&popover)
        .build()
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

/// Reports a missing Podman socket with the command that fixes it.
fn check_podman(window: &adw::ApplicationWindow) {
    let podman = Podman::new();
    if std::path::Path::new(podman.socket_path()).exists() {
        tracing::info!("podman socket at {}", podman.socket_path());
        return;
    }

    let dialog = adw::AlertDialog::builder()
        .heading("Podman is not running")
        .body(
            "Cabin needs the Podman user socket to manage desktops.\n\n\
             Start it with:\n\
             systemctl --user enable --now podman.socket",
        )
        .build();
    dialog.add_response("close", "Close");
    dialog.present(Some(window));
}

/// Logs the render nodes found on this host, with their drivers. The graphics
/// setting offers these, and Automatic skips the NVIDIA one.
/// See DESIGN.md section 7.3.
fn report_render_nodes() {
    let nodes = crate::graphics::render_nodes();
    if nodes.is_empty() {
        tracing::info!("no DRM render node found, desktops will use software rendering");
        return;
    }
    for (path, driver) in &nodes {
        tracing::info!("render node {path} uses the {driver} driver");
    }
}
