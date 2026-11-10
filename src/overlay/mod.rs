//! Overlays: Athena windows drawn on top of the screen, over EVE.
//!
//! - [`mining_bars`]: the Mining Supervisor's hold bars overlay, what it
//!   shows and how it looks.
//! - `bars_window`: the window that shows it, run by its own thread
//!   (Windows only).
//! - [`popup`]: the Intel Supervisor's alert popup.
//!
//! How the bars overlay fits together: the app's `logic` calls
//! [`MiningOverlay::show`] before every frame with what the Mining
//! Supervisor wants shown. Only a real change wakes the overlay thread,
//! which then redraws the window. When the user moved the overlay, the app
//! hands the new place back to the module with
//! [`MiningOverlay::take_moved`], so it is saved.

#[cfg(windows)]
mod bars_window;
pub mod mining_bars;
pub mod popup;

use eframe::egui;

use self::mining_bars::{BarsOverlay, OverlayPlace};

/// Whether the overlays work on this system (Windows only).
pub fn is_supported() -> bool {
    cfg!(windows)
}

/// The mining bars overlay, owned by the app.
pub struct MiningOverlay {
    #[cfg(windows)]
    window: bars_window::BarsWindow,
}

impl MiningOverlay {
    pub fn new(egui_ctx: &egui::Context) -> MiningOverlay {
        #[cfg(windows)]
        return MiningOverlay {
            window: bars_window::BarsWindow::start(egui_ctx),
        };
        #[cfg(not(windows))]
        {
            let _ = egui_ctx;
            MiningOverlay {}
        }
    }

    /// Shows `overlay`, or no overlay for `None`. Cheap to call every
    /// frame: the window is only redrawn when something changed.
    pub fn show(&mut self, overlay: Option<BarsOverlay>) {
        #[cfg(windows)]
        self.window.set(overlay);
        #[cfg(not(windows))]
        let _ = overlay;
    }

    /// Where the user moved or resized the overlay to, once per move.
    pub fn take_moved(&mut self) -> Option<OverlayPlace> {
        #[cfg(windows)]
        return self.window.take_moved();
        #[cfg(not(windows))]
        None
    }
}
