//! Jita prices and what ore is worth.
//!
//! Ore is valued by what it reprocesses into, at Jita 4-4 prices:
//!
//! ```text
//! value of 1 unit = sum over its materials of
//!                   (material per unit) x (reprocessing yield) x (Jita price of the material)
//! ```
//!
//! The yield is 90.63% for ore, ice and moon ore and 95% for gas (both
//! settings). Gas can't be reprocessed: raw gas is worth its own Jita price,
//! and compressed gas is worth the raw gas it decompresses into (1:1 in the
//! SDE) times the gas yield.
//!
//! Prices come from ESI's public market orders for The Forge, keeping only
//! orders at the Jita 4-4 station: the highest buy and the lowest sell. They
//! are fetched on a background thread at most once an hour and cached in
//! the data folder, so a restart doesn't fetch them again.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

use anyhow::{Context, Result};
use eframe::egui;
use serde::{Deserialize, Serialize};

use crate::esi::{EsiClient, Fetched};
use crate::static_data::{ItemKind, StaticData};
use crate::storage::write_atomic;

/// The Forge, the region Jita is in.
const THE_FORGE: i64 = 10_000_002;
/// Jita IV - Moon 4 - Caldari Navy Assembly Plant ("Jita 4-4").
const JITA_4_4: i64 = 60_003_760;

/// Prices are refreshed when they are older than this.
pub const MAX_AGE_SECS: i64 = 60 * 60;
/// After a failed fetch, try again after this long.
const RETRY_SECS: u64 = 5 * 60;

/// The cache file in the data folder.
pub const CACHE_FILE: &str = "jita_prices.toml";

/// Best Jita 4-4 prices for one item. `None` if nobody buys or sells it there.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct MarketPrice {
    pub buy: Option<f64>,
    pub sell: Option<f64>,
}

/// Which Jita price to value ore with (a setting).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PriceSide {
    /// The highest buy order: what you get selling right away.
    #[default]
    Buy,
    /// The lowest sell order.
    Sell,
}

impl MarketPrice {
    pub fn get(&self, side: PriceSide) -> Option<f64> {
        match side {
            PriceSide::Buy => self.buy,
            PriceSide::Sell => self.sell,
        }
    }

    /// Combines the best prices of two pages of orders.
    fn merge(self, other: MarketPrice) -> MarketPrice {
        MarketPrice {
            buy: max_option(self.buy, other.buy),
            sell: min_option(self.sell, other.sell),
        }
    }
}

fn max_option(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    }
}

fn min_option(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// All prices from one fetch.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct JitaPrices {
    /// When they were fetched, in seconds since 1970 (UTC).
    pub fetched_unix: i64,
    pub by_type: HashMap<i64, MarketPrice>,
}

impl JitaPrices {
    pub fn price(&self, type_id: i64, side: PriceSide) -> Option<f64> {
        self.by_type.get(&type_id)?.get(side)
    }

    pub fn age_secs(&self, now_unix: i64) -> i64 {
        now_unix - self.fetched_unix
    }
}

/// One market order from ESI (only the fields we use).
#[derive(Debug, Clone, Deserialize)]
pub struct Order {
    pub is_buy_order: bool,
    pub location_id: i64,
    pub price: f64,
}

/// The best Jita 4-4 buy and sell price in a list of orders.
pub fn best_jita_prices(orders: &[Order]) -> MarketPrice {
    let mut best = MarketPrice::default();
    for order in orders.iter().filter(|o| o.location_id == JITA_4_4) {
        if order.is_buy_order {
            best.buy = max_option(best.buy, Some(order.price));
        } else {
            best.sell = min_option(best.sell, Some(order.price));
        }
    }
    best
}

/// Reprocessing yields, as fractions (0.9063 = 90.63%).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Yields {
    pub ore: f64,
    pub gas: f64,
}

impl Default for Yields {
    fn default() -> Self {
        Yields {
            ore: 0.9063,
            gas: 0.95,
        }
    }
}

/// What one unit of an item is worth, by name. `None` if the item is
/// unknown or has no reprocessing data. A material nobody trades at Jita
/// counts as worthless.
pub fn unit_value(
    name: &str,
    data: &StaticData,
    prices: &JitaPrices,
    side: PriceSide,
    yields: Yields,
) -> Option<f64> {
    let item = data.item_by_name(name)?;
    let materials = data.materials_per_unit(item.type_id)?;
    // Raw gas "decompresses" into itself (see `materials_per_unit`): it is
    // sold as it is, so it gets its full Jita price. Only compressed gas
    // loses the decompression share.
    let is_raw_gas = item.kind == ItemKind::Gas
        && materials
            .iter()
            .all(|&(material_id, _)| material_id == item.type_id);
    let yield_share = if is_raw_gas {
        1.0
    } else if item.kind == ItemKind::Gas {
        yields.gas
    } else {
        yields.ore
    };
    let value = materials
        .iter()
        .map(|&(material_id, per_unit)| {
            per_unit * yield_share * prices.price(material_id, side).unwrap_or(0.0)
        })
        .sum();
    Some(value)
}

/// The value of a pile of ore.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Valuation {
    pub isk: f64,
    /// Items that could not be valued (not counted in `isk`).
    pub unknown: Vec<String>,
}

/// Values units per item name, e.g. a session's `mined_units`.
pub fn value_units(
    units: &BTreeMap<String, u64>,
    data: &StaticData,
    prices: &JitaPrices,
    side: PriceSide,
    yields: Yields,
) -> Valuation {
    let mut valuation = Valuation::default();
    for (name, &count) in units {
        match unit_value(name, data, prices, side, yields) {
            Some(value) => valuation.isk += value * count as f64,
            None => valuation.unknown.push(name.clone()),
        }
    }
    valuation
}

// ---------------------------------------------------------------------------
// Cache file

/// The cache file as written to disk (TOML needs text keys, so a list).
#[derive(Serialize, Deserialize)]
struct CacheFile {
    fetched_unix: i64,
    prices: Vec<CachedPrice>,
}

#[derive(Serialize, Deserialize)]
struct CachedPrice {
    type_id: i64,
    #[serde(flatten)]
    price: MarketPrice,
}

fn prices_to_toml(prices: &JitaPrices) -> Result<String> {
    let mut rows: Vec<CachedPrice> = prices
        .by_type
        .iter()
        .map(|(&type_id, &price)| CachedPrice { type_id, price })
        .collect();
    rows.sort_by_key(|row| row.type_id);
    let file = CacheFile {
        fetched_unix: prices.fetched_unix,
        prices: rows,
    };
    Ok(toml::to_string(&file)?)
}

fn prices_from_toml(text: &str) -> Result<JitaPrices> {
    let file: CacheFile = toml::from_str(text)?;
    Ok(JitaPrices {
        fetched_unix: file.fetched_unix,
        by_type: file
            .prices
            .into_iter()
            .map(|row| (row.type_id, row.price))
            .collect(),
    })
}

fn load_cache(path: &Path) -> Option<JitaPrices> {
    let text = std::fs::read_to_string(path).ok()?;
    prices_from_toml(&text).ok()
}

// ---------------------------------------------------------------------------
// Background fetching

/// What the price thread sends to the module.
#[derive(Debug, Clone)]
pub enum PriceUpdate {
    Prices(JitaPrices),
    /// A fetch failed; the old prices (if any) stay in use.
    Failed(String),
}

/// Handle to the background thread that keeps Jita prices fresh. Dropping it
/// stops the thread at its next wait.
pub struct PriceFetcher {
    pub updates: Receiver<PriceUpdate>,
    // Never sent on; the thread notices when this is dropped.
    _stop: Sender<()>,
}

impl PriceFetcher {
    /// Starts the thread. `type_ids` are the materials to price (from
    /// `StaticData::material_ids`). `data_dir` holds the cache file.
    pub fn start(type_ids: Vec<i64>, data_dir: &Path, egui_ctx: egui::Context) -> PriceFetcher {
        let (update_tx, updates) = mpsc::channel();
        let (_stop, stop_rx) = mpsc::channel();
        let cache_path = data_dir.join(CACHE_FILE);
        // `move` hands everything the closure uses over to the new thread.
        std::thread::spawn(move || {
            price_thread(type_ids, cache_path, update_tx, stop_rx, egui_ctx);
        });
        PriceFetcher { updates, _stop }
    }
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn price_thread(
    type_ids: Vec<i64>,
    cache_path: PathBuf,
    updates: Sender<PriceUpdate>,
    stop: Receiver<()>,
    egui_ctx: egui::Context,
) {
    let send = |update: PriceUpdate| {
        let sent = updates.send(update).is_ok();
        egui_ctx.request_repaint();
        sent
    };

    let mut current = load_cache(&cache_path);
    if let Some(prices) = &current {
        send(PriceUpdate::Prices(prices.clone()));
    }
    let mut client = match EsiClient::new() {
        Ok(client) => client,
        Err(e) => {
            send(PriceUpdate::Failed(format!("{e:#}")));
            return;
        }
    };
    // What we learned from each request URL, so an unchanged page isn't
    // downloaded again.
    let mut etags: HashMap<String, CachedPage> = HashMap::new();

    loop {
        // Sleep until the prices are an hour old (or right away if they
        // are, or if some material is missing from the cache).
        let complete = current
            .as_ref()
            .is_some_and(|p| type_ids.iter().all(|id| p.by_type.contains_key(id)));
        let wait_secs = match &current {
            Some(prices) if complete => (MAX_AGE_SECS - prices.age_secs(now_unix())).max(0),
            _ => 0,
        };
        if !wait(&stop, wait_secs as u64) {
            return;
        }

        match fetch_all(&mut client, &type_ids, &mut etags) {
            Ok(by_type) => {
                let prices = JitaPrices {
                    fetched_unix: now_unix(),
                    by_type,
                };
                if let Ok(text) = prices_to_toml(&prices) {
                    // A failed write only costs a refetch after a restart.
                    let _ = write_atomic(&cache_path, &text);
                }
                if !send(PriceUpdate::Prices(prices.clone())) {
                    return;
                }
                current = Some(prices);
            }
            Err(e) => {
                if !send(PriceUpdate::Failed(format!("{e:#}"))) {
                    return;
                }
                if !wait(&stop, RETRY_SECS) {
                    return;
                }
                // Retry even if the old prices aren't an hour old yet.
                if let Some(prices) = &mut current {
                    prices.fetched_unix = 0;
                }
            }
        }
    }
}

/// Waits `secs` seconds. Returns false if the module was dropped meanwhile,
/// meaning the thread should end.
fn wait(stop: &Receiver<()>, secs: u64) -> bool {
    if secs == 0 {
        return true;
    }
    // Nothing is ever sent on `stop`: this waits for the timeout, or ends
    // early when the sender is dropped ("disconnected").
    !matches!(
        stop.recv_timeout(Duration::from_secs(secs)),
        Err(RecvTimeoutError::Disconnected)
    )
}

/// What one page of orders gave last time, kept with its ETag.
struct CachedPage {
    etag: String,
    best: MarketPrice,
    pages: u32,
}

/// Fetches the best Jita prices for every type, one type (and page) at a time.
fn fetch_all(
    client: &mut EsiClient,
    type_ids: &[i64],
    etags: &mut HashMap<String, CachedPage>,
) -> Result<HashMap<i64, MarketPrice>> {
    let mut by_type = HashMap::new();
    for &type_id in type_ids {
        let mut best = MarketPrice::default();
        let mut page = 1;
        loop {
            let path =
                format!("/markets/{THE_FORGE}/orders?order_type=all&type_id={type_id}&page={page}");
            let known_etag = etags.get(&path).map(|cached| cached.etag.clone());
            let (page_best, pages) =
                match client.get_json::<Vec<Order>>(&path, known_etag.as_deref())? {
                    Fetched::Data { value, etag, pages } => {
                        let page_best = best_jita_prices(&value);
                        if let Some(etag) = etag {
                            let cached = CachedPage {
                                etag,
                                best: page_best,
                                pages,
                            };
                            etags.insert(path.clone(), cached);
                        }
                        (page_best, pages)
                    }
                    // Unchanged: reuse what we worked out last time.
                    Fetched::NotModified => {
                        let cached = etags
                            .get(&path)
                            .context("ESI answered 304 to a request without an ETag")?;
                        (cached.best, cached.pages)
                    }
                };
            best = best.merge(page_best);
            if page >= pages {
                break;
            }
            page += 1;
        }
        by_type.insert(type_id, best);
    }
    Ok(by_type)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::static_data::tests::sample;

    fn order(buy: bool, location: i64, price: f64) -> Order {
        Order {
            is_buy_order: buy,
            location_id: location,
            price,
        }
    }

    #[test]
    fn best_prices_only_at_jita_4_4() {
        let orders = vec![
            order(true, JITA_4_4, 3.5),
            order(true, JITA_4_4, 3.9),
            order(true, 60_008_494, 9.0), // Amarr, ignored
            order(false, JITA_4_4, 4.2),
            order(false, JITA_4_4, 4.1),
            order(false, 60_008_494, 1.0),
        ];
        assert_eq!(
            best_jita_prices(&orders),
            MarketPrice {
                buy: Some(3.9),
                sell: Some(4.1)
            }
        );
        assert_eq!(best_jita_prices(&[]), MarketPrice::default());
    }

    #[test]
    fn pages_merge() {
        let a = MarketPrice {
            buy: Some(3.0),
            sell: None,
        };
        let b = MarketPrice {
            buy: Some(2.0),
            sell: Some(5.0),
        };
        assert_eq!(
            a.merge(b),
            MarketPrice {
                buy: Some(3.0),
                sell: Some(5.0)
            }
        );
    }

    fn prices() -> JitaPrices {
        let mut by_type = HashMap::new();
        by_type.insert(
            34,
            MarketPrice {
                buy: Some(4.0),
                sell: Some(5.0),
            },
        );
        by_type.insert(
            30370,
            MarketPrice {
                buy: Some(1_000.0),
                sell: None,
            },
        );
        JitaPrices {
            fetched_unix: 1_000,
            by_type,
        }
    }

    #[test]
    fn ore_value_uses_reprocessing_and_yield() {
        let data = sample();
        let yields = Yields::default();
        // 4 Tritanium per Veldspar x 90.63% x 4 ISK.
        let value = unit_value("Veldspar", &data, &prices(), PriceSide::Buy, yields).unwrap();
        assert!((value - 4.0 * 0.9063 * 4.0).abs() < 1e-9);
        // Compressed Veldspar gives the same per unit.
        let compressed = unit_value(
            "Compressed Veldspar",
            &data,
            &prices(),
            PriceSide::Buy,
            yields,
        )
        .unwrap();
        assert!((compressed - value).abs() < 1e-9);
        let sell = unit_value("Veldspar", &data, &prices(), PriceSide::Sell, yields).unwrap();
        assert!((sell - 4.0 * 0.9063 * 5.0).abs() < 1e-9);
    }

    #[test]
    fn raw_gas_full_price_compressed_gas_uses_yield() {
        let data = sample();
        let yields = Yields::default();
        let raw = unit_value("Fullerite-C50", &data, &prices(), PriceSide::Buy, yields).unwrap();
        assert!((raw - 1_000.0).abs() < 1e-9);
        let compressed = unit_value(
            "Compressed Fullerite-C50",
            &data,
            &prices(),
            PriceSide::Buy,
            yields,
        )
        .unwrap();
        assert!((compressed - 950.0).abs() < 1e-9);
    }

    #[test]
    fn unknown_items_and_missing_prices() {
        let data = sample();
        let yields = Yields::default();
        // Heavy Water has no price: White Glaze is worth 0, not unknown.
        assert_eq!(
            unit_value("White Glaze", &data, &prices(), PriceSide::Buy, yields),
            Some(0.0)
        );
        let mut units = BTreeMap::new();
        units.insert("Veldspar".to_string(), 1_000);
        units.insert("Mystery Ore".to_string(), 5);
        let valuation = value_units(&units, &data, &prices(), PriceSide::Buy, yields);
        assert!((valuation.isk - 1_000.0 * 4.0 * 0.9063 * 4.0).abs() < 1e-6);
        assert_eq!(valuation.unknown, vec!["Mystery Ore"]);
    }

    #[test]
    fn cache_round_trip() {
        let original = prices();
        let text = prices_to_toml(&original).unwrap();
        assert_eq!(prices_from_toml(&text).unwrap(), original);
    }

    #[test]
    fn wait_ends_when_module_is_dropped() {
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        drop(stop_tx);
        let started = std::time::Instant::now();
        assert!(!wait(&stop_rx, 60));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    /// Talks to the real ESI, so it is skipped by default. Run it with
    /// `cargo test live_jita_prices -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_jita_prices() {
        let mut client = EsiClient::new().unwrap();
        let mut etags = HashMap::new();
        // Tritanium and Fullerite-C50.
        let prices = fetch_all(&mut client, &[34, 30370], &mut etags).unwrap();
        println!("{prices:?}");
        assert!(prices[&34].buy.is_some() && prices[&34].sell.is_some());
        // Again: every page should now come back as "not modified" or new data.
        let again = fetch_all(&mut client, &[34], &mut etags).unwrap();
        assert!(again[&34].buy.is_some());
    }
}
