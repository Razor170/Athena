//! Parsing of EVE game logs (`Documents\EVE\logs\Gamelogs\*.txt`).
//!
//! Game logs are UTF-8 text. Each file starts with a header that names the
//! character ("Listener"), followed by one line per event:
//!
//! ```text
//! ------------------------------------------------------------
//!   Gamelog
//!   Listener: Some Pilot
//!   Session Started: 2026.11.08 07:30:12
//! ------------------------------------------------------------
//! [ 2026.11.08 07:31:05 ] (mining) You mined <color=#ff8dc169>1,234</color> units of <color=#ffffffff>Veldspar</color>
//! ```
//!
//! The text part contains EVE's markup tags (`<color=...>`, `<font ...>`),
//! which we strip before matching. Everything here is plain functions on
//! strings, so it can be tested without files.

/// A point in time as written in the log (EVE time, which is UTC).
///
/// We keep our own small type instead of adding a date crate: the log only
/// needs parsing, ordering and "seconds between two times".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct LogTime {
    /// Seconds since 1970-01-01 00:00:00 UTC.
    pub unix_seconds: i64,
}

impl LogTime {
    /// Parses `2026.11.08 07:31:05`.
    pub fn parse(text: &str) -> Option<LogTime> {
        let (date, time) = text.trim().split_once(' ')?;
        let mut date_parts = date.split('.');
        let year: i64 = date_parts.next()?.parse().ok()?;
        let month: i64 = date_parts.next()?.parse().ok()?;
        let day: i64 = date_parts.next()?.parse().ok()?;
        let mut time_parts = time.trim().split(':');
        let hour: i64 = time_parts.next()?.parse().ok()?;
        let minute: i64 = time_parts.next()?.parse().ok()?;
        let second: i64 = time_parts.next()?.parse().ok()?;
        if date_parts.next().is_some() || time_parts.next().is_some() {
            return None;
        }
        if !(1..=12).contains(&month)
            || !(1..=31).contains(&day)
            || !(0..24).contains(&hour)
            || !(0..60).contains(&minute)
            || !(0..61).contains(&second)
        {
            return None;
        }
        let days = days_from_civil(year, month, day);
        Some(LogTime {
            unix_seconds: days * 86_400 + hour * 3_600 + minute * 60 + second,
        })
    }

    /// Seconds from `earlier` to `self` (negative if `earlier` is later).
    pub fn seconds_since(self, earlier: LogTime) -> i64 {
        self.unix_seconds - earlier.unix_seconds
    }
}

/// Days since 1970-01-01 for a calendar date (proleptic Gregorian).
/// This is Howard Hinnant's well-known `days_from_civil` algorithm.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let year_of_era = y - era * 400;
    let shifted_month = (month + 9) % 12; // March = 0 ... February = 11
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The header at the top of a game log file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameLogHeader {
    /// The character this file belongs to. Several EVE clients write into the
    /// same folder, so this is how we tell the files apart.
    pub character: String,
    pub session_started: Option<LogTime>,
}

/// Reads the header from the first lines of a game log.
///
/// Returns `None` if there is no `Listener:` line, for example when EVE has
/// only written part of the header so far.
pub fn parse_header(text: &str) -> Option<GameLogHeader> {
    let mut character = None;
    let mut session_started = None;
    // The header is short; don't scan the whole file.
    for line in text.lines().take(10) {
        let line = line.trim();
        if let Some(name) = line.strip_prefix("Listener:") {
            let name = name.trim();
            if !name.is_empty() {
                character = Some(name.to_string());
            }
        } else if let Some(time) = line.strip_prefix("Session Started:") {
            session_started = LogTime::parse(time);
        }
    }
    Some(GameLogHeader {
        character: character?,
        session_started,
    })
}

/// Something from the game log that the Mining Supervisor cares about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MiningEvent {
    /// One mining cycle finished and put ore in the hold.
    /// `critical` is true for the "Critical mining success!" bonus line.
    Mined {
        units: u64,
        ore: String,
        critical: bool,
    },
    /// Ore lost from the asteroid as residue (modulated strip miners with
    /// certain crystals). It is gone from the rock but not in your hold.
    Residue { units: u64 },
    /// Ore in the hold was compressed. Compression is 1:1 in units: `units`
    /// of `from` became `units` of `to` (which takes far less space).
    Compressed {
        from: String,
        to: String,
        units: u64,
    },
    /// A mining module switched itself off, e.g. because the asteroid is
    /// depleted or the crystal broke. `reason` is the rest of the message.
    MinerStopped { module: String, reason: String },
}

/// A mining event with the time it happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimedMiningEvent {
    pub time: LogTime,
    pub event: MiningEvent,
}

/// Parses one line of a game log. Returns `None` for lines that aren't about
/// mining, which is most of them.
pub fn parse_mining_line(line: &str) -> Option<TimedMiningEvent> {
    // "[ 2026.11.08 07:31:05 ] (mining) ..."
    let rest = line.trim().strip_prefix('[')?;
    let (time_text, rest) = rest.split_once(']')?;
    let time = LogTime::parse(time_text)?;
    let rest = rest.trim_start();
    let (category, message) = rest.strip_prefix('(')?.split_once(')')?;
    let message = strip_tags(message);
    let message = message.trim();

    let event = match category {
        "mining" => parse_mining_message(message)?,
        "notify" => parse_notify_message(message)?,
        _ => return None,
    };
    Some(TimedMiningEvent { time, event })
}

/// The text after "(mining)", with tags already removed.
fn parse_mining_message(message: &str) -> Option<MiningEvent> {
    // "Critical mining success! You mined an additional 12 units of Veldspar"
    if let Some(after) = message.strip_prefix("Critical mining success! You mined an additional ") {
        let (units, ore) = parse_units_of(after)?;
        return Some(MiningEvent::Mined {
            units,
            ore,
            critical: true,
        });
    }
    // "You mined 1,234 units of Veldspar"
    if let Some(after) = message.strip_prefix("You mined ") {
        let (units, ore) = parse_units_of(after)?;
        return Some(MiningEvent::Mined {
            units,
            ore,
            critical: false,
        });
    }
    // "Additional 12 units depleted from asteroid as residue"
    if let Some(after) = message.strip_prefix("Additional ") {
        let (number, tail) = after.split_once(' ')?;
        if !tail.contains("residue") {
            return None;
        }
        return Some(MiningEvent::Residue {
            units: parse_units(number)?,
        });
    }
    None
}

/// The text after "(notify)", with tags already removed.
fn parse_notify_message(message: &str) -> Option<MiningEvent> {
    // "Successfully compressed Veldspar into 3430 Compressed Veldspar."
    if let Some(after) = message.strip_prefix("Successfully compressed ") {
        let (from, rest) = after.split_once(" into ")?;
        let (number, to) = rest.split_once(' ')?;
        let to = to.trim().trim_end_matches('.').trim();
        let from = from.trim();
        if from.is_empty() || to.is_empty() {
            return None;
        }
        return Some(MiningEvent::Compressed {
            from: from.to_string(),
            to: to.to_string(),
            units: parse_units(number)?,
        });
    }
    // "Modulated Strip Miner II deactivates as it finds the resource ..."
    // "Modulated Strip Miner II deactivates due to the destruction of ..."
    // Only mining modules: other modules (e.g. a hardener) also "deactivate".
    if let Some((module, reason)) = message.split_once(" deactivates ") {
        let lower = module.to_lowercase();
        let is_miner = lower.contains("miner") || lower.contains("harvester");
        if !is_miner {
            return None;
        }
        return Some(MiningEvent::MinerStopped {
            module: module.trim().to_string(),
            reason: reason.trim().to_string(),
        });
    }
    None
}

/// Parses "1,234 units of Veldspar" into (1234, "Veldspar").
fn parse_units_of(text: &str) -> Option<(u64, String)> {
    let (number, ore) = text.split_once(" units of ")?;
    let ore = ore.trim().trim_end_matches('.').trim();
    if ore.is_empty() {
        return None;
    }
    Some((parse_units(number)?, ore.to_string()))
}

/// Parses "1,234" or "1.234" or "1 234" (the separator depends on the
/// client's language settings) into 1234.
fn parse_units(text: &str) -> Option<u64> {
    let digits: String = text
        .trim()
        .chars()
        .filter(|c| !matches!(c, ',' | '.' | ' ' | '\u{a0}' | '\''))
        .collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse().ok()
}

/// Removes EVE markup like `<color=#ff8dc169>` and `</font>`.
fn strip_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut inside_tag = false;
    for c in text.chars() {
        match c {
            '<' => inside_tag = true,
            '>' if inside_tag => inside_tag = false,
            _ if !inside_tag => out.push(c),
            _ => {}
        }
    }
    out
}

/// Turns a growing game log file into events, a chunk at a time.
///
/// EVE appends to the log while we read it, so a read can end in the middle of
/// a line (or even in the middle of a multi-byte character). This keeps the
/// unfinished part until the rest arrives. The caller (a background thread)
/// reads new bytes from the file and passes them in; this struct does no I/O
/// itself, which keeps it easy to test.
#[derive(Debug, Default)]
pub struct GameLogReader {
    header: Option<GameLogHeader>,
    /// Text seen so far while the header is still incomplete.
    header_text: String,
    /// Bytes of a line whose end hasn't been written yet.
    partial_line: Vec<u8>,
}

impl GameLogReader {
    pub fn new() -> GameLogReader {
        GameLogReader::default()
    }

    /// The header, once it has been read.
    pub fn header(&self) -> Option<&GameLogHeader> {
        self.header.as_ref()
    }

    /// Feeds newly read bytes and returns the mining events they completed.
    pub fn push(&mut self, new_bytes: &[u8]) -> Vec<TimedMiningEvent> {
        self.partial_line.extend_from_slice(new_bytes);
        let mut events = Vec::new();

        // Everything up to the last '\n' is complete; the rest stays buffered.
        let Some(last_newline) = self.partial_line.iter().rposition(|&b| b == b'\n') else {
            return events;
        };
        let complete: Vec<u8> = self.partial_line.drain(..=last_newline).collect();
        // Game logs are UTF-8. A broken byte becomes a replacement character
        // instead of an error, so one bad line can't stop the whole file.
        let complete = String::from_utf8_lossy(&complete);

        // `lines()` splits on '\n' and drops one '\r' before it. EVE sometimes
        // writes "\r\r\n"; the extra '\r' is removed by `trim` in the parsers.
        for line in complete.lines() {
            if self.header.is_none() {
                self.header_text.push_str(line);
                self.header_text.push('\n');
                self.header = parse_header(&self.header_text);
                if self.header.is_some() {
                    self.header_text.clear();
                }
                continue;
            }
            if let Some(event) = parse_mining_line(line) {
                events.push(event);
            }
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Shaped like a real game log (CRLF line ends, EVE's unclosed tags), but
    // with made-up names. Never paste real character names into tests.
    const SAMPLE: &str = "------------------------------------------------------------\r\n  Gamelog\r\n  Listener: Test Pilot\r\n  Session Started: 2026.11.08 07:30:12\r\n------------------------------------------------------------\r\n[ 2026.11.08 07:31:05 ] (question) Are you sure you want to join the fleet?\r\r\n<br><br>\r\r\nNOTE: a second line\r\n[ 2026.11.08 07:31:10 ] (mining) <color=0x77ffffff>You mined <font size=12><color=#ff8dc169>1,234<color=0x77ffffff><font size=10> units of <color=0xffffffff><font size=12>Veldspar\r\n[ 2026.11.08 07:31:10 ] (mining) Critical mining success! You mined an additional <color=#ff8dc169>56<color=0x77ffffff> units of <color=0xffffffff>Veldspar\r\n[ 2026.11.08 07:31:12 ] (mining) <color=0x77ffffff>Additional <font size=12><color=#ffff454b>572<color=0x77ffffff><font size=10> units depleted from asteroid as residue\r\n[ 2026.11.08 07:32:00 ] (notify) Successfully compressed Veldspar into 1290 Compressed Veldspar.\r\n[ 2026.11.08 07:32:10 ] (combat) <b>Something</b> hits you\r\n[ 2026.11.08 07:33:00 ] (notify) Modulated Strip Miner II deactivates as it finds the resource it was harvesting a pale shadow of its former glory.\r\n";

    fn t(text: &str) -> LogTime {
        LogTime::parse(text).unwrap()
    }

    #[test]
    fn parses_time() {
        assert_eq!(t("1970.01.02 00:00:01").unix_seconds, 86_401);
        assert_eq!(t("2000.03.01 00:00:00").unix_seconds, 951_868_800);
        assert!(LogTime::parse("2026.13.01 00:00:00").is_none());
        assert!(LogTime::parse("nonsense").is_none());
    }

    #[test]
    fn parses_header() {
        let header = parse_header(SAMPLE).unwrap();
        assert_eq!(header.character, "Test Pilot");
        assert_eq!(header.session_started, Some(t("2026.11.08 07:30:12")));
        assert!(parse_header("  Gamelog\n  Listener: \n").is_none());
    }

    #[test]
    fn parses_mined_line() {
        let line = "[ 2026.11.08 07:31:10 ] (mining) <color=0x77ffffff>You mined <font size=12><color=#ff8dc169>571<color=0x77ffffff><font size=10> units of <color=0xffffffff><font size=12>Compressed Kylixium III-Grade";
        let event = parse_mining_line(line).unwrap();
        assert_eq!(event.time, t("2026.11.08 07:31:10"));
        assert_eq!(
            event.event,
            MiningEvent::Mined {
                units: 571,
                ore: "Compressed Kylixium III-Grade".to_string(),
                critical: false,
            }
        );
    }

    #[test]
    fn parses_critical_line() {
        let line = "[ 2026.11.08 07:31:10 ] (mining) Critical mining success! You mined an additional 56 units of Veldspar";
        assert_eq!(
            parse_mining_line(line).unwrap().event,
            MiningEvent::Mined {
                units: 56,
                ore: "Veldspar".to_string(),
                critical: true,
            }
        );
    }

    #[test]
    fn parses_residue_line() {
        let line = "[ 2026.11.08 07:31:12 ] (mining) Additional <color=#ff8dc169>56</color> units depleted from asteroid as residue";
        assert_eq!(
            parse_mining_line(line).unwrap().event,
            MiningEvent::Residue { units: 56 }
        );
    }

    #[test]
    fn parses_compression_line() {
        let line = "[ 2026.11.08 07:32:00 ] (notify) Successfully compressed Kylixium III-Grade into 3430 Compressed Kylixium III-Grade.";
        assert_eq!(
            parse_mining_line(line).unwrap().event,
            MiningEvent::Compressed {
                from: "Kylixium III-Grade".to_string(),
                to: "Compressed Kylixium III-Grade".to_string(),
                units: 3430,
            }
        );
    }

    #[test]
    fn parses_miner_stopped_lines() {
        let depleted = "[ 2026.11.08 07:33:00 ] (notify) Modulated Strip Miner II deactivates as it finds the resource it was harvesting a pale shadow of its former glory.";
        let crystal = "[ 2026.11.08 07:33:00 ] (notify) Modulated Strip Miner II deactivates due to the destruction of the Variegated Asteroid Mining Crystal Type B II it was fitted with. ";
        for line in [depleted, crystal] {
            match parse_mining_line(line).unwrap().event {
                MiningEvent::MinerStopped { module, .. } => {
                    assert_eq!(module, "Modulated Strip Miner II")
                }
                other => panic!("unexpected event {other:?}"),
            }
        }
        let not_a_miner =
            "[ 2026.11.08 07:33:00 ] (notify) Shield Hardener deactivates for some reason.";
        assert!(parse_mining_line(not_a_miner).is_none());
    }

    #[test]
    fn ignores_other_lines() {
        assert!(parse_mining_line("[ 2026.11.08 07:31:05 ] (notify) Ship stopping").is_none());
        assert!(parse_mining_line("[ 2026.11.08 07:31:05 ] (None) Jumping from A to B").is_none());
        assert!(parse_mining_line("  Listener: Test Pilot").is_none());
        assert!(parse_mining_line("<br><br>").is_none());
        assert!(parse_mining_line("").is_none());
        assert!(parse_mining_line("[ 2026.11.08 07:31:05 ] (mining) Something new").is_none());
    }

    #[test]
    fn unit_separators() {
        assert_eq!(parse_units("1,234"), Some(1234));
        assert_eq!(parse_units("1.234"), Some(1234));
        assert_eq!(parse_units("12"), Some(12));
        assert_eq!(parse_units(""), None);
        assert_eq!(parse_units("abc"), None);
    }

    #[test]
    fn reader_handles_whole_file() {
        let mut reader = GameLogReader::new();
        let events = reader.push(SAMPLE.as_bytes());
        assert_eq!(reader.header().unwrap().character, "Test Pilot");
        // mined, critical, residue, compressed, miner stopped
        assert_eq!(events.len(), 5);
    }

    #[test]
    fn reader_handles_lines_split_across_reads() {
        let mut reader = GameLogReader::new();
        let mut events = Vec::new();
        // Feed the file in awkward 7-byte pieces, like a log that is still
        // being written.
        for piece in SAMPLE.as_bytes().chunks(7) {
            events.extend(reader.push(piece));
        }
        assert_eq!(reader.header().unwrap().character, "Test Pilot");
        assert_eq!(events, GameLogReader::new().push(SAMPLE.as_bytes()));
    }

    #[test]
    fn reader_handles_split_utf8_character() {
        let mut reader = GameLogReader::new();
        reader.push(b"  Listener: Test Pilot\n");
        let line = "[ 2026.11.08 07:31:10 ] (mining) You mined 5 units of Veldspär\n";
        let bytes = line.as_bytes();
        // Split inside the two-byte 'ä'.
        let split = line.find('ä').unwrap() + 1;
        assert!(reader.push(&bytes[..split]).is_empty());
        let events = reader.push(&bytes[split..]);
        assert_eq!(
            events[0].event,
            MiningEvent::Mined {
                units: 5,
                ore: "Veldspär".to_string(),
                critical: false,
            }
        );
    }
}
