//! The hold bars on screen: one small window per bar (Windows only).
//!
//! Each bar is a borderless, click-through, top-most window exactly the size
//! of the bar, sitting on its card. Why not one big window over all cards?
//! On Windows that big window showed a visible frame around all the cards.
//! A small window has nothing around the bar to show.
//!
//! The windows are "layered" and get their picture once with
//! `UpdateLayeredWindow`: Windows keeps that picture and shows it by itself,
//! so a bar is only drawn again when its hold, colour or size changes. A
//! move is a plain `SetWindowPos`.
//!
//! All of this runs on the card finder's thread, which owns the windows and
//! handles their messages. Nothing here ever reads from or sends anything
//! to EVE or APM; it only places Athena's own windows.

use std::collections::HashMap;

use eframe::egui::Color32;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION,
    CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, ReleaseDC,
    SelectObject,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, HWND_TOPMOST, IsWindowVisible, RegisterClassW,
    SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetWindowPos,
    ShowWindow, ULW_ALPHA, UpdateLayeredWindow, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

use super::bars::{CardBar, CardBars, bar_pixels, bar_rect, card_for};
use super::cards::{Card, PixelRect};

/// The bar windows' class and title. Not shown anywhere.
const CLASS_NAME: &str = "AthenaCardBar";

/// What a bar window shows right now.
#[derive(Debug, Clone, PartialEq)]
struct Drawn {
    bar: CardBar,
    compressed_color: Color32,
    width: i32,
    height: i32,
}

struct BarWindow {
    hwnd: HWND,
    /// Where the window is, in screen pixels.
    rect: PixelRect,
    drawn: Option<Drawn>,
}

/// All bar windows, by character name.
pub struct BarWindows {
    windows: HashMap<String, BarWindow>,
    class_registered: bool,
}

impl BarWindows {
    pub fn new() -> BarWindows {
        BarWindows {
            windows: HashMap::new(),
            class_registered: false,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.windows.is_empty()
    }

    /// Makes the windows match the bars: one per bar that has a card, at
    /// the card's bottom left. Windows of bars without a card go away.
    pub fn sync(&mut self, cards: &[Card], bars: Option<&CardBars>) {
        let mut wanted: Vec<(&CardBar, PixelRect)> = Vec::new();
        if let Some(bars) = bars {
            let names: Vec<&str> = bars.all_names.iter().map(String::as_str).collect();
            for bar in &bars.bars {
                if let Some(card) = card_for(cards, &bar.character, &names) {
                    wanted.push((bar, bar_rect(card.rect, bars.height_px)));
                }
            }
        }

        // Close the windows nobody wants any more.
        self.windows.retain(|name, window| {
            let keep = wanted.iter().any(|(bar, _)| &bar.character == name);
            if !keep {
                destroy(window.hwnd);
            }
            keep
        });

        let Some(bars) = bars else {
            return;
        };
        for (bar, rect) in wanted {
            if !self.windows.contains_key(&bar.character) {
                let Some(hwnd) = self.create_window() else {
                    continue;
                };
                self.windows.insert(
                    bar.character.clone(),
                    BarWindow {
                        hwnd,
                        rect,
                        drawn: None,
                    },
                );
            }
            let Some(window) = self.windows.get_mut(&bar.character) else {
                continue;
            };
            let drawn = Drawn {
                bar: bar.clone(),
                compressed_color: bars.compressed_color,
                width: rect.width(),
                height: rect.height(),
            };
            if window.drawn.as_ref() != Some(&drawn) {
                // New picture (also places the window).
                if draw(window.hwnd, rect, &drawn) {
                    window.drawn = Some(drawn);
                    window.rect = rect;
                }
            } else if window.rect != rect {
                // Same picture, the card moved.
                // SAFETY: our own window, made on this thread.
                unsafe {
                    SetWindowPos(
                        window.hwnd,
                        std::ptr::null_mut(),
                        rect.left,
                        rect.top,
                        0,
                        0,
                        SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }
                window.rect = rect;
            }
            // SAFETY: as above. "No activate": EVE keeps the focus.
            unsafe {
                if window.drawn.is_some() && IsWindowVisible(window.hwnd) == 0 {
                    ShowWindow(window.hwnd, SW_SHOWNOACTIVATE);
                }
            }
        }
    }

    /// Puts every bar back on top. APM's cards are top-most too and come
    /// to the front when you switch clients, so this runs after each such
    /// change and every so often in between. Cheap: it only changes the
    /// z-order of a few tiny windows, never activates them.
    pub fn raise(&self) {
        for window in self.windows.values() {
            // SAFETY: our own window, made on this thread.
            unsafe {
                SetWindowPos(
                    window.hwnd,
                    HWND_TOPMOST,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );
            }
        }
    }

    /// Closes every bar window.
    pub fn clear(&mut self) {
        for window in self.windows.values() {
            destroy(window.hwnd);
        }
        self.windows.clear();
    }

    fn create_window(&mut self) -> Option<HWND> {
        let class = wide(CLASS_NAME);
        // SAFETY: the class and title strings live until the calls return;
        // the window procedure is a plain function.
        unsafe {
            let instance = GetModuleHandleW(std::ptr::null());
            if !self.class_registered {
                let mut class_info: WNDCLASSW = std::mem::zeroed();
                class_info.lpfnWndProc = Some(window_proc);
                class_info.hInstance = instance;
                class_info.lpszClassName = class.as_ptr();
                // Fails harmlessly if it is already registered.
                RegisterClassW(&class_info);
                self.class_registered = true;
            }
            let hwnd = CreateWindowExW(
                // Layered (own picture with see-through pixels), clicks go
                // through to the card, top-most, never activated, and no
                // taskbar or Alt+Tab entry.
                WS_EX_LAYERED
                    | WS_EX_TRANSPARENT
                    | WS_EX_TOPMOST
                    | WS_EX_NOACTIVATE
                    | WS_EX_TOOLWINDOW,
                class.as_ptr(),
                class.as_ptr(),
                WS_POPUP,
                0,
                0,
                1,
                1,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                instance,
                std::ptr::null(),
            );
            (!hwnd.is_null()).then_some(hwnd)
        }
    }
}

impl Drop for BarWindows {
    fn drop(&mut self) {
        self.clear();
    }
}

/// The bar windows need no handling of their own.
unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: passing Windows' own arguments on unchanged.
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

fn destroy(hwnd: HWND) {
    // SAFETY: our own window, made on this thread (a stale handle fails).
    unsafe {
        DestroyWindow(hwnd);
    }
}

/// Gives the window its picture and puts it at `rect`. Returns false if
/// Windows refused (then it is tried again on the next change).
fn draw(hwnd: HWND, rect: PixelRect, drawn: &Drawn) -> bool {
    let (width, height) = (drawn.width.max(1), drawn.height.max(1));
    let pixels = bar_pixels(
        width as usize,
        height as usize,
        &drawn.bar,
        drawn.compressed_color,
    );
    // SAFETY: every GDI object made here is released before returning. The
    // bitmap's memory is exactly `width * height` 32-bit pixels, and that
    // is what is copied into it.
    unsafe {
        let screen = GetDC(std::ptr::null_mut());
        let memory = CreateCompatibleDC(screen);
        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            // Negative: rows run from the top, like `bar_pixels`.
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..std::mem::zeroed()
        };
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let bitmap = CreateDIBSection(
            memory,
            &info,
            DIB_RGB_COLORS,
            &mut bits,
            std::ptr::null_mut(),
            0,
        );
        let mut ok = false;
        if !bitmap.is_null() && !bits.is_null() {
            std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits as *mut u32, pixels.len());
            let previous = SelectObject(memory, bitmap);
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            let position = POINT {
                x: rect.left,
                y: rect.top,
            };
            let size = SIZE {
                cx: width,
                cy: height,
            };
            let source = POINT { x: 0, y: 0 };
            ok = UpdateLayeredWindow(
                hwnd, screen, &position, &size, memory, &source, 0, &blend, ULW_ALPHA,
            ) != 0;
            SelectObject(memory, previous);
            DeleteObject(bitmap);
        }
        DeleteDC(memory);
        ReleaseDC(std::ptr::null_mut(), screen);
        ok
    }
}

/// A Windows (UTF-16, zero-ended) string.
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}
