//! The cards of EVE-APM Preview, the thumbnail tool Kevin multiboxes with,
//! and which character each card belongs to.
//!
//! This file only holds plain data and the rules for telling a card apart
//! from other windows, so it can be tested anywhere. The Windows calls that
//! find the windows are in `finder.rs`.

/// A rectangle on the screen in physical pixels (not egui points), as
/// Windows' `GetWindowRect` reports it. `right` and `bottom` are exclusive.
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

    /// The smallest rectangle that holds both.
    pub fn union(&self, other: &PixelRect) -> PixelRect {
        PixelRect {
            left: self.left.min(other.left),
            top: self.top.min(other.top),
            right: self.right.max(other.right),
            bottom: self.bottom.max(other.bottom),
        }
    }
}

/// One APM card: the window title (for example `EVE - Noa Askari [Mining]`)
/// and where it is on the screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Card {
    pub title: String,
    pub rect: PixelRect,
}

// Only the Windows card finder uses this.
#[cfg_attr(not(windows), allow(dead_code))]
/// The title every APM card starts with. The EVE clients use it too, so the
/// title alone never tells a card from a client.
const CARD_TITLE_PREFIX: &str = "EVE - ";

// Only the Windows card finder uses this.
#[cfg_attr(not(windows), allow(dead_code))]
/// APM's executable. Checked last, after the class and the title.
pub const APM_EXE_NAME: &str = "EVEAPMPreview.exe";

// Only the Windows card finder uses this.
#[cfg_attr(not(windows), allow(dead_code))]
/// Whether a window class looks like one of APM's card windows.
///
/// APM is built with Qt, which names its tool windows like
/// `Qt6101QWindowToolSaveBits`. The `6101` is the Qt version (6.10.1) and
/// changes when APM updates, so only the stable parts are checked. EVE
/// clients have a different class, so this check alone already skips them.
pub fn is_card_class(class: &str) -> bool {
    class.starts_with("Qt") && class.contains("QWindowTool")
}

// Only the Windows card finder uses this.
#[cfg_attr(not(windows), allow(dead_code))]
/// Whether a window title looks like a card (`EVE - <character>`). APM's
/// label windows, titled `EVE-APM Preview`, don't match.
pub fn is_card_title(title: &str) -> bool {
    title.starts_with(CARD_TITLE_PREFIX) && title.len() > CARD_TITLE_PREFIX.len()
}

// Only the Windows card finder uses this.
#[cfg_attr(not(windows), allow(dead_code))]
/// Whether a full exe path (as `QueryFullProcessImageNameW` gives it) is APM.
pub fn is_apm_exe(path: &str) -> bool {
    let file_name = path.rsplit(['\\', '/']).next().unwrap_or(path);
    file_name.eq_ignore_ascii_case(APM_EXE_NAME)
}

/// The area that holds every card, or `None` without cards. The overlay
/// window covers exactly this area.
pub fn covered_area(cards: &[Card]) -> Option<PixelRect> {
    let mut cards = cards.iter();
    let first = cards.next()?.rect;
    Some(cards.fold(first, |area, card| area.union(&card.rect)))
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

    #[test]
    fn card_class_ignores_the_qt_version() {
        assert!(is_card_class("Qt6101QWindowToolSaveBits"));
        assert!(is_card_class("Qt6110QWindowToolSaveBits"));
        assert!(is_card_class("Qt5QWindowTool"));
        // The EVE client and other windows.
        assert!(!is_card_class("triuiScreen"));
        assert!(!is_card_class("Qt6101QWindowIcon"));
        assert!(!is_card_class(""));
    }

    #[test]
    fn card_title_needs_a_name() {
        assert!(is_card_title("EVE - Noa Askari"));
        assert!(is_card_title("EVE - Noa Askari [Mining]"));
        assert!(!is_card_title("EVE - "));
        assert!(!is_card_title("EVE-APM Preview"));
        assert!(!is_card_title("EVE"));
    }

    #[test]
    fn apm_exe_is_found_by_file_name() {
        assert!(is_apm_exe(r"C:\Tools\EVE-APM\EVEAPMPreview.exe"));
        assert!(is_apm_exe(r"D:\x\eveapmpreview.EXE"));
        assert!(!is_apm_exe(r"C:\EVE\bin\exefile.exe"));
        assert!(!is_apm_exe(r"C:\Fake\NotEVEAPMPreview.exe.bak"));
    }

    #[test]
    fn covered_area_spans_all_cards() {
        assert_eq!(covered_area(&[]), None);
        let cards = vec![card("EVE - A", 100, 50), card("EVE - B", -300, 400)];
        assert_eq!(
            covered_area(&cards),
            Some(PixelRect {
                left: -300,
                top: 50,
                right: 280,
                bottom: 501,
            })
        );
    }
}
