//! Viewing and editing the guest recipe.
//!
//! The recipe is `guest/Containerfile`. The copy shipped with Cabin is
//! read-only, because it lives inside the installation. A user who wants to
//! change it takes a copy first, and builds then use the copy. See DESIGN.md
//! section 20.

use crate::guestdir::{self, Source};
use adw::prelude::*;
use std::path::PathBuf;
use std::rc::Rc;

/// Shows the active base recipe.
pub fn present(parent: &impl IsA<gtk::Widget>, on_changed: impl Fn() + 'static) {
    let Some((dir, _)) = guestdir::active() else {
        missing(parent);
        return;
    };
    present_path(parent, dir, on_changed);
}

/// Shows the recipe in `dir`, editable unless it is the shipped tree.
pub fn present_path(parent: &impl IsA<gtk::Widget>, dir: PathBuf, on_changed: impl Fn() + 'static) {
    // The shipped tree is the only one that cannot be written, and it is the
    // only one that is not under the user's own directory.
    let source = if guestdir::shipped().as_deref() == Some(dir.as_path()) {
        Source::Shipped
    } else {
        Source::Custom
    };
    let path = dir.join("Containerfile");
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) => {
            let error = adw::AlertDialog::builder()
                .heading("Could not read the recipe")
                .body(format!("{}\n\n{e}", path.display()))
                .build();
            error.add_response("close", "Close");
            error.present(Some(parent));
            return;
        }
    };

    let editable = source == Source::Custom;
    let on_changed: Rc<dyn Fn()> = Rc::new(on_changed);

    let dialog = adw::Dialog::builder()
        .title(if editable {
            "Your Guest Recipe"
        } else {
            "Guest Recipe"
        })
        .content_width(820)
        .content_height(640)
        .build();

    let view = gtk::TextView::builder()
        .monospace(true)
        .editable(editable)
        .cursor_visible(editable)
        .wrap_mode(gtk::WrapMode::None)
        .left_margin(12)
        .right_margin(12)
        .top_margin(12)
        .bottom_margin(12)
        .build();
    view.buffer().set_text(&text);

    let scroller = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .child(&view)
        .build();

    // Say which recipe this is and where it came from, because the same
    // window shows two different things and only one of them can be saved.
    let banner = adw::Banner::builder()
        .title(match source {
            Source::Shipped => "This is the recipe Cabin ships with. Take a copy to change it.",
            Source::Custom => "This is your copy. Builds use it instead of the one Cabin ships.",
        })
        .revealed(true)
        .build();

    if source == Source::Shipped {
        banner.set_button_label(Some("Take a Copy"));
        let parent_widget = parent.as_ref().clone();
        let dialog_ref = dialog.clone();
        let on_changed_ref = on_changed.clone();
        banner.connect_button_clicked(move |_| {
            dialog_ref.close();
            customise(&parent_widget, on_changed_ref.clone());
        });
    } else if guestdir::shipped_has_moved_on() {
        // An update brought a newer default, and this copy predates it.
        banner.set_title(
            "Cabin now ships a newer recipe. Your copy is still used, and does \
             not have the changes.",
        );
        banner.add_css_class("warning");
    }

    let header = adw::HeaderBar::new();

    let copy = gtk::Button::builder()
        .icon_name("edit-copy-symbolic")
        .tooltip_text("Copy the whole recipe")
        .build();
    let text_for_copy = text.clone();
    copy.connect_clicked(move |button| {
        button.display().clipboard().set_text(&text_for_copy);
    });
    header.pack_end(&copy);

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.add_top_bar(&banner);

    let box_ = gtk::Box::new(gtk::Orientation::Vertical, 0);
    box_.append(&scroller);

    let location = gtk::Label::builder()
        .label(path.display().to_string())
        .css_classes(vec!["caption", "dim-label"])
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::Middle)
        .margin_start(12)
        .margin_end(12)
        .margin_top(6)
        .margin_bottom(6)
        .selectable(true)
        .build();
    box_.append(&location);

    if editable {
        let save = gtk::Button::builder()
            .label("Save")
            .css_classes(vec!["suggested-action"])
            .build();
        header.pack_start(&save);

        let buffer = view.buffer();
        let dialog_ref = dialog.clone();
        let on_changed_ref = on_changed.clone();
        save.connect_clicked(move |button| {
            let text = buffer
                .text(&buffer.start_iter(), &buffer.end_iter(), false)
                .to_string();
            match std::fs::write(&path, text) {
                Ok(()) => {
                    on_changed_ref();
                    dialog_ref.close();
                }
                Err(e) => {
                    let error = adw::AlertDialog::builder()
                        .heading("Could not save the recipe")
                        .body(e.to_string())
                        .build();
                    error.add_response("close", "Close");
                    error.present(Some(button));
                }
            }
        });

        let note = gtk::Label::builder()
            .label("Changes take effect the next time the image is built.")
            .css_classes(vec!["caption", "dim-label"])
            .xalign(0.0)
            .margin_start(12)
            .margin_end(12)
            .margin_bottom(6)
            .build();
        box_.append(&note);
    }

    toolbar.set_content(Some(&box_));
    dialog.set_child(Some(&toolbar));
    dialog.present(Some(parent));
}

/// Takes a copy of the shipped recipe, after saying what that means.
pub fn customise(parent: &impl IsA<gtk::Widget>, on_changed: Rc<dyn Fn()>) {
    let dialog = adw::AlertDialog::builder()
        .heading("Take a copy of the recipe?")
        .body(format!(
            "Cabin copies the guest files to a folder you own, and builds use \
             that copy from then on. Updates to Cabin will no longer change \
             your guest image.\n\nThe copy goes in:\n{}",
            guestdir::custom_dir().display()
        ))
        .build();
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("copy", "Take a Copy");
    dialog.set_response_appearance("copy", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("copy"));

    let parent_widget = parent.as_ref().clone();
    dialog.connect_response(None, move |_, response| {
        if response != "copy" {
            return;
        }
        match guestdir::customise() {
            Ok(_) => {
                on_changed();
                present(&parent_widget, {
                    let again = on_changed.clone();
                    move || again()
                });
            }
            Err(message) => {
                let error = adw::AlertDialog::builder()
                    .heading("Could not take a copy")
                    .body(message)
                    .build();
                error.add_response("close", "Close");
                error.present(Some(&parent_widget));
            }
        }
    });
    dialog.present(Some(parent));
}

/// Puts the shipped recipe back, after saying what is lost.
pub fn confirm_reset(parent: &impl IsA<gtk::Widget>, on_changed: impl Fn() + 'static) {
    let dialog = adw::AlertDialog::builder()
        .heading("Use the recipe Cabin ships with?")
        .body(
            "Your copy of the guest files will be deleted, including anything \
             you changed in it. Images you have already built are not affected.",
        )
        .build();
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("reset", "Delete My Copy");
    dialog.set_response_appearance("reset", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));

    let parent_widget = parent.as_ref().clone();
    dialog.connect_response(None, move |_, response| {
        if response != "reset" {
            return;
        }
        if let Err(message) = guestdir::reset() {
            let error = adw::AlertDialog::builder()
                .heading("Could not delete your copy")
                .body(message)
                .build();
            error.add_response("close", "Close");
            error.present(Some(&parent_widget));
            return;
        }
        on_changed();
    });
    dialog.present(Some(parent));
}

fn missing(parent: &impl IsA<gtk::Widget>) {
    let error = adw::AlertDialog::builder()
        .heading("The guest files are missing")
        .body(format!(
            "Cabin builds the guest image from files it ships with, and could \
             not find them.\n\nIt looked in:\n{}",
            guestdir::searched()
        ))
        .build();
    error.add_response("close", "Close");
    error.present(Some(parent));
}

/// What each way of starting a recipe costs, measured on the development host
/// by adding one package to the base.
const BASIS_DESCRIPTIONS: &[&str] = &[
    "Adds to the Cabin desktop. Builds in seconds and follows Cabin's updates. \
     Right for adding software.",
    "Copies every line of Cabin's recipe, with notes on what each one is for. \
     Builds in minutes and stops following Cabin's updates. Right for changing \
     the desktop itself.",
];

/// Asks for a name, creates the recipe, and opens it ready to edit.
pub fn present_new(parent: &impl IsA<gtk::Widget>, on_changed: impl Fn() + 'static) {
    let on_changed: Rc<dyn Fn()> = Rc::new(on_changed);
    let taken: Vec<String> = crate::recipes::all().into_iter().map(|r| r.name).collect();

    let dialog = adw::AlertDialog::builder()
        .heading("New Recipe")
        .body(
            "A recipe builds a guest image, and you pick which image a desktop \
             uses when you create it.",
        )
        .build();

    let name_row = adw::EntryRow::builder().title("Name").build();

    // Two ways to start, and the difference is worth stating in numbers
    // rather than in adjectives.
    let basis_row = adw::ComboRow::builder()
        .title("Start from")
        .model(&gtk::StringList::new(&[
            "The Cabin guest image",
            "A full copy of the Cabin recipe",
        ]))
        .subtitle(BASIS_DESCRIPTIONS[0])
        .build();
    basis_row.set_subtitle_lines(0);
    basis_row.connect_selected_notify(|row| {
        row.set_subtitle(BASIS_DESCRIPTIONS[(row.selected() as usize).min(1)]);
    });

    let group = adw::PreferencesGroup::new();
    group.add(&name_row);
    group.add(&basis_row);
    dialog.set_extra_child(Some(&group));

    dialog.add_response("cancel", "Cancel");
    dialog.add_response("create", "Create");
    dialog.set_response_appearance("create", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("create"));
    dialog.set_close_response("cancel");

    // Checked as it is typed. An AdwAlertDialog closes on any response, so a
    // name rejected on the click costs the user the dialogue.
    let dialog_for_check = dialog.clone();
    let group_for_check = group.clone();
    let taken_for_check = taken.clone();
    let check = move |entry: &adw::EntryRow| match crate::recipes::check_name(
        &entry.text(),
        &taken_for_check,
    ) {
        Ok(()) => {
            entry.remove_css_class("error");
            group_for_check.set_description(None);
            dialog_for_check.set_response_enabled("create", true);
        }
        Err(why) => {
            entry.add_css_class("error");
            group_for_check.set_description(Some(&why));
            dialog_for_check.set_response_enabled("create", false);
        }
    };
    name_row.connect_changed(check.clone());
    check(&name_row);

    let parent_widget = parent.as_ref().clone();
    dialog.connect_response(None, move |_, response| {
        if response != "create" {
            return;
        }
        let basis = if basis_row.selected() == 0 {
            crate::recipes::Basis::Layer
        } else {
            crate::recipes::Basis::FullCopy
        };
        match crate::recipes::create(&name_row.text(), basis) {
            Ok(recipe) => {
                on_changed();
                // Straight into the editor. A recipe that only says FROM is
                // not why anyone made one.
                let again = on_changed.clone();
                present_path(&parent_widget, recipe.dir, move || again());
            }
            Err(message) => {
                let error = adw::AlertDialog::builder()
                    .heading("Could not create the recipe")
                    .body(message)
                    .build();
                error.add_response("close", "Close");
                error.present(Some(&parent_widget));
            }
        }
    });

    dialog.present(Some(parent));
}
