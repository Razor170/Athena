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
//! What ESI shows is the colony at its last in-game view. Two things are
//! estimated from that moment to now: how much the extractors have added
//! to storage, and how long the factories' inputs last.

use std::collections::{BTreeMap, BTreeSet};

use super::config::Thresholds;
use super::data::{Colony, Pin};
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
    /// Estimated to have been added since then (0 when not estimated).
    pub added_m3: f64,
    /// (type id, amount), biggest volume first.
    pub contents: Vec<(i64, i64)>,
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
    /// How many were idle at the last view.
    pub idle: usize,
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
}

impl Assessment {
    /// The stores the row shows as bars: the launchpads, or the storage
    /// facilities on a planet without one.
    pub fn bar_stores(&self) -> &[Store] {
        if self.launchpads.is_empty() {
            &self.storages
        } else {
            &self.launchpads
        }
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
        let group = groups
            .entry(pin.schematic_id.filter(|_| schematic.is_some()))
            .or_insert(FactoryGroup {
                schematic_id: pin.schematic_id.filter(|_| schematic.is_some()),
                count: 0,
                idle: 0,
            });
        group.count += 1;
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
        stores.push((
            pin.pin_id,
            Store {
                kind: kind.unwrap_or(PiStructureKind::Storage),
                capacity_m3: data
                    .pi_structure(pin.type_id)
                    .map_or(0.0, |s| s.capacity_m3),
                used_m3: contents.iter().map(|(id, n)| *n as f64 * volume(*id)).sum(),
                added_m3: 0.0,
                contents,
            },
        ));
    }

    // ---- Estimate what the extractors added since the last view.
    if t.show_estimate {
        for pin in layout.pins.iter() {
            let (Some(ex), Some(expiry)) = (&pin.extractor, pin.expiry) else {
                continue;
            };
            let from = colony.last_update.max(pin.install.unwrap_or(i64::MIN));
            let to = now.min(expiry);
            if to <= from || ex.heads == 0 {
                continue;
            }
            let units = ((to - from) / ex.cycle_secs.max(1)) as f64 * ex.qty_per_cycle as f64;
            // If a factory here turns the raw resource into something,
            // what lands in storage is the factory's output.
            let converted = layout.pins.iter().find_map(|p| {
                let s = data.pi_schematic(p.schematic_id?)?;
                let (_, in_qty) = s.inputs.iter().find(|(id, _)| *id == ex.product)?;
                let (out_id, out_qty) = s.output?;
                Some((out_id, units * out_qty as f64 / (*in_qty).max(1) as f64))
            });
            let (type_id, amount) = converted.unwrap_or((ex.product, units));
            let added = amount * volume(type_id);
            // Into the store a route sends this commodity to, else the
            // first launchpad, else the first storage.
            let routed = layout.routes.iter().find_map(|r| {
                (r.type_id == type_id)
                    .then(|| stores.iter().position(|(id, _)| *id == r.destination))
                    .flatten()
            });
            let target = routed
                .or_else(|| {
                    stores
                        .iter()
                        .position(|(_, s)| s.kind == PiStructureKind::Launchpad)
                })
                .or(if stores.is_empty() { None } else { Some(0) });
            if let Some(index) = target {
                let store = &mut stores[index].1;
                let room = (store.capacity_m3 - store.used_m3 - store.added_m3).max(0.0);
                store.added_m3 += added.min(room);
            }
        }
    }

    // ---- How long the factories' outside inputs last.
    let extracted: BTreeSet<i64> = extractors.iter().filter_map(|e| e.product).collect();
    let inputs: Vec<i64> = needed_per_hour
        .keys()
        .copied()
        .filter(|id| !made.contains(id) && !extracted.contains(id))
        .collect();
    let inputs_left = if inputs.is_empty() {
        None
    } else {
        let stock = |type_id: i64| -> f64 {
            layout
                .pins
                .iter()
                .flat_map(|p| p.contents.iter())
                .filter(|(id, _)| *id == type_id)
                .map(|(_, n)| *n as f64)
                .sum()
        };
        let hours = inputs
            .iter()
            .map(|id| stock(*id) / needed_per_hour[id].max(f64::MIN_POSITIVE))
            .fold(f64::INFINITY, f64::min);
        let at_view = (hours * 3600.0) as i64;
        Some(at_view - (now - colony.last_update))
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
            format!("{idle_count} of {factory_count} factories idle · inputs empty")
        } else {
            "Factory inputs ran out".to_string()
        };
        issues.push((Level::Attention, text));
    } else {
        if let Some(left) = inputs_left
            && left < t.factory_soon_hours as i64 * 3600
        {
            issues.push((Level::Soon, format!("Inputs last ~{}", duration_text(left))));
        }
        // Basic factories fed by a running extractor are often idle when
        // you look, waiting for the next batch. That's normal; idle
        // factories only matter when they live on outside inputs or no
        // extractor is running any more.
        let idle_matters = !inputs.is_empty() || running.is_empty();
        if idle_count > 0 && idle_matters {
            issues.push((
                Level::Soon,
                format!("{idle_count} of {factory_count} factories idle at last view"),
            ));
        }
    }
    let fullest = stores
        .iter()
        .map(|(_, s)| s)
        .max_by(|a, b| a.percent().total_cmp(&b.percent()));
    if let Some(store) = fullest {
        let name = store_name(&stores, store);
        let pct = store.percent();
        if pct >= t.full_percent as f64 {
            issues.push((Level::Attention, format!("{name} full")));
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
            if factory_count > 0 {
                let noun = if factory_count == 1 {
                    "factory"
                } else {
                    "factories"
                };
                let mut text = format!("{factory_count} {noun} running");
                if let Some(left) = inputs_left {
                    text.push_str(&format!(" · inputs last ~{}", duration_text(left)));
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
        // Two factories need 80 of each per hour; 800 Water lasts 10 h.
        let pins = vec![
            factory(1, ADVANCED, 65, NOW - 600),
            factory(2, ADVANCED, 65, NOW - 600),
            pad(3, vec![(PLASMOIDS, 4000), (WATER, 800)]),
        ];
        let c = colony(pins, vec![], NOW);
        let a = assess(&c, &data, &defaults(), NOW);
        assert_eq!(a.kind, ColonyKind::Factory);
        assert_eq!(a.inputs, vec![PLASMOIDS, WATER]);
        assert_eq!(a.inputs_left, Some(10 * 3600));
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
        // Extractor feeds a Water factory: no outside inputs, and the
        // estimate stores Water (20 per 3,000 Aqueous Liquids).
        let pins = vec![
            ecu(1, NOW + 5 * 86_400),
            factory(2, BASIC, 121, NOW - 600),
            pad(3, vec![]),
        ];
        let c = colony(pins, vec![], NOW - 10 * 3600);
        let a = assess(&c, &data, &defaults(), NOW);
        assert_eq!(a.kind, ColonyKind::Both);
        assert!(a.inputs.is_empty());
        assert_eq!(a.inputs_left, None);
        // 200,000 Aqueous -> 1,333.33 Water x 0.19 m³.
        let expected = 200_000.0 * 20.0 / 3000.0 * 0.19;
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
