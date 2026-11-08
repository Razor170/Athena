//! The sessions shown in the History tab. "End session" ends every
//! character's session at once; together they form one History entry, which
//! can be opened to see each pilot.

use super::gamelog::LogTime;
use super::session::FinishedSession;

/// One History entry: the pilots that mined together, ended with one click
/// of "End session".
#[derive(Debug, Clone, PartialEq)]
pub struct GroupSession {
    pub started: LogTime,
    pub ended: LogTime,
    /// The per-character sessions, ordered by start time.
    pub pilots: Vec<FinishedSession>,
}

impl GroupSession {
    /// Builds the History entry from the sessions ended together. `None` if
    /// nobody mined.
    pub fn from_pilots(mut pilots: Vec<FinishedSession>) -> Option<GroupSession> {
        pilots.sort_by_key(|pilot| pilot.started);
        let started = pilots.first()?.started;
        let ended = pilots.iter().map(|pilot| pilot.ended).max()?;
        Some(GroupSession {
            started,
            ended,
            pilots,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn at(secs: i64) -> LogTime {
        LogTime {
            unix_seconds: 1_794_096_000 + secs,
        }
    }

    fn session(character: &str, start: i64, end: i64) -> FinishedSession {
        FinishedSession {
            character: character.to_string(),
            started: at(start),
            ended: at(end),
            mined_units: BTreeMap::new(),
            voided_units: BTreeMap::new(),
            cycles: 1,
        }
    }

    #[test]
    fn group_spans_all_pilots() {
        let group = GroupSession::from_pilots(vec![
            session("Pilot Bravo", 60, 7_200),
            session("Pilot Alpha", 0, 7_000),
            session("Pilot Charlie", 3_600, 9_000),
        ])
        .unwrap();
        assert_eq!(group.started, at(0));
        assert_eq!(group.ended, at(9_000));
        let names: Vec<&str> = group.pilots.iter().map(|p| p.character.as_str()).collect();
        assert_eq!(names, ["Pilot Alpha", "Pilot Bravo", "Pilot Charlie"]);
    }

    #[test]
    fn nobody_mined() {
        assert!(GroupSession::from_pilots(Vec::new()).is_none());
    }
}
