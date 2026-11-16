//! Turns a character's ESI assets into ships with their fits, for "Open
//! ship…". Pure logic: the fetching is in `worker`.
//!
//! In ESI's asset list an assembled ship is one item; what is fitted to it
//! or carried in it are items whose `location_id` is the ship's `item_id`,
//! with a `location_flag` that says where: `LoSlot0`…`LoSlot7`, `MedSlot…`,
//! `HiSlot…`, `RigSlot…`, `SubSystemSlot…`, `DroneBay`, `FighterBay`,
//! `FighterTube…`, `Cargo`, special holds and so on.

use std::collections::{BTreeMap, HashMap};

use serde::Deserialize;

use super::eft::Section;

/// One line of `/characters/{id}/assets` (only the fields we need).
#[derive(Debug, Clone, Deserialize)]
pub struct EsiAsset {
    pub item_id: i64,
    pub type_id: i64,
    pub location_id: i64,
    pub location_flag: String,
    /// "station", "solar_system" or "item" (inside something, or in a
    /// player structure: then `location_id` is the structure).
    pub location_type: String,
    pub quantity: i64,
    #[serde(default)]
    pub is_singleton: bool,
}

/// Where a ship is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Place {
    /// An NPC station or a player structure, by its id.
    Station(i64),
    /// In space in this solar system (the ship being flown).
    InSpace(i64),
}

/// A ship found in the assets.
#[derive(Debug, Clone, PartialEq)]
pub struct Ship {
    pub item_id: i64,
    pub type_id: i64,
    pub place: Place,
    /// The ship it is stored in (ship maintenance bay or fleet hangar), if
    /// any: its item id.
    pub inside: Option<i64>,
    /// What is fitted and carried, by section: (type id, count).
    pub contents: BTreeMap<Section, Vec<(i64, u64)>>,
}

impl Ship {
    /// "23 modules · 46 drones · 7 cargo", or "unfitted".
    pub fn summary(&self) -> String {
        let count = |sections: &[Section], by_units: bool| -> u64 {
            sections
                .iter()
                .filter_map(|s| self.contents.get(s))
                .flatten()
                .map(|(_, n)| if by_units { *n } else { 1 })
                .sum()
        };
        let modules = count(
            &[
                Section::Low,
                Section::Med,
                Section::High,
                Section::Rigs,
                Section::Subsystems,
            ],
            true,
        );
        let drones = count(&[Section::Drones], true);
        // Cargo counts kinds of items, not units (2,000 charges are one).
        let cargo = count(&[Section::Cargo], false);
        let mut parts = Vec::new();
        if modules > 0 {
            parts.push(format!("{modules} modules"));
        }
        if drones > 0 {
            parts.push(format!("{drones} drones"));
        }
        if cargo > 0 {
            parts.push(format!("{cargo} cargo"));
        }
        if parts.is_empty() {
            "unfitted".to_string()
        } else {
            parts.join(" · ")
        }
    }

    /// Every type id in the ship, for name lookups.
    pub fn content_types(&self) -> impl Iterator<Item = i64> + '_ {
        self.contents
            .values()
            .flatten()
            .map(|(type_id, _)| *type_id)
    }
}

/// The section an item with this `location_flag` belongs to, or `None`
/// for places that are not part of the fit (a ship maintenance bay holds
/// other ships, not fittings).
fn section_of(flag: &str) -> Option<Section> {
    let section = if flag.starts_with("LoSlot") {
        Section::Low
    } else if flag.starts_with("MedSlot") {
        Section::Med
    } else if flag.starts_with("HiSlot") {
        Section::High
    } else if flag.starts_with("RigSlot") {
        Section::Rigs
    } else if flag.starts_with("SubSystemSlot") {
        Section::Subsystems
    } else if flag == "DroneBay" || flag == "FighterBay" || flag.starts_with("FighterTube") {
        Section::Drones
    } else if flag == "Cargo" || flag == "FleetHangar" || flag.starts_with("Specialized") {
        // Fuel bay, ore hold and the like count as cargo, as in EFT.
        Section::Cargo
    } else {
        return None;
    };
    Some(section)
}

fn is_slot(section: Section) -> bool {
    matches!(
        section,
        Section::Low | Section::Med | Section::High | Section::Rigs | Section::Subsystems
    )
}

/// Finds every assembled ship in `assets`. `is_ship` says if a type id is
/// a ship (from the SDE).
pub fn find_ships(assets: &[EsiAsset], is_ship: &dyn Fn(i64) -> bool) -> Vec<Ship> {
    let by_id: HashMap<i64, &EsiAsset> = assets.iter().map(|a| (a.item_id, a)).collect();
    // Items inside each item, so a ship's contents are found without
    // going through the whole list for every ship.
    let mut inside: HashMap<i64, Vec<&EsiAsset>> = HashMap::new();
    for asset in assets {
        if asset.location_type == "item" {
            inside.entry(asset.location_id).or_default().push(asset);
        }
    }

    let mut ships = Vec::new();
    for asset in assets {
        // Packaged ships (not singletons) have no fit.
        if !asset.is_singleton || !is_ship(asset.type_id) {
            continue;
        }
        let Some((place, carrier)) = place_of(asset, &by_id, is_ship) else {
            continue;
        };
        let items = inside.get(&asset.item_id).map_or(&[][..], |v| v.as_slice());
        ships.push(Ship {
            item_id: asset.item_id,
            type_id: asset.type_id,
            place,
            inside: carrier,
            contents: contents(items),
        });
    }
    ships.sort_by_key(|s| s.item_id);
    ships
}

/// Where `asset` is, and the ship it is stored in (if any). Follows
/// containers up to a station, structure or solar system.
fn place_of(
    asset: &EsiAsset,
    by_id: &HashMap<i64, &EsiAsset>,
    is_ship: &dyn Fn(i64) -> bool,
) -> Option<(Place, Option<i64>)> {
    let mut current = asset;
    let mut carrier = None;
    // A depth limit, so a broken list can never loop forever.
    for _ in 0..10 {
        match current.location_type.as_str() {
            "station" => return Some((Place::Station(current.location_id), carrier)),
            "solar_system" => return Some((Place::InSpace(current.location_id), carrier)),
            _ => match by_id.get(&current.location_id) {
                // Inside another of our items: a ship or a container.
                Some(parent) => {
                    if carrier.is_none() && is_ship(parent.type_id) {
                        carrier = Some(parent.item_id);
                    }
                    current = parent;
                }
                // Inside something that isn't ours: a player structure.
                None => return Some((Place::Station(current.location_id), carrier)),
            },
        }
    }
    None
}

/// Sorts a ship's items into sections. In a slot only the module counts:
/// a loaded charge or script shares the slot's flag but is never a
/// singleton, while a fitted module always is.
fn contents(items: &[&EsiAsset]) -> BTreeMap<Section, Vec<(i64, u64)>> {
    let mut by_flag: BTreeMap<&str, Vec<&EsiAsset>> = BTreeMap::new();
    for item in items {
        by_flag
            .entry(item.location_flag.as_str())
            .or_default()
            .push(item);
    }
    let mut out: BTreeMap<Section, Vec<(i64, u64)>> = BTreeMap::new();
    for (flag, items) in by_flag {
        let Some(section) = section_of(flag) else {
            continue;
        };
        let kept: Vec<&EsiAsset> = if is_slot(section) {
            let modules: Vec<&EsiAsset> =
                items.iter().copied().filter(|a| a.is_singleton).collect();
            if modules.is_empty() {
                items.into_iter().take(1).collect()
            } else {
                modules
            }
        } else {
            items
        };
        let lines = out.entry(section).or_default();
        for item in kept {
            let count = if is_slot(section) {
                1
            } else {
                u64::try_from(item.quantity.max(1)).unwrap_or(1)
            };
            match lines.iter_mut().find(|(t, _)| *t == item.type_id) {
                Some((_, n)) => *n += count,
                None => lines.push((item.type_id, count)),
            }
        }
    }
    out
}

/// Writes a ship as EFT text, so it can be shown and compared like a
/// pasted fit. `type_name` gives item names; unknown ones are written as
/// "Type 12345". Empty racks get an "[Empty … slot]" line so the racks
/// after them stay in their place.
pub fn to_eft(
    ship: &Ship,
    hull: &str,
    name: &str,
    type_name: &dyn Fn(i64) -> Option<String>,
) -> String {
    let name_of = |type_id: i64| type_name(type_id).unwrap_or_else(|| format!("Type {type_id}"));
    let mut out = format!("[{hull}, {name}]\n");
    let racks = [
        (Section::Low, "Low"),
        (Section::Med, "Med"),
        (Section::High, "High"),
        (Section::Rigs, "Rig"),
    ];
    for (section, label) in racks {
        out.push('\n');
        match ship.contents.get(&section) {
            Some(lines) if !lines.is_empty() => {
                for (type_id, count) in lines {
                    for _ in 0..*count {
                        out.push_str(&name_of(*type_id));
                        out.push('\n');
                    }
                }
            }
            _ => out.push_str(&format!("[Empty {label} slot]\n")),
        }
    }
    if let Some(lines) = ship.contents.get(&Section::Subsystems) {
        out.push('\n');
        for (type_id, _) in lines {
            out.push_str(&name_of(*type_id));
            out.push('\n');
        }
    }
    for section in [Section::Drones, Section::Cargo] {
        if let Some(lines) = ship.contents.get(&section) {
            out.push_str("\n\n");
            for (type_id, count) in lines {
                out.push_str(&format!("{} x{count}\n", name_of(*type_id)));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::fittings::eft;

    const SALVATION: i64 = 77777;
    const THANATOS: i64 = 23911;

    fn asset(
        item_id: i64,
        type_id: i64,
        location_id: i64,
        flag: &str,
        kind: &str,
        quantity: i64,
        singleton: bool,
    ) -> EsiAsset {
        EsiAsset {
            item_id,
            type_id,
            location_id,
            location_flag: flag.to_string(),
            location_type: kind.to_string(),
            quantity,
            is_singleton: singleton,
        }
    }

    fn is_ship(type_id: i64) -> bool {
        type_id == SALVATION || type_id == THANATOS
    }

    fn names(type_id: i64) -> Option<String> {
        let name = match type_id {
            1 => "Damage Control II",
            2 => "25000mm Steel Plates II",
            3 => "Capital Micro Jump Drive",
            4 => "Heavy Energy Neutralizer II",
            5 => "Navy Cap Booster 3200",
            6 => "Capital Trimark Armor Pump I",
            7 => "Templar I",
            8 => "Mobile Depot",
            _ => return None,
        };
        Some(name.to_string())
    }

    /// A Salvation in a structure with a neut (loaded with a charge in the
    /// same slot), drones, cargo, and a Thanatos holding it in its SMB.
    fn sample() -> Vec<EsiAsset> {
        vec![
            asset(100, SALVATION, 1_035_000_000_000, "Hangar", "item", 1, true),
            asset(101, 1, 100, "LoSlot0", "item", 1, true),
            asset(102, 2, 100, "LoSlot1", "item", 1, true),
            asset(103, 2, 100, "LoSlot2", "item", 1, true),
            asset(104, 3, 100, "MedSlot0", "item", 1, true),
            asset(105, 4, 100, "HiSlot0", "item", 1, true),
            asset(106, 5, 100, "HiSlot0", "item", 1, false),
            asset(107, 6, 100, "RigSlot0", "item", 1, true),
            asset(108, 7, 100, "FighterBay", "item", 6, false),
            asset(109, 7, 100, "FighterBay", "item", 2, true),
            asset(110, 8, 100, "Cargo", "item", 2, false),
            asset(111, 5, 100, "Cargo", "item", 60, false),
            // A packaged Thanatos (no fit) and one in space carrying a
            // Salvation in its ship maintenance bay.
            asset(200, THANATOS, 60003760, "Hangar", "station", 1, false),
            asset(300, THANATOS, 30000142, "Hangar", "solar_system", 1, true),
            asset(301, SALVATION, 300, "ShipHangar", "item", 1, true),
        ]
    }

    #[test]
    fn finds_assembled_ships_and_where_they_are() {
        let ships = find_ships(&sample(), &is_ship);
        let found: Vec<(i64, Place, Option<i64>)> = ships
            .iter()
            .map(|s| (s.item_id, s.place, s.inside))
            .collect();
        assert_eq!(
            found,
            vec![
                (100, Place::Station(1_035_000_000_000), None),
                (300, Place::InSpace(30000142), None),
                (301, Place::InSpace(30000142), Some(300)),
            ]
        );
    }

    #[test]
    fn contents_skip_loaded_charges_and_sum_drones() {
        let ships = find_ships(&sample(), &is_ship);
        let salvation = &ships[0];
        assert_eq!(salvation.contents[&Section::High], vec![(4, 1)]);
        assert_eq!(salvation.contents[&Section::Low], vec![(1, 1), (2, 2)]);
        assert_eq!(salvation.contents[&Section::Drones], vec![(7, 8)]);
        assert_eq!(salvation.contents[&Section::Cargo], vec![(8, 2), (5, 60)]);
        assert_eq!(salvation.summary(), "6 modules · 8 drones · 2 cargo");
        // The Thanatos's SMB is not part of its fit.
        assert_eq!(ships[1].summary(), "unfitted");
    }

    #[test]
    fn eft_text_reads_back_the_same() {
        let ships = find_ships(&sample(), &is_ship);
        let text = to_eft(&ships[0], "Salvation", "Kearsarge", &names);
        let is_drone = |n: &str| n == "Templar I";
        let fit = eft::parse_with_drones(&text, &is_drone);
        assert_eq!(fit.hull, "Salvation");
        assert_eq!(fit.name, "Kearsarge");
        assert_eq!(fit.count(Section::Low, "25000mm Steel Plates II"), 2);
        assert_eq!(fit.count(Section::Med, "Capital Micro Jump Drive"), 1);
        assert_eq!(fit.count(Section::High, "Heavy Energy Neutralizer II"), 1);
        assert_eq!(fit.count(Section::High, "Navy Cap Booster 3200"), 0);
        assert_eq!(fit.count(Section::Rigs, "Capital Trimark Armor Pump I"), 1);
        assert_eq!(fit.count(Section::Drones, "Templar I"), 8);
        assert_eq!(fit.count(Section::Cargo, "Navy Cap Booster 3200"), 60);
        // Without game data the block order still works.
        let plain = eft::parse(&text);
        assert_eq!(plain.count(Section::Drones, "Templar I"), 8);
        assert_eq!(plain.count(Section::Cargo, "Mobile Depot"), 2);
    }

    #[test]
    fn empty_racks_keep_their_place() {
        let mut ship = find_ships(&sample(), &is_ship).remove(0);
        ship.contents.remove(&Section::Med);
        ship.contents.remove(&Section::Low);
        let text = to_eft(&ship, "Salvation", "Kearsarge", &names);
        assert!(text.contains("[Empty Low slot]"));
        let fit = eft::parse(&text);
        assert_eq!(fit.count(Section::High, "Heavy Energy Neutralizer II"), 1);
        assert_eq!(fit.count(Section::Rigs, "Capital Trimark Armor Pump I"), 1);
        assert!(fit.lines(Section::Low).is_empty());
    }

    #[test]
    fn unknown_names_still_show() {
        let ships = find_ships(&sample(), &is_ship);
        let text = to_eft(&ships[0], "Salvation", "Kearsarge", &|_| None);
        assert!(text.contains("Type 4\n"));
    }
}
