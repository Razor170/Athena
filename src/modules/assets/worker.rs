//! The Assets module's background work: fetching each character's assets
//! from ESI, naming ships, containers, stations and structures, reading
//! item types from `static.sqlite`, and fetching prices. Nothing here runs
//! on the UI thread; results go to the module as [`Event`]s over a
//! channel, and each one asks egui for one repaint.
//!
//! Assets are only fetched while the Assets tab is open (the module sends
//! [`Command::FetchDue`]), and a character only again once ESI has newer
//! data (assets are cached for an hour). Prices are fetched at most once
//! an hour. Everything is saved in `assets-cache.json`, so the list shows
//! right after a restart.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use anyhow::Result;
use eframe::egui;
use reqwest::blocking::Client;
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};

use super::config::PriceSource;
use super::data::{CharacterAssets, FIRST_STRUCTURE_ID, PlaceInfo};
use super::sde::{self, CATEGORY_SHIP, Catalog, TypeInfo};
use crate::auth::{RefreshError, TokenStore, now_unix};
use crate::esi::{EsiClient, Fetched, HttpError};
use crate::modules::fittings::assets::EsiAsset;
use crate::modules::mining::prices::{CachedPage, PriceSide, fetch_all};

const CACHE_FILE: &str = "assets-cache.json";
/// `/assets/names` takes at most this many ids.
const IDS_PER_REQUEST: usize = 1000;
/// If ESI sends no cache time, ask again after this long.
const DEFAULT_CACHE_SECS: i64 = 3600;
/// Prices are fetched again when older than this.
pub const PRICES_EVERY_SECS: i64 = 60 * 60;
/// After a failure, try again after this long.
const RETRY_SECS: i64 = 5 * 60;

/// Prices of all item types the characters own.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Prices {
    pub source: PriceSource,
    /// When they were fetched, Unix seconds.
    pub fetched_at: i64,
    pub by_type: HashMap<i64, f64>,
}

/// What `assets-cache.json` holds.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Cache {
    pub characters: BTreeMap<u64, CharacterAssets>,
    /// Station and structure id -> name and system.
    pub places: BTreeMap<i64, PlaceInfo>,
    pub prices: Option<Prices>,
}

impl Cache {
    pub fn file_path(data_dir: &Path) -> PathBuf {
        data_dir.join(CACHE_FILE)
    }

    /// The saved cache, or an empty one if there is none or it's broken
    /// (it only holds what ESI sends again anyway).
    fn load(path: &Path) -> Cache {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    fn save(&self, path: &Path) -> Result<()> {
        let text = serde_json::to_string(self)?;
        crate::storage::write_atomic(path, &text)
    }
}

/// What the module asks the worker to do.
pub enum Command {
    /// These are the logged-in characters now.
    SetCharacters(Vec<u64>),
    /// The tab is open: fetch every character whose data is old.
    FetchDue,
    /// Fetch everything now.
    RefreshNow,
    /// Use this price source (fetches prices if they are from another).
    SetPriceSource(PriceSource),
    /// Find out which system this character is in (for the route).
    Locate(u64),
}

/// What the worker tells the module.
pub enum Event {
    /// Everything saved from earlier runs, and the groups and categories.
    /// Sent once, first.
    Loaded {
        cache: Box<Cache>,
        catalog: Box<Catalog>,
    },
    /// Fetching this character started.
    Started(u64),
    Assets {
        character_id: u64,
        assets: Box<CharacterAssets>,
    },
    /// Item types read from `static.sqlite`.
    Types(HashMap<i64, TypeInfo>),
    Places(Vec<(i64, PlaceInfo)>),
    Prices(Box<Prices>),
    /// Fetching prices started (true) or ended (false).
    PricesBusy(bool),
    /// Where a character is.
    Location {
        character_id: u64,
        system_id: i64,
    },
    /// Something went wrong for one character (`None`: it works again).
    CharacterProblem {
        character_id: u64,
        problem: Option<String>,
    },
    /// Something went wrong that isn't about one character.
    Problem(String),
}

/// The handle the module keeps. Dropping it stops the worker thread.
pub struct Worker {
    commands: Sender<Command>,
    pub events: Receiver<Event>,
}

impl Worker {
    pub fn start(
        egui_ctx: &egui::Context,
        tokens: Arc<TokenStore>,
        cache_path: PathBuf,
        price_source: PriceSource,
    ) -> Worker {
        let (commands, command_receiver) = mpsc::channel();
        let (event_sender, events) = mpsc::channel();
        let ctx = egui_ctx.clone();
        thread::spawn(move || {
            let send = |event: Event| {
                // Fails only when the module is gone.
                let _ = event_sender.send(event);
                ctx.request_repaint();
            };
            let cache = Cache::load(&cache_path);
            // A separate read-only connection to static.sqlite, used only
            // on this thread.
            let db = crate::static_data::StaticData::find_file().and_then(|path| {
                Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()
            });
            let catalog = db
                .as_ref()
                .and_then(|db| sde::catalog(db).ok())
                .unwrap_or_default();
            send(Event::Loaded {
                cache: Box::new(cache.clone()),
                catalog: Box::new(catalog),
            });
            match Fetcher::new(tokens, cache_path, cache, db, price_source) {
                Ok(mut fetcher) => {
                    fetcher.send_cached_types(&send);
                    fetcher.run(&command_receiver, &send);
                }
                Err(e) => send(Event::Problem(format!(
                    "Assets can't start: {e:#}. Nothing will be fetched."
                ))),
            }
        });
        Worker { commands, events }
    }

    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }
}

/// Owns the ESI client, the token store and the cache. Lives on the
/// worker thread only.
struct Fetcher {
    esi: EsiClient,
    http: Client,
    tokens: Arc<TokenStore>,
    cache_path: PathBuf,
    cache: Cache,
    db: Option<Connection>,
    price_source: PriceSource,
    characters: Vec<u64>,
    /// Character -> when it may be fetched again.
    due: HashMap<u64, i64>,
    /// When prices may be fetched again after a failure.
    prices_retry_at: i64,
    /// Types already read from `static.sqlite` (or not in it).
    known_types: HashSet<i64>,
    /// Types sold on the market, for the Jita prices.
    market_types: HashSet<i64>,
    /// Ship types, whose given names are looked up.
    ship_types: HashSet<i64>,
    /// Structures a character may not look up (no docking access).
    structure_denied: HashSet<(i64, u64)>,
    /// ETags of the Jita order pages, so unchanged pages aren't
    /// downloaded again.
    order_pages: HashMap<String, CachedPage>,
}

impl Fetcher {
    fn new(
        tokens: Arc<TokenStore>,
        cache_path: PathBuf,
        cache: Cache,
        db: Option<Connection>,
        price_source: PriceSource,
    ) -> Result<Fetcher> {
        let due = cache
            .characters
            .iter()
            .map(|(id, assets)| (*id, assets.next_at))
            .collect();
        Ok(Fetcher {
            esi: EsiClient::new()?,
            http: crate::auth::sso::http_client()?,
            tokens,
            cache_path,
            cache,
            db,
            price_source,
            characters: Vec::new(),
            due,
            prices_retry_at: 0,
            known_types: HashSet::new(),
            market_types: HashSet::new(),
            ship_types: HashSet::new(),
            structure_denied: HashSet::new(),
            order_pages: HashMap::new(),
        })
    }

    fn run(&mut self, commands: &Receiver<Command>, send: &impl Fn(Event)) {
        // `recv` waits without using CPU until a command comes; it fails
        // when the module is gone, which ends the thread.
        while let Ok(command) = commands.recv() {
            match command {
                Command::SetCharacters(ids) => {
                    self.characters = ids;
                    // Logged-out characters' assets go from the cache.
                    let before = self.cache.characters.len();
                    let keep = &self.characters;
                    self.cache.characters.retain(|id, _| keep.contains(id));
                    if self.cache.characters.len() != before {
                        let _ = self.cache.save(&self.cache_path);
                    }
                }
                Command::FetchDue => self.fetch_due(send),
                Command::RefreshNow => {
                    self.due.clear();
                    self.fetch_due(send);
                }
                Command::SetPriceSource(source) => {
                    self.price_source = source;
                    self.prices_retry_at = 0;
                    self.prices_if_due(send);
                }
                Command::Locate(id) => self.locate(id, send),
            }
        }
    }

    fn fetch_due(&mut self, send: &impl Fn(Event)) {
        let now = now_unix();
        let ids: Vec<u64> = self
            .characters
            .iter()
            .copied()
            .filter(|id| self.due.get(id).is_none_or(|at| *at <= now))
            .collect();
        let mut changed = false;
        for id in ids {
            send(Event::Started(id));
            let problem = match self.tokens.access_token(&self.http, id) {
                Err(RefreshError::LoginExpired) => {
                    Some("Login expired. Log in again on the Characters tab.".to_string())
                }
                Err(RefreshError::Other(e)) => Some(format!("Couldn't get a token: {e:#}")),
                Ok(token) => match self.fetch(id, &token, send) {
                    Ok(assets) => {
                        self.due.insert(id, assets.next_at);
                        self.cache.characters.insert(id, assets.clone());
                        changed = true;
                        send(Event::Assets {
                            character_id: id,
                            assets: Box::new(assets),
                        });
                        None
                    }
                    Err(e) => Some(format!("Assets: {e:#}")),
                },
            };
            if problem.is_some() {
                self.due.insert(id, now + RETRY_SECS);
            }
            send(Event::CharacterProblem {
                character_id: id,
                problem,
            });
        }
        if changed && let Err(e) = self.cache.save(&self.cache_path) {
            send(Event::Problem(format!(
                "Couldn't save {}: {e:#}",
                self.cache_path.display()
            )));
        }
        self.prices_if_due(send);
    }

    /// One character's assets with the names of ships and containers;
    /// also looks up types and places it hasn't seen.
    fn fetch(&mut self, id: u64, token: &str, send: &impl Fn(Event)) -> Result<CharacterAssets> {
        let now = now_unix();
        let path = format!("/characters/{id}/assets");
        let (mut items, pages) = match self
            .esi
            .get_json_authed::<Vec<EsiAsset>>(&path, token, None)?
        {
            Fetched::Data { value, pages, .. } => (value, pages),
            Fetched::NotModified => (Vec::new(), 1),
        };
        // Page 1's headers say how old the data is and when ESI has more.
        let age = self.esi.last_data_age.map_or(0, |d| d.as_secs() as i64);
        let expires = self
            .esi
            .last_expires_in
            .map_or(DEFAULT_CACHE_SECS, |d| d.as_secs() as i64);
        for page in 2..=pages {
            let path = format!("/characters/{id}/assets?page={page}");
            if let Fetched::Data { value, .. } = self
                .esi
                .get_json_authed::<Vec<EsiAsset>>(&path, token, None)?
            {
                items.extend(value);
            }
        }

        self.read_types(&items, send);
        let names = self.item_names(id, token, &items);
        self.look_up_places(id, token, &items, send);
        Ok(CharacterAssets {
            items,
            names,
            data_from: now - age,
            next_at: now + expires.max(60),
        })
    }

    /// Reads the types of the cached assets, once at start.
    fn send_cached_types(&mut self, send: &impl Fn(Event)) {
        let items: Vec<EsiAsset> = self
            .cache
            .characters
            .values()
            .flat_map(|c| c.items.iter().cloned())
            .collect();
        self.read_types(&items, send);
    }

    fn read_types(&mut self, items: &[EsiAsset], send: &impl Fn(Event)) {
        let Some(db) = &self.db else {
            return;
        };
        let mut ids: Vec<i64> = items
            .iter()
            .map(|a| a.type_id)
            .filter(|id| !self.known_types.contains(id))
            .collect();
        ids.sort_unstable();
        ids.dedup();
        if ids.is_empty() {
            return;
        }
        match sde::types(db, &ids) {
            Ok(types) => {
                self.known_types.extend(ids);
                self.market_types
                    .extend(types.iter().filter(|(_, t)| t.market).map(|(id, _)| *id));
                self.ship_types.extend(
                    types
                        .iter()
                        .filter(|(_, t)| t.category_id == CATEGORY_SHIP)
                        .map(|(id, _)| *id),
                );
                send(Event::Types(types));
            }
            Err(e) => send(Event::Problem(format!(
                "Couldn't read item types from static.sqlite: {e:#}"
            ))),
        }
    }

    /// The names given to ships and containers in game.
    fn item_names(&mut self, id: u64, token: &str, items: &[EsiAsset]) -> BTreeMap<i64, String> {
        #[derive(Deserialize)]
        struct Named {
            item_id: i64,
            name: String,
        }
        #[derive(Serialize)]
        struct Ids<'a>(&'a [i64]);
        // Ships, and anything that holds something.
        let holders: HashSet<i64> = items.iter().map(|a| a.location_id).collect();
        let ids: Vec<i64> = items
            .iter()
            .filter(|a| {
                a.is_singleton
                    && (holders.contains(&a.item_id) || self.ship_types.contains(&a.type_id))
            })
            .map(|a| a.item_id)
            .collect();
        let mut out = BTreeMap::new();
        for chunk in ids.chunks(IDS_PER_REQUEST) {
            let path = format!("/characters/{id}/assets/names");
            // The token goes in a header only (see `EsiClient`).
            let Ok(names) = self
                .esi
                .post_json_authed::<_, Vec<Named>>(&path, token, &Ids(chunk))
            else {
                // Names are a nice-to-have: the type name stands in. Keep
                // the ones we had.
                if let Some(old) = self.cache.characters.get(&id) {
                    out.extend(
                        old.names
                            .iter()
                            .filter(|(item, _)| chunk.contains(item))
                            .map(|(k, v)| (*k, v.clone())),
                    );
                }
                continue;
            };
            for n in names {
                // ESI answers "None" for items without a name.
                if !n.name.is_empty() && n.name != "None" {
                    out.insert(n.item_id, n.name);
                }
            }
        }
        out
    }

    /// Names and systems of the stations and structures not known yet.
    fn look_up_places(
        &mut self,
        character_id: u64,
        token: &str,
        items: &[EsiAsset],
        send: &impl Fn(Event),
    ) {
        let in_items: HashSet<i64> = items.iter().map(|a| a.item_id).collect();
        let mut stations: Vec<i64> = Vec::new();
        let mut structures: Vec<i64> = Vec::new();
        for a in items {
            if in_items.contains(&a.location_id) || self.cache.places.contains_key(&a.location_id) {
                continue;
            }
            match a.location_type.as_str() {
                "station" => stations.push(a.location_id),
                "item" if a.location_id >= FIRST_STRUCTURE_ID => structures.push(a.location_id),
                _ => {}
            }
        }
        stations.sort_unstable();
        stations.dedup();
        structures.sort_unstable();
        structures.dedup();
        structures.retain(|s| !self.structure_denied.contains(&(*s, character_id)));

        let mut found: Vec<(i64, PlaceInfo)> = Vec::new();
        let from_file = match &self.db {
            Some(db) => sde::stations(db, &stations).unwrap_or_default(),
            None => HashMap::new(),
        };
        for id in stations {
            match from_file.get(&id) {
                Some(s) => found.push((
                    id,
                    PlaceInfo {
                        name: s.name.clone(),
                        system_id: Some(s.system_id),
                    },
                )),
                // Not in the SDE (a new station): ask ESI.
                None => {
                    if let Some(place) = self.station_from_esi(id) {
                        found.push((id, place));
                    }
                }
            }
        }
        for id in structures {
            match self.structure(id, token) {
                Ok(Some(place)) => found.push((id, place)),
                Ok(None) => {
                    self.structure_denied.insert((id, character_id));
                }
                // ESI trouble: try again on the next fetch.
                Err(()) => {}
            }
        }
        if !found.is_empty() {
            self.cache.places.extend(found.iter().cloned());
            send(Event::Places(found));
        }
    }

    fn station_from_esi(&mut self, id: i64) -> Option<PlaceInfo> {
        #[derive(Deserialize)]
        struct Station {
            name: String,
            system_id: i64,
        }
        match self
            .esi
            .get_json::<Station>(&format!("/universe/stations/{id}"), None)
        {
            Ok(Fetched::Data { value, .. }) => Some(PlaceInfo {
                name: value.name,
                system_id: Some(value.system_id),
            }),
            _ => None,
        }
    }

    /// A player structure's name and system. `Ok(None)` when the
    /// character may not see it (403) or it is gone (404); `Err` when ESI
    /// had trouble and it is worth trying again later.
    fn structure(&mut self, id: i64, token: &str) -> Result<Option<PlaceInfo>, ()> {
        #[derive(Deserialize)]
        struct Structure {
            name: String,
            solar_system_id: i64,
        }
        let path = format!("/universe/structures/{id}");
        match self.esi.get_json_authed::<Structure>(&path, token, None) {
            Ok(Fetched::Data { value, .. }) => Ok(Some(PlaceInfo {
                name: value.name,
                system_id: Some(value.solar_system_id),
            })),
            Ok(Fetched::NotModified) => Err(()),
            Err(e) if is_status(&e, 403) || is_status(&e, 404) => Ok(None),
            Err(_) => Err(()),
        }
    }

    /// Fetches prices when they are old, from another source, or (for
    /// Jita) missing types the characters now own.
    fn prices_if_due(&mut self, send: &impl Fn(Event)) {
        let now = now_unix();
        if self.cache.characters.is_empty() || now < self.prices_retry_at {
            return;
        }
        let wanted: Vec<i64> = {
            let mut ids: Vec<i64> = self.market_types.iter().copied().collect();
            ids.sort_unstable();
            ids
        };
        let fresh = self.cache.prices.as_ref().is_some_and(|p| {
            p.source == self.price_source
                && now - p.fetched_at < PRICES_EVERY_SECS
                && (p.source == PriceSource::Average
                    || wanted.iter().all(|id| p.by_type.contains_key(id)))
        });
        if fresh {
            return;
        }
        send(Event::PricesBusy(true));
        let result = match self.price_source {
            PriceSource::Average => self.average_prices(),
            PriceSource::JitaSell => self.jita_prices(&wanted, PriceSide::Sell),
            PriceSource::JitaBuy => self.jita_prices(&wanted, PriceSide::Buy),
        };
        send(Event::PricesBusy(false));
        match result {
            Ok(by_type) => {
                let prices = Prices {
                    source: self.price_source,
                    fetched_at: now,
                    by_type,
                };
                self.cache.prices = Some(prices.clone());
                let _ = self.cache.save(&self.cache_path);
                send(Event::Prices(Box::new(prices)));
            }
            Err(e) => {
                self.prices_retry_at = now + RETRY_SECS;
                send(Event::Problem(format!("Prices: {e:#}")));
            }
        }
    }

    /// EVE's average price of every item, in one request.
    fn average_prices(&mut self) -> Result<HashMap<i64, f64>> {
        #[derive(Deserialize)]
        struct MarketPrice {
            type_id: i64,
            average_price: Option<f64>,
            adjusted_price: Option<f64>,
        }
        let list = match self
            .esi
            .get_json::<Vec<MarketPrice>>("/markets/prices", None)?
        {
            Fetched::Data { value, .. } => value,
            Fetched::NotModified => Vec::new(),
        };
        Ok(list
            .into_iter()
            .filter_map(|p| Some((p.type_id, p.average_price.or(p.adjusted_price)?)))
            .collect())
    }

    /// Jita 4-4 prices, one type at a time (the Mining module's code).
    fn jita_prices(&mut self, ids: &[i64], side: PriceSide) -> Result<HashMap<i64, f64>> {
        let best = fetch_all(&mut self.esi, ids, &mut self.order_pages)?;
        Ok(best
            .into_iter()
            .filter_map(|(id, price)| Some((id, price.get(side)?)))
            .collect())
    }

    /// Where a character is, for the route.
    fn locate(&mut self, id: u64, send: &impl Fn(Event)) {
        #[derive(Deserialize)]
        struct Location {
            solar_system_id: i64,
        }
        let Ok(token) = self.tokens.access_token(&self.http, id) else {
            return;
        };
        let path = format!("/characters/{id}/location");
        if let Ok(Fetched::Data { value, .. }) =
            self.esi.get_json_authed::<Location>(&path, &token, None)
        {
            send(Event::Location {
                character_id: id,
                system_id: value.solar_system_id,
            });
        }
    }
}

/// Whether `error` is ESI answering with this HTTP status.
fn is_status(error: &anyhow::Error, status: u16) -> bool {
    error
        .downcast_ref::<HttpError>()
        .is_some_and(|e| e.status == status)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_round_trips_through_json() {
        let mut cache = Cache::default();
        cache.places.insert(
            60003760,
            PlaceInfo {
                name: "Jita IV - Moon 4 - Caldari Navy Assembly Plant".into(),
                system_id: Some(30000142),
            },
        );
        cache.prices = Some(Prices {
            source: PriceSource::JitaSell,
            fetched_at: 1_700_000_000,
            by_type: HashMap::from([(34, 4.5)]),
        });
        cache.characters.insert(
            90000001,
            CharacterAssets {
                names: BTreeMap::from([(1, "Scout".into())]),
                data_from: 5,
                next_at: 10,
                ..CharacterAssets::default()
            },
        );
        let text = serde_json::to_string(&cache).unwrap();
        assert_eq!(serde_json::from_str::<Cache>(&text).unwrap(), cache);
    }
}
