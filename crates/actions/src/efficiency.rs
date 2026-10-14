use crate::{ActionError, open_verified};
use std::ffi::c_void;
use std::mem::size_of;
use windows::Win32::System::Threading::{
    GetProcessInformation, IDLE_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
    PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_STATE, PROCESS_SET_INFORMATION,
    ProcessPowerThrottling, SetPriorityClass, SetProcessInformation,
};

/// Reads the state from the first live target and sets every target to the opposite.
/// Returns the new state (`true` = efficiency mode on). Every target is attempted; the
/// first error other than `AlreadyExited` is returned afterwards.
pub fn toggle_efficiency_mode(targets: &[(u32, i64)]) -> Result<bool, ActionError> {
    let mut current = None;
    let mut first_err = None;
    for &(pid, ct) in targets {
        match is_on(pid, ct) {
            Ok(on) => {
                current = Some(on);
                break;
            }
            Err(ActionError::AlreadyExited) => {}
            Err(e) => {
                first_err = Some(e);
                break;
            }
        }
    }
    if let Some(e) = first_err {
        return Err(e);
    }
    let Some(current) = current else { return Err(ActionError::AlreadyExited) };
    let enable = !current;
    let mut first_err = None;
    for &(pid, ct) in targets {
        match set(pid, ct, enable) {
            Ok(()) | Err(ActionError::AlreadyExited) => {}
            Err(e) => {
                first_err.get_or_insert(e);
            }
        }
    }
    first_err.map_or(Ok(enable), Err)
}

fn is_on(pid: u32, ct: i64) -> Result<bool, ActionError> {
    let handle = open_verified(pid, ct, Default::default())?;
    let mut state = PROCESS_POWER_THROTTLING_STATE { Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION, ..Default::default() };
    // SAFETY: `state` is a writable PROCESS_POWER_THROTTLING_STATE of the size passed.
    unsafe {
        GetProcessInformation(handle.0, ProcessPowerThrottling, &mut state as *mut _ as *mut c_void, size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32)
    }
    .map_err(|e| ActionError::from_win(&e))?;
    Ok(state.ControlMask & PROCESS_POWER_THROTTLING_EXECUTION_SPEED != 0
        && state.StateMask & PROCESS_POWER_THROTTLING_EXECUTION_SPEED != 0)
}

fn set(pid: u32, ct: i64, enable: bool) -> Result<(), ActionError> {
    let handle = open_verified(pid, ct, PROCESS_SET_INFORMATION)?;
    let state = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        // Off hands control back to the system (neither bit set) rather than forcing EcoQoS off.
        ControlMask: if enable { PROCESS_POWER_THROTTLING_EXECUTION_SPEED } else { 0 },
        StateMask: if enable { PROCESS_POWER_THROTTLING_EXECUTION_SPEED } else { 0 },
    };
    // SAFETY: `state` is a valid PROCESS_POWER_THROTTLING_STATE of the size passed.
    unsafe {
        SetProcessInformation(handle.0, ProcessPowerThrottling, &state as *const _ as *const c_void, size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32)
    }
    .map_err(|e| ActionError::from_win(&e))?;
    let class = if enable { IDLE_PRIORITY_CLASS } else { NORMAL_PRIORITY_CLASS };
    // SAFETY: verified handle opened with PROCESS_SET_INFORMATION.
    unsafe { SetPriorityClass(handle.0, class) }.map_err(|e| ActionError::from_win(&e))
}
