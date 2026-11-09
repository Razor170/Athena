//! Finds EVE-APM Preview's cards and keeps the overlay window on top of them.
//!
//! Everything runs on one background thread, so the UI thread never waits
//! on a Windows call. The thread sleeps until something happens:
//!
//! - Windows tells it when one of APM's windows is created, destroyed,
//!   shown, hidden, moved or renamed (`SetWinEventHook`, filtered to APM's
//!   process), and when another window comes to the front.
//! - Every 5 seconds it looks again anyway, in case it missed an event or
//!   APM was started or restarted (a restart gets a new process id, so the
//!   hook is set again).
//!
//! Each look (`scan`) lists all top-level windows once with `EnumWindows`.
//! A window counts as a card only if its class looks like APM's Qt tool
//! window, its title starts with `EVE - `, and only then its process is
//! `EVEAPMPreview.exe`. Because the class is checked first, Athena never
//! opens a handle to an EVE client. It only reads APM's window titles and
//! positions, never anything from the EVE client.
//!
//! The same scan also finds Athena's own overlay window by its title. The
//! thread then moves that window over the cards and puts it back on top
//! (APM's cards are top-most too), without ever activating it.
//!
//! On other systems (the Linux sandbox) there is no APM, so no thread is
//! started and no cards are ever found.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use eframe::egui;

use super::cards::Card;

/// What the finder last saw. Shared with the UI thread and the overlay.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CardsView {
    /// The visible cards, in Windows' z-order (top first).
    pub cards: Vec<Card>,
    /// Whether an `EVEAPMPreview.exe` window was found at all.
    pub apm_running: bool,
}

/// Handle to the finder thread.
pub struct CardFinder {
    /// `Arc<Mutex<…>>`: the thread writes the newest view, the UI thread
    /// and the overlay read it. The lock is only held to copy or replace it.
    view: Arc<Mutex<CardsView>>,
    /// Whether anything wants the overlay. While this is false the thread
    /// unhooks everything and sleeps without waking up.
    wanted: Arc<AtomicBool>,
    #[cfg(windows)]
    thread: win::ThreadHandle,
}

impl CardFinder {
    /// Starts the finder. `overlay_title` is the title of Athena's overlay
    /// window, and `overlay_viewport` its egui viewport, which is asked to
    /// repaint when the cards change.
    pub fn start(
        egui_ctx: &egui::Context,
        overlay_title: &'static str,
        overlay_viewport: egui::ViewportId,
    ) -> CardFinder {
        let view = Arc::new(Mutex::new(CardsView::default()));
        let wanted = Arc::new(AtomicBool::new(false));
        #[cfg(windows)]
        let thread = win::start(win::ThreadShared {
            view: view.clone(),
            wanted: wanted.clone(),
            egui_ctx: egui_ctx.clone(),
            overlay_title,
            overlay_viewport,
        });
        #[cfg(not(windows))]
        let _ = (egui_ctx, overlay_title, overlay_viewport);
        CardFinder {
            view,
            wanted,
            #[cfg(windows)]
            thread,
        }
    }

    /// Whether finding cards works on this system.
    pub fn is_supported() -> bool {
        cfg!(windows)
    }

    /// Turns the finder on or off. Cheap to call every frame.
    pub fn set_wanted(&self, wanted: bool) {
        let was = self.wanted.swap(wanted, Ordering::Relaxed);
        if was != wanted {
            #[cfg(windows)]
            self.thread.wake();
        }
    }

    /// A shared handle to the newest view, for the overlay's draw callback.
    pub fn shared_view(&self) -> Arc<Mutex<CardsView>> {
        self.view.clone()
    }

    /// A copy of the newest view.
    pub fn view(&self) -> CardsView {
        match self.view.lock() {
            Ok(view) => view.clone(),
            Err(_) => CardsView::default(),
        }
    }
}

/// The Windows part. Everything `unsafe` is in here.
#[cfg(windows)]
mod win {
    use std::cell::Cell;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use eframe::egui;
    use windows_sys::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT};
    use windows_sys::Win32::System::Threading::{
        GetCurrentThreadId, INFINITE, OpenProcess, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    };
    use windows_sys::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CHILDID_SELF, DispatchMessageW, EVENT_OBJECT_CREATE, EVENT_OBJECT_NAMECHANGE,
        EVENT_SYSTEM_FOREGROUND, EnumWindows, GWL_EXSTYLE, GetClassNameW, GetWindowLongPtrW,
        GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, HWND_TOPMOST, IsIconic, IsWindow,
        IsWindowVisible, LWA_ALPHA, MSG, MsgWaitForMultipleObjects, OBJID_WINDOW, PM_NOREMOVE,
        PM_REMOVE, PeekMessageW, PostThreadMessageW, QS_ALLINPUT, SW_HIDE, SW_SHOWNOACTIVATE,
        SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetLayeredWindowAttributes,
        SetWindowLongPtrW, SetWindowPos, ShowWindowAsync, WINEVENT_OUTOFCONTEXT,
        WINEVENT_SKIPOWNPROCESS, WM_NULL, WM_QUIT, WS_EX_LAYERED, WS_EX_NOACTIVATE,
        WS_EX_TOOLWINDOW,
    };

    use super::super::cards::{
        Card, PixelRect, covered_area, is_apm_exe, is_card_class, is_card_title,
    };
    use super::CardsView;

    /// Look again this often even without events.
    const SAFETY_CHECK_EVERY: Duration = Duration::from_secs(5);
    /// Events come in bursts (dragging a card sends dozens per second), so
    /// the thread waits this long after the first one and then looks once.
    const COALESCE_EVENTS_FOR: Duration = Duration::from_millis(30);
    /// Right after the overlay is switched on, the UI thread is still making
    /// its window. Look for it this often, for at most `LOOK_FOR_OVERLAY_FOR`.
    const LOOK_FOR_OVERLAY_EVERY: Duration = Duration::from_millis(100);
    const LOOK_FOR_OVERLAY_FOR: Duration = Duration::from_secs(3);

    /// What the thread gets from the UI side.
    pub struct ThreadShared {
        pub view: Arc<Mutex<CardsView>>,
        pub wanted: Arc<AtomicBool>,
        pub egui_ctx: egui::Context,
        pub overlay_title: &'static str,
        pub overlay_viewport: egui::ViewportId,
    }

    /// Lets the UI thread wake the finder thread.
    pub struct ThreadHandle {
        /// Windows' id of the finder thread, 0 until it has a message queue.
        thread_id: Arc<AtomicU32>,
    }

    impl ThreadHandle {
        /// Wakes the thread so it reads `wanted` again.
        pub fn wake(&self) {
            let id = self.thread_id.load(Ordering::Acquire);
            if id != 0 {
                // SAFETY: posting an empty message to a thread id is always
                // allowed; if the thread is gone the call just fails.
                unsafe {
                    PostThreadMessageW(id, WM_NULL, 0, 0);
                }
            }
        }
    }

    pub fn start(shared: ThreadShared) -> ThreadHandle {
        let thread_id = Arc::new(AtomicU32::new(0));
        let id_for_thread = thread_id.clone();
        std::thread::Builder::new()
            .name("apm-card-finder".to_string())
            .spawn(move || run(shared, &id_for_thread))
            .ok();
        ThreadHandle { thread_id }
    }

    // Set by the event hook callback and read by the loop. Both run on the
    // finder thread (Windows calls the callback from inside `PeekMessageW`),
    // so a thread-local `Cell` is enough, no lock needed. The callback has
    // no "user data" pointer, which is why this can't be a local variable.
    thread_local! {
        static EVENT_SEEN: Cell<bool> = const { Cell::new(false) };
    }

    /// Called by Windows for each window event we hooked.
    unsafe extern "system" fn on_win_event(
        _hook: HWINEVENTHOOK,
        event: u32,
        _hwnd: HWND,
        id_object: i32,
        id_child: i32,
        _event_thread: u32,
        _event_time: u32,
    ) {
        // Only whole windows count; skip events about the caret, the cursor
        // and parts inside a window.
        let whole_window = id_object == OBJID_WINDOW && id_child == CHILDID_SELF as i32;
        if whole_window || event == EVENT_SYSTEM_FOREGROUND {
            EVENT_SEEN.with(|seen| seen.set(true));
        }
    }

    /// The event hooks the thread holds.
    struct Hooks {
        /// APM's process id the `apm` hook is filtered to.
        apm_pid: Option<u32>,
        apm: HWINEVENTHOOK,
        foreground: HWINEVENTHOOK,
    }

    impl Hooks {
        fn none() -> Hooks {
            Hooks {
                apm_pid: None,
                apm: std::ptr::null_mut(),
                foreground: std::ptr::null_mut(),
            }
        }

        /// Hooks APM's window events (again, if APM restarted) and the
        /// foreground change.
        fn follow(&mut self, apm_pid: Option<u32>) {
            if self.foreground.is_null() {
                // SAFETY: an out-of-context hook with a valid callback; it is
                // removed in `clear` on this same thread.
                self.foreground = unsafe {
                    SetWinEventHook(
                        EVENT_SYSTEM_FOREGROUND,
                        EVENT_SYSTEM_FOREGROUND,
                        std::ptr::null_mut(),
                        Some(on_win_event),
                        0,
                        0,
                        WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
                    )
                };
            }
            if apm_pid == self.apm_pid {
                return;
            }
            self.unhook_apm();
            if let Some(pid) = apm_pid {
                // From "created" to "renamed" covers show, hide, reorder and
                // move too. Filtered to APM's process, so these are rare
                // unless a card is being dragged.
                // SAFETY: as above.
                self.apm = unsafe {
                    SetWinEventHook(
                        EVENT_OBJECT_CREATE,
                        EVENT_OBJECT_NAMECHANGE,
                        std::ptr::null_mut(),
                        Some(on_win_event),
                        pid,
                        0,
                        WINEVENT_OUTOFCONTEXT,
                    )
                };
            }
            self.apm_pid = apm_pid;
        }

        fn unhook_apm(&mut self) {
            if !self.apm.is_null() {
                // SAFETY: the handle came from `SetWinEventHook` on this thread.
                unsafe {
                    UnhookWinEvent(self.apm);
                }
                self.apm = std::ptr::null_mut();
            }
            self.apm_pid = None;
        }

        fn clear(&mut self) {
            self.unhook_apm();
            if !self.foreground.is_null() {
                // SAFETY: as above.
                unsafe {
                    UnhookWinEvent(self.foreground);
                }
                self.foreground = std::ptr::null_mut();
            }
        }
    }

    /// Athena's overlay window, as the thread last placed it.
    struct Overlay {
        hwnd: HWND,
        placed_at: Option<PixelRect>,
        /// Whether the layer opacity was set on this window.
        layer_set: bool,
    }

    fn run(shared: ThreadShared, thread_id: &AtomicU32) {
        // A thread only gets a message queue once it calls a message
        // function. Make it now, so the UI thread's wake-ups aren't lost.
        // SAFETY: `msg` is a valid place for Windows to write to.
        unsafe {
            let mut msg: MSG = std::mem::zeroed();
            PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_NOREMOVE);
            thread_id.store(GetCurrentThreadId(), Ordering::Release);
        }

        let mut hooks = Hooks::none();
        let mut overlay = Overlay {
            hwnd: std::ptr::null_mut(),
            placed_at: None,
            layer_set: false,
        };
        let mut last_scan: Option<Instant> = None;
        let mut first_event: Option<Instant> = None;
        // Until when to look for the overlay window often.
        let mut look_for_overlay_until: Option<Instant> = None;

        loop {
            if !shared.wanted.load(Ordering::Relaxed) {
                // Nothing to do: drop the hooks, forget the cards and sleep
                // until the UI thread wakes us.
                hooks.clear();
                publish(&shared, CardsView::default());
                overlay.placed_at = None;
                last_scan = None;
                first_event = None;
                look_for_overlay_until = None;
                EVENT_SEEN.with(|seen| seen.set(false));
                if !wait_and_pump(None) {
                    return;
                }
                continue;
            }

            let now = Instant::now();
            if EVENT_SEEN.with(|seen| seen.replace(false)) && first_event.is_none() {
                first_event = Some(now);
            }
            // SAFETY: `IsWindow` accepts any value, including stale handles.
            let overlay_missing = overlay.hwnd.is_null() || unsafe { IsWindow(overlay.hwnd) } == 0;
            if overlay_missing && look_for_overlay_until.is_none() {
                look_for_overlay_until = Some(now + LOOK_FOR_OVERLAY_FOR);
            }
            if !overlay_missing {
                look_for_overlay_until = None;
            }

            // When the next look is due, whichever comes first.
            let mut due = match last_scan {
                Some(at) => at + SAFETY_CHECK_EVERY,
                None => now,
            };
            if let Some(at) = first_event {
                due = due.min(at + COALESCE_EVENTS_FOR);
            }
            if let (Some(at), Some(until)) = (last_scan, look_for_overlay_until)
                && now < until
            {
                due = due.min(at + LOOK_FOR_OVERLAY_EVERY);
            }

            if now >= due {
                let found = scan(shared.overlay_title);
                hooks.follow(found.apm_pid);
                let view = CardsView {
                    apm_running: found.apm_pid.is_some(),
                    cards: found.cards,
                };
                let area = covered_area(&view.cards);
                publish(&shared, view);
                place_overlay(&mut overlay, found.overlay, area);
                last_scan = Some(now);
                first_event = None;
                continue;
            }

            if !wait_and_pump(Some(due - now)) {
                return;
            }
        }
    }

    /// Replaces the shared view if it changed and asks for repaints.
    fn publish(shared: &ThreadShared, view: CardsView) {
        let Ok(mut current) = shared.view.lock() else {
            return;
        };
        if *current == view {
            return;
        }
        *current = view;
        drop(current);
        // The main window decides whether the overlay exists; the overlay
        // draws the cards. Both have to know.
        shared.egui_ctx.request_repaint();
        shared.egui_ctx.request_repaint_of(shared.overlay_viewport);
    }

    /// Moves the overlay window over the cards (if they moved), puts it
    /// back on top of them and shows it, or hides it when there are no
    /// cards. Never activates it.
    fn place_overlay(overlay: &mut Overlay, hwnd: HWND, area: Option<PixelRect>) {
        if hwnd != overlay.hwnd {
            overlay.hwnd = hwnd;
            overlay.placed_at = None;
            overlay.layer_set = false;
        }
        if hwnd.is_null() {
            return;
        }
        let Some(area) = area else {
            // SAFETY: `hwnd` is our own window, found by this scan.
            unsafe {
                if IsWindowVisible(hwnd) != 0 {
                    ShowWindowAsync(hwnd, SW_HIDE);
                }
            }
            return;
        };
        // SAFETY: `hwnd` is our own window, found by this scan. The calls
        // only change its style bits, position, z-order and visibility.
        // ASYNC means we don't wait for the UI thread to handle them.
        unsafe {
            // No activation on click and no Alt+Tab entry. Set again each
            // time in case the window code reset the style.
            let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            let wanted = style | (WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW) as isize;
            if wanted != style {
                SetWindowLongPtrW(hwnd, GWL_EXSTYLE, wanted);
            }
            // Click-through makes the window "layered", and Windows doesn't
            // show a layered window until its layer opacity is set once.
            // Fully opaque here; the see-through parts come from the
            // transparent pixels Athena draws.
            if style & WS_EX_LAYERED as isize != 0 && !overlay.layer_set {
                SetLayeredWindowAttributes(hwnd, 0, 255, LWA_ALPHA);
                overlay.layer_set = true;
            }
            if overlay.placed_at == Some(area) {
                SetWindowPos(
                    hwnd,
                    HWND_TOPMOST,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS,
                );
            } else {
                SetWindowPos(
                    hwnd,
                    HWND_TOPMOST,
                    area.left,
                    area.top,
                    area.width(),
                    area.height(),
                    SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS,
                );
                overlay.placed_at = Some(area);
            }
            if IsWindowVisible(hwnd) == 0 {
                // "No activate": EVE keeps the focus.
                ShowWindowAsync(hwnd, SW_SHOWNOACTIVATE);
            }
        }
    }

    /// Sleeps until a message, a hooked event or the timeout, then handles
    /// what came in. Returns false if the thread was told to quit.
    fn wait_and_pump(timeout: Option<Duration>) -> bool {
        let millis = match timeout {
            // Round up so we don't wake a moment too early and spin.
            Some(t) => u32::try_from(t.as_millis() + 1).unwrap_or(INFINITE - 1),
            None => INFINITE,
        };
        // SAFETY: no handles are passed (count 0). `msg` is a valid place
        // for Windows to write to. Dispatching runs our hook callback.
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

    /// What one look at all windows found.
    struct ScanResult {
        cards: Vec<Card>,
        apm_pid: Option<u32>,
        /// Athena's overlay window, or null.
        overlay: HWND,
    }

    /// Passed through `EnumWindows` to the callback.
    struct ScanState {
        result: ScanResult,
        overlay_title: &'static str,
        own_pid: u32,
        /// Process id -> is it APM? So each process is opened once per scan.
        checked_processes: HashMap<u32, bool>,
    }

    fn scan(overlay_title: &'static str) -> ScanResult {
        let mut state = ScanState {
            result: ScanResult {
                cards: Vec::new(),
                apm_pid: None,
                overlay: std::ptr::null_mut(),
            },
            overlay_title,
            own_pid: std::process::id(),
            checked_processes: HashMap::new(),
        };
        // SAFETY: `state` outlives the call, and the callback only uses the
        // pointer while `EnumWindows` runs.
        unsafe {
            EnumWindows(Some(check_window), &mut state as *mut ScanState as LPARAM);
        }
        state.result
    }

    /// Called by `EnumWindows` once per top-level window. Returns 1 to go on.
    unsafe extern "system" fn check_window(hwnd: HWND, lparam: LPARAM) -> i32 {
        // SAFETY: `lparam` is the `ScanState` that `scan` passed in.
        let state = unsafe { &mut *(lparam as *mut ScanState) };
        // SAFETY: plain queries on a window handle Windows just gave us.
        unsafe {
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, &mut pid);
            if pid == state.own_pid {
                // Our overlay counts even while it is hidden.
                if window_text(hwnd) == state.overlay_title {
                    state.result.overlay = hwnd;
                }
                return 1;
            }
            if IsWindowVisible(hwnd) == 0 || IsIconic(hwnd) != 0 {
                return 1;
            }
            // 1) The class: skips every EVE client before anything else.
            if !is_card_class(&class_name(hwnd)) {
                return 1;
            }
            // 2) The title.
            let title = window_text(hwnd);
            if !is_card_title(&title) {
                return 1;
            }
            // 3) Only now: is the process really APM?
            let is_apm = *state
                .checked_processes
                .entry(pid)
                .or_insert_with(|| process_is_apm(pid));
            if !is_apm {
                return 1;
            }
            state.result.apm_pid = Some(pid);
            let mut rect: RECT = std::mem::zeroed();
            if GetWindowRect(hwnd, &mut rect) == 0 {
                return 1;
            }
            let rect = PixelRect {
                left: rect.left,
                top: rect.top,
                right: rect.right,
                bottom: rect.bottom,
            };
            if rect.width() > 0 && rect.height() > 0 {
                state.result.cards.push(Card { title, rect });
            }
        }
        1
    }

    /// Whether the process is `EVEAPMPreview.exe`. Opens it with the least
    /// rights Windows has (enough to read its exe path, nothing else).
    fn process_is_apm(pid: u32) -> bool {
        // SAFETY: the handle is checked and always closed; the buffer and
        // its length match.
        unsafe {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if process.is_null() {
                return false;
            }
            let mut buffer = [0u16; 1024];
            let mut length = buffer.len() as u32;
            let ok = QueryFullProcessImageNameW(
                process,
                PROCESS_NAME_WIN32,
                buffer.as_mut_ptr(),
                &mut length,
            );
            CloseHandle(process);
            if ok == 0 {
                return false;
            }
            let path = String::from_utf16_lossy(&buffer[..length as usize]);
            is_apm_exe(&path)
        }
    }

    fn window_text(hwnd: HWND) -> String {
        let mut buffer = [0u16; 256];
        // SAFETY: the buffer length passed matches the buffer.
        let length = unsafe { GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32) };
        String::from_utf16_lossy(&buffer[..length.max(0) as usize])
    }

    fn class_name(hwnd: HWND) -> String {
        let mut buffer = [0u16; 256];
        // SAFETY: the buffer length passed matches the buffer.
        let length = unsafe { GetClassNameW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32) };
        String::from_utf16_lossy(&buffer[..length.max(0) as usize])
    }
}
