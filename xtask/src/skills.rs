//! The skills part of `cargo xtask sde`, for the Skills module:
//!
//! - every published skill group (Armor, Drones, Gunnery, ...),
//! - every published skill with its group, rank (training time multiplier)
//!   and its primary and secondary attribute.
//!
//! - each skill's prerequisites (skill and level),
//! - every implant that raises an attribute, with its bonuses, for the
//!   skill plans' implant sets.
//!
//! The rank, attributes, prerequisites and implant bonuses are dogma
//! attributes in `typeDogma.jsonl`.

use std::collections::BTreeMap;

use anyhow::{Result, bail};
use serde::Deserialize;

/// The SDE category that holds all skills.
pub const CATEGORY_SKILL: i64 = 16;
/// Dogma attribute: the skill's rank ("skillTimeConstant").
const ATTR_RANK: i64 = 275;
/// Dogma attributes: the skill's primary and secondary attribute, as the
/// id of the attribute (164 charisma ... 168 willpower).
const ATTR_PRIMARY: i64 = 180;
const ATTR_SECONDARY: i64 = 181;
/// The SDE category of implants (and boosters).
pub const CATEGORY_IMPLANT: i64 = 20;
/// Dogma attributes: required skill 1 to 6 and the level each needs.
/// Slots 4 to 6 came later, so their ids aren't in order.
const REQUIREMENTS: [(i64, i64); 6] = [
    (182, 277),
    (183, 278),
    (184, 279),
    (1285, 1286),
    (1289, 1287),
    (1290, 1288),
];
/// Dogma attributes: an implant's bonus to perception, memory, willpower,
/// intelligence and charisma, in that order (Athena's attribute order).
const IMPLANT_BONUSES: [i64; 5] = [178, 177, 179, 176, 175];

/// One line of `typeDogma.jsonl` (only the fields we need).
#[derive(Deserialize)]
pub struct SdeTypeDogma {
    #[serde(rename = "_key")]
    pub id: i64,
    #[serde(rename = "dogmaAttributes", default)]
    pub attributes: Vec<SdeDogmaValue>,
}

#[derive(Deserialize)]
pub struct SdeDogmaValue {
    #[serde(rename = "attributeID")]
    pub attribute_id: i64,
    pub value: f64,
}

/// A skill type from `types.jsonl`, already known to be in a skill group.
pub struct SkillType {
    pub id: i64,
    pub group_id: i64,
    pub name: String,
}

/// A skill ready to be written.
pub struct Skill {
    pub name: String,
    pub group_id: i64,
    pub rank: i64,
    pub primary_attribute: i64,
    pub secondary_attribute: i64,
    /// Prerequisites: (skill type id, level).
    pub requirements: Vec<(i64, i64)>,
}

/// A published implant type, already known to be in the implant category.
pub struct ImplantType {
    pub id: i64,
    pub name: String,
}

/// An implant that raises at least one attribute.
pub struct Implant {
    pub name: String,
    /// Perception, memory, willpower, intelligence, charisma.
    pub bonuses: [i64; 5],
}

pub struct SkillData {
    /// Group id -> name.
    pub groups: BTreeMap<i64, String>,
    /// Type id -> skill.
    pub skills: BTreeMap<i64, Skill>,
    /// Type id -> attribute implant.
    pub implants: BTreeMap<i64, Implant>,
}

/// Puts the skills together with their dogma values. `groups` are the
/// published skill groups, `types` the published skills in them,
/// `implant_types` the published implants.
pub fn build(
    groups: BTreeMap<i64, String>,
    types: Vec<SkillType>,
    implant_types: Vec<ImplantType>,
    dogma: Vec<SdeTypeDogma>,
) -> Result<SkillData> {
    let dogma: BTreeMap<i64, SdeTypeDogma> = dogma.into_iter().map(|d| (d.id, d)).collect();
    let skills = join(types, &dogma);
    let implants = implants(implant_types, &dogma);
    // Sanity checks, so a changed SDE format fails loudly.
    if groups.len() < 20 {
        bail!(
            "only {} skill groups found; has the SDE format changed?",
            groups.len()
        );
    }
    for (name, rank) in [("Gunnery", 1), ("Capital Ships", 14)] {
        if !skills.values().any(|s| s.name == name && s.rank == rank) {
            bail!("skill {name} (rank {rank}) not found; has the SDE format changed?");
        }
    }
    // Mining Barge needs Astrogeology III, among others.
    let barge = skills.values().find(|s| s.name == "Mining Barge");
    if !barge.is_some_and(|b| b.requirements.len() >= 2) {
        bail!("Mining Barge has no prerequisites; has the SDE format changed?");
    }
    if !implants
        .values()
        .any(|i| i.name == "Ocular Filter - Basic" && i.bonuses == [3, 0, 0, 0, 0])
    {
        bail!("Ocular Filter - Basic (+3 perception) not found; has the SDE format changed?");
    }
    Ok(SkillData {
        groups,
        skills,
        implants,
    })
}

/// The value of one dogma attribute of a type.
fn value(row: &SdeTypeDogma, attribute: i64) -> Option<i64> {
    row.attributes
        .iter()
        .find(|a| a.attribute_id == attribute)
        .map(|a| a.value as i64)
}

/// The implants that raise at least one attribute.
fn implants(
    types: Vec<ImplantType>,
    dogma: &BTreeMap<i64, SdeTypeDogma>,
) -> BTreeMap<i64, Implant> {
    let mut implants = BTreeMap::new();
    for t in types {
        let Some(row) = dogma.get(&t.id) else {
            continue;
        };
        let bonuses = IMPLANT_BONUSES.map(|attribute| value(row, attribute).unwrap_or(0));
        if bonuses.iter().any(|&b| b > 0) {
            implants.insert(
                t.id,
                Implant {
                    name: t.name,
                    bonuses,
                },
            );
        }
    }
    implants
}

/// Each skill type with its rank and attributes. Types without a rank
/// can't be trained and are left out.
fn join(types: Vec<SkillType>, dogma: &BTreeMap<i64, SdeTypeDogma>) -> BTreeMap<i64, Skill> {
    let mut skills = BTreeMap::new();
    for t in types {
        let Some(row) = dogma.get(&t.id) else {
            continue;
        };
        let value = |attribute: i64| value(row, attribute);
        // Every real skill has a rank; one without is not trainable.
        let Some(rank) = value(ATTR_RANK) else {
            continue;
        };
        skills.insert(
            t.id,
            Skill {
                name: t.name,
                group_id: t.group_id,
                rank: rank.max(1),
                primary_attribute: value(ATTR_PRIMARY).unwrap_or(0),
                secondary_attribute: value(ATTR_SECONDARY).unwrap_or(0),
                requirements: REQUIREMENTS
                    .iter()
                    .filter_map(|&(skill, level)| {
                        let skill = value(skill)?;
                        (skill > 0).then(|| (skill, value(level).unwrap_or(1).clamp(1, 5)))
                    })
                    .collect(),
            },
        );
    }
    skills
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_types_with_their_dogma() {
        let types = vec![
            SkillType {
                id: 3300,
                group_id: 255,
                name: "Gunnery".into(),
            },
            SkillType {
                id: 1,
                group_id: 255,
                name: "No rank".into(),
            },
        ];
        let dogma = vec![
            SdeTypeDogma {
                id: 3300,
                attributes: vec![
                    SdeDogmaValue {
                        attribute_id: ATTR_RANK,
                        value: 1.0,
                    },
                    SdeDogmaValue {
                        attribute_id: ATTR_PRIMARY,
                        value: 167.0,
                    },
                    SdeDogmaValue {
                        attribute_id: ATTR_SECONDARY,
                        value: 168.0,
                    },
                    SdeDogmaValue {
                        attribute_id: 182,
                        value: 3327.0,
                    },
                    SdeDogmaValue {
                        attribute_id: 277,
                        value: 2.0,
                    },
                    SdeDogmaValue {
                        attribute_id: 1285,
                        value: 3300.0,
                    },
                    SdeDogmaValue {
                        attribute_id: 1286,
                        value: 4.0,
                    },
                ],
            },
            SdeTypeDogma {
                id: 1,
                attributes: Vec::new(),
            },
        ];
        let dogma = dogma.into_iter().map(|d| (d.id, d)).collect();
        let skills = join(types, &dogma);
        assert_eq!(skills.len(), 1);
        let gunnery = &skills[&3300];
        assert_eq!(gunnery.name, "Gunnery");
        assert_eq!(gunnery.group_id, 255);
        assert_eq!(gunnery.rank, 1);
        assert_eq!(gunnery.primary_attribute, 167);
        assert_eq!(gunnery.secondary_attribute, 168);
        assert_eq!(gunnery.requirements, [(3327, 2), (3300, 4)]);
    }

    #[test]
    fn keeps_only_implants_that_raise_an_attribute() {
        let types = vec![
            ImplantType {
                id: 9899,
                name: "Ocular Filter - Basic".into(),
            },
            ImplantType {
                id: 1,
                name: "Hardwiring".into(),
            },
        ];
        let dogma = [
            SdeTypeDogma {
                id: 9899,
                attributes: vec![
                    SdeDogmaValue {
                        attribute_id: 178,
                        value: 3.0,
                    },
                    SdeDogmaValue {
                        attribute_id: 175,
                        value: 0.0,
                    },
                ],
            },
            SdeTypeDogma {
                id: 1,
                attributes: vec![SdeDogmaValue {
                    attribute_id: 175,
                    value: 0.0,
                }],
            },
        ]
        .into_iter()
        .map(|d| (d.id, d))
        .collect();
        let implants = implants(types, &dogma);
        assert_eq!(implants.len(), 1);
        assert_eq!(implants[&9899].bonuses, [3, 0, 0, 0, 0]);
    }
}
