//! The mining bars overlay window (Windows only).
//!
//! One borderless, top-most, never-activated window that shows all hold
//! bars. It is a "layered" window that gets its picture with
//! `UpdateLayeredWindow`: Windows keeps that picture and shows it by itself,
//! so Athena only draws it again when a hold, a setting or the size
//! changes. Normally clicks go through it (`WS_EX_TRANSPARENT`).
//!
//! In move mode (the "Move / resize overlay" button in Mining › Settings)
//! the window takes clicks: dragging it moves it, dragging its right edge
//! changes its width. The window procedure does the dragging itself: on a
//! press it "captures" the mouse (so it keeps getting mouse moves even off
//! the window), follows the mouse until the button is let go, and then
//! hands the new place back to the UI thread to be saved.
//!
//! Everything runs on one background thread with its own message loop, so
//! the UI thread never waits on a Windows call. It sleeps until the UI
//! thread hands it something new, and while the overlay shows it wakes once
//! a second to put the window back on top (other top-most windows, such as
//! EVE-APM Preview's cards, may have covered it). Nothing here reads from
//! or sends anything to EVE; it only shows Athena's own window.

use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, ANTIALIASED_QUALITY, BI_RGB, BITMAPINFO, BITMAPINFOHEADER,
    BLENDFUNCTION, CLIP_DEFAULT_PRECIS, CreateCompatibleDC, CreateDIBSection, CreateFontW,
    DEFAULT_CHARSET, DIB_RGB_COLORS, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX, DT_RIGHT,
    DT_SINGLELINE, DT_TOP, DeleteDC, DeleteObject, DrawTextW, FF_SWISS, FW_NORMAL, GdiFlush, GetDC,
    GetTextExtentPoint32W, MONITOR_DEFAULTTONULL, MonitorFromPoint, OUT_DEFAULT_PRECIS, ReleaseDC,
    SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::{GetCurrentThreadId, INFINITE};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GWL_EXSTYLE, GetCursorPos,
    GetWindowLongPtrW, HTCLIENT, HTTRANSPARENT, HWND_TOPMOST, IDC_SIZEALL, IDC_SIZEWE,
    IsWindowVisible, LoadCursorW, MA_NOACTIVATE, MSG, MsgWaitForMultipleObjects, PM_NOREMOVE,
    PM_REMOVE, PeekMessageW, PostThreadMessageW, QS_ALLINPUT, RegisterClassW, SW_HIDE,
    SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetCursor,
    SetWindowLongPtrW, SetWindowPos, ShowWindow, ULW_ALPHA, UpdateLayeredWindow, WM_CAPTURECHANGED,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_MOUSEMOVE, WM_NCHITTEST, WM_NULL, WM_QUIT,
    WM_SETCURSOR, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_EX_TRANSPARENT, WS_POPUP,
};

use super::mining_bars::{
    BarsOverlay, DEFAULT_POSITION, MAX_WIDTH_PX, MIN_WIDTH_PX, OverlayPlace, Row, TEXT,
    TEXT_HEIGHT_PX, apply_text, compressed_percent_text, layout, paint, percent_text,
};

/// The window's class name and title. Not shown anywhere.
const CLASS_NAME: &str = "AthenaMiningOverlay";
/// Put the window back on top this often while it shows.
const RAISE_EVERY: Duration = Duration::from_secs(1);
/// How wide the grab area at the right edge is in move mode, in pixels.
const RESIZE_GRIP_PX: i32 = 6;
/// The font for names and percentages.
const FONT_NAME: &str = "Segoe UI";

/// What the UI thread and the overlay thread share. `Arc` lets both hold
/// it; the mutexes and atomics make it safe to use from both.
struct Shared {
    /// The newest overlay from the Mining Supervisor, `None` while it's off.
    overlay: Mutex<Option<BarsOverlay>>,
    /// Set when `overlay` changed, so the thread redraws.
    changed: AtomicBool,
    /// Where the user moved the overlay to, until the UI thread takes it.
    moved: Mutex<Option<OverlayPlace>>,
    egui_ctx: egui::Context,
}

/// The UI thread's handle to the overlay thread.
pub struct BarsWindow {
    shared: Arc<Shared>,
    /// Windows' id of the overlay thread, 0 until it has a message queue.
    thread_id: Arc<AtomicU32>,
}

impl BarsWindow {
    pub fn start(egui_ctx: &egui::Context) -> BarsWindow {
        let shared = Arc::new(Shared {
            overlay: Mutex::new(None),
            changed: AtomicBool::new(false),
            moved: Mutex::new(None),
            egui_ctx: egui_ctx.clone(),
        });
        let thread_id = Arc::new(AtomicU32::new(0));
        let (shared_for_thread, id_for_thread) = (shared.clone(), thread_id.clone());
        let spawned = std::thread::Builder::new()
            .name("mining-overlay".to_string())
            .spawn(move || run(shared_for_thread, &id_for_thread));
        if let Err(e) = spawned {
            // Not fatal: the overlay just never shows.
            eprintln!("Can't start the mining overlay thread: {e}");
        }
        BarsWindow { shared, thread_id }
    }

    /// Hands the thread a new overlay. Only a real change wakes it, so this
    /// is cheap to call every frame.
    pub fn set(&self, overlay: Option<BarsOverlay>) {
        let Ok(mut current) = self.shared.overlay.lock() else {
            return;
        };
        if *current == overlay {
            return;
        }
        *current = overlay;
        drop(current);
        self.shared.changed.store(true, Ordering::Release);
        let id = self.thread_id.load(Ordering::Acquire);
        if id != 0 {
            // SAFETY: posting an empty message to a thread id is always
            // allowed; if the thread is gone the call just fails.
            unsafe {
                PostThreadMessageW(id, WM_NULL, 0, 0);
            }
        }
    }

    /// Where the user last moved the overlay to, once.
    pub fn take_moved(&self) -> Option<OverlayPlace> {
        self.shared.moved.lock().ok().and_then(|mut m| m.take())
    }
}

// State the window procedure needs. Windows calls it on the overlay thread
// (from inside `DispatchMessageW`, `SetWindowPos` and so on), and it gets no
// pointer to our data, so the data lives in thread-locals. Borrows are
// never held across a Windows call, so the procedure can't find one taken.
thread_local! {
    static SHARED: RefCell<Option<Arc<Shared>>> = const { RefCell::new(None) };
    /// The overlay as last applied, for redrawing while the width is pulled.
    static CURRENT: RefCell<Option<BarsOverlay>> = const { RefCell::new(None) };
    /// Where the window is and how wide.
    static PLACE: Cell<OverlayPlace> = const { Cell::new(OverlayPlace { x: 0, y: 0, width: 0 }) };
    static MOVE_MODE: Cell<bool> = const { Cell::new(false) };
    /// The drag going on, if any.
    static DRAG: Cell<Option<Drag>> = const { Cell::new(None) };
}

/// A drag in move mode: what it changes, where the mouse was pressed and
/// where the window was then.
#[derive(Clone, Copy)]
struct Drag {
    resize: bool,
    start_mouse: POINT,
    start_place: OverlayPlace,
}

/// The overlay thread's own bookkeeping.
struct Window {
    hwnd: HWND,
    class_registered: bool,
    /// The saved position and width last put into effect. A new one from
    /// the settings moves the window; the same one leaves it where the user
    /// dragged it (the save may still be on its way).
    applied: Option<(Option<[i32; 2]>, i32)>,
    /// What the picture shows, so it is only drawn again on a change.
    drawn: Option<(BarsOverlay, OverlayPlace)>,
    last_raise: Instant,
}

fn run(shared: Arc<Shared>, thread_id: &AtomicU32) {
    // A thread only gets a message queue once it calls a message function.
    // Make it now, so the UI thread's wake-ups aren't lost.
    // SAFETY: `msg` is a valid place for Windows to write to.
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_NOREMOVE);
        thread_id.store(GetCurrentThreadId(), Ordering::Release);
    }
    SHARED.set(Some(shared.clone()));

    let mut window = Window {
        hwnd: std::ptr::null_mut(),
        class_registered: false,
        applied: None,
        drawn: None,
        last_raise: Instant::now(),
    };
    loop {
        if shared.changed.swap(false, Ordering::Acquire) {
            // Copy it out, so the UI thread never waits on window calls.
            let overlay = shared.overlay.lock().map(|o| o.clone()).unwrap_or(None);
            window.apply(overlay);
        }
        let timeout = window
            .is_visible()
            .then(|| RAISE_EVERY.saturating_sub(window.last_raise.elapsed()));
        if !wait_and_pump(timeout) {
            window.destroy();
            return;
        }
        if window.is_visible() && window.last_raise.elapsed() >= RAISE_EVERY {
            window.raise();
        }
    }
}

impl Window {
    fn is_visible(&self) -> bool {
        // SAFETY: `IsWindowVisible` accepts any handle, null included.
        !self.hwnd.is_null() && unsafe { IsWindowVisible(self.hwnd) } != 0
    }

    /// Makes the window show `overlay`: made, moved, drawn, shown or hidden.
    fn apply(&mut self, overlay: Option<BarsOverlay>) {
        let Some(overlay) = overlay else {
            // Switched off: no window at all.
            self.destroy();
            CURRENT.set(None);
            return;
        };
        MOVE_MODE.set(overlay.move_mode);
        CURRENT.set(Some(overlay.clone()));
        if overlay.bars.is_empty() && !overlay.move_mode {
            // Nobody to show: hide, but keep the window for later.
            self.hide();
            return;
        }
        if self.hwnd.is_null() {
            match self.create() {
                Some(hwnd) => self.hwnd = hwnd,
                None => return,
            }
        }

        let wanted = (overlay.position, overlay.clamped_width());
        if self.applied != Some(wanted) {
            let [x, y] = on_screen(overlay.position.unwrap_or(DEFAULT_POSITION));
            PLACE.set(OverlayPlace {
                x,
                y,
                width: wanted.1,
            });
            self.applied = Some(wanted);
        }
        set_click_through(self.hwnd, !overlay.move_mode);

        let place = PLACE.get();
        let key = (overlay, place);
        if self.drawn.as_ref() != Some(&key) && draw(self.hwnd, place, &key.0) {
            self.drawn = Some(key);
        }
        // SAFETY: our own window, made on this thread. "No activate": EVE
        // keeps the focus.
        unsafe {
            if IsWindowVisible(self.hwnd) == 0 {
                ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            }
        }
        self.raise();
    }

    /// Puts the window back on top. Cheap: it only changes the z-order and
    /// never activates the window or redraws it.
    fn raise(&mut self) {
        self.last_raise = Instant::now();
        if self.hwnd.is_null() {
            return;
        }
        // SAFETY: our own window, made on this thread.
        unsafe {
            SetWindowPos(
                self.hwnd,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }

    fn hide(&self) {
        if self.is_visible() {
            // SAFETY: our own window, made on this thread.
            unsafe {
                ShowWindow(self.hwnd, SW_HIDE);
            }
        }
    }

    fn destroy(&mut self) {
        if !self.hwnd.is_null() {
            // SAFETY: our own window, made on this thread.
            unsafe {
                DestroyWindow(self.hwnd);
            }
            self.hwnd = std::ptr::null_mut();
        }
        self.drawn = None;
        self.applied = None;
    }

    fn create(&mut self) -> Option<HWND> {
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
                // through, top-most, never activated, and no taskbar or
                // Alt+Tab entry.
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

/// Turns letting clicks through on or off.
fn set_click_through(hwnd: HWND, on: bool) {
    // SAFETY: our own window; only its extended style bits change.
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let wanted = if on {
            style | WS_EX_TRANSPARENT as isize
        } else {
            style & !(WS_EX_TRANSPARENT as isize)
        };
        if wanted != style {
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, wanted);
        }
    }
}

/// `position`, or the default spot if no screen shows it (say, the monitor
/// it was on was unplugged), so the overlay never ends up out of sight.
fn on_screen(position: [i32; 2]) -> [i32; 2] {
    // A point a little inside the window's top-left corner.
    let point = POINT {
        x: position[0] + 10,
        y: position[1] + 5,
    };
    // SAFETY: a plain query.
    let monitor = unsafe { MonitorFromPoint(point, MONITOR_DEFAULTTONULL) };
    if monitor.is_null() {
        DEFAULT_POSITION
    } else {
        position
    }
}

/// Handles the window's messages. Only move mode needs anything special.
unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        // Never become the active window when clicked (move mode).
        WM_MOUSEACTIVATE => return MA_NOACTIVATE as LRESULT,
        // Outside move mode the mouse goes through the window.
        WM_NCHITTEST => {
            return if MOVE_MODE.get() {
                HTCLIENT as LRESULT
            } else {
                HTTRANSPARENT as LRESULT
            };
        }
        WM_SETCURSOR if MOVE_MODE.get() => {
            // Left-right arrows on the right edge, the move cross elsewhere.
            let cursor = if on_resize_edge(mouse_position(), PLACE.get()) {
                IDC_SIZEWE
            } else {
                IDC_SIZEALL
            };
            // SAFETY: loading one of Windows' own cursors.
            unsafe {
                SetCursor(LoadCursorW(std::ptr::null_mut(), cursor));
            }
            return 1;
        }
        WM_LBUTTONDOWN if MOVE_MODE.get() => {
            let mouse = mouse_position();
            let place = PLACE.get();
            DRAG.set(Some(Drag {
                resize: on_resize_edge(mouse, place),
                start_mouse: mouse,
                start_place: place,
            }));
            // SAFETY: our own window; keeps the mouse moves coming while
            // the button is held, even off the window.
            unsafe {
                SetCapture(hwnd);
            }
            return 0;
        }
        WM_MOUSEMOVE => {
            if let Some(drag) = DRAG.get() {
                follow_drag(hwnd, drag);
            }
            return 0;
        }
        WM_LBUTTONUP => {
            if DRAG.get().is_some() {
                // SAFETY: lets go of the mouse captured on the press. That
                // sends WM_CAPTURECHANGED, which ends the drag.
                unsafe {
                    ReleaseCapture();
                }
            }
            return 0;
        }
        WM_CAPTURECHANGED => {
            if let Some(drag) = DRAG.take() {
                follow_drag(hwnd, drag);
                // Hand the new place to the UI thread, which saves it.
                if let Some(shared) = SHARED.with_borrow(|s| s.clone()) {
                    if let Ok(mut moved) = shared.moved.lock() {
                        *moved = Some(PLACE.get());
                    }
                    shared.egui_ctx.request_repaint();
                }
            }
            return 0;
        }
        _ => {}
    }
    // SAFETY: passing Windows' own arguments on unchanged.
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

/// Moves or widens the window to where the mouse is now.
fn follow_drag(hwnd: HWND, drag: Drag) {
    let mouse = mouse_position();
    let (dx, dy) = (mouse.x - drag.start_mouse.x, mouse.y - drag.start_mouse.y);
    let start = drag.start_place;
    if drag.resize {
        let place = OverlayPlace {
            width: (start.width + dx).clamp(MIN_WIDTH_PX, MAX_WIDTH_PX),
            ..start
        };
        if place != PLACE.get() {
            PLACE.set(place);
            // A new width needs a new picture (which also sets the size).
            if let Some(overlay) = CURRENT.with_borrow(|o| o.clone()) {
                draw(hwnd, place, &overlay);
            }
        }
    } else {
        let place = OverlayPlace {
            x: start.x + dx,
            y: start.y + dy,
            ..start
        };
        if place != PLACE.get() {
            PLACE.set(place);
            // SAFETY: our own window; only its position changes.
            unsafe {
                SetWindowPos(
                    hwnd,
                    std::ptr::null_mut(),
                    place.x,
                    place.y,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
        }
    }
}

/// Whether the mouse is on the window's right edge, which pulls the width.
fn on_resize_edge(mouse: POINT, place: OverlayPlace) -> bool {
    mouse.x >= place.x + place.width - RESIZE_GRIP_PX
}

/// The mouse position on the screen, in pixels.
fn mouse_position() -> POINT {
    let mut point = POINT { x: 0, y: 0 };
    // SAFETY: `point` is a valid place to write to.
    unsafe {
        GetCursorPos(&mut point);
    }
    point
}

/// Gives the window its picture and puts it at `place`. Returns false if
/// Windows refused (then it is tried again on the next change).
fn draw(hwnd: HWND, place: OverlayPlace, overlay: &BarsOverlay) -> bool {
    let width = place.width.clamp(MIN_WIDTH_PX, MAX_WIDTH_PX);
    let (rows, height) = layout(overlay, width);
    let mut pixels = paint(overlay, &rows, width, height);
    if let Some(coverage) = text_coverage(overlay, &rows, width, height) {
        apply_text(&mut pixels, &coverage.white, TEXT, overlay.bar_alpha);
        apply_text(
            &mut pixels,
            &coverage.purple,
            overlay.compressed_color,
            overlay.bar_alpha,
        );
    }
    // SAFETY: every GDI object made here is released before returning. The
    // bitmap's memory is exactly `width * height` 32-bit pixels, and that
    // is what is copied into it.
    unsafe {
        let screen = GetDC(std::ptr::null_mut());
        let memory = CreateCompatibleDC(screen);
        let info = bitmap_info(width, height);
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
                x: place.x,
                y: place.y,
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

/// How much each pixel is covered by text (0 to 255), one list for the
/// white text and one for the purple compressed percent.
struct TextCoverage {
    white: Vec<u8>,
    purple: Vec<u8>,
}

/// Text colour for the purple percent while drawing the coverage: pure red
/// (Windows colours are 0x00BBGGRR). White text has the same value in all
/// three channels and red text only in the red one, so the two can be told
/// apart afterwards. They never overlap.
const PURPLE_MARKER: u32 = 0x0000_00ff;
const WHITE_MARKER: u32 = 0x00ff_ffff;

/// Draws the names and percentages with Windows' font rendering on black,
/// and returns how much each pixel is covered by text. `apply_text` then
/// lays the text over the picture in its colour. `None` when there is no
/// text to draw.
fn text_coverage(
    overlay: &BarsOverlay,
    rows: &[Row],
    width: i32,
    height: i32,
) -> Option<TextCoverage> {
    if !overlay.has_text_line() || rows.is_empty() {
        return None;
    }
    let font_name = wide(FONT_NAME);
    // SAFETY: every GDI object made here is released before returning. The
    // bitmap's memory is exactly `width * height` 32-bit pixels; it is
    // cleared before drawing and read only after `GdiFlush`.
    unsafe {
        let screen = GetDC(std::ptr::null_mut());
        let memory = CreateCompatibleDC(screen);
        let info = bitmap_info(width, height);
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let bitmap = CreateDIBSection(
            memory,
            &info,
            DIB_RGB_COLORS,
            &mut bits,
            std::ptr::null_mut(),
            0,
        );
        let mut coverage = None;
        if !bitmap.is_null() && !bits.is_null() {
            let count = (width * height) as usize;
            std::ptr::write_bytes(bits as *mut u32, 0, count);
            let previous_bitmap = SelectObject(memory, bitmap);
            // Negative height: the letters' height in pixels. Grey-scale
            // smoothing (not ClearType), so the coverage is the same in all
            // three colours.
            let font = CreateFontW(
                -TEXT_HEIGHT_PX,
                0,
                0,
                0,
                FW_NORMAL as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET as u32,
                OUT_DEFAULT_PRECIS as u32,
                CLIP_DEFAULT_PRECIS as u32,
                ANTIALIASED_QUALITY as u32,
                FF_SWISS as u32,
                font_name.as_ptr(),
            );
            let previous_font = SelectObject(memory, font);
            SetTextColor(memory, WHITE_MARKER);
            SetBkMode(memory, TRANSPARENT as i32);

            for (row, bar) in rows.iter().zip(&overlay.bars) {
                let Some(line) = row.text else {
                    continue;
                };
                // A little taller than the line, so letters like "g" and
                // "y" aren't cut; the next row's gap is below it.
                let bottom = (line.bottom + 3).min(height);
                // Texts on the right, from the right edge leftwards: the hold
                // percent in white, then the compressed percent in purple.
                let mut name_right = line.right;
                let right_texts = [
                    (overlay.show_percent, percent_text(bar), WHITE_MARKER),
                    (
                        overlay.show_compressed_percent,
                        compressed_percent_text(bar),
                        PURPLE_MARKER,
                    ),
                ];
                for (shown, text, color) in right_texts {
                    let Some(text) = text.filter(|_| shown) else {
                        continue;
                    };
                    let text: Vec<u16> = text.encode_utf16().collect();
                    let mut rect = RECT {
                        left: line.left,
                        top: line.top,
                        right: name_right,
                        bottom,
                    };
                    SetTextColor(memory, color);
                    DrawTextW(
                        memory,
                        text.as_ptr(),
                        text.len() as i32,
                        &mut rect,
                        DT_RIGHT | DT_TOP | DT_SINGLELINE | DT_NOPREFIX,
                    );
                    SetTextColor(memory, WHITE_MARKER);
                    let mut size = SIZE { cx: 0, cy: 0 };
                    GetTextExtentPoint32W(memory, text.as_ptr(), text.len() as i32, &mut size);
                    name_right -= size.cx + 4;
                }
                if overlay.show_names && name_right > line.left {
                    let text: Vec<u16> = bar.character.encode_utf16().collect();
                    let mut rect = RECT {
                        left: line.left,
                        top: line.top,
                        right: name_right,
                        bottom,
                    };
                    // A name too long for the space ends in "…".
                    DrawTextW(
                        memory,
                        text.as_ptr(),
                        text.len() as i32,
                        &mut rect,
                        DT_LEFT | DT_TOP | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
                    );
                }
            }
            GdiFlush();
            let drawn = std::slice::from_raw_parts(bits as *const u32, count);
            // White text: blue channel (equal to red and green). Purple
            // text: whatever red has beyond blue.
            coverage = Some(TextCoverage {
                white: drawn.iter().map(|pixel| pixel.to_be_bytes()[3]).collect(),
                purple: drawn
                    .iter()
                    .map(|pixel| {
                        let [_, r, _, b] = pixel.to_be_bytes();
                        r.saturating_sub(b)
                    })
                    .collect(),
            });
            SelectObject(memory, previous_font);
            DeleteObject(font);
            SelectObject(memory, previous_bitmap);
            DeleteObject(bitmap);
        }
        DeleteDC(memory);
        ReleaseDC(std::ptr::null_mut(), screen);
        coverage
    }
}

/// A 32-bit bitmap, `width` × `height`, rows from the top.
fn bitmap_info(width: i32, height: i32) -> BITMAPINFO {
    // SAFETY: all-zero is a valid BITMAPINFO; the header is filled in below.
    let mut info: BITMAPINFO = unsafe { std::mem::zeroed() };
    info.bmiHeader = BITMAPINFOHEADER {
        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: width,
        // Negative: rows run from the top, like `paint`.
        biHeight: -height,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB,
        // SAFETY: the remaining fields are plain numbers; zero is fine.
        ..unsafe { std::mem::zeroed() }
    };
    info
}

/// Sleeps until a message or the timeout, then handles what came in.
/// Returns false if the thread was told to quit.
fn wait_and_pump(timeout: Option<Duration>) -> bool {
    let millis = match timeout {
        // Round up so we don't wake a moment too early and spin.
        Some(t) => u32::try_from(t.as_millis() + 1).unwrap_or(INFINITE - 1),
        None => INFINITE,
    };
    // SAFETY: no handles are passed (count 0). `msg` is a valid place for
    // Windows to write to. Dispatching runs our window procedure.
    unsafe {
        MsgWaitForMultipleObjects(0, std::ptr::null(), 0, millis, QS_ALLINPUT);
        let mut msg: MSG = std::mem::zeroed();
        while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
            if msg.message == WM_QUIT {
                return false;
            }
            DispatchMessageW(&msg);
        }
    }
    true
}

/// A Windows (UTF-16, zero-ended) string.
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}
