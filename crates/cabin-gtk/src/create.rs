//! The "New Desktop" dialogue.

use crate::graphics;
use crate::runtime;
use adw::prelude::*;
use cabin_core::{DesktopConfig, GraphicsMode};
use cabin_podman::Podman;
use std::rc::Rc;
use std::sync::Arc;

/// The guest RDP port maps to a free host port in this range. It sits above
/// the usual ephemeral services and below the ephemeral allocation range.
const PORT_RANGE: std::ops::Range<u16> = 13389..13500;

/// Presents the New Desktop dialogue.
///
/// `on_created` receives the name of the desktop that was made and whether the
/// user asked for its settings. Only the essentials are asked for here.
/// Everything else is in the desktop's own settings.
pub fn present(
    parent: &impl IsA<gtk::Widget>,
    podman: Arc<Podman>,
    taken: Vec<String>,
    on_created: impl Fn(String, bool) + 'static,
) {
    // connect_response can fire more than once over the dialogue's life, so
    // the callback is shared rather than moved.
    let on_created: Rc<dyn Fn(String, bool)> = Rc::new(on_created);
    let dialog = adw::AlertDialog::builder()
        .heading("New Desktop")
        .body("A new desktop runs its own Fedora GNOME session.")
        .build();

    let name_row = adw::EntryRow::builder().title("Name").build();
    // A name that is free, so the dialogue opens ready to use rather than
    // opening with an error against a name the user did not choose.
    name_row.set_text(&first_free_name("fedora", &taken));

    let memory_row = adw::SpinRow::with_range(1024.0, 65536.0, 1024.0);
    memory_row.set_title("Memory");
    memory_row.set_subtitle("Megabytes");
    memory_row.set_value(4096.0);

    // Offer software rendering plus every render node, labelled with its
    // driver. Never guess the driver from the node number.
    let nodes = graphics::render_nodes();
    let mut options: Vec<String> = vec!["Automatic".into(), "Software rendering".into()];
    for (path, driver) in &nodes {
        let leaf = path.rsplit('/').next().unwrap_or(path.as_str());
        options.push(format!("{leaf} ({driver})"));
    }
    let option_refs: Vec<&str> = options.iter().map(String::as_str).collect();
    let graphics_row = adw::ComboRow::builder()
        .title("Graphics")
        .model(&gtk::StringList::new(&option_refs))
        .build();

    // Shared folders and the rest are not asked for here, so offer the way to
    // them rather than leaving the user to find the settings afterwards.
    let open_settings = adw::SwitchRow::builder()
        .title("Open settings")
        .subtitle("Set up shared folders and access straight away.")
        .active(false)
        .build();

    // A fresh installation has no guest image, and creating a desktop then
    // fails with "no such image" after everything here has been filled in.
    // Ask Podman now, and offer the way out rather than only the message.
    let image_missing = adw::ActionRow::builder()
        .title("The guest image is not built yet")
        .subtitle("A desktop needs it. Building takes several minutes.")
        .css_classes(vec!["warning"])
        .visible(false)
        .build();
    let build_now = gtk::Button::builder()
        .label("Build")
        .valign(gtk::Align::Center)
        .css_classes(vec!["suggested-action"])
        .build();
    image_missing.add_suffix(&build_now);

    let group = adw::PreferencesGroup::new();
    group.add(&name_row);
    group.add(&memory_row);
    group.add(&graphics_row);
    group.add(&open_settings);
    group.add(&image_missing);

    dialog.set_extra_child(Some(&group));
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("create", "Create");
    dialog.set_response_appearance("create", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("create"));
    dialog.set_close_response("cancel");

    // Check the name as it is typed rather than when Create is pressed.
    // An AdwAlertDialog closes on any response, so a name rejected at that
    // point costs the user the whole dialogue and everything else they set.
    let dialog_for_name = dialog.clone();
    let group_for_name = group.clone();
    let missing_row = image_missing.clone();
    let taken_names = taken.clone();
    let check = move |entry: &adw::EntryRow| match cabin_core::check_name(&entry.text(), &taken) {
        Ok(()) => {
            entry.remove_css_class("error");
            group_for_name.set_description(None);
            // Create can be off for two reasons, and clearing one must not
            // clear the other.
            dialog_for_name.set_response_enabled("create", !missing_row.is_visible());
        }
        Err(why) => {
            entry.add_css_class("error");
            group_for_name.set_description(Some(&why));
            dialog_for_name.set_response_enabled("create", false);
        }
    };
    name_row.connect_changed(check.clone());
    check(&name_row);

    let default_image = crate::state::State::new(crate::APP_ID).default_image();

    // Re-run after a build, so the warning clears without reopening.
    let row_for_check = image_missing.clone();
    let dialog_for_check = dialog.clone();
    let name_for_check = name_row.clone();
    let taken_for_check = taken_names.clone();
    let podman_for_check = podman.clone();
    let image_for_check = default_image.clone();
    let check_image: Rc<dyn Fn()> = Rc::new(move || {
        let row = row_for_check.clone();
        let dialog = dialog_for_check.clone();
        let name = name_for_check.clone();
        let taken = taken_for_check.clone();
        let image = image_for_check.clone();
        let podman = podman_for_check.clone();
        runtime::spawn(
            async move { podman.image_exists(&image).await },
            move |result| {
                // A Podman that cannot answer is not a missing image, so say
                // nothing rather than send the user to build one they have.
                let missing = matches!(result, Ok(false));
                row.set_visible(missing);
                dialog.set_response_enabled(
                    "create",
                    !missing && cabin_core::check_name(&name.text(), &taken).is_ok(),
                );
            },
        );
    });
    check_image();

    let check_after_build = check_image.clone();
    let podman_for_build = podman.clone();
    let build_parent = parent.as_ref().clone();
    build_now.connect_clicked(move |_| {
        let recheck = check_after_build.clone();
        crate::images::present_build(
            &build_parent,
            podman_for_build.clone(),
            crate::images::DEFAULT_TAG,
            move || recheck(),
        );
    });
    let parent_widget = parent.as_ref().clone();
    dialog.connect_response(None, move |dialog, response| {
        if response != "create" {
            return;
        }
        // Create is only enabled for a name that passed the check above.
        let name = name_row.text().trim().to_string();

        let mut config = DesktopConfig::new(&name);
        config.image = default_image.clone();
        config.memory_mb = Some(memory_row.value() as u64);
        config.graphics = match graphics_row.selected() {
            0 => GraphicsMode::Automatic,
            1 => GraphicsMode::Software,
            n => GraphicsMode::RenderNode {
                path: nodes[(n - 2) as usize].0.clone(),
            },
        };

        let podman = podman.clone();
        let nodes = nodes.clone();
        let interfaces = crate::network::interfaces();
        let parent_widget = parent_widget.clone();
        let on_created = on_created.clone();
        let port = free_port().unwrap_or(PORT_RANGE.start);
        let password = generate_password();
        let created_name = name.clone();
        let wants_settings = open_settings.is_active();

        runtime::spawn(
            async move {
                podman
                    .create_desktop(&config, port, &nodes, &interfaces, &password)
                    .await
            },
            move |result| match result {
                Ok(id) => {
                    tracing::info!("created desktop {id}");
                    on_created(created_name, wants_settings);
                }
                Err(e) => show_error(&parent_widget, &e.to_string()),
            },
        );
        let _ = dialog;
    });

    dialog.present(Some(parent));
}

fn show_error(parent: &gtk::Widget, message: &str) {
    tracing::error!("could not create the desktop: {message}");
    let dialog = adw::AlertDialog::builder()
        .heading("Could not create the desktop")
        .body(message)
        .build();
    dialog.add_response("close", "Close");
    dialog.present(Some(parent));
}

/// Suggests a name that is not in use, by adding a number if it has to.
fn first_free_name(base: &str, taken: &[String]) -> String {
    if !taken.iter().any(|t| t.eq_ignore_ascii_case(base)) {
        return base.to_string();
    }
    for suffix in 2..100 {
        let candidate = format!("{base}-{suffix}");
        if !taken.iter().any(|t| t.eq_ignore_ascii_case(&candidate)) {
            return candidate;
        }
    }
    base.to_string()
}

/// Finds a free TCP port by asking the kernel to bind one.
///
/// This races with anything else that binds in the gap before Podman
/// publishes the port. The window is small, and Podman reports a clear error
/// if it loses that race.
fn free_port() -> Option<u16> {
    PORT_RANGE
        .into_iter()
        .find(|port| std::net::TcpListener::bind(("127.0.0.1", *port)).is_ok())
}

/// Generates a per-desktop RDP password.
///
/// The password reaches the guest as an environment variable, so `podman
/// inspect` shows it. That is the same exposure the guest already has. Storing
/// it in the host keyring through the Secret portal is still the right home
/// and is tracked in DESIGN.md section 8.
fn generate_password() -> String {
    use std::io::Read;
    const ALPHABET: &[u8] = b"abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut bytes = [0u8; 20];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .expect("/dev/urandom must be readable");
    bytes
        .iter()
        .map(|b| ALPHABET[*b as usize % ALPHABET.len()] as char)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggests_the_plain_name_when_it_is_free() {
        assert_eq!(first_free_name("fedora", &[]), "fedora");
    }

    #[test]
    fn suggests_a_numbered_name_when_the_plain_one_is_taken() {
        let taken = vec!["fedora".to_string(), "fedora-2".to_string()];
        assert_eq!(first_free_name("fedora", &taken), "fedora-3");
    }

    #[test]
    fn the_suggestion_is_always_a_valid_name() {
        let taken: Vec<String> = (2..20).map(|n| format!("fedora-{n}")).collect();
        let mut all = vec!["fedora".to_string()];
        all.extend(taken);
        let suggested = first_free_name("fedora", &all);
        assert!(cabin_core::check_name(&suggested, &all).is_ok());
    }

    #[test]
    fn passwords_are_long_and_differ() {
        let a = generate_password();
        let b = generate_password();
        assert_eq!(a.len(), 20);
        assert_ne!(a, b, "two generated passwords must not match");
    }

    #[test]
    fn passwords_avoid_ambiguous_characters() {
        for _ in 0..20 {
            let p = generate_password();
            for bad in ['l', 'I', 'O', '0', '1'] {
                assert!(!p.contains(bad), "{p} contains the ambiguous {bad}");
            }
        }
    }

    #[test]
    fn free_port_is_inside_the_range() {
        let port = free_port().expect("some port in the range should be free");
        assert!(PORT_RANGE.contains(&port));
    }
}
