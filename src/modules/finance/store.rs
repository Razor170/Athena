//! `finance.sqlite`: the wallet journal, market trades, last balances,
//! loyalty points and looked-up names, kept in the data folder.
//!
//! ESI only hands out the last 30 days of a wallet journal, so Athena
//! saves every entry it sees and the history grows from the first day
//! Finance runs. Entries never change once EVE wrote them, so they're only
//! ever added (`INSERT OR IGNORE`), never updated.
//!
//! Only the worker thread opens this file; the UI gets copies through the
//! worker's channel. No tokens ever go in here.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::{Connection, params};

use super::data::{JournalEntry, LoyaltyPoints, MarketTrade};

pub const FILE_NAME: &str = "finance.sqlite";

/// Bump when the tables change, and add the upgrade to `open`.
const SCHEMA_VERSION: i64 = 1;

pub struct Store {
    db: Connection,
}

/// Everything saved, read at startup.
#[derive(Debug, Default)]
pub struct Saved {
    pub journal: Vec<JournalEntry>,
    pub trades: Vec<MarketTrade>,
    /// Character id -> (balance, when it was fetched, Unix seconds).
    pub balances: HashMap<u64, (f64, i64)>,
    pub loyalty: HashMap<u64, Vec<LoyaltyPoints>>,
    pub names: HashMap<i64, String>,
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
            CREATE TABLE IF NOT EXISTS journal (
                character_id    INTEGER NOT NULL,
                id              INTEGER NOT NULL,
                date            INTEGER NOT NULL,  -- Unix seconds
                ref_type        TEXT NOT NULL,
                amount          REAL NOT NULL,
                balance         REAL,
                first_party_id  INTEGER,
                second_party_id INTEGER,
                context_id      INTEGER,
                context_id_type TEXT,
                description     TEXT NOT NULL,
                reason          TEXT,
                PRIMARY KEY (character_id, id)
            );
            CREATE TABLE IF NOT EXISTS trades (
                character_id   INTEGER NOT NULL,
                transaction_id INTEGER NOT NULL,
                journal_ref_id INTEGER NOT NULL,
                date           INTEGER NOT NULL,
                type_id        INTEGER NOT NULL,
                quantity       INTEGER NOT NULL,
                unit_price     REAL NOT NULL,
                is_buy         INTEGER NOT NULL,
                client_id      INTEGER NOT NULL,
                location_id    INTEGER NOT NULL,
                PRIMARY KEY (character_id, transaction_id)
            );
            CREATE TABLE IF NOT EXISTS balances (
                character_id INTEGER PRIMARY KEY,
                balance      REAL NOT NULL,
                fetched      INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS loyalty (
                character_id   INTEGER NOT NULL,
                corporation_id INTEGER NOT NULL,
                points         INTEGER NOT NULL,
                PRIMARY KEY (character_id, corporation_id)
            );
            -- Names of characters, corporations, items, stations and
            -- structures, from ESI.
            CREATE TABLE IF NOT EXISTS names (
                id   INTEGER PRIMARY KEY,
                name TEXT NOT NULL
            );
            ",
        )?;
        db.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}"))?;
        Ok(Store { db })
    }

    /// Reads everything. Runs once when the worker starts.
    pub fn load(&self) -> Result<Saved> {
        let mut saved = Saved::default();

        let mut query = self.db.prepare(
            "SELECT character_id, id, date, ref_type, amount, balance, first_party_id,
                    second_party_id, context_id, context_id_type, description, reason
             FROM journal ORDER BY date DESC, id DESC",
        )?;
        let rows = query.query_map([], |row| {
            Ok(JournalEntry {
                character_id: row.get::<_, i64>(0)? as u64,
                id: row.get(1)?,
                when: row.get(2)?,
                ref_type: row.get(3)?,
                amount: row.get(4)?,
                balance: row.get(5)?,
                first_party_id: row.get(6)?,
                second_party_id: row.get(7)?,
                context_id: row.get(8)?,
                context_id_type: row.get(9)?,
                description: row.get(10)?,
                reason: row.get(11)?,
            })
        })?;
        for row in rows {
            saved.journal.push(row?);
        }

        let mut query = self.db.prepare(
            "SELECT character_id, transaction_id, journal_ref_id, date, type_id, quantity,
                    unit_price, is_buy, client_id, location_id FROM trades",
        )?;
        let rows = query.query_map([], trade_from_row)?;
        for row in rows {
            saved.trades.push(row?);
        }

        let mut query = self
            .db
            .prepare("SELECT character_id, balance, fetched FROM balances")?;
        let rows = query.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)? as u64,
                row.get::<_, f64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        for row in rows {
            let (id, balance, fetched) = row?;
            saved.balances.insert(id, (balance, fetched));
        }

        let mut query = self.db.prepare(
            "SELECT character_id, corporation_id, points FROM loyalty
             ORDER BY character_id, points DESC",
        )?;
        let rows = query.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)? as u64,
                LoyaltyPoints {
                    corporation_id: row.get(1)?,
                    loyalty_points: row.get(2)?,
                },
            ))
        })?;
        for row in rows {
            let (id, points) = row?;
            saved.loyalty.entry(id).or_default().push(points);
        }

        let mut query = self.db.prepare("SELECT id, name FROM names")?;
        let rows = query.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        for row in rows {
            let (id, name) = row?;
            saved.names.insert(id, name);
        }
        Ok(saved)
    }

    /// Saves journal entries and returns the ones that weren't saved yet.
    pub fn add_journal(&mut self, entries: Vec<JournalEntry>) -> Result<Vec<JournalEntry>> {
        let tx = self.db.transaction()?;
        let mut new = Vec::new();
        {
            let mut insert = tx.prepare(
                "INSERT OR IGNORE INTO journal (character_id, id, date, ref_type, amount,
                    balance, first_party_id, second_party_id, context_id, context_id_type,
                    description, reason)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            )?;
            for e in entries {
                let added = insert.execute(params![
                    e.character_id as i64,
                    e.id,
                    e.when,
                    e.ref_type,
                    e.amount,
                    e.balance,
                    e.first_party_id,
                    e.second_party_id,
                    e.context_id,
                    e.context_id_type,
                    e.description,
                    e.reason,
                ])?;
                if added == 1 {
                    new.push(e);
                }
            }
        }
        tx.commit()?;
        Ok(new)
    }

    /// Saves market trades and returns the ones that weren't saved yet.
    pub fn add_trades(&mut self, trades: Vec<MarketTrade>) -> Result<Vec<MarketTrade>> {
        let tx = self.db.transaction()?;
        let mut new = Vec::new();
        {
            let mut insert = tx.prepare(
                "INSERT OR IGNORE INTO trades (character_id, transaction_id, journal_ref_id,
                    date, type_id, quantity, unit_price, is_buy, client_id, location_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            )?;
            for t in trades {
                let added = insert.execute(params![
                    t.character_id as i64,
                    t.transaction_id,
                    t.journal_ref_id,
                    t.when,
                    t.type_id,
                    t.quantity,
                    t.unit_price,
                    t.is_buy,
                    t.client_id,
                    t.location_id,
                ])?;
                if added == 1 {
                    new.push(t);
                }
            }
        }
        tx.commit()?;
        Ok(new)
    }

    pub fn set_balance(&self, character_id: u64, balance: f64, fetched: i64) -> Result<()> {
        self.db.execute(
            "INSERT OR REPLACE INTO balances (character_id, balance, fetched) VALUES (?1, ?2, ?3)",
            params![character_id as i64, balance, fetched],
        )?;
        Ok(())
    }

    /// Replaces a character's loyalty points with the new list.
    pub fn set_loyalty(&mut self, character_id: u64, points: &[LoyaltyPoints]) -> Result<()> {
        let tx = self.db.transaction()?;
        tx.execute(
            "DELETE FROM loyalty WHERE character_id = ?1",
            [character_id as i64],
        )?;
        {
            let mut insert = tx.prepare(
                "INSERT OR REPLACE INTO loyalty (character_id, corporation_id, points)
                 VALUES (?1, ?2, ?3)",
            )?;
            for p in points {
                insert.execute(params![
                    character_id as i64,
                    p.corporation_id,
                    p.loyalty_points
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn add_names(&mut self, names: &[(i64, String)]) -> Result<()> {
        let tx = self.db.transaction()?;
        {
            let mut insert =
                tx.prepare("INSERT OR REPLACE INTO names (id, name) VALUES (?1, ?2)")?;
            for (id, name) in names {
                insert.execute(params![id, name])?;
            }
        }
        tx.commit()?;
        Ok(())
    }
}

fn trade_from_row(row: &rusqlite::Row) -> rusqlite::Result<MarketTrade> {
    Ok(MarketTrade {
        character_id: row.get::<_, i64>(0)? as u64,
        transaction_id: row.get(1)?,
        journal_ref_id: row.get(2)?,
        when: row.get(3)?,
        type_id: row.get(4)?,
        quantity: row.get(5)?,
        unit_price: row.get(6)?,
        is_buy: row.get(7)?,
        client_id: row.get(8)?,
        location_id: row.get(9)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(character_id: u64, id: i64, amount: f64) -> JournalEntry {
        JournalEntry {
            character_id,
            id,
            when: 1_794_000_000 + id,
            ref_type: "player_donation".to_string(),
            amount,
            balance: Some(1000.0),
            first_party_id: Some(5),
            second_party_id: Some(6),
            context_id: None,
            context_id_type: None,
            description: "A donation".to_string(),
            reason: Some("thanks".to_string()),
        }
    }

    #[test]
    fn journal_entries_are_added_once() {
        let mut store = Store::in_memory();
        let new = store
            .add_journal(vec![entry(1, 10, 5.0), entry(1, 11, -5.0)])
            .unwrap();
        assert_eq!(new.len(), 2);
        // The same entries again, plus the same id in another wallet (one
        // of your characters paying another).
        let new = store
            .add_journal(vec![entry(1, 10, 5.0), entry(2, 10, -5.0)])
            .unwrap();
        assert_eq!(new, [entry(2, 10, -5.0)]);
        let saved = store.load().unwrap();
        assert_eq!(saved.journal.len(), 3);
        // Newest first.
        assert_eq!(saved.journal[0].id, 11);
        assert_eq!(saved.journal[0].reason.as_deref(), Some("thanks"));
    }

    #[test]
    fn loyalty_is_replaced_balances_and_names_saved() {
        let mut store = Store::in_memory();
        let points = |corp, lp| LoyaltyPoints {
            corporation_id: corp,
            loyalty_points: lp,
        };
        store
            .set_loyalty(1, &[points(100, 5), points(200, 50)])
            .unwrap();
        store.set_loyalty(1, &[points(200, 60)]).unwrap();
        store.set_balance(1, 123.0, 99).unwrap();
        store.add_names(&[(5, "Someone".to_string())]).unwrap();
        let saved = store.load().unwrap();
        assert_eq!(saved.loyalty[&1], [points(200, 60)]);
        assert_eq!(saved.balances[&1], (123.0, 99));
        assert_eq!(saved.names[&5], "Someone");
    }
}
