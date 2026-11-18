//! The skills part of `cargo xtask sde`, for the Skills module:
//!
//! - every published skill group (Armor, Drones, Gunnery, ...),
//! - every published skill with its group, rank (training time multiplier)
//!   and its primary and secondary attribute.
//!
//! The rank and attributes are dogma attributes in `typeDogma.jsonl`.

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
}

pub struct SkillData {
    /// Group id -> name.
    pub groups: BTreeMap<i64, String>,
    /// Type id -> skill.
    pub skills: BTreeMap<i64, Skill>,
}

/// Puts the skills together with their dogma values. `groups` are the
/// published skill groups, `types` the published skills in them.
pub fn build(
    groups: BTreeMap<i64, String>,
    types: Vec<SkillType>,
    dogma: Vec<SdeTypeDogma>,
) -> Result<SkillData> {
    let skills = join(types, dogma);
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
    Ok(SkillData { groups, skills })
}

/// Each skill type with its rank and attributes. Types without a rank
/// can't be trained and are left out.
fn join(types: Vec<SkillType>, dogma: Vec<SdeTypeDogma>) -> BTreeMap<i64, Skill> {
    let dogma: BTreeMap<i64, SdeTypeDogma> = dogma.into_iter().map(|d| (d.id, d)).collect();
    let mut skills = BTreeMap::new();
    for t in types {
        let Some(row) = dogma.get(&t.id) else {
            continue;
        };
        let value = |attribute: i64| {
            row.attributes
                .iter()
                .find(|a| a.attribute_id == attribute)
                .map(|a| a.value as i64)
        };
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
                ],
            },
            SdeTypeDogma {
                id: 1,
                attributes: Vec::new(),
            },
        ];
        let skills = join(types, dogma);
        assert_eq!(skills.len(), 1);
        let gunnery = &skills[&3300];
        assert_eq!(gunnery.name, "Gunnery");
        assert_eq!(gunnery.group_id, 255);
        assert_eq!(gunnery.rank, 1);
        assert_eq!(gunnery.primary_attribute, 167);
        assert_eq!(gunnery.secondary_attribute, 168);
    }
}
