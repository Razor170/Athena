//! Contracts: the status of every contract of the logged-in characters,
//! and a changelog of what changed, including what changed while Athena
//! was closed.
//!
//! The pieces:
//!
//! - [`worker`]: fetches contracts, items and bids from ESI on a background
//!   thread, finds the changes and saves everything in `contracts.sqlite`
//!   ([`store`]).
//! - [`data`]: what ESI sends, and the [`Contract`] Athena keeps.
//! - [`changes`]: what counts as a change, and which alert covers it.
//! - [`config`]: alert switches, filters and the split, in `contracts.toml`.
//! - `ui`: draws the panel.
//!
//! This file holds the [`Contracts`] module: it keeps what the worker sent,
//! raises alerts for new changelog entries and tracks which were seen.

pub mod changes;
pub mod config;
pub mod data;
pub mod store;
mod ui;
pub mod worker;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui;

use self::config::ContractsSettings;
use self::data::{Contract, ContractType, Side};
use self::store::LogEntry;
use self::worker::{Command, Event, Worker};
use super::shared::{RosterCharacter, SharedRoster, roster_if_changed};
use super::{Module, ModuleContext, SharedStaticData};
use crate::auth::TokenStore;
use crate::modules::finance::data::short_isk;
use crate::static_data::StaticData;
use crate::storage::Saver;

/// "Refresh" works at most this often.
const REFRESH_COOLDOWN_SECS: i64 = 60;
/// Changelog entries drawn at first; "Show older" adds this many more.
const LOG_PAGE: usize = 300;

pub struct Contracts {
    data_dir: PathBuf,
    saver: Saver,
    settings: ContractsSettings,
    worker: Worker,
    egui_ctx: egui::Context,
    roster: SharedRoster,
    roster_version: u64,
    /// The logged-in characters, in the Characters tab's order.
    characters: Vec<RosterCharacter>,
    static_data: Option<Arc<StaticData>>,
    /// False until the worker has read `contracts.sqlite`.
    loaded: bool,
    /// When Athena started: older changes happened "while closed".
    started_at: i64,
    contracts: HashMap<i64, Contract>,
    /// Newest first.
    log: Vec<LogEntry>,
    names: HashMap<i64, String>,
    /// Every character whose contracts were ever fetched.
    known: Vec<u64>,
    checked: Option<i64>,
    character_problems: HashMap<u64, String>,
    problems: Vec<String>,
    view: ui::View,
    /// Contracts whose details are open.
    open: HashSet<i64>,
    /// Scroll the list to this contract on the next frame.
    scroll_to: Option<i64>,
    search: String,
    /// How many changelog entries are drawn.
    log_shown: usize,
    last_refresh: Option<i64>,
}

impl Contracts {
    pub fn new(
        egui_ctx: &egui::Context,
        data_dir: &Path,
        tokens: Arc<TokenStore>,
        roster: SharedRoster,
        static_data: &SharedStaticData,
    ) -> Contracts {
        let (settings, problem) = ContractsSettings::load(data_dir);
        let started_at = crate::auth::now_unix();
        let worker = Worker::start(
            egui_ctx,
            tokens,
            store::Store::file_path(data_dir),
            settings.soon_hours,
            started_at,
        );
        let mut module = Contracts {
            data_dir: data_dir.to_path_buf(),
            saver: Saver::start(),
            settings,
            worker,
            egui_ctx: egui_ctx.clone(),
            roster,
            roster_version: 0,
            characters: Vec::new(),
            static_data: static_data.data.clone(),
            loaded: false,
            started_at,
            contracts: HashMap::new(),
            log: Vec::new(),
            names: HashMap::new(),
            known: Vec::new(),
            checked: None,
            character_problems: HashMap::new(),
            problems: problem.into_iter().collect(),
            view: ui::View::default(),
            open: HashSet::new(),
            scroll_to: None,
            search: String::new(),
            log_shown: LOG_PAGE,
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
    }

    fn save_settings(&mut self) {
        if let Ok(text) = toml::to_string(&self.settings) {
            self.saver
                .save(ContractsSettings::file_path(&self.data_dir), text);
        }
    }

    /// The "soon" hours changed: save, and tell the worker.
    fn soon_hours_changed(&mut self) {
        self.worker
            .send(Command::SetSoonHours(self.settings.soon_hours));
        self.save_settings();
    }

    /// "Refresh": ask ESI for every contract list now.
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

    /// Our characters: the logged-in ones and any fetched before.
    fn mine(&self) -> Vec<u64> {
        let mut ids = self.known.clone();
        for c in &self.characters {
            if !ids.contains(&c.id) {
                ids.push(c.id);
            }
        }
        ids
    }

    fn unseen_count(&self) -> usize {
        self.log.iter().filter(|e| !e.seen).count()
    }

    /// Marks one changelog entry as seen.
    fn mark_seen(&mut self, entry_id: i64) {
        if let Some(entry) = self.log.iter_mut().find(|e| e.id == entry_id && !e.seen) {
            entry.seen = true;
            self.worker.send(Command::MarkSeen(vec![entry_id]));
        }
    }

    fn mark_all_seen(&mut self) {
        for entry in &mut self.log {
            entry.seen = true;
        }
        self.worker.send(Command::MarkAllSeen);
    }

    /// A name from ESI, or "…" while it's being looked up.
    fn name(&self, id: i64) -> String {
        match self.names.get(&id) {
            Some(name) if !name.is_empty() => name.clone(),
            _ => "…".to_string(),
        }
    }

    fn character_name(&self, id: u64) -> String {
        match self.characters.iter().find(|c| c.id == id) {
            Some(c) => c.name.clone(),
            None => self.name(id as i64),
        }
    }

    /// The solar system a station or structure is in, from the start of
    /// its name ("Jita IV - Moon 4 - ..." is in Jita). The longest start
    /// that is a known system wins, so "New Caldari" isn't read as "New".
    fn system_of(&self, location_id: i64) -> String {
        let Some(name) = self.names.get(&location_id).filter(|n| !n.is_empty()) else {
            return "…".to_string();
        };
        let words: Vec<&str> = name.split(' ').collect();
        if let Some(data) = self.static_data.as_deref() {
            for count in (1..=words.len().min(4)).rev() {
                let start = words[..count].join(" ");
                if data.universe.system_by_name(&start).is_some() {
                    return start;
                }
            }
        }
        words.first().map_or_else(String::new, |w| w.to_string())
    }

    /// The contract's line in the list: a courier's route, its own title,
    /// or its first item.
    fn contract_title(&self, contract: &Contract) -> String {
        if contract.kind == ContractType::Courier {
            return format!(
                "{} » {}, {} m³",
                self.system_of(contract.start_location_id),
                self.system_of(contract.end_location_id),
                thousands(contract.volume)
            );
        }
        if !contract.title.is_empty() {
            return contract.title.clone();
        }
        let Some(items) = &contract.items else {
            return format!("{} #{}", contract.kind.label(), contract.id);
        };
        let given: Vec<_> = items.iter().filter(|i| i.included).collect();
        let (list, prefix) = if given.is_empty() {
            (items.iter().collect::<Vec<_>>(), "Want to buy: ")
        } else {
            (given, "")
        };
        match list.first() {
            None => format!("{} #{}", contract.kind.label(), contract.id),
            Some(first) => {
                let mut text = format!(
                    "{prefix}{} ×{}",
                    self.name(first.type_id),
                    thousands(first.quantity as f64)
                );
                if list.len() > 1 {
                    text.push_str(&format!(" + {} more", list.len() - 1));
                }
                text
            }
        }
    }

    /// The money in the Price column, as a number: courier reward, top
    /// bid, or price. A "want to buy" (the issuer pays) is minus for the
    /// issuer, plus for the other side.
    fn price_value(&self, contract: &Contract, side: Side) -> f64 {
        match contract.kind {
            ContractType::Courier => contract.reward,
            ContractType::Auction => contract.top_bid().map_or(contract.price, |b| b.amount),
            _ if contract.price == 0.0 && contract.reward > 0.0 => {
                if side == Side::Issued {
                    -contract.reward
                } else {
                    contract.reward
                }
            }
            _ => contract.price,
        }
    }

    /// The Price column's text: "620 m", "-24.0 m", "+412 m".
    fn price_text(&self, contract: &Contract, side: Side) -> String {
        let value = self.price_value(contract, side);
        let wanted = contract.kind == ContractType::ItemExchange
            && contract.price == 0.0
            && contract.reward > 0.0;
        if wanted && value > 0.0 {
            format!("+{}", short_isk(value))
        } else {
            short_isk(value)
        }
    }

    fn apply(&mut self, event: Event) {
        match event {
            Event::Loaded(saved) => {
                self.loaded = true;
                let saved = *saved;
                for (contract, _) in saved.contracts {
                    self.contracts.entry(contract.id).or_insert(contract);
                }
                self.log.extend(saved.log);
                self.sort_log();
                self.names.extend(saved.names);
                self.known = saved.known_characters;
            }
            Event::Contracts(list) => {
                for contract in list {
                    self.contracts.insert(contract.id, contract);
                }
            }
            Event::Log(entries) => self.add_log(entries),
            Event::Names(names) => self.names.extend(names),
            Event::Known(ids) => self.known = ids,
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
    }

    /// New changelog entries: alert for the ones whose switch is on. An
    /// entry from this session with its alert off counts as seen at once;
    /// one from while Athena was closed always stays new until clicked.
    fn add_log(&mut self, mut entries: Vec<LogEntry>) {
        let mut flash = false;
        let mut beep = false;
        let mut auto_seen = Vec::new();
        let mine = self.mine();
        for entry in &mut entries {
            // The worker sends the contracts before their entries.
            let side = self
                .contracts
                .get(&entry.contract_id)
                .map_or(Side::Other, |c| c.side(&mine).0);
            let switch = entry
                .kind
                .alert_kind(side)
                .map(|kind| self.settings.alerts.get(kind));
            let alert = switch.is_some_and(|s| s.alert);
            if alert {
                flash = true;
            } else if !entry.offline {
                entry.seen = true;
                auto_seen.push(entry.id);
            }
            beep |= switch.is_some_and(|s| s.sound);
        }
        if !auto_seen.is_empty() {
            self.worker.send(Command::MarkSeen(auto_seen));
        }
        self.log.extend(entries);
        self.sort_log();
        if beep {
            super::shared::alert_sound();
        }
        if flash {
            // Flashes the taskbar button until Athena gets the focus.
            self.egui_ctx
                .send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(
                    egui::UserAttentionType::Informational,
                ));
        }
    }

    fn sort_log(&mut self) {
        self.log
            .sort_by(|a, b| b.at.cmp(&a.at).then(b.id.cmp(&a.id)));
    }
}

/// "320,000": a whole number with thousands separators.
fn thousands(value: f64) -> String {
    crate::modules::finance::data::full_isk(value, false)
}

impl Module for Contracts {
    fn id(&self) -> &'static str {
        "contracts"
    }

    fn title(&self) -> &'static str {
        "Contracts"
    }

    fn badge(&self) -> Option<String> {
        let count = self.unseen_count();
        (count > 0).then(|| count.to_string())
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
        ui::show(self, ui, now);
        // "Expires in" and "x ago" move with the clock: redraw once a
        // minute while the tab is visible, at the start of the next minute.
        let wait = 60 - now.rem_euclid(60) + 1;
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs(wait as u64));
    }

    fn on_exit(&mut self) {
        self.saver.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::contracts::changes::ChangeKind;

    fn module(name: &str) -> (Contracts, PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("athena-contracts-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let shared = SharedStaticData {
            data: Some(Arc::new(crate::static_data::tests::sample())),
            problem: None,
        };
        let module = Contracts::new(
            &egui::Context::default(),
            &dir,
            Arc::new(TokenStore::new()),
            SharedRoster::default(),
            &shared,
        );
        (module, dir)
    }

    fn entry(id: i64, kind: ChangeKind, offline: bool) -> LogEntry {
        LogEntry {
            id,
            at: 1_794_500_000 + id,
            contract_id: 1,
            kind,
            amount: None,
            offline,
            seen: false,
        }
    }

    #[test]
    fn alerts_decide_what_stays_new() {
        let (mut module, dir) = module("alerts");
        module.settings.alerts.ended.alert = false;
        module.add_log(vec![
            // Alert on: stays new.
            entry(1, ChangeKind::Accepted, false),
            // Alert off, this session: seen at once.
            entry(2, ChangeKind::Expired, false),
            // Alert off, but while closed: stays new.
            entry(3, ChangeKind::Expired, true),
            // Never an alert (you did it), this session: seen.
            entry(4, ChangeKind::Issued, false),
        ]);
        let seen: Vec<(i64, bool)> = module.log.iter().map(|e| (e.id, e.seen)).collect();
        // Newest first.
        assert_eq!(seen, vec![(4, true), (3, false), (2, true), (1, false)]);
        assert_eq!(module.badge(), Some("2".to_string()));
        module.mark_seen(3);
        assert_eq!(module.badge(), Some("1".to_string()));
        module.mark_all_seen();
        assert_eq!(module.badge(), None);
        drop(module);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn titles_and_systems() {
        let (mut module, dir) = module("titles");
        let mut contract =
            Contract::from_esi(crate::modules::contracts::data::tests::esi_courier(), 1).unwrap();
        // The sample map has a system called "A1-XYZ".
        module.names.insert(
            contract.start_location_id,
            "A1-XYZ IV - Moon 4 - Station".into(),
        );
        assert_eq!(module.system_of(contract.start_location_id), "A1-XYZ");
        assert_eq!(module.system_of(contract.end_location_id), "…");
        assert_eq!(module.contract_title(&contract), "A1-XYZ » …, 320,000 m³");

        contract.kind = ContractType::ItemExchange;
        contract.items = Some(vec![
            data::Item {
                type_id: 34,
                quantity: 2_000_000,
                included: true,
            },
            data::Item {
                type_id: 35,
                quantity: 5,
                included: true,
            },
        ]);
        module.names.insert(34, "Tritanium".into());
        assert_eq!(
            module.contract_title(&contract),
            "Tritanium ×2,000,000 + 1 more"
        );
        contract.title = "Ore buyback".into();
        assert_eq!(module.contract_title(&contract), "Ore buyback");
        drop(module);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
