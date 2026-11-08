//! Per-character mining state: what is in the ore hold, the current session,
//! and which alerts are due.
//!
//! [`CharacterTracker`] is fed the events from [`super::gamelog`] for one
//! character, plus a regular "what time is it now" tick for the alerts that
//! depend on time passing (idle, miner stopped, session end). It does no I/O
//! and knows nothing about the UI.
//!
//! All times are EVE time, which is UTC, like the log itself.

use std::collections::{BTreeMap, VecDeque};

use super::gamelog::{LogTime, MiningEvent, TimedMiningEvent};

/// Settings that control alerts and sessions for one character.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackerSettings {
    /// Size of the ore hold in m³ (from Show Info in game, with skills).
    /// `None` until the user has entered it; fill alerts are off until then.
    pub hold_capacity_m3: Option<f64>,
    /// Alert when the hold is at least this full, in percent (e.g. 90.0).
    pub fill_alert_percent: f64,
    /// Alert when the hold is at least this full, in percent, as "full".
    /// Slightly under 100 because the last cycle rarely fits exactly.
    pub full_percent: f64,
    /// The shortest wait before "miner stopped". The real wait is worked
    /// out from the character's own cycle rhythm (see
    /// [`CharacterTracker::miner_stopped_after_secs`]), because a strip miner
    /// or ice harvester cycles far slower than a mining laser.
    pub miner_stopped_min_secs: i64,
    /// "Idle" alert after this many seconds without a cycle.
    pub idle_after_secs: i64,
    /// A session ends after this many seconds without a cycle.
    pub session_gap_secs: i64,
}

impl Default for TrackerSettings {
    fn default() -> Self {
        TrackerSettings {
            hold_capacity_m3: None,
            fill_alert_percent: 90.0,
            full_percent: 99.0,
            miner_stopped_min_secs: 60,
            idle_after_secs: 5 * 60,
            session_gap_secs: 15 * 60,
        }
    }
}

/// Looks up the volume of one unit of an item by its name.
///
/// Later this is filled from the SDE. It is a plain map so tests can build
/// one by hand.
#[derive(Debug, Clone, Default)]
pub struct VolumeTable {
    volume_by_name: BTreeMap<String, f64>,
}

impl VolumeTable {
    pub fn insert(&mut self, name: &str, unit_volume_m3: f64) {
        self.volume_by_name.insert(name.to_string(), unit_volume_m3);
    }

    pub fn get(&self, name: &str) -> Option<f64> {
        self.volume_by_name.get(name).copied()
    }
}

/// How full the ore hold is, split by compressed and uncompressed ore.
#[derive(Debug, Clone, PartialEq)]
pub struct HoldFill {
    pub compressed_m3: f64,
    pub uncompressed_m3: f64,
    /// Item names we have no volume for (not counted above). Should stay
    /// empty once the SDE is loaded; shown so a gap doesn't go unnoticed.
    pub unknown_items: Vec<String>,
}

impl HoldFill {
    pub fn total_m3(&self) -> f64 {
        self.compressed_m3 + self.uncompressed_m3
    }
}

/// True for compressed ore, ice and gas. EVE names them all "Compressed ...".
pub fn is_compressed(name: &str) -> bool {
    name.starts_with("Compressed ")
}

/// Something the user should be told about.
#[derive(Debug, Clone, PartialEq)]
pub enum Alert {
    /// The hold reached the user's threshold.
    HoldAboveThreshold { percent: f64 },
    /// The hold is (practically) full.
    HoldFull,
    /// A mining module switched off, or no cycle finished for a while.
    MinerStopped { reason: String },
    /// No cycle for the configured idle time.
    Idle { minutes: i64 },
}

/// A mining session that has ended. The caller values it with the current
/// prices and stores it; the value is never recalculated later.
#[derive(Debug, Clone, PartialEq)]
pub struct FinishedSession {
    pub character: String,
    pub started: LogTime,
    /// Time of the last mining cycle.
    pub ended: LogTime,
    /// Units mined per ore name (critical bonuses included).
    pub mined_units: BTreeMap<String, u64>,
    pub cycles: u64,
}

/// The session that is running now.
#[derive(Debug, Clone, PartialEq)]
pub struct ActiveSession {
    pub started: LogTime,
    pub last_cycle: LogTime,
    pub mined_units: BTreeMap<String, u64>,
    pub cycles: u64,
}

/// All mining state for one character.
#[derive(Debug, Clone)]
pub struct CharacterTracker {
    pub character: String,
    pub settings: TrackerSettings,
    /// Units in the ore hold per item name, as far as the log tells us.
    /// Unloading at a station writes no log line, so the user resets this
    /// with [`CharacterTracker::mark_emptied`].
    hold_units: BTreeMap<String, u64>,
    session: Option<ActiveSession>,
    /// Times of the most recent cycles (distinct seconds, oldest first), to
    /// learn how often this character's miners normally finish a cycle.
    recent_cycles: VecDeque<LogTime>,
    /// Alerts already raised, so each one fires once instead of every tick.
    /// They re-arm when the situation changes (hold emptied, mining resumes).
    raised_fill: bool,
    raised_full: bool,
    raised_stopped: bool,
    raised_idle: bool,
}

impl CharacterTracker {
    pub fn new(character: &str, settings: TrackerSettings) -> CharacterTracker {
        CharacterTracker {
            character: character.to_string(),
            settings,
            hold_units: BTreeMap::new(),
            session: None,
            recent_cycles: VecDeque::new(),
            raised_fill: false,
            raised_full: false,
            raised_stopped: false,
            raised_idle: false,
        }
    }

    /// Units in the hold per item name.
    pub fn hold_units(&self) -> &BTreeMap<String, u64> {
        &self.hold_units
    }

    pub fn session(&self) -> Option<&ActiveSession> {
        self.session.as_ref()
    }

    /// How many seconds without a cycle count as "miner stopped".
    ///
    /// Twice the longest gap between the last few cycles, but at least
    /// `miner_stopped_min_secs`. With several modules the gaps are short;
    /// with one slow strip miner they are long, and the limit follows.
    /// `None` until enough cycles were seen to tell; until then only the
    /// "deactivates" log line and the idle alert apply.
    pub fn miner_stopped_after_secs(&self) -> Option<i64> {
        const MIN_GAPS: usize = 3;
        if self.recent_cycles.len() < MIN_GAPS + 1 {
            return None;
        }
        let longest_gap = self
            .recent_cycles
            .iter()
            .zip(self.recent_cycles.iter().skip(1))
            .map(|(earlier, later)| later.seconds_since(*earlier))
            .max()?;
        Some((longest_gap * 2).max(self.settings.miner_stopped_min_secs))
    }

    /// Remembers a cycle time for [`Self::miner_stopped_after_secs`].
    fn record_cycle_time(&mut self, time: LogTime) {
        const KEEP: usize = 9;
        // Several modules finishing in the same second count as one.
        if self.recent_cycles.back() == Some(&time) {
            return;
        }
        self.recent_cycles.push_back(time);
        while self.recent_cycles.len() > KEEP {
            self.recent_cycles.pop_front();
        }
    }

    /// The user unloaded the hold at a station.
    pub fn mark_emptied(&mut self) {
        self.hold_units.clear();
        self.raised_fill = false;
        self.raised_full = false;
    }

    /// Works out how full the hold is.
    pub fn hold_fill(&self, volumes: &VolumeTable) -> HoldFill {
        let mut fill = HoldFill {
            compressed_m3: 0.0,
            uncompressed_m3: 0.0,
            unknown_items: Vec::new(),
        };
        for (name, &units) in &self.hold_units {
            let Some(unit_volume) = volumes.get(name) else {
                fill.unknown_items.push(name.clone());
                continue;
            };
            let m3 = unit_volume * units as f64;
            if is_compressed(name) {
                fill.compressed_m3 += m3;
            } else {
                fill.uncompressed_m3 += m3;
            }
        }
        fill
    }

    /// Hold fill in percent of capacity, if the capacity is known.
    pub fn fill_percent(&self, volumes: &VolumeTable) -> Option<f64> {
        let capacity = self.settings.hold_capacity_m3?;
        if capacity <= 0.0 {
            return None;
        }
        Some(self.hold_fill(volumes).total_m3() / capacity * 100.0)
    }

    /// Applies one event from the log. Returns any alerts it caused, and the
    /// previous session if this event came after a long enough gap to start
    /// a new one.
    pub fn apply(
        &mut self,
        event: &TimedMiningEvent,
        volumes: &VolumeTable,
    ) -> (Vec<Alert>, Option<FinishedSession>) {
        let mut alerts = Vec::new();
        let mut finished = None;

        match &event.event {
            MiningEvent::Mined {
                units,
                ore,
                critical,
            } => {
                // A cycle after a long gap: the old session is over.
                finished = self.end_session_if_gap(event.time);

                let session = self.session.get_or_insert_with(|| ActiveSession {
                    started: event.time,
                    last_cycle: event.time,
                    mined_units: BTreeMap::new(),
                    cycles: 0,
                });
                session.last_cycle = event.time;
                *session.mined_units.entry(ore.clone()).or_insert(0) += units;
                // A critical success is a bonus on a cycle that already has
                // its own "You mined" line, so it is not a cycle of its own.
                if !critical {
                    session.cycles += 1;
                    self.record_cycle_time(event.time);
                }

                *self.hold_units.entry(ore.clone()).or_insert(0) += units;

                // Mining again re-arms the time based alerts.
                self.raised_stopped = false;
                self.raised_idle = false;
            }
            MiningEvent::Compressed { from, to, units } => {
                // Compression is 1:1 in units. If the log says more was
                // compressed than we saw mined (ore that came from elsewhere),
                // the uncompressed count just drops to zero.
                if let Some(have) = self.hold_units.get_mut(from) {
                    *have = have.saturating_sub(*units);
                    if *have == 0 {
                        self.hold_units.remove(from);
                    }
                }
                *self.hold_units.entry(to.clone()).or_insert(0) += units;
            }
            MiningEvent::MinerStopped { reason, .. } => {
                if !self.raised_stopped {
                    self.raised_stopped = true;
                    alerts.push(Alert::MinerStopped {
                        reason: reason.clone(),
                    });
                }
            }
            // Residue never reaches the hold.
            MiningEvent::Residue { .. } => {}
        }

        alerts.extend(self.fill_alerts(volumes));
        (alerts, finished)
    }

    /// Called regularly (e.g. every few seconds) with the current time, for
    /// the alerts that come from nothing happening.
    ///
    /// `now` must be EVE time (UTC), not the PC's local time, since the log
    /// times it is compared with are UTC.
    pub fn tick(&mut self, now: LogTime) -> (Vec<Alert>, Option<FinishedSession>) {
        let mut alerts = Vec::new();
        let Some(session) = &self.session else {
            return (alerts, None);
        };
        let quiet_secs = now.seconds_since(session.last_cycle);

        let stopped_after = self.miner_stopped_after_secs();
        let stopped = stopped_after.is_some_and(|limit| quiet_secs >= limit);
        if stopped && !self.raised_stopped {
            self.raised_stopped = true;
            alerts.push(Alert::MinerStopped {
                reason: format!("no mining cycle for {quiet_secs} seconds"),
            });
        }
        if quiet_secs >= self.settings.idle_after_secs && !self.raised_idle {
            self.raised_idle = true;
            alerts.push(Alert::Idle {
                minutes: quiet_secs / 60,
            });
        }
        let finished = self.end_session_if_gap(now);
        (alerts, finished)
    }

    /// Ends the running session if nothing was mined for `session_gap_secs`
    /// before `now`.
    fn end_session_if_gap(&mut self, now: LogTime) -> Option<FinishedSession> {
        let session = self.session.as_ref()?;
        if now.seconds_since(session.last_cycle) < self.settings.session_gap_secs {
            return None;
        }
        // `take` moves the session out and leaves `None` behind.
        let session = self.session.take()?;
        // The gap to the next session says nothing about cycle times.
        self.recent_cycles.clear();
        Some(FinishedSession {
            character: self.character.clone(),
            started: session.started,
            ended: session.last_cycle,
            mined_units: session.mined_units,
            cycles: session.cycles,
        })
    }

    /// Fill alerts. Each fires once until the hold drops below the level
    /// again (after compressing or emptying).
    fn fill_alerts(&mut self, volumes: &VolumeTable) -> Vec<Alert> {
        let mut alerts = Vec::new();
        let Some(percent) = self.fill_percent(volumes) else {
            return alerts;
        };

        if percent >= self.settings.full_percent {
            if !self.raised_full {
                self.raised_full = true;
                // "Full" covers "above threshold" too; don't send both.
                self.raised_fill = true;
                alerts.push(Alert::HoldFull);
            }
        } else {
            self.raised_full = false;
            if percent >= self.settings.fill_alert_percent {
                if !self.raised_fill {
                    self.raised_fill = true;
                    alerts.push(Alert::HoldAboveThreshold { percent });
                }
            } else {
                self.raised_fill = false;
            }
        }
        alerts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: i64) -> LogTime {
        // 2026-11-08 00:00:00 UTC plus `secs`.
        LogTime {
            unix_seconds: 1_794_096_000 + secs,
        }
    }

    fn mined(secs: i64, units: u64, ore: &str) -> TimedMiningEvent {
        TimedMiningEvent {
            time: at(secs),
            event: MiningEvent::Mined {
                units,
                ore: ore.to_string(),
                critical: false,
            },
        }
    }

    fn compressed(secs: i64, units: u64, from: &str) -> TimedMiningEvent {
        TimedMiningEvent {
            time: at(secs),
            event: MiningEvent::Compressed {
                from: from.to_string(),
                to: format!("Compressed {from}"),
                units,
            },
        }
    }

    fn volumes() -> VolumeTable {
        // Made-up round numbers, not the real SDE values.
        let mut v = VolumeTable::default();
        v.insert("Rock", 10.0);
        v.insert("Compressed Rock", 0.1);
        v
    }

    fn tracker(capacity: f64) -> CharacterTracker {
        let settings = TrackerSettings {
            hold_capacity_m3: Some(capacity),
            ..TrackerSettings::default()
        };
        CharacterTracker::new("Test Pilot", settings)
    }

    #[test]
    fn mining_fills_hold_and_compression_shrinks_it() {
        let v = volumes();
        let mut t = tracker(10_000.0);
        t.apply(&mined(0, 300, "Rock"), &v);
        let fill = t.hold_fill(&v);
        assert_eq!(fill.uncompressed_m3, 3_000.0);
        assert_eq!(fill.compressed_m3, 0.0);

        t.apply(&compressed(10, 300, "Rock"), &v);
        let fill = t.hold_fill(&v);
        assert_eq!(fill.uncompressed_m3, 0.0);
        assert!((fill.compressed_m3 - 30.0).abs() < 1e-9);
        assert!(t.hold_units().get("Rock").is_none());
    }

    #[test]
    fn compressing_more_than_seen_does_not_underflow() {
        let v = volumes();
        let mut t = tracker(10_000.0);
        t.apply(&mined(0, 100, "Rock"), &v);
        t.apply(&compressed(5, 500, "Rock"), &v);
        assert_eq!(t.hold_units().get("Rock"), None);
        assert_eq!(t.hold_units().get("Compressed Rock"), Some(&500));
    }

    #[test]
    fn unknown_items_are_reported() {
        let v = volumes();
        let mut t = tracker(10_000.0);
        t.apply(&mined(0, 10, "Mystery Ore"), &v);
        assert_eq!(t.hold_fill(&v).unknown_items, vec!["Mystery Ore"]);
    }

    #[test]
    fn threshold_and_full_alerts_fire_once() {
        let v = volumes();
        let mut t = tracker(1_000.0);
        // 80% -> nothing
        let (alerts, _) = t.apply(&mined(0, 80, "Rock"), &v);
        assert!(alerts.is_empty());
        // 92% -> threshold alert
        let (alerts, _) = t.apply(&mined(10, 12, "Rock"), &v);
        assert!(matches!(alerts[..], [Alert::HoldAboveThreshold { .. }]));
        // 95% -> no repeat
        let (alerts, _) = t.apply(&mined(20, 3, "Rock"), &v);
        assert!(alerts.is_empty());
        // 100% -> full
        let (alerts, _) = t.apply(&mined(30, 5, "Rock"), &v);
        assert_eq!(alerts, vec![Alert::HoldFull]);
        let (alerts, _) = t.apply(&mined(40, 0, "Rock"), &v);
        assert!(alerts.is_empty());
        // Emptied re-arms both.
        t.mark_emptied();
        let (alerts, _) = t.apply(&mined(50, 100, "Rock"), &v);
        assert_eq!(alerts, vec![Alert::HoldFull]);
    }

    #[test]
    fn compressing_rearms_fill_alert() {
        let v = volumes();
        let mut t = tracker(1_000.0);
        let (alerts, _) = t.apply(&mined(0, 95, "Rock"), &v);
        assert_eq!(alerts.len(), 1);
        t.apply(&compressed(5, 95, "Rock"), &v);
        let (alerts, _) = t.apply(&mined(10, 95, "Rock"), &v);
        assert!(matches!(alerts[..], [Alert::HoldAboveThreshold { .. }]));
    }

    #[test]
    fn no_fill_alerts_without_capacity() {
        let v = volumes();
        let mut t = CharacterTracker::new("Test Pilot", TrackerSettings::default());
        let (alerts, _) = t.apply(&mined(0, 1_000_000, "Rock"), &v);
        assert!(alerts.is_empty());
        assert_eq!(t.fill_percent(&v), None);
    }

    #[test]
    fn miner_stopped_from_log_line() {
        let v = volumes();
        let mut t = tracker(10_000.0);
        t.apply(&mined(0, 1, "Rock"), &v);
        let stop = TimedMiningEvent {
            time: at(5),
            event: MiningEvent::MinerStopped {
                module: "Strip Miner".to_string(),
                reason: "rock depleted".to_string(),
            },
        };
        let (alerts, _) = t.apply(&stop, &v);
        assert_eq!(
            alerts,
            vec![Alert::MinerStopped {
                reason: "rock depleted".to_string()
            }]
        );
        // A second miner stopping right after doesn't alert again...
        let (alerts, _) = t.apply(&stop, &v);
        assert!(alerts.is_empty());
        // ...until mining resumed.
        t.apply(&mined(60, 1, "Rock"), &v);
        let (alerts, _) = t.apply(&stop, &v);
        assert_eq!(alerts.len(), 1);
    }

    #[test]
    fn time_based_alerts_and_session_end() {
        let v = volumes();
        let mut t = tracker(10_000.0);
        // Four cycles 20 s apart: the stop limit becomes the 60 s minimum.
        for secs in [0, 20, 40, 60] {
            t.apply(&mined(secs, 10, "Rock"), &v);
        }
        assert_eq!(t.miner_stopped_after_secs(), Some(60));

        let (alerts, finished) = t.tick(at(100));
        assert!(alerts.is_empty());
        assert!(finished.is_none());

        // 60 s after the last cycle: miner stopped.
        let (alerts, _) = t.tick(at(120));
        assert!(matches!(alerts[..], [Alert::MinerStopped { .. }]));
        assert!(t.tick(at(121)).0.is_empty());

        // 5 min: idle.
        let (alerts, _) = t.tick(at(60 + 300));
        assert_eq!(alerts, vec![Alert::Idle { minutes: 5 }]);

        // 15 min: session over.
        let (_, finished) = t.tick(at(60 + 900));
        let finished = finished.unwrap();
        assert_eq!(finished.started, at(0));
        assert_eq!(finished.ended, at(60));
        assert_eq!(finished.cycles, 4);
        assert_eq!(finished.mined_units.get("Rock"), Some(&40));
        assert!(t.session().is_none());
        // The hold is not touched by a session ending.
        assert_eq!(t.hold_units().get("Rock"), Some(&40));
    }

    #[test]
    fn slow_cycles_do_not_look_stopped() {
        // One strip miner with a 180 s cycle.
        let v = volumes();
        let mut t = tracker(100_000.0);
        for i in 0..5 {
            t.apply(&mined(i * 180, 10, "Rock"), &v);
        }
        assert_eq!(t.miner_stopped_after_secs(), Some(360));
        // Between two normal cycles: no alert.
        assert!(t.tick(at(4 * 180 + 179)).0.is_empty());
        // Two whole cycles missed: stopped.
        let (alerts, _) = t.tick(at(4 * 180 + 360));
        assert!(
            alerts
                .iter()
                .any(|a| matches!(a, Alert::MinerStopped { .. }))
        );
    }

    #[test]
    fn no_time_based_stop_before_rhythm_is_known() {
        let v = volumes();
        let mut t = tracker(10_000.0);
        t.apply(&mined(0, 10, "Rock"), &v);
        t.apply(&mined(30, 10, "Rock"), &v);
        assert_eq!(t.miner_stopped_after_secs(), None);
        assert!(t.tick(at(200)).0.is_empty());
    }

    #[test]
    fn modules_in_the_same_second_count_once_for_rhythm() {
        let v = volumes();
        let mut t = tracker(10_000.0);
        for secs in [0, 0, 100, 100, 200, 200, 300, 300] {
            t.apply(&mined(secs, 10, "Rock"), &v);
        }
        assert_eq!(t.miner_stopped_after_secs(), Some(200));
        assert_eq!(t.session().unwrap().cycles, 8);
    }

    #[test]
    fn critical_success_adds_units_but_not_a_cycle() {
        let v = volumes();
        let mut t = tracker(10_000.0);
        t.apply(&mined(0, 100, "Rock"), &v);
        let crit = TimedMiningEvent {
            time: at(0),
            event: MiningEvent::Mined {
                units: 20,
                ore: "Rock".to_string(),
                critical: true,
            },
        };
        t.apply(&crit, &v);
        let session = t.session().unwrap();
        assert_eq!(session.cycles, 1);
        assert_eq!(session.mined_units.get("Rock"), Some(&120));
        assert_eq!(t.hold_units().get("Rock"), Some(&120));
    }

    #[test]
    fn late_cycle_after_gap_starts_new_session() {
        // E.g. Athena was closed, so no tick ended the session in time.
        let v = volumes();
        let mut t = tracker(10_000.0);
        t.apply(&mined(0, 10, "Rock"), &v);
        let (_, finished) = t.apply(&mined(3_600, 5, "Rock"), &v);
        assert_eq!(finished.unwrap().mined_units.get("Rock"), Some(&10));
        let session = t.session().unwrap();
        assert_eq!(session.started, at(3_600));
        assert_eq!(session.mined_units.get("Rock"), Some(&5));
    }
}
