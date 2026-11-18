//! Plays a colony forward from its last in-game view, cycle by cycle, to
//! estimate what is in its launchpads and storage now, and when its
//! factories run out of inputs.
//!
//! ESI only shows the colony as it was when its owner last opened it. From
//! there the simulation follows the colony's own routes:
//!
//! - An extractor adds its units per cycle at the end of each cycle, until
//!   its program ends, and sends them along its routes.
//! - An idle factory takes what its schematic needs from the launchpads
//!   and storage that have a route to it. Once it has everything, it
//!   starts a cycle; at the end of the cycle its output goes along its
//!   routes: to factories that need it first, then to launchpads and
//!   storage. (A store only passes things on to factories, which take
//!   them; store-to-store routes aren't followed.)
//! - Launchpads and storage never take more than fits. What doesn't fit
//!   anywhere is lost, as in the game.
//!
//! It's an estimate: an extractor's real yield goes up and down from cycle
//! to cycle, and the game's exact routing order isn't public. But it uses
//! the same links, schematics and stock as the game, so a factory planet's
//! output pad now fills (and its input pad empties) the way it really does.
//!
//! Everything here is plain math, run once a minute at most, never every
//! frame. Events are handled one moment at a time (the next cycle end of
//! any extractor or factory), so a day of a 20-factory planet is about a
//! thousand small steps.

use std::collections::{BTreeMap, BTreeSet};

use super::data::{Layout, Route};
use crate::static_data::{PiSchematic, PiStructureKind, StaticData};

/// How far past "now" the simulation looks for the inputs running out and
/// for pads getting full.
pub const HORIZON_SECS: i64 = 30 * 86_400;

/// A safety stop for very large or very old colonies.
const MAX_STEPS: usize = 200_000;

/// What the simulation found.
#[derive(Debug, Clone, PartialEq)]
pub struct SimResult {
    /// m³ in each launchpad and storage (by pin id) at "now".
    pub used_now_m3: BTreeMap<i64, f64>,
    /// When a factory first couldn't start because a commodity from
    /// outside the planet had run out. `None` if that doesn't happen
    /// before the horizon (or nothing needs outside inputs).
    pub starved_at: Option<i64>,
    /// The last moment the simulation looked at. If the inputs never ran
    /// out, they last at least until then.
    pub horizon: i64,
    /// For each watched store (by pin id): when it gets full, or where it
    /// stops. See [`Outlook`].
    pub outlook: BTreeMap<i64, Outlook>,
}

/// Where a watched launchpad or storage is heading after "now".
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Outlook {
    /// Full (nothing more fits) at this moment. Can be before "now".
    FullAt(i64),
    /// Fills up a bit more, then stops at this many m³: what feeds it
    /// runs out or ends first.
    StopsAt(f64),
    /// Still filling at the end of the look-ahead.
    Beyond,
    /// Doesn't get fuller than it is now.
    NotFilling,
}

/// A launchpad or storage facility while simulating.
struct Store {
    pin_id: i64,
    capacity_m3: f64,
    /// Type id -> amount. `f64` so splitting never rounds anything away.
    contents: BTreeMap<i64, f64>,
    /// The m³ of `contents`, kept up to date as things come and go.
    used_m3: f64,
    /// Something didn't fit into it in this step.
    overflowed: bool,
    /// When it last got fuller.
    grew_at: i64,
}

/// An extractor control unit with a running program.
struct Ecu {
    pin_id: i64,
    product: i64,
    qty: f64,
    cycle_secs: i64,
    /// The end of its next cycle.
    next_end: i64,
    expiry: i64,
}

/// A factory with a schematic Athena knows.
struct Factory<'a> {
    pin_id: i64,
    schematic: &'a PiSchematic,
    /// The inputs it holds for its next cycle.
    buffer: BTreeMap<i64, f64>,
    /// When its running cycle ends. `None` while idle.
    busy_until: Option<i64>,
    /// For each input: (type id, quantity per cycle, the stores routed to
    /// this factory with it, as indexes into the stores list). Worked out
    /// once, so each step doesn't search the routes again.
    pulls: Vec<(i64, f64, Vec<usize>)>,
    /// No route brings anything to it: a spare factory, set up but not
    /// in use. It never counts as having run out of inputs.
    spare: bool,
}

/// Simulates `layout` from `viewed` (the last in-game view) to `now`, and
/// on (up to [`HORIZON_SECS`] past `now`) to find when the factories run
/// out of outside inputs (`outside`) and where the `watch`ed stores (by
/// pin id) are heading.
pub fn simulate(
    layout: &Layout,
    data: &StaticData,
    kinds: &BTreeMap<i64, PiStructureKind>,
    outside: &BTreeSet<i64>,
    watch: &BTreeSet<i64>,
    viewed: i64,
    now: i64,
) -> SimResult {
    let volume = |type_id: i64| data.pi_commodity(type_id).map_or(0.0, |c| c.volume_m3);
    let kind = |pin_id: i64| kinds.get(&pin_id).copied();

    // ---- The structures, as they were at the view.
    let mut stores: Vec<Store> = Vec::new();
    let mut ecus: Vec<Ecu> = Vec::new();
    let mut factories: Vec<Factory> = Vec::new();
    for pin in &layout.pins {
        match kind(pin.pin_id) {
            Some(PiStructureKind::Launchpad) | Some(PiStructureKind::Storage) => {
                stores.push(Store {
                    pin_id: pin.pin_id,
                    capacity_m3: data
                        .pi_structure(pin.type_id)
                        .map_or(0.0, |s| s.capacity_m3),
                    contents: pin.contents.iter().map(|&(t, n)| (t, n as f64)).collect(),
                    used_m3: pin
                        .contents
                        .iter()
                        .map(|&(t, n)| n as f64 * volume(t))
                        .sum(),
                    overflowed: false,
                    grew_at: viewed,
                });
            }
            Some(PiStructureKind::Extractor) => {
                let (Some(ex), Some(expiry)) = (&pin.extractor, pin.expiry) else {
                    continue;
                };
                if ex.heads == 0 || ex.cycle_secs <= 0 {
                    continue;
                }
                // Cycles end at a fixed beat from the program's start.
                let anchor = pin.last_cycle_start.or(pin.install).unwrap_or(viewed);
                let mut next_end = anchor + ex.cycle_secs;
                if next_end <= viewed {
                    let behind = (viewed - next_end) / ex.cycle_secs + 1;
                    next_end += behind * ex.cycle_secs;
                }
                ecus.push(Ecu {
                    pin_id: pin.pin_id,
                    product: ex.product,
                    qty: ex.qty_per_cycle as f64,
                    cycle_secs: ex.cycle_secs,
                    next_end,
                    expiry,
                });
            }
            Some(PiStructureKind::Factory) => {
                let Some(schematic) = pin.schematic_id.and_then(|id| data.pi_schematic(id)) else {
                    continue;
                };
                let busy_until = pin
                    .last_cycle_start
                    .map(|start| start + schematic.cycle_secs)
                    .filter(|end| *end > viewed);
                factories.push(Factory {
                    pin_id: pin.pin_id,
                    schematic,
                    buffer: pin.contents.iter().map(|&(t, n)| (t, n as f64)).collect(),
                    busy_until,
                    pulls: Vec::new(),
                    spare: false,
                });
            }
            _ => {}
        }
    }

    let routes = if layout.routes.is_empty() {
        guessed_routes(&stores, &ecus, &factories, kinds)
    } else {
        layout.routes.clone()
    };

    // Which stores each factory takes its inputs from.
    for f in factories.iter_mut() {
        f.pulls = f
            .schematic
            .inputs
            .iter()
            .map(|&(type_id, qty)| {
                let sources = routes
                    .iter()
                    .filter(|r| r.destination == f.pin_id && r.type_id == type_id)
                    .filter_map(|r| stores.iter().position(|s| s.pin_id == r.source))
                    .collect();
                (type_id, qty as f64, sources)
            })
            .collect();
        f.spare = !routes.iter().any(|r| r.destination == f.pin_id);
    }
    // Where each (pin, commodity) is sent: factories, then stores.
    let mut outlets: BTreeMap<(i64, i64), Outlets> = BTreeMap::new();
    for r in &routes {
        let entry = outlets.entry((r.source, r.type_id)).or_default();
        if let Some(i) = factories.iter().position(|f| f.pin_id == r.destination) {
            entry.factories.push(i);
        } else if let Some(i) = stores.iter().position(|s| s.pin_id == r.destination) {
            entry.stores.push(i);
        }
    }

    let mut sim = Sim {
        stores,
        factories,
        outlets,
        volume: &volume,
        outside,
        starved_at: None,
    };

    // Indexes of the watched stores, and when each got full.
    let watched: Vec<usize> = (0..sim.stores.len())
        .filter(|&i| watch.contains(&sim.stores[i].pin_id) && sim.stores[i].capacity_m3 > 0.0)
        .collect();
    let mut full_at: BTreeMap<usize, i64> = BTreeMap::new();

    let wants_inputs = !outside.is_empty();
    let horizon = (now + HORIZON_SECS).max(viewed);
    let mut used_now: Option<BTreeMap<i64, f64>> = None;
    let mut t = viewed;
    // Did everything stop before the horizon?
    let mut stopped = false;
    sim.start_idle_factories(t);
    for _ in 0..MAX_STEPS {
        sim.note_full(&watched, &mut full_at, t);
        // The next moment anything happens.
        let next = ecus
            .iter()
            .filter(|e| e.next_end <= e.expiry)
            .map(|e| e.next_end)
            .chain(sim.factories.iter().filter_map(|f| f.busy_until))
            .min();
        if used_now.is_none() && next.is_none_or(|n| n > now) {
            used_now = Some(sim.store_volumes());
        }
        // Everything we want to know is known.
        let done = used_now.is_some()
            && (!wants_inputs || sim.starved_at.is_some())
            && full_at.len() == watched.len();
        if done {
            break;
        }
        let Some(next) = next.filter(|n| *n <= horizon) else {
            stopped = next.is_none();
            break;
        };
        t = next;

        // Extractors that finish a cycle now.
        for ecu in ecus.iter_mut() {
            if ecu.next_end == t && t <= ecu.expiry {
                sim.deliver(ecu.pin_id, ecu.product, ecu.qty, t);
                ecu.next_end += ecu.cycle_secs;
            }
        }
        // Factories that finish a cycle now.
        for i in 0..sim.factories.len() {
            if sim.factories[i].busy_until == Some(t) {
                sim.factories[i].busy_until = None;
                if let Some((output, qty)) = sim.factories[i].schematic.output {
                    let from = sim.factories[i].pin_id;
                    sim.deliver(from, output, qty as f64, t);
                }
            }
        }
        sim.start_idle_factories(t);
    }

    let used_now = used_now.unwrap_or_else(|| sim.store_volumes());
    let outlook = watched
        .iter()
        .map(|&i| {
            let store = &sim.stores[i];
            let then = used_now.get(&store.pin_id).copied().unwrap_or(0.0);
            let outlook = if let Some(&at) = full_at.get(&i) {
                Outlook::FullAt(at)
            } else if store.used_m3 <= then + 1e-6 || store.grew_at <= now {
                Outlook::NotFilling
            } else if stopped || store.grew_at + STOPPED_SECS < horizon {
                Outlook::StopsAt(store.used_m3)
            } else {
                Outlook::Beyond
            };
            (store.pin_id, outlook)
        })
        .collect();

    SimResult {
        used_now_m3: used_now,
        starved_at: sim.starved_at,
        horizon,
        outlook,
    }
}

/// A store that didn't get fuller during the last day of the look-ahead
/// counts as stopped, even if other things on the planet still run.
const STOPPED_SECS: i64 = 86_400;

/// Where one pin sends one commodity, as indexes into the factory and
/// store lists.
#[derive(Default)]
struct Outlets {
    factories: Vec<usize>,
    stores: Vec<usize>,
}

/// The colony's changing state while simulating.
struct Sim<'a, 'b> {
    stores: Vec<Store>,
    factories: Vec<Factory<'a>>,
    outlets: BTreeMap<(i64, i64), Outlets>,
    /// m³ per unit of a commodity. A reference to a closure: `dyn Fn` lets
    /// this struct hold it without naming the closure's type.
    volume: &'b dyn Fn(i64) -> f64,
    outside: &'b BTreeSet<i64>,
    starved_at: Option<i64>,
}

impl Sim<'_, '_> {
    fn store_volumes(&self) -> BTreeMap<i64, f64> {
        self.stores.iter().map(|s| (s.pin_id, s.used_m3)).collect()
    }

    /// Sends `amount` of `type_id` from pin `from` along its routes:
    /// factories that need it first, then launchpads and storage. What
    /// finds no place is lost.
    fn deliver(&mut self, from: i64, type_id: i64, mut amount: f64, t: i64) {
        let Some(outlets) = self.outlets.get(&(from, type_id)) else {
            return;
        };
        for &i in &outlets.factories {
            if amount <= 0.0 {
                break;
            }
            let f = &mut self.factories[i];
            let need = f
                .pulls
                .iter()
                .find(|(id, _, _)| *id == type_id)
                .map_or(0.0, |(_, q, _)| *q);
            let have = f.buffer.entry(type_id).or_default();
            let take = (need - *have).clamp(0.0, amount);
            *have += take;
            amount -= take;
        }
        let unit = (self.volume)(type_id);
        for &i in &outlets.stores {
            if amount <= 0.0 {
                break;
            }
            let store = &mut self.stores[i];
            let fits = if unit > 0.0 {
                ((store.capacity_m3 - store.used_m3) / unit).max(0.0)
            } else {
                f64::INFINITY
            };
            let take = amount.min(fits);
            if take > 0.0 {
                *store.contents.entry(type_id).or_default() += take;
                store.used_m3 += take * unit;
                amount -= take;
                if unit > 0.0 {
                    store.grew_at = t;
                }
            }
            if amount > 0.0 {
                store.overflowed = true;
            }
        }
    }

    /// Notes the moment each watched store is full: something didn't fit,
    /// or it is at its capacity.
    fn note_full(&mut self, watched: &[usize], full_at: &mut BTreeMap<usize, i64>, t: i64) {
        for &i in watched {
            let store = &mut self.stores[i];
            let full = store.overflowed || store.used_m3 >= store.capacity_m3 - 1e-6;
            store.overflowed = false;
            if full {
                full_at.entry(i).or_insert(t);
            }
        }
    }

    /// Lets every idle factory take its inputs from the stores routed to
    /// it, and start a cycle if it has everything. Notes the first moment
    /// one can't start for lack of an outside input.
    fn start_idle_factories(&mut self, t: i64) {
        for i in 0..self.factories.len() {
            if self.factories[i].busy_until.is_some() {
                continue;
            }
            self.pull_inputs(i);
            let f = &mut self.factories[i];
            let mut complete = true;
            let mut lacks_outside = false;
            for (id, q, _) in &f.pulls {
                if f.buffer.get(id).copied().unwrap_or(0.0) + 1e-6 < *q {
                    complete = false;
                    lacks_outside |= self.outside.contains(id);
                }
            }
            if complete {
                for (id, q, _) in &f.pulls {
                    if let Some(have) = f.buffer.get_mut(id) {
                        *have = (*have - *q).max(0.0);
                    }
                }
                f.busy_until = Some(t + f.schematic.cycle_secs.max(1));
            } else if lacks_outside && !f.spare && self.starved_at.is_none() {
                self.starved_at = Some(t);
            }
        }
    }

    /// Fills factory `i`'s buffer from the launchpads and storage that
    /// have a route to it.
    fn pull_inputs(&mut self, i: usize) {
        // `factories` and `stores` are separate fields, so Rust lets us
        // borrow one factory and the stores mutably at the same time.
        let f = &mut self.factories[i];
        for (type_id, qty, sources) in &f.pulls {
            let have = f.buffer.entry(*type_id).or_default();
            for &s in sources {
                let need = qty - *have;
                if need <= 0.0 {
                    break;
                }
                let store = &mut self.stores[s];
                let Some(stock) = store.contents.get_mut(type_id) else {
                    continue;
                };
                let take = need.min(*stock);
                if take > 0.0 {
                    *stock -= take;
                    store.used_m3 = (store.used_m3 - take * (self.volume)(*type_id)).max(0.0);
                    *have += take;
                }
            }
        }
    }
}

/// Routes for a colony whose layout came without any (an old saved
/// layout, or a test): extractors and factories send to factories that
/// need their product, then to the first launchpad (or storage); every
/// factory takes its inputs from every launchpad and storage.
fn guessed_routes(
    stores: &[Store],
    ecus: &[Ecu],
    factories: &[Factory],
    kinds: &BTreeMap<i64, PiStructureKind>,
) -> Vec<Route> {
    let hub = stores
        .iter()
        .find(|s| kinds.get(&s.pin_id) == Some(&PiStructureKind::Launchpad))
        .or(stores.first())
        .map(|s| s.pin_id);
    let route = |source: i64, destination: i64, type_id: i64| Route {
        source,
        destination,
        type_id,
        quantity: 0.0,
    };
    let mut routes = Vec::new();
    let send = |from: i64, type_id: i64, routes: &mut Vec<Route>| {
        for f in factories {
            if f.schematic.inputs.iter().any(|(id, _)| *id == type_id) {
                routes.push(route(from, f.pin_id, type_id));
            }
        }
        if let Some(hub) = hub {
            routes.push(route(from, hub, type_id));
        }
    };
    for e in ecus {
        send(e.pin_id, e.product, &mut routes);
    }
    for f in factories {
        if let Some((output, _)) = f.schematic.output {
            send(f.pin_id, output, &mut routes);
        }
    }
    for f in factories {
        for (input, _) in &f.schematic.inputs {
            for s in stores {
                routes.push(route(s.pin_id, f.pin_id, *input));
            }
        }
    }
    routes
}
