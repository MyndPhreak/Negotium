//! Top-level window enumeration used for app grouping and "Not responding" detection.
use negotium_snapshot::WindowSample;
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GW_OWNER, GetWindow, GetWindowThreadProcessId, InternalGetWindowText,
    IsHungAppWindow, IsWindowVisible,
};
use windows::core::BOOL;

/// Visible, unowned, titled top-level windows.
pub fn scan_windows() -> Result<Vec<WindowSample>, String> {
    let mut out: Vec<WindowSample> = Vec::with_capacity(64);
    // SAFETY: `out` outlives the synchronous EnumWindows call that receives its address.
    unsafe { EnumWindows(Some(visit), LPARAM(&mut out as *mut Vec<WindowSample> as isize)) }
        .map_err(|e| format!("windows: EnumWindows failed: {e}"))?;
    Ok(out)
}

unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: lparam is the `&mut Vec<WindowSample>` passed by scan_windows.
    let out = unsafe { &mut *(lparam.0 as *mut Vec<WindowSample>) };
    // SAFETY: all calls below only read window state for a handle supplied by EnumWindows.
    // None of them sends a window message (IsHungAppWindow and InternalGetWindowText read
    // cached state), so none can block on a hung window, even one in this process.
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

/// Reads the window's stored title without sending any window message.
///
/// # Safety
/// `hwnd` must be a window handle from EnumWindows. `InternalGetWindowText` copies the
/// text cached in the window structure, so unlike `GetWindowText` it never sends
/// `WM_GETTEXT`, not even for windows owned by the calling process. It therefore cannot
/// block on a hung window.
unsafe fn window_title(hwnd: HWND) -> String {
    let mut buf = [0u16; 512];
    // SAFETY: `buf` is a valid writable buffer; its length is passed with the slice.
    let copied = unsafe { InternalGetWindowText(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..copied.max(0) as usize])
}
