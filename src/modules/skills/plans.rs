//! Skill plans: lists of skill levels to train, shared by all characters
//! and saved in `skills_plans.toml`, plus the sums the Plans view shows
//! (time per entry, the whole plan, the best attribute remap).
//!
//! Plain data and functions, so everything here can be tested without
//! the UI.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::data::{
    ATTR_CHARISMA, ATTR_INTELLIGENCE, ATTR_MEMORY, ATTR_PERCEPTION, ATTR_WILLPOWER, Attributes,
    CharacterSkills, sp_for_level,
};
use crate::static_data::StaticData;

const FILE_NAME: &str = "skills_plans.toml";
/// Undo keeps at most this many steps per plan.
pub const UNDO_MAX: usize = 100;

/// The five attributes in Athena's order: perception, memory, willpower,
/// intelligence, charisma.
pub type AttributeSet = [i64; 5];
pub const ATTRIBUTE_NAMES: [&str; 5] = [
    "Perception",
    "Memory",
    "Willpower",
    "Intelligence",
    "Charisma",
];
/// A character that never remapped: 20 in each, 19 charisma.
pub const FRESH_ATTRIBUTES: AttributeSet = [20, 20, 20, 20, 19];
/// A remap sets every attribute between these, using all the points.
pub const REMAP_MIN: i64 = 17;
pub const REMAP_MAX: i64 = 27;
pub const REMAP_TOTAL: i64 = 99;

/// The position of a dogma attribute id in an [`AttributeSet`].
pub fn attribute_index(attribute_id: i64) -> Option<usize> {
    match attribute_id {
        ATTR_PERCEPTION => Some(0),
        ATTR_MEMORY => Some(1),
        ATTR_WILLPOWER => Some(2),
        ATTR_INTELLIGENCE => Some(3),
        ATTR_CHARISMA => Some(4),
        _ => None,
    }
}

pub fn attribute_set(a: &Attributes) -> AttributeSet {
    [
        a.perception,
        a.memory,
        a.willpower,
        a.intelligence,
        a.charisma,
    ]
}

// ---------------------------------------------------------------------------
// The plans and their file.

/// One skill level in a plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanEntry {
    pub skill_id: i64,
    /// 1 to 5.
    pub level: u8,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub name: String,
    /// When it was last saved, Unix seconds.
    pub changed: i64,
    pub entries: Vec<PlanEntry>,
}

/// An unsaved plan, kept over a restart.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Draft {
    /// Which saved plan it edits (`None`: a new plan never saved).
    pub index: Option<usize>,
    pub plan: Plan,
}

/// What `skills_plans.toml` holds.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlansFile {
    pub plans: Vec<Plan>,
    /// The open plan with changes not saved yet when Athena closed.
    pub draft: Option<Draft>,
}

impl PlansFile {
    pub fn file_path(data_dir: &Path) -> PathBuf {
        data_dir.join(FILE_NAME)
    }

    /// Reads the file, or an empty list if there is none. A broken file
    /// is first copied to `skills_plans.broken-<now>.toml`, so the next
    /// save can't wipe plans; then the list starts empty with a message.
    /// If even the copy fails, `read_only` is true and nothing is saved.
    pub fn load(data_dir: &Path, now: i64) -> (PlansFile, Option<String>, bool) {
        let path = PlansFile::file_path(data_dir);
        let problem = match std::fs::read_to_string(&path) {
            Ok(text) => match toml::from_str::<PlansFile>(&text) {
                Ok(file) => return (file, None, false),
                Err(e) => e.to_string(),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return (PlansFile::default(), None, false);
            }
            Err(e) => e.to_string(),
        };
        let backup = data_dir.join(format!("skills_plans.broken-{now}.toml"));
        match std::fs::copy(&path, &backup) {
            Ok(_) => (
                PlansFile::default(),
                Some(format!(
                    "Couldn't read {FILE_NAME} ({problem}). It was copied to {} and the \
                     plans start empty.",
                    backup.display()
                )),
                false,
            ),
            Err(e) => (
                PlansFile::default(),
                Some(format!(
                    "Couldn't read {FILE_NAME} ({problem}), and couldn't copy it aside \
                     ({e}). Plans won't be saved until Athena restarts."
                )),
                true,
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// Editing.

/// The highest level of a skill the plan reaches (0 if it's not in it).
pub fn planned_level(plan: &Plan, skill_id: i64) -> u8 {
    plan.entries
        .iter()
        .filter(|e| e.skill_id == skill_id)
        .map(|e| e.level)
        .max()
        .unwrap_or(0)
}

/// Adds a skill up to `level`, with every prerequisite and lower level
/// it still needs in front. `trained` is the character's level of a
/// skill (0 when checking from scratch). Levels already trained or
/// already in the plan aren't added again.
pub fn add_with_prereqs(
    plan: &mut Plan,
    data: &StaticData,
    skill_id: i64,
    level: u8,
    trained: &dyn Fn(i64) -> u8,
) {
    add_inner(plan, data, skill_id, level.clamp(1, 5), trained, 0);
}

fn add_inner(
    plan: &mut Plan,
    data: &StaticData,
    skill_id: i64,
    level: u8,
    trained: &dyn Fn(i64) -> u8,
    depth: u8,
) {
    // The SDE has no loops, but a broken file must not hang Athena.
    if depth > 20 {
        return;
    }
    let have = trained(skill_id).max(planned_level(plan, skill_id));
    if have >= level {
        return;
    }
    if let Some(skill) = data.skill(skill_id) {
        for &(required, required_level) in &skill.requirements {
            add_inner(plan, data, required, required_level, trained, depth + 1);
        }
    }
    for l in (have + 1)..=level {
        plan.entries.push(PlanEntry { skill_id, level: l });
    }
}

/// Takes a skill out of the plan from `level` up, and every later entry
/// that needs it.
pub fn remove_from(plan: &mut Plan, data: &StaticData, skill_id: i64, level: u8) {
    let mut removed: Vec<(i64, u8)> = vec![(skill_id, level)];
    let mut kept = Vec::with_capacity(plan.entries.len());
    for entry in &plan.entries {
        let gone = removed
            .iter()
            .any(|&(skill, from)| entry.skill_id == skill && entry.level >= from)
            || data.skill(entry.skill_id).is_some_and(|s| {
                s.requirements.iter().any(|&(required, needed)| {
                    removed
                        .iter()
                        .any(|&(skill, from)| skill == required && needed >= from)
                })
            });
        if gone {
            // Whatever needed this entry's level goes too.
            removed.push((entry.skill_id, entry.level));
        } else {
            kept.push(*entry);
        }
    }
    plan.entries = kept;
}

/// Removes one entry (by position), and every later entry that needs it.
pub fn remove_entry(plan: &mut Plan, data: &StaticData, index: usize) {
    if let Some(entry) = plan.entries.get(index).copied() {
        remove_from(plan, data, entry.skill_id, entry.level);
    }
}

/// Moves the entry at `from` to `to`, if every entry still comes after
/// its prerequisites and lower levels. Returns false (and changes
/// nothing) when the move would break that.
pub fn move_entry(plan: &mut Plan, data: &StaticData, from: usize, to: usize) -> bool {
    if from >= plan.entries.len() || to >= plan.entries.len() || from == to {
        return false;
    }
    let mut entries = plan.entries.clone();
    let entry = entries.remove(from);
    entries.insert(to, entry);
    if !in_order(&entries, data) {
        return false;
    }
    plan.entries = entries;
    true
}

/// True when no entry comes before a lower level of its skill or before
/// a prerequisite that the plan itself trains.
fn in_order(entries: &[PlanEntry], data: &StaticData) -> bool {
    for (i, entry) in entries.iter().enumerate() {
        let later = &entries[i + 1..];
        if later
            .iter()
            .any(|e| e.skill_id == entry.skill_id && e.level < entry.level)
        {
            return false;
        }
        if let Some(skill) = data.skill(entry.skill_id) {
            for &(required, needed) in &skill.requirements {
                if later
                    .iter()
                    .any(|e| e.skill_id == required && e.level <= needed)
                {
                    return false;
                }
            }
        }
    }
    true
}

/// Entries a character has already trained.
pub fn without_trained(plan: &Plan, trained: &dyn Fn(i64) -> u8) -> Vec<PlanEntry> {
    plan.entries
        .iter()
        .copied()
        .filter(|e| e.level > trained(e.skill_id))
        .collect()
}

// ---------------------------------------------------------------------------
// Import and export.

/// What a pasted plan held.
#[derive(Debug, Default, PartialEq)]
pub struct Imported {
    /// (skill, level), in the pasted order.
    pub skills: Vec<(i64, u8)>,
    /// Lines that aren't a known skill and level.
    pub skipped: Vec<String>,
}

/// Reads a plan: one skill per line, "Skill name 4", "Skill name IV" or
/// "Skill name Level 4" (tabs work too). Empty lines are ignored.
pub fn parse_plan(text: &str, data: &StaticData) -> Imported {
    let mut out = Imported::default();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match parse_line(line, data) {
            Some(found) => out.skills.push(found),
            None => out.skipped.push(line.to_string()),
        }
    }
    out
}

fn parse_line(line: &str, data: &StaticData) -> Option<(i64, u8)> {
    let (name, level) = line.trim().rsplit_once(char::is_whitespace)?;
    let level = match level.to_ascii_uppercase().as_str() {
        "1" | "I" => 1,
        "2" | "II" => 2,
        "3" | "III" => 3,
        "4" | "IV" => 4,
        "5" | "V" => 5,
        _ => return None,
    };
    let name = name.trim();
    let name = name
        .strip_suffix("Level")
        .or_else(|| name.strip_suffix("level"))
        .unwrap_or(name);
    Some((data.skill_id_by_name(name)?, level))
}

/// The plan as text: one "Skill name level" line per entry, which EVE's
/// skill plan import and [`parse_plan`] read.
pub fn export_text(plan: &Plan, data: &StaticData) -> String {
    let mut text = String::new();
    for entry in &plan.entries {
        if let Some(skill) = data.skill(entry.skill_id) {
            text.push_str(&format!("{} {}\n", skill.name, entry.level));
        }
    }
    text
}

/// A file name for an exported plan: the plan's name without characters
/// Windows doesn't allow.
pub fn export_file_name(plan: &Plan) -> String {
    let clean: String = plan
        .name
        .chars()
        .map(|c| {
            if r#"<>:"/\|?*"#.contains(c) || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let clean = clean.trim().trim_end_matches('.');
    if clean.is_empty() {
        "plan.txt".to_string()
    } else {
        format!("{clean}.txt")
    }
}

// ---------------------------------------------------------------------------
// Time.

/// Where a character stands for checking a plan.
#[derive(Debug, Clone, Default)]
pub struct Standing {
    /// Skill id -> (trained level, skill points).
    pub skills: BTreeMap<i64, (u8, i64)>,
}

impl Standing {
    /// A character's skills as of `now` (queue entries that finished
    /// count as trained).
    pub fn of(skills: &CharacterSkills, now: i64) -> Standing {
        let mut out = BTreeMap::new();
        for s in &skills.skills {
            let level = skills.level_at(s.skill_id, now).clamp(0, 5) as u8;
            out.insert(s.skill_id, (level, s.sp));
        }
        for e in &skills.queue {
            if e.finish.is_some_and(|f| f <= now) {
                let level = skills.level_at(e.skill_id, now).clamp(0, 5) as u8;
                let sp = e.level_end_sp.unwrap_or(0);
                let slot = out.entry(e.skill_id).or_insert((0, 0));
                slot.0 = slot.0.max(level);
                slot.1 = slot.1.max(sp);
            }
        }
        Standing { skills: out }
    }

    pub fn level(&self, skill_id: i64) -> u8 {
        self.skills.get(&skill_id).map_or(0, |s| s.0)
    }

    fn sp(&self, skill_id: i64) -> i64 {
        self.skills.get(&skill_id).map_or(0, |s| s.1)
    }
}

/// The skill points one entry still needs: from where the skill is now
/// (or the start of the level) to the end of the level. 0 when trained.
pub fn entry_sp(data: &StaticData, entry: PlanEntry, standing: &Standing) -> i64 {
    let Some(skill) = data.skill(entry.skill_id) else {
        return 0;
    };
    // A level ESI reports as trained needs nothing more, whatever the SP say.
    if standing.level(entry.skill_id) >= entry.level {
        return 0;
    }
    let level = i64::from(entry.level);
    let end = sp_for_level(skill.rank, level);
    let start = sp_for_level(skill.rank, level - 1).max(standing.sp(entry.skill_id));
    (end - start).max(0)
}

/// Seconds to train `sp` of a skill with these attributes (Omega):
/// primary + secondary / 2 skill points per minute.
pub fn train_secs(sp: i64, primary: i64, secondary: i64) -> f64 {
    let per_minute = primary as f64 + secondary as f64 / 2.0;
    if per_minute <= 0.0 {
        return 0.0;
    }
    sp as f64 / per_minute * 60.0
}

/// One entry's training time with `attributes`.
pub fn entry_secs(
    data: &StaticData,
    entry: PlanEntry,
    standing: &Standing,
    attributes: &AttributeSet,
) -> f64 {
    let Some(skill) = data.skill(entry.skill_id) else {
        return 0.0;
    };
    let (Some(p), Some(s)) = (
        attribute_index(skill.primary_attribute),
        attribute_index(skill.secondary_attribute),
    ) else {
        return 0.0;
    };
    train_secs(
        entry_sp(data, entry, standing),
        attributes[p],
        attributes[s],
    )
}

/// The whole plan's training time with `attributes`.
pub fn plan_secs(
    data: &StaticData,
    plan: &Plan,
    standing: &Standing,
    attributes: &AttributeSet,
) -> f64 {
    plan.entries
        .iter()
        .map(|&e| entry_secs(data, e, standing, attributes))
        .sum()
}

/// The skill points the plan still needs, summed by (primary, secondary)
/// attribute position: everything a remap search needs to know.
fn sp_by_attributes(
    data: &StaticData,
    plan: &Plan,
    standing: &Standing,
) -> BTreeMap<(usize, usize), i64> {
    let mut out = BTreeMap::new();
    for &entry in &plan.entries {
        let Some(skill) = data.skill(entry.skill_id) else {
            continue;
        };
        let (Some(p), Some(s)) = (
            attribute_index(skill.primary_attribute),
            attribute_index(skill.secondary_attribute),
        ) else {
            continue;
        };
        *out.entry((p, s)).or_insert(0) += entry_sp(data, entry, standing);
    }
    out
}

/// How much of the plan's training each attribute drives, 0.0 to 1.0
/// (primary counts fully, secondary half).
pub fn attribute_shares(data: &StaticData, plan: &Plan, standing: &Standing) -> [f64; 5] {
    let mut shares = [0.0; 5];
    for ((p, s), sp) in sp_by_attributes(data, plan, standing) {
        shares[p] += sp as f64;
        shares[s] += sp as f64 / 2.0;
    }
    let total: f64 = shares.iter().sum();
    if total > 0.0 {
        shares.iter_mut().for_each(|x| *x /= total);
    }
    shares
}

/// The remap that trains the plan fastest with these implant `bonuses`,
/// and the plan's time with it.
pub fn best_remap(
    data: &StaticData,
    plan: &Plan,
    standing: &Standing,
    bonuses: &AttributeSet,
) -> (AttributeSet, f64) {
    let sp = sp_by_attributes(data, plan, standing);
    let time = |base: &AttributeSet| -> f64 {
        sp.iter()
            .map(|(&(p, s), &sp)| train_secs(sp, base[p] + bonuses[p], base[s] + bonuses[s]))
            .sum()
    };
    let mut best = FRESH_ATTRIBUTES;
    let mut best_time = f64::INFINITY;
    // Every legal remap: about 1,000 of them, each a handful of sums.
    for per in REMAP_MIN..=REMAP_MAX {
        for mem in REMAP_MIN..=REMAP_MAX {
            for wil in REMAP_MIN..=REMAP_MAX {
                for int in REMAP_MIN..=REMAP_MAX {
                    let cha = REMAP_TOTAL - per - mem - wil - int;
                    if !(REMAP_MIN..=REMAP_MAX).contains(&cha) {
                        continue;
                    }
                    let base = [per, mem, wil, int, cha];
                    let t = time(&base);
                    if t < best_time - 0.5 {
                        best_time = t;
                        best = base;
                    }
                }
            }
        }
    }
    (best, best_time)
}

/// The cerebral accelerators the Check column always lists: the New Eden
/// Store 'Boost' line and the market's Expert Cerebral Accelerator. More
/// can be added from the static data.
pub const DEFAULT_ACCELERATORS: [i64; 7] = [72238, 72239, 72240, 72241, 72242, 77919, 55826];

/// How much longer boosters last for this character with these implants,
/// as a factor (1.0 = base duration). Biology adds its percentage per
/// level, a BY-805/BY-810 implant its own; EVE multiplies the two.
pub fn booster_factor(data: &StaticData, standing: &Standing, implants: &[i64]) -> f64 {
    let mut factor = 1.0;
    for (&skill_id, &(level, _)) in &standing.skills {
        if let Some(percent) = data.booster_duration_bonus(skill_id) {
            factor *= 1.0 + percent * f64::from(level) / 100.0;
        }
    }
    for id in implants {
        if let Some(percent) = data.booster_duration_bonus(*id) {
            factor *= 1.0 + percent / 100.0;
        }
    }
    factor
}

/// Accelerators used back to back for a whole plan: how many, and how much
/// time is left on the last one when the plan is done.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoosterRun {
    pub count: u32,
    pub left_secs: f64,
}

/// `plan_secs` of training with accelerators that each last `one_secs`.
pub fn booster_run(plan_secs: f64, one_secs: f64) -> BoosterRun {
    if one_secs <= 0.0 || plan_secs <= 0.0 {
        return BoosterRun {
            count: 0,
            left_secs: 0.0,
        };
    }
    // Small rounding errors must not ask for one more accelerator.
    let count = ((plan_secs - 0.5) / one_secs).ceil().max(1.0);
    BoosterRun {
        count: count as u32,
        left_secs: (count * one_secs - plan_secs).max(0.0),
    }
}

/// The attribute bonuses of a set of implants.
pub fn implant_bonuses(data: &StaticData, implants: &[i64]) -> AttributeSet {
    let mut sum = [0; 5];
    for id in implants {
        if let Some(implant) = data.attribute_implant(*id) {
            for (s, b) in sum.iter_mut().zip(implant.bonuses) {
                *s += b;
            }
        }
    }
    sum
}

/// Short text for implant bonuses: "+4 Per/Mem/Wil/Int, +3 Cha", or "no
/// attribute implants".
pub fn bonuses_text(bonuses: &AttributeSet) -> String {
    const SHORT: [&str; 5] = ["Per", "Mem", "Wil", "Int", "Cha"];
    let mut parts: Vec<(i64, Vec<&str>)> = Vec::new();
    for (i, &b) in bonuses.iter().enumerate() {
        if b <= 0 {
            continue;
        }
        match parts.iter_mut().find(|(v, _)| *v == b) {
            Some((_, names)) => names.push(SHORT[i]),
            None => parts.push((b, vec![SHORT[i]])),
        }
    }
    if parts.is_empty() {
        return "no attribute implants".to_string();
    }
    parts.sort_by_key(|p| std::cmp::Reverse(p.0));
    parts
        .iter()
        .map(|(v, names)| format!("+{v} {}", names.join("/")))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::static_data::tests::sample;

    const GUNNERY: i64 = 3300;
    const SMALL_HYBRID: i64 = 3301;
    const MEDIUM_HYBRID: i64 = 3304;
    const MINING: i64 = 3386;
    const MINING_UPGRADES: i64 = 22578;

    fn plan(entries: &[(i64, u8)]) -> Plan {
        Plan {
            name: "Test".into(),
            changed: 0,
            entries: entries
                .iter()
                .map(|&(skill_id, level)| PlanEntry { skill_id, level })
                .collect(),
        }
    }

    fn pairs(plan: &Plan) -> Vec<(i64, u8)> {
        plan.entries.iter().map(|e| (e.skill_id, e.level)).collect()
    }

    #[test]
    fn adding_brings_prerequisites_and_lower_levels() {
        let data = sample();
        let mut p = plan(&[]);
        add_with_prereqs(&mut p, &data, MEDIUM_HYBRID, 2, &|_| 0);
        assert_eq!(
            pairs(&p),
            [
                (GUNNERY, 1),
                (GUNNERY, 2),
                (GUNNERY, 3),
                (SMALL_HYBRID, 1),
                (SMALL_HYBRID, 2),
                (SMALL_HYBRID, 3),
                (MEDIUM_HYBRID, 1),
                (MEDIUM_HYBRID, 2),
            ]
        );
        // Trained levels are skipped; levels already planned aren't
        // added twice.
        let mut p = plan(&[(MINING, 4)]);
        add_with_prereqs(&mut p, &data, MINING_UPGRADES, 1, &|id| {
            if id == MINING { 3 } else { 0 }
        });
        assert_eq!(pairs(&p), [(MINING, 4), (MINING_UPGRADES, 1)]);
    }

    #[test]
    fn removing_takes_what_needs_it_along() {
        let data = sample();
        let mut p = plan(&[]);
        add_with_prereqs(&mut p, &data, MEDIUM_HYBRID, 1, &|_| 0);
        add_with_prereqs(&mut p, &data, MINING, 2, &|_| 0);
        // Gunnery III goes, so Small Hybrid III (needs it? no: needs I)
        // stays, but Medium Hybrid (needs Gunnery III) goes.
        remove_from(&mut p, &data, GUNNERY, 3);
        assert_eq!(
            pairs(&p),
            [
                (GUNNERY, 1),
                (GUNNERY, 2),
                (SMALL_HYBRID, 1),
                (SMALL_HYBRID, 2),
                (SMALL_HYBRID, 3),
                (MINING, 1),
                (MINING, 2),
            ]
        );
        // Gunnery I goes: everything gunnery goes.
        remove_from(&mut p, &data, GUNNERY, 1);
        assert_eq!(pairs(&p), [(MINING, 1), (MINING, 2)]);
    }

    #[test]
    fn moves_keep_prerequisites_first() {
        let data = sample();
        let mut p = plan(&[(GUNNERY, 1), (MINING, 1), (SMALL_HYBRID, 1)]);
        assert!(move_entry(&mut p, &data, 1, 0));
        assert_eq!(pairs(&p), [(MINING, 1), (GUNNERY, 1), (SMALL_HYBRID, 1)]);
        // Small Hybrid can't go above Gunnery.
        assert!(!move_entry(&mut p, &data, 2, 0));
        // Level II can't go above level I.
        let mut p = plan(&[(MINING, 1), (MINING, 2)]);
        assert!(!move_entry(&mut p, &data, 1, 0));
    }

    #[test]
    fn imports_names_with_numbers_or_roman_levels() {
        let data = sample();
        let text = "Gunnery 3\n\nmining IV\nMining Upgrades\tLevel 2\nNo Such Skill 5\nGunnery";
        let imported = parse_plan(text, &data);
        assert_eq!(
            imported.skills,
            [(GUNNERY, 3), (MINING, 4), (MINING_UPGRADES, 2)]
        );
        assert_eq!(imported.skipped, ["No Such Skill 5", "Gunnery"]);
        let p = plan(&[(GUNNERY, 1), (MINING, 4)]);
        let text = export_text(&p, &data);
        assert_eq!(text, "Gunnery 1\nMining 4\n");
        assert_eq!(parse_plan(&text, &data).skills, [(GUNNERY, 1), (MINING, 4)]);
    }

    #[test]
    fn export_file_names_are_safe() {
        let mut p = plan(&[]);
        p.name = "Orca: links/boosts?".into();
        assert_eq!(export_file_name(&p), "Orca_ links_boosts_.txt");
        p.name = "  ".into();
        assert_eq!(export_file_name(&p), "plan.txt");
    }

    #[test]
    fn times_count_only_what_is_missing() {
        let data = sample();
        // Mining: memory primary, intelligence secondary.
        let attributes = [20, 20, 20, 20, 19];
        let p = plan(&[(MINING, 1), (MINING, 2)]);
        let fresh = Standing::default();
        // 1,415 SP at 30 SP/min.
        let secs = plan_secs(&data, &p, &fresh, &attributes);
        assert!((secs - 1415.0 / 30.0 * 60.0).abs() < 0.01);
        // Level I trained, half of level II's points already in.
        let mut standing = Standing::default();
        standing.skills.insert(MINING, (1, 800));
        assert_eq!(entry_sp(&data, p.entries[0], &standing), 0);
        assert_eq!(entry_sp(&data, p.entries[1], &standing), 615);
    }

    #[test]
    fn best_remap_favours_the_plan_attributes() {
        let data = sample();
        // Mining Upgrades V: memory / intelligence.
        let mut p = plan(&[]);
        add_with_prereqs(&mut p, &data, MINING_UPGRADES, 5, &|_| 0);
        let none = [0; 5];
        let (remap, secs) = best_remap(&data, &p, &Standing::default(), &none);
        assert_eq!(remap, [17, 27, 17, 21, 17]);
        assert!(secs < plan_secs(&data, &p, &Standing::default(), &FRESH_ATTRIBUTES));
        assert_eq!(remap.iter().sum::<i64>(), REMAP_TOTAL);
        let shares = attribute_shares(&data, &p, &Standing::default());
        assert!((shares[1] - 2.0 / 3.0).abs() < 1e-9);
        assert!((shares[3] - 1.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn implants_add_up() {
        let data = sample();
        let bonuses = implant_bonuses(&data, &[9899, 10209, 3300]);
        assert_eq!(bonuses, [3, 5, 0, 0, 0]);
        assert_eq!(bonuses_text(&bonuses), "+5 Mem, +3 Per");
        assert_eq!(bonuses_text(&[4, 4, 4, 4, 3]), "+4 Per/Mem/Wil/Int, +3 Cha");
        assert_eq!(bonuses_text(&[0; 5]), "no attribute implants");
    }

    #[test]
    fn plans_file_round_trips_and_survives_breaking() {
        let dir = std::env::temp_dir().join(format!("athena-plans-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = PlansFile {
            plans: vec![plan(&[(GUNNERY, 1), (MINING, 2)])],
            draft: Some(Draft {
                index: Some(0),
                plan: plan(&[(GUNNERY, 1)]),
            }),
        };
        let path = PlansFile::file_path(&dir);
        std::fs::write(&path, toml::to_string(&file).unwrap()).unwrap();
        let (loaded, problem, read_only) = PlansFile::load(&dir, 1);
        assert_eq!(loaded, file);
        assert!(problem.is_none() && !read_only);
        std::fs::write(&path, "[[plans]]\nname = \"half").unwrap();
        let (loaded, problem, read_only) = PlansFile::load(&dir, 77);
        assert!(loaded.plans.is_empty() && !read_only);
        assert!(problem.unwrap().contains("skills_plans.broken-77.toml"));
        assert!(dir.join("skills_plans.broken-77.toml").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn accelerators_back_to_back() {
        let day = 86_400.0;
        let run = booster_run(10.0 * day, 4.0 * day);
        assert_eq!(run.count, 3);
        assert!((run.left_secs - 2.0 * day).abs() < 1.0);
        // Exactly two: no third one.
        assert_eq!(booster_run(8.0 * day, 4.0 * day).count, 2);
        assert_eq!(booster_run(0.0, 4.0 * day).count, 0);
        assert_eq!(booster_run(1.0, 4.0 * day).count, 1);
    }

    #[test]
    fn biology_and_implants_stretch_accelerators() {
        let data = sample();
        let mut standing = Standing::default();
        assert_eq!(booster_factor(&data, &standing, &[]), 1.0);
        // Biology IV (+80%) and BY-810 (+10%): 1.8 * 1.1.
        standing.skills.insert(3405, (4, 0));
        let factor = booster_factor(&data, &standing, &[27148, 9899]);
        assert!((factor - 1.98).abs() < 1e-9);
    }
}
