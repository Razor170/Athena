//! The ISIS role icons the Logi view uses, drawn in any colour.
//!
//! The icons are CCP's art (EVE Online's ISIS ship tree), taken from the
//! EVE University wiki (<https://wiki.eveuniversity.org/UniWiki:Icons>).
//! Each one is a single-colour 30×30 picture, so only its transparency
//! matters: `assets/isis/*.a8` hold one alpha byte per pixel, row by row
//! (see `assets/isis/README.md`). That way no image decoder is needed.
//! They become white textures once, and the painter tints them.

use eframe::egui::{self, Color32, ColorImage, Rect, TextureHandle, TextureOptions, pos2};

/// Width and height of every icon, in pixels.
const SIZE: usize = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Shield,
    Armor,
    Cap,
    Damage,
    Ecm,
    Tackle,
}

/// The textures, made on first use. A `TextureHandle` frees its texture
/// when dropped.
pub struct Icons {
    shield: TextureHandle,
    armor: TextureHandle,
    cap: TextureHandle,
    damage: TextureHandle,
    ecm: TextureHandle,
    tackle: TextureHandle,
}

impl Icons {
    pub fn load(ctx: &egui::Context) -> Icons {
        let make = |name: &str, alpha: &[u8]| {
            ctx.load_texture(name, white_image(alpha), TextureOptions::LINEAR)
        };
        Icons {
            shield: make(
                "isis-shield",
                include_bytes!("../../../assets/isis/shield.a8"),
            ),
            armor: make(
                "isis-armor",
                include_bytes!("../../../assets/isis/armor.a8"),
            ),
            cap: make("isis-cap", include_bytes!("../../../assets/isis/cap.a8")),
            damage: make(
                "isis-attack",
                include_bytes!("../../../assets/isis/attack.a8"),
            ),
            ecm: make("isis-ecm", include_bytes!("../../../assets/isis/ecm.a8")),
            tackle: make("isis-warp", include_bytes!("../../../assets/isis/warp.a8")),
        }
    }

    /// Draws `icon` into `rect` in `color`.
    pub fn paint(&self, painter: &egui::Painter, icon: Icon, rect: Rect, color: Color32) {
        let texture = match icon {
            Icon::Shield => &self.shield,
            Icon::Armor => &self.armor,
            Icon::Cap => &self.cap,
            Icon::Damage => &self.damage,
            Icon::Ecm => &self.ecm,
            Icon::Tackle => &self.tackle,
        };
        let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        painter.image(texture.id(), rect, uv, color);
    }
}

/// A white picture with the given transparency. A file of the wrong size
/// gives an empty icon instead of a crash.
fn white_image(alpha: &[u8]) -> ColorImage {
    let mut rgba = Vec::with_capacity(SIZE * SIZE * 4);
    for i in 0..SIZE * SIZE {
        let a = alpha.get(i).copied().unwrap_or(0);
        rgba.extend_from_slice(&[255, 255, 255, a]);
    }
    ColorImage::from_rgba_unmultiplied([SIZE, SIZE], &rgba)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_files_have_the_right_size() {
        for bytes in [
            include_bytes!("../../../assets/isis/shield.a8").as_slice(),
            include_bytes!("../../../assets/isis/armor.a8"),
            include_bytes!("../../../assets/isis/cap.a8"),
            include_bytes!("../../../assets/isis/attack.a8"),
            include_bytes!("../../../assets/isis/ecm.a8"),
            include_bytes!("../../../assets/isis/warp.a8"),
        ] {
            assert_eq!(bytes.len(), SIZE * SIZE);
            // Not empty: some pixels are solid.
            assert!(bytes.iter().any(|&a| a > 200));
        }
    }
}
