//! Skills: what every character trains now and when its queue ends
//! (Overview), and one character's skills by group with its training
//! queue (Character).
//!
//! The pieces:
//!
//! - [`worker`]: fetches skills, skill queue and attributes from ESI on a
//!   background thread and keeps a copy in `skills-cache.json`.
//! - [`data`]: what ESI sends, what Athena keeps, and the queue maths.
//! - [`config`]: settings and filters, saved in `skills.toml`.
//! - `ui`: draws the panel.
//!
//! Skill names, groups and ranks come from `static.sqlite`.

pub mod config;
pub mod data;
mod ui;
pub mod worker;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui;

use self::config::SkillsSettings;
use self::data::{CharacterSkills, QueueNow, QueueState, SkillInfo};
use self::worker::{Cache, Command, Event, Worker};
use super::shared::{RosterCharacter, SharedRoster, roster_if_changed};
use super::{Module, ModuleContext, SharedStaticData};
use crate::auth::TokenStore;
use crate::static_data::StaticData;
use crate::storage::Saver;

/// "Refresh" works at most this often.
const REFRESH_COOLDOWN_SECS: i64 = 60;

/// How a character's training looks, for the card colour and the badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    /// Not training: empty or paused queue, or no data (login expired).
    Stopped,
    /// Training, but the queue ends within the "soon" hours.
    EndsSoon,
    Training,
}

pub struct Skills {
    data_dir: PathBuf,
    saver: Saver,
    settings: SkillsSettings,
    worker: Worker,
    roster: SharedRoster,
    roster_version: u64,
    /// The logged-in characters, in the Characters tab's order.
    characters: Vec<RosterCharacter>,
    static_data: Option<Arc<StaticData>>,
    /// False until the worker has read the saved cache.
    loaded: bool,
    /// Character id -> their skills.
    skills: BTreeMap<u64, CharacterSkills>,
    /// Jita prices of skill injectors, for the queue footer.
    injector_prices: Option<worker::InjectorPrices>,
    /// When ESI was last asked.
    checked: Option<i64>,
    /// Problems per character, shown on their card.
    character_problems: HashMap<u64, String>,
    /// Problems to show on top of the panel.
    problems: Vec<String>,
    ui: ui::UiState,
    last_refresh: Option<i64>,
}

impl Skills {
    pub fn new(
        egui_ctx: &egui::Context,
        data_dir: &Path,
        tokens: Arc<TokenStore>,
        roster: SharedRoster,
        static_data: &SharedStaticData,
    ) -> Skills {
        let (settings, problem) = SkillsSettings::load(data_dir);
        let worker = Worker::start(
            egui_ctx,
            tokens,
            Cache::file_path(data_dir),
            settings.poll_secs(),
        );
        let mut problems: Vec<String> = problem.into_iter().collect();
        if let Some(problem) = &static_data.problem {
            problems.push(problem.clone());
        }
        let mut module = Skills {
            data_dir: data_dir.to_path_buf(),
            saver: Saver::start(),
            settings,
            worker,
            roster,
            roster_version: 0,
            characters: Vec::new(),
            static_data: static_data.data.clone(),
            loaded: false,
            skills: BTreeMap::new(),
            injector_prices: None,
            checked: None,
            character_problems: HashMap::new(),
            problems,
            ui: ui::UiState::default(),
            last_refresh: None,
        };
        module.read_roster();
        module
    }

    /// Takes the newest character list from the Characters module, and
    /// tells the worker if it changed.
    fn read_roster(&mut self) {
        let Some(roster) = roster_if_changed(&self.roster, self.roster_version) else {
            return;
        };
        self.roster_version = roster.version;
        self.characters = roster.characters;
        if crate::auth::is_configured() {
            let ids = self.characters.iter().map(|c| c.id).collect();
            self.worker.send(Command::SetCharacters(ids));
        }
    }

    fn save_settings(&mut self) {
        if let Ok(text) = toml::to_string(&self.settings) {
            self.saver
                .save(SkillsSettings::file_path(&self.data_dir), text);
        }
    }

    /// "Refresh": ask ESI for everything now.
    pub fn refresh_now(&mut self) {
        let now = crate::auth::now_unix();
        if self.can_refresh(now) {
            self.last_refresh = Some(now);
            self.worker.send(Command::RefreshNow);
        }
    }

    pub fn can_refresh(&self, now: i64) -> bool {
        self.last_refresh
            .is_none_or(|at| now - at >= REFRESH_COOLDOWN_SECS)
    }

    /// The queue of one character as of `now`, or `None` without data.
    pub fn queue_now(&self, character_id: u64, now: i64) -> Option<QueueNow> {
        let skills = self.skills.get(&character_id)?;
        let static_data = self.static_data.as_deref();
        Some(data::queue_now(skills, now, |id| {
            let skill = static_data?.skill(id)?;
            Some(SkillInfo {
                rank: skill.rank,
                primary: skill.primary_attribute,
                secondary: skill.secondary_attribute,
            })
        }))
    }

    /// How one character's training looks at `now`.
    pub fn training(&self, character_id: u64, now: i64) -> Status {
        match self.queue_now(character_id, now).map(|q| q.state) {
            Some(QueueState::Training { ends }) => {
                if ends - now <= i64::from(self.settings.soon_hours) * 3600 {
                    Status::EndsSoon
                } else {
                    Status::Training
                }
            }
            _ => Status::Stopped,
        }
    }

    /// The characters the Overview shows: tag filter, "hide without data"
    /// and the sort order applied.
    pub fn shown_characters(&self, now: i64) -> Vec<&RosterCharacter> {
        let mut list: Vec<&RosterCharacter> = self
            .characters
            .iter()
            .filter(|c| {
                self.settings
                    .tag
                    .as_ref()
                    .is_none_or(|tag| c.tags.contains(tag))
            })
            .filter(|c| !self.settings.hide_without_data || self.skills.contains_key(&c.id))
            .collect();
        match self.settings.sort {
            config::SortBy::Name => list.sort_by_key(|c| c.name.to_lowercase()),
            config::SortBy::SkillPoints => list.sort_by_key(|c| {
                std::cmp::Reverse(self.skills.get(&c.id).map_or(0, |s| s.total_sp))
            }),
            config::SortBy::QueueEnd => list.sort_by_key(|c| {
                // Not training first, then the queue that ends soonest.
                match self.queue_now(c.id, now).map(|q| q.state) {
                    Some(QueueState::Training { ends }) => (1, ends),
                    _ => (0, 0),
                }
            }),
        }
        list
    }

    fn apply(&mut self, event: Event) {
        match event {
            Event::Loaded(cache) => {
                self.loaded = true;
                // Anything the worker already sent wins over the saved copy.
                for (id, skills) in cache.characters {
                    self.skills.entry(id).or_insert(skills);
                }
                if self.injector_prices.is_none() {
                    self.injector_prices = cache.injector_prices;
                }
            }
            Event::Skills {
                character_id,
                skills,
            } => {
                self.skills.insert(character_id, *skills);
            }
            Event::Checked(at) => self.checked = Some(at),
            Event::CharacterProblem {
                character_id,
                problem,
            } => match problem {
                Some(text) => {
                    self.character_problems.insert(character_id, text);
                }
                None => {
                    self.character_problems.remove(&character_id);
                }
            },
            Event::Prices(prices) => self.injector_prices = Some(prices),
            Event::Problem(text) => self.problems.push(text),
        }
    }
}

impl Module for Skills {
    fn id(&self) -> &'static str {
        "skills"
    }

    fn title(&self) -> &'static str {
        "Skills"
    }

    fn badge(&self) -> Option<String> {
        if !self.settings.badge {
            return None;
        }
        let now = crate::auth::now_unix();
        let count = self
            .characters
            .iter()
            .filter(|c| self.training(c.id, now) == Status::Stopped)
            .count();
        (count > 0).then(|| count.to_string())
    }

    fn update(&mut self, _ctx: &ModuleContext) {
        self.read_roster();
        // `try_recv` returns at once when nothing is waiting.
        while let Ok(event) = self.worker.events.try_recv() {
            self.apply(event);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _ctx: &ModuleContext) {
        let now = crate::auth::now_unix();
        ui::show(self, ui, now);
        // Time left moves with the clock: redraw once a minute while the
        // tab is visible, at the start of the next minute.
        let wait = 60 - now.rem_euclid(60) + 1;
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs(wait as u64));
    }

    fn on_exit(&mut self) {
        self.saver.finish();
    }
}
