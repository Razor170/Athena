//! Item types, groups, categories and NPC stations for the Assets module,
//! read from `static.sqlite` (schema 11, see `xtask/src/assets.rs`).
//!
//! The file has about 54,000 item types. Loading them all would cost
//! several megabytes of RAM for every Athena run, so the worker reads only
//! the types the characters own, when their assets arrive. The ~1,600
//! groups and ~50 categories are small and loaded whole, for the filter
//! editor.

use std::collections::HashMap;

use anyhow::Result;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

/// The SDE category of ships.
pub const CATEGORY_SHIP: i64 = 6;
/// The SDE category of blueprints (originals and copies).
pub const CATEGORY_BLUEPRINT: i64 = 9;

/// One item type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TypeInfo {
    pub name: String,
    pub group_id: i64,
    pub category_id: i64,
    /// m³ of one unit as it is (an assembled ship or container).
    pub volume: f64,
    /// m³ of one unit packaged.
    pub packaged_volume: f64,
    /// Sold on the market, so it can have a price.
    pub market: bool,
}

/// An SDE group, such as "Carrier".
#[derive(Debug, Clone, PartialEq)]
pub struct GroupInfo {
    pub group_id: i64,
    pub category_id: i64,
    pub name: String,
}

/// All groups and categories, sorted by name, for the filter editor.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Catalog {
    pub groups: Vec<GroupInfo>,
    /// (category id, name).
    pub categories: Vec<(i64, String)>,
}

impl Catalog {
    pub fn group_name(&self, group_id: i64) -> Option<&str> {
        self.groups
            .iter()
            .find(|g| g.group_id == group_id)
            .map(|g| g.name.as_str())
    }

    pub fn category_name(&self, category_id: i64) -> Option<&str> {
        self.categories
            .iter()
            .find(|(id, _)| *id == category_id)
            .map(|(_, name)| name.as_str())
    }
}

/// An NPC station: its system and name.
#[derive(Debug, Clone, PartialEq)]
pub struct Station {
    pub system_id: i64,
    pub name: String,
}

/// Reads the groups players see and the categories. Leaves out
/// SDE-internal categories ("#System", "Owner").
pub fn catalog(db: &Connection) -> Result<Catalog> {
    let mut query =
        db.prepare("SELECT group_id, category_id, name FROM type_groups WHERE published = 1")?;
    let mut groups: Vec<GroupInfo> = query
        .query_map([], |row| {
            Ok(GroupInfo {
                group_id: row.get(0)?,
                category_id: row.get(1)?,
                name: row.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    groups.retain(|g| !g.name.is_empty());
    groups.sort_by_key(|g| g.name.to_lowercase());
    let mut query = db.prepare("SELECT category_id, name FROM type_categories")?;
    let mut categories: Vec<(i64, String)> = query
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    categories.retain(|(id, name)| !name.starts_with('#') && *id != 1);
    categories.sort_by_key(|(_, name)| name.to_lowercase());
    Ok(Catalog { groups, categories })
}

/// The types with these ids that the file knows.
pub fn types(db: &Connection, ids: &[i64]) -> Result<HashMap<i64, TypeInfo>> {
    let mut query = db.prepare_cached(
        "SELECT t.name, t.group_id, COALESCE(g.category_id, 0), t.volume,
                t.packaged_volume, t.market
         FROM types t LEFT JOIN type_groups g ON g.group_id = t.group_id
         WHERE t.type_id = ?1",
    )?;
    let mut out = HashMap::new();
    for &id in ids {
        let mut rows = query.query([id])?;
        if let Some(row) = rows.next()? {
            out.insert(
                id,
                TypeInfo {
                    name: row.get(0)?,
                    group_id: row.get(1)?,
                    category_id: row.get(2)?,
                    volume: row.get(3)?,
                    packaged_volume: row.get(4)?,
                    market: row.get(5)?,
                },
            );
        }
    }
    Ok(out)
}

/// The NPC stations with these ids that the file knows.
pub fn stations(db: &Connection, ids: &[i64]) -> Result<HashMap<i64, Station>> {
    let mut query =
        db.prepare_cached("SELECT system_id, name FROM stations WHERE station_id = ?1")?;
    let mut out = HashMap::new();
    for &id in ids {
        let mut rows = query.query([id])?;
        if let Some(row) = rows.next()? {
            out.insert(
                id,
                Station {
                    system_id: row.get(0)?,
                    name: row.get(1)?,
                },
            );
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::static_data::tests::sample_db;

    #[test]
    fn reads_types_with_their_category() {
        let db = sample_db();
        let types = types(&db, &[587, 34, 999_999]).unwrap();
        assert_eq!(types.len(), 2);
        let rifter = &types[&587];
        assert_eq!(rifter.name, "Rifter");
        assert_eq!(rifter.category_id, CATEGORY_SHIP);
        assert_eq!(rifter.packaged_volume, 2500.0);
        assert!(rifter.market);
    }

    #[test]
    fn reads_stations_and_catalog() {
        let db = sample_db();
        let stations = stations(&db, &[60003760, 1]).unwrap();
        assert_eq!(stations[&60003760].system_id, 30000142);
        assert_eq!(stations.len(), 1);
        let catalog = catalog(&db).unwrap();
        assert_eq!(catalog.group_name(547), Some("Carrier"));
        assert_eq!(catalog.category_name(6), Some("Ship"));
        // Sorted by name.
        assert_eq!(catalog.groups[0].name, "Carrier");
    }
}
