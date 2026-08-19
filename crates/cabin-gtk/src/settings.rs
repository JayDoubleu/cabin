//! Per-desktop settings: mounts, graphics, and container runtime sockets.
//!
//! Changing any of these needs the container recreated, which is why the home
//! directory lives in a named volume. See `cabin_core::home_volume_name`.

use crate::graphics;
use crate::runtime;
use adw::prelude::*;
use cabin_core::{
    Desktop, DesktopConfig, Exposure, GraphicsMode, Mount, MountKind, MountMode, SocketPolicy,
};
use cabin_podman::Podman;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

/// The socket options, in the order they appear, safest first.
///
/// `SocketPolicy::Nested` is deliberately absent. Rootless Podman inside a
/// rootless container does not work yet, and offering a choice that quietly
/// fails is worse than not offering it. DESIGN.md section 15.17 records how
/// far it got.
const SOCKET_CHOICES: &[(SocketPolicy, &str, &str)] = &[
    (
        SocketPolicy::None,
        "None",
        "The desktop cannot build or run containers.",
    ),
    (
        SocketPolicy::HostPodman,
        "Podman on this computer",
        "Dangerous. The desktop gets the power of your user account.",
    ),
    (
        SocketPolicy::HostDocker,
        "Docker on this computer",
        "Dangerous. The desktop gets root on your computer.",
    ),
];

pub fn present(
    parent: &impl IsA<gtk::Widget>,
    podman: Arc<Podman>,
    desktop: Desktop,
    on_applied: impl Fn() + 'static,
) {
    let config = Rc::new(RefCell::new(desktop.config.clone()));
    let on_applied: Rc<dyn Fn()> = Rc::new(on_applied);

    let dialog = adw::PreferencesDialog::builder()
        .title(format!("{} Settings", desktop.name))
        .build();

    let page = adw::PreferencesPage::new();
    page.add(&build_mounts_group(parent, config.clone()));
    page.add(&build_graphics_group(config.clone()));
    page.add(&build_socket_group(parent, config.clone()));
    page.add(&build_remote_group(parent, config.clone()));
    dialog.add(&page);

    // Applying recreates the container, so it is an explicit action rather
    // than something that happens as each row changes.
    let apply = gtk::Button::builder()
        .label("Apply and Restart")
        .css_classes(vec!["suggested-action", "pill"])
        .margin_top(12)
        .halign(gtk::Align::Center)
        .build();

    let group = adw::PreferencesGroup::new();
    group.add(&apply);
    page.add(&group);

    let parent_widget = parent.as_ref().clone();
    let dialog_ref = dialog.clone();
    apply.connect_clicked(move |_| {
        let updated = config.borrow().clone();
        if !desktop.config.needs_recreate(&updated) {
            dialog_ref.close();
            return;
        }
        confirm_and_apply(
            &parent_widget,
            podman.clone(),
            desktop.clone(),
            updated,
            on_applied.clone(),
            dialog_ref.clone(),
        );
    });

    dialog.present(Some(parent));
}

fn build_mounts_group(
    parent: &impl IsA<gtk::Widget>,
    config: Rc<RefCell<DesktopConfig>>,
) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder()
        .title("Shared Files and Folders")
        .description(
            "Items from this computer, made visible inside the desktop. Each one \
             appears at a location you choose.",
        )
        .build();

    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(vec!["boxed-list"])
        .build();

    let rebuild = {
        let list = list.clone();
        let config = config.clone();
        Rc::new(move || refresh_mount_rows(&list, config.clone()))
    };
    rebuild();

    let buttons = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .halign(gtk::Align::Start)
        .margin_top(6)
        .build();
    buttons.append(&add_button(
        parent,
        config.clone(),
        rebuild.clone(),
        MountKind::Directory,
    ));
    buttons.append(&add_button(parent, config, rebuild, MountKind::File));

    let box_ = gtk::Box::new(gtk::Orientation::Vertical, 0);
    box_.append(&list);
    box_.append(&buttons);
    group.add(&box_);
    group
}

/// A button that adds one share, of either kind.
///
/// Sharing a single file is worth having on its own. A configuration file such
/// as `.gitconfig` should not need its whole directory shared with it.
fn add_button(
    parent: &impl IsA<gtk::Widget>,
    config: Rc<RefCell<DesktopConfig>>,
    rebuild: Rc<impl Fn() + 'static>,
    kind: MountKind,
) -> gtk::Button {
    let button = gtk::Button::builder()
        .label(match kind {
            MountKind::Directory => "Add Folder",
            MountKind::File => "Add File",
        })
        .build();

    let parent_widget = parent.as_ref().clone();
    button.connect_clicked(move |_| {
        // The file chooser portal returns a path without Cabin needing any
        // filesystem permission of its own. See DESIGN.md section 7.1.
        let chooser = gtk::FileDialog::builder()
            .title(match kind {
                MountKind::Directory => "Choose a folder to share",
                MountKind::File => "Choose a file to share",
            })
            .build();
        let config = config.clone();
        let rebuild = rebuild.clone();
        let window = parent_widget.root().and_downcast::<gtk::Window>();

        let chosen = move |result: Result<gtk::gio::File, gtk::glib::Error>| {
            let Ok(file) = result else { return };
            let Some(path) = file.path() else { return };
            let host_path = path.to_string_lossy().into_owned();
            let guest_path = Mount::suggest_guest_path(&host_path);
            config.borrow_mut().mounts.push(Mount {
                host_path,
                guest_path,
                mode: MountMode::ReadOnly,
                relabel: true,
                kind,
            });
            rebuild();
        };

        match kind {
            MountKind::Directory => {
                chooser.select_folder(window.as_ref(), gtk::gio::Cancellable::NONE, chosen)
            }
            MountKind::File => chooser.open(window.as_ref(), gtk::gio::Cancellable::NONE, chosen),
        }
    });
    button
}

fn refresh_mount_rows(list: &gtk::ListBox, config: Rc<RefCell<DesktopConfig>>) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }

    if config.borrow().mounts.is_empty() {
        let empty = adw::ActionRow::builder()
            .title("Nothing shared")
            .subtitle("The desktop can only see its own files.")
            .build();
        list.append(&empty);
        return;
    }

    let count = config.borrow().mounts.len();
    for index in 0..count {
        let mount = config.borrow().mounts[index].clone();
        list.append(&build_mount_row(index, &mount, config.clone(), list));
    }
}

/// One share, opening to show where it appears inside the desktop.
///
/// The location is editable. A suggestion from the source name is right most
/// of the time and wrong whenever the desktop expects a particular path, and
/// a configuration file shared to the wrong place does nothing at all.
fn build_mount_row(
    index: usize,
    mount: &Mount,
    config: Rc<RefCell<DesktopConfig>>,
    list: &gtk::ListBox,
) -> adw::ExpanderRow {
    let row = adw::ExpanderRow::builder()
        .title(&mount.host_path)
        .subtitle(format!("Appears at {}", mount.guest_path))
        .build();
    row.add_prefix(&gtk::Image::from_icon_name(match mount.kind {
        MountKind::Directory => "folder-symbolic",
        MountKind::File => "text-x-generic-symbolic",
    }));

    let remove = gtk::Button::builder()
        .icon_name("user-trash-symbolic")
        .valign(gtk::Align::Center)
        .css_classes(vec!["flat"])
        .tooltip_text("Stop sharing this")
        .build();
    let config_for_remove = config.clone();
    let list_for_remove = list.clone();
    remove.connect_clicked(move |_| {
        config_for_remove.borrow_mut().mounts.remove(index);
        refresh_mount_rows(&list_for_remove, config_for_remove.clone());
    });
    row.add_suffix(&remove);

    let location = adw::EntryRow::builder()
        .title("Location inside the desktop")
        .text(&mount.guest_path)
        .build();
    // Applying is a separate action, so a half typed path must not be stored
    // as though it were finished. The row keeps the last valid value and says
    // when what is typed is not one.
    let config_for_location = config.clone();
    let row_for_location = row.clone();
    location.connect_changed(move |entry| {
        let typed = entry.text().to_string();
        match Mount::check_guest_path(&typed) {
            Ok(()) => {
                entry.remove_css_class("error");
                entry.set_tooltip_text(None);
                if let Some(m) = config_for_location.borrow_mut().mounts.get_mut(index) {
                    m.guest_path = typed.trim().to_string();
                }
                row_for_location.set_subtitle(&format!("Appears at {}", entry.text()));
            }
            Err(why) => {
                entry.add_css_class("error");
                entry.set_tooltip_text(Some(why));
                row_for_location.set_subtitle(why);
            }
        }
    });
    row.add_row(&location);

    // Read-only is the default, so the safe choice needs no thought.
    let writable = adw::SwitchRow::builder()
        .title("Writable")
        .subtitle("Let the desktop change what is shared.")
        .active(mount.mode == MountMode::ReadWrite)
        .build();
    let config_for_switch = config.clone();
    writable.connect_active_notify(move |sw| {
        if let Some(m) = config_for_switch.borrow_mut().mounts.get_mut(index) {
            m.mode = if sw.is_active() {
                MountMode::ReadWrite
            } else {
                MountMode::ReadOnly
            };
        }
    });
    row.add_row(&writable);

    if mount.kind == MountKind::File {
        // Worth stating, because the file looks shared and then stops
        // following the original with no sign that anything is wrong.
        let note = adw::ActionRow::builder()
            .title("Editing on this computer may not show up")
            .subtitle(
                "A shared file is bound by itself. Most editors replace a file \
                 rather than change it, and the desktop keeps seeing the \
                 original. Share the folder instead if you plan to edit it.",
            )
            .css_classes(vec!["warning"])
            .build();
        note.set_subtitle_lines(0);
        row.add_row(&note);
    }

    row
}

fn build_graphics_group(config: Rc<RefCell<DesktopConfig>>) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder().title("Graphics").build();

    let nodes = graphics::render_nodes();
    let mut labels: Vec<String> = vec!["Automatic".into(), "Software rendering".into()];
    for (path, driver) in &nodes {
        let leaf = path.rsplit('/').next().unwrap_or(path.as_str());
        labels.push(format!("{leaf} ({driver})"));
    }
    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();

    let selected = match &config.borrow().graphics {
        GraphicsMode::Automatic => 0,
        GraphicsMode::Software => 1,
        GraphicsMode::RenderNode { path } => nodes
            .iter()
            .position(|(p, _)| p == path)
            .map(|i| i as u32 + 2)
            .unwrap_or(0),
    };

    let row = adw::ComboRow::builder()
        .title("Renderer")
        .subtitle("A graphics card makes the desktop smoother. Software rendering always works.")
        .model(&gtk::StringList::new(&refs))
        .selected(selected)
        .build();

    row.connect_selected_notify(move |row| {
        config.borrow_mut().graphics = match row.selected() {
            0 => GraphicsMode::Automatic,
            1 => GraphicsMode::Software,
            n => GraphicsMode::RenderNode {
                path: nodes[(n - 2) as usize].0.clone(),
            },
        };
    });

    group.add(&row);
    group
}

fn build_socket_group(
    parent: &impl IsA<gtk::Widget>,
    config: Rc<RefCell<DesktopConfig>>,
) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder()
        .title("Container Runtime")
        .description(
            "The desktop has no container runtime of its own. This shares the one \
             already on this computer, so containers it starts run outside the \
             desktop and outside its sandbox.",
        )
        .build();

    let labels: Vec<&str> = SOCKET_CHOICES.iter().map(|(_, label, _)| *label).collect();
    let selected = SOCKET_CHOICES
        .iter()
        .position(|(policy, _, _)| *policy == config.borrow().socket)
        .unwrap_or(0) as u32;

    let row = adw::ComboRow::builder()
        .title("Share a runtime with this desktop")
        .subtitle(SOCKET_CHOICES[selected as usize].2)
        .model(&gtk::StringList::new(&labels))
        .selected(selected)
        .build();

    let parent_widget = parent.as_ref().clone();
    let previous = Rc::new(std::cell::Cell::new(selected));
    row.connect_selected_notify(move |row| {
        let index = row.selected() as usize;
        let (policy, _, description) = SOCKET_CHOICES[index];
        row.set_subtitle(description);

        // The dangerous options warn before they take, and the row snaps back
        // if the warning is declined. See DESIGN.md section 7.2.
        if let Some(warning) = policy.warning() {
            let dialog = adw::AlertDialog::builder()
                .heading("This is dangerous")
                .body(warning)
                .build();
            dialog.add_response("cancel", "Cancel");
            dialog.add_response("accept", "I understand the risk");
            dialog.set_response_appearance("accept", adw::ResponseAppearance::Destructive);
            dialog.set_default_response(Some("cancel"));

            let config = config.clone();
            let row = row.clone();
            let previous = previous.clone();
            dialog.connect_response(None, move |_, response| {
                if response == "accept" {
                    config.borrow_mut().socket = policy;
                    previous.set(row.selected());
                } else {
                    row.set_selected(previous.get());
                }
            });
            dialog.present(Some(&parent_widget));
            return;
        }

        config.borrow_mut().socket = policy;
        previous.set(row.selected());
    });

    group.add(&row);
    group
}

/// Whether other devices can reach this desktop, and over which networks.
///
/// The choice is two questions, not one. The first is whether to leave
/// loopback at all. The second is which networks to accept, which matters
/// because a laptop joins networks it does not trust. See DESIGN.md section
/// 7.5.
fn build_remote_group(
    parent: &impl IsA<gtk::Widget>,
    config: Rc<RefCell<DesktopConfig>>,
) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder()
        .title("Remote Access")
        .description("Connect to this desktop from a phone or another computer.")
        .build();

    let interfaces = crate::network::selectable();

    // Everything below the master switch, hidden while it is off.
    let detail = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(12)
        .build();

    let scope = adw::ComboRow::builder()
        .title("Networks")
        .model(&gtk::StringList::new(&[
            "Every network",
            "Chosen networks only",
        ]))
        .build();
    let scope_list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(vec!["boxed-list"])
        .build();
    scope_list.append(&scope);
    detail.append(&scope_list);

    let chooser = build_interface_list(&interfaces, config.clone());
    detail.append(&chooser);

    let switch = adw::SwitchRow::builder()
        .title("Allow connections from other devices")
        .subtitle("Off means only this computer can connect.")
        .active(config.borrow().exposure.reaches_other_devices())
        .build();

    // Reflect what was saved.
    match &config.borrow().exposure {
        Exposure::Loopback => {}
        Exposure::Everywhere => scope.set_selected(0),
        Exposure::Interfaces { .. } => scope.set_selected(1),
    }
    detail.set_visible(switch.is_active());
    chooser.set_visible(scope.selected() == 1);

    let parent_widget = parent.as_ref().clone();
    let config_for_switch = config.clone();
    let detail_for_switch = detail.clone();
    let scope_for_switch = scope.clone();
    switch.connect_active_notify(move |switch| {
        if !switch.is_active() {
            config_for_switch.borrow_mut().exposure = Exposure::Loopback;
            detail_for_switch.set_visible(false);
            return;
        }

        // Going from loopback to a network is worth one deliberate
        // confirmation, because it is not obvious that the desktop becomes
        // reachable by anything that can route to this machine.
        let dialog = adw::AlertDialog::builder()
            .heading("Open this desktop to a network?")
            .body(
                "Other devices will be able to reach the login screen. The desktop \
                 still needs its password, and the connection is encrypted. Choose \
                 which networks next.",
            )
            .build();
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("allow", "Allow");
        dialog.set_response_appearance("allow", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));

        let config = config_for_switch.clone();
        let switch = switch.clone();
        let detail = detail_for_switch.clone();
        let scope = scope_for_switch.clone();
        dialog.connect_response(None, move |_, response| {
            if response != "allow" {
                switch.set_active(false);
                return;
            }
            detail.set_visible(true);
            // Chosen networks is the safer of the two, so it is what the
            // switch lands on. The user can widen it deliberately.
            scope.set_selected(1);
            config.borrow_mut().exposure = Exposure::Interfaces { names: Vec::new() };
        });
        dialog.present(Some(&parent_widget));
    });

    let parent_widget = parent.as_ref().clone();
    let chooser_for_scope = chooser.clone();
    let config_for_scope = config.clone();
    scope.connect_selected_notify(move |scope| {
        if scope.selected() == 1 {
            chooser_for_scope.set_visible(true);
            let names = current_selection(&chooser_for_scope);
            config_for_scope.borrow_mut().exposure = Exposure::Interfaces { names };
            return;
        }

        // Every network includes the ones this computer has not joined yet.
        let dialog = adw::AlertDialog::builder()
            .heading("Listen on every network?")
            .body(
                "The desktop will accept connections on every network this computer \
                 joins. That includes public Wi-Fi in cafes, hotels and airports, \
                 where other people are on the same network and can reach the login \
                 screen.\n\n\
                 Choosing networks instead limits it to the ones you name.",
            )
            .build();
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("accept", "Listen on every network");
        dialog.set_response_appearance("accept", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));

        let config = config_for_scope.clone();
        let scope_ref = scope.clone();
        let chooser = chooser_for_scope.clone();
        dialog.connect_response(None, move |_, response| {
            if response == "accept" {
                config.borrow_mut().exposure = Exposure::Everywhere;
                chooser.set_visible(false);
            } else {
                scope_ref.set_selected(1);
            }
        });
        dialog.present(Some(&parent_widget));
    });

    let box_ = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let switch_list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(vec!["boxed-list"])
        .build();
    switch_list.append(&switch);
    box_.append(&switch_list);
    box_.append(&detail);
    group.add(&box_);
    group
}

/// One switch per interface, with the addresses it currently holds.
///
/// The addresses are shown because interface names alone do not say which
/// network is which. `wlp3s0` means nothing; `192.168.1.24` is recognisable.
fn build_interface_list(
    interfaces: &[cabin_core::NetInterface],
    config: Rc<RefCell<DesktopConfig>>,
) -> gtk::ListBox {
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(vec!["boxed-list"])
        .build();

    if interfaces.is_empty() {
        let empty = adw::ActionRow::builder()
            .title("No networks found")
            .subtitle("This computer is not connected to a network.")
            .build();
        list.append(&empty);
        return list;
    }

    let chosen: Vec<String> = config.borrow().exposure.chosen_interfaces().to_vec();
    for interface in interfaces {
        let row = adw::SwitchRow::builder()
            .title(&interface.name)
            .subtitle(interface.addresses.join(", "))
            .active(chosen.contains(&interface.name))
            .build();
        let config = config.clone();
        let list_ref = list.clone();
        row.connect_active_notify(move |row| {
            // Read every row rather than tracking one, so the stored list
            // always matches what the user can see.
            let names = current_selection(&list_ref);
            config.borrow_mut().exposure = Exposure::Interfaces { names };
            let _ = row;
        });
        list.append(&row);
    }
    list
}

/// The interface names whose switches are on.
fn current_selection(list: &gtk::ListBox) -> Vec<String> {
    let mut names = Vec::new();
    let mut child = list.first_child();
    while let Some(widget) = child {
        child = widget.next_sibling();
        if let Some(row) = widget.downcast_ref::<adw::SwitchRow>() {
            if row.is_active() {
                names.push(row.title().to_string());
            }
        }
    }
    names
}

fn confirm_and_apply(
    parent: &gtk::Widget,
    podman: Arc<Podman>,
    desktop: Desktop,
    updated: DesktopConfig,
    on_applied: Rc<dyn Fn()>,
    settings_dialog: adw::PreferencesDialog,
) {
    let dialog = adw::AlertDialog::builder()
        .heading("Restart the desktop?")
        .body(
            "These changes need the desktop to be recreated, so anything running \
             in it will close. Your files are kept.",
        )
        .build();
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("apply", "Apply and Restart");
    dialog.set_response_appearance("apply", adw::ResponseAppearance::Destructive);

    let parent_for_closure = parent.clone();
    dialog.connect_response(None, move |_, response| {
        if response != "apply" {
            return;
        }
        let podman = podman.clone();
        let desktop = desktop.clone();
        let updated = updated.clone();
        let on_applied = on_applied.clone();
        let nodes = graphics::render_nodes();
        let interfaces = crate::network::interfaces();
        let parent = parent_for_closure.clone();
        let settings_dialog = settings_dialog.clone();

        runtime::spawn(
            async move { recreate(podman, desktop, updated, nodes, interfaces).await },
            move |result| match result {
                Ok(()) => {
                    settings_dialog.close();
                    on_applied();
                }
                Err(message) => {
                    let error = adw::AlertDialog::builder()
                        .heading("Could not apply the changes")
                        .body(message)
                        .build();
                    error.add_response("close", "Close");
                    error.present(Some(&parent));
                }
            },
        );
    });
    dialog.present(Some(parent));
}

/// Stops, removes and recreates a desktop with a new configuration.
///
/// The home volume is not touched, so the user's files survive.
async fn recreate(
    podman: Arc<Podman>,
    desktop: Desktop,
    updated: DesktopConfig,
    nodes: Vec<(String, String)>,
    interfaces: Vec<cabin_core::NetInterface>,
) -> Result<(), String> {
    let password = podman
        .desktop_password(&desktop.id)
        .await
        .map_err(|e| format!("could not read the desktop password: {e}"))?;
    let port = desktop.rdp_port.unwrap_or(13389);
    if desktop.state.is_running() {
        podman
            .stop(&desktop.id, 20)
            .await
            .map_err(|e| format!("could not stop the desktop: {e}"))?;
    }
    podman
        .remove(&desktop.id, true)
        .await
        .map_err(|e| format!("could not remove the old container: {e}"))?;

    let id = podman
        .create_desktop(&updated, port, &nodes, &interfaces, &password)
        .await
        .map_err(|e| format!("could not recreate the desktop: {e}"))?;
    podman
        .start_desktop(&id, &updated.name)
        .await
        .map_err(|e| format!("the desktop was recreated but would not start.\n\n{e}"))
}
