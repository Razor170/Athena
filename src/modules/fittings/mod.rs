//! Fittings: tools for ship fits. It has submodules, shown as tabs inside
//! the Fittings panel (like CIC). The first one is **Compare**: paste the
//! fit you have (Is) and the fit you want (Should) in EFT format, and the
//! Diff column lists what is missing (green) and what is too much (red).
//!
//! The pieces:
//!
//! - [`eft`]: reads EFT text and compares two fits.
//! - `ui`: draws the panel.
//!
//! Both fits and the "Show matching" switch are saved in `fittings.toml`
//! in the data folder, so they are still there after a restart.

pub mod eft;
mod ui;

use std::path::{Path, PathBuf};

use eframe::egui;
use serde::{Deserialize, Serialize};

use self::eft::{Diff, Fit};
use super::{Module, ModuleContext};
use crate::storage::Saver;

const FILE_NAME: &str = "fittings.toml";
/// Clipboard text bigger than this can't be a fit (a full one is ~2 KB).
const MAX_PASTE_BYTES: usize = 256 * 1024;

/// The submodules, one tab each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Submodule {
    #[default]
    Compare,
}

/// Which of the two input columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Is,
    Should,
}

/// A column shows the parsed list or the raw text to edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColumnMode {
    #[default]
    List,
    Text,
}

/// What `fittings.toml` holds. New fields need a default so older files
/// still load.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Saved {
    pub is_text: String,
    pub should_text: String,
    pub show_matching: bool,
}

impl Saved {
    fn file_path(data_dir: &Path) -> PathBuf {
        data_dir.join(FILE_NAME)
    }

    /// Reads the saved fits. A missing file is normal (first start); a
    /// broken one gives empty fits and a message.
    fn load(data_dir: &Path) -> (Saved, Option<String>) {
        let path = Saved::file_path(data_dir);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return (Saved::default(), None);
        };
        match toml::from_str::<Saved>(&text) {
            Ok(saved) => (saved, None),
            Err(err) => (
                Saved::default(),
                Some(format!("Couldn't read {FILE_NAME}, starting empty: {err}")),
            ),
        }
    }
}

pub struct Fittings {
    data_dir: PathBuf,
    saver: Saver,
    saved: Saved,
    submodule: Submodule,
    is_mode: ColumnMode,
    should_mode: ColumnMode,
    /// Parsed from the texts. Only rebuilt when a text changes, not every
    /// frame.
    is_fit: Fit,
    should_fit: Fit,
    diff: Diff,
    /// A short note under the header, such as "Copied 7 lines".
    message: Option<String>,
}

impl Fittings {
    pub fn new(data_dir: &Path) -> Fittings {
        let (saved, problem) = Saved::load(data_dir);
        let mut module = Fittings {
            data_dir: data_dir.to_path_buf(),
            saver: Saver::start(),
            saved,
            submodule: Submodule::default(),
            is_mode: ColumnMode::default(),
            should_mode: ColumnMode::default(),
            is_fit: Fit::default(),
            should_fit: Fit::default(),
            diff: Diff::default(),
            message: problem,
        };
        module.reparse();
        module
    }

    /// Parses both texts again and compares them.
    fn reparse(&mut self) {
        self.is_fit = eft::parse(&self.saved.is_text);
        self.should_fit = eft::parse(&self.saved.should_text);
        self.diff = eft::compare(&self.is_fit, &self.should_fit);
    }

    fn save(&mut self) {
        if let Ok(text) = toml::to_string(&self.saved) {
            self.saver.save(Saved::file_path(&self.data_dir), text);
        }
    }

    /// A text was edited: parse, compare and save.
    fn texts_changed(&mut self) {
        self.reparse();
        self.save();
    }

    fn text_mut(&mut self, side: Side) -> &mut String {
        match side {
            Side::Is => &mut self.saved.is_text,
            Side::Should => &mut self.saved.should_text,
        }
    }

    /// "Paste": the clipboard text replaces that column's fit.
    fn paste(&mut self, side: Side) {
        // Reading the clipboard once is quick (no disk or network), like
        // Intel's tag import.
        let Some(text) = crate::modules::mining::clipboard::read_text_once(MAX_PASTE_BYTES) else {
            self.message = Some(if crate::modules::mining::clipboard::is_supported() {
                "Couldn't read text from the clipboard. In EVE's fitting window use \
                 \"Copy to clipboard\", then try again."
                    .to_string()
            } else {
                "Paste only works on Windows. Use Text and Ctrl+V instead.".to_string()
            });
            return;
        };
        *self.text_mut(side) = text;
        self.message = None;
        self.texts_changed();
    }

    fn swap(&mut self) {
        std::mem::swap(&mut self.saved.is_text, &mut self.saved.should_text);
        std::mem::swap(&mut self.is_mode, &mut self.should_mode);
        self.texts_changed();
    }

    /// "Copy missing": the green items as Multibuy lines.
    fn copy_missing(&mut self, ctx: &egui::Context) {
        let text = self.diff.multibuy();
        let lines = text.lines().count();
        if lines == 0 {
            self.message = Some("Nothing is missing.".to_string());
            return;
        }
        ctx.copy_text(text);
        self.message = Some(format!(
            "Copied {lines} lines. Paste them into EVE's Multibuy."
        ));
    }
}

impl Module for Fittings {
    fn id(&self) -> &'static str {
        "fittings"
    }

    fn title(&self) -> &'static str {
        "Fittings"
    }

    fn ui(&mut self, ui: &mut egui::Ui, _ctx: &ModuleContext) {
        ui::show(self, ui);
    }

    fn on_exit(&mut self) {
        self.saver.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_fits_read_back() {
        let saved = Saved {
            is_text: "[Drake, A]\n\nDamage Control II".to_string(),
            should_text: "[Drake, B]".to_string(),
            show_matching: true,
        };
        let text = toml::to_string(&saved).unwrap();
        let back: Saved = toml::from_str(&text).unwrap();
        assert_eq!(back, saved);
        // An older or partial file still loads.
        let partial: Saved = toml::from_str("show_matching = true").unwrap();
        assert!(partial.show_matching);
        assert!(partial.is_text.is_empty());
    }
}
