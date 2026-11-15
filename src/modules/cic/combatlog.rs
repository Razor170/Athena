//! Parsing of the `(combat)` lines of EVE game logs, for the CIC module.
//!
//! The game log and its header (`Listener: <character>`) are described in
//! [`crate::modules::mining::gamelog`]; this file only looks at the text
//! after `(combat)`. The lines are full of EVE markup (`<color=…>`, `<b>`,
//! `<font size=…>`), which is stripped before matching. Shapes we read
//! (shown without the markup):
//!
//! ```text
//! 353 remote shield boosted to Metenox Moon Drill [AA] [TICK]  [System - Name] - - Large Remote Shield Booster II
//! 428 remote shield boosted by Pilot Name [AA][TICK] Scimitar - Large Remote Shield Booster II
//! 117 remote capacitor transmitted to Basilisk [AA] [TICK]  [Pilot Name] - - Medium Remote Capacitor Transmitter II
//! 36 from Guristas Eraser - Penetrates
//! 886 from Pilot Name[TICK](Drake Navy Issue) - Heavy Missile - Hits
//! Guristas Eraser misses you completely
//! You're jammed by Guristas Eliminator - Guristas Eliminator
//! Warp disruption attempt from Pilot [AA][TICK] Malediction to Other Pilot  Basilisk
//! Warp scramble attempt from …
//! ```
//!
//! Remote armor reps are assumed to read like the shield ones, with
//! "remote armor repaired to/by" (no sample yet).
//!
//! EVE writes the other ship in two orders: older lines have "Ship [tickers]
//! [Name]" (name last, in brackets), newer ones "Name [tickers] Ship". Both
//! are read here.
//!
//! Everything here is plain functions on strings, so it is tested without
//! files. All matching is on the English log text.

/// Shield, armor or capacitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepKind {
    Shield,
    Armor,
    Cap,
}

/// The other side of a remote rep or cap transfer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Other {
    /// The pilot's name, or a structure's own name.
    pub name: String,
    /// The ship or structure type, e.g. "Basilisk" or "Metenox Moon Drill".
    /// Empty if the line doesn't say.
    pub ship: String,
}

/// Something from a combat line that the Logi view shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CombatEvent {
    /// A remote shield/armor rep or cap transfer, sent (`outgoing`) or
    /// received. `amount` can be 0: a cap transfer of 0 still means the
    /// chain link is up.
    Rep {
        kind: RepKind,
        outgoing: bool,
        amount: u32,
        other: Other,
        module: String,
    },
    /// Damage taken. Can be 0.
    Damage { amount: u32 },
    /// "… misses you completely".
    Miss,
    /// "You're jammed by …": the ship lost its locks.
    Jammed { by: String },
    /// A warp disruption (point) or, with `scram`, scramble attempt on the
    /// listener. `from` is the tackler as the log writes it.
    Tackle { scram: bool, from: String },
}

/// Parses one game log line. `listener` is the file's character, needed to
/// tell tackle attempts on us from those on others. `None` for every line
/// that isn't one of the shapes above, which is most of them.
pub fn parse_combat_line(line: &str, listener: &str) -> Option<CombatEvent> {
    // "[ 2026.11.08 10:58:12 ] (combat) …"
    let (_, rest) = line.trim().split_once(']')?;
    let raw = rest.trim_start().strip_prefix("(combat)")?;
    let text = clean(raw);
    let text = text.trim();

    if let Some(event) = parse_rep(text) {
        return Some(event);
    }
    if let Some(event) = parse_damage(text) {
        return Some(event);
    }
    if text.ends_with("misses you completely") && !text.starts_with("Your ") {
        return Some(CombatEvent::Miss);
    }
    for prefix in ["You're jammed by ", "You are jammed by "] {
        if let Some(by) = text.strip_prefix(prefix) {
            let by = by.split(" - ").next().unwrap_or(by).trim();
            return Some(CombatEvent::Jammed { by: by.to_string() });
        }
    }
    for (prefix, scram) in [
        ("Warp disruption attempt from ", false),
        ("Warp scramble attempt from ", true),
    ] {
        if let Some(rest) = text.strip_prefix(prefix) {
            return parse_tackle(raw, rest, scram, listener);
        }
    }
    None
}

/// "N remote shield boosted to|by …", "N remote armor repaired to|by …",
/// "N remote capacitor transmitted to|by …".
fn parse_rep(text: &str) -> Option<CombatEvent> {
    let (number, rest) = text.split_once(' ')?;
    let amount = parse_number(number)?;
    let rest = rest.trim_start().strip_prefix("remote ")?;
    // The kind of rep, and whether we sent it ("to") or got it ("by").
    let (kind, rest) = [
        (RepKind::Shield, "shield boosted "),
        (RepKind::Armor, "armor repaired "),
        (RepKind::Cap, "capacitor transmitted "),
    ]
    .into_iter()
    .find_map(|(kind, words)| Some((kind, rest.strip_prefix(words)?)))?;
    let (outgoing, rest) = [(true, "to "), (false, "by ")]
        .into_iter()
        .find_map(|(outgoing, word)| Some((outgoing, rest.strip_prefix(word)?)))?;
    // The module is after the last " - ". Module names have no " - ", but
    // structure names do ("System - Name"), so split from the right.
    let (target, module) = match rest.rsplit_once(" - ") {
        Some((target, module)) => (target, module.trim()),
        None => (rest, ""),
    };
    Some(CombatEvent::Rep {
        kind,
        outgoing,
        amount,
        other: parse_other(target),
        module: module.to_string(),
    })
}

/// Reads "Ship [tickers] [Name]" (old) or "Name [tickers] Ship" (new).
fn parse_other(target: &str) -> Other {
    // The old shape ends with " -" before the module's " - ".
    let target = target.trim().trim_end_matches('-').trim();
    let (Some(first), Some(last)) = (target.find('['), target.rfind(']')) else {
        // No tickers: an NPC or something unusual. Keep it whole.
        return Other {
            name: target.to_string(),
            ship: String::new(),
        };
    };
    if last + 1 == target.len() {
        // Old: the name is in the last brackets.
        let open = target.rfind('[').unwrap_or(first);
        Other {
            name: target[open + 1..last].trim().to_string(),
            ship: target[..first].trim().to_string(),
        }
    } else {
        Other {
            name: target[..first].trim().to_string(),
            ship: target[last + 1..].trim().to_string(),
        }
    }
}

/// "N from Someone - … - Hits": damage we took. ("N to …" is damage we
/// dealt, which the Logi view doesn't show.)
fn parse_damage(text: &str) -> Option<CombatEvent> {
    let (number, rest) = text.split_once(' ')?;
    let amount = parse_number(number)?;
    rest.trim_start().strip_prefix("from ")?;
    Some(CombatEvent::Damage { amount })
}

/// The target is after "to" in the markup; only attempts on the listener
/// count. `rest` is the plain text after "attempt from ".
fn parse_tackle(raw: &str, rest: &str, scram: bool, listener: &str) -> Option<CombatEvent> {
    // The plain text has "… Malediction to Other Pilot  Basilisk". Names
    // can contain " to ", so find the "to" that follows a markup tag in the
    // raw line instead.
    let after_to = raw.rfind(">to ").map(|at| &raw[at + 4..])?;
    // The target's name is the first bold text after "to", its ship the
    // next one: "<b>…<b>Other Pilot</b> …<b>Basilisk</b>".
    let target = match (after_to.find("<b>"), after_to.find("</b>")) {
        (Some(open), Some(close)) if open < close => clean(&after_to[open..close]),
        _ => clean(after_to),
    };
    let target = target.trim();
    let on_us = target.eq_ignore_ascii_case("you")
        || target.eq_ignore_ascii_case("you!")
        || (!listener.is_empty() && target == listener);
    if !on_us {
        return None;
    }
    let from = match rest.rfind(" to ") {
        Some(at) => &rest[..at],
        None => rest,
    };
    let from = parse_other(from);
    let from = if from.ship.is_empty() {
        from.name
    } else {
        format!("{} ({})", from.name, from.ship)
    };
    Some(CombatEvent::Tackle { scram, from })
}

/// "353" or "1,234" (EVE may group thousands).
fn parse_number(text: &str) -> Option<u32> {
    if text.is_empty()
        || !text
            .chars()
            .all(|c| c.is_ascii_digit() || c == ',' || c == '.')
    {
        return None;
    }
    let digits: String = text.chars().filter(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// Removes EVE markup like `<color=0xffccff66>` and `</font>`, turns the
/// few HTML entities EVE writes back into characters, and squeezes runs of
/// spaces to one (tags often leave two behind).
fn clean(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut inside_tag = false;
    for c in text.chars() {
        match c {
            '<' => inside_tag = true,
            '>' if inside_tag => inside_tag = false,
            _ if !inside_tag => plain.push(c),
            _ => {}
        }
    }
    let plain = plain
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&");
    let mut out = String::with_capacity(plain.len());
    let mut last_space = false;
    for c in plain.chars() {
        let space = c.is_whitespace();
        if !(space && last_space) {
            out.push(if space { ' ' } else { c });
        }
        last_space = space;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // Shaped like real game log lines, with the markup EVE writes, but with
    // made-up names and tickers. Never paste real names into tests.
    const SHIELD_OUT_STRUCTURE: &str = "[ 2026.11.08 10:58:12 ] (combat) <color=0xffccff66><b>353</b><color=0x77ffffff><font size=10> remote shield boosted to </font><b><color=0xffffffff><font size=12><color=0xFFFFB300> <u><b>Metenox Moon Drill</b></u></color></font><font size=12><color=0xFFFFFF66> [<b>AA</b>]</color></font> [<b>TICK</b>]  [Test-1 - Drill]<color=0xFFFFFFFF><b> -</b><color=0x77ffffff><font size=10> - Large S95a Scoped Remote Shield Booster</font>";
    const CAP_OUT_OLD: &str = "[ 2026.11.08 10:58:15 ] (combat) <color=0xffccff66><b>117</b><color=0x77ffffff><font size=10> remote capacitor transmitted to </font><b><color=0xffffffff><font size=12><color=0xFFFFB300> <u><b>Basilisk</b></u></color></font><font size=12><color=0xFFFFFF66> [<b>AA</b>]</color></font> [<b>-TCK</b>]  [Mira Tal]<color=0xFFFFFFFF><b> -</b><color=0x77ffffff><font size=10> - Medium Remote Capacitor Transmitter II</font>";
    const CAP_IN_OLD: &str = "[ 2026.11.08 10:58:15 ] (combat) <color=0xffccff66><b>0</b><color=0x77ffffff><font size=10> remote capacitor transmitted by </font><b><color=0xffffffff><font size=12><color=0xFFFFB300> <u><b>Basilisk</b></u></color></font><font size=12><color=0xFFFFFF66> [<b>AA</b>]</color></font> [<b>-TCK</b>]  [Mira Tal]<color=0xFFFFFFFF><b> -</b><color=0x77ffffff><font size=10> - Large Inductive Compact Remote Capacitor Transmitter</font>";
    const SHIELD_IN_NEW: &str = "[ 2026.11.12 20:21:05 ] (combat) <color=0xffccff66><b>428</b><color=0x77ffffff><font size=10> remote shield boosted by </font><b><color=0xffffffff><font size=12><color=0xFFFFFFFF><b>Juno Varr</b> </color></font><font size=12><color=0xFFFFB300>[AA]</color></font><font size=12>[DDD-]</font> <font size=12><color=0xFFFFFFFF><b>Scimitar</b></color></font></b><color=0x77ffffff><font size=10> - Pithum C-Type Medium Remote Shield Booster</font>";
    const SHIELD_OUT_NEW: &str = "[ 2026.11.12 20:01:37 ] (combat) <color=0xffccff66><b>428</b><color=0x77ffffff><font size=10> remote shield boosted to </font><b><color=0xffffffff><font size=12><color=0xFFFFFFFF><b>Count Test</b> </color></font><font size=12><color=0xFFFFB300>[AA]</color></font><font size=12>[BBBB]</font> <font size=12><color=0xFFFFFFFF><b>Muninn</b></color></font></b><color=0x77ffffff><font size=10> - Pithum C-Type Medium Remote Shield Booster</font>";
    const DAMAGE_NPC: &str = "[ 2026.11.08 09:28:02 ] (combat) <color=0xffcc0000><b>36</b> <color=0x77ffffff><font size=10>from</font> <b><color=0xffffffff>Guristas Eraser</b><font size=10><color=0x77ffffff> - Penetrates";
    const DAMAGE_PLAYER: &str = "[ 2026.11.12 20:21:02 ] (combat) <color=0xffcc0000><b>886</b> <color=0x77ffffff><font size=10>from</font> <b><color=0xffffffff>Some Pilot[TICK](Drake Navy Issue)</b><font size=10><color=0x77ffffff> - Caldari Navy Nova Heavy Missile - Hits";
    const DAMAGE_OUT: &str = "[ 2026.11.08 09:27:12 ] (combat) <color=0xff00ffff><b>36</b> <color=0x77ffffff><font size=10>to</font> <b><color=0xffffffff>Guristas Destroyer</b><font size=10><color=0x77ffffff> - Vespa II - Grazes";
    const MISS_IN: &str = "[ 2026.11.08 09:27:52 ] (combat) Guristas Eraser misses you completely";
    const MISS_OUT: &str = "[ 2026.11.08 09:28:33 ] (combat) Your Vespa II misses Guristas Destroyer completely - Vespa II";
    const JAMMED: &str = "[ 2026.11.12 17:01:40 ] (combat) <color=0x77ffffff><font size=10>You're</font> <color=0xffffffff><b>jammed</b> <color=0x77ffffff><font size=10>by</font> <color=0xffffffff><b>Guristas Eliminator</b><color=0x77ffffff><font size=10> - Guristas Eliminator</font>";
    const POINT: &str = "[ 2026.11.12 19:45:52 ] (combat) <color=0xffffffff><b>Warp disruption attempt</b> <color=0x77ffffff><font size=10>from</font> <color=0xffffffff><b><font size=12><color=0xFFFFFFFF><b>Vel Arkon</b> </color></font><font size=12><color=0xFFFFB300>[AA]</color></font><font size=12>[CCCC]</font> <font size=12><color=0xFFFFFFFF><b>Malediction</b></color></font></b> <color=0x77ffffff><font size=10>to <b><color=0xffffffff></font><font size=12><color=0xFFFFFFFF><b>Test Pilot</b> </color></font> <font size=12><color=0xFFFFFFFF><b>Thrasher Fleet Issue</b></color></font>";
    const SCRAM: &str = "[ 2026.11.12 20:00:50 ] (combat) <color=0xffffffff><b>Warp scramble attempt</b> <color=0x77ffffff><font size=10>from</font> <color=0xffffffff><b><font size=12><color=0xFFFFFFFF><b>Rex Halden</b> </color></font><font size=12><color=0xFFFFB300>[AA]</color></font><font size=12>[BBB]</font> <font size=12><color=0xFFFFFFFF><b>Sabre</b></color></font></b> <color=0x77ffffff><font size=10>to <b><color=0xffffffff></font><font size=12><color=0xFFFFFFFF><b>Test Pilot</b> </color></font> <font size=12><color=0xFFFFFFFF><b>Basilisk</b></color></font>";

    fn parse(line: &str) -> Option<CombatEvent> {
        parse_combat_line(line, "Test Pilot")
    }

    #[test]
    fn shield_rep_to_a_structure_old_shape() {
        assert_eq!(
            parse(SHIELD_OUT_STRUCTURE),
            Some(CombatEvent::Rep {
                kind: RepKind::Shield,
                outgoing: true,
                amount: 353,
                other: Other {
                    name: "Test-1 - Drill".into(),
                    ship: "Metenox Moon Drill".into()
                },
                module: "Large S95a Scoped Remote Shield Booster".into(),
            })
        );
    }

    #[test]
    fn cap_transfers_old_shape() {
        let Some(CombatEvent::Rep {
            kind,
            outgoing,
            amount,
            other,
            module,
        }) = parse(CAP_OUT_OLD)
        else {
            panic!("not a rep");
        };
        assert_eq!((kind, outgoing, amount), (RepKind::Cap, true, 117));
        assert_eq!(other.name, "Mira Tal");
        assert_eq!(other.ship, "Basilisk");
        assert_eq!(module, "Medium Remote Capacitor Transmitter II");

        let Some(CombatEvent::Rep {
            outgoing,
            amount,
            other,
            module,
            ..
        }) = parse(CAP_IN_OLD)
        else {
            panic!("not a rep");
        };
        assert!(!outgoing);
        assert_eq!(amount, 0);
        assert_eq!(other.name, "Mira Tal");
        assert_eq!(
            module,
            "Large Inductive Compact Remote Capacitor Transmitter"
        );
    }

    #[test]
    fn shield_reps_new_shape() {
        let Some(CombatEvent::Rep {
            outgoing, other, ..
        }) = parse(SHIELD_IN_NEW)
        else {
            panic!("not a rep");
        };
        assert!(!outgoing);
        assert_eq!(
            other,
            Other {
                name: "Juno Varr".into(),
                ship: "Scimitar".into()
            }
        );
        let Some(CombatEvent::Rep {
            outgoing,
            other,
            module,
            ..
        }) = parse(SHIELD_OUT_NEW)
        else {
            panic!("not a rep");
        };
        assert!(outgoing);
        assert_eq!(other.name, "Count Test");
        assert_eq!(other.ship, "Muninn");
        assert_eq!(module, "Pithum C-Type Medium Remote Shield Booster");
    }

    #[test]
    fn armor_reps_assumed_like_shield() {
        let line = SHIELD_IN_NEW.replace("remote shield boosted by", "remote armor repaired by");
        let Some(CombatEvent::Rep { kind, amount, .. }) = parse(&line) else {
            panic!("not a rep");
        };
        assert_eq!((kind, amount), (RepKind::Armor, 428));
    }

    #[test]
    fn damage_taken_but_not_dealt() {
        assert_eq!(parse(DAMAGE_NPC), Some(CombatEvent::Damage { amount: 36 }));
        assert_eq!(
            parse(DAMAGE_PLAYER),
            Some(CombatEvent::Damage { amount: 886 })
        );
        assert_eq!(parse(DAMAGE_OUT), None);
    }

    #[test]
    fn misses_on_us_only() {
        assert_eq!(parse(MISS_IN), Some(CombatEvent::Miss));
        assert_eq!(parse(MISS_OUT), None);
    }

    #[test]
    fn jammed() {
        assert_eq!(
            parse(JAMMED),
            Some(CombatEvent::Jammed {
                by: "Guristas Eliminator".into()
            })
        );
        let entity = JAMMED.replace("You're", "You&#39;re");
        assert!(matches!(parse(&entity), Some(CombatEvent::Jammed { .. })));
    }

    #[test]
    fn tackle_on_us() {
        assert_eq!(
            parse(POINT),
            Some(CombatEvent::Tackle {
                scram: false,
                from: "Vel Arkon (Malediction)".into()
            })
        );
        assert_eq!(
            parse(SCRAM),
            Some(CombatEvent::Tackle {
                scram: true,
                from: "Rex Halden (Sabre)".into()
            })
        );
    }

    #[test]
    fn tackle_on_someone_else_is_ignored() {
        assert_eq!(parse_combat_line(POINT, "Someone Else"), None);
        // A name that only starts like ours is someone else.
        assert_eq!(parse_combat_line(POINT, "Test"), None);
    }

    #[test]
    fn other_lines_are_ignored() {
        for line in [
            "[ 2026.11.08 09:27:02 ] (notify) Drones engaging Guristas Destroyer",
            "[ 2026.11.08 09:26:22 ] (mining) You mined 137 units of Gneiss",
            "[ 2026.11.08 09:26:22 ] (combat) something new",
            "not a log line",
        ] {
            assert_eq!(parse(line), None, "{line}");
        }
    }

    #[test]
    fn other_without_tickers() {
        assert_eq!(
            parse_other("Guristas Eraser"),
            Other {
                name: "Guristas Eraser".into(),
                ship: String::new()
            }
        );
    }
}
