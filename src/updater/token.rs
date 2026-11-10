//! The GitHub token, kept only in Windows Credential Manager.
//!
//! It never goes into the data folder, settings, logs or debug output. In
//! Credential Manager it shows up under "Windows Credentials" as
//! `github-update-token.Athena`. (On Linux, where Athena is only built for
//! testing, `keyring` keeps it in memory instead.)

use anyhow::{Context, Result};
use keyring::Entry;

const SERVICE: &str = "Athena";
const USER: &str = "github-update-token";

fn entry() -> Result<Entry> {
    Entry::new(SERVICE, USER).context("can't open Windows Credential Manager")
}

/// The saved token, or `None` if there is none.
pub fn load() -> Result<Option<String>> {
    match entry()?.get_password() {
        Ok(token) => Ok(Some(token)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => {
            Err(error).context("can't read the GitHub token from Windows Credential Manager")
        }
    }
}

pub fn save(token: &str) -> Result<()> {
    entry()?
        .set_password(token)
        .context("can't save the GitHub token in Windows Credential Manager")
}

pub fn remove() -> Result<()> {
    match entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => {
            Err(error).context("can't remove the GitHub token from Windows Credential Manager")
        }
    }
}
