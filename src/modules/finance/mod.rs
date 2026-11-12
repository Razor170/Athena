//! Finance: the wallets, income and expenses, and loyalty points of the
//! characters logged in on the Characters tab.
//!
//! The pieces:
//!
//! - [`worker`]: fetches from ESI on a background thread and saves to
//!   `finance.sqlite` ([`store`]), so the history grows past ESI's 30 days.
//! - [`data`]: the entries and the sums the tabs show.
//! - [`categories`]: which category each kind of journal entry belongs to.
//! - [`config`]: the picked wallets, saved in `finance.toml`.
//! - `ui`: draws the panel.
//!
//! This file holds the [`Finance`] module: it keeps everything the worker
//! sent, and applies new events in `update`.

pub mod categories;
pub mod config;
pub mod data;
pub mod store;
mod ui;
pub mod worker;

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui;

use self::config::FinanceSettings;
use self::data::{JournalEntry, LoyaltyPoints, MarketTrade};
use self::store::Store;
use self::worker::{Command, Event, Worker};
use super::shared::{PilotSystems, RosterCharacter, SharedRoster, roster_if_changed};
use super::{Module, ModuleContext, SharedStaticData};
use crate::auth::TokenStore;
use crate::static_data::StaticData;
use crate::storage::Saver;

pub struct Finance {
    data_dir: PathBuf,
    saver: Saver,
    settings: FinanceSettings,
    worker: Worker,
    roster: SharedRoster,
    roster_version: u64,
    /// The logged-in characters, in the Characters tab's order.
    characters: Vec<RosterCharacter>,
    static_data: Option<Arc<StaticData>>,
    pilot_systems: PilotSystems,
    /// False until the worker has read `finance.sqlite`.
    loaded: bool,
    /// Every saved journal entry, newest first.
    journal: Vec<JournalEntry>,
    /// Market trades by (character, journal entry id).
    trades: HashMap<(u64, i64), MarketTrade>,
    /// Character -> (balance, fetched at).
    balances: HashMap<u64, (f64, i64)>,
    loyalty: HashMap<u64, Vec<LoyaltyPoints>>,
    /// Character -> system, from ESI (the Local log, if Intel has it, wins).
    esi_locations: HashMap<u64, i64>,
    names: HashMap<i64, String>,
    /// When the journals were last fetched.
    journal_checked: Option<i64>,
    /// Problems per character, shown on their wallet card.
    character_problems: HashMap<u64, String>,
    /// Problems to show on top of the panel.
    problems: Vec<String>,
    /// Goes up whenever the data changes, so the tabs know when their
    /// cached lists are stale.
    data_version: u64,
    /// What the panel shows: tab, filters, cached lists.
    view: ui::ViewState,
    /// When "Refresh" was last clicked.
    last_refresh: Option<i64>,
}

/// "Refresh" works at most this often.
const REFRESH_COOLDOWN_SECS: i64 = 60;

impl Finance {
    pub fn new(
        egui_ctx: &egui::Context,
        data_dir: &Path,
        tokens: Arc<TokenStore>,
        roster: SharedRoster,
        static_data: &SharedStaticData,
        pilot_systems: PilotSystems,
    ) -> Finance {
        let (settings, problem) = FinanceSettings::load(data_dir);
        let worker = Worker::start(egui_ctx, tokens, Store::file_path(data_dir));
        let mut module = Finance {
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
            journal: Vec::new(),
            trades: HashMap::new(),
            balances: HashMap::new(),
            loyalty: HashMap::new(),
            esi_locations: HashMap::new(),
            names: HashMap::new(),
            journal_checked: None,
            character_problems: HashMap::new(),
            problems: problem.into_iter().collect(),
            data_version: 0,
            view: ui::ViewState::default(),
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

    fn save_settings(&self) {
        if let Ok(text) = toml::to_string(&self.settings) {
            self.saver
                .save(FinanceSettings::file_path(&self.data_dir), text);
        }
    }

    /// "Refresh": fetch everything now. With many characters that's a
    /// lot of ESI requests, so it works at most once a minute.
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

    /// The characters whose wallets are picked.
    pub fn picked(&self) -> BTreeSet<u64> {
        self.characters
            .iter()
            .map(|c| c.id)
            .filter(|id| !self.settings.unpicked.contains(id))
            .collect()
    }

    /// Clicking a wallet card: picks it, or unpicks it.
    pub fn toggle_pick(&mut self, id: u64) {
        if !self.settings.unpicked.remove(&id) {
            self.settings.unpicked.insert(id);
        }
        self.data_version += 1;
        self.save_settings();
    }

    /// The "All characters" card: picks all, or none if all were picked.
    pub fn toggle_all(&mut self) {
        let all_picked = self
            .characters
            .iter()
            .all(|c| !self.settings.unpicked.contains(&c.id));
        if all_picked {
            self.settings.unpicked = self.characters.iter().map(|c| c.id).collect();
        } else {
            self.settings.unpicked.clear();
        }
        self.data_version += 1;
        self.save_settings();
    }

    /// Picks exactly the characters with this tag.
    pub fn pick_tag(&mut self, tag: &str) {
        self.settings.unpicked = self
            .characters
            .iter()
            .filter(|c| !c.tags.iter().any(|t| t == tag))
            .map(|c| c.id)
            .collect();
        self.data_version += 1;
        self.save_settings();
    }

    /// The system the character is in: from the Local log if Intel saw
    /// them since the last downtime, else from ESI.
    pub fn location(&self, character: &RosterCharacter) -> Option<i64> {
        let from_log = self.pilot_systems.lock().ok().and_then(|pilots| {
            let pilot = pilots.get(&character.name)?;
            let downtime = last_downtime_unix(crate::auth::now_unix());
            (pilot.since_unix >= downtime).then_some(pilot.system_id)
        });
        from_log.or_else(|| self.esi_locations.get(&character.id).copied())
    }

    /// A name for an id from the journal, or the id itself.
    pub fn name_of(&self, id: i64) -> String {
        // An empty name means ESI doesn't know the id.
        if let Some(name) = self.names.get(&id).filter(|name| !name.is_empty()) {
            return name.clone();
        }
        if let Some(name) = self
            .static_data
            .as_deref()
            .and_then(|data| data.corporation_name(id))
        {
            return name.to_string();
        }
        if let Some(c) = self.characters.iter().find(|c| c.id as i64 == id) {
            return c.name.clone();
        }
        format!("#{id}")
    }

    fn apply(&mut self, event: Event) {
        match event {
            Event::Loaded(saved) => {
                self.loaded = true;
                self.journal = saved.journal;
                self.trades = saved
                    .trades
                    .into_iter()
                    .map(|t| ((t.character_id, t.journal_ref_id), t))
                    .collect();
                self.balances = saved.balances;
                self.loyalty = saved.loyalty;
                self.names = saved.names;
            }
            Event::Balance {
                character_id,
                balance,
                fetched,
            } => {
                self.balances.insert(character_id, (balance, fetched));
            }
            Event::Journal(entries, trades) => {
                self.journal.extend(entries);
                // Newest first; equal times by id so the order is stable.
                self.journal
                    .sort_by(|a, b| b.when.cmp(&a.when).then(b.id.cmp(&a.id)));
                for t in trades {
                    self.trades.insert((t.character_id, t.journal_ref_id), t);
                }
            }
            Event::Loyalty {
                character_id,
                points,
            } => {
                self.loyalty.insert(character_id, points);
            }
            Event::Location {
                character_id,
                system_id,
            } => {
                self.esi_locations.insert(character_id, system_id);
            }
            Event::Names(names) => self.names.extend(names),
            Event::JournalChecked(at) => self.journal_checked = Some(at),
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

/// EVE's daily downtime is at 11:00 UTC. The last one before `now`.
fn last_downtime_unix(now: i64) -> i64 {
    let today = now.div_euclid(86_400) * 86_400 + 11 * 3600;
    if today <= now { today } else { today - 86_400 }
}

impl Module for Finance {
    fn id(&self) -> &'static str {
        "finance"
    }

    fn title(&self) -> &'static str {
        "Finance"
    }

    fn update(&mut self, _ctx: &ModuleContext) {
        self.read_roster();
        // `try_recv` returns at once when nothing is waiting.
        while let Ok(event) = self.worker.events.try_recv() {
            self.apply(event);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _ctx: &ModuleContext) {
        ui::show(self, ui);
    }

    fn on_exit(&mut self) {
        self.saver.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downtime_is_11_utc() {
        let day = 1_794_096_000; // 2026-11-08 00:00 UTC
        assert_eq!(last_downtime_unix(day + 12 * 3600), day + 11 * 3600);
        assert_eq!(last_downtime_unix(day + 11 * 3600), day + 11 * 3600);
        assert_eq!(last_downtime_unix(day + 10 * 3600), day - 13 * 3600);
    }
}
