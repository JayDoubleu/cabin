//! Guest image management.
//!
//! Cabin's guest image is built on this machine, never downloaded. A fresh
//! installation therefore has no image, and the first desktop fails with
//! "no such image" unless something offers to build it. This dialogue is that
//! something. See DESIGN.md section 19.

use crate::runtime;
use adw::prelude::*;
use cabin_podman::images::human_bytes;
use cabin_podman::{Image, Podman};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

/// Holds the refresh closure, which cannot be built until the widgets it
/// updates exist, and which those widgets' handlers then need to call.
type RefreshCell = Rc<RefCell<Option<Rc<dyn Fn()>>>>;

/// Opens the image manager.
pub fn present(
    parent: &impl IsA<gtk::Widget>,
    podman: Arc<Podman>,
    on_changed: impl Fn() + 'static,
) {
    let on_changed: Rc<dyn Fn()> = Rc::new(on_changed);

    let dialog = adw::Dialog::builder()
        .title("Guest Images")
        .content_width(720)
        .content_height(620)
        .build();

    let page = adw::PreferencesPage::new();

    // Recipes first. This is the part that answers "what can I make a desktop
    // from", and the built images below are the consequence of it.
    let recipe_group = adw::PreferencesGroup::builder()
        .title("Recipes")
        .description(
            "A recipe says what goes into a guest image. Cabin ships one and you \
             can add your own. Build a recipe to get its image, then pick that \
             image when you create a desktop. The star marks the one new \
             desktops start on.",
        )
        .build();
    let recipe_list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(vec!["boxed-list"])
        .build();
    let add_recipe = gtk::Button::builder()
        .label("New Recipe")
        .halign(gtk::Align::Start)
        .margin_top(6)
        .build();
    let recipe_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    recipe_box.append(&recipe_list);
    recipe_box.append(&add_recipe);
    recipe_group.add(&recipe_box);
    page.add(&recipe_group);

    let group = adw::PreferencesGroup::builder()
        .title("Built images")
        .description("What is on this computer now.")
        .build();
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(vec!["boxed-list"])
        .build();
    group.add(&list);
    page.add(&group);

    let reclaim = gtk::Button::builder()
        .css_classes(vec!["pill"])
        .halign(gtk::Align::Center)
        .margin_top(6)
        .visible(false)
        .build();
    let reclaim_group = adw::PreferencesGroup::new();
    reclaim_group.add(&reclaim);
    page.add(&reclaim_group);
    // What the last refresh found, so the confirmation lists exactly what it
    // is about to remove instead of describing it in the abstract.
    let stale_images: Rc<RefCell<Vec<Image>>> = Rc::new(RefCell::new(Vec::new()));

    // Cabin's own images only, unless asked otherwise. The host carries plenty
    // that belongs to other work, and an image manager that offers to delete
    // those by default is a trap rather than a convenience.
    let show_all = adw::SwitchRow::builder()
        .title("Show every image on this computer")
        .subtitle("Including images that have nothing to do with Cabin.")
        .active(false)
        .build();
    let switch_group = adw::PreferencesGroup::new();
    switch_group.add(&show_all);
    page.add(&switch_group);

    let other_list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(vec!["boxed-list"])
        .build();
    let other_holder = adw::PreferencesGroup::builder()
        .title("Other images")
        .visible(false)
        .build();
    other_holder.add(&other_list);
    page.add(&other_holder);

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(&page));
    dialog.set_child(Some(&toolbar));

    // One refresh for the whole dialogue. Recipes and images are two views of
    // the same question, and letting them update separately would show a
    // recipe as unbuilt beside the image it just built.
    let refresh: RefreshCell = Rc::new(RefCell::new(None));
    let refresh_now: Rc<dyn Fn()> = {
        let refresh = refresh.clone();
        Rc::new(move || {
            let held = refresh.borrow().clone();
            if let Some(f) = held {
                f();
            }
        })
    };

    let built: Rc<dyn Fn()> = {
        let recipe_list = recipe_list.clone();
        let list = list.clone();
        let other_list = other_list.clone();
        let other_holder = other_holder.clone();
        let show_all = show_all.clone();
        let reclaim = reclaim.clone();
        let stale_images = stale_images.clone();
        let podman = podman.clone();
        let dialog = dialog.clone();
        let on_changed = on_changed.clone();
        let refresh_now = refresh_now.clone();
        Rc::new(move || {
            let recipe_list = recipe_list.clone();
            let list = list.clone();
            let other_list = other_list.clone();
            let other_holder = other_holder.clone();
            let show_all = show_all.clone();
            let reclaim = reclaim.clone();
            let stale_images = stale_images.clone();
            let podman_for_rows = podman.clone();
            let dialog = dialog.clone();
            let on_changed = on_changed.clone();
            let refresh_now = refresh_now.clone();
            let podman = podman.clone();
            runtime::spawn(async move { podman.list_images().await }, move |result| {
                let images = match result {
                    Ok(images) => images,
                    Err(e) => {
                        tracing::error!("could not list images: {e}");
                        Vec::new()
                    }
                };

                populate_recipes(
                    &recipe_list,
                    &images,
                    podman_for_rows.clone(),
                    &dialog,
                    refresh_now.clone(),
                    on_changed.clone(),
                );

                let (ours, theirs): (Vec<_>, Vec<_>) =
                    images.iter().cloned().partition(|i| i.is_guest);
                populate(
                    &list,
                    &ours,
                    podman_for_rows.clone(),
                    &dialog,
                    refresh_now.clone(),
                    on_changed.clone(),
                );
                populate(
                    &other_list,
                    &theirs,
                    podman_for_rows.clone(),
                    &dialog,
                    refresh_now.clone(),
                    on_changed.clone(),
                );
                other_holder.set_visible(show_all.is_active());

                // Offer the clean-up only when there is something to clean
                // up, and say what it is worth. An untagged image that a
                // container still uses is not pruned, so it is not counted.
                let stale: Vec<Image> = images.iter().filter(|i| i.prunable()).cloned().collect();
                let freed: u64 = stale.iter().map(|i| i.size_bytes).sum();
                reclaim.set_visible(!stale.is_empty());
                if !stale.is_empty() {
                    // The trailing ellipsis is the platform's own way of
                    // saying that a button asks before it acts.
                    reclaim.set_label(&format!(
                        "Delete {} Unused Image{} ({})\u{2026}",
                        stale.len(),
                        if stale.len() == 1 { "" } else { "s" },
                        human_bytes(freed)
                    ));
                }
                stale_images.replace(stale);
            });
        })
    };
    refresh.replace(Some(built));
    refresh_now();

    let refresh_for_show_all = refresh_now.clone();
    show_all.connect_active_notify(move |_| refresh_for_show_all());

    let refresh_after_prune = refresh_now.clone();
    let dialog_for_prune = dialog.clone();
    let podman_for_prune = podman.clone();
    let on_changed_for_prune = on_changed.clone();
    let stale_for_prune = stale_images.clone();
    reclaim.connect_clicked(move |_| {
        confirm_prune(
            &dialog_for_prune,
            podman_for_prune.clone(),
            stale_for_prune.borrow().clone(),
            refresh_after_prune.clone(),
            on_changed_for_prune.clone(),
        );
    });

    let refresh_after_add = refresh_now.clone();
    let dialog_for_add = dialog.clone();
    add_recipe.connect_clicked(move |_| {
        let refresh = refresh_after_add.clone();
        crate::recipe::present_new(&dialog_for_add, move || refresh());
    });

    dialog.present(Some(parent));
}

/// One row per recipe, saying whether its image is built and offering to
/// build it.
fn populate_recipes(
    list: &gtk::ListBox,
    images: &[Image],
    podman: Arc<Podman>,
    parent: &adw::Dialog,
    refresh: Rc<dyn Fn()>,
    on_changed: Rc<dyn Fn()>,
) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }

    let base_built = images
        .iter()
        .any(|i| i.tags.iter().any(|t| t == crate::recipes::BASE_TAG));
    let default_tag = crate::recipes::default_tag();

    for recipe in crate::recipes::all() {
        let image = images.iter().find(|i| i.tags.contains(&recipe.tag));
        // Only a layer starts `FROM` the base image, so only a layer has to
        // wait for it. A full copy starts from Fedora and builds on its own.
        let needs_base = recipe.kind == crate::recipes::Kind::Custom && !recipe.customised;

        let row = adw::ActionRow::builder().title(&recipe.name).build();
        // These carry data, not markup. A name or a tag holding an angle
        // bracket or an ampersand would otherwise be read as markup, and the
        // label dropped with a warning rather than shown.
        row.set_use_markup(false);
        row.set_subtitle_lines(0);
        // Anything qualifying the name goes beside it. Putting it in the name
        // makes the name too long for the dropdown that also shows it.
        let badge = match recipe.kind {
            crate::recipes::Kind::Shipped => Some("Shipped with Cabin"),
            crate::recipes::Kind::Custom if recipe.customised => Some("Your copy"),
            crate::recipes::Kind::Custom => Some("Yours"),
        };
        if let Some(text) = badge {
            row.add_suffix(
                &gtk::Label::builder()
                    .label(text)
                    .css_classes(vec!["caption", "dim-label"])
                    .valign(gtk::Align::Center)
                    .build(),
            );
        }
        row.set_subtitle(&match image {
            Some(image) => format!("Built, {} \u{b7} {}", image.human_size(), recipe.tag),
            // A layer builds on the base, so it cannot be built before the
            // base is. Say that instead of letting Podman report a missing
            // image the user never asked for.
            None if needs_base && !base_built => {
                "Not built. Build the Cabin guest image first.".to_string()
            }
            None => format!("Not built yet \u{b7} {}", recipe.tag),
        });

        // Which image a new desktop starts on. A toggle rather than a menu
        // entry, because the answer has to be visible as well as settable:
        // the lit star is how the user sees which recipe it is.
        let is_default = recipe.tag == default_tag;
        let star = gtk::ToggleButton::builder()
            .icon_name(if is_default {
                "starred-symbolic"
            } else {
                "non-starred-symbolic"
            })
            .valign(gtk::Align::Center)
            .css_classes(vec!["flat"])
            .active(is_default)
            .sensitive(image.is_some() && !is_default)
            .tooltip_text(if is_default {
                "New desktops start on this image"
            } else if image.is_some() {
                "Use this image for new desktops"
            } else {
                "Build this image before new desktops can use it"
            })
            .build();
        let refresh_for_star = refresh.clone();
        let tag_for_star = recipe.tag.clone();
        star.connect_toggled(move |button| {
            if !button.is_active() {
                return;
            }
            crate::recipes::set_default_tag(&tag_for_star);
            refresh_for_star();
        });
        row.add_suffix(&star);

        let build = gtk::Button::builder()
            .label(if image.is_some() { "Rebuild" } else { "Build" })
            .valign(gtk::Align::Center)
            .css_classes(vec![if image.is_some() {
                "flat"
            } else {
                "suggested-action"
            }])
            .sensitive(!needs_base || base_built)
            .build();
        let parent_for_build = parent.clone();
        let podman_for_build = podman.clone();
        let refresh_for_build = refresh.clone();
        let on_changed_for_build = on_changed.clone();
        let tag = recipe.tag.clone();
        let dir = recipe.dir.clone();
        build.connect_clicked(move |_| {
            let refresh = refresh_for_build.clone();
            let on_changed = on_changed_for_build.clone();
            present_build_from(
                &parent_for_build,
                podman_for_build.clone(),
                &tag,
                dir.clone(),
                move || {
                    refresh();
                    on_changed();
                },
            );
        });
        row.add_suffix(&build);
        row.add_suffix(&recipe_menu(
            &recipe,
            parent,
            refresh.clone(),
            podman.clone(),
        ));
        list.append(&row);
    }
}

/// The per-recipe menu: view or edit, and remove.
fn recipe_menu(
    recipe: &crate::recipes::Recipe,
    parent: &adw::Dialog,
    refresh: Rc<dyn Fn()>,
    podman: Arc<Podman>,
) -> gtk::MenuButton {
    let popover = gtk::Popover::new();
    let box_ = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .margin_top(6)
        .margin_bottom(6)
        .margin_start(6)
        .margin_end(6)
        .build();

    let open = flat_button(
        if recipe.editable() {
            "Edit recipe"
        } else {
            "View recipe"
        },
        "text-x-generic-symbolic",
    );
    let parent_for_open = parent.clone();
    let refresh_for_open = refresh.clone();
    let dir_for_open = recipe.dir.clone();
    let popover_ref = popover.clone();
    open.connect_clicked(move |_| {
        popover_ref.popdown();
        let refresh = refresh_for_open.clone();
        crate::recipe::present_path(&parent_for_open, dir_for_open.clone(), move || refresh());
    });
    box_.append(&open);

    match recipe.kind {
        crate::recipes::Kind::Shipped => {
            // The shipped recipe cannot be changed, so the only thing to
            // offer is a copy that can. The copy is a recipe of its own, so
            // this one stays where it is and stays selectable.
            let action = flat_button("Copy to a new recipe", "edit-copy-symbolic");
            let parent_for_action = parent.clone();
            let refresh_for_action = refresh.clone();
            let popover_ref = popover.clone();
            action.connect_clicked(move |_| {
                popover_ref.popdown();
                let refresh = refresh_for_action.clone();
                crate::recipe::present_new_from(
                    &parent_for_action,
                    crate::recipes::Basis::FullCopy,
                    move || refresh(),
                );
            });
            box_.append(&action);
        }
        crate::recipes::Kind::Custom => {
            box_.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
            let remove = flat_button("Delete recipe", "user-trash-symbolic");
            remove.add_css_class("destructive-action");
            let parent_for_remove = parent.clone();
            let refresh_for_remove = refresh.clone();
            let name = recipe.name.clone();
            let tag = recipe.tag.clone();
            let popover_ref = popover.clone();
            let podman = podman.clone();
            remove.connect_clicked(move |_| {
                popover_ref.popdown();
                confirm_delete_recipe(
                    &parent_for_remove,
                    podman.clone(),
                    name.clone(),
                    tag.clone(),
                    refresh_for_remove.clone(),
                );
            });
            box_.append(&remove);
        }
    }

    popover.set_child(Some(&box_));
    gtk::MenuButton::builder()
        .icon_name("view-more-symbolic")
        .valign(gtk::Align::Center)
        .css_classes(vec!["flat"])
        .popover(&popover)
        .build()
}

fn flat_button(label: &str, icon: &str) -> gtk::Button {
    let content = adw::ButtonContent::builder()
        .label(label)
        .icon_name(icon)
        .halign(gtk::Align::Start)
        .build();
    gtk::Button::builder()
        .child(&content)
        .css_classes(vec!["flat"])
        .build()
}

/// Deleting a recipe asks about its image separately, because the two are not
/// the same thing and a desktop may still be running from the image.
fn confirm_delete_recipe(
    parent: &adw::Dialog,
    podman: Arc<Podman>,
    name: String,
    tag: String,
    refresh: Rc<dyn Fn()>,
) {
    let dialog = adw::AlertDialog::builder()
        .heading(format!("Delete the {name} recipe?"))
        .body(
            "The recipe's files are removed. The image it built is left alone, \
             so desktops made from it keep working. You can delete the image \
             separately.",
        )
        .build();
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("delete", "Delete");
    dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));

    let parent_for_response = parent.clone();
    dialog.connect_response(None, move |_, response| {
        if response != "delete" {
            return;
        }
        let _ = (&podman, &tag);
        if let Err(message) = crate::recipes::delete(&name) {
            let error = adw::AlertDialog::builder()
                .heading("Could not delete the recipe")
                .body(message)
                .build();
            error.add_response("close", "Close");
            error.present(Some(&parent_for_response));
            return;
        }
        refresh();
    });
    dialog.present(Some(parent));
}

fn populate(
    list: &gtk::ListBox,
    images: &[Image],
    podman: Arc<Podman>,
    parent: &adw::Dialog,
    refresh: Rc<dyn Fn()>,
    on_changed: Rc<dyn Fn()>,
) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }

    if images.is_empty() {
        list.append(
            &adw::ActionRow::builder()
                .title("No images yet")
                .subtitle("Build the Cabin guest recipe above to make your first desktop.")
                .build(),
        );
        return;
    }

    let default_tag = crate::recipes::default_tag();

    for image in images {
        let row = adw::ActionRow::builder()
            .title(image.display_name())
            .subtitle(format!("{} · {}", image.human_size(), image.short_id()))
            .build();
        // The name comes from Podman, so it is data rather than markup.
        row.set_use_markup(false);
        // The default can be any built image, so the mark has to follow the
        // setting. A fixed tag here would keep pointing at Cabin's own image
        // after the user chose another.
        if image.tags.iter().any(|t| t == &default_tag) {
            let badge = gtk::Label::builder()
                .label("Default")
                .css_classes(vec!["caption", "accent"])
                .valign(gtk::Align::Center)
                .build();
            row.add_suffix(&badge);
        }

        let remove = gtk::Button::builder()
            .icon_name("user-trash-symbolic")
            .valign(gtk::Align::Center)
            .css_classes(vec!["flat"])
            .tooltip_text("Delete this image")
            .build();
        let podman = podman.clone();
        let parent = parent.clone();
        let refresh = refresh.clone();
        let on_changed = on_changed.clone();
        let name = image.display_name();
        let target = image
            .tags
            .first()
            .cloned()
            .unwrap_or_else(|| image.id.clone());
        remove.connect_clicked(move |_| {
            confirm_delete(
                &parent,
                podman.clone(),
                name.clone(),
                target.clone(),
                refresh.clone(),
                on_changed.clone(),
            );
        });
        row.add_suffix(&remove);
        list.append(&row);
    }
}

/// Lists what a prune would remove, and asks.
///
/// A bulk delete that only says how many is not something a person can agree
/// to. The images are named, so the answer is informed rather than trusting.
fn confirm_prune(
    parent: &adw::Dialog,
    podman: Arc<Podman>,
    stale: Vec<Image>,
    refresh: Rc<dyn Fn()>,
    on_changed: Rc<dyn Fn()>,
) {
    let freed: u64 = stale.iter().map(|i| i.size_bytes).sum();
    let dialog = adw::AlertDialog::builder()
        .heading(format!(
            "Delete {} unused image{}?",
            stale.len(),
            if stale.len() == 1 { "" } else { "s" }
        ))
        .body(format!(
            "These images have no name and no container using them. Deleting \
             them frees about {}.\n\nThis is not limited to Cabin: Podman \
             removes every unused image on this computer. A rebuild leaves the \
             image it replaced in this state, which is usually what this \
             reclaims.",
            human_bytes(freed)
        ))
        .build();

    // The review itself. Scrolled, because a host that has not been tidied in
    // a while can have dozens.
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(vec!["boxed-list"])
        .build();
    for image in &stale {
        list.append(
            &adw::ActionRow::builder()
                .title(image.short_id())
                .subtitle(image.human_size())
                .css_classes(vec!["property"])
                .build(),
        );
    }
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .max_content_height(260)
        .propagate_natural_height(true)
        .child(&list)
        .build();
    dialog.set_extra_child(Some(&scroller));

    dialog.add_response("cancel", "Cancel");
    dialog.add_response("delete", "Delete");
    dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));

    let parent_for_response = parent.clone();
    dialog.connect_response(None, move |_, response| {
        if response != "delete" {
            return;
        }
        let podman = podman.clone();
        let refresh = refresh.clone();
        let on_changed = on_changed.clone();
        let parent = parent_for_response.clone();
        runtime::spawn(async move { podman.prune_images().await }, move |result| {
            match result {
                // Report what Podman actually removed rather than what
                // was predicted. The two can differ, because the decision
                // is made on the host at the moment of the request.
                Ok((count, freed)) => {
                    tracing::info!("removed {count} image(s), freeing {freed} bytes");
                    let done = adw::AlertDialog::builder()
                        .heading("Unused images deleted")
                        .body(format!(
                            "{count} image{} removed, freeing {}.",
                            if count == 1 { "" } else { "s" },
                            human_bytes(freed)
                        ))
                        .build();
                    done.add_response("close", "Close");
                    done.present(Some(&parent));
                }
                Err(e) => {
                    let error = adw::AlertDialog::builder()
                        .heading("Could not delete the unused images")
                        .body(e.to_string())
                        .build();
                    error.add_response("close", "Close");
                    error.present(Some(&parent));
                }
            }
            refresh();
            on_changed();
        });
    });
    dialog.present(Some(parent));
}

fn confirm_delete(
    parent: &adw::Dialog,
    podman: Arc<Podman>,
    name: String,
    target: String,
    refresh: Rc<dyn Fn()>,
    on_changed: Rc<dyn Fn()>,
) {
    let dialog = adw::AlertDialog::builder()
        .heading(format!("Delete {name}?"))
        .body(
            "The image will have to be built again before it can make another \
             desktop. Desktops that already exist are not affected.",
        )
        .build();
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("delete", "Delete");
    dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));

    let parent_for_response = parent.clone();
    dialog.connect_response(None, move |_, response| {
        if response != "delete" {
            return;
        }
        let podman = podman.clone();
        let target = target.clone();
        let refresh = refresh.clone();
        let on_changed = on_changed.clone();
        let parent = parent_for_response.clone();
        runtime::spawn(
            async move { podman.remove_image(&target, false).await },
            move |result| {
                if let Err(e) = result {
                    // Podman refuses while a container still uses the image,
                    // and that refusal is the useful part of the message.
                    let error = adw::AlertDialog::builder()
                        .heading("Could not delete the image")
                        .body(e.to_string())
                        .build();
                    error.add_response("close", "Close");
                    error.present(Some(&parent));
                }
                // The dialogue's own lists as well as the sidebar, or the
                // deleted image stays listed and its recipe still reads as
                // built.
                refresh();
                on_changed();
            },
        );
    });
    dialog.present(Some(parent));
}

/// Runs a build, showing its output as it arrives.
pub fn present_build(
    parent: &impl IsA<gtk::Widget>,
    podman: Arc<Podman>,
    tag: &str,
    on_done: impl Fn() + 'static,
) {
    let Some(context_dir) = crate::guestdir::shipped() else {
        let error = adw::AlertDialog::builder()
            .heading("The guest files are missing")
            .body(format!(
                "Cabin builds the guest image from files it ships with, and \
                 could not find them.\n\nIt looked in:\n{}",
                crate::guestdir::searched()
            ))
            .build();
        error.add_response("close", "Close");
        error.present(Some(parent));
        return;
    };
    present_build_from(parent, podman, tag, context_dir, on_done);
}

/// Builds `tag` from the context in `context_dir`, showing its output.
pub fn present_build_from(
    parent: &impl IsA<gtk::Widget>,
    podman: Arc<Podman>,
    tag: &str,
    context_dir: std::path::PathBuf,
    on_done: impl Fn() + 'static,
) {
    tracing::info!("building {tag} from {}", context_dir.display());

    let dialog = adw::Dialog::builder()
        .title("Building the Guest Image")
        .content_width(760)
        .content_height(560)
        .can_close(false)
        .build();

    let output = gtk::TextView::builder()
        .editable(false)
        .monospace(true)
        .cursor_visible(false)
        .wrap_mode(gtk::WrapMode::WordChar)
        .left_margin(12)
        .right_margin(12)
        .top_margin(12)
        .bottom_margin(12)
        .build();
    let buffer = output.buffer();

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .child(&output)
        .build();

    let status = gtk::Label::builder()
        .label("Starting. This takes several minutes the first time.")
        .css_classes(vec!["dim-label"])
        .xalign(0.0)
        .margin_start(12)
        .margin_end(12)
        .margin_top(6)
        .margin_bottom(6)
        .build();

    let close = gtk::Button::builder()
        .label("Close")
        .css_classes(vec!["pill"])
        .halign(gtk::Align::Center)
        .margin_top(6)
        .margin_bottom(12)
        .sensitive(false)
        .build();

    let box_ = gtk::Box::new(gtk::Orientation::Vertical, 0);
    box_.append(&scroller);
    box_.append(&status);
    box_.append(&close);

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(&box_));
    dialog.set_child(Some(&toolbar));

    let dialog_for_close = dialog.clone();
    close.connect_clicked(move |_| {
        dialog_for_close.close();
    });

    dialog.present(Some(parent));

    // Build output arrives on a Tokio thread and the buffer belongs to the
    // main one, so it crosses the same way frames do.
    let (sender, receiver) = async_channel::bounded::<Result<String, String>>(64);
    let tag = tag.to_string();
    let podman_for_build = podman.clone();
    std::thread::Builder::new()
        .name("cabin-image-build".into())
        .spawn(move || {
            let context = match cabin_podman::images::build_context(&context_dir) {
                Ok(context) => context,
                Err(e) => {
                    let _ = sender.send_blocking(Err(e.to_string()));
                    return;
                }
            };
            let outcome = runtime::block_on(async {
                let relay = sender.clone();
                podman_for_build
                    .build_image(&tag, context, |line| {
                        let _ = relay.send_blocking(Ok(line.to_string()));
                    })
                    .await
            });
            let _ = match outcome {
                Ok(()) => sender.send_blocking(Ok(String::from("\u{0}done"))),
                Err(e) => sender.send_blocking(Err(e.to_string())),
            };
        })
        .expect("could not start the build thread");

    let on_done = Rc::new(on_done);
    let finished = Rc::new(RefCell::new(false));
    gtk::glib::spawn_future_local(async move {
        while let Ok(item) = receiver.recv().await {
            match item {
                // A private marker rather than a second channel, so ordering
                // with the output is guaranteed.
                Ok(line) if line == "\u{0}done" => {
                    status.set_label("Finished. The guest image is ready.");
                    status.remove_css_class("dim-label");
                    status.add_css_class("success");
                    *finished.borrow_mut() = true;
                }
                Ok(line) => {
                    let mut end = buffer.end_iter();
                    buffer.insert(&mut end, &line);
                    // Follow the output, which is the only reason to show it.
                    let mark = buffer.create_mark(None, &buffer.end_iter(), false);
                    output.scroll_to_mark(&mark, 0.0, false, 0.0, 0.0);
                }
                Err(message) => {
                    let mut end = buffer.end_iter();
                    buffer.insert(&mut end, &format!("\n{message}\n"));
                    status.set_label("The build failed. The output above says why.");
                    status.remove_css_class("dim-label");
                    status.add_css_class("error");
                    *finished.borrow_mut() = true;
                }
            }
        }
        dialog.set_can_close(true);
        close.set_sensitive(true);
        on_done();
    });
}
