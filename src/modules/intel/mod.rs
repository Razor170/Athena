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
//! - [`alerts`]: report colours on the map, and which reports raise an alert.
//! - [`sound`]: plays the alert voice lines or a beep on its own thread.
//! - [`tags`]: labels on systems, in RIFT's text format.
//! - [`lookup`]: ESI on a background thread: which pilot-name guesses are
//!   real characters (for zKillboard links), and the optional kills/jumps
//!   tint.
//! - [`ui`]: draws the panel.
//!
//! This file holds the [`IntelSupervisor`] module: it owns the watcher,
//! drains its channel in `update`, and keeps the reports and where each own
//! pilot is.

pub mod alerts;
pub mod chatlog;
pub mod config;
pub mod dedupe;
pub mod lookup;
pub mod map;
pub mod parser;
pub mod sound;
pub mod tags;
mod ui;
pub mod watcher;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Instant, SystemTime};

use eframe::egui;

use self::chatlog::{ChatLine, EVE_SYSTEM, local_system_change};
use self::config::IntelSettings;
use self::dedupe::Deduper;
use self::lookup::{Answer, Lookup, SystemActivity};
use self::map::MapView;
use self::parser::{IntelMessage, ParseContext};
use self::sound::{PlayRequest, SoundPlayer};
use self::tags::Tag;
use self::watcher::{ChatLogWatcher, LOCAL, WatchEvent, last_downtime};
use super::shared::{PilotSystem, PilotSystems};
use super::{Module, ModuleContext, SharedStaticData};
use crate::modules::mining::gamelog::LogTime;
use crate::overlay::popup::{AlertPopup, PopupContent};
use crate::settings::Settings;
use crate::static_data::StaticData;
use crate::storage::Saver;
use crate::universe::Universe;

/// How many reports are kept in memory.
const REPORTS_KEPT: usize = 200;
/// Jump distances from the own pilots are worked out this far, for the
/// "4 jumps" badges; alerts go up to `sound::MAX_JUMPS`.
const DISTANCE_LIMIT: u32 = 30;
/// "Mute 5 min".
const MUTE_SECS: i64 = 5 * 60;
/// Tag text bigger than this on the clipboard isn't read.
const MAX_IMPORT_BYTES: usize = 4 * 1024 * 1024;

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

/// The "Add tag…" window's fields.
#[derive(Debug, Clone, PartialEq)]
pub struct TagDialog {
    pub system_id: i64,
    pub label: String,
    pub group: String,
    /// The group box is a text field for a new group.
    pub new_group: bool,
    pub color: egui::Color32,
    pub icon: String,
    /// Put the cursor in the label field (once, when the window opens).
    pub focus_label: bool,
}

/// The alert banner over the map: the newest report that raised an alert.
#[derive(Debug, Clone, PartialEq)]
pub struct Alert {
    pub report: Report,
    /// The reported system nearest to an own pilot.
    pub system_id: i64,
    pub jumps: u32,
    /// The own pilots that close to it.
    pub pilots: Vec<String>,
    pub loud: bool,
    /// The popup over EVE was shown for this alert: the banner goes when
    /// the popup does.
    pub popup_shown: bool,
    /// When the banner appeared. Without a popup, the banner stays as long
    /// as the popup would have.
    pub shown_at: Instant,
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
    /// The same pilot locations, shared with other modules (Finance uses
    /// them for the nearest LP store).
    pilot_systems: PilotSystems,
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
    /// Jumps from the nearest shown own pilot, for systems up to
    /// `DISTANCE_LIMIT` away. Rebuilt when a pilot moves or is hidden.
    distances: HashMap<i64, u32>,

    sound: SoundPlayer,
    /// The banner over the map, while the alert popup shows (or would
    /// show, see [`banner_is_over`]).
    alert: Option<Alert>,
    /// The alert popup over EVE (shown even while muted).
    popup: AlertPopup,
    /// "Mute 5 min" was clicked: no sound or flashing until then.
    muted_until: Option<LogTime>,
    /// "Silence Alarms" was clicked: no sound or flashing until "Enable
    /// Alarms" is clicked. Not saved, so a restart turns alarms back on.
    silenced: bool,
    /// The clear words box in Settings while it is being typed in.
    clear_words_text: String,
    /// System tags, sorted by system and label.
    tags: Vec<Tag>,
    /// Lines of the tag file that aren't readable tags, written back as
    /// they were.
    unread_tag_lines: Vec<String>,
    /// False when the tag file couldn't be read: then it is never saved.
    tags_writable: bool,
    /// The "Add tag…" window, while it is open.
    tag_dialog: Option<TagDialog>,
    /// The group of the last tag added with that window.
    last_tag_group: Option<String>,

    /// ESI lookups on a background thread.
    lookup: Lookup,
    /// Pilot-name guesses (lowercase) that ESI answered: the character,
    /// or `None` if it isn't one.
    characters: HashMap<String, Option<lookup::Character>>,
    /// Names sent to ESI and not answered yet.
    asked: HashSet<String>,
    /// Kills and jumps of the last hour, while the tint is on.
    activity: HashMap<i64, SystemActivity>,
    /// What the last tag import or export did, shown in Settings.
    tag_message: Option<String>,

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

/// Whether the banner over the map should go. It lives as long as the
/// alert popup: when the popup was shown for this alert, until the popup
/// hides (time up, ✕, or popups turned off). Without a popup (turned off,
/// or "only loud" and this one isn't), as long as the popup would have
/// shown. Kept "until ✕" without a popup, it goes once its report has
/// faded. A faded report always ends it.
fn banner_is_over(
    alert: &Alert,
    popup_showing: bool,
    popup_hide_after: Option<std::time::Duration>,
    faded: bool,
) -> bool {
    if faded {
        return true;
    }
    if alert.popup_shown {
        return !popup_showing;
    }
    popup_hide_after.is_some_and(|after| alert.shown_at.elapsed() >= after)
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
        pilot_systems: PilotSystems,
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
        let clear_words_text = settings.clear_words.join(", ");
        let lookup = Lookup::start(egui_ctx.clone());
        if settings.esi_tint {
            lookup.ask(lookup::Request::Activity(true));
        }
        // If the tag file can't be read, it is never written either, so
        // nothing in it gets lost.
        let (tags, unread_tag_lines, tags_writable, tag_message) = match &shared.data {
            Some(data) => match tags::load(data_dir, &data.universe) {
                Ok(parsed) => {
                    let message = (!parsed.unread.is_empty()).then(|| {
                        format!(
                            "{} lines of {} name no known system; they are kept in the file \
                             but not shown.",
                            parsed.unread.len(),
                            tags::file_path(data_dir).display()
                        )
                    });
                    (parsed.tags, parsed.unread, true, message)
                }
                Err(problem) => (
                    Vec::new(),
                    Vec::new(),
                    false,
                    Some(format!("{problem} Tags are off until it can be read.")),
                ),
            },
            None => (Vec::new(), Vec::new(), false, None),
        };
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
            pilot_systems,
            last_jump: BTreeMap::new(),
            reports: VecDeque::new(),
            nearby: HashSet::new(),
            nearby_regions: BTreeSet::new(),
            distances: HashMap::new(),
            sound: SoundPlayer::start(),
            alert: None,
            muted_until: None,
            silenced: false,
            popup: AlertPopup::new(),
            clear_words_text,
            tags,
            unread_tag_lines,
            tags_writable,
            tag_dialog: None,
            last_tag_group: None,
            lookup,
            characters: HashMap::new(),
            asked: HashSet::new(),
            activity: HashMap::new(),
            tag_message,
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
            self.update_distances();
        } else if self.settings.channels.iter().any(|c| c == channel) {
            let now = now();
            for line in lines {
                // At startup, only what is still recent, and no alerts for
                // it: it's old news.
                if catch_up && now.seconds_since(line.time) > self.fade_seconds() {
                    continue;
                }
                if let Some(at) = self.add_report(channel, line)
                    && !catch_up
                {
                    self.check_alert(at);
                }
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
                if let Ok(mut shared) = self.pilot_systems.lock() {
                    shared.remove(pilot);
                }
            }
            Some(system) => {
                if let Ok(mut shared) = self.pilot_systems.lock() {
                    shared.insert(
                        pilot.to_string(),
                        PilotSystem {
                            system_id: system.id,
                            since_unix: line.time.unix_seconds,
                        },
                    );
                }
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
            // With Follow off, the region picked last wins at start-up.
            let pick = if self.settings.follow_regions {
                pilot_region.or(saved)
            } else {
                saved.or(pilot_region)
            };
            if let Some(region) = pick.or(first) {
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

    /// Pilots were ticked or unticked in the Pilots list: show or hide
    /// them on the map and for alerts. Saves once for all of them.
    fn set_pilots_hidden(&mut self, names: &[String], hidden: bool) {
        for name in names {
            let is_hidden = self.settings.hidden_pilots.contains(name);
            if hidden && !is_hidden {
                self.settings.hidden_pilots.push(name.clone());
            } else if !hidden && is_hidden {
                self.settings.hidden_pilots.retain(|n| n != name);
            }
        }
        self.save_settings();
        self.update_nearby();
        self.update_distances();
        self.choose_region();
    }

    /// Rebuilds the jump distances from the shown own pilots.
    fn update_distances(&mut self) {
        let starts: Vec<i64> = self.shown_pilots().map(|p| p.system_id).collect();
        let distances = match self.universe() {
            Some(universe) => universe.jumps_within(&starts, DISTANCE_LIMIT),
            None => HashMap::new(),
        };
        self.distances = distances;
    }

    /// The fade time in seconds.
    pub fn fade_seconds(&self) -> i64 {
        i64::from(self.settings.fade_minutes.max(1)) * 60
    }

    /// The highest alert range the settings allow.
    fn alert_jumps(&self) -> u32 {
        self.settings.alert_jumps.min(sound::MAX_JUMPS)
    }

    /// No sound or flashing right now: silenced, or muted for 5 minutes.
    pub fn is_muted(&self) -> bool {
        self.silenced || self.mute_timer_running()
    }

    /// "Mute 5 min" is counting down.
    fn mute_timer_running(&self) -> bool {
        self.muted_until
            .is_some_and(|until| now().seconds_since(until) < 0)
    }

    /// "Mute 5 min", or unmute when clicked again. Does nothing while
    /// silenced (the button is greyed out then).
    fn toggle_mute(&mut self) {
        if self.silenced {
            return;
        }
        self.muted_until = if self.mute_timer_running() {
            None
        } else {
            Some(LogTime {
                unix_seconds: now().unix_seconds + MUTE_SECS,
            })
        };
    }

    /// "Silence Alarms" / "Enable Alarms". Either way a running 5 min
    /// mute ends: silence replaces it, and enabling turns everything on.
    fn toggle_silence(&mut self) {
        self.silenced = !self.silenced;
        self.muted_until = None;
    }

    /// The report at `at` just arrived live: raise an alert if it is close
    /// enough to an own pilot. (It may not be the newest one: another
    /// channel or pilot's log can be read a moment later.)
    fn check_alert(&mut self, at: usize) {
        let Some(report) = self.reports.get(at) else {
            return;
        };
        let alert_jumps = self.alert_jumps();
        let Some(level) = alerts::alert_for(
            report,
            &self.distances,
            alert_jumps,
            self.settings.loud_jumps.min(alert_jumps),
        ) else {
            return;
        };
        // The named system that is that close, and who is that close to it.
        let Some(&system_id) = report
            .message
            .systems
            .iter()
            .find(|id| self.distances.get(id) == Some(&level.jumps))
        else {
            return;
        };
        let pilots = match self.universe() {
            Some(universe) => {
                let around = universe.jumps_within(&[system_id], level.jumps);
                self.shown_pilots()
                    .filter(|p| around.contains_key(&p.system_id))
                    .map(|p| p.name.clone())
                    .collect()
            }
            None => Vec::new(),
        };
        // A report read late that is older than the banner's and further
        // away doesn't replace it; it still sounds.
        let keep_banner = self
            .alert
            .as_ref()
            .is_some_and(|shown| shown.jumps < level.jumps && shown.report.time >= report.time);
        if !keep_banner {
            let popup_shown = self.settings.popup && (level.loud || !self.settings.popup_only_loud);
            let alert = Alert {
                report: report.clone(),
                system_id,
                jumps: level.jumps,
                pilots,
                loud: level.loud,
                popup_shown,
                shown_at: Instant::now(),
            };
            let hide_after = self.popup_hide_after();
            // Same rule as the banner: what replaces the banner replaces
            // the popup. Muting doesn't stop it (mute is about sound).
            if popup_shown {
                let (title, details) = ui::alert_texts(self, &alert);
                let content = PopupContent {
                    title,
                    details,
                    loud: alert.loud,
                };
                self.popup.show(&self.egui_ctx, content, hide_after);
            } else if let Some(after) = hide_after {
                // Wakes `update` up in time to take the banner away.
                self.egui_ctx.request_repaint_after(after);
            }
            self.alert = Some(alert);
        }
        if self.is_muted() {
            return;
        }
        self.sound.play(PlayRequest {
            jumps: level.jumps,
            kind: self.settings.sound,
            volume: self.settings.volume,
            loud: level.loud,
        });
        if self.settings.flash {
            // Flashes the taskbar button until Athena gets the focus.
            self.egui_ctx
                .send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(
                    egui::UserAttentionType::Critical,
                ));
        }
    }

    /// Opens the "Add tag…" window for a system. It starts with the group
    /// used for the last tag added here (else the first group), and that
    /// group's colour and icon.
    fn open_tag_dialog(&mut self, system_id: i64) {
        if !self.tags_writable {
            if let Some(message) = &self.tag_message {
                self.problems.insert("tags", message.clone());
            }
            return;
        }
        let group = self
            .last_tag_group
            .clone()
            .filter(|g| self.tags.iter().any(|t| &t.group == g))
            .or_else(|| tags::groups(&self.tags).into_iter().next())
            .unwrap_or_default();
        let color = tags::group_color(&self.tags, &group)
            .unwrap_or(egui::Color32::from_rgb(0xdc, 0xdc, 0xdc));
        // An icon Athena doesn't draw (from an import) becomes the dot.
        let icon = self
            .tags
            .iter()
            .rev()
            .find(|t| t.group == group)
            .map(|t| t.icon.clone())
            .filter(|icon| tags::ICONS.iter().any(|(known, _)| known == icon))
            .unwrap_or_else(|| "dot".to_string());
        self.tag_dialog = Some(TagDialog {
            system_id,
            label: String::new(),
            new_group: group.is_empty() && self.tags.is_empty(),
            group,
            color,
            icon,
            focus_label: true,
        });
    }

    /// "Add" in the "Add tag…" window.
    fn add_tag_from_dialog(&mut self) {
        let Some(dialog) = self.tag_dialog.take() else {
            return;
        };
        let label = dialog.label.trim().replace(';', ",");
        let Some(name) = self
            .universe()
            .and_then(|u| u.system(dialog.system_id))
            .map(|s| s.name.clone())
        else {
            return;
        };
        if label.is_empty() {
            return;
        }
        let tag = Tag {
            system_id: dialog.system_id,
            system_name: name,
            label,
            color: dialog.color,
            color_text: tags::color_text(dialog.color),
            icon: dialog.icon,
            flag: "false".to_string(),
            group: dialog.group.trim().replace(';', ","),
            extra: Vec::new(),
        };
        self.last_tag_group = Some(tag.group.clone());
        // The same label again on the same system updates that tag.
        if let Some(existing) = self.tags.iter_mut().find(|t| t.same_as(&tag)) {
            *existing = tag;
        } else {
            tags::merge(&mut self.tags, vec![tag]);
        }
        self.save_tags();
    }

    fn remove_tag(&mut self, index: usize) {
        if index < self.tags.len() {
            self.tags.remove(index);
            self.save_tags();
        }
    }

    /// "Import": adds the tags on the clipboard (RIFT's format).
    fn import_tags(&mut self) {
        let Some(text) = crate::modules::mining::clipboard::read_text_once(MAX_IMPORT_BYTES) else {
            self.tag_message = Some(
                "Couldn't read text from the clipboard. Copy the tags (in RIFT: Export) and \
                 try again."
                    .to_string(),
            );
            return;
        };
        self.import_tag_text(&text);
    }

    fn import_tag_text(&mut self, text: &str) {
        if !self.tags_writable {
            return;
        }
        let Some(universe) = self.universe() else {
            return;
        };
        let parsed = tags::parse(text, universe);
        let found = parsed.tags.len();
        let added = tags::merge(&mut self.tags, parsed.tags);
        let mut message = format!("Imported {added} tags");
        if added < found {
            message += &format!(" ({} were already there)", found - added);
        }
        if !parsed.unread.is_empty() {
            message += &format!(
                "; {} lines weren't tags and were skipped",
                parsed.unread.len()
            );
        }
        self.tag_message = Some(message + ".");
        if added > 0 {
            self.save_tags();
        }
    }

    /// "Export": copies all tags to the clipboard.
    fn export_tags(&mut self) {
        self.egui_ctx.copy_text(tags::format(&self.tags, &[]));
        self.tag_message = Some(format!("Copied {} tags to the clipboard.", self.tags.len()));
    }

    /// A tag group was ticked or unticked under "Show groups on map".
    fn set_tag_group_shown(&mut self, group: &str, shown: bool) {
        self.settings.hidden_tag_groups.retain(|g| g != group);
        if !shown {
            self.settings.hidden_tag_groups.push(group.to_string());
        }
        self.save_settings();
    }

    fn save_tags(&mut self) {
        if !self.tags_writable {
            return;
        }
        self.saver.save(
            tags::file_path(&self.data_dir),
            tags::format(&self.tags, &self.unread_tag_lines),
        );
    }

    /// Something picked in the map's right-click menu.
    fn map_action(&mut self, action: map::MapAction) {
        match action {
            map::MapAction::AddTag(system) => self.open_tag_dialog(system),
            map::MapAction::MarkClear(system) => self.mark_clear(system),
            map::MapAction::OpenOnDotlan(system) => {
                let Some(name) = self
                    .universe()
                    .and_then(|u| u.system(system))
                    .map(|s| s.name.clone())
                else {
                    return;
                };
                let url = format!(
                    "https://evemaps.dotlan.net/system/{}",
                    name.replace(' ', "_")
                );
                match crate::shell::open_url(&url) {
                    Ok(()) => {
                        self.problems.remove("browser");
                    }
                    Err(problem) => {
                        self.problems.insert("browser", problem);
                    }
                }
            }
        }
    }

    /// Asks ESI about pilot-name guesses it hasn't been asked about.
    fn look_up_names(&mut self, candidates: &[String]) {
        let new: Vec<String> = candidates
            .iter()
            .map(|c| c.to_lowercase())
            .filter(|c| !self.characters.contains_key(c) && !self.asked.contains(c))
            .collect();
        if new.is_empty() {
            return;
        }
        self.asked.extend(new.iter().cloned());
        self.lookup.ask(lookup::Request::Names(new));
    }

    /// The tint switch in Settings.
    fn set_esi_tint(&mut self, on: bool) {
        self.settings.esi_tint = on;
        self.save_settings();
        self.lookup.ask(lookup::Request::Activity(on));
        if !on {
            self.activity.clear();
        }
    }

    /// "Mark clear": a clear report from the user, which turns the system
    /// green like a "clr" in an intel channel.
    fn mark_clear(&mut self, system: i64) {
        let Some(name) = self
            .universe()
            .and_then(|u| u.system(system))
            .map(|s| s.name.clone())
        else {
            return;
        };
        let report = Report {
            time: now(),
            channel: "Athena".to_string(),
            sender: "You".to_string(),
            text: format!("{name} marked clear"),
            message: IntelMessage {
                systems: vec![system],
                clear: true,
                ..IntelMessage::default()
            },
        };
        self.reports.push_front(report);
        self.reports.truncate(REPORTS_KEPT);
    }

    /// Plays the sound for a distance, from the play buttons in Settings.
    fn play_sample(&self, jumps: u32) {
        self.sound.play(PlayRequest {
            jumps,
            kind: self.settings.sound,
            volume: self.settings.volume,
            loud: false,
        });
    }

    /// The clear words typed in Settings ("clr, clear"), saved lowercase.
    fn set_clear_words(&mut self, text: &str) {
        let words: Vec<String> = text
            .split([',', ' '])
            .map(|w| w.trim().to_lowercase())
            .filter(|w| !w.is_empty())
            .collect();
        if words != self.settings.clear_words {
            self.settings.clear_words = words;
            self.save_settings();
        }
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
    /// copy of one we already have. Returns where it was put in `reports`.
    fn add_report(&mut self, channel: &str, line: ChatLine) -> Option<usize> {
        // The channel's message of the day and similar.
        if line.sender == EVE_SYSTEM {
            return None;
        }
        if !self.deduper.is_new(line.time, &line.sender, &line.text) {
            return None;
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
        self.look_up_names(&message.pilot_candidates);
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
        (at < REPORTS_KEPT).then_some(at)
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

    /// How long the popup stays, `None` until ✕.
    fn popup_hide_after(&self) -> Option<std::time::Duration> {
        (self.settings.popup_hide_secs > 0)
            .then(|| std::time::Duration::from_secs(self.settings.popup_hide_secs.into()))
    }

    /// "Show popup to move it" in Settings: an example that stays until ✕.
    fn show_example_popup(&mut self) {
        let content = PopupContent {
            title: "Example · drag me by the grip".to_string(),
            details: "Put the popup where you want alerts to show, then close it with ✕."
                .to_string(),
            loud: true,
        };
        self.popup.show(&self.egui_ctx, content, None);
    }

    fn reset_popup_position(&mut self) {
        self.settings.popup_position = None;
        self.popup.reset_position(&self.egui_ctx);
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
        "Intel"
    }

    fn update(&mut self, _ctx: &ModuleContext) {
        // Keeps the popup's window alive (hidden until an alert) and saves
        // where the user dragged it.
        let saved = self.settings.popup_position.map(|[x, y]| egui::pos2(x, y));
        if let Some(moved) = self
            .popup
            .keep_alive(&self.egui_ctx, self.settings.popup, saved)
        {
            self.settings.popup_position = Some([moved.x, moved.y]);
            self.save_settings();
        }

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
        while let Ok(answer) = self.lookup.answers.try_recv() {
            match answer {
                Answer::Names(names) => {
                    for (name, character) in names {
                        self.asked.remove(&name);
                        self.characters.insert(name, character);
                    }
                }
                Answer::NotAnswered(names) => {
                    // Forget them, so the next report with them asks again.
                    for name in names {
                        self.asked.remove(&name);
                    }
                }
                Answer::Activity(activity) => {
                    if self.settings.esi_tint {
                        self.activity = activity;
                    }
                }
                Answer::Problem(Some(text)) => {
                    self.problems.insert("esi", text);
                }
                Answer::Problem(None) => {
                    self.problems.remove("esi");
                }
            }
        }
        // Cheap when nothing changed: a few comparisons.
        self.choose_region();
        // The banner goes with the popup (or once its report has faded).
        if let Some(alert) = &self.alert
            && banner_is_over(
                alert,
                self.popup.is_showing(),
                self.popup_hide_after(),
                now().seconds_since(alert.report.time) >= self.fade_seconds(),
            )
        {
            self.alert = None;
        }
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
        self.distances.clear();
        self.alert = None;
    }

    fn on_exit(&mut self) {
        self.saver.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module() -> IntelSupervisor {
        // Tests run in parallel in one process, and a test that hides a
        // pilot saves intel.toml, so each module gets a folder of its own.
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("athena-intel-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let settings = Settings {
            log_folder: dir.join("no-logs-here"),
            ..Settings::default()
        };
        // The sample static data includes a tiny made-up map.
        let data = crate::static_data::tests::sample();
        let shared = SharedStaticData {
            data: Some(Arc::new(data)),
            problem: None,
        };
        let mut module = IntelSupervisor::new(
            &egui::Context::default(),
            &settings,
            &dir,
            &shared,
            PilotSystems::default(),
        );
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
    fn with_follow_off_the_saved_region_wins_at_start_up() {
        let mut m = module();
        m.settings.follow_regions = false;
        m.settings.region_id = Some(2);
        jump(&mut m, "Pilot Alpha", 10, "A1-XYZ");
        assert_eq!(m.map.region_id(), Some(2));
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
        m.set_pilots_hidden(&["Pilot Alpha".to_string()], true);
        assert_eq!(m.shown_pilots().count(), 1);
        assert_eq!(m.current_pilots().count(), 2);
        // The picked pilot is hidden, so it falls back to the last jumper.
        assert_eq!(m.active_pilot().unwrap().name, "Pilot Bravo");
        assert!(!m.pilots_by_system().contains_key(&101));
    }

    #[test]
    fn live_reports_near_a_pilot_raise_an_alert() {
        let mut m = module();
        jump(&mut m, "Pilot Alpha", 60, "A1-XYZ");
        assert_eq!(m.distances.get(&102), Some(&1));
        // Read at start-up: old news, no alert.
        m.handle_lines(
            "Branch Intel",
            "Pilot Alpha",
            vec![line(30, "Scout", "A2-XYZ")],
            true,
        );
        assert!(m.alert.is_none());
        // A clear word: no alert either.
        m.handle_lines(
            "Branch Intel",
            "Pilot Alpha",
            vec![line(20, "Scout", "A2-XYZ clr")],
            false,
        );
        assert!(m.alert.is_none());
        m.handle_lines(
            "Branch Intel",
            "Pilot Alpha",
            vec![line(10, "Scout", "A2-XYZ loki")],
            false,
        );
        let alert = m.alert.clone().unwrap();
        assert_eq!((alert.system_id, alert.jumps, alert.loud), (102, 1, true));
        assert_eq!(alert.pilots, ["Pilot Alpha"]);
        // Out of range.
        m.alert = None;
        m.settings.alert_jumps = 0;
        m.handle_lines(
            "Branch Intel",
            "Pilot Alpha",
            vec![line(5, "Scout", "A3-QQQ")],
            false,
        );
        assert!(m.alert.is_none());
    }

    #[test]
    fn a_live_report_older_than_the_newest_still_alerts() {
        let mut m = module();
        jump(&mut m, "Pilot Alpha", 60, "A1-XYZ");
        m.handle_lines(
            "Region Intel",
            "Pilot Alpha",
            vec![line(5, "Scout", "hi")],
            false,
        );
        // Read a moment later from the other channel, but written earlier.
        m.handle_lines(
            "Branch Intel",
            "Pilot Alpha",
            vec![line(8, "Scout", "A2-XYZ")],
            false,
        );
        assert_eq!(m.reports[1].message.systems, [102]);
        assert_eq!(m.alert.as_ref().map(|a| a.system_id), Some(102));
    }

    #[test]
    fn the_banner_goes_with_the_popup() {
        let mut m = module();
        jump(&mut m, "Pilot Alpha", 60, "A1-XYZ");
        m.handle_lines(
            "Branch Intel",
            "Pilot Alpha",
            vec![line(5, "Scout", "A2-XYZ")],
            false,
        );
        let mut alert = m.alert.clone().unwrap();
        let five = Some(std::time::Duration::from_secs(5));
        // Shown with a popup: lasts exactly as long as the popup.
        alert.popup_shown = true;
        assert!(!banner_is_over(&alert, true, five, false));
        assert!(banner_is_over(&alert, false, five, false));
        // No popup: as long as the popup would have shown.
        alert.popup_shown = false;
        assert!(!banner_is_over(&alert, false, five, false));
        alert.shown_at = Instant::now() - std::time::Duration::from_secs(6);
        assert!(banner_is_over(&alert, false, five, false));
        // No popup and "until ✕": until the report fades.
        assert!(!banner_is_over(&alert, false, None, false));
        assert!(banner_is_over(&alert, false, None, true));
    }

    #[test]
    fn an_older_further_report_keeps_the_closer_banner() {
        let mut m = module();
        jump(&mut m, "Pilot Alpha", 60, "A1-XYZ");
        m.handle_lines(
            "Branch Intel",
            "Pilot Alpha",
            vec![line(5, "Scout", "A1-XYZ")],
            false,
        );
        m.handle_lines(
            "Region Intel",
            "Pilot Alpha",
            vec![line(8, "Scout", "A2-XYZ")],
            false,
        );
        assert_eq!(m.alert.as_ref().map(|a| a.system_id), Some(101));
    }

    #[test]
    fn tags_import_add_and_remove() {
        let mut m = module();
        m.import_tag_text("A1-XYZ;POS;#F5FF83;structure_skyhook;false;POS\nnot a tag\n");
        assert_eq!(m.tags.len(), 1);
        assert!(m.tag_message.as_ref().unwrap().contains("Imported 1 tags"));
        // Importing again adds nothing.
        m.import_tag_text("A1-XYZ;POS;#F5FF83;structure_skyhook;false;POS");
        assert_eq!(m.tags.len(), 1);
        // The dialog starts from the newest tag's group, colour and icon.
        m.open_tag_dialog(201);
        let dialog = m.tag_dialog.as_mut().unwrap();
        assert_eq!(dialog.group, "POS");
        assert_eq!(dialog.icon, "structure_skyhook");
        dialog.label = "Ice; belt".into();
        m.add_tag_from_dialog();
        assert!(m.tag_dialog.is_none());
        assert_eq!(
            m.tags[1].to_line(),
            "B1;Ice, belt;#F5FF83;structure_skyhook;false;POS"
        );
        m.set_tag_group_shown("POS", false);
        assert!(tags::by_system(&m.tags, &m.settings.hidden_tag_groups).is_empty());
        m.remove_tag(0);
        assert_eq!(m.tags.len(), 1);
    }

    #[test]
    fn an_unreadable_tag_file_is_never_overwritten() {
        let dir = std::env::temp_dir().join(format!("athena-tags-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = tags::file_path(&dir);
        std::fs::write(&path, [0xff, 0xfe, 0x00]).unwrap();
        let universe = crate::static_data::tests::sample().universe;
        assert!(tags::load(&dir, &universe).is_err());

        let mut m = module();
        m.tags_writable = false;
        m.import_tag_text("A1-XYZ;POS");
        m.open_tag_dialog(101);
        assert!(m.tags.is_empty());
        assert!(m.tag_dialog.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn adding_a_tag_again_updates_it() {
        let mut m = module();
        m.import_tag_text("A1-XYZ;POS;;dot;false;POS");
        m.open_tag_dialog(101);
        let dialog = m.tag_dialog.as_mut().unwrap();
        dialog.label = "pos".into();
        dialog.color = egui::Color32::from_rgb(1, 2, 3);
        m.add_tag_from_dialog();
        assert_eq!(m.tags.len(), 1);
        assert_eq!(m.tags[0].to_line(), "A1-XYZ;pos;#010203;dot;false;POS");
    }

    #[test]
    fn mark_clear_turns_the_system_green() {
        let mut m = module();
        m.handle_lines(
            "Branch Intel",
            "Pilot Alpha",
            vec![line(30, "Scout", "A2-XYZ")],
            false,
        );
        m.mark_clear(102);
        let refs: Vec<&Report> = m.reports.iter().collect();
        let marks = alerts::system_marks(&refs, now(), 600, &HashMap::new(), &HashMap::new());
        assert_eq!(marks[&102].color, alerts::GREEN);
    }

    #[test]
    fn mute_toggles_and_clear_words_are_cleaned_up() {
        let mut m = module();
        assert!(!m.is_muted());
        m.toggle_mute();
        assert!(m.is_muted());
        m.toggle_mute();
        assert!(!m.is_muted());
        m.set_clear_words("Clr,  CLEAR  gone");
        assert_eq!(m.settings.clear_words, ["clr", "clear", "gone"]);
    }

    #[test]
    fn silence_ends_the_mute_and_stays_until_enabled() {
        let mut m = module();
        m.toggle_mute();
        m.toggle_silence();
        assert!(m.is_muted());
        assert!(m.muted_until.is_none());
        // Mute 5 min does nothing while silenced.
        m.toggle_mute();
        assert!(m.muted_until.is_none());
        assert!(m.is_muted());
        m.toggle_silence();
        assert!(!m.is_muted());
    }

    #[test]
    fn pilots_can_be_hidden_and_shown_together() {
        let mut m = module();
        let names = vec!["Pilot Alpha".to_string(), "Pilot Bravo".to_string()];
        m.set_pilots_hidden(&names, true);
        m.set_pilots_hidden(&names, true);
        assert_eq!(m.settings.hidden_pilots, names);
        m.set_pilots_hidden(&names[..1], false);
        assert_eq!(m.settings.hidden_pilots, ["Pilot Bravo"]);
        m.set_pilots_hidden(&names, false);
        assert!(m.settings.hidden_pilots.is_empty());
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
