//! The "New Desktop" dialogue.

use crate::graphics;
use crate::runtime;
use adw::prelude::*;
use cabin_core::{DesktopConfig, GraphicsMode};
use cabin_podman::Podman;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

/// The guest RDP port maps to a free host port in this range. It sits above
/// the usual ephemeral services and below the ephemeral allocation range.
const PORT_RANGE: std::ops::Range<u16> = 13389..13500;

/// What already exists on the host, so the dialogue can refuse a clash as
/// it is typed rather than after Create is pressed.
#[derive(Clone, Default)]
pub struct InUse {
    /// The names of Cabin's desktops.
    pub desktops: Vec<String>,
    /// The host port that every container publishes, running or not, Cabin's
    /// or not. A stopped container holds no socket, so a bind test alone
    /// would hand its port out again.
    pub ports: Vec<u16>,
    /// The name of every container on the host, Cabin's or not.
    pub containers: Vec<String>,
    /// The name of every volume on the host.
    pub volumes: Vec<String>,
}

impl InUse {
    /// Whether a desktop can take this name.
    ///
    /// Podman refuses a container name that any container holds, and that
    /// refusal would arrive after the dialogue closed. Volumes left by a
    /// desktop with the same name would hand its files and certificate to
    /// the new one. Other containers are only used to refuse their names,
    /// never shown as desktops.
    fn check(&self, name: &str) -> Result<(), String> {
        cabin_core::check_name(name, &self.desktops)?;
        let name = name.trim();
        if self.containers.iter().any(|c| c == name) {
            return Err(format!("A container called {name} already exists."));
        }
        let home = cabin_core::home_volume_name(name);
        let state = cabin_core::state_volume_name(name);
        if self.volumes.iter().any(|v| *v == home || *v == state) {
            return Err(format!(
                "Old files for a desktop called {name} still exist."
            ));
        }
        Ok(())
    }
}

/// Presents the New Desktop dialogue.
///
/// `on_created` receives the name of the desktop that was made and whether the
/// user asked for its settings. Only the essentials are asked for here.
/// Everything else is in the desktop's own settings.
pub fn present(
    parent: &impl IsA<gtk::Widget>,
    podman: Arc<Podman>,
    in_use: InUse,
    on_created: impl Fn(String, bool) + 'static,
) {
    let in_use = Rc::new(in_use);
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
    name_row.set_text(&first_free_name("fedora", |name| {
        in_use.check(name).is_ok()
    }));

    let memory_row = adw::SpinRow::with_range(1024.0, 65536.0, 1024.0);
    memory_row.set_title("Memory");
    memory_row.set_subtitle("Megabytes");
    memory_row.set_value(4096.0);

    // Offer software rendering plus every render node, labelled with its
    // driver. Never guess the driver from the node number.
    let nodes = graphics::render_nodes();
    // The mode behind each entry, in the same order, so the selection maps
    // through a list rather than through arithmetic on its position.
    let mut options: Vec<String> = vec!["Automatic".into(), "Software rendering".into()];
    let mut modes: Vec<GraphicsMode> = vec![GraphicsMode::Automatic, GraphicsMode::Software];
    for (path, driver) in &nodes {
        let leaf = path.rsplit('/').next().unwrap_or(path.as_str());
        options.push(format!("{leaf} ({driver})"));
        modes.push(GraphicsMode::RenderNode { path: path.clone() });
    }
    let option_refs: Vec<&str> = options.iter().map(String::as_str).collect();
    // On its own line, because the entries are longer than a word and the
    // title's line leaves them a few characters (rule 32).
    let graphics_row = adw::ComboRow::builder()
        .title("Graphics")
        .use_subtitle(true)
        .model(&gtk::StringList::new(&option_refs))
        .build();
    // The entries carry device paths and driver names, which are data.
    graphics_row.set_use_markup(false);

    // Shared folders and the rest are not asked for here, so offer the way to
    // them rather than leaving the user to find the settings afterwards.
    let open_settings = adw::SwitchRow::builder()
        .title("Open settings")
        .subtitle("Set up shared folders and access straight away.")
        .active(false)
        .build();

    // Which image the desktop is built from. Only images that exist are
    // offered, because a desktop cannot be made from one that does not.
    // use_subtitle puts the chosen image on its own line. Without it the name
    // shares the row with the title and is truncated to "Cabin guest (de...",
    // which is the one thing the row exists to show.
    let image_row = adw::ComboRow::builder().title("Image").build();
    image_row.set_use_subtitle(true);
    image_row.set_subtitle_lines(0);
    // Which image that name refers to, kept out of the name so the name stays
    // short enough for a dropdown.
    let detail_row = adw::ActionRow::builder()
        .title("From")
        .css_classes(vec!["property"])
        .build();
    detail_row.set_use_markup(false);
    detail_row.set_subtitle_lines(0);
    // The tag for each entry, in the same order as the model.
    let image_tags: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));

    // A fresh installation has no image at all, and creating a desktop then
    // fails with "no such image" after everything here has been filled in.
    // Ask Podman now, and offer the way out rather than only the message.
    let image_missing = adw::ActionRow::builder()
        .title("This image is not built yet")
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
    group.add(&image_row);
    group.add(&detail_row);
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
    let in_use_for_name = in_use.clone();
    let check = move |entry: &adw::EntryRow| match in_use_for_name.check(&entry.text()) {
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

    let default_image = crate::recipes::default_tag();

    // Re-run after a build, so a new image appears without reopening.
    let row_for_check = image_missing.clone();
    let combo_for_check = image_row.clone();
    let detail_for_check = detail_row.clone();
    let tags_for_check = image_tags.clone();
    let dialog_for_check = dialog.clone();
    let name_for_check = name_row.clone();
    let taken_for_check = in_use.clone();
    let podman_for_check = podman.clone();
    // The image chosen in the image manager, or Cabin's own when none was.
    let preferred = crate::recipes::default_tag();
    let check_image: Rc<dyn Fn()> = Rc::new(move || {
        let row = row_for_check.clone();
        let combo = combo_for_check.clone();
        let detail_row = detail_for_check.clone();
        let tags = tags_for_check.clone();
        let dialog = dialog_for_check.clone();
        let name = name_for_check.clone();
        let taken = taken_for_check.clone();
        let preferred = preferred.clone();
        let podman = podman_for_check.clone();
        runtime::spawn(async move { podman.list_images().await }, move |result| {
            // A Podman that cannot answer is not a missing image, so say
            // nothing rather than send the user to build one they have.
            let Ok(images) = result else {
                return;
            };
            let choices = buildable_images(&images);

            let labels: Vec<&str> = choices.iter().map(|c| c.label.as_str()).collect();
            combo.set_model(Some(&gtk::StringList::new(&labels)));
            combo.set_sensitive(!choices.is_empty());
            // Land on the chosen default, and only if its image exists.
            // Opening on a choice that cannot be used would leave Create
            // disabled with nothing saying which selection would fix it.
            let landing = choices
                .iter()
                .position(|c| c.tag == preferred && c.built)
                .or_else(|| choices.iter().position(|c| c.built))
                .unwrap_or(0);
            combo.set_selected(landing as u32);

            // The dropdown shows a name. The rows beneath say which image
            // that is and whether it exists, because only the tag is
            // unambiguous and only a built image can be used.
            let details: Vec<String> = choices.iter().map(|c| c.detail.clone()).collect();
            let built: Vec<bool> = choices.iter().map(|c| c.built).collect();
            let name_for_settle = name.clone();
            let taken_for_settle = taken.clone();
            let dialog_for_settle = dialog.clone();
            let row_for_settle = row.clone();
            let settle = move |combo: &adw::ComboRow| {
                let index = combo.selected() as usize;
                detail_row.set_subtitle(details.get(index).map(String::as_str).unwrap_or(""));
                let ready = built.get(index).copied().unwrap_or(false);
                row_for_settle.set_visible(!ready);
                dialog_for_settle.set_response_enabled(
                    "create",
                    ready && taken_for_settle.check(&name_for_settle.text()).is_ok(),
                );
            };
            settle(&combo);
            combo.connect_selected_notify(settle);

            tags.replace(choices.into_iter().map(|c| c.tag).collect());
        });
    });
    check_image();

    let check_after_build = check_image.clone();
    let podman_for_build = podman.clone();
    let build_parent = parent.as_ref().clone();
    let tags_for_build = image_tags.clone();
    let combo_for_build = image_row.clone();
    build_now.connect_clicked(move |_| {
        let recheck = check_after_build.clone();
        // The button sits under the dropdown, so it has to build what the
        // dropdown says. Building Cabin's own image instead would leave the
        // warning showing after a build that appeared to succeed.
        let tag = tags_for_build
            .borrow()
            .get(combo_for_build.selected() as usize)
            .cloned()
            .unwrap_or_else(|| crate::recipes::BASE_TAG.to_string());
        match crate::recipes::all().into_iter().find(|r| r.tag == tag) {
            Some(recipe) => crate::images::present_build_from(
                &build_parent,
                podman_for_build.clone(),
                &recipe.tag,
                recipe.dir,
                move || recheck(),
            ),
            None => crate::images::present_build(
                &build_parent,
                podman_for_build.clone(),
                &tag,
                move || recheck(),
            ),
        }
    });
    let parent_widget = parent.as_ref().clone();
    dialog.connect_response(None, move |dialog, response| {
        if response != "create" {
            return;
        }
        // Create is only enabled for a name that passed the check above.
        let name = name_row.text().trim().to_string();

        let mut config = DesktopConfig::new(&name);
        config.image = image_tags
            .borrow()
            .get(image_row.selected() as usize)
            .cloned()
            .unwrap_or_else(|| default_image.clone());
        // The default is not changed here. It is set on purpose in the image
        // manager, and a desktop made once from a different image should not
        // quietly become the answer for every desktop after it.
        config.memory_mb = Some(memory_row.value() as u64);
        config.graphics = modes
            .get(graphics_row.selected() as usize)
            .cloned()
            .unwrap_or(GraphicsMode::Automatic);

        let podman = podman.clone();
        let nodes = nodes.clone();
        let interfaces = crate::network::interfaces();
        let parent_widget = parent_widget.clone();
        let on_created = on_created.clone();
        let bindable = |port: u16| std::net::TcpListener::bind(("127.0.0.1", port)).is_ok();
        let Some(port) = pick_port(&in_use.ports, bindable) else {
            show_error(
                &parent_widget,
                &format!(
                    "Every port from {} to {} is in use. Delete a desktop you no \
                     longer need, then try again.",
                    PORT_RANGE.start,
                    PORT_RANGE.end - 1
                ),
            );
            return;
        };
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

/// One image a desktop can be made from.
struct Choice {
    /// What the dropdown shows.
    label: String,
    /// The tag the desktop is created with.
    tag: String,
    /// Where the image came from and what it costs, for the row beneath.
    detail: String,
    /// False when the recipe is known but its image is not built. The choice
    /// is still offered, because hiding it makes Cabin's own image vanish
    /// from a fresh installation with nothing to say why.
    built: bool,
}

/// The images a desktop can be made from.
///
/// Every recipe is offered, whether or not its image exists yet, and Cabin's
/// own is offered first and always. A recipe with no image says so and cannot
/// be chosen, which is a reason the user can act on. Untagged images are left
/// out: one cannot be referred to and would not survive the next clean-up.
fn buildable_images(images: &[cabin_podman::Image]) -> Vec<Choice> {
    let recipes = crate::recipes::all();
    let mut choices: Vec<Choice> = Vec::new();

    for recipe in &recipes {
        let image = images
            .iter()
            .filter(|i| i.is_guest && !i.dangling)
            .find(|i| i.tags.contains(&recipe.tag));
        let origin = match recipe.kind {
            crate::recipes::Kind::Shipped => "shipped with Cabin",
            crate::recipes::Kind::Custom if recipe.customised => "your copy",
            crate::recipes::Kind::Custom => "yours",
        };
        choices.push(Choice {
            // The name only. Anything qualifying it goes in the row below,
            // because this string also has to fit a dropdown.
            label: recipe.name.clone(),
            detail: match image {
                Some(image) => format!(
                    "{origin} \u{b7} {} \u{b7} {}",
                    image.human_size(),
                    recipe.tag
                ),
                None => format!("{origin} \u{b7} not built yet \u{b7} {}", recipe.tag),
            },
            tag: recipe.tag.clone(),
            built: image.is_some(),
        });
    }

    // An image with no recipe behind it: one the user built by hand, or one
    // whose recipe they deleted. It still works, so it is still offered.
    for image in images.iter().filter(|i| i.is_guest && !i.dangling) {
        for tag in &image.tags {
            if choices.iter().any(|c| &c.tag == tag) {
                continue;
            }
            choices.push(Choice {
                label: tag.clone(),
                detail: format!("no recipe \u{b7} {} \u{b7} {tag}", image.human_size()),
                tag: tag.clone(),
                built: true,
            });
        }
    }

    // The image Cabin ships comes first. It is the one that works without
    // anything else having been built, so it is the answer whenever the user
    // has no reason to prefer another.
    choices.sort_by(|a, b| {
        let not_base = |c: &Choice| c.tag != crate::recipes::BASE_TAG;
        not_base(a)
            .cmp(&not_base(b))
            .then_with(|| a.label.cmp(&b.label))
    });
    choices
}

/// Suggests a name that is not in use, by adding a number if it has to.
fn first_free_name(base: &str, is_free: impl Fn(&str) -> bool) -> String {
    if is_free(base) {
        return base.to_string();
    }
    for suffix in 2..100 {
        let candidate = format!("{base}-{suffix}");
        if is_free(&candidate) {
            return candidate;
        }
    }
    base.to_string()
}

/// Picks a host port for a new desktop.
///
/// Every port in `taken` is skipped first. Those belong to other desktops,
/// and a stopped desktop holds no socket, so the bind test alone would give
/// its port to the new desktop and the two could never run together.
/// `is_bindable` then catches ports that other software holds.
///
/// The bind test races with anything else that binds in the gap before
/// Podman publishes the port. The window is small, and Podman reports a clear
/// error if it loses that race.
fn pick_port(taken: &[u16], is_bindable: impl Fn(u16) -> bool) -> Option<u16> {
    PORT_RANGE
        .into_iter()
        .find(|port| !taken.contains(port) && is_bindable(*port))
}

/// Generates a per-desktop RDP password.
///
/// The password reaches the guest as an environment variable, so `podman
/// inspect` shows it. That is the same exposure the guest already has. Storing
/// it in the host keyring through the Secret portal is a plan only, and is
/// not done. DESIGN.md section 13.4 records it.
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

    fn image(tag: &str, guest: bool) -> cabin_podman::Image {
        cabin_podman::Image {
            id: format!("id-{tag}"),
            tags: vec![tag.to_string()],
            size_bytes: 1_650_000_000,
            created: 0,
            is_guest: guest,
            dangling: false,
            in_use: false,
        }
    }

    #[test]
    fn cabins_own_image_is_offered_even_when_nothing_is_built() {
        // The one choice that always works has to be visible on a machine
        // with nothing on it, or a fresh installation offers an empty list.
        let choices = buildable_images(&[]);
        let base = choices
            .first()
            .expect("the shipped recipe is always a choice");
        assert_eq!(base.tag, crate::recipes::BASE_TAG);
        assert!(!base.built);
        assert!(base.detail.contains("not built yet"));
    }

    #[test]
    fn cabins_own_image_stays_first_when_others_exist() {
        // It is the answer whenever the user has no reason to prefer another,
        // so it must not be pushed down the list by a name that sorts before
        // it.
        let choices = buildable_images(&[
            image("localhost/cabin-guest-aaa:latest", true),
            image(crate::recipes::BASE_TAG, true),
        ]);
        assert_eq!(choices[0].tag, crate::recipes::BASE_TAG);
        assert!(choices[0].built);
    }

    #[test]
    fn a_recipe_is_offered_once_however_many_images_carry_its_tag() {
        let choices = buildable_images(&[image(crate::recipes::BASE_TAG, true)]);
        let base = choices
            .iter()
            .filter(|c| c.tag == crate::recipes::BASE_TAG)
            .count();
        assert_eq!(base, 1, "the shipped recipe appeared {base} times");
    }

    #[test]
    fn an_image_that_is_not_cabins_is_never_offered() {
        // A desktop made from one would not have a session in it, and the
        // failure would arrive minutes later as a connection that never
        // comes.
        let choices = buildable_images(&[image("docker.io/library/nginx:latest", false)]);
        assert!(choices
            .iter()
            .all(|c| c.tag != "docker.io/library/nginx:latest"));
    }

    #[test]
    fn an_image_whose_recipe_is_gone_is_still_offered() {
        // Deleting a recipe leaves its image alone on purpose, and desktops
        // made from it keep working, so it stays a choice.
        let choices = buildable_images(&[image("localhost/cabin-guest-old:latest", true)]);
        let orphan = choices
            .iter()
            .find(|c| c.tag == "localhost/cabin-guest-old:latest")
            .expect("an image with no recipe should still be a choice");
        assert!(orphan.built);
        assert!(orphan.detail.contains("no recipe"));
    }

    #[test]
    fn every_choice_says_where_it_came_from() {
        // The name alone does not say whether an image is Cabin's or the
        // user's, and the name is all the dropdown has room for.
        for choice in buildable_images(&[image(crate::recipes::BASE_TAG, true)]) {
            assert!(
                !choice.detail.is_empty(),
                "{} has nothing under it",
                choice.label
            );
        }
    }

    fn in_use(desktops: &[&str]) -> InUse {
        InUse {
            desktops: desktops.iter().map(|d| d.to_string()).collect(),
            ..InUse::default()
        }
    }

    fn suggest(in_use: &InUse) -> String {
        first_free_name("fedora", |name| in_use.check(name).is_ok())
    }

    #[test]
    fn suggests_the_plain_name_when_it_is_free() {
        assert_eq!(suggest(&InUse::default()), "fedora");
    }

    #[test]
    fn suggests_a_numbered_name_when_the_plain_one_is_taken() {
        assert_eq!(suggest(&in_use(&["fedora", "fedora-2"])), "fedora-3");
    }

    #[test]
    fn the_suggestion_is_always_a_valid_name() {
        let names: Vec<String> = std::iter::once("fedora".to_string())
            .chain((2..20).map(|n| format!("fedora-{n}")))
            .collect();
        let all = InUse {
            desktops: names,
            ..InUse::default()
        };
        assert!(all.check(&suggest(&all)).is_ok());
    }

    #[test]
    fn a_name_another_container_holds_is_refused() {
        // Podman would refuse the create after the dialogue had closed.
        let in_use = InUse {
            containers: vec!["fedora-toolbox-44".to_string()],
            ..InUse::default()
        };
        let why = in_use.check("fedora-toolbox-44").unwrap_err();
        assert!(why.contains("container"), "{why}");
    }

    #[test]
    fn a_name_with_volumes_left_behind_is_refused() {
        // The new desktop would inherit the old files and certificate.
        for volume in ["cabin-old-home", "cabin-old-state"] {
            let in_use = InUse {
                volumes: vec![volume.to_string()],
                ..InUse::default()
            };
            let why = in_use.check("old").unwrap_err();
            assert!(why.contains("Old files"), "{why}");
        }
    }

    #[test]
    fn the_suggestion_avoids_other_containers() {
        let in_use = InUse {
            containers: vec!["fedora".to_string()],
            ..InUse::default()
        };
        assert_eq!(suggest(&in_use), "fedora-2");
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
    fn the_port_of_a_stopped_desktop_is_skipped() {
        // A stopped desktop holds no socket, so its port binds, and still
        // belongs to it.
        let start = PORT_RANGE.start;
        assert_eq!(pick_port(&[start], |_| true), Some(start + 1));
    }

    #[test]
    fn a_port_something_else_holds_is_skipped() {
        let start = PORT_RANGE.start;
        assert_eq!(pick_port(&[], |p| p != start), Some(start + 1));
    }

    #[test]
    fn no_port_is_picked_when_the_range_is_full() {
        let taken: Vec<u16> = PORT_RANGE.collect();
        assert_eq!(pick_port(&taken, |_| true), None);
        assert_eq!(pick_port(&[], |_| false), None);
    }
}
