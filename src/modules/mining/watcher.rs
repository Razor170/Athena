//! Watches the EVE `Gamelogs` folder on a background thread and sends the
//! mining events it finds to the module.
//!
//! How it works:
//! - Every [`LIST_EVERY`] it lists the folder to find new log files (a new
//!   file starts each time a character logs in).
//! - Every [`POLL_EVERY`] it checks the size of the files it follows and
//!   reads only the bytes that were added. Windows doesn't reliably report
//!   changes to a file that another program keeps open, so we look ourselves;
//!   a size check is one cheap system call per file.
//! - It follows files changed in the last [`FOLLOW_HOURS`] hours. Files the
//!   module already read before (it saves where it stopped, see
//!   [`FileOffset`]) continue from there. Other files changed in the last
//!   [`CATCH_UP_HOURS`] hours are read from the start ("catch-up"), so mining
//!   from before Athena was started still counts. Older ones are only
//!   followed from their current end.
//!
//! Every batch of events carries the file offset it ends at. The module
//! saves that offset together with the state the events produced, so after
//! a restart nothing is counted twice and nothing is missed.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant, SystemTime};

use eframe::egui;
use serde::{Deserialize, Serialize};

use super::gamelog::{GameLogReader, TimedMiningEvent};

/// How often new files are looked for.
const LIST_EVERY: Duration = Duration::from_secs(3);
/// How often followed files are checked for new lines.
const POLL_EVERY: Duration = Duration::from_millis(500);
/// Files changed longer ago than this are not followed at all.
const FOLLOW_HOURS: u64 = 48;
/// Files we haven't seen before are read from the start if they changed
/// within this many hours (and followed from their end if older).
const CATCH_UP_HOURS: u64 = 12;
/// Read at most this much per file per poll, so a huge file can't hold up
/// the others (or use a lot of memory at once).
const READ_CHUNK: usize = 1024 * 1024;

/// Where reading of one file stopped. Saved by the module.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileOffset {
    /// The file name only, e.g. `20261108_073012_123456789.txt`.
    pub file: String,
    pub character: String,
    /// Bytes from the start of the file up to the end of the last complete
    /// line that was handled.
    pub offset: u64,
}

/// What the watcher thread sends.
#[derive(Debug, Clone)]
pub enum WatchEvent {
    /// New events from one file, and the offset they end at. `catch_up` is
    /// true for lines that were already in the file when Athena started;
    /// don't raise alerts for those.
    Events {
        position: FileOffset,
        events: Vec<TimedMiningEvent>,
        catch_up: bool,
    },
    /// The first pass over the existing files is done.
    CaughtUp,
    /// Something is wrong, e.g. the folder doesn't exist. Shown to the user.
    Problem(String),
}

/// Handle to the watcher thread. Dropping it stops the thread.
pub struct GameLogWatcher {
    pub events: Receiver<WatchEvent>,
    _stop: Sender<()>,
}

impl GameLogWatcher {
    /// Starts watching `<log_folder>/Gamelogs`. `known` are the offsets the
    /// module saved last time.
    pub fn start(log_folder: &Path, known: Vec<FileOffset>, egui_ctx: egui::Context) -> Self {
        let folder = log_folder.join("Gamelogs");
        let (event_tx, events) = mpsc::channel();
        let (_stop, stop_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut watcher = Watcher {
                folder,
                known: known.into_iter().map(|k| (k.file.clone(), k)).collect(),
                files: HashMap::new(),
                send: event_tx,
                egui_ctx,
                last_problem: None,
            };
            watcher.run(&stop_rx);
        });
        GameLogWatcher { events, _stop }
    }
}

/// A file being followed.
struct Followed {
    path: PathBuf,
    reader: GameLogReader,
    /// Bytes read from the file so far (including a buffered partial line).
    read_to: u64,
}

/// The state of the watcher thread.
struct Watcher {
    folder: PathBuf,
    /// Offsets saved by the module, by file name. Used when a file is first seen.
    known: HashMap<String, FileOffset>,
    /// Followed files by file name.
    files: HashMap<String, Followed>,
    send: Sender<WatchEvent>,
    egui_ctx: egui::Context,
    /// The last problem sent, so the same one isn't sent every few seconds.
    last_problem: Option<String>,
}

impl Watcher {
    fn run(&mut self, stop: &Receiver<()>) {
        // First pass: everything already in the files counts as catch-up.
        self.list_folder(true);
        self.poll_files(true);
        if !self.send(WatchEvent::CaughtUp) {
            return;
        }

        let mut last_list = Instant::now();
        loop {
            // Sleep, but wake up at once when the module is dropped.
            match stop.recv_timeout(POLL_EVERY) {
                Err(RecvTimeoutError::Timeout) => {}
                _ => return,
            }
            if last_list.elapsed() >= LIST_EVERY {
                self.list_folder(false);
                last_list = Instant::now();
            }
            if !self.poll_files(false) {
                return;
            }
        }
    }

    /// Sends to the module and wakes the UI. False if the module is gone.
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

    /// Finds log files we don't follow yet.
    fn list_folder(&mut self, first_pass: bool) {
        let entries = match std::fs::read_dir(&self.folder) {
            Ok(entries) => entries,
            Err(e) => {
                let text = format!("Can't read {}: {e}", self.folder.display());
                self.problem(text);
                return;
            }
        };
        let now = SystemTime::now();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.ends_with(".txt") || self.files.contains_key(&name) {
                continue;
            }
            // On Windows the directory listing's size and time are only
            // updated when the writer closes the file, and EVE keeps its log
            // open all session. So the listing is only good for skipping
            // files that are clearly old (a stale time is never newer than
            // the real one, and no log stays open across daily downtime).
            let Ok(listed) = entry.metadata() else {
                continue;
            };
            if age_in_hours(&listed, now) >= FOLLOW_HOURS {
                continue;
            }
            // For the files that remain, ask the file itself for the real
            // size and time.
            let path = entry.path();
            let Ok(metadata) = std::fs::metadata(&path) else {
                continue;
            };
            let age_hours = age_in_hours(&metadata, now);
            if age_hours >= FOLLOW_HOURS {
                continue;
            }
            let size = metadata.len();
            let followed = match self.known.get(&name) {
                // Read before: continue where we stopped. A file that got
                // smaller than that isn't the same file any more.
                Some(known) if known.offset <= size => Followed {
                    path,
                    reader: GameLogReader::resume(&known.character),
                    read_to: known.offset,
                },
                // New since the last run, or recent enough to catch up on.
                _ if !first_pass || age_hours < CATCH_UP_HOURS => Followed {
                    path,
                    reader: GameLogReader::new(),
                    read_to: 0,
                },
                // Old: only what gets added from now on.
                _ => match start_at_end(&path, size) {
                    Some(followed) => followed,
                    None => continue,
                },
            };
            self.files.insert(name, followed);
        }
        self.last_problem = None;
    }

    /// Reads what was added to each followed file. False if the module is gone.
    fn poll_files(&mut self, catch_up: bool) -> bool {
        let mut batches = Vec::new();
        for (name, followed) in &mut self.files {
            // Keep reading in chunks until the end, so catch-up of a big
            // file finishes in one pass.
            // An error means the file went away or can't be read right
            // now; it is tried again next time.
            while let Ok(Some((events, more))) = read_new_bytes(followed) {
                // No header yet: the client has only just created the file.
                let Some(header) = followed.reader.header() else {
                    break;
                };
                if !events.is_empty() {
                    let offset = followed.read_to - followed.reader.buffered_len() as u64;
                    batches.push(WatchEvent::Events {
                        position: FileOffset {
                            file: name.clone(),
                            character: header.character.clone(),
                            offset,
                        },
                        events,
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

/// Reads up to [`READ_CHUNK`] new bytes and turns them into events, plus
/// whether there is more to read. `Ok(None)` if nothing was added.
fn read_new_bytes(
    followed: &mut Followed,
) -> std::io::Result<Option<(Vec<TimedMiningEvent>, bool)>> {
    let size = std::fs::metadata(&followed.path)?.len();
    if size <= followed.read_to {
        return Ok(None);
    }
    let mut file = File::open(&followed.path)?;
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

/// Hours since the file was last changed (0 if unknown).
fn age_in_hours(metadata: &std::fs::Metadata, now: SystemTime) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|modified| now.duration_since(modified).ok())
        .map(|age| age.as_secs() / 3600)
        .unwrap_or(0)
}

/// Follows an old file from its current end. The header is read first, so
/// we know whose file it is.
fn start_at_end(path: &Path, size: u64) -> Option<Followed> {
    const HEADER_BYTES: u64 = 2048;
    let mut buffer = Vec::new();
    File::open(path)
        .ok()?
        .take(HEADER_BYTES)
        .read_to_end(&mut buffer)
        .ok()?;
    let mut probe = GameLogReader::new();
    // Make sure the last header line is complete, even in a short file.
    buffer.push(b'\n');
    probe.push(&buffer);
    let character = probe.header()?.character.clone();
    Some(Followed {
        path: path.to_path_buf(),
        reader: GameLogReader::resume(&character),
        read_to: size,
    })
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    const HEADER: &str = "------------------------------------------------------------\r\n  Gamelog\r\n  Listener: Test Pilot\r\n  Session Started: 2026.11.08 07:30:12\r\n------------------------------------------------------------\r\n";
    const MINED: &str = "[ 2026.11.08 07:31:10 ] (mining) You mined 100 units of Veldspar\r\n";

    fn temp_folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("athena-watch-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Gamelogs")).unwrap();
        dir
    }

    fn watcher(folder: &Path, known: Vec<FileOffset>) -> (Watcher, Receiver<WatchEvent>) {
        let (send, receive) = mpsc::channel();
        let watcher = Watcher {
            folder: folder.join("Gamelogs"),
            known: known.into_iter().map(|k| (k.file.clone(), k)).collect(),
            files: HashMap::new(),
            send,
            egui_ctx: egui::Context::default(),
            last_problem: None,
        };
        (watcher, receive)
    }

    fn mined_units(receive: &Receiver<WatchEvent>) -> Vec<(u64, bool)> {
        let mut out = Vec::new();
        while let Ok(event) = receive.try_recv() {
            if let WatchEvent::Events {
                position,
                events,
                catch_up,
            } = event
            {
                assert_eq!(position.character, "Test Pilot");
                out.push((position.offset, catch_up));
                assert!(!events.is_empty());
            }
        }
        out
    }

    #[test]
    fn catches_up_then_follows_appended_lines() {
        let dir = temp_folder("follow");
        let path = dir.join("Gamelogs").join("20261108_073012_1.txt");
        std::fs::write(&path, format!("{HEADER}{MINED}")).unwrap();

        let (mut w, receive) = watcher(&dir, Vec::new());
        w.list_folder(true);
        w.poll_files(true);
        let first_end = (HEADER.len() + MINED.len()) as u64;
        assert_eq!(mined_units(&receive), vec![(first_end, true)]);

        // EVE appends a line, the last part not finished yet.
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        file.write_all(MINED.as_bytes()).unwrap();
        file.write_all(b"[ 2026.11.08 07:32").unwrap();
        w.poll_files(false);
        // The offset stops at the end of the last complete line.
        assert_eq!(
            mined_units(&receive),
            vec![(first_end + MINED.len() as u64, false)]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn resumes_from_saved_offset() {
        let dir = temp_folder("resume");
        let name = "20261108_073012_1.txt";
        let path = dir.join("Gamelogs").join(name);
        std::fs::write(&path, format!("{HEADER}{MINED}{MINED}")).unwrap();
        let saved = FileOffset {
            file: name.to_string(),
            character: "Test Pilot".to_string(),
            offset: (HEADER.len() + MINED.len()) as u64,
        };

        let (mut w, receive) = watcher(&dir, vec![saved]);
        w.list_folder(true);
        w.poll_files(true);
        // Only the second line is new.
        let end = (HEADER.len() + 2 * MINED.len()) as u64;
        assert_eq!(mined_units(&receive), vec![(end, true)]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_folder_is_reported_once() {
        let dir = temp_folder("missing");
        let (mut w, receive) = watcher(&dir.join("nope"), Vec::new());
        w.list_folder(true);
        w.list_folder(false);
        let problems = receive
            .try_iter()
            .filter(|e| matches!(e, WatchEvent::Problem(_)))
            .count();
        assert_eq!(problems, 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn old_file_is_followed_from_its_end() {
        let dir = temp_folder("old");
        let path = dir.join("Gamelogs").join("x.txt");
        std::fs::write(&path, format!("{HEADER}{MINED}")).unwrap();
        let size = std::fs::metadata(&path).unwrap().len();
        let followed = start_at_end(&path, size).unwrap();
        assert_eq!(followed.read_to, size);
        assert_eq!(followed.reader.header().unwrap().character, "Test Pilot");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
