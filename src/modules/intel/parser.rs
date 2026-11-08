//! Reading an intel message: which systems, ship types and pilots it names,
//! and whether it says "clear" or "no visual".
//!
//! Intel is typed fast and loosely ("x7r loki +2 nv", "Hostile Pilot X7R-JW
//! gate"), so the parser is forgiving:
//!
//! - Systems match by full name (also multi-word names like "New Caldari"),
//!   ignoring upper/lower case, or by a prefix of at least 3 characters
//!   ("X7R" finds X7R-JW) when only one nearby system starts that way.
//!   "Nearby" is a set of systems the caller passes in: the shown region,
//!   the own pilots' regions and the regions next to both. Prefixes
//!   anywhere in New Eden would be ambiguous far too often.
//! - Ship types match by full name, also a plural "s" ("2 sabres").
//! - "nv" means no visual; the clear words (setting, "clr" and "clear" by
//!   default) mark the system as clear.
//! - Words left over, minus common intel words, may be pilot names. They're
//!   only candidates: a later step asks ESI which of them are real
//!   characters.

use std::collections::HashSet;

use crate::universe::Universe;

/// Common intel words that are never treated as pilot names, even though
/// some of them are real character names in EVE. Lowercase.
const STOP_WORDS: &[&str] = &[
    "gate", "gates", "ratting", "rat", "nv", "no", "visual", "clr", "clear", "in", "on", "at",
    "to", "from", "the", "and", "with", "local", "spike", "red", "reds", "neut", "neuts",
    "hostile", "hostiles", "gang", "fleet", "camp", "camping", "bubble", "bubbled", "bubbles",
    "cyno", "dock", "docked", "undock", "undocked", "station", "citadel", "keepstar", "fort",
    "astra", "tatara", "athanor", "anom", "anoms", "belt", "belts", "sun", "safe", "ess", "jumped",
    "jump", "jumps", "j", "inc", "incoming", "headed", "heading", "towards", "via", "out", "of",
    "is", "are", "was", "a", "an", "x", "plus", "+", "pod", "pods", "cloaky", "cloaked", "cloak",
    "afk", "status", "stat", "pls", "ty", "o7", "lol", "same", "still", "now", "here", "there",
    "near", "probes", "probing", "combat", "scan", "dscan", "d-scan", "warp", "warped", "warping",
    "landed", "pinged", "ping", "tackle", "tackled", "anyone", "someone", "any", "around", "eyes",
    "intel", "help",
];

/// What one intel message says.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IntelMessage {
    /// Systems named, in the order they appear, each once.
    pub systems: Vec<i64>,
    /// Ship type names as EVE writes them ("Loki"), each once.
    pub ships: Vec<String>,
    /// The message contains a clear word.
    pub clear: bool,
    /// The message contains "nv".
    pub no_visual: bool,
    /// Possible pilot names: runs of one to three leftover words, as
    /// typed. Longest first within each run.
    pub pilot_candidates: Vec<String>,
}

/// Everything the parser needs besides the text.
pub struct ParseContext<'a> {
    pub universe: &'a Universe,
    /// Systems whose names may be shortened to a prefix.
    pub nearby: &'a HashSet<i64>,
    /// Lowercase clear words, e.g. ["clr", "clear"].
    pub clear_words: &'a [String],
}

/// How a word was used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Used {
    Free,
    Taken,
}

/// Longest system or ship names we try, in words ("Federation Navy Comet").
const MAX_NAME_WORDS: usize = 4;

/// Reads one intel message.
pub fn parse(text: &str, context: &ParseContext) -> IntelMessage {
    let words: Vec<&str> = text.split_whitespace().map(clean_word).collect();
    let lower: Vec<String> = words.iter().map(|w| w.to_lowercase()).collect();
    let mut used = vec![Used::Free; words.len()];
    let mut message = IntelMessage::default();
    // Found names with the position of their first word, so they can be
    // listed in the order they were written.
    let mut systems: Vec<(usize, i64)> = Vec::new();
    let mut ships: Vec<(usize, String)> = Vec::new();

    // Systems and ships, full names first, longest names first, so "New
    // Caldari" wins over a pilot called "New".
    for length in (1..=MAX_NAME_WORDS).rev() {
        for start in 0..words.len() {
            let end = start + length;
            if end > words.len() || used[start..end].contains(&Used::Taken) {
                continue;
            }
            let phrase = lower[start..end].join(" ");
            if phrase.is_empty() {
                continue;
            }
            if let Some(system) = context.universe.system_by_name(&phrase) {
                systems.push((start, system.id));
            } else if let Some(ship) = ship_by_name(context.universe, &phrase) {
                ships.push((start, ship));
            } else {
                continue;
            }
            used[start..end].fill(Used::Taken);
        }
    }

    for (index, word) in lower.iter().enumerate() {
        if used[index] == Used::Taken || word.is_empty() {
            continue;
        }
        if context.clear_words.iter().any(|w| w == word) {
            message.clear = true;
            used[index] = Used::Taken;
        } else if word == "nv" {
            message.no_visual = true;
            used[index] = Used::Taken;
        } else if STOP_WORDS.contains(&word.as_str()) {
            // "out" would find Outuni, "gate" a system starting "Gate...".
        } else if let Some(id) = system_by_prefix(word, context) {
            systems.push((index, id));
            used[index] = Used::Taken;
        }
    }
    systems.sort();
    ships.sort();
    for (_, id) in systems {
        push_unique(&mut message.systems, id);
    }
    for (_, name) in ships {
        push_unique(&mut message.ships, name);
    }

    // Leftover words, split into runs at every word that was used, a stop
    // word or a number like "+3".
    let mut run: Vec<&str> = Vec::new();
    for (index, word) in words.iter().enumerate() {
        let free = used[index] == Used::Free && could_be_name_word(word, &lower[index]);
        if free {
            run.push(word);
        } else {
            add_candidates(&run, &mut message.pilot_candidates);
            run.clear();
        }
    }
    add_candidates(&run, &mut message.pilot_candidates);
    message
}

/// Strips punctuation people put around words: "X7R-JW," "(loki)" "nv!".
/// Characters inside the word ("-", "'", ".") are kept, since EVE names
/// can contain them.
fn clean_word(word: &str) -> &str {
    word.trim_matches(|c: char| {
        matches!(
            c,
            ',' | '.' | '!' | '?' | ':' | ';' | '(' | ')' | '[' | ']' | '"' | '*' | '/'
        )
    })
}

/// A ship by name, also accepting a plural "s" ("sabres"), but not for
/// intel words ("probes" isn't two Probes).
fn ship_by_name(universe: &Universe, phrase: &str) -> Option<String> {
    if let Some(ship) = universe.ship_by_name(phrase) {
        return Some(ship.name.clone());
    }
    if STOP_WORDS.contains(&phrase) {
        return None;
    }
    let singular = phrase.strip_suffix('s')?;
    universe
        .ship_by_name(singular)
        .map(|ship| ship.name.clone())
}

/// The one nearby system whose name starts with `word` (3+ characters).
fn system_by_prefix(word: &str, context: &ParseContext) -> Option<i64> {
    if word.chars().count() < 3 {
        return None;
    }
    let mut found = None;
    for &id in context.nearby {
        let Some(system) = context.universe.system(id) else {
            continue;
        };
        if system.name.to_lowercase().starts_with(word) {
            if found.is_some() {
                // Two systems start like this: too vague to guess.
                return None;
            }
            found = Some(id);
        }
    }
    found
}

/// False for words that can't be (part of) a pilot name here.
fn could_be_name_word(word: &str, lower: &str) -> bool {
    if word.is_empty() || STOP_WORDS.contains(&lower) {
        return false;
    }
    // "+3", "2", "x5": counts, not names.
    let digits_and_signs = word
        .trim_start_matches(['+', 'x', 'X'])
        .chars()
        .all(|c| c.is_ascii_digit());
    if digits_and_signs {
        return false;
    }
    // Names are letters, digits, spaces, "-", "'", "." and "_".
    word.chars()
        .all(|c| c.is_alphanumeric() || matches!(c, '-' | '\'' | '.' | '_'))
}

/// Adds every run of 1 to 3 consecutive words, longest first.
fn add_candidates(run: &[&str], candidates: &mut Vec<String>) {
    for length in (1..=3.min(run.len())).rev() {
        for start in 0..=(run.len() - length) {
            let name = run[start..start + length].join(" ");
            // EVE names are 3 to 37 characters.
            if (3..=37).contains(&name.chars().count()) && !candidates.contains(&name) {
                candidates.push(name);
            }
        }
    }
}

fn push_unique<T: PartialEq>(list: &mut Vec<T>, value: T) {
    if !list.contains(&value) {
        list.push(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::universe::tests::SAMPLE_MAP_SQL;

    fn universe() -> Universe {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch(SAMPLE_MAP_SQL).unwrap();
        // A multi-word system name too.
        db.execute_batch(
            "INSERT INTO systems VALUES (105, 'New Caldari', 11, 1, 1.0, 0, 36),
                 (106, 'ZZA-111', 11, 1, -0.5, 36, 36), (107, 'ZZA-222', 11, 1, -0.5, 72, 36),
                 (108, 'Outuni', 11, 1, 0.4, 108, 36);
             INSERT INTO ship_types VALUES (22456, 'Sabre', 'Interdictor'),
                 (72811, 'Federation Navy Comet', 'Frigate'),
                 (3532, 'Probe', 'Frigate');",
        )
        .unwrap();
        Universe::from_connection(&db).unwrap()
    }

    fn parse_with(text: &str, nearby: &[i64]) -> IntelMessage {
        let universe = universe();
        let nearby: HashSet<i64> = nearby.iter().copied().collect();
        let clear_words = vec!["clr".to_string(), "clear".to_string()];
        parse(
            text,
            &ParseContext {
                universe: &universe,
                nearby: &nearby,
                clear_words: &clear_words,
            },
        )
    }

    #[test]
    fn full_system_name_ship_and_pilot() {
        let m = parse_with("Evil Pilot A1-XYZ Loki nv", &[]);
        assert_eq!(m.systems, [101]);
        assert_eq!(m.ships, ["Loki"]);
        assert!(m.no_visual);
        assert!(!m.clear);
        assert_eq!(m.pilot_candidates, ["Evil Pilot", "Evil", "Pilot"]);
    }

    #[test]
    fn prefixes_only_match_nearby_and_unique() {
        // "a3-q" fits only A3-QQQ among the nearby systems.
        assert_eq!(parse_with("a3-q", &[101, 102, 103]).systems, [103]);
        // Not nearby: no match.
        assert!(parse_with("a3-q", &[201]).systems.is_empty());
        // Upper or lower case doesn't matter.
        assert_eq!(parse_with("A1-", &[101, 102]).systems, [101]);
        // Too short to guess.
        assert!(parse_with("a3", &[103]).systems.is_empty());
    }

    #[test]
    fn ambiguous_prefix_is_ignored() {
        // ZZA-111 and ZZA-222 both start with "zza".
        assert!(parse_with("zza", &[106, 107]).systems.is_empty());
        assert_eq!(parse_with("zza-1", &[106, 107]).systems, [106]);
        // With only one of them nearby, "zza" is clear enough.
        assert_eq!(parse_with("zza", &[107]).systems, [107]);
    }

    #[test]
    fn multi_word_names_and_plurals() {
        let m = parse_with("new caldari 2 sabres + federation navy comet", &[]);
        assert_eq!(m.systems, [105]);
        assert_eq!(m.ships, ["Sabre", "Federation Navy Comet"]);
        assert!(m.pilot_candidates.is_empty());
    }

    #[test]
    fn clear_words_and_punctuation() {
        let m = parse_with("A1-XYZ, clr!", &[]);
        assert_eq!(m.systems, [101]);
        assert!(m.clear);
        assert!(m.pilot_candidates.is_empty());
    }

    #[test]
    fn stop_words_are_not_prefixes_or_plurals() {
        // "out" would match Outuni as a prefix; "probes" the ship Probe.
        let m = parse_with("out probes", &[108]);
        assert!(m.systems.is_empty());
        assert!(m.ships.is_empty());
        // The real names still work.
        let m = parse_with("outun probe", &[108]);
        assert_eq!(m.systems, [108]);
        assert_eq!(m.ships, ["Probe"]);
    }

    #[test]
    fn counts_and_stop_words_are_not_names() {
        let m = parse_with("+3 reds on gate x5", &[]);
        assert!(m.pilot_candidates.is_empty());
    }

    #[test]
    fn runs_split_at_systems() {
        let m = parse_with("Pilot One A1-XYZ Pilot Two Three", &[]);
        assert_eq!(m.systems, [101]);
        assert!(m.pilot_candidates.contains(&"Pilot One".to_string()));
        assert!(m.pilot_candidates.contains(&"Pilot Two Three".to_string()));
        // Never across the system name.
        assert!(!m.pilot_candidates.iter().any(|c| c.contains("A1-XYZ")));
    }

    #[test]
    fn a_system_named_twice_is_listed_once() {
        let m = parse_with("A1-XYZ A1-XYZ a1-xyz", &[]);
        assert_eq!(m.systems, [101]);
    }
}
