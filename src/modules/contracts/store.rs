//! `contracts.sqlite`: every contract seen, its last [`Snapshot`], the
//! changelog and looked-up names, kept in the data folder.
//!
//! ESI only lists contracts from the last 30 days (plus ones still being
//! hauled), so Athena keeps its own copy and the changelog grows forever.
//! A contract is stored as JSON, so new fields don't need a new table.
//!
//! Only the worker thread opens this file; the UI gets copies through the
//! worker's channel. No tokens ever go in here.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};

use super::changes::{ChangeKind, Snapshot};
use super::data::Contract;

pub const FILE_NAME: &str = "contracts.sqlite";

/// Bump when the tables change, and add the upgrade to `setup`.
const SCHEMA_VERSION: i64 = 1;

/// One changelog entry.
#[derive(Debug, Clone, PartialEq)]
pub struct LogEntry {
    /// Given by the database when saved.
    pub id: i64,
    /// When it happened (Unix seconds).
    pub at: i64,
    pub contract_id: i64,
    pub kind: ChangeKind,
    pub amount: Option<f64>,
    /// It happened while Athena was closed.
    pub offline: bool,
    /// The user saw it (clicked it or "Mark all seen").
    pub seen: bool,
}

/// Everything saved, read at startup.
#[derive(Debug, Default)]
pub struct Saved {
    pub contracts: Vec<(Contract, Option<Snapshot>)>,
    /// Newest first.
    pub log: Vec<LogEntry>,
    pub names: HashMap<i64, String>,
    /// Characters whose contracts were fetched before. A new character's
    /// existing contracts are taken in without changelog entries.
    pub known_characters: Vec<u64>,
}

pub struct Store {
    db: Connection,
}

impl Store {
    pub fn file_path(data_dir: &Path) -> PathBuf {
        data_dir.join(FILE_NAME)
    }

    /// Opens (or creates) the file. File I/O: worker thread only.
    pub fn open(path: &Path) -> Result<Store> {
        let db = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        Store::setup(db)
    }

    #[cfg(test)]
    pub fn in_memory() -> Store {
        Store::setup(Connection::open_in_memory().unwrap()).unwrap()
    }

    fn setup(db: Connection) -> Result<Store> {
        let version: i64 = db.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version > SCHEMA_VERSION {
            anyhow::bail!(
                "{FILE_NAME} was written by a newer Athena (version {version}); \
                 update Athena to read it"
            );
        }
        db.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS contracts (
                contract_id INTEGER PRIMARY KEY,
                data        TEXT NOT NULL,  -- the Contract as JSON
                snapshot    TEXT            -- the last Snapshot as JSON
            );
            CREATE TABLE IF NOT EXISTS changelog (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                at          INTEGER NOT NULL,  -- Unix seconds
                contract_id INTEGER NOT NULL,
                kind        TEXT NOT NULL,
                amount      REAL,
                offline     INTEGER NOT NULL,
                seen        INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS names (
                id   INTEGER PRIMARY KEY,
                name TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS meta (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            ",
        )?;
        db.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}"))?;
        Ok(Store { db })
    }

    pub fn load(&self) -> Result<Saved> {
        let mut saved = Saved::default();
        let mut stmt = self
            .db
            .prepare("SELECT data, snapshot FROM contracts ORDER BY contract_id")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })?;
        for row in rows {
            let (data, snapshot) = row?;
            // A row this version can't read is skipped, not fatal.
            let Ok(contract) = serde_json::from_str::<Contract>(&data) else {
                continue;
            };
            let snapshot = snapshot.and_then(|text| serde_json::from_str(&text).ok());
            saved.contracts.push((contract, snapshot));
        }

        let mut stmt = self.db.prepare(
            "SELECT id, at, contract_id, kind, amount, offline, seen FROM changelog
             ORDER BY at DESC, id DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<f64>>(4)?,
                row.get::<_, bool>(5)?,
                row.get::<_, bool>(6)?,
            ))
        })?;
        for row in rows {
            let (id, at, contract_id, kind, amount, offline, seen) = row?;
            let Some(kind) = ChangeKind::from_key(&kind) else {
                continue;
            };
            saved.log.push(LogEntry {
                id,
                at,
                contract_id,
                kind,
                amount,
                offline,
                seen,
            });
        }

        let mut stmt = self.db.prepare("SELECT id, name FROM names")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        for row in rows {
            let (id, name) = row?;
            saved.names.insert(id, name);
        }

        saved.known_characters = self
            .meta("known_characters")?
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        Ok(saved)
    }

    pub fn save_contract(&self, contract: &Contract, snapshot: Option<&Snapshot>) -> Result<()> {
        let data = serde_json::to_string(contract)?;
        let snapshot = snapshot.map(serde_json::to_string).transpose()?;
        self.db.execute(
            "INSERT INTO contracts (contract_id, data, snapshot) VALUES (?1, ?2, ?3)
             ON CONFLICT(contract_id) DO UPDATE SET data = ?2, snapshot = ?3",
            params![contract.id, data, snapshot],
        )?;
        Ok(())
    }

    /// Saves new entries and fills in their ids.
    pub fn add_log(&self, entries: &mut [LogEntry]) -> Result<()> {
        for entry in entries {
            self.db.execute(
                "INSERT INTO changelog (at, contract_id, kind, amount, offline, seen)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    entry.at,
                    entry.contract_id,
                    entry.kind.key(),
                    entry.amount,
                    entry.offline,
                    entry.seen
                ],
            )?;
            entry.id = self.db.last_insert_rowid();
        }
        Ok(())
    }

    /// Starts a transaction: everything saved until `commit` is written to
    /// disk at once (dropping it without `commit` undoes it). It borrows
    /// the store only shared (`unchecked`), so the store's other methods
    /// can still be called while it is open; they all use this one
    /// connection, so they are part of it.
    pub fn begin(&self) -> Result<rusqlite::Transaction<'_>> {
        Ok(self.db.unchecked_transaction()?)
    }

    pub fn mark_seen(&self, ids: &[i64]) -> Result<()> {
        for id in ids {
            self.db
                .execute("UPDATE changelog SET seen = 1 WHERE id = ?1", params![id])?;
        }
        Ok(())
    }

    pub fn mark_all_seen(&self) -> Result<()> {
        self.db
            .execute("UPDATE changelog SET seen = 1 WHERE seen = 0", [])?;
        Ok(())
    }

    pub fn add_names(&self, names: &[(i64, String)]) -> Result<()> {
        for (id, name) in names {
            self.db.execute(
                "INSERT OR REPLACE INTO names (id, name) VALUES (?1, ?2)",
                params![id, name],
            )?;
        }
        Ok(())
    }

    pub fn set_known_characters(&self, ids: &[u64]) -> Result<()> {
        self.set_meta("known_characters", &serde_json::to_string(ids)?)
    }

    fn meta(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .db
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()?)
    }

    fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.db.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
            params![key, value],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::contracts::data::{Stage, tests::esi_courier};

    #[test]
    fn saves_and_loads_everything() {
        let store = Store::in_memory();
        let contract = Contract::from_esi(esi_courier(), 1).unwrap();
        let snapshot = Snapshot {
            stage: Stage::Waiting,
            bids: 0,
            soon: false,
        };
        store.save_contract(&contract, Some(&snapshot)).unwrap();
        // Saving again replaces the row.
        store.save_contract(&contract, Some(&snapshot)).unwrap();

        let entry = |at: i64, offline: bool| LogEntry {
            id: 0,
            at,
            contract_id: contract.id,
            kind: ChangeKind::Issued,
            amount: None,
            offline,
            seen: false,
        };
        let mut entries = vec![entry(100, true), entry(200, false)];
        store.add_log(&mut entries).unwrap();
        assert!(entries[0].id > 0 && entries[1].id > entries[0].id);
        store.mark_seen(&[entries[0].id]).unwrap();
        store.add_names(&[(1, "Aria".into())]).unwrap();
        store.set_known_characters(&[1, 2]).unwrap();

        let saved = store.load().unwrap();
        assert_eq!(saved.contracts, vec![(contract, Some(snapshot))]);
        // Newest first.
        assert_eq!(saved.log.len(), 2);
        assert_eq!(saved.log[0].at, 200);
        assert!(!saved.log[0].seen);
        assert!(saved.log[1].seen && saved.log[1].offline);
        assert_eq!(saved.names[&1], "Aria");
        assert_eq!(saved.known_characters, vec![1, 2]);

        store.mark_all_seen().unwrap();
        assert!(store.load().unwrap().log.iter().all(|e| e.seen));
    }
}
