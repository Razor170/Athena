//! Build helpers for Athena. Not part of the app and never shipped.
//!
//! ```text
//! cargo xtask sde <sde-jsonl.zip> [<out file>] [--dotlan <folder>]
//! cargo xtask regions <sde-jsonl.zip>
//! ```
//!
//! Turns the official EVE static data export (SDE) into the small
//! `static.sqlite` that Athena reads at startup. Download the zip from
//! <https://developers.eveonline.com/static-data> ("JSON Lines" format), for
//! example `eve-online-static-data-latest-jsonl.zip`. The output defaults to
//! `static.sqlite` in the current folder; put it next to `Athena.exe` (or in
//! the folder you run `cargo run` from).
//!
//! What goes in (see `create_schema` for the tables):
//! - every published ore, ice, moon ore and gas type, compressed or not, with
//!   its volume (for the ore hold math) and reprocessing portion size,
//! - what each of them reprocesses into (`typeMaterials`),
//! - which type each one compresses into (`compressibleTypes`),
//! - the names of the minerals and other materials they reprocess into, so
//!   their Jita prices can be looked up,
//! - for the Intel Supervisor: regions, constellations, solar systems with
//!   their security and map position, stargate connections and the names of
//!   all ship types (see `map.rs`).
//!
//! With `--dotlan <folder>`, the Intel map's layout comes from DOTLAN
//! EveMaps' region maps in that folder (see `dotlan.rs`). `cargo xtask
//! regions` prints the file name of every region map to fetch, one per line;
//! `xtask/dotlan.ps1` uses it to download them.

mod dotlan;
mod map;

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

/// Bump when the tables change, so the app can refuse a file it can't read.
const SCHEMA_VERSION: i64 = 2;

/// The SDE category that holds all asteroid ore, ice and moon ore.
const CATEGORY_ASTEROID: i64 = 25;
/// Gas: "Harvestable Cloud" (raw gas) and "Compressed Gas".
const GAS_GROUPS: [i64; 2] = [711, 4168];
/// The SDE category of all ships.
const CATEGORY_SHIP: i64 = 6;

/// With Dotlan maps, at least this share of all systems must get their spot
/// from them, or the build fails: a changed Dotlan format must not quietly
/// bring back the old layout.
const MIN_DOTLAN_SHARE: f64 = 0.95;

const USAGE: &str = "usage:
  cargo xtask sde <sde-jsonl.zip> [<out file>] [--dotlan <folder>]
  cargo xtask regions <sde-jsonl.zip>";

fn main() -> Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    // `--dotlan <folder>` may come anywhere after the command.
    let mut dotlan_folder = None;
    if let Some(index) = args.iter().position(|a| a == "--dotlan") {
        let Some(folder) = args.get(index + 1) else {
            bail!("--dotlan needs a folder\n{USAGE}");
        };
        dotlan_folder = Some(PathBuf::from(folder));
        args.drain(index..index + 2);
    }
    match (args.first().map(String::as_str), args.get(1)) {
        (Some("sde"), Some(zip_path)) => {
            let out = args
                .get(2)
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("static.sqlite"));
            build_static_data(Path::new(zip_path), &out, dotlan_folder.as_deref())
        }
        (Some("regions"), Some(zip_path)) => print_region_files(Path::new(zip_path)),
        _ => bail!("{USAGE}"),
    }
}

/// Prints the Dotlan file name of every region the Intel map shows.
fn print_region_files(zip_path: &Path) -> Result<()> {
    let file = File::open(zip_path).with_context(|| format!("opening {}", zip_path.display()))?;
    let mut archive = zip::ZipArchive::new(file).context("the SDE file is not a zip")?;
    let regions = read_map_regions(&mut archive)?;
    for name in regions.values() {
        println!("{}", dotlan::file_name(name));
    }
    Ok(())
}

/// Every region with stargates (id → English name).
fn read_map_regions(archive: &mut zip::ZipArchive<File>) -> Result<BTreeMap<i64, String>> {
    let systems: Vec<map::SdeSolarSystem> = read_jsonl(archive, "mapSolarSystems.jsonl")?;
    let gates: Vec<map::SdeStargate> = read_jsonl(archive, "mapStargates.jsonl")?;
    let regions =
        map::regions_with_gates(read_jsonl(archive, "mapRegions.jsonl")?, &systems, &gates);
    if regions.len() < 60 {
        bail!(
            "only {} regions with gates found; has the SDE format changed?",
            regions.len()
        );
    }
    Ok(regions)
}

/// A name in all languages; we only keep English, which is what the logs use.
#[derive(Deserialize)]
pub struct Name {
    en: Option<String>,
}

/// One line of `types.jsonl` (only the fields we need).
#[derive(Deserialize)]
struct SdeType {
    #[serde(rename = "_key")]
    id: i64,
    #[serde(rename = "groupID")]
    group_id: i64,
    name: Option<Name>,
    volume: Option<f64>,
    #[serde(rename = "portionSize")]
    portion_size: Option<i64>,
    #[serde(default)]
    published: bool,
}

/// One line of `groups.jsonl`.
#[derive(Deserialize)]
struct SdeGroup {
    #[serde(rename = "_key")]
    id: i64,
    #[serde(rename = "categoryID")]
    category_id: i64,
    name: Option<Name>,
}

/// One line of `typeMaterials.jsonl`.
#[derive(Deserialize)]
struct SdeTypeMaterials {
    #[serde(rename = "_key")]
    id: i64,
    #[serde(default)]
    materials: Vec<SdeMaterial>,
}

#[derive(Deserialize)]
struct SdeMaterial {
    #[serde(rename = "materialTypeID")]
    material_id: i64,
    quantity: i64,
}

/// One line of `compressibleTypes.jsonl`.
#[derive(Deserialize)]
struct SdeCompressible {
    #[serde(rename = "_key")]
    id: i64,
    #[serde(rename = "compressedTypeID")]
    compressed_id: i64,
}

/// One line of `_sde.jsonl`: which SDE build this is.
#[derive(Deserialize)]
struct SdeInfo {
    #[serde(rename = "buildNumber")]
    build_number: i64,
    #[serde(rename = "releaseDate")]
    release_date: Option<String>,
}

/// An item we keep, ready to be written.
struct Item {
    name: String,
    volume_m3: f64,
    portion_size: i64,
    group_name: String,
    /// "ore", "moon", "ice", "gas", or "material" for reprocessing outputs.
    kind: &'static str,
}

/// Reads every line of `file_name` inside the zip as JSON into `T`.
fn read_jsonl<T: serde::de::DeserializeOwned>(
    archive: &mut zip::ZipArchive<File>,
    file_name: &str,
) -> Result<Vec<T>> {
    let entry = archive
        .by_name(file_name)
        .with_context(|| format!("{file_name} is missing from the SDE zip"))?;
    let mut rows = Vec::new();
    for (index, line) in BufReader::new(entry).lines().enumerate() {
        let line = line.with_context(|| format!("reading {file_name}"))?;
        if line.trim().is_empty() {
            continue;
        }
        let row = serde_json::from_str(&line)
            .with_context(|| format!("{file_name} line {}", index + 1))?;
        rows.push(row);
    }
    Ok(rows)
}

pub fn english(name: &Option<Name>) -> Option<String> {
    name.as_ref()?.en.clone()
}

/// Sorts an asteroid group into ore, moon ore or ice by its name.
fn asteroid_kind(group_name: &str) -> &'static str {
    if group_name == "Ice" {
        "ice"
    } else if group_name.contains("Moon Asteroids") {
        "moon"
    } else {
        "ore"
    }
}

fn build_static_data(zip_path: &Path, out: &Path, dotlan_folder: Option<&Path>) -> Result<()> {
    let file = File::open(zip_path).with_context(|| format!("opening {}", zip_path.display()))?;
    let mut archive = zip::ZipArchive::new(file).context("the SDE file is not a zip")?;

    let info: Vec<SdeInfo> = read_jsonl(&mut archive, "_sde.jsonl")?;
    let info = info.first().context("_sde.jsonl is empty")?;
    println!("SDE build {}", info.build_number);

    let groups: Vec<SdeGroup> = read_jsonl(&mut archive, "groups.jsonl")?;
    // Group id -> (category id, English name).
    let groups: BTreeMap<i64, (i64, String)> = groups
        .into_iter()
        .map(|g| (g.id, (g.category_id, english(&g.name).unwrap_or_default())))
        .collect();

    println!("Reading types (this is the big one)...");
    let types: Vec<SdeType> = read_jsonl(&mut archive, "types.jsonl")?;
    let materials: Vec<SdeTypeMaterials> = read_jsonl(&mut archive, "typeMaterials.jsonl")?;
    let compressible: Vec<SdeCompressible> = read_jsonl(&mut archive, "compressibleTypes.jsonl")?;

    // Pick the minable types: published ore, ice, moon ore and gas.
    let mut items: BTreeMap<i64, Item> = BTreeMap::new();
    for t in &types {
        let Some((category, group_name)) = groups.get(&t.group_id) else {
            continue;
        };
        let kind = if *category == CATEGORY_ASTEROID {
            asteroid_kind(group_name)
        } else if GAS_GROUPS.contains(&t.group_id) {
            "gas"
        } else {
            continue;
        };
        let (Some(name), Some(volume_m3)) = (english(&t.name), t.volume) else {
            continue;
        };
        // "Batch Compressed ..." are retired types from before the 2022
        // compression change. Nobody mines them, and their name would fool
        // the "Compressed ..." check in the app.
        if !t.published || name.starts_with("Batch Compressed ") {
            continue;
        }
        items.insert(
            t.id,
            Item {
                name,
                volume_m3,
                portion_size: t.portion_size.unwrap_or(1).max(1),
                group_name: group_name.clone(),
                kind,
            },
        );
    }

    // Keep the reprocessing lists of those types, and remember which
    // materials they produce so we can add the materials' names too.
    let mut reprocess: Vec<(i64, i64, i64)> = Vec::new();
    let mut material_ids: BTreeSet<i64> = BTreeSet::new();
    for row in &materials {
        if !items.contains_key(&row.id) {
            continue;
        }
        for m in &row.materials {
            reprocess.push((row.id, m.material_id, m.quantity));
            material_ids.insert(m.material_id);
        }
    }
    for t in &types {
        if !material_ids.contains(&t.id) || items.contains_key(&t.id) {
            continue;
        }
        let Some(name) = english(&t.name) else {
            continue;
        };
        let group_name = groups
            .get(&t.group_id)
            .map(|(_, name)| name.clone())
            .unwrap_or_default();
        items.insert(
            t.id,
            Item {
                name,
                volume_m3: t.volume.unwrap_or(0.0),
                portion_size: t.portion_size.unwrap_or(1).max(1),
                group_name,
                kind: "material",
            },
        );
    }

    // Ship type names, so intel lines like "loki + 2" can be read.
    let mut ships: BTreeMap<i64, (String, String)> = BTreeMap::new();
    for t in &types {
        let Some((category, group_name)) = groups.get(&t.group_id) else {
            continue;
        };
        if *category != CATEGORY_SHIP || !t.published {
            continue;
        }
        if let Some(name) = english(&t.name) {
            ships.insert(t.id, (name, group_name.clone()));
        }
    }
    drop(types);

    println!("Reading the map...");
    // The Dotlan maps of the regions the Intel map shows, if we have them.
    let dotlan_maps = match dotlan_folder {
        Some(folder) => dotlan::load(folder, &read_map_regions(&mut archive)?)?,
        None => None,
    };
    let no_maps = BTreeMap::new();
    let map = map::build(
        read_jsonl(&mut archive, "mapRegions.jsonl")?,
        read_jsonl(&mut archive, "mapConstellations.jsonl")?,
        read_jsonl(&mut archive, "mapSolarSystems.jsonl")?,
        read_jsonl(&mut archive, "mapStargates.jsonl")?,
        dotlan_maps.as_ref().unwrap_or(&no_maps),
    )?;
    if dotlan_maps.is_some() {
        let share = map.from_dotlan as f64 / map.systems.len().max(1) as f64;
        println!(
            "Map layout: {} of {} systems from Dotlan ({:.1}%)",
            map.from_dotlan,
            map.systems.len(),
            share * 100.0
        );
        if share < MIN_DOTLAN_SHARE {
            bail!(
                "only {:.1}% of systems were found on the Dotlan maps (at least {:.0}% needed); \
                 has Dotlan's map format changed?",
                share * 100.0,
                MIN_DOTLAN_SHARE * 100.0
            );
        }
    }

    let compressed: Vec<(i64, i64)> = compressible
        .iter()
        .filter(|c| items.contains_key(&c.id) && items.contains_key(&c.compressed_id))
        .map(|c| (c.id, c.compressed_id))
        .collect();

    // Sanity checks, so a changed SDE format fails loudly instead of
    // producing an empty file.
    for must_have in [
        "Veldspar",
        "Compressed Veldspar",
        "White Glaze",
        "Tritanium",
    ] {
        if !items.values().any(|item| item.name == must_have) {
            bail!("{must_have} not found; has the SDE format changed?");
        }
    }
    if reprocess.is_empty() || compressed.is_empty() {
        bail!("no reprocessing or compression data found; has the SDE format changed?");
    }
    for must_have in ["Rifter", "Loki", "Capsule"] {
        if !ships.values().any(|(name, _)| name == must_have) {
            bail!("ship {must_have} not found; has the SDE format changed?");
        }
    }
    for must_have in ["Jita", "X7R-JW", "Zarzakh"] {
        if !map.systems.values().any(|s| s.name == must_have) {
            bail!("system {must_have} not found; has the SDE format changed?");
        }
    }
    if map.systems.len() < 5000 || map.regions.len() < 60 || map.gates.len() < 6000 {
        bail!(
            "only {} systems, {} regions and {} gates found; has the SDE format changed?",
            map.systems.len(),
            map.regions.len(),
            map.gates.len()
        );
    }

    write_database(out, info, &items, &reprocess, &compressed, &ships, &map)?;
    println!(
        "Wrote {}: {} items, {} reprocessing rows, {} compressible types, {} ships, \
         {} regions, {} systems, {} gates, {} map layout",
        out.display(),
        items.len(),
        reprocess.len(),
        compressed.len(),
        ships.len(),
        map.regions.len(),
        map.systems.len(),
        map.gates.len(),
        map.layout_name()
    );
    Ok(())
}

fn create_schema(db: &rusqlite::Connection) -> Result<()> {
    db.execute_batch(
        "
        -- Key/value facts about the file: schema_version, sde_build,
        -- sde_release_date, map_layout (dotlan, position2d or mixed).
        CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);

        -- Ore, ice, moon ore and gas (compressed and not), plus the materials
        -- they reprocess into. Names are English, as in the game logs.
        CREATE TABLE items (
            type_id      INTEGER PRIMARY KEY,
            name         TEXT NOT NULL,
            volume_m3    REAL NOT NULL,
            portion_size INTEGER NOT NULL,
            group_name   TEXT NOT NULL,
            kind         TEXT NOT NULL  -- ore, moon, ice, gas, material
        );

        -- Reprocessing one portion (items.portion_size units) of type_id at
        -- 100% yield gives `quantity` of material_id.
        CREATE TABLE reprocess (
            type_id     INTEGER NOT NULL,
            material_id INTEGER NOT NULL,
            quantity    INTEGER NOT NULL,
            PRIMARY KEY (type_id, material_id)
        );

        -- type_id compresses 1:1 (in units) into compressed_type_id.
        CREATE TABLE compressed (
            type_id            INTEGER PRIMARY KEY,
            compressed_type_id INTEGER NOT NULL
        );

        -- Every published ship type, for reading intel lines.
        CREATE TABLE ship_types (
            type_id    INTEGER PRIMARY KEY,
            name       TEXT NOT NULL,
            group_name TEXT NOT NULL  -- e.g. Frigate, Strategic Cruiser
        );

        -- The map: only regions, constellations and systems that have
        -- stargates (known space, Pochven, Zarzakh).
        CREATE TABLE regions (
            region_id INTEGER PRIMARY KEY,
            name      TEXT NOT NULL
        );
        CREATE TABLE constellations (
            constellation_id INTEGER PRIMARY KEY,
            name             TEXT NOT NULL,
            region_id        INTEGER NOT NULL
        );
        -- map_x/map_y: where the system is drawn on its region's map, in
        -- points at zoom 1.0, y down, the region's top-left at (0, 0).
        CREATE TABLE systems (
            system_id        INTEGER PRIMARY KEY,
            name             TEXT NOT NULL,
            constellation_id INTEGER NOT NULL,
            region_id        INTEGER NOT NULL,
            security         REAL NOT NULL,  -- unrounded, e.g. -0.3684
            map_x            REAL NOT NULL,
            map_y            REAL NOT NULL
        );
        -- Each gate connection once (from_id < to_id); gates work both ways.
        CREATE TABLE gates (
            from_id INTEGER NOT NULL,
            to_id   INTEGER NOT NULL,
            PRIMARY KEY (from_id, to_id)
        );
        -- Systems of other regions drawn on a region's map (hollow dots)
        -- because a gate leads there, with their spot on that map.
        CREATE TABLE region_borders (
            region_id INTEGER NOT NULL,
            system_id INTEGER NOT NULL,
            map_x     REAL NOT NULL,
            map_y     REAL NOT NULL,
            PRIMARY KEY (region_id, system_id)
        );
        ",
    )?;
    Ok(())
}

fn write_database(
    out: &Path,
    info: &SdeInfo,
    items: &BTreeMap<i64, Item>,
    reprocess: &[(i64, i64, i64)],
    compressed: &[(i64, i64)],
    ships: &BTreeMap<i64, (String, String)>,
    map: &map::MapData,
) -> Result<()> {
    // Write to a temporary file first, so a failed run never leaves a
    // half-written static.sqlite behind.
    let tmp = out.with_extension("sqlite.tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut db =
        rusqlite::Connection::open(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
    create_schema(&db)?;

    // One transaction for everything: much faster than one per row.
    let tx = db.transaction()?;
    {
        let mut meta = tx.prepare("INSERT INTO meta (key, value) VALUES (?1, ?2)")?;
        meta.execute(("schema_version", SCHEMA_VERSION.to_string()))?;
        meta.execute(("sde_build", info.build_number.to_string()))?;
        meta.execute((
            "sde_release_date",
            info.release_date.clone().unwrap_or_default(),
        ))?;
        meta.execute(("map_layout", map.layout_name()))?;

        let mut insert = tx.prepare(
            "INSERT INTO items (type_id, name, volume_m3, portion_size, group_name, kind)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;
        for (id, item) in items {
            insert.execute((
                id,
                &item.name,
                item.volume_m3,
                item.portion_size,
                &item.group_name,
                item.kind,
            ))?;
        }

        let mut insert = tx.prepare(
            "INSERT INTO reprocess (type_id, material_id, quantity) VALUES (?1, ?2, ?3)",
        )?;
        for row in reprocess {
            insert.execute(*row)?;
        }

        let mut insert =
            tx.prepare("INSERT INTO compressed (type_id, compressed_type_id) VALUES (?1, ?2)")?;
        for row in compressed {
            insert.execute(*row)?;
        }

        let mut insert =
            tx.prepare("INSERT INTO ship_types (type_id, name, group_name) VALUES (?1, ?2, ?3)")?;
        for (id, (name, group_name)) in ships {
            insert.execute((id, name, group_name))?;
        }

        let mut insert = tx.prepare("INSERT INTO regions (region_id, name) VALUES (?1, ?2)")?;
        for (id, region) in &map.regions {
            insert.execute((id, &region.name))?;
        }

        let mut insert = tx.prepare(
            "INSERT INTO constellations (constellation_id, name, region_id) VALUES (?1, ?2, ?3)",
        )?;
        for (id, c) in &map.constellations {
            insert.execute((id, &c.name, c.region_id))?;
        }

        let mut insert = tx.prepare(
            "INSERT INTO systems (system_id, name, constellation_id, region_id, security, map_x, map_y)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )?;
        for (id, s) in &map.systems {
            insert.execute((
                id,
                &s.name,
                s.constellation_id,
                s.region_id,
                s.security,
                s.map_x,
                s.map_y,
            ))?;
        }

        let mut insert = tx.prepare("INSERT INTO gates (from_id, to_id) VALUES (?1, ?2)")?;
        for row in &map.gates {
            insert.execute(*row)?;
        }

        let mut insert = tx.prepare(
            "INSERT INTO region_borders (region_id, system_id, map_x, map_y) VALUES (?1, ?2, ?3, ?4)",
        )?;
        for b in &map.borders {
            insert.execute((b.region_id, b.system_id, b.map_x, b.map_y))?;
        }
    }
    tx.commit()?;
    db.execute_batch("VACUUM")?;
    drop(db);

    std::fs::rename(&tmp, out).with_context(|| format!("writing {}", out.display()))?;
    Ok(())
}
