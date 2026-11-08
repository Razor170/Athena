//! Mining history on disk: `mining.sqlite` in the data folder.
//!
//! When the user clicks "End session", the module values each pilot's ore
//! with the Jita prices of that moment ([`value_group`]) and hands the
//! result to the history thread ([`HistoryStore`]), which writes it and
//! sends back a fresh [`HistorySnapshot`] for the History tab. Stored values
//! are never recalculated, even when prices change later.
//!
//! All database work happens on the history thread, never the UI thread.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender};

use anyhow::{Context, Result};
use eframe::egui;
use rusqlite::Connection;

use super::history::GroupSession;
use super::prices::{JitaPrices, PriceSide, Yields, unit_value};
use crate::static_data::{ItemKind, StaticData};

pub const FILE_NAME: &str = "mining.sqlite";

/// How many sessions the History tab lists.
const RECENT_SESSIONS: usize = 50;
const DAYS_LISTED: i64 = 14;
const WEEKS_LISTED: i64 = 8;
const SECONDS_PER_DAY: i64 = 86_400;

/// One ore of one pilot in a stored session.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredOre {
    pub ore: String,
    /// Units mined, as the log reports them.
    pub units: u64,
    pub m3: f64,
    pub isk: f64,
}

/// One pilot of a stored session.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredPilot {
    pub character: String,
    pub started_unix: i64,
    pub ended_unix: i64,
    pub cycles: u64,
    pub ores: Vec<StoredOre>,
}

impl StoredPilot {
    pub fn isk(&self) -> f64 {
        self.ores.iter().map(|ore| ore.isk).sum()
    }

    pub fn m3(&self) -> f64 {
        self.ores.iter().map(|ore| ore.m3).sum()
    }
}

/// What the session was mostly: shown as "Ice" or "Moon" in the History list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    Ore,
    Moon,
    Ice,
    Gas,
    /// Several kinds, none above 80% of the m³.
    Mixed,
}

impl SessionKind {
    fn to_db(self) -> &'static str {
        match self {
            SessionKind::Ore => "ore",
            SessionKind::Moon => "moon",
            SessionKind::Ice => "ice",
            SessionKind::Gas => "gas",
            SessionKind::Mixed => "mixed",
        }
    }

    fn from_db(text: &str) -> SessionKind {
        match text {
            "moon" => SessionKind::Moon,
            "ice" => SessionKind::Ice,
            "gas" => SessionKind::Gas,
            "mixed" => SessionKind::Mixed,
            _ => SessionKind::Ore,
        }
    }
}

/// One History entry as stored: all pilots ended with one "End session".
#[derive(Debug, Clone, PartialEq)]
pub struct StoredSession {
    /// Database id; 0 before it is stored.
    pub id: i64,
    pub started_unix: i64,
    pub ended_unix: i64,
    pub kind: SessionKind,
    /// The prices the value was frozen with, for reference.
    pub price_side: PriceSide,
    pub prices_fetched_unix: i64,
    pub pilots: Vec<StoredPilot>,
}

impl StoredSession {
    pub fn isk(&self) -> f64 {
        self.pilots.iter().map(StoredPilot::isk).sum()
    }

    pub fn m3(&self) -> f64 {
        self.pilots.iter().map(StoredPilot::m3).sum()
    }

    /// Units per ore over all pilots, largest m³ first, for the ore mix line.
    pub fn ore_mix(&self) -> Vec<(String, f64)> {
        let mut by_ore: BTreeMap<&str, f64> = BTreeMap::new();
        for ore in self.pilots.iter().flat_map(|p| &p.ores) {
            *by_ore.entry(&ore.ore).or_insert(0.0) += ore.m3;
        }
        let mut mix: Vec<(String, f64)> = by_ore
            .into_iter()
            .map(|(ore, m3)| (ore.to_string(), m3))
            .collect();
        mix.sort_by(|a, b| b.1.total_cmp(&a.1));
        mix
    }
}

/// Values a finished group session with the current prices. Ore without a
/// known volume or value counts as 0 m³ or 0 ISK.
pub fn value_group(
    group: &GroupSession,
    data: &StaticData,
    prices: &JitaPrices,
    side: PriceSide,
    yields: Yields,
) -> StoredSession {
    let mut m3_by_kind: BTreeMap<&'static str, f64> = BTreeMap::new();
    let pilots = group
        .pilots
        .iter()
        .map(|pilot| {
            let ores = pilot
                .mined_units
                .iter()
                .map(|(ore, &units)| {
                    let item = data.item_by_name(ore);
                    let m3 = item.map_or(0.0, |item| item.volume_m3 * units as f64);
                    let kind = match item.map(|item| item.kind) {
                        Some(ItemKind::MoonOre) => SessionKind::Moon,
                        Some(ItemKind::Ice) => SessionKind::Ice,
                        Some(ItemKind::Gas) => SessionKind::Gas,
                        _ => SessionKind::Ore,
                    };
                    *m3_by_kind.entry(kind.to_db()).or_insert(0.0) += m3;
                    let isk =
                        unit_value(ore, data, prices, side, yields).unwrap_or(0.0) * units as f64;
                    StoredOre {
                        ore: ore.clone(),
                        units,
                        m3,
                        isk,
                    }
                })
                .collect();
            StoredPilot {
                character: pilot.character.clone(),
                started_unix: pilot.started.unix_seconds,
                ended_unix: pilot.ended.unix_seconds,
                cycles: pilot.cycles,
                ores,
            }
        })
        .collect();

    let total: f64 = m3_by_kind.values().sum();
    let kind = m3_by_kind
        .iter()
        .find(|(_, m3)| total > 0.0 && **m3 / total >= 0.8)
        .map_or(SessionKind::Mixed, |(kind, _)| SessionKind::from_db(kind));

    StoredSession {
        id: 0,
        started_unix: group.started.unix_seconds,
        ended_unix: group.ended.unix_seconds,
        kind,
        price_side: side,
        prices_fetched_unix: prices.fetched_unix,
        pilots,
    }
}

/// ISK and m³ over some period.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Totals {
    pub isk: f64,
    pub m3: f64,
    pub sessions: u64,
}

/// Totals of one EVE day or week (by when the sessions started).
#[derive(Debug, Clone, PartialEq)]
pub struct PeriodTotal {
    /// Start of the day or week (Monday 00:00), as Unix seconds (UTC).
    pub start_unix: i64,
    pub totals: Totals,
}

/// Everything the History tab shows.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HistorySnapshot {
    /// Newest first.
    pub sessions: Vec<StoredSession>,
    pub today: Totals,
    pub yesterday: Totals,
    pub this_week: Totals,
    pub last_week: Totals,
    /// Newest first, only days or weeks with sessions.
    pub days: Vec<PeriodTotal>,
    pub weeks: Vec<PeriodTotal>,
}

/// The start of the EVE day (UTC) that `unix` falls in.
pub fn day_start(unix: i64) -> i64 {
    unix.div_euclid(SECONDS_PER_DAY) * SECONDS_PER_DAY
}

/// The start of the week (Monday 00:00 UTC) that `unix` falls in.
pub fn week_start(unix: i64) -> i64 {
    // 1970-01-01 was a Thursday, so Monday is 3 days before day 0.
    let day = unix.div_euclid(SECONDS_PER_DAY);
    let monday = day - (day + 3).rem_euclid(7);
    monday * SECONDS_PER_DAY
}

/// The schema version this code reads and writes.
const SCHEMA_VERSION: i64 = 1;

/// Creates the tables of a new file, in one transaction.
const CREATE_SCHEMA_V1: &str = "
    BEGIN;
    CREATE TABLE sessions (
        id                  INTEGER PRIMARY KEY,
        started             INTEGER NOT NULL,  -- Unix seconds, UTC
        ended               INTEGER NOT NULL,
        kind                TEXT NOT NULL,     -- ore, moon, ice, gas, mixed
        price_side          TEXT NOT NULL,     -- buy or sell
        prices_fetched      INTEGER NOT NULL   -- when the prices used were fetched
    );
    CREATE TABLE pilots (
        session_id  INTEGER NOT NULL REFERENCES sessions(id),
        character   TEXT NOT NULL,
        started     INTEGER NOT NULL,
        ended       INTEGER NOT NULL,
        cycles      INTEGER NOT NULL,
        PRIMARY KEY (session_id, character)
    );
    -- ISK is frozen when the session ends and never recalculated.
    CREATE TABLE pilot_ores (
        session_id  INTEGER NOT NULL REFERENCES sessions(id),
        character   TEXT NOT NULL,
        ore         TEXT NOT NULL,
        units       INTEGER NOT NULL,
        m3          REAL NOT NULL,
        isk         REAL NOT NULL,
        PRIMARY KEY (session_id, character, ore)
    );
    CREATE INDEX sessions_started ON sessions(started);
    PRAGMA user_version = 1;
    COMMIT;
";

/// The database. Only used on the history thread (and in tests).
pub struct Database {
    db: Connection,
}

impl Database {
    pub fn open(path: &Path) -> Result<Database> {
        let db = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        Database::with_connection(db)
    }

    fn with_connection(db: Connection) -> Result<Database> {
        // Foreign keys are off in SQLite unless switched on per connection.
        db.execute_batch("PRAGMA foreign_keys = ON;")
            .context("setting up the mining history database")?;
        // `user_version` is the schema version stored in the file (0 for a
        // new file). It is only ever raised, never written back down, so an
        // older Athena can't hide a newer schema from a later migration.
        let version: i64 = db
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .context("reading the mining history version")?;
        match version {
            0 => db
                .execute_batch(CREATE_SCHEMA_V1)
                .context("setting up the mining history database")?,
            SCHEMA_VERSION => {}
            newer => anyhow::bail!(
                "mining.sqlite was written by a newer Athena (version {newer}); \
                 please use that version"
            ),
        }
        Ok(Database { db })
    }

    /// Stores a session and returns its id.
    pub fn insert(&mut self, session: &StoredSession) -> Result<i64> {
        // A transaction makes the session appear completely or not at all.
        let tx = self.db.transaction()?;
        let side = match session.price_side {
            PriceSide::Buy => "buy",
            PriceSide::Sell => "sell",
        };
        tx.execute(
            "INSERT INTO sessions (started, ended, kind, price_side, prices_fetched)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            (
                session.started_unix,
                session.ended_unix,
                session.kind.to_db(),
                side,
                session.prices_fetched_unix,
            ),
        )?;
        let id = tx.last_insert_rowid();
        for pilot in &session.pilots {
            tx.execute(
                "INSERT INTO pilots (session_id, character, started, ended, cycles)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                (
                    id,
                    &pilot.character,
                    pilot.started_unix,
                    pilot.ended_unix,
                    pilot.cycles as i64,
                ),
            )?;
            for ore in &pilot.ores {
                tx.execute(
                    "INSERT INTO pilot_ores (session_id, character, ore, units, m3, isk)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    (
                        id,
                        &pilot.character,
                        &ore.ore,
                        ore.units as i64,
                        ore.m3,
                        ore.isk,
                    ),
                )?;
            }
        }
        tx.commit()?;
        Ok(id)
    }

    /// The newest `limit` sessions with their pilots and ores, newest first.
    pub fn recent_sessions(&self, limit: usize) -> Result<Vec<StoredSession>> {
        let mut query = self.db.prepare(
            "SELECT id, started, ended, kind, price_side, prices_fetched
             FROM sessions ORDER BY started DESC, id DESC LIMIT ?1",
        )?;
        let rows = query.query_map([limit as i64], |row| {
            let side: String = row.get(4)?;
            let kind: String = row.get(3)?;
            Ok(StoredSession {
                id: row.get(0)?,
                started_unix: row.get(1)?,
                ended_unix: row.get(2)?,
                kind: SessionKind::from_db(&kind),
                price_side: if side == "sell" {
                    PriceSide::Sell
                } else {
                    PriceSide::Buy
                },
                prices_fetched_unix: row.get(5)?,
                pilots: Vec::new(),
            })
        })?;
        let mut sessions = rows.collect::<rusqlite::Result<Vec<_>>>()?;

        let mut pilots_query = self.db.prepare(
            "SELECT character, started, ended, cycles FROM pilots
             WHERE session_id = ?1 ORDER BY started, character",
        )?;
        let mut ores_query = self.db.prepare(
            "SELECT ore, units, m3, isk FROM pilot_ores
             WHERE session_id = ?1 AND character = ?2 ORDER BY m3 DESC",
        )?;
        for session in &mut sessions {
            let pilots = pilots_query.query_map([session.id], |row| {
                Ok(StoredPilot {
                    character: row.get(0)?,
                    started_unix: row.get(1)?,
                    ended_unix: row.get(2)?,
                    cycles: row.get::<_, i64>(3)? as u64,
                    ores: Vec::new(),
                })
            })?;
            session.pilots = pilots.collect::<rusqlite::Result<Vec<_>>>()?;
            for pilot in &mut session.pilots {
                let ores = ores_query.query_map((session.id, &pilot.character), |row| {
                    Ok(StoredOre {
                        ore: row.get(0)?,
                        units: row.get::<_, i64>(1)? as u64,
                        m3: row.get(2)?,
                        isk: row.get(3)?,
                    })
                })?;
                pilot.ores = ores.collect::<rusqlite::Result<Vec<_>>>()?;
            }
        }
        Ok(sessions)
    }

    /// Totals of the sessions that started in `[from, to)`.
    pub fn totals_between(&self, from_unix: i64, to_unix: i64) -> Result<Totals> {
        let totals = self.db.query_row(
            "SELECT COALESCE(SUM(o.isk), 0), COALESCE(SUM(o.m3), 0), COUNT(DISTINCT s.id)
             FROM sessions s LEFT JOIN pilot_ores o ON o.session_id = s.id
             WHERE s.started >= ?1 AND s.started < ?2",
            (from_unix, to_unix),
            |row| {
                Ok(Totals {
                    isk: row.get(0)?,
                    m3: row.get(1)?,
                    sessions: row.get::<_, i64>(2)? as u64,
                })
            },
        )?;
        Ok(totals)
    }

    /// Everything the History tab needs, as of `now_unix`.
    pub fn snapshot(&self, now_unix: i64) -> Result<HistorySnapshot> {
        let today = day_start(now_unix);
        let this_week = week_start(now_unix);
        const WEEK: i64 = 7 * SECONDS_PER_DAY;

        let mut days = Vec::new();
        for i in 0..DAYS_LISTED {
            let start = today - i * SECONDS_PER_DAY;
            let totals = self.totals_between(start, start + SECONDS_PER_DAY)?;
            if totals.sessions > 0 {
                days.push(PeriodTotal {
                    start_unix: start,
                    totals,
                });
            }
        }
        let mut weeks = Vec::new();
        for i in 0..WEEKS_LISTED {
            let start = this_week - i * WEEK;
            let totals = self.totals_between(start, start + WEEK)?;
            if totals.sessions > 0 {
                weeks.push(PeriodTotal {
                    start_unix: start,
                    totals,
                });
            }
        }

        Ok(HistorySnapshot {
            sessions: self.recent_sessions(RECENT_SESSIONS)?,
            today: self.totals_between(today, today + SECONDS_PER_DAY)?,
            yesterday: self.totals_between(today - SECONDS_PER_DAY, today)?,
            this_week: self.totals_between(this_week, this_week + WEEK)?,
            last_week: self.totals_between(this_week - WEEK, this_week)?,
            days,
            weeks,
        })
    }
}

// ---------------------------------------------------------------------------
// The history thread

enum Command {
    Store(Box<StoredSession>),
    Refresh,
}

/// What the history thread sends back.
#[derive(Debug, Clone)]
pub enum HistoryUpdate {
    Snapshot(Box<HistorySnapshot>),
    Problem(String),
}

/// Handle to the history thread. Dropping it lets the thread finish the
/// writes it already has and then stop; [`HistoryStore::finish`] also waits
/// for that.
pub struct HistoryStore {
    /// `Option` so `finish` can drop it, which tells the thread to stop.
    commands: Option<Sender<Command>>,
    pub updates: Receiver<HistoryUpdate>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl HistoryStore {
    /// Opens `mining.sqlite` in `data_dir` on a new thread and sends a first
    /// snapshot.
    pub fn start(data_dir: &Path, egui_ctx: egui::Context) -> HistoryStore {
        let (commands, command_rx) = mpsc::channel();
        let (update_tx, updates) = mpsc::channel();
        let path = data_dir.join(FILE_NAME);
        let thread =
            std::thread::spawn(move || history_thread(&path, command_rx, update_tx, egui_ctx));
        let store = HistoryStore {
            commands: Some(commands),
            updates,
            thread: Some(thread),
        };
        store.refresh();
        store
    }

    /// Saves a finished session; a new snapshot follows.
    pub fn store(&self, session: StoredSession) {
        self.send(Command::Store(Box::new(session)));
    }

    /// Asks for a new snapshot, e.g. after midnight so "Today" moves on.
    pub fn refresh(&self) {
        self.send(Command::Refresh);
    }

    /// Waits until every session handed over so far is written. Called when
    /// Athena closes, so an "End session" just before closing isn't lost.
    pub fn finish(&mut self) {
        self.commands = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }

    fn send(&self, command: Command) {
        if let Some(commands) = &self.commands {
            // An error only means the thread already stopped (it reported
            // why as a `Problem`).
            let _ = commands.send(command);
        }
    }
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn history_thread(
    path: &Path,
    commands: Receiver<Command>,
    updates: Sender<HistoryUpdate>,
    egui_ctx: egui::Context,
) {
    let send = |update: HistoryUpdate| {
        let _ = updates.send(update);
        egui_ctx.request_repaint();
    };
    let mut db = match Database::open(path) {
        Ok(db) => db,
        Err(e) => {
            send(HistoryUpdate::Problem(format!(
                "Mining history can't be saved: {e:#}"
            )));
            return;
        }
    };
    // Ends when the module drops its sender.
    for command in commands {
        if let Command::Store(session) = command
            && let Err(e) = db.insert(&session)
        {
            send(HistoryUpdate::Problem(format!(
                "A finished session could not be saved: {e:#}"
            )));
        }
        match db.snapshot(now_unix()) {
            Ok(snapshot) => send(HistoryUpdate::Snapshot(Box::new(snapshot))),
            Err(e) => send(HistoryUpdate::Problem(format!(
                "Mining history can't be read: {e:#}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::modules::mining::gamelog::LogTime;
    use crate::modules::mining::prices::MarketPrice;
    use crate::modules::mining::session::FinishedSession;
    use crate::static_data::tests::sample;

    // 2026-11-08 (a Sunday) 00:00 UTC.
    const SUNDAY: i64 = 1_794_096_000;

    fn pilot(name: &str, start: i64, end: i64, ore: &str, units: u64) -> FinishedSession {
        let mut mined = BTreeMap::new();
        mined.insert(ore.to_string(), units);
        FinishedSession {
            character: name.to_string(),
            started: LogTime {
                unix_seconds: start,
            },
            ended: LogTime { unix_seconds: end },
            mined_units: mined,
            cycles: 10,
        }
    }

    fn prices() -> JitaPrices {
        let mut by_type = HashMap::new();
        by_type.insert(
            34,
            MarketPrice {
                buy: Some(4.0),
                sell: Some(5.0),
            },
        );
        JitaPrices {
            fetched_unix: SUNDAY - 600,
            by_type,
        }
    }

    fn session(start: i64) -> StoredSession {
        let group = GroupSession::from_pilots(vec![
            pilot("Pilot Alpha", start, start + 3_600, "Veldspar", 10_000),
            pilot("Pilot Bravo", start + 60, start + 3_000, "Veldspar", 5_000),
        ])
        .unwrap();
        value_group(
            &group,
            &sample(),
            &prices(),
            PriceSide::Buy,
            Yields::default(),
        )
    }

    #[test]
    fn valuing_freezes_isk_and_m3() {
        let s = session(SUNDAY + 3_600);
        assert_eq!(s.kind, SessionKind::Ore);
        assert_eq!(s.pilots.len(), 2);
        // 10,000 Veldspar x 4 Tritanium x 90.63% x 4 ISK.
        assert!((s.pilots[0].isk() - 10_000.0 * 4.0 * 0.9063 * 4.0).abs() < 1e-6);
        assert!((s.m3() - 1_500.0).abs() < 1e-9);
        assert_eq!(s.prices_fetched_unix, SUNDAY - 600);
    }

    #[test]
    fn mostly_ice_is_an_ice_session() {
        let group = GroupSession::from_pilots(vec![
            pilot("Pilot Alpha", SUNDAY, SUNDAY + 60, "White Glaze", 100),
            pilot("Pilot Bravo", SUNDAY, SUNDAY + 60, "Veldspar", 10),
        ])
        .unwrap();
        let s = value_group(
            &group,
            &sample(),
            &prices(),
            PriceSide::Buy,
            Yields::default(),
        );
        assert_eq!(s.kind, SessionKind::Ice);
    }

    #[test]
    fn store_and_read_back() {
        let mut db = Database::with_connection(Connection::open_in_memory().unwrap()).unwrap();
        let original = session(SUNDAY + 3_600);
        let id = db.insert(&original).unwrap();
        let read = db.recent_sessions(10).unwrap();
        assert_eq!(read.len(), 1);
        let mut expected = original;
        expected.id = id;
        assert_eq!(read[0], expected);
    }

    #[test]
    fn opening_again_keeps_the_data() {
        let dir = std::env::temp_dir().join(format!("athena-reopen-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(FILE_NAME);
        Database::open(&path)
            .unwrap()
            .insert(&session(SUNDAY))
            .unwrap();
        let db = Database::open(&path).unwrap();
        assert_eq!(db.recent_sessions(10).unwrap().len(), 1);
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn newer_schema_is_refused_and_left_alone() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("PRAGMA user_version = 2;").unwrap();
        let error = Database::with_connection(db).err().unwrap();
        assert!(format!("{error:#}").contains("newer Athena"));
    }

    #[test]
    fn finish_waits_for_pending_writes() {
        let dir = std::env::temp_dir().join(format!("athena-history-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let mut store = HistoryStore::start(&dir, egui::Context::default());
        store.store(session(SUNDAY + 3_600));
        store.finish();
        // Sending after `finish` is ignored instead of failing.
        store.refresh();

        let db = Database::open(&dir.join(FILE_NAME)).unwrap();
        assert_eq!(db.recent_sessions(10).unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn day_and_week_starts() {
        assert_eq!(day_start(SUNDAY + 5_000), SUNDAY);
        // The Sunday belongs to the week that started on Monday Nov 2.
        assert_eq!(week_start(SUNDAY + 5_000), SUNDAY - 6 * SECONDS_PER_DAY);
        // Monday Nov 9 starts a new week.
        let monday = SUNDAY + SECONDS_PER_DAY;
        assert_eq!(week_start(monday + 10), monday);
    }

    #[test]
    fn snapshot_totals() {
        let mut db = Database::with_connection(Connection::open_in_memory().unwrap()).unwrap();
        // Today (Sunday), yesterday (Saturday), and the week before.
        db.insert(&session(SUNDAY + 3_600)).unwrap();
        db.insert(&session(SUNDAY - 20_000)).unwrap();
        db.insert(&session(SUNDAY - 8 * SECONDS_PER_DAY)).unwrap();
        let one = session(0).isk();

        let snap = db.snapshot(SUNDAY + 10_000).unwrap();
        assert_eq!(snap.sessions.len(), 3);
        // Newest first.
        assert_eq!(snap.sessions[0].started_unix, SUNDAY + 3_600);
        assert!((snap.today.isk - one).abs() < 1e-6);
        assert_eq!(snap.yesterday.sessions, 1);
        assert_eq!(snap.this_week.sessions, 2);
        assert_eq!(snap.last_week.sessions, 1);
        assert_eq!(snap.days.len(), 3);
        assert_eq!(snap.days[0].start_unix, SUNDAY);
        assert_eq!(snap.weeks.len(), 2);
        assert!((snap.weeks[0].totals.isk - 2.0 * one).abs() < 1e-6);
    }
}
