//! Application start-up.

use adw::prelude::*;

pub fn activate(app: &adw::Application) {
    let window = crate::window::build(app);
    window.present();
}
