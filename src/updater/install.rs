//! Unpacking a downloaded update and swapping it in.
//!
//! Windows won't let a program overwrite its own exe while it runs, but it
//! does let it *rename* it. So the swap happens when Athena starts, before
//! it opens any other file:
//!
//! 1. The download is unpacked into `data\update\staged\` ([`stage`]).
//! 2. "Restart now" starts Athena again with `--wait-for-exit <pid>` and
//!    closes. The new process waits until the old one is gone, so no file
//!    is still open ([`restart_to_install`]).
//! 3. At startup ([`on_startup`]) each file from the zip is swapped in: the
//!    old file moves to `data\update\previous\`, the new one takes its place.
//!    Then the new `Athena.exe` is started and this process ends.
//! 4. The new version counts its start in `installed.toml`. Once its window
//!    has drawn ([`confirm_started`]), the old files are deleted. If it
//!    never gets that far, its next start puts the old files back.
//!
//! Only files that are in the zip are touched, and a zip with a `data`
//! folder is refused, so settings, layout and history are never changed.
//!
//! Keep this compatible: the install is done by the *old* exe, so the zip
//! layout (`Athena\...`), `staged.toml`, `installed.toml` and the
//! `--wait-for-exit <pid>` argument must keep working for every older
//! version that might install a newer one.

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// The exe's name inside the zip.
const EXE_NAME: &str = "Athena.exe";
/// The folder inside the zip that holds everything.
const ZIP_TOP_FOLDER: &str = "Athena";

const STAGED_DIR: &str = "staged";
const STAGED_INFO: &str = "staged.toml";
const PREVIOUS_DIR: &str = "previous";
const FAILED_DIR: &str = "failed";
const INSTALLED_INFO: &str = "installed.toml";
/// A message for the user that survives a restart, shown once in Settings.
const NOTE_FILE: &str = "last-update.txt";
/// The version that was rolled back, so the startup check doesn't offer it again.
const SKIP_FILE: &str = "skip-version.txt";
/// The folder next to the exe that holds the user's data; never part of an update.
const DATA_FOLDER: &str = "data";
/// Where the zip is downloaded to.
pub const DOWNLOAD_FILE: &str = "download.zip";

/// The updater's folder inside the data folder.
pub fn update_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("update")
}

/// Written next to the unpacked files once they are complete.
#[derive(Serialize, Deserialize)]
struct StagedInfo {
    version: String,
}

/// Written when a new version was swapped in, deleted once it has started.
#[derive(Serialize, Deserialize)]
struct InstalledInfo {
    version: String,
    previous_version: String,
    /// How often this version has started without reaching its first frame.
    starts: u32,
}

/// Unpacks the downloaded zip into `staged\`, then deletes the zip.
pub fn stage(zip_path: &Path, data_dir: &Path, version: &str) -> Result<()> {
    let update_dir = update_dir(data_dir);
    let staged = update_dir.join(STAGED_DIR);
    let _ = fs::remove_file(update_dir.join(STAGED_INFO));
    if staged.exists() {
        fs::remove_dir_all(&staged).context("can't clear the old unpacked update")?;
    }

    let file = File::open(zip_path).context("can't open the downloaded zip")?;
    let mut archive = zip::ZipArchive::new(file).context("the download isn't a valid zip")?;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).context("can't read the zip")?;
        // `enclosed_name` refuses paths such as `..\..\Windows`, which a
        // broken or hostile zip could contain.
        let Some(path) = entry.enclosed_name() else {
            bail!("the zip contains an unsafe path");
        };
        let Some(relative) = strip_top_folder(&path) else {
            continue;
        };
        // An update must never write into the user's data folder.
        // Windows paths ignore case, so `Data` counts too.
        if relative
            .components()
            .next()
            .is_some_and(|first| first.as_os_str().eq_ignore_ascii_case(DATA_FOLDER))
        {
            bail!("the zip contains a data folder, which an update must never replace");
        }
        let target = staged.join(&relative);
        if entry.is_dir() {
            fs::create_dir_all(&target)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut out =
            File::create(&target).with_context(|| format!("can't write {}", target.display()))?;
        // The zip crate checks each file's checksum while reading, so a
        // damaged download fails here.
        io::copy(&mut entry, &mut out)
            .with_context(|| format!("can't unpack {}", relative.display()))?;
    }
    if !staged.join(EXE_NAME).is_file() {
        bail!("the download has no {EXE_NAME} in it");
    }

    let info = toml::to_string(&StagedInfo {
        version: version.to_string(),
    })?;
    fs::write(update_dir.join(STAGED_INFO), info)?;
    let _ = fs::remove_file(zip_path);
    Ok(())
}

/// `Athena\sounds\a.wav` becomes `sounds\a.wav`. Anything outside the
/// `Athena` folder, and the folder itself, gives `None`.
fn strip_top_folder(path: &Path) -> Option<PathBuf> {
    let mut parts = path.components();
    if parts.next()?.as_os_str() != ZIP_TOP_FOLDER {
        return None;
    }
    let rest: PathBuf = parts.collect();
    if rest.as_os_str().is_empty() {
        None
    } else {
        Some(rest)
    }
}

/// What `main` should do after [`on_startup`].
#[derive(Debug, PartialEq)]
pub enum Startup {
    /// Carry on starting Athena.
    Continue(StartupInfo),
    /// Files were swapped: start `Athena.exe` again and end this process.
    LaunchAndExit,
}

#[derive(Debug, Default, PartialEq)]
pub struct StartupInfo {
    /// Something to tell the user once, such as "Updated to 0.1.1-alpha.7."
    pub note: Option<String>,
    /// True on the first start of a freshly installed version; the app then
    /// calls [`confirm_started`] after its first frame.
    pub just_installed: bool,
    /// A version that was rolled back, which the startup check skips.
    pub skipped_version: Option<String>,
}

/// Runs first thing at startup: installs a staged update or rolls back a
/// broken one. `current` is this exe's version (`None` for a local build).
/// Errors never stop Athena; they become a note for the user.
pub fn on_startup(data_dir: &Path, exe_dir: &Path, current: Option<&str>) -> Startup {
    let update_dir = update_dir(data_dir);
    if !update_dir.exists() {
        return Startup::Continue(StartupInfo::default());
    }
    let mut note = take_note(&update_dir);
    let skipped_version = fs::read_to_string(update_dir.join(SKIP_FILE))
        .ok()
        .map(|text| text.trim().to_string());

    // 1. Is this the first start of a version we just installed?
    let installed_path = update_dir.join(INSTALLED_INFO);
    if let Some(mut info) = read_toml::<InstalledInfo>(&installed_path) {
        if current == Some(info.version.as_str()) {
            if info.starts == 0 {
                info.starts = 1;
                let _ = toml::to_string(&info).map(|text| fs::write(&installed_path, text));
                return Startup::Continue(StartupInfo {
                    note: note.or(Some(format!("Updated to {}.", info.version))),
                    just_installed: true,
                    skipped_version,
                });
            }
            // The last start of this version never drew its window: go back.
            let _ = fs::remove_file(&installed_path);
            let previous = update_dir.join(PREVIOUS_DIR);
            let failed = update_dir.join(FAILED_DIR);
            let _ = fs::remove_dir_all(&failed);
            return match swap_in(&previous, exe_dir, &failed) {
                Ok(()) => {
                    let _ = fs::write(update_dir.join(SKIP_FILE), &info.version);
                    write_note(
                        &update_dir,
                        &format!(
                            "{} didn't start properly, so Athena went back to {}. The error, if any, is in athena-startup-error.txt next to Athena.exe.",
                            info.version, info.previous_version
                        ),
                    );
                    Startup::LaunchAndExit
                }
                Err(error) => Startup::Continue(StartupInfo {
                    note: Some(format!(
                        "{} didn't start properly the last time, and going back to {} failed: {error:#}",
                        info.version, info.previous_version
                    )),
                    just_installed: false,
                    skipped_version,
                }),
            };
        }
        // Another version is running (copied by hand, or a local build).
        let _ = fs::remove_file(&installed_path);
        let _ = fs::remove_dir_all(update_dir.join(PREVIOUS_DIR));
    }

    // 2. Is an unpacked update waiting? Local builds never install one.
    let staged_info_path = update_dir.join(STAGED_INFO);
    if let (Some(staged), Some(current)) = (read_toml::<StagedInfo>(&staged_info_path), current) {
        let staged_dir = update_dir.join(STAGED_DIR);
        let previous = update_dir.join(PREVIOUS_DIR);
        let result = fs::remove_dir_all(&previous)
            .or_else(|error| {
                if error.kind() == io::ErrorKind::NotFound {
                    Ok(())
                } else {
                    Err(error)
                }
            })
            .context("can't clear the folder for the old version")
            .and_then(|()| swap_in(&staged_dir, exe_dir, &previous));
        let _ = fs::remove_file(&staged_info_path);
        let _ = fs::remove_dir_all(&staged_dir);
        match result {
            Ok(()) => {
                let info = InstalledInfo {
                    version: staged.version,
                    previous_version: current.to_string(),
                    starts: 0,
                };
                let _ = toml::to_string(&info).map(|text| fs::write(&installed_path, text));
                return Startup::LaunchAndExit;
            }
            Err(error) => {
                note = Some(format!(
                    "Couldn't install {}: {error:#}. Athena is still on {current}.",
                    staged.version
                ));
            }
        }
    }

    // 3. Leftovers: a roll-back's files, which couldn't be deleted while
    // running, and a download or unpack that Athena was closed during.
    let _ = fs::remove_dir_all(update_dir.join(FAILED_DIR));
    if !staged_info_path.exists() {
        let _ = fs::remove_dir_all(update_dir.join(STAGED_DIR));
        let _ = fs::remove_file(update_dir.join(DOWNLOAD_FILE));
    }
    Startup::Continue(StartupInfo {
        note,
        just_installed: false,
        skipped_version,
    })
}

/// Moves every file in `source` into `exe_dir`, first moving the file it
/// replaces into `backup`. If any step fails, everything done so far is
/// put back, so Athena's folder is never left half-updated.
fn swap_in(source: &Path, exe_dir: &Path, backup: &Path) -> Result<()> {
    let files = list_files(source).context("can't read the new files")?;
    if files.is_empty() {
        bail!("there are no files to install");
    }
    // (file, whether an old copy was moved to `backup`), for undoing.
    let mut done: Vec<(PathBuf, bool)> = Vec::new();
    let mut result = Ok(());
    for relative in &files {
        let target = exe_dir.join(relative);
        let had_old = target.exists();
        if had_old && let Err(error) = move_file(&target, &backup.join(relative)) {
            result = Err(error);
            break;
        }
        done.push((relative.clone(), had_old));
        if let Err(error) = move_file(&source.join(relative), &target) {
            result = Err(error);
            break;
        }
    }
    if result.is_err() {
        for (relative, had_old) in done.iter().rev() {
            let target = exe_dir.join(relative);
            let _ = move_file(&target, &source.join(relative));
            if *had_old {
                let _ = move_file(&backup.join(relative), &target);
            }
        }
    }
    result
}

/// Moves a file, creating the target's folder. Renaming works on the
/// running exe too. Falls back to copy + delete across drives.
fn move_file(from: &Path, to: &Path) -> Result<()> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)?;
    }
    if fs::rename(from, to).is_ok() {
        return Ok(());
    }
    fs::copy(from, to)
        .and_then(|_| fs::remove_file(from))
        .with_context(|| format!("can't move {} to {}", from.display(), to.display()))
}

/// Every file below `dir`, as paths relative to it.
fn list_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut folders = vec![PathBuf::new()];
    while let Some(folder) = folders.pop() {
        for entry in fs::read_dir(dir.join(&folder))? {
            let entry = entry?;
            let relative = folder.join(entry.file_name());
            if entry.file_type()?.is_dir() {
                folders.push(relative);
            } else {
                files.push(relative);
            }
        }
    }
    files.sort();
    Ok(files)
}

fn read_toml<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    let text = fs::read_to_string(path).ok()?;
    toml::from_str(&text).ok()
}

fn write_note(update_dir: &Path, text: &str) {
    let _ = fs::write(update_dir.join(NOTE_FILE), text);
}

fn take_note(update_dir: &Path) -> Option<String> {
    let path = update_dir.join(NOTE_FILE);
    let text = fs::read_to_string(&path).ok()?;
    let _ = fs::remove_file(&path);
    Some(text)
}

/// Called once the new version has drawn its window: it works, so forget
/// the old files. Runs on its own thread, because the process that
/// installed us may still be ending and holding its old exe for a moment.
pub fn confirm_started(data_dir: &Path) {
    let update_dir = update_dir(data_dir);
    std::thread::spawn(move || {
        let gone = |result: io::Result<()>| match result {
            Ok(()) => true,
            Err(error) => error.kind() == io::ErrorKind::NotFound,
        };
        for _ in 0..20 {
            // The marker must go first: if it stayed while the backup was
            // deleted, the next start would try to roll back without one.
            if gone(fs::remove_file(update_dir.join(INSTALLED_INFO))) {
                let previous = fs::remove_dir_all(update_dir.join(PREVIOUS_DIR));
                let failed = fs::remove_dir_all(update_dir.join(FAILED_DIR));
                if gone(previous) && gone(failed) {
                    return;
                }
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    });
}

/// True when an unpacked update is waiting for the next start.
#[cfg(test)]
fn is_staged(data_dir: &Path) -> bool {
    update_dir(data_dir).join(STAGED_INFO).is_file()
}

/// Starts Athena again with `--wait-for-exit <this process>`. Called while
/// closing; the new process installs the update once this one is gone.
pub fn restart_to_install() -> Result<()> {
    let exe = std::env::current_exe().context("can't find Athena.exe")?;
    std::process::Command::new(exe)
        .arg("--wait-for-exit")
        .arg(std::process::id().to_string())
        .spawn()
        .context("can't start Athena again")?;
    Ok(())
}

/// Starts the freshly swapped-in `Athena.exe`.
pub fn launch(exe_dir: &Path) -> Result<()> {
    std::process::Command::new(exe_dir.join(EXE_NAME))
        .spawn()
        .context("can't start the new Athena.exe")?;
    Ok(())
}

/// Waits (up to 15 s) for the process with this id to end, so its files
/// are closed. Used with `--wait-for-exit`.
#[cfg(windows)]
pub fn wait_for_exit(pid: u32) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };
    // SAFETY: plain Win32 calls. The handle is checked before use and
    // closed afterwards. If the process is already gone, OpenProcess fails
    // and there is nothing to wait for.
    unsafe {
        let handle = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if !handle.is_null() {
            WaitForSingleObject(handle, 15_000);
            CloseHandle(handle);
        }
    }
}

/// Linux test builds: just give the old process a moment.
#[cfg(not(windows))]
pub fn wait_for_exit(_pid: u32) {
    std::thread::sleep(Duration::from_secs(1));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A fresh, empty folder for one test.
    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("athena-update-test-{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn make_zip(path: &Path, files: &[(&str, &str)]) {
        let mut zip = zip::ZipWriter::new(File::create(path).unwrap());
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (name, text) in files {
            zip.start_file(*name, options).unwrap();
            zip.write_all(text.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }

    fn read(path: &Path) -> String {
        fs::read_to_string(path).unwrap()
    }

    /// An "installed" Athena: exe folder with old files and a data folder.
    fn setup(name: &str) -> (PathBuf, PathBuf) {
        let root = temp_dir(name);
        let exe_dir = root.join("Athena");
        let data_dir = exe_dir.join("data");
        fs::create_dir_all(exe_dir.join("sounds")).unwrap();
        fs::create_dir_all(&data_dir).unwrap();
        fs::write(exe_dir.join("Athena.exe"), "old exe").unwrap();
        fs::write(exe_dir.join("static.sqlite"), "old db").unwrap();
        fs::write(exe_dir.join("sounds/a.wav"), "old a").unwrap();
        fs::write(data_dir.join("settings.toml"), "mine").unwrap();
        (exe_dir, data_dir)
    }

    fn stage_new_version(data_dir: &Path) {
        let zip_path = data_dir.join("download.zip");
        make_zip(
            &zip_path,
            &[
                ("Athena/Athena.exe", "new exe"),
                ("Athena/static.sqlite", "new db"),
                ("Athena/sounds/a.wav", "new a"),
                ("Athena/sounds/b.wav", "new b"),
            ],
        );
        stage(&zip_path, data_dir, "0.1.1-alpha.7").unwrap();
        assert!(!zip_path.exists());
        assert!(is_staged(data_dir));
    }

    #[test]
    fn full_update_then_first_start() {
        let (exe_dir, data_dir) = setup("full");
        stage_new_version(&data_dir);

        // The old version restarts and swaps the files.
        let step = on_startup(&data_dir, &exe_dir, Some("0.1.1-alpha.6"));
        assert_eq!(step, Startup::LaunchAndExit);
        assert_eq!(read(&exe_dir.join("Athena.exe")), "new exe");
        assert_eq!(read(&exe_dir.join("sounds/b.wav")), "new b");
        assert_eq!(read(&data_dir.join("settings.toml")), "mine");
        let previous = update_dir(&data_dir).join(PREVIOUS_DIR);
        assert_eq!(read(&previous.join("Athena.exe")), "old exe");
        assert!(!is_staged(&data_dir));

        // The new version starts for the first time.
        let step = on_startup(&data_dir, &exe_dir, Some("0.1.1-alpha.7"));
        assert_eq!(
            step,
            Startup::Continue(StartupInfo {
                note: Some("Updated to 0.1.1-alpha.7.".into()),
                just_installed: true,
                skipped_version: None,
            })
        );
    }

    #[test]
    fn a_new_version_that_never_draws_is_rolled_back() {
        let (exe_dir, data_dir) = setup("rollback");
        stage_new_version(&data_dir);
        on_startup(&data_dir, &exe_dir, Some("0.1.1-alpha.6"));
        // First start of the new version, which then crashes before
        // `confirm_started`...
        on_startup(&data_dir, &exe_dir, Some("0.1.1-alpha.7"));
        // ...so the second start goes back.
        let step = on_startup(&data_dir, &exe_dir, Some("0.1.1-alpha.7"));
        assert_eq!(step, Startup::LaunchAndExit);
        assert_eq!(read(&exe_dir.join("Athena.exe")), "old exe");
        assert_eq!(read(&exe_dir.join("sounds/a.wav")), "old a");
        // The old version then starts and tells the user.
        let Startup::Continue(info) = on_startup(&data_dir, &exe_dir, Some("0.1.1-alpha.6")) else {
            panic!("expected Continue");
        };
        assert!(info.note.unwrap().contains("went back to 0.1.1-alpha.6"));
        assert_eq!(info.skipped_version.as_deref(), Some("0.1.1-alpha.7"));
    }

    #[test]
    fn refuses_a_zip_with_a_data_folder() {
        let (_exe_dir, data_dir) = setup("datazip");
        let zip_path = data_dir.join("download.zip");
        make_zip(
            &zip_path,
            &[
                ("Athena/Athena.exe", "new exe"),
                ("Athena/Data/settings.toml", "overwritten"),
            ],
        );
        assert!(stage(&zip_path, &data_dir, "0.1.1-alpha.7").is_err());
        assert!(!is_staged(&data_dir));
        assert_eq!(read(&data_dir.join("settings.toml")), "mine");
    }

    #[test]
    fn cleans_up_an_unfinished_download() {
        let (exe_dir, data_dir) = setup("leftover");
        let update = update_dir(&data_dir);
        fs::create_dir_all(update.join(STAGED_DIR)).unwrap();
        fs::write(update.join(STAGED_DIR).join("half"), "x").unwrap();
        fs::write(update.join(DOWNLOAD_FILE), "x").unwrap();
        on_startup(&data_dir, &exe_dir, Some("0.1.1-alpha.6"));
        assert!(!update.join(STAGED_DIR).exists());
        assert!(!update.join(DOWNLOAD_FILE).exists());
    }

    #[test]
    fn local_builds_never_install() {
        let (exe_dir, data_dir) = setup("local");
        stage_new_version(&data_dir);
        let step = on_startup(&data_dir, &exe_dir, None);
        assert_eq!(step, Startup::Continue(StartupInfo::default()));
        assert_eq!(read(&exe_dir.join("Athena.exe")), "old exe");
    }

    #[test]
    fn a_failed_swap_puts_everything_back() {
        let (exe_dir, data_dir) = setup("undo");
        stage_new_version(&data_dir);
        // The update wants a file inside a folder `README.md`, but there is
        // a file of that name, so the second move fails (after Athena.exe
        // was already swapped).
        let staged = update_dir(&data_dir).join(STAGED_DIR);
        fs::create_dir_all(staged.join("README.md")).unwrap();
        fs::write(staged.join("README.md/inner"), "x").unwrap();
        fs::write(exe_dir.join("README.md"), "old readme").unwrap();

        let step = on_startup(&data_dir, &exe_dir, Some("0.1.1-alpha.6"));
        let Startup::Continue(info) = step else {
            panic!("expected Continue");
        };
        assert!(
            info.note
                .unwrap()
                .starts_with("Couldn't install 0.1.1-alpha.7")
        );
        assert_eq!(read(&exe_dir.join("Athena.exe")), "old exe");
        assert_eq!(read(&exe_dir.join("static.sqlite")), "old db");
        assert_eq!(read(&exe_dir.join("sounds/a.wav")), "old a");
        assert_eq!(read(&exe_dir.join("README.md")), "old readme");
        assert!(!exe_dir.join("sounds/b.wav").exists());
    }

    #[test]
    fn rejects_a_zip_without_the_exe() {
        let (_exe_dir, data_dir) = setup("noexe");
        let zip_path = data_dir.join("download.zip");
        make_zip(&zip_path, &[("Athena/README.md", "hi")]);
        assert!(stage(&zip_path, &data_dir, "0.1.1-alpha.7").is_err());
        assert!(!is_staged(&data_dir));
    }

    #[test]
    fn strips_the_top_folder() {
        assert_eq!(
            strip_top_folder(Path::new("Athena/sounds/a.wav")),
            Some(PathBuf::from("sounds/a.wav"))
        );
        assert_eq!(strip_top_folder(Path::new("Athena/")), None);
        assert_eq!(strip_top_folder(Path::new("Other/a.txt")), None);
    }
}
