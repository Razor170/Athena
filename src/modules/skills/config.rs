//! The Skills module's own settings, saved as `skills.toml` in the data
//! folder: the Settings view plus the filters and sort order.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const FILE_NAME: &str = "skills.toml";

/// The shortest ESI check interval the Settings view allows.
pub const MIN_POLL_MINUTES: u32 = 5;

/// How the Overview cards are sorted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SortBy {
    /// Not training first, then the queue that ends soonest.
    #[default]
    QueueEnd,
    Name,
    /// Most skill points first.
    SkillPoints,
}

/// New fields need a default so older files still load.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SkillsSettings {
    /// A queue that ends within this many hours turns yellow.
    pub soon_hours: u32,
    /// The thin queue bar on Overview cards is full at this many days.
    pub bar_days: u32,
    /// Show the number of characters not training on the Skills button.
    pub badge: bool,
    /// How often ESI is asked, in minutes.
    pub poll_minutes: u32,
    /// Hide characters without skills data (login expired).
    pub hide_without_data: bool,
    pub sort: SortBy,
    /// Only show characters with this tag.
    pub tag: Option<String>,
    /// Characters hidden on the Overview, by id. They get no red or
    /// yellow card and don't count in the badge.
    pub hidden: BTreeSet<u64>,
}

impl Default for SkillsSettings {
    fn default() -> Self {
        SkillsSettings {
            soon_hours: 24,
            bar_days: 7,
            badge: true,
            poll_minutes: 15,
            hide_without_data: false,
            sort: SortBy::default(),
            tag: None,
            hidden: BTreeSet::new(),
        }
    }
}

impl SkillsSettings {
    pub fn file_path(data_dir: &Path) -> PathBuf {
        data_dir.join(FILE_NAME)
    }

    /// Reads the file, or the defaults if there is none. A broken file
    /// gives the defaults plus a message to show.
    pub fn load(data_dir: &Path) -> (SkillsSettings, Option<String>) {
        let path = SkillsSettings::file_path(data_dir);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return (SkillsSettings::default(), None);
        };
        match toml::from_str(&text) {
            Ok(settings) => (settings, None),
            Err(e) => (
                SkillsSettings::default(),
                Some(format!(
                    "{} could not be read, so defaults are used: {e}",
                    path.display()
                )),
            ),
        }
    }

    /// The ESI check interval in seconds, never below the minimum.
    pub fn poll_secs(&self) -> i64 {
        i64::from(self.poll_minutes.max(MIN_POLL_MINUTES)) * 60
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_toml() {
        let settings = SkillsSettings {
            soon_hours: 12,
            badge: false,
            sort: SortBy::SkillPoints,
            tag: Some("Miner".into()),
            hidden: BTreeSet::from([90000001, 90000002]),
            ..SkillsSettings::default()
        };
        let text = toml::to_string(&settings).unwrap();
        assert_eq!(toml::from_str::<SkillsSettings>(&text).unwrap(), settings);
        assert_eq!(
            toml::from_str::<SkillsSettings>("").unwrap(),
            SkillsSettings::default()
        );
        let partial: SkillsSettings = toml::from_str("sort = \"name\"").unwrap();
        assert_eq!(partial.sort, SortBy::Name);
        assert_eq!(partial.soon_hours, 24);
    }

    #[test]
    fn poll_never_below_minimum() {
        let settings = SkillsSettings {
            poll_minutes: 1,
            ..SkillsSettings::default()
        };
        assert_eq!(settings.poll_secs(), 300);
    }
}
