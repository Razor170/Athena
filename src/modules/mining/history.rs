//! Groups finished per-character sessions into the sessions shown in the
//! History tab: everyone who mined at the same time counts as one session,
//! which can be opened to see each pilot.

use super::gamelog::LogTime;
use super::session::FinishedSession;

/// One History entry: the pilots that mined together.
#[derive(Debug, Clone, PartialEq)]
pub struct GroupSession {
    pub started: LogTime,
    pub ended: LogTime,
    /// The per-character sessions, ordered by start time.
    pub pilots: Vec<FinishedSession>,
}

/// Groups sessions that overlap in time, or are less than `gap_secs`
/// apart, into one entry. Returns the newest entry first, as History lists
/// them.
pub fn group_sessions(sessions: &[FinishedSession], gap_secs: i64) -> Vec<GroupSession> {
    let mut sorted: Vec<&FinishedSession> = sessions.iter().collect();
    sorted.sort_by_key(|session| session.started);

    let mut groups: Vec<GroupSession> = Vec::new();
    for session in sorted {
        // `last_mut` gives the group that started most recently, which is
        // the only one this session can join since we go in time order.
        match groups.last_mut() {
            Some(group) if session.started.seconds_since(group.ended) < gap_secs => {
                if session.ended > group.ended {
                    group.ended = session.ended;
                }
                group.pilots.push(session.clone());
            }
            _ => groups.push(GroupSession {
                started: session.started,
                ended: session.ended,
                pilots: vec![session.clone()],
            }),
        }
    }
    groups.reverse();
    groups
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
    fn overlapping_sessions_form_one_group() {
        let sessions = vec![
            session("Pilot Bravo", 60, 7_200),
            session("Pilot Alpha", 0, 7_000),
            // Joins late, but while the others still mine.
            session("Pilot Charlie", 3_600, 9_000),
            // Starts 10 min after the group ended: still the same op.
            session("Pilot Delta", 9_600, 10_000),
            // The next evening: a new group.
            session("Pilot Alpha", 86_400, 90_000),
        ];
        let groups = group_sessions(&sessions, 15 * 60);
        assert_eq!(groups.len(), 2);
        // Newest first.
        assert_eq!(groups[0].pilots.len(), 1);
        let op = &groups[1];
        assert_eq!(op.started, at(0));
        assert_eq!(op.ended, at(10_000));
        let names: Vec<&str> = op.pilots.iter().map(|p| p.character.as_str()).collect();
        assert_eq!(
            names,
            ["Pilot Alpha", "Pilot Bravo", "Pilot Charlie", "Pilot Delta"]
        );
    }

    #[test]
    fn empty_history() {
        assert!(group_sessions(&[], 900).is_empty());
    }
}
