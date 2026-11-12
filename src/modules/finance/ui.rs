//! Draws the Finance panel. Placeholder until the mockup is approved.

use eframe::egui;

use super::Finance;

pub fn show(module: &mut Finance, ui: &mut egui::Ui) {
    ui.label(format!(
        "{} characters, {} journal entries saved.",
        module.characters.len(),
        module.journal.len()
    ));
}
