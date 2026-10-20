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
use std::time::Duration;

/// How long to let a guest shut down before Podman kills it. systemd inside
/// the guest stops the session cleanly, which takes a moment.
const STOP_TIMEOUT_SECS: u32 = 20;

/// Called when the user picks a desktop from the list.
type SelectionCallback = Rc<RefCell<Option<Box<dyn Fn(Desktop)>>>>;

/// Called after every refresh, with the desktops Podman reported.
type ListCallback = Rc<RefCell<Option<Box<dyn Fn(&[Desktop])>>>>;

pub struct Sidebar {
    pub widget: gtk::Box,
    list: gtk::ListBox,
    podman: Arc<Podman>,
    desktops: Rc<RefCell<Vec<Desktop>>>,
    on_select: SelectionCallback,
    on_list: ListCallback,
    /// Guards the one-off opening selection, so a refresh from the event feed
    /// does not keep reselecting and reconnecting behind the user.
    opened: Cell<bool>,
    /// The desktop the user chose, by name. The list is rebuilt from scratch
    /// on every refresh, which drops the selection, so it is restored by name
    /// rather than by row.
    selected: RefCell<Option<String>>,
    /// True while the selection is being restored. The restore is only there
    /// to keep the row highlighted, so it must not be mistaken for the user
    /// choosing a desktop.
    restoring: Cell<bool>,
    /// Set when `selected` names a desktop the user has just asked for, and
    /// the row does not exist yet. The next refresh that finds it opens it.
    pending_choice: Cell<bool>,
    /// Set when the desktop being waited for should have its settings opened
    /// as soon as it appears.
    pending_settings: Cell<bool>,
    on_settings: SelectionCallback,
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
            on_list: Rc::new(RefCell::new(None)),
            opened: Cell::new(false),
            selected: RefCell::new(None),
            restoring: Cell::new(false),
            pending_choice: Cell::new(false),
            pending_settings: Cell::new(false),
            on_settings: Rc::new(RefCell::new(None)),
        });

        let for_new = sidebar.clone();
        new_button.connect_clicked(move |button| {
            // One dialogue at a time. The button comes back when the names
            // have arrived and the dialogue is up.
            button.set_sensitive(false);
            let refresh_target = for_new.clone();
            // The names and ports already in use, so the dialogue can refuse
            // a clash as it is typed rather than after Create is pressed.
            let (desktops, ports): (Vec<String>, Vec<u16>) = {
                let listed = for_new.desktops.borrow();
                (
                    listed.iter().map(|d| d.name.clone()).collect(),
                    listed.iter().filter_map(|d| d.rdp_port).collect(),
                )
            };
            let podman = for_new.podman.clone();
            let for_dialog = for_new.podman.clone();
            let button = button.clone();
            runtime::spawn(
                async move {
                    let containers = podman.container_names().await;
                    let volumes = podman.volume_names().await;
                    (containers, volumes)
                },
                move |(containers, volumes)| {
                    button.set_sensitive(true);
                    // Without the lists the check is weaker, not wrong: Podman
                    // still refuses a clash, only later. That is no reason to
                    // keep the user from making a desktop.
                    let containers = containers.unwrap_or_else(|e| {
                        tracing::warn!("could not list the containers: {e}");
                        Vec::new()
                    });
                    let volumes = volumes.unwrap_or_else(|e| {
                        tracing::warn!("could not list the volumes: {e}");
                        Vec::new()
                    });
                    let in_use = crate::create::InUse {
                        desktops,
                        ports,
                        containers,
                        volumes,
                    };
                    // Select the new desktop once it appears, so its settings
                    // are one click away and the pane stops showing the
                    // previous one.
                    crate::create::present(
                        &button,
                        for_dialog,
                        in_use,
                        move |name, open_settings| {
                            refresh_target.pending_settings.set(open_settings);
                            refresh_target.choose(&name);
                        },
                    );
                },
            );
        });

        let for_select = sidebar.clone();
        sidebar.list.connect_row_selected(move |_, row| {
            if for_select.restoring.get() {
                return;
            }
            let Some(row) = row else { return };
            let index = row.index();
            if index < 0 {
                return;
            }
            let desktop = for_select.desktops.borrow().get(index as usize).cloned();
            if let Some(desktop) = desktop {
                *for_select.selected.borrow_mut() = Some(desktop.name.clone());
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

    /// Called after every refresh. The viewer uses this to notice a desktop
    /// that was deleted or stopped somewhere else.
    pub fn connect_listed(&self, callback: impl Fn(&[Desktop]) + 'static) {
        *self.on_list.borrow_mut() = Some(Box::new(callback));
    }

    /// Called when a desktop's settings should be opened. Used by the create
    /// dialogue, which offers to go straight there.
    pub fn connect_settings_requested(&self, callback: impl Fn(Desktop) + 'static) {
        *self.on_settings.borrow_mut() = Some(Box::new(callback));
    }

    /// Selects a desktop by name, and opens it once it appears.
    ///
    /// A desktop that was just created is not in the list yet, so the choice
    /// is recorded and acted on by the refresh that finds it.
    pub fn choose(self: &Rc<Self>, name: &str) {
        *self.selected.borrow_mut() = Some(name.to_string());
        self.pending_choice.set(true);
        self.refresh();
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

        // Prefer the desktop that was last in use, so reopening Cabin puts
        // the user back where they were. Fall back to the first that is
        // running, which is better than an empty pane.
        let preferred = crate::state::State::new(crate::APP_ID).last_desktop();
        let opening_choice = preferred
            .and_then(|name| {
                desktops
                    .iter()
                    .position(|d| d.name == name && d.state.is_running())
            })
            .or_else(|| desktops.iter().position(|d| d.state.is_running()));

        // Removing the rows above dropped the selection, so find where the
        // chosen desktop landed before the list is replaced.
        let chosen = self
            .selected
            .borrow()
            .as_ref()
            .and_then(|name| desktops.iter().position(|d| &d.name == name));
        *self.desktops.borrow_mut() = desktops.clone();

        if let Some(index) = chosen {
            // A choice that is still pending must reach the viewer. Every
            // other restore is a highlight only: treating it as a fresh
            // choice would reconnect the session on every Podman event.
            let pending = self.pending_choice.replace(false);
            self.restoring.set(!pending);
            if let Some(row) = self.list.row_at_index(index as i32) {
                if pending && self.list.selected_row().as_ref() == Some(&row) {
                    // Already the selected row, so selecting it again emits
                    // nothing. Call through directly.
                    self.list.unselect_all();
                }
                self.list.select_row(Some(&row));
            }
            self.restoring.set(false);
            self.opened.set(true);
        } else {
            // The chosen desktop has gone. Stop trying to restore it.
            *self.selected.borrow_mut() = None;
        }

        // Open the first running desktop once, at start-up. Without this the
        // viewer shows an empty pane until the user thinks to click a row,
        // which reads as the application being broken.
        if !self.opened.get() && chosen.is_none() {
            if let Some(index) = opening_choice {
                self.opened.set(true);
                if let Some(row) = self.list.row_at_index(index as i32) {
                    self.list.select_row(Some(&row));
                }
            }
        }

        // Last, so the viewer reconciles against a list the sidebar has
        // already settled on.
        if let Some(callback) = self.on_list.borrow().as_ref() {
            callback(&desktops);
        }

        // A desktop created with "Open settings" switched on. It exists now,
        // so this is the first moment its settings can be shown.
        if self.pending_settings.get() {
            if let Some(desktop) = chosen.and_then(|index| desktops.get(index)) {
                self.pending_settings.set(false);
                let callback = self.on_settings.borrow();
                if let Some(callback) = callback.as_ref() {
                    callback(desktop.clone());
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

        let for_action = desktop.clone();
        let podman = self.podman.clone();
        let sidebar = self.clone();
        button.connect_clicked(move |button| {
            // Stop double clicks queueing a second operation. The refresh that
            // follows rebuilds the row and replaces this button.
            button.set_sensitive(false);
            let desktop = for_action.clone();
            let podman = podman.clone();
            let sidebar = sidebar.clone();
            // The button is only borrowed for the closure body, so keep an
            // owned handle for the reply that arrives later.
            let button = button.clone();
            // Read on the main thread: both are quick, and the start needs
            // them to resolve the chosen networks again (rule 40).
            let nodes = crate::graphics::render_nodes();
            let interfaces = crate::network::interfaces();
            runtime::spawn(
                async move {
                    if running {
                        podman.stop(&desktop.id, STOP_TIMEOUT_SECS).await
                    } else {
                        // The id changes when the addresses moved and the
                        // desktop was recreated. The refresh below finds it.
                        podman
                            .start_with_current_addresses(&desktop, &nodes, &interfaces)
                            .await
                            .map(|_| ())
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
        let for_restart = desktop.clone();
        let popover_ref = popover.clone();
        restart.connect_clicked(move |_| {
            popover_ref.popdown();
            let podman = sidebar.podman.clone();
            let desktop = for_restart.clone();
            let sidebar = sidebar.clone();
            let nodes = crate::graphics::render_nodes();
            let interfaces = crate::network::interfaces();
            runtime::spawn(
                async move {
                    podman
                        .restart_with_current_addresses(&desktop, &nodes, &interfaces)
                        .await
                },
                move |result| {
                    if let Err(e) = result {
                        tracing::error!("could not restart the desktop: {e}");
                        // The menu button can be gone by now, because a
                        // refresh rebuilds the rows.
                        show_error(
                            &sidebar.widget,
                            "The desktop did not restart",
                            &e.to_string(),
                        );
                    }
                    // A recreate gives the desktop a new id.
                    sidebar.refresh();
                },
            );
        });
        box_.append(&restart);

        let shell = flat_button("Open a shell", "utilities-terminal-symbolic");
        shell.set_sensitive(desktop.state.is_running());
        let for_shell = desktop.clone();
        let popover_ref = popover.clone();
        shell.connect_clicked(move |button| {
            popover_ref.popdown();
            crate::shell::present(button, &for_shell);
        });
        box_.append(&shell);

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
                    Err(e) => {
                        tracing::error!("could not read the desktop password: {e}");
                        show_error(
                            &button,
                            "Could not read the desktop password",
                            &e.to_string(),
                        );
                    }
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
            let name = desktop.name.clone();
            let sidebar = sidebar.clone();
            runtime::spawn(
                async move {
                    let result = podman.delete_desktop(&id, &name).await;
                    (name, result)
                },
                move |(name, result)| {
                    match result {
                        // Otherwise a new desktop reusing the name would
                        // present a different certificate and be reported as
                        // an interception. Only once the state volume is
                        // gone, because the old certificate lives there.
                        Ok(()) => crate::pinning::forget(&name),
                        Err(e) => {
                            tracing::error!("could not delete the desktop: {e}");
                            // The menu button sits in a popover that has
                            // closed, and the refresh rebuilds the rows, so it
                            // may have no parent by now.
                            show_error(
                                &sidebar.widget,
                                "The desktop was not deleted",
                                &e.to_string(),
                            );
                        }
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
                    // The feed ends whenever Podman restarts or its socket
                    // goes away. Without a reconnect, changes made outside
                    // Cabin, such as a desktop powering off from inside,
                    // would stop reaching the list. DESIGN.md section 5 rules
                    // out a polling timer, so the feed is reopened instead.
                    let mut delay = FEED_RETRY_FIRST;
                    let mut first = true;
                    loop {
                        if !first {
                            // Changes made during the gap sent no event, so
                            // ask for one refresh.
                            let _ = sender.try_send("reconnected".to_string());
                        }
                        first = false;
                        let opened = std::time::Instant::now();
                        let mut delivered = false;
                        let result = podman
                            .watch_events(|event| {
                                delivered = true;
                                let _ = sender.try_send(event.action);
                            })
                            .await;
                        match result {
                            Err(e) => tracing::warn!("event feed stopped: {e}"),
                            Ok(()) => tracing::info!("event feed closed by Podman, reconnecting"),
                        }
                        // The window has gone, so nobody is listening.
                        if sender.is_closed() {
                            break;
                        }
                        let wait = feed_wait(delay, delivered, opened.elapsed());
                        tokio::time::sleep(wait).await;
                        delay = (wait * 2).min(FEED_RETRY_MAX);
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

/// The wait before the first attempt to reopen the event feed.
const FEED_RETRY_FIRST: Duration = Duration::from_secs(1);

/// The longest wait between attempts. Podman can be down for a while, and
/// a fast loop would only fill the log.
const FEED_RETRY_MAX: Duration = Duration::from_secs(30);

/// A connection that stayed open this long counts as healthy.
const FEED_HEALTHY: Duration = Duration::from_secs(5);

/// The wait before the next attempt to reopen the event feed.
///
/// `planned` doubles after each failed attempt, up to a limit. A connection
/// that delivered an event, or that stayed open for a while, worked, so the
/// wait starts again from the shortest.
fn feed_wait(planned: Duration, delivered: bool, lasted: Duration) -> Duration {
    if delivered || lasted >= FEED_HEALTHY {
        FEED_RETRY_FIRST
    } else {
        planned
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
    fn feed_keeps_the_planned_wait_after_a_failed_connection() {
        let planned = Duration::from_secs(8);
        assert_eq!(
            feed_wait(planned, false, Duration::from_millis(10)),
            planned
        );
    }

    #[test]
    fn feed_wait_starts_again_after_a_connection_that_worked() {
        let planned = Duration::from_secs(16);
        assert_eq!(feed_wait(planned, true, Duration::ZERO), FEED_RETRY_FIRST);
        assert_eq!(feed_wait(planned, false, FEED_HEALTHY), FEED_RETRY_FIRST);
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
