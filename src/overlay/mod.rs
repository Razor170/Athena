//! Overlays: Athena windows drawn on top of the screen, over EVE.
//!
//! This holds the card overlay: one borderless, transparent, always-on-top,
//! click-through window that covers the cards of EVE-APM Preview. It draws
//! the Mining Supervisor's hold bars on the cards ("Card bars"), and, for
//! checking, a debug outline around each card it found (Settings ›
//! Diagnostics).
//!
//! - [`cards`]: what a card is and how it is told apart from other windows.
//! - [`bars`]: which card belongs to which character, and the bars' look.
//! - [`finder`]: the background thread that finds the cards, follows them
//!   and keeps the overlay window on top of them (Windows only).
//!
//! How it fits together: the app's `logic` calls
//! [`CardOverlay::keep_alive`] before every frame, which keeps the overlay
//! window alive while it is wanted. egui draws that window separately from
//! the main window (a "deferred viewport"), so it can repaint on its own,
//! also while the main window is minimized. The finder thread writes the
//! cards it sees into a shared [`CardsView`], shows, hides and moves the
//! window in physical pixels, and asks the overlay to repaint.

pub mod bars;
pub mod cards;
pub mod finder;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui;

use self::bars::CardBars;
use self::cards::{PixelRect, covered_area};
use self::finder::{CardFinder, CardsView};

/// The overlay window's title. The finder finds the window by it, so it
/// must be unique to Athena. Nobody sees it (no taskbar, no Alt+Tab entry).
const OVERLAY_TITLE: &str = "Athena card bars";

/// Colour of the debug outlines.
const OUTLINE_COLOR: egui::Color32 = egui::Color32::from_rgb(255, 214, 0);

/// Numbers about the overlay window, shown in Settings › Diagnostics so its
/// cost can be checked.
#[derive(Debug, Clone, Copy, Default)]
pub struct OverlayStats {
    /// How many times the overlay window was drawn.
    pub frames_drawn: u64,
    /// How long the last draw callback took (Athena's part, not the GPU's).
    pub last_draw: Duration,
}

/// What the overlay sees, for the modules' settings (for example "Found 7
/// cards · 5 matched to miners").
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OverlayStatus {
    /// Whether the overlay works on this system (Windows only).
    pub supported: bool,
    /// Whether the finder is running (something wants the overlay).
    pub active: bool,
    pub apm_running: bool,
    pub cards_found: usize,
    /// How many hold bars have a card to sit on.
    pub bars_matched: usize,
}

/// The card overlay, owned by the app.
pub struct CardOverlay {
    finder: CardFinder,
    viewport_id: egui::ViewportId,
    /// Shared with the draw callback, which runs whenever the overlay
    /// window repaints. `Arc` lets both hold it; the atomic and the mutex
    /// make it safe to use from both.
    debug_outlines: Arc<AtomicBool>,
    /// The hold bars to draw, `None` while "Card bars" is off.
    bars: Arc<Mutex<Option<CardBars>>>,
    stats: Arc<Mutex<OverlayStats>>,
}

impl CardOverlay {
    pub fn new(egui_ctx: &egui::Context) -> CardOverlay {
        let viewport_id = egui::ViewportId::from_hash_of("athena_card_overlay");
        CardOverlay {
            finder: CardFinder::start(egui_ctx, OVERLAY_TITLE, viewport_id),
            viewport_id,
            debug_outlines: Arc::new(AtomicBool::new(false)),
            bars: Arc::new(Mutex::new(None)),
            stats: Arc::new(Mutex::new(OverlayStats::default())),
        }
    }

    /// Whether the outlines are on.
    pub fn debug_outlines(&self) -> bool {
        self.debug_outlines.load(Ordering::Relaxed)
    }

    pub fn set_debug_outlines(&mut self, on: bool) {
        self.debug_outlines.store(on, Ordering::Relaxed);
    }

    /// What the finder sees right now.
    pub fn cards(&self) -> CardsView {
        self.finder.view()
    }

    pub fn stats(&self) -> OverlayStats {
        self.stats.lock().map(|s| *s).unwrap_or_default()
    }

    /// Whether finding cards works on this system (Windows only).
    pub fn is_supported() -> bool {
        CardFinder::is_supported()
    }

    /// Keeps the overlay window alive while something wants it. Called from
    /// the app's `logic`, which runs before every frame of the main window
    /// and also while it is minimized: egui closes a viewport that isn't
    /// shown for one frame.
    ///
    /// The window is made once, hidden, when it is first wanted (egui can
    /// only make windows while the main window is drawn, which it is when
    /// the outlines or the bars are switched on). From then on the finder
    /// thread shows, hides and moves it with plain Windows calls, so it can
    /// appear over new cards even while Athena is minimized, and never takes
    /// focus.
    ///
    /// `bars` are the Mining Supervisor's hold bars, `None` while its "Card
    /// bars" switch is off. Returns what the overlay sees, for Settings.
    pub fn keep_alive(&mut self, ctx: &egui::Context, bars: Option<CardBars>) -> OverlayStatus {
        let supported = CardOverlay::is_supported();
        let wanted = supported && (self.debug_outlines() || bars.is_some());
        self.finder.set_wanted(wanted);

        let view = self.finder.view();
        let status = OverlayStatus {
            supported,
            active: wanted,
            apm_running: view.apm_running,
            cards_found: view.cards.len(),
            bars_matched: bars
                .as_ref()
                .map_or(0, |b| bars::matched_count(&view.cards, b)),
        };

        // Hand new bars to the overlay. Only a real change repaints it, so
        // this costs nothing on frames where no hold changed.
        if let Ok(mut shown) = self.bars.lock()
            && *shown != bars
        {
            *shown = bars;
            ctx.request_repaint_of(self.viewport_id);
        }

        if !wanted {
            return status;
        }

        let builder = egui::ViewportBuilder::default()
            .with_title(OVERLAY_TITLE)
            // Hidden and tiny until the finder places it over the cards.
            .with_visible(false)
            .with_inner_size([1.0, 1.0])
            .with_decorations(false)
            .with_resizable(false)
            .with_transparent(true)
            .with_always_on_top()
            .with_mouse_passthrough(true)
            .with_taskbar(false)
            // Never take focus from EVE.
            .with_active(false);

        let view = self.finder.shared_view();
        let debug_outlines = self.debug_outlines.clone();
        let bars = self.bars.clone();
        let stats = self.stats.clone();
        // `move` hands the clones to the callback, which egui keeps and
        // calls whenever the overlay repaints, possibly later. That's why it
        // owns its data (`'static`) and shares it through `Arc`s.
        ctx.show_viewport_deferred(self.viewport_id, builder, move |ui, _class| {
            let started = Instant::now();
            // Copy the cards out so the lock isn't held while drawing.
            let cards = view.lock().map(|v| v.cards.clone()).unwrap_or_default();
            if debug_outlines.load(Ordering::Relaxed) {
                draw_outlines(ui, &cards);
            }
            if let Ok(bars) = bars.lock()
                && let Some(bars) = bars.as_ref()
            {
                draw_bars(ui, &cards, bars);
            }
            if let Ok(mut stats) = stats.lock() {
                stats.frames_drawn += 1;
                stats.last_draw = started.elapsed();
            }
        });
        status
    }
}

/// Draws each character's hold bar on their card.
fn draw_bars(ui: &mut egui::Ui, cards: &[cards::Card], card_bars: &CardBars) {
    let Some(area) = covered_area(cards) else {
        return;
    };
    let pixels_per_point = ui.ctx().pixels_per_point();
    let painter = ui.painter();
    let names: Vec<&str> = card_bars
        .bars
        .iter()
        .map(|b| b.character.as_str())
        .collect();
    for bar in &card_bars.bars {
        let Some(card) = bars::card_for(cards, &bar.character, &names) else {
            continue;
        };
        let pixels = bars::bar_rect(card.rect, card_bars.height_px);
        let rect = card_rect_in_overlay(pixels, area, pixels_per_point);
        bars::draw_bar(painter, rect, bar, card_bars);
    }
}

/// Draws a yellow frame and the window title on every card.
fn draw_outlines(ui: &mut egui::Ui, cards: &[cards::Card]) {
    let Some(area) = covered_area(cards) else {
        return;
    };
    let pixels_per_point = ui.ctx().pixels_per_point();
    let painter = ui.painter();
    for card in cards {
        let rect = card_rect_in_overlay(card.rect, area, pixels_per_point);
        painter.rect_stroke(
            rect,
            0.0,
            egui::Stroke::new(2.0, OUTLINE_COLOR),
            egui::StrokeKind::Inside,
        );
        painter.text(
            rect.left_top() + egui::vec2(5.0, 4.0),
            egui::Align2::LEFT_TOP,
            &card.title,
            egui::FontId::proportional(11.0),
            OUTLINE_COLOR,
        );
    }
}

/// A screen rectangle in physical pixels as egui points.
fn to_points(rect: PixelRect, pixels_per_point: f32) -> egui::Rect {
    egui::Rect::from_min_max(
        egui::pos2(rect.left as f32, rect.top as f32) / pixels_per_point,
        egui::pos2(rect.right as f32, rect.bottom as f32) / pixels_per_point,
    )
}

/// Where a card is inside the overlay window, which covers `area`, in the
/// overlay's points.
fn card_rect_in_overlay(card: PixelRect, area: PixelRect, pixels_per_point: f32) -> egui::Rect {
    let relative = PixelRect {
        left: card.left - area.left,
        top: card.top - area.top,
        right: card.right - area.left,
        bottom: card.bottom - area.top,
    };
    to_points(relative, pixels_per_point)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn card_rect_is_relative_to_the_overlay() {
        let area = PixelRect {
            left: 1000,
            top: 200,
            right: 1400,
            bottom: 400,
        };
        let card = PixelRect {
            left: 1200,
            top: 299,
            right: 1380,
            bottom: 400,
        };
        assert_eq!(
            card_rect_in_overlay(card, area, 1.0),
            egui::Rect::from_min_max(egui::pos2(200.0, 99.0), egui::pos2(380.0, 200.0))
        );
        // At 125% Windows scaling the same pixels are fewer points.
        assert_eq!(
            card_rect_in_overlay(card, area, 1.25),
            egui::Rect::from_min_max(egui::pos2(160.0, 79.2), egui::pos2(304.0, 160.0))
        );
    }
}
