//! One tick = one consistent Snapshot. Each source fails independently.
use crate::{IdentityCache, PdhSources, ProcessReader, logical_cpus, memory_totals, scan_windows};
use negotium_snapshot::{ProcessSample, Snapshot, SystemTotals};
use std::collections::HashSet;
use std::time::Instant;

pub struct Collector {
    processes: ProcessReader,
    identity: IdentityCache,
    pdh: Result<PdhSources, u32>,
    started: Instant,
    logical_cpus: u32,
}

impl Default for Collector {
    fn default() -> Self {
        Self::new()
    }
}

impl Collector {
    pub fn new() -> Self {
        Self {
            processes: ProcessReader::new(),
            identity: IdentityCache::new(),
            pdh: PdhSources::open(),
            started: Instant::now(),
            logical_cpus: logical_cpus(),
        }
    }

    pub fn tick(&mut self) -> Snapshot {
        let taken_at_ms = self.started.elapsed().as_millis() as u64;
        let mut source_errors = Vec::new();

        let raw = self.processes.read().unwrap_or_else(|status| {
            source_errors.push(format!("processes: NTSTATUS {status:#010x}"));
            Vec::new()
        });
        let alive: HashSet<(u32, i64)> = raw.iter().map(|p| (p.pid, p.create_time)).collect();
        self.identity.retain_alive(&alive);
        let processes = raw
            .into_iter()
            .map(|p| {
                let id = self.identity.get(p.pid, p.create_time).clone();
                ProcessSample {
                    pid: p.pid,
                    parent_pid: p.parent_pid,
                    create_time: p.create_time,
                    name: p.name,
                    session_id: p.session_id,
                    cpu_time_100ns: p.cpu_time_100ns,
                    private_working_set: p.private_working_set,
                    disk_io_bytes: p.disk_io_bytes,
                    threads: p.threads,
                    handles: p.handles,
                    image_path: id.image_path,
                    package_family: id.package_family,
                }
            })
            .collect();

        let mut totals = SystemTotals { logical_cpus: self.logical_cpus, memory: memory_totals(), ..Default::default() };
        if totals.memory.is_none() {
            source_errors.push("memory: GlobalMemoryStatusEx failed".into());
        }
        match &mut self.pdh {
            Ok(pdh) => {
                let r = pdh.read();
                totals.cpu_pct = r.cpu_pct;
                totals.per_core_pct = r.per_core_pct;
                totals.disks = r.disks;
                totals.net_bps = r.net_bps;
                totals.gpu_pct = r.gpu_pct;
                totals.gpu_pct_by_pid = r.gpu_pct_by_pid;
                source_errors.extend(r.errors);
            }
            Err(status) => source_errors.push(format!("pdh open: {status:#010x}")),
        }

        Snapshot { taken_at_ms, processes, windows: scan_windows(), totals, source_errors }
    }
}
