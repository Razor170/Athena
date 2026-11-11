//! The Mining Supervisor's own settings (the Settings view inside the
//! panel), saved as `mining.toml` in the data folder.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::prices::{PriceSide, Yields};
use super::session::{Alert, TrackerSettings};

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
    /// After an alert sound, other alerts stay silent for this many seconds.
    /// They still show in the alert log.
    pub quiet_seconds: f64,
    /// Which alerts are raised at all. A switched-off alert makes no sound
    /// and adds no line to the alert log; the cards still show the state.
    pub alert_crystal: bool,
    pub alert_stopped: bool,
    pub alert_threshold: bool,
    pub alert_full: bool,
    pub alert_idle: bool,
    /// Which alerts play a sound (only when the alert itself is on).
    pub sound_crystal: bool,
    pub sound_stopped: bool,
    pub sound_threshold: bool,
    pub sound_full: bool,
    pub sound_idle: bool,
    /// Reprocessing yield in percent for ore, ice and moon ore.
    pub ore_yield_percent: f64,
    /// Decompression yield in percent for compressed gas.
    pub gas_yield_percent: f64,
    pub price_side: PriceSide,
    /// The order of the characters on the Live view and in Settings, set
    /// with the arrows in Settings. Characters not listed come after these,
    /// in alphabetical order.
    pub order: Vec<String>,
    /// By character name.
    pub characters: BTreeMap<String, CharacterSettings>,
    /// How many columns of character cards the Live view shows, 1 to 4.
    /// Each column is 480 px wide, so more columns need a wider tab.
    pub columns: u8,
    /// The hold bars overlay. Last, because it is a TOML table.
    pub overlay: OverlaySettings,
}

/// Settings of the hold bars overlay (`[overlay]` in `mining.toml`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OverlaySettings {
    /// Show the overlay at all.
    pub enabled: bool,
    /// Height of each bar in pixels.
    pub bar_height_px: f64,
    /// The overlay's width in pixels, set by pulling its right edge.
    pub width_px: i32,
    /// Show each character's name under their bar.
    pub show_names: bool,
    /// Show how full the hold is, in percent, under each bar on the right.
    pub show_percent: bool,
    /// Opacity of the background, 0 to 100 percent.
    pub background_opacity: f64,
    /// Opacity of the bars and their text, 10 to 100 percent.
    pub bar_opacity: f64,
    /// The top-left corner on the screen, in pixels. `None` until moved.
    pub position: Option<[i32; 2]>,
}

impl Default for OverlaySettings {
    fn default() -> Self {
        OverlaySettings {
            // Off until switched on, so Athena opens no window over EVE for
            // people who don't want one.
            enabled: false,
            bar_height_px: 6.0,
            width_px: 170,
            show_names: true,
            show_percent: true,
            background_opacity: 55.0,
            bar_opacity: 100.0,
            position: None,
        }
    }
}

impl Default for MiningSettings {
    fn default() -> Self {
        MiningSettings {
            hold_m3: 11_500.0,
            alert_percent: 70.0,
            idle_minutes: 5.0,
            sound: true,
            quiet_seconds: 10.0,
            alert_crystal: true,
            alert_stopped: true,
            alert_threshold: true,
            alert_full: true,
            alert_idle: true,
            sound_crystal: true,
            sound_stopped: true,
            sound_threshold: true,
            sound_full: true,
            sound_idle: true,
            ore_yield_percent: 90.63,
            gas_yield_percent: 95.0,
            price_side: PriceSide::Buy,
            order: Vec::new(),
            characters: BTreeMap::new(),
            columns: 1,
            overlay: OverlaySettings::default(),
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

    /// True if this kind of alert is switched on in Settings.
    pub fn alert_enabled(&self, alert: &Alert) -> bool {
        match alert {
            Alert::CrystalDestroyed { .. } => self.alert_crystal,
            Alert::MinerStopped { .. } => self.alert_stopped,
            Alert::HoldAboveThreshold { .. } => self.alert_threshold,
            Alert::HoldFull => self.alert_full,
            Alert::Idle { .. } => self.alert_idle,
        }
    }

    /// True if this kind of alert should play a sound: the alert is on, its
    /// own sound is on, and the "Play alert sounds" switch is on.
    pub fn alert_sound(&self, alert: &Alert) -> bool {
        let own = match alert {
            Alert::CrystalDestroyed { .. } => self.sound_crystal,
            Alert::MinerStopped { .. } => self.sound_stopped,
            Alert::HoldAboveThreshold { .. } => self.sound_threshold,
            Alert::HoldFull => self.sound_full,
            Alert::Idle { .. } => self.sound_idle,
        };
        self.sound && own && self.alert_enabled(alert)
    }

    /// How long alert sounds stay quiet after one has played. Kept between
    /// 0 s and 10 minutes even if the file was edited by hand.
    pub fn quiet_time(&self) -> Duration {
        let seconds = if self.quiet_seconds.is_finite() {
            self.quiet_seconds.clamp(0.0, 600.0)
        } else {
            0.0
        };
        Duration::from_secs_f64(seconds)
    }

    pub fn to_toml(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }

    /// The column count, kept between 1 and 4 even if the file was edited
    /// by hand.
    pub fn columns(&self) -> usize {
        usize::from(self.columns.clamp(1, 4))
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

    /// `names` sorted in the user's order: the ones in `order` first, then
    /// the rest alphabetically.
    pub fn ordered<'a>(&self, names: impl Iterator<Item = &'a String>) -> Vec<String> {
        let mut names: Vec<String> = names.cloned().collect();
        // Not in `order` gives `usize::MAX`, so those sort last; the name
        // breaks ties, which sorts them alphabetically.
        let rank = |name: &String| {
            self.order
                .iter()
                .position(|o| o == name)
                .unwrap_or(usize::MAX)
        };
        names.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| a.cmp(b)));
        names
    }

    /// Moves `name` (dragged on the Live view) to just before `target`, or
    /// just after it when `after` is true. `names` are the characters
    /// currently known.
    pub fn place_character(&mut self, names: &[String], name: &str, target: &str, after: bool) {
        if name == target {
            return;
        }
        let mut list = self.ordered(names.iter());
        let Some(from) = list.iter().position(|n| n == name) else {
            return;
        };
        let moved = list.remove(from);
        let Some(to) = list.iter().position(|n| n == target) else {
            return;
        };
        list.insert(if after { to + 1 } else { to }, moved);
        // Keep characters that aren't in the logs right now, after the known
        // ones, so they come back in a sensible place.
        for old in &self.order {
            if !list.contains(old) {
                list.push(old.clone());
            }
        }
        self.order = list;
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
    fn alert_switches_pick_the_right_kind() {
        let mut settings = MiningSettings::default();
        let crystal = Alert::CrystalDestroyed {
            module: "Modulated Strip Miner II".to_string(),
        };
        let stopped = Alert::MinerStopped {
            reason: "deactivates".to_string(),
        };
        let all = [
            crystal.clone(),
            stopped.clone(),
            Alert::HoldAboveThreshold { percent: 71.0 },
            Alert::HoldFull,
            Alert::Idle { minutes: 5 },
        ];
        assert!(all.iter().all(|a| settings.alert_enabled(a)));

        settings.alert_crystal = false;
        settings.alert_full = false;
        assert!(!settings.alert_enabled(&crystal));
        assert!(!settings.alert_enabled(&Alert::HoldFull));
        assert!(settings.alert_enabled(&stopped));
        assert!(settings.alert_enabled(&Alert::Idle { minutes: 5 }));
    }

    #[test]
    fn sound_needs_alert_own_switch_and_main_switch() {
        let mut settings = MiningSettings::default();
        assert!(settings.alert_sound(&Alert::HoldFull));

        settings.sound_full = false;
        assert!(!settings.alert_sound(&Alert::HoldFull));
        assert!(settings.alert_enabled(&Alert::HoldFull));
        assert!(settings.alert_sound(&Alert::Idle { minutes: 5 }));

        // A switched-off alert never beeps, even with its sound on.
        settings.alert_idle = false;
        assert!(!settings.alert_sound(&Alert::Idle { minutes: 5 }));

        settings.sound = false;
        assert!(!settings.alert_sound(&Alert::HoldAboveThreshold { percent: 75.0 }));
    }

    #[test]
    fn old_settings_files_get_the_new_alert_defaults() {
        let settings: MiningSettings = toml::from_str("hold_m3 = 20000.0").unwrap();
        assert!(settings.alert_crystal && settings.alert_stopped && settings.alert_idle);
        assert_eq!(settings.quiet_time(), Duration::from_secs(10));
    }

    #[test]
    fn quiet_time_is_clamped() {
        let mut settings = MiningSettings {
            quiet_seconds: -5.0,
            ..MiningSettings::default()
        };
        assert_eq!(settings.quiet_time(), Duration::ZERO);
        settings.quiet_seconds = f64::NAN;
        assert_eq!(settings.quiet_time(), Duration::ZERO);
        settings.quiet_seconds = 1e9;
        assert_eq!(settings.quiet_time(), Duration::from_secs(600));
    }

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
    fn character_order() {
        let names = |list: &[&str]| -> Vec<String> { list.iter().map(|n| n.to_string()).collect() };
        let known = names(&["Pilot Alpha", "Pilot Bravo", "Pilot Charlie"]);
        let mut settings = MiningSettings::default();
        // No order yet: alphabetical.
        assert_eq!(settings.ordered(known.iter()), known);
        // Charlie dropped on the top half of Alpha: goes first.
        settings.place_character(&known, "Pilot Charlie", "Pilot Alpha", false);
        assert_eq!(
            settings.ordered(known.iter()),
            names(&["Pilot Charlie", "Pilot Alpha", "Pilot Bravo"])
        );
        // Charlie dropped on the bottom half of Bravo: goes last.
        settings.place_character(&known, "Pilot Charlie", "Pilot Bravo", true);
        assert_eq!(settings.ordered(known.iter()), known);
        // Dropped on itself: nothing changes.
        settings.place_character(&known, "Pilot Alpha", "Pilot Alpha", true);
        assert_eq!(settings.ordered(known.iter()), known);
        // A new character goes last. One that's gone stays in the saved
        // order, after the known ones.
        let later = names(&["Pilot Alpha", "Pilot Bravo", "Pilot Delta"]);
        settings.place_character(&later, "Pilot Delta", "Pilot Alpha", false);
        assert_eq!(
            settings.order,
            names(&["Pilot Delta", "Pilot Alpha", "Pilot Bravo", "Pilot Charlie"])
        );
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
        settings.order = vec!["Pilot Echo".to_string()];
        settings.overlay.enabled = true;
        settings.overlay.position = Some([-1500, 40]);
        let text = settings.to_toml().unwrap();
        let back: MiningSettings = toml::from_str(&text).unwrap();
        assert_eq!(back, settings);
        let empty: MiningSettings = toml::from_str("").unwrap();
        assert_eq!(empty, MiningSettings::default());
        assert!((empty.yields().ore - 0.9063).abs() < 1e-12);
        // Files from before the column setting get one column.
        assert_eq!(empty.columns(), 1);
    }

    #[test]
    fn columns_stay_between_one_and_four() {
        let columns = |n: u8| {
            MiningSettings {
                columns: n,
                ..MiningSettings::default()
            }
            .columns()
        };
        assert_eq!(columns(0), 1);
        assert_eq!(columns(3), 3);
        assert_eq!(columns(9), 4);
    }

    #[test]
    fn old_card_bar_settings_still_load() {
        // Files from before the own overlay have these two keys; they are
        // ignored and the overlay starts off.
        let old = "card_bars = true\ncard_bar_height_px = 8.0\n";
        let settings: MiningSettings = toml::from_str(old).unwrap();
        assert_eq!(settings.overlay, OverlaySettings::default());
    }
}
