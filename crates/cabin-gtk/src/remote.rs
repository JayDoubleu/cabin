//! "Connect from another device": what to type into an RDP client elsewhere.

use crate::netaddr;
use adw::prelude::*;
use cabin_core::{Desktop, Exposure};

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

    if !desktop.config.exposure.reaches_other_devices() {
        page.add(&message_group(
            "Only this computer can connect",
            "This desktop listens on 127.0.0.1. To reach it from a phone or \
             another computer, turn on \"Allow connections from other devices\" \
             in Settings.",
        ));
        finish(&dialog, &page, parent);
        return;
    }

    let details = adw::PreferencesGroup::builder()
        .title("Connection details")
        .description("Use any RDP client. Choose RDP, not VNC.")
        .build();
    // With chosen networks the desktop answers on those addresses only, so
    // offering the route to the wider network would name an address that
    // refuses the connection.
    let addresses = reachable_addresses(desktop);
    if addresses.is_empty() {
        // Turning remote access on with no network ticked binds loopback
        // only, so the two empty cases need different advice.
        let body = if desktop.config.exposure.chosen_interfaces().is_empty() {
            "No network is chosen. Choose one in Settings, then apply."
        } else {
            "The chosen networks hold no address at the moment. Connect this \
             computer to one of them, or choose a different network in Settings."
        };
        page.add(&message_group("No usable address", body));
        finish(&dialog, &page, parent);
        return;
    }
    for (index, address) in addresses.iter().enumerate() {
        let title = if addresses.len() == 1 {
            "Address".to_string()
        } else {
            format!("Address {}", index + 1)
        };
        details.add(&copyable_row(&title, &endpoint(address, port)));
    }
    details.add(&copyable_row("Username", "cabin"));
    details.add(&copyable_row("Password", &password));
    // The pin that Cabin's own viewer recorded is the only thing another
    // device can check the certificate against. Without it a client on the
    // network cannot tell this desktop from an impostor on first use.
    let fingerprint = crate::pinning::recorded(&desktop.name);
    if let Some(fingerprint) = &fingerprint {
        details.add(&copyable_row(
            "Certificate fingerprint (SHA-256)",
            fingerprint,
        ));
    }
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
    let certificate_note = if fingerprint.is_some() {
        "Each desktop makes its own certificate, so no authority has signed it. \
         Compare the SHA-256 fingerprint that your client shows with the one \
         above. Accept the certificate only if they are the same. Also make \
         your client require NLA, for example /sec:nla in FreeRDP."
    } else {
        "Each desktop makes its own certificate, so no authority has signed it. \
         Cabin has no fingerprint for this desktop yet, so you cannot check the \
         certificate. Open the desktop in Cabin once to record its fingerprint, \
         then open this dialogue again."
    };
    notes.add(
        &adw::ActionRow::builder()
            .title("Expect a certificate warning")
            .subtitle(certificate_note)
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

/// The addresses another device could actually use.
///
/// For chosen networks these are the addresses on those interfaces, and none
/// when no network is chosen, because then only loopback is bound. For every
/// network there is no single answer, so the route to the wider network is the
/// useful one.
fn reachable_addresses(desktop: &Desktop) -> Vec<String> {
    match &desktop.config.exposure {
        Exposure::Loopback => Vec::new(),
        Exposure::Everywhere => match netaddr::best_local_address() {
            Some(ip) => vec![ip.to_string()],
            None => vec![netaddr::describe(None)],
        },
        Exposure::Interfaces { names } => crate::network::interfaces()
            .into_iter()
            .filter(|i| names.contains(&i.name))
            .flat_map(|i| i.addresses)
            .collect(),
    }
}

/// What to type into a client: an address and a port.
///
/// An IPv6 address needs brackets, or a client reads the port as part of the
/// address. Text that is not an address, such as the description used when
/// no address is known, passes through as it is.
fn endpoint(address: &str, port: u16) -> String {
    match address.parse::<std::net::IpAddr>() {
        Ok(ip) => std::net::SocketAddr::new(ip, port).to_string(),
        Err(_) => format!("{address}:{port}"),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ipv4_endpoint_has_no_brackets() {
        assert_eq!(endpoint("192.0.2.5", 3390), "192.0.2.5:3390");
    }

    #[test]
    fn an_ipv6_endpoint_is_bracketed() {
        assert_eq!(endpoint("fd7a::1", 3390), "[fd7a::1]:3390");
    }

    #[test]
    fn text_that_is_not_an_address_passes_through() {
        let text = netaddr::describe(None);
        assert_eq!(endpoint(&text, 3390), format!("{text}:3390"));
    }
}
