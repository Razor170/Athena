//! Drawing into a plain pixel buffer, for the overlay windows.
//!
//! The overlays are Windows "layered" windows: Athena hands Windows a
//! finished picture, one `0xAARRGGBB` number per pixel with the colour
//! already multiplied by its alpha ("premultiplied"). The helpers here
//! build such pictures. They are plain Rust, so they are tested anywhere;
//! only the text comes from Windows (see `window.rs`).

// Used only by the Windows window; on other systems (the Linux sandbox)
// still built and tested.
#![cfg_attr(not(windows), allow(dead_code))]

use eframe::egui::Color32;

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

/// A picture being drawn: `width` × `height` pixels, row by row from the
/// top, each a premultiplied `Color32`.
pub struct Canvas {
    pub width: usize,
    pub height: usize,
    pixels: Vec<Color32>,
}

impl Canvas {
    /// A canvas filled with `background`.
    pub fn new(width: i32, height: i32, background: Color32) -> Canvas {
        let (width, height) = (width.max(1) as usize, height.max(1) as usize);
        Canvas {
            width,
            height,
            pixels: vec![background; width * height],
        }
    }

    /// Draws `color` over the pixel at (`x`, `y`). Pixels off the canvas
    /// are skipped.
    pub fn blend(&mut self, x: i32, y: i32, color: Color32) {
        if x < 0 || y < 0 || x as usize >= self.width || y as usize >= self.height {
            return;
        }
        let pixel = &mut self.pixels[y as usize * self.width + x as usize];
        *pixel = over(color, *pixel);
    }

    /// Fills `rect`. With `rounded`, the four corner pixels are left out,
    /// so it looks slightly rounded.
    pub fn fill_rect(&mut self, rect: PixelRect, color: Color32, rounded: bool) {
        for y in rect.top..rect.bottom {
            for x in rect.left..rect.right {
                let corner = rounded
                    && (x == rect.left || x + 1 == rect.right)
                    && (y == rect.top || y + 1 == rect.bottom);
                if !corner {
                    self.blend(x, y, color);
                }
            }
        }
    }

    /// A 1 px frame just inside `rect`, corners left out like `fill_rect`.
    pub fn stroke_rect(&mut self, rect: PixelRect, color: Color32) {
        self.frame(rect, color, |_| true);
    }

    /// A 1 px dashed frame just inside `rect`: 3 px dash, 2 px gap.
    pub fn dashed_rect(&mut self, rect: PixelRect, color: Color32) {
        self.frame(rect, color, |i| i % 5 < 3);
    }

    fn frame(&mut self, rect: PixelRect, color: Color32, dash: impl Fn(i32) -> bool) {
        let (right, bottom) = (rect.right - 1, rect.bottom - 1);
        for x in rect.left + 1..right {
            if dash(x - rect.left) {
                self.blend(x, rect.top, color);
                self.blend(x, bottom, color);
            }
        }
        for y in rect.top + 1..bottom {
            if dash(y - rect.top) {
                self.blend(rect.left, y, color);
                self.blend(right, y, color);
            }
        }
    }

    /// A filled triangle 7 px wide and 4 px tall pointing up (or down),
    /// its box's top-left corner at (`x`, `y`).
    pub fn triangle(&mut self, x: i32, y: i32, up: bool, color: Color32) {
        for row in 0..4 {
            // Row 0 is the tip: 1 px, then 3, 5 and 7 px wide.
            let y = if up { y + row } else { y + 3 - row };
            for dx in 3 - row..=3 + row {
                self.blend(x + dx, y, color);
            }
        }
    }

    /// A small "✕", 7 × 7 px, its top-left corner at (`x`, `y`).
    pub fn cross(&mut self, x: i32, y: i32, color: Color32) {
        for i in 0..7 {
            self.blend(x + i, y + i, color);
            self.blend(x + 6 - i, y + i, color);
        }
    }

    /// The pixels themselves, row by row from the top.
    pub fn pixels_mut(&mut self) -> &mut [Color32] {
        &mut self.pixels
    }

    /// The finished picture as `0xAARRGGBB` numbers, what Windows'
    /// `UpdateLayeredWindow` wants.
    pub fn into_argb(self) -> Vec<u32> {
        self.pixels.into_iter().map(premultiplied_argb).collect()
    }
}

/// Draws text into `pixels` (already in `0xAARRGGBB`). `coverage` holds one
/// value per pixel, 0 where there is no text and 255 where it is solid, as
/// Windows drew it. The text gets `color` at `alpha` opacity.
pub fn apply_text(pixels: &mut [u32], coverage: &[u8], color: Color32, alpha: u8) {
    for (pixel, &cover) in pixels.iter_mut().zip(coverage) {
        put_text_pixel(pixel, cover, color, alpha);
    }
}

/// Like `apply_text`, but only inside `rect` of a picture `width` pixels
/// wide: there the text gets `color`. Used when each piece of text has its
/// own colour and its own box.
pub fn apply_text_in(
    pixels: &mut [u32],
    coverage: &[u8],
    width: usize,
    rect: PixelRect,
    color: Color32,
) {
    let height = pixels.len() / width.max(1);
    let (left, right) = (
        rect.left.max(0) as usize,
        (rect.right.max(0) as usize).min(width),
    );
    let (top, bottom) = (
        rect.top.max(0) as usize,
        (rect.bottom.max(0) as usize).min(height),
    );
    for y in top..bottom {
        for x in left..right {
            let i = y * width + x;
            put_text_pixel(&mut pixels[i], coverage[i], color, 255);
        }
    }
}

fn put_text_pixel(pixel: &mut u32, cover: u8, color: Color32, alpha: u8) {
    if cover == 0 {
        return;
    }
    let text_alpha = ((cover as u32 * alpha as u32 + 127) / 255) as u8;
    let below = from_premultiplied_argb(*pixel);
    *pixel = premultiplied_argb(over(with_alpha(color, text_alpha), below));
}

/// `color` (taken as solid) at `alpha` opacity, premultiplied.
pub fn with_alpha(color: Color32, alpha: u8) -> Color32 {
    let [r, g, b, _] = color.to_array();
    Color32::from_rgba_unmultiplied(r, g, b, alpha)
}

/// `top` drawn over `below`, both premultiplied ("source over").
pub fn over(top: Color32, below: Color32) -> Color32 {
    let [tr, tg, tb, ta] = top.to_array();
    let [br, bg, bb, ba] = below.to_array();
    let rest = 255 - ta as u32;
    let mix = |t: u8, b: u8| (t as u32 + (b as u32 * rest + 127) / 255).min(255) as u8;
    Color32::from_rgba_premultiplied(mix(tr, br), mix(tg, bg), mix(tb, bb), mix(ta, ba))
}

/// `Color32` (already premultiplied) as `0xAARRGGBB`.
pub fn premultiplied_argb(color: Color32) -> u32 {
    let [r, g, b, a] = color.to_array();
    u32::from_be_bytes([a, r, g, b])
}

pub fn from_premultiplied_argb(pixel: u32) -> Color32 {
    let [a, r, g, b] = pixel.to_be_bytes();
    Color32::from_rgba_premultiplied(r, g, b, a)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> PixelRect {
        PixelRect {
            left,
            top,
            right,
            bottom,
        }
    }

    #[test]
    fn rounded_fill_skips_the_corners() {
        let mut c = Canvas::new(4, 3, Color32::TRANSPARENT);
        c.fill_rect(rect(0, 0, 4, 3), Color32::RED, true);
        let px = c.into_argb();
        assert_eq!(px[0], 0);
        assert_eq!(px[1], premultiplied_argb(Color32::RED));
        assert_eq!(px[3], 0);
        assert_eq!(px[4], premultiplied_argb(Color32::RED));
        assert_eq!(px[11], 0);
    }

    #[test]
    fn drawing_off_the_canvas_is_skipped() {
        let mut c = Canvas::new(2, 2, Color32::TRANSPARENT);
        c.fill_rect(rect(-3, -3, 5, 5), Color32::RED, false);
        c.cross(-2, -2, Color32::RED);
        assert!(
            c.into_argb()
                .iter()
                .all(|&p| p == premultiplied_argb(Color32::RED))
        );
    }

    #[test]
    fn triangles_point_the_right_way() {
        let mut c = Canvas::new(7, 4, Color32::TRANSPARENT);
        c.triangle(0, 0, true, Color32::RED);
        let up = c.into_argb();
        // The tip is one pixel in the middle of the top row.
        assert_eq!(up[..7].iter().filter(|&&p| p != 0).count(), 1);
        assert_eq!(up[21..].iter().filter(|&&p| p != 0).count(), 7);
        let mut c = Canvas::new(7, 4, Color32::TRANSPARENT);
        c.triangle(0, 0, false, Color32::RED);
        let down = c.into_argb();
        assert_eq!(down[..7].iter().filter(|&&p| p != 0).count(), 7);
        assert_eq!(down[21..].iter().filter(|&&p| p != 0).count(), 1);
    }

    #[test]
    fn dashed_frame_has_gaps() {
        let mut c = Canvas::new(12, 4, Color32::TRANSPARENT);
        c.dashed_rect(rect(0, 0, 12, 4), Color32::RED);
        let px = c.into_argb();
        let top: Vec<bool> = px[..12].iter().map(|&p| p != 0).collect();
        assert!(top.contains(&true) && top[1..11].contains(&false));
    }

    #[test]
    fn text_only_lands_inside_its_box() {
        let mut pixels = vec![0u32; 4];
        apply_text_in(&mut pixels, &[255; 4], 2, rect(1, 0, 2, 2), Color32::WHITE);
        assert_eq!(pixels, vec![0, 0xffff_ffff, 0, 0xffff_ffff]);
    }
}
