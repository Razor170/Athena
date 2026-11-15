//! Overlays: Athena windows drawn on top of the screen, over EVE.
//!
//! - [`mining_bars`]: the Mining Supervisor's hold bars overlay, what it
//!   shows and how it looks.
//! - [`logi_lines`]: the CIC Logi overlay, one line per pilot.
//! - `pixels`: drawing into a pixel buffer, shared by both.
//! - `window`: the window that shows either of them, run by its own thread
//!   (Windows only).
//! - [`popup`]: the Intel Supervisor's alert popup.
//!
//! How the bars and Logi overlays fit together: the app's `logic` calls
//! [`OverlayWindow::show`] before every frame with what the module wants
//! shown. Only a real change wakes the overlay thread, which then redraws
//! the window. When the user moved the overlay, the app hands the new place
//! back to the module with [`OverlayWindow::take_moved`], so it is saved.

pub mod logi_lines;
pub mod mining_bars;
mod pixels;
pub mod popup;
#[cfg(windows)]
mod window;

use eframe::egui;

use self::logi_lines::LogiOverlay;
use self::mining_bars::{BarsOverlay, OverlayPlace};

/// Whether the overlays work on this system (Windows only).
pub fn is_supported() -> bool {
    cfg!(windows)
}

/// What an overlay window shows.
#[derive(Debug, Clone, PartialEq)]
pub enum Content {
    /// The Mining Supervisor's hold bars.
    Bars(BarsOverlay),
    /// The CIC Logi lines.
    Logi(LogiOverlay),
}

// Used by the Windows window only.
#[cfg_attr(not(windows), allow(dead_code))]
impl Content {
    pub fn move_mode(&self) -> bool {
        match self {
            Content::Bars(o) => o.move_mode,
            Content::Logi(o) => o.move_mode,
        }
    }

    pub fn position(&self) -> Option<[i32; 2]> {
        match self {
            Content::Bars(o) => o.position,
            Content::Logi(o) => o.position,
        }
    }

    pub fn clamped_width(&self) -> i32 {
        match self {
            Content::Bars(o) => o.clamped_width(),
            Content::Logi(o) => o.clamped_width(),
        }
    }

    /// Nothing to show: no bars, or no lines.
    pub fn is_empty(&self) -> bool {
        match self {
            Content::Bars(o) => o.bars.is_empty(),
            Content::Logi(o) => o.lines.is_empty(),
        }
    }
}

/// One overlay window, owned by the app.
pub struct OverlayWindow {
    #[cfg(windows)]
    thread: window::OverlayThread,
}

impl OverlayWindow {
    /// `name` names the window's thread, such as "mining-overlay".
    pub fn new(egui_ctx: &egui::Context, name: &str) -> OverlayWindow {
        #[cfg(windows)]
        return OverlayWindow {
            thread: window::OverlayThread::start(egui_ctx, name),
        };
        #[cfg(not(windows))]
        {
            let _ = (egui_ctx, name);
            OverlayWindow {}
        }
    }

    /// Shows `content`, or no overlay for `None`. Cheap to call every
    /// frame: the window is only redrawn when something changed.
    pub fn show(&mut self, content: Option<Content>) {
        #[cfg(windows)]
        self.thread.set(content);
        #[cfg(not(windows))]
        let _ = content;
    }

    /// Where the user moved or resized the overlay to, once per move.
    pub fn take_moved(&mut self) -> Option<OverlayPlace> {
        #[cfg(windows)]
        return self.thread.take_moved();
        #[cfg(not(windows))]
        None
    }
}
