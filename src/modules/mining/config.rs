//! The Mining Supervisor's own settings (the Settings view inside the
//! panel), saved as `mining.toml` in the data folder.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::prices::{PriceSide, Yields};
use super::session::TrackerSettings;

const FILE_NAME: &str = "mining.toml";

/// Per-character settings. `None` means "use the default".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CharacterSettings {
    /// Show this character's card on the Live view.
    pub show: bool,
    pub hold_m3: Option<f64>,
    pub alert_percent: Option<f64>,
}

impl Default for CharacterSettings {
    fn default() -> Self {
        CharacterSettings {
            show: true,
            hold_m3: None,
            alert_percent: None,
        }
    }
}

/// All Mining Supervisor settings. New fields need a default so older
/// files still load.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MiningSettings {
    /// Ore hold size for every character without its own value.
    pub hold_m3: f64,
    /// Warn when the hold is this full, in percent.
    pub alert_percent: f64,
    /// "Idle" alert after this many minutes without a cycle.
    pub idle_minutes: f64,
    /// Beep on alerts.
    pub sound: bool,
    /// Reprocessing yield in percent for ore, ice and moon ore.
    pub ore_yield_percent: f64,
    /// Decompression yield in percent for compressed gas.
    pub gas_yield_percent: f64,
    pub price_side: PriceSide,
    /// By character name.
    pub characters: BTreeMap<String, CharacterSettings>,
}

impl Default for MiningSettings {
    fn default() -> Self {
        MiningSettings {
            hold_m3: 11_500.0,
            alert_percent: 70.0,
            idle_minutes: 5.0,
            sound: true,
            ore_yield_percent: 90.63,
            gas_yield_percent: 95.0,
            price_side: PriceSide::Buy,
            characters: BTreeMap::new(),
        }
    }
}

impl MiningSettings {
    pub fn file_path(data_dir: &Path) -> PathBuf {
        data_dir.join(FILE_NAME)
    }

    /// Loads the settings; defaults if the file is missing, plus a warning
    /// if it is broken. Runs once at startup, before the window shows.
    pub fn load(data_dir: &Path) -> (MiningSettings, Option<String>) {
        let path = MiningSettings::file_path(data_dir);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return (MiningSettings::default(), None);
        };
        match toml::from_str(&text) {
            Ok(settings) => (settings, None),
            Err(e) => (
                MiningSettings::default(),
                Some(format!(
                    "{} could not be read, so defaults are used: {e}",
                    path.display()
                )),
            ),
        }
    }

    pub fn to_toml(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }

    pub fn character(&self, name: &str) -> CharacterSettings {
        self.characters.get(name).cloned().unwrap_or_default()
    }

    /// The settings a character's tracker runs with.
    pub fn tracker_settings(&self, name: &str) -> TrackerSettings {
        let own = self.character(name);
        TrackerSettings {
            hold_capacity_m3: Some(own.hold_m3.unwrap_or(self.hold_m3)),
            fill_alert_percent: own.alert_percent.unwrap_or(self.alert_percent),
            idle_after_secs: (self.idle_minutes * 60.0).round() as i64,
            ..TrackerSettings::default()
        }
    }

    pub fn yields(&self) -> Yields {
        Yields {
            ore: self.ore_yield_percent / 100.0,
            gas: self.gas_yield_percent / 100.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn character_overrides_fall_back_to_defaults() {
        let mut settings = MiningSettings::default();
        settings.characters.insert(
            "Pilot Delta".to_string(),
            CharacterSettings {
                show: true,
                hold_m3: Some(43_500.0),
                alert_percent: Some(85.0),
            },
        );
        let delta = settings.tracker_settings("Pilot Delta");
        assert_eq!(delta.hold_capacity_m3, Some(43_500.0));
        assert_eq!(delta.fill_alert_percent, 85.0);
        let other = settings.tracker_settings("Pilot Alpha");
        assert_eq!(other.hold_capacity_m3, Some(11_500.0));
        assert_eq!(other.fill_alert_percent, 70.0);
        assert_eq!(other.idle_after_secs, 300);
    }

    #[test]
    fn round_trip_and_defaults() {
        let mut settings = MiningSettings {
            price_side: PriceSide::Sell,
            ..MiningSettings::default()
        };
        settings
            .characters
            .insert("Pilot Echo".to_string(), CharacterSettings::default());
        let text = settings.to_toml().unwrap();
        let back: MiningSettings = toml::from_str(&text).unwrap();
        assert_eq!(back, settings);
        let empty: MiningSettings = toml::from_str("").unwrap();
        assert_eq!(empty, MiningSettings::default());
        assert!((empty.yields().ore - 0.9063).abs() < 1e-12);
    }
}
