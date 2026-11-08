//! The Intel Supervisor's own settings (its Settings view), saved as
//! `intel.toml` in the data folder.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::sound::SoundKind;

const FILE_NAME: &str = "intel.toml";

/// All Intel Supervisor settings. New fields need a default so older files
/// still load.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct IntelSettings {
    /// The chat channels to read intel from, by their name in the log
    /// header ("Branch Intel").
    pub channels: Vec<String>,
    /// Words that mark a system as clear, lowercase.
    pub clear_words: Vec<String>,
    /// The region the map showed last.
    pub region_id: Option<i64>,
    /// Switch the map to the active pilot's region when they jump into
    /// another one.
    pub follow_regions: bool,
    /// Own pilots hidden from the map and from jump distances (clicked
    /// off on their chip).
    pub hidden_pilots: Vec<String>,
    /// Width of the intel list next to the map, in points.
    pub list_width: f32,
    /// Alert when a report is at most this many jumps from an own pilot.
    pub alert_jumps: u32,
    /// Loud alert (full volume, played twice) at most this many jumps away.
    pub loud_jumps: u32,
    pub sound: SoundKind,
    /// 0.0 to 1.0.
    pub volume: f32,
    /// Flash the taskbar button and draw a red frame around the map.
    pub flash: bool,
    /// Reports fade from red to grey over this many minutes.
    pub fade_minutes: u32,
    /// The intel list shows only reports within the alert range.
    pub only_in_range: bool,
    /// Tag groups not shown on the map (unticked in Settings).
    pub hidden_tag_groups: Vec<String>,
    /// Tint systems by ESI's kills and jumps of the last hour.
    pub esi_tint: bool,
}

impl Default for IntelSettings {
    fn default() -> Self {
        IntelSettings {
            channels: Vec::new(),
            clear_words: vec!["clr".to_string(), "clear".to_string()],
            region_id: None,
            follow_regions: true,
            hidden_pilots: Vec::new(),
            list_width: 420.0,
            alert_jumps: 5,
            loud_jumps: 2,
            sound: SoundKind::Voice,
            volume: 0.7,
            flash: true,
            fade_minutes: 10,
            only_in_range: false,
            hidden_tag_groups: Vec::new(),
            esi_tint: false,
        }
    }
}

impl IntelSettings {
    pub fn file_path(data_dir: &Path) -> PathBuf {
        data_dir.join(FILE_NAME)
    }

    /// Loads the settings; defaults if the file is missing, plus a warning
    /// if it is broken. Runs once at startup, before the window shows.
    pub fn load(data_dir: &Path) -> (IntelSettings, Option<String>) {
        let path = IntelSettings::file_path(data_dir);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return (IntelSettings::default(), None);
        };
        match toml::from_str(&text) {
            Ok(settings) => (settings, None),
            Err(e) => (
                IntelSettings::default(),
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let settings = IntelSettings {
            channels: vec!["Branch Intel".into(), "Region Intel".into()],
            ..IntelSettings::default()
        };
        let text = settings.to_toml().unwrap();
        assert_eq!(toml::from_str::<IntelSettings>(&text).unwrap(), settings);
    }

    #[test]
    fn missing_fields_use_defaults() {
        let settings: IntelSettings = toml::from_str("").unwrap();
        assert_eq!(settings, IntelSettings::default());
        assert_eq!(settings.clear_words, ["clr", "clear"]);
        assert_eq!((settings.alert_jumps, settings.loud_jumps), (5, 2));
    }
}
