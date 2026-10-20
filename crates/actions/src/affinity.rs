use crate::{ActionError, open_verified};
use windows::Win32::System::Threading::{
    GetProcessAffinityMask, SetProcessAffinityMask, SetProcessDefaultCpuSets,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_INFORMATION,
};

pub fn set_process_affinity(pid: u32, create_time: i64, mask: u64) -> Result<(), ActionError> {
    let handle = open_verified(pid, create_time, PROCESS_SET_INFORMATION)?;
    // SAFETY: SetProcessAffinityMask with valid handle and mask for current processor group.
    unsafe { SetProcessAffinityMask(handle.0, mask as usize) }.map_err(|e| ActionError::from_win(&e))
}

pub fn get_process_affinity(pid: u32, create_time: i64) -> Result<(u64, u64), ActionError> {
    let handle = open_verified(pid, create_time, PROCESS_QUERY_LIMITED_INFORMATION)?;
    let mut proc_mask: usize = 0;
    let mut sys_mask: usize = 0;
    // SAFETY: pointers to local stack variables.
    unsafe { GetProcessAffinityMask(handle.0, &mut proc_mask, &mut sys_mask) }
        .map_err(|e| ActionError::from_win(&e))?;
    Ok((proc_mask as u64, sys_mask as u64))
}

pub fn set_process_cpu_sets(pid: u32, create_time: i64, cpu_set_ids: &[u32]) -> Result<(), ActionError> {
    let handle = open_verified(pid, create_time, PROCESS_SET_INFORMATION)?;
    // In windows-rs, SetProcessDefaultCpuSets takes `Option<&[u32]>`
    let sets = if cpu_set_ids.is_empty() { None } else { Some(cpu_set_ids) };
    // SAFETY: verified handle with valid CPU set slice or None to clear.
    unsafe { SetProcessDefaultCpuSets(handle.0, sets) }.ok().map_err(|e| ActionError::from_win(&e))
}
