//! CIC (combat information center): combat views made from the game logs.
//! It has submodules, shown as tabs inside the CIC panel. The first one is
//! **Logi**: one card per logi pilot, with the reps and cap they send
//! (arrows above the card) and get (arrows below), damage taken, ECM,
//! point/scram and the cap chain.
//!
//! The pieces:
//!
//! - [`combatlog`]: reads `(combat)` lines.
//! - [`watcher`]: follows the game logs of the shown characters on a
//!   background thread.
//! - [`logi`]: what each pilot's card shows, over time.
//! - [`config`]: Settings, saved in `cic.toml`.
//! - `icons`: the ISIS icons (CCP art, from the EVE University wiki).
//! - `ui`: draws the panel.
//!
//! Nothing is saved besides the settings: the view is about right now.

pub mod combatlog;
pub mod config;
mod icons;
pub mod logi;
mod ui;
pub mod watcher;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use eframe::egui;

use self::combatlog::CombatEvent;
use self::config::CicSettings;
use self::logi::PilotState;
use self::watcher::{CombatLogWatcher, WatchEvent};
use super::shared::{RosterCharacter, SharedRoster, roster_if_changed};
use super::{Module, ModuleContext, SharedStaticData};
use crate::settings::Settings;
use crate::static_data::StaticData;
use crate::storage::Saver;

/// The submodules, one tab each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Submodule {
    #[default]
    Logi,
}

/// Live cards or their settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum View {
    #[default]
    Live,
    Settings,
}

pub struct Cic {
    data_dir: PathBuf,
    log_folder: PathBuf,
    saver: Saver,
    settings: CicSettings,
    watcher: CombatLogWatcher,
    egui_ctx: egui::Context,
    static_data: Option<Arc<StaticData>>,
    roster: SharedRoster,
    roster_version: u64,
    /// The module's clock starts here; see [`Cic::now`].
    started: Instant,
    /// Characters with a recent game log.
    in_logs: BTreeSet<String>,
    /// Card state by character name.
    pilots: HashMap<String, PilotState>,
    problems: Vec<String>,
    log_problem: Option<String>,
    submodule: Submodule,
    view: View,
    icons: Option<icons::Icons>,
}

impl Cic {
    pub fn new(
        egui_ctx: &egui::Context,
        app_settings: &Settings,
        data_dir: &Path,
        static_data: &SharedStaticData,
        roster: SharedRoster,
    ) -> Cic {
        let (settings, problem) = CicSettings::load(data_dir);
        let watcher = CombatLogWatcher::start(
            &app_settings.log_folder,
            shown_names(&settings),
            egui_ctx.clone(),
        );
        let mut module = Cic {
            data_dir: data_dir.to_path_buf(),
            log_folder: app_settings.log_folder.clone(),
            saver: Saver::start(),
            settings,
            watcher,
            egui_ctx: egui_ctx.clone(),
            static_data: static_data.data.clone(),
            roster,
            roster_version: 0,
            started: Instant::now(),
            in_logs: BTreeSet::new(),
            pilots: HashMap::new(),
            problems: problem.into_iter().collect(),
            log_problem: None,
            submodule: Submodule::default(),
            view: View::default(),
            icons: None,
        };
        module.read_roster();
        module
    }

    /// Seconds since the module started: the clock all card timing uses.
    /// Events get the time they arrive; they are read within a fraction of
    /// a second of EVE writing them.
    fn now(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    /// Adds the logged-in characters (Characters tab) to the Settings list.
    fn read_roster(&mut self) {
        let Some(roster) = roster_if_changed(&self.roster, self.roster_version) else {
            return;
        };
        self.roster_version = roster.version;
        let names: Vec<String> = roster
            .characters
            .iter()
            .map(|c: &RosterCharacter| c.name.clone())
            .collect();
        if self.settings.add_characters(&names) {
            self.save_settings();
        }
    }

    fn save_settings(&mut self) {
        if let Ok(text) = toml::to_string(&self.settings) {
            self.saver
                .save(CicSettings::file_path(&self.data_dir), text);
        }
    }

    /// Settings changed in the UI: save, and tell the watcher whose logs
    /// to read.
    fn settings_edited(&mut self) {
        self.settings.clamp();
        self.watcher.follow(shown_names(&self.settings));
        self.save_settings();
        self.egui_ctx.request_repaint();
    }

    /// The cap chain modules changed: the arrows of known links are stale.
    fn chain_modules_edited(&mut self) {
        for pilot in self.pilots.values_mut() {
            pilot.clear_links();
        }
        self.settings_edited();
    }

    fn handle(&mut self, character: String, events: Vec<CombatEvent>) {
        let Some(ship) = self.settings.character(&character).map(|c| c.ship) else {
            return;
        };
        let now = self.now();
        let pilot = self.pilots.entry(character).or_default();
        for event in events {
            let who = match &event {
                CombatEvent::Rep { other, .. } => display_name(other, self.static_data.as_deref()),
                _ => String::new(),
            };
            pilot.apply(event, who, now, ship, &self.settings);
        }
    }
}

/// The name shown for the other side of a rep: the pilot, or for a
/// structure its type ("Metenox Moon Drill"), as its own name means little.
fn display_name(other: &combatlog::Other, static_data: Option<&StaticData>) -> String {
    let structure = match static_data {
        Some(data) => data.is_structure_type(&other.ship),
        // Without the SDE: structure names look like "System - Name".
        None => other.name.contains(" - "),
    };
    if structure && !other.ship.is_empty() {
        other.ship.clone()
    } else {
        other.name.clone()
    }
}

fn shown_names(settings: &CicSettings) -> HashSet<String> {
    settings.shown().map(|c| c.name.clone()).collect()
}

impl Module for Cic {
    fn id(&self) -> &'static str {
        "cic"
    }

    fn title(&self) -> &'static str {
        "CIC"
    }

    fn update(&mut self, _ctx: &ModuleContext) {
        self.read_roster();
        // `try_recv` never waits: it takes what the watcher has sent so far.
        while let Ok(event) = self.watcher.events.try_recv() {
            match event {
                WatchEvent::Events { character, events } => self.handle(character, events),
                WatchEvent::Characters(names) => {
                    if self.settings.add_characters(&names) {
                        self.save_settings();
                    }
                    self.in_logs = names.into_iter().collect();
                    self.log_problem = None;
                }
                WatchEvent::Problem(text) => self.log_problem = Some(text),
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _ctx: &ModuleContext) {
        let now = self.now();
        ui::show(self, ui, now);
    }

    fn settings_changed(&mut self, settings: &Settings) {
        if settings.log_folder == self.log_folder {
            return;
        }
        self.log_folder = settings.log_folder.clone();
        self.in_logs.clear();
        self.watcher = CombatLogWatcher::start(
            &settings.log_folder,
            shown_names(&self.settings),
            self.egui_ctx.clone(),
        );
    }

    fn on_exit(&mut self) {
        self.saver.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::combatlog::Other;
    use super::*;

    #[test]
    fn structures_show_their_type() {
        let data = crate::static_data::tests::sample();
        let structure = Other {
            name: "Test-1 - Drill".into(),
            ship: "Metenox Moon Drill".into(),
        };
        let pilot = Other {
            name: "Mira Tal".into(),
            ship: "Basilisk".into(),
        };
        assert_eq!(display_name(&structure, Some(&data)), "Metenox Moon Drill");
        assert_eq!(display_name(&pilot, Some(&data)), "Mira Tal");
        assert_eq!(display_name(&structure, None), "Metenox Moon Drill");
        assert_eq!(display_name(&pilot, None), "Mira Tal");
    }
}
