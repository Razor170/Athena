//! Logging characters in with EVE's SSO and handing out their ESI access tokens.
//!
//! - [`sso`]: the login server (login URL, code to tokens, renewing, revoking).
//! - [`callback`]: the local web server that catches the browser after the login.
//! - [`pkce`]: the per-login secrets that replace a client secret.
//! - [`vault`]: refresh tokens in Windows Credential Manager.
//! - [`scopes`]: the scopes Athena asks for (all of them).
//! - [`TokenStore`]: access tokens in memory, renewed when needed. Modules
//!   that call ESI for a character get their token from here.
//!
//! The Characters module (`src/modules/characters`) drives the login and
//! shows the characters.

pub mod callback;
pub mod pkce;
pub mod scopes;
pub mod sso;
pub mod vault;

use std::collections::HashMap;
use std::sync::Mutex;

use reqwest::blocking::Client;

pub use sso::RefreshError;

/// Athena's ESI application, registered by Kevin on developers.eveonline.com.
/// The Client ID is public (it's in every login URL), so it may be in the
/// code. There is no client secret anywhere: the login uses PKCE.
pub const CLIENT_ID: &str = "bc0f0aacd6494508bec45921467f1383";

/// The local port the login comes back to. Must match the Callback URL of
/// the ESI application.
pub const CALLBACK_PORT: u16 = 51547;

/// Where EVE sends the browser after the login. Registered with the ESI
/// application exactly like this.
pub const CALLBACK_URL: &str = "http://localhost:51547/callback";

/// True once a Client ID is filled in above.
pub fn is_configured() -> bool {
    !CLIENT_ID.is_empty()
}

/// Seconds since 1970 (Unix time), the way the tokens count time.
pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Renew an access token this many seconds before it runs out, so a request
/// never starts with a token that expires on the way.
const RENEW_EARLY_SECS: i64 = 60;

/// An access token we have in memory.
struct AccessToken {
    token: String,
    expires_at: i64,
}

/// Access tokens for all logged-in characters, kept in memory only.
///
/// Shared between background threads through an `Arc<TokenStore>`; the
/// `Mutex` inside makes sure only one thread at a time touches it. That
/// matters because EVE hands out a new refresh token with every renewal and
/// the old one stops working soon after: two threads renewing the same
/// character at once could lose the login.
///
/// Never use it from the UI thread: [`TokenStore::access_token`] may wait
/// for the network.
pub struct TokenStore {
    tokens: Mutex<HashMap<u64, AccessToken>>,
}

impl TokenStore {
    pub fn new() -> TokenStore {
        TokenStore {
            tokens: Mutex::new(HashMap::new()),
        }
    }

    /// Keeps a fresh access token, for example right after a login.
    pub fn put(&self, character_id: u64, token: String, expires_at: i64) {
        if let Ok(mut tokens) = self.tokens.lock() {
            tokens.insert(character_id, AccessToken { token, expires_at });
        }
    }

    /// Forgets a character's access token, for example when it's removed.
    pub fn forget(&self, character_id: u64) {
        if let Ok(mut tokens) = self.tokens.lock() {
            tokens.remove(&character_id);
        }
    }

    /// A working access token for the character, renewed first if needed.
    /// Blocks while it renews, so call it only from a background thread.
    // Not called yet: the first module that reads a character's data from
    // ESI (wallet, skills, ...) gets its token through this.
    #[allow(dead_code)]
    pub fn access_token(&self, http: &Client, character_id: u64) -> Result<String, RefreshError> {
        // Held across the renewal on purpose, see the note on the struct.
        let mut tokens = self
            .tokens
            .lock()
            .map_err(|_| RefreshError::Other(anyhow::anyhow!("token store is broken")))?;
        if let Some(cached) = tokens.get(&character_id)
            && cached.expires_at - RENEW_EARLY_SECS > now_unix()
        {
            return Ok(cached.token.clone());
        }
        let fresh = renew(http, character_id)?;
        let token = fresh.access_token.clone();
        tokens.insert(
            character_id,
            AccessToken {
                token: fresh.access_token,
                expires_at: fresh.expires_at,
            },
        );
        Ok(token)
    }

    /// Renews the character's token now, even if the one in memory still
    /// works, and returns what the new token says about the character (its
    /// scopes, for example). Used at startup to check every login.
    pub fn renew_now(
        &self,
        http: &Client,
        character_id: u64,
    ) -> Result<sso::TokenCharacter, RefreshError> {
        let mut tokens = self
            .tokens
            .lock()
            .map_err(|_| RefreshError::Other(anyhow::anyhow!("token store is broken")))?;
        let fresh = renew(http, character_id)?;
        tokens.insert(
            character_id,
            AccessToken {
                token: fresh.access_token,
                expires_at: fresh.expires_at,
            },
        );
        Ok(fresh.character)
    }
}

/// Uses the saved refresh token to get new tokens, and saves the new
/// refresh token EVE sends back.
fn renew(http: &Client, character_id: u64) -> Result<sso::Tokens, RefreshError> {
    let refresh_token = vault::load(character_id)
        .map_err(RefreshError::Other)?
        .ok_or(RefreshError::LoginExpired)?;
    let fresh = sso::refresh(http, &refresh_token)?;
    if fresh.character.id != character_id {
        return Err(RefreshError::Other(anyhow::anyhow!(
            "the login server sent a token for another character"
        )));
    }
    if fresh.refresh_token != refresh_token {
        vault::save(character_id, &fresh.refresh_token).map_err(RefreshError::Other)?;
    }
    Ok(fresh)
}
