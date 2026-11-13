//! Belt scans: the result of the Mining Survey Scanner, copied from EVE.
//! Athena watches the clipboard and picks up anything that is a valid scan,
//! so the user only has to copy it in EVE.
//!
//! EVE copies the scan as text, one rock per line, with tab-separated
//! columns:
//!
//! ```text
//! Omber III-Grade<TAB>833,320<TAB>499,992 m3<TAB>76,800,000.00 ISK<TAB>96 km
//! ```
//!
//! Lines with only an ore name are group headers and are skipped. The ISK
//! column is ignored: it is the raw ore price, while Athena values ore by
//! what it reprocesses into. It is "-" for ore without a market price.
//!
//! Two scans of the same belt taken some time apart give the rate at which
//! the belt is being mined by everyone on it, and from that an estimate of
//! when it will be empty. The rate is rolling: each new scan is compared
//! with the one before it, so it follows miners arriving and leaving.

use super::gamelog::LogTime;

/// One asteroid from a scan.
#[derive(Debug, Clone, PartialEq)]
pub struct Rock {
    pub ore: String,
    pub units: u64,
    pub volume_m3: f64,
    /// Distance from the ship when the scan was taken, in metres.
    pub distance_m: f64,
}

/// A parsed scan and when it was pasted.
#[derive(Debug, Clone, PartialEq)]
pub struct BeltScan {
    pub taken: LogTime,
    pub rocks: Vec<Rock>,
}

impl BeltScan {
    /// Parses clipboard text as a scan.
    ///
    /// Because this runs on whatever the user copies, it is strict: every
    /// non-empty line must be a rock or a group header naming an ore that
    /// also appears as a rock. Anything else (a chat line, a fitting, a
    /// web page) gives `None`, and the clipboard is left alone.
    pub fn parse(text: &str, taken: LogTime) -> Option<BeltScan> {
        let mut rocks = Vec::new();
        let mut headers = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if line.contains('\t') {
                rocks.push(parse_rock(line)?);
            } else {
                headers.push(line);
            }
        }
        if rocks.is_empty() {
            return None;
        }
        let headers_match = headers
            .iter()
            .all(|header| rocks.iter().any(|rock| rock.ore == *header));
        if !headers_match {
            return None;
        }
        Some(BeltScan { taken, rocks })
    }

    pub fn total_m3(&self) -> f64 {
        self.rocks.iter().map(|rock| rock.volume_m3).sum()
    }

    pub fn rock_count(&self) -> usize {
        self.rocks.len()
    }

    /// Rocks, units and m³ per ore family (all grades together), sorted by
    /// m³, largest first. "Mordunium III-Grade" belongs to "Mordunium".
    pub fn by_family(&self) -> Vec<FamilyTotal> {
        let mut totals: Vec<FamilyTotal> = Vec::new();
        for rock in &self.rocks {
            let family = ore_family(&rock.ore);
            match totals.iter_mut().find(|t| t.family == family) {
                Some(total) => {
                    total.rocks += 1;
                    total.volume_m3 += rock.volume_m3;
                }
                None => totals.push(FamilyTotal {
                    family: family.to_string(),
                    rocks: 1,
                    volume_m3: rock.volume_m3,
                }),
            }
        }
        // `total_cmp` sorts floats without needing them to be non-NaN.
        totals.sort_by(|a, b| b.volume_m3.total_cmp(&a.volume_m3));
        totals
    }
}

/// Totals for one ore family in a scan.
#[derive(Debug, Clone, PartialEq)]
pub struct FamilyTotal {
    pub family: String,
    pub rocks: usize,
    pub volume_m3: f64,
}

/// What two scans of the same belt say about it.
#[derive(Debug, Clone, PartialEq)]
pub struct BeltProgress {
    pub m3_left: f64,
    pub rocks_left: usize,
    /// m³ mined between the two scans (by everyone).
    pub m3_mined: f64,
    pub rocks_gone: usize,
    /// m³ per hour, if the scans are far enough apart to tell.
    pub m3_per_hour: Option<f64>,
    /// Seconds until the belt is empty at that rate.
    pub seconds_to_clear: Option<i64>,
}

/// Scans closer together than this give no rate: the belt barely changed,
/// so the number would be mostly noise.
const MIN_SECONDS: i64 = 60;

/// Most scans kept per belt. Only scans less than `MIN_SECONDS` apart pile
/// up, so this is just a safety cap.
const MAX_KEPT: usize = 20;

/// The recent scans of the current belt, for a rolling rate.
///
/// The rate compares the latest scan with the newest earlier scan that is at
/// least `MIN_SECONDS` older. So it always covers the shortest useful window,
/// a quick double copy doesn't wipe it, and copying every 30–50 s still rolls
/// forward instead of growing into an average over the whole belt.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BeltScans {
    /// Oldest first. The first entry is the comparison scan once one exists;
    /// everything older than that has been dropped.
    recent: Vec<BeltScan>,
    /// Total m³ of every scan of this belt, oldest first, for the depletion
    /// bar. The first one is the full bar. Kept apart from `recent`, which
    /// forgets old scans.
    totals_m3: Vec<f64>,
}

/// Most scan totals kept for the depletion bar's ticks. The first scan is
/// always kept.
const MAX_TOTALS: usize = 50;

/// What the belt scan's depletion bar shows. All shares are of the first
/// scan of the belt, from 0 to 1.
#[derive(Debug, Clone, PartialEq)]
pub struct BeltDepletion {
    /// m³ in the first scan: the full bar.
    pub first_m3: f64,
    /// What the latest scan still showed.
    pub scanned_share: f64,
    /// What is probably left now, after mining at the last belt rate since
    /// the latest scan. Never more than `scanned_share`.
    pub estimated_share: f64,
    pub estimated_m3: f64,
    /// Whether there is a belt rate. Without one there is no estimate and
    /// `estimated_share` equals `scanned_share`.
    pub has_rate: bool,
    /// Where the scans between the first and the latest one were.
    pub earlier_shares: Vec<f64>,
}

impl BeltScans {
    /// Adds a new scan from the clipboard. A scan of a different belt
    /// starts over.
    pub fn add(&mut self, scan: BeltScan) {
        let same_belt = self
            .recent
            .last()
            .is_some_and(|newest| is_same_belt(newest, &scan));
        if !same_belt {
            self.recent.clear();
            self.totals_m3.clear();
        }
        self.totals_m3.push(scan.total_m3());
        if self.totals_m3.len() > MAX_TOTALS {
            // Drop the oldest tick, but never the first scan.
            self.totals_m3.remove(1);
        }
        self.recent.push(scan);

        // Drop every scan older than the comparison scan: they are no longer
        // needed for the rate.
        if let Some(anchor) = self.previous_index() {
            self.recent.drain(..anchor);
        }
        if self.recent.len() > MAX_KEPT {
            let extra = self.recent.len() - MAX_KEPT;
            self.recent.drain(..extra);
        }
    }

    /// The newest scan.
    pub fn latest(&self) -> Option<&BeltScan> {
        self.recent.last()
    }

    /// The scan the latest one is compared with: the newest one at least
    /// `MIN_SECONDS` older. `None` until there is one.
    pub fn previous(&self) -> Option<&BeltScan> {
        self.previous_index().map(|index| &self.recent[index])
    }

    fn previous_index(&self) -> Option<usize> {
        let latest = self.recent.last()?;
        self.recent
            .iter()
            .rposition(|scan| latest.taken.seconds_since(scan.taken) >= MIN_SECONDS)
    }

    /// Forgets all scans ("New belt").
    pub fn clear(&mut self) {
        self.recent.clear();
        self.totals_m3.clear();
    }

    /// The depletion bar at `now`, or `None` before the first scan.
    ///
    /// The estimate takes the rate between the latest scan and the one it
    /// is compared with, and assumes the belt kept being mined at that rate
    /// since the latest scan.
    pub fn depletion(&self, now: LogTime) -> Option<BeltDepletion> {
        let latest = self.latest()?;
        let first_m3 = *self.totals_m3.first()?;
        if first_m3 <= 0.0 {
            return None;
        }
        let share = |m3: f64| (m3 / first_m3).clamp(0.0, 1.0);
        let scanned_m3 = latest.total_m3();
        let rate = self
            .previous()
            .and_then(|previous| belt_progress(previous, latest).m3_per_hour);
        let estimated_m3 = match rate {
            Some(per_hour) => {
                let seconds = now.seconds_since(latest.taken).max(0) as f64;
                (scanned_m3 - per_hour / 3600.0 * seconds).max(0.0)
            }
            None => scanned_m3,
        };
        let earlier = match self.totals_m3.len() {
            0..=2 => &[][..],
            len => &self.totals_m3[1..len - 1],
        };
        Some(BeltDepletion {
            first_m3,
            scanned_share: share(scanned_m3),
            estimated_share: share(estimated_m3),
            estimated_m3,
            has_rate: rate.is_some(),
            earlier_shares: earlier.iter().map(|&m3| share(m3)).collect(),
        })
    }
}

/// Compares an earlier and a later scan of a belt.
///
/// The rate is only given when the scans are at least `MIN_SECONDS` apart
/// and the belt actually shrank; otherwise it is `None` ("paste another scan
/// later").
pub fn belt_progress(first: &BeltScan, latest: &BeltScan) -> BeltProgress {
    let m3_left = latest.total_m3();
    let m3_mined = (first.total_m3() - m3_left).max(0.0);
    let rocks_gone = first.rock_count().saturating_sub(latest.rock_count());
    let seconds = latest.taken.seconds_since(first.taken);

    let mut m3_per_hour = None;
    let mut seconds_to_clear = None;
    if seconds >= MIN_SECONDS && m3_mined > 0.0 {
        let per_second = m3_mined / seconds as f64;
        m3_per_hour = Some(per_second * 3600.0);
        seconds_to_clear = Some((m3_left / per_second).round() as i64);
    }
    BeltProgress {
        m3_left,
        rocks_left: latest.rock_count(),
        m3_mined,
        rocks_gone,
        m3_per_hour,
        seconds_to_clear,
    }
}

/// Whether `latest` can be a later scan of the same belt as `first`.
///
/// Scans arrive from the clipboard automatically, so a scan of a different
/// belt must not be compared with the old one (a smaller new belt would look
/// like a huge mining rate). A belt only shrinks and never gains ore types,
/// so a scan with more m³ or an ore family the first scan didn't have
/// starts a new belt.
pub fn is_same_belt(first: &BeltScan, latest: &BeltScan) -> bool {
    if latest.total_m3() > first.total_m3() {
        return false;
    }
    latest.rocks.iter().all(|rock| {
        let family = ore_family(&rock.ore);
        first.rocks.iter().any(|old| ore_family(&old.ore) == family)
    })
}

/// "Mordunium III-Grade" -> "Mordunium". Names without a grade stay as
/// they are.
pub fn ore_family(ore: &str) -> &str {
    match ore.rsplit_once(' ') {
        Some((family, grade)) if grade.ends_with("-Grade") => family,
        _ => ore,
    }
}

/// Parses one rock line, or returns `None` if it isn't one.
fn parse_rock(line: &str) -> Option<Rock> {
    let columns: Vec<&str> = line.split('\t').map(str::trim).collect();
    // Name, quantity, volume, ISK, distance.
    if columns.len() != 5 {
        return None;
    }
    if !columns[2].ends_with("m3") && !columns[2].ends_with("m³") {
        return None;
    }
    let ore = columns[0];
    if ore.is_empty() {
        return None;
    }
    let units = parse_whole_number(columns[1])?;
    // "499,992 m3"
    let volume_m3 = parse_number(columns[2].trim_end_matches("m3").trim_end_matches("m³"))?;
    // Column 3 is ISK, ignored. EVE shows "-" when the ore has no market
    // price. Column 4 is the distance.
    if !columns[3].ends_with("ISK") && columns[3] != "-" {
        return None;
    }
    let distance_m = parse_distance(columns[4])?;
    Some(Rock {
        ore: ore.to_string(),
        units,
        volume_m3,
        distance_m,
    })
}

/// "30 km" -> 30000.0, "1,507 m" -> 1507.0.
fn parse_distance(text: &str) -> Option<f64> {
    if let Some(km) = text.strip_suffix("km") {
        return Some(parse_number(km)? * 1000.0);
    }
    if let Some(m) = text.strip_suffix('m') {
        return parse_number(m);
    }
    None
}

/// Parses a whole number in English format: "5,718,887" or "1,507".
///
/// Only the English client format is accepted. A German "833.320" is
/// rejected instead of being read as 833, so a scan from a client in
/// another language is ignored rather than shown with wrong numbers.
fn parse_whole_number(text: &str) -> Option<u64> {
    let text = text.trim();
    let groups: Vec<&str> = text.split(',').collect();
    let first = groups.first()?;
    let first_ok = (1..=3).contains(&first.len()) || groups.len() == 1;
    let rest_ok = groups[1..].iter().all(|group| group.len() == 3);
    let digits_ok = groups
        .iter()
        .all(|group| !group.is_empty() && group.chars().all(|c| c.is_ascii_digit()));
    if !(first_ok && rest_ok && digits_ok) {
        return None;
    }
    groups.concat().parse().ok()
}

/// Parses an English-format number with an optional decimal part:
/// "499,992" or "93,300,000.00". See [`parse_whole_number`].
fn parse_number(text: &str) -> Option<f64> {
    let text = text.trim();
    let (whole, fraction) = match text.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (text, None),
    };
    let whole = parse_whole_number(whole)? as f64;
    match fraction {
        None => Some(whole),
        Some(digits) if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) => {
            let value: f64 = format!("0.{digits}").parse().ok()?;
            Some(whole + value)
        }
        Some(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A shortened scan in the format EVE copies, including group headers.
    const SCAN: &str = "Hedbergite III-Grade\t166,650\t499,950 m3\t93,300,000.00 ISK\t30 km
Omber\t900,000\t540,000 m3\t89,300,000.00 ISK\t64 km
Scordite IV-Grade
Scordite IV-Grade\t3,133,333\t469,999 m3\t56,900,000.00 ISK\t46 km
Mordunium\t5,000,000\t500,000 m3\t77,600,000.00 ISK\t1,507 m
Mordunium\t2,855,327\t285,532 m3\t44,300,000.00 ISK\t38 km
";

    fn at(secs: i64) -> LogTime {
        LogTime {
            unix_seconds: 1_794_096_000 + secs,
        }
    }

    #[test]
    fn parses_rocks_and_skips_headers() {
        let scan = BeltScan::parse(SCAN, at(0)).unwrap();
        assert_eq!(scan.rock_count(), 5);
        assert_eq!(
            scan.rocks[0],
            Rock {
                ore: "Hedbergite III-Grade".to_string(),
                units: 166_650,
                volume_m3: 499_950.0,
                distance_m: 30_000.0,
            }
        );
        assert_eq!(scan.rocks[3].distance_m, 1_507.0);
        assert_eq!(scan.total_m3(), 2_295_481.0);
    }

    /// Kevin's real scan (2026-11-11), copied in EVE. It has no character
    /// or corp names. Some rocks show "-" instead of an ISK value, and the
    /// first ores have no group header lines.
    const KEVIN_SCAN: &str = include_str!("testdata/belt_scan_kylixium.txt");

    #[test]
    fn parses_a_real_scan_with_missing_isk() {
        let scan = BeltScan::parse(KEVIN_SCAN, at(0)).unwrap();
        assert_eq!(scan.rock_count(), 29);
        assert_eq!(scan.total_m3(), 9_385_640.0);
        // "-" in the ISK column.
        assert_eq!(
            scan.rocks[4],
            Rock {
                ore: "Kylixium III-Grade".to_string(),
                units: 446_875,
                volume_m3: 536_250.0,
                distance_m: 30_000.0,
            }
        );
        // Distances in metres.
        assert_eq!(scan.rocks[11].distance_m, 3_801.0);
        let families = scan.by_family();
        assert_eq!(families[0].family, "Kylixium");
        assert_eq!(families[0].rocks, 17);
        assert_eq!(families.len(), 4);
    }

    #[test]
    fn rejects_other_isk_placeholders() {
        let line = "Omber\t900,000\t540,000 m3\tn/a\t64 km\n";
        assert!(BeltScan::parse(line, at(0)).is_none());
    }

    #[test]
    fn handles_windows_line_ends() {
        let text = "Omber\r\nOmber\t900,000\t540,000 m3\t1.00 ISK\t64 km\r\n\r\n";
        let scan = BeltScan::parse(text, at(0)).unwrap();
        assert_eq!(scan.rock_count(), 1);
    }

    #[test]
    fn rejects_other_clipboard_contents() {
        assert!(BeltScan::parse("", at(0)).is_none());
        assert!(BeltScan::parse("not a scan at all", at(0)).is_none());
        // A scan with an unrelated line mixed in.
        let mixed = "hello\nOmber\t900,000\t540,000 m3\t1.00 ISK\t64 km\n";
        assert!(BeltScan::parse(mixed, at(0)).is_none());
        // Tab-separated data that isn't a scan (e.g. from a spreadsheet).
        assert!(BeltScan::parse("a\t1\t2\t3\t4\n", at(0)).is_none());
        assert!(BeltScan::parse("Omber\t900,000\t540,000 m3\n", at(0)).is_none());
    }

    #[test]
    fn groups_by_family() {
        let scan = BeltScan::parse(SCAN, at(0)).unwrap();
        let families = scan.by_family();
        assert_eq!(families[0].family, "Mordunium");
        assert_eq!(families[0].rocks, 2);
        assert_eq!(families[0].volume_m3, 785_532.0);
        assert_eq!(families.len(), 4);
        assert_eq!(ore_family("Omber IV-Grade"), "Omber");
        assert_eq!(ore_family("Omber"), "Omber");
        assert_eq!(ore_family("Dark Glitter"), "Dark Glitter");
    }

    #[test]
    fn rate_and_time_to_clear() {
        let first = BeltScan::parse(SCAN, at(0)).unwrap();
        // 30 minutes later: one rock is gone and another lost 95,481 m³.
        let later_text = "Hedbergite III-Grade\t166,650\t499,950 m3\t1 ISK\t30 km
Omber\t900,000\t540,000 m3\t1 ISK\t64 km
Scordite IV-Grade\t3,133,333\t469,999 m3\t1 ISK\t46 km
Mordunium\t4,000,000\t400,000 m3\t1 ISK\t1,507 m
";
        let latest = BeltScan::parse(later_text, at(1_800)).unwrap();
        let progress = belt_progress(&first, &latest);
        assert_eq!(progress.rocks_left, 4);
        assert_eq!(progress.rocks_gone, 1);
        assert_eq!(progress.m3_left, 1_909_949.0);
        assert_eq!(progress.m3_mined, 385_532.0);
        assert_eq!(progress.m3_per_hour, Some(771_064.0));
        // 1,909,949 m³ at 771,064 m³/h is about 2.48 h.
        assert_eq!(progress.seconds_to_clear, Some(8_917));
    }

    #[test]
    fn no_rate_from_a_single_scan_or_no_change() {
        let scan = BeltScan::parse(SCAN, at(0)).unwrap();
        let same_time = belt_progress(&scan, &scan);
        assert_eq!(same_time.m3_per_hour, None);
        let later = BeltScan {
            taken: at(3_600),
            ..scan.clone()
        };
        let unchanged = belt_progress(&scan, &later);
        assert_eq!(unchanged.m3_per_hour, None);
        assert_eq!(unchanged.seconds_to_clear, None);
    }

    #[test]
    fn strict_numbers() {
        assert_eq!(parse_whole_number("5,718,887"), Some(5_718_887));
        assert_eq!(parse_whole_number("12"), Some(12));
        assert_eq!(parse_whole_number("833.320"), None);
        assert_eq!(parse_whole_number("1,50"), None);
        assert_eq!(parse_whole_number("-5"), None);
        assert_eq!(parse_whole_number("NaN"), None);
        assert_eq!(parse_number("93,300,000.00"), Some(93_300_000.0));
        assert_eq!(parse_number("499,992"), Some(499_992.0));
        assert_eq!(parse_number("inf"), None);
        // A German-format scan line is rejected, not misread.
        let german = "Omber\t833.320\t499.992 m3\t76.800.000,00 ISK\t96 km";
        assert!(BeltScan::parse(german, at(0)).is_none());
        // So is a row with an unreadable distance.
        let bad_distance = "Omber\t900,000\t540,000 m3\t1.00 ISK\tfar away";
        assert!(BeltScan::parse(bad_distance, at(0)).is_none());
    }

    #[test]
    fn detects_a_new_belt() {
        let first = BeltScan::parse(SCAN, at(0)).unwrap();
        let smaller =
            BeltScan::parse("Omber\t900,000\t500,000 m3\t1 ISK\t64 km\n", at(600)).unwrap();
        assert!(is_same_belt(&first, &smaller));
        let other_ore =
            BeltScan::parse("Veldspar\t900,000\t50,000 m3\t1 ISK\t64 km\n", at(600)).unwrap();
        assert!(!is_same_belt(&first, &other_ore));
        let bigger =
            BeltScan::parse("Omber\t900,000\t9,000,000 m3\t1 ISK\t64 km\n", at(600)).unwrap();
        assert!(!is_same_belt(&first, &bigger));
    }

    /// A copy of `SCAN` with the Mordunium rock at `units` and `m3`.
    fn scan_with_mordunium(units: &str, m3: &str, secs: i64) -> BeltScan {
        let text = format!(
            "Hedbergite III-Grade\t166,650\t499,950 m3\t1 ISK\t30 km
Omber\t900,000\t540,000 m3\t1 ISK\t64 km
Scordite IV-Grade\t3,133,333\t469,999 m3\t1 ISK\t46 km
Mordunium\t{units}\t{m3} m3\t1 ISK\t1,507 m
"
        );
        BeltScan::parse(&text, at(secs)).unwrap()
    }

    #[test]
    fn rate_rolls_over_the_last_two_scans() {
        let mut scans = BeltScans::default();
        scans.add(scan_with_mordunium("5,000,000", "500,000", 0));
        assert!(scans.previous().is_none());
        // 10 minutes later 100,000 m³ are gone.
        scans.add(scan_with_mordunium("4,000,000", "400,000", 600));
        // Another 10 minutes later only 20,000 m³ more are gone: the rate
        // must follow the slower pace, not average it with the first scan.
        scans.add(scan_with_mordunium("3,800,000", "380,000", 1_200));
        let previous = scans.previous().unwrap();
        let latest = scans.latest().unwrap();
        assert_eq!(previous.taken, at(600));
        assert_eq!(latest.taken, at(1_200));
        let progress = belt_progress(previous, latest);
        assert_eq!(progress.m3_mined, 20_000.0);
        // 20,000 m³ in 10 minutes is 120,000 m³/h (floats, so allow rounding).
        let rate = progress.m3_per_hour.unwrap();
        assert!((rate - 120_000.0).abs() < 0.01, "rate was {rate}");
    }

    #[test]
    fn quick_recopy_keeps_the_previous_scan() {
        let mut scans = BeltScans::default();
        scans.add(scan_with_mordunium("5,000,000", "500,000", 0));
        scans.add(scan_with_mordunium("4,000,000", "400,000", 600));
        // Copied again 20 s later: replaces the latest scan only.
        scans.add(scan_with_mordunium("3,990,000", "399,000", 620));
        assert_eq!(scans.previous().unwrap().taken, at(0));
        assert_eq!(scans.latest().unwrap().taken, at(620));
    }

    #[test]
    fn a_new_belt_starts_over() {
        let mut scans = BeltScans::default();
        scans.add(scan_with_mordunium("5,000,000", "500,000", 0));
        scans.add(scan_with_mordunium("4,000,000", "400,000", 600));
        let other = BeltScan::parse("Veldspar\t900,000\t50,000 m3\t1 ISK\t64 km\n", at(900));
        scans.add(other.unwrap());
        assert!(scans.previous().is_none());
        assert_eq!(scans.latest().unwrap().taken, at(900));
        scans.clear();
        assert_eq!(scans, BeltScans::default());
    }

    #[test]
    fn copies_under_a_minute_apart_still_give_a_rate() {
        let mut scans = BeltScans::default();
        // A copy every 45 s, each 1,000 m³ smaller than the one before.
        for step in 0..20_i64 {
            let m3 = 500_000 - step * 1_000;
            let units = m3 * 10;
            scans.add(scan_with_mordunium(
                &units.to_string(),
                &m3.to_string(),
                step * 45,
            ));
            if step < 2 {
                // 0 s and 45 s: no scan is a minute older yet.
                assert!(scans.previous().is_none());
                continue;
            }
            // The rate covers the last 90 s (two copies back), not the whole
            // time since the first copy.
            let previous = scans.previous().unwrap();
            let latest = scans.latest().unwrap();
            assert_eq!(latest.taken.seconds_since(previous.taken), 90);
            let rate = belt_progress(previous, latest).m3_per_hour.unwrap();
            assert!((rate - 80_000.0).abs() < 0.01, "rate was {rate}");
        }
        // Old scans are dropped, not kept forever.
        assert!(scans.recent.len() <= 3);
    }

    #[test]
    fn frequent_copies_after_a_rate_keep_rolling() {
        let mut scans = BeltScans::default();
        scans.add(scan_with_mordunium("5,000,000", "500,000", 0));
        scans.add(scan_with_mordunium("4,000,000", "400,000", 600));
        // Now copies every 45 s: the comparison scan must move on from 0 s.
        scans.add(scan_with_mordunium("3,990,000", "399,000", 645));
        scans.add(scan_with_mordunium("3,980,000", "398,000", 690));
        assert_eq!(scans.previous().unwrap().taken, at(600));
        scans.add(scan_with_mordunium("3,970,000", "397,000", 735));
        assert_eq!(scans.previous().unwrap().taken, at(645));
    }

    #[test]
    fn depletion_bar_starts_full_and_drains_live() {
        let mut scans = BeltScans::default();
        assert_eq!(scans.depletion(at(0)), None);
        scans.add(scan_with_mordunium("5,000,000", "500,000", 0));
        let first_m3 = scans.latest().unwrap().total_m3();
        let bar = scans.depletion(at(300)).unwrap();
        // One scan: full, no rate, nothing estimated.
        assert_eq!(bar.scanned_share, 1.0);
        assert_eq!(bar.estimated_share, 1.0);
        assert!(!bar.has_rate);
        assert!(bar.earlier_shares.is_empty());

        // 10 minutes later 100,000 m³ are gone: 600,000 m³/h.
        scans.add(scan_with_mordunium("4,000,000", "400,000", 600));
        let left = first_m3 - 100_000.0;
        let bar = scans.depletion(at(600)).unwrap();
        assert!(bar.has_rate);
        assert!((bar.scanned_share - left / first_m3).abs() < 1e-9);
        assert_eq!(bar.estimated_share, bar.scanned_share);
        // 5 minutes after that scan another 50,000 m³ should be gone.
        let bar = scans.depletion(at(900)).unwrap();
        assert!((bar.estimated_m3 - (left - 50_000.0)).abs() < 0.01);
        assert!(bar.estimated_share < bar.scanned_share);
        // Much later the estimate stops at empty.
        let bar = scans.depletion(at(100_000)).unwrap();
        assert_eq!(bar.estimated_m3, 0.0);
        assert_eq!(bar.estimated_share, 0.0);

        // A third scan: the second one becomes a tick, the first stays 100%.
        scans.add(scan_with_mordunium("3,800,000", "380,000", 1_200));
        let bar = scans.depletion(at(1_200)).unwrap();
        assert_eq!(bar.first_m3, first_m3);
        assert_eq!(bar.earlier_shares.len(), 1);
        assert!((bar.earlier_shares[0] - left / first_m3).abs() < 1e-9);
    }

    #[test]
    fn depletion_bar_restarts_with_a_new_belt() {
        let mut scans = BeltScans::default();
        scans.add(scan_with_mordunium("5,000,000", "500,000", 0));
        scans.add(scan_with_mordunium("4,000,000", "400,000", 600));
        let other = BeltScan::parse("Veldspar\t900,000\t50,000 m3\t1 ISK\t64 km\n", at(900));
        scans.add(other.unwrap());
        let bar = scans.depletion(at(900)).unwrap();
        assert_eq!(bar.first_m3, 50_000.0);
        assert_eq!(bar.scanned_share, 1.0);
        assert!(bar.earlier_shares.is_empty());
    }

    #[test]
    fn depletion_bar_keeps_the_first_scan_when_trimming_ticks() {
        let mut scans = BeltScans::default();
        for step in 0..(MAX_TOTALS as i64 + 10) {
            let m3 = 500_000 - step * 1_000;
            scans.add(scan_with_mordunium(
                &(m3 * 10).to_string(),
                &m3.to_string(),
                step * 120,
            ));
        }
        assert_eq!(scans.totals_m3.len(), MAX_TOTALS);
        let bar = scans.depletion(at(0)).unwrap();
        // The first scan is still the full bar.
        assert!(bar.first_m3 > scans.totals_m3[1]);
    }
}
