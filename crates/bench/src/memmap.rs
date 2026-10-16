//! Where a process's memory goes: its working set and private working set, split by what backs
//! each resident page (an executable image, a mapped file, or private memory such as heaps,
//! stacks and graphics-driver allocations), much like VMMap's summary. Attaches to a running
//! process with the same user's rights; no elevation for one's own processes.
use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::HashMap;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Memory::{MEM_COMMIT, MEM_IMAGE, MEM_MAPPED, MEMORY_BASIC_INFORMATION, VirtualQueryEx};
use windows::Win32::System::ProcessStatus::{
    GetMappedFileNameW, K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX2,
    PSAPI_WORKING_SET_EX_INFORMATION, QueryWorkingSetEx,
};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ};

/// x64 pages.
const PAGE: usize = 4096;
/// Pages asked about per QueryWorkingSetEx call.
const CHUNK: usize = 4096;
const MB: f64 = 1_048_576.0;
/// Label under which all private memory is counted.
const PRIVATE_LABEL: &str = "private (heaps, stacks, driver allocations)";

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Usage {
    /// Resident (in the working set).
    pub resident_mb: f64,
    /// Resident and not shared with another process.
    pub private_mb: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct MemoryReport {
    pub working_set_mb: f64,
    pub private_working_set_mb: f64,
    /// Resident memory by what backs it: "image", "mapped", "private".
    pub by_kind: Vec<(String, Usage)>,
    /// The largest sources (one module, one mapped file, or all private memory), largest first.
    pub top: Vec<(String, Usage)>,
}

/// label -> (kind, resident bytes, private bytes)
type Sources = HashMap<String, (&'static str, u64, u64)>;

struct Process(HANDLE);

impl Drop for Process {
    fn drop(&mut self) {
        // SAFETY: the handle came from OpenProcess and is closed exactly once, here.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

pub fn measure(pid: u32, top: usize) -> Result<MemoryReport> {
    // SAFETY: plain call; the handle is owned by `Process`.
    let p = Process(
        unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, false, pid) }
            .with_context(|| format!("open pid {pid}"))?,
    );
    let mut counters = PROCESS_MEMORY_COUNTERS_EX2 { cb: size_of::<PROCESS_MEMORY_COUNTERS_EX2>() as u32, ..Default::default() };
    // SAFETY: `counters` is a PROCESS_MEMORY_COUNTERS_EX2 whose `cb` says so.
    unsafe { K32GetProcessMemoryInfo(p.0, &mut counters as *mut _ as *mut PROCESS_MEMORY_COUNTERS, counters.cb) }
        .ok()
        .context("GetProcessMemoryInfo")?;
    let mut sources = Sources::new();
    let mut addr = 0usize;
    loop {
        let mut mbi = MEMORY_BASIC_INFORMATION::default();
        // SAFETY: `mbi` is a valid out buffer of the size passed.
        let got = unsafe { VirtualQueryEx(p.0, Some(addr as *const _), &mut mbi, size_of::<MEMORY_BASIC_INFORMATION>()) };
        if got == 0 {
            break;
        }
        let (base, len) = (mbi.BaseAddress as usize, mbi.RegionSize);
        if mbi.State == MEM_COMMIT {
            let (resident, private) = resident_bytes(&p, base, len)?;
            if resident > 0 {
                let (kind, label) = label_of(&p, &mbi);
                add(&mut sources, kind, label, resident, private);
            }
        }
        match base.checked_add(len) {
            Some(next) if next > addr => addr = next,
            _ => break,
        }
    }
    Ok(report(counters.WorkingSetSize as f64 / MB, counters.PrivateWorkingSetSize as f64 / MB, sources, top))
}

fn add(sources: &mut Sources, kind: &'static str, label: String, resident: u64, private: u64) {
    let e = sources.entry(label).or_insert((kind, 0, 0));
    e.1 += resident;
    e.2 += private;
}

/// Turns the per-source byte counts into the report: totals per kind, and the `top` largest
/// sources, largest first (ties by name, so the output is stable).
fn report(working_set_mb: f64, private_working_set_mb: f64, sources: Sources, top: usize) -> MemoryReport {
    let usage = |resident: u64, private: u64| Usage { resident_mb: resident as f64 / MB, private_mb: private as f64 / MB };
    let by_kind = ["image", "mapped", "private"]
        .into_iter()
        .map(|kind| {
            let (r, p) = sources.values().filter(|s| s.0 == kind).fold((0, 0), |(r, p), s| (r + s.1, p + s.2));
            (kind.to_string(), usage(r, p))
        })
        .collect();
    let mut all: Vec<(String, Usage)> = sources.into_iter().map(|(label, (_, r, p))| (label, usage(r, p))).collect();
    all.sort_by(|a, b| b.1.resident_mb.total_cmp(&a.1.resident_mb).then_with(|| a.0.cmp(&b.0)));
    all.truncate(top);
    MemoryReport { working_set_mb, private_working_set_mb, by_kind, top: all }
}

/// Resident and resident-private bytes of the committed region at `base`.
fn resident_bytes(p: &Process, base: usize, len: usize) -> Result<(u64, u64)> {
    let pages = len / PAGE;
    let (mut resident, mut private) = (0u64, 0u64);
    for first in (0..pages).step_by(CHUNK) {
        let n = (pages - first).min(CHUNK);
        let mut info: Vec<PSAPI_WORKING_SET_EX_INFORMATION> = (0..n)
            .map(|i| PSAPI_WORKING_SET_EX_INFORMATION { VirtualAddress: (base + (first + i) * PAGE) as *mut _, ..Default::default() })
            .collect();
        // SAFETY: `info` holds `n` entries; the call fills in their attributes.
        unsafe { QueryWorkingSetEx(p.0, info.as_mut_ptr() as *mut _, (n * size_of::<PSAPI_WORKING_SET_EX_INFORMATION>()) as u32) }
            .context("QueryWorkingSetEx")?;
        for e in &info {
            // SAFETY: the attributes union read as its plain `Flags` word (bit 0 valid, bit 15 shared).
            let flags = unsafe { e.VirtualAttributes.Flags };
            if flags & 1 != 0 {
                resident += PAGE as u64;
                if flags & (1 << 15) == 0 {
                    private += PAGE as u64;
                }
            }
        }
    }
    Ok((resident, private))
}

/// The region's kind and the source it is counted under: an image's or mapped file's name, or
/// all private memory together.
fn label_of(p: &Process, mbi: &MEMORY_BASIC_INFORMATION) -> (&'static str, String) {
    let kind = if mbi.Type == MEM_IMAGE {
        "image"
    } else if mbi.Type == MEM_MAPPED {
        "mapped"
    } else {
        return ("private", PRIVATE_LABEL.into());
    };
    let mut buf = [0u16; 1024];
    // SAFETY: `buf` is a valid writable buffer; the call writes at most its length.
    let n = unsafe { GetMappedFileNameW(p.0, mbi.AllocationBase, &mut buf) } as usize;
    let name = if n == 0 {
        "unnamed".to_string()
    } else {
        let full = String::from_utf16_lossy(&buf[..n]);
        full.rsplit('\\').next().unwrap_or(&full).to_string()
    };
    (kind, if kind == "mapped" { format!("{name} (mapped)") } else { name })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_process_is_broken_down_by_what_backs_its_pages() {
        let r = measure(std::process::id(), 1_000).expect("measure this process");
        assert!(r.working_set_mb > 1.0 && r.private_working_set_mb > 0.0, "{r:?}");
        assert!(r.private_working_set_mb <= r.working_set_mb);
        let resident: f64 = r.by_kind.iter().map(|(_, u)| u.resident_mb).sum();
        assert!((resident - r.working_set_mb).abs() <= r.working_set_mb * 0.25, "the pages add up to the working set: {resident} vs {}", r.working_set_mb);
        assert!(r.top.iter().any(|(name, _)| name.eq_ignore_ascii_case("ntdll.dll")), "modules are named: {:?}", r.top);
        assert!(r.top.windows(2).all(|w| w[0].1.resident_mb >= w[1].1.resident_mb), "largest first");
    }

    #[test]
    fn the_report_sums_by_kind_and_keeps_the_largest_sources_first() {
        let mb = 1_048_576u64;
        let mut s = Sources::new();
        add(&mut s, "image", "a.dll".into(), 2 * mb, mb);
        add(&mut s, "image", "a.dll".into(), mb, 0);
        add(&mut s, "image", "b.dll".into(), mb, 0);
        add(&mut s, "mapped", "f (mapped)".into(), 4 * mb, 0);
        add(&mut s, "private", PRIVATE_LABEL.into(), 10 * mb, 10 * mb);
        let r = report(20.0, 11.0, s, 2);
        let kinds: Vec<_> = r.by_kind.iter().map(|(k, u)| (k.as_str(), u.resident_mb, u.private_mb)).collect();
        assert_eq!(kinds, [("image", 4.0, 1.0), ("mapped", 4.0, 0.0), ("private", 10.0, 10.0)]);
        let top: Vec<_> = r.top.iter().map(|(n, u)| (n.as_str(), u.resident_mb)).collect();
        assert_eq!(top, [(PRIVATE_LABEL, 10.0), ("f (mapped)", 4.0)], "truncated to --top, largest first");
    }
}
