//! The map part of `cargo xtask sde`: regions, constellations, solar systems,
//! stargate connections and the drawing layout of every region.
//!
//! Only systems with stargates are kept: that is all of known space plus
//! Pochven and Zarzakh. Wormhole space and the Abyss have no gates, so the
//! Intel map can't show them anyway.
//!
//! ## The layout
//!
//! Positions come from the SDE's `position2D`: CCP's own schematic layout,
//! the one the in-game 2D map uses. It is one big picture of the whole
//! universe in metres, already lined up on a grid (one grid step is about
//! 4.1e15 m, with half steps in places).
//!
//! For each region we turn those positions into "map points": pixels at zoom
//! 1.0, with y pointing down like on screen. Then labels that would overlap
//! get nudged apart once, here, so the app never lays anything out while it
//! runs. Systems in other regions that a region's systems connect to by gate
//! (drawn as hollow dots) are placed a short way out from the system they
//! connect to, in the direction of where they really are.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, bail};
use serde::Deserialize;

use crate::{Name, english};

/// Map points per metre of `position2D`. One grid step (about 4.1e15 m)
/// becomes about 36 points.
const POINTS_PER_METRE: f64 = 36.0 / 4.1089e15;

/// How far out, in map points, a system of a neighbouring region is drawn
/// from the system it connects to.
const BORDER_DISTANCE: f64 = 64.0;

/// The size the app draws a system with at zoom 1.0, in map points. The app
/// uses the same numbers (`src/universe.rs`); if they change, change both.
/// A dot of this radius sits at the system's point...
const DOT_RADIUS: f64 = 4.0;
/// ...and the label (name and security, monospace) starts this far to the
/// lower right of it.
const LABEL_OFFSET: (f64, f64) = (6.0, 3.0);
/// Width of one monospace character in the label, plus the label's padding.
const LABEL_CHAR_WIDTH: f64 = 6.0;
const LABEL_PADDING: f64 = 6.0;
const LABEL_HEIGHT: f64 = 14.0;
/// Extra room kept free around every box, so nudged labels don't touch.
const GAP: f64 = 2.0;

/// One line of `mapRegions.jsonl`.
#[derive(Deserialize)]
pub struct SdeRegion {
    #[serde(rename = "_key")]
    pub id: i64,
    pub name: Option<Name>,
}

/// One line of `mapConstellations.jsonl`.
#[derive(Deserialize)]
pub struct SdeConstellation {
    #[serde(rename = "_key")]
    pub id: i64,
    pub name: Option<Name>,
    #[serde(rename = "regionID")]
    pub region_id: i64,
}

#[derive(Deserialize, Clone, Copy)]
pub struct Position2D {
    pub x: f64,
    pub y: f64,
}

/// One line of `mapSolarSystems.jsonl`.
#[derive(Deserialize)]
pub struct SdeSolarSystem {
    #[serde(rename = "_key")]
    pub id: i64,
    pub name: Option<Name>,
    #[serde(rename = "regionID")]
    pub region_id: i64,
    #[serde(rename = "constellationID")]
    pub constellation_id: i64,
    #[serde(rename = "securityStatus")]
    pub security: f64,
    #[serde(rename = "position2D")]
    pub position_2d: Option<Position2D>,
}

#[derive(Deserialize)]
pub struct SdeGateEnd {
    #[serde(rename = "solarSystemID")]
    pub system_id: i64,
}

/// One line of `mapStargates.jsonl`.
#[derive(Deserialize)]
pub struct SdeStargate {
    #[serde(rename = "solarSystemID")]
    pub system_id: i64,
    pub destination: SdeGateEnd,
}

pub struct Region {
    pub name: String,
}

pub struct Constellation {
    pub name: String,
    pub region_id: i64,
}

pub struct System {
    pub name: String,
    pub region_id: i64,
    pub constellation_id: i64,
    pub security: f64,
    /// Where the system is drawn on its own region's map, in map points.
    pub map_x: f64,
    pub map_y: f64,
}

/// A system of another region, drawn on `region_id`'s map because a gate
/// leads there.
pub struct BorderSystem {
    pub region_id: i64,
    pub system_id: i64,
    pub map_x: f64,
    pub map_y: f64,
}

/// Everything the map tables hold, ready to be written.
pub struct MapData {
    pub regions: BTreeMap<i64, Region>,
    pub constellations: BTreeMap<i64, Constellation>,
    pub systems: BTreeMap<i64, System>,
    /// Each gate connection once, as (lower system id, higher system id).
    pub gates: BTreeSet<(i64, i64)>,
    pub borders: Vec<BorderSystem>,
}

/// Picks the systems with gates and lays out every region.
pub fn build(
    sde_regions: Vec<SdeRegion>,
    sde_constellations: Vec<SdeConstellation>,
    sde_systems: Vec<SdeSolarSystem>,
    sde_gates: Vec<SdeStargate>,
) -> Result<MapData> {
    let mut gates = BTreeSet::new();
    for gate in &sde_gates {
        let (a, b) = (gate.system_id, gate.destination.system_id);
        if a != b {
            gates.insert((a.min(b), a.max(b)));
        }
    }
    let gated: BTreeSet<i64> = gates.iter().flat_map(|&(a, b)| [a, b]).collect();

    // Keep the systems with gates, at their raw `position2D` for now.
    let mut systems = BTreeMap::new();
    let mut raw_position = BTreeMap::new();
    for s in sde_systems {
        if !gated.contains(&s.id) {
            continue;
        }
        let Some(name) = english(&s.name) else {
            bail!("system {} has no English name", s.id);
        };
        let Some(position) = s.position_2d else {
            bail!("{name} has gates but no position2D; has the SDE format changed?");
        };
        raw_position.insert(s.id, position);
        systems.insert(
            s.id,
            System {
                name,
                region_id: s.region_id,
                constellation_id: s.constellation_id,
                security: s.security,
                map_x: 0.0,
                map_y: 0.0,
            },
        );
    }
    if let Some(&(a, b)) = gates
        .iter()
        .find(|(a, b)| !systems.contains_key(a) || !systems.contains_key(b))
    {
        bail!("gate between {a} and {b} leads to an unknown system");
    }

    // Only regions and constellations that have such systems.
    let used_regions: BTreeSet<i64> = systems.values().map(|s| s.region_id).collect();
    let used_constellations: BTreeSet<i64> = systems.values().map(|s| s.constellation_id).collect();
    let mut regions = BTreeMap::new();
    for r in sde_regions {
        if used_regions.contains(&r.id) {
            let name = english(&r.name).unwrap_or_else(|| r.id.to_string());
            regions.insert(r.id, Region { name });
        }
    }
    let mut constellations = BTreeMap::new();
    for c in sde_constellations {
        if used_constellations.contains(&c.id) {
            let name = english(&c.name).unwrap_or_else(|| c.id.to_string());
            constellations.insert(
                c.id,
                Constellation {
                    name,
                    region_id: c.region_id,
                },
            );
        }
    }
    if let Some(id) = used_regions.iter().find(|id| !regions.contains_key(id)) {
        bail!("region {id} is missing from mapRegions");
    }

    let mut borders = Vec::new();
    for &region_id in &used_regions {
        borders.extend(layout_region(
            region_id,
            &mut systems,
            &raw_position,
            &gates,
        ));
    }

    Ok(MapData {
        regions,
        constellations,
        systems,
        gates,
        borders,
    })
}

/// One box to keep apart from the others while nudging labels.
struct Placed {
    /// The system's point (the dot's centre), in map points.
    x: f64,
    y: f64,
    /// The width of its label.
    label_width: f64,
}

impl Placed {
    /// The box covering the dot and the label: (left, top, right, bottom).
    fn bounds(&self) -> (f64, f64, f64, f64) {
        (
            self.x - DOT_RADIUS - GAP,
            self.y - DOT_RADIUS - GAP,
            self.x + LABEL_OFFSET.0 + self.label_width + GAP,
            self.y + LABEL_OFFSET.1 + LABEL_HEIGHT + GAP,
        )
    }
}

/// The label reads like "X7R-JW -0.4": the name, a space and the security.
fn label_width(name: &str) -> f64 {
    let chars = name.chars().count() + 5;
    chars as f64 * LABEL_CHAR_WIDTH + LABEL_PADDING
}

/// Lays out one region: sets `map_x`/`map_y` of its systems and returns the
/// border systems of neighbouring regions to draw on its map.
fn layout_region(
    region_id: i64,
    systems: &mut BTreeMap<i64, System>,
    raw_position: &BTreeMap<i64, Position2D>,
    gates: &BTreeSet<(i64, i64)>,
) -> Vec<BorderSystem> {
    // `position2D` has y pointing up (north); the screen has y pointing down.
    let to_points = |p: Position2D| (p.x * POINTS_PER_METRE, -p.y * POINTS_PER_METRE);

    let inside: Vec<i64> = systems
        .iter()
        .filter(|(_, s)| s.region_id == region_id)
        .map(|(&id, _)| id)
        .collect();
    let mut placed: Vec<(i64, Placed)> = inside
        .iter()
        .map(|&id| {
            let (x, y) = to_points(raw_position[&id]);
            let label_width = label_width(&systems[&id].name);
            (id, Placed { x, y, label_width })
        })
        .collect();
    let inside_count = placed.len();

    // Systems of other regions, one per system even when several of ours
    // lead there: placed out from each of them, then averaged.
    let mut outside: BTreeMap<i64, Vec<(f64, f64)>> = BTreeMap::new();
    for &(a, b) in gates {
        let (ours, theirs) = match (
            systems[&a].region_id == region_id,
            systems[&b].region_id == region_id,
        ) {
            (true, false) => (a, b),
            (false, true) => (b, a),
            _ => continue,
        };
        let (ox, oy) = to_points(raw_position[&ours]);
        let (tx, ty) = to_points(raw_position[&theirs]);
        let length = ((tx - ox).powi(2) + (ty - oy).powi(2)).sqrt().max(1.0);
        outside.entry(theirs).or_default().push((
            ox + (tx - ox) / length * BORDER_DISTANCE,
            oy + (ty - oy) / length * BORDER_DISTANCE,
        ));
    }
    for (&id, points) in &outside {
        let count = points.len() as f64;
        let x = points.iter().map(|p| p.0).sum::<f64>() / count;
        let y = points.iter().map(|p| p.1).sum::<f64>() / count;
        let label_width = label_width(&systems[&id].name);
        placed.push((id, Placed { x, y, label_width }));
    }

    nudge_apart(&mut placed);

    // Move the region so its top-left corner is at (0, 0). Border systems
    // may end up at negative points, which is fine.
    let min_x = placed[..inside_count]
        .iter()
        .map(|(_, p)| p.x)
        .fold(f64::INFINITY, f64::min);
    let min_y = placed[..inside_count]
        .iter()
        .map(|(_, p)| p.y)
        .fold(f64::INFINITY, f64::min);

    let mut borders = Vec::new();
    for (index, (id, p)) in placed.into_iter().enumerate() {
        // Rounded to a tenth of a point: plenty, and keeps the file small.
        let x = ((p.x - min_x) * 10.0).round() / 10.0;
        let y = ((p.y - min_y) * 10.0).round() / 10.0;
        if index < inside_count {
            let system = systems.get_mut(&id).expect("an id from this map");
            system.map_x = x;
            system.map_y = y;
        } else {
            borders.push(BorderSystem {
                region_id,
                system_id: id,
                map_x: x,
                map_y: y,
            });
        }
    }
    borders
}

/// Pushes overlapping boxes apart until none overlap (or we give up after a
/// few hundred rounds, which leaves a few overlaps rather than hanging).
///
/// Each overlapping pair moves apart along the axis where they overlap
/// least, half each. Labels are wide and flat, so two systems next to each
/// other on a row usually move a little up and down rather than far apart
/// sideways, and the map keeps CCP's shape.
fn nudge_apart(placed: &mut [(i64, Placed)]) {
    for _round in 0..500 {
        let mut moved = false;
        for i in 0..placed.len() {
            for j in (i + 1)..placed.len() {
                let a = placed[i].1.bounds();
                let b = placed[j].1.bounds();
                let overlap_x = a.2.min(b.2) - a.0.max(b.0);
                let overlap_y = a.3.min(b.3) - a.1.max(b.1);
                if overlap_x <= 0.0 || overlap_y <= 0.0 {
                    continue;
                }
                moved = true;
                // Which way to push: away from each other. Ties (same spot)
                // go by list order, so the result is always the same.
                let (dx, dy) = if overlap_y <= overlap_x {
                    let sign = if placed[j].1.y >= placed[i].1.y {
                        1.0
                    } else {
                        -1.0
                    };
                    (0.0, sign * (overlap_y / 2.0 + 0.1))
                } else {
                    let sign = if placed[j].1.x >= placed[i].1.x {
                        1.0
                    } else {
                        -1.0
                    };
                    (sign * (overlap_x / 2.0 + 0.1), 0.0)
                };
                placed[i].1.x -= dx;
                placed[i].1.y -= dy;
                placed[j].1.x += dx;
                placed[j].1.y += dy;
            }
        }
        if !moved {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overlaps(placed: &[(i64, Placed)]) -> usize {
        let mut count = 0;
        for i in 0..placed.len() {
            for j in (i + 1)..placed.len() {
                let a = placed[i].1.bounds();
                let b = placed[j].1.bounds();
                if a.2.min(b.2) > a.0.max(b.0) && a.3.min(b.3) > a.1.max(b.1) {
                    count += 1;
                }
            }
        }
        count
    }

    #[test]
    fn neighbours_on_a_row_move_apart_vertically() {
        let label_width = label_width("X7R-JW");
        let mut placed = vec![
            (
                1,
                Placed {
                    x: 0.0,
                    y: 0.0,
                    label_width,
                },
            ),
            (
                2,
                Placed {
                    x: 36.0,
                    y: 0.0,
                    label_width,
                },
            ),
        ];
        nudge_apart(&mut placed);
        assert_eq!(overlaps(&placed), 0);
        // Only moved up/down, not sideways.
        assert_eq!(placed[0].1.x, 0.0);
        assert_eq!(placed[1].1.x, 36.0);
        assert!(placed[0].1.y < placed[1].1.y);
    }

    #[test]
    fn a_crowded_spot_ends_without_overlaps() {
        let mut placed: Vec<(i64, Placed)> = (0..12)
            .map(|i| {
                (
                    i,
                    Placed {
                        x: (i % 4) as f64 * 18.0,
                        y: (i / 4) as f64 * 18.0,
                        label_width: label_width("ABC-12"),
                    },
                )
            })
            .collect();
        nudge_apart(&mut placed);
        assert_eq!(overlaps(&placed), 0);
    }
}
