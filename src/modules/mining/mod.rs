//! Mining Supervisor: a panel for multiboxed miners. One card per
//! character with the ore hold fill and alerts, a belt scan read from the
//! clipboard, and a history of finished sessions.
//!
//! The pieces:
//!
//! - [`gamelog`]: reads EVE game log files and turns mining lines into events.
//! - [`watcher`]: follows the game log files on a background thread.
//! - [`session`]: adds those events up into per-character mining state.
//! - [`history`]: one History entry per "End session" click, with all pilots.
//! - [`prices`]: Jita prices from ESI (refreshed hourly) and ore values.
//! - [`store`]: saves finished sessions (valued once) in `mining.sqlite`.
//! - [`scan`]: parses Mining Survey Scanner results and works out how fast a
//!   belt is being mined.
//! - [`clipboard`]: picks up belt scans copied in EVE (Windows only).
//! - [`config`], [`state`]: the module's settings and the live state that
//!   survives a restart (`mining.toml`, `mining_state.toml`).
//! - [`ui`]: draws the panel, following the mockup Kevin approved.
//!
//! This file holds the [`MiningSupervisor`] module itself: it owns the
//! background threads, drains their channels in `update`, and keeps one
//! `CharacterTracker` per character.

pub mod clipboard;
pub mod config;
pub mod gamelog;
pub mod history;
pub mod prices;
pub mod scan;
pub mod session;
pub mod state;
pub mod store;
mod ui;
pub mod watcher;

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;

use self::clipboard::ClipboardWatcher;
use self::config::MiningSettings;
use self::gamelog::LogTime;
use self::history::GroupSession;
use self::prices::{JitaPrices, PriceFetcher, PriceUpdate};
use self::scan::{BeltScan, BeltScans};
use self::session::{Alert, CharacterTracker, VolumeTable};
use self::state::{SavedCharacter, SavedState};
use self::store::{HistorySnapshot, HistoryStore, HistoryUpdate};
use self::watcher::{FileOffset, GameLogWatcher, WatchEvent};
use super::{Module, ModuleContext, SharedStaticData};
use crate::overlay::OverlayStatus;
use crate::overlay::bars::CardBars;
use crate::settings::Settings;
use crate::static_data::StaticData;
use crate::storage::Saver;

/// How often time-based alerts (idle, miner stopped) are checked while
/// someone is mining. Also how often the durations on screen move on.
const TICK_EVERY: Duration = Duration::from_secs(5);
/// The live state is saved at most this often while mining (and right away
/// after "Emptied" or "End session").
const SAVE_STATE_EVERY: Duration = Duration::from_secs(15);
/// How many alerts the Live view lists.
const ALERTS_KEPT: usize = 4;

/// Which view of the panel is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    Live,
    History,
    Settings,
}

/// How the History view groups sessions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HistoryMode {
    Sessions,
    Daily,
    Weekly,
}

/// How an alert is coloured in the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AlertLevel {
    Warning,
    Error,
    Crystal,
    Info,
}

/// One line in the Live view's alert list.
#[derive(Debug, Clone)]
struct AlertEntry {
    time: LogTime,
    character: String,
    text: String,
    level: AlertLevel,
}

pub struct MiningSupervisor {
    egui_ctx: egui::Context,
    data_dir: PathBuf,
    /// The game log folder the watcher follows (from the app settings).
    log_folder: PathBuf,
    /// Writes settings and state files on a background thread.
    saver: Saver,
    settings: MiningSettings,

    /// `None` if static.sqlite is missing or broken (see `problems`).
    static_data: Option<Arc<StaticData>>,
    volumes: VolumeTable,
    prices: Option<JitaPrices>,

    trackers: BTreeMap<String, CharacterTracker>,
    /// How far each log file has been read, by file name.
    offsets: BTreeMap<String, FileOffset>,
    /// False until the watcher has read what was already in the logs.
    caught_up: bool,
    alerts: VecDeque<AlertEntry>,

    /// The last two belt scans; the belt rate compares them.
    belt_scans: BeltScans,

    history: HistorySnapshot,

    // Background threads. Dropping a handle stops its thread.
    watcher: GameLogWatcher,
    price_fetcher: Option<PriceFetcher>,
    clipboard: ClipboardWatcher,
    history_store: HistoryStore,

    /// Things the user should know about, by source, shown on top of the panel.
    problems: BTreeMap<&'static str, String>,

    // UI state.
    view: View,
    history_mode: HistoryMode,
    /// The History session that is opened, by database id.
    open_session: Option<i64>,
    /// Text of the number fields in Settings while they are being edited.
    edit_buffers: BTreeMap<String, String>,

    state_dirty: bool,
    last_state_save: Instant,
    last_tick: Instant,
    /// The EVE day the history snapshot was made on, to refresh it at midnight.
    history_day: i64,
    /// What the card overlay saw on the last frame, for Settings.
    overlay_status: OverlayStatus,
}

/// The current time as EVE time (UTC).
fn now() -> LogTime {
    let unix_seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    LogTime { unix_seconds }
}

impl MiningSupervisor {
    /// Runs once at startup, before the window shows: reads the small
    /// settings, state and static data files, then starts the threads.
    pub fn new(
        egui_ctx: &egui::Context,
        app_settings: &Settings,
        data_dir: &Path,
        shared: &SharedStaticData,
    ) -> Self {
        let mut problems = BTreeMap::new();
        let (settings, settings_problem) = MiningSettings::load(data_dir);
        let (saved, state_problem) = SavedState::load(data_dir);
        if let Some(problem) = settings_problem {
            problems.insert("settings", problem);
        }
        if let Some(problem) = state_problem {
            problems.insert("state", problem);
        }

        let static_data = shared.data.clone();
        if let Some(problem) = &shared.problem {
            problems.insert("static", problem.clone());
        }
        let mut volumes = VolumeTable::default();
        if let Some(data) = &static_data {
            for item in data.minable_items() {
                volumes.insert(&item.name, item.volume_m3);
            }
        }

        let mut trackers = BTreeMap::new();
        for SavedCharacter {
            name,
            hold_units,
            session,
            hold_reported_full,
            stopped_by_log,
        } in saved.characters
        {
            let mut tracker = CharacterTracker::new(&name, settings.tracker_settings(&name));
            tracker.restore(hold_units, session, hold_reported_full, stopped_by_log);
            tracker.settle_fill_alerts(&volumes);
            trackers.insert(name, tracker);
        }
        let offsets: BTreeMap<String, FileOffset> = saved
            .files
            .into_iter()
            .map(|offset| (offset.file.clone(), offset))
            .collect();

        let watcher = GameLogWatcher::start(
            &app_settings.log_folder,
            offsets.values().cloned().collect(),
            egui_ctx.clone(),
        );
        let price_fetcher = static_data
            .as_ref()
            .map(|data| PriceFetcher::start(data.material_ids(), data_dir, egui_ctx.clone()));

        MiningSupervisor {
            egui_ctx: egui_ctx.clone(),
            data_dir: data_dir.to_path_buf(),
            log_folder: app_settings.log_folder.clone(),
            saver: Saver::start(),
            settings,
            static_data,
            volumes,
            prices: None,
            trackers,
            offsets,
            caught_up: false,
            alerts: VecDeque::new(),
            belt_scans: BeltScans::default(),
            history: HistorySnapshot::default(),
            watcher,
            price_fetcher,
            clipboard: ClipboardWatcher::start(egui_ctx.clone()),
            history_store: HistoryStore::start(data_dir, egui_ctx.clone()),
            problems,
            view: View::Live,
            history_mode: HistoryMode::Sessions,
            open_session: None,
            edit_buffers: BTreeMap::new(),
            state_dirty: false,
            last_state_save: Instant::now(),
            last_tick: Instant::now(),
            history_day: store::day_start(now().unix_seconds),
            overlay_status: OverlayStatus::default(),
        }
    }

    /// Applies new log lines for one character.
    fn apply_events(
        &mut self,
        position: FileOffset,
        events: Vec<gamelog::TimedMiningEvent>,
        catch_up: bool,
    ) {
        let name = position.character.clone();
        let settings = self.settings.tracker_settings(&name);
        // `entry().or_insert_with` creates the tracker the first time a
        // character shows up in the logs.
        let tracker = self
            .trackers
            .entry(name.clone())
            .or_insert_with(|| CharacterTracker::new(&name, settings));
        let mut raised = Vec::new();
        for event in &events {
            for alert in tracker.apply(event, &self.volumes) {
                raised.push((event.time, alert));
            }
        }
        // Lines that were already in the file at startup are old news.
        if !catch_up {
            for (time, alert) in raised {
                self.raise(time, &name, alert);
            }
        }
        self.offsets.insert(position.file.clone(), position);
        self.state_dirty = true;
    }

    /// Adds an alert to the list and beeps if that is switched on.
    fn raise(&mut self, time: LogTime, character: &str, alert: Alert) {
        let (text, level) = match alert {
            Alert::HoldAboveThreshold { percent } => {
                (format!("Ore hold at {percent:.0}%"), AlertLevel::Warning)
            }
            Alert::HoldFull => ("Ore hold full".to_string(), AlertLevel::Error),
            Alert::MinerStopped { reason } => (
                format!("Miner stopped: {}", short_stop_reason(&reason)),
                AlertLevel::Error,
            ),
            Alert::CrystalDestroyed { .. } => {
                ("Mining crystal destroyed".to_string(), AlertLevel::Crystal)
            }
            Alert::Idle { minutes } => (format!("Idle for {minutes} min"), AlertLevel::Info),
        };
        self.alerts.push_front(AlertEntry {
            time,
            character: character.to_string(),
            text,
            level,
        });
        self.alerts.truncate(ALERTS_KEPT);
        if self.settings.sound {
            sound::alert();
        }
    }

    /// A new belt scan from the clipboard.
    fn add_scan(&mut self, scan: BeltScan) {
        self.belt_scans.add(scan);
    }

    /// True when prices and ore data are loaded, so a session can be valued.
    fn can_value_sessions(&self) -> bool {
        let prices_loaded = self.prices.as_ref().is_some_and(|p| p.fetched_unix > 0);
        prices_loaded && self.static_data.is_some()
    }

    /// The "End session" button: ends every character's session, values it
    /// with the prices of this moment and stores it.
    fn end_session(&mut self) {
        // The value is frozen when the session ends, so without prices or
        // ore data it would be stored as 0 ISK forever. Keep the session
        // running instead and say why.
        if !self.can_value_sessions() {
            self.problems.insert(
                "end",
                "The session can't be valued yet (Jita prices or ore data not loaded), \
                 so it keeps running. Try \"End session\" again once prices are in."
                    .to_string(),
            );
            return;
        }
        self.problems.remove("end");
        let pilots = self
            .trackers
            .values_mut()
            .filter_map(CharacterTracker::finish_session)
            .collect();
        if let Some(group) = GroupSession::from_pilots(pilots) {
            let empty = StaticData::default();
            let data = self.static_data.as_deref().unwrap_or(&empty);
            let no_prices = JitaPrices::default();
            let prices = self.prices.as_ref().unwrap_or(&no_prices);
            let stored = store::value_group(
                &group,
                data,
                prices,
                self.settings.price_side,
                self.settings.yields(),
            );
            self.history_store.store(stored);
        }
        self.save_state();
    }

    /// The "Empty all" button: everyone unloaded at a station.
    fn mark_all_emptied(&mut self) {
        for tracker in self.trackers.values_mut() {
            tracker.mark_emptied();
        }
        self.save_state();
    }

    /// The "Emptied" button on a card.
    fn mark_emptied(&mut self, character: &str) {
        if let Some(tracker) = self.trackers.get_mut(character) {
            tracker.mark_emptied();
        }
        self.save_state();
    }

    /// Settings were changed in the Settings view.
    fn settings_edited(&mut self) {
        for (name, tracker) in &mut self.trackers {
            tracker.settings = self.settings.tracker_settings(name);
        }
        if let Ok(text) = self.settings.to_toml() {
            self.saver
                .save(MiningSettings::file_path(&self.data_dir), text);
        }
    }

    /// Queues the live state to be written.
    fn save_state(&mut self) {
        let state = SavedState {
            characters: self
                .trackers
                .values()
                .map(|tracker| SavedCharacter {
                    name: tracker.character.clone(),
                    hold_units: tracker.hold_units().clone(),
                    session: tracker.session().cloned(),
                    hold_reported_full: tracker.hold_reported_full(),
                    stopped_by_log: tracker.stopped_by_log(),
                })
                .collect(),
            files: self.offsets.values().cloned().collect(),
        };
        if let Ok(text) = state.to_toml() {
            self.saver.save(SavedState::file_path(&self.data_dir), text);
        }
        self.state_dirty = false;
        self.last_state_save = Instant::now();
    }

    fn anyone_mining(&self) -> bool {
        self.trackers.values().any(|t| t.session().is_some())
    }
}

/// Shortens EVE's long "deactivates ..." text for the alert list.
fn short_stop_reason(reason: &str) -> String {
    if reason.contains("pale shadow") {
        "rock depleted".to_string()
    } else if reason.contains("destruction of") {
        "crystal destroyed".to_string()
    } else if reason.len() > 40 {
        let cut: String = reason.chars().take(40).collect();
        format!("{cut}...")
    } else {
        reason.to_string()
    }
}

impl Module for MiningSupervisor {
    fn id(&self) -> &'static str {
        "mining"
    }

    fn title(&self) -> &'static str {
        "Mining Supervisor"
    }

    fn update(&mut self, ctx: &ModuleContext) {
        self.overlay_status = ctx.overlay;
        // Drain every channel. `try_recv` returns at once when it's empty.
        while let Ok(event) = self.watcher.events.try_recv() {
            match event {
                WatchEvent::Events {
                    position,
                    events,
                    catch_up,
                } => self.apply_events(position, events, catch_up),
                WatchEvent::CaughtUp => {
                    // Check the time-based alerts once without showing them:
                    // a pilot who stopped before Athena started is old news.
                    let now = now();
                    for tracker in self.trackers.values_mut() {
                        tracker.tick(now);
                    }
                    self.caught_up = true;
                    self.problems.remove("logs");
                }
                WatchEvent::Problem(text) => {
                    self.problems.insert("logs", text);
                }
            }
        }
        if let Some(fetcher) = &self.price_fetcher {
            while let Ok(update) = fetcher.updates.try_recv() {
                match update {
                    PriceUpdate::Prices(prices) => {
                        self.prices = Some(prices);
                        self.problems.remove("prices");
                        // "End session" works again.
                        self.problems.remove("end");
                    }
                    PriceUpdate::Failed(text) => {
                        self.problems.insert(
                            "prices",
                            format!("Jita prices could not be updated: {text}"),
                        );
                    }
                }
            }
        }
        while let Ok(scan) = self.clipboard.scans.try_recv() {
            self.add_scan(scan);
        }
        while let Ok(update) = self.history_store.updates.try_recv() {
            match update {
                HistoryUpdate::Snapshot(snapshot) => self.history = *snapshot,
                HistoryUpdate::Problem(text) => {
                    self.problems.insert("history", text);
                }
            }
        }

        // Time-based alerts, only while someone is mining. Each tick asks
        // egui for the next frame in TICK_EVERY, so nothing redraws while
        // nobody mines.
        if self.anyone_mining() {
            if self.last_tick.elapsed() >= TICK_EVERY {
                self.last_tick = Instant::now();
                let now = now();
                let mut raised = Vec::new();
                for (name, tracker) in &mut self.trackers {
                    for alert in tracker.tick(now) {
                        raised.push((name.clone(), alert));
                    }
                }
                if self.caught_up {
                    for (name, alert) in raised {
                        self.raise(now, &name, alert);
                    }
                }
            }
            self.egui_ctx.request_repaint_after(TICK_EVERY);
        }

        // Midnight EVE time: "Today" and "Yesterday" move on.
        let today = store::day_start(now().unix_seconds);
        if today != self.history_day {
            self.history_day = today;
            self.history_store.refresh();
        }

        if self.state_dirty && self.last_state_save.elapsed() >= SAVE_STATE_EVERY {
            self.save_state();
        }
        if self.state_dirty {
            // Make sure a frame comes to save the rest, even if nothing else happens.
            self.egui_ctx.request_repaint_after(SAVE_STATE_EVERY);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _ctx: &ModuleContext) {
        ui::show(self, ui);
    }

    fn card_bars(&self) -> Option<CardBars> {
        if !self.settings.card_bars {
            return None;
        }
        let bars = self
            .trackers
            .values()
            // Characters hidden from the Live view get no bar either.
            .filter(|t| self.settings.character(&t.character).show)
            .map(|t| ui::card_bar(self, t))
            .collect();
        Some(CardBars {
            bars,
            height_px: self.settings.card_bar_height_px as f32,
            compressed_color: ui::PURPLE,
        })
    }

    fn settings_changed(&mut self, settings: &Settings) {
        if settings.log_folder == self.log_folder {
            return;
        }
        self.log_folder = settings.log_folder.clone();
        // Follow the new log folder. Offsets are by file name, so files
        // already read are not read again.
        self.watcher = GameLogWatcher::start(
            &settings.log_folder,
            self.offsets.values().cloned().collect(),
            self.egui_ctx.clone(),
        );
        self.caught_up = false;
    }

    fn on_exit(&mut self) {
        self.save_state();
        self.saver.finish();
        // Writes a session ended just before closing.
        self.history_store.finish();
    }
}

/// The alert sound.
mod sound {
    /// Plays the Windows "exclamation" sound. It returns at once; Windows
    /// plays it in the background.
    #[cfg(windows)]
    pub fn alert() {
        // MessageBeep lives in the "Diagnostics::Debug" part of the bindings.
        use windows_sys::Win32::System::Diagnostics::Debug::MessageBeep;
        use windows_sys::Win32::UI::WindowsAndMessaging::MB_ICONEXCLAMATION;
        // SAFETY: MessageBeep takes a plain number and has no other effects.
        unsafe {
            MessageBeep(MB_ICONEXCLAMATION);
        }
    }

    #[cfg(not(windows))]
    pub fn alert() {}
}
