//! User settings, stored as `settings.toml` in the data folder.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};

const FILE_NAME: &str = "settings.toml";

/// All user settings. Add new fields with a default so older files still load.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
// `default` fills in any field that is missing from the file.
#[serde(default)]
pub struct Settings {
    /// The EVE `logs` folder, which holds `Chatlogs` and `Gamelogs`.
    pub log_folder: PathBuf,
    /// How often the log watchers check their files for new lines.
    pub file_poll_rate: FilePollRate,
    /// Look for a newer Athena on GitHub when Athena starts.
    pub update_check_on_start: bool,
    /// Offer pre-releases (every main build, e.g. 0.1.1-alpha.7), not only
    /// versions tagged by hand.
    pub update_include_prereleases: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            log_folder: default_log_folder(),
            file_poll_rate: FilePollRate::default(),
            update_check_on_start: true,
            update_include_prereleases: true,
        }
    }
}

/// The choices for how often log files are checked for new lines. Saved in
/// `settings.toml` as the text in `rename`, e.g. `file_poll_rate = "250 ms"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FilePollRate {
    #[serde(rename = "100 ms")]
    Ms100,
    #[default]
    #[serde(rename = "250 ms")]
    Ms250,
    #[serde(rename = "500 ms")]
    Ms500,
    #[serde(rename = "1 s")]
    S1,
    #[serde(rename = "2 s")]
    S2,
    #[serde(rename = "3 s")]
    S3,
}

impl FilePollRate {
    /// Every choice, in the order Settings shows them as radio buttons.
    pub const ALL: [FilePollRate; 6] = [
        FilePollRate::Ms100,
        FilePollRate::Ms250,
        FilePollRate::Ms500,
        FilePollRate::S1,
        FilePollRate::S2,
        FilePollRate::S3,
    ];

    pub fn millis(self) -> u64 {
        match self {
            FilePollRate::Ms100 => 100,
            FilePollRate::Ms250 => 250,
            FilePollRate::Ms500 => 500,
            FilePollRate::S1 => 1000,
            FilePollRate::S2 => 2000,
            FilePollRate::S3 => 3000,
        }
    }

    /// The text shown in Settings.
    pub fn label(self) -> &'static str {
        match self {
            FilePollRate::Ms100 => "100 ms",
            FilePollRate::Ms250 => "250 ms",
            FilePollRate::Ms500 => "500 ms",
            FilePollRate::S1 => "1 s",
            FilePollRate::S2 => "2 s",
            FilePollRate::S3 => "3 s",
        }
    }
}

/// The poll interval in milliseconds that the watcher threads use right now.
///
/// It is a global so every watcher thread can read the current value each
/// time it goes to sleep, which makes a change in Settings apply at once
/// without restarting anything. An `AtomicU64` is a number that several
/// threads may read and write at the same time without a `Mutex`.
static FILE_POLL_MILLIS: AtomicU64 = AtomicU64::new(250);

/// Makes all log watchers use `rate` from their next check on. The app
/// calls this at startup and whenever the setting changes.
pub fn set_file_poll_rate(rate: FilePollRate) {
    // `Relaxed` is enough: it's a single number and nothing else depends
    // on seeing it in a particular order.
    FILE_POLL_MILLIS.store(rate.millis(), Ordering::Relaxed);
}

/// How long a log watcher sleeps between two checks of its files.
pub fn file_poll_interval() -> Duration {
    Duration::from_millis(FILE_POLL_MILLIS.load(Ordering::Relaxed))
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
            file_poll_rate: FilePollRate::S2,
            update_check_on_start: false,
            update_include_prereleases: false,
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
    fn poll_rate_defaults_to_250_ms() {
        assert_eq!(Settings::default().file_poll_rate.millis(), 250);
    }

    #[test]
    fn poll_rate_is_saved_as_readable_text() {
        let settings = Settings::from_toml("file_poll_rate = \"1 s\"").unwrap();
        assert_eq!(settings.file_poll_rate, FilePollRate::S1);
        assert!(
            settings
                .to_toml()
                .unwrap()
                .contains("file_poll_rate = \"1 s\"")
        );
    }

    #[test]
    fn poll_rate_labels_match_the_saved_text() {
        for rate in FilePollRate::ALL {
            let text = toml::to_string(&Settings {
                file_poll_rate: rate,
                ..Settings::default()
            })
            .unwrap();
            assert!(text.contains(&format!("\"{}\"", rate.label())));
        }
    }

    #[test]
    fn broken_file_is_an_error() {
        assert!(Settings::from_toml("log_folder = [").is_err());
    }
}
