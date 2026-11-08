//! The live state that must survive a restart, saved as
//! `mining_state.toml` in the data folder:
//!
//! - what is in each ore hold (unloading at a station leaves no trace in the
//!   log, so the hold can't be worked out again from the logs),
//! - each character's running session,
//! - how far each game log file has been read, so after a restart nothing
//!   is counted twice and nothing is missed.
//!
//! All three are saved together, so they always match each other.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::session::ActiveSession;
use super::watcher::FileOffset;

const FILE_NAME: &str = "mining_state.toml";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedCharacter {
    pub name: String,
    pub hold_units: BTreeMap<String, u64>,
    pub session: Option<ActiveSession>,
    /// EVE said the hold is full and nothing has freed room since.
    pub hold_reported_full: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedState {
    pub characters: Vec<SavedCharacter>,
    pub files: Vec<FileOffset>,
}

impl SavedState {
    pub fn file_path(data_dir: &Path) -> PathBuf {
        data_dir.join(FILE_NAME)
    }

    /// Loads the saved state. A missing file is a first start. A broken one
    /// starts fresh, with a warning for the user. Runs once at startup.
    pub fn load(data_dir: &Path) -> (SavedState, Option<String>) {
        let path = SavedState::file_path(data_dir);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return (SavedState::default(), None);
        };
        match toml::from_str(&text) {
            Ok(state) => (state, None),
            Err(e) => (
                SavedState::default(),
                Some(format!(
                    "{} could not be read, so ore holds start empty: {e}",
                    path.display()
                )),
            ),
        }
    }

    pub fn to_toml(&self) -> Result<String, toml::ser::Error> {
        toml::to_string(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::mining::gamelog::LogTime;

    #[test]
    fn round_trip() {
        let mut hold = BTreeMap::new();
        hold.insert("Compressed Veldspar".to_string(), 1_234);
        let mut mined = BTreeMap::new();
        mined.insert("Veldspar".to_string(), 5_000);
        let state = SavedState {
            characters: vec![SavedCharacter {
                name: "Pilot Alpha".to_string(),
                hold_units: hold,
                session: Some(ActiveSession {
                    started: LogTime { unix_seconds: 100 },
                    last_cycle: LogTime { unix_seconds: 200 },
                    mined_units: mined,
                    cycles: 3,
                }),
                hold_reported_full: true,
            }],
            files: vec![FileOffset {
                file: "20261108_073012_1.txt".to_string(),
                character: "Pilot Alpha".to_string(),
                offset: 4_096,
            }],
        };
        let text = state.to_toml().unwrap();
        let back: SavedState = toml::from_str(&text).unwrap();
        assert_eq!(back, state);
    }
}
