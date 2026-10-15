//! Physical memory totals, commit charge, processor topology, uptime and the local clock.
use negotium_snapshot::MemoryTotals;
use std::mem::size_of;
use windows::Win32::System::SystemInformation::{GetLocalTime, GetTickCount64, GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::{ALL_PROCESSOR_GROUPS, GetActiveProcessorCount, GetActiveProcessorGroupCount};

pub fn memory_totals() -> Option<MemoryTotals> {
    let mut status = MEMORYSTATUSEX { dwLength: size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    // SAFETY: `status` is initialized with its size as the API requires.
    unsafe { GlobalMemoryStatusEx(&mut status) }.ok()?;
    Some(MemoryTotals {
        total: status.ullTotalPhys,
        used: status.ullTotalPhys.saturating_sub(status.ullAvailPhys),
        // ullTotalPageFile is the commit limit; ullAvailPageFile what is left of it.
        commit_limit: status.ullTotalPageFile,
        commit_total: status.ullTotalPageFile.saturating_sub(status.ullAvailPageFile),
        lists: None,
        compressed: None,
    })
}

/// Logical processors across all processor groups (matters above 64 cores).
pub fn logical_cpus() -> u32 {
    // SAFETY: simple query with no pointers.
    let n = unsafe { GetActiveProcessorCount(ALL_PROCESSOR_GROUPS) };
    n.max(1)
}

/// Active logical processors in each processor group, in group order.
pub fn processor_groups() -> Vec<u32> {
    // SAFETY: simple query with no pointers.
    let count = unsafe { GetActiveProcessorGroupCount() };
    let groups: Vec<u32> = (0..count)
        .map(|g| {
            // SAFETY: simple query with no pointers.
            unsafe { GetActiveProcessorCount(g) }
        })
        .collect();
    if groups.iter().sum::<u32>() == 0 { vec![logical_cpus()] } else { groups }
}

/// Seconds since Windows started.
pub fn uptime_s() -> u64 {
    // SAFETY: simple query with no pointers.
    let ms = unsafe { GetTickCount64() };
    ms / 1000
}

/// Local wall-clock time, milliseconds since midnight.
pub fn local_time_ms() -> u64 {
    // SAFETY: simple query with no pointers.
    let t = unsafe { GetLocalTime() };
    ((t.wHour as u64 * 60 + t.wMinute as u64) * 60 + t.wSecond as u64) * 1000 + t.wMilliseconds as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn processor_groups_add_up_to_the_logical_processor_count() {
        assert_eq!(processor_groups().iter().sum::<u32>(), logical_cpus());
    }

    #[test]
    fn local_time_is_within_a_day_and_commit_is_reported() {
        assert!(local_time_ms() < 86_400_000);
        assert!(uptime_s() > 0);
        let m = memory_totals().unwrap();
        assert!(m.commit_limit > 0 && m.commit_total <= m.commit_limit);
    }
}
