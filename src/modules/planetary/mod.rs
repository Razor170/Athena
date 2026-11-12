//! Planetary: per character, which planets are active and which need
//! attention, and how full each planet's launchpads are.
//!
//! The pieces:
//!
//! - [`worker`]: fetches the colonies from ESI on a background thread and
//!   keeps a copy in `planetary-cache.json`.
//! - [`data`]: what ESI sends, and the structs Athena keeps.
//! - [`status`]: red / yellow / green and the numbers each planet shows.
//! - [`config`]: thresholds and filters, saved in `planetary.toml`.
//! - `ui`: draws the panel.
//!
//! This file holds the [`Planetary`] module: it keeps what the worker sent
//! and works out each planet's status when the data changes, or once a
//! minute while the tab is open (extractor end times move with the clock).

pub mod config;
pub mod data;
pub mod status;
mod ui;
pub mod worker;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui;

use self::config::PlanetarySettings;
use self::data::Colony;
use self::status::Assessment;
use self::worker::{Cache, Command, Event, Worker};
use super::shared::{RosterCharacter, SharedRoster, roster_if_changed};
use super::{Module, ModuleContext, SharedStaticData};
use crate::auth::TokenStore;
use crate::static_data::StaticData;
use crate::storage::Saver;

/// "Refresh" works at most this often.
const REFRESH_COOLDOWN_SECS: i64 = 60;

pub struct Planetary {
    data_dir: PathBuf,
    saver: Saver,
    settings: PlanetarySettings,
    worker: Worker,
    roster: SharedRoster,
    roster_version: u64,
    /// The logged-in characters, in the Characters tab's order.
    characters: Vec<RosterCharacter>,
    static_data: Option<Arc<StaticData>>,
    /// False until the worker has read the saved cache.
    loaded: bool,
    /// Character id -> their colonies.
    colonies: BTreeMap<u64, Vec<Colony>>,
    planet_names: BTreeMap<i64, String>,
    max_planets: BTreeMap<u64, u32>,
    /// When the colony lists were last fetched.
    checked: Option<i64>,
    /// Problems per character, shown on their card.
    character_problems: HashMap<u64, String>,
    /// Problems to show on top of the panel.
    problems: Vec<String>,
    /// Planet id -> its status, worked out by `assess_all`.
    assessed: HashMap<i64, Assessment>,
    /// The (data version, minute) `assessed` was worked out for.
    assessed_for: Option<(u64, i64)>,
    /// Goes up whenever the data or a threshold changes.
    data_version: u64,
    /// Planets whose details are open.
    open: HashSet<i64>,
    view: ui::View,
    last_refresh: Option<i64>,
}

impl Planetary {
    pub fn new(
        egui_ctx: &egui::Context,
        data_dir: &Path,
        tokens: Arc<TokenStore>,
        roster: SharedRoster,
        static_data: &SharedStaticData,
    ) -> Planetary {
        let (settings, problem) = PlanetarySettings::load(data_dir);
        let worker = Worker::start(egui_ctx, tokens, Cache::file_path(data_dir));
        let mut problems: Vec<String> = problem.into_iter().collect();
        if let Some(problem) = &static_data.problem {
            problems.push(problem.clone());
        }
        let mut module = Planetary {
            data_dir: data_dir.to_path_buf(),
            saver: Saver::start(),
            settings,
            worker,
            roster,
            roster_version: 0,
            characters: Vec::new(),
            static_data: static_data.data.clone(),
            loaded: false,
            colonies: BTreeMap::new(),
            planet_names: BTreeMap::new(),
            max_planets: BTreeMap::new(),
            checked: None,
            character_problems: HashMap::new(),
            problems,
            assessed: HashMap::new(),
            assessed_for: None,
            data_version: 0,
            open: HashSet::new(),
            view: ui::View::default(),
            last_refresh: None,
        };
        module.read_roster();
        module
    }

    /// Takes the newest character list from the Characters module, and
    /// tells the worker if it changed.
    fn read_roster(&mut self) {
        let Some(roster) = roster_if_changed(&self.roster, self.roster_version) else {
            return;
        };
        self.roster_version = roster.version;
        self.characters = roster.characters;
        if crate::auth::is_configured() {
            let ids = self.characters.iter().map(|c| c.id).collect();
            self.worker.send(Command::SetCharacters(ids));
        }
        self.data_version += 1;
    }

    fn save_settings(&mut self) {
        if let Ok(text) = toml::to_string(&self.settings) {
            self.saver
                .save(PlanetarySettings::file_path(&self.data_dir), text);
        }
        self.data_version += 1;
    }

    /// "Refresh": ask ESI for every colony list now.
    pub fn refresh_now(&mut self) {
        let now = crate::auth::now_unix();
        if self.can_refresh(now) {
            self.last_refresh = Some(now);
            self.worker.send(Command::RefreshNow);
        }
    }

    pub fn can_refresh(&self, now: i64) -> bool {
        self.last_refresh
            .is_none_or(|at| now - at >= REFRESH_COOLDOWN_SECS)
    }

    /// Works out every planet's status, unless that was done for the same
    /// data within this minute.
    fn assess_all(&mut self, now: i64) {
        let key = (self.data_version, now.div_euclid(60));
        if self.assessed_for == Some(key) {
            return;
        }
        self.assessed_for = Some(key);
        self.assessed.clear();
        let Some(data) = self.static_data.as_deref() else {
            return;
        };
        for colony in self.colonies.values().flatten() {
            self.assessed.insert(
                colony.planet_id,
                status::assess(colony, data, &self.settings.thresholds, now),
            );
        }
    }

    /// "Amamake II", or the system name until ESI sent the planet's name.
    pub fn planet_name(&self, colony: &Colony) -> String {
        if let Some(name) = self.planet_names.get(&colony.planet_id) {
            return name.clone();
        }
        let system = self
            .static_data
            .as_deref()
            .and_then(|data| data.universe.system(colony.system_id))
            .map(|s| s.name.clone())
            .unwrap_or_else(|| format!("System {}", colony.system_id));
        format!("{system} (planet)")
    }

    fn apply(&mut self, event: Event) {
        match event {
            Event::Loaded(cache) => {
                self.loaded = true;
                let cache = *cache;
                // Anything the worker already sent wins over the saved copy.
                for (id, list) in cache.colonies {
                    self.colonies.entry(id).or_insert(list);
                }
                for (id, name) in cache.planet_names {
                    self.planet_names.entry(id).or_insert(name);
                }
                for (id, max) in cache.max_planets {
                    self.max_planets.entry(id).or_insert(max);
                }
            }
            Event::Colonies {
                character_id,
                colonies,
            } => {
                self.colonies.insert(character_id, colonies);
            }
            Event::PlanetNames(names) => self.planet_names.extend(names),
            Event::MaxPlanets { character_id, max } => {
                self.max_planets.insert(character_id, max);
            }
            Event::Checked(at) => self.checked = Some(at),
            Event::CharacterProblem {
                character_id,
                problem,
            } => match problem {
                Some(text) => {
                    self.character_problems.insert(character_id, text);
                }
                None => {
                    self.character_problems.remove(&character_id);
                }
            },
            Event::Problem(text) => self.problems.push(text),
        }
        self.data_version += 1;
    }
}

impl Module for Planetary {
    fn id(&self) -> &'static str {
        "planetary"
    }

    fn title(&self) -> &'static str {
        "Planetary"
    }

    fn update(&mut self, _ctx: &ModuleContext) {
        self.read_roster();
        // `try_recv` returns at once when nothing is waiting.
        while let Ok(event) = self.worker.events.try_recv() {
            self.apply(event);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _ctx: &ModuleContext) {
        let now = crate::auth::now_unix();
        self.assess_all(now);
        ui::show(self, ui, now);
        // "Ends in" and "viewed x ago" move with the clock: redraw once a
        // minute while the tab is visible, at the start of the next minute.
        let wait = 60 - now.rem_euclid(60) + 1;
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs(wait as u64));
    }

    fn on_exit(&mut self) {
        self.saver.finish();
    }
}
