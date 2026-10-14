//! Physical memory totals and logical processor count.
use negotium_snapshot::MemoryTotals;
use std::mem::size_of;
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::{ALL_PROCESSOR_GROUPS, GetActiveProcessorCount};

pub fn memory_totals() -> Option<MemoryTotals> {
    let mut status = MEMORYSTATUSEX { dwLength: size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    // SAFETY: `status` is initialized with its size as the API requires.
    unsafe { GlobalMemoryStatusEx(&mut status) }.ok()?;
    Some(MemoryTotals { total: status.ullTotalPhys, used: status.ullTotalPhys.saturating_sub(status.ullAvailPhys) })
}

/// Logical processors across all processor groups (matters above 64 cores).
pub fn logical_cpus() -> u32 {
    // SAFETY: simple query with no pointers.
    let n = unsafe { GetActiveProcessorCount(ALL_PROCESSOR_GROUPS) };
    n.max(1)
}
