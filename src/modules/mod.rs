//! Dashboard modules.
//!
//! Each module lives in its own folder under `src/modules/` and implements the
//! [`Module`] trait. To add a module:
//!
//! 1. Create `src/modules/<name>/mod.rs` with a struct that implements `Module`.
//! 2. Add `pub mod <name>;` below.
//! 3. Add one line for it in [`create_all`].
//!
//! The app then shows it as a tab, gives it a toggle button in the top bar,
//! and saves where the user docked it.

use std::path::Path;
use std::sync::Arc;

use eframe::egui;

use crate::overlay::mining_bars::{BarsOverlay, OverlayPlace};
use crate::settings::Settings;
use crate::static_data::StaticData;

pub mod characters;
pub mod intel;
pub mod mining;
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

    /// The name shown on the tab and on its toggle button.
    fn title(&self) -> &'static str;

    /// Called once per frame for every module, even when its tab is hidden or
    /// the window is minimized (as long as someone called `request_repaint`).
    /// Use it to drain channels from background work. Keep it cheap.
    fn update(&mut self, _ctx: &ModuleContext) {}

    /// Draws the module's panel. Only called while its tab is visible.
    fn ui(&mut self, ui: &mut egui::Ui, ctx: &ModuleContext);

    /// The hold bars overlay to show, or `None` while the module doesn't
    /// want one. Called before every frame, so keep it cheap; the overlay
    /// only redraws when the result changes.
    fn bars_overlay(&self) -> Option<BarsOverlay> {
        None
    }

    /// Called when the user moved or resized the bars overlay, so the
    /// module can save where it is.
    fn bars_overlay_moved(&mut self, _place: OverlayPlace) {}

    /// Called after the user changed a setting, with the new settings.
    /// For example, a log watcher restarts here when the log folder changed.
    fn settings_changed(&mut self, _settings: &Settings) {}

    /// Called once when Athena closes. Save anything that isn't saved yet
    /// and wait for it to be written.
    fn on_exit(&mut self) {}
}

/// Creates every module, in the order their toggle buttons appear.
///
/// `egui_ctx` can be cloned and handed to background threads so they can call
/// `request_repaint()` when they have new data. `settings` are the settings at
/// startup, for example the log folder a watcher should start on. `data_dir`
/// is the folder where modules keep their own files.
///
/// This runs once before the window shows, so reading a module's small
/// settings files here is fine. Anything slower goes to a background thread.
pub fn create_all(
    egui_ctx: &egui::Context,
    settings: &Settings,
    data_dir: &Path,
) -> Vec<Box<dyn Module>> {
    let static_data = SharedStaticData::load();
    // ESI access tokens of the logged-in characters, shared by every module
    // that calls ESI for a character (see `crate::auth`).
    let tokens = Arc::new(crate::auth::TokenStore::new());
    vec![
        Box::new(mining::MiningSupervisor::new(
            egui_ctx,
            settings,
            data_dir,
            &static_data,
        )),
        Box::new(intel::IntelSupervisor::new(
            egui_ctx,
            settings,
            data_dir,
            &static_data,
        )),
        Box::new(characters::Characters::new(egui_ctx, data_dir, tokens)),
        Box::new(welcome::Welcome),
        // Add new modules here, one line each.
    ]
}

/// `static.sqlite`, loaded once and shared by the modules that need it.
pub struct SharedStaticData {
    /// `Arc` ("atomically reference counted") lets several modules hold the
    /// same data without copying it; it is freed when the last one drops it.
    /// `None` if the file is missing or broken.
    pub data: Option<Arc<StaticData>>,
    /// Why `data` is `None`, to show in the modules.
    pub problem: Option<String>,
}

impl SharedStaticData {
    /// Reads the file. Runs once at startup, before the window shows.
    fn load() -> SharedStaticData {
        match StaticData::find_file() {
            Some(path) => match StaticData::load(&path) {
                Ok(data) => SharedStaticData {
                    data: Some(Arc::new(data)),
                    problem: None,
                },
                Err(e) => SharedStaticData {
                    data: None,
                    problem: Some(format!("Static game data could not be read: {e:#}")),
                },
            },
            None => SharedStaticData {
                data: None,
                problem: Some(format!(
                    "{} is missing next to Athena.exe, so ore data and the map are unknown.",
                    crate::static_data::FILE_NAME
                )),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_ids_are_unique() {
        let data_dir = std::env::temp_dir().join(format!("athena-ids-{}", std::process::id()));
        std::fs::create_dir_all(&data_dir).unwrap();
        let modules = create_all(&egui::Context::default(), &Settings::default(), &data_dir);
        let mut ids: Vec<&str> = modules.iter().map(|m| m.id()).collect();
        let count = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), count, "two modules share an id");
        drop(modules);
        let _ = std::fs::remove_dir_all(&data_dir);
    }
}
