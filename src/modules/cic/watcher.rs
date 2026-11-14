//! Follows the EVE `Gamelogs` folder on a background thread and sends the
//! combat lines of the characters the Logi view shows.
//!
//! Unlike the Mining watcher it never reads old lines: the Logi view is
//! about what happens now. Files that are already there when it starts are
//! followed from their end; files that appear later (a character logged
//! in) are read from the start.
//!
//! Every [`LIST_EVERY`] it lists the folder and reads the header of new
//! files, to know whose they are; every character found is sent to the
//! module for the Settings list. Every "Check log files" interval (Settings,
//! read through [`crate::settings::file_poll_interval`]) it checks the size
//! of the files of the shown characters only, and reads what was added.
//! Files of other characters are not read at all.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant, SystemTime};

use eframe::egui;

use super::combatlog::{CombatEvent, parse_combat_line};
use crate::modules::mining::gamelog::GameLogReader;

/// How often new files are looked for.
const LIST_EVERY: Duration = Duration::from_secs(3);
/// Files changed longer ago than this are not followed (no log stays open
/// across the daily downtime).
const FOLLOW_HOURS: u64 = 24;
/// Read at most this much per file per poll.
const READ_CHUNK: usize = 256 * 1024;

/// What the watcher thread sends.
#[derive(Debug, Clone)]
pub enum WatchEvent {
    /// New combat events from one character's log.
    Events {
        character: String,
        events: Vec<CombatEvent>,
    },
    /// Every character with a recent game log, sorted.
    Characters(Vec<String>),
    /// Something is wrong, e.g. the folder doesn't exist.
    Problem(String),
}

/// What the module tells the watcher.
enum Command {
    /// Read the logs of these characters only.
    Follow(HashSet<String>),
}

/// Handle to the watcher thread. Dropping it stops the thread.
pub struct CombatLogWatcher {
    pub events: Receiver<WatchEvent>,
    commands: Sender<Command>,
}

impl CombatLogWatcher {
    /// Starts watching `<log_folder>/Gamelogs` for the `follow` characters.
    pub fn start(log_folder: &Path, follow: HashSet<String>, egui_ctx: egui::Context) -> Self {
        let folder = log_folder.join("Gamelogs");
        let (event_tx, events) = mpsc::channel();
        let (commands, command_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut watcher = Watcher {
                folder,
                follow,
                files: HashMap::new(),
                characters: BTreeSet::new(),
                send: event_tx,
                egui_ctx,
                last_problem: None,
            };
            watcher.run(&command_rx);
        });
        CombatLogWatcher { events, commands }
    }

    /// Changes which characters' logs are read.
    pub fn follow(&self, characters: HashSet<String>) {
        let _ = self.commands.send(Command::Follow(characters));
    }
}

/// A file being followed.
struct Followed {
    path: PathBuf,
    reader: GameLogReader,
    /// Bytes read from the file so far (including a buffered partial line).
    read_to: u64,
    /// False while its character isn't shown: the file is then not read,
    /// and reading starts at its end once the character is shown.
    active: bool,
}

impl Followed {
    fn character(&self) -> Option<&str> {
        self.reader.header().map(|h| h.character.as_str())
    }
}

struct Watcher {
    folder: PathBuf,
    follow: HashSet<String>,
    /// Followed files by file name.
    files: HashMap<String, Followed>,
    /// Every character seen in a header.
    characters: BTreeSet<String>,
    send: Sender<WatchEvent>,
    egui_ctx: egui::Context,
    last_problem: Option<String>,
}

impl Watcher {
    fn run(&mut self, commands: &Receiver<Command>) {
        self.list_folder(true);
        let mut last_list = Instant::now();
        loop {
            // Sleep, but wake up at once for a command, and stop when the
            // module is dropped (the channel is then disconnected).
            match commands.recv_timeout(crate::settings::file_poll_interval()) {
                Ok(Command::Follow(follow)) => {
                    self.follow = follow;
                    self.update_active();
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            if last_list.elapsed() >= LIST_EVERY {
                self.list_folder(false);
                last_list = Instant::now();
            }
            if !self.poll_files() {
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

    /// Finds log files we don't follow yet and reads their headers.
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
            // The listing's time can be stale on Windows while EVE keeps
            // the file open, but never newer than the real one, so it is
            // good for skipping old files. See the Mining watcher.
            let Ok(listed) = entry.metadata() else {
                continue;
            };
            if age_in_hours(&listed, now) >= FOLLOW_HOURS {
                continue;
            }
            let path = entry.path();
            let Ok(metadata) = std::fs::metadata(&path) else {
                continue;
            };
            if age_in_hours(&metadata, now) >= FOLLOW_HOURS {
                continue;
            }
            let followed = if first_pass {
                // Already there: only what gets added from now on.
                match start_at_end(&path, metadata.len()) {
                    Some(followed) => followed,
                    None => continue,
                }
            } else {
                // New: read it from the start (its header first).
                Followed {
                    path,
                    reader: GameLogReader::new(),
                    read_to: 0,
                    active: true,
                }
            };
            self.files.insert(name, followed);
        }
        self.last_problem = None;
        self.update_active();
    }

    /// Marks the files of shown characters active, and reports characters.
    fn update_active(&mut self) {
        let mut found = false;
        for followed in self.files.values_mut() {
            let Some(character) = followed.character().map(str::to_string) else {
                // No header yet: keep reading until there is one.
                continue;
            };
            found |= self.characters.insert(character.clone());
            let shown = self.follow.contains(&character);
            if shown && !followed.active {
                // Start at the end: what happened while hidden is old news.
                if let Ok(metadata) = std::fs::metadata(&followed.path) {
                    followed.read_to = metadata.len();
                    followed.reader = GameLogReader::resume(&character);
                }
            }
            followed.active = shown;
        }
        if found {
            let characters = self.characters.iter().cloned().collect();
            self.send(WatchEvent::Characters(characters));
        }
    }

    /// Reads what was added to each active file. False if the module is gone.
    fn poll_files(&mut self) -> bool {
        let mut batches = Vec::new();
        let mut new_header = false;
        let follow = &self.follow;
        for followed in self.files.values_mut() {
            if !followed.active {
                continue;
            }
            let had_header = followed.character().is_some();
            // An error means the file went away or can't be read right
            // now; it is tried again next time.
            while let Ok(Some((lines, more))) = read_new_bytes(followed) {
                let Some(character) = followed.character().map(str::to_string) else {
                    break;
                };
                new_header |= !had_header;
                if !follow.contains(&character) {
                    // A new file of a character who isn't shown.
                    followed.active = false;
                    break;
                }
                let events: Vec<CombatEvent> = lines
                    .iter()
                    .filter_map(|line| parse_combat_line(line, &character))
                    .collect();
                if !events.is_empty() {
                    batches.push(WatchEvent::Events { character, events });
                }
                if !more {
                    break;
                }
            }
        }
        if new_header {
            self.update_active();
        }
        for batch in batches {
            if !self.send(batch) {
                return false;
            }
        }
        true
    }
}

/// Reads up to [`READ_CHUNK`] new bytes and returns the lines they
/// completed, plus whether there is more to read. `Ok(None)` if nothing
/// was added.
fn read_new_bytes(followed: &mut Followed) -> std::io::Result<Option<(Vec<String>, bool)>> {
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
    Ok(Some((followed.reader.push_lines(&buffer), more)))
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

/// Follows a file from its current end. The header is read first, so we
/// know whose file it is. Starts inactive; `update_active` turns it on.
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
    probe.push_lines(&buffer);
    let character = probe.header()?.character.clone();
    Some(Followed {
        path: path.to_path_buf(),
        reader: GameLogReader::resume(&character),
        read_to: size,
        active: false,
    })
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    const HEADER: &str = "------------------------------------------------------------\r\n  Gamelog\r\n  Listener: Test Pilot\r\n  Session Started: 2026.11.08 07:30:12\r\n------------------------------------------------------------\r\n";
    const HIT: &str = "[ 2026.11.08 07:31:10 ] (combat) <color=0xffcc0000><b>36</b> <color=0x77ffffff><font size=10>from</font> <b><color=0xffffffff>Guristas Eraser</b><font size=10><color=0x77ffffff> - Penetrates\r\n";

    fn temp_folder(name: &str) -> PathBuf {
        let folder = std::env::temp_dir()
            .join(format!("athena-cic-{name}-{}", std::process::id()))
            .join("Gamelogs");
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        folder
    }

    fn watcher(folder: &Path, follow: &[&str]) -> (Watcher, Receiver<WatchEvent>) {
        let (send, receive) = mpsc::channel();
        let watcher = Watcher {
            folder: folder.to_path_buf(),
            follow: follow.iter().map(|s| s.to_string()).collect(),
            files: HashMap::new(),
            characters: BTreeSet::new(),
            send,
            egui_ctx: egui::Context::default(),
            last_problem: None,
        };
        (watcher, receive)
    }

    fn append(path: &Path, text: &str) {
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(path)
            .unwrap();
        file.write_all(text.as_bytes()).unwrap();
    }

    fn hits(receive: &Receiver<WatchEvent>) -> usize {
        receive
            .try_iter()
            .map(|event| match event {
                WatchEvent::Events { events, .. } => events.len(),
                _ => 0,
            })
            .sum()
    }

    #[test]
    fn follows_shown_characters_from_the_end() {
        let folder = temp_folder("follow");
        let path = folder.join("20261108_073012_1.txt");
        append(&path, &format!("{HEADER}{HIT}"));
        let (mut watcher, receive) = watcher(&folder, &["Test Pilot"]);
        watcher.list_folder(true);
        assert!(matches!(
            receive.try_recv(),
            Ok(WatchEvent::Characters(c)) if c == ["Test Pilot"]
        ));
        // The old hit isn't sent.
        watcher.poll_files();
        assert_eq!(hits(&receive), 0);
        append(&path, HIT);
        watcher.poll_files();
        assert_eq!(hits(&receive), 1);
        let _ = std::fs::remove_dir_all(folder.parent().unwrap());
    }

    #[test]
    fn hidden_characters_are_not_read() {
        let folder = temp_folder("hidden");
        let path = folder.join("20261108_073012_2.txt");
        append(&path, HEADER);
        let (mut watcher, receive) = watcher(&folder, &[]);
        watcher.list_folder(true);
        append(&path, HIT);
        watcher.poll_files();
        assert_eq!(hits(&receive), 0);
        // Shown now: starts at the end, so the earlier hit stays unread.
        watcher.follow = ["Test Pilot".to_string()].into_iter().collect();
        watcher.update_active();
        watcher.poll_files();
        assert_eq!(hits(&receive), 0);
        append(&path, HIT);
        watcher.poll_files();
        assert_eq!(hits(&receive), 1);
        let _ = std::fs::remove_dir_all(folder.parent().unwrap());
    }

    #[test]
    fn new_file_is_read_from_the_start() {
        let folder = temp_folder("new");
        let (mut watcher, receive) = watcher(&folder, &["Test Pilot"]);
        watcher.list_folder(true);
        let path = folder.join("20261108_073012_3.txt");
        append(&path, &format!("{HEADER}{HIT}"));
        watcher.list_folder(false);
        watcher.poll_files();
        let events: Vec<WatchEvent> = receive.try_iter().collect();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, WatchEvent::Characters(c) if c == &["Test Pilot"]))
        );
        let count: usize = events
            .iter()
            .map(|e| match e {
                WatchEvent::Events { events, .. } => events.len(),
                _ => 0,
            })
            .sum();
        assert_eq!(count, 1);
        let _ = std::fs::remove_dir_all(folder.parent().unwrap());
    }
}
