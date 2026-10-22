//! Terminate-rights probe and the pieces to restart Explorer without admin rights.
use crate::{ActionError, OwnedHandle, is_elevated, open_verified};
use std::mem::size_of;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Security::{
    DuplicateTokenEx, SecurityImpersonation, TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_QUERY, TokenPrimary,
};
use windows::Win32::System::Threading::{
    CreateProcessW, CreateProcessWithTokenW, OpenProcessToken, PROCESS_INFORMATION,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE, STARTUPINFOW,
};
use windows::Win32::UI::WindowsAndMessaging::FindWindowW;
use windows::core::{PCWSTR, PWSTR, w};

/// A duplicate of the shell's primary token, to start Explorer again with the user's own rights.
/// The handle closes on drop.
pub struct ShellToken(OwnedHandle);

// SAFETY: a token handle may be used and closed from any thread.
unsafe impl Send for ShellToken {}

/// Whether this process can open `pid` with terminate rights. A process that already exited
/// counts as `true`: there is nothing to warn about.
pub fn can_terminate(pid: u32, create_time: i64) -> bool {
    match open_verified(pid, create_time, PROCESS_TERMINATE) {
        Ok(_) | Err(ActionError::AlreadyExited) => true,
        Err(_) => false,
    }
}

/// Copies the shell's primary token; `Ok(None)` when Negotium is not elevated (a plain start
/// then already runs with the user's rights). The shell is checked by (pid, create_time), so a
/// reused pid is never opened: an exited or reused pid is `AlreadyExited`.
pub fn capture_shell_token(shell_pid: u32, create_time: i64) -> Result<Option<ShellToken>, ActionError> {
    if !is_elevated() {
        return Ok(None);
    }
    let process = open_verified(shell_pid, create_time, PROCESS_QUERY_LIMITED_INFORMATION)?;
    let mut token = HANDLE::default();
    // SAFETY: valid process handle; `token` is writable and wrapped in OwnedHandle below.
    unsafe { OpenProcessToken(process.0, TOKEN_DUPLICATE | TOKEN_QUERY | TOKEN_ASSIGN_PRIMARY, &mut token) }
        .map_err(|e| ActionError::from_win(&e))?;
    let token = OwnedHandle(token);
    let mut primary = HANDLE::default();
    // SAFETY: valid token handle opened with TOKEN_DUPLICATE; `primary` is writable and wrapped
    // in OwnedHandle below.
    unsafe {
        DuplicateTokenEx(
            token.0,
            TOKEN_DUPLICATE | TOKEN_QUERY | TOKEN_ASSIGN_PRIMARY,
            None,
            SecurityImpersonation,
            TokenPrimary,
            &mut primary,
        )
    }
    .map_err(|e| ActionError::from_win(&e))?;
    Ok(Some(ShellToken(OwnedHandle(primary))))
}

/// Starts `<windows dir>\explorer.exe`, with `token` when given. Never searches PATH. When
/// Negotium is elevated and `capture_shell_token` failed, do not call this with `None`: that
/// would start an elevated shell.
pub fn start_explorer(token: Option<&ShellToken>) -> Result<(), ActionError> {
    let path = std::path::Path::new(&negotium_collector::windows_dir()).join("explorer.exe");
    let app: Vec<u16> = path.as_os_str().to_string_lossy().encode_utf16().chain(Some(0)).collect();
    let mut cmdline: Vec<u16> = format!("\"{}\"", path.display()).encode_utf16().chain(Some(0)).collect();
    let startup = STARTUPINFOW { cb: size_of::<STARTUPINFOW>() as u32, ..Default::default() };
    let mut info = PROCESS_INFORMATION::default();
    let result = match token {
        // SAFETY: the token is a live primary token; `app` and `cmdline` are NUL-terminated and
        // outlive the call; `startup` is initialised and `info` is writable.
        Some(token) => unsafe {
            CreateProcessWithTokenW(
                token.0.0,
                Default::default(),
                PCWSTR(app.as_ptr()),
                Some(PWSTR(cmdline.as_mut_ptr())),
                Default::default(),
                None,
                PCWSTR::null(),
                &startup,
                &mut info,
            )
        },
        // SAFETY: as above, with no token.
        None => unsafe {
            CreateProcessW(
                PCWSTR(app.as_ptr()),
                Some(PWSTR(cmdline.as_mut_ptr())),
                None,
                None,
                false,
                Default::default(),
                None,
                PCWSTR::null(),
                &startup,
                &mut info,
            )
        },
    };
    result.map_err(|e| ActionError::from_win(&e))?;
    // SAFETY: both handles were returned by the call above and are closed exactly once here.
    unsafe {
        let _ = CloseHandle(info.hProcess);
        let _ = CloseHandle(info.hThread);
    }
    Ok(())
}

/// Waits up to `timeout` for a `Shell_TrayWnd` window (polled every 100 ms). It is `true` while
/// the OLD taskbar still exists, so wait for the old shell process to exit first.
pub fn wait_for_taskbar(timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        // SAFETY: plain Win32 call with a static class name.
        if unsafe { FindWindowW(w!("Shell_TrayWnd"), PCWSTR::null()) }.is_ok() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
