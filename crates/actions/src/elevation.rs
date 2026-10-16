use crate::{ActionError, OwnedHandle};
use std::ffi::c_void;
use std::mem::size_of;
use windows::Win32::Foundation::{ERROR_NOT_ALL_ASSIGNED, GetLastError, HANDLE, HWND, LUID};
use windows::Win32::Security::{
    AdjustTokenPrivileges, GetTokenInformation, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW, SE_DEBUG_NAME,
    SE_PRIVILEGE_ENABLED, TOKEN_ADJUST_PRIVILEGES, TOKEN_ELEVATION, TOKEN_PRIVILEGES, TOKEN_QUERY, TokenElevation,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::Win32::UI::Shell::{SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, ShellExecuteExW};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{HSTRING, PCWSTR, w};

/// Whether this process runs with an elevated (administrator) token.
pub fn is_elevated() -> bool {
    let mut token = HANDLE::default();
    // SAFETY: opens our own token for query; wrapped in OwnedHandle below.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.is_err() {
        return false;
    }
    let token = OwnedHandle(token);
    let mut elevation = TOKEN_ELEVATION::default();
    let mut len = 0u32;
    // SAFETY: `elevation` is a writable TOKEN_ELEVATION of the size passed.
    let ok = unsafe {
        GetTokenInformation(
            token.0,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut c_void),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
    }
    .is_ok();
    ok && elevation.TokenIsElevated != 0
}

/// Enables SeDebugPrivilege on this process's token, so an elevated instance can open and act on
/// other users' and service processes the way Task Manager does. Only an elevated token holds the
/// privilege; without it this returns an error and nothing changes.
pub fn enable_debug_privilege() -> Result<(), ActionError> {
    let mut token = HANDLE::default();
    // SAFETY: opens our own token; wrapped in OwnedHandle below.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut token) }
        .map_err(|e| ActionError::from_win(&e))?;
    let token = OwnedHandle(token);
    let mut luid = LUID::default();
    // SAFETY: SE_DEBUG_NAME is a static wide string; `luid` is writable.
    unsafe { LookupPrivilegeValueW(None, SE_DEBUG_NAME, &mut luid) }.map_err(|e| ActionError::from_win(&e))?;
    let privileges = TOKEN_PRIVILEGES {
        PrivilegeCount: 1,
        Privileges: [LUID_AND_ATTRIBUTES { Luid: luid, Attributes: SE_PRIVILEGE_ENABLED }],
    };
    // SAFETY: `privileges` is a fully initialised TOKEN_PRIVILEGES with one entry.
    unsafe { AdjustTokenPrivileges(token.0, false, Some(&privileges), 0, None, None) }
        .map_err(|e| ActionError::from_win(&e))?;
    // AdjustTokenPrivileges succeeds even when the token lacks the privilege; that case is only
    // visible through the last error.
    // SAFETY: plain thread-local error read, immediately after the call above.
    if unsafe { GetLastError() } == ERROR_NOT_ALL_ASSIGNED {
        return Err(ActionError::Other("the token does not hold SeDebugPrivilege".into()));
    }
    Ok(())
}

/// Relaunches the current executable with the `runas` verb (UAC prompt). On `Ok` the caller
/// should quit this instance. Returns only once the prompt is answered, so call it off the UI
/// thread; the calling thread gets a COM apartment for the call (ShellExecuteEx needs one).
/// `owner` is the main window's HWND as an integer (0: none), so the UAC prompt can come to the
/// front instead of flashing in the taskbar.
pub fn restart_as_admin(args: &str, owner: isize) -> Result<(), ActionError> {
    let _com = crate::startup::ComApartment::enter();
    let exe = std::env::current_exe().map_err(|e| ActionError::Other(e.to_string()))?;
    let file = HSTRING::from(exe.as_os_str());
    let params = HSTRING::from(args);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        // Only the UAC prompt's owner (z-order): 0 means none, and a destroyed handle is tolerated.
        hwnd: HWND(owner as *mut c_void),
        // The caller exits right after: the launch must complete inside this call.
        fMask: SEE_MASK_NOASYNC,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    // SAFETY: `info` is initialised and its strings outlive the call.
    unsafe { ShellExecuteExW(&mut info) }.map_err(|e| ActionError::from_win(&e))
}
