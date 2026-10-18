use crate::{ActionError, OwnedHandle};
use std::ffi::{OsStr, c_void};
use std::path::Path;
use std::mem::size_of;
use windows::Win32::Foundation::{ERROR_NOT_ALL_ASSIGNED, GetLastError, HANDLE, HWND, LUID};
use windows::Win32::Security::{
    AdjustTokenPrivileges, GetTokenInformation, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW, SE_DEBUG_NAME,
    SE_PRIVILEGE_ENABLED, TOKEN_ADJUST_PRIVILEGES, TOKEN_ELEVATION, TOKEN_PRIVILEGES, TOKEN_QUERY, TokenElevation,
};
use windows::Win32::Foundation::{WAIT_EVENT, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::System::Threading::{GetCurrentProcess, GetExitCodeProcess, INFINITE, OpenProcessToken, WaitForSingleObject};
use std::time::Duration;
use windows::Win32::UI::Shell::{SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
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
    let exe = std::env::current_exe().map_err(|e| ActionError::Other(e.to_string()))?;
    shell_execute(w!("runas"), exe.as_os_str(), args, owner, false).map(drop)
}

/// Opens `path` the way Explorer would (the `open` verb), so a program whose manifest asks for
/// elevation gets its UAC prompt. Returns once the launch is done; call it off the UI thread.
pub fn shell_open(path: &Path, owner: isize) -> Result<(), ActionError> {
    shell_execute(w!("open"), path.as_os_str(), "", owner, false).map(drop)
}

/// Starts `exe` with `args` elevated (the `runas` verb: one UAC prompt) and doesn't wait for it:
/// `Ok` once it has started, `Cancelled` if the prompt was declined (nothing runs). Returns once
/// the prompt is answered; call it off the UI thread. `owner` as for [`restart_as_admin`].
pub fn run_elevated_detached(exe: &Path, args: &str, owner: isize) -> Result<(), ActionError> {
    shell_execute(w!("runas"), exe.as_os_str(), args, owner, false).map(drop)
}

/// How long [`run_elevated`] waits for the elevated command once it runs (the UAC prompt before
/// it is not counted): a command that hangs ends the wait with an error, so the caller is never
/// stuck.
pub const ELEVATED_WAIT: Duration = Duration::from_secs(30);

/// Runs the current executable with `args` elevated (the `runas` verb: a UAC prompt unless this
/// process is already elevated) and waits for it to finish: `Ok` once it exits with code 0, an
/// error for any other exit code or once [`ELEVATED_WAIT`] has passed. A declined prompt is
/// `Cancelled` and nothing runs. Blocks, so call it off the UI thread; `owner` as for
/// [`restart_as_admin`].
pub fn run_elevated(args: &str, owner: isize) -> Result<(), ActionError> {
    let exe = std::env::current_exe().map_err(|e| ActionError::Other(e.to_string()))?;
    run_elevated_exe(&exe, args, owner, || {})
}

/// The elevated process, watched from another thread after the wait has run out.
struct Watched(OwnedHandle);

// SAFETY: a process handle may be waited on and closed from any thread; only the watcher thread
// uses it once it is moved there.
unsafe impl Send for Watched {}

/// [`run_elevated`] for `exe` (a development build asks the installed copy). A command still
/// running after [`ELEVATED_WAIT`] is watched on its own thread, and `exited_late` runs there once
/// it exits (the caller reads the machine again).
pub fn run_elevated_exe(
    exe: &Path,
    args: &str,
    owner: isize,
    exited_late: impl FnOnce() + Send + 'static,
) -> Result<(), ActionError> {
    let process = shell_execute(w!("runas"), exe.as_os_str(), args, owner, true)?
        .ok_or_else(|| ActionError::Other("the elevated process could not be watched".into()))?;
    let handle = process.0;
    finish(
        // SAFETY: `handle` is the live process handle ShellExecuteEx returned (owned by
        // `process`, alive for this call); waiting has no other effect.
        |timeout| unsafe { WaitForSingleObject(handle, timeout.as_millis() as u32) },
        || {
            let mut code = 0u32;
            // SAFETY: the process has exited (waited); `code` is writable.
            unsafe { GetExitCodeProcess(handle, &mut code) }.map_err(|e| ActionError::from_win(&e))?;
            Ok(code)
        },
        move || {
            let watched = Watched(process);
            std::thread::spawn(move || {
                // SAFETY: the handle is owned by `watched` (closed when it drops, after the
                // wait); waiting has no other effect.
                let _ = unsafe { WaitForSingleObject(watched.0.0, INFINITE) };
                drop(watched);
                exited_late();
            });
        },
    )
}

/// An elevated command's outcome: `wait` for it (at most [`ELEVATED_WAIT`]), then its exit code.
/// A process still running then is handed to `still_running` (to be watched until it exits).
fn finish(
    wait: impl FnOnce(Duration) -> WAIT_EVENT,
    exit_code: impl FnOnce() -> Result<u32, ActionError>,
    still_running: impl FnOnce(),
) -> Result<(), ActionError> {
    match wait(ELEVATED_WAIT) {
        WAIT_OBJECT_0 => exit_status(exit_code()?),
        WAIT_TIMEOUT => {
            still_running();
            Err(ActionError::Other(format!(
                "the elevated command didn't finish within {} seconds",
                ELEVATED_WAIT.as_secs()
            )))
        }
        _ => Err(ActionError::Other("waiting for the elevated process failed".into())),
    }
}

/// An elevated command's exit code as a result (0 is success).
fn exit_status(code: u32) -> Result<(), ActionError> {
    if code == 0 { Ok(()) } else { Err(ActionError::Other(format!("the elevated command failed (exit code {code})"))) }
}

/// ShellExecuteEx `verb` on `file` with `args`; with `keep_process` it returns the new process's
/// handle (`None` if Windows gave none). The calling thread gets a COM apartment for the call.
fn shell_execute(
    verb: PCWSTR,
    file: &OsStr,
    args: &str,
    owner: isize,
    keep_process: bool,
) -> Result<Option<OwnedHandle>, ActionError> {
    let _com = crate::startup::ComApartment::enter();
    let file = HSTRING::from(file);
    let params = HSTRING::from(args);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        // Only the UAC prompt's owner (z-order): 0 means none, and a destroyed handle is tolerated.
        hwnd: HWND(owner as *mut c_void),
        // The caller may exit right after: the launch must complete inside this call.
        fMask: if keep_process { SEE_MASK_NOASYNC | SEE_MASK_NOCLOSEPROCESS } else { SEE_MASK_NOASYNC },
        lpVerb: verb,
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    // SAFETY: `info` is initialised and its strings outlive the call.
    unsafe { ShellExecuteExW(&mut info) }.map_err(|e| ActionError::from_win(&e))?;
    // With SEE_MASK_NOCLOSEPROCESS the handle (if any) is ours to close: OwnedHandle does.
    Ok((keep_process && !info.hProcess.is_invalid()).then_some(OwnedHandle(info.hProcess)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_elevated_commands_exit_code_is_its_result() {
        assert_eq!(exit_status(0), Ok(()));
        assert_eq!(exit_status(1), Err(ActionError::Other("the elevated command failed (exit code 1)".into())));
        assert!(exit_status(2).is_err());
    }

    #[test]
    fn a_hung_elevated_command_ends_the_wait_and_is_watched() {
        let watched = std::cell::Cell::new(false);
        let timed_out = finish(
            |timeout| {
                assert_eq!(timeout, Duration::from_secs(30));
                WAIT_TIMEOUT
            },
            || panic!("no exit code while it still runs"),
            || watched.set(true),
        );
        assert_eq!(timed_out, Err(ActionError::Other("the elevated command didn't finish within 30 seconds".into())));
        assert!(watched.get(), "it is watched until it exits");
        let never = || panic!("only a running command is watched");
        assert_eq!(finish(|_| WAIT_OBJECT_0, || Ok(0), never), Ok(()));
        assert!(finish(|_| WAIT_OBJECT_0, || Ok(3), never).is_err());
        assert!(finish(|_| windows::Win32::Foundation::WAIT_FAILED, || Ok(0), never).is_err());
    }
}
