//! Draws the Skills panel. (Placeholder until the mockup is approved.)

use eframe::egui;

use super::Skills;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum View {
    #[default]
    Overview,
}

pub fn show(module: &mut Skills, ui: &mut egui::Ui, _now: i64) {
    let _ = module.view;
    ui.label("Skills");
}
