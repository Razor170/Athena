//! PKCE ("proof key for code exchange"), which lets Athena log in without a
//! client secret.
//!
//! Before the login Athena makes a random secret, the *verifier*, and sends
//! only its SHA-256 hash (the *challenge*) to EVE's login page. When it trades
//! the login code for tokens it sends the verifier itself. Someone who catches
//! the code can't use it without the verifier, which never left Athena.
//! See <https://developers.eveonline.com/docs/services/sso/>.

use anyhow::{Result, anyhow};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

/// One login attempt's secrets.
pub struct Pkce {
    /// Sent when trading the code for tokens.
    pub verifier: String,
    /// The verifier's hash, sent with the login URL.
    pub challenge: String,
    /// A random value sent with the login URL and checked when the browser
    /// comes back, so a page can't send us a code we didn't ask for.
    pub state: String,
}

impl Pkce {
    pub fn new() -> Result<Pkce> {
        let verifier = random_text(32)?;
        let challenge = challenge_for(&verifier);
        let state = random_text(16)?;
        Ok(Pkce {
            verifier,
            challenge,
            state,
        })
    }
}

/// `bytes` random bytes from the operating system, as URL-safe base64 text.
fn random_text(bytes: usize) -> Result<String> {
    let mut buffer = vec![0u8; bytes];
    aws_lc_rs::rand::fill(&mut buffer).map_err(|_| anyhow!("no random numbers available"))?;
    Ok(URL_SAFE_NO_PAD.encode(&buffer))
}

/// The S256 challenge: base64url (no padding) of the SHA-256 of the verifier.
fn challenge_for(verifier: &str) -> String {
    let hash = aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(hash.as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn challenge_matches_the_rfc_example() {
        // The example from RFC 7636, appendix B.
        assert_eq!(
            challenge_for("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn each_login_gets_new_secrets() {
        let a = Pkce::new().unwrap();
        let b = Pkce::new().unwrap();
        assert_ne!(a.verifier, b.verifier);
        assert_ne!(a.state, b.state);
        // 32 bytes are 43 base64 characters, within PKCE's 43 to 128.
        assert_eq!(a.verifier.len(), 43);
    }
}
