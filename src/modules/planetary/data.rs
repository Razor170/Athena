//! The Planetary module's data: what ESI sends about a character's
//! colonies, and the plain structs Athena keeps (and saves) from it.
//!
//! ESI only updates a colony when its owner opens it in the game. So
//! `last_update` matters: everything in a colony's layout (how full a
//! launchpad is, whether a factory was running) is as of that moment.
//! Extractor end times are exact anyway, because they're set when the
//! program is started.

use serde::{Deserialize, Serialize};

use crate::modules::finance::data::parse_esi_date;

// ---------------------------------------------------------------- From ESI

/// One line of `/characters/{id}/planets`.
#[derive(Debug, Deserialize)]
pub struct EsiColony {
    pub last_update: String,
    pub num_pins: i64,
    pub planet_id: i64,
    pub planet_type: String,
    pub solar_system_id: i64,
    pub upgrade_level: i64,
}

/// `/characters/{id}/planets/{planet_id}`. Links (the lines between
/// structures) aren't needed and are skipped.
#[derive(Debug, Deserialize)]
pub struct EsiLayout {
    pub pins: Vec<EsiPin>,
    #[serde(default)]
    pub routes: Vec<EsiRoute>,
}

#[derive(Debug, Deserialize)]
pub struct EsiPin {
    pub pin_id: i64,
    pub type_id: i64,
    #[serde(default)]
    pub contents: Vec<EsiContent>,
    pub expiry_time: Option<String>,
    pub install_time: Option<String>,
    pub last_cycle_start: Option<String>,
    pub schematic_id: Option<i64>,
    pub extractor_details: Option<EsiExtractor>,
    pub factory_details: Option<EsiFactory>,
}

#[derive(Debug, Deserialize)]
pub struct EsiContent {
    pub type_id: i64,
    pub amount: i64,
}

#[derive(Debug, Deserialize)]
pub struct EsiExtractor {
    /// Seconds per cycle.
    pub cycle_time: Option<i64>,
    /// Only counted. `IgnoredAny` reads a value without keeping it.
    #[serde(default)]
    pub heads: Vec<serde::de::IgnoredAny>,
    pub product_type_id: Option<i64>,
    pub qty_per_cycle: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct EsiFactory {
    pub schematic_id: i64,
}

#[derive(Debug, Deserialize)]
pub struct EsiRoute {
    pub source_pin_id: i64,
    pub destination_pin_id: i64,
    pub content_type_id: i64,
    pub quantity: f64,
}

// ------------------------------------------------------- What Athena keeps

/// One planet a character has a colony on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Colony {
    pub character_id: u64,
    pub planet_id: i64,
    pub system_id: i64,
    /// "barren", "lava", ... as ESI writes it.
    pub planet_type: String,
    pub upgrade_level: i64,
    pub num_pins: i64,
    /// When the owner last opened the colony in the game, Unix seconds.
    pub last_update: i64,
    /// The structures, from the second ESI call. `None` until fetched.
    pub layout: Option<Layout>,
}

impl Colony {
    /// `None` if ESI sent a date we can't read.
    pub fn from_esi(character_id: u64, esi: EsiColony) -> Option<Colony> {
        Some(Colony {
            character_id,
            planet_id: esi.planet_id,
            system_id: esi.solar_system_id,
            planet_type: esi.planet_type,
            upgrade_level: esi.upgrade_level,
            num_pins: esi.num_pins,
            last_update: parse_esi_date(&esi.last_update)?,
            layout: None,
        })
    }

    /// Identifies this colony: (character id, planet id). Several
    /// characters of one player often have colonies on the same planet,
    /// so the planet id alone isn't enough.
    pub fn key(&self) -> (u64, i64) {
        (self.character_id, self.planet_id)
    }

    /// "Barren", "Lava", ...
    pub fn type_label(&self) -> String {
        let mut chars = self.planet_type.chars();
        match chars.next() {
            Some(first) => first.to_uppercase().chain(chars).collect(),
            None => String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Layout {
    pub pins: Vec<Pin>,
    pub routes: Vec<Route>,
}

impl Layout {
    pub fn from_esi(esi: EsiLayout) -> Layout {
        let date = |text: &Option<String>| text.as_deref().and_then(parse_esi_date);
        let pins = esi
            .pins
            .into_iter()
            .map(|p| Pin {
                pin_id: p.pin_id,
                type_id: p.type_id,
                contents: p
                    .contents
                    .iter()
                    .filter(|c| c.amount > 0)
                    .map(|c| (c.type_id, c.amount))
                    .collect(),
                expiry: date(&p.expiry_time),
                install: date(&p.install_time),
                last_cycle_start: date(&p.last_cycle_start),
                // ESI sends the schematic in one place or the other.
                schematic_id: p.factory_details.map(|f| f.schematic_id).or(p.schematic_id),
                extractor: p.extractor_details.and_then(|e| {
                    Some(Extractor {
                        cycle_secs: e.cycle_time?,
                        heads: e.heads.len() as u32,
                        product: e.product_type_id?,
                        qty_per_cycle: e.qty_per_cycle.unwrap_or(0),
                    })
                }),
            })
            .collect();
        let routes = esi
            .routes
            .into_iter()
            .map(|r| Route {
                source: r.source_pin_id,
                destination: r.destination_pin_id,
                type_id: r.content_type_id,
                quantity: r.quantity,
            })
            .collect();
        Layout { pins, routes }
    }
}

/// One structure on a planet.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Pin {
    pub pin_id: i64,
    pub type_id: i64,
    /// (type id, amount) of what's stored in it.
    pub contents: Vec<(i64, i64)>,
    /// When an extractor program ends.
    pub expiry: Option<i64>,
    /// When an extractor program started.
    pub install: Option<i64>,
    /// When a factory started its current cycle.
    pub last_cycle_start: Option<i64>,
    pub schematic_id: Option<i64>,
    /// Set for extractor control units with a program.
    pub extractor: Option<Extractor>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Extractor {
    pub cycle_secs: i64,
    pub heads: u32,
    /// The P0 type it extracts.
    pub product: i64,
    /// Units per cycle at the start of the program. The real amount goes
    /// up and down from cycle to cycle, so estimates from it are rough.
    pub qty_per_cycle: i64,
}

/// A route moves a commodity from one structure to another.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Route {
    pub source: i64,
    pub destination: i64,
    pub type_id: i64,
    pub quantity: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_an_esi_layout() {
        // Shortened from the ESI documentation's shape.
        let json = r#"{
            "links": [{"source_pin_id": 1, "destination_pin_id": 2, "link_level": 0}],
            "pins": [
              {"pin_id": 1, "type_id": 3068, "latitude": 1.0, "longitude": 2.0,
               "expiry_time": "2026-11-12T14:20:00Z", "install_time": "2026-11-10T14:20:00Z",
               "last_cycle_start": "2026-11-12T13:50:00Z",
               "extractor_details": {"cycle_time": 1800, "head_radius": 0.01,
                 "heads": [{"head_id": 0, "latitude": 1.0, "longitude": 2.0},
                           {"head_id": 1, "latitude": 1.0, "longitude": 2.0}],
                 "product_type_id": 2268, "qty_per_cycle": 900}},
              {"pin_id": 2, "type_id": 2256, "latitude": 1.0, "longitude": 2.0,
               "contents": [{"type_id": 2268, "amount": 5000}, {"type_id": 3645, "amount": 0}]},
              {"pin_id": 3, "type_id": 2481, "latitude": 1.0, "longitude": 2.0,
               "factory_details": {"schematic_id": 121}}
            ],
            "routes": [{"route_id": 7, "source_pin_id": 1, "destination_pin_id": 2,
                        "content_type_id": 2268, "quantity": 900.0}]
        }"#;
        let esi: EsiLayout = serde_json::from_str(json).unwrap();
        let layout = Layout::from_esi(esi);
        assert_eq!(layout.pins.len(), 3);
        let ecu = &layout.pins[0];
        assert_eq!(
            ecu.extractor,
            Some(Extractor {
                cycle_secs: 1800,
                heads: 2,
                product: 2268,
                qty_per_cycle: 900
            })
        );
        assert_eq!(ecu.expiry, parse_esi_date("2026-11-12T14:20:00Z"));
        // Empty contents are left out.
        assert_eq!(layout.pins[1].contents, vec![(2268, 5000)]);
        assert_eq!(layout.pins[2].schematic_id, Some(121));
        assert_eq!(layout.routes[0].destination, 2);
    }

    #[test]
    fn reads_a_colony_line() {
        let json = r#"[{"last_update": "2026-11-12T10:00:00Z", "num_pins": 9,
            "owner_id": 90000001, "planet_id": 40000002, "planet_type": "barren",
            "solar_system_id": 30000001, "upgrade_level": 5}]"#;
        let mut list: Vec<EsiColony> = serde_json::from_str(json).unwrap();
        let colony = Colony::from_esi(7, list.remove(0)).unwrap();
        assert_eq!(colony.planet_id, 40000002);
        assert_eq!(colony.type_label(), "Barren");
        assert_eq!(
            colony.last_update,
            parse_esi_date("2026-11-12T10:00:00Z").unwrap()
        );
    }
}
