//! The EVE map for the Intel Supervisor: regions, solar systems, stargate
//! connections, where each system is drawn on its region's map, and the
//! names of all ship types.
//!
//! It comes from the same `static.sqlite` as the ore data (see
//! `xtask/src/map.rs` for how the tables are made) and is loaded with it by
//! [`crate::static_data::StaticData::load`]. About 5,300 systems and 7,000
//! gates: a few hundred kilobytes in memory.
//!
//! Jump distances are a breadth-first search over the gates, done when
//! needed. Up to 10 jumps from a few systems touches a few hundred systems,
//! which takes well under a millisecond, so nothing is precomputed.

// The Intel Supervisor that uses all of this comes in the next pull
// requests; until then most of it is only used by the tests.
#![allow(dead_code)]

use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap, VecDeque};

use anyhow::{Context, Result};

/// The size of a system on the map at zoom 1.0, in points. The xtask leaves
/// room for exactly this when it nudges labels apart (`xtask/src/map.rs`),
/// so the map should draw with these numbers.
pub mod drawing {
    /// Radius of a system's dot.
    pub const DOT_RADIUS: f32 = 4.0;
    /// Where the label (name and security) starts, from the dot's centre.
    pub const LABEL_OFFSET: [f32; 2] = [6.0, 3.0];
    /// Height of the label.
    pub const LABEL_HEIGHT: f32 = 14.0;
    /// Width of one monospace character of the label, and the label's
    /// padding left and right together.
    pub const LABEL_CHAR_WIDTH: f32 = 6.0;
    pub const LABEL_PADDING: f32 = 6.0;

    /// The label reads like "X7R-JW -0.4": the name, a space and the
    /// security. This is its width.
    pub fn label_width(name: &str) -> f32 {
        let chars = name.chars().count() + 5;
        chars as f32 * LABEL_CHAR_WIDTH + LABEL_PADDING
    }
}

/// A region, with the systems drawn on its map.
#[derive(Debug, Clone, PartialEq)]
pub struct Region {
    pub id: i64,
    pub name: String,
    /// Its own systems, by id.
    pub systems: Vec<i64>,
    /// Systems of other regions that its systems have gates to. They are
    /// drawn as hollow dots at `map_pos`, which is on this region's map
    /// (not where they are on their own region's map).
    pub borders: Vec<BorderSystem>,
    /// The size of the region's map at zoom 1.0, in points: the bottom-right
    /// corner of its last label. The top-left is (0, 0).
    pub size: [f32; 2],
}

/// A system of a neighbouring region, drawn on another region's map.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BorderSystem {
    pub system_id: i64,
    pub map_pos: [f32; 2],
}

/// One solar system.
#[derive(Debug, Clone, PartialEq)]
pub struct SolarSystem {
    pub id: i64,
    /// As the game writes it, e.g. "X7R-JW".
    pub name: String,
    pub region_id: i64,
    pub constellation_id: i64,
    /// Unrounded, e.g. -0.3684. Use [`SolarSystem::display_security`] to show it.
    pub security: f64,
    /// Where it's drawn on its own region's map, in points at zoom 1.0.
    pub map_pos: [f32; 2],
}

impl SolarSystem {
    /// The security status the way the game shows it: rounded to one
    /// decimal, except that anything just above 0.0 shows as 0.1 (it is
    /// low sec, not null sec).
    pub fn display_security(&self) -> f64 {
        if self.security > 0.0 && self.security < 0.05 {
            0.1
        } else {
            // `+ 0.0` turns -0.0 into 0.0, so it never shows as "-0.0".
            (self.security * 10.0).round() / 10.0 + 0.0
        }
    }
}

/// A ship type, e.g. Loki (Strategic Cruiser).
#[derive(Debug, Clone, PartialEq)]
pub struct ShipType {
    pub type_id: i64,
    pub name: String,
    pub group_name: String,
}

/// The whole map.
#[derive(Debug, Clone, Default)]
pub struct Universe {
    systems: HashMap<i64, SolarSystem>,
    /// Lowercase name -> system id, for reading names out of chat.
    system_by_name: HashMap<String, i64>,
    /// System id -> the systems one jump away.
    gates: HashMap<i64, Vec<i64>>,
    /// Sorted by id; `BTreeMap` keeps the order stable.
    regions: BTreeMap<i64, Region>,
    /// Lowercase name -> ship type.
    ship_by_name: HashMap<String, ShipType>,
}

impl Universe {
    /// Reads the map tables. Does file I/O when `db` is a file: not on the
    /// UI thread.
    pub fn from_connection(db: &rusqlite::Connection) -> Result<Universe> {
        let mut universe = Universe::default();

        let mut query = db.prepare("SELECT region_id, name FROM regions")?;
        let rows = query.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (id, name) = row?;
            universe.regions.insert(
                id,
                Region {
                    id,
                    name,
                    systems: Vec::new(),
                    borders: Vec::new(),
                    size: [0.0, 0.0],
                },
            );
        }

        let mut query = db.prepare(
            "SELECT system_id, name, constellation_id, region_id, security, map_x, map_y
             FROM systems ORDER BY system_id",
        )?;
        let rows = query.query_map([], |row| {
            Ok(SolarSystem {
                id: row.get(0)?,
                name: row.get(1)?,
                constellation_id: row.get(2)?,
                region_id: row.get(3)?,
                security: row.get(4)?,
                map_pos: [row.get::<_, f64>(5)? as f32, row.get::<_, f64>(6)? as f32],
            })
        })?;
        for row in rows {
            let system = row?;
            let region = universe
                .regions
                .get_mut(&system.region_id)
                .with_context(|| format!("{} is in an unknown region", system.name))?;
            region.systems.push(system.id);
            grow_to_fit(&mut region.size, system.map_pos, &system.name);
            universe
                .system_by_name
                .insert(system.name.to_lowercase(), system.id);
            universe.systems.insert(system.id, system);
        }

        let mut query = db.prepare("SELECT from_id, to_id FROM gates")?;
        let rows = query.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?;
        for row in rows {
            let (a, b) = row?;
            // Gates work both ways, but each is stored once.
            universe.gates.entry(a).or_default().push(b);
            universe.gates.entry(b).or_default().push(a);
        }

        let mut query = db.prepare(
            "SELECT region_id, system_id, map_x, map_y FROM region_borders
             ORDER BY region_id, system_id",
        )?;
        let rows = query.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                BorderSystem {
                    system_id: row.get(1)?,
                    map_pos: [row.get::<_, f64>(2)? as f32, row.get::<_, f64>(3)? as f32],
                },
            ))
        })?;
        for row in rows {
            let (region_id, border) = row?;
            let name = universe
                .systems
                .get(&border.system_id)
                .map(|s| s.name.clone())
                .unwrap_or_default();
            if let Some(region) = universe.regions.get_mut(&region_id) {
                grow_to_fit(&mut region.size, border.map_pos, &name);
                region.borders.push(border);
            }
        }

        let mut query = db.prepare("SELECT type_id, name, group_name FROM ship_types")?;
        let rows = query.query_map([], |row| {
            Ok(ShipType {
                type_id: row.get(0)?,
                name: row.get(1)?,
                group_name: row.get(2)?,
            })
        })?;
        for row in rows {
            let ship = row?;
            universe.ship_by_name.insert(ship.name.to_lowercase(), ship);
        }

        // Regions without any systems can't be drawn; leave them out.
        universe
            .regions
            .retain(|_, region| !region.systems.is_empty());
        Ok(universe)
    }

    pub fn system(&self, id: i64) -> Option<&SolarSystem> {
        self.systems.get(&id)
    }

    /// Finds a system by its exact name, ignoring upper/lower case.
    pub fn system_by_name(&self, name: &str) -> Option<&SolarSystem> {
        self.systems
            .get(self.system_by_name.get(&name.to_lowercase())?)
    }

    /// All systems, in no particular order.
    pub fn systems(&self) -> impl Iterator<Item = &SolarSystem> {
        self.systems.values()
    }

    /// The systems one jump away from `id`.
    pub fn neighbours(&self, id: i64) -> &[i64] {
        self.gates.get(&id).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn region(&self, id: i64) -> Option<&Region> {
        self.regions.get(&id)
    }

    /// All regions that have a map, sorted by name (for the region picker).
    pub fn regions_by_name(&self) -> Vec<&Region> {
        let mut regions: Vec<&Region> = self.regions.values().collect();
        regions.sort_by(|a, b| a.name.cmp(&b.name));
        regions
    }

    /// The regions next to `region_id`: those its border systems belong to.
    pub fn neighbour_regions(&self, region_id: i64) -> Vec<i64> {
        let mut ids: Vec<i64> = self
            .region(region_id)
            .map(|region| {
                region
                    .borders
                    .iter()
                    .filter_map(|b| self.system(b.system_id).map(|s| s.region_id))
                    .collect()
            })
            .unwrap_or_default();
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    /// Finds a ship type by its exact name, ignoring upper/lower case.
    pub fn ship_by_name(&self, name: &str) -> Option<&ShipType> {
        self.ship_by_name.get(&name.to_lowercase())
    }

    /// How many jumps each system is from the nearest of `starts`, for all
    /// systems at most `max_jumps` away. The starts themselves are 0.
    ///
    /// Use it once per update with all own pilots' systems, then look up
    /// each reported system in the result.
    pub fn jumps_within(&self, starts: &[i64], max_jumps: u32) -> HashMap<i64, u32> {
        // Breadth-first search: visit systems in order of distance, so the
        // first time we reach a system is by the shortest way.
        let mut distance: HashMap<i64, u32> = HashMap::new();
        let mut queue: VecDeque<i64> = VecDeque::new();
        for &start in starts {
            if self.systems.contains_key(&start) && !distance.contains_key(&start) {
                distance.insert(start, 0);
                queue.push_back(start);
            }
        }
        while let Some(current) = queue.pop_front() {
            let jumps = distance[&current];
            if jumps >= max_jumps {
                continue;
            }
            for &next in self.neighbours(current) {
                // `Entry::Vacant`: not reached before, so this is the shortest way.
                if let Entry::Vacant(slot) = distance.entry(next) {
                    slot.insert(jumps + 1);
                    queue.push_back(next);
                }
            }
        }
        distance
    }

    /// The number of jumps between two systems, if it's at most `max_jumps`.
    pub fn jumps_between(&self, from: i64, to: i64, max_jumps: u32) -> Option<u32> {
        self.jumps_within(&[from], max_jumps).get(&to).copied()
    }
}

/// Makes `size` big enough to hold the dot and label of a system at `pos`.
fn grow_to_fit(size: &mut [f32; 2], pos: [f32; 2], name: &str) {
    let right = pos[0] + drawing::LABEL_OFFSET[0] + drawing::label_width(name);
    let bottom = pos[1] + drawing::LABEL_OFFSET[1] + drawing::LABEL_HEIGHT;
    size[0] = size[0].max(right);
    size[1] = size[1].max(bottom);
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// The map tables with a tiny made-up map, for the static data tests:
    ///
    /// ```text
    /// Region "Alpha":  A1 - A2 - A3 - A4      (A2 also connects to B1)
    ///                        \
    /// Region "Beta":          B1 - B2
    /// ```
    pub const SAMPLE_MAP_SQL: &str = "
        CREATE TABLE ship_types (type_id INTEGER PRIMARY KEY, name TEXT NOT NULL,
            group_name TEXT NOT NULL);
        CREATE TABLE regions (region_id INTEGER PRIMARY KEY, name TEXT NOT NULL);
        CREATE TABLE constellations (constellation_id INTEGER PRIMARY KEY,
            name TEXT NOT NULL, region_id INTEGER NOT NULL);
        CREATE TABLE systems (system_id INTEGER PRIMARY KEY, name TEXT NOT NULL,
            constellation_id INTEGER NOT NULL, region_id INTEGER NOT NULL,
            security REAL NOT NULL, map_x REAL NOT NULL, map_y REAL NOT NULL);
        CREATE TABLE gates (from_id INTEGER NOT NULL, to_id INTEGER NOT NULL,
            PRIMARY KEY (from_id, to_id));
        CREATE TABLE region_borders (region_id INTEGER NOT NULL,
            system_id INTEGER NOT NULL, map_x REAL NOT NULL, map_y REAL NOT NULL,
            PRIMARY KEY (region_id, system_id));

        INSERT INTO ship_types VALUES
            (29990, 'Loki', 'Strategic Cruiser'),
            (587, 'Rifter', 'Frigate');
        INSERT INTO regions VALUES (1, 'Alpha'), (2, 'Beta'), (3, 'Empty');
        INSERT INTO constellations VALUES (11, 'Alpha One', 1), (21, 'Beta One', 2);
        INSERT INTO systems VALUES
            (101, 'A1-XYZ', 11, 1, -0.3684, 0, 0),
            (102, 'A2-XYZ', 11, 1, 0.02, 36, 0),
            (103, 'A3-QQQ', 11, 1, 0.46, 72, 0),
            (104, 'A4', 11, 1, -0.04, 108, 0),
            (201, 'B1', 21, 2, 0.9, 0, 0),
            (202, 'B2', 21, 2, 1.0, 36, 0);
        INSERT INTO gates VALUES (101, 102), (102, 103), (103, 104), (102, 201), (201, 202);
        INSERT INTO region_borders VALUES (1, 201, 36, 64), (2, 102, 0, -64);
    ";

    fn sample() -> Universe {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch(SAMPLE_MAP_SQL).unwrap();
        Universe::from_connection(&db).unwrap()
    }

    #[test]
    fn loads_regions_systems_and_borders() {
        let universe = sample();
        let names: Vec<&str> = universe
            .regions_by_name()
            .iter()
            .map(|r| r.name.as_str())
            .collect();
        // "Empty" has no systems, so it isn't offered.
        assert_eq!(names, ["Alpha", "Beta"]);
        let alpha = universe.region(1).unwrap();
        assert_eq!(alpha.systems, [101, 102, 103, 104]);
        assert_eq!(
            alpha.borders,
            [BorderSystem {
                system_id: 201,
                map_pos: [36.0, 64.0]
            }]
        );
        // Wide enough for the label of A4 at x = 108.
        assert_eq!(
            alpha.size,
            [108.0 + 6.0 + 7.0 * 6.0 + 6.0, 64.0 + 3.0 + 14.0]
        );
        assert_eq!(universe.neighbour_regions(1), [2]);
        assert_eq!(universe.neighbour_regions(2), [1]);
    }

    #[test]
    fn finds_systems_and_ships_by_name_ignoring_case() {
        let universe = sample();
        assert_eq!(universe.system_by_name("a3-qqq").unwrap().id, 103);
        assert_eq!(universe.system_by_name("A3-QQQ").unwrap().id, 103);
        assert!(universe.system_by_name("A3").is_none());
        assert_eq!(universe.ship_by_name("LOKI").unwrap().type_id, 29990);
        assert!(universe.ship_by_name("Titan").is_none());
    }

    #[test]
    fn neighbours_go_both_ways() {
        let universe = sample();
        let mut around_a2 = universe.neighbours(102).to_vec();
        around_a2.sort_unstable();
        assert_eq!(around_a2, [101, 103, 201]);
        assert_eq!(universe.neighbours(202), [201]);
        assert!(universe.neighbours(999).is_empty());
    }

    #[test]
    fn jump_distances() {
        let universe = sample();
        assert_eq!(universe.jumps_between(101, 101, 10), Some(0));
        assert_eq!(universe.jumps_between(101, 104, 10), Some(3));
        assert_eq!(universe.jumps_between(104, 202, 10), Some(4));
        // Further than allowed.
        assert_eq!(universe.jumps_between(101, 202, 2), None);
    }

    #[test]
    fn jumps_count_from_the_nearest_start() {
        let universe = sample();
        let jumps = universe.jumps_within(&[101, 202], 10);
        assert_eq!(jumps[&101], 0);
        assert_eq!(jumps[&103], 2);
        assert_eq!(jumps[&104], 3);
        assert_eq!(jumps[&201], 1);
        // Within 1 jump only the starts and their neighbours.
        let mut near: Vec<i64> = universe.jumps_within(&[101], 1).into_keys().collect();
        near.sort_unstable();
        assert_eq!(near, [101, 102]);
        // Unknown systems are ignored.
        assert!(universe.jumps_within(&[999], 5).is_empty());
    }

    #[test]
    fn security_shows_like_in_game() {
        let universe = sample();
        let shown = |id| universe.system(id).unwrap().display_security();
        assert_eq!(shown(101), -0.4);
        // Just above 0.0 is low sec, shown as 0.1.
        assert_eq!(shown(102), 0.1);
        assert_eq!(shown(103), 0.5);
        // Rounds to 0.0 without a minus sign.
        assert_eq!(shown(104), 0.0);
        assert!(shown(104).is_sign_positive());
    }

    /// Checks a real `static.sqlite` built by `cargo xtask sde`. Run with
    /// `ATHENA_STATIC=path/to/static.sqlite cargo test -- --ignored real_map`.
    #[test]
    #[ignore]
    fn real_map() {
        let path = std::env::var("ATHENA_STATIC").unwrap_or("static.sqlite".to_string());
        let data = crate::static_data::StaticData::load(std::path::Path::new(&path)).unwrap();
        let universe = &data.universe;
        let jita = universe.system_by_name("Jita").unwrap();
        let perimeter = universe.system_by_name("Perimeter").unwrap();
        assert_eq!(universe.jumps_between(jita.id, perimeter.id, 10), Some(1));
        let branch = universe
            .regions_by_name()
            .into_iter()
            .find(|r| r.name == "Branch")
            .unwrap();
        assert_eq!(branch.systems.len(), 94);
        assert!(!branch.borders.is_empty());
        assert!(universe.ship_by_name("loki").is_some());

        // How long a 10-jump search from four pilots takes.
        let starts: Vec<i64> = branch.systems.iter().take(4).copied().collect();
        let started = std::time::Instant::now();
        let reach = universe.jumps_within(&starts, 10);
        println!(
            "10 jumps from 4 systems: {} systems in {:?}",
            reach.len(),
            started.elapsed()
        );
    }
}
