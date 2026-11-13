//! The Contracts module's own settings, saved as `contracts.toml` in the
//! data folder: alert switches, the "soon" warning, the filters and the
//! width of the changelog.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::changes::AlertKind;

const FILE_NAME: &str = "contracts.toml";

/// New fields need a default so older files still load.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ContractsSettings {
    pub alerts: Alerts,
    /// "Expires soon" and "due soon" come this many hours before.
    pub soon_hours: u32,
    /// Finished contracts older than this many days are hidden from the
    /// list (0: show all). The changelog keeps everything.
    pub finished_days: u32,
    /// Width of the changelog on the right, in points.
    pub log_width: f32,
    pub status_filter: StatusFilter,
    /// `None`: all types.
    pub type_filter: Option<TypeFilter>,
    /// Only show contracts of characters with this tag.
    pub tag: Option<String>,
    /// The changelog shows only the unseen "while closed" entries.
    pub log_only_closed: bool,
    /// The column the list is sorted by, clicked in its header. `None`:
    /// the default order (what needs you soonest first).
    pub sort: Option<Sort>,
}

/// A list column to sort by, and which way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sort {
    pub column: SortColumn,
    pub descending: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortColumn {
    Status,
    Type,
    Contract,
    Character,
    Price,
    Where,
    Issued,
    /// "End" in the list: the deadline, or when it ended. Saved as "Time"
    /// by the first version, so that name still reads.
    #[serde(alias = "Time")]
    End,
}

impl Sort {
    /// A click on `column`'s header: ascending first, then descending,
    /// then back to the default order.
    pub fn click(current: Option<Sort>, column: SortColumn) -> Option<Sort> {
        match current {
            Some(sort) if sort.column == column && !sort.descending => Some(Sort {
                column,
                descending: true,
            }),
            Some(sort) if sort.column == column => None,
            _ => Some(Sort {
                column,
                descending: false,
            }),
        }
    }
}

impl Default for ContractsSettings {
    fn default() -> Self {
        ContractsSettings {
            alerts: Alerts::default(),
            soon_hours: 24,
            finished_days: 30,
            log_width: 430.0,
            status_filter: StatusFilter::Active,
            type_filter: None,
            tag: None,
            log_only_closed: false,
            sort: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StatusFilter {
    Active,
    Finished,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TypeFilter {
    ItemExchange,
    Courier,
    Auction,
}

/// One Alert and one Sound switch per kind of change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Switch {
    /// Counts on the Contracts button and flashes the taskbar.
    pub alert: bool,
    pub sound: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Alerts {
    pub assigned: Switch,
    pub accepted: Switch,
    pub finished: Switch,
    pub bid: Switch,
    pub ended: Switch,
    pub soon: Switch,
}

impl Default for Alerts {
    /// As in the mockup: every alert on, sound only for "assigned to me"
    /// and "due soon".
    fn default() -> Self {
        let on = |sound| Switch { alert: true, sound };
        Alerts {
            assigned: on(true),
            accepted: on(false),
            finished: on(false),
            bid: on(false),
            ended: on(false),
            soon: on(true),
        }
    }
}

impl Alerts {
    pub fn get(&self, kind: AlertKind) -> Switch {
        match kind {
            AlertKind::Assigned => self.assigned,
            AlertKind::Accepted => self.accepted,
            AlertKind::Finished => self.finished,
            AlertKind::Bid => self.bid,
            AlertKind::Ended => self.ended,
            AlertKind::Soon => self.soon,
        }
    }

    pub fn get_mut(&mut self, kind: AlertKind) -> &mut Switch {
        match kind {
            AlertKind::Assigned => &mut self.assigned,
            AlertKind::Accepted => &mut self.accepted,
            AlertKind::Finished => &mut self.finished,
            AlertKind::Bid => &mut self.bid,
            AlertKind::Ended => &mut self.ended,
            AlertKind::Soon => &mut self.soon,
        }
    }
}

impl ContractsSettings {
    pub fn file_path(data_dir: &Path) -> PathBuf {
        data_dir.join(FILE_NAME)
    }

    /// Reads the file, or the defaults if there is none. A broken file
    /// gives the defaults plus a message to show.
    pub fn load(data_dir: &Path) -> (ContractsSettings, Option<String>) {
        let path = ContractsSettings::file_path(data_dir);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return (ContractsSettings::default(), None);
        };
        match toml::from_str(&text) {
            Ok(settings) => (settings, None),
            Err(e) => (
                ContractsSettings::default(),
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
    fn header_clicks_cycle() {
        let asc = Sort::click(None, SortColumn::Price);
        assert_eq!(
            asc,
            Some(Sort {
                column: SortColumn::Price,
                descending: false
            })
        );
        let desc = Sort::click(asc, SortColumn::Price);
        assert!(desc.unwrap().descending);
        assert_eq!(Sort::click(desc, SortColumn::Price), None);
        // Another column starts ascending again.
        // The first version saved the End column as "Time".
        let old: Sort = toml::from_str("column = \"Time\"\ndescending = true").unwrap();
        assert_eq!(old.column, SortColumn::End);
        let other = Sort::click(desc, SortColumn::End).unwrap();
        assert_eq!(other.column, SortColumn::End);
        assert!(!other.descending);
    }

    #[test]
    fn round_trips_through_toml() {
        let mut settings = ContractsSettings {
            soon_hours: 12,
            finished_days: 0,
            type_filter: Some(TypeFilter::Courier),
            tag: Some("Hauler".into()),
            sort: Some(Sort {
                column: SortColumn::Where,
                descending: true,
            }),
            ..ContractsSettings::default()
        };
        settings.alerts.get_mut(AlertKind::Bid).sound = true;
        let text = toml::to_string(&settings).unwrap();
        assert_eq!(
            toml::from_str::<ContractsSettings>(&text).unwrap(),
            settings
        );
        assert_eq!(
            toml::from_str::<ContractsSettings>("").unwrap(),
            ContractsSettings::default()
        );
    }
}
