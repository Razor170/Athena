//! The Planetary Industry part of `cargo xtask sde`, for the Planetary
//! module:
//!
//! - every planetary commodity (P0 to P4) with its volume, to work out how
//!   full a launchpad or storage facility is,
//! - every colony structure (command center, extractor control unit,
//!   factory, storage facility, launchpad) with how much it holds,
//! - every factory schematic: its cycle time, inputs and output.

use std::collections::BTreeMap;

use anyhow::{Result, bail};
use serde::Deserialize;

use crate::{Name, english};

/// Commodity groups and their tier (P0 = raw resource ... P4 = advanced).
const COMMODITY_GROUPS: [(i64, i64); 7] = [
    (1032, 0), // Planet Solid - Raw Resource
    (1033, 0), // Planet Liquid-Gas - Raw Resource
    (1035, 0), // Planet Organic - Raw Resource
    (1042, 1), // Basic Commodities - Tier 1
    (1034, 2), // Refined Commodities - Tier 2
    (1040, 3), // Specialized Commodities - Tier 3
    (1041, 4), // Advanced Commodities - Tier 4
];

/// Structure groups and the kind name the app reads.
const STRUCTURE_GROUPS: [(i64, &str); 5] = [
    (1027, "command_center"),
    (1063, "extractor"), // Extractor Control Units
    (1028, "factory"),   // Processors
    (1029, "storage"),
    (1030, "launchpad"), // Spaceports
];

/// The fields of a `types.jsonl` line this part needs.
pub struct PiType {
    pub id: i64,
    pub group_id: i64,
    pub name: Option<String>,
    pub volume: Option<f64>,
    pub capacity: Option<f64>,
    pub published: bool,
}

/// True for the groups this part reads, so the caller can keep only those
/// lines of the (big) types list.
pub fn wanted_group(group_id: i64) -> bool {
    COMMODITY_GROUPS.iter().any(|(g, _)| *g == group_id)
        || STRUCTURE_GROUPS.iter().any(|(g, _)| *g == group_id)
}

/// One line of `planetSchematics.jsonl`.
#[derive(Deserialize)]
pub struct SdeSchematic {
    #[serde(rename = "_key")]
    pub id: i64,
    #[serde(rename = "cycleTime")]
    pub cycle_time: i64,
    pub name: Option<Name>,
    #[serde(default)]
    pub types: Vec<SdeSchematicType>,
}

#[derive(Deserialize)]
pub struct SdeSchematicType {
    #[serde(rename = "_key")]
    pub type_id: i64,
    #[serde(rename = "isInput")]
    pub is_input: bool,
    pub quantity: i64,
}

/// A commodity, ready to be written.
pub struct Commodity {
    pub name: String,
    pub volume_m3: f64,
    pub tier: i64,
}

/// A colony structure, ready to be written.
pub struct Structure {
    pub name: String,
    pub kind: &'static str,
    /// 0 for structures that hold nothing (factories, extractors).
    pub capacity_m3: f64,
}

/// A schematic, ready to be written.
pub struct Schematic {
    pub name: String,
    pub cycle_secs: i64,
    /// (type id, quantity, is input).
    pub types: Vec<(i64, i64, bool)>,
}

pub struct PiData {
    pub commodities: BTreeMap<i64, Commodity>,
    pub structures: BTreeMap<i64, Structure>,
    pub schematics: BTreeMap<i64, Schematic>,
}

pub fn build(types: &[PiType], schematics: Vec<SdeSchematic>) -> Result<PiData> {
    let mut commodities = BTreeMap::new();
    let mut structures = BTreeMap::new();
    for t in types {
        let Some(name) = t.name.clone() else {
            continue;
        };
        if let Some(&(_, tier)) = COMMODITY_GROUPS.iter().find(|(g, _)| *g == t.group_id) {
            if !t.published {
                continue;
            }
            commodities.insert(
                t.id,
                Commodity {
                    name,
                    volume_m3: t.volume.unwrap_or(0.0),
                    tier,
                },
            );
        } else if let Some(&(_, kind)) = STRUCTURE_GROUPS.iter().find(|(g, _)| *g == t.group_id) {
            // Unpublished structures (the old per-planet command center
            // levels) still exist on colonies, so they're kept too.
            structures.insert(
                t.id,
                Structure {
                    name,
                    kind,
                    capacity_m3: t.capacity.unwrap_or(0.0),
                },
            );
        }
    }

    let schematics: BTreeMap<i64, Schematic> = schematics
        .into_iter()
        .filter_map(|s| {
            Some((
                s.id,
                Schematic {
                    name: english(&s.name)?,
                    cycle_secs: s.cycle_time,
                    types: s
                        .types
                        .iter()
                        .map(|t| (t.type_id, t.quantity, t.is_input))
                        .collect(),
                },
            ))
        })
        .collect();

    // Sanity checks, so a changed SDE format fails loudly.
    for must_have in [
        "Aqueous Liquids",
        "Water",
        "Coolant",
        "Robotics",
        "Nano-Factory",
    ] {
        if !commodities.values().any(|c| c.name == must_have) {
            bail!("commodity {must_have} not found; has the SDE format changed?");
        }
    }
    let launchpad = structures
        .values()
        .find(|s| s.name == "Temperate Launchpad");
    if launchpad.is_none_or(|s| s.capacity_m3 < 1000.0) {
        bail!("no Temperate Launchpad with a capacity found; has the SDE format changed?");
    }
    if schematics.len() < 60 || !schematics.values().any(|s| s.name == "Coolant") {
        bail!(
            "only {} schematics found; has the SDE format changed?",
            schematics.len()
        );
    }
    Ok(PiData {
        commodities,
        structures,
        schematics,
    })
}
