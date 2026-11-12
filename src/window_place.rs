//! Remembers where Athena's main window is on screen and puts it back there
//! on the next start.
//!
//! On Windows this uses the window's "placement" (`GetWindowPlacement` /
//! `SetWindowPlacement`): the position and size the window has when it is
//! not maximized, plus whether it is maximized. Windows keeps those in
//! screen pixels, so this works across several monitors, and a maximized
//! window opens maximized on the monitor it was on. Other systems (only
//! used for development) keep the old behaviour: start maximized.

use crate::layout::WindowPlacement;

/// Smallest window we accept from the file. Anything smaller is treated as
/// broken, and Athena starts maximized instead.
const MIN_SIZE: i32 = 200;
/// Largest window we accept from the file (several 4K monitors side by side).
const MAX_SIZE: i32 = 30_000;
/// How much of the window's top edge must be on a monitor, in pixels, so the
/// user can still grab the title bar and move the window.
const MIN_VISIBLE_WIDTH: i32 = 100;
const MIN_VISIBLE_HEIGHT: i32 = 20;
/// The strip at the top of the window that counts as its title bar.
const TITLE_BAR_HEIGHT: i32 = 30;

/// A rectangle in screen pixels, edges inclusive on the left/top and
/// exclusive on the right/bottom, like a Windows `RECT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// True if the saved size is believable.
pub fn sane_size(placement: &WindowPlacement) -> bool {
    (MIN_SIZE..=MAX_SIZE).contains(&placement.width)
        && (MIN_SIZE..=MAX_SIZE).contains(&placement.height)
}

/// True if enough of the window's title bar lies inside `work_area` (the part
/// of a monitor not covered by the taskbar) to grab it with the mouse.
///
/// Strictly, the placement is in Windows "workspace" coordinates, which are
/// shifted by the taskbar's thickness when it sits on the top or left of the
/// main monitor, while the work area is in screen coordinates. That only
/// moves this check by a few dozen pixels, so we don't correct for it.
pub fn title_bar_visible(placement: &WindowPlacement, work_area: ScreenRect) -> bool {
    let bar = title_bar(placement);
    let width = bar.right.min(work_area.right) - bar.left.max(work_area.left);
    let height = bar.bottom.min(work_area.bottom) - bar.top.max(work_area.top);
    width >= MIN_VISIBLE_WIDTH && height >= MIN_VISIBLE_HEIGHT
}

fn title_bar(placement: &WindowPlacement) -> ScreenRect {
    // `saturating_add` can't overflow, even with a nonsense value in the file.
    ScreenRect {
        left: placement.x,
        top: placement.y,
        right: placement.x.saturating_add(placement.width),
        bottom: placement.y.saturating_add(TITLE_BAR_HEIGHT),
    }
}

/// Checks a saved placement before the window is created. Returns it only if
/// the window would be usable: a believable size, and a title bar on one of
/// the monitors that are connected now (a monitor may have been unplugged
/// since the last run).
pub fn usable(placement: WindowPlacement) -> Option<WindowPlacement> {
    if !sane_size(&placement) {
        return None;
    }
    let work_area = win::work_area_near(title_bar(&placement))?;
    title_bar_visible(&placement, work_area).then_some(placement)
}

/// Makes Athena DPI-aware, so monitor rects come back in real pixels like
/// the saved placement. Must run before `usable`: winit does the same, but
/// only when the window is created, and until then Windows scales monitor
/// rects down to 96 DPI at display scaling above 100%. winit's own later
/// call then fails quietly, which is fine.
pub fn become_dpi_aware() {
    win::become_dpi_aware();
}

/// The main window, so we can read and set its placement.
pub struct MainWindow {
    /// The window handle as a number (0 when unknown). A raw `HWND` is a
    /// pointer, which Rust won't let a struct share between threads; only the
    /// UI thread uses this anyway.
    #[cfg_attr(not(windows), allow(dead_code))]
    hwnd: isize,
}

impl MainWindow {
    /// Gets the window handle from eframe while the app is being created.
    pub fn new(cc: &eframe::CreationContext<'_>) -> MainWindow {
        MainWindow {
            hwnd: win::handle_of(cc),
        }
    }

    /// Moves the window to `placement`. Called once at startup, while the
    /// window is still hidden. A normal window stays hidden (eframe shows it
    /// after drawing the first frame); a maximized one is shown maximized on
    /// its monitor.
    pub fn restore(&self, placement: WindowPlacement) {
        win::set_placement(self.hwnd, placement);
    }

    /// Where the window is now. `None` if it can't be read.
    pub fn read(&self) -> Option<WindowPlacement> {
        win::get_placement(self.hwnd)
    }
}

#[cfg(windows)]
mod win {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MONITOR_DEFAULTTONULL, MONITORINFO, MonitorFromRect,
    };
    use windows_sys::Win32::UI::HiDpi::{
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        SetProcessDpiAwarenessContext,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowPlacement, SW_HIDE, SW_SHOWMAXIMIZED, SW_SHOWMINIMIZED, SetWindowPlacement,
        WINDOWPLACEMENT, WPF_RESTORETOMAXIMIZED,
    };

    use super::ScreenRect;
    use crate::layout::WindowPlacement;

    pub fn become_dpi_aware() {
        // Same as winit: per-monitor v2, or v1 on Windows 10 before 1703.
        // SAFETY: plain calls with constant arguments; failure changes nothing.
        unsafe {
            if SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) == 0 {
                SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE);
            }
        }
    }

    pub fn handle_of(cc: &eframe::CreationContext<'_>) -> isize {
        match cc.window_handle().map(|handle| handle.as_raw()) {
            Ok(RawWindowHandle::Win32(handle)) => handle.hwnd.get(),
            _ => 0,
        }
    }

    /// The work area of the monitor that overlaps `rect` the most, or `None`
    /// if `rect` isn't on any monitor.
    pub fn work_area_near(rect: ScreenRect) -> Option<ScreenRect> {
        let rect = RECT {
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        };
        // SAFETY: `rect` is a valid RECT that lives for the call.
        let monitor = unsafe { MonitorFromRect(&rect, MONITOR_DEFAULTTONULL) };
        if monitor.is_null() {
            return None;
        }
        // SAFETY: MONITORINFO is plain data, so all zeros is a valid value.
        let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        // SAFETY: `monitor` came from Windows just now and `info` has its size set.
        if unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 {
            return None;
        }
        let work = info.rcWork;
        Some(ScreenRect {
            left: work.left,
            top: work.top,
            right: work.right,
            bottom: work.bottom,
        })
    }

    pub fn set_placement(hwnd: isize, placement: WindowPlacement) {
        if hwnd == 0 {
            return;
        }
        let show = if placement.maximized {
            SW_SHOWMAXIMIZED
        } else {
            SW_HIDE
        };
        let wp = WINDOWPLACEMENT {
            length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
            flags: 0,
            showCmd: show as u32,
            // -1 means "let Windows choose" for the minimized/maximized corners.
            ptMinPosition: POINT { x: -1, y: -1 },
            ptMaxPosition: POINT { x: -1, y: -1 },
            rcNormalPosition: RECT {
                left: placement.x,
                top: placement.y,
                right: placement.x.saturating_add(placement.width),
                bottom: placement.y.saturating_add(placement.height),
            },
        };
        // SAFETY: `hwnd` is our own main window; `wp` is fully filled in.
        // If it fails, the window simply stays where it is.
        unsafe { SetWindowPlacement(hwnd as HWND, &wp) };
    }

    pub fn get_placement(hwnd: isize) -> Option<WindowPlacement> {
        if hwnd == 0 {
            return None;
        }
        // SAFETY: WINDOWPLACEMENT is plain data, so all zeros is a valid value.
        let mut wp: WINDOWPLACEMENT = unsafe { std::mem::zeroed() };
        wp.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
        // SAFETY: `wp` has its length set; a closed window just makes this fail.
        if unsafe { GetWindowPlacement(hwnd as HWND, &mut wp) } == 0 {
            return None;
        }
        // A minimized window remembers whether it goes back to maximized.
        // We never start minimized, so save that state instead.
        let maximized = wp.showCmd == SW_SHOWMAXIMIZED as u32
            || (wp.showCmd == SW_SHOWMINIMIZED as u32 && wp.flags & WPF_RESTORETOMAXIMIZED != 0);
        let r = wp.rcNormalPosition;
        Some(WindowPlacement {
            x: r.left,
            y: r.top,
            width: r.right - r.left,
            height: r.bottom - r.top,
            maximized,
        })
    }
}

/// Stand-ins for other systems, so the app still builds and runs there.
#[cfg(not(windows))]
mod win {
    use super::ScreenRect;
    use crate::layout::WindowPlacement;

    pub fn become_dpi_aware() {}

    pub fn handle_of(_cc: &eframe::CreationContext<'_>) -> isize {
        0
    }

    pub fn work_area_near(_rect: ScreenRect) -> Option<ScreenRect> {
        None
    }

    pub fn set_placement(_hwnd: isize, _placement: WindowPlacement) {}

    pub fn get_placement(_hwnd: isize) -> Option<WindowPlacement> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(x: i32, y: i32, width: i32, height: i32) -> WindowPlacement {
        WindowPlacement {
            x,
            y,
            width,
            height,
            maximized: false,
        }
    }

    const SCREEN: ScreenRect = ScreenRect {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1040,
    };

    #[test]
    fn window_on_screen_is_visible() {
        assert!(title_bar_visible(&place(100, 100, 1280, 720), SCREEN));
    }

    #[test]
    fn window_far_right_is_not_visible() {
        // Was on a second monitor that is gone now.
        assert!(!title_bar_visible(&place(2000, 100, 1280, 720), SCREEN));
    }

    #[test]
    fn window_mostly_off_the_left_edge_still_counts_if_enough_shows() {
        assert!(title_bar_visible(&place(-1100, 100, 1280, 720), SCREEN));
        assert!(!title_bar_visible(&place(-1250, 100, 1280, 720), SCREEN));
    }

    #[test]
    fn title_bar_above_the_screen_is_not_visible() {
        assert!(!title_bar_visible(&place(100, -25, 1280, 720), SCREEN));
    }

    #[test]
    fn tiny_or_huge_sizes_are_rejected() {
        assert!(sane_size(&place(0, 0, 1280, 720)));
        assert!(!sane_size(&place(0, 0, 0, 720)));
        assert!(!sane_size(&place(0, 0, 1280, -5)));
        assert!(!sane_size(&place(0, 0, 100_000, 720)));
    }

    #[test]
    fn nonsense_coordinates_do_not_overflow() {
        assert!(!title_bar_visible(
            &place(i32::MAX, i32::MAX, 1280, 720),
            SCREEN
        ));
    }
}
