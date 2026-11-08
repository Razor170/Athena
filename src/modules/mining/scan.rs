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
//! what it reprocesses into.
//!
//! Two scans of the same belt taken some time apart give the rate at which
//! the belt is being mined by everyone on it, and from that an estimate of
//! when it will be empty.

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

/// Compares the first and the latest scan of a belt.
///
/// The rate is only given when the scans are at least `MIN_SECONDS` apart
/// and the belt actually shrank; otherwise it is `None` ("paste another scan
/// later").
pub fn belt_progress(first: &BeltScan, latest: &BeltScan) -> BeltProgress {
    const MIN_SECONDS: i64 = 60;
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
    // Column 3 is ISK, ignored. Column 4 is the distance.
    if !columns[3].ends_with("ISK") {
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
}
