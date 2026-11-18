//! The Assets module's own settings, saved as `assets.toml` in the data
//! folder: sort order, character picker, route start, price source and the
//! user's saved filters.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const FILE_NAME: &str = "assets.toml";

/// How the locations are sorted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SortBy {
    /// Fewest jumps from the route start first; unreachable places last.
    #[default]
    Distance,
    /// System, then station.
    Name,
    /// Most items first.
    Count,
    /// Most ISK first.
    Value,
}

impl SortBy {
    pub const ALL: [SortBy; 4] = [SortBy::Distance, SortBy::Name, SortBy::Count, SortBy::Value];

    pub fn label(self) -> &'static str {
        match self {
            SortBy::Distance => "Distance",
            SortBy::Name => "Name",
            SortBy::Count => "Asset count",
            SortBy::Value => "Total price",
        }
    }
}

/// Whose assets are shown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Who {
    #[default]
    All,
    Tag(String),
    Character(u64),
}

/// Where jumps are counted from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteFrom {
    /// Where this character is now.
    Character(u64),
    /// A fixed solar system.
    System(i64),
}

/// Which price values the items.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PriceSource {
    /// EVE's average price (ESI `/markets/prices`): one request for every
    /// item, like the in-game "Estimated price".
    #[default]
    Average,
    /// The lowest Jita 4-4 sell order, one request per item type.
    JitaSell,
    /// The highest Jita 4-4 buy order, one request per item type.
    JitaBuy,
}

impl PriceSource {
    pub const ALL: [PriceSource; 3] = [
        PriceSource::Average,
        PriceSource::JitaSell,
        PriceSource::JitaBuy,
    ];

    pub fn label(self) -> &'static str {
        match self {
            PriceSource::Average => "EVE average price (fast, one request)",
            PriceSource::JitaSell => "Jita 4-4 lowest sell (one request per item type)",
            PriceSource::JitaBuy => "Jita 4-4 highest buy (one request per item type)",
        }
    }
}

/// A saved filter, shown as a chip. An item matches when it passes every
/// part that is set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Filter {
    pub name: String,
    /// SDE category ids. With `groups`: the item must be in one of them
    /// (none of either = any item).
    pub categories: Vec<i64>,
    /// SDE group ids.
    pub groups: Vec<i64>,
    /// Part of the item's type name or given name (any case). Empty = any.
    pub text: String,
    /// The item, with what is in it, is worth at least this much ISK.
    pub min_isk: f64,
    pub high_sec: bool,
    pub low_sec: bool,
    pub null_sec: bool,
    pub wormhole: bool,
}

impl Default for Filter {
    fn default() -> Self {
        Filter {
            name: String::new(),
            categories: Vec::new(),
            groups: Vec::new(),
            text: String::new(),
            min_isk: 0.0,
            high_sec: true,
            low_sec: true,
            null_sec: true,
            wormhole: true,
        }
    }
}

/// The SDE ids the starter filters use.
const CATEGORY_SHIP: i64 = 6;
/// Titan, Dreadnought, Freighter, Carrier, Supercarrier, Capital
/// Industrial Ship, Jump Freighter, Force Auxiliary, Lancer Dreadnought.
const CAPITAL_GROUPS: [i64; 9] = [30, 485, 513, 547, 659, 883, 902, 1538, 4594];

/// New fields need a default so older files still load.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AssetsSettings {
    pub sort: SortBy,
    /// Reverses `sort`.
    pub reversed: bool,
    pub who: Who,
    /// `None`: the first logged-in character.
    pub route_from: Option<RouteFrom>,
    pub price_source: PriceSource,
    pub filters: Vec<Filter>,
    /// Indexes into `filters` of the chips that are on.
    pub active: Vec<usize>,
    /// The chip row is open.
    pub show_filters: bool,
}

impl Default for AssetsSettings {
    fn default() -> Self {
        AssetsSettings {
            sort: SortBy::default(),
            reversed: false,
            who: Who::All,
            route_from: None,
            price_source: PriceSource::default(),
            // Two starters, like Kevin's examples. They can be edited or
            // deleted like any other.
            filters: vec![
                Filter {
                    name: "Ships".into(),
                    categories: vec![CATEGORY_SHIP],
                    ..Filter::default()
                },
                Filter {
                    name: "Caps".into(),
                    groups: CAPITAL_GROUPS.to_vec(),
                    ..Filter::default()
                },
            ],
            active: Vec::new(),
            show_filters: true,
        }
    }
}

impl AssetsSettings {
    pub fn file_path(data_dir: &Path) -> PathBuf {
        data_dir.join(FILE_NAME)
    }

    /// Reads the file, or the defaults if there is none. A broken file
    /// gives the defaults plus a message to show.
    pub fn load(data_dir: &Path) -> (AssetsSettings, Option<String>) {
        let path = AssetsSettings::file_path(data_dir);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return (AssetsSettings::default(), None);
        };
        match toml::from_str::<AssetsSettings>(&text) {
            Ok(mut settings) => {
                let count = settings.filters.len();
                settings.active.retain(|i| *i < count);
                (settings, None)
            }
            Err(e) => (
                AssetsSettings::default(),
                Some(format!(
                    "{} could not be read, so defaults are used: {e}",
                    path.display()
                )),
            ),
        }
    }

    /// The filters whose chips are on.
    pub fn active_filters(&self) -> Vec<&Filter> {
        self.active
            .iter()
            .filter_map(|i| self.filters.get(*i))
            .collect()
    }

    /// Removes a filter and fixes the indexes of the chips that are on.
    pub fn delete_filter(&mut self, index: usize) {
        if index >= self.filters.len() {
            return;
        }
        self.filters.remove(index);
        self.active.retain(|i| *i != index);
        for i in &mut self.active {
            if *i > index {
                *i -= 1;
            }
        }
    }

    /// Turns a chip on or off.
    pub fn toggle_filter(&mut self, index: usize) {
        if let Some(at) = self.active.iter().position(|i| *i == index) {
            self.active.remove(at);
        } else if index < self.filters.len() {
            self.active.push(index);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_toml() {
        let mut settings = AssetsSettings {
            sort: SortBy::Value,
            reversed: true,
            who: Who::Tag("Miners".into()),
            route_from: Some(RouteFrom::System(30000142)),
            price_source: PriceSource::JitaSell,
            ..AssetsSettings::default()
        };
        settings.filters.push(Filter {
            name: "Over 1b".into(),
            min_isk: 1e9,
            wormhole: false,
            ..Filter::default()
        });
        settings.active = vec![2];
        let text = toml::to_string(&settings).unwrap();
        assert_eq!(toml::from_str::<AssetsSettings>(&text).unwrap(), settings);
        assert_eq!(
            toml::from_str::<AssetsSettings>("").unwrap(),
            AssetsSettings::default()
        );
    }

    #[test]
    fn deleting_a_filter_fixes_active_chips() {
        let mut settings = AssetsSettings::default();
        settings.filters.push(Filter {
            name: "Third".into(),
            ..Filter::default()
        });
        settings.active = vec![0, 2];
        settings.delete_filter(1);
        assert_eq!(settings.active, vec![0, 1]);
        assert_eq!(settings.filters[1].name, "Third");
        settings.delete_filter(0);
        assert_eq!(settings.active, vec![0]);
        settings.toggle_filter(0);
        assert!(settings.active.is_empty());
        settings.toggle_filter(5);
        assert!(settings.active.is_empty());
    }
}
