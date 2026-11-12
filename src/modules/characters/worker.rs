//! The Characters module's background work: logins, token checks,
//! corporation names and removing logins. Nothing here runs on the UI
//! thread; results come back as [`Event`]s over a channel, and every event
//! asks egui for one repaint.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use anyhow::Result;
use eframe::egui;
use reqwest::blocking::Client;
use serde::Deserialize;

use super::store::Affiliation;
use crate::auth::callback::{CallbackServer, Outcome};
use crate::auth::pkce::Pkce;
use crate::auth::sso::{self, TokenCharacter};
use crate::auth::{RefreshError, TokenStore, vault};
use crate::esi::{EsiClient, Fetched};

/// What the module asks the worker to do.
pub enum Command {
    /// Renew each character's token once to see if its login still works,
    /// then look up its corporation.
    CheckAll(Vec<u64>),
    /// Look up a character's corporation (after a new login).
    FetchCorporation(u64),
    /// Forget a character's login: revoke it at EVE and delete it from
    /// Credential Manager.
    Remove(u64),
}

/// How a character's login is doing.
#[derive(Debug, Clone, PartialEq)]
pub enum TokenStatus {
    /// Not checked yet this run.
    Checking,
    /// The login works.
    Ok,
    /// EVE refused the refresh token. Needs a new login.
    LoginExpired,
    /// The check failed for another reason (network, EVE down).
    Error(String),
}

/// What the worker tells the module.
pub enum Event {
    /// A login finished: the character, or why it failed. `None` means the
    /// user cancelled in Athena, which needs no message.
    LoginFinished(Option<Result<TokenCharacter, String>>),
    /// The result of a token check. Carries the token's details when it worked.
    Status {
        id: u64,
        status: TokenStatus,
        character: Option<TokenCharacter>,
    },
    Corporation {
        character_id: u64,
        corporation: Affiliation,
        alliance: Option<Affiliation>,
    },
    /// Something went wrong that the user should know about.
    Problem(String),
}

/// The handle the module keeps. Dropping it stops the worker thread.
pub struct Worker {
    commands: Sender<Command>,
    pub events: Receiver<Event>,
    /// A clone of the events sender, for login threads.
    event_sender: Sender<Event>,
    tokens: Arc<TokenStore>,
    egui_ctx: egui::Context,
}

impl Worker {
    pub fn start(egui_ctx: &egui::Context, tokens: Arc<TokenStore>) -> Worker {
        let (commands, command_receiver) = mpsc::channel::<Command>();
        let (event_sender, events) = mpsc::channel::<Event>();
        let thread_events = event_sender.clone();
        let thread_tokens = tokens.clone();
        let thread_ctx = egui_ctx.clone();
        // `move` hands the receiver, sender and token store to the thread.
        // The loop ends when the module drops its `commands` sender.
        thread::spawn(move || {
            let mut state = WorkerState::new(thread_tokens);
            for command in command_receiver {
                state.run(command, &thread_events, &thread_ctx);
            }
        });
        Worker {
            commands,
            events,
            event_sender,
            tokens,
            egui_ctx: egui_ctx.clone(),
        }
    }

    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }

    /// Starts a login on its own thread, so a 5-minute wait never holds up
    /// the other work. Opens the browser. Setting `cancel` to true ends it.
    pub fn start_login(&self, cancel: Arc<AtomicBool>) {
        let events = self.event_sender.clone();
        let tokens = self.tokens.clone();
        let ctx = self.egui_ctx.clone();
        thread::spawn(move || {
            let result = login(&tokens, &cancel);
            let _ = events.send(Event::LoginFinished(result));
            ctx.request_repaint();
        });
    }
}

/// One whole login: local server, browser, code, tokens, Credential Manager.
/// `None` if the user cancelled in Athena.
fn login(tokens: &TokenStore, cancel: &Arc<AtomicBool>) -> Option<Result<TokenCharacter, String>> {
    let pkce = match Pkce::new() {
        Ok(pkce) => pkce,
        Err(e) => return Some(Err(format!("{e:#}"))),
    };
    let server = match CallbackServer::start() {
        Ok(server) => server,
        Err(e) => return Some(Err(format!("{e:#}"))),
    };
    let login_url = match sso::authorize_url(&pkce) {
        Ok(url) => url,
        Err(e) => return Some(Err(format!("{e:#}"))),
    };
    if let Err(e) = crate::shell::open_local_url(&crate::auth::callback::start_url()) {
        return Some(Err(format!("Can't open the browser: {e}")));
    }
    let code = match server.wait(&login_url, &pkce.state, cancel) {
        Outcome::Code(code) => code,
        Outcome::Cancelled => return None,
        Outcome::TimedOut => {
            return Some(Err(
                "The login took longer than 5 minutes and was stopped.".to_string()
            ));
        }
        Outcome::Refused(why) => return Some(Err(format!("The login didn't finish: {why}"))),
    };
    // The browser is done with the server; close the port right away.
    drop(server);
    Some(finish_login(tokens, &code, &pkce).map_err(|e| format!("{e:#}")))
}

fn finish_login(tokens: &TokenStore, code: &str, pkce: &Pkce) -> Result<TokenCharacter> {
    let http = sso::http_client()?;
    let fresh = sso::exchange_code(&http, code, pkce)?;
    let id = fresh.character.id;
    vault::save(id, &fresh.refresh_token)?;
    tokens.put(id, fresh.access_token, fresh.expires_at);
    Ok(fresh.character)
}

/// The worker thread's own state.
struct WorkerState {
    tokens: Arc<TokenStore>,
    /// `None` if the HTTP client couldn't be made (reported on first use).
    sso_http: Option<Client>,
    esi: Option<EsiClient>,
    /// Corporations (with their alliance id) and alliances already looked
    /// up this run, by id.
    corporations: HashMap<u64, (Affiliation, Option<u64>)>,
    alliances: HashMap<u64, Affiliation>,
}

impl WorkerState {
    fn new(tokens: Arc<TokenStore>) -> WorkerState {
        WorkerState {
            tokens,
            sso_http: sso::http_client().ok(),
            esi: EsiClient::new().ok(),
            corporations: HashMap::new(),
            alliances: HashMap::new(),
        }
    }

    fn run(&mut self, command: Command, events: &Sender<Event>, ctx: &egui::Context) {
        // Sends one event and asks for one repaint, so the UI shows it.
        let send = |event: Event| {
            let _ = events.send(event);
            ctx.request_repaint();
        };
        match command {
            Command::CheckAll(ids) => {
                for id in &ids {
                    send(self.check(*id));
                }
                for id in ids {
                    if let Some(event) = self.corporation_of(id) {
                        send(event);
                    }
                }
            }
            Command::FetchCorporation(id) => {
                if let Some(event) = self.corporation_of(id) {
                    send(event);
                }
            }
            Command::Remove(id) => {
                if let Some(problem) = self.remove(id) {
                    send(Event::Problem(problem));
                }
            }
        }
    }

    fn check(&mut self, id: u64) -> Event {
        let Some(http) = &self.sso_http else {
            return Event::Status {
                id,
                status: TokenStatus::Error("can't set up the HTTP client".to_string()),
                character: None,
            };
        };
        match self.tokens.renew_now(http, id) {
            Ok(character) => Event::Status {
                id,
                status: TokenStatus::Ok,
                character: Some(character),
            },
            Err(RefreshError::LoginExpired) => Event::Status {
                id,
                status: TokenStatus::LoginExpired,
                character: None,
            },
            Err(RefreshError::Other(e)) => Event::Status {
                id,
                status: TokenStatus::Error(format!("{e:#}")),
                character: None,
            },
        }
    }

    /// Looks up the character's corporation and alliance with ESI's public
    /// endpoints. `None` if ESI couldn't be reached; the old names stay then.
    fn corporation_of(&mut self, character_id: u64) -> Option<Event> {
        let esi = self.esi.as_mut()?;
        let character: PublicCharacter =
            match esi.get_json(&format!("/characters/{character_id}"), None) {
                Ok(Fetched::Data { value, .. }) => value,
                _ => return None,
            };
        let corporation_id = character.corporation_id;
        // `entry` looks the id up once; `Vacant` means we don't know it yet.
        if let Entry::Vacant(slot) = self.corporations.entry(corporation_id) {
            let corporation: PublicGroup =
                match esi.get_json(&format!("/corporations/{corporation_id}"), None) {
                    Ok(Fetched::Data { value, .. }) => value,
                    _ => return None,
                };
            let affiliation = Affiliation {
                id: corporation_id,
                name: corporation.name,
                ticker: corporation.ticker,
            };
            slot.insert((affiliation, corporation.alliance_id));
        }
        let (corporation, alliance_id) = self.corporations.get(&corporation_id)?.clone();
        let alliance = match alliance_id {
            None => None,
            Some(alliance_id) => {
                if let Entry::Vacant(slot) = self.alliances.entry(alliance_id) {
                    let alliance: PublicGroup =
                        match esi.get_json(&format!("/alliances/{alliance_id}"), None) {
                            Ok(Fetched::Data { value, .. }) => value,
                            _ => return None,
                        };
                    slot.insert(Affiliation {
                        id: alliance_id,
                        name: alliance.name,
                        ticker: alliance.ticker,
                    });
                }
                self.alliances.get(&alliance_id).cloned()
            }
        };
        Some(Event::Corporation {
            character_id,
            corporation,
            alliance,
        })
    }

    /// Revokes and deletes a login. Returns a message if something failed.
    fn remove(&mut self, id: u64) -> Option<String> {
        self.tokens.forget(id);
        let refresh_token = match vault::load(id) {
            Ok(token) => token,
            Err(e) => return Some(format!("{e:#}")),
        };
        // Revoking is a courtesy to EVE: the login stops working at once
        // instead of lingering under "Authorized Apps". Not being able to
        // reach EVE must not keep the token on this PC, so the local copy
        // is deleted whatever happens.
        if let (Some(token), Some(http)) = (&refresh_token, &self.sso_http) {
            let _ = sso::revoke(http, token);
        }
        vault::remove(id).err().map(|e| format!("{e:#}"))
    }
}

/// The part of ESI's `/characters/{id}` answer we use.
#[derive(Deserialize)]
struct PublicCharacter {
    corporation_id: u64,
}

/// The part of ESI's `/corporations/{id}` and `/alliances/{id}` answers we use.
#[derive(Deserialize)]
struct PublicGroup {
    name: String,
    ticker: String,
    /// Only corporations have this, and only when they're in an alliance.
    #[serde(default)]
    alliance_id: Option<u64>,
}

/// Tells a running login to stop.
pub fn cancel(flag: &AtomicBool) {
    flag.store(true, Ordering::Relaxed);
}
