//! Refresh tokens, kept only in Windows Credential Manager.
//!
//! One entry per character, shown under "Windows Credentials" as
//! `esi-refresh-<character id>.Athena`. They never go into the data folder,
//! settings, logs or debug output. (On Linux, where Athena is only built for
//! testing, `keyring` keeps them in memory instead.)
//!
//! Only the refresh token is stored. The access token is a 2-4 KB JWT with
//! all 72 scopes in it, which is too big for a Credential Manager entry
//! (2,560 bytes at most). It lives only in memory and is renewed with the
//! refresh token whenever it's needed.

use anyhow::{Context, Result};
use keyring::Entry;

const SERVICE: &str = "Athena";

fn entry(character_id: u64) -> Result<Entry> {
    Entry::new(SERVICE, &format!("esi-refresh-{character_id}"))
        .context("can't open Windows Credential Manager")
}

/// The saved refresh token for a character, or `None` if there is none.
pub fn load(character_id: u64) -> Result<Option<String>> {
    match entry(character_id)?.get_password() {
        Ok(token) => Ok(Some(token)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(error).context("can't read a login from Windows Credential Manager"),
    }
}

pub fn save(character_id: u64, refresh_token: &str) -> Result<()> {
    entry(character_id)?
        .set_password(refresh_token)
        .context("can't save the login in Windows Credential Manager")
}

pub fn remove(character_id: u64) -> Result<()> {
    match entry(character_id)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(error).context("can't remove the login from Windows Credential Manager"),
    }
}
