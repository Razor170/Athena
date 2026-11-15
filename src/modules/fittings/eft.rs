//! Reads fittings in EFT format (what EVE's fitting window copies with
//! "Copy to clipboard", and what Pyfa exports) and compares two of them.
//!
//! An EFT fit looks like this:
//!
//! ```text
//! [Thanatos, DPS - Thanatos]
//!
//! Damage Control II            <- low slots
//! ...
//!
//! 10000MN Afterburner I        <- med slots
//! ...
//!
//! Heavy Energy Neutralizer II  <- high slots
//! ...
//!
//! Capital Trimark Armor Pump I <- rigs (then subsystems on T3 cruisers)
//!
//!
//! Einherji II x6               <- drones and fighters
//!
//!
//! Nanite Repair Paste x500     <- cargo
//! ```
//!
//! Blank lines split the text into blocks. The blocks before the first one
//! with "xN" lines are the slot racks, in the order low, med, high, rigs,
//! subsystems.
//!
//! The "xN" blocks are the drone bay (with fighters) and the cargo. Their
//! order differs: Pyfa and EVE's saved fittings put drones first, while
//! "Copy to clipboard" on a ship in EVE puts the cargo first. So with the
//! game data loaded ([`parse_with_drones`]) a block counts as drones when
//! most of its items are drones or fighters, whatever its place. Without
//! game data ([`parse`]) the last "xN" block is the cargo and the ones
//! before it are drones; a single one counts as cargo until [`align`]
//! compares it with the other fit.

use std::collections::BTreeMap;

/// Where an item sits in the fit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Section {
    Low,
    Med,
    High,
    Rigs,
    Subsystems,
    Drones,
    Cargo,
}

impl Section {
    /// The order Kevin wants on screen: rigs first, cargo last.
    pub const SHOWN: [Section; 7] = [
        Section::Rigs,
        Section::High,
        Section::Med,
        Section::Low,
        Section::Subsystems,
        Section::Drones,
        Section::Cargo,
    ];

    /// The slot racks in the order EFT lists them.
    const RACKS: [Section; 5] = [
        Section::Low,
        Section::Med,
        Section::High,
        Section::Rigs,
        Section::Subsystems,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Section::Low => "Low slots",
            Section::Med => "Med slots",
            Section::High => "High slots",
            Section::Rigs => "Rigs",
            Section::Subsystems => "Subsystems",
            Section::Drones => "Drones & fighters",
            Section::Cargo => "Cargo",
        }
    }
}

/// One item of a fit with how many there are (duplicate lines summed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub name: String,
    pub count: u64,
}

/// A parsed fit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Fit {
    /// The ship type, such as "Thanatos". Empty without a header line.
    pub hull: String,
    /// The fit's name, such as "DPS - Thanatos".
    pub name: String,
    /// Items per section, in the order they first appear in the text.
    pub sections: BTreeMap<Section, Vec<Line>>,
    /// The fit had exactly one block of "xN" lines, so it was put in
    /// Cargo without knowing if it really is cargo or drones. [`align`]
    /// may move it to Drones.
    pub lone_counted_block: bool,
}

impl Fit {
    /// The items of one section (empty if it has none).
    pub fn lines(&self, section: Section) -> &[Line] {
        self.sections.get(&section).map_or(&[], |v| v.as_slice())
    }

    /// How many of `name` the fit has in `section`.
    pub fn count(&self, section: Section, name: &str) -> u64 {
        self.lines(section)
            .iter()
            .find(|l| l.name == name)
            .map_or(0, |l| l.count)
    }

    pub fn is_empty(&self) -> bool {
        self.hull.is_empty() && self.sections.is_empty()
    }

    fn add(&mut self, section: Section, name: &str, count: u64) {
        let lines = self.sections.entry(section).or_default();
        match lines.iter_mut().find(|l| l.name == name) {
            Some(line) => line.count = line.count.saturating_add(count),
            None => lines.push(Line {
                name: name.to_string(),
                count,
            }),
        }
    }
}

/// Reads an EFT fit without game data: drones and cargo are told apart by
/// block order (see the top of this file). Never fails: lines it doesn't
/// understand are read as item names, so a typo shows up in the compare
/// instead of vanishing.
pub fn parse(text: &str) -> Fit {
    parse_blocks(text, None)
}

/// Reads an EFT fit, using `is_drone` (true for drone and fighter names,
/// from the SDE) to tell the drone bay from the cargo.
pub fn parse_with_drones(text: &str, is_drone: &dyn Fn(&str) -> bool) -> Fit {
    parse_blocks(text, Some(is_drone))
}

// `&dyn Fn(&str) -> bool` is "any function that takes a name and answers
// yes or no", so the tests can pass a short list instead of the SDE.
fn parse_blocks(text: &str, is_drone: Option<&dyn Fn(&str) -> bool>) -> Fit {
    let mut fit = Fit::default();
    let mut blocks: Vec<Vec<&str>> = Vec::new();
    let mut block: Vec<&str> = Vec::new();
    for line in text.lines() {
        // `lines()` already drops "\n" and "\r\n"; trim removes the rest.
        let line = line.trim();
        if fit.hull.is_empty()
            && blocks.is_empty()
            && block.is_empty()
            && let Some((hull, name)) = header(line)
        {
            fit.hull = hull.to_string();
            fit.name = name.to_string();
            continue;
        }
        if line.is_empty() {
            if !block.is_empty() {
                blocks.push(std::mem::take(&mut block));
            }
        } else {
            block.push(line);
        }
    }
    if !block.is_empty() {
        blocks.push(block);
    }

    let has_counts = |b: &Vec<&str>| b.iter().any(|l| split_count(l).1.is_some());
    let first_counted = blocks.iter().position(has_counts);
    let last_counted = blocks.iter().rposition(has_counts);
    // Only a guess without game data; `align` may fix it up.
    fit.lone_counted_block =
        is_drone.is_none() && blocks.iter().filter(|b| has_counts(b)).count() == 1;
    let mut racks = Section::RACKS.iter();
    for (i, block) in blocks.iter().enumerate() {
        let section = match first_counted {
            // Slot racks; any block past the fifth rack goes to cargo.
            None => *racks.next().unwrap_or(&Section::Cargo),
            Some(first) if i < first => *racks.next().unwrap_or(&Section::Cargo),
            // Counted blocks before the last are drones and fighters.
            // Uncounted blocks after the racks (Pyfa's implants and
            // boosters) go to cargo.
            Some(_) if !has_counts(block) => Section::Cargo,
            // With game data: drones when most items are drones/fighters.
            Some(_) if is_drone.is_some_and(|f| mostly_drones(block, f)) => Section::Drones,
            Some(_) if is_drone.is_some() => Section::Cargo,
            Some(_) if Some(i) != last_counted => Section::Drones,
            Some(_) => Section::Cargo,
        };
        for line in block {
            if let Some(name_and_count) = item(line) {
                fit.add(section, name_and_count.0, name_and_count.1);
            }
        }
    }
    fit
}

/// More than half of the block's items are drones or fighters.
fn mostly_drones(block: &[&str], is_drone: &dyn Fn(&str) -> bool) -> bool {
    let names: Vec<&str> = block
        .iter()
        .filter_map(|l| item(l))
        .map(|(n, _)| n)
        .collect();
    let drones = names.iter().filter(|n| is_drone(n)).count();
    drones * 2 > names.len()
}

/// `[Thanatos, DPS - Thanatos]` gives `("Thanatos", "DPS - Thanatos")`.
fn header(line: &str) -> Option<(&str, &str)> {
    let inner = line.strip_prefix('[')?.strip_suffix(']')?;
    // EVE and Pyfa always write "Hull, Name"; a bare "[Hull]" works too.
    let (hull, name) = inner.split_once(',').unwrap_or((inner, ""));
    let hull = hull.trim();
    if hull.is_empty() || hull.to_ascii_lowercase().starts_with("empty ") {
        return None;
    }
    Some((hull, name.trim()))
}

/// `Einherji II x6` gives `("Einherji II", Some(6))`.
fn split_count(line: &str) -> (&str, Option<u64>) {
    if let Some((name, last)) = line.rsplit_once(' ')
        && let Some(digits) = last.strip_prefix('x')
        && !digits.is_empty()
        && digits.bytes().all(|b| b.is_ascii_digit())
        && let Ok(count) = digits.parse::<u64>()
    {
        return (name.trim_end(), Some(count));
    }
    (line, None)
}

/// The item name and count of one line, or `None` for lines that hold no
/// item (`[Empty Low slot]`).
fn item(line: &str) -> Option<(&str, u64)> {
    let lower = line.to_ascii_lowercase();
    if lower.starts_with("[empty ") {
        return None;
    }
    let (rest, count) = split_count(line);
    // "Heavy Neutralizer II, Some Charge": only the module counts.
    let name = rest.split(',').next().unwrap_or(rest).trim();
    // Pyfa marks offline modules with "/OFFLINE".
    let name = name
        .strip_suffix("/OFFLINE")
        .or_else(|| name.strip_suffix("/offline"))
        .unwrap_or(name)
        .trim();
    if name.is_empty() {
        return None;
    }
    Some((name, count.unwrap_or(1)))
}

/// One item in the Diff column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub name: String,
    /// Should minus Is: above 0 is missing, below 0 is surplus, 0 matches.
    pub change: i64,
    /// How many the Is fit has (shown for matching items).
    pub have: u64,
}

/// Is compared with Should.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Diff {
    pub sections: BTreeMap<Section, Vec<DiffLine>>,
    pub missing: usize,
    pub surplus: usize,
    pub matching: usize,
}

impl Diff {
    pub fn lines(&self, section: Section) -> &[DiffLine] {
        self.sections.get(&section).map_or(&[], |v| v.as_slice())
    }

    /// The missing items as "Name Count" lines, which EVE's Multibuy
    /// accepts. The same item missing in two sections is added up.
    pub fn multibuy(&self) -> String {
        let mut total: Vec<(&str, i64)> = Vec::new();
        for section in Section::SHOWN {
            for line in self.lines(section).iter().filter(|l| l.change > 0) {
                match total.iter_mut().find(|(n, _)| *n == line.name) {
                    Some((_, count)) => *count += line.change,
                    None => total.push((&line.name, line.change)),
                }
            }
        }
        total
            .iter()
            .map(|(name, count)| format!("{name} {count}"))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Makes the drones/cargo guess of a fit with a single "xN" block match
/// the other fit. If the other fit has both drones and cargo, the lone
/// block goes to whichever of the two shares more item names with it
/// (a tie keeps it as cargo). Without this, a doctrine fit with only drones
/// compared with a ship that also has cargo would show the same drones as
/// both missing and surplus.
pub fn align(is: &mut Fit, should: &mut Fit) {
    align_one(is, should);
    align_one(should, is);
}

fn align_one(fit: &mut Fit, other: &Fit) {
    if !fit.lone_counted_block
        || other.lines(Section::Drones).is_empty()
        || other.lines(Section::Cargo).is_empty()
    {
        return;
    }
    let shared = |section: Section| {
        fit.lines(Section::Cargo)
            .iter()
            .filter(|l| other.count(section, &l.name) > 0)
            .count()
    };
    if shared(Section::Drones) > shared(Section::Cargo)
        && let Some(lines) = fit.sections.remove(&Section::Cargo)
    {
        // Pyfa's implants/boosters blocks also land in Cargo, but they
        // only exist next to more than one "xN" block, so here Cargo is
        // exactly the lone block.
        fit.sections.insert(Section::Drones, lines);
    }
}

/// Compares the fit you have (`is`) with the one you want (`should`), item
/// by item within each section. A module in cargo only counts against
/// cargo, so a spare in the hold doesn't hide a missing fitted one.
pub fn compare(is: &Fit, should: &Fit) -> Diff {
    let mut diff = Diff::default();
    for section in Section::SHOWN {
        let mut names: Vec<&str> = Vec::new();
        for line in is.lines(section).iter().chain(should.lines(section)) {
            if !names.contains(&line.name.as_str()) {
                names.push(&line.name);
            }
        }
        let mut lines: Vec<DiffLine> = names
            .into_iter()
            .map(|name| {
                let have = is.count(section, name);
                let want = should.count(section, name);
                // Counts are far below i64::MAX; `try_from` just avoids a cast.
                let change = i64::try_from(want).unwrap_or(i64::MAX)
                    - i64::try_from(have).unwrap_or(i64::MAX);
                DiffLine {
                    name: name.to_string(),
                    change,
                    have,
                }
            })
            .collect();
        for line in &lines {
            match line.change {
                c if c > 0 => diff.missing += 1,
                c if c < 0 => diff.surplus += 1,
                _ => diff.matching += 1,
            }
        }
        // Missing first, then surplus, then matching; by name within each.
        lines.sort_by(|a, b| {
            order(a.change)
                .cmp(&order(b.change))
                .then_with(|| a.name.cmp(&b.name))
        });
        if !lines.is_empty() {
            diff.sections.insert(section, lines);
        }
    }
    diff
}

fn order(change: i64) -> u8 {
    match change {
        c if c > 0 => 0,
        c if c < 0 => 1,
        _ => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Kevin's example fit from the Fittings request.
    pub const THANATOS: &str = "[Thanatos, DPS - Thanatos]

25000mm Crystalline Carbonide Restrained Plates
25000mm Crystalline Carbonide Restrained Plates
Coreli A-Type Multispectrum Coating
Coreli A-Type Multispectrum Coating
Damage Control II
Drone Damage Amplifier II

10000MN Afterburner I
Capital Capacitor Booster I
Drone Navigation Computer II
Drone Navigation Computer II
Omnidirectional Tracking Link II

Heavy Energy Neutralizer II
Fighter Support Unit I
Fighter Support Unit I
Heavy Energy Neutralizer II
Integrated Sensor Array

Capital Trimark Armor Pump I
Capital Trimark Armor Pump I
Capital Trimark Armor Pump I



Einherji II x6
Einherji II x6
Einherji II x6
Templar II x6
Templar II x6
Templar II x6
Dromi II x3
Dromi II x3
Gram I x36




Armor Energizing Charge x1000
Armor Reinforcement Charge x1000
Rapid Repair Charge x1000
Cap Booster 3200 x60
Electronic Hardening Charge x1000
Electronic Superiority Charge x1000
Sensor Optimization Charge x1000
Nanite Repair Paste x500
Optimal Range Script x2
Tracking Speed Script x2
Mobile Depot x2
Prototype Cloaking Device I x1
Armor Command Burst II x2
Information Command Burst II x2
Drone Navigation Computer II x2
Omnidirectional Tracking Link II x1
Heavy Energy Neutralizer II x1
Inertial Stabilizers II x2
Ship Scanner II x1
Oxygen Isotopes x100000";

    #[test]
    fn reads_kevins_thanatos() {
        let fit = parse(THANATOS);
        assert_eq!(fit.hull, "Thanatos");
        assert_eq!(fit.name, "DPS - Thanatos");
        assert_eq!(fit.count(Section::Low, "Damage Control II"), 1);
        assert_eq!(
            fit.count(Section::Low, "Coreli A-Type Multispectrum Coating"),
            2
        );
        assert_eq!(fit.count(Section::Med, "Drone Navigation Computer II"), 2);
        assert_eq!(fit.count(Section::High, "Heavy Energy Neutralizer II"), 2);
        assert_eq!(fit.count(Section::Rigs, "Capital Trimark Armor Pump I"), 3);
        // Duplicate lines are summed.
        assert_eq!(fit.count(Section::Drones, "Einherji II"), 18);
        assert_eq!(fit.count(Section::Drones, "Dromi II"), 6);
        assert_eq!(fit.count(Section::Drones, "Gram I"), 36);
        assert_eq!(fit.count(Section::Cargo, "Oxygen Isotopes"), 100_000);
        // The same module in cargo is kept apart from the fitted ones.
        assert_eq!(fit.count(Section::Cargo, "Drone Navigation Computer II"), 2);
        assert_eq!(fit.count(Section::Cargo, "Heavy Energy Neutralizer II"), 1);
        assert_eq!(fit.lines(Section::Subsystems).len(), 0);
        assert_eq!(fit.lines(Section::Cargo).len(), 20);
    }

    #[test]
    fn same_fit_has_no_differences() {
        let fit = parse(THANATOS);
        let diff = compare(&fit, &fit);
        assert_eq!((diff.missing, diff.surplus), (0, 0));
        assert_eq!(diff.matching, 36);
        assert_eq!(diff.multibuy(), "");
    }

    #[test]
    fn missing_and_surplus() {
        let should = parse(THANATOS);
        let is_text = THANATOS
            .replacen(
                "Coreli A-Type Multispectrum Coating\nDamage",
                "Imperial Navy Multispectrum Coating\nDamage",
                1,
            )
            .replacen("Einherji II x6\n", "", 1)
            .replace("Nanite Repair Paste x500", "Nanite Repair Paste x200")
            .replace("Cap Booster 3200 x60", "Cap Booster 3200 x90");
        let is = parse(&is_text);
        let diff = compare(&is, &should);
        assert_eq!(diff.missing, 3);
        assert_eq!(diff.surplus, 2);

        let low = diff.lines(Section::Low);
        assert_eq!(low[0].name, "Coreli A-Type Multispectrum Coating");
        assert_eq!(low[0].change, 1);
        assert_eq!(low[1].name, "Imperial Navy Multispectrum Coating");
        assert_eq!(low[1].change, -1);

        let drones = diff.lines(Section::Drones);
        assert_eq!(drones[0].name, "Einherji II");
        assert_eq!(drones[0].change, 6);

        let cargo = diff.lines(Section::Cargo);
        assert_eq!(
            (cargo[0].name.as_str(), cargo[0].change),
            ("Nanite Repair Paste", 300)
        );
        assert_eq!(
            (cargo[1].name.as_str(), cargo[1].change),
            ("Cap Booster 3200", -30)
        );

        assert_eq!(
            diff.multibuy(),
            "Coreli A-Type Multispectrum Coating 1\nEinherji II 6\nNanite Repair Paste 300"
        );
    }

    #[test]
    fn charges_empty_slots_offline_and_crlf() {
        let text = "[Drake, Test]\r\n\r\nBallistic Control System II\r\n[Empty Low slot]\r\n\r\n\
                    Large Shield Extender II /OFFLINE\r\n\r\n\
                    Heavy Missile Launcher II, Scourge Heavy Missile\r\n\
                    Heavy Missile Launcher II, Nova Heavy Missile\r\n\r\n\
                    Medium Core Defense Field Extender I\r\n\r\n\r\n\
                    Hornet EC-300 x2\r\n\r\n\r\nScourge Heavy Missile x1000\r\n";
        let fit = parse(text);
        assert_eq!(fit.hull, "Drake");
        assert_eq!(fit.lines(Section::Low).len(), 1);
        assert_eq!(fit.count(Section::Med, "Large Shield Extender II"), 1);
        assert_eq!(fit.count(Section::High, "Heavy Missile Launcher II"), 2);
        assert_eq!(
            fit.count(Section::Rigs, "Medium Core Defense Field Extender I"),
            1
        );
        assert_eq!(fit.count(Section::Drones, "Hornet EC-300"), 2);
        assert_eq!(fit.count(Section::Cargo, "Scourge Heavy Missile"), 1000);
    }

    #[test]
    fn single_counted_block_is_cargo() {
        let fit = parse("[Venture, Mining]\n\nMining Laser Upgrade II\n\nNanite Repair Paste x5");
        assert_eq!(fit.count(Section::Low, "Mining Laser Upgrade II"), 1);
        assert_eq!(fit.count(Section::Cargo, "Nanite Repair Paste"), 5);
    }

    #[test]
    fn empty_and_headerless_text() {
        assert!(parse("").is_empty());
        assert!(parse("\n\n  \n").is_empty());
        let fit = parse("Damage Control II");
        assert_eq!(fit.hull, "");
        assert_eq!(fit.count(Section::Low, "Damage Control II"), 1);
    }

    const VEXOR_RACKS: &str = "[Vexor, PvE]\n\nDrone Damage Amplifier II\n\n\
                               10MN Afterburner II\n\nDrone Link Augmentor I\n\n\
                               Medium Auxiliary Nano Pump I";

    #[test]
    fn drones_only_fit_lines_up_with_drones_and_cargo() {
        let drones_only = format!("{VEXOR_RACKS}\n\n\nHammerhead II x5");
        let with_cargo =
            format!("{VEXOR_RACKS}\n\n\nHammerhead II x5\n\n\nNanite Repair Paste x50");
        // Both directions: Should drones-only, then Is drones-only.
        for (is_text, should_text) in [(&with_cargo, &drones_only), (&drones_only, &with_cargo)] {
            let mut is = parse(is_text);
            let mut should = parse(should_text);
            align(&mut is, &mut should);
            let diff = compare(&is, &should);
            assert!(diff.lines(Section::Drones).iter().all(|l| l.change == 0));
            assert_eq!(diff.lines(Section::Drones).len(), 1);
            assert_eq!(diff.missing + diff.surplus, 1);
            let cargo = diff.lines(Section::Cargo);
            assert_eq!(cargo.len(), 1);
            assert_eq!(cargo[0].name, "Nanite Repair Paste");
            assert!(!diff.multibuy().contains("Hammerhead"));
        }
    }

    #[test]
    fn cargo_only_fit_stays_cargo() {
        let cargo_only = format!("{VEXOR_RACKS}\n\n\nNanite Repair Paste x50");
        let with_drones =
            format!("{VEXOR_RACKS}\n\n\nHammerhead II x5\n\n\nNanite Repair Paste x50");
        let mut is = parse(&cargo_only);
        let mut should = parse(&with_drones);
        align(&mut is, &mut should);
        assert_eq!(is.count(Section::Cargo, "Nanite Repair Paste"), 50);
        assert!(is.lines(Section::Drones).is_empty());
        let diff = compare(&is, &should);
        assert_eq!((diff.missing, diff.surplus), (1, 0));
        assert_eq!(diff.multibuy(), "Hammerhead II 5");
    }

    /// Kevin's in-game "Copy to clipboard" of a ship: no blank line after
    /// the header, and the cargo comes before the drones.
    const SALVATION_IN_GAME: &str = "[Salvation, Kearsarge]
Syndicate Damage Control
25000mm Steel Plates II

Capital Micro Jump Drive

Armor Command Burst II
Heavy Energy Neutralizer II

Capital Trimark Armor Pump I
Capital Trimark Armor Pump I




Tracking Speed Script x1
Mobile Depot x2

Gram I x36
Siren I x1
Dromi I x1
Templar I x8";

    const SALVATION_EFT: &str = "[Salvation, Links - Salvation]

Syndicate Damage Control
25000mm Steel Plates II

Capital Micro Jump Drive

Armor Command Burst II
Heavy Energy Neutralizer II

Capital Trimark Armor Pump I
Capital Trimark Armor Pump I



Gram I x12
Gram I x12
Gram I x12
Templar I x6
Templar I x6
Templar I x6
Dromi I x3
Siren I x3




Mid-grade Amulet Alpha x1
Tracking Speed Script x1
Mobile Depot x2";

    fn salvation_drone(name: &str) -> bool {
        ["Gram I", "Siren I", "Dromi I", "Templar I"].contains(&name)
    }

    #[test]
    fn in_game_copy_puts_cargo_first() {
        let is = parse_with_drones(SALVATION_IN_GAME, &salvation_drone);
        assert_eq!(is.hull, "Salvation");
        assert_eq!(is.count(Section::Low, "Syndicate Damage Control"), 1);
        assert_eq!(is.count(Section::Rigs, "Capital Trimark Armor Pump I"), 2);
        assert_eq!(is.count(Section::Cargo, "Mobile Depot"), 2);
        assert_eq!(is.count(Section::Drones, "Templar I"), 8);
        assert_eq!(is.lines(Section::Cargo).len(), 2);

        let should = parse_with_drones(SALVATION_EFT, &salvation_drone);
        assert_eq!(should.count(Section::Drones, "Templar I"), 18);
        assert_eq!(should.lines(Section::Cargo).len(), 3);

        let diff = compare(&is, &should);
        // Drones only differ in numbers, never as missing + surplus pairs.
        let drones = diff.lines(Section::Drones);
        assert_eq!(drones.len(), 4);
        assert!(drones.iter().all(|l| l.change >= 0));
        let gram = drones.iter().find(|l| l.name == "Gram I").unwrap();
        assert_eq!(gram.change, 0);
        let templar = drones.iter().find(|l| l.name == "Templar I").unwrap();
        assert_eq!(templar.change, 10);
        let cargo = diff.lines(Section::Cargo);
        assert_eq!(cargo.len(), 3);
        assert_eq!(cargo[0].name, "Mid-grade Amulet Alpha");
        assert_eq!(cargo[0].change, 1);
        assert_eq!(diff.surplus, 0);
    }

    #[test]
    fn game_data_decides_a_lone_block() {
        let fit = parse_with_drones(
            "[Vexor, X]\n\nDrone Damage Amplifier II\n\n\nHammerhead II x5",
            &|n| n == "Hammerhead II",
        );
        assert_eq!(fit.count(Section::Drones, "Hammerhead II"), 5);
        assert!(!fit.lone_counted_block);
    }

    #[test]
    fn header_without_fit_name() {
        let fit = parse("[Thanatos]\n\nDamage Control II");
        assert_eq!(fit.hull, "Thanatos");
        assert_eq!(fit.name, "");
        assert_eq!(fit.count(Section::Low, "Damage Control II"), 1);
    }

    #[test]
    fn names_ending_in_x_are_not_counts() {
        assert_eq!(
            split_count("Small Tractor Beam I"),
            ("Small Tractor Beam I", None)
        );
        assert_eq!(split_count("Something x"), ("Something x", None));
        assert_eq!(split_count("Hobgoblin II x5"), ("Hobgoblin II", Some(5)));
    }
}
