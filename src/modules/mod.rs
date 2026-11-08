//! Dashboard modules.
//!
//! Each module lives in its own folder under `src/modules/` and implements the
//! [`Module`] trait. To add a module:
//!
//! 1. Create `src/modules/<name>/mod.rs` with a struct that implements `Module`.
//! 2. Add `pub mod <name>;` below.
//! 3. Add one line for it in [`create_all`].
//!
//! The app then shows it as a tab, lists it in the View menu, and saves where
//! the user docked it.

use eframe::egui;

use crate::settings::Settings;

pub mod welcome;

/// What a module may read from the rest of the app while it runs.
pub struct ModuleContext<'a> {
    pub settings: &'a Settings,
}

/// The shared interface every dashboard module implements.
///
/// Performance rules (see CLAUDE.md):
/// - Never do file or network I/O in `update` or `ui`. Do it on a background
///   thread or task, send the results over a channel, and call
///   `egui::Context::request_repaint()` from that thread when new data arrives.
/// - Never call `request_repaint` just to animate or poll. Athena only redraws
///   when something changed.
pub trait Module {
    /// A stable id, saved in `layout.toml`. Don't change it once released,
    /// or users lose where they docked the module.
    fn id(&self) -> &'static str;

    /// The name shown on the tab and in the View menu.
    fn title(&self) -> &'static str;

    /// Called once per frame for every module, even when its tab is hidden or
    /// the window is minimized (as long as someone called `request_repaint`).
    /// Use it to drain channels from background work. Keep it cheap.
    fn update(&mut self, _ctx: &ModuleContext) {}

    /// Draws the module's panel. Only called while its tab is visible.
    fn ui(&mut self, ui: &mut egui::Ui, ctx: &ModuleContext);

    /// Called after the user changed a setting, with the new settings.
    /// For example, a log watcher restarts here when the log folder changed.
    fn settings_changed(&mut self, _settings: &Settings) {}
}

/// Creates every module, in the order they appear in the View menu.
///
/// `egui_ctx` can be cloned and handed to background threads so they can call
/// `request_repaint()` when they have new data. `settings` are the settings at
/// startup, for example the log folder a watcher should start on.
pub fn create_all(egui_ctx: &egui::Context, settings: &Settings) -> Vec<Box<dyn Module>> {
    // Unused until a module needs them; this keeps clippy quiet.
    let _ = (egui_ctx, settings);
    vec![
        Box::new(welcome::Welcome),
        // Add new modules here, one line each.
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_ids_are_unique() {
        let modules = create_all(&egui::Context::default(), &Settings::default());
        let mut ids: Vec<&str> = modules.iter().map(|m| m.id()).collect();
        let count = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), count, "two modules share an id");
    }
}
