//! "Connect from another device": what to type into an RDP client elsewhere.

use crate::netaddr;
use adw::prelude::*;
use cabin_core::Desktop;

pub fn present(parent: &impl IsA<gtk::Widget>, desktop: &Desktop, password: String) {
    let dialog = adw::Dialog::builder()
        .title("Connect From Another Device")
        .content_width(460)
        .build();

    let page = adw::PreferencesPage::new();

    let Some(port) = desktop.rdp_port else {
        page.add(&message_group(
            "This desktop is not running",
            "Start it before connecting from another device.",
        ));
        finish(&dialog, &page, parent);
        return;
    };

    if !desktop.config.expose_on_lan {
        page.add(&message_group(
            "Only this computer can connect",
            "This desktop listens on 127.0.0.1. To reach it from a phone or \
             another computer, turn on \"Allow connections from other devices\" \
             in Settings.",
        ));
        finish(&dialog, &page, parent);
        return;
    }

    let address = netaddr::describe(netaddr::best_local_address());
    let details = adw::PreferencesGroup::builder()
        .title("Connection details")
        .description("Use any RDP client. Choose RDP, not VNC.")
        .build();
    details.add(&copyable_row("Address", &format!("{address}:{port}")));
    details.add(&copyable_row("Username", "cabin"));
    details.add(&copyable_row("Password", &password));
    page.add(&details);

    // Both of these will otherwise waste the user's time on their first try.
    let notes = adw::PreferencesGroup::builder()
        .title("Before you connect")
        .build();
    notes.add(
        &adw::ActionRow::builder()
            .title("Turn the clipboard off in your client")
            .subtitle(
                "A client that offers clipboard sharing will crash this desktop's \
                 remote session. In FreeRDP the option is -clipboard. This is a \
                 fault in gnome-remote-desktop, not in your client.",
            )
            .css_classes(vec!["warning"])
            .build(),
    );
    notes.add(
        &adw::ActionRow::builder()
            .title("Expect a certificate warning")
            .subtitle(
                "Each desktop makes its own certificate, so no authority has signed \
                 it. Accepting it once is normal here.",
            )
            .build(),
    );
    page.add(&notes);

    finish(&dialog, &page, parent);
}

fn finish(dialog: &adw::Dialog, page: &adw::PreferencesPage, parent: &impl IsA<gtk::Widget>) {
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(page));
    dialog.set_child(Some(&toolbar));
    dialog.present(Some(parent));
}

fn message_group(title: &str, body: &str) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::new();
    group.add(
        &adw::ActionRow::builder()
            .title(title)
            .subtitle(body)
            .build(),
    );
    group
}

/// A detail with a copy button, because these get typed into a phone.
fn copyable_row(title: &str, value: &str) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(title)
        .subtitle(value)
        .css_classes(vec!["property"])
        .build();

    let copy = gtk::Button::builder()
        .icon_name("edit-copy-symbolic")
        .valign(gtk::Align::Center)
        .css_classes(vec!["flat"])
        .tooltip_text(format!("Copy the {}", title.to_lowercase()))
        .build();

    let value = value.to_string();
    copy.connect_clicked(move |button| {
        button.display().clipboard().set_text(&value);
    });
    row.add_suffix(&copy);
    row
}
