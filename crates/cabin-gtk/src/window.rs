//! The main window: a sidebar of desktops beside the viewer.
//!
//! See the layout sketch in DESIGN.md section 5.3.

use crate::sidebar::Sidebar;
use crate::viewer::Viewer;
use adw::prelude::*;
use cabin_core::Desktop;
use cabin_podman::Podman;
use std::rc::Rc;
use std::sync::Arc;

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

    // An overlay split view rather than a navigation one, because the desktop
    // list is something to hide when the desktop needs the room, not a place
    // to navigate back to.
    let split = adw::OverlaySplitView::builder()
        .min_sidebar_width(220.0)
        .max_sidebar_width(320.0)
        .build();

    let podman = Arc::new(Podman::new());
    let sidebar = Sidebar::new(podman.clone());
    let viewer = Viewer::new();

    let sidebar_toolbar = adw::ToolbarView::new();
    let sidebar_header = adw::HeaderBar::new();
    sidebar_header.set_title_widget(Some(&adw::WindowTitle::new("Cabin", "")));
    sidebar_header.pack_end(&build_main_menu(&window, podman.clone(), sidebar.clone()));
    sidebar_toolbar.add_top_bar(&sidebar_header);
    sidebar_toolbar.set_content(Some(&sidebar.widget));

    // The pane asks for a desktop to be opened, which needs its password, and
    // only the window holds the Podman handle.
    let podman_for_open = podman.clone();
    let viewer_for_open = viewer.clone();
    viewer.connect_open(move |desktop| {
        open(&viewer_for_open, &podman_for_open, desktop);
    });

    // Settings for the desktop on screen, reachable from its own header bar
    // rather than only from the sidebar row menu.
    let podman_for_settings = podman.clone();
    let sidebar_for_settings = sidebar.clone();
    let window_for_settings = window.clone();
    viewer.connect_settings(move |desktop| {
        let refresh_target = sidebar_for_settings.clone();
        crate::settings::present(
            &window_for_settings,
            podman_for_settings.clone(),
            desktop,
            move || refresh_target.refresh(),
        );
    });

    // A desktop created with "Open settings" switched on goes straight there.
    let podman_for_new = podman.clone();
    let sidebar_for_new = sidebar.clone();
    let window_for_new = window.clone();
    sidebar.connect_settings_requested(move |desktop| {
        let refresh_target = sidebar_for_new.clone();
        crate::settings::present(
            &window_for_new,
            podman_for_new.clone(),
            desktop,
            move || refresh_target.refresh(),
        );
    });

    // Selecting a desktop opens it. Selecting one that is stopped says so,
    // rather than leaving whatever the pane showed before.
    let podman_for_select = podman.clone();
    let viewer_for_select = viewer.clone();
    let state_for_select = state.clone();
    sidebar.connect_selected(move |desktop| {
        state_for_select.set_last_desktop(&desktop.name);
        open(&viewer_for_select, &podman_for_select, desktop);
    });

    // Podman is the only store of desktop state, so the pane follows the list
    // rather than being told separately about deletions and stops.
    let viewer_for_list = viewer.clone();
    sidebar.connect_listed(move |desktops| {
        viewer_for_list.reconcile(desktops);
    });

    split.set_sidebar(Some(&sidebar_toolbar));
    split.set_content(Some(&viewer.content));
    window.set_content(Some(&split));

    // The toggle lives in the pane's header bar, and the split view follows
    // it. Two way, so a collapse that the split view decides for itself, on a
    // narrow window, still leaves the button showing the truth.
    viewer
        .sidebar_toggle
        .bind_property("active", &split, "show-sidebar")
        .bidirectional()
        .sync_create()
        .build();
    viewer.sidebar_toggle.connect_active_notify(move |button| {
        button.set_tooltip_text(Some(if button.is_active() {
            "Hide the desktop list"
        } else {
            "Show the desktop list"
        }));
    });
    viewer.sidebar_toggle.set_active(state.sidebar_visible());

    // Saved with the window geometry, so the layout is where it was left.
    let state_for_sidebar = state.clone();
    let toggle_for_close = viewer.sidebar_toggle.clone();
    window.connect_close_request(move |_| {
        state_for_sidebar.set_sidebar_visible(toggle_for_close.is_active());
        gtk::glib::Propagation::Proceed
    });

    // Podman is reachable or it is not. Tell the user at start-up rather
    // than failing later with a less obvious message.
    check_podman(&window);
    report_render_nodes();

    window
}

/// Shows `desktop` in the pane, connecting to it when it is running.
///
/// Every route into the pane goes through here: the sidebar selection, the
/// Reconnect button, and a desktop that starts running again. One route means
/// one set of rules about what the pane shows.
fn open(viewer: &Rc<Viewer>, podman: &Arc<Podman>, desktop: Desktop) {
    if !desktop.state.is_running() {
        viewer.show_stopped(&desktop);
        return;
    }
    // Reselecting the desktop already on screen must not tear down its
    // session and build another one.
    if viewer.is_connected_to(&desktop.id) {
        return;
    }

    let viewer = viewer.clone();
    let podman = podman.clone();
    let id = desktop.id.clone();
    crate::runtime::spawn(
        async move { podman.desktop_password(&id).await },
        move |result| match result {
            Ok(password) => viewer.attach(&desktop, password),
            Err(e) => {
                tracing::error!("could not read the desktop password: {e}");
                viewer.clear();
            }
        },
    );
}

/// The application menu, for what belongs to Cabin rather than to a desktop.
fn build_main_menu(
    window: &adw::ApplicationWindow,
    podman: Arc<Podman>,
    sidebar: Rc<Sidebar>,
) -> gtk::MenuButton {
    let popover = gtk::Popover::new();
    let box_ = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .margin_top(6)
        .margin_bottom(6)
        .margin_start(6)
        .margin_end(6)
        .build();

    let images = gtk::Button::builder()
        .child(
            &adw::ButtonContent::builder()
                .label("Guest Images")
                .icon_name("drive-harddisk-symbolic")
                .halign(gtk::Align::Start)
                .build(),
        )
        .css_classes(vec!["flat"])
        .build();

    let window_for_images = window.clone();
    let popover_ref = popover.clone();
    images.connect_clicked(move |_| {
        popover_ref.popdown();
        let refresh_target = sidebar.clone();
        crate::images::present(&window_for_images, podman.clone(), move || {
            refresh_target.refresh()
        });
    });
    box_.append(&images);

    let about = gtk::Button::builder()
        .child(
            &adw::ButtonContent::builder()
                .label("About Cabin")
                .icon_name("help-about-symbolic")
                .halign(gtk::Align::Start)
                .build(),
        )
        .css_classes(vec!["flat"])
        .build();
    let window_for_about = window.clone();
    let popover_ref = popover.clone();
    about.connect_clicked(move |_| {
        popover_ref.popdown();
        present_about(&window_for_about);
    });
    box_.append(&about);

    popover.set_child(Some(&box_));
    gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .tooltip_text("Main menu")
        .popover(&popover)
        .build()
}

/// Shows the version, the licence and where to report a problem.
fn present_about(window: &adw::ApplicationWindow) {
    let dialog = adw::AboutDialog::builder()
        .application_name("Cabin")
        // The application ID, so a development build shows its own icon.
        .application_icon(crate::APP_ID)
        .version(env!("CARGO_PKG_VERSION"))
        .developer_name("Jay W")
        .license_type(gtk::License::Gpl30)
        .website("https://github.com/JayDoubleu/cabin")
        .issue_url("https://github.com/JayDoubleu/cabin/issues")
        .build();
    dialog.present(Some(window));
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
