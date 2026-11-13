//! The Planetary module's own settings, saved as `planetary.toml` in the
//! data folder: the thresholds from the Settings view and the filters.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const FILE_NAME: &str = "planetary.toml";

/// New fields need a default so older files still load.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct PlanetarySettings {
    pub thresholds: Thresholds,
    /// "Needs attention": hide the planets that are fine.
    pub only_attention: bool,
    /// Only show characters with this tag.
    pub tag: Option<String>,
    /// Hide characters that have no colonies.
    pub hide_empty: bool,
}

/// When a planet turns yellow or red.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Thresholds {
    /// Extractors count as "soon" when they end within this many hours.
    pub extractor_soon_hours: u32,
    /// Factory inputs count as "soon" when they last less than this.
    pub factory_soon_hours: u32,
    /// A launchpad or storage turns yellow at this fill, in percent.
    pub warn_percent: u32,
    /// And red ("full") at this one.
    pub full_percent: u32,
    /// "Viewed x ago" turns yellow after this many days.
    pub old_days: u32,
    /// Show the estimated launchpad fill since the last in-game view.
    pub show_estimate: bool,
}

impl Default for Thresholds {
    fn default() -> Self {
        Thresholds {
            extractor_soon_hours: 24,
            factory_soon_hours: 12,
            warn_percent: 75,
            full_percent: 95,
            old_days: 3,
            show_estimate: true,
        }
    }
}

impl PlanetarySettings {
    pub fn file_path(data_dir: &Path) -> PathBuf {
        data_dir.join(FILE_NAME)
    }

    /// Reads the file, or the defaults if there is none. A broken file
    /// gives the defaults plus a message to show.
    pub fn load(data_dir: &Path) -> (PlanetarySettings, Option<String>) {
        let path = PlanetarySettings::file_path(data_dir);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return (PlanetarySettings::default(), None);
        };
        match toml::from_str(&text) {
            Ok(settings) => (settings, None),
            Err(e) => (
                PlanetarySettings::default(),
                Some(format!(
                    "{} could not be read, so defaults are used: {e}",
                    path.display()
                )),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_toml() {
        let settings = PlanetarySettings {
            thresholds: Thresholds {
                extractor_soon_hours: 12,
                show_estimate: false,
                ..Thresholds::default()
            },
            only_attention: true,
            tag: Some("Industry".into()),
            hide_empty: true,
        };
        let text = toml::to_string(&settings).unwrap();
        assert_eq!(
            toml::from_str::<PlanetarySettings>(&text).unwrap(),
            settings
        );
        assert_eq!(
            toml::from_str::<PlanetarySettings>("").unwrap(),
            PlanetarySettings::default()
        );
        // A file with only some thresholds keeps the other defaults.
        let partial: PlanetarySettings = toml::from_str("[thresholds]\nwarn_percent = 60").unwrap();
        assert_eq!(partial.thresholds.warn_percent, 60);
        assert_eq!(partial.thresholds.full_percent, 95);
    }
}
