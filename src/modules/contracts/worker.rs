//! The Contracts module's background work: fetching each character's
//! contracts (and their items and bids) from ESI, finding what changed and
//! saving it all to `contracts.sqlite`. Nothing here runs on the UI
//! thread; results go to the module as [`Event`]s over a channel, and each
//! batch asks egui for one repaint.
//!
//! How often:
//!
//! - the contract list: every 5 minutes (ESI's cache time), with its ETag,
//!   so an unchanged list costs almost nothing.
//! - items: once per contract, ever (they never change).
//! - bids: every round, for auctions still open.
//! - the changelog check: after every round, and once a minute, because
//!   "expired", "overdue" and "soon" come with the clock, not from ESI.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

use anyhow::Result;
use eframe::egui;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};

use super::changes::{self, Snapshot};
use super::data::{Bid, Contract, ContractType, EsiBid, EsiContract, EsiItem, Item};
use super::store::{LogEntry, Saved, Store};
use crate::auth::{RefreshError, TokenStore, now_unix};
use crate::esi::{EsiClient, Fetched, HttpError};
use crate::modules::finance::data::parse_esi_date;

const CONTRACTS_EVERY_SECS: i64 = 5 * 60;
/// The longest the thread sleeps before checking the clock-based changes.
const MAX_SLEEP: Duration = Duration::from_secs(60);
/// Item lists fetched per character and round, so a first start with many
/// contracts doesn't hammer ESI. The rest come in the next rounds.
const ITEMS_PER_ROUND: usize = 50;
/// `/universe/names` takes at most this many ids per request.
const NAMES_PER_REQUEST: usize = 1000;
/// Ids from this one up are player structures, whose names need a token.
const FIRST_STRUCTURE_ID: i64 = 1_000_000_000_000;

/// What the module asks the worker to do.
pub enum Command {
    /// These are the logged-in characters now.
    SetCharacters(Vec<u64>),
    /// Fetch every contract list now.
    RefreshNow,
    /// The "soon" warning setting changed (hours).
    SetSoonHours(u32),
    MarkSeen(Vec<i64>),
    MarkAllSeen,
}

/// What the worker tells the module.
pub enum Event {
    /// Everything saved from earlier runs. Sent once, first.
    Loaded(Box<Saved>),
    /// New or changed contracts.
    Contracts(Vec<Contract>),
    /// New changelog entries (saved, with their ids).
    Log(Vec<LogEntry>),
    Names(Vec<(i64, String)>),
    /// Every character whose contracts Athena has fetched, ever.
    Known(Vec<u64>),
    /// The contract lists were fetched (or tried) at this time.
    Checked(i64),
    CharacterProblem {
        character_id: u64,
        problem: Option<String>,
    },
    Problem(String),
}

/// The handle the module keeps. Dropping it stops the worker thread.
pub struct Worker {
    commands: Sender<Command>,
    pub events: Receiver<Event>,
}

impl Worker {
    /// `started_at`: when Athena started. Changes from before it are
    /// marked "while closed".
    pub fn start(
        egui_ctx: &egui::Context,
        tokens: Arc<TokenStore>,
        db_path: PathBuf,
        soon_hours: u32,
        started_at: i64,
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
            match Fetcher::new(tokens, &db_path, soon_hours, started_at) {
                Ok((mut fetcher, saved)) => {
                    send(Event::Loaded(Box::new(saved)));
                    fetcher.run(&command_receiver, &send);
                }
                Err(e) => send(Event::Problem(format!(
                    "Contracts can't start: {e:#}. Nothing will be fetched."
                ))),
            }
        });
        Worker { commands, events }
    }

    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }
}

#[derive(Default)]
struct Schedule {
    due: i64,
    etag: Option<String>,
}

/// Lives on the worker thread only.
struct Fetcher {
    esi: EsiClient,
    http: Client,
    tokens: Arc<TokenStore>,
    store: Store,
    started_at: i64,
    soon_secs: i64,
    characters: HashMap<u64, Schedule>,
    /// Characters fetched at least once, ever (saved).
    known: Vec<u64>,
    contracts: HashMap<i64, Contract>,
    snapshots: HashMap<i64, Snapshot>,
    /// Contract id -> ETag of its bids.
    bid_etags: HashMap<i64, String>,
    named: HashSet<i64>,
    /// Ids that got no name this run (tried again next start).
    unnamed: HashSet<i64>,
    /// The `known` list last sent to the module.
    known_sent: Option<Vec<u64>>,
}

impl Fetcher {
    fn new(
        tokens: Arc<TokenStore>,
        db_path: &std::path::Path,
        soon_hours: u32,
        started_at: i64,
    ) -> Result<(Fetcher, Saved)> {
        let store = Store::open(db_path)?;
        let saved = store.load()?;
        let mut contracts = HashMap::new();
        let mut snapshots = HashMap::new();
        for (contract, snapshot) in &saved.contracts {
            if let Some(snapshot) = snapshot {
                snapshots.insert(contract.id, *snapshot);
            }
            contracts.insert(contract.id, contract.clone());
        }
        let fetcher = Fetcher {
            esi: EsiClient::new()?,
            http: crate::auth::sso::http_client()?,
            tokens,
            store,
            started_at,
            soon_secs: i64::from(soon_hours) * 3600,
            characters: HashMap::new(),
            known: saved.known_characters.clone(),
            contracts,
            snapshots,
            bid_etags: HashMap::new(),
            named: saved.names.keys().copied().collect(),
            unnamed: HashSet::new(),
            known_sent: None,
        };
        Ok((fetcher, saved))
    }

    fn run(&mut self, commands: &Receiver<Command>, send: &impl Fn(Event)) {
        // Changes that came with the clock while Athena was closed (an
        // expiry, say) are found at once, before ESI answers.
        self.check_changes(send);
        loop {
            match commands.recv_timeout(self.time_to_next_job()) {
                Ok(Command::SetCharacters(ids)) => {
                    self.characters.retain(|id, _| ids.contains(id));
                    for id in ids {
                        self.characters.entry(id).or_default();
                    }
                }
                Ok(Command::RefreshNow) => {
                    for schedule in self.characters.values_mut() {
                        schedule.due = 0;
                    }
                }
                Ok(Command::SetSoonHours(hours)) => {
                    self.soon_secs = i64::from(hours) * 3600;
                }
                Ok(Command::MarkSeen(ids)) => self.report(self.store.mark_seen(&ids), send),
                Ok(Command::MarkAllSeen) => self.report(self.store.mark_all_seen(), send),
                Err(RecvTimeoutError::Timeout) => {}
                // The module is gone: stop.
                Err(RecvTimeoutError::Disconnected) => return,
            }
            self.do_due_rounds(send);
            self.check_changes(send);
        }
    }

    fn report(&self, result: Result<()>, send: &impl Fn(Event)) {
        if let Err(e) = result {
            send(Event::Problem(format!(
                "Couldn't save to contracts.sqlite: {e:#}"
            )));
        }
    }

    fn time_to_next_job(&self) -> Duration {
        let now = now_unix();
        match self.characters.values().map(|s| s.due).min() {
            None => MAX_SLEEP,
            Some(due) => Duration::from_secs((due - now).clamp(0, 60) as u64).min(MAX_SLEEP),
        }
    }

    fn do_due_rounds(&mut self, send: &impl Fn(Event)) {
        let now = now_unix();
        let mut ids: Vec<u64> = self
            .characters
            .iter()
            .filter(|(_, s)| s.due <= now)
            .map(|(id, _)| *id)
            .collect();
        if ids.is_empty() {
            return;
        }
        ids.sort_unstable();
        let mut changed: Vec<Contract> = Vec::new();
        for id in ids {
            let mut next = now + CONTRACTS_EVERY_SECS;
            let problem = match self.tokens.access_token(&self.http, id) {
                Err(RefreshError::LoginExpired) => {
                    Some("Login expired. Log in again on the Characters tab.".to_string())
                }
                Err(RefreshError::Other(e)) => Some(format!("Couldn't get a token: {e:#}")),
                Ok(token) => match self.round(id, &token, &mut changed) {
                    Ok(expires) => {
                        next = next.max(now + expires);
                        None
                    }
                    Err(e) => Some(format!("Contracts: {e:#}")),
                },
            };
            if let Some(s) = self.characters.get_mut(&id) {
                s.due = next;
            }
            send(Event::CharacterProblem {
                character_id: id,
                problem,
            });
        }
        if !changed.is_empty() {
            let names = self.look_up_names(&changed);
            if !names.is_empty() {
                send(Event::Names(names));
            }
            send(Event::Contracts(changed));
        }
        send(Event::Checked(now));
    }

    /// One character's round. Returns seconds until ESI has news.
    fn round(&mut self, id: u64, token: &str, changed: &mut Vec<Contract>) -> Result<i64> {
        let list = self.fetch_list(id, token)?;
        let expires = self.esi.last_expires_in.map_or(0, |d| d.as_secs() as i64);
        let first_time = !self.known.contains(&id);
        let mut touched: HashSet<i64> = HashSet::new();
        if let Some(list) = list {
            for esi in list {
                let Some(contract) = Contract::from_esi(esi, id) else {
                    continue;
                };
                touched.insert(contract.id);
                match self.contracts.get_mut(&contract.id) {
                    Some(saved) => {
                        if !saved.merge_from(contract) {
                            touched.remove(&saved.id);
                        }
                    }
                    None => {
                        self.contracts.insert(contract.id, contract);
                    }
                }
            }
        }

        // Items, once per contract this character can see.
        let mut wanted: Vec<i64> = self
            .contracts
            .values()
            .filter(|c| c.items.is_none() && c.characters.contains(&id))
            .map(|c| c.id)
            .collect();
        wanted.sort_unstable();
        for contract_id in wanted.into_iter().take(ITEMS_PER_ROUND) {
            if let Some(items) = self.fetch_items(id, token, contract_id)
                && let Some(c) = self.contracts.get_mut(&contract_id)
            {
                c.items = Some(items);
                touched.insert(contract_id);
            }
        }

        // Bids of auctions still open.
        let mine = self.mine();
        let now = now_unix();
        let auctions: Vec<i64> = self
            .contracts
            .values()
            .filter(|c| {
                c.kind == ContractType::Auction
                    && c.characters.contains(&id)
                    && c.stage(&mine, now).is_open()
            })
            .map(|c| c.id)
            .collect();
        for contract_id in auctions {
            if let Some(bids) = self.fetch_bids(id, token, contract_id)
                && let Some(c) = self.contracts.get_mut(&contract_id)
                && c.bids != bids
            {
                c.bids = bids;
                touched.insert(contract_id);
            }
        }

        if first_time {
            // Take this character's contracts in as they are, without
            // changelog entries: they aren't news.
            self.known.push(id);
            self.known.sort_unstable();
            let mine = self.mine();
            for c in self
                .contracts
                .values()
                .filter(|c| c.characters.contains(&id))
            {
                let snapshot = changes::snapshot(c, &mine, now, self.soon_secs);
                // Another of our characters may already have it.
                self.snapshots.entry(c.id).or_insert(snapshot);
                touched.insert(c.id);
            }
            self.store.set_known_characters(&self.known)?;
        }
        let mut sorted: Vec<i64> = touched.into_iter().collect();
        sorted.sort_unstable();
        for contract_id in sorted {
            if let Some(c) = self.contracts.get(&contract_id) {
                self.store
                    .save_contract(c, self.snapshots.get(&contract_id))?;
                changed.push(c.clone());
            }
        }
        Ok(expires)
    }

    /// Every character whose contracts we fetched.
    fn mine(&self) -> Vec<u64> {
        self.known.clone()
    }

    /// Compares every contract with its saved snapshot and logs the
    /// changes. Contracts of characters never fetched are left alone.
    fn check_changes(&mut self, send: &impl Fn(Event)) {
        self.check_changes_at(now_unix(), send);
    }

    fn check_changes_at(&mut self, now: i64, send: &impl Fn(Event)) {
        let mine = self.mine();
        let mut new_entries = Vec::new();
        let mut ids: Vec<i64> = self.contracts.keys().copied().collect();
        ids.sort_unstable();
        for contract_id in ids {
            let Some(contract) = self.contracts.get(&contract_id) else {
                continue;
            };
            if !contract.characters.iter().any(|c| mine.contains(c)) {
                continue;
            }
            let new = changes::snapshot(contract, &mine, now, self.soon_secs);
            let old = self.snapshots.get(&contract_id).copied();
            if old == Some(new) {
                continue;
            }
            for change in changes::changes(contract, old, new, &mine, now, self.soon_secs) {
                new_entries.push(LogEntry {
                    id: 0,
                    at: change.at,
                    contract_id,
                    kind: change.kind,
                    amount: change.amount,
                    offline: change.at < self.started_at,
                    seen: false,
                });
            }
            self.snapshots.insert(contract_id, new);
            if let Err(e) = self.store.save_contract(contract, Some(&new)) {
                self.report(Err(e), send);
            }
        }
        if !new_entries.is_empty() {
            new_entries.sort_by_key(|e| e.at);
            match self.store.add_log(&mut new_entries) {
                Ok(()) => send(Event::Log(new_entries)),
                Err(e) => self.report(Err(e), send),
            }
        }
        // Tell the module whenever a character got its first round.
        if self.known_sent.as_ref() != Some(&self.known) {
            self.known_sent = Some(self.known.clone());
            send(Event::Known(self.known.clone()));
        }
    }

    /// The contract list, all pages. `None` if unchanged since last time.
    fn fetch_list(&mut self, id: u64, token: &str) -> Result<Option<Vec<EsiContract>>> {
        let etag = self.characters.get(&id).and_then(|s| s.etag.clone());
        let path = format!("/characters/{id}/contracts");
        let (mut list, new_etag, pages) =
            match self
                .esi
                .get_json_authed::<Vec<EsiContract>>(&path, token, etag.as_deref())?
            {
                Fetched::NotModified => return Ok(None),
                Fetched::Data { value, etag, pages } => (value, etag, pages),
            };
        for page in 2..=pages {
            let path = format!("/characters/{id}/contracts?page={page}");
            if let Fetched::Data { value, .. } = self
                .esi
                .get_json_authed::<Vec<EsiContract>>(&path, token, None)?
            {
                list.extend(value);
            }
        }
        if let Some(s) = self.characters.get_mut(&id) {
            // A list with more pages is always fetched whole, because the
            // ETag only covers page 1.
            s.etag = if pages > 1 { None } else { new_etag };
        }
        Ok(Some(list))
    }

    /// A contract's items. `Some(empty)` when EVE won't show them to this
    /// character; `None` when it should be tried again later.
    fn fetch_items(&mut self, id: u64, token: &str, contract_id: i64) -> Option<Vec<Item>> {
        let path = format!("/characters/{id}/contracts/{contract_id}/items");
        match self.esi.get_json_authed::<Vec<EsiItem>>(&path, token, None) {
            Ok(Fetched::Data { value, .. }) => Some(
                value
                    .into_iter()
                    .map(|i| Item {
                        type_id: i.type_id,
                        quantity: i.quantity,
                        included: i.is_included,
                    })
                    .collect(),
            ),
            Ok(Fetched::NotModified) => None,
            Err(e) => {
                let hidden = e
                    .downcast_ref::<HttpError>()
                    .is_some_and(|http| matches!(http.status, 403 | 404));
                hidden.then(Vec::new)
            }
        }
    }

    /// An auction's bids, lowest first. `None` if unchanged or failed.
    fn fetch_bids(&mut self, id: u64, token: &str, contract_id: i64) -> Option<Vec<Bid>> {
        let path = format!("/characters/{id}/contracts/{contract_id}/bids");
        let etag = self.bid_etags.get(&contract_id).cloned();
        match self
            .esi
            .get_json_authed::<Vec<EsiBid>>(&path, token, etag.as_deref())
        {
            Ok(Fetched::Data { value, etag, .. }) => {
                if let Some(etag) = etag {
                    self.bid_etags.insert(contract_id, etag);
                }
                let mut bids: Vec<Bid> = value
                    .into_iter()
                    .map(|b| Bid {
                        id: b.bid_id,
                        bidder_id: b.bidder_id,
                        amount: b.amount,
                        at: parse_esi_date(&b.date_bid).unwrap_or(0),
                    })
                    .collect();
                bids.sort_by(|a, b| a.amount.total_cmp(&b.amount));
                Some(bids)
            }
            _ => None,
        }
    }

    /// Names for the people, items and places in `contracts` that have
    /// none yet. Saves them and returns them. Failures are quiet: the UI
    /// shows "…" instead.
    fn look_up_names(&mut self, contracts: &[Contract]) -> Vec<(i64, String)> {
        let mut wanted: Vec<i64> = Vec::new();
        let mut structures: Vec<(i64, u64)> = Vec::new();
        for c in contracts {
            wanted.extend([c.issuer_id, c.assignee_id, c.acceptor_id]);
            wanted.extend(c.bids.iter().map(|b| b.bidder_id));
            wanted.extend(c.items.iter().flatten().map(|i| i.type_id));
            for location in [c.start_location_id, c.end_location_id] {
                if location >= FIRST_STRUCTURE_ID {
                    if let Some(character) = c.characters.first() {
                        structures.push((location, *character));
                    }
                } else {
                    wanted.push(location);
                }
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
    /// id is unknown to it, so then the list is split in halves.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::contracts::changes::ChangeKind;
    use crate::modules::contracts::data::tests::esi_courier;

    fn temp_db(name: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("athena-cw-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = Store::file_path(&dir);
        (dir, db)
    }

    /// Runs the check at `now` and returns the new changelog entries.
    fn check(fetcher: &mut Fetcher, now: i64) -> Vec<LogEntry> {
        let (sender, receiver) = mpsc::channel();
        fetcher.check_changes_at(now, &|event| {
            let _ = sender.send(event);
        });
        receiver
            .try_iter()
            .filter_map(|event| match event {
                Event::Log(entries) => Some(entries),
                _ => None,
            })
            .flatten()
            .collect()
    }

    #[test]
    fn changes_while_closed_are_marked_and_saved() {
        let (dir, db) = temp_db("closed");
        let contract = Contract::from_esi(esi_courier(), 1).unwrap();
        // Last run: character 1 known, the contract waiting.
        {
            let store = Store::open(&db).unwrap();
            store.set_known_characters(&[1]).unwrap();
            let snapshot = changes::snapshot(&contract, &[1], contract.issued, 3600);
            store.save_contract(&contract, Some(&snapshot)).unwrap();
        }
        // Athena starts again after the contract expired.
        let started = contract.expires + 600;
        let tokens = Arc::new(TokenStore::new());
        let (mut fetcher, saved) = Fetcher::new(tokens.clone(), &db, 1, started).unwrap();
        assert_eq!(saved.contracts.len(), 1);
        let entries = check(&mut fetcher, started);
        let kinds: Vec<_> = entries.iter().map(|e| (e.kind, e.offline)).collect();
        assert_eq!(kinds, vec![(ChangeKind::Expired, true)]);
        assert!(entries[0].id > 0);
        // Found once only, also after another start.
        assert!(check(&mut fetcher, started + 30).is_empty());
        drop(fetcher);
        let (mut again, saved) = Fetcher::new(tokens, &db, 1, started + 60).unwrap();
        assert_eq!(saved.log.len(), 1);
        assert!(check(&mut again, started + 60).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn contracts_of_new_characters_are_not_news() {
        let (dir, db) = temp_db("new-char");
        let contract = Contract::from_esi(esi_courier(), 1).unwrap();
        let started = contract.issued + 3600;
        let (mut fetcher, _) = Fetcher::new(Arc::new(TokenStore::new()), &db, 24, started).unwrap();
        // Character 1 was never fetched: its contract is only taken in by
        // its first round, so the check leaves it alone.
        fetcher.contracts.insert(contract.id, contract);
        assert!(check(&mut fetcher, started).is_empty());
        // Once known, a new contract is news, made this session.
        fetcher.known.push(1);
        let mut other = Contract::from_esi(esi_courier(), 1).unwrap();
        other.id = 501;
        other.issued = started + 60;
        fetcher.snapshots.insert(
            500,
            changes::snapshot(&fetcher.contracts[&500], &[1], started + 120, 0),
        );
        fetcher.contracts.insert(other.id, other);
        let entries = check(&mut fetcher, started + 120);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].contract_id, 501);
        assert_eq!(entries[0].kind, ChangeKind::Issued);
        assert!(!entries[0].offline);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
