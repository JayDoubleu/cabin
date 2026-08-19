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

/// The image a desktop is built from unless the setting says otherwise.
pub const DEFAULT_TAG: &str = "localhost/cabin-guest:f44";

/// Opens the image manager.
pub fn present(
    parent: &impl IsA<gtk::Widget>,
    podman: Arc<Podman>,
    on_changed: impl Fn() + 'static,
) {
    let on_changed: Rc<dyn Fn()> = Rc::new(on_changed);

    let dialog = adw::Dialog::builder()
        .title("Guest Images")
        .content_width(640)
        .content_height(560)
        .build();

    let page = adw::PreferencesPage::new();

    let group = adw::PreferencesGroup::builder()
        .title("Guest images")
        .description(
            "A desktop runs from a guest image. Cabin builds it here from the \
             files it ships with, so nothing is downloaded from anyone else.",
        )
        .build();

    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(vec!["boxed-list"])
        .build();
    group.add(&list);
    page.add(&group);

    // Cabin's own images only, unless asked otherwise. The host carries
    // plenty that belongs to other work, and an image manager that offers to
    // delete those by default is a trap rather than a convenience.
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

    // The recipe the image is built from. It sits above the build button
    // because it is what the build uses, and a user who wants a different
    // image needs to change this rather than press build again.
    let recipe_group = adw::PreferencesGroup::builder()
        .title("Recipe")
        .description("The Containerfile that says what goes into a guest image.")
        .build();
    let recipe_row = adw::ActionRow::builder().build();
    recipe_row.set_subtitle_lines(0);
    let recipe_list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(vec!["boxed-list"])
        .build();
    recipe_list.append(&recipe_row);
    recipe_group.add(&recipe_list);
    page.add(&recipe_group);

    let recipe_open = gtk::Button::builder()
        .valign(gtk::Align::Center)
        .css_classes(vec!["flat"])
        .build();
    recipe_row.add_suffix(&recipe_open);
    let recipe_reset = gtk::Button::builder()
        .icon_name("edit-undo-symbolic")
        .valign(gtk::Align::Center)
        .css_classes(vec!["flat"])
        .tooltip_text("Go back to the recipe Cabin ships with")
        .visible(false)
        .build();
    recipe_row.add_suffix(&recipe_reset);

    let build = gtk::Button::builder()
        .css_classes(vec!["suggested-action", "pill"])
        .build();
    let reclaim = gtk::Button::builder()
        .css_classes(vec!["pill"])
        .visible(false)
        .build();
    // What the last refresh found, so the confirmation lists exactly what it
    // is about to remove instead of describing it in the abstract.
    let stale_images: Rc<RefCell<Vec<Image>>> = Rc::new(RefCell::new(Vec::new()));

    let actions = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .halign(gtk::Align::Center)
        .margin_top(12)
        .build();
    actions.append(&build);
    actions.append(&reclaim);

    let build_group = adw::PreferencesGroup::new();
    build_group.add(&actions);
    page.add(&build_group);

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(&page));
    dialog.set_child(Some(&toolbar));

    let refresh = {
        let list = list.clone();
        let other_list = other_list.clone();
        let other_holder = other_holder.clone();
        let show_all = show_all.clone();
        let reclaim = reclaim.clone();
        let stale_images = stale_images.clone();
        let build = build.clone();
        let podman = podman.clone();
        let dialog = dialog.clone();
        let on_changed = on_changed.clone();
        Rc::new(move || {
            let list = list.clone();
            let other_list = other_list.clone();
            let other_holder = other_holder.clone();
            let show_all = show_all.clone();
            let reclaim = reclaim.clone();
            let stale_for_rows = stale_images.clone();
            let build = build.clone();
            let podman_for_rows = podman.clone();
            let dialog = dialog.clone();
            let on_changed = on_changed.clone();
            let podman = podman.clone();
            runtime::spawn(async move { podman.list_images().await }, move |result| {
                let images = match result {
                    Ok(images) => images,
                    Err(e) => {
                        tracing::error!("could not list images: {e}");
                        Vec::new()
                    }
                };
                let (ours, theirs): (Vec<_>, Vec<_>) =
                    images.iter().cloned().partition(|i| i.is_guest);
                populate(
                    &list,
                    &ours,
                    podman_for_rows.clone(),
                    &dialog,
                    on_changed.clone(),
                );
                populate(
                    &other_list,
                    &theirs,
                    podman_for_rows.clone(),
                    &dialog,
                    on_changed.clone(),
                );
                other_holder.set_visible(show_all.is_active());

                // The label says which of the two jobs the button does,
                // because building the first image and replacing an
                // existing one are not the same act.
                let have = images
                    .iter()
                    .any(|i| i.tags.iter().any(|t| t == DEFAULT_TAG));
                build.set_label(if have {
                    "Rebuild the Guest Image"
                } else {
                    "Build the Guest Image"
                });

                // Offer the clean-up only when there is something to clean
                // up, and say what it is worth.
                let stale: Vec<Image> = images.iter().filter(|i| i.dangling).cloned().collect();
                let freed: u64 = stale.iter().map(|i| i.size_bytes).sum();
                reclaim.set_visible(!stale.is_empty());
                if !stale.is_empty() {
                    // The trailing ellipsis is the platform's own way of
                    // saying that a button asks before it acts. Spelling that
                    // out in the label would be longer and less familiar.
                    reclaim.set_label(&format!(
                        "Delete {} Unused Image{} ({})\u{2026}",
                        stale.len(),
                        if stale.len() == 1 { "" } else { "s" },
                        human_bytes(freed)
                    ));
                }
                stale_for_rows.replace(stale);
            });
        })
    };
    refresh();

    // The recipe row does not depend on Podman, so it is updated on its own
    // rather than waiting for the image list to come back.
    let show_recipe = {
        let row = recipe_row.clone();
        let open = recipe_open.clone();
        let reset = recipe_reset.clone();
        Rc::new(move || {
            let custom = crate::guestdir::has_custom();
            row.set_title(if custom {
                "Your own recipe"
            } else {
                "The recipe Cabin ships with"
            });
            row.set_subtitle(if custom {
                if crate::guestdir::shipped_has_moved_on() {
                    "Builds use your copy. Cabin now ships a newer one, which your \
                     copy does not have."
                } else {
                    "Builds use your copy instead of the one Cabin ships."
                }
            } else {
                "Read-only, because it lives inside the installation. Take a copy \
                 to change it."
            });
            open.set_label(if custom { "Edit" } else { "View" });
            reset.set_visible(custom);
        })
    };
    show_recipe();

    let show_after_recipe = show_recipe.clone();
    let refresh_after_recipe = refresh.clone();
    let dialog_for_recipe = dialog.clone();
    recipe_open.connect_clicked(move |_| {
        let show = show_after_recipe.clone();
        let refresh = refresh_after_recipe.clone();
        crate::recipe::present(&dialog_for_recipe, move || {
            show();
            refresh();
        });
    });

    let show_after_reset = show_recipe.clone();
    let dialog_for_reset = dialog.clone();
    recipe_reset.connect_clicked(move |_| {
        let show = show_after_reset.clone();
        crate::recipe::confirm_reset(&dialog_for_reset, move || show());
    });

    let refresh_for_show_all = refresh.clone();
    show_all.connect_active_notify(move |_| refresh_for_show_all());

    let refresh_after_prune = refresh.clone();
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

    let refresh_after_build = refresh.clone();
    let dialog_for_build = dialog.clone();
    let podman_for_build = podman.clone();
    let on_changed_for_build = on_changed.clone();
    build.connect_clicked(move |_| {
        let refresh = refresh_after_build.clone();
        let on_changed = on_changed_for_build.clone();
        present_build(
            &dialog_for_build,
            podman_for_build.clone(),
            DEFAULT_TAG,
            move || {
                refresh();
                on_changed();
            },
        );
    });

    dialog.present(Some(parent));
}

fn populate(
    list: &gtk::ListBox,
    images: &[Image],
    podman: Arc<Podman>,
    parent: &adw::Dialog,
    on_changed: Rc<dyn Fn()>,
) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }

    if images.is_empty() {
        list.append(
            &adw::ActionRow::builder()
                .title("No images yet")
                .subtitle("Build the guest image to make your first desktop.")
                .build(),
        );
        return;
    }

    for image in images {
        let row = adw::ActionRow::builder()
            .title(image.display_name())
            .subtitle(format!("{} · {}", image.human_size(), image.short_id()))
            .build();
        if image.tags.iter().any(|t| t == DEFAULT_TAG) {
            let badge = gtk::Label::builder()
                .label("In use by Cabin")
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
    let Some((context_dir, source)) = crate::guestdir::active() else {
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
    tracing::info!(
        "building from the {} recipe at {}",
        match source {
            crate::guestdir::Source::Shipped => "shipped",
            crate::guestdir::Source::Custom => "customised",
        },
        context_dir.display()
    );

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
