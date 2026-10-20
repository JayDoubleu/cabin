//! Viewing and editing a guest recipe.
//!
//! The recipe shipped with Cabin is read-only, because it lives inside the
//! installation. A user who wants a different desktop makes a recipe of their
//! own, which may be a copy of the shipped one. See DESIGN.md section 20.

use crate::guestdir;
use adw::prelude::*;
use std::path::PathBuf;
use std::rc::Rc;

/// Shows the recipe in `dir`, editable unless it is the shipped tree.
pub fn present_path(parent: &impl IsA<gtk::Widget>, dir: PathBuf, on_changed: impl Fn() + 'static) {
    present_path_boxed(parent.as_ref(), dir, Rc::new(on_changed));
}

/// The body of `present_path`, with the callback's type erased.
///
/// This dialogue opens the New Recipe dialogue, which opens this one again.
/// While the callback stays generic each hop is a fresh closure type, so the
/// compiler instantiates the pair forever and gives up at the recursion limit.
fn present_path_boxed(parent: &gtk::Widget, dir: PathBuf, on_changed: Rc<dyn Fn()>) {
    // The shipped tree is the only one that cannot be written, and it is the
    // only one that is not under the user's own directory.
    let shipped = guestdir::is_shipped(&dir);
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

    let editable = !shipped;

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
        .title(if shipped {
            "This is the recipe Cabin ships with. Copy it to make one you can change."
        } else {
            "This is your recipe. Edit it, then build it to use it."
        })
        .revealed(true)
        .build();

    if shipped {
        banner.set_button_label(Some("Copy It"));
        let parent_widget = parent.clone();
        let dialog_ref = dialog.clone();
        let on_changed_ref = on_changed.clone();
        banner.connect_button_clicked(move |_| {
            dialog_ref.close();
            present_new_boxed(
                &parent_widget,
                crate::recipes::Basis::FullCopy,
                on_changed_ref.clone(),
            );
        });
    } else if guestdir::shipped_has_moved_on(&dir) {
        // An update brought a newer default, and this copy predates it.
        banner.set_title(
            "Cabin now ships a newer recipe. This copy does not have the \
             changes, and will not get them.",
        );
        banner.add_css_class("warning");
    }

    let header = adw::HeaderBar::new();

    let copy = gtk::Button::builder()
        .icon_name("edit-copy-symbolic")
        .tooltip_text("Copy the whole recipe")
        .build();
    // The buffer, not the text read at open time, so unsaved edits are copied
    // as the user sees them.
    let copy_buffer = view.buffer();
    copy.connect_clicked(move |button| {
        let current = copy_buffer.text(&copy_buffer.start_iter(), &copy_buffer.end_iter(), false);
        button.display().clipboard().set_text(&current);
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

        // One write for the Save button and for the question asked on close,
        // so the two cannot drift apart. True when the recipe was saved.
        let write: Rc<dyn Fn(&gtk::Widget) -> bool> = {
            let buffer = buffer.clone();
            let on_changed = on_changed.clone();
            Rc::new(move |anchor| {
                let text = buffer
                    .text(&buffer.start_iter(), &buffer.end_iter(), false)
                    .to_string();
                match std::fs::write(&path, text) {
                    Ok(()) => {
                        on_changed();
                        true
                    }
                    Err(e) => {
                        let error = adw::AlertDialog::builder()
                            .heading("Could not save the recipe")
                            .body(e.to_string())
                            .build();
                        error.add_response("close", "Close");
                        error.present(Some(anchor));
                        false
                    }
                }
            })
        };

        // Escape and the close button would otherwise drop every edit
        // without a word. Comparing with the text as opened, rather than
        // noting any change, lets an undo back to it close freely again.
        let dialog_ref = dialog.clone();
        buffer.connect_changed(move |buffer| {
            let current = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
            dialog_ref.set_can_close(current.as_str() == text.as_str());
        });

        let write_ref = write.clone();
        dialog.connect_close_attempt(move |dialog| {
            let question = adw::AlertDialog::builder()
                .heading("Save changes to the recipe?")
                .body("Changes that are not saved are lost.")
                .build();
            question.add_response("cancel", "Cancel");
            question.add_response("discard", "Discard");
            question.add_response("save", "Save");
            question.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
            question.set_response_appearance("save", adw::ResponseAppearance::Suggested);
            question.set_default_response(Some("cancel"));
            question.set_close_response("cancel");

            let dialog_ref = dialog.clone();
            let write_ref = write_ref.clone();
            question.connect_response(None, move |_, response| match response {
                "discard" => {
                    dialog_ref.force_close();
                }
                // If the write fails, the dialogue stays open with the error.
                "save" if write_ref(dialog_ref.upcast_ref()) => {
                    dialog_ref.force_close();
                }
                _ => {}
            });
            question.present(Some(dialog));
        });

        let dialog_ref = dialog.clone();
        save.connect_clicked(move |button| {
            // force_close, because close would ask about the changes that
            // were just saved.
            if write(button.upcast_ref()) {
                dialog_ref.force_close();
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

/// What each way of starting a recipe costs, measured on the development host
/// by adding one package to the base.
const BASIS_DESCRIPTIONS: &[&str] = &[
    "Adds to the Cabin desktop. Builds in seconds and keeps up with Cabin's \
     updates. Right for adding software.",
    "Copies every line of Cabin's recipe, with notes on what each is for. \
     Builds in minutes and stops following Cabin's updates. Right for \
     changing the desktop itself.",
];

/// Asks for a name, creates the recipe, and opens it ready to edit.
pub fn present_new(parent: &impl IsA<gtk::Widget>, on_changed: impl Fn() + 'static) {
    present_new_boxed(
        parent.as_ref(),
        crate::recipes::Basis::Layer,
        Rc::new(on_changed),
    );
}

/// The same dialogue, opened on `basis`.
///
/// Copying the shipped recipe and adding to it are the same act with a
/// different starting point, so they are one dialogue rather than two that
/// have to be kept in step.
pub fn present_new_from(
    parent: &impl IsA<gtk::Widget>,
    basis: crate::recipes::Basis,
    on_changed: impl Fn() + 'static,
) {
    present_new_boxed(parent.as_ref(), basis, Rc::new(on_changed));
}

/// The body of `present_new_from`, with the callback's type erased. See
/// `present_path_boxed` for why.
fn present_new_boxed(parent: &gtk::Widget, basis: crate::recipes::Basis, on_changed: Rc<dyn Fn()>) {
    let taken: Vec<String> = crate::recipes::all().into_iter().map(|r| r.name).collect();

    let dialog = adw::AlertDialog::builder()
        .heading("New Recipe")
        .body(
            "A recipe builds a guest image, and you pick which image a desktop \
             uses when you create it.",
        )
        .build();

    let name_row = adw::EntryRow::builder().title("Name").build();
    // A name that is free, so the dialogue opens ready to use rather than
    // opening with an error against a name the user did not choose.
    name_row.set_text(&first_free_name(
        if basis == crate::recipes::Basis::FullCopy {
            "my-guest"
        } else {
            "my-tools"
        },
        &taken,
    ));

    // Two ways to start, and the difference is worth stating in numbers
    // rather than in adjectives.
    let basis_row = adw::ComboRow::builder()
        .title("Start from")
        .model(&gtk::StringList::new(&[
            "The Cabin guest image",
            "A full copy of the recipe",
        ]))
        .build();
    // use_subtitle puts the chosen option on its own line. Sharing the row
    // with the title leaves it about eight characters wide, which turns
    // "The Cabin guest image" into "The Cab...".
    basis_row.set_use_subtitle(true);
    basis_row.set_subtitle_lines(0);
    if basis == crate::recipes::Basis::FullCopy {
        basis_row.set_selected(1);
    }

    // The explanation needs the full width, so it is a row of its own rather
    // than the combo's subtitle.
    let basis_note = adw::ActionRow::builder()
        .title(BASIS_DESCRIPTIONS[basis_row.selected() as usize])
        .css_classes(vec!["dim-label"])
        .build();
    basis_note.set_title_lines(0);
    basis_note.set_use_markup(false);
    let note_for_basis = basis_note.clone();
    basis_row.connect_selected_notify(move |row| {
        note_for_basis.set_title(BASIS_DESCRIPTIONS[(row.selected() as usize).min(1)]);
    });

    let group = adw::PreferencesGroup::new();
    group.add(&name_row);
    group.add(&basis_row);
    group.add(&basis_note);
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

    let parent_widget = parent.clone();
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
                present_path_boxed(&parent_widget, recipe.dir, on_changed.clone());
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

/// Suggests a recipe name that is not in use.
fn first_free_name(base: &str, taken: &[String]) -> String {
    if !taken.iter().any(|t| t == base) {
        return base.to_string();
    }
    for suffix in 2..100 {
        let candidate = format!("{base}-{suffix}");
        if !taken.iter().any(|t| t == &candidate) {
            return candidate;
        }
    }
    base.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggests_a_name_that_is_free() {
        assert_eq!(first_free_name("my-guest", &[]), "my-guest");
        assert_eq!(
            first_free_name("my-guest", &["my-guest".to_string()]),
            "my-guest-2"
        );
    }

    #[test]
    fn the_suggested_name_passes_the_check_the_dialogue_applies() {
        // The dialogue disables Create for a name that fails, so a suggestion
        // that fails would open the dialogue unusable.
        for basis in ["my-guest", "my-tools"] {
            assert!(
                crate::recipes::check_name(&first_free_name(basis, &[]), &[]).is_ok(),
                "{basis}"
            );
        }
    }

    #[test]
    fn every_basis_has_its_own_description() {
        // The note under the dropdown is indexed by the selection, so a
        // missing entry would panic rather than show nothing.
        assert_eq!(BASIS_DESCRIPTIONS.len(), 2);
        assert_ne!(BASIS_DESCRIPTIONS[0], BASIS_DESCRIPTIONS[1]);
    }
}
