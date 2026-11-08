//! Watches the EVE `Chatlogs` folder on a background thread: finds the
//! channels there, follows the watched intel channels and every Local
//! channel, and sends new lines to the module.
//!
//! How it works (the same idea as the Mining Supervisor's game log watcher):
//! - Every [`LIST_EVERY`] it lists the folder. Each new file's header is
//!   read once to learn its channel and character; files are matched to a
//!   channel by the header, not by the file name.
//! - Every [`POLL_EVERY`] it checks the size of the files it follows and
//!   reads only the bytes that were added.
//! - It follows files changed since the last downtime (11:00 UTC). No
//!   client stays logged in across downtime, so older files are finished.
//!   Files changed in the last [`CHANNEL_LIST_DAYS`] days only add their
//!   channel to the list in Settings.
//! - Files are read from the start, so Local gives the system each pilot is
//!   in right away. Lines that were already there when the file was first
//!   seen are marked `catch_up`: they are shown if still recent, but never
//!   raise an alert.
//!
//! From Local only the "Channel changed to Local" lines are sent; from
//! intel channels every line.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use eframe::egui;

use super::chatlog::{ChatLine, ChatLogHeader, ChatLogReader, local_system_change};

/// How often new files are looked for.
const LIST_EVERY: Duration = Duration::from_secs(3);
/// How often followed files are checked for new lines. Intel should show
/// up fast; checking the size of a dozen files is cheap.
const POLL_EVERY: Duration = Duration::from_millis(500);
/// Files changed within this many days add their channel to the list.
const CHANNEL_LIST_DAYS: u64 = 14;
/// Read at most this much per file per poll.
const READ_CHUNK: usize = 1024 * 1024;
/// The header is in the first few hundred bytes (UTF-16, so twice the text).
const HEADER_BYTES: u64 = 4096;
/// The daily downtime starts at 11:00 UTC.
const DOWNTIME_SECONDS_OF_DAY: u64 = 11 * 3600;

/// The channel name of Local in the header.
pub const LOCAL: &str = "Local";

/// What the watcher thread sends.
#[derive(Debug, Clone, PartialEq)]
pub enum WatchEvent {
    /// The channels found in the folder, sorted, without Local. Sent at
    /// start and whenever a new one shows up.
    Channels(Vec<String>),
    /// New lines from one file. For Local only system changes.
    Lines {
        channel: String,
        listener: String,
        lines: Vec<ChatLine>,
        catch_up: bool,
    },
    /// The first pass over the existing files is done.
    CaughtUp,
    /// Something is wrong, e.g. the folder doesn't exist. Shown to the user.
    Problem(String),
}

/// Handle to the watcher thread. Dropping it stops the thread.
pub struct ChatLogWatcher {
    pub events: Receiver<WatchEvent>,
    /// Sends a new list of watched channels to the thread.
    watched: Sender<Vec<String>>,
}

impl ChatLogWatcher {
    /// Starts watching `<log_folder>/Chatlogs` for the `watched` channels
    /// (plus Local).
    pub fn start(log_folder: &Path, watched: Vec<String>, egui_ctx: egui::Context) -> Self {
        let folder = log_folder.join("Chatlogs");
        let (event_tx, events) = mpsc::channel();
        let (watched_tx, watched_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut watcher = Watcher::new(folder, watched, event_tx, egui_ctx);
            watcher.run(&watched_rx);
        });
        ChatLogWatcher {
            events,
            watched: watched_tx,
        }
    }

    /// Changes which channels are followed. Newly watched channels are
    /// followed from their current end.
    pub fn set_watched(&self, channels: Vec<String>) {
        // If the thread has stopped, there is nothing to tell.
        let _ = self.watched.send(channels);
    }
}

/// A chat log with a header; followed while its channel is watched.
struct SeenFile {
    path: PathBuf,
    header: ChatLogHeader,
    /// `None` while its channel isn't watched.
    followed: Option<Followed>,
    /// Changed since the last downtime.
    current: bool,
}

/// A file being followed.
struct Followed {
    reader: ChatLogReader,
    read_to: u64,
}

struct Watcher {
    folder: PathBuf,
    watched: HashSet<String>,
    /// By file name. `None`: too old to matter, or not a chat log; never
    /// looked at again.
    files: HashMap<String, Option<SeenFile>>,
    channels: BTreeSet<String>,
    send: Sender<WatchEvent>,
    egui_ctx: egui::Context,
    last_problem: Option<String>,
}

impl Watcher {
    fn new(
        folder: PathBuf,
        watched: Vec<String>,
        send: Sender<WatchEvent>,
        egui_ctx: egui::Context,
    ) -> Watcher {
        Watcher {
            folder,
            watched: watched.into_iter().collect(),
            files: HashMap::new(),
            channels: BTreeSet::new(),
            send,
            egui_ctx,
            last_problem: None,
        }
    }

    fn run(&mut self, watched: &Receiver<Vec<String>>) {
        self.list_folder(SystemTime::now());
        if !self.poll_files(true) || !self.send(WatchEvent::CaughtUp) {
            return;
        }
        let mut last_list = Instant::now();
        loop {
            // Sleep, but wake up at once for a new channel list or when
            // the module is dropped.
            match watched.recv_timeout(POLL_EVERY) {
                Ok(channels) => self.set_watched(channels),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            // More changes may be queued; the last one counts.
            loop {
                match watched.try_recv() {
                    Ok(channels) => self.set_watched(channels),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return,
                }
            }
            if last_list.elapsed() >= LIST_EVERY {
                self.list_folder(SystemTime::now());
                last_list = Instant::now();
            }
            if !self.poll_files(false) {
                return;
            }
        }
    }

    fn send(&self, event: WatchEvent) -> bool {
        let sent = self.send.send(event).is_ok();
        self.egui_ctx.request_repaint();
        sent
    }

    fn problem(&mut self, text: String) {
        if self.last_problem.as_ref() != Some(&text) {
            self.last_problem = Some(text.clone());
            self.send(WatchEvent::Problem(text));
        }
    }

    fn is_followed_channel(&self, channel: &str) -> bool {
        channel == LOCAL || self.watched.contains(channel)
    }

    /// A new list of watched channels: start following files of newly
    /// watched channels from their end, stop following the others.
    fn set_watched(&mut self, channels: Vec<String>) {
        self.watched = channels.into_iter().collect();
        let watched = self.watched.clone();
        for SeenFile {
            path,
            header,
            followed,
            current,
        } in self.files.values_mut().flatten()
        {
            let wanted = *current && (header.channel == LOCAL || watched.contains(&header.channel));
            if !wanted {
                *followed = None;
            } else if followed.is_none() {
                let size = std::fs::metadata(&*path).map(|m| m.len()).unwrap_or(0);
                *followed = Some(Followed {
                    reader: ChatLogReader::resume(header.clone()),
                    read_to: size,
                });
            }
        }
    }

    /// Finds files we haven't looked at yet.
    fn list_folder(&mut self, now: SystemTime) {
        let entries = match std::fs::read_dir(&self.folder) {
            Ok(entries) => entries,
            Err(e) => {
                let text = format!("Can't read {}: {e}", self.folder.display());
                self.problem(text);
                return;
            }
        };
        self.last_problem = None;
        let downtime = last_downtime(now);
        let list_since = now - Duration::from_secs(CHANNEL_LIST_DAYS * 86_400);
        let mut new_channels = false;
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.ends_with(".txt") || self.files.contains_key(&name) {
                continue;
            }
            // The listing's time can be older than the real one while EVE
            // has the file open, but never newer. So it's only used to skip
            // files that are clearly old.
            let listed = entry.metadata().ok().and_then(|m| m.modified().ok());
            if listed.is_some_and(|time| time < list_since) {
                self.files.insert(name, None);
                continue;
            }
            let path = entry.path();
            let Some(header) = read_header(&path) else {
                // No header yet: EVE has only just created the file. A file
                // older than the downtime will never get one.
                if listed.is_some_and(|time| time < downtime) {
                    self.files.insert(name, None);
                }
                continue;
            };
            // The real time, from the file itself.
            let modified = std::fs::metadata(&path)
                .and_then(|m| m.modified())
                .unwrap_or(now);
            let current = modified >= downtime;
            if header.channel != LOCAL && self.channels.insert(header.channel.clone()) {
                new_channels = true;
            }
            let followed =
                (current && self.is_followed_channel(&header.channel)).then(|| Followed {
                    reader: ChatLogReader::new(),
                    read_to: 0,
                });
            self.files.insert(
                name,
                Some(SeenFile {
                    path,
                    header,
                    followed,
                    current,
                }),
            );
        }
        if new_channels {
            self.send(WatchEvent::Channels(
                self.channels.iter().cloned().collect(),
            ));
        }
    }

    /// Reads what was added to each followed file. False if the module is gone.
    fn poll_files(&mut self, catch_up: bool) -> bool {
        let mut batches = Vec::new();
        for seen in self.files.values_mut().flatten() {
            let SeenFile {
                path,
                header,
                followed: Some(followed),
                ..
            } = seen
            else {
                continue;
            };
            // An error means the file can't be read right now; it is tried
            // again next time.
            while let Ok(Some((lines, more))) = read_new_bytes(path, followed) {
                // Only the first pass reads lines that were there before
                // Athena started. Files found later are new sessions, so
                // everything in them is live.
                let lines: Vec<ChatLine> = if header.channel == LOCAL {
                    lines
                        .into_iter()
                        .filter(|line| local_system_change(line).is_some())
                        .collect()
                } else {
                    lines
                };
                if !lines.is_empty() {
                    batches.push(WatchEvent::Lines {
                        channel: header.channel.clone(),
                        listener: header.listener.clone(),
                        lines,
                        catch_up,
                    });
                }
                if !more {
                    break;
                }
            }
        }
        for batch in batches {
            if !self.send(batch) {
                return false;
            }
        }
        true
    }
}

/// Reads up to [`READ_CHUNK`] new bytes and turns them into lines, plus
/// whether there is more. `Ok(None)` if nothing was added.
fn read_new_bytes(
    path: &Path,
    followed: &mut Followed,
) -> std::io::Result<Option<(Vec<ChatLine>, bool)>> {
    let size = std::fs::metadata(path)?.len();
    if size <= followed.read_to {
        return Ok(None);
    }
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(followed.read_to))?;
    let wanted = (size - followed.read_to).min(READ_CHUNK as u64) as usize;
    let mut buffer = vec![0; wanted];
    let got = file.read(&mut buffer)?;
    if got == 0 {
        return Ok(None);
    }
    buffer.truncate(got);
    followed.read_to += got as u64;
    let more = followed.read_to < size;
    Ok(Some((followed.reader.push(&buffer), more)))
}

/// Reads just the header of a chat log.
fn read_header(path: &Path) -> Option<ChatLogHeader> {
    let mut buffer = Vec::new();
    File::open(path)
        .ok()?
        .take(HEADER_BYTES)
        .read_to_end(&mut buffer)
        .ok()?;
    let mut reader = ChatLogReader::new();
    reader.push(&buffer);
    reader.header().cloned()
}

/// The start of the most recent daily downtime (11:00 UTC) before `now`.
pub fn last_downtime(now: SystemTime) -> SystemTime {
    let seconds = now
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let day_start = seconds - seconds % 86_400;
    let mut downtime = day_start + DOWNTIME_SECONDS_OF_DAY;
    if downtime > seconds {
        downtime = downtime.saturating_sub(86_400);
    }
    UNIX_EPOCH + Duration::from_secs(downtime)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::super::chatlog::tests::{sample_text, utf16};
    use super::*;

    fn temp_folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("athena-chat-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn watcher(folder: &Path, watched: &[&str]) -> (Watcher, Receiver<WatchEvent>) {
        let (send, receive) = mpsc::channel();
        let watched = watched.iter().map(|s| s.to_string()).collect();
        let watcher = Watcher::new(
            folder.to_path_buf(),
            watched,
            send,
            egui::Context::default(),
        );
        (watcher, receive)
    }

    fn write(dir: &Path, file: &str, channel: &str, listener: &str, body: &str) -> PathBuf {
        let path = dir.join(file);
        std::fs::write(&path, utf16(&sample_text(channel, listener, body))).unwrap();
        path
    }

    const INTEL: &str = "[ 2026.11.08 14:01:02 ] Scout > X7R-JW loki\r\n";
    const JUMP: &str = "[ 2026.11.08 14:01:30 ] EVE System > Channel changed to Local : KV-8SN\r\n[ 2026.11.08 14:01:31 ] Someone > hi\r\n";

    #[test]
    fn lists_channels_and_follows_watched_ones() {
        let dir = temp_folder("follow");
        write(
            &dir,
            "Branch Intel_1.txt",
            "Branch Intel",
            "Pilot Alpha",
            INTEL,
        );
        write(&dir, "Corp_1.txt", "Corp", "Pilot Alpha", INTEL);
        write(&dir, "Local_1.txt", "Local", "Pilot Alpha", JUMP);

        let (mut w, receive) = watcher(&dir, &["Branch Intel"]);
        w.list_folder(SystemTime::now());
        w.poll_files(true);
        let events: Vec<WatchEvent> = receive.try_iter().collect();
        assert!(events.contains(&WatchEvent::Channels(vec![
            "Branch Intel".to_string(),
            "Corp".to_string()
        ])));
        let lines: Vec<(&str, usize, bool)> = events
            .iter()
            .filter_map(|e| match e {
                WatchEvent::Lines {
                    channel,
                    lines,
                    catch_up,
                    ..
                } => Some((channel.as_str(), lines.len(), *catch_up)),
                _ => None,
            })
            .collect();
        // Corp isn't watched; Local only sends the system change.
        assert_eq!(lines.len(), 2);
        assert!(lines.contains(&("Branch Intel", 1, true)));
        assert!(lines.contains(&("Local", 1, true)));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn appended_lines_are_live_and_newly_watched_start_at_the_end() {
        let dir = temp_folder("append");
        let intel = write(
            &dir,
            "Branch Intel_1.txt",
            "Branch Intel",
            "Pilot Alpha",
            INTEL,
        );
        let corp = write(&dir, "Corp_1.txt", "Corp", "Pilot Alpha", INTEL);
        let (mut w, receive) = watcher(&dir, &["Branch Intel"]);
        w.list_folder(SystemTime::now());
        w.poll_files(true);
        receive.try_iter().count();

        let append = |path: &Path| {
            let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
            file.write_all(&utf16(INTEL)).unwrap();
        };
        append(&intel);
        w.set_watched(vec!["Branch Intel".into(), "Corp".into()]);
        w.poll_files(false);
        let live: Vec<WatchEvent> = receive.try_iter().collect();
        assert_eq!(live.len(), 1);
        assert!(
            matches!(&live[0], WatchEvent::Lines { channel, catch_up: false, .. } if channel == "Branch Intel")
        );

        // Corp is followed from its end: only what's added now.
        append(&corp);
        w.poll_files(false);
        let live: Vec<WatchEvent> = receive.try_iter().collect();
        assert_eq!(live.len(), 1);
        assert!(
            matches!(&live[0], WatchEvent::Lines { channel, lines, .. } if channel == "Corp" && lines.len() == 1)
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_folder_is_reported_once() {
        let dir = temp_folder("missing");
        let (mut w, receive) = watcher(&dir.join("nope"), &[]);
        w.list_folder(SystemTime::now());
        w.list_folder(SystemTime::now());
        let problems = receive
            .try_iter()
            .filter(|e| matches!(e, WatchEvent::Problem(_)))
            .count();
        assert_eq!(problems, 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn downtime_is_11_utc() {
        let at = |s: u64| UNIX_EPOCH + Duration::from_secs(s);
        let day = 20_400 * 86_400;
        assert_eq!(last_downtime(at(day + 12 * 3600)), at(day + 11 * 3600));
        assert_eq!(last_downtime(at(day + 11 * 3600)), at(day + 11 * 3600));
        // Before 11:00: yesterday's.
        assert_eq!(last_downtime(at(day + 3600)), at(day - 13 * 3600));
    }
}
