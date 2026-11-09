//! The intel popup: a small always-on-top window that shows an alert over
//! EVE without taking the focus from it.
//!
//! How it works:
//!
//! - eframe can only make a window while Athena's main window is drawn, and
//!   it isn't drawn while minimized (which is when the popup matters most).
//!   So the popup's window is made once, hidden, while the popup is switched
//!   on, and kept alive from the app's `logic`. Showing it is then a plain
//!   Windows call that works at any time.
//! - It is shown with `SW_SHOWNOACTIVATE` and carries `WS_EX_NOACTIVATE`,
//!   so neither opening it nor clicking or dragging it takes the focus from
//!   EVE. (egui's own "show window" would activate it.)
//! - Dragging the grip moves it (`ViewportCommand::StartDrag`); where it
//!   ends up is reported back so the Intel module can save it.
//! - It hides itself after the set time, or on ✕. While it is showing it
//!   repaints a few times a second for the countdown line, and not at all
//!   while hidden.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, RichText, Stroke};

/// The popup window's title; how Athena finds its own window handle.
const POPUP_TITLE: &str = "Athena intel alert";
/// The window's size in points.
pub const POPUP_SIZE: egui::Vec2 = egui::vec2(380.0, 66.0);
/// How often the countdown line moves while the popup shows.
const COUNTDOWN_REPAINT: Duration = Duration::from_millis(200);

// Colours from the approved mockup: the map banner's red for a loud
// (close) alert, orange for the rest of the alert range.
const RED: Color32 = Color32::from_rgb(0xe5, 0x53, 0x4b);
const RED_FILL: Color32 = Color32::from_rgb(0x3a, 0x15, 0x12);
const RED_TITLE: Color32 = Color32::from_rgb(0xff, 0xb3, 0xad);
const ORANGE: Color32 = Color32::from_rgb(0xd9, 0x88, 0x3a);
const ORANGE_FILL: Color32 = Color32::from_rgb(0x2e, 0x1f, 0x12);
const ORANGE_TITLE: Color32 = Color32::from_rgb(0xff, 0xd2, 0xa3);
const DETAILS: Color32 = Color32::from_rgb(0xf0, 0xd0, 0xcc);
const GRIP: Color32 = Color32::from_rgb(0x8b, 0x6f, 0x6c);

/// What the popup shows.
#[derive(Debug, Clone, PartialEq)]
pub struct PopupContent {
    /// "E-312G · 2 jumps from Ako Askari and Noa Askari"
    pub title: String,
    /// "Sabre · Some Pilot · reported by Scout Alt · 07:42:10"
    pub details: String,
    /// Red frame when true, orange otherwise.
    pub loud: bool,
}

/// State shared with the draw callback (which runs whenever the popup
/// window repaints, possibly while the Intel module isn't running).
#[derive(Debug, Default)]
struct Shared {
    content: Option<PopupContent>,
    shown_at: Option<Instant>,
    /// `None` keeps the popup until ✕.
    hide_after: Option<Duration>,
    /// ✕ was clicked.
    close_clicked: bool,
    /// Where the window's top-left corner is, in points, as last drawn
    /// while no button was held.
    position: Option<egui::Pos2>,
}

/// The popup, owned by the Intel module.
pub struct AlertPopup {
    viewport_id: egui::ViewportId,
    shared: Arc<Mutex<Shared>>,
    /// Where the window first opens; it never changes while the window
    /// exists, because egui would move the window back to it.
    opened_at: Option<egui::Pos2>,
    /// The position last reported back, so each move is reported once.
    reported_position: Option<egui::Pos2>,
    window: win::OwnWindow,
}

impl AlertPopup {
    pub fn new() -> AlertPopup {
        AlertPopup {
            viewport_id: egui::ViewportId::from_hash_of("athena_intel_popup"),
            shared: Arc::new(Mutex::new(Shared::default())),
            opened_at: None,
            reported_position: None,
            window: win::OwnWindow::new(),
        }
    }

    /// Shows `content` now, replacing whatever showed. `hide_after` of
    /// `None` keeps it until ✕.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        content: PopupContent,
        hide_after: Option<Duration>,
    ) {
        if let Ok(mut shared) = self.shared.lock() {
            shared.content = Some(content);
            shared.shown_at = Some(Instant::now());
            shared.hide_after = hide_after;
            shared.close_clicked = false;
        }
        self.window.show(ctx, self.viewport_id);
        ctx.request_repaint_of(self.viewport_id);
        // So `keep_alive` runs again to hide it in time (it re-asks for
        // the time left on every pass, see there).
        if let Some(after) = hide_after {
            ctx.request_repaint_after(after);
        }
    }

    pub fn hide(&mut self, ctx: &egui::Context) {
        if let Ok(mut shared) = self.shared.lock() {
            shared.content = None;
        }
        self.window.hide(ctx, self.viewport_id);
    }

    /// Moves the popup back to its default place, top centre of the screen.
    pub fn reset_position(&mut self, ctx: &egui::Context) {
        let position = default_position(ctx);
        ctx.send_viewport_cmd_to(
            self.viewport_id,
            egui::ViewportCommand::OuterPosition(position),
        );
        ctx.request_repaint_of(self.viewport_id);
    }

    /// Keeps the popup's window alive while `enabled`, hides it when its
    /// time is up or ✕ was clicked, and returns a new position after the
    /// user dragged it (to be saved). Must run before every frame of the
    /// main window, also while minimized (the app's `logic`).
    ///
    /// `saved_position` is where the window opens the first time.
    pub fn keep_alive(
        &mut self,
        ctx: &egui::Context,
        enabled: bool,
        saved_position: Option<egui::Pos2>,
    ) -> Option<egui::Pos2> {
        if !enabled {
            // Not shown for a frame: egui closes the window.
            self.opened_at = None;
            self.window.forget();
            if let Ok(mut shared) = self.shared.lock() {
                shared.content = None;
            }
            return None;
        }

        // Time up, or ✕?
        let (check, moved_to) = match self.shared.lock() {
            Ok(mut shared) => {
                let check = hide_check(
                    shared.content.is_some(),
                    shared.shown_at.map(|at| at.elapsed()),
                    shared.hide_after,
                    shared.close_clicked,
                );
                shared.close_clicked = false;
                (check, shared.position)
            }
            Err(_) => (HideCheck::Nothing, None),
        };
        match check {
            HideCheck::Hide => self.hide(ctx),
            // Ask again for a pass when the time is up. eframe keeps only
            // the earliest wake-up per window, so the one asked for in
            // `show` may belong to an older alert that a newer one replaced.
            HideCheck::WakeIn(left) => ctx.request_repaint_after(left),
            HideCheck::Nothing => {}
        }

        let opened_at = *self.opened_at.get_or_insert_with(|| {
            // A saved place that is no longer on any screen (say, its
            // monitor was unplugged) would hide every alert: use the
            // default place instead.
            saved_position
                .filter(|&position| {
                    let pixels_per_point = ctx.pixels_per_point();
                    win::monitors().is_none_or(|monitors| {
                        let monitors: Vec<egui::Rect> = monitors
                            .iter()
                            .map(|m| {
                                egui::Rect::from_min_max(
                                    m.min / pixels_per_point,
                                    m.max / pixels_per_point,
                                )
                            })
                            .collect();
                        is_on_a_monitor(position, &monitors)
                    })
                })
                .unwrap_or_else(|| default_position(ctx))
        });
        let builder = egui::ViewportBuilder::default()
            .with_title(POPUP_TITLE)
            // Hidden until an alert shows it.
            .with_visible(false)
            .with_position(opened_at)
            .with_inner_size(POPUP_SIZE)
            .with_decorations(false)
            .with_resizable(false)
            .with_always_on_top()
            .with_taskbar(false)
            .with_active(false);

        let shared = self.shared.clone();
        // `move` hands the clone to the callback, which egui keeps and
        // calls whenever the popup repaints (`'static`, shared via `Arc`).
        ctx.show_viewport_deferred(self.viewport_id, builder, move |ui, _class| {
            draw(ui, &shared);
        });
        self.window.prepare(ctx, self.viewport_id);

        // Report a drag once it has ended.
        match moved_to {
            Some(position) if self.reported_position != Some(position) => {
                let first = self.reported_position.is_none();
                self.reported_position = Some(position);
                // The first report is just where it opened, not a move.
                if first && Some(position) == saved_position {
                    return None;
                }
                Some(position)
            }
            _ => None,
        }
    }
}

/// What `keep_alive` should do about hiding the popup.
#[derive(Debug, PartialEq)]
enum HideCheck {
    /// Time is up, or ✕ was clicked.
    Hide,
    /// Still showing: check again after this long.
    WakeIn(Duration),
    /// Hidden, or kept until ✕.
    Nothing,
}

/// Decides [`HideCheck`] for a popup that has been `shown_for` (since its
/// latest alert) and should hide after `hide_after` (`None`: until ✕).
fn hide_check(
    showing: bool,
    shown_for: Option<Duration>,
    hide_after: Option<Duration>,
    close_clicked: bool,
) -> HideCheck {
    if !showing {
        return HideCheck::Nothing;
    }
    if close_clicked {
        return HideCheck::Hide;
    }
    match (shown_for, hide_after) {
        (Some(shown_for), Some(after)) if shown_for >= after => HideCheck::Hide,
        (Some(shown_for), Some(after)) => HideCheck::WakeIn(after - shown_for),
        _ => HideCheck::Nothing,
    }
}

/// Whether a popup at `position` can be grabbed: its grip corner (the top
/// left 40 × 24 points) must be at least partly on one of `monitors`.
fn is_on_a_monitor(position: egui::Pos2, monitors: &[egui::Rect]) -> bool {
    let grip = egui::Rect::from_min_size(position, egui::vec2(40.0, 24.0));
    monitors.iter().any(|monitor| monitor.intersects(grip))
}

/// Top centre of the main window's screen.
fn default_position(ctx: &egui::Context) -> egui::Pos2 {
    let screen = ctx.input(|i| i.viewport().monitor_size);
    let width = screen.map_or(1920.0, |s| s.x);
    egui::pos2(((width - POPUP_SIZE.x) / 2.0).max(0.0), 80.0)
}

/// Draws the popup: grip, title, ✕, details and the countdown line.
fn draw(ui: &mut egui::Ui, shared: &Arc<Mutex<Shared>>) {
    // Copy out what is needed, so the lock isn't held while drawing.
    let (content, progress) = match shared.lock() {
        Ok(mut s) => {
            // Remember where the window is. (Not "once the button is up":
            // Windows runs the drag itself, so egui may never see the
            // button go up.)
            if let Some(rect) = ui.input(|i| i.viewport().outer_rect) {
                s.position = Some(rect.min);
            }
            let progress = match (s.shown_at, s.hide_after) {
                (Some(at), Some(after)) if !after.is_zero() => {
                    Some(1.0 - (at.elapsed().as_secs_f32() / after.as_secs_f32()).min(1.0))
                }
                _ => None,
            };
            (s.content.clone(), progress)
        }
        Err(_) => return,
    };
    let Some(content) = content else {
        return;
    };
    let (frame_color, fill, title_color) = if content.loud {
        (RED, RED_FILL, RED_TITLE)
    } else {
        (ORANGE, ORANGE_FILL, ORANGE_TITLE)
    };

    let full = ui.max_rect();
    ui.painter().rect_filled(full, 0.0, fill);
    ui.painter().rect_stroke(
        full,
        0.0,
        Stroke::new(2.0, frame_color),
        egui::StrokeKind::Inside,
    );

    let inner = full.shrink2(egui::vec2(10.0, 7.0));
    let mut content_ui = ui.new_child(egui::UiBuilder::new().max_rect(inner));
    content_ui.spacing_mut().item_spacing = egui::vec2(8.0, 4.0);
    let mut close = false;
    content_ui.horizontal(|ui| {
        // The grip: drag it to move the window.
        // Painted as six dots rather than a "⋮⋮" glyph, which not every
        // font has.
        let (grip_rect, grip) =
            ui.allocate_exact_size(egui::vec2(10.0, 14.0), egui::Sense::click_and_drag());
        let grip = grip.on_hover_cursor(egui::CursorIcon::Move);
        for column in 0..2 {
            for row in 0..3 {
                let dot = grip_rect.left_top()
                    + egui::vec2(2.5 + column as f32 * 5.0, 2.5 + row as f32 * 4.5);
                ui.painter().circle_filled(dot, 1.3, GRIP);
            }
        }
        if grip.drag_started() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            close = close_button(ui, title_color).clicked();
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.add(
                    egui::Label::new(RichText::new(&content.title).color(title_color).strong())
                        .truncate(),
                );
            });
        });
    });
    content_ui.add(
        egui::Label::new(RichText::new(&content.details).color(DETAILS).size(12.0)).truncate(),
    );

    // The countdown line along the bottom.
    if let Some(progress) = progress {
        let line = egui::Rect::from_min_size(
            egui::pos2(inner.left(), full.bottom() - 7.0),
            egui::vec2(inner.width(), 2.0),
        );
        ui.painter()
            .rect_filled(line, 1.0, Color32::from_white_alpha(30));
        let mut left = line;
        left.set_width(line.width() * progress);
        ui.painter().rect_filled(left, 1.0, title_color);
        if progress > 0.0 {
            ui.ctx().request_repaint_after(COUNTDOWN_REPAINT);
        }
    }

    if close && let Ok(mut s) = shared.lock() {
        s.close_clicked = true;
        // The Intel module hides the window on the next frame of the main
        // window; ask for one.
        ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
    }
}

/// A small × button, painted as two lines rather than a "✕" glyph, which
/// not every font has.
fn close_button(ui: &mut egui::Ui, color: Color32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::click());
    let response = response.on_hover_text("Close");
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, 3.0, Color32::from_white_alpha(25));
    }
    let cross = rect.shrink(4.5);
    let stroke = Stroke::new(1.5, color);
    ui.painter()
        .line_segment([cross.left_top(), cross.right_bottom()], stroke);
    ui.painter()
        .line_segment([cross.right_top(), cross.left_bottom()], stroke);
    response
}

/// Showing and hiding the popup window without activating it.
#[cfg(windows)]
mod win {
    use eframe::egui;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT};
    use windows_sys::Win32::Graphics::Gdi::{EnumDisplayMonitors, HDC, HMONITOR};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GWL_EXSTYLE, GetWindowLongPtrW, GetWindowThreadProcessId, HWND_TOPMOST,
        InternalGetWindowText, IsWindow, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE,
        SWP_NOSIZE, SetWindowLongPtrW, SetWindowPos, ShowWindow, WS_EX_NOACTIVATE,
        WS_EX_TOOLWINDOW,
    };

    use super::POPUP_TITLE;

    /// The popup's window handle, found by its title once it exists. Kept
    /// as a number: a raw `HWND` is a pointer, which Rust won't let a
    /// struct share between threads, and only the UI thread uses it anyway.
    pub struct OwnWindow {
        hwnd: isize,
    }

    impl OwnWindow {
        pub fn new() -> OwnWindow {
            OwnWindow { hwnd: 0 }
        }

        fn handle(&self) -> HWND {
            self.hwnd as HWND
        }

        /// Finds the window (once it exists) and makes sure it can't be
        /// activated and has no Alt+Tab entry.
        pub fn prepare(&mut self, _ctx: &egui::Context, _id: egui::ViewportId) {
            // SAFETY: `IsWindow` accepts any value.
            if self.hwnd != 0 && unsafe { IsWindow(self.handle()) } != 0 {
                return;
            }
            self.hwnd = find_own_window(POPUP_TITLE).map_or(0, |h| h as isize);
            self.make_no_activate();
        }

        /// Sets "can't be activated" and "no Alt+Tab entry" on the window,
        /// if they aren't set. Run again before every show: winit may
        /// rewrite the window's extended style when its state changes.
        fn make_no_activate(&self) {
            if self.hwnd == 0 {
                return;
            }
            let wanted = (WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW) as isize;
            // SAFETY: our own window.
            unsafe {
                let style = GetWindowLongPtrW(self.handle(), GWL_EXSTYLE);
                if style & wanted != wanted {
                    SetWindowLongPtrW(self.handle(), GWL_EXSTYLE, style | wanted);
                }
            }
        }

        pub fn show(&mut self, ctx: &egui::Context, id: egui::ViewportId) {
            self.prepare(ctx, id);
            if self.hwnd == 0 {
                return;
            }
            self.make_no_activate();
            // SAFETY: our own window. "No activate" keeps EVE focused;
            // top-most puts it above APM's cards and EVE.
            unsafe {
                ShowWindow(self.handle(), SW_SHOWNOACTIVATE);
                SetWindowPos(
                    self.handle(),
                    HWND_TOPMOST,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );
            }
        }

        pub fn hide(&mut self, _ctx: &egui::Context, _id: egui::ViewportId) {
            if self.hwnd != 0 {
                // SAFETY: our own window (or a stale handle, which fails).
                unsafe {
                    ShowWindow(self.handle(), SW_HIDE);
                }
            }
        }

        pub fn forget(&mut self) {
            self.hwnd = 0;
        }
    }

    /// The screens' rectangles in physical pixels.
    pub fn monitors() -> Option<Vec<egui::Rect>> {
        unsafe extern "system" fn add(
            _monitor: HMONITOR,
            _dc: HDC,
            rect: *mut RECT,
            lparam: LPARAM,
        ) -> i32 {
            // SAFETY: `lparam` is the Vec passed below, `rect` is valid
            // for this call.
            unsafe {
                let list = &mut *(lparam as *mut Vec<egui::Rect>);
                let r = &*rect;
                list.push(egui::Rect::from_min_max(
                    egui::pos2(r.left as f32, r.top as f32),
                    egui::pos2(r.right as f32, r.bottom as f32),
                ));
            }
            1
        }
        let mut list: Vec<egui::Rect> = Vec::new();
        // SAFETY: `list` outlives the call.
        unsafe {
            EnumDisplayMonitors(
                std::ptr::null_mut(),
                std::ptr::null(),
                Some(add),
                &mut list as *mut Vec<egui::Rect> as LPARAM,
            );
        }
        // An empty list means asking failed: then don't judge the place.
        (!list.is_empty()).then_some(list)
    }

    /// One of Athena's own top-level windows by its exact title.
    fn find_own_window(title: &str) -> Option<HWND> {
        struct Search<'a> {
            title: &'a str,
            own_pid: u32,
            found: HWND,
        }
        unsafe extern "system" fn check(hwnd: HWND, lparam: LPARAM) -> i32 {
            // SAFETY: `lparam` is the `Search` passed below.
            let search = unsafe { &mut *(lparam as *mut Search) };
            let mut pid = 0u32;
            // SAFETY: plain queries on a handle Windows just gave us. The
            // title is read without sending our UI thread a message (we
            // are on it).
            unsafe {
                GetWindowThreadProcessId(hwnd, &mut pid);
                if pid != search.own_pid {
                    return 1;
                }
                let mut buffer = [0u16; 128];
                let length = InternalGetWindowText(hwnd, buffer.as_mut_ptr(), buffer.len() as i32);
                if String::from_utf16_lossy(&buffer[..length.max(0) as usize]) == search.title {
                    search.found = hwnd;
                    return 0;
                }
            }
            1
        }
        let mut search = Search {
            title,
            own_pid: std::process::id(),
            found: std::ptr::null_mut(),
        };
        // SAFETY: `search` outlives the call.
        unsafe {
            EnumWindows(Some(check), &mut search as *mut Search as LPARAM);
        }
        (!search.found.is_null()).then_some(search.found)
    }
}

/// Elsewhere (the Linux sandbox) egui shows and hides the window.
#[cfg(not(windows))]
mod win {
    use eframe::egui;

    pub struct OwnWindow;

    impl OwnWindow {
        pub fn new() -> OwnWindow {
            OwnWindow
        }

        pub fn prepare(&mut self, _ctx: &egui::Context, _id: egui::ViewportId) {}

        pub fn show(&mut self, ctx: &egui::Context, id: egui::ViewportId) {
            ctx.send_viewport_cmd_to(id, egui::ViewportCommand::Visible(true));
        }

        pub fn hide(&mut self, ctx: &egui::Context, id: egui::ViewportId) {
            ctx.send_viewport_cmd_to(id, egui::ViewportCommand::Visible(false));
        }

        pub fn forget(&mut self) {}
    }

    /// Unknown here, so any saved place is used as it is.
    pub fn monitors() -> Option<Vec<egui::Rect>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn hides_when_time_is_up_or_on_close() {
        assert_eq!(
            hide_check(true, Some(secs(5)), Some(secs(5)), false),
            HideCheck::Hide
        );
        assert_eq!(
            hide_check(true, Some(secs(1)), Some(secs(5)), true),
            HideCheck::Hide
        );
        // Kept until ✕, or already hidden: nothing to wait for.
        assert_eq!(
            hide_check(true, Some(secs(60)), None, false),
            HideCheck::Nothing
        );
        assert_eq!(
            hide_check(false, Some(secs(9)), Some(secs(5)), false),
            HideCheck::Nothing
        );
    }

    #[test]
    fn a_newer_alert_gets_its_own_wake_up() {
        // Alert A at 0 s asked for a wake-up at 5 s. Alert B at 3 s
        // replaced it, so at the 5 s pass B has shown for 2 s: the check
        // must ask for another pass in 3 s instead of doing nothing.
        assert_eq!(
            hide_check(true, Some(secs(2)), Some(secs(5)), false),
            HideCheck::WakeIn(secs(3))
        );
    }

    #[test]
    fn a_saved_place_off_every_screen_is_not_used() {
        let monitors = [
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1920.0, 1080.0)),
            egui::Rect::from_min_max(egui::pos2(1920.0, 0.0), egui::pos2(3840.0, 1080.0)),
        ];
        assert!(is_on_a_monitor(egui::pos2(2500.0, 80.0), &monitors));
        // Partly off the right edge, but the grip is still on screen.
        assert!(is_on_a_monitor(egui::pos2(3820.0, 80.0), &monitors));
        // The second monitor was unplugged.
        assert!(!is_on_a_monitor(egui::pos2(2500.0, 80.0), &monitors[..1]));
        assert!(!is_on_a_monitor(egui::pos2(-500.0, 80.0), &monitors));
    }
}
