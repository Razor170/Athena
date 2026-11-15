//! The CIC module's own settings, saved as `cic.toml` in the data folder:
//! which characters the Logi view follows, in which order and in which
//! ship, the cap chain modules, how long things stay on screen, and the
//! Logi overlay.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const FILE_NAME: &str = "cic.toml";

/// The logi ships a character can fly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Ship {
    #[default]
    Basilisk,
    Scimitar,
    Guardian,
    Oneiros,
}

impl Ship {
    pub const ALL: [Ship; 4] = [
        Ship::Basilisk,
        Ship::Scimitar,
        Ship::Guardian,
        Ship::Oneiros,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Ship::Basilisk => "Basilisk",
            Ship::Scimitar => "Scimitar",
            Ship::Guardian => "Guardian",
            Ship::Oneiros => "Oneiros",
        }
    }

    /// Shield logi (Basilisk, Scimitar) or armor logi (Guardian, Oneiros).
    pub fn is_shield(self) -> bool {
        matches!(self, Ship::Basilisk | Ship::Scimitar)
    }

    /// Basilisk and Guardian fly in a cap chain.
    pub fn has_cap_chain(self) -> bool {
        matches!(self, Ship::Basilisk | Ship::Guardian)
    }
}

/// One character in the Settings list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogiCharacter {
    pub name: String,
    /// Read this character's combat log and show its card.
    pub show: bool,
    pub ship: Ship,
}

/// Which transmitter sends cap up the chain and which sends it down.
/// Module names as the logs and the SDE write them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainModules {
    pub up: String,
    pub down: String,
}

/// New fields need a default so older files still load.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CicSettings {
    /// In card order. Characters found in the logs later are added at the end.
    pub characters: Vec<LogiCharacter>,
    pub basilisk_chain: ChainModules,
    pub guardian_chain: ChainModules,
    /// Entries per arrow; older ones drop off.
    pub keep_entries: u32,
    /// Entries are hidden this many seconds after they came in, and fade
    /// out over the second half of that time.
    pub keep_secs: u32,
    /// A chain link counts as broken after this many seconds without a
    /// transfer.
    pub chain_secs: u32,
    /// After "You're jammed", the ship can't lock for this many seconds.
    pub ecm_secs: u32,
    pub overlay: OverlaySettings,
}

/// The Logi overlay: one line per shown pilot, over EVE (Windows only).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OverlaySettings {
    pub enabled: bool,
    pub show_names: bool,
    /// Show only pilots who are hit, jammed, tackled or missing a cap
    /// chain link.
    pub only_busy: bool,
    /// Opacity of the background, 0 to 100 percent.
    pub background_opacity: f64,
    /// The width the user pulled it to, in pixels.
    pub width_px: i32,
    /// The top-left corner on the screen, in pixels. `None` until moved.
    pub position: Option<[i32; 2]>,
}

impl Default for OverlaySettings {
    fn default() -> Self {
        OverlaySettings {
            // Off until switched on: no window over EVE nobody asked for.
            enabled: false,
            show_names: true,
            only_busy: false,
            background_opacity: 55.0,
            width_px: 380,
            position: None,
        }
    }
}

impl Default for CicSettings {
    fn default() -> Self {
        CicSettings {
            characters: Vec::new(),
            // Medium goes up the chain, Large goes down (Kevin's setup,
            // with the modules from his log).
            basilisk_chain: ChainModules {
                up: "Medium Remote Capacitor Transmitter II".to_string(),
                down: "Large Inductive Compact Remote Capacitor Transmitter".to_string(),
            },
            guardian_chain: ChainModules {
                up: "Medium Remote Capacitor Transmitter II".to_string(),
                down: "Large Remote Capacitor Transmitter II".to_string(),
            },
            keep_entries: 8,
            keep_secs: 30,
            chain_secs: 12,
            ecm_secs: 20,
            overlay: OverlaySettings::default(),
        }
    }
}

/// Limits of the number fields in Settings, as (min, max).
pub const KEEP_ENTRIES: (u32, u32) = (3, 20);
pub const KEEP_SECS: (u32, u32) = (10, 300);
pub const CHAIN_SECS: (u32, u32) = (6, 60);
pub const ECM_SECS: (u32, u32) = (5, 60);

impl CicSettings {
    pub fn file_path(data_dir: &Path) -> PathBuf {
        data_dir.join(FILE_NAME)
    }

    /// Reads the file, or the defaults if there is none. A broken file
    /// gives the defaults plus a message to show.
    pub fn load(data_dir: &Path) -> (CicSettings, Option<String>) {
        let path = CicSettings::file_path(data_dir);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return (CicSettings::default(), None);
        };
        match toml::from_str::<CicSettings>(&text) {
            Ok(mut settings) => {
                settings.clamp();
                (settings, None)
            }
            Err(e) => (
                CicSettings::default(),
                Some(format!(
                    "{} could not be read, so defaults are used: {e}",
                    path.display()
                )),
            ),
        }
    }

    /// Keeps the numbers inside their limits, e.g. after a hand edit.
    pub fn clamp(&mut self) {
        self.keep_entries = self.keep_entries.clamp(KEEP_ENTRIES.0, KEEP_ENTRIES.1);
        self.keep_secs = self.keep_secs.clamp(KEEP_SECS.0, KEEP_SECS.1);
        self.chain_secs = self.chain_secs.clamp(CHAIN_SECS.0, CHAIN_SECS.1);
        self.ecm_secs = self.ecm_secs.clamp(ECM_SECS.0, ECM_SECS.1);
        let opacity = self.overlay.background_opacity;
        self.overlay.background_opacity = if opacity.is_finite() {
            opacity.clamp(0.0, 100.0)
        } else {
            55.0
        };
    }

    /// Adds characters that aren't in the list yet, at the end, not shown.
    /// True if any were added.
    pub fn add_characters<'a>(&mut self, names: impl IntoIterator<Item = &'a String>) -> bool {
        let mut added = false;
        for name in names {
            if !self.characters.iter().any(|c| &c.name == name) {
                self.characters.push(LogiCharacter {
                    name: name.clone(),
                    show: false,
                    ship: Ship::default(),
                });
                added = true;
            }
        }
        added
    }

    /// The names of the characters whose cards are shown.
    pub fn shown(&self) -> impl Iterator<Item = &LogiCharacter> {
        self.characters.iter().filter(|c| c.show)
    }

    pub fn character(&self, name: &str) -> Option<&LogiCharacter> {
        self.characters.iter().find(|c| c.name == name)
    }

    /// The chain modules of a ship, if it flies in a cap chain.
    pub fn chain_modules(&self, ship: Ship) -> Option<&ChainModules> {
        match ship {
            Ship::Basilisk => Some(&self.basilisk_chain),
            Ship::Guardian => Some(&self.guardian_chain),
            Ship::Scimitar | Ship::Oneiros => None,
        }
    }

    /// Moves `dragged` before or after `target` (drag and drop in Settings).
    pub fn move_character(&mut self, dragged: &str, target: &str, after: bool) {
        let Some(from) = self.characters.iter().position(|c| c.name == dragged) else {
            return;
        };
        let moved = self.characters.remove(from);
        let Some(to) = self.characters.iter().position(|c| c.name == target) else {
            self.characters.insert(from, moved);
            return;
        };
        let to = if after { to + 1 } else { to };
        self.characters.insert(to, moved);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(settings: &CicSettings) -> Vec<&str> {
        settings
            .characters
            .iter()
            .map(|c| c.name.as_str())
            .collect()
    }

    #[test]
    fn new_characters_go_last_and_hidden() {
        let mut settings = CicSettings::default();
        let found = vec!["A".to_string(), "B".to_string()];
        assert!(settings.add_characters(&found));
        assert!(!settings.add_characters(&found));
        assert_eq!(names(&settings), ["A", "B"]);
        assert_eq!(settings.shown().count(), 0);
    }

    #[test]
    fn drag_and_drop_order() {
        let mut settings = CicSettings::default();
        let found: Vec<String> = ["A", "B", "C"].iter().map(|s| s.to_string()).collect();
        settings.add_characters(&found);
        settings.move_character("A", "C", true);
        assert_eq!(names(&settings), ["B", "C", "A"]);
        settings.move_character("A", "B", false);
        assert_eq!(names(&settings), ["A", "B", "C"]);
        settings.move_character("C", "missing", false);
        assert_eq!(names(&settings), ["A", "B", "C"]);
    }

    #[test]
    fn old_or_partial_file_still_loads() {
        let settings: CicSettings = toml::from_str("keep_entries = 5").unwrap();
        assert_eq!(settings.keep_entries, 5);
        assert_eq!(settings.ecm_secs, 20);
    }

    #[test]
    fn saved_and_read_back() {
        let mut settings = CicSettings::default();
        settings.characters.push(LogiCharacter {
            name: "Test Pilot".into(),
            show: true,
            ship: Ship::Guardian,
        });
        let text = toml::to_string(&settings).unwrap();
        let back: CicSettings = toml::from_str(&text).unwrap();
        assert_eq!(back, settings);
    }

    #[test]
    fn numbers_are_kept_in_limits() {
        let mut settings = CicSettings {
            keep_entries: 0,
            ecm_secs: 1000,
            ..CicSettings::default()
        };
        settings.clamp();
        assert_eq!(settings.keep_entries, KEEP_ENTRIES.0);
        assert_eq!(settings.ecm_secs, ECM_SECS.1);
    }
}
