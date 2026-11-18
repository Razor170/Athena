//! The Skills module's background work: fetching each character's skills,
//! skill queue and attributes from ESI. Nothing here runs on the UI
//! thread; results go to the module as [`Event`]s over a channel, and each
//! batch asks egui for one repaint.
//!
//! Each character is fetched every few minutes (the Settings view, 15 by
//! default), or when ESI says its copy expires, whichever is later. Time
//! left counts down in the UI from the finish dates, so no ESI call is
//! needed for that.
//!
//! The Jita prices of skill injectors (for the queue footer) are fetched
//! at most once an hour from ESI's public market orders.
//!
//! Everything fetched is saved in `skills-cache.json`, so Athena shows the
//! skills right after a restart, before ESI answers.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use eframe::egui;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};

use super::data::{Attributes, CharacterSkills, EsiQueueEntry, EsiSkills};
use crate::auth::{RefreshError, TokenStore, now_unix};
use crate::esi::{EsiClient, Fetched};
use crate::modules::mining::prices::{Order, best_jita_prices};

/// The longest the thread sleeps before looking at the schedule again.
const MAX_SLEEP: Duration = Duration::from_secs(60);
const CACHE_FILE: &str = "skills-cache.json";
/// Injector prices are fetched again when older than this.
const PRICES_EVERY_SECS: i64 = 60 * 60;
/// After a failed price fetch, try again after this long.
const PRICES_RETRY_SECS: i64 = 5 * 60;
/// The Forge, the region Jita is in.
const THE_FORGE: i64 = 10_000_002;
/// Skill injector type ids (SDE).
pub const LARGE_INJECTOR: i64 = 40_520;
pub const SMALL_INJECTOR: i64 = 45_635;

/// The lowest Jita 4-4 sell price of each injector: what buying one costs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct InjectorPrices {
    pub large: Option<f64>,
    pub small: Option<f64>,
    /// When they were fetched, Unix seconds.
    pub fetched_at: i64,
}

/// What the module asks the worker to do.
pub enum Command {
    /// These are the logged-in characters now. New ones get fetched at
    /// once; missing ones aren't fetched any more.
    SetCharacters(Vec<u64>),
    /// Fetch everything now.
    RefreshNow,
    /// The check interval changed (seconds).
    SetPollSecs(i64),
}

/// What the worker tells the module.
pub enum Event {
    /// Everything saved from earlier runs. Sent once, first.
    Loaded(Box<Cache>),
    /// One character's skills, as of now.
    Skills {
        character_id: u64,
        skills: Box<CharacterSkills>,
    },
    /// New injector prices.
    Prices(InjectorPrices),
    /// A round of fetching ended at this time.
    Checked(i64),
    /// Something went wrong for one character (`None`: it works again).
    CharacterProblem {
        character_id: u64,
        problem: Option<String>,
    },
    /// Something went wrong that isn't about one character.
    Problem(String),
}

/// What `skills-cache.json` holds.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Cache {
    /// Character id -> their skills.
    pub characters: BTreeMap<u64, CharacterSkills>,
    pub injector_prices: Option<InjectorPrices>,
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
        poll_secs: i64,
    ) -> Worker {
        let (commands, command_receiver) = mpsc::channel();
        let (event_sender, events) = mpsc::channel();
        let ctx = egui_ctx.clone();
        thread::spawn(move || {
            let send = |event: Event| {
                // Fails only when the module is gone; then there's nobody
                // to tell anyway.
                let _ = event_sender.send(event);
                ctx.request_repaint();
            };
            let cache = Cache::load(&cache_path);
            send(Event::Loaded(Box::new(cache.clone())));
            match Fetcher::new(tokens, cache_path, cache, poll_secs) {
                Ok(mut fetcher) => fetcher.run(&command_receiver, &send),
                Err(e) => send(Event::Problem(format!(
                    "Skills can't start: {e:#}. Nothing will be fetched."
                ))),
            }
        });
        Worker { commands, events }
    }

    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }
}

/// Owns the ESI client, the token store, the cache and the schedule.
/// Lives on the worker thread only.
struct Fetcher {
    esi: EsiClient,
    http: Client,
    tokens: Arc<TokenStore>,
    cache_path: PathBuf,
    cache: Cache,
    poll_secs: i64,
    /// Character id -> when it's due next (Unix seconds).
    due: HashMap<u64, i64>,
    /// When the injector prices are due next.
    prices_due: i64,
}

impl Fetcher {
    fn new(
        tokens: Arc<TokenStore>,
        cache_path: PathBuf,
        cache: Cache,
        poll_secs: i64,
    ) -> Result<Fetcher> {
        Ok(Fetcher {
            esi: EsiClient::new()?,
            http: crate::auth::sso::http_client()?,
            tokens,
            cache_path,
            poll_secs,
            prices_due: cache
                .injector_prices
                .map_or(0, |p| p.fetched_at + PRICES_EVERY_SECS),
            cache,
            due: HashMap::new(),
        })
    }

    fn run(&mut self, commands: &Receiver<Command>, send: &impl Fn(Event)) {
        loop {
            let wait = self.time_to_next_job();
            match commands.recv_timeout(wait) {
                Ok(Command::SetCharacters(ids)) => {
                    self.due.retain(|id, _| ids.contains(id));
                    for id in ids {
                        // New characters are due at once.
                        self.due.entry(id).or_insert(0);
                    }
                }
                Ok(Command::RefreshNow) => self.due.values_mut().for_each(|due| *due = 0),
                Ok(Command::SetPollSecs(secs)) => {
                    // Shorter than before: don't wait out the old interval.
                    let latest = now_unix() + secs;
                    self.due
                        .values_mut()
                        .for_each(|due| *due = (*due).min(latest));
                    self.poll_secs = secs;
                }
                Err(RecvTimeoutError::Timeout) => {}
                // The module is gone: stop.
                Err(RecvTimeoutError::Disconnected) => return,
            }
            self.do_due_jobs(send);
        }
    }

    fn time_to_next_job(&self) -> Duration {
        let now = now_unix();
        match self.due.values().min() {
            None => MAX_SLEEP,
            Some(due) => Duration::from_secs((due - now).clamp(0, 60) as u64).min(MAX_SLEEP),
        }
    }

    fn do_due_jobs(&mut self, send: &impl Fn(Event)) {
        let now = now_unix();
        // Prices only matter once there are characters to show.
        if !self.due.is_empty() && self.prices_due <= now {
            match self.fetch_prices(now) {
                Ok(prices) => {
                    self.prices_due = now + PRICES_EVERY_SECS;
                    self.cache.injector_prices = Some(prices);
                    // Not worth a message if it fails: the next save
                    // writes them too.
                    let _ = self.cache.save(&self.cache_path);
                    send(Event::Prices(prices));
                }
                // Quiet: the footer just shows no ISK until it works.
                Err(_) => self.prices_due = now + PRICES_RETRY_SECS,
            }
        }
        let mut ids: Vec<u64> = self
            .due
            .iter()
            .filter(|(_, due)| **due <= now)
            .map(|(id, _)| *id)
            .collect();
        if ids.is_empty() {
            return;
        }
        ids.sort_unstable();
        let mut changed = false;
        for id in ids {
            let mut next = now + self.poll_secs;
            let problem = match self.tokens.access_token(&self.http, id) {
                Err(RefreshError::LoginExpired) => {
                    Some("Login expired. Log in again on the Characters tab.".to_string())
                }
                Err(RefreshError::Other(e)) => Some(format!("Couldn't get a token: {e:#}")),
                Ok(token) => match self.fetch(id, &token, now) {
                    Ok((skills, expires_in)) => {
                        next = next.max(now + expires_in);
                        if self.cache.characters.get(&id) != Some(&skills) {
                            self.cache.characters.insert(id, skills.clone());
                            changed = true;
                        }
                        send(Event::Skills {
                            character_id: id,
                            skills: Box::new(skills),
                        });
                        None
                    }
                    Err(e) => Some(format!("Skills: {e:#}")),
                },
            };
            send(Event::CharacterProblem {
                character_id: id,
                problem,
            });
            self.due.insert(id, next);
        }
        if changed && let Err(e) = self.cache.save(&self.cache_path) {
            send(Event::Problem(format!(
                "Couldn't save {}: {e:#}",
                self.cache_path.display()
            )));
        }
        send(Event::Checked(now));
    }

    /// Skills, queue and attributes of one character, and how many
    /// seconds until ESI has newer data.
    fn fetch(&mut self, id: u64, token: &str, now: i64) -> Result<(CharacterSkills, i64)> {
        let skills: EsiSkills = self.get(&format!("/characters/{id}/skills"), token)?;
        let mut expires = self.expires_in_secs();
        let queue: Vec<EsiQueueEntry> = self.get(&format!("/characters/{id}/skillqueue"), token)?;
        expires = expires.max(self.expires_in_secs());
        // Only needed for the time of a paused queue: not worth failing
        // the rest for.
        let attributes: Option<Attributes> = self
            .get(&format!("/characters/{id}/attributes"), token)
            .ok();
        Ok((
            CharacterSkills::from_esi(skills, queue, attributes, now),
            expires,
        ))
    }

    /// The lowest Jita 4-4 sell order of each injector, from every page
    /// of The Forge's orders for it.
    fn fetch_prices(&mut self, now: i64) -> Result<InjectorPrices> {
        let mut lowest = |type_id: i64| -> Result<Option<f64>> {
            let mut best: Option<f64> = None;
            let mut page = 1;
            loop {
                let path = format!(
                    "/markets/{THE_FORGE}/orders?order_type=sell&type_id={type_id}&page={page}"
                );
                let pages = match self.esi.get_json::<Vec<Order>>(&path, None)? {
                    Fetched::Data { value, pages, .. } => {
                        if let Some(sell) = best_jita_prices(&value).sell {
                            best = Some(best.map_or(sell, |b: f64| b.min(sell)));
                        }
                        pages
                    }
                    Fetched::NotModified => 1,
                };
                if page >= pages {
                    return Ok(best);
                }
                page += 1;
            }
        };
        Ok(InjectorPrices {
            large: lowest(LARGE_INJECTOR)?,
            small: lowest(SMALL_INJECTOR)?,
            fetched_at: now,
        })
    }

    fn get<T: serde::de::DeserializeOwned>(&mut self, path: &str, token: &str) -> Result<T> {
        match self.esi.get_json_authed::<T>(path, token, None)? {
            Fetched::Data { value, .. } => Ok(value),
            Fetched::NotModified => None.context("ESI sent no data"),
        }
    }

    fn expires_in_secs(&self) -> i64 {
        self.esi
            .last_expires_in
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::skills::data::{QueueEntry, Skill};

    #[test]
    fn cache_round_trips_through_json() {
        let mut cache = Cache::default();
        cache.characters.insert(
            7,
            CharacterSkills {
                skills: vec![Skill {
                    skill_id: 3300,
                    sp: 256_000,
                    trained_level: 5,
                    active_level: 5,
                }],
                total_sp: 5_000_000,
                unallocated_sp: 1000,
                queue: vec![QueueEntry {
                    skill_id: 3301,
                    level: 2,
                    start: Some(1),
                    finish: Some(2),
                    training_start_sp: Some(250),
                    level_start_sp: Some(250),
                    level_end_sp: Some(1415),
                }],
                attributes: None,
                fetched_at: 1_794_960_000,
            },
        );
        cache.injector_prices = Some(InjectorPrices {
            large: Some(745_000_000.0),
            small: None,
            fetched_at: 1_794_960_000,
        });
        let dir = std::env::temp_dir().join(format!("athena-skills-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = Cache::file_path(&dir);
        cache.save(&path).unwrap();
        assert_eq!(Cache::load(&path), cache);
        // A broken file gives an empty cache.
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(Cache::load(&path), Cache::default());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
