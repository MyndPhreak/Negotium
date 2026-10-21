use crate::{ActionError, guard_critical, handle_breaks_on_termination, open_verified};
use std::time::Duration;
use windows::Win32::Foundation::WAIT_OBJECT_0;
use windows::Win32::System::Threading::{PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, TerminateProcess, WaitForSingleObject};

/// How long to wait for a terminated process to actually be gone before reporting it stuck.
const EXIT_WAIT: Duration = Duration::from_secs(2);

pub fn end_process(pid: u32, create_time: i64) -> Result<(), ActionError> {
    let handle = open_verified(pid, create_time, PROCESS_TERMINATE | PROCESS_SYNCHRONIZE)?;
    guard_critical(handle_breaks_on_termination(&handle))?;
    // SAFETY: verified handle with PROCESS_TERMINATE.
    unsafe { TerminateProcess(handle.0, 1) }.map_err(|e| ActionError::from_win(&e))?;
    // TerminateProcess only queues the kill: a process blocked in a driver or on a dead disk or share
    // stays listed. Say so instead of reporting success for something still running.
    // SAFETY: verified handle opened with SYNCHRONIZE.
    if unsafe { WaitForSingleObject(handle.0, EXIT_WAIT.as_millis() as u32) } == WAIT_OBJECT_0 {
        Ok(())
    } else {
        Err(ActionError::Other(
            "it was told to end but hasn't exited, so it is probably stuck in a driver or waiting on a disk or network share".into(),
        ))
    }
}

pub fn end_many(targets: &[(u32, i64)]) -> Vec<((u32, i64), Result<(), ActionError>)> {
    targets
        .iter()
        .map(|&(pid, ct)| {
            let r = match end_process(pid, ct) {
                Err(ActionError::AlreadyExited) => Ok(()),
                other => other,
            };
            ((pid, ct), r)
        })
        .collect()
}
