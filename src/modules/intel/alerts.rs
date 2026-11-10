//! What reports mean for the map and for alerts, as plain functions:
//!
//! - [`system_marks`]: how each reported system is drawn. A report turns
//!   its system red, fading through orange to grey over the fade time
//!   (10 minutes by default); a clear word turns it green. The colour comes
//!   from the report's age whenever a frame is drawn, so nothing animates.
//! - [`alert_for`]: whether a new report is close enough to an own pilot to
//!   sound an alert, and how loud.

use std::collections::HashMap;

use eframe::egui::Color32;

use super::Report;
use super::lookup::{self, Character};
use crate::modules::mining::gamelog::LogTime;

// Colours from the mockup.
pub const RED: Color32 = Color32::from_rgb(0xe5, 0x53, 0x4b);
pub const ORANGE: Color32 = Color32::from_rgb(0xd9, 0x88, 0x3a);
pub const FADED: Color32 = Color32::from_rgb(0x8a, 0x6a, 0x5a);
pub const GREEN: Color32 = Color32::from_rgb(0x5c, 0xbf, 0x6a);
pub const OLD_GREY: Color32 = Color32::from_rgb(0x5a, 0x5a, 0x5a);

/// How a reported system is drawn on the map.
#[derive(Debug, Clone, PartialEq)]
pub struct SystemMark {
    pub color: Color32,
    /// "1j · now", "4j · 3m", or "clr".
    pub badge: String,
    /// Gate lines touching the system are drawn in its colour. True while
    /// it isn't cleared.
    pub hot: bool,
}

/// The colour of a report `fraction` of the way through its fade time:
/// red, then orange halfway, then the faded brown-grey.
pub fn fade_color(fraction: f32) -> Color32 {
    let t = fraction.clamp(0.0, 1.0);
    if t < 0.5 {
        mix(RED, ORANGE, t * 2.0)
    } else {
        mix(ORANGE, FADED, (t - 0.5) * 2.0)
    }
}

/// `a` blended towards `b` by `t` (0 = a, 1 = b).
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let channel = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(
        channel(a.r(), b.r()),
        channel(a.g(), b.g()),
        channel(a.b(), b.b()),
    )
}

/// "now" under a minute, then "3m".
pub fn age_text(seconds: i64) -> String {
    if seconds < 60 {
        "now".to_string()
    } else {
        format!("{}m", seconds / 60)
    }
}

/// "1 jump", "4 jumps", "in system".
pub fn jumps_text(jumps: u32) -> String {
    match jumps {
        0 => "in system".to_string(),
        1 => "1 jump".to_string(),
        n => format!("{n} jumps"),
    }
}

/// The fewest jumps from an own pilot to any system the report names.
pub fn report_jumps(report: &Report, distances: &HashMap<i64, u32>) -> Option<u32> {
    report
        .message
        .systems
        .iter()
        .filter_map(|id| distances.get(id))
        .min()
        .copied()
}

/// How far through its fade a report is: 0 when new, 1 (or more) when it
/// has faded out.
pub fn fade_fraction(report: &Report, now: LogTime, fade_seconds: i64) -> f32 {
    let age = now.seconds_since(report.time).max(0);
    age as f32 / fade_seconds.max(1) as f32
}

/// How every system named in a recent report is drawn. Reports are newest
/// first, and only the newest report about a system counts, so "clr" after
/// a sighting turns the system green and a new sighting after "clr" turns
/// it red again.
///
/// A pilot is only where they were last reported: a sighting whose pilots
/// (the names ESI confirmed, see [`lookup::pilots_to_show`]) were all
/// reported again later in another system doesn't light its system any
/// more. A sighting without confirmed pilots always counts.
pub fn system_marks(
    reports: &[&Report],
    now: LogTime,
    fade_seconds: i64,
    distances: &HashMap<i64, u32>,
    known: &HashMap<String, Option<Character>>,
) -> HashMap<i64, SystemMark> {
    let mut marks = HashMap::new();
    // Each pilot's systems in the newest sighting naming them (by
    // character id).
    let mut last_seen: HashMap<i64, &[i64]> = HashMap::new();
    for report in reports {
        let fraction = fade_fraction(report, now, fade_seconds);
        if fraction >= 1.0 {
            // Older ones are older still.
            break;
        }
        if !report.message.clear {
            let pilots = lookup::pilots_to_show(&report.message.pilot_candidates, known);
            let systems = report.message.systems.as_slice();
            let moved_on = |id: &i64| {
                last_seen
                    .get(id)
                    .is_some_and(|newer| !newer.iter().any(|s| systems.contains(s)))
            };
            let all_moved_on = !pilots.is_empty() && pilots.iter().all(|(_, id)| moved_on(id));
            for (_, id) in &pilots {
                // Only sightings that name a system say where a pilot is.
                if !systems.is_empty() {
                    last_seen.entry(*id).or_insert(systems);
                }
            }
            if all_moved_on {
                continue;
            }
        }
        for &system in &report.message.systems {
            if marks.contains_key(&system) {
                continue;
            }
            let mark = if report.message.clear {
                SystemMark {
                    color: GREEN,
                    badge: "clr".to_string(),
                    hot: false,
                }
            } else {
                let age = age_text(now.seconds_since(report.time).max(0));
                let badge = match distances.get(&system) {
                    Some(jumps) => format!("{jumps}j · {age}"),
                    None => age,
                };
                SystemMark {
                    color: fade_color(fraction),
                    badge,
                    hot: true,
                }
            };
            marks.insert(system, mark);
        }
    }
    marks
}

/// How loud an alert is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlertLevel {
    /// The fewest jumps from an own pilot.
    pub jumps: u32,
    /// Within the loud range: full volume, played twice.
    pub loud: bool,
}

/// Whether a new report should raise an alert: it names a system within
/// `alert_jumps` of an own pilot and isn't a "clear" message.
pub fn alert_for(
    report: &Report,
    distances: &HashMap<i64, u32>,
    alert_jumps: u32,
    loud_jumps: u32,
) -> Option<AlertLevel> {
    if report.message.clear {
        return None;
    }
    let jumps = report_jumps(report, distances)?;
    (jumps <= alert_jumps).then_some(AlertLevel {
        jumps,
        loud: jumps <= loud_jumps,
    })
}

#[cfg(test)]
mod tests {
    use super::super::parser::IntelMessage;
    use super::*;

    fn at(seconds: i64) -> LogTime {
        LogTime {
            unix_seconds: 1_800_000_000 + seconds,
        }
    }

    fn report(seconds: i64, systems: &[i64], clear: bool) -> Report {
        Report {
            time: at(seconds),
            channel: "Branch Intel".into(),
            sender: "Scout".into(),
            text: String::new(),
            message: IntelMessage {
                systems: systems.to_vec(),
                clear,
                ..IntelMessage::default()
            },
        }
    }

    #[test]
    fn fade_goes_red_orange_faded() {
        assert_eq!(fade_color(0.0), RED);
        assert_eq!(fade_color(0.5), ORANGE);
        assert_eq!(fade_color(1.0), FADED);
        assert_eq!(fade_color(2.0), FADED);
    }

    #[test]
    fn newest_report_per_system_wins() {
        let distances: HashMap<i64, u32> = [(1, 1), (2, 4)].into_iter().collect();
        // Newest first: system 1 cleared after a sighting; system 2 seen 2m ago.
        let reports = [
            report(-30, &[1], true),
            report(-120, &[1, 2], false),
            report(-3600, &[3], false),
        ];
        let refs: Vec<&Report> = reports.iter().collect();
        let marks = system_marks(&refs, at(0), 600, &distances, &HashMap::new());
        assert_eq!(marks[&1].color, GREEN);
        assert_eq!(marks[&1].badge, "clr");
        assert!(!marks[&1].hot);
        assert_eq!(marks[&2].badge, "4j · 2m");
        assert!(marks[&2].hot);
        // An hour old: faded out.
        assert!(!marks.contains_key(&3));
    }

    /// A sighting naming pilots, as typed.
    fn sighting(seconds: i64, systems: &[i64], pilots: &[&str]) -> Report {
        let mut report = report(seconds, systems, false);
        report.message.pilot_candidates = pilots.iter().map(|p| p.to_string()).collect();
        report
    }

    #[test]
    fn a_pilot_reported_elsewhere_leaves_the_old_system() {
        let known: HashMap<String, Option<Character>> =
            [("evil pilot", 11, "Evil Pilot"), ("bad guy", 12, "Bad Guy")]
                .into_iter()
                .map(|(key, id, name)| {
                    let character = Character {
                        id,
                        name: name.to_string(),
                    };
                    (key.to_string(), Some(character))
                })
                .collect();
        // Newest first.
        let reports = [
            // Evil Pilot moved from 1 to 2.
            sighting(-10, &[2], &["Evil Pilot"]),
            // Both were in 1: Bad Guy wasn't seen anywhere else, so 1 stays.
            sighting(-60, &[1], &["Evil Pilot", "Bad Guy"]),
            // Only Evil Pilot was in 3: it goes dark.
            sighting(-120, &[3], &["Evil Pilot"]),
            // Seen again in 2 earlier: still 2, stays lit.
            sighting(-150, &[2], &["Evil Pilot"]),
            // No confirmed pilot ("gate" isn't one): always lit.
            sighting(-200, &[4], &["gate"]),
        ];
        let refs: Vec<&Report> = reports.iter().collect();
        let marks = system_marks(&refs, at(0), 600, &HashMap::new(), &known);
        assert!(marks.contains_key(&1));
        assert!(marks[&2].badge.starts_with("now"));
        assert!(!marks.contains_key(&3));
        assert!(marks.contains_key(&4));
    }

    #[test]
    fn new_report_is_red_with_now() {
        let reports = [report(-5, &[7], false)];
        let refs: Vec<&Report> = reports.iter().collect();
        let marks = system_marks(&refs, at(0), 600, &HashMap::new(), &HashMap::new());
        assert_eq!(marks[&7].badge, "now");
        assert_eq!(marks[&7].color, fade_color(5.0 / 600.0));
    }

    #[test]
    fn alerts_within_range_and_loud_close_by() {
        let distances: HashMap<i64, u32> = [(1, 1), (2, 4), (3, 7)].into_iter().collect();
        let level =
            |systems: &[i64], clear| alert_for(&report(0, systems, clear), &distances, 5, 2);
        assert_eq!(
            level(&[1], false),
            Some(AlertLevel {
                jumps: 1,
                loud: true
            })
        );
        assert_eq!(
            level(&[2], false),
            Some(AlertLevel {
                jumps: 4,
                loud: false
            })
        );
        // The nearest named system counts.
        assert_eq!(level(&[3, 2], false).unwrap().jumps, 4);
        assert_eq!(level(&[3], false), None);
        assert_eq!(level(&[1], true), None);
        assert_eq!(level(&[99], false), None);
    }

    #[test]
    fn texts() {
        assert_eq!(age_text(59), "now");
        assert_eq!(age_text(125), "2m");
        assert_eq!(jumps_text(0), "in system");
        assert_eq!(jumps_text(1), "1 jump");
        assert_eq!(jumps_text(6), "6 jumps");
    }
}
