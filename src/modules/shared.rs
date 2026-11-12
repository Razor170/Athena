//! Small pieces of state that one module owns and others read.
//!
//! Each is an `Arc<Mutex<…>>`: the `Arc` lets several modules hold the same
//! value, and the `Mutex` makes sure only one of them touches it at a time.
//! Locks are only held for a quick copy, never across I/O.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// A logged-in character, as the Characters module lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct RosterCharacter {
    pub id: u64,
    pub name: String,
    pub tags: Vec<String>,
}

/// The logged-in characters, in the order the Characters tab shows them
/// (accounts first, then unassigned). Written by the Characters module.
#[derive(Debug, Clone, Default)]
pub struct Roster {
    /// Goes up by one with every change, so readers can tell cheaply
    /// whether they need a new copy.
    pub version: u64,
    pub characters: Vec<RosterCharacter>,
}

pub type SharedRoster = Arc<Mutex<Roster>>;

/// Replaces the roster's characters, if they changed.
pub fn publish_roster(roster: &SharedRoster, characters: Vec<RosterCharacter>) {
    if let Ok(mut roster) = roster.lock()
        && roster.characters != characters
    {
        roster.characters = characters;
        roster.version += 1;
    }
}

/// A copy of the roster, if its version isn't `known_version`.
pub fn roster_if_changed(roster: &SharedRoster, known_version: u64) -> Option<Roster> {
    let roster = roster.lock().ok()?;
    (roster.version != known_version).then(|| roster.clone())
}

/// Where a pilot is, from their Local chat log.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PilotSystem {
    pub system_id: i64,
    /// When they arrived there, Unix seconds.
    pub since_unix: i64,
}

/// Pilot name -> the system they're in, from "Channel changed to Local"
/// lines. Written by the Intel module, which reads the Local logs anyway.
/// A pilot in a wormhole (no gates, not on the map) is left out.
pub type PilotSystems = Arc<Mutex<HashMap<String, PilotSystem>>>;

#[cfg(test)]
mod tests {
    use super::*;

    fn character(id: u64) -> RosterCharacter {
        RosterCharacter {
            id,
            name: format!("Pilot {id}"),
            tags: Vec::new(),
        }
    }

    #[test]
    fn version_only_moves_on_a_change() {
        let roster = SharedRoster::default();
        assert!(roster_if_changed(&roster, 0).is_none());
        publish_roster(&roster, vec![character(1)]);
        let copy = roster_if_changed(&roster, 0).unwrap();
        assert_eq!(copy.version, 1);
        publish_roster(&roster, vec![character(1)]);
        assert!(roster_if_changed(&roster, 1).is_none());
        publish_roster(&roster, vec![character(2), character(1)]);
        assert_eq!(roster_if_changed(&roster, 1).unwrap().version, 2);
    }
}
