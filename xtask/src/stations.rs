//! The stations part of `cargo xtask sde`: NPC corporations and their
//! stations that have a loyalty point store, for the Finance module's
//! "nearest LP store".
//!
//! The SDE doesn't hold station names, so they're built the way the game
//! builds them:
//!
//! ```text
//! <system> <planet in Roman numerals>[ - Moon <n>] - <corporation>[ <operation>]
//! Jita IV - Moon 4 - Caldari Navy Assembly Plant
//! ```

use std::collections::BTreeMap;

use anyhow::{Result, bail};
use serde::Deserialize;

use crate::{Name, english};

/// The station service id of "Loyalty Point Store" (`stationServices.jsonl`).
const SERVICE_LP_STORE: i64 = 25;

/// One line of `npcCorporations.jsonl`.
#[derive(Deserialize)]
pub struct SdeCorporation {
    #[serde(rename = "_key")]
    pub id: i64,
    pub name: Option<Name>,
}

/// One line of `stationOperations.jsonl`.
#[derive(Deserialize)]
pub struct SdeOperation {
    #[serde(rename = "_key")]
    pub id: i64,
    #[serde(rename = "operationName")]
    pub name: Option<Name>,
    #[serde(default)]
    pub services: Vec<i64>,
}

/// One line of `npcStations.jsonl`.
#[derive(Deserialize)]
pub struct SdeStation {
    #[serde(rename = "_key")]
    pub id: i64,
    #[serde(rename = "solarSystemID")]
    pub system_id: i64,
    #[serde(rename = "ownerID")]
    pub owner_id: i64,
    #[serde(rename = "operationID")]
    pub operation_id: i64,
    /// The planet's number in its system (IV in "Jita IV").
    #[serde(rename = "celestialIndex")]
    pub celestial_index: Option<i64>,
    /// The moon's number, when the station orbits a moon.
    #[serde(rename = "orbitIndex")]
    pub orbit_index: Option<i64>,
    #[serde(rename = "useOperationName", default)]
    pub use_operation_name: bool,
}

/// A station with an LP store, ready to be written.
pub struct Station {
    pub system_id: i64,
    pub corporation_id: i64,
    pub name: String,
}

/// The NPC corporations (id -> name) and their stations with an LP store
/// (station id -> station). `system_names` are the map's systems; stations
/// in other systems are left out.
pub fn build(
    corporations: Vec<SdeCorporation>,
    operations: Vec<SdeOperation>,
    stations: Vec<SdeStation>,
    system_names: &BTreeMap<i64, String>,
) -> Result<(BTreeMap<i64, String>, BTreeMap<i64, Station>)> {
    let corporations: BTreeMap<i64, String> = corporations
        .into_iter()
        .filter_map(|c| Some((c.id, english(&c.name)?)))
        .collect();
    let operations: BTreeMap<i64, SdeOperation> =
        operations.into_iter().map(|o| (o.id, o)).collect();

    let mut out = BTreeMap::new();
    for s in stations {
        let Some(operation) = operations.get(&s.operation_id) else {
            continue;
        };
        if !operation.services.contains(&SERVICE_LP_STORE) {
            continue;
        }
        let (Some(system), Some(corporation), Some(planet)) = (
            system_names.get(&s.system_id),
            corporations.get(&s.owner_id),
            s.celestial_index,
        ) else {
            continue;
        };
        let mut name = format!("{system} {}", roman(planet));
        if let Some(moon) = s.orbit_index {
            name.push_str(&format!(" - Moon {moon}"));
        }
        name.push_str(&format!(" - {corporation}"));
        if s.use_operation_name
            && let Some(operation_name) = english(&operation.name)
        {
            name.push(' ');
            name.push_str(&operation_name);
        }
        out.insert(
            s.id,
            Station {
                system_id: s.system_id,
                corporation_id: s.owner_id,
                name,
            },
        );
    }

    // Sanity checks, so a changed SDE format fails loudly.
    match out.get(&60003760) {
        Some(jita) if jita.name == "Jita IV - Moon 4 - Caldari Navy Assembly Plant" => {}
        Some(jita) => bail!(
            "Jita 4-4 came out as \"{}\"; has the SDE format changed?",
            jita.name
        ),
        None => bail!("Jita 4-4 not found among LP stations; has the SDE format changed?"),
    }
    if out.len() < 3000 {
        bail!(
            "only {} stations with an LP store found; has the SDE format changed?",
            out.len()
        );
    }
    Ok((corporations, out))
}

/// 1 -> "I", 4 -> "IV", 14 -> "XIV". Planets go up to about 20.
fn roman(mut n: i64) -> String {
    const DIGITS: [(i64, &str); 7] = [
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];
    let mut out = String::new();
    for (value, text) in DIGITS {
        while n >= value {
            out.push_str(text);
            n -= value;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roman_numerals() {
        assert_eq!(roman(1), "I");
        assert_eq!(roman(4), "IV");
        assert_eq!(roman(9), "IX");
        assert_eq!(roman(14), "XIV");
        assert_eq!(roman(19), "XIX");
        assert_eq!(roman(40), "XL");
    }
}
