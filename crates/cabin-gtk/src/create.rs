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

pub fn present(
    parent: &impl IsA<gtk::Widget>,
    podman: Arc<Podman>,
    on_created: impl Fn() + 'static,
) {
    // connect_response can fire more than once over the dialogue's life, so
    // the callback is shared rather than moved.
    let on_created: Rc<dyn Fn()> = Rc::new(on_created);
    let dialog = adw::AlertDialog::builder()
        .heading("New Desktop")
        .body("A new desktop runs its own Fedora GNOME session.")
        .build();

    let name_row = adw::EntryRow::builder()
        .title("Name")
        .text("fedora")
        .build();

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

    let group = adw::PreferencesGroup::new();
    group.add(&name_row);
    group.add(&memory_row);
    group.add(&graphics_row);

    dialog.set_extra_child(Some(&group));
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("create", "Create");
    dialog.set_response_appearance("create", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("create"));
    dialog.set_close_response("cancel");

    let default_image = crate::state::State::new(crate::APP_ID).default_image();
    let parent_widget = parent.as_ref().clone();
    dialog.connect_response(None, move |dialog, response| {
        if response != "create" {
            return;
        }
        let name = name_row.text().trim().to_string();
        if name.is_empty() {
            return;
        }

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
        let parent_widget = parent_widget.clone();
        let on_created = on_created.clone();
        let port = free_port().unwrap_or(PORT_RANGE.start);
        let password = generate_password();

        runtime::spawn(
            async move {
                podman
                    .create_desktop(&config, port, &nodes, &password)
                    .await
            },
            move |result| match result {
                Ok(id) => {
                    tracing::info!("created desktop {id}");
                    on_created();
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
