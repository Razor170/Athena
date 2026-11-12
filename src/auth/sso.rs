//! Talking to EVE's login server (SSO): the login URL, trading the login code
//! for tokens, renewing tokens and revoking them.
//!
//! Follows <https://developers.eveonline.com/docs/services/sso/>, the
//! "Authorization Code with PKCE" flow: only the Client ID is sent, never a
//! client secret.
//!
//! Requests block, so call these only from a background thread.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use reqwest::StatusCode;
use reqwest::blocking::Client;
use serde::Deserialize;

use super::pkce::Pkce;
use super::{CALLBACK_URL, CLIENT_ID, scopes};

const AUTHORIZE_URL: &str = "https://login.eveonline.com/v2/oauth/authorize";
const TOKEN_URL: &str = "https://login.eveonline.com/v2/oauth/token";
const REVOKE_URL: &str = "https://login.eveonline.com/v2/oauth/revoke";

/// The HTTP client for the login server.
pub fn http_client() -> Result<Client> {
    Client::builder()
        .user_agent(crate::esi::USER_AGENT)
        .timeout(Duration::from_secs(30))
        .build()
        .context("can't set up the HTTP client")
}

/// The EVE login page's address for one login attempt, asking for every scope.
pub fn authorize_url(pkce: &Pkce) -> Result<String> {
    let url = reqwest::Url::parse_with_params(
        AUTHORIZE_URL,
        &[
            ("response_type", "code"),
            ("redirect_uri", CALLBACK_URL),
            ("client_id", CLIENT_ID),
            ("scope", &scopes::joined()),
            ("code_challenge", &pkce.challenge),
            ("code_challenge_method", "S256"),
            ("state", &pkce.state),
        ],
    )
    .context("can't build the login URL")?;
    Ok(url.to_string())
}

/// What the login server answers when it hands out tokens.
#[derive(Deserialize)]
struct TokenAnswer {
    access_token: String,
    /// Seconds the access token is valid (about 20 minutes).
    expires_in: u64,
    refresh_token: String,
}

/// Fresh tokens for one character.
pub struct Tokens {
    pub access_token: String,
    /// When the access token stops working, as Unix seconds.
    pub expires_at: i64,
    /// EVE hands out a new refresh token with every renewal; this is the one
    /// to keep from now on.
    pub refresh_token: String,
    /// Who the tokens are for, read from the access token.
    pub character: TokenCharacter,
}

/// Why renewing a token failed.
pub enum RefreshError {
    /// EVE refused the refresh token (revoked, password changed, character
    /// sold, ...). Only a new login helps.
    LoginExpired,
    /// Anything else, such as no network or EVE being down. Worth trying again later.
    Other(anyhow::Error),
}

/// Trades the code from the login page for tokens.
pub fn exchange_code(http: &Client, code: &str, pkce: &Pkce) -> Result<Tokens> {
    let response = http
        .post(TOKEN_URL)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("client_id", CLIENT_ID),
            ("code_verifier", &pkce.verifier),
        ])
        .send()
        .context("can't reach EVE's login server")?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().unwrap_or_default();
        bail!("EVE's login server answered {status}: {}", short(&body));
    }
    let answer: TokenAnswer = response
        .json()
        .context("can't read the login server's answer")?;
    tokens_from(answer)
}

/// Gets a new access token with a refresh token.
pub fn refresh(http: &Client, refresh_token: &str) -> Result<Tokens, RefreshError> {
    let response = http
        .post(TOKEN_URL)
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", CLIENT_ID),
        ])
        .send()
        .map_err(|e| {
            RefreshError::Other(anyhow::Error::new(e).context("can't reach EVE's login server"))
        })?;
    let status = response.status();
    if status == StatusCode::BAD_REQUEST || status == StatusCode::UNAUTHORIZED {
        // OAuth answers `invalid_grant` with 400 when the refresh token is
        // no good any more.
        let body = response.text().unwrap_or_default();
        if body.contains("invalid_grant") || status == StatusCode::UNAUTHORIZED {
            return Err(RefreshError::LoginExpired);
        }
        return Err(RefreshError::Other(anyhow::anyhow!(
            "EVE's login server answered {status}: {}",
            short(&body)
        )));
    }
    if !status.is_success() {
        return Err(RefreshError::Other(anyhow::anyhow!(
            "EVE's login server answered {status}"
        )));
    }
    let answer: TokenAnswer = response.json().map_err(|e| {
        RefreshError::Other(anyhow::Error::new(e).context("can't read the login server's answer"))
    })?;
    tokens_from(answer).map_err(RefreshError::Other)
}

/// Tells EVE to forget a refresh token, so it can never be used again.
/// Used when a character is removed from Athena.
pub fn revoke(http: &Client, refresh_token: &str) -> Result<()> {
    let response = http
        .post(REVOKE_URL)
        .form(&[
            ("token_type_hint", "refresh_token"),
            ("token", refresh_token),
            ("client_id", CLIENT_ID),
        ])
        .send()
        .context("can't reach EVE's login server")?;
    if !response.status().is_success() {
        bail!("EVE's login server answered {}", response.status());
    }
    Ok(())
}

fn tokens_from(answer: TokenAnswer) -> Result<Tokens> {
    let character = read_access_token(&answer.access_token)?;
    let expires_at = crate::auth::now_unix() + answer.expires_in as i64;
    Ok(Tokens {
        access_token: answer.access_token,
        expires_at,
        refresh_token: answer.refresh_token,
        character,
    })
}

/// The parts of the access token we use.
#[derive(Debug, Clone, PartialEq)]
pub struct TokenCharacter {
    pub id: u64,
    pub name: String,
    /// Changes when the character moves to another EVE account (for
    /// example when it's sold).
    pub owner_hash: String,
    /// The scopes this token has.
    pub scopes: Vec<String>,
}

/// The access token's claims (its middle part). See "JWT Token Claims" in
/// the SSO docs.
#[derive(Deserialize)]
struct Claims {
    /// `CHARACTER:EVE:<character id>`
    sub: String,
    name: String,
    owner: String,
    /// One scope comes as a plain string, several as a list.
    #[serde(default)]
    scp: Option<OneOrMany>,
    iss: String,
    /// Our Client ID plus "EVE Online".
    aud: OneOrMany,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

impl OneOrMany {
    fn into_vec(self) -> Vec<String> {
        match self {
            OneOrMany::One(one) => vec![one],
            OneOrMany::Many(many) => many,
        }
    }
}

/// Reads who an access token is for and which scopes it has.
///
/// The access token is a JWT: three base64 parts joined by dots, the middle
/// one is JSON. We check that it comes from EVE's login server and is meant
/// for Athena. We don't check its signature: Athena got it straight from
/// login.eveonline.com over HTTPS a moment ago, so nobody could have
/// changed it, and ESI checks the signature itself on every request.
pub fn read_access_token(token: &str) -> Result<TokenCharacter> {
    let payload = token
        .split('.')
        .nth(1)
        .context("the access token is not a JWT")?;
    let json = URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .context("the access token is not valid base64")?;
    let claims: Claims =
        serde_json::from_slice(&json).context("can't read the access token's contents")?;

    if claims.iss != "https://login.eveonline.com" && claims.iss != "login.eveonline.com" {
        bail!("the access token comes from {}, not EVE", claims.iss);
    }
    let audience = claims.aud.into_vec();
    if !audience.iter().any(|a| a == CLIENT_ID) || !audience.iter().any(|a| a == "EVE Online") {
        bail!("the access token is meant for another application");
    }
    let id = claims
        .sub
        .strip_prefix("CHARACTER:EVE:")
        .and_then(|id| id.parse().ok())
        .with_context(|| format!("the access token is for {}, not a character", claims.sub))?;
    Ok(TokenCharacter {
        id,
        name: claims.name,
        owner_hash: claims.owner,
        scopes: claims.scp.map(OneOrMany::into_vec).unwrap_or_default(),
    })
}

/// The first 200 characters of an error answer, for messages.
fn short(text: &str) -> String {
    text.chars().take(200).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Makes a fake access token with the given claims. The signature part
    /// is junk, which is fine since we don't check it.
    fn fake_token(claims: &str) -> String {
        format!(
            "{}.{}.c2ln",
            URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256"}"#),
            URL_SAFE_NO_PAD.encode(claims)
        )
    }

    fn claims(scp: &str, aud: &str) -> String {
        format!(
            r#"{{"scp":{scp},"sub":"CHARACTER:EVE:2119000001","name":"Tessa Vahl","owner":"abc123=","exp":1794462000,"iss":"https://login.eveonline.com","aud":{aud}}}"#
        )
    }

    #[test]
    fn reads_character_and_scopes() {
        let aud = format!(r#"["{CLIENT_ID}","EVE Online"]"#);
        let token = fake_token(&claims(
            r#"["esi-skills.read_skills.v1","esi-wallet.read_character_wallet.v1"]"#,
            &aud,
        ));
        let character = read_access_token(&token).unwrap();
        assert_eq!(character.id, 2_119_000_001);
        assert_eq!(character.name, "Tessa Vahl");
        assert_eq!(character.owner_hash, "abc123=");
        assert_eq!(character.scopes.len(), 2);
    }

    #[test]
    fn a_single_scope_is_a_plain_string() {
        let aud = format!(r#"["{CLIENT_ID}","EVE Online"]"#);
        let token = fake_token(&claims(r#""esi-skills.read_skills.v1""#, &aud));
        assert_eq!(
            read_access_token(&token).unwrap().scopes,
            vec!["esi-skills.read_skills.v1"]
        );
    }

    #[test]
    fn rejects_tokens_for_other_apps() {
        let token = fake_token(&claims(r#"[]"#, r#"["someone-else","EVE Online"]"#));
        assert!(read_access_token(&token).is_err());
    }

    #[test]
    fn login_url_has_every_scope_and_pkce() {
        let pkce = Pkce::new().unwrap();
        let url = reqwest::Url::parse(&authorize_url(&pkce).unwrap()).unwrap();
        let get = |key: &str| {
            url.query_pairs()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.into_owned())
        };
        assert_eq!(get("code_challenge_method").as_deref(), Some("S256"));
        assert_eq!(get("code_challenge"), Some(pkce.challenge.clone()));
        assert_eq!(get("state"), Some(pkce.state.clone()));
        assert_eq!(get("redirect_uri").as_deref(), Some(CALLBACK_URL));
        let scope = get("scope").unwrap();
        assert_eq!(scope.split(' ').count(), scopes::ALL_SCOPES.len());
        // No secret of any kind in the URL.
        assert!(get("client_secret").is_none());
    }
}
