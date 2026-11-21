//! Cleans up the GitHub token older versions of Athena asked for.
//!
//! The repo used to be private, so the updater needed a read-only token,
//! kept in Windows Credential Manager as `github-update-token.Athena`. The
//! repo is public now and the updater reads releases without one, so a
//! token left behind there is only removed. (On Linux, where Athena is only
//! built for testing, `keyring` keeps entries in memory.)

use anyhow::{Context, Result};
use keyring::Entry;

const SERVICE: &str = "Athena";
const USER: &str = "github-update-token";

/// Deletes the old token, if there is one. Having none is not an error.
pub fn remove_old() -> Result<()> {
    let entry = Entry::new(SERVICE, USER).context("can't open Windows Credential Manager")?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => {
            Err(error).context("can't remove the old GitHub token from Windows Credential Manager")
        }
    }
}
