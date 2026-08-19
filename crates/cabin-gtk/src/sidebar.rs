//! The desktop list.
//!
//! Podman holds all state, so this list is always rebuilt from a fresh query
//! rather than mutated in place. The event feed tells it when to re-query. See
//! DESIGN.md section 5.1.

use crate::runtime;
use adw::prelude::*;
use cabin_core::{Desktop, State};
use cabin_podman::Podman;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

/// How long to let a guest shut down before Podman kills it. systemd inside
/// the guest stops the session cleanly, which takes a moment.
const STOP_TIMEOUT_SECS: u32 = 20;

/// Called when the user picks a desktop from the list.
type SelectionCallback = Rc<RefCell<Option<Box<dyn Fn(Desktop)>>>>;

pub struct Sidebar {
    pub widget: gtk::Box,
    list: gtk::ListBox,
    podman: Arc<Podman>,
    desktops: Rc<RefCell<Vec<Desktop>>>,
    on_select: SelectionCallback,
    /// Guards the one-off opening selection, so a refresh from the event feed
    /// does not keep reselecting and reconnecting behind the user.
    opened: Cell<bool>,
}

impl Sidebar {
    pub fn new(podman: Arc<Podman>) -> Rc<Self> {
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Single)
            .css_classes(vec!["navigation-sidebar"])
            .build();

        let placeholder = adw::StatusPage::builder()
            .icon_name("computer-symbolic")
            .title("No desktops")
            .description("Create one to get started.")
            .build();
        list.set_placeholder(Some(&placeholder));

        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&list)
            .build();

        let new_button = gtk::Button::builder()
            .label("New Desktop")
            .css_classes(vec!["suggested-action", "pill"])
            .margin_top(6)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .build();

        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.append(&scroller);
        widget.append(&new_button);

        let sidebar = Rc::new(Self {
            widget,
            list,
            podman,
            desktops: Rc::new(RefCell::new(Vec::new())),
            on_select: Rc::new(RefCell::new(None)),
            opened: Cell::new(false),
        });

        let for_new = sidebar.clone();
        new_button.connect_clicked(move |button| {
            let sidebar = for_new.clone();
            let refresh_target = for_new.clone();
            crate::create::present(button, sidebar.podman.clone(), move || {
                refresh_target.refresh();
            });
        });

        let for_select = sidebar.clone();
        sidebar.list.connect_row_selected(move |_, row| {
            let Some(row) = row else { return };
            let index = row.index();
            if index < 0 {
                return;
            }
            let desktop = for_select.desktops.borrow().get(index as usize).cloned();
            if let Some(desktop) = desktop {
                if let Some(callback) = for_select.on_select.borrow().as_ref() {
                    callback(desktop);
                }
            }
        });

        // A desktop that is created or removed elsewhere, with podman on the
        // command line for example, must still show up here.
        sidebar.watch_events();
        sidebar.refresh();
        sidebar
    }

    pub fn connect_selected(&self, callback: impl Fn(Desktop) + 'static) {
        *self.on_select.borrow_mut() = Some(Box::new(callback));
    }

    /// Re-queries Podman and rebuilds the list.
    pub fn refresh(self: &Rc<Self>) {
        let podman = self.podman.clone();
        let sidebar = self.clone();
        runtime::spawn(
            async move { podman.list_desktops().await },
            move |result| match result {
                Ok(desktops) => sidebar.populate(desktops),
                Err(e) => tracing::error!("could not list desktops: {e}"),
            },
        );
    }

    fn populate(self: &Rc<Self>, desktops: Vec<Desktop>) {
        tracing::info!("listing {} desktop(s)", desktops.len());
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        for desktop in &desktops {
            self.list.append(&self.build_row(desktop));
        }
        let first_running = desktops.iter().position(|d| d.state.is_running());
        *self.desktops.borrow_mut() = desktops;

        // Open the first running desktop once, at start-up. Without this the
        // viewer shows an empty pane until the user thinks to click a row,
        // which reads as the application being broken.
        if !self.opened.get() {
            if let Some(index) = first_running {
                self.opened.set(true);
                if let Some(row) = self.list.row_at_index(index as i32) {
                    self.list.select_row(Some(&row));
                }
            }
        }
    }

    fn build_row(self: &Rc<Self>, desktop: &Desktop) -> gtk::ListBoxRow {
        let row_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(12)
            .margin_top(8)
            .margin_bottom(8)
            .margin_start(12)
            .margin_end(6)
            .build();

        let indicator = gtk::Image::from_icon_name(state_icon(desktop.state));
        indicator.add_css_class(if desktop.state.is_running() {
            "success"
        } else {
            "dim-label"
        });
        row_box.append(&indicator);

        let labels = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let name = gtk::Label::builder()
            .label(&desktop.name)
            .halign(gtk::Align::Start)
            .build();
        let subtitle = gtk::Label::builder()
            .label(state_text(desktop))
            .halign(gtk::Align::Start)
            .css_classes(vec!["caption", "dim-label"])
            .build();
        labels.append(&name);
        labels.append(&subtitle);
        labels.set_hexpand(true);
        row_box.append(&labels);

        row_box.append(&self.build_action_button(desktop));
        row_box.append(&self.build_overflow_menu(desktop));

        gtk::ListBoxRow::builder().child(&row_box).build()
    }

    fn build_action_button(self: &Rc<Self>, desktop: &Desktop) -> gtk::Button {
        let running = desktop.state.is_running();
        let button = gtk::Button::builder()
            .icon_name(if running {
                "media-playback-stop-symbolic"
            } else {
                "media-playback-start-symbolic"
            })
            .tooltip_text(if running { "Stop" } else { "Start" })
            .valign(gtk::Align::Center)
            .css_classes(vec!["flat", "circular"])
            .build();

        let id = desktop.id.clone();
        let name = desktop.name.clone();
        let podman = self.podman.clone();
        let sidebar = self.clone();
        button.connect_clicked(move |button| {
            // Stop double clicks queueing a second operation. The refresh that
            // follows rebuilds the row and replaces this button.
            button.set_sensitive(false);
            let id = id.clone();
            let name = name.clone();
            let podman = podman.clone();
            let sidebar = sidebar.clone();
            // The button is only borrowed for the closure body, so keep an
            // owned handle for the reply that arrives later.
            let button = button.clone();
            runtime::spawn(
                async move {
                    if running {
                        podman.stop(&id, STOP_TIMEOUT_SECS).await
                    } else {
                        podman.start_desktop(&id, &name).await
                    }
                },
                move |result| {
                    if let Err(e) = result {
                        tracing::error!("could not change desktop state: {e}");
                        // A desktop that will not start needs to say why. The
                        // usual cause, an exhausted inotify limit, is
                        // invisible otherwise: the container exits 255 with an
                        // empty log.
                        show_error(&button, "The desktop did not start", &e.to_string());
                    }
                    sidebar.refresh();
                },
            );
        });
        button
    }

    /// The per-desktop menu: settings, restart and delete.
    fn build_overflow_menu(self: &Rc<Self>, desktop: &Desktop) -> gtk::MenuButton {
        let popover = gtk::Popover::new();
        let box_ = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .margin_top(6)
            .margin_bottom(6)
            .margin_start(6)
            .margin_end(6)
            .build();

        let settings = flat_button("Settings", "preferences-system-symbolic");
        let sidebar = self.clone();
        let for_settings = desktop.clone();
        let popover_ref = popover.clone();
        settings.connect_clicked(move |button| {
            popover_ref.popdown();
            let refresh_target = sidebar.clone();
            crate::settings::present(
                button,
                sidebar.podman.clone(),
                for_settings.clone(),
                move || refresh_target.refresh(),
            );
        });
        box_.append(&settings);

        let restart = flat_button("Restart", "view-refresh-symbolic");
        restart.set_sensitive(desktop.state.is_running());
        let sidebar = self.clone();
        let id = desktop.id.clone();
        let popover_ref = popover.clone();
        restart.connect_clicked(move |_| {
            popover_ref.popdown();
            let podman = sidebar.podman.clone();
            let id = id.clone();
            let sidebar = sidebar.clone();
            runtime::spawn(
                async move { podman.restart(&id, STOP_TIMEOUT_SECS).await },
                move |result| {
                    if let Err(e) = result {
                        tracing::error!("could not restart the desktop: {e}");
                    }
                    sidebar.refresh();
                },
            );
        });
        box_.append(&restart);

        let remote = flat_button("Connect from another device", "network-server-symbolic");
        remote.set_sensitive(desktop.state.is_running());
        let sidebar = self.clone();
        let for_remote = desktop.clone();
        let popover_ref = popover.clone();
        remote.connect_clicked(move |button| {
            popover_ref.popdown();
            let podman = sidebar.podman.clone();
            let id = for_remote.id.clone();
            let desktop = for_remote.clone();
            let button = button.clone();
            runtime::spawn(
                async move { podman.desktop_password(&id).await },
                move |result| match result {
                    Ok(password) => crate::remote::present(&button, &desktop, password),
                    Err(e) => tracing::error!("could not read the desktop password: {e}"),
                },
            );
        });
        box_.append(&remote);

        box_.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

        let delete = flat_button("Delete", "user-trash-symbolic");
        delete.add_css_class("destructive-action");
        let sidebar = self.clone();
        let for_delete = desktop.clone();
        let popover_ref = popover.clone();
        delete.connect_clicked(move |button| {
            popover_ref.popdown();
            sidebar.confirm_delete(button, for_delete.clone());
        });
        box_.append(&delete);

        popover.set_child(Some(&box_));
        gtk::MenuButton::builder()
            .icon_name("view-more-symbolic")
            .valign(gtk::Align::Center)
            .css_classes(vec!["flat", "circular"])
            .popover(&popover)
            .build()
    }

    /// Deleting destroys the desktop's files, so it asks first and says so.
    fn confirm_delete(self: &Rc<Self>, parent: &impl IsA<gtk::Widget>, desktop: Desktop) {
        let dialog = adw::AlertDialog::builder()
            .heading(format!("Delete {}?", desktop.name))
            .body("The desktop and everything saved inside it will be removed. This cannot be undone.")
            .build();
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("delete", "Delete");
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));

        let sidebar = self.clone();
        dialog.connect_response(None, move |_, response| {
            if response != "delete" {
                return;
            }
            let podman = sidebar.podman.clone();
            let id = desktop.id.clone();
            let sidebar = sidebar.clone();
            runtime::spawn(
                async move { podman.remove(&id, true).await },
                move |result| {
                    if let Err(e) = result {
                        tracing::error!("could not delete the desktop: {e}");
                    }
                    sidebar.refresh();
                },
            );
        });
        dialog.present(Some(parent));
    }

    /// Follows the Podman event feed so the list reflects reality without
    /// polling.
    fn watch_events(self: &Rc<Self>) {
        let podman = self.podman.clone();
        let sidebar = self.clone();
        runtime::spawn_stream(
            move |sender| {
                Box::pin(async move {
                    let result = podman
                        .watch_events(|event| {
                            let _ = sender.try_send(event.action);
                        })
                        .await;
                    if let Err(e) = result {
                        tracing::warn!("event feed stopped: {e}");
                    }
                })
            },
            move |action| {
                tracing::debug!("podman event: {action}");
                sidebar.refresh();
            },
        );
    }
}

/// Reports a failure the user can act on, rather than only logging it.
fn show_error(parent: &impl IsA<gtk::Widget>, heading: &str, body: &str) {
    let dialog = adw::AlertDialog::builder()
        .heading(heading)
        .body(body)
        .build();
    dialog.add_response("close", "Close");
    dialog.present(Some(parent));
}

fn flat_button(label: &str, icon: &str) -> gtk::Button {
    let content = adw::ButtonContent::builder()
        .label(label)
        .icon_name(icon)
        .halign(gtk::Align::Start)
        .build();
    gtk::Button::builder()
        .child(&content)
        .css_classes(vec!["flat"])
        .build()
}

fn state_icon(state: State) -> &'static str {
    match state {
        State::Running => "media-playback-start-symbolic",
        State::Paused => "media-playback-pause-symbolic",
        State::Exited | State::Created => "media-playback-stop-symbolic",
        State::Stopping => "content-loading-symbolic",
        State::Unknown => "dialog-question-symbolic",
    }
}

fn state_text(desktop: &Desktop) -> String {
    match desktop.state {
        State::Running => match desktop.rdp_port {
            Some(port) => format!("Running on port {port}"),
            None => "Running".to_string(),
        },
        State::Paused => "Paused".to_string(),
        State::Exited => "Stopped".to_string(),
        State::Created => "Not started".to_string(),
        State::Stopping => "Stopping".to_string(),
        State::Unknown => "Unknown".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cabin_core::DesktopConfig;

    fn desktop(state: State, port: Option<u16>) -> Desktop {
        Desktop {
            id: "abc".into(),
            name: "test".into(),
            state,
            config: DesktopConfig::new("test"),
            rdp_port: port,
        }
    }

    #[test]
    fn running_desktop_shows_its_port() {
        assert_eq!(
            state_text(&desktop(State::Running, Some(13389))),
            "Running on port 13389"
        );
    }

    #[test]
    fn running_without_a_port_still_reads_sensibly() {
        assert_eq!(state_text(&desktop(State::Running, None)), "Running");
    }

    #[test]
    fn stopped_desktop_does_not_mention_a_port() {
        assert_eq!(state_text(&desktop(State::Exited, Some(13389))), "Stopped");
    }
}
