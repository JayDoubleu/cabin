//! The main window: a sidebar of desktops beside the viewer.
//!
//! See the layout sketch in DESIGN.md section 5.3.

use crate::display::CabinDisplay;
use crate::sidebar::Sidebar;
use adw::prelude::*;
use cabin_podman::Podman;
use std::sync::Arc;

const PAGE_EMPTY: &str = "empty";
const PAGE_DESKTOP: &str = "desktop";

/// Swaps the viewer between its placeholder and the live desktop.
fn apply_viewer_state(stack: &gtk::Stack, name: &str, state: crate::viewer::State) {
    use crate::viewer::State;
    match state {
        State::Connecting => {
            set_placeholder(
                stack,
                "content-loading-symbolic",
                &format!("Connecting to {name}"),
                "Starting the remote desktop session.",
            );
            stack.set_visible_child_name(PAGE_EMPTY);
        }
        State::Showing => stack.set_visible_child_name(PAGE_DESKTOP),
        State::Failed(reason) => {
            set_placeholder(
                stack,
                "dialog-warning-symbolic",
                &format!("{name} disconnected"),
                &reason,
            );
            stack.set_visible_child_name(PAGE_EMPTY);
        }
    }
}

fn set_placeholder(stack: &gtk::Stack, icon: &str, title: &str, description: &str) {
    if let Some(page) = stack
        .child_by_name(PAGE_EMPTY)
        .and_downcast::<adw::StatusPage>()
    {
        page.set_icon_name(Some(icon));
        page.set_title(title);
        page.set_description(Some(description));
    }
}

/// The fixed sizes offered in the display menu.
const RESOLUTIONS: &[(u32, u32)] = &[(1280, 720), (1920, 1080), (2560, 1440), (3840, 2160)];

/// Builds the main window.
pub fn build(app: &adw::Application) -> adw::ApplicationWindow {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Cabin")
        .build();

    let state = std::rc::Rc::new(crate::state::State::new(crate::APP_ID));
    state.restore_window(&window);

    // Saved as the window closes, so the next launch opens the same size.
    let state_for_close = state.clone();
    window.connect_close_request(move |window| {
        state_for_close.save_window(window);
        gtk::glib::Propagation::Proceed
    });

    let split = adw::NavigationSplitView::builder()
        .min_sidebar_width(220.0)
        .max_sidebar_width(320.0)
        .build();

    let podman = Arc::new(Podman::new());
    let podman_for_viewer = podman.clone();
    let sidebar = Sidebar::new(podman);

    let sidebar_toolbar = adw::ToolbarView::new();
    sidebar_toolbar.add_top_bar(&adw::HeaderBar::new());
    sidebar_toolbar.set_content(Some(&sidebar.widget));
    let sidebar_page = adw::NavigationPage::builder()
        .title("Cabin")
        .child(&sidebar_toolbar)
        .build();

    let (content, display, stack) = build_content();

    // Selecting a running desktop opens a session and starts drawing it.
    let podman_for_select = podman_for_viewer;
    let state_for_select = state.clone();
    sidebar.connect_selected(move |desktop| {
        state_for_select.set_last_desktop(&desktop.name);
        if !desktop.state.is_running() {
            tracing::info!("{} is not running", desktop.name);
            return;
        }
        let display = display.clone();
        let stack = stack.clone();
        let podman = podman_for_select.clone();
        let id = desktop.id.clone();
        let desktop = desktop.clone();
        crate::runtime::spawn(
            async move { podman.desktop_password(&id).await },
            move |result| match result {
                Ok(password) => {
                    let stack = stack.clone();
                    let name = desktop.name.clone();
                    crate::viewer::attach(&display, &desktop, password, move |state| {
                        apply_viewer_state(&stack, &name, state)
                    });
                }
                Err(e) => tracing::error!("could not read the desktop password: {e}"),
            },
        );
    });

    split.set_sidebar(Some(&sidebar_page));
    split.set_content(Some(&content));
    window.set_content(Some(&split));

    // Podman is reachable or it is not. Tell the user at start-up rather
    // than failing later with a less obvious message.
    check_podman(&window);
    report_render_nodes();

    window
}

/// The viewer pane. A stack so the user sees an explanation rather than a
/// black rectangle before a session is connected.
fn build_content() -> (adw::NavigationPage, CabinDisplay, gtk::Stack) {
    let display = CabinDisplay::new();
    display.set_hexpand(true);
    display.set_vexpand(true);

    let placeholder = adw::StatusPage::builder()
        .icon_name("video-display-symbolic")
        .title("No desktop connected")
        .description("Choose a desktop from the sidebar to connect to it.")
        .build();

    let stack = gtk::Stack::new();
    stack.add_named(&placeholder, Some(PAGE_EMPTY));
    stack.add_named(&display, Some(PAGE_DESKTOP));
    stack.set_visible_child_name(PAGE_EMPTY);

    let header = adw::HeaderBar::new();
    header.pack_end(&build_display_menu(&display));
    header.pack_end(&fullscreen_button());

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&stack));

    let page = adw::NavigationPage::builder()
        .title("Desktop")
        .child(&toolbar)
        .build();
    (page, display, stack)
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
