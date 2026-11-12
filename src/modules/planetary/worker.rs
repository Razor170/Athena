//! The Planetary module's background work: fetching each character's
//! colonies from ESI, the layout of each colony, planet names and the
//! planet skill. Nothing here runs on the UI thread; results go to the
//! module as [`Event`]s over a channel, and each batch asks egui for one
//! repaint.
//!
//! How often (ESI caches these answers, so asking more often only gets the
//! same data back):
//!
//! - the colony list: every 10 minutes, or when ESI says its copy expires,
//!   whichever is later. Sent with its ETag, so an unchanged list costs
//!   almost nothing.
//! - a colony's layout: only when the list says the colony was viewed in
//!   the game since we last fetched it.
//! - planet names: once per planet, ever (they're saved).
//! - the Interplanetary Consolidation skill: every 6 hours.
//!
//! Everything fetched is saved in `planetary-cache.json`, so Athena shows
//! the planets right after a restart, before ESI answers.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

use anyhow::Result;
use eframe::egui;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};

use super::data::{Colony, EsiColony, EsiLayout, Layout};
use crate::auth::{RefreshError, TokenStore, now_unix};
use crate::esi::{EsiClient, Fetched};

const COLONIES_EVERY_SECS: i64 = 10 * 60;
const SKILLS_EVERY_SECS: i64 = 6 * 60 * 60;
/// The longest the thread sleeps before looking at the schedule again.
const MAX_SLEEP: Duration = Duration::from_secs(60);
/// The skill that allows more planets: one plus its level.
const INTERPLANETARY_CONSOLIDATION: i64 = 2495;
const CACHE_FILE: &str = "planetary-cache.json";

/// What the module asks the worker to do.
pub enum Command {
    /// These are the logged-in characters now. New ones get fetched at
    /// once; missing ones aren't fetched any more.
    SetCharacters(Vec<u64>),
    /// Fetch every colony list now.
    RefreshNow,
}

/// What the worker tells the module.
pub enum Event {
    /// Everything saved from earlier runs. Sent once, first.
    Loaded(Box<Cache>),
    /// All colonies of one character, as of now.
    Colonies {
        character_id: u64,
        colonies: Vec<Colony>,
    },
    PlanetNames(Vec<(i64, String)>),
    /// How many planets the character may have.
    MaxPlanets {
        character_id: u64,
        max: u32,
    },
    /// The colony lists were fetched (or tried) at this time.
    Checked(i64),
    /// Something went wrong for one character (`None`: it works again).
    CharacterProblem {
        character_id: u64,
        problem: Option<String>,
    },
    /// Something went wrong that isn't about one character.
    Problem(String),
}

/// What `planetary-cache.json` holds.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Cache {
    /// Character id -> their colonies.
    pub colonies: BTreeMap<u64, Vec<Colony>>,
    /// Planet id -> name, such as "Amamake II".
    pub planet_names: BTreeMap<i64, String>,
    /// Character id -> how many planets they may have.
    pub max_planets: BTreeMap<u64, u32>,
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
    pub fn start(egui_ctx: &egui::Context, tokens: Arc<TokenStore>, cache_path: PathBuf) -> Worker {
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
            match Fetcher::new(tokens, cache_path, cache) {
                Ok(mut fetcher) => fetcher.run(&command_receiver, &send),
                Err(e) => send(Event::Problem(format!(
                    "Planetary can't start: {e:#}. Nothing will be fetched."
                ))),
            }
        });
        Worker { commands, events }
    }

    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }
}

/// When each kind of data is due next for one character, and the ETag of
/// the colony list from last time.
#[derive(Default)]
struct Schedule {
    colonies_due: i64,
    skills_due: i64,
    etag: Option<String>,
}

/// Owns the ESI client, the token store, the cache and the schedule.
/// Lives on the worker thread only.
struct Fetcher {
    esi: EsiClient,
    http: Client,
    tokens: Arc<TokenStore>,
    cache_path: PathBuf,
    cache: Cache,
    characters: HashMap<u64, Schedule>,
}

impl Fetcher {
    fn new(tokens: Arc<TokenStore>, cache_path: PathBuf, cache: Cache) -> Result<Fetcher> {
        Ok(Fetcher {
            esi: EsiClient::new()?,
            http: crate::auth::sso::http_client()?,
            tokens,
            cache_path,
            cache,
            characters: HashMap::new(),
        })
    }

    fn run(&mut self, commands: &Receiver<Command>, send: &impl Fn(Event)) {
        loop {
            let wait = self.time_to_next_job();
            match commands.recv_timeout(wait) {
                Ok(Command::SetCharacters(ids)) => self.set_characters(ids),
                Ok(Command::RefreshNow) => {
                    for schedule in self.characters.values_mut() {
                        schedule.colonies_due = 0;
                    }
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
        let next = self
            .characters
            .values()
            .map(|s| s.colonies_due.min(s.skills_due))
            .min();
        match next {
            None => MAX_SLEEP,
            Some(due) => Duration::from_secs((due - now).clamp(0, 60) as u64).min(MAX_SLEEP),
        }
    }

    fn set_characters(&mut self, ids: Vec<u64>) {
        self.characters.retain(|id, _| ids.contains(id));
        for id in ids {
            // New characters are due at once (all times stay 0).
            self.characters.entry(id).or_default();
        }
    }

    fn do_due_jobs(&mut self, send: &impl Fn(Event)) {
        let now = now_unix();
        let mut ids: Vec<u64> = self.characters.keys().copied().collect();
        ids.sort_unstable();
        let mut checked = false;
        let mut changed = false;
        for id in ids {
            let (colonies_due, skills_due) = match self.characters.get(&id) {
                Some(s) => (s.colonies_due <= now, s.skills_due <= now),
                None => continue,
            };
            if !colonies_due && !skills_due {
                continue;
            }
            let token = match self.tokens.access_token(&self.http, id) {
                Ok(token) => token,
                Err(e) => {
                    let problem = match e {
                        RefreshError::LoginExpired => {
                            "Login expired. Log in again on the Characters tab.".to_string()
                        }
                        RefreshError::Other(e) => format!("Couldn't get a token: {e:#}"),
                    };
                    send(Event::CharacterProblem {
                        character_id: id,
                        problem: Some(problem),
                    });
                    // Try again at the next regular time, not every minute.
                    if let Some(s) = self.characters.get_mut(&id) {
                        s.colonies_due = now + COLONIES_EVERY_SECS;
                        s.skills_due = s.skills_due.max(now + COLONIES_EVERY_SECS);
                    }
                    continue;
                }
            };

            let mut problem = None;
            if colonies_due {
                checked = true;
                let mut next = now + COLONIES_EVERY_SECS;
                match self.fetch_colonies(id, &token) {
                    Ok((colonies, expires, layout_problem)) => {
                        next = next.max(now + expires);
                        problem = layout_problem;
                        if let Some(colonies) = colonies {
                            self.cache.colonies.insert(id, colonies.clone());
                            changed = true;
                            send(Event::Colonies {
                                character_id: id,
                                colonies,
                            });
                        }
                    }
                    Err(e) => problem = Some(format!("Colonies: {e:#}")),
                }
                if let Some(s) = self.characters.get_mut(&id) {
                    s.colonies_due = next;
                }
            }
            if skills_due {
                // Not worth a problem message if it fails: the card then
                // shows "5 planets" instead of "5 of 6 planets".
                if let Ok(max) = self.fetch_max_planets(id, &token) {
                    if self.cache.max_planets.get(&id) != Some(&max) {
                        self.cache.max_planets.insert(id, max);
                        changed = true;
                    }
                    send(Event::MaxPlanets {
                        character_id: id,
                        max,
                    });
                }
                if let Some(s) = self.characters.get_mut(&id) {
                    s.skills_due = now + SKILLS_EVERY_SECS;
                }
            }
            send(Event::CharacterProblem {
                character_id: id,
                problem,
            });
        }

        let names = self.look_up_planet_names();
        if !names.is_empty() {
            changed = true;
            send(Event::PlanetNames(names));
        }
        if changed && let Err(e) = self.cache.save(&self.cache_path) {
            send(Event::Problem(format!(
                "Couldn't save {}: {e:#}",
                self.cache_path.display()
            )));
        }
        if checked {
            send(Event::Checked(now));
        }
    }

    /// The character's colony list, with each layout fetched again only
    /// when the colony was viewed since the saved copy. Returns `None` for
    /// the list when ESI says nothing changed, how many seconds until ESI
    /// has fresh data, and a problem if some layouts couldn't be fetched.
    #[allow(clippy::type_complexity)]
    fn fetch_colonies(
        &mut self,
        id: u64,
        token: &str,
    ) -> Result<(Option<Vec<Colony>>, i64, Option<String>)> {
        let etag = self.characters.get(&id).and_then(|s| s.etag.clone());
        let path = format!("/characters/{id}/planets");
        let (list, new_etag) =
            match self
                .esi
                .get_json_authed::<Vec<EsiColony>>(&path, token, etag.as_deref())?
            {
                Fetched::NotModified => return Ok((None, self.expires_in_secs(), None)),
                Fetched::Data { value, etag, .. } => (value, etag),
            };
        let expires = self.expires_in_secs();
        let saved: HashMap<i64, Colony> = self
            .cache
            .colonies
            .get(&id)
            .map(|list| list.iter().map(|c| (c.planet_id, c.clone())).collect())
            .unwrap_or_default();

        let mut colonies = Vec::new();
        let mut failed = 0;
        for line in list {
            let Some(mut colony) = Colony::from_esi(id, line) else {
                continue;
            };
            let old = saved.get(&colony.planet_id);
            let unchanged = old
                .is_some_and(|old| old.last_update == colony.last_update && old.layout.is_some());
            if unchanged {
                colony.layout = old.and_then(|old| old.layout.clone());
            } else {
                let path = format!("/characters/{id}/planets/{}", colony.planet_id);
                match self.esi.get_json_authed::<EsiLayout>(&path, token, None) {
                    Ok(Fetched::Data { value, .. }) => {
                        colony.layout = Some(Layout::from_esi(value));
                    }
                    _ => {
                        // Keep the old layout, if any, and try again next
                        // time (the ETag isn't saved below).
                        colony.layout = old.and_then(|old| old.layout.clone());
                        failed += 1;
                    }
                }
            }
            colonies.push(colony);
        }
        let problem = (failed > 0)
            .then(|| format!("{failed} planet layout(s) couldn't be fetched; trying again later"));
        // Only remember the ETag when every layout is in, so a failed one
        // is fetched again next time.
        if failed == 0
            && let Some(s) = self.characters.get_mut(&id)
        {
            s.etag = new_etag;
        }
        Ok((Some(colonies), expires, problem))
    }

    fn fetch_max_planets(&mut self, id: u64, token: &str) -> Result<u32> {
        #[derive(Deserialize)]
        struct Skills {
            skills: Vec<Skill>,
        }
        #[derive(Deserialize)]
        struct Skill {
            skill_id: i64,
            active_skill_level: i64,
        }
        let path = format!("/characters/{id}/skills");
        match self.esi.get_json_authed::<Skills>(&path, token, None)? {
            Fetched::Data { value, .. } => {
                let level = value
                    .skills
                    .iter()
                    .find(|s| s.skill_id == INTERPLANETARY_CONSOLIDATION)
                    .map_or(0, |s| s.active_skill_level);
                Ok(level.clamp(0, 5) as u32 + 1)
            }
            Fetched::NotModified => anyhow::bail!("ESI sent no skills"),
        }
    }

    /// Names for planets that have none yet, from `/universe/planets`
    /// (`/universe/names` doesn't do planets). Failures are quiet: the UI
    /// shows the system name instead, and it's tried again next round.
    fn look_up_planet_names(&mut self) -> Vec<(i64, String)> {
        #[derive(Deserialize)]
        struct Planet {
            name: String,
        }
        let mut wanted: Vec<i64> = self
            .cache
            .colonies
            .values()
            .flatten()
            .map(|c| c.planet_id)
            .filter(|id| !self.cache.planet_names.contains_key(id))
            .collect();
        wanted.sort_unstable();
        wanted.dedup();
        let mut found = Vec::new();
        for planet_id in wanted {
            let path = format!("/universe/planets/{planet_id}");
            if let Ok(Fetched::Data { value, .. }) = self.esi.get_json::<Planet>(&path, None) {
                self.cache
                    .planet_names
                    .insert(planet_id, value.name.clone());
                found.push((planet_id, value.name));
            }
        }
        found
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

    #[test]
    fn cache_round_trips_through_json() {
        let mut cache = Cache::default();
        cache.colonies.insert(
            7,
            vec![Colony {
                character_id: 7,
                planet_id: 40_000_002,
                system_id: 30_000_001,
                planet_type: "barren".into(),
                upgrade_level: 5,
                num_pins: 3,
                last_update: 1_794_500_000,
                layout: Some(Layout::default()),
            }],
        );
        cache.planet_names.insert(40_000_002, "Tanoo I".into());
        cache.max_planets.insert(7, 6);
        let text = serde_json::to_string(&cache).unwrap();
        assert_eq!(serde_json::from_str::<Cache>(&text).unwrap(), cache);

        let dir = std::env::temp_dir().join(format!("athena-pi-{}", std::process::id()));
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
