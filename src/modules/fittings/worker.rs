//! "Open ship…": fetches the characters' assets from ESI on a background
//! thread and finds their ships. Nothing runs until the window is opened,
//! and a character is only fetched again once ESI has newer data (assets
//! are cached for up to an hour). Results reach the module as [`Event`]s
//! over a channel; each one asks egui for one repaint.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use anyhow::Result;
use eframe::egui;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};

use super::assets::{self, EsiAsset, Place, Ship};
use crate::auth::{RefreshError, TokenStore, now_unix};
use crate::esi::{EsiClient, Fetched};
use crate::static_data::StaticData;

/// `/universe/names` and `/assets/names` take at most this many ids.
const IDS_PER_REQUEST: usize = 1000;
/// Ids from this one up are player structures, whose names need a token.
const FIRST_STRUCTURE_ID: i64 = 1_000_000_000_000;
/// If ESI sends no cache time, ask again after this long.
const DEFAULT_CACHE_SECS: i64 = 3600;

/// What the module asks the worker to do.
pub enum Command {
    /// Fetch these characters, unless their data is still current.
    Fetch(Vec<u64>),
}

/// One character's ships.
pub struct CharacterShips {
    pub character_id: u64,
    /// Ship item id -> its name (the one given in game; the type name if
    /// it has none).
    pub ships: Vec<(Ship, String)>,
    /// When ESI made this data (Unix seconds, our clock).
    pub data_from: i64,
    /// When ESI has newer data.
    pub next_at: i64,
}

/// What the worker tells the module.
pub enum Event {
    /// Fetching this character started.
    Started(u64),
    Ships(CharacterShips),
    /// Names of types, stations and structures.
    Names(Vec<(i64, String)>),
    Problem {
        character_id: u64,
        text: String,
        /// When it may be tried again.
        retry_at: i64,
    },
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
        static_data: Arc<StaticData>,
    ) -> Worker {
        let (commands, command_receiver) = mpsc::channel::<Command>();
        let (event_sender, events) = mpsc::channel();
        let ctx = egui_ctx.clone();
        thread::spawn(move || {
            let send = |event: Event| {
                // Fails only when the module is gone.
                let _ = event_sender.send(event);
                ctx.request_repaint();
            };
            let mut fetcher = match Fetcher::new(tokens, static_data) {
                Ok(fetcher) => fetcher,
                Err(e) => {
                    send(Event::Problem {
                        character_id: 0,
                        text: format!("Can't reach ESI: {e:#}"),
                        retry_at: i64::MAX,
                    });
                    return;
                }
            };
            // `recv` waits without using CPU until a command comes; it
            // fails when the module is gone, which ends the thread.
            while let Ok(Command::Fetch(ids)) = command_receiver.recv() {
                for id in ids {
                    fetcher.fetch_if_due(id, &send);
                }
            }
        });
        Worker { commands, events }
    }

    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }
}

/// Lives on the worker thread only.
struct Fetcher {
    esi: EsiClient,
    http: Client,
    tokens: Arc<TokenStore>,
    static_data: Arc<StaticData>,
    /// Character -> when ESI has newer assets.
    next_at: HashMap<u64, i64>,
    /// Ids already named (or tried), so they are looked up once.
    named: HashSet<i64>,
}

impl Fetcher {
    fn new(tokens: Arc<TokenStore>, static_data: Arc<StaticData>) -> Result<Fetcher> {
        Ok(Fetcher {
            esi: EsiClient::new()?,
            http: crate::auth::sso::http_client()?,
            tokens,
            static_data,
            next_at: HashMap::new(),
            named: HashSet::new(),
        })
    }

    fn fetch_if_due(&mut self, id: u64, send: &impl Fn(Event)) {
        let now = now_unix();
        if self.next_at.get(&id).is_some_and(|at| *at > now) {
            return;
        }
        send(Event::Started(id));
        let token = match self.tokens.access_token(&self.http, id) {
            Ok(token) => token,
            Err(RefreshError::LoginExpired) => {
                self.failed(
                    id,
                    "Login expired. Log in again on the Characters tab.",
                    send,
                );
                return;
            }
            Err(RefreshError::Other(e)) => {
                self.failed(id, &format!("Couldn't get a token: {e:#}"), send);
                return;
            }
        };
        match self.fetch(id, &token, send) {
            Ok(ships) => {
                self.next_at.insert(id, ships.next_at);
                send(Event::Ships(ships));
            }
            Err(e) => self.failed(id, &format!("Assets: {e:#}"), send),
        }
    }

    fn failed(&mut self, id: u64, text: &str, send: &impl Fn(Event)) {
        // Try again in 5 minutes, not on every click.
        let retry_at = now_unix() + 300;
        self.next_at.insert(id, retry_at);
        send(Event::Problem {
            character_id: id,
            text: text.to_string(),
            retry_at,
        });
    }

    fn fetch(&mut self, id: u64, token: &str, send: &impl Fn(Event)) -> Result<CharacterShips> {
        let now = now_unix();
        let path = format!("/characters/{id}/assets");
        let (mut list, pages) = match self
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
                list.extend(value);
            }
        }

        // A clone of the Arc (cheap: just a counter) so `universe` doesn't
        // keep `self` borrowed while `self.ship_names` needs it mutably.
        let static_data = self.static_data.clone();
        let universe = &static_data.universe;
        let is_ship = |type_id: i64| universe.ship_by_id(type_id).is_some();
        let ships = assets::find_ships(&list, &is_ship);
        drop(list);

        let ship_names = self.ship_names(id, token, &ships);
        let named_ships = ships
            .into_iter()
            .map(|ship| {
                let name = ship_names
                    .get(&ship.item_id)
                    .cloned()
                    .or_else(|| universe.ship_by_id(ship.type_id).map(|t| t.name.clone()))
                    .unwrap_or_default();
                (ship, name)
            })
            .collect::<Vec<_>>();

        let names = self.look_up_names(token, &named_ships);
        if !names.is_empty() {
            send(Event::Names(names));
        }
        Ok(CharacterShips {
            character_id: id,
            ships: named_ships,
            data_from: now - age,
            next_at: now + expires.max(60),
        })
    }

    /// The names given to ships in game. Unnamed ones are left out.
    fn ship_names(&mut self, id: u64, token: &str, ships: &[Ship]) -> HashMap<i64, String> {
        #[derive(Deserialize)]
        struct Named {
            item_id: i64,
            name: String,
        }
        #[derive(Serialize)]
        struct Ids<'a>(&'a [i64]);
        let ids: Vec<i64> = ships.iter().map(|s| s.item_id).collect();
        let mut out = HashMap::new();
        for chunk in ids.chunks(IDS_PER_REQUEST) {
            let path = format!("/characters/{id}/assets/names");
            // The token goes in a header only (see `EsiClient`).
            let Ok(names) = self
                .esi
                .post_json_authed::<_, Vec<Named>>(&path, token, &Ids(chunk))
            else {
                // Names are a nice-to-have: the type name stands in.
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

    /// Names of the item types in the ships and of their stations and
    /// structures, the ones not looked up yet.
    fn look_up_names(&mut self, token: &str, ships: &[(Ship, String)]) -> Vec<(i64, String)> {
        let mut wanted: Vec<i64> = Vec::new();
        let mut structures: Vec<i64> = Vec::new();
        for (ship, _) in ships {
            wanted.extend(ship.content_types());
            if let Place::Station(location) = ship.place {
                if location >= FIRST_STRUCTURE_ID {
                    structures.push(location);
                } else {
                    wanted.push(location);
                }
            }
        }
        wanted.retain(|i| !self.named.contains(i));
        wanted.sort_unstable();
        wanted.dedup();
        structures.retain(|i| !self.named.contains(i));
        structures.sort_unstable();
        structures.dedup();

        let mut found = Vec::new();
        for chunk in wanted.chunks(IDS_PER_REQUEST) {
            self.names_for(chunk, &mut found);
        }
        for structure_id in structures {
            if let Some(name) = self.structure_name(structure_id, token) {
                found.push((structure_id, name));
            }
            self.named.insert(structure_id);
        }
        found
    }

    /// `/universe/names`. ESI refuses the whole list (404) if any id is
    /// unknown to it, so then the list is split in halves.
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
            Ok(names) => {
                self.named.extend(ids.iter().copied());
                found.extend(names.into_iter().map(|n| (n.id, n.name)));
            }
            Err(_) if ids.len() > 1 => {
                let (a, b) = ids.split_at(ids.len() / 2);
                self.names_for(a, found);
                self.names_for(b, found);
            }
            Err(_) => {
                self.named.insert(ids[0]);
            }
        }
    }

    /// A player structure's name, if the character has docking access.
    fn structure_name(&mut self, structure_id: i64, token: &str) -> Option<String> {
        #[derive(Deserialize)]
        struct Structure {
            name: String,
        }
        let path = format!("/universe/structures/{structure_id}");
        match self
            .esi
            .get_json_authed::<Structure>(&path, token, None)
            .ok()?
        {
            Fetched::Data { value, .. } => Some(value.name),
            Fetched::NotModified => None,
        }
    }
}
