//! "Open a shell in this desktop": the command to run in a host terminal.
//!
//! Cabin shows the command rather than running it. As a Flatpak it cannot
//! start a terminal on the host without `--talk-name=org.freedesktop.Flatpak`,
//! which the sandbox design rules out. See DESIGN.md section 19.3.

use adw::prelude::*;
use cabin_core::{Desktop, GUEST_HOME};

/// The user inside the guest. Everything the desktop runs belongs to it.
const GUEST_USER: &str = "cabin";

pub fn present(parent: &impl IsA<gtk::Widget>, desktop: &Desktop) {
    let dialog = adw::Dialog::builder()
        .title("Open a Shell")
        .content_width(560)
        .build();

    let page = adw::PreferencesPage::new();

    if !desktop.state.is_running() {
        page.add(&message_group(
            "This desktop is not running",
            "Start it before opening a shell in it.",
        ));
        finish(&dialog, &page, parent);
        return;
    }

    let group = adw::PreferencesGroup::builder()
        .title("Run this in a terminal")
        .description(
            "The shell runs inside the desktop, as the same user, with the same \
             files. Nothing is shared with this computer that the desktop does \
             not already have.",
        )
        .build();
    group.add(&copyable_row("Command", &shell_command(&desktop.name)));
    page.add(&group);

    let notes = adw::PreferencesGroup::builder()
        .title("Good to know")
        .build();
    notes.add(&row(
        "The desktop has its own terminal too",
        "GNOME Console is installed, so a terminal inside the session works \
             without this.",
    ));
    notes.add(&row(
        "Cabin cannot open the terminal for you",
        "Doing that would need permission to run programs on this computer, \
         which Cabin deliberately does not ask for.",
    ));
    page.add(&notes);

    finish(&dialog, &page, parent);
}

/// The command that opens a shell in `name`.
///
/// `-u` matters. Without it the shell is root, which is not the user the
/// desktop runs as and not the user whose files are in the home directory.
/// `-w` matters for the same reason: the default working directory is `/`.
pub fn shell_command(name: &str) -> String {
    format!("podman exec -it -u {GUEST_USER} -w {GUEST_HOME} {name} bash")
}

fn row(title: &str, subtitle: &str) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(title)
        .subtitle(subtitle)
        .build();
    row.set_subtitle_lines(0);
    row
}

fn message_group(title: &str, body: &str) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::new();
    group.add(&row(title, body));
    group
}

fn finish(dialog: &adw::Dialog, page: &adw::PreferencesPage, parent: &impl IsA<gtk::Widget>) {
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(page));
    dialog.set_child(Some(&toolbar));
    dialog.present(Some(parent));
}

fn copyable_row(title: &str, value: &str) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(title)
        .subtitle(value)
        .css_classes(vec!["property", "monospace"])
        .build();
    row.set_subtitle_selectable(true);

    let copy = gtk::Button::builder()
        .icon_name("edit-copy-symbolic")
        .valign(gtk::Align::Center)
        .css_classes(vec!["flat"])
        .tooltip_text("Copy the command")
        .build();

    let value = value.to_string();
    copy.connect_clicked(move |button| {
        button.display().clipboard().set_text(&value);
    });
    row.add_suffix(&copy);
    row
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_names_the_guest_user_and_home() {
        // Without -u the shell is root, which is not the user the desktop
        // runs as, and its files are not in that home directory.
        let command = shell_command("fedora");
        assert!(command.contains("-u cabin"), "{command}");
        assert!(command.contains("-w /home/cabin"), "{command}");
        assert!(command.ends_with(" fedora bash"), "{command}");
    }

    #[test]
    fn the_command_is_interactive() {
        // Without -it the shell exits at once and looks broken.
        assert!(shell_command("x").contains("-it"));
    }
}
