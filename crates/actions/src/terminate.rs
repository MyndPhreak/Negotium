use crate::{ActionError, guard_critical, handle_breaks_on_termination, open_verified};
use windows::Win32::System::Threading::{PROCESS_TERMINATE, TerminateProcess};

pub fn end_process(pid: u32, create_time: i64) -> Result<(), ActionError> {
    let handle = open_verified(pid, create_time, PROCESS_TERMINATE)?;
    guard_critical(handle_breaks_on_termination(&handle))?;
    // SAFETY: verified handle with PROCESS_TERMINATE.
    unsafe { TerminateProcess(handle.0, 1) }.map_err(|e| ActionError::from_win(&e))
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
