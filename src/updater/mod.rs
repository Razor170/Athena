//! The auto-updater: finds newer Athena releases on GitHub, downloads one
//! and swaps it in on restart (PLAN.md Step 7).
//!
//! All GitHub, Credential Manager and file work happens on one background
//! thread. The UI sends it a [`Command`] and gets [`Event`]s back through
//! channels, which `update` drains each frame. The thread asks for a repaint
//! only when it has news, so the updater costs nothing while idle.

mod github;
pub mod install;
mod semver;
mod token;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{SystemTime, UNIX_EPOCH};

use eframe::egui;

use crate::settings::Settings;
use crate::storage::DataDir;
use crate::version;
use github::{CheckResult, Release};
use semver::Version;

pub use install::{Startup, StartupInfo};

/// Where to make the token. Opens GitHub's "new fine-grained token" page.
const NEW_TOKEN_URL: &str = "https://github.com/settings/personal-access-tokens/new";
/// From this many days before the token expires, Settings warns about it.
const EXPIRY_WARNING_DAYS: i64 = 14;

/// Work for the background thread.
// No `Debug` on purpose: `SaveToken` holds the token, and it must never
// end up in debug output.
enum Command {
    /// Read the token and, if asked, check for updates.
    Startup {
        check: bool,
        include_prereleases: bool,
    },
    SaveToken {
        token: String,
        include_prereleases: bool,
    },
    RemoveToken,
    Check {
        include_prereleases: bool,
    },
    Download(Release),
}

/// News from the background thread.
enum Event {
    TokenStatus {
        saved: bool,
        error: Option<String>,
    },
    Checked {
        /// True when the user pressed a button (errors are then shown in
        /// the strip); false for the quiet check at startup.
        manual: bool,
        result: Result<CheckResult, String>,
    },
    Progress {
        done: u64,
        total: u64,
    },
    Staged {
        version: String,
    },
    DownloadFailed(String),
}

/// The strip under the toggle bar.
enum Strip {
    Hidden,
    Available(Release),
    Downloading {
        version: String,
        done: u64,
        total: u64,
    },
    Ready {
        version: String,
    },
    Error(String),
}

/// What the app should do after drawing the strip.
#[derive(PartialEq)]
pub enum StripAction {
    None,
    OpenSettings,
    /// Close Athena and start it again to install the update.
    Restart,
}

/// How a status line in Settings is coloured.
enum Tone {
    Normal,
    Good,
    Bad,
}

pub struct Updater {
    commands: Sender<Command>,
    events: Receiver<Event>,
    /// Set to stop a running download.
    cancel: Arc<AtomicBool>,
    /// True when the data folder fell back to %APPDATA%, which means
    /// Athena's own folder isn't writable and files can't be swapped there.
    folder_read_only: bool,
    /// `None` until the background thread has looked.
    token_saved: Option<bool>,
    token_expires: Option<String>,
    /// What the user is typing into the token field. Cleared on save.
    token_input: String,
    /// True while the "Replace" field is open for a saved token.
    replacing_token: bool,
    strip: Strip,
    notes_open: bool,
    status: Option<(Tone, String)>,
    checked_at: Option<String>,
    checking: bool,
    /// True on the first start after an update, until the first frame.
    confirm_pending: bool,
    /// A version that was rolled back; the startup check doesn't offer it.
    skipped_version: Option<String>,
    data_dir: PathBuf,
}

impl Updater {
    /// Starts the background thread. `startup` is what `install::on_startup`
    /// found before the window opened.
    pub fn start(
        ctx: &egui::Context,
        settings: &Settings,
        data_dir: &DataDir,
        startup: StartupInfo,
    ) -> Updater {
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let ctx = ctx.clone();
            let data_dir = data_dir.path.clone();
            let cancel = cancel.clone();
            std::thread::Builder::new()
                .name("updater".into())
                .spawn(move || run_worker(ctx, data_dir, command_rx, event_tx, cancel))
                .ok();
        }
        // Local builds don't update themselves, so they don't check on their own.
        let check = settings.update_check_on_start && version::BUILT_VERSION.is_some();
        let _ = command_tx.send(Command::Startup {
            check,
            include_prereleases: settings.update_include_prereleases,
        });
        Updater {
            commands: command_tx,
            events: event_rx,
            cancel,
            folder_read_only: data_dir.is_fallback,
            token_saved: None,
            token_expires: None,
            token_input: String::new(),
            replacing_token: false,
            strip: Strip::Hidden,
            notes_open: false,
            status: startup.note.map(|note| (Tone::Normal, note)),
            checked_at: None,
            checking: check,
            confirm_pending: startup.just_installed,
            skipped_version: startup.skipped_version,
            data_dir: data_dir.path.clone(),
        }
    }

    /// Reads the background thread's news. Call once per frame.
    pub fn update(&mut self) {
        while let Ok(event) = self.events.try_recv() {
            self.handle(event);
        }
    }

    fn handle(&mut self, event: Event) {
        match event {
            Event::TokenStatus { saved, error } => {
                self.token_saved = Some(saved);
                if !saved {
                    self.token_expires = None;
                    self.checking = false;
                }
                if let Some(error) = error {
                    self.status = Some((Tone::Bad, error));
                }
            }
            Event::Checked { manual, result } => {
                self.checking = false;
                self.checked_at = Some(utc_clock());
                match result {
                    Ok(result) => {
                        self.token_expires = result.token_expires;
                        self.found(result.newest, manual);
                    }
                    Err(error) => {
                        if manual {
                            self.strip = Strip::Error(error.clone());
                        }
                        self.status = Some((Tone::Bad, error));
                    }
                }
            }
            Event::Progress { done, total } => {
                if let Strip::Downloading {
                    done: d, total: t, ..
                } = &mut self.strip
                {
                    *d = done;
                    *t = total;
                }
            }
            Event::Staged { version } => {
                self.strip = Strip::Ready { version };
            }
            Event::DownloadFailed(error) => {
                if self.cancel.load(Ordering::Relaxed) {
                    self.strip = Strip::Hidden;
                } else {
                    self.status = Some((Tone::Bad, error.clone()));
                    self.strip = Strip::Error(error);
                }
            }
        }
    }

    /// A check finished: offer the release if it is newer than us.
    fn found(&mut self, newest: Option<Release>, manual: bool) {
        let current = current_version();
        let newer = newest.filter(|release| current.as_ref().is_none_or(|c| release.version > *c));
        match newer {
            None => {
                self.status = Some((Tone::Good, "You have the newest version.".into()));
                if matches!(self.strip, Strip::Error(_)) {
                    self.strip = Strip::Hidden;
                }
            }
            Some(release)
                if !manual && self.skipped_version.as_deref() == Some(&release.version_text) =>
            {
                // It was installed before and didn't start, so don't offer
                // it on its own again. "Check for updates" still offers it.
                self.status = Some((
                    Tone::Normal,
                    format!(
                        "{} is available, but it didn't start last time, so it isn't offered on startup.",
                        release.version_text
                    ),
                ));
            }
            Some(release) => {
                self.status = Some((
                    Tone::Normal,
                    format!("{} is available.", release.version_text),
                ));
                // Don't interrupt a download or a finished one.
                if matches!(
                    self.strip,
                    Strip::Hidden | Strip::Error(_) | Strip::Available(_)
                ) {
                    self.strip = Strip::Available(release);
                }
            }
        }
    }

    /// Call after a frame was drawn: the first frame after an update proves
    /// the new version works, so its backup can go.
    pub fn frame_drawn(&mut self) {
        if self.confirm_pending {
            self.confirm_pending = false;
            install::confirm_started(&self.data_dir);
        }
    }

    /// Why this copy can't install updates, if it can't.
    fn cannot_install(&self) -> Option<&'static str> {
        if version::BUILT_VERSION.is_none() {
            Some("This is a local build, so it doesn't update itself.")
        } else if self.folder_read_only {
            Some("Athena's folder isn't writable, so it can't update itself here.")
        } else {
            None
        }
    }

    pub fn strip_visible(&self) -> bool {
        !matches!(self.strip, Strip::Hidden)
    }

    /// Draws the strip under the toggle bar. Call only when `strip_visible`.
    pub fn strip_ui(&mut self, ui: &mut egui::Ui) -> StripAction {
        let fill = match self.strip {
            Strip::Ready { .. } => egui::Color32::from_rgb(29, 48, 32),
            Strip::Error(_) => egui::Color32::from_rgb(58, 34, 34),
            _ => egui::Color32::from_rgb(31, 45, 61),
        };
        let frame = egui::Frame::new()
            .fill(fill)
            .inner_margin(egui::Margin::symmetric(8, 5));
        let mut action = StripAction::None;
        egui::Panel::top("update_strip")
            .frame(frame)
            .show(ui, |ui| {
                action = self.strip_contents(ui);
            });
        action
    }

    fn strip_contents(&mut self, ui: &mut egui::Ui) -> StripAction {
        let mut action = StripAction::None;
        let cannot_install = self.cannot_install();
        let mut next: Option<Strip> = None;
        match &self.strip {
            Strip::Hidden => {}
            Strip::Available(release) => {
                ui.horizontal_wrapped(|ui| {
                    ui.strong("Update available:");
                    ui.label(format!(
                        "{} (you have {})",
                        release.version_text,
                        version::display_text()
                    ));
                    let link = if self.notes_open {
                        "Hide what's new"
                    } else {
                        "What's new"
                    };
                    if ui.link(link).clicked() {
                        self.notes_open = !self.notes_open;
                    }
                    let update =
                        ui.add_enabled(cannot_install.is_none(), egui::Button::new("Update now"));
                    let update = match cannot_install {
                        Some(reason) => update.on_disabled_hover_text(reason),
                        None => update,
                    };
                    if update.clicked() {
                        self.cancel.store(false, Ordering::Relaxed);
                        let _ = self.commands.send(Command::Download(release.clone()));
                        next = Some(Strip::Downloading {
                            version: release.version_text.clone(),
                            done: 0,
                            total: release.size,
                        });
                    }
                    if ui.button("Later").clicked() {
                        next = Some(Strip::Hidden);
                    }
                });
                if self.notes_open {
                    ui.add_space(4.0);
                    ui.weak(format!(
                        "{}, released {}",
                        release.version_text, release.date
                    ));
                    if release.notes.is_empty() {
                        ui.weak("No notes for this version.");
                    }
                    for line in &release.notes {
                        ui.label(format!("• {line}"));
                    }
                }
            }
            Strip::Downloading {
                version,
                done,
                total,
            } => {
                ui.horizontal_wrapped(|ui| {
                    ui.strong(format!("Downloading {version}…"));
                    let fraction = if *total > 0 {
                        *done as f32 / *total as f32
                    } else {
                        0.0
                    };
                    ui.add(
                        egui::ProgressBar::new(fraction)
                            .desired_width(240.0)
                            .text(format!("{} / {}", megabytes(*done), megabytes(*total))),
                    );
                    if ui.button("Cancel").clicked() {
                        self.cancel.store(true, Ordering::Relaxed);
                    }
                });
            }
            Strip::Ready { version } => {
                ui.horizontal_wrapped(|ui| {
                    ui.strong(format!("{version} is ready."));
                    ui.label(
                        "Restarting takes a few seconds; your settings, layout and mining history are kept.",
                    );
                    if ui.button("Restart now").clicked() {
                        action = StripAction::Restart;
                    }
                    if ui.button("On next start").clicked() {
                        next = Some(Strip::Hidden);
                    }
                });
            }
            Strip::Error(error) => {
                ui.horizontal_wrapped(|ui| {
                    ui.strong("Can't update:");
                    ui.label(error);
                    if ui.button("Open Settings").clicked() {
                        action = StripAction::OpenSettings;
                    }
                    if ui.button("Close").clicked() {
                        next = Some(Strip::Hidden);
                    }
                });
            }
        }
        if let Some(next) = next {
            self.strip = next;
        }
        action
    }

    /// The "Updates" section of the Settings tab. Returns true when a
    /// setting changed and needs saving.
    pub fn settings_ui(&mut self, ui: &mut egui::Ui, settings: &mut Settings) -> bool {
        let mut changed = false;
        ui.heading("Updates");
        ui.label(format!("This version: {}", version::display_text()));

        match self.token_saved {
            None => {
                ui.weak("Reading the GitHub token…");
            }
            Some(false) => {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "Add a GitHub token to get updates.",
                );
                self.token_field(ui, settings);
            }
            Some(true) => {
                ui.colored_label(
                    good_color(ui),
                    "GitHub token saved in Windows Credential Manager.",
                );
                ui.horizontal_wrapped(|ui| {
                    self.expiry_label(ui);
                    if ui.link("Replace").clicked() {
                        self.replacing_token = !self.replacing_token;
                    }
                    if ui.link("Remove").clicked() {
                        self.replacing_token = false;
                        let _ = self.commands.send(Command::RemoveToken);
                    }
                });
                if self.replacing_token {
                    self.token_field(ui, settings);
                }
            }
        }

        changed |= ui
            .checkbox(
                &mut settings.update_check_on_start,
                "Check for updates when Athena starts",
            )
            .changed();
        changed |= ui
            .checkbox(
                &mut settings.update_include_prereleases,
                "Include pre-releases (alpha builds)",
            )
            .changed();
        ui.weak("Off: only versions tagged by hand, like v0.2.0.");

        ui.horizontal_wrapped(|ui| {
            let busy = self.checking || matches!(self.strip, Strip::Downloading { .. });
            let can_check = self.token_saved == Some(true) && !busy;
            if ui
                .add_enabled(can_check, egui::Button::new("Check for updates"))
                .clicked()
            {
                self.checking = true;
                let _ = self.commands.send(Command::Check {
                    include_prereleases: settings.update_include_prereleases,
                });
            }
            if self.checking {
                ui.spinner();
            } else if let Some((tone, text)) = &self.status {
                let color = match tone {
                    Tone::Normal => ui.visuals().text_color(),
                    Tone::Good => good_color(ui),
                    Tone::Bad => ui.visuals().error_fg_color,
                };
                ui.colored_label(color, text);
            }
            if let Some(time) = &self.checked_at {
                ui.weak(format!("Checked {time} EVE time"));
            }
        });
        if let Some(reason) = self.cannot_install() {
            ui.weak(reason);
        }
        changed
    }

    /// The paste field, Save button and the steps to make a token.
    fn token_field(&mut self, ui: &mut egui::Ui, settings: &Settings) {
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.token_input)
                    .password(true)
                    .hint_text("github_pat_…")
                    .desired_width(260.0),
            );
            let token = self.token_input.trim();
            if ui
                .add_enabled(!token.is_empty(), egui::Button::new("Save token"))
                .clicked()
            {
                self.checking = true;
                let _ = self.commands.send(Command::SaveToken {
                    token: token.to_string(),
                    include_prereleases: settings.update_include_prereleases,
                });
                // Forget it on the UI side straight away.
                self.token_input.clear();
                self.replacing_token = false;
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.weak("Make one at ");
            if ui.link("github.com/settings/personal-access-tokens/new").clicked()
                && let Err(error) = crate::shell::open_url(NEW_TOKEN_URL)
            {
                self.status = Some((Tone::Bad, error));
            }
            ui.weak(
                ". Repository access: \"Only select repositories\", pick Athena. Permissions: Contents, Read-only. Nothing else.",
            );
        });
    }

    fn expiry_label(&self, ui: &mut egui::Ui) {
        let Some(date) = &self.token_expires else {
            ui.weak("Expiry date not known yet.");
            return;
        };
        match days_until(date) {
            Some(days) if days < 0 => {
                ui.colored_label(ui.visuals().error_fg_color, format!("Expired on {date}."));
            }
            Some(days) if days <= EXPIRY_WARNING_DAYS => {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    format!("Expires in {days} days, make a new token."),
                );
            }
            _ => {
                ui.weak(format!("Expires {date}."));
            }
        }
    }
}

/// This exe's version for comparing. A local build counts as the version
/// in Cargo.toml.
fn current_version() -> Option<Version> {
    version::BUILT_VERSION
        .and_then(Version::parse)
        .or_else(|| Version::parse(env!("CARGO_PKG_VERSION")))
}

/// The background thread. Handles one command at a time until the app ends.
fn run_worker(
    ctx: egui::Context,
    data_dir: PathBuf,
    commands: Receiver<Command>,
    events: Sender<Event>,
    cancel: Arc<AtomicBool>,
) {
    // The token lives here in memory only while Athena runs.
    let mut saved_token: Option<String> = None;
    let send = |event: Event| {
        let _ = events.send(event);
        ctx.request_repaint();
    };
    let client = github::client();

    let check = |token: &str, include_prereleases: bool, manual: bool| {
        let result = match &client {
            Ok(client) => github::check(client, token, include_prereleases)
                .map_err(|error| format!("{error:#}")),
            Err(error) => Err(format!("{error:#}")),
        };
        send(Event::Checked { manual, result });
    };

    // `for` over a receiver waits for the next command and ends when the
    // app (the sender) is gone.
    for command in commands {
        match command {
            Command::Startup {
                check: wanted,
                include_prereleases,
            } => {
                match token::load() {
                    Ok(token) => {
                        saved_token = token;
                        send(Event::TokenStatus {
                            saved: saved_token.is_some(),
                            error: None,
                        });
                    }
                    Err(error) => send(Event::TokenStatus {
                        saved: false,
                        error: Some(format!("{error:#}")),
                    }),
                }
                if wanted && let Some(token) = &saved_token {
                    check(token, include_prereleases, false);
                }
            }
            Command::SaveToken {
                token,
                include_prereleases,
            } => match token::save(&token) {
                Ok(()) => {
                    send(Event::TokenStatus {
                        saved: true,
                        error: None,
                    });
                    // Check right away, which also shows whether the token works.
                    check(&token, include_prereleases, true);
                    saved_token = Some(token);
                }
                Err(error) => send(Event::Checked {
                    manual: false,
                    result: Err(format!("{error:#}")),
                }),
            },
            Command::RemoveToken => {
                let error = token::remove().err().map(|error| format!("{error:#}"));
                if error.is_none() {
                    saved_token = None;
                }
                send(Event::TokenStatus {
                    saved: saved_token.is_some(),
                    error,
                });
            }
            Command::Check {
                include_prereleases,
            } => match &saved_token {
                Some(token) => check(token, include_prereleases, true),
                None => send(Event::Checked {
                    manual: true,
                    result: Err("Add a GitHub token first.".into()),
                }),
            },
            Command::Download(release) => {
                let result = match (&client, &saved_token) {
                    (Ok(client), Some(token)) => {
                        download_and_stage(client, token, &release, &data_dir, &cancel, &send)
                    }
                    (Err(error), _) => Err(format!("{error:#}")),
                    (_, None) => Err("Add a GitHub token first.".into()),
                };
                send(match result {
                    Ok(()) => Event::Staged {
                        version: release.version_text.clone(),
                    },
                    Err(error) => Event::DownloadFailed(error),
                });
            }
        }
    }
}

fn download_and_stage(
    client: &reqwest::blocking::Client,
    token: &str,
    release: &Release,
    data_dir: &std::path::Path,
    cancel: &AtomicBool,
    send: &dyn Fn(Event),
) -> Result<(), String> {
    let update_dir = install::update_dir(data_dir);
    std::fs::create_dir_all(&update_dir)
        .map_err(|error| format!("can't create {}: {error}", update_dir.display()))?;
    let zip_path = update_dir.join(install::DOWNLOAD_FILE);
    let result = github::download(client, token, release, &zip_path, cancel, |done, total| {
        send(Event::Progress { done, total })
    })
    .and_then(|()| install::stage(&zip_path, data_dir, &release.version_text));
    if result.is_err() {
        let _ = std::fs::remove_file(&zip_path);
    }
    result.map_err(|error| format!("{error:#}"))
}

/// "24.1 MB"
fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_000_000.0)
}

fn good_color(ui: &egui::Ui) -> egui::Color32 {
    if ui.visuals().dark_mode {
        egui::Color32::from_rgb(108, 207, 108)
    } else {
        egui::Color32::from_rgb(30, 130, 30)
    }
}

/// The current UTC time as "19:42" (UTC is EVE time).
fn utc_clock() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    let of_day = seconds % 86_400;
    format!("{:02}:{:02}", of_day / 3600, of_day % 3600 / 60)
}

/// Whole days from today (UTC) until a `YYYY-MM-DD` date.
fn days_until(date: &str) -> Option<i64> {
    let today = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs() as i64 / 86_400;
    Some(date_to_days(date)? - today)
}

/// Days since 1970-01-01 for a `YYYY-MM-DD` date.
fn date_to_days(date: &str) -> Option<i64> {
    let mut parts = date.split('-').map(|part| part.parse::<i64>().ok());
    let (year, month, day) = (parts.next()??, parts.next()??, parts.next()??);
    if parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some(crate::modules::mining::gamelog::days_from_civil(
        year, month, day,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_expiry_dates() {
        assert_eq!(date_to_days("1970-01-01"), Some(0));
        assert_eq!(date_to_days("2027-11-10"), Some(21_132));
        assert_eq!(date_to_days("2027-13-10"), None);
        assert_eq!(date_to_days("soon"), None);
    }

    #[test]
    fn shows_megabytes() {
        assert_eq!(megabytes(38_912_000), "38.9 MB");
    }
}
