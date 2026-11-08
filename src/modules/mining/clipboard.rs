//! Watches the clipboard for a copied Mining Survey Scanner result.
//!
//! Windows counts every clipboard change (`GetClipboardSequenceNumber`).
//! A background thread checks that number twice a second, which is a single
//! cheap system call, and only reads the clipboard text when it changed.
//! Text that isn't a belt scan is ignored (see `BeltScan::parse`, which is
//! strict on purpose); the clipboard itself is never changed.
//!
//! On other systems (the Linux sandbox the code is tested in) watching does
//! nothing and [`is_supported`] is false.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

use eframe::egui;

use super::gamelog::LogTime;
use super::scan::BeltScan;

const CHECK_EVERY: Duration = Duration::from_millis(500);
/// Bigger clipboard text can't be a belt scan (a huge belt is a few KB).
const MAX_TEXT_BYTES: usize = 256 * 1024;

/// Reads the clipboard text once, if there is text of at most
/// `max_bytes`. Used by the Intel tag import. Always `None` off Windows.
pub fn read_text_once(max_bytes: usize) -> Option<String> {
    match win::read_text(max_bytes) {
        ClipboardText::Text(text) => Some(text),
        _ => None,
    }
}

/// Whether clipboard watching works on this system.
pub fn is_supported() -> bool {
    cfg!(windows)
}

/// Handle to the clipboard thread. Dropping it stops the thread.
pub struct ClipboardWatcher {
    pub scans: Receiver<BeltScan>,
    _stop: Sender<()>,
}

impl ClipboardWatcher {
    pub fn start(egui_ctx: egui::Context) -> ClipboardWatcher {
        let (scan_tx, scans) = mpsc::channel();
        let (_stop, stop_rx) = mpsc::channel::<()>();
        if is_supported() {
            std::thread::spawn(move || watch(scan_tx, stop_rx, egui_ctx));
        }
        ClipboardWatcher { scans, _stop }
    }
}

fn watch(scans: Sender<BeltScan>, stop: Receiver<()>, egui_ctx: egui::Context) {
    // Whatever is on the clipboard at startup is old; only react to changes.
    let mut last_seen = win::sequence_number();
    loop {
        match stop.recv_timeout(CHECK_EVERY) {
            Err(RecvTimeoutError::Timeout) => {}
            _ => return,
        }
        let current = win::sequence_number();
        if current == last_seen {
            continue;
        }
        let text = match win::read_text(MAX_TEXT_BYTES) {
            ClipboardText::Text(text) => text,
            ClipboardText::NoText => {
                last_seen = current;
                continue;
            }
            // Another program has the clipboard open right now (often just
            // after a copy). Keep `last_seen` as it is, so the next tick
            // tries this change again instead of losing the scan.
            ClipboardText::Busy => continue,
        };
        last_seen = current;
        if let Some(scan) = BeltScan::parse(&text, now()) {
            if scans.send(scan).is_err() {
                return;
            }
            egui_ctx.request_repaint();
        }
    }
}

/// What reading the clipboard gave.
// Only the Windows code makes `Text` and `Busy`.
#[cfg_attr(not(windows), allow(dead_code))]
enum ClipboardText {
    Text(String),
    /// No text on it, or too much to be a scan.
    NoText,
    /// Another program has it open; try again shortly.
    Busy,
}

/// The current time as EVE time (UTC).
fn now() -> LogTime {
    let unix_seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    LogTime { unix_seconds }
}

/// The Windows API calls. Everything `unsafe` is in here.
#[cfg(windows)]
mod win {
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, GetClipboardData, GetClipboardSequenceNumber, IsClipboardFormatAvailable,
        OpenClipboard,
    };
    use windows_sys::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};

    use super::ClipboardText;

    /// The clipboard format for UTF-16 text (`CF_UNICODETEXT`).
    const CF_UNICODETEXT: u32 = 13;

    pub fn sequence_number() -> u32 {
        // SAFETY: takes no arguments and only returns a number.
        unsafe { GetClipboardSequenceNumber() }
    }

    /// The clipboard text, if there is any and it is at most `max_bytes`.
    pub fn read_text(max_bytes: usize) -> ClipboardText {
        // SAFETY: the Windows clipboard protocol is followed exactly:
        // open, get the handle, lock it, copy the text out, unlock, close.
        // The pointer is only used while the memory is locked, and never
        // read past the size Windows reports for it.
        unsafe {
            if IsClipboardFormatAvailable(CF_UNICODETEXT) == 0 {
                return ClipboardText::NoText;
            }
            if OpenClipboard(std::ptr::null_mut()) == 0 {
                return ClipboardText::Busy;
            }
            let text = (|| {
                let handle = GetClipboardData(CF_UNICODETEXT);
                if handle.is_null() {
                    return None;
                }
                let pointer = GlobalLock(handle) as *const u16;
                if pointer.is_null() {
                    return None;
                }
                // At most the size of the memory block, stopping at the
                // terminating zero.
                let max_units = GlobalSize(handle) / 2;
                if max_units * 2 > max_bytes {
                    GlobalUnlock(handle);
                    return None;
                }
                let units = std::slice::from_raw_parts(pointer, max_units);
                let length = units.iter().position(|&u| u == 0).unwrap_or(max_units);
                let text = String::from_utf16_lossy(&units[..length]);
                GlobalUnlock(handle);
                Some(text)
            })();
            CloseClipboard();
            match text {
                Some(text) => ClipboardText::Text(text),
                None => ClipboardText::NoText,
            }
        }
    }
}

/// Stand-ins so the code builds on other systems; never called there.
#[cfg(not(windows))]
mod win {
    use super::ClipboardText;

    pub fn sequence_number() -> u32 {
        0
    }

    pub fn read_text(_max_bytes: usize) -> ClipboardText {
        ClipboardText::NoText
    }
}
