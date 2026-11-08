//! A placeholder module that shows how modules work. It will go away once the
//! first real modules (Mining Supervisor, Intel Supervisor) are in.

use eframe::egui;

use super::{Module, ModuleContext};

pub struct Welcome;

impl Module for Welcome {
    fn id(&self) -> &'static str {
        "welcome"
    }

    fn title(&self) -> &'static str {
        "Welcome"
    }

    fn ui(&mut self, ui: &mut egui::Ui, ctx: &ModuleContext) {
        ui.heading("Athena");
        ui.label("Modules show up here as tabs.");
        ui.label("Drag a tab by its title to move it, dock it next to another one, or pull it out into its own window.");
        ui.label("Closed tabs can be opened again from the View menu.");
        ui.add_space(8.0);
        ui.label(format!(
            "EVE log folder: {}",
            ctx.settings.log_folder.display()
        ));
    }
}
