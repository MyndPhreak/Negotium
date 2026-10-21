use crate::{ActionError, guard_critical, handle_breaks_on_termination, open_verified};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Threading::PROCESS_SUSPEND_RESUME;

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn NtSuspendProcess(process: HANDLE) -> i32;
    fn NtResumeProcess(process: HANDLE) -> i32;
}

const STATUS_ACCESS_DENIED: i32 = 0xC000_0022_u32 as i32;

fn check(status: i32) -> Result<(), ActionError> {
    match status {
        s if s >= 0 => Ok(()),
        STATUS_ACCESS_DENIED => Err(ActionError::AccessDenied),
        s => Err(ActionError::Other(format!("NTSTATUS {:#010X}", s as u32))),
    }
}

/// Freezes every thread of the process until `resume_process`; unlike ending it, nothing is lost.
pub fn suspend_process(pid: u32, create_time: i64) -> Result<(), ActionError> {
    let handle = open_verified(pid, create_time, PROCESS_SUSPEND_RESUME)?;
    // Freezing a process Windows marks critical would hang the machine just as ending it would crash it.
    guard_critical(handle_breaks_on_termination(&handle))?;
    // SAFETY: verified handle opened with PROCESS_SUSPEND_RESUME.
    check(unsafe { NtSuspendProcess(handle.0) })
}

pub fn resume_process(pid: u32, create_time: i64) -> Result<(), ActionError> {
    let handle = open_verified(pid, create_time, PROCESS_SUSPEND_RESUME)?;
    // SAFETY: verified handle opened with PROCESS_SUSPEND_RESUME.
    check(unsafe { NtResumeProcess(handle.0) })
}

/// Suspends or resumes every target, returning the first failure (already-exited targets are fine).
pub fn set_suspended(targets: &[(u32, i64)], suspend: bool) -> Result<(), ActionError> {
    let mut first = None;
    for &(pid, ct) in targets {
        let r = if suspend { suspend_process(pid, ct) } else { resume_process(pid, ct) };
        match r {
            Ok(()) | Err(ActionError::AlreadyExited) => {}
            Err(e) => first = first.or(Some(e)),
        }
    }
    first.map_or(Ok(()), Err)
}
