//! The list of characters and accounts, saved in `characters.toml`.
//!
//! Only public facts go here: names, ids, which account a character is in,
//! the scopes it granted. The login itself (the refresh token) lives in
//! Windows Credential Manager, see `crate::auth::vault`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::auth::scopes::ALL_SCOPES;
use crate::auth::sso::TokenCharacter;

/// EVE allows three characters per account.
pub const SLOTS_PER_ACCOUNT: usize = 3;

pub const FILE_NAME: &str = "characters.toml";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CharacterList {
    /// How many columns of account cards the panel shows (1 to 4).
    #[serde(default = "default_columns")]
    pub columns: u8,
    /// Accounts in the order they're shown.
    #[serde(default)]
    pub accounts: Vec<Account>,
    /// Every logged-in character. The ones in no account show under
    /// "Unassigned", in this order.
    #[serde(default)]
    pub characters: Vec<Character>,
}

fn default_columns() -> u8 {
    3
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Account {
    pub name: String,
    /// Character ids, in the order shown. At most [`SLOTS_PER_ACCOUNT`].
    #[serde(default)]
    pub characters: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Character {
    pub id: u64,
    pub name: String,
    /// Changes when the character moves to another EVE account.
    pub owner_hash: String,
    /// The scopes the last token had.
    #[serde(default)]
    pub scopes: Vec<String>,
    /// When it was first logged in, Unix seconds.
    #[serde(default)]
    pub added_unix: i64,
    #[serde(default)]
    pub corporation_id: Option<u64>,
    #[serde(default)]
    pub corporation_name: Option<String>,
    #[serde(default)]
    pub corporation_ticker: Option<String>,
    /// The corporation's alliance, if it has one.
    #[serde(default)]
    pub alliance: Option<Affiliation>,
    /// Kevin's own labels, such as "Miner" or "Cyno". Any number, in the
    /// order they were added.
    #[serde(default)]
    pub tags: Vec<String>,
}

impl Character {
    /// How many of Athena's scopes this character hasn't granted yet.
    pub fn missing_scopes(&self) -> usize {
        ALL_SCOPES
            .iter()
            .filter(|scope| !self.scopes.iter().any(|s| s == *scope))
            .count()
    }

    /// "Corp Name [TICK]", or just the name, or nothing yet.
    pub fn corporation_label(&self) -> Option<String> {
        match (&self.corporation_name, &self.corporation_ticker) {
            (Some(name), Some(ticker)) => Some(format!("{name} [{ticker}]")),
            (Some(name), None) => Some(name.clone()),
            _ => None,
        }
    }

    /// "Alliance Name <TICK>", the way EVE writes alliance tickers.
    pub fn alliance_label(&self) -> Option<String> {
        self.alliance
            .as_ref()
            .map(|a| format!("{} <{}>", a.name, a.ticker))
    }
}

/// An alliance's id, name and ticker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Affiliation {
    pub id: u64,
    pub name: String,
    pub ticker: String,
}

/// Where a character should go when it's dragged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Unassigned,
    /// An account, by its position in the list.
    Account(usize),
}

impl CharacterList {
    pub fn file_path(data_dir: &Path) -> PathBuf {
        data_dir.join(FILE_NAME)
    }

    /// Loads the list; an empty one if the file is missing, plus a warning
    /// if it couldn't be read. Runs once at startup.
    pub fn load(data_dir: &Path) -> (CharacterList, Option<String>) {
        let path = Self::file_path(data_dir);
        let empty = CharacterList {
            columns: default_columns(),
            ..CharacterList::default()
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (empty, None),
            Err(e) => {
                return (empty, Some(format!("Can't read {}: {e}", path.display())));
            }
        };
        match toml::from_str::<CharacterList>(&text) {
            Ok(mut list) => {
                list.tidy();
                (list, None)
            }
            Err(e) => (
                empty,
                Some(format!("{} is broken and was ignored: {e}", path.display())),
            ),
        }
    }

    pub fn to_toml(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }

    /// Fixes what a hand-edited file could get wrong: unknown or doubled
    /// ids in accounts, too many characters in one account, odd columns.
    fn tidy(&mut self) {
        self.columns = self.columns.clamp(1, 4);
        let mut seen = Vec::new();
        for account in &mut self.accounts {
            account.characters.retain(|id| {
                let known = self.characters.iter().any(|c| c.id == *id);
                let fresh = !seen.contains(id);
                seen.push(*id);
                known && fresh
            });
            account.characters.truncate(SLOTS_PER_ACCOUNT);
        }
    }

    pub fn character(&self, id: u64) -> Option<&Character> {
        self.characters.iter().find(|c| c.id == id)
    }

    fn character_mut(&mut self, id: u64) -> Option<&mut Character> {
        self.characters.iter_mut().find(|c| c.id == id)
    }

    /// The characters in no account, in list order.
    pub fn unassigned(&self) -> Vec<&Character> {
        self.characters
            .iter()
            .filter(|c| self.account_of(c.id).is_none())
            .collect()
    }

    /// The account a character is in, by position.
    pub fn account_of(&self, id: u64) -> Option<usize> {
        self.accounts
            .iter()
            .position(|a| a.characters.contains(&id))
    }

    /// Adds a character after a login, or updates it if it's already listed
    /// (a new login keeps its account). Returns true if it's new.
    pub fn add_or_update(&mut self, from_token: &TokenCharacter, now_unix: i64) -> bool {
        if let Some(existing) = self.character_mut(from_token.id) {
            let owner_changed = existing.owner_hash != from_token.owner_hash;
            existing.name = from_token.name.clone();
            existing.owner_hash = from_token.owner_hash.clone();
            existing.scopes = from_token.scopes.clone();
            if owner_changed {
                // The character is on another EVE account now, so the
                // account it was sorted into is wrong.
                self.take_out_of_accounts(from_token.id);
            }
            return false;
        }
        self.characters.push(Character {
            id: from_token.id,
            name: from_token.name.clone(),
            owner_hash: from_token.owner_hash.clone(),
            scopes: from_token.scopes.clone(),
            added_unix: now_unix,
            corporation_id: None,
            corporation_name: None,
            corporation_ticker: None,
            alliance: None,
            tags: Vec::new(),
        });
        true
    }

    /// Every tag in use, sorted by name (case ignored), each once.
    pub fn all_tags(&self) -> Vec<String> {
        let mut tags: Vec<String> = Vec::new();
        for character in &self.characters {
            for tag in &character.tags {
                if !tags.iter().any(|t| t.eq_ignore_ascii_case(tag)) {
                    tags.push(tag.clone());
                }
            }
        }
        tags.sort_by_key(|t| t.to_lowercase());
        tags
    }

    /// Adds a tag to a character. A tag that's already in use elsewhere
    /// keeps its spelling ("miner" becomes "Miner"). Returns true if added.
    pub fn add_tag(&mut self, id: u64, tag: &str) -> bool {
        let tag = tag.trim();
        if tag.is_empty() {
            return false;
        }
        let spelling = self
            .all_tags()
            .into_iter()
            .find(|t| t.eq_ignore_ascii_case(tag))
            .unwrap_or_else(|| tag.to_string());
        let Some(character) = self.character_mut(id) else {
            return false;
        };
        if character
            .tags
            .iter()
            .any(|t| t.eq_ignore_ascii_case(&spelling))
        {
            return false;
        }
        character.tags.push(spelling);
        true
    }

    pub fn remove_tag(&mut self, id: u64, tag: &str) -> bool {
        let Some(character) = self.character_mut(id) else {
            return false;
        };
        let before = character.tags.len();
        character.tags.retain(|t| t != tag);
        character.tags.len() != before
    }

    /// Updates the scopes after a token renewal. Returns true if they changed.
    pub fn set_scopes(&mut self, id: u64, scopes: &[String]) -> bool {
        match self.character_mut(id) {
            Some(c) if c.scopes != scopes => {
                c.scopes = scopes.to_vec();
                true
            }
            _ => false,
        }
    }

    /// Sets the corporation and alliance. Returns true if anything changed.
    pub fn set_corporation(
        &mut self,
        id: u64,
        corporation: &Affiliation,
        alliance: Option<&Affiliation>,
    ) -> bool {
        let Some(c) = self.character_mut(id) else {
            return false;
        };
        let changed = c.corporation_id != Some(corporation.id)
            || c.corporation_name.as_deref() != Some(corporation.name.as_str())
            || c.corporation_ticker.as_deref() != Some(corporation.ticker.as_str())
            || c.alliance.as_ref() != alliance;
        c.corporation_id = Some(corporation.id);
        c.corporation_name = Some(corporation.name.clone());
        c.corporation_ticker = Some(corporation.ticker.clone());
        c.alliance = alliance.cloned();
        changed
    }

    pub fn remove_character(&mut self, id: u64) {
        self.take_out_of_accounts(id);
        self.characters.retain(|c| c.id != id);
    }

    fn take_out_of_accounts(&mut self, id: u64) {
        for account in &mut self.accounts {
            account.characters.retain(|c| *c != id);
        }
    }

    /// Moves a character into an account (or back to Unassigned) at
    /// `index` (clamped to the end). Refuses a full account and returns false.
    pub fn move_character(&mut self, id: u64, to: Place, index: usize) -> bool {
        if self.character(id).is_none() {
            return false;
        }
        match to {
            Place::Unassigned => {
                self.take_out_of_accounts(id);
                // Unassigned follows the character list's order.
                let Some(from) = self.characters.iter().position(|c| c.id == id) else {
                    return false;
                };
                let character = self.characters.remove(from);
                let others: Vec<usize> = self
                    .characters
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| self.account_of(c.id).is_none())
                    .map(|(i, _)| i)
                    .collect();
                let at = others.get(index).copied().unwrap_or(self.characters.len());
                self.characters.insert(at, character);
                true
            }
            Place::Account(target) => {
                let Some(account) = self.accounts.get(target) else {
                    return false;
                };
                let already_there = account.characters.contains(&id);
                if !already_there && account.characters.len() >= SLOTS_PER_ACCOUNT {
                    return false;
                }
                self.take_out_of_accounts(id);
                let list = &mut self.accounts[target].characters;
                let at = index.min(list.len());
                list.insert(at, id);
                true
            }
        }
    }

    /// Adds an empty account with a free "Account N" name. Returns its position.
    pub fn add_account(&mut self) -> usize {
        let mut n = self.accounts.len() + 1;
        while self
            .accounts
            .iter()
            .any(|a| a.name == format!("Account {n}"))
        {
            n += 1;
        }
        self.accounts.push(Account {
            name: format!("Account {n}"),
            characters: Vec::new(),
        });
        self.accounts.len() - 1
    }

    /// Deletes an account; its characters go back to Unassigned.
    pub fn delete_account(&mut self, index: usize) {
        if index < self.accounts.len() {
            self.accounts.remove(index);
        }
    }

    /// Moves an account one place up (`-1`) or down (`1`).
    pub fn shift_account(&mut self, index: usize, by: isize) {
        let target = index as isize + by;
        if index < self.accounts.len() && target >= 0 && (target as usize) < self.accounts.len() {
            self.accounts.swap(index, target as usize);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token(id: u64, name: &str, owner: &str) -> TokenCharacter {
        TokenCharacter {
            id,
            name: name.to_string(),
            owner_hash: owner.to_string(),
            scopes: ALL_SCOPES.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn list_with(ids: &[u64]) -> CharacterList {
        let mut list = CharacterList {
            columns: 3,
            ..CharacterList::default()
        };
        for id in ids {
            list.add_or_update(&token(*id, &format!("Pilot {id}"), "o"), 0);
        }
        list
    }

    #[test]
    fn new_logins_are_unassigned() {
        let list = list_with(&[1, 2]);
        let ids: Vec<u64> = list.unassigned().iter().map(|c| c.id).collect();
        assert_eq!(ids, vec![1, 2]);
    }

    #[test]
    fn logging_in_again_keeps_the_account() {
        let mut list = list_with(&[1]);
        let a = list.add_account();
        assert!(list.move_character(1, Place::Account(a), 0));
        assert!(!list.add_or_update(&token(1, "Pilot 1", "o"), 5));
        assert_eq!(list.account_of(1), Some(a));
    }

    #[test]
    fn a_sold_character_leaves_its_account() {
        let mut list = list_with(&[1]);
        let a = list.add_account();
        list.move_character(1, Place::Account(a), 0);
        list.add_or_update(&token(1, "Pilot 1", "new owner"), 5);
        assert_eq!(list.account_of(1), None);
    }

    #[test]
    fn accounts_hold_three_characters() {
        let mut list = list_with(&[1, 2, 3, 4]);
        let a = list.add_account();
        for id in [1, 2, 3] {
            assert!(list.move_character(id, Place::Account(a), 9));
        }
        assert!(!list.move_character(4, Place::Account(a), 0));
        assert_eq!(list.accounts[a].characters, vec![1, 2, 3]);
        // Reordering inside a full account still works.
        assert!(list.move_character(3, Place::Account(a), 0));
        assert_eq!(list.accounts[a].characters, vec![3, 1, 2]);
    }

    #[test]
    fn moving_between_accounts_and_back() {
        let mut list = list_with(&[1, 2, 3]);
        let a = list.add_account();
        let b = list.add_account();
        list.move_character(2, Place::Account(a), 0);
        list.move_character(2, Place::Account(b), 0);
        assert!(list.accounts[a].characters.is_empty());
        assert_eq!(list.accounts[b].characters, vec![2]);
        // Back to Unassigned, first place.
        list.move_character(2, Place::Unassigned, 0);
        let ids: Vec<u64> = list.unassigned().iter().map(|c| c.id).collect();
        assert_eq!(ids, vec![2, 1, 3]);
    }

    #[test]
    fn deleting_an_account_frees_its_characters() {
        let mut list = list_with(&[1]);
        let a = list.add_account();
        list.move_character(1, Place::Account(a), 0);
        list.delete_account(a);
        assert_eq!(list.unassigned().len(), 1);
    }

    #[test]
    fn new_account_names_are_unique() {
        let mut list = list_with(&[]);
        list.add_account();
        list.add_account();
        list.accounts[0].name = "Account 3".to_string();
        list.add_account();
        let names: Vec<&str> = list.accounts.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["Account 3", "Account 2", "Account 4"]);
    }

    #[test]
    fn tags_are_shared_and_not_doubled() {
        let mut list = list_with(&[1, 2]);
        assert!(list.add_tag(1, "Miner"));
        assert!(list.add_tag(1, " Hauler "));
        assert!(!list.add_tag(1, "miner"));
        assert!(!list.add_tag(1, "  "));
        // The second character gets the spelling already in use.
        assert!(list.add_tag(2, "miner"));
        assert_eq!(list.character(2).unwrap().tags, vec!["Miner"]);
        assert_eq!(list.all_tags(), vec!["Hauler", "Miner"]);
        assert!(list.remove_tag(1, "Miner"));
        assert_eq!(list.character(1).unwrap().tags, vec!["Hauler"]);
        // Logging in again keeps the tags.
        list.add_or_update(&token(2, "Pilot 2", "o"), 9);
        assert_eq!(list.character(2).unwrap().tags, vec!["Miner"]);
    }

    #[test]
    fn missing_scopes_are_counted() {
        let mut list = list_with(&[1]);
        assert_eq!(list.character(1).unwrap().missing_scopes(), 0);
        list.set_scopes(1, &["esi-skills.read_skills.v1".to_string()]);
        assert_eq!(
            list.character(1).unwrap().missing_scopes(),
            ALL_SCOPES.len() - 1
        );
    }

    #[test]
    fn saves_and_loads_and_tidies() {
        let mut list = list_with(&[1, 2]);
        let a = list.add_account();
        list.move_character(1, Place::Account(a), 0);
        let corp = Affiliation {
            id: 98000001,
            name: "Deep Core Mining".to_string(),
            ticker: "DCMN".to_string(),
        };
        let alliance = Affiliation {
            id: 99000001,
            name: "Northern Ore Coalition".to_string(),
            ticker: "NOCO".to_string(),
        };
        list.set_corporation(1, &corp, Some(&alliance));
        assert_eq!(
            list.character(1).unwrap().alliance_label().as_deref(),
            Some("Northern Ore Coalition <NOCO>")
        );
        list.add_tag(1, "Miner");
        let text = list.to_toml().unwrap();
        // No token-like fields ever end up in the file.
        assert!(!text.contains("token"));

        // A hand edit that puts an unknown id and a double into an account.
        let edited = text.replace("characters = [1]", "characters = [1, 1, 77]");
        let dir = std::env::temp_dir().join(format!("athena-chars-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(CharacterList::file_path(&dir), edited).unwrap();
        let (loaded, warning) = CharacterList::load(&dir);
        assert!(warning.is_none());
        assert_eq!(loaded, list);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
