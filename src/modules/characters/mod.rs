//! Characters: log characters in with EVE's SSO (asking for every ESI
//! scope) and sort them into accounts.
//!
//! The pieces:
//!
//! - [`store`]: the character and account list, saved in `characters.toml`.
//! - [`worker`]: logins, token checks and corporation lookups on
//!   background threads.
//! - [`ui`]: draws the panel, following the mockup Kevin approved.
//! - The login itself lives in `crate::auth`, because other modules will
//!   use the tokens too.
//!
//! This file holds the [`Characters`] module: it owns the list and the
//! worker, and applies the worker's events in `update`.

pub mod store;
mod ui;
pub mod worker;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use eframe::egui;

use self::store::{CharacterList, Place};
use self::worker::{Command, Event, TokenStatus, Worker};
use super::shared::{RosterCharacter, SharedRoster, publish_roster};
use super::{Module, ModuleContext};
use crate::auth::TokenStore;
use crate::storage::Saver;

/// A login that's waiting for the browser.
struct LoginInProgress {
    /// Set to true to stop it (the Cancel button).
    cancel: Arc<AtomicBool>,
}

/// A short message under the toolbar after a login.
struct Notice {
    text: String,
    is_error: bool,
}

pub struct Characters {
    data_dir: PathBuf,
    /// Writes `characters.toml` on a background thread.
    saver: Saver,
    list: CharacterList,
    /// How each character's login is doing this run. Not saved.
    statuses: HashMap<u64, TokenStatus>,
    worker: Worker,
    login: Option<LoginInProgress>,
    notice: Option<Notice>,
    /// Problems to show on top of the panel (a broken file, Credential
    /// Manager errors).
    problems: Vec<String>,
    /// UI state: which account's name is being edited, and its text.
    renaming: Option<(usize, String)>,
    /// UI state: the "Tags…" window: which character, and the typed text.
    tag_editor: Option<(u64, String)>,
    /// UI state: show only characters with this tag.
    tag_filter: Option<String>,
    /// The character list other modules read (Finance). Updated on every
    /// save.
    roster: SharedRoster,
}

impl Characters {
    pub fn new(
        egui_ctx: &egui::Context,
        data_dir: &Path,
        tokens: Arc<TokenStore>,
        roster: SharedRoster,
    ) -> Characters {
        let (list, warning) = CharacterList::load(data_dir);
        let worker = Worker::start(egui_ctx, tokens);
        let ids: Vec<u64> = list.characters.iter().map(|c| c.id).collect();
        let statuses = ids.iter().map(|id| (*id, TokenStatus::Checking)).collect();
        if !ids.is_empty() && crate::auth::is_configured() {
            worker.send(Command::CheckAll(ids));
        }
        let module = Characters {
            data_dir: data_dir.to_path_buf(),
            saver: Saver::start(),
            list,
            statuses,
            worker,
            login: None,
            notice: None,
            problems: warning.into_iter().collect(),
            renaming: None,
            tag_editor: None,
            tag_filter: None,
            roster,
        };
        module.publish_roster();
        module
    }

    fn save(&self) {
        if let Ok(text) = self.list.to_toml() {
            self.saver
                .save(CharacterList::file_path(&self.data_dir), text);
        }
        self.publish_roster();
    }

    /// Hands the character list to the other modules, in the order this
    /// tab shows it: accounts first, then the unassigned characters.
    fn publish_roster(&self) {
        let in_accounts = self
            .list
            .accounts
            .iter()
            .flat_map(|account| account.characters.iter())
            .filter_map(|id| self.list.character(*id));
        let characters = in_accounts
            .chain(self.list.unassigned())
            .map(|c| RosterCharacter {
                id: c.id,
                name: c.name.clone(),
                tags: c.tags.clone(),
            })
            .collect();
        publish_roster(&self.roster, characters);
    }

    /// The "+ Add character" button.
    fn start_login(&mut self) {
        if self.login.is_some() {
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.worker.start_login(cancel.clone());
        self.login = Some(LoginInProgress { cancel });
        self.notice = None;
    }

    fn cancel_login(&mut self) {
        if let Some(login) = &self.login {
            worker::cancel(&login.cancel);
        }
    }

    /// Removes a character from Athena and deletes its login.
    fn remove_character(&mut self, id: u64) {
        self.list.remove_character(id);
        self.statuses.remove(&id);
        let generation = self.worker.login_generation(id);
        self.worker.send(Command::Remove { id, generation });
        self.save();
    }

    fn move_character(&mut self, id: u64, to: Place, index: usize) {
        if self.list.move_character(id, to, index) {
            self.save();
        }
    }

    fn apply(&mut self, event: Event) {
        match event {
            Event::LoginFinished(result) => {
                self.login = None;
                match result {
                    None => {}
                    Some(Ok(character)) => {
                        let is_new = self.list.add_or_update(&character, crate::auth::now_unix());
                        self.statuses.insert(character.id, TokenStatus::Ok);
                        self.worker.send(Command::FetchCorporation(character.id));
                        let text = if is_new {
                            format!("Added {}. Drag them onto an account.", character.name)
                        } else {
                            format!("Logged in {} again.", character.name)
                        };
                        self.notice = Some(Notice {
                            text,
                            is_error: false,
                        });
                        self.save();
                    }
                    Some(Err(why)) => {
                        self.notice = Some(Notice {
                            text: format!("Login failed: {why}"),
                            is_error: true,
                        });
                    }
                }
            }
            Event::Status {
                id,
                status,
                character,
            } => {
                if let Some(character) = character
                    && self.list.set_scopes(id, &character.scopes)
                {
                    self.save();
                }
                // A character removed meanwhile stays removed.
                if self.list.character(id).is_some() {
                    self.statuses.insert(id, status);
                }
            }
            Event::Corporation {
                character_id,
                corporation,
                alliance,
            } => {
                if self
                    .list
                    .set_corporation(character_id, &corporation, alliance.as_ref())
                {
                    self.save();
                }
            }
            Event::Problem(text) => self.problems.push(text),
        }
    }
}

impl Module for Characters {
    fn id(&self) -> &'static str {
        "characters"
    }

    fn title(&self) -> &'static str {
        "Characters"
    }

    fn update(&mut self, _ctx: &ModuleContext) {
        // `try_recv` returns at once when nothing is waiting.
        while let Ok(event) = self.worker.events.try_recv() {
            self.apply(event);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _ctx: &ModuleContext) {
        ui::show(self, ui);
    }

    fn on_exit(&mut self) {
        self.cancel_login();
        self.saver.finish();
    }
}
