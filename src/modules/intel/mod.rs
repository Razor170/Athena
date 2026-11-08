//! Intel Supervisor: a RIFT-like region map with an intel list, built from
//! the intel channels in the chat logs and the own pilots' Local channels.
//! See PLAN.md and the approved mockup
//! (<https://claude.ai/artifact/AAU2CByJPT4bJj9xbcFqeY>).
//!
//! The pieces:
//!
//! - [`chatlog`]: reads UTF-16 chat log files into lines.
//! - [`watcher`]: follows the chat logs on a background thread.
//! - [`dedupe`]: drops the copies of a report that every pilot's log has.
//! - [`parser`]: finds systems, ships, "clear"/"nv" and pilot names in a line.
//! - [`config`]: the module's settings (`intel.toml`).
//! - [`map`]: draws a region map with pan and zoom.
//! - [`ui`]: draws the panel.
//!
//! This file holds the [`IntelSupervisor`] module: it owns the watcher,
//! drains its channel in `update`, and keeps the reports and where each own
//! pilot is.
//!
//! So far (PR 3 of 6) the panel has the map with the own pilots, the
//! region picker, Follow and the pilot chips, plus a plain list of reports
//! next to it; the styled intel list and alerts come next.

pub mod chatlog;
pub mod config;
pub mod dedupe;
pub mod map;
pub mod parser;
mod ui;
pub mod watcher;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use eframe::egui;

use self::chatlog::{ChatLine, EVE_SYSTEM, local_system_change};
use self::config::IntelSettings;
use self::dedupe::Deduper;
use self::map::MapView;
use self::parser::{IntelMessage, ParseContext};
use self::watcher::{ChatLogWatcher, LOCAL, WatchEvent, last_downtime};
use super::{Module, ModuleContext, SharedStaticData};
use crate::modules::mining::gamelog::LogTime;
use crate::settings::Settings;
use crate::static_data::StaticData;
use crate::storage::Saver;
use crate::universe::Universe;

/// How many reports are kept in memory.
const REPORTS_KEPT: usize = 200;
/// Reports older than this when Athena starts aren't shown. PR 4 makes the
/// fade time a setting and uses it here.
const STARTUP_REPORT_MAX_AGE_SECS: i64 = 10 * 60;

/// Which view of the panel is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    Map,
    Settings,
}

/// Where one of the user's own characters is.
#[derive(Debug, Clone, PartialEq)]
pub struct Pilot {
    pub name: String,
    pub system_id: i64,
    /// When they arrived (the Local line's time).
    pub since: LogTime,
}

/// One intel report: a message from a watched channel, read.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub time: LogTime,
    pub channel: String,
    pub sender: String,
    pub text: String,
    pub message: IntelMessage,
}

pub struct IntelSupervisor {
    egui_ctx: egui::Context,
    data_dir: PathBuf,
    log_folder: PathBuf,
    saver: Saver,
    settings: IntelSettings,

    /// `None` if static.sqlite is missing or broken (see `problems`).
    /// Shared with the Mining Supervisor; the map is in `universe`.
    static_data: Option<Arc<StaticData>>,

    watcher: ChatLogWatcher,
    deduper: Deduper,
    /// Channels found in the Chatlogs folder (without Local).
    channels_found: Vec<String>,
    /// By character name.
    pilots: BTreeMap<String, Pilot>,
    /// The time of each pilot's newest system change, also when it was
    /// into a wormhole (and the pilot left `pilots`).
    last_jump: BTreeMap<String, LogTime>,
    /// Newest first.
    reports: VecDeque<Report>,
    /// Systems whose names may be shortened to a prefix in intel: the own
    /// pilots' regions and the regions next to them.
    nearby: HashSet<i64>,
    /// The regions `nearby` was built from, to rebuild it only on change.
    nearby_regions: BTreeSet<i64>,

    /// Things the user should know about, by source.
    problems: BTreeMap<&'static str, String>,
    view: View,
    map: MapView,
    /// The pilot picked on the Follow button; `None` follows whoever
    /// jumped most recently.
    follow: Option<String>,
    /// The active pilot's region at the last check, to notice when they
    /// jump into another one.
    followed_region: Option<i64>,
}

/// The current time as EVE time (UTC).
fn now() -> LogTime {
    to_log_time(SystemTime::now())
}

fn to_log_time(time: SystemTime) -> LogTime {
    let unix_seconds = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    LogTime { unix_seconds }
}

impl IntelSupervisor {
    /// Runs once at startup, before the window shows: reads the settings
    /// and the static data, then starts the watcher thread.
    pub fn new(
        egui_ctx: &egui::Context,
        app_settings: &Settings,
        data_dir: &Path,
        shared: &SharedStaticData,
    ) -> Self {
        let mut problems = BTreeMap::new();
        let (settings, settings_problem) = IntelSettings::load(data_dir);
        if let Some(problem) = settings_problem {
            problems.insert("settings", problem);
        }
        if let Some(problem) = &shared.problem {
            problems.insert("static", problem.clone());
        }
        let watcher = ChatLogWatcher::start(
            &app_settings.log_folder,
            settings.channels.clone(),
            egui_ctx.clone(),
        );
        IntelSupervisor {
            egui_ctx: egui_ctx.clone(),
            data_dir: data_dir.to_path_buf(),
            log_folder: app_settings.log_folder.clone(),
            saver: Saver::start(),
            settings,
            static_data: shared.data.clone(),
            watcher,
            deduper: Deduper::new(),
            channels_found: Vec::new(),
            pilots: BTreeMap::new(),
            last_jump: BTreeMap::new(),
            reports: VecDeque::new(),
            nearby: HashSet::new(),
            nearby_regions: BTreeSet::new(),
            problems,
            view: View::Map,
            map: MapView::default(),
            follow: None,
            followed_region: None,
        }
    }

    /// The map, if static data loaded.
    fn universe(&self) -> Option<&Universe> {
        self.static_data.as_deref().map(|data| &data.universe)
    }

    /// New lines from one chat log file.
    fn handle_lines(
        &mut self,
        channel: &str,
        listener: &str,
        lines: Vec<ChatLine>,
        catch_up: bool,
    ) {
        if channel == LOCAL {
            for line in &lines {
                self.pilot_moved(listener, line);
            }
            self.update_nearby();
        } else if self.settings.channels.iter().any(|c| c == channel) {
            let now = now();
            for line in lines {
                // At startup, only what is still recent.
                if catch_up && now.seconds_since(line.time) > STARTUP_REPORT_MAX_AGE_SECS {
                    continue;
                }
                self.add_report(channel, line);
            }
        }
    }

    /// A "Channel changed to Local" line: the pilot is now in that system.
    fn pilot_moved(&mut self, pilot: &str, line: &ChatLine) {
        // A clone of the `Arc` (cheap: it only counts up), so `self` can be
        // changed below while the map is in use.
        let (Some(data), Some(name)) = (self.static_data.clone(), local_system_change(line)) else {
            return;
        };
        let universe = &data.universe;
        // Lines from a relog's files can arrive out of order; only a newer
        // line moves the pilot.
        let newer = self
            .last_jump
            .get(pilot)
            .is_none_or(|&known| known <= line.time);
        if !newer {
            return;
        }
        self.last_jump.insert(pilot.to_string(), line.time);
        match universe.system_by_name(name) {
            // Wormhole systems have no gates and aren't on the map.
            None => {
                self.pilots.remove(pilot);
            }
            Some(system) => {
                self.pilots.insert(
                    pilot.to_string(),
                    Pilot {
                        name: pilot.to_string(),
                        system_id: system.id,
                        since: line.time,
                    },
                );
            }
        }
    }

    /// Own pilots who logged in since the last downtime; the others are
    /// old news (a character not played for weeks still has Local logs).
    /// Includes pilots hidden on their chip (see [`Self::shown_pilots`]).
    pub fn current_pilots(&self) -> impl Iterator<Item = &Pilot> {
        let downtime = to_log_time(last_downtime(SystemTime::now()));
        self.pilots.values().filter(move |p| p.since >= downtime)
    }

    /// The pilots on the map, who count for jump distances.
    pub fn shown_pilots(&self) -> impl Iterator<Item = &Pilot> {
        self.current_pilots()
            .filter(|p| !self.settings.hidden_pilots.contains(&p.name))
    }

    /// The pilot the map follows: the one picked on the Follow button, or
    /// whoever jumped most recently.
    pub fn active_pilot(&self) -> Option<&Pilot> {
        if let Some(name) = &self.follow
            && let Some(pilot) = self.shown_pilots().find(|p| &p.name == name)
        {
            return Some(pilot);
        }
        self.shown_pilots().max_by_key(|p| p.since)
    }

    /// Own pilot names by the system they're in, for the map.
    pub fn pilots_by_system(&self) -> HashMap<i64, Vec<String>> {
        let mut by_system: HashMap<i64, Vec<String>> = HashMap::new();
        for pilot in self.shown_pilots() {
            by_system
                .entry(pilot.system_id)
                .or_default()
                .push(pilot.name.clone());
        }
        by_system
    }

    /// Picks the map's region. At first the one shown last (or the first
    /// one). With Follow on, it switches whenever the active pilot turns up
    /// in another region, but a region picked by hand stays until then.
    fn choose_region(&mut self) {
        let Some(data) = self.static_data.clone() else {
            return;
        };
        let universe = &data.universe;
        let pilot_region = self
            .active_pilot()
            .and_then(|p| universe.system(p.system_id))
            .map(|s| s.region_id);
        let pilot_moved_region = pilot_region.is_some() && pilot_region != self.followed_region;
        self.followed_region = pilot_region;
        if pilot_moved_region && self.settings.follow_regions {
            if let Some(region) = pilot_region {
                self.show_region(region);
            }
        } else if self.map.region_id().is_none() {
            let first = universe.regions_by_name().first().map(|r| r.id);
            let saved = self
                .settings
                .region_id
                .filter(|&id| universe.region(id).is_some());
            if let Some(region) = pilot_region.or(saved).or(first) {
                self.show_region(region);
            }
        }
    }

    /// Shows a region on the map and remembers it.
    fn show_region(&mut self, region_id: i64) {
        let Some(data) = self.static_data.clone() else {
            return;
        };
        self.map.show_region(&data.universe, region_id);
        if self.settings.region_id != Some(region_id) {
            self.settings.region_id = Some(region_id);
            self.save_settings();
        }
        self.update_nearby();
    }

    /// A pilot chip was clicked: hide or show that pilot.
    fn toggle_pilot_hidden(&mut self, name: &str) {
        if self.settings.hidden_pilots.iter().any(|n| n == name) {
            self.settings.hidden_pilots.retain(|n| n != name);
        } else {
            self.settings.hidden_pilots.push(name.to_string());
        }
        self.save_settings();
        self.update_nearby();
        self.choose_region();
    }

    /// Rebuilds the set of systems that may be named by a prefix.
    fn update_nearby(&mut self) {
        let Some(universe) = self.universe() else {
            return;
        };
        let mut regions = BTreeSet::new();
        if let Some(shown) = self.map.region_id() {
            regions.insert(shown);
            regions.extend(universe.neighbour_regions(shown));
        }
        for pilot in self.shown_pilots() {
            if let Some(system) = universe.system(pilot.system_id) {
                regions.insert(system.region_id);
                regions.extend(universe.neighbour_regions(system.region_id));
            }
        }
        if regions == self.nearby_regions {
            return;
        }
        self.nearby = regions
            .iter()
            .filter_map(|&id| universe.region(id))
            .flat_map(|region| region.systems.iter().copied())
            .collect();
        self.nearby_regions = regions;
    }

    /// Reads one message from an intel channel and keeps it, unless it's a
    /// copy of one we already have.
    fn add_report(&mut self, channel: &str, line: ChatLine) {
        // The channel's message of the day and similar.
        if line.sender == EVE_SYSTEM {
            return;
        }
        if !self.deduper.is_new(line.time, &line.sender, &line.text) {
            return;
        }
        let message = match self.universe() {
            Some(universe) => parser::parse(
                &line.text,
                &ParseContext {
                    universe,
                    nearby: &self.nearby,
                    clear_words: &self.settings.clear_words,
                },
            ),
            None => IntelMessage::default(),
        };
        let report = Report {
            time: line.time,
            channel: channel.to_string(),
            sender: line.sender,
            text: line.text,
            message,
        };
        // Keep newest first, even when files are read slightly out of order.
        let at = self
            .reports
            .iter()
            .position(|r| r.time <= report.time)
            .unwrap_or(self.reports.len());
        self.reports.insert(at, report);
        self.reports.truncate(REPORTS_KEPT);
    }

    /// A channel was ticked or unticked in Settings.
    fn set_channel_watched(&mut self, channel: &str, watched: bool) {
        self.settings.channels.retain(|c| c != channel);
        if watched {
            self.settings.channels.push(channel.to_string());
            self.settings.channels.sort();
        }
        self.watcher.set_watched(self.settings.channels.clone());
        self.save_settings();
    }

    fn save_settings(&mut self) {
        if let Ok(text) = self.settings.to_toml() {
            self.saver
                .save(IntelSettings::file_path(&self.data_dir), text);
        }
    }
}

impl Module for IntelSupervisor {
    fn id(&self) -> &'static str {
        "intel"
    }

    fn title(&self) -> &'static str {
        "Intel Supervisor"
    }

    fn update(&mut self, _ctx: &ModuleContext) {
        // `try_recv` returns at once when the channel is empty.
        while let Ok(event) = self.watcher.events.try_recv() {
            match event {
                WatchEvent::Channels(channels) => {
                    self.channels_found = channels;
                }
                WatchEvent::Lines {
                    channel,
                    listener,
                    lines,
                    catch_up,
                } => self.handle_lines(&channel, &listener, lines, catch_up),
                WatchEvent::CaughtUp => {}
                WatchEvent::Problem(Some(text)) => {
                    self.problems.insert("chatlogs", text);
                }
                WatchEvent::Problem(None) => {
                    self.problems.remove("chatlogs");
                }
            }
        }
        // Cheap when nothing changed: a few comparisons.
        self.choose_region();
    }

    fn ui(&mut self, ui: &mut egui::Ui, _ctx: &ModuleContext) {
        ui::show(self, ui);
    }

    fn settings_changed(&mut self, settings: &Settings) {
        if settings.log_folder == self.log_folder {
            return;
        }
        self.log_folder = settings.log_folder.clone();
        self.watcher = ChatLogWatcher::start(
            &settings.log_folder,
            self.settings.channels.clone(),
            self.egui_ctx.clone(),
        );
        self.channels_found.clear();
        self.pilots.clear();
        self.last_jump.clear();
    }

    fn on_exit(&mut self) {
        self.saver.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module() -> IntelSupervisor {
        let dir = std::env::temp_dir().join(format!("athena-intel-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let settings = Settings {
            log_folder: dir.join("no-logs-here"),
        };
        // The sample static data includes a tiny made-up map.
        let data = crate::static_data::tests::sample();
        let shared = SharedStaticData {
            data: Some(Arc::new(data)),
            problem: None,
        };
        let mut module = IntelSupervisor::new(&egui::Context::default(), &settings, &dir, &shared);
        module.settings.channels = vec!["Branch Intel".into(), "Region Intel".into()];
        module
    }

    fn line(seconds_ago: i64, sender: &str, text: &str) -> ChatLine {
        ChatLine {
            time: LogTime {
                unix_seconds: now().unix_seconds - seconds_ago,
            },
            sender: sender.into(),
            text: text.into(),
        }
    }

    #[test]
    fn local_places_pilots_and_enables_prefixes() {
        let mut m = module();
        m.handle_lines(
            LOCAL,
            "Pilot Alpha",
            vec![line(1, EVE_SYSTEM, "Channel changed to Local : A2-XYZ")],
            true,
        );
        let shown: Vec<&Pilot> = m.shown_pilots().collect();
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].system_id, 102);
        // Alpha region and its neighbour Beta are nearby now, so "a3-" works.
        m.handle_lines(
            "Branch Intel",
            "Pilot Alpha",
            vec![line(5, "Scout", "a3-q loki")],
            false,
        );
        assert_eq!(m.reports[0].message.systems, [103]);
        assert_eq!(m.reports[0].message.ships, ["Loki"]);
    }

    #[test]
    fn copies_from_several_pilots_and_channels_become_one_report() {
        let mut m = module();
        for (channel, pilot) in [
            ("Branch Intel", "Pilot Alpha"),
            ("Branch Intel", "Pilot Bravo"),
            ("Region Intel", "Pilot Alpha"),
        ] {
            m.handle_lines(channel, pilot, vec![line(5, "Scout", "A1-XYZ")], false);
        }
        assert_eq!(m.reports.len(), 1);
        assert_eq!(m.reports[0].channel, "Branch Intel");
    }

    #[test]
    fn unwatched_channels_old_catch_up_and_motd_are_skipped() {
        let mut m = module();
        m.handle_lines(
            "Corp",
            "Pilot Alpha",
            vec![line(5, "Scout", "A1-XYZ")],
            false,
        );
        m.handle_lines(
            "Branch Intel",
            "Pilot Alpha",
            vec![
                line(3600, "Scout", "A1-XYZ old"),
                line(5, EVE_SYSTEM, "Channel MOTD: be nice"),
            ],
            true,
        );
        assert!(m.reports.is_empty());
    }

    #[test]
    fn reports_stay_newest_first() {
        let mut m = module();
        m.handle_lines("Branch Intel", "A", vec![line(5, "Scout", "one")], false);
        m.handle_lines("Region Intel", "A", vec![line(60, "Scout", "two")], false);
        m.handle_lines("Branch Intel", "A", vec![line(1, "Scout", "three")], false);
        let texts: Vec<&str> = m.reports.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(texts, ["three", "one", "two"]);
    }

    #[test]
    fn a_pilot_in_a_wormhole_leaves_the_map() {
        let mut m = module();
        m.handle_lines(
            LOCAL,
            "Pilot Alpha",
            vec![
                line(2, EVE_SYSTEM, "Channel changed to Local : A1-XYZ"),
                line(1, EVE_SYSTEM, "Channel changed to Local : J123456"),
            ],
            false,
        );
        assert_eq!(m.shown_pilots().count(), 0);
    }

    #[test]
    fn an_older_wormhole_line_doesnt_remove_a_pilot() {
        let mut m = module();
        m.handle_lines(
            LOCAL,
            "Pilot Alpha",
            vec![line(1, EVE_SYSTEM, "Channel changed to Local : A1-XYZ")],
            false,
        );
        // An earlier session's file, read later.
        m.handle_lines(
            LOCAL,
            "Pilot Alpha",
            vec![line(2, EVE_SYSTEM, "Channel changed to Local : J123456")],
            false,
        );
        assert_eq!(m.shown_pilots().count(), 1);
        // And the other way round: an older system line after a newer
        // wormhole line doesn't bring the pilot back.
        let mut m = module();
        m.handle_lines(
            LOCAL,
            "Pilot Alpha",
            vec![line(1, EVE_SYSTEM, "Channel changed to Local : J123456")],
            false,
        );
        m.handle_lines(
            LOCAL,
            "Pilot Alpha",
            vec![line(2, EVE_SYSTEM, "Channel changed to Local : A1-XYZ")],
            false,
        );
        assert_eq!(m.shown_pilots().count(), 0);
    }

    fn jump(m: &mut IntelSupervisor, pilot: &str, seconds_ago: i64, system: &str) {
        let text = format!("Channel changed to Local : {system}");
        m.handle_lines(
            LOCAL,
            pilot,
            vec![line(seconds_ago, EVE_SYSTEM, &text)],
            false,
        );
        m.choose_region();
    }

    #[test]
    fn follow_switches_region_only_when_the_pilot_does() {
        let mut m = module();
        jump(&mut m, "Pilot Alpha", 50, "A1-XYZ");
        assert_eq!(m.map.region_id(), Some(1));
        // Picked by hand: stays while the pilot moves inside region 1.
        m.show_region(2);
        jump(&mut m, "Pilot Alpha", 40, "A2-XYZ");
        assert_eq!(m.map.region_id(), Some(2));
        // Into region 2 and back to region 1: the map follows.
        jump(&mut m, "Pilot Alpha", 30, "B1");
        jump(&mut m, "Pilot Alpha", 20, "A2-XYZ");
        assert_eq!(m.map.region_id(), Some(1));
        // With Follow off it stays.
        m.settings.follow_regions = false;
        jump(&mut m, "Pilot Alpha", 10, "B1");
        assert_eq!(m.map.region_id(), Some(1));
    }

    #[test]
    fn hidden_pilots_are_left_out_and_the_follow_pick_wins() {
        let mut m = module();
        jump(&mut m, "Pilot Alpha", 20, "A1-XYZ");
        jump(&mut m, "Pilot Bravo", 10, "B2");
        // Whoever jumped last.
        assert_eq!(m.active_pilot().unwrap().name, "Pilot Bravo");
        m.follow = Some("Pilot Alpha".into());
        assert_eq!(m.active_pilot().unwrap().name, "Pilot Alpha");
        m.toggle_pilot_hidden("Pilot Alpha");
        assert_eq!(m.shown_pilots().count(), 1);
        assert_eq!(m.current_pilots().count(), 2);
        // The picked pilot is hidden, so it falls back to the last jumper.
        assert_eq!(m.active_pilot().unwrap().name, "Pilot Bravo");
        assert!(!m.pilots_by_system().contains_key(&101));
    }

    #[test]
    fn the_shown_region_counts_as_nearby() {
        let mut m = module();
        // No pilots: only the shown region (and its neighbours) are nearby.
        m.show_region(2);
        assert!(m.nearby.contains(&202));
        assert!(m.nearby.contains(&101));
    }
}
