//! Where Athena keeps its files, and how it writes them.
//!
//! Everything goes into one data folder: `data` next to the exe (portable), or
//! `%APPDATA%\Athena` when the exe's folder isn't writable (for example under
//! Program Files). Tokens never go here; they live in Windows Credential Manager.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

use anyhow::{Context, Result, bail};

/// The folder Athena writes its settings, layout and cache to.
#[derive(Debug, Clone)]
pub struct DataDir {
    pub path: PathBuf,
    /// True when we had to fall back to `%APPDATA%\Athena`. Shown in settings.
    pub is_fallback: bool,
}

impl DataDir {
    /// Picks the data folder: `<exe folder>\data` if writable, else `%APPDATA%\Athena`.
    pub fn resolve() -> Result<DataDir> {
        let exe = std::env::current_exe().context("can't find the exe's location")?;
        let exe_dir = exe.parent().context("the exe has no parent folder")?;
        let portable = exe_dir.join("data");
        if is_writable_dir(&portable) {
            return Ok(DataDir {
                path: portable,
                is_fallback: false,
            });
        }

        // `config_dir` is the roaming AppData folder on Windows (%APPDATA%).
        let base = directories::BaseDirs::new().context("can't find the AppData folder")?;
        let fallback = base.config_dir().join("Athena");
        if is_writable_dir(&fallback) {
            return Ok(DataDir {
                path: fallback,
                is_fallback: true,
            });
        }

        bail!(
            "neither {} nor {} is writable",
            portable.display(),
            fallback.display()
        )
    }
}

/// Creates `dir` if needed and checks that we can write a file into it.
fn is_writable_dir(dir: &Path) -> bool {
    if fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(".write-test");
    let ok = fs::write(&probe, b"ok").is_ok();
    let _ = fs::remove_file(&probe);
    ok
}

/// Writes `contents` to `path` without ever leaving a half-written file:
/// it writes a temporary file next to it first, then renames it over the old one.
pub fn write_atomic(path: &Path, contents: &str) -> Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, contents).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

/// One file to write: where, and what.
struct SaveJob {
    path: PathBuf,
    contents: String,
}

/// Writes files on a background thread so the UI thread never waits on the disk.
///
/// Jobs are written one after another in the order they were sent, so a newer
/// save can never be overwritten by an older one.
pub struct Saver {
    // `Option` so `finish` can drop the sender, which tells the thread to stop.
    sender: Option<mpsc::Sender<SaveJob>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Saver {
    pub fn start() -> Saver {
        let (sender, receiver) = mpsc::channel::<SaveJob>();
        // `move` hands the receiver over to the new thread. The loop ends when
        // every sender is dropped (see `finish`).
        let thread = thread::spawn(move || {
            for job in receiver {
                // There is no UI to report to from here. A failed save keeps
                // the previous file, which is the best we can do.
                let _ = write_atomic(&job.path, &job.contents);
            }
        });
        Saver {
            sender: Some(sender),
            thread: Some(thread),
        }
    }

    /// Queues a file to be written in the background.
    pub fn save(&self, path: PathBuf, contents: String) {
        if let Some(sender) = &self.sender {
            let _ = sender.send(SaveJob { path, contents });
        }
    }

    /// Writes everything still queued, then stops the thread. Called on exit.
    pub fn finish(&mut self) {
        self.sender = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("athena-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn writable_dir_is_created() {
        let dir = temp_dir("writable");
        assert!(is_writable_dir(&dir));
        assert!(dir.is_dir());
        assert!(!dir.join(".write-test").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn saver_writes_in_order() {
        let dir = temp_dir("saver");
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("settings.toml");

        let mut saver = Saver::start();
        saver.save(file.clone(), "first".to_string());
        saver.save(file.clone(), "second".to_string());
        saver.finish();

        assert_eq!(fs::read_to_string(&file).unwrap(), "second");
        assert!(!file.with_extension("tmp").exists());
        fs::remove_dir_all(&dir).unwrap();
    }
}
