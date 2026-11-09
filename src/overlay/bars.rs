//! The Mining Supervisor's hold bars, drawn over EVE-APM Preview's cards.
//!
//! The Mining Supervisor hands over one [`CardBar`] per tracked character
//! (see `Module::card_bars`); this file works out where on a card the bar
//! goes and draws it, in the same look as the bar in the Mining panel.

use eframe::egui::{self, Color32, CornerRadius, Stroke};

use super::cards::{Card, PixelRect};

/// The bar starts this far inside the card's left edge, in pixels.
const LEFT_INSET_PX: i32 = 3;
/// The bar's bottom edge sits this far above the card's bottom, in pixels.
const BOTTOM_INSET_PX: i32 = 4;
/// Behind the coloured part, slightly see-through so the card shows a little.
const BAR_BACK: Color32 = Color32::from_rgba_premultiplied(43, 43, 43, 217);
/// The alert-level mark.
const MARK: Color32 = Color32::from_rgb(0xf0, 0xf0, 0xf0);

/// One character's hold bar, as fractions of the bar's width (0.0 to 1.0).
#[derive(Debug, Clone, PartialEq)]
pub struct CardBar {
    /// The character's name, matched against the card titles.
    pub character: String,
    /// Compressed ore, drawn first in purple.
    pub compressed: f32,
    /// The rest of the hold, drawn after it in `color`.
    pub uncompressed: f32,
    /// Green, yellow from the alert level, red when full.
    pub color: Color32,
    /// Where the white alert-level mark goes, if the hold size is known.
    pub alert_mark: Option<f32>,
}

/// Everything the overlay needs to draw the bars.
#[derive(Debug, Clone, PartialEq)]
pub struct CardBars {
    pub bars: Vec<CardBar>,
    /// Every character name the module knows, including those without a
    /// bar (hidden ones), so a shorter name never takes a longer name's
    /// card just because the longer one has no bar.
    pub all_names: Vec<String>,
    /// The bar's height in pixels (6 unless changed in Settings).
    pub height_px: f32,
    /// The purple for compressed ore.
    pub compressed_color: Color32,
}

/// The card that belongs to `character`, if any.
///
/// APM can rename cards (Kevin's profile uses names like
/// `Noa Askari [Mining]`), so a card belongs to a character when its title
/// *contains* the name, ignoring upper and lower case. When several of
/// `all_characters` fit one card (say "Noa" and "Noa Askari"), the longest
/// name wins the card, so "Noa" doesn't get Noa Askari's card. Of two
/// names of the same length, the one earlier in the title wins.
pub fn card_for<'a>(
    cards: &'a [Card],
    character: &str,
    all_characters: &[&str],
) -> Option<&'a Card> {
    let wanted = character.to_lowercase();
    if wanted.is_empty() {
        return None;
    }
    cards.iter().find(|card| {
        let title = card.title.to_lowercase();
        let Some(at) = title.find(&wanted) else {
            return false;
        };
        // Another name that also fits this card and beats this one takes it.
        !all_characters.iter().any(|other| {
            let other = other.to_lowercase();
            if other == wanted {
                return false;
            }
            match title.find(&other) {
                Some(other_at) => {
                    other.len() > wanted.len() || (other.len() == wanted.len() && other_at < at)
                }
                None => false,
            }
        })
    })
}

/// How many of the bars have a card to sit on.
pub fn matched_count(cards: &[Card], bars: &CardBars) -> usize {
    let names: Vec<&str> = bars.all_names.iter().map(String::as_str).collect();
    bars.bars
        .iter()
        .filter(|bar| card_for(cards, &bar.character, &names).is_some())
        .count()
}

/// Where a bar goes on a card, in screen pixels: from 3 px inside the left
/// edge to half the card's width, `height_px` high, 4 px above the bottom.
/// That keeps it clear of APM's system name in the bottom-right corner.
pub fn bar_rect(card: PixelRect, height_px: f32) -> PixelRect {
    let height = (height_px.round() as i32).max(1);
    let bottom = card.bottom - BOTTOM_INSET_PX;
    PixelRect {
        left: card.left + LEFT_INSET_PX,
        top: bottom - height,
        right: card.left + card.width() / 2,
        bottom,
    }
}

/// Draws one bar into `rect` (already in the overlay's points).
pub fn draw_bar(painter: &egui::Painter, rect: egui::Rect, bar: &CardBar, bars: &CardBars) {
    painter.rect_filled(rect, CornerRadius::same(2), BAR_BACK);
    let width = rect.width();
    let compressed_w = bar.compressed.clamp(0.0, 1.0) * width;
    let uncompressed_w = (bar.uncompressed.clamp(0.0, 1.0) * width).min(width - compressed_w);
    let part = |x: f32, w: f32| {
        egui::Rect::from_min_size(egui::pos2(x, rect.top()), egui::vec2(w, rect.height()))
    };
    if compressed_w > 0.0 {
        painter.rect_filled(
            part(rect.left(), compressed_w),
            CornerRadius::ZERO,
            bars.compressed_color,
        );
    }
    if uncompressed_w > 0.0 {
        painter.rect_filled(
            part(rect.left() + compressed_w, uncompressed_w),
            CornerRadius::ZERO,
            bar.color,
        );
    }
    if let Some(mark) = bar.alert_mark {
        let x = rect.left() + mark.clamp(0.0, 1.0) * width;
        painter.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            Stroke::new(2.0, MARK),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(title: &str, left: i32, top: i32) -> Card {
        Card {
            title: title.to_string(),
            rect: PixelRect {
                left,
                top,
                right: left + 180,
                bottom: top + 101,
            },
        }
    }

    fn bar(name: &str) -> CardBar {
        CardBar {
            character: name.to_string(),
            compressed: 0.2,
            uncompressed: 0.3,
            color: Color32::GREEN,
            alert_mark: Some(0.7),
        }
    }

    #[test]
    fn card_title_contains_the_name() {
        let cards = vec![card("EVE - Noa Askari [Mining]", 0, 0)];
        let all = ["Noa Askari"];
        assert!(card_for(&cards, "Noa Askari", &all).is_some());
        assert!(card_for(&cards, "noa askari", &all).is_some());
        assert!(card_for(&cards, "Ria Tolvo", &all).is_none());
        assert!(card_for(&cards, "", &all).is_none());
    }

    #[test]
    fn longest_name_wins_a_card() {
        let cards = vec![
            card("EVE - Noa Askari [Mining]", 0, 0),
            card("EVE - Noa", 200, 0),
        ];
        let all = ["Noa", "Noa Askari"];
        assert_eq!(
            card_for(&cards, "Noa Askari", &all).map(|c| c.rect.left),
            Some(0)
        );
        assert_eq!(
            card_for(&cards, "Noa", &all).map(|c| c.rect.left),
            Some(200)
        );
    }

    #[test]
    fn a_name_without_a_bar_still_wins_its_card() {
        // "Noa Askari" is hidden (no bar) but known: "Noa" must not take
        // the "Noa Askari [Mining]" card.
        let cards = vec![card("EVE - Noa Askari [Mining]", 0, 0)];
        let bars = CardBars {
            bars: vec![bar("Noa")],
            all_names: vec!["Noa".into(), "Noa Askari".into()],
            height_px: 6.0,
            compressed_color: Color32::BLUE,
        };
        assert_eq!(matched_count(&cards, &bars), 0);
    }

    #[test]
    fn same_length_names_go_by_position() {
        let cards = vec![card("EVE - Ann Bob", 0, 0)];
        let all = ["Ann", "Bob"];
        assert!(card_for(&cards, "Ann", &all).is_some());
        assert!(card_for(&cards, "Bob", &all).is_none());
    }

    #[test]
    fn matched_count_skips_characters_without_a_card() {
        let cards = vec![card("EVE - Noa Askari [Mining]", 0, 0)];
        let bars = CardBars {
            bars: vec![bar("Noa Askari"), bar("Ria Tolvo")],
            all_names: vec!["Noa Askari".into(), "Ria Tolvo".into()],
            height_px: 6.0,
            compressed_color: Color32::BLUE,
        };
        assert_eq!(matched_count(&cards, &bars), 1);
    }

    #[test]
    fn bar_sits_in_the_bottom_left_corner() {
        // A 180 x 101 card: the bar is 87 x 6 px, 4 px above the bottom.
        let rect = bar_rect(card("EVE - A", 315, 0).rect, 6.0);
        assert_eq!(
            rect,
            PixelRect {
                left: 318,
                top: 91,
                right: 405,
                bottom: 97,
            }
        );
        assert_eq!(rect.width(), 87);
    }
}
