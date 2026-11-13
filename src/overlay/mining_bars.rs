//! The Mining Supervisor's hold bars overlay: what it shows and how it
//! looks.
//!
//! The overlay is one small, always-on-top, click-through window with one
//! hold bar per character, stacked from top to bottom. Under each bar there
//! can be a line with the character's name on the left and the hold's fill
//! in percent on the right, with the compressed share in purple just left
//! of it. The background and the bars each have their
//! own opacity.
//!
//! This file holds only plain data and the drawing into a pixel buffer, so
//! it can be tested anywhere. The window itself is in `bars_window.rs`
//! (Windows only), which also draws the text, because that needs Windows'
//! font rendering.

// The drawing part is used only by the Windows window; on other systems
// (the Linux sandbox) it is still built and tested.
#![cfg_attr(not(windows), allow(dead_code))]

use eframe::egui::Color32;

/// Space between the window's edge and the bars, in pixels.
pub const PADDING_PX: i32 = 3;
/// Space between one character's row and the next, in pixels.
pub const ROW_GAP_PX: i32 = 2;
/// Height of the text line under a bar (name and percent), in pixels.
pub const TEXT_LINE_PX: i32 = 13;
/// Height of the text itself, in pixels.
pub const TEXT_HEIGHT_PX: i32 = 10;
/// The narrowest and widest the overlay can be made, in pixels.
pub const MIN_WIDTH_PX: i32 = 40;
pub const MAX_WIDTH_PX: i32 = 1200;
/// Where the overlay first opens, before it was ever moved.
pub const DEFAULT_POSITION: [i32; 2] = [100, 100];

/// The background colour, before its opacity is applied.
const BACKGROUND: Color32 = Color32::from_rgb(18, 18, 18);
/// Behind the coloured part of a bar.
const BAR_BACK: Color32 = Color32::from_rgb(0x33, 0x33, 0x33);
/// The alert-level mark.
const MARK: Color32 = Color32::from_rgb(0xf0, 0xf0, 0xf0);
/// The alert-level mark's width in pixels.
const MARK_WIDTH_PX: usize = 2;
/// The colour of the names and percentages.
pub const TEXT: Color32 = Color32::from_rgb(0xf0, 0xf0, 0xf0);
/// The dashed frame shown while the overlay is being moved.
const MOVE_FRAME: Color32 = Color32::from_rgb(0xa3, 0x7e, 0xf0);
/// While the overlay is being moved its background is at least this opaque
/// (0-255): Windows lets clicks through fully see-through pixels, so the
/// overlay couldn't be grabbed otherwise.
const MOVE_MIN_BACKGROUND_ALPHA: u8 = 110;

/// A rectangle in pixels, `right` and `bottom` exclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl PixelRect {
    pub fn width(&self) -> i32 {
        self.right - self.left
    }

    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }
}

/// One character's hold bar, as fractions of the bar's width (0.0 to 1.0).
#[derive(Debug, Clone, PartialEq)]
pub struct HoldBar {
    /// Shown under the bar when names are on.
    pub character: String,
    /// Compressed ore, drawn first in purple.
    pub compressed: f32,
    /// The rest of the hold, drawn after it in `color`.
    pub uncompressed: f32,
    /// Green, yellow from the alert level, red when full.
    pub color: Color32,
    /// Where the white alert-level mark goes, if the hold size is known.
    pub alert_mark: Option<f32>,
    /// How full the hold is, in percent, if the hold size is known.
    pub percent: Option<u32>,
    /// How much of the hold is compressed ore, in percent. `None` when the
    /// hold size is unknown or there is no compressed ore.
    pub compressed_percent: Option<u32>,
}

/// Where the user put the overlay: its top-left corner and its width, in
/// screen pixels. Reported back when a move ends, so it can be saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverlayPlace {
    pub x: i32,
    pub y: i32,
    pub width: i32,
}

/// Everything the overlay window needs to draw itself.
#[derive(Debug, Clone, PartialEq)]
pub struct BarsOverlay {
    pub bars: Vec<HoldBar>,
    /// The saved top-left corner, `None` until the overlay was first moved.
    pub position: Option<[i32; 2]>,
    pub width_px: i32,
    pub bar_height_px: i32,
    pub show_names: bool,
    pub show_percent: bool,
    /// Show the compressed share of the hold, in purple, left of the hold %.
    pub show_compressed_percent: bool,
    /// 0 (invisible) to 255 (solid).
    pub background_alpha: u8,
    /// 0 (invisible) to 255 (solid). Also used for the text.
    pub bar_alpha: u8,
    /// The purple for compressed ore.
    pub compressed_color: Color32,
    /// While true the overlay takes clicks, so it can be dragged and its
    /// right edge pulled; otherwise clicks go through it.
    pub move_mode: bool,
}

impl BarsOverlay {
    /// Whether there is a text line under each bar.
    pub fn has_text_line(&self) -> bool {
        self.show_names || self.show_percent || self.show_compressed_percent
    }

    /// The width the window gets, kept within sensible limits.
    pub fn clamped_width(&self) -> i32 {
        self.width_px.clamp(MIN_WIDTH_PX, MAX_WIDTH_PX)
    }
}

/// Where one character's bar and text line go inside the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    pub bar: PixelRect,
    /// `None` while names and percentages are both off.
    pub text: Option<PixelRect>,
}

/// Lays out the rows for a window `width` pixels wide. Returns the rows and
/// the window's height. With no bars the window is one empty row high, so
/// there is still something to grab in move mode.
pub fn layout(overlay: &BarsOverlay, width: i32) -> (Vec<Row>, i32) {
    let bar_height = overlay.bar_height_px.max(1);
    let text_height = if overlay.has_text_line() {
        TEXT_LINE_PX
    } else {
        0
    };
    let inner_left = PADDING_PX;
    let inner_right = (width - PADDING_PX).max(inner_left + 1);
    let mut rows = Vec::with_capacity(overlay.bars.len());
    let mut y = PADDING_PX;
    for (index, _) in overlay.bars.iter().enumerate() {
        if index > 0 {
            y += ROW_GAP_PX;
        }
        let bar = PixelRect {
            left: inner_left,
            top: y,
            right: inner_right,
            bottom: y + bar_height,
        };
        y += bar_height;
        let text = (text_height > 0).then(|| {
            let line = PixelRect {
                left: inner_left,
                top: y,
                right: inner_right,
                bottom: y + text_height,
            };
            y += text_height;
            line
        });
        rows.push(Row { bar, text });
    }
    if rows.is_empty() {
        y += bar_height + text_height;
    }
    (rows, y + PADDING_PX)
}

/// The overlay as pixels, `width` × `height`, row by row from the top, the
/// text not yet included. Each pixel is `0xAARRGGBB` with the colour already
/// multiplied by its alpha, which is what Windows' `UpdateLayeredWindow`
/// wants (and what egui's `Color32` stores anyway).
pub fn paint(overlay: &BarsOverlay, rows: &[Row], width: i32, height: i32) -> Vec<u32> {
    let (width, height) = (width.max(1) as usize, height.max(1) as usize);
    let background_alpha = if overlay.move_mode {
        overlay.background_alpha.max(MOVE_MIN_BACKGROUND_ALPHA)
    } else {
        overlay.background_alpha
    };
    let background = with_alpha(BACKGROUND, background_alpha);
    let mut pixels = vec![background; width * height];

    for (row, bar) in rows.iter().zip(&overlay.bars) {
        paint_bar(
            &mut pixels,
            width,
            row.bar,
            bar,
            overlay.compressed_color,
            overlay.bar_alpha,
        );
    }
    if overlay.move_mode {
        paint_dashed_frame(&mut pixels, width, height);
    }
    pixels.into_iter().map(premultiplied_argb).collect()
}

/// Draws one bar into `pixels` (a buffer `width` wide): compressed ore,
/// then the rest of the hold in the bar's colour, then the dark back, with
/// a white mark at the alert level. The four corner pixels are left out, so
/// the bar looks slightly rounded.
fn paint_bar(
    pixels: &mut [Color32],
    width: usize,
    rect: PixelRect,
    bar: &HoldBar,
    compressed_color: Color32,
    alpha: u8,
) {
    let bar_width = rect.width().max(0) as usize;
    let bar_height = rect.height().max(0) as usize;
    let part_width = |fraction: f32| (fraction.clamp(0.0, 1.0) * bar_width as f32).round() as usize;
    let compressed_end = part_width(bar.compressed).min(bar_width);
    let uncompressed_end = (compressed_end + part_width(bar.uncompressed)).min(bar_width);
    // The mark is centred on its spot and kept inside the bar.
    let mark_columns = bar.alert_mark.map(|mark| {
        let centre = part_width(mark);
        let first = centre
            .saturating_sub(MARK_WIDTH_PX / 2)
            .min(bar_width.saturating_sub(MARK_WIDTH_PX));
        first..first + MARK_WIDTH_PX
    });

    for y in 0..bar_height {
        for x in 0..bar_width {
            let corner = (x == 0 || x + 1 == bar_width) && (y == 0 || y + 1 == bar_height);
            if corner {
                continue;
            }
            let color = if mark_columns.as_ref().is_some_and(|m| m.contains(&x)) {
                MARK
            } else if x < compressed_end {
                compressed_color
            } else if x < uncompressed_end {
                bar.color
            } else {
                BAR_BACK
            };
            let (px, py) = (rect.left as usize + x, rect.top as usize + y);
            if let Some(pixel) = pixels.get_mut(py * width + px) {
                *pixel = over(with_alpha(color, alpha), *pixel);
            }
        }
    }
}

/// A 1 px dashed purple frame around the whole window (move mode).
fn paint_dashed_frame(pixels: &mut [Color32], width: usize, height: usize) {
    // 4 px dash, 3 px gap.
    let dash = |i: usize| i % 7 < 4;
    for x in 0..width {
        if dash(x) {
            pixels[x] = MOVE_FRAME;
            pixels[(height - 1) * width + x] = MOVE_FRAME;
        }
    }
    for y in 0..height {
        if dash(y) {
            pixels[y * width] = MOVE_FRAME;
            pixels[y * width + width - 1] = MOVE_FRAME;
        }
    }
    // The right edge is the one to pull: a solid grip in its middle.
    let grip = 12.min(height);
    let first = (height - grip) / 2;
    for y in first..first + grip {
        for x in width.saturating_sub(3)..width {
            pixels[y * width + x] = MOVE_FRAME;
        }
    }
}

/// Draws text into `pixels` (already in `0xAARRGGBB`). `coverage` holds one
/// value per pixel, 0 where there is no text and 255 where it is solid, as
/// Windows drew it. The text gets `color` at `alpha` opacity.
pub fn apply_text(pixels: &mut [u32], coverage: &[u8], color: Color32, alpha: u8) {
    for (pixel, &cover) in pixels.iter_mut().zip(coverage) {
        if cover == 0 {
            continue;
        }
        let text_alpha = ((cover as u32 * alpha as u32 + 127) / 255) as u8;
        let below = from_premultiplied_argb(*pixel);
        *pixel = premultiplied_argb(over(with_alpha(color, text_alpha), below));
    }
}

/// `color` (taken as solid) at `alpha` opacity, premultiplied.
fn with_alpha(color: Color32, alpha: u8) -> Color32 {
    let [r, g, b, _] = color.to_array();
    Color32::from_rgba_unmultiplied(r, g, b, alpha)
}

/// `top` drawn over `below`, both premultiplied ("source over").
fn over(top: Color32, below: Color32) -> Color32 {
    let [tr, tg, tb, ta] = top.to_array();
    let [br, bg, bb, ba] = below.to_array();
    let rest = 255 - ta as u32;
    let mix = |t: u8, b: u8| (t as u32 + (b as u32 * rest + 127) / 255).min(255) as u8;
    Color32::from_rgba_premultiplied(mix(tr, br), mix(tg, bg), mix(tb, bb), mix(ta, ba))
}

/// `Color32` (already premultiplied) as `0xAARRGGBB`.
fn premultiplied_argb(color: Color32) -> u32 {
    let [r, g, b, a] = color.to_array();
    u32::from_be_bytes([a, r, g, b])
}

fn from_premultiplied_argb(pixel: u32) -> Color32 {
    let [a, r, g, b] = pixel.to_be_bytes();
    Color32::from_rgba_premultiplied(r, g, b, a)
}

/// The percent text shown under a bar, for example "64%".
pub fn percent_text(bar: &HoldBar) -> Option<String> {
    bar.percent.map(|p| format!("{p}%"))
}

/// The purple compressed-ore percent shown left of the hold percent.
pub fn compressed_percent_text(bar: &HoldBar) -> Option<String> {
    bar.compressed_percent.map(|p| format!("{p}%"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar(name: &str) -> HoldBar {
        HoldBar {
            character: name.to_string(),
            compressed: 0.2,
            uncompressed: 0.3,
            color: Color32::GREEN,
            alert_mark: Some(0.7),
            percent: Some(50),
            compressed_percent: Some(20),
        }
    }

    fn overlay(bars: Vec<HoldBar>) -> BarsOverlay {
        BarsOverlay {
            bars,
            position: None,
            width_px: 16,
            bar_height_px: 3,
            show_names: true,
            show_percent: true,
            show_compressed_percent: true,
            background_alpha: 0,
            bar_alpha: 255,
            compressed_color: Color32::from_rgb(0x80, 0x40, 0xc0),
            move_mode: false,
        }
    }

    #[test]
    fn rows_stack_tightly() {
        let o = overlay(vec![bar("A"), bar("B")]);
        let (rows, height) = layout(&o, 100);
        assert_eq!(rows.len(), 2);
        // Padding 3, bar 3, text 13, gap 2, bar 3, text 13, padding 3.
        assert_eq!(rows[0].bar.top, 3);
        assert_eq!(rows[0].text.map(|t| t.top), Some(6));
        assert_eq!(rows[1].bar.top, 21);
        assert_eq!(height, 3 + 3 + 13 + 2 + 3 + 13 + 3);
        assert_eq!(rows[0].bar.left, 3);
        assert_eq!(rows[0].bar.right, 97);
    }

    #[test]
    fn no_text_line_when_names_and_percent_are_off() {
        let mut o = overlay(vec![bar("A"), bar("B")]);
        o.show_names = false;
        o.show_percent = false;
        o.show_compressed_percent = false;
        let (rows, height) = layout(&o, 100);
        assert!(rows.iter().all(|r| r.text.is_none()));
        assert_eq!(height, 3 + 3 + 2 + 3 + 3);
        // Only the percent: the line stays.
        o.show_percent = true;
        let (rows, _) = layout(&o, 100);
        assert!(rows[0].text.is_some());
        // Only the compressed percent: the line stays too.
        o.show_percent = false;
        o.show_compressed_percent = true;
        let (rows, _) = layout(&o, 100);
        assert!(rows[0].text.is_some());
    }

    #[test]
    fn empty_overlay_is_one_row_high() {
        let o = overlay(Vec::new());
        let (rows, height) = layout(&o, 100);
        assert!(rows.is_empty());
        assert_eq!(height, 3 + 3 + 13 + 3);
    }

    #[test]
    fn bar_follows_the_live_bar() {
        let mut o = overlay(vec![bar("A")]);
        o.show_names = false;
        o.show_percent = false;
        o.show_compressed_percent = false;
        let (rows, height) = layout(&o, 16);
        let pixels = paint(&o, &rows, 16, height);
        assert_eq!(pixels.len(), 16 * height as usize);
        // The bar is 10 px wide from x = 3; its middle row is y = 4.
        let row = &pixels[4 * 16..5 * 16];
        let purple = premultiplied_argb(o.compressed_color);
        let green = premultiplied_argb(Color32::GREEN);
        assert_eq!(row[0], 0, "background is see-through at 0 opacity");
        assert_eq!(row[3], purple);
        assert_eq!(row[4], purple);
        assert_eq!(row[5], green);
        assert_eq!(row[7], green);
        assert_eq!(row[8], premultiplied_argb(BAR_BACK));
        // The mark: two columns around 7 (x = 9 and 10).
        assert_eq!(row[9], premultiplied_argb(MARK));
        assert_eq!(row[10], premultiplied_argb(MARK));
        assert_eq!(row[11], premultiplied_argb(BAR_BACK));
        // The bar's corners show the background.
        assert_eq!(pixels[3 * 16 + 3], 0);
        assert_eq!(pixels[3 * 16 + 4], purple);
    }

    #[test]
    fn opacities_apply() {
        let mut o = overlay(vec![bar("A")]);
        o.background_alpha = 128;
        o.bar_alpha = 128;
        let (rows, height) = layout(&o, 16);
        let pixels = paint(&o, &rows, 16, height);
        let [a, ..] = pixels[0].to_be_bytes();
        assert_eq!(a, 128);
        // A half see-through bar over a half see-through background.
        let [a, ..] = pixels[4 * 16 + 5].to_be_bytes();
        assert_eq!(a as u32, 128 + (128 * 127 + 127) / 255);
    }

    #[test]
    fn move_mode_can_be_grabbed() {
        let mut o = overlay(vec![bar("A")]);
        o.move_mode = true;
        let (rows, height) = layout(&o, 16);
        let pixels = paint(&o, &rows, 16, height);
        // No fully see-through pixel: Windows would let the click through.
        assert!(pixels.iter().all(|p| p.to_be_bytes()[0] > 0));
        assert_eq!(pixels[0], premultiplied_argb(MOVE_FRAME));
    }

    #[test]
    fn text_blends_over_the_background() {
        let mut pixels = vec![0u32, premultiplied_argb(Color32::from_rgb(10, 10, 10))];
        apply_text(&mut pixels, &[255, 0], TEXT, 255);
        assert_eq!(pixels[0], premultiplied_argb(TEXT));
        assert_eq!(pixels[1], premultiplied_argb(Color32::from_rgb(10, 10, 10)));
        let mut half = vec![0u32];
        apply_text(&mut half, &[255], Color32::WHITE, 128);
        assert_eq!(half[0], 0x8080_8080);
    }

    #[test]
    fn width_is_kept_in_limits() {
        let mut o = overlay(Vec::new());
        o.width_px = 5;
        assert_eq!(o.clamped_width(), MIN_WIDTH_PX);
        o.width_px = 99_999;
        assert_eq!(o.clamped_width(), MAX_WIDTH_PX);
    }

    #[test]
    fn percent_text_reads_well() {
        assert_eq!(percent_text(&bar("A")).as_deref(), Some("50%"));
        let mut unknown = bar("A");
        unknown.percent = None;
        assert_eq!(percent_text(&unknown), None);
        assert_eq!(compressed_percent_text(&bar("A")).as_deref(), Some("20%"));
        unknown.compressed_percent = None;
        assert_eq!(compressed_percent_text(&unknown), None);
    }
}
