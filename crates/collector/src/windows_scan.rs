//! Top-level window enumeration used for app grouping and "Not responding" detection.
use negotium_snapshot::WindowSample;
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GW_OWNER, GetClassNameW, GetWindow, GetWindowThreadProcessId,
    InternalGetWindowText, IsHungAppWindow, IsWindowVisible,
};
use std::sync::OnceLock;
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::core::{BOOL, s, w};

type HungWindowFromGhostWindowFn = unsafe extern "system" fn(HWND) -> HWND;

/// `HungWindowFromGhostWindow` is exported by user32.dll (Task Manager imports it) but is
/// absent from the SDK import library, so it cannot be linked statically; resolve it once.
fn hung_window_from_ghost_window() -> Option<HungWindowFromGhostWindowFn> {
    static RESOLVED: OnceLock<Option<HungWindowFromGhostWindowFn>> = OnceLock::new();
    *RESOLVED.get_or_init(|| {
        // SAFETY: user32.dll is already loaded in every process that links user32 (this one
        // does); GetModuleHandleW adds no reference. The export, when present, has the
        // signature `HWND HungWindowFromGhostWindow(HWND)`, so the transmute is sound.
        unsafe {
            let user32 = GetModuleHandleW(w!("user32.dll")).ok()?;
            let proc = GetProcAddress(user32, s!("HungWindowFromGhostWindow"))?;
            Some(std::mem::transmute::<unsafe extern "system" fn() -> isize, HungWindowFromGhostWindowFn>(proc))
        }
    })
}

/// UTF-16 for the window class DWM gives the stand-in of a hung window.
const GHOST_CLASS: [u16; 5] = [b'G' as u16, b'h' as u16, b'o' as u16, b's' as u16, b't' as u16];

pub(crate) fn is_ghost_class(class: &[u16]) -> bool {
    class == GHOST_CLASS
}

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
    // GetClassNameW reads the window's class atom and HungWindowFromGhostWindow reads
    // user32's ghost table; neither sends a window message. A `Ghost` whose original cannot be
    // resolved is skipped (it is owned by dwm.exe and must never become an app row).
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
        let mut class = [0u16; 64];
        let class_len = GetClassNameW(hwnd, &mut class).max(0) as usize;
        let (source, hung) = if is_ghost_class(&class[..class_len]) {
            let original = match hung_window_from_ghost_window() {
                Some(f) => f(hwnd),
                None => HWND::default(),
            };
            if original.is_invalid() {
                return true.into();
            }
            (original, true)
        } else {
            (hwnd, IsHungAppWindow(hwnd).as_bool())
        };
        let mut pid = 0u32;
        GetWindowThreadProcessId(source, Some(&mut pid));
        if pid == 0 {
            return true.into();
        }
        out.push(WindowSample { pid, hung, title });
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ghost_class_name_is_recognised() {
        assert!(is_ghost_class(&"Ghost".encode_utf16().collect::<Vec<_>>()));
        assert!(!is_ghost_class(&"Chrome_WidgetWin_1".encode_utf16().collect::<Vec<_>>()));
        assert!(!is_ghost_class(&[]));
    }

    #[test]
    fn hung_window_from_ghost_window_resolves() {
        assert!(hung_window_from_ghost_window().is_some());
    }
}
