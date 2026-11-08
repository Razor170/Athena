//! The Intel Supervisor's own settings (its Settings view), saved as
//! `intel.toml` in the data folder. Later pull requests add the alert,
//! sound and map settings here.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

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
}

impl Default for IntelSettings {
    fn default() -> Self {
        IntelSettings {
            channels: Vec::new(),
            clear_words: vec!["clr".to_string(), "clear".to_string()],
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
    }
}
