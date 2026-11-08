//! The Intel Supervisor's ESI work, on its own background thread:
//!
//! - **Pilot names:** the parser's pilot-name guesses are looked up with
//!   `POST /universe/ids` (up to 500 names per call). Only names that are
//!   real characters come back; those become zKillboard links. Every name
//!   is asked about once per session, hits and misses alike (ESI itself
//!   caches these answers for 12 hours); only a failed call is asked
//!   again, when the name next turns up.
//! - **Activity tint (optional):** `GET /universe/system_kills` and
//!   `/universe/system_jumps`, the kills and jumps of the last hour per
//!   system. ESI updates them once an hour, so they are fetched at most
//!   once an hour, and only while the tint is switched on.
//!
//! Only public endpoints, no login. Requests block, which is why they run
//! here and never on the UI thread; results come back through a channel
//! and ask egui for one repaint.

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use eframe::egui;
use serde::Deserialize;

use crate::esi::{EsiClient, Fetched};

/// ESI takes at most this many names per lookup.
const NAMES_PER_CALL: usize = 500;
/// Names asked for within this time go into one call.
const GATHER: Duration = Duration::from_millis(500);
/// Kills and jumps change once an hour.
const ACTIVITY_EVERY: Duration = Duration::from_secs(60 * 60);
/// After a failed fetch, try again this much later.
const RETRY_AFTER: Duration = Duration::from_secs(5 * 60);

/// What the module asks the thread to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    /// Find out which of these names are characters.
    Names(Vec<String>),
    /// Switch the hourly kills and jumps fetch on or off.
    Activity(bool),
}

/// Kills and jumps of one system in the last hour.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SystemActivity {
    pub jumps: u32,
    pub ship_kills: u32,
    pub pod_kills: u32,
    pub npc_kills: u32,
}

/// A character ESI found: its id, and its name as ESI spells it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Character {
    pub id: i64,
    pub name: String,
}

/// What the thread sends back.
#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    /// Each name asked about, with the character if it is one.
    Names(Vec<(String, Option<Character>)>),
    /// The lookup for these names failed; they may be asked about again.
    NotAnswered(Vec<String>),
    Activity(HashMap<i64, SystemActivity>),
    /// Something went wrong (`None`: it works again).
    Problem(Option<String>),
}

/// Handle to the thread. Dropping it stops the thread.
pub struct Lookup {
    send: Sender<Request>,
    pub answers: Receiver<Answer>,
}

impl Lookup {
    pub fn start(egui_ctx: egui::Context) -> Lookup {
        let (send, requests) = mpsc::channel();
        let (answer_tx, answers) = mpsc::channel();
        // Unit tests build the module many times and must not call ESI, so
        // there the requests just wait in the channel.
        if !cfg!(test) {
            std::thread::spawn(move || run(requests, answer_tx, egui_ctx));
        } else {
            std::mem::forget((requests, answer_tx));
        }
        Lookup { send, answers }
    }

    pub fn ask(&self, request: Request) {
        // If the thread is gone, there is nobody to ask.
        let _ = self.send.send(request);
    }
}

#[derive(Deserialize)]
struct IdsAnswer {
    #[serde(default)]
    characters: Vec<NamedId>,
}

#[derive(Deserialize)]
struct NamedId {
    id: i64,
    name: String,
}

#[derive(Deserialize)]
struct KillsRow {
    system_id: i64,
    ship_kills: u32,
    pod_kills: u32,
    npc_kills: u32,
}

#[derive(Deserialize)]
struct JumpsRow {
    system_id: i64,
    ship_jumps: u32,
}

/// The thread: waits for requests, gathers names for a moment so they go
/// out together, and fetches activity when it is due.
fn run(requests: Receiver<Request>, answers: Sender<Answer>, egui_ctx: egui::Context) {
    let mut client: Option<EsiClient> = None;
    let mut names: Vec<String> = Vec::new();
    let mut activity_on = false;
    let mut activity_due: Option<Instant> = None;
    let mut etags: (Option<String>, Option<String>) = (None, None);
    let mut activity: HashMap<i64, SystemActivity> = HashMap::new();
    let mut had_problem = false;

    loop {
        // Sleep until a request comes, or the next fetch is due.
        let wait = if !names.is_empty() {
            GATHER
        } else if let Some(due) = activity_due.filter(|_| activity_on) {
            due.saturating_duration_since(Instant::now())
        } else {
            Duration::from_secs(3600)
        };
        match requests.recv_timeout(wait) {
            Ok(Request::Names(new)) => {
                for name in new {
                    if !names.contains(&name) {
                        names.push(name);
                    }
                }
                // Gather more before calling.
                continue;
            }
            Ok(Request::Activity(on)) => {
                activity_on = on;
                if on && activity_due.is_none() {
                    activity_due = Some(Instant::now());
                }
                if !on {
                    activity_due = None;
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }

        let client = match &mut client {
            Some(client) => client,
            None => match EsiClient::new() {
                Ok(new) => client.insert(new),
                Err(e) => {
                    let _ = answers.send(Answer::Problem(Some(format!("ESI: {e:#}"))));
                    return;
                }
            },
        };

        let mut problem: Option<String> = None;
        if !names.is_empty() {
            for chunk in names.chunks(NAMES_PER_CALL) {
                match look_up(client, chunk) {
                    Ok(found) => {
                        let _ = answers.send(Answer::Names(found));
                    }
                    Err(e) => {
                        problem = Some(format!("ESI name lookup: {e:#}"));
                        let _ = answers.send(Answer::NotAnswered(chunk.to_vec()));
                    }
                }
            }
            names.clear();
        }
        if activity_on && activity_due.is_some_and(|due| due <= Instant::now()) {
            match fetch_activity(client, &mut etags, &mut activity) {
                Ok(()) => {
                    let _ = answers.send(Answer::Activity(activity.clone()));
                    activity_due = Some(Instant::now() + ACTIVITY_EVERY);
                }
                Err(e) => {
                    problem = Some(format!("ESI kills and jumps: {e:#}"));
                    activity_due = Some(Instant::now() + RETRY_AFTER);
                }
            }
        }
        if problem.is_some() || had_problem {
            had_problem = problem.is_some();
            let _ = answers.send(Answer::Problem(problem));
        }
        egui_ctx.request_repaint();
    }
}

/// One `POST /universe/ids` call. Names that aren't characters come back
/// with `None`.
fn look_up(
    client: &mut EsiClient,
    names: &[String],
) -> anyhow::Result<Vec<(String, Option<Character>)>> {
    let answer: IdsAnswer = client.post_json("/universe/ids", &names)?;
    Ok(match_names(names, answer.characters))
}

/// Pairs each asked name with the character ESI found for it (ESI's
/// spelling may differ in case).
fn match_names(names: &[String], found: Vec<NamedId>) -> Vec<(String, Option<Character>)> {
    names
        .iter()
        .map(|name| {
            let character = found
                .iter()
                .find(|c| c.name.eq_ignore_ascii_case(name))
                .map(|c| Character {
                    id: c.id,
                    name: c.name.clone(),
                });
            (name.clone(), character)
        })
        .collect()
}

/// Fetches kills and jumps, sending the last ETags so unchanged data costs
/// almost nothing. Keeps the old numbers for a list that didn't change.
fn fetch_activity(
    client: &mut EsiClient,
    etags: &mut (Option<String>, Option<String>),
    activity: &mut HashMap<i64, SystemActivity>,
) -> anyhow::Result<()> {
    let kills = client.get_json::<Vec<KillsRow>>("/universe/system_kills", etags.0.as_deref())?;
    let jumps = client.get_json::<Vec<JumpsRow>>("/universe/system_jumps", etags.1.as_deref())?;
    if let Fetched::Data { value, etag, .. } = kills {
        etags.0 = etag;
        for entry in activity.values_mut() {
            entry.ship_kills = 0;
            entry.pod_kills = 0;
            entry.npc_kills = 0;
        }
        for row in value {
            let entry = activity.entry(row.system_id).or_default();
            entry.ship_kills = row.ship_kills;
            entry.pod_kills = row.pod_kills;
            entry.npc_kills = row.npc_kills;
        }
    }
    if let Fetched::Data { value, etag, .. } = jumps {
        etags.1 = etag;
        for entry in activity.values_mut() {
            entry.jumps = 0;
        }
        for row in value {
            activity.entry(row.system_id).or_default().jumps = row.ship_jumps;
        }
    }
    activity.retain(|_, a| *a != SystemActivity::default());
    Ok(())
}

/// Which of a report's pilot-name guesses to show, with ESI's spelling:
/// the ones that are characters, longest first, leaving out a shorter
/// guess inside a longer one already shown ("Evil" inside "Evil Pilot").
///
/// Lots of ordinary words are also character names ("anyone", "around"),
/// so a one-word guess counts only when it is spelled exactly like the
/// character, upper and lower case included. Names dragged into chat
/// always are; a chatty lowercase word usually isn't.
pub fn pilots_to_show(
    candidates: &[String],
    known: &HashMap<String, Option<Character>>,
) -> Vec<(String, i64)> {
    let mut shown: Vec<(String, i64)> = Vec::new();
    // The parser lists longer guesses first within each run of words.
    for candidate in candidates {
        let Some(Some(character)) = known.get(&candidate.to_lowercase()) else {
            continue;
        };
        if !candidate.contains(' ') && *candidate != character.name {
            continue;
        }
        let inside_shown = shown.iter().any(|(name, _)| {
            name.to_lowercase()
                .split(' ')
                .collect::<Vec<_>>()
                .windows(candidate.split(' ').count())
                .any(|w| w.join(" ") == candidate.to_lowercase())
        });
        if !inside_shown {
            shown.push((character.name.clone(), character.id));
        }
    }
    shown
}

/// A pilot's zKillboard page.
pub fn zkillboard_url(character_id: i64) -> String {
    format!("https://zkillboard.com/character/{character_id}/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_matched_ignoring_case() {
        let names = vec!["evil pilot".to_string(), "anyone".to_string()];
        let found = vec![NamedId {
            id: 90000001,
            name: "Evil Pilot".into(),
        }];
        assert_eq!(
            match_names(&names, found),
            [
                (
                    "evil pilot".to_string(),
                    Some(Character {
                        id: 90000001,
                        name: "Evil Pilot".into()
                    })
                ),
                ("anyone".to_string(), None)
            ]
        );
    }

    #[test]
    fn shorter_guesses_inside_a_shown_name_are_left_out() {
        let candidates: Vec<String> = ["evil pilot", "Evil", "Pilot", "Scout", "anyone"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let character = |id: i64, name: &str| {
            Some(Character {
                id,
                name: name.to_string(),
            })
        };
        let known: HashMap<String, Option<Character>> = [
            ("evil pilot".to_string(), character(1, "Evil Pilot")),
            ("evil".to_string(), character(2, "Evil")),
            ("pilot".to_string(), None),
            ("scout".to_string(), character(3, "Scout")),
            ("anyone".to_string(), character(4, "Anyone")),
        ]
        .into_iter()
        .collect();
        // "evil pilot" shows with ESI's spelling; "Evil" is inside it;
        // "anyone" is a character, but written in lowercase.
        assert_eq!(
            pilots_to_show(&candidates, &known),
            [("Evil Pilot".to_string(), 1), ("Scout".to_string(), 3)]
        );
        // Unknown yet: nothing shown.
        assert!(pilots_to_show(&candidates, &HashMap::new()).is_empty());
    }

    #[test]
    fn zkillboard_links() {
        assert_eq!(zkillboard_url(42), "https://zkillboard.com/character/42/");
    }
}
