use crate::{ActionError, OwnedHandle};
use std::ffi::c_void;
use std::mem::size_of;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::Win32::UI::Shell::{SHELLEXECUTEINFOW, ShellExecuteExW};
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

/// Relaunches the current executable with the `runas` verb (UAC prompt). On `Ok` the caller
/// should quit this instance.
pub fn restart_as_admin(args: &str) -> Result<(), ActionError> {
    let exe = std::env::current_exe().map_err(|e| ActionError::Other(e.to_string()))?;
    let file = HSTRING::from(exe.as_os_str());
    let params = HSTRING::from(args);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    // SAFETY: `info` is initialised and its strings outlive the call.
    unsafe { ShellExecuteExW(&mut info) }.map_err(|e| ActionError::from_win(&e))
}
