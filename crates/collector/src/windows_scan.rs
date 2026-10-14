//! Top-level window enumeration used for app grouping and "Not responding" detection.
use negotium_snapshot::WindowSample;
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GW_OWNER, GetWindow, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
    IsHungAppWindow, IsWindowVisible,
};
use windows::core::BOOL;

/// Visible, unowned, titled top-level windows.
pub fn scan_windows() -> Vec<WindowSample> {
    let mut out: Vec<WindowSample> = Vec::with_capacity(64);
    // SAFETY: `out` outlives the synchronous EnumWindows call that receives its address.
    unsafe {
        let _ = EnumWindows(Some(visit), LPARAM(&mut out as *mut Vec<WindowSample> as isize));
    }
    out
}

unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: lparam is the `&mut Vec<WindowSample>` passed by scan_windows.
    let out = unsafe { &mut *(lparam.0 as *mut Vec<WindowSample>) };
    // SAFETY: all calls below only read window state for a handle supplied by EnumWindows.
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() {
            return true.into();
        }
        if GetWindow(hwnd, GW_OWNER).is_ok_and(|owner| !owner.is_invalid()) {
            return true.into();
        }
        let title = window_title(hwnd);
        if title.is_empty() {
            return true.into();
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        out.push(WindowSample { pid, hung: IsHungAppWindow(hwnd).as_bool(), title });
    }
    true.into()
}

/// # Safety
/// `hwnd` must be a window handle from EnumWindows. GetWindowText does not send
/// messages to other processes, so it cannot block on a hung window.
unsafe fn window_title(hwnd: HWND) -> String {
    // SAFETY: caller guarantees `hwnd` came from EnumWindows.
    let len = unsafe { GetWindowTextLengthW(hwnd) };
    if len <= 0 {
        return String::new();
    }
    let mut buf = vec![0u16; len as usize + 1];
    // SAFETY: `buf` is a valid writable buffer of the length passed implicitly by the slice.
    let copied = unsafe { GetWindowTextW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..copied.max(0) as usize])
}
