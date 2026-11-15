//! The CIC Logi overlay: what it shows and how it looks. Follows the
//! approved mockup (<https://claude.ai/artifact/XJYh6fyFQ7cBNAhNvRdTkk>,
//! layout A).
//!
//! One small, always-on-top, click-through window with one line per pilot,
//! in the order of the Live cards:
//!
//! ```text
//! ▌Pilot name     [IN 2 ▲▼] [OUT 1 ▼] [DMG] [ECM 14] [SCRAM]
//! ```
//!
//! - The thin bar on the left is the ship type: blue shield, orange armor.
//! - Cap chain: green IN (cap coming in) and red OUT (cap going out), with
//!   the number of links and one arrow per link, like the card. Grey
//!   "IN 0 ✕" when nothing came in for the chain time. Ships without a cap
//!   chain leave this space empty.
//! - DMG: a chip and a dashed red frame, both blinking, while hit.
//! - ECM: a yellow chip with the seconds left, a yellow frame and a bar
//!   along the bottom that shrinks. DMG's red frame beats it.
//! - POINT (outlined) or SCRAM (solid red).
//!
//! Every chip has a fixed spot, so the eye learns where to look.
//!
//! This file holds only plain data and the drawing into a pixel buffer, so
//! it can be tested anywhere. The text is drawn by the window (Windows'
//! font rendering), from the list of [`TextRun`]s made here.

#![cfg_attr(not(windows), allow(dead_code))]

use eframe::egui::Color32;

use super::mining_bars::{MOVE_MIN_BACKGROUND_ALPHA, paint_dashed_frame};
use super::pixels::{Canvas, PixelRect, with_alpha};

/// Space between the window's edge and the lines, in pixels.
const PADDING_PX: i32 = 3;
/// One pilot's line.
const LINE_PX: i32 = 20;
const LINE_GAP_PX: i32 = 2;
/// The ship-type bar at the left of a line.
const STRIPE_PX: i32 = 3;
/// Space after the ship-type bar and after the name.
const NAME_GAP_PX: i32 = 6;
/// Names never get less room than this while they are shown.
const MIN_NAME_PX: i32 = 40;
const CHIP_PX: i32 = 16;
/// Space inside a chip, left and right of its text.
const CHIP_PAD_PX: i32 = 4;
/// Space between two chip spots.
const SLOT_GAP_PX: i32 = 4;
/// A painted arrow is 7 px wide; two of them are 9 px apart.
const ARROW_PX: i32 = 7;
const ARROW_STEP_PX: i32 = 9;
/// The most links a chain chip shows, like the card.
const MAX_LINKS: usize = 2;
/// The height of the chip text and of the names, in pixels.
pub const CHIP_TEXT_PX: i32 = 11;
pub const NAME_TEXT_PX: i32 = 12;
/// The narrowest and widest the overlay can be made, in pixels.
pub const MIN_WIDTH_PX: i32 = 40;
pub const MAX_WIDTH_PX: i32 = 1200;

// Colours, the same as the Logi cards and the mockup.
const BACKGROUND: Color32 = Color32::from_rgb(10, 13, 18);
const SHIELD: Color32 = Color32::from_rgb(0x7f, 0xb4, 0xff);
const ARMOR: Color32 = Color32::from_rgb(0xf0, 0x95, 0x4f);
const CAP: Color32 = Color32::from_rgb(0x5c, 0xbf, 0x6a);
const CAP_FILL: Color32 = Color32::from_rgb(0x1c, 0x2b, 0x1e);
const CAP_EDGE: Color32 = Color32::from_rgb(0x2f, 0x5a, 0x35);
const DMG: Color32 = Color32::from_rgb(0xe5, 0x53, 0x4b);
const DMG_DARK: Color32 = Color32::from_rgb(0x7a, 0x24, 0x21);
const SEND_FILL: Color32 = Color32::from_rgb(0x2e, 0x1c, 0x1b);
const SEND_EDGE: Color32 = Color32::from_rgb(0x6a, 0x2f, 0x2c);
const MISS: Color32 = Color32::from_rgb(0x6b, 0x73, 0x80);
const MISS_FILL: Color32 = Color32::from_rgb(0x22, 0x25, 0x2a);
const MISS_EDGE: Color32 = Color32::from_rgb(0x33, 0x3a, 0x44);
const ECM: Color32 = Color32::from_rgb(0xe8, 0xc5, 0x47);
const ECM_TEXT: Color32 = Color32::from_rgb(0x1a, 0x1a, 0x1a);
const TACKLE: Color32 = Color32::from_rgb(0xaa, 0x33, 0x33);
const POINT_FILL: Color32 = Color32::from_rgb(0x2e, 0x17, 0x16);
const POINT_TEXT: Color32 = Color32::from_rgb(0xff, 0x8a, 0x80);
const NAME: Color32 = Color32::from_rgb(0xd7, 0xdd, 0xe5);
const WHITE: Color32 = Color32::from_rgb(0xff, 0xff, 0xff);

/// Which way along the chain a link goes, as on the card's chip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkArrow {
    Up,
    Down,
    /// Both directions are the same module: ↕.
    Both,
    /// Not one of the chain modules: "?".
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tackle {
    Point,
    Scram,
}

/// The cap chain links that are up, up first. Empty means the chain is
/// missing on that side.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChainLinks {
    pub incoming: Vec<LinkArrow>,
    pub outgoing: Vec<LinkArrow>,
}

/// One pilot's line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogiLine {
    pub name: String,
    /// Shield logi (blue bar) or armor logi (orange bar).
    pub shield: bool,
    /// `None` for a ship that doesn't fly in a cap chain.
    pub chain: Option<ChainLinks>,
    /// While hit: `Some(true)` in the bright half of the blink.
    pub damage: Option<bool>,
    /// While jammed: whole seconds left, out of `ecm_secs`.
    pub ecm: Option<(u32, u32)>,
    pub tackle: Option<Tackle>,
}

impl LogiLine {
    /// Something is going on: hit, jammed, tackled, or a cap chain side is
    /// missing.
    pub fn is_busy(&self) -> bool {
        let chain_missing = self
            .chain
            .as_ref()
            .is_some_and(|c| c.incoming.is_empty() || c.outgoing.is_empty());
        self.damage.is_some() || self.ecm.is_some() || self.tackle.is_some() || chain_missing
    }
}

/// Everything the overlay window needs to draw itself.
#[derive(Debug, Clone, PartialEq)]
pub struct LogiOverlay {
    pub lines: Vec<LogiLine>,
    /// The saved top-left corner, `None` until the overlay was first moved.
    pub position: Option<[i32; 2]>,
    /// The width the user pulled it to. The names get what the chips leave.
    pub width_px: i32,
    pub show_names: bool,
    /// 0 (invisible) to 255 (solid).
    pub background_alpha: u8,
    /// While true the overlay takes clicks, so it can be dragged and its
    /// right edge pulled; otherwise clicks go through it.
    pub move_mode: bool,
}

impl LogiOverlay {
    pub fn clamped_width(&self) -> i32 {
        self.width_px.clamp(MIN_WIDTH_PX, MAX_WIDTH_PX)
    }
}

/// A piece of text for the window to draw: left-aligned in `rect`,
/// vertically centred, in `color`. Bold is the chip font, otherwise the
/// name font; `ellipsis` ends text that doesn't fit in "…".
#[derive(Debug, Clone, PartialEq)]
pub struct TextRun {
    pub rect: PixelRect,
    pub text: String,
    pub color: Color32,
    pub bold: bool,
    pub ellipsis: bool,
}

/// The overlay drawn, the text not yet included.
pub struct Picture {
    pub width: i32,
    pub height: i32,
    /// `0xAARRGGBB`, premultiplied, row by row from the top.
    pub pixels: Vec<u32>,
    pub texts: Vec<TextRun>,
}

/// How wide each chip spot is. They depend on the font, so they are
/// measured with `measure` (the width of a chip text in pixels).
struct Slots {
    chain_in: i32,
    chain_out: i32,
    damage: i32,
    ecm: i32,
    tackle: i32,
}

impl Slots {
    fn measure(measure: &dyn Fn(&str) -> i32) -> Slots {
        let widest = |texts: &[&str]| texts.iter().map(|t| measure(t)).max().unwrap_or(0);
        let chain = |label: &str| {
            let text = widest(&[&format!("{label} 0"), &format!("{label} 2")]);
            let arrows = MAX_LINKS as i32 * ARROW_STEP_PX - (ARROW_STEP_PX - ARROW_PX);
            CHIP_PAD_PX + text + 3 + arrows + CHIP_PAD_PX
        };
        Slots {
            chain_in: chain("IN"),
            chain_out: chain("OUT"),
            damage: 2 * CHIP_PAD_PX + measure("DMG"),
            ecm: 2 * CHIP_PAD_PX + widest(&["ECM 00", "ECM 60", "ECM 88"]),
            tackle: 2 * CHIP_PAD_PX + widest(&["POINT", "SCRAM"]),
        }
    }

    /// All chip spots, the gaps between them and a little room after.
    fn total(&self) -> i32 {
        self.chain_in
            + self.chain_out
            + self.damage
            + self.ecm
            + self.tackle
            + 4 * SLOT_GAP_PX
            + CHIP_PAD_PX
    }
}

/// From a line's left edge to where the name starts.
const NAME_LEFT_PX: i32 = 1 + STRIPE_PX + NAME_GAP_PX;

/// The overlay's width: what the user pulled it to, but at least room for
/// the chips and a short name. Without names it is just the chips.
fn picture_width(overlay: &LogiOverlay, slots: &Slots) -> i32 {
    let without_name = 2 * PADDING_PX + NAME_LEFT_PX + slots.total();
    if overlay.show_names {
        let least = without_name + MIN_NAME_PX + NAME_GAP_PX;
        overlay
            .clamped_width()
            .max(least)
            .min(MAX_WIDTH_PX.max(least))
    } else {
        without_name
    }
}

/// Draws the overlay. `measure` gives the width of a chip text in pixels
/// (Windows measures it with the chip font).
pub fn picture(overlay: &LogiOverlay, measure: &dyn Fn(&str) -> i32) -> Picture {
    let slots = Slots::measure(measure);
    let width = picture_width(overlay, &slots);
    let line_count = overlay.lines.len().max(1) as i32;
    let height = 2 * PADDING_PX + line_count * LINE_PX + (line_count - 1) * LINE_GAP_PX;

    let background_alpha = if overlay.move_mode {
        overlay.background_alpha.max(MOVE_MIN_BACKGROUND_ALPHA)
    } else {
        overlay.background_alpha
    };
    let mut canvas = Canvas::new(width, height, with_alpha(BACKGROUND, background_alpha));
    let mut texts = Vec::new();
    let name_width = width - 2 * PADDING_PX - NAME_LEFT_PX - slots.total() - NAME_GAP_PX;

    for (index, line) in overlay.lines.iter().enumerate() {
        let top = PADDING_PX + index as i32 * (LINE_PX + LINE_GAP_PX);
        let rect = PixelRect {
            left: PADDING_PX,
            top,
            right: width - PADDING_PX,
            bottom: top + LINE_PX,
        };
        let mut chips_left = rect.left + NAME_LEFT_PX;
        if overlay.show_names {
            texts.push(TextRun {
                rect: PixelRect {
                    left: chips_left,
                    top: rect.top,
                    right: chips_left + name_width,
                    bottom: rect.bottom,
                },
                text: line.name.clone(),
                color: NAME,
                bold: false,
                ellipsis: true,
            });
            chips_left += name_width + NAME_GAP_PX;
        }
        draw_line(
            &mut canvas,
            &mut texts,
            line,
            rect,
            chips_left,
            &slots,
            measure,
        );
    }

    if overlay.move_mode {
        paint_dashed_frame(canvas.pixels_mut(), width as usize, height as usize);
    }
    Picture {
        width,
        height,
        pixels: canvas.into_argb(),
        texts,
    }
}

/// One line: the ship-type bar, the frame, the chips and the ECM bar.
fn draw_line(
    canvas: &mut Canvas,
    texts: &mut Vec<TextRun>,
    line: &LogiLine,
    rect: PixelRect,
    chips_left: i32,
    slots: &Slots,
    measure: &dyn Fn(&str) -> i32,
) {
    let stripe = PixelRect {
        left: rect.left + 1,
        top: rect.top + 2,
        right: rect.left + 1 + STRIPE_PX,
        bottom: rect.bottom - 2,
    };
    canvas.fill_rect(stripe, if line.shield { SHIELD } else { ARMOR }, true);

    // The frame: blinking dashed red while hit (it beats ECM), else yellow
    // while jammed.
    if line.damage == Some(true) {
        canvas.dashed_rect(rect, DMG);
    } else if line.ecm.is_some() {
        canvas.stroke_rect(rect, ECM);
    }

    let mut chips = Chips {
        canvas,
        texts,
        measure,
        x: chips_left,
        top: rect.top + (LINE_PX - CHIP_PX) / 2,
    };
    match &line.chain {
        Some(chain) => {
            chips.chain(
                "IN",
                &chain.incoming,
                CAP,
                CAP_FILL,
                CAP_EDGE,
                slots.chain_in,
            );
            chips.chain(
                "OUT",
                &chain.outgoing,
                DMG,
                SEND_FILL,
                SEND_EDGE,
                slots.chain_out,
            );
        }
        None => chips.skip(slots.chain_in + SLOT_GAP_PX + slots.chain_out + SLOT_GAP_PX),
    }
    match line.damage {
        Some(bright) => {
            let fill = if bright { DMG } else { DMG_DARK };
            chips.chip("DMG", WHITE, fill, fill, slots.damage);
        }
        None => chips.skip(slots.damage + SLOT_GAP_PX),
    }
    match line.ecm {
        Some((left, _)) => chips.chip(&format!("ECM {left}"), ECM_TEXT, ECM, ECM, slots.ecm),
        None => chips.skip(slots.ecm + SLOT_GAP_PX),
    }
    match line.tackle {
        Some(Tackle::Point) => chips.chip("POINT", POINT_TEXT, POINT_FILL, TACKLE, slots.tackle),
        Some(Tackle::Scram) => chips.chip("SCRAM", WHITE, TACKLE, TACKLE, slots.tackle),
        None => {}
    }

    // The ECM countdown bar along the bottom, inside the frame.
    if let Some((left, total)) = line.ecm {
        let full = rect.right - 1 - stripe.right;
        let share = (left as f32 / total.max(1) as f32).clamp(0.0, 1.0);
        let bar = PixelRect {
            left: stripe.right,
            top: rect.bottom - 3,
            right: stripe.right + (full as f32 * share).round() as i32,
            bottom: rect.bottom - 1,
        };
        canvas.fill_rect(bar, ECM, false);
    }
}

/// Lays chips out from left to right, each in its fixed spot.
struct Chips<'a> {
    canvas: &'a mut Canvas,
    texts: &'a mut Vec<TextRun>,
    measure: &'a dyn Fn(&str) -> i32,
    /// Where the next spot starts.
    x: i32,
    top: i32,
}

impl Chips<'_> {
    fn skip(&mut self, width: i32) {
        self.x += width;
    }

    /// A chip as wide as its text, at the start of a spot `slot` wide.
    fn chip(&mut self, text: &str, color: Color32, fill: Color32, edge: Color32, slot: i32) {
        let text_width = (self.measure)(text);
        let rect = self.chip_rect(2 * CHIP_PAD_PX + text_width);
        self.canvas.fill_rect(rect, fill, true);
        self.canvas.stroke_rect(rect, edge);
        self.text(rect.left + CHIP_PAD_PX, text_width, text, color);
        self.skip(slot + SLOT_GAP_PX);
    }

    /// A cap chain chip: "IN 2" and one arrow per link, or a grey
    /// "IN 0 ✕" when the chain is missing on that side.
    fn chain(
        &mut self,
        label: &str,
        links: &[LinkArrow],
        color: Color32,
        fill: Color32,
        edge: Color32,
        slot: i32,
    ) {
        let links = &links[..links.len().min(MAX_LINKS)];
        let (color, fill, edge) = if links.is_empty() {
            (MISS, MISS_FILL, MISS_EDGE)
        } else {
            (color, fill, edge)
        };
        let text = format!("{label} {}", links.len());
        let text_width = (self.measure)(&text);
        // An empty chain gets the ✕, which is as wide as an arrow.
        let marks = links.len().max(1) as i32;
        let marks_width = marks * ARROW_STEP_PX - (ARROW_STEP_PX - ARROW_PX);
        let rect = self.chip_rect(CHIP_PAD_PX + text_width + 3 + marks_width + CHIP_PAD_PX);
        self.canvas.fill_rect(rect, fill, true);
        self.canvas.stroke_rect(rect, edge);
        self.text(rect.left + CHIP_PAD_PX, text_width, &text, color);

        let mut x = rect.left + CHIP_PAD_PX + text_width + 3;
        let middle = rect.top + CHIP_PX / 2;
        if links.is_empty() {
            self.canvas.cross(x, middle - 3, color);
        }
        for arrow in links {
            match arrow {
                LinkArrow::Up => self.canvas.triangle(x, middle - 2, true, color),
                LinkArrow::Down => self.canvas.triangle(x, middle - 2, false, color),
                LinkArrow::Both => {
                    self.canvas.triangle(x, middle - 5, true, color);
                    self.canvas.triangle(x, middle + 1, false, color);
                }
                LinkArrow::Unknown => {
                    let width = (self.measure)("?");
                    self.text(x + (ARROW_PX - width) / 2, width, "?", color);
                }
            }
            x += ARROW_STEP_PX;
        }
        self.skip(slot + SLOT_GAP_PX);
    }

    fn chip_rect(&self, width: i32) -> PixelRect {
        PixelRect {
            left: self.x,
            top: self.top,
            right: self.x + width,
            bottom: self.top + CHIP_PX,
        }
    }

    fn text(&mut self, left: i32, width: i32, text: &str, color: Color32) {
        self.texts.push(TextRun {
            // One pixel to spare, so Windows never cuts the last letter.
            rect: PixelRect {
                left,
                top: self.top,
                right: left + width + 1,
                bottom: self.top + CHIP_PX,
            },
            text: text.to_string(),
            color,
            bold: true,
            ellipsis: false,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::super::pixels::premultiplied_argb;
    use super::*;

    /// A stand-in for Windows' text measuring: 6 px a letter.
    fn measure(text: &str) -> i32 {
        6 * text.chars().count() as i32
    }

    fn line(name: &str) -> LogiLine {
        LogiLine {
            name: name.to_string(),
            shield: true,
            chain: Some(ChainLinks {
                incoming: vec![LinkArrow::Up, LinkArrow::Down],
                outgoing: vec![LinkArrow::Down],
            }),
            damage: None,
            ecm: None,
            tackle: None,
        }
    }

    fn overlay(lines: Vec<LogiLine>) -> LogiOverlay {
        LogiOverlay {
            lines,
            position: None,
            width_px: 360,
            show_names: true,
            background_alpha: 0,
            move_mode: false,
        }
    }

    fn pixel(p: &Picture, x: i32, y: i32) -> u32 {
        p.pixels[(y * p.width + x) as usize]
    }

    fn texts(p: &Picture) -> Vec<&str> {
        p.texts.iter().map(|t| t.text.as_str()).collect()
    }

    #[test]
    fn lines_stack_and_names_get_the_rest() {
        let p = picture(&overlay(vec![line("A"), line("B")]), &measure);
        assert_eq!(p.width, 360);
        assert_eq!(p.height, 3 + 20 + 2 + 20 + 3);
        assert_eq!(p.pixels.len(), (p.width * p.height) as usize);
        assert_eq!(texts(&p), ["A", "IN 2", "OUT 1", "B", "IN 2", "OUT 1"]);
        // The name starts after the ship bar and gets all the spare room.
        let name = p.texts[0].rect;
        assert_eq!(name.left, 3 + 1 + 3 + 6);
        let slots = Slots::measure(&measure);
        assert_eq!(name.right + 6 + slots.total() + 3, 360);
        assert!(p.texts[0].ellipsis && !p.texts[0].bold);
        // The ship bar is blue for shield.
        assert_eq!(pixel(&p, 5, 12), premultiplied_argb(SHIELD));
    }

    #[test]
    fn chips_keep_their_spots() {
        let mut busy = line("A");
        busy.damage = Some(true);
        busy.ecm = Some((14, 20));
        busy.tackle = Some(Tackle::Scram);
        let mut quiet = line("B");
        quiet.chain = None;
        quiet.tackle = Some(Tackle::Point);
        let p = picture(&overlay(vec![busy, quiet]), &measure);
        assert_eq!(
            texts(&p),
            ["A", "IN 2", "OUT 1", "DMG", "ECM 14", "SCRAM", "B", "POINT"]
        );
        // POINT sits right under SCRAM even though B has no other chips.
        let scram = p.texts[5].rect;
        let point = p.texts[7].rect;
        assert_eq!(scram.left, point.left);
        assert_eq!(point.top, scram.top + 22);
    }

    #[test]
    fn missing_chain_is_grey_with_zero() {
        let mut l = line("A");
        l.chain = Some(ChainLinks {
            incoming: Vec::new(),
            outgoing: vec![LinkArrow::Up, LinkArrow::Down, LinkArrow::Unknown],
        });
        let p = picture(&overlay(vec![l]), &measure);
        let chain: Vec<_> = p.texts[1..]
            .iter()
            .map(|t| (t.text.as_str(), t.color))
            .collect();
        // At most two links are shown, like the card.
        assert_eq!(chain, [("IN 0", MISS), ("OUT 2", DMG)]);
    }

    #[test]
    fn damage_frame_blinks_and_beats_ecm() {
        let mut l = line("A");
        l.ecm = Some((10, 20));
        l.damage = Some(true);
        let hit = picture(&overlay(vec![l.clone()]), &measure);
        // The frame's top edge starts with a dash: red, not yellow.
        assert_eq!(pixel(&hit, 4, 3), premultiplied_argb(DMG));
        l.damage = Some(false);
        let dark = picture(&overlay(vec![l]), &measure);
        assert_eq!(pixel(&dark, 4, 3), premultiplied_argb(ECM));
    }

    #[test]
    fn ecm_bar_shrinks() {
        let mut l = line("A");
        l.ecm = Some((20, 20));
        let full = picture(&overlay(vec![l.clone()]), &measure);
        l.ecm = Some((5, 20));
        let quarter = picture(&overlay(vec![l]), &measure);
        let bar_y = 3 + 20 - 2;
        let lit = |p: &Picture| {
            (0..p.width)
                .filter(|&x| pixel(p, x, bar_y) == premultiplied_argb(ECM))
                .count()
        };
        assert!(lit(&full) > 3 * lit(&quarter));
        assert!(lit(&quarter) > 0);
    }

    #[test]
    fn without_names_it_is_just_the_chips() {
        let mut o = overlay(vec![line("A")]);
        o.show_names = false;
        let p = picture(&o, &measure);
        let slots = Slots::measure(&measure);
        assert_eq!(p.width, 2 * 3 + 1 + 3 + 6 + slots.total());
        assert_eq!(texts(&p), ["IN 2", "OUT 1"]);
    }

    #[test]
    fn too_narrow_still_fits_the_chips() {
        let mut o = overlay(vec![line("A")]);
        o.width_px = 50;
        let p = picture(&o, &measure);
        assert_eq!(p.texts[0].rect.width(), MIN_NAME_PX);
    }

    #[test]
    fn empty_overlay_can_still_be_grabbed() {
        let mut o = overlay(Vec::new());
        o.move_mode = true;
        let p = picture(&o, &measure);
        assert_eq!(p.height, 3 + 20 + 3);
        assert!(p.pixels.iter().all(|px| px.to_be_bytes()[0] > 0));
    }

    #[test]
    fn busy_lines() {
        let mut l = line("A");
        assert!(!l.is_busy());
        if let Some(c) = l.chain.as_mut() {
            c.outgoing.clear();
        }
        assert!(l.is_busy());
        l.chain = None;
        assert!(!l.is_busy());
        l.tackle = Some(Tackle::Point);
        assert!(l.is_busy());
    }
}
