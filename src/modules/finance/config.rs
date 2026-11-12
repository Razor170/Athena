//! The Finance module's own settings, saved as `finance.toml` in the data
//! folder: which wallets are picked, and the filters of each tab.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const FILE_NAME: &str = "finance.toml";

/// New fields need a default so older files still load.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct FinanceSettings {
    /// Characters whose wallet is NOT picked. Stored this way round so a
    /// newly logged-in character is picked from the start.
    pub unpicked: BTreeSet<u64>,
    /// What the Overview chart's candles follow.
    pub chart: ChartMode,
}

/// The Overview chart's Balance | Net switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ChartMode {
    /// The combined wallet balance of the picked characters.
    #[default]
    Balance,
    /// Income minus expenses, from 0 at the start of the time range.
    Net,
}

impl FinanceSettings {
    pub fn file_path(data_dir: &Path) -> PathBuf {
        data_dir.join(FILE_NAME)
    }

    /// Reads the file, or the defaults if there is none. A broken file
    /// gives the defaults plus a message to show.
    pub fn load(data_dir: &Path) -> (FinanceSettings, Option<String>) {
        let path = FinanceSettings::file_path(data_dir);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return (FinanceSettings::default(), None);
        };
        match toml::from_str(&text) {
            Ok(settings) => (settings, None),
            Err(e) => (
                FinanceSettings::default(),
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
        let settings = FinanceSettings {
            unpicked: [5, 7].into_iter().collect(),
            chart: ChartMode::Net,
        };
        let text = toml::to_string(&settings).unwrap();
        assert_eq!(toml::from_str::<FinanceSettings>(&text).unwrap(), settings);
        assert_eq!(
            toml::from_str::<FinanceSettings>("").unwrap(),
            FinanceSettings::default()
        );
        // Files from before the chart existed still load.
        let old: FinanceSettings = toml::from_str("unpicked = [5]").unwrap();
        assert_eq!(old.chart, ChartMode::Balance);
    }
}
