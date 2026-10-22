//! Win32 side of the picker: lists top-level windows in z-order as plain `WindowInfo`
//! records, finds the shell process and lists monitors. Nothing here sends a window message,
//! so a hung window can never block the picker.
use std::ffi::c_void;

use negotium_collector::original_of_ghost;
use windows::Win32::Foundation::{CloseHandle, FILETIME, HWND, LPARAM, RECT};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO};
use windows::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FindWindowExW, FindWindowW, GetClassNameW, GetShellWindow, GetWindowRect,
    GetWindowThreadProcessId, InternalGetWindowText, IsHungAppWindow, IsIconic, IsWindow, IsWindowVisible,
};
use windows::core::{BOOL, PCWSTR, PWSTR, w};

use super::target::{Rect, WindowInfo};

const UWP_FRAME_CLASS: &str = "ApplicationFrameWindow";
/// Window class DWM gives the stand-in of a hung window.
const GHOST_CLASS: &str = "Ghost";

/// One display: its rectangle in virtual-screen pixels and its effective DPI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Monitor {
    pub rect: Rect,
    pub dpi: u32,
}

/// Top-level windows, owned ones included, top of the z-order first. Visible, cloaked and
/// minimised windows are all listed; `classify` filters them.
pub fn top_level_windows() -> Vec<WindowInfo> {
    let mut out: Vec<WindowInfo> = Vec::with_capacity(64);
    // SAFETY: `out` outlives the synchronous EnumWindows call that receives its address.
    // A failed enumeration leaves whatever was collected, which is the best answer available.
    let _ = unsafe { EnumWindows(Some(visit), LPARAM(&mut out as *mut Vec<WindowInfo> as isize)) };
    out
}

unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: lparam is the `&mut Vec<WindowInfo>` passed by top_level_windows.
    let out = unsafe { &mut *(lparam.0 as *mut Vec<WindowInfo>) };
    // SAFETY: hwnd comes from EnumWindows; window_info sends no window message.
    if let Some(info) = unsafe { window_info(hwnd) } {
        out.push(info);
    }
    true.into()
}

/// Reads one window, with its own process id (an owned window may belong to another process
/// than its owner). `None` for a `Ghost` whose original is unknown.
///
/// # Safety
/// `hwnd` must be a window handle from EnumWindows. Every call below reads state cached by
/// user32 or DWM: `InternalGetWindowText` (not `GetWindowText`), `GetClassNameW`,
/// `IsHungAppWindow`, `IsIconic`, `DwmGetWindowAttribute`, `FindWindowExW`. None sends a
/// window message, so none can block on a hung window, even one in this process.
unsafe fn window_info(hwnd: HWND) -> Option<WindowInfo> {
    // SAFETY: see the function contract.
    unsafe {
        let class = class_name(hwnd);
        // A ghost is reported with the original window's pid and as hung; its frame is its own.
        let (mut source, hung) = if class == GHOST_CLASS {
            (original_of_ghost(hwnd)?, true)
        } else {
            (hwnd, IsHungAppWindow(hwnd).as_bool())
        };
        let mut uwp_unresolved = false;
        if class == UWP_FRAME_CLASS {
            match FindWindowExW(Some(hwnd), None, w!("Windows.UI.Core.CoreWindow"), None) {
                Ok(core) if !core.is_invalid() => source = core,
                _ => uwp_unresolved = true,
            }
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(source, Some(&mut pid));
        let mut buf = [0u16; 512];
        let copied = InternalGetWindowText(hwnd, &mut buf).max(0) as usize;
        let mut cloak = 0u32;
        let cloaked =
            DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, (&mut cloak as *mut u32).cast(), size_of::<u32>() as u32).is_ok() && cloak != 0;
        Some(WindowInfo {
            hwnd: hwnd.0 as isize,
            pid,
            create_time: create_time(pid),
            class,
            title: String::from_utf16_lossy(&buf[..copied]),
            frame: frame(hwnd),
            visible: IsWindowVisible(hwnd).as_bool(),
            cloaked,
            minimized: IsIconic(hwnd).as_bool(),
            hung,
            uwp_unresolved,
        })
    }
}

/// # Safety
/// `hwnd` must be a window handle. `GetClassNameW` reads the class atom; it sends no message.
unsafe fn class_name(hwnd: HWND) -> String {
    let mut class = [0u16; 256];
    // SAFETY: `class` is a valid writable buffer; its length is passed with the slice.
    let len = unsafe { GetClassNameW(hwnd, &mut class) }.max(0) as usize;
    String::from_utf16_lossy(&class[..len])
}

/// The visible frame (without the invisible resize border), falling back to the window rect.
///
/// # Safety
/// `hwnd` must be a window handle.
unsafe fn frame(hwnd: HWND) -> Rect {
    let mut r = RECT::default();
    // SAFETY: `r` is a valid RECT and its size is passed.
    let dwm = unsafe {
        DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, (&mut r as *mut RECT).cast::<c_void>(), size_of::<RECT>() as u32)
    };
    // SAFETY: valid window handle and out pointer.
    if dwm.is_err() && unsafe { GetWindowRect(hwnd, &mut r) }.is_err() {
        r = RECT::default();
    }
    Rect { left: r.left, top: r.top, right: r.right, bottom: r.bottom }
}

/// Process create time as FILETIME 100 ns units; 0 when the process cannot be opened.
fn create_time(pid: u32) -> i64 {
    if pid == 0 {
        return 0;
    }
    // SAFETY: the handle is used for the one GetProcessTimes call below and then closed.
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return 0;
        };
        let (mut created, mut exit, mut kernel, mut user) =
            (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        let ok = GetProcessTimes(h, &mut created, &mut exit, &mut kernel, &mut user).is_ok();
        let _ = CloseHandle(h);
        if ok { (i64::from(created.dwHighDateTime) << 32) | i64::from(created.dwLowDateTime) } else { 0 }
    }
}

/// Process id and create time (FILETIME, 100 ns units) of the shell window's process.
pub fn shell_pid() -> Option<(u32, i64)> {
    // SAFETY: GetShellWindow returns the shell's HWND or NULL; the pid out pointer is valid.
    let pid = unsafe {
        let shell = GetShellWindow();
        if shell.is_invalid() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(shell, Some(&mut pid));
        pid
    };
    (pid != 0).then(|| (pid, create_time(pid)))
}

/// The file name of the process's image (`notepad.exe`); "" when it cannot be opened.
pub fn image_name(pid: u32) -> String {
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    // SAFETY: the handle is used for the one query below and then closed; `buf` and `len`
    // describe a valid writable buffer.
    let ok = unsafe {
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return String::new();
        };
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(h);
        ok
    };
    if !ok {
        return String::new();
    }
    let path = String::from_utf16_lossy(&buf[..len as usize]);
    path.rsplit('\\').next().unwrap_or_default().to_string()
}

/// The primary taskbar window, if there is one.
pub fn taskbar() -> Option<isize> {
    // SAFETY: plain Win32 call with a static class name.
    unsafe { FindWindowW(w!("Shell_TrayWnd"), PCWSTR::null()) }.ok().map(|h| h.0 as isize)
}

/// Waits up to `timeout` (polled every 100 ms) for `hwnd` to be destroyed.
pub fn wait_window_gone(hwnd: isize, timeout: std::time::Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    // SAFETY: IsWindow accepts any value; a destroyed or reused handle only changes the answer.
    while unsafe { IsWindow(Some(HWND(hwnd as *mut c_void))) }.as_bool() {
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    true
}

pub fn monitors() -> Vec<Monitor> {
    let mut out: Vec<Monitor> = Vec::new();
    // SAFETY: `out` outlives the synchronous EnumDisplayMonitors call that receives its address.
    let _ = unsafe { EnumDisplayMonitors(None, None, Some(visit_monitor), LPARAM(&mut out as *mut Vec<Monitor> as isize)) };
    out
}

unsafe extern "system" fn visit_monitor(monitor: HMONITOR, _dc: HDC, _rect: *mut RECT, lparam: LPARAM) -> BOOL {
    // SAFETY: lparam is the `&mut Vec<Monitor>` passed by monitors().
    let out = unsafe { &mut *(lparam.0 as *mut Vec<Monitor>) };
    let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
    // SAFETY: `info` has cbSize set; the DPI out pointers are valid stack u32s.
    unsafe {
        if GetMonitorInfoW(monitor, &mut info).as_bool() {
            let (mut dx, mut dy) = (96u32, 96u32);
            let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
            let r = info.rcMonitor;
            out.push(Monitor { rect: Rect { left: r.left, top: r.top, right: r.right, bottom: r.bottom }, dpi: dx });
        }
    }
    true.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, GetWindowRect, WINDOW_EX_STYLE, WS_OVERLAPPED, WS_VISIBLE,
    };
    use windows::core::{PCWSTR, w};

    fn make(title: PCWSTR, owner: Option<HWND>, x: i32) -> HWND {
        // SAFETY: plain window creation with a system class; destroyed by the test.
        unsafe {
            CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), title, WS_OVERLAPPED | WS_VISIBLE, x, 100, 200, 150, owner, None, None, None)
        }
        .expect("create test window")
    }

    fn destroy(hwnds: &[HWND]) {
        for h in hwnds {
            // SAFETY: our own test windows.
            unsafe {
                let _ = DestroyWindow(*h);
            }
        }
    }

    fn find(list: &[WindowInfo], h: HWND) -> Option<usize> {
        list.iter().position(|w| w.hwnd == h.0 as isize)
    }

    #[test]
    fn lists_a_visible_test_window_with_its_class_and_frame() {
        let h = make(w!("negotium-picker-a"), None, 100);
        let list = top_level_windows();
        let mut rect = RECT::default();
        // SAFETY: valid window and out pointer.
        unsafe { GetWindowRect(h, &mut rect).unwrap() };
        destroy(&[h]);
        let w = &list[find(&list, h).expect("listed")];
        assert_eq!(w.class, "Static");
        assert_eq!(w.title, "negotium-picker-a");
        assert_eq!(w.pid, std::process::id());
        assert!(w.create_time > 0);
        assert!(w.visible && !w.cloaked && !w.minimized && !w.hung && !w.uwp_unresolved);
        assert!(w.frame.right > w.frame.left && w.frame.bottom > w.frame.top);
        let slack = 16;
        assert!(w.frame.left >= rect.left - slack && w.frame.right <= rect.right + slack);
        assert!(w.frame.top >= rect.top - slack && w.frame.bottom <= rect.bottom + slack);
    }

    #[test]
    fn a_newer_window_is_listed_before_an_older_one() {
        let old = make(w!("negotium-picker-old"), None, 100);
        let new = make(w!("negotium-picker-new"), None, 120);
        let list = top_level_windows();
        destroy(&[old, new]);
        assert!(find(&list, new).expect("new listed") < find(&list, old).expect("old listed"));
    }

    #[test]
    fn owned_windows_are_listed_above_their_owner() {
        let owner = make(w!("negotium-picker-owner"), None, 100);
        let owned = make(w!("negotium-picker-owned"), Some(owner), 120);
        let list = top_level_windows();
        destroy(&[owned, owner]);
        let owned_at = find(&list, owned).expect("owned listed");
        assert!(owned_at < find(&list, owner).expect("owner listed"));
        assert_eq!(list[owned_at].title, "negotium-picker-owned");
        assert_eq!(list[owned_at].pid, std::process::id());
    }

    #[test]
    fn monitors_cover_the_primary_monitor() {
        let list = monitors();
        assert!(!list.is_empty());
        assert!(list.iter().any(|m| m.rect.left <= 0 && m.rect.top <= 0 && m.rect.right > 0 && m.rect.bottom > 0));
        assert!(list.iter().all(|m| m.dpi >= 96));
    }
}
