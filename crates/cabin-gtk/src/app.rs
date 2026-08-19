//! Application start-up.

use adw::prelude::*;

pub fn activate(app: &adw::Application) {
    // An earlier version let one copy of the guest recipe take the shipped
    // recipe's own tag, which hid the shipped recipe. Move any such copy into
    // the named recipes before anything reads the list, so the user sees both.
    if let Some(name) = crate::recipes::migrate_legacy_copy() {
        tracing::info!("your guest recipe copy is now the recipe named {name}");
    }

    let window = crate::window::build(app);
    window.present();
}
