//! The map part of `cargo xtask sde`: regions, constellations, solar systems,
//! stargate connections and the drawing layout of every region.
//!
//! Only systems with stargates are kept: that is all of known space plus
//! Pochven and Zarzakh. Wormhole space and the Abyss have no gates, so the
//! Intel map can't show them anyway.
//!
//! ## The layout
//!
//! Positions come from DOTLAN EveMaps' hand-made region maps (see
//! `dotlan.rs`) when they were downloaded: each system's dot goes at the
//! left of its Dotlan box, stretched a little because Athena's labels are
//! wider than Dotlan's boxes. Systems of other regions that a gate leads to
//! (drawn as hollow dots) use the spot Dotlan gives them on the edge of the
//! region's map.
//!
//! Without a Dotlan map for a region, its positions come from the SDE's
//! `position2D`: CCP's own schematic layout, the one the in-game 2D map
//! uses. It is one big picture of the whole universe in metres, already
//! lined up on a grid (one grid step is about 4.1e15 m, with half steps in
//! places). A system that Dotlan's map lacks (say, one a patch just added)
//! is put next to its gate neighbours, using `position2D` for the direction
//! and distance.
//!
//! Positions are "map points": pixels at zoom 1.0, with y pointing down like
//! on screen. Then labels that would overlap get nudged apart once, here, so
//! the app never lays anything out while it runs. Border systems without a
//! spot of their own are placed a short way out from the system they connect
//! to, in the direction of where they really are.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, bail};
use serde::Deserialize;

use crate::dotlan::Boxes;
use crate::{Name, english};

/// Map points per metre of `position2D`. One grid step (about 4.1e15 m)
/// becomes about 36 points.
const POINTS_PER_METRE: f64 = 36.0 / 4.1089e15;

/// Where a system's dot goes in its Dotlan box: near the left edge and
/// halfway down, so the label to its lower right covers where the box was.
const DOTLAN_DOT_IN_BOX: (f64, f64) = (6.0, 10.0);
/// How much Dotlan's map is stretched. Athena's labels are wider than
/// Dotlan's 62.5-point boxes because they show the security too
/// ("X7R-JW -0.4"), so x gets more room than y. The nudge pass below
/// separates the few labels that still touch.
const DOTLAN_STRETCH: (f64, f64) = (1.5, 1.1);

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
    /// How many systems got their position from a Dotlan box.
    pub from_dotlan: usize,
}

impl MapData {
    /// Which layout the map got, for the `meta` table: "dotlan",
    /// "position2d" or "mixed".
    pub fn layout_name(&self) -> &'static str {
        if self.from_dotlan == 0 {
            "position2d"
        } else if self.from_dotlan == self.systems.len() {
            "dotlan"
        } else {
            "mixed"
        }
    }
}

/// The region ids and English names of every region with stargates, which
/// are the regions the Intel map shows (and so the Dotlan maps to fetch).
pub fn regions_with_gates(
    sde_regions: Vec<SdeRegion>,
    sde_systems: &[SdeSolarSystem],
    sde_gates: &[SdeStargate],
) -> BTreeMap<i64, String> {
    let gated: BTreeSet<i64> = sde_gates
        .iter()
        .flat_map(|g| [g.system_id, g.destination.system_id])
        .collect();
    let used: BTreeSet<i64> = sde_systems
        .iter()
        .filter(|s| gated.contains(&s.id))
        .map(|s| s.region_id)
        .collect();
    sde_regions
        .into_iter()
        .filter(|r| used.contains(&r.id))
        .map(|r| (r.id, english(&r.name).unwrap_or_else(|| r.id.to_string())))
        .collect()
}

/// Picks the systems with gates and lays out every region, from the Dotlan
/// maps in `dotlan` (by region id) where there is one.
pub fn build(
    sde_regions: Vec<SdeRegion>,
    sde_constellations: Vec<SdeConstellation>,
    sde_systems: Vec<SdeSolarSystem>,
    sde_gates: Vec<SdeStargate>,
    dotlan: &BTreeMap<i64, Boxes>,
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
    let mut from_dotlan = 0;
    for &region_id in &used_regions {
        let layout = layout_region(
            region_id,
            &mut systems,
            &raw_position,
            &gates,
            dotlan.get(&region_id),
        );
        borders.extend(layout.borders);
        from_dotlan += layout.from_dotlan;
        if !layout.not_on_dotlan.is_empty() {
            let names: Vec<&str> = layout
                .not_on_dotlan
                .iter()
                .map(|id| systems[id].name.as_str())
                .collect();
            println!(
                "{}: not on Dotlan's map, placed next to their gate neighbours: {}",
                regions[&region_id].name,
                names.join(", ")
            );
        }
    }

    Ok(MapData {
        regions,
        constellations,
        systems,
        gates,
        borders,
        from_dotlan,
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

/// What laying out one region gives, besides the systems' `map_x`/`map_y`.
struct RegionLayout {
    /// Systems of neighbouring regions to draw on its map.
    borders: Vec<BorderSystem>,
    /// How many of its own systems got their spot from a Dotlan box.
    from_dotlan: usize,
    /// Its own systems that its Dotlan map lacks (empty without a map).
    not_on_dotlan: Vec<i64>,
}

/// `position2D` in map points. It has y pointing up (north); the screen has
/// y pointing down.
fn to_points(p: Position2D) -> (f64, f64) {
    (p.x * POINTS_PER_METRE, -p.y * POINTS_PER_METRE)
}

/// Where a system's dot goes for a Dotlan box with its top-left at `corner`.
fn dotlan_point(corner: (f64, f64)) -> (f64, f64) {
    (
        (corner.0 + DOTLAN_DOT_IN_BOX.0) * DOTLAN_STRETCH.0,
        (corner.1 + DOTLAN_DOT_IN_BOX.1) * DOTLAN_STRETCH.1,
    )
}

/// The starting spot of each of a region's own systems (before nudging).
/// With a Dotlan map that's the system's box; a system the map lacks goes
/// next to its gate neighbours, offset by the difference of their
/// `position2D`. Without a map every system uses its `position2D`.
///
/// Returns the spots and the systems that the Dotlan map lacked.
fn starting_points(
    inside: &[i64],
    raw_position: &BTreeMap<i64, Position2D>,
    gates: &BTreeSet<(i64, i64)>,
    dotlan: Option<&Boxes>,
) -> (BTreeMap<i64, (f64, f64)>, Vec<i64>) {
    let Some(boxes) = dotlan else {
        let points = inside
            .iter()
            .map(|&id| (id, to_points(raw_position[&id])))
            .collect();
        return (points, Vec::new());
    };

    let mut points: BTreeMap<i64, (f64, f64)> = BTreeMap::new();
    let mut missing: Vec<i64> = Vec::new();
    for &id in inside {
        match boxes.get(&id) {
            Some(&corner) => {
                points.insert(id, dotlan_point(corner));
            }
            None => missing.push(id),
        }
    }
    let not_on_dotlan = missing.clone();

    // Where `id` would be if `from` (already placed) is where it is now,
    // going by their `position2D`.
    let offset_from = |points: &BTreeMap<i64, (f64, f64)>, from: i64, id: i64| {
        let (fx, fy) = to_points(raw_position[&from]);
        let (ix, iy) = to_points(raw_position[&id]);
        let (px, py) = points[&from];
        (px + ix - fx, py + iy - fy)
    };

    // Place the missing ones next to placed gate neighbours in the region,
    // a round at a time, so a chain of missing systems is placed one after
    // the other.
    loop {
        let mut placed_any = false;
        missing.retain(|&id| {
            let near: Vec<(f64, f64)> = gates
                .iter()
                .filter_map(|&(a, b)| match (a == id, b == id) {
                    (true, _) => Some(b),
                    (_, true) => Some(a),
                    _ => None,
                })
                .filter(|other| points.contains_key(other))
                .map(|other| offset_from(&points, other, id))
                .collect();
            if near.is_empty() {
                return true;
            }
            let count = near.len() as f64;
            let x = near.iter().map(|p| p.0).sum::<f64>() / count;
            let y = near.iter().map(|p| p.1).sum::<f64>() / count;
            points.insert(id, (x, y));
            placed_any = true;
            false
        });
        if missing.is_empty() || !placed_any {
            break;
        }
    }

    // Anything still left has no placed neighbour at all (a group of new
    // systems on their own): line it up with the region as a whole.
    if !missing.is_empty() {
        // The middle of the placed systems, on the map and in `position2D`.
        let count = points.len().max(1) as f64;
        let (mut cx, mut cy, mut rx, mut ry) = (0.0, 0.0, 0.0, 0.0);
        for (&id, &(x, y)) in &points {
            let (px, py) = to_points(raw_position[&id]);
            cx += x / count;
            cy += y / count;
            rx += px / count;
            ry += py / count;
        }
        for id in missing {
            let (ix, iy) = to_points(raw_position[&id]);
            points.insert(id, (cx + ix - rx, cy + iy - ry));
        }
    }
    (points, not_on_dotlan)
}

/// Lays out one region: sets `map_x`/`map_y` of its systems and returns the
/// border systems of neighbouring regions to draw on its map.
fn layout_region(
    region_id: i64,
    systems: &mut BTreeMap<i64, System>,
    raw_position: &BTreeMap<i64, Position2D>,
    gates: &BTreeSet<(i64, i64)>,
    dotlan: Option<&Boxes>,
) -> RegionLayout {
    let inside: Vec<i64> = systems
        .iter()
        .filter(|(_, s)| s.region_id == region_id)
        .map(|(&id, _)| id)
        .collect();
    let (start, not_on_dotlan) = starting_points(&inside, raw_position, gates, dotlan);
    let from_dotlan = if dotlan.is_some() {
        inside.len() - not_on_dotlan.len()
    } else {
        0
    };
    let mut placed: Vec<(i64, Placed)> = inside
        .iter()
        .map(|&id| {
            let (x, y) = start[&id];
            let label_width = label_width(&systems[&id].name);
            (id, Placed { x, y, label_width })
        })
        .collect();
    let inside_count = placed.len();

    // Systems of other regions that a gate leads to. Their box on this
    // region's Dotlan map if it has one; otherwise one spot per system even
    // when several of ours lead there: placed out from each of them, then
    // averaged.
    let mut outside: BTreeMap<i64, Vec<(f64, f64)>> = BTreeMap::new();
    let mut outside_on_dotlan: BTreeMap<i64, (f64, f64)> = BTreeMap::new();
    for &(a, b) in gates {
        let (ours, theirs) = match (
            systems[&a].region_id == region_id,
            systems[&b].region_id == region_id,
        ) {
            (true, false) => (a, b),
            (false, true) => (b, a),
            _ => continue,
        };
        if let Some(&corner) = dotlan.and_then(|boxes| boxes.get(&theirs)) {
            outside_on_dotlan.insert(theirs, dotlan_point(corner));
            continue;
        }
        // The direction comes from `position2D`, the start from where our
        // system is on this map.
        let (ox, oy) = start[&ours];
        let (rx, ry) = to_points(raw_position[&ours]);
        let (tx, ty) = to_points(raw_position[&theirs]);
        let length = ((tx - rx).powi(2) + (ty - ry).powi(2)).sqrt().max(1.0);
        outside.entry(theirs).or_default().push((
            ox + (tx - rx) / length * BORDER_DISTANCE,
            oy + (ty - ry) / length * BORDER_DISTANCE,
        ));
    }
    for (&id, points) in &outside {
        if outside_on_dotlan.contains_key(&id) {
            continue;
        }
        let count = points.len() as f64;
        let x = points.iter().map(|p| p.0).sum::<f64>() / count;
        let y = points.iter().map(|p| p.1).sum::<f64>() / count;
        let label_width = label_width(&systems[&id].name);
        placed.push((id, Placed { x, y, label_width }));
    }
    for (&id, &(x, y)) in &outside_on_dotlan {
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
    RegionLayout {
        borders,
        from_dotlan,
        not_on_dotlan,
    }
}

/// Pushes overlapping boxes apart until none overlap (or we give up after a
/// few hundred rounds, which leaves a few overlaps rather than hanging).
///
/// Each overlapping pair moves apart along the axis where they overlap
/// least, half each. Labels are wide and flat, so two systems next to each
/// other on a row usually move a little up and down rather than far apart
/// sideways, and the map keeps its shape.
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

    /// One `position2D` grid step in metres (36 map points).
    const STEP: f64 = 4.1089e15;

    fn name(text: &str) -> Option<Name> {
        Some(Name {
            en: Some(text.to_string()),
        })
    }

    fn system(id: i64, region_id: i64, grid: (f64, f64)) -> SdeSolarSystem {
        SdeSolarSystem {
            id,
            name: name(&format!("SYS-{id}")),
            region_id,
            constellation_id: region_id * 10,
            security: -0.5,
            // `position2D` has y pointing up, so a step "down" on the map is
            // a negative y here.
            position_2d: Some(Position2D {
                x: grid.0 * STEP,
                y: -grid.1 * STEP,
            }),
        }
    }

    fn gate(from: i64, to: i64) -> SdeStargate {
        SdeStargate {
            system_id: from,
            destination: SdeGateEnd { system_id: to },
        }
    }

    /// Region 1 has systems 11 - 12 - 13 in a row (far enough apart that
    /// no labels touch); 13 has a gate to 21 in region 2, which is below it.
    fn tiny_map(dotlan: &BTreeMap<i64, Boxes>) -> MapData {
        build(
            vec![
                SdeRegion {
                    id: 1,
                    name: name("Alpha"),
                },
                SdeRegion {
                    id: 2,
                    name: name("Beta"),
                },
            ],
            vec![
                SdeConstellation {
                    id: 10,
                    name: name("A"),
                    region_id: 1,
                },
                SdeConstellation {
                    id: 20,
                    name: name("B"),
                    region_id: 2,
                },
            ],
            vec![
                system(11, 1, (0.0, 0.0)),
                system(12, 1, (5.0, 0.0)),
                system(13, 1, (10.0, 2.0)),
                system(21, 2, (10.0, 10.0)),
            ],
            vec![gate(11, 12), gate(12, 13), gate(13, 21), gate(21, 13)],
            dotlan,
        )
        .unwrap()
    }

    fn spot(map: &MapData, id: i64) -> (f64, f64) {
        (map.systems[&id].map_x, map.systems[&id].map_y)
    }

    fn border(map: &MapData, region_id: i64, id: i64) -> (f64, f64) {
        let b = map
            .borders
            .iter()
            .find(|b| b.region_id == region_id && b.system_id == id)
            .unwrap();
        (b.map_x, b.map_y)
    }

    #[test]
    fn dotlan_boxes_give_the_spots() {
        // Region 1's Dotlan map, with 21 drawn at its edge.
        let boxes = Boxes::from([
            (11, (10.0, 100.0)),
            (12, (210.0, 100.0)),
            (13, (410.0, 200.0)),
            (21, (610.0, 200.0)),
        ]);
        let map = tiny_map(&BTreeMap::from([(1, boxes)]));

        // Moved so the top-left system is at (0, 0), stretched by 1.5 / 1.1.
        assert_eq!(spot(&map, 11), (0.0, 0.0));
        assert_eq!(spot(&map, 12), (300.0, 0.0));
        assert_eq!(spot(&map, 13), (600.0, 110.0));
        // The border system uses its box on region 1's map.
        assert_eq!(border(&map, 1, 21), (900.0, 110.0));

        // Region 2 has no Dotlan map, so it falls back to position2D.
        assert_eq!(map.from_dotlan, 3);
        assert_eq!(map.layout_name(), "mixed");
    }

    #[test]
    fn a_system_missing_from_dotlan_goes_next_to_its_neighbour() {
        // 13 isn't on the map (say a patch just added it), and neither is 21.
        let boxes = Boxes::from([(11, (10.0, 100.0)), (12, (210.0, 100.0))]);
        let map = tiny_map(&BTreeMap::from([(1, boxes)]));

        // 13 is 5 grid steps right of 12 and 2 down: 180 and 72 points.
        assert_eq!(spot(&map, 13), (300.0 + 180.0, 72.0));
        assert_eq!(map.from_dotlan, 2);

        // 21 has no box either: 64 points out from 13, towards where it
        // really is (straight down).
        assert_eq!(border(&map, 1, 21), (480.0, 72.0 + BORDER_DISTANCE));
    }

    #[test]
    fn without_dotlan_everything_uses_position2d() {
        let map = tiny_map(&BTreeMap::new());
        assert_eq!(spot(&map, 11), (0.0, 0.0));
        assert_eq!(spot(&map, 12), (180.0, 0.0));
        assert_eq!(spot(&map, 13), (360.0, 72.0));
        assert_eq!(map.from_dotlan, 0);
        assert_eq!(map.layout_name(), "position2d");
    }

    #[test]
    fn a_full_dotlan_map_is_named_dotlan() {
        let alpha = Boxes::from([
            (11, (10.0, 100.0)),
            (12, (210.0, 100.0)),
            (13, (410.0, 200.0)),
        ]);
        let beta = Boxes::from([(21, (500.0, 300.0))]);
        let map = tiny_map(&BTreeMap::from([(1, alpha), (2, beta)]));
        assert_eq!(map.from_dotlan, 4);
        assert_eq!(map.layout_name(), "dotlan");
        // Boxes in another region's file don't decide which region a system
        // is in: 21 is still in region 2, at (0, 0) of its own map.
        assert_eq!(map.systems[&21].region_id, 2);
        assert_eq!(spot(&map, 21), (0.0, 0.0));
    }

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
