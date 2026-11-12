//! The tiny local web server that catches the browser coming back from
//! EVE's login page.
//!
//! It listens only on this PC (localhost, port [`CALLBACK_PORT`]) and only
//! while a login is running. It understands two addresses:
//!
//! - `/login` sends the browser on to EVE's login page. Athena opens this
//!   short address instead of the login URL itself, because the login URL
//!   with all 72 scopes is about 2,600 characters long, more than Windows
//!   reliably passes to the browser.
//! - `/callback?code=...&state=...` is where EVE sends the browser after
//!   the login. We check `state`, show "you can close this tab" and hand
//!   the code back.
//!
//! This is plain blocking code with `std::net`: one request at a time is
//! all a login needs, so no web server crate is needed.

use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use super::CALLBACK_PORT;

/// How long we wait for the user to finish logging in.
pub const LOGIN_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// How often the waiting loop looks for a new connection or a cancel.
/// Only runs while a login is in progress.
const POLL: Duration = Duration::from_millis(100);

/// The address Athena opens in the browser to start a login.
pub fn start_url() -> String {
    format!("http://127.0.0.1:{CALLBACK_PORT}/login")
}

/// The local server for one login.
pub struct CallbackServer {
    /// Browsers may try `localhost` as IPv6 (`::1`) or IPv4 (`127.0.0.1`),
    /// so we listen on both when we can.
    listeners: Vec<TcpListener>,
}

/// How a login attempt ended.
#[derive(Debug, PartialEq)]
pub enum Outcome {
    /// EVE sent a login code.
    Code(String),
    /// The user clicked Cancel on EVE's page, or EVE reported an error.
    Refused(String),
    /// Athena's Cancel button.
    Cancelled,
    TimedOut,
}

impl CallbackServer {
    /// Starts listening. Fails if another program (or another Athena) uses the port.
    pub fn start() -> Result<CallbackServer> {
        let mut listeners = Vec::new();
        let v4 = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, CALLBACK_PORT)))
            .with_context(|| {
                format!("port {CALLBACK_PORT} is in use, so the login can't come back to Athena")
            })?;
        listeners.push(v4);
        // IPv6 is a bonus: some PCs have it switched off.
        if let Ok(v6) = TcpListener::bind(SocketAddr::from((Ipv6Addr::LOCALHOST, CALLBACK_PORT))) {
            listeners.push(v6);
        }
        for listener in &listeners {
            // Non-blocking, so the loop below can also watch for Cancel.
            listener
                .set_nonblocking(true)
                .context("can't set up the login listener")?;
        }
        Ok(CallbackServer { listeners })
    }

    /// Answers browser requests until EVE sends a login code, the user
    /// cancels (`cancel` turns true) or [`LOGIN_TIMEOUT`] runs out.
    /// `login_url` is where `/login` sends the browser.
    pub fn wait(&self, login_url: &str, state: &str, cancel: &Arc<AtomicBool>) -> Outcome {
        let deadline = Instant::now() + LOGIN_TIMEOUT;
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Outcome::Cancelled;
            }
            if Instant::now() >= deadline {
                return Outcome::TimedOut;
            }
            let mut handled = false;
            for listener in &self.listeners {
                if let Ok((stream, _)) = listener.accept() {
                    handled = true;
                    if let Some(outcome) = handle(stream, login_url, state) {
                        return outcome;
                    }
                }
            }
            if !handled {
                std::thread::sleep(POLL);
            }
        }
    }
}

/// Answers one browser request. Returns the outcome if this was the callback.
fn handle(stream: TcpStream, login_url: &str, state: &str) -> Option<Outcome> {
    // Accepted sockets may inherit non-blocking mode; we want plain blocking
    // reads with a time limit, so a stuck browser can't hang the login.
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut reader = BufReader::new(&stream);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return None;
    }
    // "GET /callback?code=...&state=... HTTP/1.1"
    let target = request_line.split_whitespace().nth(1).unwrap_or("");
    let (reply, outcome) = route(target, login_url, state);
    let _ = write_reply(&stream, &reply);
    outcome
}

/// What to send back to the browser.
#[derive(Debug, PartialEq)]
enum Reply {
    Redirect(String),
    Page { title: String, text: String },
    NotFound,
}

/// Decides what a request means. Kept apart from the socket code so it can
/// be tested.
fn route(target: &str, login_url: &str, state: &str) -> (Reply, Option<Outcome>) {
    let Ok(url) = reqwest::Url::parse(&format!("http://localhost{target}")) else {
        return (Reply::NotFound, None);
    };
    match url.path() {
        "/login" => (Reply::Redirect(login_url.to_string()), None),
        "/callback" => {
            let get = |key: &str| {
                url.query_pairs()
                    .find(|(k, _)| k == key)
                    .map(|(_, v)| v.into_owned())
            };
            if get("state").as_deref() != Some(state) {
                // An old tab, or a page that isn't ours: ignore it.
                let page = Reply::Page {
                    title: "Athena: old login".to_string(),
                    text: "This login page is out of date. Start the login again in Athena."
                        .to_string(),
                };
                return (page, None);
            }
            if let Some(code) = get("code") {
                let page = Reply::Page {
                    title: "Athena: logged in".to_string(),
                    text: "Logged in. You can close this tab and go back to Athena.".to_string(),
                };
                return (page, Some(Outcome::Code(code)));
            }
            let why = get("error_description")
                .or_else(|| get("error"))
                .unwrap_or_else(|| "no login code came back".to_string());
            let page = Reply::Page {
                title: "Athena: login cancelled".to_string(),
                text: "The login didn't finish. You can close this tab.".to_string(),
            };
            (page, Some(Outcome::Refused(why)))
        }
        _ => (Reply::NotFound, None),
    }
}

fn write_reply(mut stream: &TcpStream, reply: &Reply) -> std::io::Result<()> {
    let response = match reply {
        Reply::Redirect(location) => format!(
            "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        ),
        Reply::Page { title, text } => {
            let body = format!(
                "<!doctype html><html><head><meta charset=\"utf-8\"><title>{title}</title></head>\
                 <body style=\"background:#1b1b1b;color:#ddd;font-family:Segoe UI,sans-serif;padding:40px\">\
                 <h2>{title}</h2><p>{text}</p></body></html>"
            );
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
        }
        Reply::NotFound => {
            "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string()
        }
    };
    stream.write_all(response.as_bytes())?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_redirects_to_eve() {
        let (reply, outcome) = route("/login", "https://login.eveonline.com/x", "s");
        assert_eq!(
            reply,
            Reply::Redirect("https://login.eveonline.com/x".to_string())
        );
        assert_eq!(outcome, None);
    }

    #[test]
    fn callback_with_the_right_state_gives_the_code() {
        let (_, outcome) = route("/callback?code=abc%2B1&state=s1", "", "s1");
        assert_eq!(outcome, Some(Outcome::Code("abc+1".to_string())));
    }

    #[test]
    fn callback_with_another_state_is_ignored() {
        let (_, outcome) = route("/callback?code=abc&state=other", "", "s1");
        assert_eq!(outcome, None);
    }

    #[test]
    fn cancel_on_eve_page_is_reported() {
        let (_, outcome) = route("/callback?error=access_denied&state=s1", "", "s1");
        assert_eq!(outcome, Some(Outcome::Refused("access_denied".to_string())));
    }

    #[test]
    fn other_paths_are_not_found() {
        let (reply, outcome) = route("/favicon.ico", "", "s1");
        assert_eq!(reply, Reply::NotFound);
        assert_eq!(outcome, None);
    }

    /// The whole round trip over a real socket: `/login` redirects, then
    /// the callback hands back the code.
    #[test]
    fn serves_a_login_over_localhost() {
        use std::io::Read;
        let server = CallbackServer::start().unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let waiting = std::thread::spawn(move || {
            server.wait(
                "https://login.eveonline.com/v2/oauth/authorize?x=1",
                "st",
                &cancel,
            )
        });
        let get = |path: &str| {
            let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, CALLBACK_PORT)).unwrap();
            write!(stream, "GET {path} HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap();
            let mut answer = String::new();
            stream.read_to_string(&mut answer).unwrap();
            answer
        };
        let redirect = get("/login");
        assert!(redirect.starts_with("HTTP/1.1 302"));
        assert!(redirect.contains("Location: https://login.eveonline.com/v2/oauth/authorize?x=1"));
        let page = get("/callback?code=the-code&state=st");
        assert!(page.contains("You can close this tab"));
        assert_eq!(
            waiting.join().unwrap(),
            Outcome::Code("the-code".to_string())
        );
    }
}
