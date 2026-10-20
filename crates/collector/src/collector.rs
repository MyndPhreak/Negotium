//! One tick = one consistent Snapshot. Each source fails independently.
use crate::{
    IdentityCache, PdhSources, ProcessReader, RawProcess, SystemInfo, local_time_ms, logical_cpus, memory_totals, read_system_info,
    scan_windows, uptime_s,
};
use negotium_snapshot::{GpuSample, ProcessSample, Snapshot, SystemTotals};
use std::collections::{BTreeMap, HashSet};
use std::time::Instant;

pub struct Collector {
    processes: ProcessReader,
    /// The last process list read. Reused when a read fails, so one failed read doesn't make every
    /// process look exited to the caches downstream (its unchanged CPU times give 0% for that tick).
    last_raw: Vec<RawProcess>,
    identity: IdentityCache,
    pdh: Result<PdhSources, u32>,
    started: Instant,
    logical_cpus: u32,
    info: SystemInfo,
    net: crate::net::NetTable,
    gpu_temps: crate::gpu::GpuTemps,
    /// When DXGI adapters were last enumerated, and the PDH adapter keys that enumeration did not
    /// list (`gpu::needs_rescan`).
    gpu_scan: (Instant, Vec<String>),
    cpu_temp: crate::cputemp::CpuTemp,
    /// Per-process network: the kernel network trace, only when asked for (`with_net_feed`) and
    /// elevated.
    net_by_pid: crate::netetw::PerPidNet<crate::netetw::NetFeed>,
}

impl Default for Collector {
    fn default() -> Self {
        Self::new()
    }
}

impl Collector {
    pub fn new() -> Self {
        Self::with_epoch(Instant::now())
    }

    /// Collector time (`Snapshot::taken_at_ms`) counts from `started`. A restarted worker passes
    /// the same instant, so time never goes backwards for the history it keeps.
    pub fn with_epoch(started: Instant) -> Self {
        let info = read_system_info();
        Self {
            processes: ProcessReader::new(),
            last_raw: Vec::new(),
            identity: IdentityCache::new(),
            pdh: PdhSources::open(),
            started,
            logical_cpus: logical_cpus(),
            gpu_temps: crate::gpu::GpuTemps::open(&info.gpus),
            gpu_scan: (Instant::now(), Vec::new()),
            cpu_temp: crate::cputemp::CpuTemp::open(),
            info,
            net: crate::net::NetTable::new(),
            net_by_pid: crate::netetw::PerPidNet::off(Instant::now()),
        }
    }

    /// Adds the per-process network feed: a trace session called `name`, started only when
    /// elevated. Only the app asks for it (with `netetw::session_name()`); tests, the bench and the
    /// probe go without, so running them elevated never replaces the app's session. A restarted
    /// worker drops the old Collector (stopping its session) before building this one, and `start`
    /// also replaces any leftover session of the name.
    pub fn with_net_feed(mut self, name: &str) -> Self {
        self.start_net_feed(name);
        self
    }

    /// `with_net_feed` on a collector already built (replacing any feed it had). A feed that stops
    /// or fails to start is tried again every `netetw::RETRY`.
    pub fn start_net_feed(&mut self, name: &str) {
        // Drop the old feed first: its Drop stops its session and joins its consumer. Otherwise
        // the new start (same name) would stop the old session by name under a consumer still
        // attached, and two feeds would live side by side until the assignment.
        self.net_by_pid = crate::netetw::PerPidNet::off(Instant::now());
        let name = name.to_string();
        self.net_by_pid = crate::netetw::PerPidNet::new(
            crate::netetw::is_elevated(),
            move || crate::netetw::NetFeed::start(&name),
            Instant::now(),
        );
    }

    /// Whether this collector runs a per-process network feed.
    pub fn has_net_feed(&self) -> bool {
        self.net_by_pid.is_on()
    }

    pub fn system_info(&self) -> &SystemInfo {
        &self.info
    }

    pub fn tick(&mut self) -> Snapshot {
        let taken_at_ms = self.started.elapsed().as_millis() as u64;
        let mut source_errors = Vec::new();

        match self.processes.read() {
            Ok(raw) => self.last_raw = raw,
            Err(status) => source_errors.push(format!("processes: NTSTATUS {status:#010x}")),
        }
        let alive: HashSet<(u32, i64)> = self.last_raw.iter().map(|p| (p.pid, p.create_time)).collect();
        self.identity.retain_alive(&alive);
        let processes: Vec<ProcessSample> = self
            .last_raw
            .iter()
            .cloned()
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
                    description: id.description,
                    publisher: id.publisher,
                }
            })
            .collect();

        let mut totals = SystemTotals {
            logical_cpus: self.logical_cpus,
            cpu_temp_c: self.cpu_temp.read(),
            memory: memory_totals(),
            uptime_s: uptime_s(),
            local_time_ms: local_time_ms(),
            ..Default::default()
        };
        totals.net_by_pid = self.net_by_pid.step(Instant::now(), &processes, &mut source_errors);
        if totals.memory.is_none() {
            source_errors.push("memory: GlobalMemoryStatusEx failed".into());
        }
        // Needs only the process list, so it is reported even when PDH could not be opened.
        if let Some(m) = totals.memory.as_mut() {
            m.compressed = compressed_bytes(&processes);
        }
        let mut gpu_parts = (None, None, None);
        let readings = match &mut self.pdh {
            Ok(pdh) => Ok(pdh.read()),
            Err(status) => Err(*status),
        };
        match readings {
            Ok(r) => {
                totals.cpu_pct = r.cpu_pct;
                totals.per_core_pct = r.per_core_pct;
                totals.cpu_mhz = r.cpu_mhz;
                totals.disks = r.disks;
                if let Some(raw) = r.adapters {
                    let now = Instant::now();
                    let instances = || raw.iter().map(|(n, _, _)| n.as_str());
                    if self.net.needs_refresh(instances(), now) {
                        match self.net.refresh(now) {
                            Ok(warning) => source_errors.extend(warning),
                            Err(e) => source_errors.push(e),
                        }
                        self.net.note_unresolved(instances());
                    }
                    let adapters = crate::net::join_adapters(&raw, &self.net);
                    totals.net_bps = Some(crate::net::hardware_total(&adapters));
                    totals.adapters = Some(adapters);
                }
                totals.gpu_pct = r.gpu_pct;
                totals.gpu_pct_by_pid = r.gpu_pct_by_pid;
                gpu_parts = (r.gpus, r.gpu_dedicated, r.gpu_shared);
                if let Some(m) = totals.memory.as_mut() {
                    m.lists = r.mem_lists;
                }
                source_errors.extend(r.errors);
            }
            Err(status) => source_errors.push(format!("pdh open: {status:#010x}")),
        }

        // A new LUID (driver update, re-enabled adapter) would drop every engine sample of that
        // adapter: enumerate again, and reopen the temperature handles for the new list.
        let now = Instant::now();
        let pdh_keys = || gpu_parts.0.iter().flatten().map(|s| s.key.as_str());
        if crate::gpu::needs_rescan(pdh_keys(), &self.info.gpus, &self.gpu_scan.1, self.gpu_scan.0, now) {
            let (gpus, errors) = crate::gpu::enumerate_gpus();
            self.gpu_temps = crate::gpu::GpuTemps::open(&gpus);
            self.gpu_scan = (now, pdh_keys().filter(|k| !gpus.iter().any(|g| g.key == *k)).map(String::from).collect());
            (self.info.gpus, self.info.gpu_errors) = (gpus, errors);
        }

        // Independent of PDH: DXGI enumeration and kernel temperature queries fail on their own.
        source_errors.extend(self.info.gpu_errors.iter().cloned());
        let (temps, temp_errors) = self.gpu_temps.read_checked();
        source_errors.extend(temp_errors);
        let (engines, dedicated, shared) = gpu_parts;
        totals.gpus = merge_gpus(engines, &self.info.gpus, dedicated.as_ref(), shared.as_ref(), &temps);

        let windows = scan_windows().unwrap_or_else(|message| {
            source_errors.push(message);
            Vec::new()
        });

        Snapshot { taken_at_ms, processes, windows, totals, source_errors }
    }
}

/// One sample per hardware adapter. When DXGI enumerated adapters (`info` is non-empty), only
/// those adapters are kept (dropping software adapters like Microsoft Basic Render Driver that PDH
/// still reports), in DXGI order; any DXGI adapter without engine samples is seeded with unknown
/// utilization (NaN, shown as "—"). When `info` is empty, PDH samples are kept as-is.
pub(crate) fn merge_gpus(
    pdh: Option<Vec<GpuSample>>, info: &[crate::gpu::GpuInfo], dedicated: Option<&BTreeMap<String, u64>>,
    shared: Option<&BTreeMap<String, u64>>, temps: &[(&str, f32)],
) -> Option<Vec<GpuSample>> {
    let mut gpus = pdh.unwrap_or_default();
    if !info.is_empty() {
        gpus.retain(|s| info.iter().any(|g| g.key == s.key));
    }
    for g in info {
        if !gpus.iter().any(|s| s.key == g.key) {
            gpus.push(GpuSample { key: g.key.clone(), util_pct: f32::NAN, ..Default::default() });
        }
    }
    if !info.is_empty() {
        let pos = |k: &str| info.iter().position(|g| g.key == k).unwrap_or(usize::MAX);
        gpus.sort_by_key(|s| pos(&s.key));
    }
    for g in &mut gpus {
        g.dedicated_used = dedicated.and_then(|m| m.get(&g.key).copied());
        g.shared_used = shared.and_then(|m| m.get(&g.key).copied());
        g.temp_c = temps.iter().find(|(k, _)| *k == g.key).map(|(_, c)| *c);
    }
    (!gpus.is_empty()).then_some(gpus)
}

/// The memory compression store lives in the minimal process "Memory Compression".
pub(crate) fn compressed_bytes(processes: &[ProcessSample]) -> Option<u64> {
    processes.iter().find(|p| p.name == "Memory Compression").map(|p| p.private_working_set)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proc(name: &str, ws: u64) -> ProcessSample {
        ProcessSample {
            pid: 1, parent_pid: 0, create_time: 1, name: name.into(), session_id: 0, cpu_time_100ns: 0,
            private_working_set: ws, disk_io_bytes: 0, threads: 1, handles: 0, image_path: None,
            package_family: None, description: None, publisher: None,
        }
    }

    fn info(key: &str) -> crate::gpu::GpuInfo {
        crate::gpu::GpuInfo { key: key.into(), ..Default::default() }
    }

    #[test]
    fn temperatures_and_memory_survive_missing_engine_data() {
        let mem: BTreeMap<String, u64> = [("a".to_string(), 7u64)].into();
        let g = merge_gpus(None, &[info("a"), info("b")], Some(&mem), None, &[("b", 61.0)]).unwrap();
        assert_eq!(g.iter().map(|s| s.key.as_str()).collect::<Vec<_>>(), ["a", "b"], "DXGI order");
        assert!(g.iter().all(|s| s.util_pct.is_nan()), "utilization unknown, never 0");
        assert_eq!((g[0].dedicated_used, g[0].temp_c), (Some(7), None));
        assert_eq!((g[1].dedicated_used, g[1].temp_c), (None, Some(61.0)));
        assert_eq!(merge_gpus(None, &[], None, None, &[]), None);
    }

    #[test]
    fn a_temperature_for_an_adapter_without_engine_samples_is_attached() {
        let pdh = vec![
            GpuSample { key: "a".into(), util_pct: 40.0, ..Default::default() },
            GpuSample { key: "software_basic_render".into(), util_pct: 0.0, ..Default::default() },
        ];
        let g = merge_gpus(Some(pdh), &[info("a"), info("b")], None, None, &[("a", 50.0), ("b", 60.0)]).unwrap();
        assert_eq!(g.len(), 2, "software PDH adapter not in DXGI hardware list is dropped");
        assert_eq!((g[0].util_pct, g[0].temp_c), (40.0, Some(50.0)));
        assert!(g[1].util_pct.is_nan());
        assert_eq!(g[1].temp_c, Some(60.0));
    }

    #[test]
    fn compressed_memory_is_the_memory_compression_process() {
        assert_eq!(compressed_bytes(&[proc("System", 5), proc("Memory Compression", 512)]), Some(512));
        assert_eq!(compressed_bytes(&[proc("System", 5)]), None);
    }
}
