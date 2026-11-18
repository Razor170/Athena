//! The Assets module's data: turning the ESI asset lists of all characters
//! into a tree of locations, ships, containers and items, and choosing
//! what to show for the current search and filters.
//!
//! Pure logic without I/O, so it can be tested. The module rebuilds the
//! tree only when new data arrives, and the view only when the search,
//! a filter or the sort changes; drawing a frame just reads them.
//!
//! How ESI describes where an item is: every item has a `location_id`. If
//! that is the `item_id` of another item of the same character, the item is
//! inside it (fitted to a ship, in its cargo, in a container). Otherwise it
//! is the place: an NPC station, a player structure or a solar system.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use super::config::{Filter, SortBy};
use super::sde::{CATEGORY_BLUEPRINT, CATEGORY_SHIP, TypeInfo};
use crate::modules::fittings::assets::EsiAsset;
use crate::universe::Universe;

/// Location ids from here up are player structures.
pub const FIRST_STRUCTURE_ID: i64 = 1_000_000_000_000;
/// Region ids from here up are wormhole space (and other unmapped space).
const FIRST_WORMHOLE_REGION: i64 = 11_000_000;
/// SDE categories whose assembled items hold other items: Celestial
/// (cargo containers) and Deployable (mobile depots).
const CONTAINER_CATEGORIES: [i64; 2] = [2, 22];

/// One character's assets, as fetched (and kept in the cache file).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CharacterAssets {
    pub items: Vec<EsiAsset>,
    /// Item id -> the name given in game (ships and containers).
    pub names: BTreeMap<i64, String>,
    /// When ESI made this data (Unix seconds).
    pub data_from: i64,
    /// When ESI has newer data.
    pub next_at: i64,
}

/// A station or structure: its name and system.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlaceInfo {
    pub name: String,
    /// `None` for a structure the characters may not look up.
    pub system_id: Option<i64>,
}

/// What a location is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum LocationKey {
    Station(i64),
    Structure(i64),
    /// In space in this system (the ship being flown, deployables).
    Space(i64),
    /// Asset safety in this system.
    AssetSafety(i64),
    /// Anywhere else ESI names (by its location id).
    Other(i64),
}

/// One item or stack.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub item_id: i64,
    pub type_id: i64,
    pub owner: u64,
    pub quantity: i64,
    /// ESI's `location_flag`: "Hangar", "HiSlot0", "Cargo", ...
    pub flag: String,
    /// The name given in game, if any.
    pub name: Option<String>,
    /// What is inside, as indexes into [`Tree::nodes`].
    pub children: Vec<u32>,
    pub is_ship: bool,
    /// Shown as a ship or container row (two lines) instead of an item
    /// line.
    pub big: bool,
    /// Value of this stack alone.
    pub own_value: f64,
    /// Value with everything inside.
    pub value: f64,
    /// m³ of this stack (an assembled ship or container as it is, a
    /// stack packaged). What is inside takes no extra room.
    pub volume: f64,
    /// Stacks in this subtree, this one included.
    pub count: u32,
}

/// One place with what the characters have there.
#[derive(Debug, Clone, PartialEq)]
pub struct Location {
    pub key: LocationKey,
    /// The station or structure name, or "In space".
    pub name: String,
    pub system_id: Option<i64>,
    /// The items directly there.
    pub roots: Vec<u32>,
}

/// Everything, from all characters.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tree {
    pub nodes: Vec<Node>,
    pub locations: Vec<Location>,
}

/// What [`build`] looks things up in.
pub struct Lookup<'a> {
    pub types: &'a HashMap<i64, TypeInfo>,
    pub prices: &'a HashMap<i64, f64>,
    pub places: &'a HashMap<i64, PlaceInfo>,
}

/// Builds the tree from every character's assets.
pub fn build<'a>(
    characters: impl Iterator<Item = (u64, &'a CharacterAssets)>,
    lookup: &Lookup,
) -> Tree {
    let mut tree = Tree::default();
    let mut location_index: HashMap<LocationKey, usize> = HashMap::new();
    for (owner, assets) in characters {
        // Item id -> its position in the list, and what is inside each.
        let by_id: HashMap<i64, usize> = assets
            .items
            .iter()
            .enumerate()
            .map(|(i, a)| (a.item_id, i))
            .collect();
        let mut inside: HashMap<i64, Vec<usize>> = HashMap::new();
        for (i, a) in assets.items.iter().enumerate() {
            if by_id.contains_key(&a.location_id) && a.location_id != a.item_id {
                inside.entry(a.location_id).or_default().push(i);
            }
        }
        for (i, a) in assets.items.iter().enumerate() {
            if by_id.contains_key(&a.location_id) && a.location_id != a.item_id {
                continue;
            }
            let key = location_key(a);
            let index = *location_index.entry(key).or_insert_with(|| {
                let (name, system_id) = location_name(key, a, lookup.places);
                tree.locations.push(Location {
                    key,
                    name,
                    system_id,
                    roots: Vec::new(),
                });
                tree.locations.len() - 1
            });
            // `depth` guards against a broken list where items contain
            // each other in a circle.
            let node = add_node(&mut tree, owner, assets, i, &inside, lookup, 0);
            tree.locations[index].roots.push(node);
        }
    }
    tree
}

fn location_key(asset: &EsiAsset) -> LocationKey {
    let id = asset.location_id;
    match asset.location_type.as_str() {
        "station" => LocationKey::Station(id),
        "solar_system" if asset.location_flag == "AssetSafety" => LocationKey::AssetSafety(id),
        "solar_system" => LocationKey::Space(id),
        "item" if id >= FIRST_STRUCTURE_ID => LocationKey::Structure(id),
        _ => LocationKey::Other(id),
    }
}

fn location_name(
    key: LocationKey,
    asset: &EsiAsset,
    places: &HashMap<i64, PlaceInfo>,
) -> (String, Option<i64>) {
    match key {
        LocationKey::Station(id) | LocationKey::Structure(id) => match places.get(&id) {
            Some(place) => (place.name.clone(), place.system_id),
            None if matches!(key, LocationKey::Structure(_)) => {
                ("Structure (name not known yet)".to_string(), None)
            }
            None => ("Station (name not known yet)".to_string(), None),
        },
        LocationKey::Space(system) => ("In space".to_string(), Some(system)),
        LocationKey::AssetSafety(system) => ("Asset safety".to_string(), Some(system)),
        LocationKey::Other(id) => (format!("{} ({id})", asset.location_flag), None),
    }
}

fn add_node(
    tree: &mut Tree,
    owner: u64,
    assets: &CharacterAssets,
    index: usize,
    inside: &HashMap<i64, Vec<usize>>,
    lookup: &Lookup,
    depth: u32,
) -> u32 {
    let a = &assets.items[index];
    let info = lookup.types.get(&a.type_id);
    let category = info.map_or(0, |t| t.category_id);
    let is_ship = category == CATEGORY_SHIP;
    let is_copy = a.is_blueprint_copy == Some(true);
    // A blueprint copy has no market price; using the original's would
    // be far too much.
    let unit_price = if is_copy || (category == CATEGORY_BLUEPRINT && a.quantity < 0) {
        0.0
    } else {
        lookup.prices.get(&a.type_id).copied().unwrap_or(0.0)
    };
    // ESI sends -1 or -2 as the quantity of some singletons
    // (blueprints); they are one item.
    let quantity = a.quantity.max(1);
    let volume = match info {
        Some(t) if a.is_singleton => t.volume,
        Some(t) => t.packaged_volume * quantity as f64,
        None => 0.0,
    };
    let own_value = unit_price * quantity as f64;
    let position = tree.nodes.len() as u32;
    tree.nodes.push(Node {
        item_id: a.item_id,
        type_id: a.type_id,
        owner,
        quantity,
        flag: a.location_flag.clone(),
        name: assets.names.get(&a.item_id).cloned(),
        children: Vec::new(),
        is_ship,
        big: false,
        own_value,
        value: own_value,
        volume,
        count: 1,
    });
    let mut children = Vec::new();
    if depth < 8
        && let Some(kids) = inside.get(&a.item_id)
    {
        for &kid in kids {
            children.push(add_node(
                tree,
                owner,
                assets,
                kid,
                inside,
                lookup,
                depth + 1,
            ));
        }
    }
    let value: f64 = children.iter().map(|c| tree.nodes[*c as usize].value).sum();
    let count: u32 = children.iter().map(|c| tree.nodes[*c as usize].count).sum();
    let node = &mut tree.nodes[position as usize];
    node.big = is_ship
        || !children.is_empty()
        || (a.is_singleton && CONTAINER_CATEGORIES.contains(&category));
    node.value += value;
    node.count += count;
    node.children = children;
    position
}

/// Where in a ship an item is, from its `location_flag`: the order to
/// show sections in, and the section's title.
pub fn section_of(flag: &str) -> (u8, String) {
    let fixed = |order: u8, title: &str| (order, title.to_string());
    if flag.starts_with("HiSlot") {
        fixed(0, "High slots")
    } else if flag.starts_with("MedSlot") {
        fixed(1, "Med slots")
    } else if flag.starts_with("LoSlot") {
        fixed(2, "Low slots")
    } else if flag.starts_with("RigSlot") {
        fixed(3, "Rigs")
    } else if flag.starts_with("SubSystemSlot") {
        fixed(4, "Subsystems")
    } else if flag == "DroneBay" {
        fixed(5, "Drone bay")
    } else if flag == "FighterBay" || flag.starts_with("FighterTube") {
        fixed(6, "Fighters")
    } else if flag == "Cargo" {
        fixed(7, "Cargo")
    } else if flag == "ShipHangar" {
        fixed(8, "Ship maintenance bay")
    } else if flag == "FleetHangar" {
        fixed(9, "Fleet hangar")
    } else {
        // "SpecializedOreHold" -> "Ore hold".
        let words = flag.strip_prefix("Specialized").unwrap_or(flag);
        let mut title = String::new();
        for (i, c) in words.chars().enumerate() {
            if c.is_uppercase() && i > 0 {
                title.push(' ');
                title.extend(c.to_lowercase());
            } else {
                title.push(c);
            }
        }
        (10, title)
    }
}

/// The security band a system is in, for filters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecBand {
    High,
    Low,
    Null,
    Wormhole,
}

pub fn sec_band(universe: &Universe, system_id: i64) -> Option<SecBand> {
    let system = universe.system(system_id)?;
    if system.region_id >= FIRST_WORMHOLE_REGION {
        return Some(SecBand::Wormhole);
    }
    let security = system.display_security();
    Some(if security >= 0.5 {
        SecBand::High
    } else if security > 0.0 {
        SecBand::Low
    } else {
        SecBand::Null
    })
}

/// What decides which items are shown.
pub struct ViewOptions<'a> {
    /// Characters whose assets are shown.
    pub owners: &'a HashSet<u64>,
    /// The chips that are on. An item must match one of them.
    pub filters: Vec<&'a Filter>,
    /// The search text, any case. Empty = no search.
    pub search: &'a str,
    pub sort: SortBy,
    pub reversed: bool,
    /// System id -> jumps from the route start.
    pub jumps: &'a HashMap<i64, u32>,
}

/// One location as shown.
#[derive(Debug, Clone, PartialEq)]
pub struct LocationView {
    /// Index into [`Tree::locations`].
    pub location: usize,
    /// The roots shown.
    pub roots: Vec<u32>,
    /// Nodes shown, when only some are (search or filter on).
    pub shown: Option<HashSet<u32>>,
    /// Nodes that hold a hit without being one: opened by themselves.
    pub auto_open: HashSet<u32>,
    pub count: u32,
    pub value: f64,
    pub volume: f64,
    pub jumps: Option<u32>,
}

impl LocationView {
    pub fn is_shown(&self, node: u32) -> bool {
        self.shown.as_ref().is_none_or(|s| s.contains(&node))
    }
}

/// What is shown, with the totals line.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct View {
    pub locations: Vec<LocationView>,
    pub count: u32,
    pub value: f64,
    pub volume: f64,
    /// A search or filter hides something.
    pub narrowed: bool,
    /// A search is on: locations with a hit, and ships or containers that
    /// hold one, open by themselves.
    pub searching: bool,
}

/// Picks and sorts what to show. `type_name` and `system` look up names;
/// they come from the module (types) and the map.
pub fn view(
    tree: &Tree,
    options: &ViewOptions,
    types: &HashMap<i64, TypeInfo>,
    universe: Option<&Universe>,
) -> View {
    let search = options.search.trim().to_lowercase();
    let restricted = !options.filters.is_empty() || !search.is_empty();
    let mut out = View {
        narrowed: restricted,
        searching: !search.is_empty(),
        ..View::default()
    };
    for (index, location) in tree.locations.iter().enumerate() {
        let system = location.system_id.and_then(|id| universe?.system(id));
        let band = location.system_id.and_then(|id| sec_band(universe?, id));
        let place_hit = !search.is_empty()
            && (location.name.to_lowercase().contains(&search)
                || system.is_some_and(|s| s.name.to_lowercase().contains(&search)));
        let mut lv = LocationView {
            location: index,
            roots: Vec::new(),
            shown: restricted.then(HashSet::new),
            auto_open: HashSet::new(),
            count: 0,
            value: 0.0,
            volume: 0.0,
            jumps: location
                .system_id
                .and_then(|id| options.jumps.get(&id).copied()),
        };
        let matches = |node: &Node| -> bool {
            let info = types.get(&node.type_id);
            let filters_ok = options.filters.is_empty()
                || options
                    .filters
                    .iter()
                    .any(|f| filter_matches(f, node, info, band));
            let search_ok = search.is_empty() || place_hit || text_contains(node, info, &search);
            filters_ok && search_ok
        };
        for &root in &location.roots {
            let node = &tree.nodes[root as usize];
            if !options.owners.contains(&node.owner) {
                continue;
            }
            if mark(tree, root, &matches, &mut lv) {
                lv.roots.push(root);
            }
        }
        if !lv.roots.is_empty() {
            out.count += lv.count;
            out.value += lv.value;
            out.volume += lv.volume;
            out.locations.push(lv);
        }
    }
    sort_locations(&mut out.locations, tree, options, universe);
    out
}

/// Marks `node` and what is shown below it. True when anything is shown.
fn mark(tree: &Tree, index: u32, matches: &impl Fn(&Node) -> bool, lv: &mut LocationView) -> bool {
    let node = &tree.nodes[index as usize];
    if matches(node) {
        lv.count += node.count;
        lv.value += node.value;
        lv.volume += node.volume;
        if let Some(shown) = &mut lv.shown {
            add_all(tree, index, shown);
        }
        return true;
    }
    let mut any = false;
    for &child in &node.children {
        any |= mark(tree, child, matches, lv);
    }
    if any {
        if let Some(shown) = &mut lv.shown {
            shown.insert(index);
        }
        lv.auto_open.insert(index);
    }
    any
}

fn add_all(tree: &Tree, index: u32, shown: &mut HashSet<u32>) {
    shown.insert(index);
    for &child in &tree.nodes[index as usize].children {
        add_all(tree, child, shown);
    }
}

fn text_contains(node: &Node, info: Option<&TypeInfo>, lower: &str) -> bool {
    info.is_some_and(|t| t.name.to_lowercase().contains(lower))
        || node
            .name
            .as_ref()
            .is_some_and(|n| n.to_lowercase().contains(lower))
}

/// Whether one saved filter matches an item in a system of this band.
pub fn filter_matches(
    filter: &Filter,
    node: &Node,
    info: Option<&TypeInfo>,
    band: Option<SecBand>,
) -> bool {
    let band_ok = match band {
        Some(SecBand::High) => filter.high_sec,
        Some(SecBand::Low) => filter.low_sec,
        Some(SecBand::Null) => filter.null_sec,
        Some(SecBand::Wormhole) => filter.wormhole,
        // Unknown system (a structure we can't look up): don't hide it.
        None => true,
    };
    let kind_ok = (filter.categories.is_empty() && filter.groups.is_empty())
        || info.is_some_and(|t| {
            filter.categories.contains(&t.category_id) || filter.groups.contains(&t.group_id)
        });
    let text = filter.text.trim().to_lowercase();
    let text_ok = text.is_empty() || text_contains(node, info, &text);
    band_ok && kind_ok && text_ok && node.value >= filter.min_isk
}

fn sort_locations(
    list: &mut [LocationView],
    tree: &Tree,
    options: &ViewOptions,
    universe: Option<&Universe>,
) {
    let name_key = |lv: &LocationView| -> String {
        let location = &tree.locations[lv.location];
        let system = location
            .system_id
            .and_then(|id| universe?.system(id))
            .map_or(String::new(), |s| s.name.to_lowercase());
        format!("{system} {}", location.name.to_lowercase())
    };
    list.sort_by(|a, b| {
        let order = match options.sort {
            // Unreachable last, then by name.
            SortBy::Distance => a
                .jumps
                .unwrap_or(u32::MAX)
                .cmp(&b.jumps.unwrap_or(u32::MAX)),
            SortBy::Name => std::cmp::Ordering::Equal,
            SortBy::Count => b.count.cmp(&a.count),
            SortBy::Value => b.value.total_cmp(&a.value),
        };
        let order = if options.reversed {
            order.reverse()
        } else {
            order
        };
        let by_name = name_key(a).cmp(&name_key(b));
        let by_name = if options.reversed && options.sort == SortBy::Name {
            by_name.reverse()
        } else {
            by_name
        };
        order.then(by_name)
    });
}

/// One line of the list. All lines are equally tall, so the list can draw
/// only the lines that are on screen; a ship or container takes two.
#[derive(Debug, Clone, PartialEq)]
pub enum Row {
    /// Index into [`View::locations`].
    Location(usize),
    /// A ship or container: name, type, m³ and value.
    BigFirst { node: u32, depth: u8 },
    /// Its second line: what is inside.
    BigSecond { node: u32, depth: u8 },
    /// A section title inside a ship ("Low slots") or above the loose
    /// items of a location.
    Section { title: String, depth: u8 },
    /// A stack of items. `owner` shows the owner's name in front.
    Item { node: u32, depth: u8, owner: bool },
}

/// Which rows are open. A row is open when exactly one of "the user
/// opened it" and "the search opened it" is true, so a click always
/// flips it.
pub struct OpenState<'a> {
    pub locations: &'a HashSet<LocationKey>,
    /// By item id, which stays the same across fetches.
    pub nodes: &'a HashSet<i64>,
}

/// The rows to draw. `owner_name` and `type_name` are only used for
/// sorting ships and items by name.
pub fn rows(
    tree: &Tree,
    view: &View,
    open: &OpenState,
    owner_name: &impl Fn(u64) -> String,
    type_name: &impl Fn(i64) -> String,
) -> Vec<Row> {
    let mut out = Vec::new();
    for (index, lv) in view.locations.iter().enumerate() {
        out.push(Row::Location(index));
        let key = tree.locations[lv.location].key;
        // The search opens every location it found something in.
        if open.locations.contains(&key) == view.searching {
            continue;
        }
        let ctx = RowContext {
            tree,
            lv,
            open,
            searching: view.searching,
            owner_name,
            type_name,
        };
        ctx.children(&lv.roots, 1, true, &mut out);
    }
    out
}

struct RowContext<'a, F, G> {
    tree: &'a Tree,
    lv: &'a LocationView,
    open: &'a OpenState<'a>,
    searching: bool,
    owner_name: &'a F,
    type_name: &'a G,
}

impl<F: Fn(u64) -> String, G: Fn(i64) -> String> RowContext<'_, F, G> {
    fn node(&self, index: u32) -> &Node {
        &self.tree.nodes[index as usize]
    }

    /// Ships and containers first (by owner, then name), then items by
    /// type name. `top` marks the items directly in a location, which get
    /// a section title and their owner's name.
    fn children(&self, list: &[u32], depth: u8, top: bool, out: &mut Vec<Row>) {
        let mut big: Vec<u32> = Vec::new();
        let mut small: Vec<u32> = Vec::new();
        for &i in list {
            if !self.lv.is_shown(i) {
                continue;
            }
            if self.node(i).big {
                big.push(i);
            } else {
                small.push(i);
            }
        }
        big.sort_by_cached_key(|i| {
            let n = self.node(*i);
            (
                (self.owner_name)(n.owner).to_lowercase(),
                n.name.clone().unwrap_or_default().to_lowercase(),
                (self.type_name)(n.type_id).to_lowercase(),
            )
        });
        small.sort_by_cached_key(|i| {
            let n = self.node(*i);
            ((self.type_name)(n.type_id).to_lowercase(), n.owner)
        });
        for i in big {
            self.big(i, depth, out);
        }
        if top && !small.is_empty() {
            out.push(Row::Section {
                title: "Items in hangar".into(),
                depth,
            });
        }
        for i in small {
            out.push(Row::Item {
                node: i,
                depth,
                owner: top,
            });
        }
    }

    fn big(&self, index: u32, depth: u8, out: &mut Vec<Row>) {
        out.push(Row::BigFirst { node: index, depth });
        out.push(Row::BigSecond { node: index, depth });
        let node = self.node(index);
        let auto = self.searching && self.lv.auto_open.contains(&index);
        if node.children.is_empty() || self.open.nodes.contains(&node.item_id) == auto {
            return;
        }
        if !node.is_ship {
            self.children(&node.children, depth + 1, false, out);
            return;
        }
        // A ship: by section.
        let mut sections: BTreeMap<(u8, String), Vec<u32>> = BTreeMap::new();
        for &child in &node.children {
            if self.lv.is_shown(child) {
                sections
                    .entry(section_of(&self.node(child).flag))
                    .or_default()
                    .push(child);
            }
        }
        for ((_, title), list) in sections {
            out.push(Row::Section {
                title,
                depth: depth + 1,
            });
            self.children(&list, depth + 1, false, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWNER: u64 = 90_000_001;
    const ALT: u64 = 90_000_002;
    const JITA_44: i64 = 60_003_760;
    const RIFTER: i64 = 587;
    const ARCHON: i64 = 23_757;
    const HAMMERHEAD: i64 = 2185;
    const TRITANIUM: i64 = 34;
    const CONTAINER: i64 = 3829;
    const STRUCTURE: i64 = 1_035_000_000_001;

    fn asset(
        item_id: i64,
        type_id: i64,
        location_id: i64,
        flag: &str,
        kind: &str,
        quantity: i64,
    ) -> EsiAsset {
        EsiAsset {
            item_id,
            type_id,
            location_id,
            location_flag: flag.to_string(),
            location_type: kind.to_string(),
            quantity,
            is_singleton: quantity == 1 && type_id != TRITANIUM,
            is_blueprint_copy: None,
        }
    }

    fn types() -> HashMap<i64, TypeInfo> {
        let t = |name: &str, group_id, category_id, volume, packaged_volume| TypeInfo {
            name: name.to_string(),
            group_id,
            category_id,
            volume,
            packaged_volume,
            market: true,
        };
        HashMap::from([
            (RIFTER, t("Rifter", 25, 6, 27289.0, 2500.0)),
            (ARCHON, t("Archon", 547, 6, 13_950_000.0, 1_300_000.0)),
            (HAMMERHEAD, t("Hammerhead II", 100, 18, 10.0, 10.0)),
            (TRITANIUM, t("Tritanium", 18, 4, 0.01, 0.01)),
            (CONTAINER, t("Medium Secure Container", 340, 2, 33.0, 33.0)),
        ])
    }

    fn prices() -> HashMap<i64, f64> {
        HashMap::from([
            (RIFTER, 400_000.0),
            (ARCHON, 1_500_000_000.0),
            (HAMMERHEAD, 1_000_000.0),
            (TRITANIUM, 4.0),
            (CONTAINER, 100_000.0),
        ])
    }

    fn places() -> HashMap<i64, PlaceInfo> {
        HashMap::from([(
            JITA_44,
            PlaceInfo {
                name: "Jita IV - Moon 4 - Caldari Navy Assembly Plant".into(),
                system_id: Some(30000142),
            },
        )])
    }

    /// Main: a Rifter with 2 Hammerheads in Jita, a container with
    /// Tritanium, loose Tritanium, and an Archon in a structure.
    /// Alt: a Rifter in Jita.
    fn sample() -> (CharacterAssets, CharacterAssets) {
        let main = CharacterAssets {
            items: vec![
                asset(1, RIFTER, JITA_44, "Hangar", "station", 1),
                asset(2, HAMMERHEAD, 1, "DroneBay", "item", 2),
                asset(3, CONTAINER, JITA_44, "Hangar", "station", 1),
                asset(4, TRITANIUM, 3, "Unlocked", "item", 1000),
                asset(5, TRITANIUM, JITA_44, "Hangar", "station", 500),
                asset(6, ARCHON, STRUCTURE, "Hangar", "item", 1),
            ],
            names: BTreeMap::from([(1, "Scout".to_string())]),
            ..CharacterAssets::default()
        };
        let alt = CharacterAssets {
            items: vec![asset(10, RIFTER, JITA_44, "Hangar", "station", 1)],
            ..CharacterAssets::default()
        };
        (main, alt)
    }

    fn sample_tree() -> Tree {
        let (main, alt) = sample();
        let types = types();
        let prices = prices();
        let places = places();
        let lookup = Lookup {
            types: &types,
            prices: &prices,
            places: &places,
        };
        build([(OWNER, &main), (ALT, &alt)].into_iter(), &lookup)
    }

    #[test]
    fn builds_locations_with_nested_items() {
        let tree = sample_tree();
        assert_eq!(tree.locations.len(), 2);
        let jita = &tree.locations[0];
        assert_eq!(jita.key, LocationKey::Station(JITA_44));
        assert_eq!(jita.system_id, Some(30000142));
        // Rifter, container, loose Tritanium (main) and the alt's Rifter.
        assert_eq!(jita.roots.len(), 4);
        let rifter = &tree.nodes[jita.roots[0] as usize];
        assert!(rifter.is_ship && rifter.big);
        assert_eq!(rifter.name.as_deref(), Some("Scout"));
        assert_eq!(rifter.count, 2);
        assert_eq!(rifter.value, 400_000.0 + 2_000_000.0);
        // Assembled: the full hull volume.
        assert_eq!(rifter.volume, 27289.0);
        let container = &tree.nodes[jita.roots[1] as usize];
        assert!(container.big && !container.is_ship);
        assert_eq!(container.value, 100_000.0 + 4000.0);
        let loose = &tree.nodes[jita.roots[2] as usize];
        assert!(!loose.big);
        assert!((loose.volume - 5.0).abs() < 1e-9);
        let structure = &tree.locations[1];
        assert_eq!(structure.key, LocationKey::Structure(STRUCTURE));
        assert_eq!(structure.system_id, None);
    }

    fn options<'a>(
        owners: &'a HashSet<u64>,
        filters: Vec<&'a Filter>,
        search: &'a str,
        jumps: &'a HashMap<i64, u32>,
    ) -> ViewOptions<'a> {
        ViewOptions {
            owners,
            filters,
            search,
            sort: SortBy::Value,
            reversed: false,
            jumps,
        }
    }

    #[test]
    fn view_totals_and_sort() {
        let tree = sample_tree();
        let owners = HashSet::from([OWNER, ALT]);
        let jumps = HashMap::new();
        let v = view(&tree, &options(&owners, vec![], "", &jumps), &types(), None);
        assert!(!v.narrowed);
        assert_eq!(v.locations.len(), 2);
        // The Archon's structure is worth most.
        assert_eq!(
            tree.locations[v.locations[0].location].key,
            LocationKey::Structure(STRUCTURE)
        );
        assert_eq!(v.count, 7);
        // Only the alt: one location.
        let owners = HashSet::from([ALT]);
        let v = view(&tree, &options(&owners, vec![], "", &jumps), &types(), None);
        assert_eq!(v.locations.len(), 1);
        assert_eq!(v.count, 1);
        assert_eq!(v.value, 400_000.0);
    }

    #[test]
    fn filters_and_search_show_only_hits() {
        let tree = sample_tree();
        let owners = HashSet::from([OWNER, ALT]);
        let jumps = HashMap::new();
        let ships = Filter {
            name: "Ships".into(),
            categories: vec![CATEGORY_SHIP],
            ..Filter::default()
        };
        let v = view(
            &tree,
            &options(&owners, vec![&ships], "", &jumps),
            &types(),
            None,
        );
        assert_eq!(
            v.count,
            2 + 1 + 1,
            "two Rifters (one with drones) and the Archon"
        );
        let caps = Filter {
            groups: vec![547],
            ..Filter::default()
        };
        let v = view(
            &tree,
            &options(&owners, vec![&caps], "", &jumps),
            &types(),
            None,
        );
        assert_eq!(v.locations.len(), 1);
        // Several chips: any may match.
        let v = view(
            &tree,
            &options(&owners, vec![&caps, &ships], "", &jumps),
            &types(),
            None,
        );
        assert_eq!(v.locations.len(), 2);

        // Search for drones: the Rifter is shown to hold them and opens.
        let v = view(
            &tree,
            &options(&owners, vec![], "hammer", &jumps),
            &types(),
            None,
        );
        assert_eq!(v.locations.len(), 1);
        let lv = &v.locations[0];
        assert_eq!(lv.count, 1);
        assert_eq!(lv.value, 2_000_000.0);
        let rifter = tree.locations[0].roots[0];
        assert!(lv.auto_open.contains(&rifter));
        assert!(lv.is_shown(rifter));
        assert!(!lv.is_shown(tree.locations[0].roots[1]));

        // A ship's given name is searched too.
        let v = view(
            &tree,
            &options(&owners, vec![], "SCOUT", &jumps),
            &types(),
            None,
        );
        assert_eq!(v.count, 2);

        // A place name match shows everything there.
        let v = view(
            &tree,
            &options(&owners, vec![], "caldari navy", &jumps),
            &types(),
            None,
        );
        assert_eq!(v.locations.len(), 1);
        assert_eq!(v.count, 6);

        // Minimum value.
        let pricey = Filter {
            min_isk: 1e9,
            ..Filter::default()
        };
        let v = view(
            &tree,
            &options(&owners, vec![&pricey], "", &jumps),
            &types(),
            None,
        );
        assert_eq!(v.count, 1);
    }

    #[test]
    fn rows_open_and_close() {
        let tree = sample_tree();
        let owners = HashSet::from([OWNER, ALT]);
        let jumps = HashMap::new();
        let mut opts = options(&owners, vec![], "", &jumps);
        opts.sort = SortBy::Count;
        let v = view(&tree, &opts, &types(), None);
        let owner_name = |id: u64| {
            if id == OWNER {
                "Main".to_string()
            } else {
                "Alt".to_string()
            }
        };
        let all_types = types();
        let type_name = |id: i64| {
            all_types
                .get(&id)
                .map(|t| t.name.clone())
                .unwrap_or_default()
        };
        let none_open = HashSet::new();
        let no_nodes = HashSet::new();
        let closed = rows(
            &tree,
            &v,
            &OpenState {
                locations: &none_open,
                nodes: &no_nodes,
            },
            &owner_name,
            &type_name,
        );
        assert_eq!(closed, vec![Row::Location(0), Row::Location(1)]);

        let jita_open = HashSet::from([LocationKey::Station(JITA_44)]);
        let open = rows(
            &tree,
            &v,
            &OpenState {
                locations: &jita_open,
                nodes: &no_nodes,
            },
            &owner_name,
            &type_name,
        );
        // Location, 3 big rows of 2 lines (alt's Rifter first: "alt" <
        // "main"), the hangar title and the loose Tritanium, then the
        // structure.
        assert_eq!(open.len(), 1 + 6 + 2 + 1);
        assert_eq!(
            open[1],
            Row::BigFirst {
                node: tree.locations[0].roots[3],
                depth: 1
            }
        );
        assert!(matches!(open[8], Row::Item { owner: true, .. }));

        // Open the Rifter: its drone bay.
        let rifter_open = HashSet::from([1_i64]);
        let deeper = rows(
            &tree,
            &v,
            &OpenState {
                locations: &jita_open,
                nodes: &rifter_open,
            },
            &owner_name,
            &type_name,
        );
        assert_eq!(deeper.len(), open.len() + 2);
        assert!(deeper.contains(&Row::Section {
            title: "Drone bay".into(),
            depth: 2
        }));
    }

    #[test]
    fn search_opens_rows_and_a_click_closes_them() {
        let tree = sample_tree();
        let owners = HashSet::from([OWNER, ALT]);
        let jumps = HashMap::new();
        let v = view(
            &tree,
            &options(&owners, vec![], "hammer", &jumps),
            &types(),
            None,
        );
        let owner_name = |_: u64| String::new();
        let type_name = |_: i64| String::new();
        let empty_l = HashSet::new();
        let empty_n = HashSet::new();
        let auto = rows(
            &tree,
            &v,
            &OpenState {
                locations: &empty_l,
                nodes: &empty_n,
            },
            &owner_name,
            &type_name,
        );
        // Location, the Rifter (2 lines), "Drone bay", the drones.
        assert_eq!(auto.len(), 5);
        let clicked = HashSet::from([LocationKey::Station(JITA_44)]);
        let closed = rows(
            &tree,
            &v,
            &OpenState {
                locations: &clicked,
                nodes: &empty_n,
            },
            &owner_name,
            &type_name,
        );
        assert_eq!(closed.len(), 1);
    }

    #[test]
    fn section_titles() {
        assert_eq!(section_of("HiSlot3"), (0, "High slots".to_string()));
        assert_eq!(section_of("DroneBay").1, "Drone bay");
        assert_eq!(section_of("SpecializedOreHold").1, "Ore hold");
        assert_eq!(section_of("FighterTube2").1, "Fighters");
    }
}
