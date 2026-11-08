//! Static game data from EVE's static data export (SDE): ore, ice, moon ore
//! and gas types with their volumes, what they reprocess into, and which
//! type each one compresses into.
//!
//! The data comes from `static.sqlite`, which `cargo xtask sde` builds from
//! the official SDE (see `xtask/src/main.rs`). CI builds it and puts it in the
//! release zip next to `Athena.exe`; it is not committed to the repo.
//!
//! [`StaticData::load`] reads the whole file into memory (a few hundred
//! rows), so lookups afterwards are plain map lookups. Loading is file I/O,
//! so call it on a background thread, not the UI thread.

// Nothing outside the tests uses this yet; the Mining Supervisor panel will.
// Remove this line when it is wired in.
#![allow(dead_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// The file name, next to the exe.
pub const FILE_NAME: &str = "static.sqlite";

/// The `schema_version` this code can read. Must match the xtask.
const SCHEMA_VERSION: &str = "1";

/// What kind of thing an item is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    /// Asteroid belt ore (Veldspar, Mordunium, ...), compressed or not.
    Ore,
    /// Moon ore (Bitumens, Coesite, ...).
    MoonOre,
    Ice,
    Gas,
    /// Something ore reprocesses into: minerals, moon materials, ice
    /// products. Only needed to look up prices.
    Material,
}

impl ItemKind {
    fn from_db(text: &str) -> Option<ItemKind> {
        match text {
            "ore" => Some(ItemKind::Ore),
            "moon" => Some(ItemKind::MoonOre),
            "ice" => Some(ItemKind::Ice),
            "gas" => Some(ItemKind::Gas),
            "material" => Some(ItemKind::Material),
            _ => None,
        }
    }
}

/// One item type.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub type_id: i64,
    /// English name, as the game logs and belt scans write it.
    pub name: String,
    /// Volume of one unit in m³.
    pub volume_m3: f64,
    /// Reprocessing works in portions of this many units (100 for most ore,
    /// 1 for ice).
    pub portion_size: i64,
    /// The SDE group, e.g. "Veldspar", "Ice", "Rare Moon Asteroids".
    pub group_name: String,
    pub kind: ItemKind,
}

/// All static data Athena uses, in memory.
#[derive(Debug, Clone, Default)]
pub struct StaticData {
    /// The SDE build the file was made from, shown in Settings.
    pub sde_build: String,
    items: HashMap<i64, Item>,
    id_by_name: HashMap<String, i64>,
    /// type id -> (material id, quantity per portion at 100% yield).
    reprocess: HashMap<i64, Vec<(i64, i64)>>,
    /// type id -> its compressed type id.
    compressed: HashMap<i64, i64>,
}

impl StaticData {
    /// Where to look for `static.sqlite`: next to the exe (the release zip),
    /// then the current folder (handy for `cargo run` from the repo).
    pub fn find_file() -> Option<PathBuf> {
        let next_to_exe = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join(FILE_NAME)));
        let in_current_dir = Some(PathBuf::from(FILE_NAME));
        [next_to_exe, in_current_dir]
            .into_iter()
            .flatten()
            .find(|path| path.is_file())
    }

    /// Reads the whole file. Does file I/O: not on the UI thread.
    pub fn load(path: &Path) -> Result<StaticData> {
        // Read-only, so a copy in a read-only folder (Program Files) works too.
        let db =
            rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .with_context(|| format!("opening {}", path.display()))?;
        StaticData::from_connection(&db)
    }

    /// Reads everything from an open database. Split from `load` so tests
    /// can use an in-memory database.
    pub fn from_connection(db: &rusqlite::Connection) -> Result<StaticData> {
        let meta = |key: &str| -> Result<String> {
            db.query_row("SELECT value FROM meta WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .with_context(|| format!("static data has no {key}"))
        };
        let version = meta("schema_version")?;
        if version != SCHEMA_VERSION {
            bail!(
                "static data has schema version {version}, this Athena needs {SCHEMA_VERSION}; rebuild it with `cargo xtask sde`"
            );
        }
        let mut data = StaticData {
            sde_build: meta("sde_build")?,
            ..StaticData::default()
        };

        let mut query = db.prepare(
            "SELECT type_id, name, volume_m3, portion_size, group_name, kind FROM items",
        )?;
        // `query_map` runs the closure once per row and gives back an
        // iterator of `Result`s, one per row.
        let rows = query.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, f64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?;
        for row in rows {
            let (type_id, name, volume_m3, portion_size, group_name, kind) = row?;
            let kind = ItemKind::from_db(&kind)
                .with_context(|| format!("unknown item kind {kind} for {name}"))?;
            data.id_by_name.insert(name.clone(), type_id);
            data.items.insert(
                type_id,
                Item {
                    type_id,
                    name,
                    volume_m3,
                    portion_size: portion_size.max(1),
                    group_name,
                    kind,
                },
            );
        }

        let mut query = db.prepare("SELECT type_id, material_id, quantity FROM reprocess")?;
        let rows = query.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        for row in rows {
            let (type_id, material_id, quantity) = row?;
            data.reprocess
                .entry(type_id)
                .or_default()
                .push((material_id, quantity));
        }

        let mut query = db.prepare("SELECT type_id, compressed_type_id FROM compressed")?;
        let rows = query.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?;
        for row in rows {
            let (type_id, compressed_id) = row?;
            data.compressed.insert(type_id, compressed_id);
        }

        if data.items.is_empty() {
            bail!("static data contains no items");
        }
        Ok(data)
    }

    pub fn item(&self, type_id: i64) -> Option<&Item> {
        self.items.get(&type_id)
    }

    /// Looks up an item by its English name, as written in the logs.
    pub fn item_by_name(&self, name: &str) -> Option<&Item> {
        self.items.get(self.id_by_name.get(name)?)
    }

    /// Every ore, ice, moon ore and gas type (not the materials).
    pub fn minable_items(&self) -> impl Iterator<Item = &Item> {
        self.items
            .values()
            .filter(|item| item.kind != ItemKind::Material)
    }

    /// The compressed version of an item, if it has one.
    pub fn compressed_of(&self, type_id: i64) -> Option<&Item> {
        self.items.get(self.compressed.get(&type_id)?)
    }

    /// What one unit of `type_id` reprocesses into at 100% yield, as
    /// (material id, quantity per unit). Quantities are fractions because
    /// reprocessing works in portions (usually 100 units).
    ///
    /// Raw gas can't be reprocessed, and some ore has no list of its own in
    /// the SDE. Since compression is 1:1 in units, those use the list of
    /// their compressed type: compressed gas "reprocesses" back into raw gas,
    /// which is the decompression that the gas yield applies to.
    ///
    /// `None` if neither the item nor its compressed type has a list.
    pub fn materials_per_unit(&self, type_id: i64) -> Option<Vec<(i64, f64)>> {
        let source = if self.reprocess.contains_key(&type_id) {
            type_id
        } else {
            *self.compressed.get(&type_id)?
        };
        let materials = self.reprocess.get(&source)?;
        let portion = self.items.get(&source)?.portion_size as f64;
        Some(
            materials
                .iter()
                .map(|&(material_id, quantity)| (material_id, quantity as f64 / portion))
                .collect(),
        )
    }

    /// Every material id any item reprocesses into: the types whose Jita
    /// prices are needed to value ore.
    pub fn material_ids(&self) -> Vec<i64> {
        let mut ids: Vec<i64> = self
            .reprocess
            .values()
            .flatten()
            .map(|&(material_id, _)| material_id)
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A tiny in-memory static database with real SDE numbers for a few
    /// items. Other tests can use it too.
    pub fn sample_db() -> rusqlite::Connection {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch(
            "
            CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
            CREATE TABLE items (type_id INTEGER PRIMARY KEY, name TEXT NOT NULL,
                volume_m3 REAL NOT NULL, portion_size INTEGER NOT NULL,
                group_name TEXT NOT NULL, kind TEXT NOT NULL);
            CREATE TABLE reprocess (type_id INTEGER NOT NULL, material_id INTEGER NOT NULL,
                quantity INTEGER NOT NULL, PRIMARY KEY (type_id, material_id));
            CREATE TABLE compressed (type_id INTEGER PRIMARY KEY,
                compressed_type_id INTEGER NOT NULL);

            INSERT INTO meta VALUES ('schema_version', '1'), ('sde_build', '3670638');
            INSERT INTO items VALUES
                (1230, 'Veldspar', 0.1, 100, 'Veldspar', 'ore'),
                (62516, 'Compressed Veldspar', 0.001, 100, 'Veldspar', 'ore'),
                (16265, 'White Glaze', 1000.0, 1, 'Ice', 'ice'),
                (30370, 'Fullerite-C50', 1.0, 1, 'Harvestable Cloud', 'gas'),
                (62399, 'Compressed Fullerite-C50', 0.1, 1, 'Compressed Gas', 'gas'),
                (34, 'Tritanium', 0.01, 1, 'Mineral', 'material'),
                (16272, 'Heavy Water', 0.4, 1, 'Ice Product', 'material');
            INSERT INTO reprocess VALUES
                (1230, 34, 400),
                (62516, 34, 400),
                (16265, 16272, 69),
                (62399, 30370, 1);
            INSERT INTO compressed VALUES (1230, 62516), (30370, 62399);
            ",
        )
        .unwrap();
        db
    }

    pub fn sample() -> StaticData {
        StaticData::from_connection(&sample_db()).unwrap()
    }

    #[test]
    fn loads_items_by_name() {
        let data = sample();
        assert_eq!(data.sde_build, "3670638");
        let veldspar = data.item_by_name("Veldspar").unwrap();
        assert_eq!(veldspar.volume_m3, 0.1);
        assert_eq!(veldspar.kind, ItemKind::Ore);
        assert_eq!(
            data.compressed_of(veldspar.type_id).unwrap().name,
            "Compressed Veldspar"
        );
        assert!(data.item_by_name("Mystery Ore").is_none());
        assert_eq!(data.minable_items().count(), 5);
    }

    #[test]
    fn materials_per_unit_uses_portions() {
        let data = sample();
        // 400 Tritanium per 100 Veldspar = 4 per unit.
        assert_eq!(data.materials_per_unit(1230), Some(vec![(34, 4.0)]));
        // Ice reprocesses per single unit.
        assert_eq!(data.materials_per_unit(16265), Some(vec![(16272, 69.0)]));
    }

    #[test]
    fn raw_gas_uses_its_compressed_type() {
        let data = sample();
        // Raw gas has no list; compressed gas decompresses 1:1.
        assert_eq!(data.materials_per_unit(30370), Some(vec![(30370, 1.0)]));
        // Tritanium itself can't be reprocessed.
        assert_eq!(data.materials_per_unit(34), None);
    }

    #[test]
    fn material_ids_are_unique() {
        assert_eq!(sample().material_ids(), vec![34, 16272, 30370]);
    }

    #[test]
    fn wrong_schema_version_is_refused() {
        let db = sample_db();
        db.execute(
            "UPDATE meta SET value = '99' WHERE key = 'schema_version'",
            [],
        )
        .unwrap();
        let error = StaticData::from_connection(&db).unwrap_err().to_string();
        assert!(error.contains("schema version 99"), "{error}");
    }

    #[test]
    fn missing_file_is_an_error() {
        assert!(StaticData::load(Path::new("does-not-exist/static.sqlite")).is_err());
    }
}
