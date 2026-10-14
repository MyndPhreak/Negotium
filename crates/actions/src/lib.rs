//! Process actions. Every call verifies identity by (pid, create_time) before acting,
//! so a PID reused since the last tick is never touched. Call off the UI thread.
mod close;
mod details;
mod efficiency;
mod elevation;
mod location;
mod terminate;

pub use close::{FORCE_CLOSE_GRACE, force_close};
pub use details::{Details, process_details};
pub use efficiency::toggle_efficiency_mode;
pub use elevation::{enable_debug_privilege, is_elevated, restart_as_admin};
pub use location::open_file_location;
pub use terminate::{end_many, end_process};

use std::ffi::c_void;
use std::mem::size_of;
use windows::Wdk::System::Threading::{NtQueryInformationProcess, ProcessBreakOnTermination};
use windows::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, ERROR_INVALID_PARAMETER, FILETIME, HANDLE};
use windows::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, PROCESS_ACCESS_RIGHTS, PROCESS_QUERY_LIMITED_INFORMATION,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActionError {
    AlreadyExited,
    AccessDenied,
    /// Windows flags the process BreakOnTermination: ending it bugchecks the machine.
    Critical,
    Other(String),
}

impl ActionError {
    pub fn message(&self, what: &str, elevated: bool) -> String {
        match self {
            ActionError::AlreadyExited => format!("{what} already exited."),
            ActionError::AccessDenied if elevated => format!("Couldn't change {what}: Windows protects this process."),
            ActionError::AccessDenied => {
                format!("Couldn't change {what}: access denied. Restart Negotium as admin and try again.")
            }
            ActionError::Critical => {
                "Windows marks this process as critical \u{2014} ending it would crash the PC.".to_string()
            }
            ActionError::Other(detail) => format!("Couldn't change {what}: {detail}."),
        }
    }

    pub(crate) fn from_win(e: &windows::core::Error) -> Self {
        let code = e.code();
        if code == ERROR_ACCESS_DENIED.to_hresult() {
            ActionError::AccessDenied
        } else if code == ERROR_INVALID_PARAMETER.to_hresult() {
            ActionError::AlreadyExited
        } else {
            ActionError::Other(e.message().trim().to_string())
        }
    }
}

pub const CRITICAL_PROCESSES: &[&str] = &["csrss", "wininit", "winlogon", "lsass", "smss", "services", "System"];

pub fn is_critical(name: &str) -> bool {
    let stem = name.strip_suffix(".exe").or_else(|| name.strip_suffix(".EXE")).unwrap_or(name);
    CRITICAL_PROCESSES.iter().any(|c| c.eq_ignore_ascii_case(stem))
}

pub(crate) struct OwnedHandle(pub(crate) HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: the handle came from OpenProcess and is closed exactly once here.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// Opens `pid` with `access` (plus query rights) and checks its creation time.
pub(crate) fn open_verified(pid: u32, create_time: i64, access: PROCESS_ACCESS_RIGHTS) -> Result<OwnedHandle, ActionError> {
    // SAFETY: plain Win32 call; the handle is owned by OwnedHandle.
    let handle = unsafe { OpenProcess(access | PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
        .map(OwnedHandle)
        .map_err(|e| ActionError::from_win(&e))?;
    let (mut created, mut exited, mut kernel, mut user) =
        (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    // SAFETY: valid handle and four writable FILETIMEs.
    unsafe { GetProcessTimes(handle.0, &mut created, &mut exited, &mut kernel, &mut user) }
        .map_err(|e| ActionError::from_win(&e))?;
    // A zombie (exited, but still held open by someone) has a nonzero exit time; acting on it
    // would fail with a misleading access-denied, so report it as exited.
    if exited.dwHighDateTime != 0 || exited.dwLowDateTime != 0 {
        return Err(ActionError::AlreadyExited);
    }
    let actual = ((created.dwHighDateTime as i64) << 32) | created.dwLowDateTime as i64;
    if actual != create_time {
        return Err(ActionError::AlreadyExited);
    }
    Ok(handle)
}

/// Whether Windows marks the process BreakOnTermination (ending it bugchecks the machine).
/// `None` when the flag can't be read (typically access denied).
pub(crate) fn handle_breaks_on_termination(handle: &OwnedHandle) -> Option<bool> {
    let mut flag = 0u32;
    // SAFETY: valid process handle; `flag` is a writable ULONG of the size passed.
    let status = unsafe {
        NtQueryInformationProcess(
            handle.0,
            ProcessBreakOnTermination,
            &mut flag as *mut u32 as *mut c_void,
            size_of::<u32>() as u32,
            std::ptr::null_mut(),
        )
    };
    status.is_ok().then_some(flag != 0)
}

/// Reads the BreakOnTermination flag of a verified process. Read-only.
pub fn breaks_on_termination(pid: u32, create_time: i64) -> Option<bool> {
    let handle = open_verified(pid, create_time, PROCESS_ACCESS_RIGHTS(0)).ok()?;
    handle_breaks_on_termination(&handle)
}

/// Refuses to terminate a process Windows marks critical. An unreadable flag proceeds: the
/// terminate itself then fails if access is lacking.
pub(crate) fn guard_critical(flag: Option<bool>) -> Result<(), ActionError> {
    if flag == Some(true) { Err(ActionError::Critical) } else { Ok(()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn critical_names_match_with_or_without_exe() {
        assert!(is_critical("csrss.exe") && is_critical("LSASS") && is_critical("System"));
        assert!(!is_critical("chrome.exe"));
    }

    #[test]
    fn a_break_on_termination_process_is_refused_with_a_plain_message() {
        assert_eq!(guard_critical(Some(true)), Err(ActionError::Critical));
        assert_eq!(
            ActionError::Critical.message("csrss.exe", true),
            "Windows marks this process as critical \u{2014} ending it would crash the PC."
        );
        // Not critical, or the flag couldn't be read: proceed (the terminate fails on its own if denied).
        assert_eq!(guard_critical(Some(false)), Ok(()));
        assert_eq!(guard_critical(None), Ok(()));
    }

    #[test]
    fn messages_are_plain_english() {
        assert_eq!(ActionError::AlreadyExited.message("Notepad", false), "Notepad already exited.");
        assert_eq!(
            ActionError::AccessDenied.message("svchost.exe", false),
            "Couldn't change svchost.exe: access denied. Restart Negotium as admin and try again."
        );
        assert_eq!(
            ActionError::AccessDenied.message("svchost.exe", true),
            "Couldn't change svchost.exe: Windows protects this process."
        );
    }
}
