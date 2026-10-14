use crate::{ActionError, guard_critical, handle_breaks_on_termination, open_verified};
use std::time::Duration;
use windows::Win32::Foundation::{HWND, LPARAM, WAIT_OBJECT_0, WPARAM};
use windows::Win32::System::Threading::{PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, TerminateProcess, WaitForSingleObject};
use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowThreadProcessId, IsWindowVisible, PostMessageW, WM_CLOSE};
use windows::core::BOOL;

pub const FORCE_CLOSE_GRACE: Duration = Duration::from_secs(3);

/// Asks every visible top-level window of the process to close, waits up to
/// `FORCE_CLOSE_GRACE`, then terminates whatever is left.
pub fn force_close(pid: u32, create_time: i64) -> Result<(), ActionError> {
    let handle = open_verified(pid, create_time, PROCESS_TERMINATE | PROCESS_SYNCHRONIZE)?;
    // Refused up front: a process Windows marks critical is neither asked to close nor ended.
    guard_critical(handle_breaks_on_termination(&handle))?;
    for hwnd in windows_of(pid) {
        // SAFETY: asks the window to close; PostMessage never blocks on a hung window.
        unsafe {
            let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }
    // SAFETY: verified handle opened with SYNCHRONIZE.
    if unsafe { WaitForSingleObject(handle.0, FORCE_CLOSE_GRACE.as_millis() as u32) } == WAIT_OBJECT_0 {
        return Ok(());
    }
    // SAFETY: verified handle opened with PROCESS_TERMINATE.
    match unsafe { TerminateProcess(handle.0, 1) } {
        Ok(()) => Ok(()),
        // The process may have exited between the wait timing out and the terminate call.
        // SAFETY: verified handle opened with SYNCHRONIZE.
        Err(_) if unsafe { WaitForSingleObject(handle.0, 0) } == WAIT_OBJECT_0 => Ok(()),
        Err(e) => Err(ActionError::from_win(&e)),
    }
}

fn windows_of(pid: u32) -> Vec<HWND> {
    struct Search {
        pid: u32,
        found: Vec<HWND>,
    }
    unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: lparam is the `&mut Search` passed below for the duration of EnumWindows.
        let search = unsafe { &mut *(lparam.0 as *mut Search) };
        let mut owner = 0u32;
        // SAFETY: read-only queries on a handle supplied by EnumWindows.
        unsafe {
            GetWindowThreadProcessId(hwnd, Some(&mut owner));
            if owner == search.pid && IsWindowVisible(hwnd).as_bool() {
                search.found.push(hwnd);
            }
        }
        true.into()
    }
    let mut search = Search { pid, found: Vec::new() };
    // SAFETY: `search` outlives the synchronous enumeration.
    unsafe {
        let _ = EnumWindows(Some(visit), LPARAM(&mut search as *mut Search as isize));
    }
    search.found
}
