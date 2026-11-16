//! Fittings: tools for ship fits. It has submodules, shown as tabs inside
//! the Fittings panel (like CIC). The first one is **Compare**: paste the
//! fit you have (Is) and the fit you want (Should) in EFT format, and the
//! Diff column lists what is missing (green) and what is too much (red).
//!
//! The pieces:
//!
//! - [`eft`]: reads EFT text and compares two fits.
//! - [`assets`]: finds ships and their fits in ESI assets ("Open ship…").
//! - `worker`: fetches the assets on a background thread.
//! - `ui` and `picker`: draw the panel and the "Open ship" window.
//!
//! Both fits and the "Show matching" switch are saved in `fittings.toml`
//! in the data folder, so they are still there after a restart.

pub mod assets;
pub mod eft;
mod picker;
mod ui;
mod worker;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui;
use serde::{Deserialize, Serialize};

use self::assets::{Place, Ship};
use self::eft::{Diff, Fit};
use self::worker::{Command, Event, Worker};
use super::shared::{RosterCharacter, SharedRoster, roster_if_changed};
use super::{Module, ModuleContext, SharedStaticData};
use crate::auth::{TokenStore, now_unix};
use crate::static_data::StaticData;
use crate::storage::Saver;

const FILE_NAME: &str = "fittings.toml";
/// Clipboard text bigger than this can't be a fit (a full one is ~2 KB).
const MAX_PASTE_BYTES: usize = 256 * 1024;

/// The submodules, one tab each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Submodule {
    #[default]
    Compare,
}

/// Which of the two input columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Is,
    Should,
}

/// A column shows the parsed list or the raw text to edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColumnMode {
    #[default]
    List,
    Text,
}

/// What `fittings.toml` holds. New fields need a default so older files
/// still load.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Saved {
    pub is_text: String,
    pub should_text: String,
    pub show_matching: bool,
    /// Where a fit loaded with "Open ship…" came from. Cleared when the
    /// text is edited or pasted over.
    pub is_source: Option<EsiSource>,
    pub should_source: Option<EsiSource>,
}

/// A fit loaded from ESI assets: whose ship, where, and how old the data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EsiSource {
    pub character: String,
    pub place: String,
    /// When ESI made the data (Unix seconds).
    pub data_from: i64,
}

/// One character's ships for "Open ship…".
#[derive(Default)]
pub struct CharacterAssets {
    pub ships: Vec<(Ship, String)>,
    /// When ESI made the data, once fetched.
    pub data_from: Option<i64>,
    /// When ESI has newer data (or a failed fetch may be tried again).
    pub next_at: i64,
    pub loading: bool,
    pub problem: Option<String>,
}

/// The "Open ship" window.
pub struct Picker {
    pub open: bool,
    /// The column the ship goes into.
    pub target: Side,
    pub search: String,
    /// Only this character's ships; `None` for all.
    pub character: Option<u64>,
    pub place: Option<Place>,
}

impl Saved {
    fn file_path(data_dir: &Path) -> PathBuf {
        data_dir.join(FILE_NAME)
    }

    /// Reads the saved fits. A missing file is normal (first start); a
    /// broken one gives empty fits and a message.
    fn load(data_dir: &Path) -> (Saved, Option<String>) {
        let path = Saved::file_path(data_dir);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return (Saved::default(), None);
        };
        match toml::from_str::<Saved>(&text) {
            Ok(saved) => (saved, None),
            Err(err) => (
                Saved::default(),
                Some(format!("Couldn't read {FILE_NAME}, starting empty: {err}")),
            ),
        }
    }
}

pub struct Fittings {
    data_dir: PathBuf,
    /// Game data, to tell drones from cargo. `None` without static.sqlite.
    static_data: Option<Arc<StaticData>>,
    saver: Saver,
    saved: Saved,
    submodule: Submodule,
    is_mode: ColumnMode,
    should_mode: ColumnMode,
    /// Parsed from the texts. Only rebuilt when a text changes, not every
    /// frame.
    is_fit: Fit,
    should_fit: Fit,
    diff: Diff,
    /// A short note under the header, such as "Copied 7 lines".
    message: Option<String>,
    tokens: Arc<TokenStore>,
    roster: SharedRoster,
    roster_version: u64,
    /// The logged-in characters (Characters tab).
    characters: Vec<RosterCharacter>,
    /// Started the first time "Open ship…" is clicked.
    worker: Option<Worker>,
    assets: BTreeMap<u64, CharacterAssets>,
    /// Names of item types, stations and structures, from ESI.
    names: HashMap<i64, String>,
    picker: Picker,
}

impl Fittings {
    pub fn new(
        data_dir: &Path,
        static_data: &SharedStaticData,
        tokens: Arc<TokenStore>,
        roster: SharedRoster,
    ) -> Fittings {
        let (saved, problem) = Saved::load(data_dir);
        let mut module = Fittings {
            data_dir: data_dir.to_path_buf(),
            static_data: static_data.data.clone(),
            saver: Saver::start(),
            saved,
            submodule: Submodule::default(),
            is_mode: ColumnMode::default(),
            should_mode: ColumnMode::default(),
            is_fit: Fit::default(),
            should_fit: Fit::default(),
            diff: Diff::default(),
            message: problem,
            tokens,
            roster,
            roster_version: 0,
            characters: Vec::new(),
            worker: None,
            assets: BTreeMap::new(),
            names: HashMap::new(),
            picker: Picker {
                open: false,
                target: Side::Is,
                search: String::new(),
                character: None,
                place: None,
            },
        };
        module.reparse();
        module
    }

    /// Parses both texts again and compares them.
    fn reparse(&mut self) {
        match &self.static_data {
            Some(data) => {
                let is_drone = |name: &str| data.is_drone_or_fighter(name);
                self.is_fit = eft::parse_with_drones(&self.saved.is_text, &is_drone);
                self.should_fit = eft::parse_with_drones(&self.saved.should_text, &is_drone);
            }
            // No static.sqlite: guess drones vs cargo from the block order.
            None => {
                self.is_fit = eft::parse(&self.saved.is_text);
                self.should_fit = eft::parse(&self.saved.should_text);
            }
        }
        eft::align(&mut self.is_fit, &mut self.should_fit);
        self.diff = eft::compare(&self.is_fit, &self.should_fit);
    }

    fn save(&mut self) {
        if let Ok(text) = toml::to_string(&self.saved) {
            self.saver.save(Saved::file_path(&self.data_dir), text);
        }
    }

    /// A text was edited: parse, compare and save.
    fn texts_changed(&mut self) {
        self.reparse();
        self.save();
    }

    fn text_mut(&mut self, side: Side) -> &mut String {
        match side {
            Side::Is => &mut self.saved.is_text,
            Side::Should => &mut self.saved.should_text,
        }
    }

    /// "Paste": the clipboard text replaces that column's fit.
    fn paste(&mut self, side: Side) {
        // Reading the clipboard once is quick (no disk or network), like
        // Intel's tag import.
        let Some(text) = crate::modules::mining::clipboard::read_text_once(MAX_PASTE_BYTES) else {
            self.message = Some(if crate::modules::mining::clipboard::is_supported() {
                "Couldn't read text from the clipboard. In EVE's fitting window use \
                 \"Copy to clipboard\", then try again."
                    .to_string()
            } else {
                "Paste only works on Windows. Use Text and Ctrl+V instead.".to_string()
            });
            return;
        };
        *self.text_mut(side) = text;
        *self.source_mut(side) = None;
        self.message = None;
        self.texts_changed();
    }

    fn source_mut(&mut self, side: Side) -> &mut Option<EsiSource> {
        match side {
            Side::Is => &mut self.saved.is_source,
            Side::Should => &mut self.saved.should_source,
        }
    }

    pub fn source(&self, side: Side) -> Option<&EsiSource> {
        match side {
            Side::Is => self.saved.is_source.as_ref(),
            Side::Should => self.saved.should_source.as_ref(),
        }
    }

    /// The logged-in characters changed on the Characters tab.
    fn read_roster(&mut self) {
        let Some(roster) = roster_if_changed(&self.roster, self.roster_version) else {
            return;
        };
        self.roster_version = roster.version;
        self.characters = roster.characters;
        let ids: Vec<u64> = self.characters.iter().map(|c| c.id).collect();
        self.assets.retain(|id, _| ids.contains(id));
    }

    /// "Open ship…": opens the window and asks ESI for characters whose
    /// data is out of date.
    fn open_picker(&mut self, side: Side, ctx: &egui::Context) {
        self.picker.open = true;
        self.picker.target = side;
        self.fetch_assets(ctx);
    }

    /// Asks the worker for every character's assets. The worker skips
    /// characters whose ESI data is still current.
    fn fetch_assets(&mut self, ctx: &egui::Context) {
        let Some(static_data) = &self.static_data else {
            return;
        };
        let worker = self
            .worker
            .get_or_insert_with(|| Worker::start(ctx, self.tokens.clone(), static_data.clone()));
        let ids: Vec<u64> = self.characters.iter().map(|c| c.id).collect();
        worker.send(Command::Fetch(ids));
    }

    /// Takes in what the worker found.
    fn handle(&mut self, event: Event) {
        match event {
            Event::Started(id) => {
                self.assets.entry(id).or_default().loading = true;
            }
            Event::Ships(found) => {
                let entry = self.assets.entry(found.character_id).or_default();
                entry.ships = found.ships;
                entry.data_from = Some(found.data_from);
                entry.next_at = found.next_at;
                entry.loading = false;
                entry.problem = None;
            }
            Event::Names(names) => self.names.extend(names),
            Event::Problem {
                character_id,
                text,
                retry_at,
            } => {
                if character_id == 0 {
                    self.message = Some(text);
                    return;
                }
                let entry = self.assets.entry(character_id).or_default();
                entry.loading = false;
                entry.problem = Some(text);
                entry.next_at = retry_at;
            }
        }
    }

    pub fn character_name(&self, id: u64) -> String {
        self.characters
            .iter()
            .find(|c| c.id == id)
            .map_or_else(|| format!("Character {id}"), |c| c.name.clone())
    }

    /// "Jita IV - Moon 4 - …", "In space · Amarr", or the id when ESI
    /// gave no name (a structure the character can't dock at).
    pub fn place_name(&self, place: Place) -> String {
        match place {
            Place::Station(id) => self.names.get(&id).cloned().unwrap_or_else(|| {
                if id >= 1_000_000_000_000 {
                    format!("Structure {id}")
                } else {
                    format!("Station {id}")
                }
            }),
            Place::InSpace(system) => {
                let name = self
                    .static_data
                    .as_ref()
                    .and_then(|d| d.universe.system(system).map(|s| s.name.clone()))
                    .unwrap_or_else(|| system.to_string());
                format!("In space · {name}")
            }
        }
    }

    pub fn hull_name(&self, type_id: i64) -> String {
        self.static_data
            .as_ref()
            .and_then(|d| d.universe.ship_by_id(type_id).map(|s| s.name.clone()))
            .unwrap_or_else(|| format!("Type {type_id}"))
    }

    /// "Load": the ship's fit goes into the picker's column.
    fn load_ship(&mut self, character_id: u64, item_id: i64) {
        let Some(entry) = self.assets.get(&character_id) else {
            return;
        };
        let Some((ship, name)) = entry.ships.iter().find(|(s, _)| s.item_id == item_id) else {
            return;
        };
        let names = &self.names;
        let text = assets::to_eft(ship, &self.hull_name(ship.type_id), name, &|type_id| {
            names.get(&type_id).cloned()
        });
        let source = EsiSource {
            character: self.character_name(character_id),
            place: self.place_name(ship.place),
            data_from: entry.data_from.unwrap_or_else(now_unix),
        };
        let side = self.picker.target;
        *self.text_mut(side) = text;
        *self.source_mut(side) = Some(source);
        match side {
            Side::Is => self.is_mode = ColumnMode::List,
            Side::Should => self.should_mode = ColumnMode::List,
        }
        self.picker.open = false;
        self.message = None;
        self.texts_changed();
    }

    fn swap(&mut self) {
        std::mem::swap(&mut self.saved.is_text, &mut self.saved.should_text);
        std::mem::swap(&mut self.saved.is_source, &mut self.saved.should_source);
        std::mem::swap(&mut self.is_mode, &mut self.should_mode);
        self.texts_changed();
    }

    /// "Copy missing": the green items as Multibuy lines.
    fn copy_missing(&mut self, ctx: &egui::Context) {
        let text = self.diff.multibuy();
        let lines = text.lines().count();
        if lines == 0 {
            self.message = Some("Nothing is missing.".to_string());
            return;
        }
        ctx.copy_text(text);
        self.message = Some(format!(
            "Copied {lines} lines. Paste them into EVE's Multibuy."
        ));
    }
}

impl Module for Fittings {
    fn id(&self) -> &'static str {
        "fittings"
    }

    fn title(&self) -> &'static str {
        "Fittings"
    }

    fn update(&mut self, _ctx: &ModuleContext) {
        self.read_roster();
        // Nothing to drain until "Open ship…" started the worker.
        let events: Vec<Event> = match &self.worker {
            Some(worker) => worker.events.try_iter().collect(),
            None => Vec::new(),
        };
        for event in events {
            self.handle(event);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _ctx: &ModuleContext) {
        ui::show(self, ui);
        picker::show(self, ui.ctx());
    }

    fn on_exit(&mut self) {
        self.saver.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_fits_read_back() {
        let saved = Saved {
            is_text: "[Drake, A]\n\nDamage Control II".to_string(),
            should_text: "[Drake, B]".to_string(),
            show_matching: true,
            is_source: Some(EsiSource {
                character: "Char A".to_string(),
                place: "Jita IV - Moon 4".to_string(),
                data_from: 1_763_000_000,
            }),
            should_source: None,
        };
        let text = toml::to_string(&saved).unwrap();
        let back: Saved = toml::from_str(&text).unwrap();
        assert_eq!(back, saved);
        // An older or partial file still loads.
        let partial: Saved = toml::from_str("show_matching = true").unwrap();
        assert!(partial.show_matching);
        assert!(partial.is_text.is_empty());
    }
}
