//! What ESI sends about a character's skills, what Athena keeps of it, and
//! the sums the views show (time left, queue end, progress).
//!
//! Everything here is plain data and functions, so it can be tested
//! without the UI or the network.

use serde::{Deserialize, Serialize};

use crate::modules::finance::data::parse_esi_date;

/// Skill points a rank 1 skill needs for each level (index 1 to 5). A
/// skill of rank N needs N times as many.
pub const SP_PER_LEVEL: [i64; 6] = [0, 250, 1_415, 8_000, 45_255, 256_000];

/// The most skills a training queue can hold.
pub const QUEUE_MAX: usize = 150;

/// Dogma attribute ids, as `static.sqlite` lists them for each skill.
pub const ATTR_CHARISMA: i64 = 164;
pub const ATTR_INTELLIGENCE: i64 = 165;
pub const ATTR_MEMORY: i64 = 166;
pub const ATTR_PERCEPTION: i64 = 167;
pub const ATTR_WILLPOWER: i64 = 168;

/// The skill points a skill of `rank` needs to reach `level`.
pub fn sp_for_level(rank: i64, level: i64) -> i64 {
    SP_PER_LEVEL[level.clamp(0, 5) as usize] * rank.max(1)
}

// ---------------------------------------------------------------------------
// What ESI sends.

/// `GET /characters/{id}/skills`.
#[derive(Debug, Deserialize)]
pub struct EsiSkills {
    pub skills: Vec<EsiSkill>,
    pub total_sp: i64,
    #[serde(default)]
    pub unallocated_sp: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct EsiSkill {
    pub skill_id: i64,
    pub skillpoints_in_skill: i64,
    pub trained_skill_level: i64,
    /// Lower than `trained_skill_level` while an Alpha clone can't use
    /// all of it.
    pub active_skill_level: i64,
}

/// One line of `GET /characters/{id}/skillqueue`. The dates are missing
/// while the queue is paused.
#[derive(Debug, Deserialize)]
pub struct EsiQueueEntry {
    pub skill_id: i64,
    pub finished_level: i64,
    pub queue_position: i64,
    #[serde(default)]
    pub start_date: Option<String>,
    #[serde(default)]
    pub finish_date: Option<String>,
    #[serde(default)]
    pub training_start_sp: Option<i64>,
    #[serde(default)]
    pub level_start_sp: Option<i64>,
    #[serde(default)]
    pub level_end_sp: Option<i64>,
}

/// `GET /characters/{id}/attributes`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Attributes {
    pub charisma: i64,
    pub intelligence: i64,
    pub memory: i64,
    pub perception: i64,
    pub willpower: i64,
}

impl Attributes {
    fn value(&self, attribute_id: i64) -> i64 {
        match attribute_id {
            ATTR_CHARISMA => self.charisma,
            ATTR_INTELLIGENCE => self.intelligence,
            ATTR_MEMORY => self.memory,
            ATTR_PERCEPTION => self.perception,
            ATTR_WILLPOWER => self.willpower,
            _ => 0,
        }
    }

    /// Skill points per hour for a skill with these primary and secondary
    /// attributes (Omega clone): primary + secondary / 2 per minute.
    pub fn sp_per_hour(&self, primary: i64, secondary: i64) -> f64 {
        (self.value(primary) as f64 + self.value(secondary) as f64 / 2.0) * 60.0
    }
}

// ---------------------------------------------------------------------------
// What Athena keeps (and saves in `skills-cache.json`).

/// One injected skill.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Skill {
    pub skill_id: i64,
    pub sp: i64,
    pub trained_level: i64,
    pub active_level: i64,
}

/// One queued skill level.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueueEntry {
    pub skill_id: i64,
    /// The level this entry trains to (1 to 5).
    pub level: i64,
    /// Unix seconds; `None` while the queue is paused.
    pub start: Option<i64>,
    pub finish: Option<i64>,
    /// Skill points the skill had when this entry started training.
    pub training_start_sp: Option<i64>,
    /// Skill points at the start and end of this level.
    pub level_start_sp: Option<i64>,
    pub level_end_sp: Option<i64>,
}

/// Everything about one character's skills.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CharacterSkills {
    pub skills: Vec<Skill>,
    pub total_sp: i64,
    pub unallocated_sp: i64,
    /// Sorted by queue position.
    pub queue: Vec<QueueEntry>,
    pub attributes: Option<Attributes>,
    /// When ESI sent this, Unix seconds.
    pub fetched_at: i64,
}

impl CharacterSkills {
    pub fn from_esi(
        skills: EsiSkills,
        queue: Vec<EsiQueueEntry>,
        attributes: Option<Attributes>,
        now: i64,
    ) -> CharacterSkills {
        let mut queue = queue;
        queue.sort_by_key(|e| e.queue_position);
        CharacterSkills {
            skills: skills
                .skills
                .into_iter()
                .map(|s| Skill {
                    skill_id: s.skill_id,
                    sp: s.skillpoints_in_skill,
                    trained_level: s.trained_skill_level,
                    active_level: s.active_skill_level,
                })
                .collect(),
            total_sp: skills.total_sp,
            unallocated_sp: skills.unallocated_sp.unwrap_or(0),
            queue: queue
                .into_iter()
                .map(|e| QueueEntry {
                    skill_id: e.skill_id,
                    level: e.finished_level,
                    start: e.start_date.as_deref().and_then(parse_esi_date),
                    finish: e.finish_date.as_deref().and_then(parse_esi_date),
                    training_start_sp: e.training_start_sp,
                    level_start_sp: e.level_start_sp,
                    level_end_sp: e.level_end_sp,
                })
                .collect(),
            attributes,
            fetched_at: now,
        }
    }

    pub fn skill(&self, skill_id: i64) -> Option<&Skill> {
        self.skills.iter().find(|s| s.skill_id == skill_id)
    }

    /// The level a skill has at `now`: ESI's trained level, raised by
    /// queue entries that finished since ESI sent it.
    pub fn level_at(&self, skill_id: i64, now: i64) -> i64 {
        let trained = self.skill(skill_id).map_or(0, |s| s.trained_level);
        self.queue
            .iter()
            .filter(|e| e.skill_id == skill_id && e.finish.is_some_and(|f| f <= now))
            .map(|e| e.level)
            .fold(trained, i64::max)
    }

    /// The highest level the queue trains this skill to, if it's queued
    /// and not finished yet.
    pub fn queued_level(&self, skill_id: i64, now: i64) -> Option<i64> {
        self.queue
            .iter()
            .filter(|e| e.skill_id == skill_id && e.finish.is_none_or(|f| f > now))
            .map(|e| e.level)
            .max()
    }
}

// ---------------------------------------------------------------------------
// The queue as of now.

/// What state the queue is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueState {
    /// Training; the whole queue ends at this time.
    Training { ends: i64 },
    /// Skills are queued, but the queue is paused.
    Paused,
    /// Nothing (left) to train.
    Empty,
}

/// One queue entry as of now.
#[derive(Debug, Clone, PartialEq)]
pub struct QueueRow {
    pub skill_id: i64,
    pub level: i64,
    /// Seconds this entry still needs. For a paused queue it's worked out
    /// from the attributes and is `None` when they're unknown.
    pub seconds_left: Option<i64>,
    /// When this entry finishes (None while paused).
    pub finish: Option<i64>,
    /// 0.0 to 1.0, how far the level has trained.
    pub progress: f32,
    /// Skill points still needed for this entry.
    pub sp_left: i64,
}

/// The queue entries that aren't finished yet, with times worked out.
#[derive(Debug, Clone, PartialEq)]
pub struct QueueNow {
    pub state: QueueState,
    pub rows: Vec<QueueRow>,
    /// Seconds until the whole queue is done (None: paused and unknown).
    pub seconds_left: Option<i64>,
    pub sp_left: i64,
}

/// Rank and attributes of a skill, from the static data. Given as a
/// function so this file doesn't need the whole `StaticData`.
pub struct SkillInfo {
    pub rank: i64,
    pub primary: i64,
    pub secondary: i64,
}

/// Works out the queue at `now`. `info` gives a skill's rank and
/// attributes (only needed for paused queues and missing SP numbers).
pub fn queue_now(
    data: &CharacterSkills,
    now: i64,
    info: impl Fn(i64) -> Option<SkillInfo>,
) -> QueueNow {
    let open: Vec<&QueueEntry> = data
        .queue
        .iter()
        .filter(|e| e.finish.is_none_or(|f| f > now))
        .collect();
    if open.is_empty() {
        return QueueNow {
            state: QueueState::Empty,
            rows: Vec::new(),
            seconds_left: Some(0),
            sp_left: 0,
        };
    }
    // ESI leaves out the dates of every entry while the queue is paused.
    let paused = open.iter().all(|e| e.finish.is_none());

    let mut rows = Vec::new();
    let mut total_seconds: Option<i64> = Some(0);
    let mut total_sp = 0;
    for entry in open {
        let skill = info(entry.skill_id);
        let rank = skill.as_ref().map_or(1, |s| s.rank);
        let start_sp = entry
            .level_start_sp
            .unwrap_or_else(|| sp_for_level(rank, entry.level - 1));
        let end_sp = entry
            .level_end_sp
            .unwrap_or_else(|| sp_for_level(rank, entry.level));
        let level_sp = (end_sp - start_sp).max(1);

        // Skill points the skill has now.
        let sp_now = match (entry.start, entry.finish) {
            (Some(start), Some(finish)) if start <= now && finish > start => {
                let from = entry.training_start_sp.unwrap_or(start_sp);
                let share = (now - start) as f64 / (finish - start) as f64;
                from + ((end_sp - from) as f64 * share) as i64
            }
            (Some(start), _) if start > now => entry.training_start_sp.unwrap_or(start_sp),
            _ => {
                // Paused, or no dates: what the skill had, if it's partly
                // trained (only the first entry of a skill can be).
                let had = data.skill(entry.skill_id).map_or(0, |s| s.sp);
                had.clamp(start_sp, end_sp)
            }
        };
        let sp_left = (end_sp - sp_now).max(0);
        let progress = ((sp_now - start_sp) as f32 / level_sp as f32).clamp(0.0, 1.0);

        let seconds_left = match entry.finish {
            Some(finish) => Some((finish - now).max(0)),
            None => match (data.attributes, &skill) {
                (Some(attributes), Some(skill)) => {
                    let per_hour = attributes.sp_per_hour(skill.primary, skill.secondary);
                    (per_hour > 0.0).then(|| (sp_left as f64 / per_hour * 3600.0).ceil() as i64)
                }
                _ => None,
            },
        };
        total_seconds = match (total_seconds, seconds_left) {
            (Some(total), Some(seconds)) if entry.finish.is_none() => Some(total + seconds),
            (Some(_), Some(_)) => entry.finish.map(|f| f - now),
            _ => None,
        };
        total_sp += sp_left;
        rows.push(QueueRow {
            skill_id: entry.skill_id,
            level: entry.level,
            seconds_left: if entry.finish.is_some() {
                // The time this entry itself takes from now or its start.
                entry
                    .finish
                    .map(|f| f - entry.start.map_or(now, |s| s.max(now)))
            } else {
                seconds_left
            },
            finish: entry.finish,
            progress,
            sp_left,
        });
    }
    let state = if paused {
        QueueState::Paused
    } else {
        let ends = rows.iter().filter_map(|r| r.finish).max().unwrap_or(now);
        QueueState::Training { ends }
    };
    QueueNow {
        state,
        rows,
        seconds_left: total_seconds,
        sp_left: total_sp,
    }
}

/// "12d 18h 28m", "4h 03m", "12m": a duration for the views. Days drop
/// the minutes from 10 days on.
pub fn duration_text(seconds: i64) -> String {
    let seconds = seconds.max(0);
    let days = seconds / 86_400;
    let hours = seconds % 86_400 / 3600;
    let minutes = seconds % 3600 / 60;
    if days >= 10 {
        format!("{days}d {hours}h")
    } else if days > 0 {
        format!("{days}d {hours}h {minutes}m")
    } else if hours > 0 {
        format!("{hours}h {minutes:02}m")
    } else if seconds > 0 && minutes == 0 {
        "<1m".to_string()
    } else {
        format!("{minutes}m")
    }
}

/// Roman numerals for skill levels.
pub fn roman(level: i64) -> &'static str {
    match level {
        1 => "I",
        2 => "II",
        3 => "III",
        4 => "IV",
        5 => "V",
        _ => "0",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: i64 = 1_794_960_000; // 2026-11-18 ...

    fn info(_: i64) -> Option<SkillInfo> {
        Some(SkillInfo {
            rank: 1,
            primary: ATTR_PERCEPTION,
            secondary: ATTR_WILLPOWER,
        })
    }

    fn entry(skill_id: i64, level: i64, start: Option<i64>, finish: Option<i64>) -> QueueEntry {
        QueueEntry {
            skill_id,
            level,
            start,
            finish,
            training_start_sp: None,
            level_start_sp: Some(sp_for_level(1, level - 1)),
            level_end_sp: Some(sp_for_level(1, level)),
        }
    }

    #[test]
    fn reads_esi_json() {
        let skills: EsiSkills = serde_json::from_str(
            r#"{"skills":[{"skill_id":3300,"skillpoints_in_skill":256000,
                "trained_skill_level":5,"active_skill_level":5}],
                "total_sp":5000000,"unallocated_sp":355000}"#,
        )
        .unwrap();
        let queue: Vec<EsiQueueEntry> = serde_json::from_str(
            r#"[{"skill_id":3301,"finished_level":2,"queue_position":1,
                 "start_date":"2026-11-18T10:00:00Z","finish_date":"2026-11-18T11:00:00Z",
                 "training_start_sp":250,"level_start_sp":250,"level_end_sp":1415},
                {"skill_id":3301,"finished_level":1,"queue_position":0,
                 "start_date":"2026-11-18T09:00:00Z","finish_date":"2026-11-18T10:00:00Z",
                 "training_start_sp":0,"level_start_sp":0,"level_end_sp":250}]"#,
        )
        .unwrap();
        let data = CharacterSkills::from_esi(skills, queue, None, T0);
        assert_eq!(data.unallocated_sp, 355_000);
        assert_eq!(data.queue[0].level, 1);
        assert_eq!(data.queue[1].level, 2);
        assert_eq!(data.queue[0].finish, parse_esi_date("2026-11-18T10:00:00Z"));
        // A paused queue: no dates.
        let paused: Vec<EsiQueueEntry> =
            serde_json::from_str(r#"[{"skill_id":3301,"finished_level":1,"queue_position":0}]"#)
                .unwrap();
        assert!(paused[0].finish_date.is_none());
    }

    #[test]
    fn training_queue_counts_down_from_now() {
        let data = CharacterSkills {
            queue: vec![
                // Finished already: left out.
                entry(1, 1, Some(T0 - 7200), Some(T0 - 3600)),
                entry(2, 4, Some(T0 - 3600), Some(T0 + 3600)),
                entry(3, 1, Some(T0 + 3600), Some(T0 + 4600)),
            ],
            ..CharacterSkills::default()
        };
        let q = queue_now(&data, T0, info);
        assert_eq!(q.state, QueueState::Training { ends: T0 + 4600 });
        assert_eq!(q.seconds_left, Some(4600));
        assert_eq!(q.rows.len(), 2);
        assert_eq!(q.rows[0].seconds_left, Some(3600));
        assert!((q.rows[0].progress - 0.5).abs() < 0.01);
        assert_eq!(q.rows[1].seconds_left, Some(1000));
        assert_eq!(q.rows[1].progress, 0.0);
        assert_eq!(q.rows[1].sp_left, 250);
        // The finished entry counts as trained.
        assert_eq!(data.level_at(1, T0), 1);
        assert_eq!(data.queued_level(1, T0), None);
        assert_eq!(data.queued_level(2, T0), Some(4));
    }

    #[test]
    fn empty_and_finished_queues_are_empty() {
        let data = CharacterSkills {
            queue: vec![entry(1, 1, Some(T0 - 7200), Some(T0 - 3600))],
            ..CharacterSkills::default()
        };
        assert_eq!(queue_now(&data, T0, info).state, QueueState::Empty);
        assert_eq!(
            queue_now(&CharacterSkills::default(), T0, info).state,
            QueueState::Empty
        );
    }

    #[test]
    fn paused_queue_uses_attributes() {
        let data = CharacterSkills {
            queue: vec![entry(1, 1, None, None), entry(1, 2, None, None)],
            attributes: Some(Attributes {
                charisma: 20,
                intelligence: 20,
                memory: 20,
                perception: 20,
                willpower: 20,
            }),
            ..CharacterSkills::default()
        };
        let q = queue_now(&data, T0, info);
        assert_eq!(q.state, QueueState::Paused);
        // 30 SP a minute: 250 SP is 8.33 min, 1165 SP is 38.8 min.
        assert_eq!(q.rows[0].seconds_left, Some(500));
        assert_eq!(q.rows[1].seconds_left, Some(2330));
        assert_eq!(q.seconds_left, Some(2830));
        assert_eq!(q.sp_left, 1415);
        // Without attributes there's no time.
        let data = CharacterSkills {
            attributes: None,
            ..data
        };
        assert_eq!(queue_now(&data, T0, info).seconds_left, None);
    }

    #[test]
    fn durations_read_well() {
        assert_eq!(duration_text(12 * 86_400 + 18 * 3600 + 28 * 60), "12d 18h");
        assert_eq!(duration_text(86_400 + 3600 + 60), "1d 1h 1m");
        assert_eq!(duration_text(4 * 3600 + 3 * 60), "4h 03m");
        assert_eq!(duration_text(12 * 60), "12m");
        assert_eq!(duration_text(30), "<1m");
        assert_eq!(duration_text(0), "0m");
    }
}
