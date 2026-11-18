//! Static game data from EVE's static data export (SDE): ore, ice, moon ore
//! and gas types with their volumes, what they reprocess into, and which
//! type each one compresses into. Also the map (regions, systems, gates)
//! and ship names, in [`Universe`]. And NPC corporations with the stations
//! of their loyalty point stores, for the Finance module. And planetary
//! commodities, colony structures and factory schematics, for the
//! Planetary module. And remote rep and cap transmitter modules plus
//! structure type names, for the CIC module.
//!
//! The data comes from `static.sqlite`, which `cargo xtask sde` builds from
//! the official SDE (see `xtask/src/main.rs`). CI builds it and puts it in the
//! release zip next to `Athena.exe`; it is not committed to the repo.
//!
//! [`StaticData::load`] reads the whole file into memory (about 20,000 rows,
//! well under a megabyte), so lookups afterwards are plain map lookups.
//! Loading takes a few tens of milliseconds; modules do it once at startup,
//! before the window is shown. Don't call it while the UI is running.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::universe::Universe;

/// The file name, next to the exe.
pub const FILE_NAME: &str = "static.sqlite";

/// The `schema_version` this code can read. Must match the xtask.
const SCHEMA_VERSION: &str = "10";

/// What kind of thing an item is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    /// Asteroid belt ore (Veldspar, Mordunium, ...), compressed or not.
    Ore,
    /// Moon ore (Bitumens, Coesite, ...).
    MoonOre,
    Ice,
    Gas,
    /// Something ore reprocesses into: minerals, moon materials, ice
    /// products. Only needed to look up prices.
    Material,
}

impl ItemKind {
    fn from_db(text: &str) -> Option<ItemKind> {
        match text {
            "ore" => Some(ItemKind::Ore),
            "moon" => Some(ItemKind::MoonOre),
            "ice" => Some(ItemKind::Ice),
            "gas" => Some(ItemKind::Gas),
            "material" => Some(ItemKind::Material),
            _ => None,
        }
    }
}

/// One item type.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub type_id: i64,
    /// English name, as the game logs and belt scans write it.
    pub name: String,
    /// Volume of one unit in m³.
    pub volume_m3: f64,
    /// Reprocessing works in portions of this many units (100 for most ore,
    /// 1 for ice).
    pub portion_size: i64,
    /// The SDE group, e.g. "Veldspar", "Ice", "Rare Moon Asteroids".
    pub group_name: String,
    pub kind: ItemKind,
}

/// An NPC station with a loyalty point store.
#[derive(Debug, Clone, PartialEq)]
pub struct LpStation {
    pub station_id: i64,
    pub system_id: i64,
    /// For example "Jita IV - Moon 4 - Caldari Navy Assembly Plant".
    pub name: String,
}

/// What a colony structure on a planet is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PiStructureKind {
    CommandCenter,
    /// The extractor control unit.
    Extractor,
    /// Basic, advanced and high-tech industry facilities.
    Factory,
    Storage,
    Launchpad,
}

impl PiStructureKind {
    fn from_db(text: &str) -> Option<PiStructureKind> {
        match text {
            "command_center" => Some(PiStructureKind::CommandCenter),
            "extractor" => Some(PiStructureKind::Extractor),
            "factory" => Some(PiStructureKind::Factory),
            "storage" => Some(PiStructureKind::Storage),
            "launchpad" => Some(PiStructureKind::Launchpad),
            _ => None,
        }
    }
}

/// A colony structure type, such as "Temperate Launchpad".
#[derive(Debug, Clone, PartialEq)]
pub struct PiStructure {
    pub name: String,
    pub kind: PiStructureKind,
    /// How much it holds in m³; 0 for factories and extractors.
    pub capacity_m3: f64,
}

/// A planetary commodity, P0 (raw resource) to P4.
#[derive(Debug, Clone, PartialEq)]
pub struct PiCommodity {
    pub name: String,
    pub volume_m3: f64,
    pub tier: i64,
}

/// A factory schematic: what one cycle takes and makes.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PiSchematic {
    pub name: String,
    pub cycle_secs: i64,
    /// (type id, quantity per cycle).
    pub inputs: Vec<(i64, i64)>,
    /// (type id, quantity per cycle).
    pub output: Option<(i64, i64)>,
}

/// What a remote module repairs or sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteKind {
    Shield,
    Armor,
    Cap,
}

/// A skill, for the Skills module.
#[derive(Debug, Clone, PartialEq)]
pub struct SkillType {
    pub type_id: i64,
    pub name: String,
    pub group_id: i64,
    /// Multiplies the skill points each level needs.
    pub rank: i64,
    /// Dogma attribute ids (164 charisma, 165 intelligence, 166 memory,
    /// 167 perception, 168 willpower).
    pub primary_attribute: i64,
    pub secondary_attribute: i64,
    /// Prerequisites: (skill type id, level).
    pub requirements: Vec<(i64, u8)>,
}

/// An implant that raises attributes, for the skill plans.
#[derive(Debug, Clone, PartialEq)]
pub struct AttributeImplant {
    pub name: String,
    /// Bonus to perception, memory, willpower, intelligence, charisma.
    pub bonuses: [i64; 5],
}

/// A cerebral accelerator: a booster that raises attributes for a while.
#[derive(Debug, Clone, PartialEq)]
pub struct Accelerator {
    pub type_id: i64,
    pub name: String,
    /// Bonus to perception, memory, willpower, intelligence, charisma.
    pub bonuses: [i64; 5],
    /// How long it lasts before Biology and implants, in hours.
    pub hours: f64,
    /// The last moment it can be plugged in (unix seconds), 0 for never.
    pub expires: i64,
}

/// A skill group, such as "Gunnery", with its skills sorted by name.
#[derive(Debug, Clone, PartialEq)]
pub struct SkillGroup {
    pub group_id: i64,
    pub name: String,
    pub skill_ids: Vec<i64>,
}

/// All static data Athena uses, in memory.
#[derive(Debug, Clone, Default)]
pub struct StaticData {
    /// The SDE build the file was made from, shown in Settings.
    pub sde_build: String,
    items: HashMap<i64, Item>,
    id_by_name: HashMap<String, i64>,
    /// type id -> (material id, quantity per portion at 100% yield).
    reprocess: HashMap<i64, Vec<(i64, i64)>>,
    /// type id -> its compressed type id.
    compressed: HashMap<i64, i64>,
    /// The map and ship names, for the Intel Supervisor.
    pub universe: Universe,
    /// NPC corporation id -> name.
    corporation_names: HashMap<i64, String>,
    /// NPC corporation id -> its stations that have its LP store.
    lp_stations: HashMap<i64, Vec<LpStation>>,
    /// Planetary commodities by type id.
    pi_commodities: HashMap<i64, PiCommodity>,
    /// Colony structures by type id.
    pi_structures: HashMap<i64, PiStructure>,
    /// Factory schematics by schematic id.
    pi_schematics: HashMap<i64, PiSchematic>,
    /// Remote shield boosters, armor repairers and cap transmitters, sorted
    /// by name.
    remote_modules: Vec<(String, RemoteKind)>,
    /// Names of structure types, such as "Metenox Moon Drill".
    structure_types: std::collections::HashSet<String>,
    /// Names of drone and fighter types, such as "Hammerhead II".
    drone_types: std::collections::HashSet<String>,
    /// Type ids of charges (ammo, scripts, crystals).
    charge_types: std::collections::HashSet<i64>,
    /// Skills by type id.
    skills: HashMap<i64, SkillType>,
    /// Skill groups, sorted by name.
    skill_groups: Vec<SkillGroup>,
    /// Lower-case skill name -> type id, for importing plans.
    skill_by_name: HashMap<String, i64>,
    /// Implants that raise attributes, by type id.
    implants: HashMap<i64, AttributeImplant>,
    /// Cerebral accelerators, sorted by name.
    accelerators: Vec<Accelerator>,
    /// Skills (per level) and implants that make boosters last longer:
    /// type id -> (name, percent).
    booster_duration_bonuses: HashMap<i64, (String, f64)>,
}

impl StaticData {
    /// Where to look for `static.sqlite`: next to the exe (the release zip),
    /// then the current folder (handy for `cargo run` from the repo).
    pub fn find_file() -> Option<PathBuf> {
        let next_to_exe = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join(FILE_NAME)));
        let in_current_dir = Some(PathBuf::from(FILE_NAME));
        [next_to_exe, in_current_dir]
            .into_iter()
            .flatten()
            .find(|path| path.is_file())
    }

    /// Reads the whole file. Does file I/O: not on the UI thread.
    pub fn load(path: &Path) -> Result<StaticData> {
        // Read-only, so a copy in a read-only folder (Program Files) works too.
        let db =
            rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .with_context(|| format!("opening {}", path.display()))?;
        StaticData::from_connection(&db)
    }

    /// Reads everything from an open database. Split from `load` so tests
    /// can use an in-memory database.
    pub fn from_connection(db: &rusqlite::Connection) -> Result<StaticData> {
        let meta = |key: &str| -> Result<String> {
            db.query_row("SELECT value FROM meta WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .with_context(|| format!("static data has no {key}"))
        };
        let version = meta("schema_version")?;
        if version != SCHEMA_VERSION {
            bail!(
                "static data has schema version {version}, this Athena needs {SCHEMA_VERSION}; rebuild it with `cargo xtask sde`"
            );
        }
        let mut data = StaticData {
            sde_build: meta("sde_build")?,
            ..StaticData::default()
        };

        let mut query = db.prepare(
            "SELECT type_id, name, volume_m3, portion_size, group_name, kind FROM items",
        )?;
        // `query_map` runs the closure once per row and gives back an
        // iterator of `Result`s, one per row.
        let rows = query.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, f64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?;
        for row in rows {
            let (type_id, name, volume_m3, portion_size, group_name, kind) = row?;
            let kind = ItemKind::from_db(&kind)
                .with_context(|| format!("unknown item kind {kind} for {name}"))?;
            data.id_by_name.insert(name.clone(), type_id);
            data.items.insert(
                type_id,
                Item {
                    type_id,
                    name,
                    volume_m3,
                    portion_size: portion_size.max(1),
                    group_name,
                    kind,
                },
            );
        }

        let mut query = db.prepare("SELECT type_id, material_id, quantity FROM reprocess")?;
        let rows = query.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        for row in rows {
            let (type_id, material_id, quantity) = row?;
            data.reprocess
                .entry(type_id)
                .or_default()
                .push((material_id, quantity));
        }

        let mut query = db.prepare("SELECT type_id, compressed_type_id FROM compressed")?;
        let rows = query.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?;
        for row in rows {
            let (type_id, compressed_id) = row?;
            data.compressed.insert(type_id, compressed_id);
        }

        let mut query = db.prepare("SELECT corporation_id, name FROM npc_corporations")?;
        let rows = query.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (id, name) = row?;
            data.corporation_names.insert(id, name);
        }

        let mut query = db.prepare(
            "SELECT station_id, system_id, corporation_id, name FROM lp_stations
             ORDER BY station_id",
        )?;
        let rows = query.query_map([], |row| {
            Ok((
                row.get::<_, i64>(2)?,
                LpStation {
                    station_id: row.get(0)?,
                    system_id: row.get(1)?,
                    name: row.get(3)?,
                },
            ))
        })?;
        for row in rows {
            let (corporation_id, station) = row?;
            data.lp_stations
                .entry(corporation_id)
                .or_default()
                .push(station);
        }

        data.read_planetary(db).context("reading planetary data")?;

        let mut query = db.prepare("SELECT name, kind FROM remote_modules ORDER BY name")?;
        let rows = query.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (name, kind) = row?;
            let kind = match kind.as_str() {
                "shield" => RemoteKind::Shield,
                "armor" => RemoteKind::Armor,
                "cap" => RemoteKind::Cap,
                other => bail!("unknown remote module kind {other} for {name}"),
            };
            data.remote_modules.push((name, kind));
        }

        let mut query = db.prepare("SELECT name FROM structure_types")?;
        let rows = query.query_map([], |row| row.get::<_, String>(0))?;
        for row in rows {
            data.structure_types.insert(row?);
        }

        let mut query = db.prepare("SELECT name FROM drone_types")?;
        let rows = query.query_map([], |row| row.get::<_, String>(0))?;
        for row in rows {
            data.drone_types.insert(row?);
        }

        let mut query = db.prepare("SELECT type_id FROM charge_types")?;
        let rows = query.query_map([], |row| row.get::<_, i64>(0))?;
        for row in rows {
            data.charge_types.insert(row?);
        }

        data.read_skills(db).context("reading skills")?;

        if data.items.is_empty() {
            bail!("static data contains no items");
        }
        data.universe = Universe::from_connection(db).context("reading the map")?;
        Ok(data)
    }

    /// Reads `skill_groups` and `skill_types`.
    fn read_skills(&mut self, db: &rusqlite::Connection) -> Result<()> {
        let mut query = db.prepare("SELECT group_id, name FROM skill_groups ORDER BY name")?;
        let rows = query.query_map([], |row| {
            Ok(SkillGroup {
                group_id: row.get(0)?,
                name: row.get(1)?,
                skill_ids: Vec::new(),
            })
        })?;
        for row in rows {
            self.skill_groups.push(row?);
        }
        let mut query = db.prepare(
            "SELECT type_id, name, group_id, rank, primary_attribute, secondary_attribute
             FROM skill_types ORDER BY name",
        )?;
        let rows = query.query_map([], |row| {
            Ok(SkillType {
                type_id: row.get(0)?,
                name: row.get(1)?,
                group_id: row.get(2)?,
                rank: row.get(3)?,
                primary_attribute: row.get(4)?,
                secondary_attribute: row.get(5)?,
                requirements: Vec::new(),
            })
        })?;
        for row in rows {
            let skill = row?;
            // Sorted by name above, so each group's list is too.
            if let Some(group) = self
                .skill_groups
                .iter_mut()
                .find(|g| g.group_id == skill.group_id)
            {
                group.skill_ids.push(skill.type_id);
            }
            self.skill_by_name
                .insert(skill.name.to_lowercase(), skill.type_id);
            self.skills.insert(skill.type_id, skill);
        }
        let mut query = db.prepare(
            "SELECT type_id, required_id, level FROM skill_requirements
             ORDER BY type_id, rowid",
        )?;
        let rows = query.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        for row in rows {
            let (skill, required, level) = row?;
            if let Some(skill) = self.skills.get_mut(&skill) {
                skill.requirements.push((required, level.clamp(1, 5) as u8));
            }
        }
        let mut query = db.prepare(
            "SELECT type_id, name, perception, memory, willpower, intelligence, charisma
             FROM implant_attributes",
        )?;
        let rows = query.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                AttributeImplant {
                    name: row.get(1)?,
                    bonuses: [
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                    ],
                },
            ))
        })?;
        for row in rows {
            let (id, implant) = row?;
            self.implants.insert(id, implant);
        }
        let mut query = db.prepare(
            "SELECT type_id, name, perception, memory, willpower, intelligence, charisma,
                    hours, expires
             FROM accelerators ORDER BY name",
        )?;
        let rows = query.query_map([], |row| {
            Ok(Accelerator {
                type_id: row.get(0)?,
                name: row.get(1)?,
                bonuses: [
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ],
                hours: row.get(7)?,
                expires: row.get(8)?,
            })
        })?;
        for row in rows {
            self.accelerators.push(row?);
        }
        let mut query =
            db.prepare("SELECT type_id, name, percent FROM booster_duration_bonuses")?;
        let rows = query.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, (row.get(1)?, row.get(2)?)))
        })?;
        for row in rows {
            let (id, bonus) = row?;
            self.booster_duration_bonuses.insert(id, bonus);
        }
        Ok(())
    }

    /// Every cerebral accelerator, sorted by name.
    pub fn accelerators(&self) -> &[Accelerator] {
        &self.accelerators
    }

    /// A cerebral accelerator by type id.
    pub fn accelerator(&self, type_id: i64) -> Option<&Accelerator> {
        self.accelerators.iter().find(|a| a.type_id == type_id)
    }

    /// How much longer boosters last thanks to this skill (per level) or
    /// implant, in percent. `None` for everything else.
    pub fn booster_duration_bonus(&self, type_id: i64) -> Option<f64> {
        self.booster_duration_bonuses.get(&type_id).map(|b| b.1)
    }

    /// The name of a skill or implant that makes boosters last longer.
    pub fn booster_duration_name(&self, type_id: i64) -> Option<&str> {
        self.booster_duration_bonuses
            .get(&type_id)
            .map(|b| b.0.as_str())
    }

    /// A skill by its name, any case ("mining barge").
    pub fn skill_id_by_name(&self, name: &str) -> Option<i64> {
        self.skill_by_name.get(&name.trim().to_lowercase()).copied()
    }

    /// An implant that raises attributes, by type id. Other implants
    /// give `None`.
    pub fn attribute_implant(&self, type_id: i64) -> Option<&AttributeImplant> {
        self.implants.get(&type_id)
    }

    /// A skill by type id.
    pub fn skill(&self, type_id: i64) -> Option<&SkillType> {
        self.skills.get(&type_id)
    }

    /// Every skill group, sorted by name.
    pub fn skill_groups(&self) -> &[SkillGroup] {
        &self.skill_groups
    }

    /// Reads the `pi_*` tables.
    fn read_planetary(&mut self, db: &rusqlite::Connection) -> Result<()> {
        let mut query = db.prepare("SELECT type_id, name, volume_m3, tier FROM pi_commodities")?;
        let rows = query.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                PiCommodity {
                    name: row.get(1)?,
                    volume_m3: row.get(2)?,
                    tier: row.get(3)?,
                },
            ))
        })?;
        for row in rows {
            let (id, commodity) = row?;
            self.pi_commodities.insert(id, commodity);
        }

        let mut query = db.prepare("SELECT type_id, name, kind, capacity_m3 FROM pi_structures")?;
        let rows = query.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, f64>(3)?,
            ))
        })?;
        for row in rows {
            let (id, name, kind, capacity_m3) = row?;
            let kind = PiStructureKind::from_db(&kind)
                .with_context(|| format!("unknown structure kind {kind} for {name}"))?;
            self.pi_structures.insert(
                id,
                PiStructure {
                    name,
                    kind,
                    capacity_m3,
                },
            );
        }

        let mut query = db.prepare("SELECT schematic_id, name, cycle_secs FROM pi_schematics")?;
        let rows = query.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        for row in rows {
            let (id, name, cycle_secs) = row?;
            self.pi_schematics.insert(
                id,
                PiSchematic {
                    name,
                    cycle_secs,
                    ..PiSchematic::default()
                },
            );
        }

        let mut query = db.prepare(
            "SELECT schematic_id, type_id, quantity, is_input FROM pi_schematic_types
             ORDER BY schematic_id, type_id",
        )?;
        let rows = query.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, bool>(3)?,
            ))
        })?;
        for row in rows {
            let (id, type_id, quantity, is_input) = row?;
            if let Some(schematic) = self.pi_schematics.get_mut(&id) {
                if is_input {
                    schematic.inputs.push((type_id, quantity));
                } else {
                    schematic.output = Some((type_id, quantity));
                }
            }
        }
        Ok(())
    }

    /// A planetary commodity by type id.
    pub fn pi_commodity(&self, type_id: i64) -> Option<&PiCommodity> {
        self.pi_commodities.get(&type_id)
    }

    /// A colony structure type by type id.
    pub fn pi_structure(&self, type_id: i64) -> Option<&PiStructure> {
        self.pi_structures.get(&type_id)
    }

    /// A factory schematic by schematic id.
    pub fn pi_schematic(&self, schematic_id: i64) -> Option<&PiSchematic> {
        self.pi_schematics.get(&schematic_id)
    }

    /// The names of the remote modules of one kind, sorted.
    pub fn remote_modules(&self, kind: RemoteKind) -> impl Iterator<Item = &str> {
        self.remote_modules
            .iter()
            .filter(move |(_, k)| *k == kind)
            .map(|(name, _)| name.as_str())
    }

    /// True if `name` is a structure type, such as "Metenox Moon Drill".
    pub fn is_structure_type(&self, name: &str) -> bool {
        self.structure_types.contains(name)
    }

    /// Whether `name` is a drone or a fighter (Fittings: drone bay or cargo).
    pub fn is_drone_or_fighter(&self, name: &str) -> bool {
        self.drone_types.contains(name)
    }

    /// Whether `type_id` is a charge: ammo, a script or a crystal
    /// (Fittings: a charge in a slot is loaded, not fitted).
    pub fn is_charge(&self, type_id: i64) -> bool {
        self.charge_types.contains(&type_id)
    }

    /// Looks up an item by its English name, as written in the logs.
    pub fn item_by_name(&self, name: &str) -> Option<&Item> {
        self.items.get(self.id_by_name.get(name)?)
    }

    /// Every ore, ice, moon ore and gas type (not the materials).
    pub fn minable_items(&self) -> impl Iterator<Item = &Item> {
        self.items
            .values()
            .filter(|item| item.kind != ItemKind::Material)
    }

    /// What one unit of `type_id` reprocesses into at 100% yield, as
    /// (material id, quantity per unit). Quantities are fractions because
    /// reprocessing works in portions (usually 100 units).
    ///
    /// Raw gas can't be reprocessed, and some ore has no list of its own in
    /// the SDE. Since compression is 1:1 in units, those use the list of
    /// their compressed type: compressed gas "reprocesses" back into raw gas,
    /// which is the decompression that the gas yield applies to.
    ///
    /// `None` if neither the item nor its compressed type has a list.
    pub fn materials_per_unit(&self, type_id: i64) -> Option<Vec<(i64, f64)>> {
        let source = if self.reprocess.contains_key(&type_id) {
            type_id
        } else {
            *self.compressed.get(&type_id)?
        };
        let materials = self.reprocess.get(&source)?;
        let portion = self.items.get(&source)?.portion_size as f64;
        Some(
            materials
                .iter()
                .map(|&(material_id, quantity)| (material_id, quantity as f64 / portion))
                .collect(),
        )
    }

    /// An NPC corporation's name, such as "Caldari Navy".
    pub fn corporation_name(&self, corporation_id: i64) -> Option<&str> {
        self.corporation_names
            .get(&corporation_id)
            .map(|name| name.as_str())
    }

    /// The NPC corporation's stations that have its loyalty point store.
    /// Empty for corporations without one.
    pub fn lp_stations(&self, corporation_id: i64) -> &[LpStation] {
        self.lp_stations
            .get(&corporation_id)
            .map(|stations| stations.as_slice())
            .unwrap_or(&[])
    }

    /// Every material id any item reprocesses into: the types whose Jita
    /// prices are needed to value ore.
    pub fn material_ids(&self) -> Vec<i64> {
        let mut ids: Vec<i64> = self
            .reprocess
            .values()
            .flatten()
            .map(|&(material_id, _)| material_id)
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A tiny in-memory static database with real SDE numbers for a few
    /// items. Other tests can use it too.
    pub fn sample_db() -> rusqlite::Connection {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch(
            "
            CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
            CREATE TABLE items (type_id INTEGER PRIMARY KEY, name TEXT NOT NULL,
                volume_m3 REAL NOT NULL, portion_size INTEGER NOT NULL,
                group_name TEXT NOT NULL, kind TEXT NOT NULL);
            CREATE TABLE reprocess (type_id INTEGER NOT NULL, material_id INTEGER NOT NULL,
                quantity INTEGER NOT NULL, PRIMARY KEY (type_id, material_id));
            CREATE TABLE compressed (type_id INTEGER PRIMARY KEY,
                compressed_type_id INTEGER NOT NULL);
            CREATE TABLE npc_corporations (corporation_id INTEGER PRIMARY KEY,
                name TEXT NOT NULL);
            CREATE TABLE lp_stations (station_id INTEGER PRIMARY KEY,
                system_id INTEGER NOT NULL, corporation_id INTEGER NOT NULL,
                name TEXT NOT NULL);
            CREATE TABLE pi_commodities (type_id INTEGER PRIMARY KEY, name TEXT NOT NULL,
                volume_m3 REAL NOT NULL, tier INTEGER NOT NULL);
            CREATE TABLE pi_structures (type_id INTEGER PRIMARY KEY, name TEXT NOT NULL,
                kind TEXT NOT NULL, capacity_m3 REAL NOT NULL);
            CREATE TABLE pi_schematics (schematic_id INTEGER PRIMARY KEY, name TEXT NOT NULL,
                cycle_secs INTEGER NOT NULL);
            CREATE TABLE pi_schematic_types (schematic_id INTEGER NOT NULL,
                type_id INTEGER NOT NULL, quantity INTEGER NOT NULL, is_input INTEGER NOT NULL,
                PRIMARY KEY (schematic_id, type_id));
            CREATE TABLE remote_modules (type_id INTEGER PRIMARY KEY, name TEXT NOT NULL,
                kind TEXT NOT NULL);
            CREATE TABLE structure_types (type_id INTEGER PRIMARY KEY, name TEXT NOT NULL);
            CREATE TABLE drone_types (type_id INTEGER PRIMARY KEY, name TEXT NOT NULL);
            CREATE TABLE charge_types (type_id INTEGER PRIMARY KEY, name TEXT NOT NULL);
            CREATE TABLE skill_groups (group_id INTEGER PRIMARY KEY, name TEXT NOT NULL);
            CREATE TABLE skill_types (type_id INTEGER PRIMARY KEY, name TEXT NOT NULL,
                group_id INTEGER NOT NULL, rank INTEGER NOT NULL,
                primary_attribute INTEGER NOT NULL, secondary_attribute INTEGER NOT NULL);
            CREATE TABLE skill_requirements (type_id INTEGER NOT NULL,
                required_id INTEGER NOT NULL, level INTEGER NOT NULL,
                PRIMARY KEY (type_id, required_id));
            CREATE TABLE implant_attributes (type_id INTEGER PRIMARY KEY, name TEXT NOT NULL,
                perception INTEGER NOT NULL, memory INTEGER NOT NULL, willpower INTEGER NOT NULL,
                intelligence INTEGER NOT NULL, charisma INTEGER NOT NULL);
            CREATE TABLE accelerators (type_id INTEGER PRIMARY KEY, name TEXT NOT NULL,
                perception INTEGER NOT NULL, memory INTEGER NOT NULL, willpower INTEGER NOT NULL,
                intelligence INTEGER NOT NULL, charisma INTEGER NOT NULL, hours REAL NOT NULL,
                expires INTEGER NOT NULL);
            CREATE TABLE booster_duration_bonuses (type_id INTEGER PRIMARY KEY,
                name TEXT NOT NULL, percent REAL NOT NULL);

            INSERT INTO meta VALUES ('schema_version', '10'), ('sde_build', '3670638');
            INSERT INTO skill_groups VALUES (255, 'Gunnery'), (257, 'Spaceship Command'),
                (1218, 'Resource Processing');
            INSERT INTO skill_types VALUES
                (3300, 'Gunnery', 255, 1, 167, 168),
                (3301, 'Small Hybrid Turret', 255, 1, 167, 168),
                (3304, 'Medium Hybrid Turret', 255, 3, 167, 168),
                (3327, 'Spaceship Command', 257, 1, 167, 168),
                (24311, 'Amarr Carrier', 257, 14, 167, 168),
                (3386, 'Mining', 1218, 1, 166, 165),
                (22578, 'Mining Upgrades', 1218, 4, 166, 165);
            INSERT INTO skill_requirements VALUES
                (3301, 3300, 1), (3304, 3300, 3), (3304, 3301, 3), (22578, 3386, 4);
            INSERT INTO implant_attributes VALUES
                (9899, 'Ocular Filter - Basic', 3, 0, 0, 0, 0),
                (10209, 'Memory Augmentation - Improved', 0, 5, 0, 0, 0);
            INSERT INTO accelerators VALUES
                (72242, 'Expert ''Boost'' Cerebral Accelerator', 10, 10, 10, 10, 10, 240.0, 0),
                (72238, 'Basic ''Boost'' Cerebral Accelerator', 2, 2, 2, 2, 2, 48.0, 0);
            INSERT INTO booster_duration_bonuses VALUES (3405, 'Biology', 20.0),
                (27148, 'Eifyr and Co. ''Alchemist'' Biology BY-810', 10.0);
            INSERT INTO remote_modules VALUES
                (12102, 'Large Remote Capacitor Transmitter II', 'cap'),
                (12221, 'Medium Remote Capacitor Transmitter II', 'cap'),
                (3608, 'Large Remote Shield Booster II', 'shield'),
                (26914, 'Large Remote Armor Repairer II', 'armor');
            INSERT INTO structure_types VALUES (81826, 'Metenox Moon Drill');
            INSERT INTO drone_types VALUES (2185, 'Hammerhead II'), (40556, 'Einherji II');
            INSERT INTO charge_types VALUES (262, 'Multifrequency L');
            INSERT INTO pi_commodities VALUES
                (2268, 'Aqueous Liquids', 0.005, 0),
                (3645, 'Water', 0.19, 1),
                (2389, 'Plasmoids', 0.19, 1),
                (9832, 'Coolant', 0.75, 2),
                (9838, 'Superconductors', 0.75, 2),
                (2393, 'Bacteria', 0.19, 1),
                (2073, 'Microorganisms', 0.005, 0);
            INSERT INTO pi_structures VALUES
                (2254, 'Temperate Command Center', 'command_center', 500.0),
                (3068, 'Temperate Extractor Control Unit', 'extractor', 0.0),
                (2481, 'Temperate Basic Industry Facility', 'factory', 0.0),
                (2480, 'Temperate Advanced Industry Facility', 'factory', 0.0),
                (2562, 'Temperate Storage Facility', 'storage', 12000.0),
                (2256, 'Temperate Launchpad', 'launchpad', 10000.0);
            INSERT INTO pi_schematics VALUES
                (121, 'Water', 1800),
                (65, 'Superconductors', 3600);
            INSERT INTO pi_schematic_types VALUES
                (121, 2268, 3000, 1),
                (121, 3645, 20, 0),
                (65, 2389, 40, 1),
                (65, 3645, 40, 1),
                (65, 9838, 5, 0);
            INSERT INTO items VALUES
                (1230, 'Veldspar', 0.1, 100, 'Veldspar', 'ore'),
                (62516, 'Compressed Veldspar', 0.001, 100, 'Veldspar', 'ore'),
                (16265, 'White Glaze', 1000.0, 1, 'Ice', 'ice'),
                (30370, 'Fullerite-C50', 1.0, 1, 'Harvestable Cloud', 'gas'),
                (62399, 'Compressed Fullerite-C50', 0.1, 1, 'Compressed Gas', 'gas'),
                (34, 'Tritanium', 0.01, 1, 'Mineral', 'material'),
                (16272, 'Heavy Water', 0.4, 1, 'Ice Product', 'material');
            INSERT INTO reprocess VALUES
                (1230, 34, 400),
                (62516, 34, 400),
                (16265, 16272, 69),
                (62399, 30370, 1);
            INSERT INTO compressed VALUES (1230, 62516), (30370, 62399);
            INSERT INTO npc_corporations VALUES (1000035, 'Caldari Navy'), (1000120, 'DED');
            INSERT INTO lp_stations VALUES
                (60000001, 104, 1000035, 'A4 I - Caldari Navy Logistic Support'),
                (60000002, 202, 1000035, 'B2 III - Moon 1 - Caldari Navy Assembly Plant');
            ",
        )
        .unwrap();
        db.execute_batch(crate::universe::tests::SAMPLE_MAP_SQL)
            .unwrap();
        db
    }

    pub fn sample() -> StaticData {
        StaticData::from_connection(&sample_db()).unwrap()
    }

    #[test]
    fn loads_items_by_name() {
        let data = sample();
        assert_eq!(data.sde_build, "3670638");
        let veldspar = data.item_by_name("Veldspar").unwrap();
        assert_eq!(veldspar.volume_m3, 0.1);
        assert_eq!(veldspar.kind, ItemKind::Ore);
        assert!(data.item_by_name("Mystery Ore").is_none());
        assert_eq!(data.minable_items().count(), 5);
        // The map comes along.
        assert!(data.universe.system_by_name("A1-XYZ").is_some());
    }

    #[test]
    fn materials_per_unit_uses_portions() {
        let data = sample();
        // 400 Tritanium per 100 Veldspar = 4 per unit.
        assert_eq!(data.materials_per_unit(1230), Some(vec![(34, 4.0)]));
        // Ice reprocesses per single unit.
        assert_eq!(data.materials_per_unit(16265), Some(vec![(16272, 69.0)]));
    }

    #[test]
    fn raw_gas_uses_its_compressed_type() {
        let data = sample();
        // Raw gas has no list; compressed gas decompresses 1:1.
        assert_eq!(data.materials_per_unit(30370), Some(vec![(30370, 1.0)]));
        // Tritanium itself can't be reprocessed.
        assert_eq!(data.materials_per_unit(34), None);
    }

    #[test]
    fn material_ids_are_unique() {
        assert_eq!(sample().material_ids(), vec![34, 16272, 30370]);
    }

    #[test]
    fn loads_corporations_and_lp_stations() {
        let data = sample();
        assert_eq!(data.corporation_name(1000035), Some("Caldari Navy"));
        assert_eq!(data.corporation_name(1), None);
        let stations = data.lp_stations(1000035);
        assert_eq!(stations.len(), 2);
        assert_eq!(stations[0].system_id, 104);
        assert!(data.lp_stations(1000120).is_empty());
    }

    #[test]
    fn loads_skills_by_group() {
        let data = sample();
        let names: Vec<&str> = data
            .skill_groups()
            .iter()
            .map(|g| g.name.as_str())
            .collect();
        assert_eq!(
            names,
            ["Gunnery", "Resource Processing", "Spaceship Command"]
        );
        let gunnery = &data.skill_groups()[0];
        assert_eq!(gunnery.skill_ids, [3300, 3304, 3301]);
        let carrier = data.skill(24311).unwrap();
        assert_eq!(carrier.rank, 14);
        assert_eq!(carrier.group_id, 257);
        assert_eq!(
            data.skill(3304).unwrap().requirements,
            [(3300, 3), (3301, 3)]
        );
        assert_eq!(data.skill_id_by_name(" mining upgrades "), Some(22578));
        assert_eq!(
            data.attribute_implant(10209).unwrap().bonuses,
            [0, 5, 0, 0, 0]
        );
        assert!(data.attribute_implant(3300).is_none());
        let names: Vec<&str> = data
            .accelerators()
            .iter()
            .map(|a| a.name.as_str())
            .collect();
        assert_eq!(
            names,
            [
                "Basic 'Boost' Cerebral Accelerator",
                "Expert 'Boost' Cerebral Accelerator"
            ]
        );
        assert_eq!(data.accelerator(72242).unwrap().hours, 240.0);
        assert_eq!(data.booster_duration_bonus(3405), Some(20.0));
        assert_eq!(data.booster_duration_bonus(3300), None);
    }

    #[test]
    fn loads_planetary_data() {
        let data = sample();
        assert_eq!(data.pi_commodity(3645).unwrap().name, "Water");
        let pad = data.pi_structure(2256).unwrap();
        assert_eq!(pad.kind, PiStructureKind::Launchpad);
        assert_eq!(pad.capacity_m3, 10_000.0);
        let schematic = data.pi_schematic(65).unwrap();
        assert_eq!(schematic.cycle_secs, 3600);
        assert_eq!(schematic.inputs, vec![(2389, 40), (3645, 40)]);
        assert_eq!(schematic.output, Some((9838, 5)));
    }

    #[test]
    fn loads_remote_modules_and_structures() {
        let data = sample();
        let caps: Vec<&str> = data.remote_modules(RemoteKind::Cap).collect();
        assert_eq!(
            caps,
            [
                "Large Remote Capacitor Transmitter II",
                "Medium Remote Capacitor Transmitter II"
            ]
        );
        assert_eq!(data.remote_modules(RemoteKind::Armor).count(), 1);
        assert!(data.is_structure_type("Metenox Moon Drill"));
        assert!(!data.is_structure_type("Basilisk"));
        assert!(data.is_drone_or_fighter("Hammerhead II"));
        assert!(!data.is_drone_or_fighter("Nanite Repair Paste"));
        assert!(data.is_charge(262));
        assert!(!data.is_charge(2185));
    }

    #[test]
    fn wrong_schema_version_is_refused() {
        let db = sample_db();
        db.execute(
            "UPDATE meta SET value = '99' WHERE key = 'schema_version'",
            [],
        )
        .unwrap();
        let error = StaticData::from_connection(&db).unwrap_err().to_string();
        assert!(error.contains("schema version 99"), "{error}");
    }

    #[test]
    fn missing_file_is_an_error() {
        assert!(StaticData::load(Path::new("does-not-exist/static.sqlite")).is_err());
    }
}
