//! Works out how a colony is doing: red (needs attention), yellow (soon)
//! or green (active), the one-line status the planet row shows, and the
//! numbers its details show.
//!
//! Everything here is plain math on a [`Colony`] and the static data, so
//! it is easy to test. It runs when the data changes or once a minute,
//! never every frame.
//!
//! The rules (thresholds come from the Settings view):
//!
//! - Red: an extractor control unit has no program, an extractor program
//!   has ended, a launchpad or storage is full, or the factories' inputs
//!   have run out.
//! - Yellow: an extractor program ends soon, the factories' inputs run out
//!   soon, factories were idle when the colony was last viewed, or a
//!   launchpad or storage is filling up.
//! - Green: none of the above.
//!
//! What ESI shows is the colony at its last in-game view. From that moment
//! the colony is played forward along its routes (see [`super::sim`]) to
//! estimate how full the launchpads and storage are now, and how long the
//! factories' inputs last.

use std::collections::{BTreeMap, BTreeSet};

use super::config::Thresholds;
use super::data::{Colony, Pin};
use super::sim::{self, Outlook};
use crate::static_data::{PiStructureKind, StaticData};

/// How a colony is doing. The order is the sort order: worst first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Attention,
    Soon,
    Active,
    /// The layout hasn't been fetched yet.
    Unknown,
}

/// What the planet mainly does, for its label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColonyKind {
    Extractor,
    Factory,
    Both,
    /// Only a command center (or nothing Athena knows).
    Empty,
}

impl ColonyKind {
    pub fn label(self) -> &'static str {
        match self {
            ColonyKind::Extractor => "Extractor",
            ColonyKind::Factory => "Factory",
            ColonyKind::Both => "Extract + Factory",
            ColonyKind::Empty => "Empty",
        }
    }
}

/// A launchpad or storage facility.
#[derive(Debug, Clone, PartialEq)]
pub struct Store {
    pub kind: PiStructureKind,
    pub capacity_m3: f64,
    /// Filled at the last in-game view.
    pub used_m3: f64,
    /// Estimated change since then: positive when it filled up, negative
    /// when factories took inputs out of it (0 when not estimated).
    pub added_m3: f64,
    /// (type id, amount), biggest volume first.
    pub contents: Vec<(i64, i64)>,
    /// False for an input or transit store: one that only feeds factories,
    /// or passes on whatever is routed into it. Those are meant to be
    /// full, so they never warn and get no bar in the row.
    pub is_output: bool,
    /// Where an output store is heading: when it gets full, or where it
    /// stops. `None` for input stores.
    pub outlook: Option<Outlook>,
}

impl Store {
    /// Filled now (estimated), in percent of the capacity, at most 100.
    pub fn percent(&self) -> f64 {
        percent(self.used_m3 + self.added_m3, self.capacity_m3)
    }

    /// Filled at the last view, in percent.
    pub fn percent_at_view(&self) -> f64 {
        percent(self.used_m3, self.capacity_m3)
    }
}

fn percent(m3: f64, capacity: f64) -> f64 {
    if capacity <= 0.0 {
        0.0
    } else {
        (m3 / capacity * 100.0).clamp(0.0, 100.0)
    }
}

/// One extractor control unit.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtractorInfo {
    /// `None` when no program is installed.
    pub product: Option<i64>,
    pub heads: u32,
    /// Units per hour at the program's start.
    pub per_hour: f64,
    /// When the program ends (or ended).
    pub expiry: Option<i64>,
}

/// The factories that run one schematic.
#[derive(Debug, Clone, PartialEq)]
pub struct FactoryGroup {
    /// `None` for factories without a schematic.
    pub schematic_id: Option<i64>,
    pub count: usize,
    /// How many were idle at the last view (spare ones not counted).
    pub idle: usize,
    /// How many have no route bringing them anything: set up but not in
    /// use. They never warn.
    pub spare: usize,
}

/// Everything the planet row and its details show.
#[derive(Debug, Clone, PartialEq)]
pub struct Assessment {
    pub level: Level,
    pub kind: ColonyKind,
    /// The one-line status, such as "Extractors end in 14 h".
    pub headline: String,
    pub launchpads: Vec<Store>,
    pub storages: Vec<Store>,
    pub extractors: Vec<ExtractorInfo>,
    pub factories: Vec<FactoryGroup>,
    /// Commodities the factories need that nothing on the planet makes.
    pub inputs: Vec<i64>,
    /// Seconds the inputs still last from now (negative: ran out that long
    /// ago). `None` when the factories need no outside inputs.
    pub inputs_left: Option<i64>,
    /// The inputs last past the simulation's look-ahead: `inputs_left` is
    /// then only "at least".
    pub inputs_past_horizon: bool,
}

impl Assessment {
    /// The stores the row shows as bars: the launchpads, or the storage
    /// facilities on a planet without one.
    /// Only output stores count (see [`Store::is_output`]).
    pub fn bar_stores(&self) -> Vec<&Store> {
        let pads: Vec<&Store> = self.launchpads.iter().filter(|s| s.is_output).collect();
        if pads.is_empty() {
            self.storages.iter().filter(|s| s.is_output).collect()
        } else {
            pads
        }
    }
}

impl Assessment {
    /// What the row says after the percent: "full 14 h" for the bar store
    /// that gets full first, "full 30d+" when one still fills past the
    /// look-ahead, or "stops at 61%" when the fullest one stops below
    /// full. `None` when nothing fills (or a store is full already).
    pub fn row_outlook(&self, now: i64) -> Option<String> {
        let stores = self.bar_stores();
        if let Some(at) = stores
            .iter()
            .filter_map(|s| match s.outlook {
                Some(Outlook::FullAt(at)) => Some(at),
                _ => None,
            })
            .min()
        {
            return (at > now).then(|| format!("full {}", duration_text(at - now)));
        }
        if stores.iter().any(|s| s.outlook == Some(Outlook::Beyond)) {
            return Some(format!("full {}+", duration_text(sim::HORIZON_SECS)));
        }
        stores
            .iter()
            .filter_map(|s| match s.outlook {
                Some(Outlook::StopsAt(m3)) => Some(percent(m3, s.capacity_m3)),
                _ => None,
            })
            .max_by(f64::total_cmp)
            .map(|pct| format!("stops at {pct:.0}%"))
    }
}

/// A factory cycle may start a little after the last one ended.
const IDLE_SLACK_SECS: i64 = 60;

pub fn assess(colony: &Colony, data: &StaticData, t: &Thresholds, now: i64) -> Assessment {
    let Some(layout) = &colony.layout else {
        return Assessment {
            level: Level::Unknown,
            kind: ColonyKind::Empty,
            headline: "Loading the planet's layout…".to_string(),
            launchpads: Vec::new(),
            storages: Vec::new(),
            extractors: Vec::new(),
            factories: Vec::new(),
            inputs: Vec::new(),
            inputs_left: None,
            inputs_past_horizon: false,
        };
    };
    let kind_of = |pin: &Pin| -> Option<PiStructureKind> {
        match data.pi_structure(pin.type_id) {
            Some(structure) => Some(structure.kind),
            // A type the static data doesn't know: guess from the details.
            None if pin.extractor.is_some() => Some(PiStructureKind::Extractor),
            None if pin.schematic_id.is_some() => Some(PiStructureKind::Factory),
            None => None,
        }
    };
    let volume = |type_id: i64| data.pi_commodity(type_id).map_or(0.0, |c| c.volume_m3);

    // ---- Extractors
    let mut extractors = Vec::new();
    for pin in layout.pins.iter() {
        if kind_of(pin) != Some(PiStructureKind::Extractor) {
            continue;
        }
        let info = match (&pin.extractor, pin.expiry) {
            (Some(ex), Some(expiry)) if ex.heads > 0 => ExtractorInfo {
                product: Some(ex.product),
                heads: ex.heads,
                per_hour: ex.qty_per_cycle as f64 * 3600.0 / ex.cycle_secs.max(1) as f64,
                expiry: Some(expiry),
            },
            _ => ExtractorInfo {
                product: None,
                heads: 0,
                per_hour: 0.0,
                expiry: None,
            },
        };
        extractors.push(info);
    }

    // ---- Factories
    let mut groups: BTreeMap<Option<i64>, FactoryGroup> = BTreeMap::new();
    // What the factories make and need per hour.
    let mut made: BTreeSet<i64> = BTreeSet::new();
    let mut needed_per_hour: BTreeMap<i64, f64> = BTreeMap::new();
    for pin in layout.pins.iter() {
        if kind_of(pin) != Some(PiStructureKind::Factory) {
            continue;
        }
        let schematic = pin.schematic_id.and_then(|id| data.pi_schematic(id));
        let idle = match (schematic, pin.last_cycle_start) {
            (Some(s), Some(start)) => start + s.cycle_secs + IDLE_SLACK_SECS < colony.last_update,
            _ => true,
        };
        // A factory no route brings anything to is a spare one (only when
        // the layout has routes at all).
        let spare =
            !layout.routes.is_empty() && !layout.routes.iter().any(|r| r.destination == pin.pin_id);
        let group = groups
            .entry(pin.schematic_id.filter(|_| schematic.is_some()))
            .or_insert(FactoryGroup {
                schematic_id: pin.schematic_id.filter(|_| schematic.is_some()),
                count: 0,
                idle: 0,
                spare: 0,
            });
        group.count += 1;
        if spare {
            group.spare += 1;
            continue;
        }
        if idle {
            group.idle += 1;
        }
        if let Some(s) = schematic {
            if let Some((output, _)) = s.output {
                made.insert(output);
            }
            for &(input, quantity) in &s.inputs {
                *needed_per_hour.entry(input).or_default() +=
                    quantity as f64 * 3600.0 / s.cycle_secs.max(1) as f64;
            }
        }
    }
    let factories: Vec<FactoryGroup> = groups.into_values().collect();
    let factory_count: usize = factories.iter().map(|g| g.count).sum();
    let idle_count: usize = factories.iter().map(|g| g.idle).sum();
    // Factories in use: spare ones aren't counted in the texts.
    let working_count = factory_count - factories.iter().map(|g| g.spare).sum::<usize>();

    // ---- Launchpads and storage
    let mut stores: Vec<(i64, Store)> = Vec::new();
    for pin in layout.pins.iter() {
        let kind = kind_of(pin);
        if !matches!(
            kind,
            Some(PiStructureKind::Launchpad) | Some(PiStructureKind::Storage)
        ) {
            continue;
        }
        let mut contents = pin.contents.clone();
        contents.sort_by(|a, b| {
            let va = a.1 as f64 * volume(a.0);
            let vb = b.1 as f64 * volume(b.0);
            vb.total_cmp(&va)
        });
        // What routes bring into this store, and what they take out. A
        // commodity that comes in and isn't routed out again stays here:
        // that makes it an output store. One that only sends things out
        // (factory inputs), or passes on what it gets (transit), is meant
        // to be full. A planet without any routes (none fetched, or an
        // old layout) counts every store as output, as before.
        let incoming: BTreeSet<i64> = layout
            .routes
            .iter()
            .filter(|r| r.destination == pin.pin_id)
            .map(|r| r.type_id)
            .collect();
        let outgoing: BTreeSet<i64> = layout
            .routes
            .iter()
            .filter(|r| r.source == pin.pin_id)
            .map(|r| r.type_id)
            .collect();
        let is_output = layout.routes.is_empty() || incoming.iter().any(|t| !outgoing.contains(t));
        stores.push((
            pin.pin_id,
            Store {
                is_output,
                kind: kind.unwrap_or(PiStructureKind::Storage),
                capacity_m3: data
                    .pi_structure(pin.type_id)
                    .map_or(0.0, |s| s.capacity_m3),
                used_m3: contents.iter().map(|(id, n)| *n as f64 * volume(*id)).sum(),
                added_m3: 0.0,
                outlook: None,
                contents,
            },
        ));
    }

    // ---- Commodities the factories need from outside the planet.
    let extracted: BTreeSet<i64> = extractors.iter().filter_map(|e| e.product).collect();
    let inputs: Vec<i64> = needed_per_hour
        .keys()
        .copied()
        .filter(|id| !made.contains(id) && !extracted.contains(id))
        .collect();

    // ---- Play the colony forward from the last view (see `sim`): what
    // the extractors and factories have put into (or taken out of) the
    // launchpads and storage since, and when the outside inputs run out.
    let kinds: BTreeMap<i64, PiStructureKind> = layout
        .pins
        .iter()
        .filter_map(|p| Some((p.pin_id, kind_of(p)?)))
        .collect();
    let outside: BTreeSet<i64> = inputs.iter().copied().collect();
    let watch: BTreeSet<i64> = stores
        .iter()
        .filter(|(_, s)| s.is_output)
        .map(|(id, _)| *id)
        .collect();
    let played = sim::simulate(
        layout,
        data,
        &kinds,
        &outside,
        &watch,
        colony.last_update,
        now.max(colony.last_update),
    );
    if t.show_estimate {
        for (pin_id, store) in stores.iter_mut() {
            if let Some(now_m3) = played.used_now_m3.get(pin_id) {
                let change = now_m3 - store.used_m3;
                // Sums in another order can differ in the last digits.
                store.added_m3 = if change.abs() < 1e-6 { 0.0 } else { change };
            }
        }
    }
    for (pin_id, store) in stores.iter_mut() {
        store.outlook = played.outlook.get(pin_id).copied();
    }
    // Seconds from now until the inputs run out (negative: that long ago).
    // If they last past the simulation's horizon, they last at least that.
    let inputs_left = if inputs.is_empty() {
        None
    } else {
        Some(played.starved_at.unwrap_or(played.horizon) - now)
    };
    let inputs_past_horizon = inputs_left.is_some() && played.starved_at.is_none();
    // "~5d 2h", or "30d+" past the look-ahead.
    let inputs_text = |left: i64| {
        if inputs_past_horizon {
            format!("{}+", duration_text(left))
        } else {
            format!("~{}", duration_text(left))
        }
    };

    // ---- Problems, worst first.
    let mut issues: Vec<(Level, String)> = Vec::new();
    let no_program = extractors.iter().filter(|e| e.product.is_none()).count();
    if no_program > 0 {
        issues.push((
            Level::Attention,
            "No extractor program installed".to_string(),
        ));
    }
    let running: Vec<i64> = extractors
        .iter()
        .filter_map(|e| e.expiry)
        .filter(|e| *e > now)
        .collect();
    if let Some(ended) = extractors
        .iter()
        .filter_map(|e| e.expiry)
        .filter(|e| *e <= now)
        .max()
    {
        issues.push((
            Level::Attention,
            format!("Extractors stopped {} ago", duration_text(now - ended)),
        ));
    }
    let ends = running.iter().min().copied();
    if let Some(end) = ends
        && end - now < t.extractor_soon_hours as i64 * 3600
    {
        issues.push((
            Level::Soon,
            format!("Extractors end in {}", duration_text(end - now)),
        ));
    }
    let inputs_empty = inputs_left.is_some_and(|left| left <= 0);
    if inputs_empty {
        let text = if idle_count > 0 {
            format!("{idle_count} of {working_count} factories idle · inputs empty")
        } else {
            "Factory inputs ran out".to_string()
        };
        issues.push((Level::Attention, text));
    } else {
        if let Some(left) = inputs_left
            && left < t.factory_soon_hours as i64 * 3600
        {
            issues.push((Level::Soon, format!("Inputs last {}", inputs_text(left))));
        }
        // Basic factories fed by a running extractor are often idle when
        // you look, waiting for the next batch. That's normal; idle
        // factories only matter when they live on outside inputs or no
        // extractor is running any more.
        let idle_matters = !inputs.is_empty() || running.is_empty();
        if idle_count > 0 && idle_matters {
            issues.push((
                Level::Soon,
                format!("{idle_count} of {working_count} factories idle at last view"),
            ));
        }
    }
    let fullest = stores
        .iter()
        .map(|(_, s)| s)
        .filter(|s| s.is_output)
        .max_by(|a, b| a.percent().total_cmp(&b.percent()));
    // The output store that gets full first, if that is soon.
    let overflow = stores
        .iter()
        .map(|(_, s)| s)
        .filter(|s| s.is_output && s.percent() < t.full_percent as f64)
        .filter_map(|s| match s.outlook {
            Some(Outlook::FullAt(at)) if at > now => Some((at, s)),
            _ => None,
        })
        .min_by_key(|(at, _)| *at)
        .filter(|(at, _)| t.overflow_warning && at - now <= t.overflow_hours as i64 * 3600);
    if let Some(store) = fullest {
        let name = store_name(&stores, store);
        let pct = store.percent();
        if pct >= t.full_percent as f64 {
            issues.push((Level::Attention, format!("{name} full")));
        } else if let Some((at, soon)) = overflow {
            // Says more than "at 80%", so it replaces that.
            let name = store_name(&stores, soon);
            issues.push((
                Level::Soon,
                format!("{name} full in ~{}", duration_text(at - now)),
            ));
        } else if pct >= t.warn_percent as f64 {
            issues.push((Level::Soon, format!("{name} at {pct:.0}%")));
        }
    }

    let kind = match (!extractors.is_empty(), factory_count > 0) {
        (true, true) => ColonyKind::Both,
        (true, false) => ColonyKind::Extractor,
        (false, true) => ColonyKind::Factory,
        (false, false) => ColonyKind::Empty,
    };
    if kind == ColonyKind::Empty {
        issues.push((Level::Soon, "No extractors or factories".to_string()));
    }

    // The worst problem names the row; the first one of that level wins.
    let worst = issues.iter().map(|(level, _)| *level).min();
    let (level, headline) = match worst {
        Some(level) => {
            let text = issues
                .iter()
                .find(|(l, _)| *l == level)
                .map(|(_, text)| text.clone())
                .unwrap_or_default();
            (level, text)
        }
        None => {
            let mut parts = Vec::new();
            if let Some(end) = ends {
                parts.push(format!(
                    "{} extractor{} · end in {}",
                    running.len(),
                    if running.len() == 1 { "" } else { "s" },
                    duration_text(end - now)
                ));
            }
            if working_count > 0 {
                let noun = if working_count == 1 {
                    "factory"
                } else {
                    "factories"
                };
                let mut text = format!("{working_count} {noun} running");
                if let Some(left) = inputs_left {
                    text.push_str(&format!(" · inputs last {}", inputs_text(left)));
                }
                parts.push(text);
            }
            (Level::Active, parts.join(" · "))
        }
    };

    let (launchpads, storages): (Vec<Store>, Vec<Store>) = stores
        .into_iter()
        .map(|(_, s)| s)
        .partition(|s| s.kind == PiStructureKind::Launchpad);
    Assessment {
        level,
        kind,
        headline,
        launchpads,
        storages,
        extractors,
        factories,
        inputs,
        inputs_left,
        inputs_past_horizon,
    }
}

/// "Launchpad", "Launchpad 2", "Storage" or "Storage 3": numbered when the
/// planet has more than one of that kind.
fn store_name(stores: &[(i64, Store)], store: &Store) -> String {
    let same: Vec<&Store> = stores
        .iter()
        .map(|(_, s)| s)
        .filter(|s| s.kind == store.kind)
        .collect();
    let base = if store.kind == PiStructureKind::Launchpad {
        "Launchpad"
    } else {
        "Storage"
    };
    if same.len() <= 1 {
        return base.to_string();
    }
    let index = same
        .iter()
        .position(|s| std::ptr::eq(*s, store))
        .unwrap_or(0);
    format!("{base} {}", index + 1)
}

/// "45 min", "14 h", "2d 6h".
pub fn duration_text(seconds: i64) -> String {
    let seconds = seconds.max(0);
    let minutes = seconds / 60;
    let hours = seconds / 3600;
    if minutes < 60 {
        format!("{} min", minutes.max(1))
    } else if hours < 24 {
        format!("{hours} h")
    } else {
        let days = hours / 24;
        let rest = hours % 24;
        if rest == 0 {
            format!("{days}d")
        } else {
            format!("{days}d {rest}h")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::planetary::data::{Extractor, Layout, Route};
    use crate::static_data::tests::sample;

    const NOW: i64 = 1_794_500_000;
    // Type ids from the sample static data.
    const ECU: i64 = 3068;
    const BASIC: i64 = 2481;
    const ADVANCED: i64 = 2480;
    const PAD: i64 = 2256;
    const AQUEOUS: i64 = 2268;
    const WATER: i64 = 3645;
    const PLASMOIDS: i64 = 2389;

    fn colony(pins: Vec<Pin>, routes: Vec<Route>, viewed: i64) -> Colony {
        Colony {
            character_id: 1,
            planet_id: 40_000_002,
            system_id: 30_000_001,
            planet_type: "temperate".into(),
            upgrade_level: 4,
            num_pins: pins.len() as i64,
            last_update: viewed,
            layout: Some(Layout { pins, routes }),
        }
    }

    fn ecu(pin_id: i64, expiry: i64) -> Pin {
        Pin {
            pin_id,
            type_id: ECU,
            expiry: Some(expiry),
            install: Some(NOW - 2 * 86_400),
            extractor: Some(Extractor {
                cycle_secs: 1800,
                heads: 10,
                product: AQUEOUS,
                qty_per_cycle: 10_000,
            }),
            ..Pin::default()
        }
    }

    fn pad(pin_id: i64, contents: Vec<(i64, i64)>) -> Pin {
        Pin {
            pin_id,
            type_id: PAD,
            contents,
            ..Pin::default()
        }
    }

    fn factory(pin_id: i64, type_id: i64, schematic: i64, last_cycle_start: i64) -> Pin {
        Pin {
            pin_id,
            type_id,
            schematic_id: Some(schematic),
            last_cycle_start: Some(last_cycle_start),
            ..Pin::default()
        }
    }

    fn defaults() -> Thresholds {
        Thresholds::default()
    }

    #[test]
    fn running_extractors_are_active() {
        let c = colony(
            vec![
                ecu(1, NOW + 2 * 86_400 + 6 * 3600),
                ecu(2, NOW + 3 * 86_400),
                pad(3, vec![]),
            ],
            vec![],
            NOW - 3600,
        );
        let a = assess(&c, &sample(), &defaults(), NOW);
        assert_eq!(a.level, Level::Active);
        assert_eq!(a.kind, ColonyKind::Extractor);
        assert_eq!(a.headline, "2 extractors · end in 2d 6h");
    }

    #[test]
    fn ended_and_ending_extractors() {
        let data = sample();
        let c = colony(vec![ecu(1, NOW + 14 * 3600)], vec![], NOW - 3600);
        let a = assess(&c, &data, &defaults(), NOW);
        assert_eq!(a.level, Level::Soon);
        assert_eq!(a.headline, "Extractors end in 14 h");

        let c = colony(vec![ecu(1, NOW - 3 * 3600)], vec![], NOW - 86_400);
        let a = assess(&c, &data, &defaults(), NOW);
        assert_eq!(a.level, Level::Attention);
        assert_eq!(a.headline, "Extractors stopped 3 h ago");
    }

    #[test]
    fn extractor_without_program_needs_attention() {
        let mut empty = ecu(1, NOW);
        empty.extractor = None;
        empty.expiry = None;
        let a = assess(
            &colony(vec![empty], vec![], NOW),
            &sample(),
            &defaults(),
            NOW,
        );
        assert_eq!(a.level, Level::Attention);
        assert_eq!(a.headline, "No extractor program installed");
    }

    #[test]
    fn launchpad_fill_and_estimate() {
        let data = sample();
        // 1,600,000 Aqueous Liquids at 0.005 m³ = 8,000 m³ of 10,000.
        let pins = vec![ecu(1, NOW + 5 * 86_400), pad(2, vec![(AQUEOUS, 1_600_000)])];
        let routes = vec![Route {
            source: 1,
            destination: 2,
            type_id: AQUEOUS,
            quantity: 10_000.0,
        }];
        let c = colony(pins, routes, NOW);
        let a = assess(&c, &data, &defaults(), NOW);
        assert_eq!(a.launchpads[0].used_m3, 8000.0);
        assert_eq!(a.level, Level::Soon);
        assert_eq!(a.headline, "Launchpad at 80%");

        // Viewed 10 hours ago: 20 cycles x 10,000 units x 0.005 m³ = 1,000
        // m³ more, which makes it 90%.
        let mut c = c;
        c.last_update = NOW - 10 * 3600;
        let a = assess(&c, &data, &defaults(), NOW);
        assert_eq!(a.launchpads[0].added_m3, 1000.0);
        assert!((a.launchpads[0].percent() - 90.0).abs() < 1e-9);

        // Without the estimate it stays at 80%.
        let no_estimate = Thresholds {
            show_estimate: false,
            ..defaults()
        };
        let a = assess(&c, &data, &no_estimate, NOW);
        assert_eq!(a.launchpads[0].added_m3, 0.0);

        // Two days later the pad is full (capped at its capacity).
        let a = assess(&c, &data, &defaults(), NOW + 2 * 86_400);
        assert_eq!(a.level, Level::Attention);
        assert_eq!(a.headline, "Launchpad full");
        assert_eq!(a.launchpads[0].percent(), 100.0);
    }

    #[test]
    fn the_fullest_of_several_pads_names_the_row() {
        let pins = vec![
            ecu(1, NOW + 5 * 86_400),
            pad(2, vec![(AQUEOUS, 200_000)]),
            pad(3, vec![(AQUEOUS, 1_580_000)]),
            pad(4, vec![]),
        ];
        let a = assess(&colony(pins, vec![], NOW), &sample(), &defaults(), NOW);
        assert_eq!(a.launchpads.len(), 3);
        assert_eq!(a.headline, "Launchpad 2 at 79%");
    }

    #[test]
    fn factory_inputs_last_and_run_out() {
        let data = sample();
        // Superconductors: 40 Plasmoids + 40 Water per hour per factory.
        // Two factories need 80 of each per hour; 800 Water lasts 10 more
        // cycles. Both are in a cycle at the view that ends 50 min later,
        // so the 11th start fails 10 h 50 min from now.
        let pins = vec![
            factory(1, ADVANCED, 65, NOW - 600),
            factory(2, ADVANCED, 65, NOW - 600),
            pad(3, vec![(PLASMOIDS, 4000), (WATER, 800)]),
        ];
        let c = colony(pins, vec![], NOW);
        let a = assess(&c, &data, &defaults(), NOW);
        assert_eq!(a.kind, ColonyKind::Factory);
        assert_eq!(a.inputs, vec![PLASMOIDS, WATER]);
        assert_eq!(a.inputs_left, Some(10 * 3600 + 3000));
        assert_eq!(a.level, Level::Soon);
        assert_eq!(a.headline, "Inputs last ~10 h");

        // 11 hours later they're gone.
        let a = assess(&c, &data, &defaults(), NOW + 11 * 3600);
        assert_eq!(a.level, Level::Attention);
        assert_eq!(a.headline, "Factory inputs ran out");
    }

    #[test]
    fn idle_factories_are_counted_at_the_last_view() {
        let data = sample();
        let pins = vec![
            factory(1, ADVANCED, 65, NOW - 600),
            // Last cycle started 3 hours before the view: idle.
            factory(2, ADVANCED, 65, NOW - 3 * 3600),
            pad(3, vec![(PLASMOIDS, 10_000), (WATER, 10_000)]),
        ];
        let a = assess(&colony(pins, vec![], NOW), &data, &defaults(), NOW);
        assert_eq!(a.level, Level::Soon);
        assert_eq!(a.headline, "1 of 2 factories idle at last view");
        assert_eq!(a.factories[0].idle, 1);
    }

    #[test]
    fn extracted_inputs_are_not_outside_inputs() {
        let data = sample();
        // Extractor feeds a Water factory: no outside inputs. One basic
        // factory turns 3,000 Aqueous Liquids into 20 Water every 30 min;
        // the extractor brings 10,000, so most stays as Aqueous Liquids.
        let viewed = NOW - 10 * 3600;
        let pins = vec![
            ecu(1, NOW + 5 * 86_400),
            // In a cycle at the view that ends 20 min later.
            factory(2, BASIC, 121, viewed - 600),
            pad(3, vec![]),
        ];
        let c = colony(pins, vec![], viewed);
        let a = assess(&c, &data, &defaults(), NOW);
        assert_eq!(a.kind, ColonyKind::Both);
        assert!(a.inputs.is_empty());
        assert_eq!(a.inputs_left, None);
        // 20 extractor cycles bring 200,000; 20 factory cycles start and
        // take 60,000 of it. 20 cycles end: the one running at the view
        // and 19 of the new ones, 400 Water.
        let expected = 140_000.0 * 0.005 + 400.0 * 0.19;
        assert!((a.launchpads[0].added_m3 - expected).abs() < 1e-6);
        assert_eq!(a.level, Level::Active);
        assert_eq!(a.headline, "1 extractor · end in 5d · 1 factory running");
    }

    #[test]
    fn idle_basic_factory_with_running_extractor_is_fine() {
        let data = sample();
        // The Water factory's last cycle ended long before the view: it
        // waits for the next extractor batch.
        let pins = vec![
            ecu(1, NOW + 5 * 86_400),
            factory(2, BASIC, 121, NOW - 5 * 3600),
            pad(3, vec![]),
        ];
        let a = assess(&colony(pins, vec![], NOW), &data, &defaults(), NOW);
        assert_eq!(a.factories[0].idle, 1);
        assert_eq!(a.level, Level::Active);

        // Once the extractor has stopped, the idle factory does count (the
        // stopped extractor is the headline, as the worse problem).
        let pins = vec![
            ecu(1, NOW - 3600),
            factory(2, BASIC, 121, NOW - 5 * 3600),
            pad(3, vec![]),
        ];
        let a = assess(
            &colony(pins, vec![], NOW - 2 * 3600),
            &data,
            &defaults(),
            NOW,
        );
        assert_eq!(a.level, Level::Attention);
        assert_eq!(a.headline, "Extractors stopped 1 h ago");
    }

    #[test]
    fn full_input_pads_do_not_warn() {
        let data = sample();
        // Pad 3 feeds the factories (input), pad 4 gets their output.
        let pins = vec![
            factory(1, ADVANCED, 65, NOW - 600),
            factory(2, ADVANCED, 65, NOW - 600),
            pad(3, vec![(PLASMOIDS, 26_000), (WATER, 26_000)]),
            pad(4, vec![(9838, 1000)]),
        ];
        let route = |source: i64, destination: i64, type_id: i64| Route {
            source,
            destination,
            type_id,
            quantity: 40.0,
        };
        let routes = vec![
            route(3, 1, PLASMOIDS),
            route(3, 1, WATER),
            route(3, 2, PLASMOIDS),
            route(3, 2, WATER),
            route(1, 4, 9838),
            route(2, 4, 9838),
        ];
        let a = assess(&colony(pins, routes, NOW), &data, &defaults(), NOW);
        assert!(!a.launchpads[0].is_output);
        assert!(a.launchpads[0].percent() > 95.0);
        assert!(a.launchpads[1].is_output);
        // Inputs last 26,000 / 80 per hour = 325 h: all fine.
        assert_eq!(a.level, Level::Active);
        // Only the output pad gets a bar.
        assert_eq!(a.bar_stores().len(), 1);

        // A transit pad (Water in, Water out) doesn't warn either; the
        // output pad filling up does.
        let pins = vec![
            factory(1, ADVANCED, 65, NOW - 600),
            pad(3, vec![(WATER, 26_000), (PLASMOIDS, 26_000)]),
            pad(4, vec![(9838, 12_800)]),
        ];
        let routes = vec![
            route(9, 3, WATER),
            route(3, 1, WATER),
            route(9, 3, PLASMOIDS),
            route(3, 1, PLASMOIDS),
            route(1, 4, 9838),
        ];
        let a = assess(&colony(pins, routes, NOW), &data, &defaults(), NOW);
        assert!(!a.launchpads[0].is_output);
        assert_eq!(a.level, Level::Attention);
        assert_eq!(a.headline, "Launchpad 2 full");
    }

    fn route(source: i64, destination: i64, type_id: i64) -> Route {
        Route {
            source,
            destination,
            type_id,
            quantity: 40.0,
        }
    }

    /// Two Superconductor factories take from pad 3 and send to pad 4.
    fn factory_planet(
        input_pad: Vec<(i64, i64)>,
        output_pad: Vec<(i64, i64)>,
    ) -> (Vec<Pin>, Vec<Route>) {
        let pins = vec![
            factory(1, ADVANCED, 65, NOW - 600),
            factory(2, ADVANCED, 65, NOW - 600),
            pad(3, input_pad),
            pad(4, output_pad),
        ];
        let routes = vec![
            route(3, 1, PLASMOIDS),
            route(3, 1, WATER),
            route(3, 2, PLASMOIDS),
            route(3, 2, WATER),
            route(1, 4, 9838),
            route(2, 4, 9838),
        ];
        (pins, routes)
    }

    #[test]
    fn factory_planet_pads_follow_the_routes() {
        let data = sample();
        let (pins, routes) = factory_planet(
            vec![(PLASMOIDS, 26_000), (WATER, 26_000)],
            vec![(9838, 1000)],
        );
        let c = colony(pins, routes, NOW);
        // 10 hours after the view: each factory finished the cycle it was
        // in plus 9 more (10 x 5 Superconductors), and started 10 (10 x 40
        // Plasmoids and 40 Water).
        let a = assess(&c, &data, &defaults(), NOW + 10 * 3600);
        let made = 2.0 * 10.0 * 5.0 * 0.75;
        let used = 2.0 * 10.0 * 80.0 * 0.19;
        assert!((a.launchpads[1].added_m3 - made).abs() < 1e-6);
        assert!((a.launchpads[0].added_m3 + used).abs() < 1e-6);
        // 26,000 Water lasts 325 cycles of both: 13d 13h 50min after the
        // view, so 13d 3h from 10 hours later.
        assert_eq!(a.level, Level::Active);
        assert_eq!(a.headline, "2 factories running · inputs last ~13d 3h");
    }

    #[test]
    fn factory_output_pad_fills_up() {
        let data = sample();
        // 12,600 Superconductors = 9,450 m³ of 10,000: 94.5% at the view.
        let (pins, routes) = factory_planet(
            vec![(PLASMOIDS, 26_000), (WATER, 26_000)],
            vec![(9838, 12_600)],
        );
        let c = colony(pins, routes, NOW);
        let a = assess(&c, &data, &defaults(), NOW);
        assert_eq!(a.level, Level::Soon);
        // 10 hours later 75 m³ more: full.
        let a = assess(&c, &data, &defaults(), NOW + 10 * 3600);
        assert_eq!(a.level, Level::Attention);
        assert_eq!(a.headline, "Launchpad 2 full");
        // Never more than fits.
        let a = assess(&c, &data, &defaults(), NOW + 20 * 86_400);
        assert_eq!(a.launchpads[1].percent(), 100.0);
    }

    #[test]
    fn inputs_without_a_route_do_not_count() {
        let data = sample();
        // Water is on pad 4, but only Plasmoids are routed from pad 3. The
        // old estimate counted all Water on the planet.
        let pins = vec![
            factory(1, ADVANCED, 65, NOW - 3600),
            pad(3, vec![(PLASMOIDS, 10_000)]),
            pad(4, vec![(WATER, 10_000)]),
        ];
        let routes = vec![route(3, 1, PLASMOIDS)];
        let a = assess(&colony(pins, routes, NOW - 600), &data, &defaults(), NOW);
        assert_eq!(a.inputs_left, Some(0));
        assert_eq!(a.level, Level::Attention);
    }

    #[test]
    fn factories_feed_each_other_before_the_pad() {
        let data = sample();
        // An extractor feeds two basic Water factories through storage 5;
        // the Water goes to the Superconductor factory first, the rest to
        // the launchpad.
        let viewed = NOW - 2 * 3600;
        let pins = vec![
            ecu(1, NOW + 5 * 86_400),
            factory(2, BASIC, 121, viewed - 1800),
            factory(3, BASIC, 121, viewed - 1800),
            factory(4, ADVANCED, 65, viewed - 3600),
            pad(6, vec![(PLASMOIDS, 1000)]),
            Pin {
                pin_id: 5,
                type_id: 2562,
                ..Pin::default()
            },
        ];
        let routes = vec![
            route(1, 5, AQUEOUS),
            route(5, 2, AQUEOUS),
            route(5, 3, AQUEOUS),
            route(2, 4, WATER),
            route(3, 4, WATER),
            route(2, 6, WATER),
            route(3, 6, WATER),
            route(6, 4, PLASMOIDS),
            route(4, 6, 9838),
        ];
        let a = assess(&colony(pins, routes, viewed), &data, &defaults(), NOW);
        assert_eq!(a.inputs, vec![PLASMOIDS]);
        // At the view all factories are idle and the advanced one takes
        // 40 Plasmoids. Extractor cycles end 30, 60, 90 and 120 min later:
        // - 30 min: both basic factories take 3,000 Aqueous and start.
        // - 60 min: their 2 x 20 Water go to the advanced one, which starts.
        // - 90 min: 40 Water again fill the advanced one's buffer.
        // - 120 min: its buffer is full, so this Water (40) goes to the
        //   pad, the advanced cycle ends (5 Superconductors to the pad)
        //   and it takes 40 more Plasmoids.
        let pad = &a.launchpads[0];
        let expected = 5.0 * 0.75 + 40.0 * 0.19 - 80.0 * 0.19;
        assert!(
            (pad.added_m3 - expected).abs() < 1e-6,
            "added {}",
            pad.added_m3
        );
    }

    /// An extractor with 5x the yield of `ecu`: 250 m³ per 30 min.
    fn big_ecu(pin_id: i64, expiry: i64) -> Pin {
        let mut pin = ecu(pin_id, expiry);
        if let Some(ex) = pin.extractor.as_mut() {
            ex.qty_per_cycle = 50_000;
        }
        pin
    }

    #[test]
    fn overflow_warning_before_the_pad_is_full() {
        let data = sample();
        // 6,500 of 10,000 m³ (65%), 500 m³ per hour more: full in 7 h.
        let pins = vec![
            big_ecu(1, NOW + 5 * 86_400),
            pad(2, vec![(AQUEOUS, 1_300_000)]),
        ];
        let routes = vec![route(1, 2, AQUEOUS)];
        let c = colony(pins, routes, NOW);
        let a = assess(&c, &data, &defaults(), NOW);
        assert_eq!(
            a.launchpads[0].outlook,
            Some(Outlook::FullAt(NOW + 7 * 3600))
        );
        assert_eq!(a.row_outlook(NOW).as_deref(), Some("full 7 h"));
        assert_eq!(a.level, Level::Soon);
        assert_eq!(a.headline, "Launchpad full in ~7 h");

        // Outside the warning time, or switched off: no warning, but the
        // row still says when.
        for t in [
            Thresholds {
                overflow_hours: 6,
                ..defaults()
            },
            Thresholds {
                overflow_warning: false,
                ..defaults()
            },
        ] {
            let a = assess(&c, &data, &t, NOW);
            assert_eq!(a.level, Level::Active);
            assert_eq!(a.row_outlook(NOW).as_deref(), Some("full 7 h"));
        }
    }

    #[test]
    fn pad_stops_filling_when_the_extractor_ends() {
        let data = sample();
        // 4 more cycles of 50 m³ before the program ends: 10% -> 12%.
        let pins = vec![ecu(1, NOW + 2 * 3600), pad(2, vec![(AQUEOUS, 200_000)])];
        let routes = vec![route(1, 2, AQUEOUS)];
        let a = assess(&colony(pins, routes, NOW), &data, &defaults(), NOW);
        assert_eq!(a.launchpads[0].outlook, Some(Outlook::StopsAt(1200.0)));
        assert_eq!(a.row_outlook(NOW).as_deref(), Some("stops at 12%"));
    }

    #[test]
    fn inputs_and_pads_past_the_look_ahead() {
        let data = sample();
        // 100,000 Water lasts 1,250 h: past the 30-day look-ahead, and the
        // output pad (7.5 m³ an hour) isn't full by then either.
        let (pins, routes) = factory_planet(
            vec![(PLASMOIDS, 100_000), (WATER, 100_000)],
            vec![(9838, 1000)],
        );
        let a = assess(&colony(pins, routes, NOW), &data, &defaults(), NOW);
        assert!(a.inputs_past_horizon);
        assert_eq!(a.headline, "2 factories running · inputs last 30d+");
        assert_eq!(a.launchpads[1].outlook, Some(Outlook::Beyond));
        assert_eq!(a.row_outlook(NOW).as_deref(), Some("full 30d+"));
        // The input pad isn't watched.
        assert_eq!(a.launchpads[0].outlook, None);
    }

    #[test]
    fn spare_factory_without_routes_does_not_warn() {
        let data = sample();
        let (mut pins, routes) = factory_planet(
            vec![(PLASMOIDS, 26_000), (WATER, 26_000)],
            vec![(9838, 1000)],
        );
        // A third Superconductor factory nothing is routed to, idle.
        pins.push(factory(5, ADVANCED, 65, NOW - 30 * 86_400));
        let a = assess(&colony(pins, routes, NOW), &data, &defaults(), NOW);
        assert_eq!(a.level, Level::Active);
        assert_eq!(a.factories[0].count, 3);
        assert_eq!(a.factories[0].spare, 1);
        assert_eq!(a.factories[0].idle, 0);
        assert!(a.headline.starts_with("2 factories running"));
    }

    #[test]
    fn no_layout_yet() {
        let mut c = colony(vec![], vec![], NOW);
        c.layout = None;
        let a = assess(&c, &sample(), &defaults(), NOW);
        assert_eq!(a.level, Level::Unknown);
    }

    #[test]
    fn durations() {
        assert_eq!(duration_text(30), "1 min");
        assert_eq!(duration_text(45 * 60), "45 min");
        assert_eq!(duration_text(14 * 3600 + 59 * 60), "14 h");
        assert_eq!(duration_text(2 * 86_400 + 6 * 3600), "2d 6h");
        assert_eq!(duration_text(5 * 86_400), "5d");
    }
}
