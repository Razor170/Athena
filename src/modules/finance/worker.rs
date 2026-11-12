//! The Finance module's background work: fetching balances, journals,
//! market trades, loyalty points and locations from ESI, saving them in
//! `finance.sqlite`, and looking up names. Nothing here runs on the UI
//! thread; results go to the module as [`Event`]s over a channel, and each
//! batch asks egui for one repaint.
//!
//! How often (ESI caches these answers, so asking more often only gets the
//! same data back):
//!
//! - wallet balance: every 5 minutes,
//! - wallet journal, market trades, loyalty points, location: every hour,
//!   or when ESI says its copy expires, whichever is later.
//!
//! "Refresh" asks for everything right away. Between fetches the thread
//! sleeps; it wakes at most once a minute to check the schedule.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use eframe::egui;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};

use super::data::{
    EsiJournalLine, EsiLocation, EsiTransaction, JournalEntry, LoyaltyPoints, MarketTrade,
};
use super::store::{Saved, Store};
use crate::auth::{RefreshError, TokenStore, now_unix};
use crate::esi::{EsiClient, Fetched, HttpError};

const BALANCE_EVERY_SECS: i64 = 5 * 60;
const JOURNAL_EVERY_SECS: i64 = 60 * 60;
/// The longest the thread sleeps before looking at the schedule again.
const MAX_SLEEP: Duration = Duration::from_secs(60);
/// The journal has at most this many pages to read in one go (2,500
/// entries each). Far more than 30 days of normal play.
const MAX_JOURNAL_PAGES: u32 = 20;
/// `/universe/names` takes at most this many ids per request.
const NAMES_PER_REQUEST: usize = 1000;
/// Ids from this one up are player structures, whose names need a
/// different call.
const FIRST_STRUCTURE_ID: i64 = 1_000_000_000_000;

/// What the module asks the worker to do.
pub enum Command {
    /// These are the logged-in characters now. New ones get fetched at
    /// once; missing ones aren't fetched any more.
    SetCharacters(Vec<u64>),
    /// Fetch everything now.
    RefreshNow,
}

/// What the worker tells the module.
pub enum Event {
    /// Everything saved from earlier runs. Sent once, first.
    Loaded(Box<Saved>),
    Balance {
        character_id: u64,
        balance: f64,
        fetched: i64,
    },
    /// New journal entries and market trades (any characters).
    Journal(Vec<JournalEntry>, Vec<MarketTrade>),
    Loyalty {
        character_id: u64,
        points: Vec<LoyaltyPoints>,
    },
    /// Where ESI says the character is.
    Location { character_id: u64, system_id: i64 },
    /// Names for ids in the journal (parties, items, stations).
    Names(Vec<(i64, String)>),
    /// The journal of every character was fetched (or tried) at this time.
    JournalChecked(i64),
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
    pub fn start(egui_ctx: &egui::Context, tokens: Arc<TokenStore>, db_path: PathBuf) -> Worker {
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
            match Fetcher::new(tokens, &db_path) {
                Ok((mut fetcher, saved)) => {
                    send(Event::Loaded(Box::new(saved)));
                    fetcher.run(&command_receiver, &send);
                }
                Err(e) => send(Event::Problem(format!(
                    "Finance can't start: {e:#}. Nothing will be fetched."
                ))),
            }
        });
        Worker { commands, events }
    }

    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }
}

/// When each kind of data is due next for one character, and the ETags
/// from last time.
#[derive(Default)]
struct Schedule {
    balance_due: i64,
    journal_due: i64,
    journal_etag: Option<String>,
}

/// Owns the ESI client, the token store, the database and the schedule.
/// Lives on the worker thread only.
struct Fetcher {
    esi: EsiClient,
    http: Client,
    tokens: Arc<TokenStore>,
    store: Store,
    characters: HashMap<u64, Schedule>,
    /// Ids we have a name for.
    named: HashSet<i64>,
    /// Ids ESI couldn't name this run; not asked again until restart.
    unnamed: HashSet<i64>,
}

impl Fetcher {
    fn new(tokens: Arc<TokenStore>, db_path: &std::path::Path) -> Result<(Fetcher, Saved)> {
        let store = Store::open(db_path)?;
        let saved = store.load().context("reading the saved wallet data")?;
        let named = saved.names.keys().copied().collect();
        let fetcher = Fetcher {
            esi: EsiClient::new()?,
            http: crate::auth::sso::http_client()?,
            tokens,
            store,
            characters: HashMap::new(),
            named,
            unnamed: HashSet::new(),
        };
        Ok((fetcher, saved))
    }

    fn run(&mut self, commands: &Receiver<Command>, send: &impl Fn(Event)) {
        loop {
            let wait = self.time_to_next_job();
            match commands.recv_timeout(wait) {
                Ok(Command::SetCharacters(ids)) => self.set_characters(ids),
                Ok(Command::RefreshNow) => {
                    for schedule in self.characters.values_mut() {
                        schedule.balance_due = 0;
                        schedule.journal_due = 0;
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
            .map(|s| s.balance_due.min(s.journal_due))
            .min();
        match next {
            None => MAX_SLEEP,
            Some(due) => Duration::from_secs((due - now).clamp(0, 60) as u64).min(MAX_SLEEP),
        }
    }

    /// Fetches only these characters from now on. A removed character's
    /// saved data stays in the file (the tabs only show logged-in
    /// characters), so logging it in again brings its history back.
    fn set_characters(&mut self, ids: Vec<u64>) {
        self.characters.retain(|id, _| ids.contains(id));
        for id in ids {
            // New characters are due at once (both times stay 0).
            self.characters.entry(id).or_default();
        }
    }

    fn do_due_jobs(&mut self, send: &impl Fn(Event)) {
        let now = now_unix();
        let mut ids: Vec<u64> = self.characters.keys().copied().collect();
        ids.sort_unstable();
        let mut journal_fetched = false;
        let mut new_entries = Vec::new();
        let mut new_trades = Vec::new();
        for id in ids {
            let (balance_due, journal_due) = match self.characters.get(&id) {
                Some(s) => (s.balance_due <= now, s.journal_due <= now),
                None => continue,
            };
            if !balance_due && !journal_due {
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
                    // Try again at the next balance time, not every minute.
                    if let Some(s) = self.characters.get_mut(&id) {
                        s.balance_due = now + BALANCE_EVERY_SECS;
                        s.journal_due = s.journal_due.max(now + BALANCE_EVERY_SECS);
                    }
                    continue;
                }
            };

            let mut problem = None;
            if balance_due {
                match self.fetch_balance(id, &token) {
                    Ok(balance) => send(Event::Balance {
                        character_id: id,
                        balance,
                        fetched: now,
                    }),
                    Err(e) => problem = Some(format!("Wallet balance: {e:#}")),
                }
                if let Some(s) = self.characters.get_mut(&id) {
                    s.balance_due = now + BALANCE_EVERY_SECS;
                }
            }
            if journal_due {
                journal_fetched = true;
                let mut next = now + JOURNAL_EVERY_SECS;
                match self.fetch_journal(id, &token) {
                    Ok((entries, expires)) => {
                        new_entries.extend(entries);
                        next = next.max(now + expires);
                    }
                    Err(e) => problem = Some(format!("Wallet journal: {e:#}")),
                }
                match self.fetch_trades(id, &token) {
                    Ok(trades) => new_trades.extend(trades),
                    Err(e) => problem = Some(format!("Market transactions: {e:#}")),
                }
                match self.fetch_loyalty(id, &token) {
                    Ok(points) => send(Event::Loyalty {
                        character_id: id,
                        points,
                    }),
                    Err(e) => problem = Some(format!("Loyalty points: {e:#}")),
                }
                // Where the character is, for the nearest LP store. Not
                // worth a problem message if it fails.
                if let Ok(system_id) = self.fetch_location(id, &token) {
                    send(Event::Location {
                        character_id: id,
                        system_id,
                    });
                }
                if let Some(s) = self.characters.get_mut(&id) {
                    s.journal_due = next;
                }
            }
            send(Event::CharacterProblem {
                character_id: id,
                problem,
            });
        }

        if !new_entries.is_empty() || !new_trades.is_empty() {
            let names = self.look_up_names(&new_entries, &new_trades);
            send(Event::Journal(new_entries, new_trades));
            if !names.is_empty() {
                send(Event::Names(names));
            }
        }
        if journal_fetched {
            send(Event::JournalChecked(now));
        }
    }

    fn fetch_balance(&mut self, id: u64, token: &str) -> Result<f64> {
        let path = format!("/characters/{id}/wallet");
        match self.esi.get_json_authed::<f64>(&path, token, None)? {
            Fetched::Data { value, .. } => {
                self.store.set_balance(id, value, now_unix())?;
                Ok(value)
            }
            // We sent no ETag, so ESI shouldn't say "not modified".
            Fetched::NotModified => anyhow::bail!("ESI sent no balance"),
        }
    }

    /// Reads all pages of the journal and saves them. Returns the entries
    /// that are new, and how many seconds until ESI has fresh data.
    fn fetch_journal(&mut self, id: u64, token: &str) -> Result<(Vec<JournalEntry>, i64)> {
        let etag = self
            .characters
            .get(&id)
            .and_then(|s| s.journal_etag.clone());
        let first = format!("/characters/{id}/wallet/journal?page=1");
        let (lines, new_etag, pages) =
            match self
                .esi
                .get_json_authed::<Vec<EsiJournalLine>>(&first, token, etag.as_deref())?
            {
                Fetched::NotModified => return Ok((Vec::new(), self.expires_in_secs())),
                Fetched::Data { value, etag, pages } => (value, etag, pages),
            };
        let expires = self.expires_in_secs();
        let mut entries: Vec<JournalEntry> = lines
            .into_iter()
            .filter_map(|line| line.into_entry(id))
            .collect();
        for page in 2..=pages.min(MAX_JOURNAL_PAGES) {
            let path = format!("/characters/{id}/wallet/journal?page={page}");
            if let Fetched::Data { value, .. } = self
                .esi
                .get_json_authed::<Vec<EsiJournalLine>>(&path, token, None)?
            {
                entries.extend(value.into_iter().filter_map(|line| line.into_entry(id)));
            }
        }
        let new = self.store.add_journal(entries)?;
        // Only remember the ETag once everything is saved, so a failed
        // run is fetched in full next time.
        if let Some(s) = self.characters.get_mut(&id) {
            s.journal_etag = new_etag;
        }
        Ok((new, expires))
    }

    /// The latest market trades (ESI sends up to 2,500).
    fn fetch_trades(&mut self, id: u64, token: &str) -> Result<Vec<MarketTrade>> {
        let path = format!("/characters/{id}/wallet/transactions");
        match self
            .esi
            .get_json_authed::<Vec<EsiTransaction>>(&path, token, None)?
        {
            Fetched::Data { value, .. } => {
                let trades = value.into_iter().filter_map(|t| t.into_trade(id)).collect();
                self.store.add_trades(trades)
            }
            Fetched::NotModified => Ok(Vec::new()),
        }
    }

    fn fetch_loyalty(&mut self, id: u64, token: &str) -> Result<Vec<LoyaltyPoints>> {
        let path = format!("/characters/{id}/loyalty/points");
        match self
            .esi
            .get_json_authed::<Vec<LoyaltyPoints>>(&path, token, None)?
        {
            Fetched::Data { mut value, .. } => {
                value.retain(|p| p.loyalty_points > 0);
                value.sort_by_key(|p| std::cmp::Reverse(p.loyalty_points));
                self.store.set_loyalty(id, &value)?;
                Ok(value)
            }
            Fetched::NotModified => anyhow::bail!("ESI sent no loyalty points"),
        }
    }

    fn fetch_location(&mut self, id: u64, token: &str) -> Result<i64> {
        let path = format!("/characters/{id}/location");
        match self
            .esi
            .get_json_authed::<EsiLocation>(&path, token, None)?
        {
            Fetched::Data { value, .. } => Ok(value.solar_system_id),
            Fetched::NotModified => anyhow::bail!("ESI sent no location"),
        }
    }

    fn expires_in_secs(&self) -> i64 {
        self.esi
            .last_expires_in
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }

    /// Names for every id in the new entries that has none yet. Saves them
    /// and returns them. Failures are quiet: the UI shows the id instead.
    fn look_up_names(
        &mut self,
        entries: &[JournalEntry],
        trades: &[MarketTrade],
    ) -> Vec<(i64, String)> {
        let mut wanted: Vec<i64> = Vec::new();
        for e in entries {
            wanted.extend(e.first_party_id);
            wanted.extend(e.second_party_id);
        }
        // Structures, with a character who may know them.
        let mut structures: Vec<(i64, u64)> = Vec::new();
        for t in trades {
            wanted.push(t.client_id);
            wanted.push(t.type_id);
            if t.location_id >= FIRST_STRUCTURE_ID {
                structures.push((t.location_id, t.character_id));
            } else {
                wanted.push(t.location_id);
            }
        }
        wanted.retain(|id| *id > 0 && !self.named.contains(id) && !self.unnamed.contains(id));
        wanted.sort_unstable();
        wanted.dedup();
        structures.retain(|(id, _)| !self.named.contains(id) && !self.unnamed.contains(id));
        structures.sort_unstable();
        structures.dedup_by_key(|(id, _)| *id);

        let mut found = Vec::new();
        for chunk in wanted.chunks(NAMES_PER_REQUEST) {
            self.names_for(chunk, &mut found);
        }
        for (structure_id, character_id) in structures {
            match self.structure_name(structure_id, character_id) {
                Some(name) => found.push((structure_id, name)),
                None => {
                    self.unnamed.insert(structure_id);
                }
            }
        }
        for (id, _) in &found {
            self.named.insert(*id);
        }
        if !found.is_empty() {
            // A failed save only costs a lookup next run.
            let _ = self.store.add_names(&found);
        }
        found
    }

    /// `/universe/names` for `ids`. ESI refuses the whole list (404) if any
    /// id is unknown to it, so then the list is split in halves until the
    /// unknown ones are found.
    fn names_for(&mut self, ids: &[i64], found: &mut Vec<(i64, String)>) {
        #[derive(Deserialize)]
        struct Named {
            id: i64,
            name: String,
        }
        #[derive(Serialize)]
        struct Ids<'a>(&'a [i64]);
        if ids.is_empty() {
            return;
        }
        match self
            .esi
            .post_json::<_, Vec<Named>>("/universe/names", &Ids(ids))
        {
            Ok(names) => found.extend(names.into_iter().map(|n| (n.id, n.name))),
            Err(e) => {
                let not_found = e
                    .downcast_ref::<HttpError>()
                    .is_some_and(|http| http.status == 404);
                if not_found && ids.len() > 1 {
                    let (a, b) = ids.split_at(ids.len() / 2);
                    self.names_for(a, found);
                    self.names_for(b, found);
                } else {
                    self.unnamed.extend(ids.iter().copied());
                }
            }
        }
    }

    /// A player structure's name, if the character has docking access.
    fn structure_name(&mut self, structure_id: i64, character_id: u64) -> Option<String> {
        #[derive(Deserialize)]
        struct Structure {
            name: String,
        }
        let token = self.tokens.access_token(&self.http, character_id).ok()?;
        let path = format!("/universe/structures/{structure_id}");
        match self
            .esi
            .get_json_authed::<Structure>(&path, &token, None)
            .ok()?
        {
            Fetched::Data { value, .. } => Some(value.name),
            Fetched::NotModified => None,
        }
    }
}
