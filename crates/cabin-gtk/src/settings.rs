//! Per-desktop settings: mounts, graphics, and container runtime sockets.
//!
//! Changing any of these needs the container recreated, which is why the home
//! directory lives in a named volume. See `cabin_core::home_volume_name`.

use crate::graphics;
use crate::runtime;
use adw::prelude::*;
use cabin_core::{Desktop, DesktopConfig, GraphicsMode, Mount, MountMode, SocketPolicy};
use cabin_podman::Podman;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

/// The socket options, in the order they appear, safest first.
const SOCKET_CHOICES: &[(SocketPolicy, &str, &str)] = &[
    (
        SocketPolicy::None,
        "None",
        "No container runtime inside this desktop.",
    ),
    (
        SocketPolicy::Nested,
        "Podman inside the desktop",
        "Recommended. The desktop runs its own rootless Podman.",
    ),
    (
        SocketPolicy::HostPodman,
        "Host Podman socket",
        "Dangerous. Grants the power of your user account.",
    ),
    (
        SocketPolicy::HostDocker,
        "Host Docker socket",
        "Dangerous. Grants root on your computer.",
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
        .title("Shared Folders")
        .description("Folders from this computer, made visible inside the desktop.")
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

    let add = gtk::Button::builder()
        .label("Add Folder")
        .halign(gtk::Align::Start)
        .margin_top(6)
        .build();

    let parent_widget = parent.as_ref().clone();
    let rebuild_after_add = rebuild.clone();
    add.connect_clicked(move |_| {
        // The file chooser portal returns a path without Cabin needing any
        // filesystem permission of its own. See DESIGN.md section 7.1.
        let chooser = gtk::FileDialog::builder()
            .title("Choose a folder to share")
            .build();
        let config = config.clone();
        let rebuild = rebuild_after_add.clone();
        let window = parent_widget.root().and_downcast::<gtk::Window>();
        chooser.select_folder(
            window.as_ref(),
            gtk::gio::Cancellable::NONE,
            move |result: Result<gtk::gio::File, _>| {
                let Ok(file) = result else { return };
                let Some(path) = file.path() else { return };
                let host_path = path.to_string_lossy().into_owned();
                let guest_path = Mount::suggest_guest_path(&host_path);
                config.borrow_mut().mounts.push(Mount {
                    host_path,
                    guest_path,
                    mode: MountMode::ReadOnly,
                    relabel: true,
                });
                rebuild();
            },
        );
    });

    let box_ = gtk::Box::new(gtk::Orientation::Vertical, 0);
    box_.append(&list);
    box_.append(&add);
    group.add(&box_);
    group
}

fn refresh_mount_rows(list: &gtk::ListBox, config: Rc<RefCell<DesktopConfig>>) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }

    if config.borrow().mounts.is_empty() {
        let empty = adw::ActionRow::builder()
            .title("No shared folders")
            .subtitle("The desktop can only see its own files.")
            .build();
        list.append(&empty);
        return;
    }

    let count = config.borrow().mounts.len();
    for index in 0..count {
        let mount = config.borrow().mounts[index].clone();
        let row = adw::ActionRow::builder()
            .title(&mount.host_path)
            .subtitle(format!("Appears at {}", mount.guest_path))
            .build();

        // Read-only is the default, so the safe choice needs no thought.
        let writable = gtk::Switch::builder()
            .active(mount.mode == MountMode::ReadWrite)
            .valign(gtk::Align::Center)
            .tooltip_text("Allow the desktop to change these files")
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
        row.add_suffix(&gtk::Label::new(Some("Writable")));
        row.add_suffix(&writable);

        let remove = gtk::Button::builder()
            .icon_name("user-trash-symbolic")
            .valign(gtk::Align::Center)
            .css_classes(vec!["flat"])
            .tooltip_text("Stop sharing this folder")
            .build();
        let config_for_remove = config.clone();
        let list_for_remove = list.clone();
        remove.connect_clicked(move |_| {
            config_for_remove.borrow_mut().mounts.remove(index);
            refresh_mount_rows(&list_for_remove, config_for_remove.clone());
        });
        row.add_suffix(&remove);

        list.append(&row);
    }
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
        .title("Containers")
        .description("Whether this desktop can build and run containers.")
        .build();

    let labels: Vec<&str> = SOCKET_CHOICES.iter().map(|(_, label, _)| *label).collect();
    let selected = SOCKET_CHOICES
        .iter()
        .position(|(policy, _, _)| *policy == config.borrow().socket)
        .unwrap_or(0) as u32;

    let row = adw::ComboRow::builder()
        .title("Container runtime")
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

/// Whether other devices can reach this desktop.
fn build_remote_group(
    parent: &impl IsA<gtk::Widget>,
    config: Rc<RefCell<DesktopConfig>>,
) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder()
        .title("Remote Access")
        .description("Connect to this desktop from a phone or another computer.")
        .build();

    let row = adw::SwitchRow::builder()
        .title("Allow connections from other devices")
        .subtitle("Off means only this computer can connect.")
        .active(config.borrow().expose_on_lan)
        .build();

    let parent_widget = parent.as_ref().clone();
    row.connect_active_notify(move |row| {
        if !row.is_active() {
            config.borrow_mut().expose_on_lan = false;
            return;
        }

        // Going from loopback to the whole network is worth one deliberate
        // confirmation, because it is not obvious that the desktop becomes
        // reachable by anything that can route to this machine.
        let dialog = adw::AlertDialog::builder()
            .heading("Open this desktop to your network?")
            .body(
                "Anything that can reach this computer will be able to attempt a \
                 connection. The desktop still needs its password, and the \
                 connection is encrypted, but it will no longer be reachable only \
                 from here.",
            )
            .build();
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("allow", "Allow");
        dialog.set_response_appearance("allow", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));

        let config = config.clone();
        let row = row.clone();
        dialog.connect_response(None, move |_, response| {
            if response == "allow" {
                config.borrow_mut().expose_on_lan = true;
            } else {
                row.set_active(false);
            }
        });
        dialog.present(Some(&parent_widget));
    });

    group.add(&row);
    group
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
        let parent = parent_for_closure.clone();
        let settings_dialog = settings_dialog.clone();

        runtime::spawn(
            async move { recreate(podman, desktop, updated, nodes).await },
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
        .create_desktop(&updated, port, &nodes, &password)
        .await
        .map_err(|e| format!("could not recreate the desktop: {e}"))?;
    podman
        .start_desktop(&id, &updated.name)
        .await
        .map_err(|e| format!("the desktop was recreated but would not start.\n\n{e}"))
}
