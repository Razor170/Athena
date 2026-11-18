//! The Assets part of `cargo xtask sde`: every item type with its group,
//! category and volumes, and the names of all groups and categories, so
//! the Assets module can name, measure and filter anything a character
//! owns. NPC station names come from `stations.rs`.
//!
//! Unpublished types are kept too: old or special items (event gifts,
//! retired modules) can still sit in a hangar.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, bail};
use serde::Deserialize;

use crate::{Name, english};

/// One line of `categories.jsonl`.
#[derive(Deserialize)]
pub struct SdeCategory {
    #[serde(rename = "_key")]
    pub id: i64,
    pub name: Option<Name>,
}

/// An item type, ready to be written.
pub struct AssetType {
    pub name: String,
    pub group_id: i64,
    /// m³ of one unit as it is (an assembled ship or container).
    pub volume: f64,
    /// m³ of one unit packaged (what a stack in a hangar takes).
    pub packaged_volume: f64,
    /// Sold on the market (has a market group), so it can have a price.
    pub market: bool,
}

/// What [`build`] takes for each type (filled from `types.jsonl`).
pub struct TypeIn {
    pub id: i64,
    pub group_id: i64,
    pub name: Option<String>,
    pub volume: Option<f64>,
    pub packaged_volume: Option<f64>,
    pub market_group_id: Option<i64>,
}

/// Everything the Assets module needs from the SDE.
pub struct AssetData {
    pub types: BTreeMap<i64, AssetType>,
    /// Group id -> (category id, name).
    pub groups: BTreeMap<i64, (i64, String)>,
    /// The groups players see (no NPC groups).
    pub published_groups: BTreeSet<i64>,
    /// Category id -> name.
    pub categories: BTreeMap<i64, String>,
}

pub fn build(
    types: Vec<TypeIn>,
    groups: &BTreeMap<i64, (i64, String)>,
    published_groups: &BTreeSet<i64>,
    categories: Vec<SdeCategory>,
) -> Result<AssetData> {
    let categories: BTreeMap<i64, String> = categories
        .into_iter()
        .filter_map(|c| Some((c.id, english(&c.name)?)))
        .collect();
    let mut out = BTreeMap::new();
    for t in types {
        let Some(name) = t.name else {
            continue;
        };
        let volume = t.volume.unwrap_or(0.0);
        out.insert(
            t.id,
            AssetType {
                name,
                group_id: t.group_id,
                volume,
                packaged_volume: t.packaged_volume.unwrap_or(volume),
                market: t.market_group_id.is_some(),
            },
        );
    }

    // Sanity checks, so a changed SDE format fails loudly.
    match out.get(&587) {
        Some(rifter) if rifter.name == "Rifter" && rifter.packaged_volume == 2500.0 => {}
        _ => {
            bail!("Rifter (type 587) not found with 2,500 m³ packaged; has the SDE format changed?")
        }
    }
    if out.len() < 30_000 || categories.len() < 30 || groups.len() < 1000 {
        bail!(
            "only {} types, {} groups and {} categories found; has the SDE format changed?",
            out.len(),
            groups.len(),
            categories.len()
        );
    }
    Ok(AssetData {
        types: out,
        groups: groups.clone(),
        published_groups: published_groups.clone(),
        categories,
    })
}

/// The `types.jsonl` fields [`build`] needs, from the full row.
pub fn type_in(
    id: i64,
    group_id: i64,
    name: &Option<Name>,
    volume: Option<f64>,
    packaged_volume: Option<f64>,
    market_group_id: Option<i64>,
) -> TypeIn {
    TypeIn {
        id,
        group_id,
        name: english(name),
        volume,
        packaged_volume,
        market_group_id,
    }
}
