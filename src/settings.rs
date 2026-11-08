//! User settings, stored as `settings.toml` in the data folder.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const FILE_NAME: &str = "settings.toml";

/// All user settings. Add new fields with a default so older files still load.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
// `default` fills in any field that is missing from the file.
#[serde(default)]
pub struct Settings {
    /// The EVE `logs` folder, which holds `Chatlogs` and `Gamelogs`.
    pub log_folder: PathBuf,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            log_folder: default_log_folder(),
        }
    }
}

/// `Documents\EVE\logs`. Uses the real Documents folder, even when Windows
/// has moved it (for example into OneDrive).
fn default_log_folder() -> PathBuf {
    match directories::UserDirs::new().and_then(|dirs| dirs.document_dir().map(Path::to_path_buf)) {
        Some(documents) => documents.join("EVE").join("logs"),
        None => PathBuf::new(),
    }
}

impl Settings {
    pub fn file_path(data_dir: &Path) -> PathBuf {
        data_dir.join(FILE_NAME)
    }

    /// Loads the settings. A missing file gives the defaults. A broken file
    /// also gives the defaults, plus a warning to show the user.
    pub fn load(data_dir: &Path) -> (Settings, Option<String>) {
        let path = Settings::file_path(data_dir);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(_) => return (Settings::default(), None),
        };
        match Settings::from_toml(&text) {
            Ok(settings) => (settings, None),
            Err(e) => (
                Settings::default(),
                Some(format!(
                    "{} could not be read, so defaults are used: {e}",
                    path.display()
                )),
            ),
        }
    }

    pub fn from_toml(text: &str) -> Result<Settings, toml::de::Error> {
        toml::from_str(text)
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
        let settings = Settings {
            log_folder: PathBuf::from("D:/Games/EVE/logs"),
        };
        let text = settings.to_toml().unwrap();
        assert_eq!(Settings::from_toml(&text).unwrap(), settings);
    }

    #[test]
    fn missing_fields_use_defaults() {
        let settings = Settings::from_toml("").unwrap();
        assert_eq!(settings, Settings::default());
    }

    #[test]
    fn broken_file_is_an_error() {
        assert!(Settings::from_toml("log_folder = [").is_err());
    }
}
