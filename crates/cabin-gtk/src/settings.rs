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

/// Why a single file share can stop following the original.
///
/// A bind mount of one file follows the inode. Most editors write a new file
/// and rename it over the old one, which leaves the desktop looking at the
/// file that was replaced.
const FILE_SHARE_WARNING: &str =
    "A shared file is bound by itself. Most editors replace a file rather than \
     change it, and the desktop keeps seeing the original. Share the folder \
     instead if you plan to edit it.";

/// What the relabel switch does. Podman's "z" option changes the SELinux
/// label of the copy on this computer, and the change stays after the
/// desktop is deleted.
const RELABEL_SUBTITLE: &str =
    "Changes the SELinux label on this computer's copy, and the change stays \
     after the desktop is gone. Fedora needs this. Do not use it on folders \
     that other services read, such as .ssh.";

/// Folders, relative to the home directory, that other programs read and
/// that a changed SELinux label can break.
const SENSITIVE_FOLDERS: &[&str] = &[
    ".ssh",
    ".gnupg",
    ".config",
    ".local/share/keyrings",
    "public_html",
];

/// True when `host_path` is one of the sensitive folders, or inside one.
fn is_sensitive_location(host_path: &str, home: &std::path::Path) -> bool {
    let path = std::path::Path::new(host_path);
    SENSITIVE_FOLDERS
        .iter()
        .any(|folder| path.starts_with(home.join(folder)))
}

/// What the dialogue says when the portal gives no real path.
const UNRESOLVED_SHARE: &str =
    "This location cannot be shared because the system did not give its real path.";

/// The places where the document portal shows files to a sandboxed
/// application.
fn document_roots() -> Vec<std::path::PathBuf> {
    vec![
        gtk::glib::user_runtime_dir().join("doc"),
        std::path::PathBuf::from("/run/flatpak/doc"),
    ]
}

/// Splits a document portal path into the document id and the components
/// below the document itself.
///
/// The portal shows a document at `<root>/<id>/<name>`, where `<name>` is
/// the last component of the real path. A chosen folder can then have more
/// components below that. Returns `None` for a path outside every root.
fn split_document_path(
    path: &std::path::Path,
    roots: &[std::path::PathBuf],
) -> Option<(String, Vec<String>)> {
    let below = roots.iter().find_map(|root| path.strip_prefix(root).ok())?;
    let mut components = below
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned());
    let id = components.next()?;
    // The document's own name. The real path already ends with it.
    let _name = components.next();
    Some((id, components.collect()))
}

/// The real path of a document, with the components below it added.
fn join_host_path(document: &str, rest: &[String]) -> String {
    let mut path = std::path::PathBuf::from(document);
    for component in rest {
        path.push(component);
    }
    path.to_string_lossy().into_owned()
}

/// Asks the document portal for the real path of one document.
///
/// `GetHostPaths` answers only for documents this application can already
/// see, so it gives Cabin nothing new. Podman runs on the host and needs
/// the real path, because the portal's FUSE mount refuses every user but
/// this one, and the desktop's user is a different one.
async fn document_host_path(id: &str) -> Option<String> {
    use gtk::gio;
    use gtk::glib::variant::ToVariant;
    let bus = gio::bus_get_future(gio::BusType::Session).await.ok()?;
    let reply = bus
        .call_future(
            Some("org.freedesktop.portal.Documents"),
            "/org/freedesktop/portal/documents",
            "org.freedesktop.portal.Documents",
            "GetHostPaths",
            Some(&(vec![id.to_string()],).to_variant()),
            Some(gtk::glib::VariantTy::new("(a{say})").ok()?),
            gio::DBusCallFlags::NONE,
            5000,
        )
        .await
        .ok()?;
    let (paths,): (std::collections::HashMap<String, Vec<u8>>,) = reply.get()?;
    let mut bytes = paths.get(id)?.clone();
    // A GLib byte string ends with a NUL.
    while bytes.last() == Some(&0) {
        bytes.pop();
    }
    let path = String::from_utf8(bytes).ok()?;
    (!path.is_empty()).then_some(path)
}

/// The real path on this computer of a file the chooser returned.
///
/// In the Flatpak the chooser returns a document portal path. Host Podman
/// cannot give that to the desktop: the portal's FUSE mount refuses every
/// user except this one. So the path is turned back into the real one, and
/// a path that cannot be turned back is refused, never used as it is.
/// Widening the sandbox instead is ruled out by CLAUDE.md rule 6.
async fn host_path_for(file: gtk::gio::File) -> Result<String, &'static str> {
    let Some(path) = file.path() else {
        return Err(UNRESOLVED_SHARE);
    };
    let Some((id, rest)) = split_document_path(&path, &document_roots()) else {
        return Ok(path.to_string_lossy().into_owned());
    };
    if let Some(document) = document_host_path(&id).await {
        return Ok(join_host_path(&document, &rest));
    }
    // Older portals have no GetHostPaths, but put the real path in an
    // extended attribute on each file.
    const ATTRIBUTE: &str = "xattr::document-portal.host-path";
    let info = file
        .query_info_future(
            ATTRIBUTE,
            gtk::gio::FileQueryInfoFlags::NONE,
            gtk::glib::Priority::DEFAULT,
        )
        .await
        .map_err(|_| UNRESOLVED_SHARE)?;
    match info.attribute_string(ATTRIBUTE) {
        Some(real) if real.starts_with('/') => Ok(real.to_string()),
        _ => Err(UNRESOLVED_SHARE),
    }
}

/// True for a stored share that points into the document portal. An older
/// build stored such paths, and the desktop's user cannot read them.
fn is_portal_path(host_path: &str) -> bool {
    split_document_path(std::path::Path::new(host_path), &document_roots()).is_some()
}

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

    // Everything in this dialogue needs the container recreated, so applying
    // is an explicit action rather than something that happens as each row
    // changes. Two actions, because applying and running are separate
    // questions: a desktop can be reconfigured without being started.
    let running = desktop.state.is_running();

    let apply = gtk::Button::builder()
        .label("Apply")
        .tooltip_text(if running {
            "Apply the changes and leave the desktop stopped"
        } else {
            "Apply the changes without starting the desktop"
        })
        .css_classes(vec!["pill"])
        .build();
    let apply_and_run = gtk::Button::builder()
        .label(if running {
            "Apply and Restart"
        } else {
            "Apply and Start"
        })
        .css_classes(vec!["suggested-action", "pill"])
        .build();

    let buttons = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .halign(gtk::Align::Center)
        .margin_top(12)
        .build();
    buttons.append(&apply);
    buttons.append(&apply_and_run);

    // Says why Apply is off. Podman refuses a configuration with two shares
    // on one location, and the refusal would come only after the click.
    let problem = gtk::Label::builder()
        .wrap(true)
        .justify(gtk::Justification::Center)
        .css_classes(vec!["error"])
        .margin_top(12)
        .visible(false)
        .build();
    let validity = Rc::new(Validity {
        config: config.clone(),
        invalid_rows: RefCell::new(std::collections::BTreeSet::new()),
        buttons: vec![apply.clone(), apply_and_run.clone()],
        problem: problem.clone(),
    });

    let page = adw::PreferencesPage::new();
    page.add(&build_mounts_group(
        parent,
        config.clone(),
        validity.clone(),
    ));
    page.add(&build_graphics_group(config.clone()));
    page.add(&build_socket_group(
        parent,
        config.clone(),
        validity.clone(),
    ));
    page.add(&build_remote_group(parent, config.clone()));
    dialog.add(&page);
    validity.update();

    let group = adw::PreferencesGroup::builder()
        .description(if running {
            // Say it plainly. Podman fixes a container's configuration when it
            // is created, so there is no way to save these and keep running.
            "These settings are part of the container, so applying them builds \
             it again. Anything open in the desktop will close, and your files \
             are kept."
        } else {
            "Applying builds the desktop again. Your files are kept."
        })
        .build();
    group.add(&buttons);
    group.add(&problem);
    page.add(&group);

    for (button, start_after) in [(&apply, false), (&apply_and_run, true)] {
        let parent_widget = parent.as_ref().clone();
        let dialog_ref = dialog.clone();
        let podman = podman.clone();
        let on_applied = on_applied.clone();
        let desktop = desktop.clone();
        let config = config.clone();
        button.connect_clicked(move |_| {
            let updated = config.borrow().clone();
            if !desktop.config.needs_recreate(&updated) {
                // Nothing to apply. The run action still means something, so
                // it starts or restarts what is already there.
                if start_after {
                    run_only(
                        &parent_widget,
                        podman.clone(),
                        desktop.clone(),
                        on_applied.clone(),
                    );
                }
                dialog_ref.close();
                return;
            }
            confirm_and_apply(
                &parent_widget,
                podman.clone(),
                desktop.clone(),
                updated,
                start_after,
                on_applied.clone(),
                dialog_ref.clone(),
            );
        });
    }

    dialog.present(Some(parent));
}

/// Whether the Apply buttons can be used, and why not.
///
/// The settings change one at a time, and some faults only show in the whole
/// set: two shares on one location, or a share on the location that a newly
/// chosen socket takes. So the whole set is checked again after each change.
struct Validity {
    config: Rc<RefCell<DesktopConfig>>,
    /// Share rows whose typed location cannot be used. Such a row keeps the
    /// last good location in the configuration, so Apply would silently use
    /// a location the user can no longer see.
    invalid_rows: RefCell<std::collections::BTreeSet<usize>>,
    buttons: Vec<gtk::Button>,
    problem: gtk::Label,
}

impl Validity {
    fn update(&self) {
        let problem = if self.invalid_rows.borrow().is_empty() {
            self.config.borrow().check_mounts().err()
        } else {
            Some("A shared item has a location that cannot be used.".to_string())
        };
        for button in &self.buttons {
            button.set_sensitive(problem.is_none());
        }
        self.problem.set_visible(problem.is_some());
        self.problem.set_text(problem.as_deref().unwrap_or(""));
    }

    /// Records whether the share row at `index` holds a usable location.
    fn set_row(&self, index: usize, valid: bool) {
        if valid {
            self.invalid_rows.borrow_mut().remove(&index);
        } else {
            self.invalid_rows.borrow_mut().insert(index);
        }
        self.update();
    }

    /// Forgets every row, because the rows are about to be built again.
    fn clear_rows(&self) {
        self.invalid_rows.borrow_mut().clear();
    }
}

/// Starts or restarts a desktop whose settings did not change.
fn run_only(parent: &gtk::Widget, podman: Arc<Podman>, desktop: Desktop, on_applied: Rc<dyn Fn()>) {
    let parent = parent.clone();
    let running = desktop.state.is_running();
    // The chosen networks resolve to addresses again, because those can
    // have changed since the container was made (rule 40).
    let nodes = graphics::render_nodes();
    let interfaces = crate::network::interfaces();
    runtime::spawn(
        async move {
            if running {
                podman
                    .restart_with_current_addresses(&desktop, &nodes, &interfaces)
                    .await
            } else {
                podman
                    .start_with_current_addresses(&desktop, &nodes, &interfaces)
                    .await
            }
        },
        move |result| {
            if let Err(e) = result {
                let error = adw::AlertDialog::builder()
                    .heading("The desktop did not start")
                    .body(e.to_string())
                    .build();
                error.add_response("close", "Close");
                error.present(Some(&parent));
            }
            on_applied();
        },
    );
}

fn build_mounts_group(
    parent: &impl IsA<gtk::Widget>,
    config: Rc<RefCell<DesktopConfig>>,
    validity: Rc<Validity>,
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
        Rc::new(move || refresh_mount_rows(&list, config.clone(), validity.clone()))
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

        let parent_for_options = parent_widget.clone();
        let chosen = move |result: Result<gtk::gio::File, gtk::glib::Error>| {
            let Ok(file) = result else { return };
            let parent = parent_for_options.clone();
            let config = config.clone();
            let rebuild = rebuild.clone();
            gtk::glib::spawn_future_local(async move {
                let host_path = match host_path_for(file).await {
                    Ok(path) => path,
                    Err(why) => {
                        let error = adw::AlertDialog::builder()
                            .heading("Could not share this")
                            .body(why)
                            .build();
                        error.add_response("close", "Close");
                        error.present(Some(&parent));
                        return;
                    }
                };
                // Ask for the options now rather than adding a share with
                // defaults the user then has to go and find.
                present_mount_options(
                    &parent,
                    Mount {
                        guest_path: Mount::suggest_guest_path(&host_path),
                        host_path,
                        mode: MountMode::ReadOnly,
                        relabel: true,
                        kind,
                    },
                    config,
                    rebuild,
                );
            });
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

/// Asks for a new share's options before it is added.
///
/// The same options are on the row afterwards, so nothing here is a one time
/// decision. Asking now means the common case, a share that needs a location
/// other than the suggested one, is done in one pass.
fn present_mount_options(
    parent: &gtk::Widget,
    mount: Mount,
    config: Rc<RefCell<DesktopConfig>>,
    rebuild: Rc<impl Fn() + 'static>,
) {
    let dialog = adw::AlertDialog::builder()
        .heading(match mount.kind {
            MountKind::Directory => "Share This Folder",
            MountKind::File => "Share This File",
        })
        .body(&mount.host_path)
        .build();

    let group = adw::PreferencesGroup::new();

    let location = adw::EntryRow::builder()
        .title("Location inside the desktop")
        .text(&mount.guest_path)
        .build();
    group.add(&location);

    let writable = adw::SwitchRow::builder()
        .title("Writable")
        .subtitle("Let the desktop change what is shared.")
        .active(mount.mode == MountMode::ReadWrite)
        .build();
    group.add(&writable);

    // Podman's "z" option relabels the copy on this computer, recursively,
    // and nothing undoes it when the desktop goes. Say so where the choice
    // is made.
    let relabel = adw::SwitchRow::builder()
        .title("Adjust security labels")
        .subtitle(RELABEL_SUBTITLE)
        .active(mount.relabel)
        .build();
    relabel.set_subtitle_lines(0);
    group.add(&relabel);

    // A warning, not a block: the user may accept the risk. Other programs
    // can stop reading these folders once their label changes.
    if is_sensitive_location(&mount.host_path, &gtk::glib::home_dir()) {
        let note = adw::ActionRow::builder()
            .title("Other programs read this folder")
            .subtitle(
                "Changing its security label can stop them working. Turn off \
                 Adjust security labels, or share a different folder.",
            )
            .css_classes(vec!["warning"])
            .build();
        note.set_subtitle_lines(0);
        relabel
            .bind_property("active", &note, "visible")
            .sync_create()
            .build();
        group.add(&note);
    }

    if mount.kind == MountKind::File {
        let note = adw::ActionRow::builder()
            .title("Editing on this computer may not show up")
            .subtitle(FILE_SHARE_WARNING)
            .css_classes(vec!["warning"])
            .build();
        note.set_subtitle_lines(0);
        group.add(&note);
    }

    dialog.set_extra_child(Some(&group));
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("add", "Share");
    dialog.set_response_appearance("add", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("add"));
    dialog.set_close_response("cancel");

    // The location has to be usable before the share can be added, and an
    // AlertDialog closes on any response, so it is checked as it is typed.
    // The other shares count too: Podman refuses two mounts on one location.
    let others: Vec<String> = config
        .borrow()
        .mounts
        .iter()
        .map(|m| m.guest_path.clone())
        .collect();
    let dialog_for_check = dialog.clone();
    let group_for_check = group.clone();
    let check = move |entry: &adw::EntryRow| {
        let others: Vec<&str> = others.iter().map(String::as_str).collect();
        Mount::check_guest_path_among(&entry.text(), &others)
    };
    let check = move |entry: &adw::EntryRow| match check(entry) {
        Ok(()) => {
            entry.remove_css_class("error");
            group_for_check.set_description(None);
            dialog_for_check.set_response_enabled("add", true);
        }
        Err(why) => {
            entry.add_css_class("error");
            group_for_check.set_description(Some(why));
            dialog_for_check.set_response_enabled("add", false);
        }
    };
    location.connect_changed(check.clone());
    check(&location);

    dialog.connect_response(None, move |_, response| {
        if response != "add" {
            return;
        }
        let mut mount = mount.clone();
        mount.guest_path = location.text().trim().to_string();
        mount.mode = if writable.is_active() {
            MountMode::ReadWrite
        } else {
            MountMode::ReadOnly
        };
        mount.relabel = relabel.is_active();
        config.borrow_mut().mounts.push(mount);
        rebuild();
    });

    dialog.present(Some(parent));
}

fn refresh_mount_rows(
    list: &gtk::ListBox,
    config: Rc<RefCell<DesktopConfig>>,
    validity: Rc<Validity>,
) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
    // Every row starts from a location that was valid when it was stored.
    validity.clear_rows();
    validity.update();

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
        list.append(&build_mount_row(
            index,
            &mount,
            config.clone(),
            list,
            validity.clone(),
        ));
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
    validity: Rc<Validity>,
) -> adw::ExpanderRow {
    let row = adw::ExpanderRow::builder()
        .title(&mount.host_path)
        .subtitle(format!("Appears at {}", mount.guest_path))
        .build();
    // A path is data. One holding an ampersand or an angle bracket would
    // otherwise be read as markup and the label dropped with a warning.
    row.set_use_markup(false);
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
    let validity_for_remove = validity.clone();
    remove.connect_clicked(move |_| {
        config_for_remove.borrow_mut().mounts.remove(index);
        refresh_mount_rows(
            &list_for_remove,
            config_for_remove.clone(),
            validity_for_remove.clone(),
        );
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
        // Compared with every other share, because Podman refuses two
        // mounts on one location, and only after the dialogue has closed.
        let others: Vec<String> = config_for_location
            .borrow()
            .mounts
            .iter()
            .enumerate()
            .filter(|(other, _)| *other != index)
            .map(|(_, m)| m.guest_path.clone())
            .collect();
        let others: Vec<&str> = others.iter().map(String::as_str).collect();
        match Mount::check_guest_path_among(&typed, &others) {
            Ok(()) => {
                entry.remove_css_class("error");
                entry.set_tooltip_text(None);
                if let Some(m) = config_for_location.borrow_mut().mounts.get_mut(index) {
                    m.guest_path = typed.trim().to_string();
                }
                row_for_location.set_subtitle(&format!("Appears at {}", entry.text()));
                validity.set_row(index, true);
            }
            Err(why) => {
                entry.add_css_class("error");
                entry.set_tooltip_text(Some(why));
                row_for_location.set_subtitle(why);
                validity.set_row(index, false);
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
            .subtitle(FILE_SHARE_WARNING)
            .css_classes(vec!["warning"])
            .build();
        note.set_subtitle_lines(0);
        row.add_row(&note);
    }

    if is_portal_path(&mount.host_path) {
        // An older build stored the portal's path. The desktop's user cannot
        // read it, and nothing else says why the share is empty.
        let note = adw::ActionRow::builder()
            .title("The desktop cannot read this")
            .subtitle(
                "This is a temporary path from the file chooser, not the real \
                 location. Remove it and share the item again.",
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

    // The driver comes from sysfs, through render_nodes, never from the
    // node number (rule 2).
    let nodes = graphics::render_nodes();
    let stored = config.borrow().graphics.clone();
    let choices = graphics_choices(&nodes, &stored);
    let refs: Vec<&str> = choices.iter().map(|(label, _)| label.as_str()).collect();
    let selected = choices
        .iter()
        .position(|(_, mode)| *mode == stored)
        .unwrap_or(0) as u32;
    let missing = matches!(&stored, GraphicsMode::RenderNode { path }
        if !nodes.iter().any(|(p, _)| p == path));

    let row = adw::ComboRow::builder()
        .title("Renderer")
        .subtitle("A graphics card makes the desktop smoother. Software rendering always works.")
        .model(&gtk::StringList::new(&refs))
        .selected(selected)
        .build();
    // The entries carry device paths and driver names, which are data.
    row.set_use_markup(false);

    // A stored device that has gone stays selected and says so. Showing
    // Automatic instead would keep the old path in the configuration with
    // nothing on screen to say it is there.
    let warning = adw::ActionRow::builder()
        .title("This graphics device is not found")
        .subtitle("The desktop cannot start until you choose another renderer.")
        .css_classes(vec!["warning"])
        .visible(missing)
        .build();
    warning.set_subtitle_lines(0);

    let modes: Vec<GraphicsMode> = choices.into_iter().map(|(_, mode)| mode).collect();
    let warning_for_row = warning.clone();
    row.connect_selected_notify(move |row| {
        let Some(mode) = modes.get(row.selected() as usize) else {
            return;
        };
        warning_for_row.set_visible(*mode == stored && missing);
        config.borrow_mut().graphics = mode.clone();
    });

    group.add(&row);
    group.add(&warning);
    group
}

/// The renderer entries and the mode behind each one, in order.
///
/// The selection maps through this list, never through arithmetic on its
/// position. A stored render node that is not on this computer now gets an
/// entry of its own, so it stays visible and any other choice replaces it.
fn graphics_choices(
    nodes: &[(String, String)],
    stored: &GraphicsMode,
) -> Vec<(String, GraphicsMode)> {
    let leaf = |path: &str| path.rsplit('/').next().unwrap_or(path).to_string();
    let mut choices = vec![
        ("Automatic".to_string(), GraphicsMode::Automatic),
        ("Software rendering".to_string(), GraphicsMode::Software),
    ];
    for (path, driver) in nodes {
        choices.push((
            format!("{} ({driver})", leaf(path)),
            GraphicsMode::RenderNode { path: path.clone() },
        ));
    }
    if let GraphicsMode::RenderNode { path } = stored {
        if !nodes.iter().any(|(p, _)| p == path) {
            choices.push((format!("{} (not found)", leaf(path)), stored.clone()));
        }
    }
    choices
}

fn build_socket_group(
    parent: &impl IsA<gtk::Widget>,
    config: Rc<RefCell<DesktopConfig>>,
    validity: Rc<Validity>,
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

        // A declined warning snaps the row back, which lands here again. The
        // choice it returns to was already accepted, so it must not ask
        // again.
        if index as u32 == previous.get() {
            return;
        }

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
            let validity = validity.clone();
            dialog.connect_response(None, move |_, response| {
                if response == "accept" {
                    config.borrow_mut().socket = policy;
                    previous.set(row.selected());
                    // The socket takes a location that a share may hold.
                    validity.update();
                } else {
                    row.set_selected(previous.get());
                }
            });
            dialog.present(Some(&parent_widget));
            return;
        }

        config.borrow_mut().socket = policy;
        previous.set(row.selected());
        validity.update();
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

    // Remote access with no network chosen binds loopback only, so the
    // switch would read as on while nothing else can connect.
    let none_chosen = adw::ActionRow::builder()
        .title("Choose at least one network. Until you do, only this computer can connect.")
        .css_classes(vec!["warning"])
        .build();
    none_chosen.set_title_lines(0);
    let none_chosen_list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(vec!["boxed-list"])
        .build();
    none_chosen_list.append(&none_chosen);

    let warn_scope = scope.clone();
    let warn_list = none_chosen_list.clone();
    let show_none_chosen: SelectionCallback = Rc::new(move |names: &[String]| {
        warn_list.set_visible(warn_scope.selected() == 1 && names.is_empty());
    });

    let chooser = build_interface_list(&interfaces, config.clone(), show_none_chosen.clone());
    detail.append(&chooser);
    detail.append(&none_chosen_list);

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
    show_none_chosen(&current_selection(&chooser));

    let parent_widget = parent.as_ref().clone();
    let config_for_switch = config.clone();
    let detail_for_switch = detail.clone();
    let scope_for_switch = scope.clone();
    let chooser_for_switch = chooser.clone();
    let none_chosen_for_switch = show_none_chosen.clone();
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
        let chooser = chooser_for_switch.clone();
        let show_none_chosen = none_chosen_for_switch.clone();
        dialog.connect_response(None, move |_, response| {
            if response != "allow" {
                switch.set_active(false);
                return;
            }
            detail.set_visible(true);
            // Chosen networks is the safer of the two, so it is what the
            // switch lands on. The user can widen it deliberately.
            scope.set_selected(1);
            // When the scope was already on chosen networks, nothing above
            // emits, so the list is shown and read here. The stored names
            // then match the switches the user sees.
            chooser.set_visible(true);
            let names = current_selection(&chooser);
            show_none_chosen(&names);
            config.borrow_mut().exposure = Exposure::Interfaces { names };
        });
        dialog.present(Some(&parent_widget));
    });

    let parent_widget = parent.as_ref().clone();
    let chooser_for_scope = chooser.clone();
    let config_for_scope = config.clone();
    let none_chosen_for_scope = show_none_chosen.clone();
    scope.connect_selected_notify(move |scope| {
        let names = current_selection(&chooser_for_scope);
        none_chosen_for_scope(&names);
        if scope.selected() == 1 {
            chooser_for_scope.set_visible(true);
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

/// Called with the chosen interface names whenever a switch changes.
type SelectionCallback = Rc<dyn Fn(&[String])>;

/// One switch per interface, with the addresses it currently holds.
///
/// The addresses are shown because interface names alone do not say which
/// network is which. `wlp3s0` means nothing; `192.168.1.24` is recognisable.
///
/// A saved interface that is away now, such as a USB adapter or a VPN, still
/// gets a switch. Names are stored so that they survive the interface being
/// away (rule 40), and the stored list is read back from the switches, so a
/// name with no switch would be dropped by the next change.
fn build_interface_list(
    interfaces: &[cabin_core::NetInterface],
    config: Rc<RefCell<DesktopConfig>>,
    on_selection: SelectionCallback,
) -> gtk::ListBox {
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(vec!["boxed-list"])
        .build();

    let chosen: Vec<String> = config.borrow().exposure.chosen_interfaces().to_vec();
    let absent: Vec<&String> = chosen
        .iter()
        .filter(|name| !interfaces.iter().any(|i| &i.name == *name))
        .collect();

    if interfaces.is_empty() && absent.is_empty() {
        let empty = adw::ActionRow::builder()
            .title("No networks found")
            .subtitle("This computer is not connected to a network.")
            .build();
        list.append(&empty);
        return list;
    }

    let rows = interfaces
        .iter()
        .map(|interface| {
            (
                interface.name.clone(),
                interface.addresses.join(", "),
                chosen.contains(&interface.name),
            )
        })
        .chain(
            absent
                .into_iter()
                .map(|name| (name.clone(), "Not connected now".to_string(), true)),
        );
    for (name, subtitle, active) in rows {
        let row = adw::SwitchRow::builder()
            .title(&name)
            .subtitle(&subtitle)
            .active(active)
            .build();
        // Interface names and addresses come from the system, so they are
        // data, not markup (rule 33).
        row.set_use_markup(false);
        let config = config.clone();
        let list_ref = list.clone();
        let on_selection = on_selection.clone();
        row.connect_active_notify(move |_| {
            // Read every row rather than tracking one, so the stored list
            // always matches what the user can see.
            let names = current_selection(&list_ref);
            on_selection(&names);
            config.borrow_mut().exposure = Exposure::Interfaces { names };
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
    start_after: bool,
    on_applied: Rc<dyn Fn()>,
    settings_dialog: adw::PreferencesDialog,
) {
    // Only a running desktop loses anything, so only a running desktop asks.
    if !desktop.state.is_running() {
        apply_now(
            parent,
            podman,
            desktop,
            updated,
            start_after,
            on_applied,
            settings_dialog,
        );
        return;
    }

    let dialog = adw::AlertDialog::builder()
        .heading("Restart the desktop?")
        .body(
            "These changes need the desktop to be recreated, so anything running \
             in it will close. Your files are kept.",
        )
        .build();
    dialog.add_response("cancel", "Cancel");
    dialog.add_response(
        "apply",
        if start_after {
            "Apply and Restart"
        } else {
            "Apply and Stop"
        },
    );
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
        let parent = parent_for_closure.clone();
        let settings_dialog = settings_dialog.clone();

        apply_now(
            &parent,
            podman,
            desktop,
            updated,
            start_after,
            on_applied,
            settings_dialog,
        );
    });
    dialog.present(Some(parent));
}

/// Recreates the desktop, and starts it when asked to.
fn apply_now(
    parent: &gtk::Widget,
    podman: Arc<Podman>,
    desktop: Desktop,
    updated: DesktopConfig,
    start_after: bool,
    on_applied: Rc<dyn Fn()>,
    settings_dialog: adw::PreferencesDialog,
) {
    let nodes = graphics::render_nodes();
    let interfaces = crate::network::interfaces();
    let parent = parent.clone();

    runtime::spawn(
        // The library keeps the old container until the new one exists, and
        // puts it back if Podman refuses the new settings. The old container
        // holds the only copy of the configuration and the password (rule 1).
        async move {
            podman
                .recreate_desktop(&desktop, &updated, &nodes, &interfaces, start_after)
                .await
        },
        move |result| match result {
            Ok(_) => {
                settings_dialog.close();
                on_applied();
            }
            Err(e) => {
                let error = adw::AlertDialog::builder()
                    .heading("Could not apply the changes")
                    .body(e.to_string())
                    .build();
                error.add_response("close", "Close");
                error.present(Some(&parent));
                // The desktop can have a new id, a new state, or a leftover
                // beside it, so the list has to look again.
                on_applied();
            }
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    #[test]
    fn sensitive_folders_and_their_contents_are_flagged() {
        let home = Path::new("/home/user");
        for path in [
            "/home/user/.ssh",
            "/home/user/.ssh/keys",
            "/home/user/.config/git",
            "/home/user/.local/share/keyrings",
            "/home/user/public_html",
        ] {
            assert!(is_sensitive_location(path, home), "{path} was not flagged");
        }
    }

    #[test]
    fn ordinary_folders_are_not_flagged() {
        let home = Path::new("/home/user");
        for path in [
            "/home/user/Documents",
            "/home/user/.sshfs",
            "/home/user/.local/share/fonts",
            "/srv/data",
        ] {
            assert!(!is_sensitive_location(path, home), "{path} was flagged");
        }
    }

    fn roots() -> Vec<PathBuf> {
        vec![PathBuf::from("/run/user/1000/doc")]
    }

    #[test]
    fn a_portal_folder_resolves_with_the_path_below_it() {
        let (id, rest) = split_document_path(
            Path::new("/run/user/1000/doc/a1b2c3/Projects/web/site"),
            &roots(),
        )
        .expect("a path under the portal is a document");
        assert_eq!(id, "a1b2c3");
        assert_eq!(rest, vec!["web".to_string(), "site".to_string()]);
        assert_eq!(
            join_host_path("/home/user/Projects", &rest),
            "/home/user/Projects/web/site"
        );
    }

    #[test]
    fn a_portal_document_itself_resolves_to_its_real_path() {
        let (id, rest) =
            split_document_path(Path::new("/run/user/1000/doc/a1b2c3/.gitconfig"), &roots())
                .expect("a path under the portal is a document");
        assert_eq!(id, "a1b2c3");
        assert_eq!(
            join_host_path("/home/user/.gitconfig", &rest),
            "/home/user/.gitconfig"
        );
    }

    #[test]
    fn a_real_path_is_not_a_portal_path() {
        assert!(split_document_path(Path::new("/home/user/Projects"), &roots()).is_none());
        // Only a whole component matches, so a neighbour of the root is not
        // taken for it.
        assert!(split_document_path(Path::new("/run/user/1000/docs/x"), &roots()).is_none());
    }

    #[test]
    fn a_missing_render_node_keeps_its_own_entry() {
        let nodes = vec![("/dev/dri/renderD128".to_string(), "i915".to_string())];
        let stored = GraphicsMode::RenderNode {
            path: "/dev/dri/renderD129".to_string(),
        };
        let choices = graphics_choices(&nodes, &stored);
        let (label, mode) = choices.last().expect("the missing node is listed");
        assert_eq!(label, "renderD129 (not found)");
        assert_eq!(*mode, stored);
        // Every other entry maps to its own mode, so choosing Automatic
        // replaces the stale path.
        assert_eq!(choices[0].1, GraphicsMode::Automatic);
        assert_eq!(
            choices[2].1,
            GraphicsMode::RenderNode {
                path: "/dev/dri/renderD128".to_string()
            }
        );
    }

    #[test]
    fn a_present_render_node_is_listed_once() {
        let nodes = vec![("/dev/dri/renderD128".to_string(), "i915".to_string())];
        let stored = GraphicsMode::RenderNode {
            path: "/dev/dri/renderD128".to_string(),
        };
        let choices = graphics_choices(&nodes, &stored);
        assert_eq!(choices.len(), 3);
        assert_eq!(choices[2].0, "renderD128 (i915)");
    }
}
