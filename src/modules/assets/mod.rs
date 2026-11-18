//! Assets: everything the logged-in characters own, by location, with
//! search and saved filters. Built from the mockup Kevin approved
//! (<https://claude.ai/artifact/KqyP133XTJegF5Lnx4MsH9>).
//!
//! The pieces:
//!
//! - [`worker`]: fetches assets, names and prices from ESI on a background
//!   thread and keeps a copy in `assets-cache.json`.
//! - [`data`]: builds the location tree and picks what to show.
//! - [`sde`]: item types, groups and stations from `static.sqlite`.
//! - [`config`]: settings and saved filters, in `assets.toml`.
//! - `ui` and `filter_ui`: draw the panel and the filter editor.
//!
//! The tree is rebuilt only when new data arrives, and what is shown only
//! when the search, a filter, the sort or the route changes. A frame just
//! draws the rows that are on screen.

pub mod config;
pub mod data;
mod filter_ui;
pub mod sde;
mod ui;
pub mod worker;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui;

use self::config::{AssetsSettings, PriceSource, RouteFrom, Who};
use self::data::{CharacterAssets, LocationKey, Lookup, OpenState, PlaceInfo, Row, Tree, View};
use self::sde::{Catalog, TypeInfo};
use self::worker::{Cache, Command, Event, Prices, Worker};
use super::shared::{PilotSystems, RosterCharacter, SharedRoster, roster_if_changed};
use super::{Module, ModuleContext, SharedStaticData};
use crate::auth::{TokenStore, now_unix};
use crate::static_data::StaticData;
use crate::storage::Saver;

/// "Refresh" works at most this often.
const REFRESH_COOLDOWN_SECS: i64 = 60;
/// While the tab is open, ask the worker this often whether something is
/// due (it only fetches what ESI has newer data for).
const FETCH_CHECK_SECS: i64 = 60;
/// Ask ESI where the route character is at most this often.
const LOCATE_EVERY_SECS: i64 = 5 * 60;

pub struct Assets {
    data_dir: PathBuf,
    saver: Saver,
    settings: AssetsSettings,
    worker: Worker,
    roster: SharedRoster,
    roster_version: u64,
    /// The logged-in characters, in the Characters tab's order.
    characters: Vec<RosterCharacter>,
    static_data: Option<Arc<StaticData>>,
    pilot_systems: PilotSystems,
    /// False until the worker has read the saved cache.
    loaded: bool,

    // What the worker sent.
    assets: BTreeMap<u64, CharacterAssets>,
    places: HashMap<i64, PlaceInfo>,
    types: HashMap<i64, TypeInfo>,
    prices: Option<Prices>,
    catalog: Catalog,
    /// Characters being fetched right now.
    fetching: HashSet<u64>,
    prices_busy: bool,
    character_problems: HashMap<u64, String>,
    problems: Vec<String>,
    /// Character -> (system, when ESI said so).
    located: HashMap<u64, (i64, i64)>,
    locate_asked: HashMap<u64, i64>,

    // Built from the above.
    tree: Tree,
    view: View,
    rows: Vec<Row>,
    tree_dirty: bool,
    view_dirty: bool,
    rows_dirty: bool,
    /// The system jumps are counted from, and the jumps to every system.
    route_system: Option<i64>,
    jumps: HashMap<i64, u32>,

    // What the user opened and typed (not saved).
    open_locations: HashSet<LocationKey>,
    open_nodes: HashSet<i64>,
    search: String,
    ui: ui::UiState,
    editor: Option<filter_ui::Editor>,
    last_fetch_check: i64,
    last_refresh: Option<i64>,
}

impl Assets {
    pub fn new(
        egui_ctx: &egui::Context,
        data_dir: &Path,
        tokens: Arc<TokenStore>,
        roster: SharedRoster,
        static_data: &SharedStaticData,
        pilot_systems: PilotSystems,
    ) -> Assets {
        let (settings, problem) = AssetsSettings::load(data_dir);
        let worker = Worker::start(
            egui_ctx,
            tokens,
            Cache::file_path(data_dir),
            settings.price_source,
        );
        let mut problems: Vec<String> = problem.into_iter().collect();
        if let Some(problem) = &static_data.problem {
            problems.push(problem.clone());
        }
        let mut module = Assets {
            data_dir: data_dir.to_path_buf(),
            saver: Saver::start(),
            settings,
            worker,
            roster,
            roster_version: 0,
            characters: Vec::new(),
            static_data: static_data.data.clone(),
            pilot_systems,
            loaded: false,
            assets: BTreeMap::new(),
            places: HashMap::new(),
            types: HashMap::new(),
            prices: None,
            catalog: Catalog::default(),
            fetching: HashSet::new(),
            prices_busy: false,
            character_problems: HashMap::new(),
            problems,
            located: HashMap::new(),
            locate_asked: HashMap::new(),
            tree: Tree::default(),
            view: View::default(),
            rows: Vec::new(),
            tree_dirty: false,
            view_dirty: false,
            rows_dirty: false,
            route_system: None,
            jumps: HashMap::new(),
            open_locations: HashSet::new(),
            open_nodes: HashSet::new(),
            search: String::new(),
            ui: ui::UiState::default(),
            editor: None,
            last_fetch_check: 0,
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
        let ids: Vec<u64> = self.characters.iter().map(|c| c.id).collect();
        self.assets.retain(|id, _| ids.contains(id));
        if crate::auth::is_configured() {
            self.worker.send(Command::SetCharacters(ids));
        }
        self.tree_dirty = true;
    }

    fn save_settings(&mut self) {
        if let Ok(text) = toml::to_string(&self.settings) {
            self.saver
                .save(AssetsSettings::file_path(&self.data_dir), text);
        }
    }

    /// Call after changing a setting that changes what is shown.
    fn settings_changed(&mut self) {
        self.save_settings();
        self.view_dirty = true;
    }

    pub fn character_name(&self, id: u64) -> String {
        self.characters
            .iter()
            .find(|c| c.id == id)
            .map_or_else(|| format!("Character {id}"), |c| c.name.clone())
    }

    pub fn type_name(&self, type_id: i64) -> String {
        self.types
            .get(&type_id)
            .map_or_else(|| format!("Type {type_id}"), |t| t.name.clone())
    }

    /// The characters whose assets are shown.
    fn owners(&self) -> HashSet<u64> {
        self.characters
            .iter()
            .filter(|c| match &self.settings.who {
                Who::All => true,
                Who::Tag(tag) => c.tags.contains(tag),
                Who::Character(id) => c.id == *id,
            })
            .map(|c| c.id)
            .collect()
    }

    /// "Refresh": ask ESI for everything now.
    fn refresh_now(&mut self) {
        let now = now_unix();
        if self.can_refresh(now) {
            self.last_refresh = Some(now);
            self.worker.send(Command::RefreshNow);
        }
    }

    fn can_refresh(&self, now: i64) -> bool {
        self.last_refresh
            .is_none_or(|at| now - at >= REFRESH_COOLDOWN_SECS)
    }

    fn set_price_source(&mut self, source: PriceSource) {
        if self.settings.price_source != source {
            self.settings.price_source = source;
            self.save_settings();
            self.worker.send(Command::SetPriceSource(source));
        }
    }

    /// The character the route starts from, if the route follows one.
    fn route_character(&self) -> Option<u64> {
        match self.settings.route_from {
            Some(RouteFrom::Character(id)) => Some(id),
            Some(RouteFrom::System(_)) => None,
            None => self.characters.first().map(|c| c.id),
        }
    }

    /// Works out where the route starts and, if that changed, the jumps
    /// to every system. The Local chat log (via Intel) knows first where a
    /// pilot is; ESI is asked when it doesn't.
    fn update_route(&mut self, now: i64) {
        let system = match self.settings.route_from {
            Some(RouteFrom::System(id)) => Some(id),
            _ => self.route_character().and_then(|id| {
                let name = self.character_name(id);
                let from_log = self
                    .pilot_systems
                    .lock()
                    .ok()
                    .and_then(|map| map.get(&name).map(|p| (p.system_id, p.since_unix)));
                let from_esi = self.located.get(&id).copied();
                let newest = match (from_log, from_esi) {
                    (Some(a), Some(b)) => Some(if a.1 >= b.1 { a.0 } else { b.0 }),
                    (a, b) => a.or(b).map(|x| x.0),
                };
                let asked = self.locate_asked.get(&id).copied().unwrap_or(0);
                if from_log.is_none() && now - asked >= LOCATE_EVERY_SECS {
                    self.locate_asked.insert(id, now);
                    self.worker.send(Command::Locate(id));
                }
                newest
            }),
        };
        if system != self.route_system {
            self.route_system = system;
            self.jumps = match (system, &self.static_data) {
                (Some(start), Some(static_data)) => {
                    static_data.universe.jumps_within(&[start], u32::MAX)
                }
                _ => HashMap::new(),
            };
            self.view_dirty = true;
        }
    }

    /// Rebuilds the tree, the view and the rows as far as needed.
    fn rebuild(&mut self) {
        if self.tree_dirty {
            let empty = HashMap::new();
            let prices = self.prices.as_ref().map_or(&empty, |p| &p.by_type);
            let lookup = Lookup {
                types: &self.types,
                prices,
                places: &self.places,
            };
            self.tree = data::build(self.assets.iter().map(|(id, a)| (*id, a)), &lookup);
            self.tree_dirty = false;
            self.view_dirty = true;
        }
        if self.view_dirty {
            let owners = self.owners();
            let options = data::ViewOptions {
                owners: &owners,
                filters: self.settings.active_filters(),
                search: &self.search,
                sort: self.settings.sort,
                reversed: self.settings.reversed,
                jumps: &self.jumps,
            };
            let universe = self.static_data.as_ref().map(|s| &s.universe);
            self.view = data::view(&self.tree, &options, &self.types, universe);
            self.view_dirty = false;
            self.rows_dirty = true;
        }
        if self.rows_dirty {
            let names: HashMap<u64, String> = self
                .characters
                .iter()
                .map(|c| (c.id, c.name.clone()))
                .collect();
            let owner_name = |id: u64| names.get(&id).cloned().unwrap_or_default();
            let type_name = |id: i64| self.type_name(id);
            let open = OpenState {
                locations: &self.open_locations,
                nodes: &self.open_nodes,
            };
            self.rows = data::rows(&self.tree, &self.view, &open, &owner_name, &type_name);
            self.rows_dirty = false;
        }
    }

    fn toggle_location(&mut self, key: LocationKey) {
        if !self.open_locations.remove(&key) {
            self.open_locations.insert(key);
        }
        self.rows_dirty = true;
    }

    fn toggle_node(&mut self, item_id: i64) {
        if !self.open_nodes.remove(&item_id) {
            self.open_nodes.insert(item_id);
        }
        self.rows_dirty = true;
    }

    fn apply(&mut self, event: Event) {
        match event {
            Event::Loaded { cache, catalog } => {
                self.loaded = true;
                // Anything the worker already sent wins over the saved copy.
                // The roster may not be known yet (Characters starts after
                // this module), so keep every character here; only the
                // logged-in ones are shown, and the worker drops the rest.
                for (id, assets) in cache.characters {
                    self.assets.entry(id).or_insert(assets);
                }
                for (id, place) in cache.places {
                    self.places.entry(id).or_insert(place);
                }
                if self.prices.is_none() {
                    self.prices = cache.prices;
                }
                self.catalog = *catalog;
                self.tree_dirty = true;
            }
            Event::Started(id) => {
                self.fetching.insert(id);
            }
            Event::Assets {
                character_id,
                assets,
            } => {
                self.fetching.remove(&character_id);
                self.assets.insert(character_id, *assets);
                self.tree_dirty = true;
            }
            Event::Types(types) => {
                self.types.extend(types);
                self.tree_dirty = true;
            }
            Event::Places(places) => {
                self.places.extend(places);
                self.tree_dirty = true;
            }
            Event::Prices(prices) => {
                self.prices = Some(*prices);
                self.tree_dirty = true;
            }
            Event::PricesBusy(busy) => self.prices_busy = busy,
            Event::Location {
                character_id,
                system_id,
            } => {
                self.located.insert(character_id, (system_id, now_unix()));
            }
            Event::CharacterProblem {
                character_id,
                problem,
            } => {
                self.fetching.remove(&character_id);
                match problem {
                    Some(text) => {
                        self.character_problems.insert(character_id, text);
                    }
                    None => {
                        self.character_problems.remove(&character_id);
                    }
                }
            }
            Event::Problem(text) => {
                if !self.problems.contains(&text) {
                    self.problems.push(text);
                }
            }
        }
    }
}

impl Module for Assets {
    fn id(&self) -> &'static str {
        "assets"
    }

    fn title(&self) -> &'static str {
        "Assets"
    }

    fn update(&mut self, _ctx: &ModuleContext) {
        self.read_roster();
        // `try_recv` returns at once when nothing is waiting.
        while let Ok(event) = self.worker.events.try_recv() {
            self.apply(event);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _ctx: &ModuleContext) {
        let now = now_unix();
        // Fetching only happens while the tab is open.
        if crate::auth::is_configured() && now - self.last_fetch_check >= FETCH_CHECK_SECS {
            self.last_fetch_check = now;
            self.worker.send(Command::FetchDue);
        }
        self.update_route(now);
        self.rebuild();
        ui::show(self, ui, now);
        filter_ui::show(self, ui.ctx());
        // "x min ago" and the next fetch check: once a minute while the tab
        // is visible, nothing while it is hidden.
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs(FETCH_CHECK_SECS as u64));
    }

    fn on_exit(&mut self) {
        self.saver.finish();
    }
}
