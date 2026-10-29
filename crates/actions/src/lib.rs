//! Process actions. Every call verifies identity by (pid, create_time) before acting,
//! so a PID reused since the last tick is never touched. Call off the UI thread.
mod affinity;
mod close;
mod details;
mod efficiency;
mod elevation;
pub mod integration;
mod location;
mod priority;
pub mod runtask;
pub mod services;
mod shell;
pub mod startup;
mod terminate;

pub use affinity::{get_process_affinity, set_process_affinity, set_process_cpu_sets};
pub use close::{FORCE_CLOSE_GRACE, force_close};
pub use details::{Details, process_details};
pub use efficiency::{is_efficiency_mode, set_efficiency_mode, set_power_throttling, toggle_efficiency_mode};
pub use elevation::{
    ELEVATED_WAIT, enable_debug_privilege, is_elevated, restart_as_admin, run_elevated, run_elevated_detached, run_elevated_exe,
    shell_open,
};
pub use location::open_file_location;
pub use priority::{PriorityClass, get_priority_class, set_priority_class};
pub use shell::{ShellToken, can_terminate, capture_shell_token, start_explorer, wait_for_taskbar};
pub use terminate::{end_many, end_process};

use std::ffi::c_void;
use std::mem::size_of;
use windows::Wdk::System::Threading::{NtQueryInformationProcess, ProcessBreakOnTermination};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_CANCELLED, ERROR_INVALID_PARAMETER, FILETIME, HANDLE, STILL_ACTIVE,
};
use windows::Win32::System::Threading::{
    GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_ACCESS_RIGHTS, PROCESS_QUERY_LIMITED_INFORMATION,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActionError {
    AlreadyExited,
    AccessDenied,
    /// Windows flags the process BreakOnTermination: ending it bugchecks the machine.
    Critical,
    /// The user declined a prompt (ERROR_CANCELLED), such as the UAC prompt.
    Cancelled,
    /// Already ending (it has an exit code) but not closed yet: a thread is stuck, often in a
    /// driver. Windows refuses every action on it with access denied until it closes.
    Ending,
    Other(String),
}

/// The bare reason, for log and error lines ("access denied"); `message` builds the sentence
/// shown to the user.
impl std::fmt::Display for ActionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ActionError::AlreadyExited => f.write_str("it already exited"),
            ActionError::AccessDenied => f.write_str("access denied"),
            ActionError::Critical => f.write_str("Windows marks the process as critical"),
            ActionError::Cancelled => f.write_str("it was cancelled"),
            ActionError::Ending => f.write_str("it is already ending"),
            ActionError::Other(detail) => f.write_str(detail),
        }
    }
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
            ActionError::Cancelled => format!("Couldn't change {what}: it was cancelled."),
            ActionError::Ending => format!(
                "{what} is already ending, but Windows did not finish closing it. It keeps its ports and files until it closes."
            ),
            ActionError::Other(detail) => format!("Couldn't change {what}: {detail}."),
        }
    }

    pub(crate) fn from_win(e: &windows::core::Error) -> Self {
        let code = e.code();
        if code == ERROR_ACCESS_DENIED.to_hresult() {
            ActionError::AccessDenied
        } else if code == ERROR_CANCELLED.to_hresult() {
            ActionError::Cancelled
        } else {
            ActionError::Other(e.message().trim().to_string())
        }
    }
}

pub const CRITICAL_PROCESSES: &[&str] = &["csrss", "wininit", "winlogon", "lsass", "smss", "services", "System"];

pub fn is_critical(name: &str) -> bool {
    let stem = match name.len().checked_sub(4).and_then(|i| name.split_at_checked(i)) {
        Some((stem, ext)) if ext.eq_ignore_ascii_case(".exe") => stem,
        _ => name,
    };
    CRITICAL_PROCESSES.iter().any(|c| c.eq_ignore_ascii_case(stem))
}

pub(crate) struct OwnedHandle(pub(crate) HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: a handle this process owns (process or token) and nothing else closes; closed once here.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// OpenProcess on a PID that no longer exists fails with ERROR_INVALID_PARAMETER. Only here does
/// that code mean "exited": from other calls (a bad affinity mask, an unsupported info class) it is
/// a real failure, so `from_win` reports it as `Other`.
fn open_error(e: &windows::core::Error) -> ActionError {
    if e.code() == ERROR_INVALID_PARAMETER.to_hresult() { ActionError::AlreadyExited } else { ActionError::from_win(e) }
}

/// Opens `pid` with `access` (plus query rights) and checks its creation time.
pub(crate) fn open_verified(pid: u32, create_time: i64, access: PROCESS_ACCESS_RIGHTS) -> Result<OwnedHandle, ActionError> {
    // SAFETY: plain Win32 call; the handle is owned by OwnedHandle.
    let handle = unsafe { OpenProcess(access | PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
        .map(OwnedHandle)
        .map_err(|e| open_error(&e))?;
    let (actual, state) = read_state(&handle)?;
    // A zombie (exited, but still held open by someone) and a process stuck while ending would both
    // fail with a misleading access-denied, so each is reported as what it is.
    if state == ProcessState::Exited || actual != create_time {
        return Err(ActionError::AlreadyExited);
    }
    if state == ProcessState::Ending {
        return Err(ActionError::Ending);
    }
    Ok(handle)
}

/// Where a process is in its life, from `GetProcessTimes`' exit time and `GetExitCodeProcess`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessState {
    Running,
    /// It has an exit code but no exit time: told to end, not closed yet.
    Ending,
    /// Closed (a zombie when something still holds it open).
    Exited,
}

pub fn process_state(exit_time: i64, exit_code: u32) -> ProcessState {
    if exit_time != 0 {
        ProcessState::Exited
    } else if exit_code == STILL_ACTIVE.0 as u32 {
        ProcessState::Running
    } else {
        ProcessState::Ending
    }
}

/// Whether `pid` is stuck while it ends. False when it can't be opened or read.
pub fn is_ending(pid: u32) -> bool {
    // SAFETY: plain Win32 call; the handle is owned by OwnedHandle.
    let Ok(handle) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }).map(OwnedHandle) else {
        return false;
    };
    matches!(read_state(&handle), Ok((_, ProcessState::Ending)))
}

/// The creation time and state of an open process.
fn read_state(handle: &OwnedHandle) -> Result<(i64, ProcessState), ActionError> {
    let (mut created, mut exited, mut kernel, mut user) =
        (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    // SAFETY: valid handle and four writable FILETIMEs.
    unsafe { GetProcessTimes(handle.0, &mut created, &mut exited, &mut kernel, &mut user) }
        .map_err(|e| ActionError::from_win(&e))?;
    let mut code = 0u32;
    // SAFETY: valid handle with query rights; `code` is a writable u32.
    unsafe { GetExitCodeProcess(handle.0, &mut code) }.map_err(|e| ActionError::from_win(&e))?;
    let time = |t: FILETIME| ((t.dwHighDateTime as i64) << 32) | t.dwLowDateTime as i64;
    Ok((time(created), process_state(time(exited), code)))
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
        assert!(is_critical("Csrss.Exe") && is_critical("wininit.eXe"));
        assert!(!is_critical("chrome.exe"));
    }

    #[test]
    fn invalid_parameter_means_exited_only_when_opening_the_process() {
        let invalid = windows::core::Error::from(ERROR_INVALID_PARAMETER.to_hresult());
        assert_eq!(open_error(&invalid), ActionError::AlreadyExited);
        assert!(matches!(ActionError::from_win(&invalid), ActionError::Other(_)));
        let denied = windows::core::Error::from(ERROR_ACCESS_DENIED.to_hresult());
        assert_eq!(open_error(&denied), ActionError::AccessDenied);
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
    fn errors_display_as_their_bare_reason() {
        assert_eq!(ActionError::AccessDenied.to_string(), "access denied");
        assert_eq!(ActionError::Other("The system cannot find the file specified".into()).to_string(), "The system cannot find the file specified");
    }

    #[test]
    fn a_cancelled_prompt_is_told_apart_from_other_failures() {
        use windows::Win32::Foundation::ERROR_CANCELLED;
        let cancelled = windows::core::Error::from(ERROR_CANCELLED.to_hresult());
        assert_eq!(ActionError::from_win(&cancelled), ActionError::Cancelled);
        assert_eq!(ActionError::Cancelled.to_string(), "it was cancelled");
        assert_eq!(ActionError::Cancelled.message("Zoom", false), "Couldn't change Zoom: it was cancelled.");
    }

    #[test]
    fn exit_time_and_exit_code_tell_running_ending_and_exited_apart() {
        const STILL_ACTIVE: u32 = 259;
        assert_eq!(process_state(0, STILL_ACTIVE), ProcessState::Running);
        assert_eq!(process_state(0, 1), ProcessState::Ending);
        assert_eq!(process_state(0, 0), ProcessState::Ending);
        assert_eq!(process_state(134_377_003_838_014_588, 1), ProcessState::Exited);
        assert_eq!(process_state(134_377_003_838_014_588, STILL_ACTIVE), ProcessState::Exited);
    }

    #[test]
    fn a_process_stuck_while_ending_gets_its_own_message() {
        assert_eq!(ActionError::Ending.to_string(), "it is already ending");
        let text = "Node.js is already ending, but Windows did not finish closing it. It keeps its ports and files until it closes.";
        assert_eq!(ActionError::Ending.message("Node.js", true), text);
        assert_eq!(ActionError::Ending.message("Node.js", false), text);
    }

    #[test]
    fn a_running_or_missing_process_is_not_ending() {
        assert!(!is_ending(std::process::id()));
        assert!(!is_ending(u32::MAX - 3));
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
