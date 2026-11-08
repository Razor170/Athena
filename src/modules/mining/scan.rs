//! Belt scans: the result of the Mining Survey Scanner, copied from EVE and
//! pasted into Athena.
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
    pub distance_m: Option<f64>,
}

/// A parsed scan and when it was pasted.
#[derive(Debug, Clone, PartialEq)]
pub struct BeltScan {
    pub taken: LogTime,
    pub rocks: Vec<Rock>,
}

impl BeltScan {
    /// Parses pasted scan text. Lines that don't look like a rock are
    /// skipped. Returns `None` if no rock was found at all, so a wrong
    /// clipboard content can be reported to the user.
    pub fn parse(text: &str, taken: LogTime) -> Option<BeltScan> {
        let rocks: Vec<Rock> = text.lines().filter_map(parse_rock).collect();
        if rocks.is_empty() {
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

/// "Mordunium III-Grade" -> "Mordunium". Names without a grade stay as
/// they are.
pub fn ore_family(ore: &str) -> &str {
    match ore.rsplit_once(' ') {
        Some((family, grade)) if grade.ends_with("-Grade") => family,
        _ => ore,
    }
}

/// Parses one line of the scan, or returns `None` for headers and junk.
fn parse_rock(line: &str) -> Option<Rock> {
    let columns: Vec<&str> = line.split('\t').map(str::trim).collect();
    if columns.len() < 3 {
        return None;
    }
    let ore = columns[0];
    if ore.is_empty() {
        return None;
    }
    let units = parse_number(columns[1])?;
    // "499,992 m3"
    let volume_m3 = parse_number(columns[2].trim_end_matches("m3").trim_end_matches("m³"))?;
    // Column 3 is ISK, ignored. Column 4 is the distance, if present.
    let distance_m = columns.get(4).and_then(|text| parse_distance(text));
    Some(Rock {
        ore: ore.to_string(),
        units: units as u64,
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

/// Parses "5,718,887" or "1,507" or "93,300,000.00". EVE's English client
/// uses ',' for thousands and '.' for decimals.
fn parse_number(text: &str) -> Option<f64> {
    let cleaned: String = text
        .trim()
        .chars()
        .filter(|c| !matches!(c, ',' | ' ' | '\u{a0}'))
        .collect();
    if cleaned.is_empty() {
        return None;
    }
    cleaned.parse().ok()
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
                distance_m: Some(30_000.0),
            }
        );
        assert_eq!(scan.rocks[3].distance_m, Some(1_507.0));
        assert_eq!(scan.total_m3(), 2_295_481.0);
    }

    #[test]
    fn handles_windows_line_ends_and_junk() {
        let text = "hello\r\nOmber\t900,000\t540,000 m3\t1.00 ISK\t64 km\r\n\r\n";
        let scan = BeltScan::parse(text, at(0)).unwrap();
        assert_eq!(scan.rock_count(), 1);
        assert!(BeltScan::parse("not a scan at all", at(0)).is_none());
        assert!(BeltScan::parse("", at(0)).is_none());
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
}
