//! PDH counter sets, read as formatted arrays. Each counter is optional: a missing
//! counter set (e.g. no GPU counters in a VM) yields `None` for that reading only.
use negotium_snapshot::{DiskSample, GpuSample, MemoryLists};
use std::collections::BTreeMap;
use windows::Win32::System::Performance::{
    PDH_CSTATUS_NEW_DATA, PDH_FMT, PDH_FMT_COUNTERVALUE, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_HCOUNTER,
    PDH_HQUERY, PDH_INVALID_DATA, PDH_MORE_DATA, PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData,
    PdhGetFormattedCounterArrayW, PdhGetFormattedCounterValue, PdhOpenQueryW,
};
use windows::core::{PCWSTR, w};

/// `PDH_FMT_NOCAP100` from pdh.h; `windows` 0.62.2 does not expose it. Lets rates exceed 100.
const PDH_FMT_NOCAP100: u32 = 0x0000_8000;

pub struct PdhReadings {
    pub cpu_pct: Option<f32>,
    pub per_core_pct: Vec<Option<f32>>,
    pub cpu_mhz: Option<f32>,
    pub mem_lists: Option<MemoryLists>,
    pub disks: Option<Vec<DiskSample>>,
    /// (instance, sent, received) bytes per second for every network interface instance.
    pub adapters: Option<Vec<(String, f64, f64)>>,
    pub gpu_pct: Option<f32>,
    pub gpu_pct_by_pid: Option<BTreeMap<u32, f32>>,
    pub gpus: Option<Vec<GpuSample>>,
    pub errors: Vec<String>,
}

type Counter = Result<PDH_HCOUNTER, u32>;

/// Every counter of the query. A counter that could not be added stays `Err` and its reading is
/// `None` (with an error line) every tick; the others are unaffected.
pub(crate) struct Counters {
    cpu: Counter,
    cpu_freq: Counter,
    cpu_perf: Counter,
    mem_modified: Counter,
    mem_standby_core: Counter,
    mem_standby_normal: Counter,
    mem_standby_reserve: Counter,
    mem_free: Counter,
    disk_idle: Counter,
    disk_read: Counter,
    disk_write: Counter,
    disk_response: Counter,
    net_sent: Counter,
    net_recv: Counter,
    gpu: Counter,
    gpu_dedicated: Counter,
    gpu_shared: Counter,
}

impl Counters {
    fn add_all(query: PDH_HQUERY) -> Self {
        Self {
            cpu: add(query, w!("\\Processor Information(*)\\% Processor Utility")),
            cpu_freq: add(query, w!("\\Processor Information(_Total)\\Processor Frequency")),
            cpu_perf: add(query, w!("\\Processor Information(_Total)\\% Processor Performance")),
            mem_modified: add(query, w!("\\Memory\\Modified Page List Bytes")),
            mem_standby_core: add(query, w!("\\Memory\\Standby Cache Core Bytes")),
            mem_standby_normal: add(query, w!("\\Memory\\Standby Cache Normal Priority Bytes")),
            mem_standby_reserve: add(query, w!("\\Memory\\Standby Cache Reserve Bytes")),
            mem_free: add(query, w!("\\Memory\\Free & Zero Page List Bytes")),
            disk_idle: add(query, w!("\\PhysicalDisk(*)\\% Idle Time")),
            disk_read: add(query, w!("\\PhysicalDisk(*)\\Disk Read Bytes/sec")),
            disk_write: add(query, w!("\\PhysicalDisk(*)\\Disk Write Bytes/sec")),
            disk_response: add(query, w!("\\PhysicalDisk(*)\\Avg. Disk sec/Transfer")),
            net_sent: add(query, w!("\\Network Interface(*)\\Bytes Sent/sec")),
            net_recv: add(query, w!("\\Network Interface(*)\\Bytes Received/sec")),
            gpu: add(query, w!("\\GPU Engine(*)\\Utilization Percentage")),
            gpu_dedicated: add(query, w!("\\GPU Adapter Memory(*)\\Dedicated Usage")),
            gpu_shared: add(query, w!("\\GPU Adapter Memory(*)\\Shared Usage")),
        }
    }

    #[cfg(test)]
    pub(crate) fn failed(status: u32) -> Self {
        let f = Err(status);
        Self {
            cpu: f,
            cpu_freq: f,
            cpu_perf: f,
            mem_modified: f,
            mem_standby_core: f,
            mem_standby_normal: f,
            mem_standby_reserve: f,
            mem_free: f,
            disk_idle: f,
            disk_read: f,
            disk_write: f,
            disk_response: f,
            net_sent: f,
            net_recv: f,
            gpu: f,
            gpu_dedicated: f,
            gpu_shared: f,
        }
    }
}

pub struct PdhSources {
    query: PDH_HQUERY,
    counters: Counters,
    /// Active logical processors per processor group, for placing per-core instances.
    groups: Vec<u32>,
}

impl PdhSources {
    pub fn open() -> Result<Self, u32> {
        let mut query = PDH_HQUERY::default();
        // SAFETY: opens a real-time query; closed in Drop.
        let status = unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &mut query) };
        if status != 0 {
            return Err(status);
        }
        let sources = Self { query, counters: Counters::add_all(query), groups: crate::processor_groups() };
        // SAFETY: valid query; first sample primes rate counters.
        unsafe { PdhCollectQueryData(query) };
        Ok(sources)
    }

    pub fn read(&mut self) -> PdhReadings {
        let mut errors = Vec::new();
        // SAFETY: valid query handle owned by self.
        let status = unsafe { PdhCollectQueryData(self.query) };
        if status != 0 {
            errors.push(format!("pdh collect: {status:#010x}"));
        }
        let c = &self.counters;
        let cpu = get(&mut errors, "cpu", c.cpu, read_array);
        let freq = get(&mut errors, "cpu frequency", c.cpu_freq, read_value);
        let perf = get(&mut errors, "cpu performance", c.cpu_perf, read_value);
        let modified = get(&mut errors, "memory modified", c.mem_modified, read_value);
        let standby = [
            get(&mut errors, "memory standby core", c.mem_standby_core, read_value),
            get(&mut errors, "memory standby normal", c.mem_standby_normal, read_value),
            get(&mut errors, "memory standby reserve", c.mem_standby_reserve, read_value),
        ];
        let free = get(&mut errors, "memory free", c.mem_free, read_value);
        let idle = get(&mut errors, "disk idle", c.disk_idle, read_array);
        let read = get(&mut errors, "disk read", c.disk_read, read_array);
        let write = get(&mut errors, "disk write", c.disk_write, read_array);
        let response = get(&mut errors, "disk response", c.disk_response, read_array);
        let sent = get(&mut errors, "network sent", c.net_sent, read_array);
        let recv = get(&mut errors, "network received", c.net_recv, read_array);
        let gpu = get(&mut errors, "gpu", c.gpu, read_array);

        let dedicated = get(&mut errors, "gpu dedicated memory", c.gpu_dedicated, read_array).map(|v| gpu_memory_from(&v));
        let shared = get(&mut errors, "gpu shared memory", c.gpu_shared, read_array).map(|v| gpu_memory_from(&v));
        let (gpu_pct, gpu_pct_by_pid, gpus) = match gpu {
            Some(v) => {
                let (sys, per, mut gpus) = gpu_from(&v);
                for g in &mut gpus {
                    g.dedicated_used = dedicated.as_ref().and_then(|m| m.get(&g.key).copied());
                    g.shared_used = shared.as_ref().and_then(|m| m.get(&g.key).copied());
                }
                (sys, Some(per), Some(gpus))
            }
            None => (None, None, None),
        };
        PdhReadings {
            cpu_pct: cpu.as_deref().and_then(total_from),
            per_core_pct: per_core_from(cpu.as_deref().unwrap_or_default(), &self.groups),
            cpu_mhz: cpu_mhz(freq, perf),
            mem_lists: memory_lists(modified, standby, free),
            // The response counter is optional: without it the disks still report, with no response time.
            disks: match (idle, read, write) {
                (Some(i), Some(r), Some(w)) => Some(disks_from(&i, &r, &w, response.as_deref().unwrap_or(&[]))),
                _ => None,
            },
            adapters: match (sent, recv) {
                (Some(s), Some(r)) => Some(adapters_from(&s, &r)),
                _ => None,
            },
            gpu_pct,
            gpu_pct_by_pid,
            gpus,
            errors,
        }
    }
}

/// Reads one counter with `read`; a counter that failed to register or to read becomes `None`
/// with an error line, so one bad counter never affects another.
fn get<T>(
    errors: &mut Vec<String>,
    label: &str,
    c: Counter,
    read: impl FnOnce(PDH_HCOUNTER) -> Result<T, u32>,
) -> Option<T> {
    let c = match c {
        Ok(c) => c,
        Err(status) => {
            errors.push(format!("pdh {label}: counter unavailable (add failed {status:#010x})"));
            return None;
        }
    };
    match read(c) {
        Ok(v) => Some(v),
        Err(e) => {
            errors.push(format!("pdh {label}: {e:#010x}"));
            None
        }
    }
}

/// One formatted value of a single-instance counter (the `Memory` set, `_Total` instances).
fn read_value(counter: PDH_HCOUNTER) -> Result<f64, u32> {
    let mut value = PDH_FMT_COUNTERVALUE::default();
    let format = PDH_FMT(PDH_FMT_DOUBLE.0 | PDH_FMT_NOCAP100);
    // SAFETY: valid counter; `value` is a writable PDH_FMT_COUNTERVALUE.
    let status = unsafe { PdhGetFormattedCounterValue(counter, format, None, &mut value) };
    if status != 0 {
        return Err(status);
    }
    if value.CStatus > PDH_CSTATUS_NEW_DATA {
        return Err(value.CStatus);
    }
    // SAFETY: doubleValue is the active union member because we requested PDH_FMT_DOUBLE.
    let v = unsafe { value.Anonymous.doubleValue };
    // A NaN or infinite value is no value: missing, never 0.
    finite(v).ok_or(PDH_INVALID_DATA)
}

/// `Some(v)` for a finite value; NaN and infinities are missing.
fn finite(v: f64) -> Option<f64> {
    v.is_finite().then_some(v)
}

impl Drop for PdhSources {
    fn drop(&mut self) {
        // SAFETY: query opened in `open`, closed once.
        unsafe { PdhCloseQuery(self.query) };
    }
}

fn add(query: PDH_HQUERY, path: PCWSTR) -> Result<PDH_HCOUNTER, u32> {
    let mut counter = PDH_HCOUNTER::default();
    // SAFETY: valid query; English path is locale-independent.
    let status = unsafe { PdhAddEnglishCounterW(query, path, 0, &mut counter) };
    if status == 0 { Ok(counter) } else { Err(status) }
}

fn read_array(counter: PDH_HCOUNTER) -> Result<Vec<(String, f64)>, u32> {
    let format = PDH_FMT(PDH_FMT_DOUBLE.0 | PDH_FMT_NOCAP100);
    let mut size = 0u32;
    let mut count = 0u32;
    // SAFETY: size query with no buffer.
    let status = unsafe { PdhGetFormattedCounterArrayW(counter, format, &mut size, &mut count, None) };
    if status == 0 {
        return Ok(Vec::new());
    }
    if status != PDH_MORE_DATA {
        return Err(status);
    }
    let mut buf = vec![0u64; (size as usize).div_ceil(8)];
    let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
    // SAFETY: `buf` is at least `size` bytes and 8-byte aligned.
    let status = unsafe { PdhGetFormattedCounterArrayW(counter, format, &mut size, &mut count, Some(items)) };
    if status != 0 {
        return Err(status);
    }
    // SAFETY: PDH wrote `count` items into `buf`; names point into the same buffer.
    let slice = unsafe { std::slice::from_raw_parts(items, count as usize) };
    Ok(slice
        .iter()
        .filter(|it| it.FmtValue.CStatus <= PDH_CSTATUS_NEW_DATA)
        .filter_map(|it| {
            // SAFETY: szName is a NUL-terminated string inside `buf`.
            let name = unsafe { it.szName.to_string() }.unwrap_or_default();
            // SAFETY: doubleValue is the active union member because we requested PDH_FMT_DOUBLE.
            let value = unsafe { it.FmtValue.Anonymous.doubleValue };
            finite(value).map(|v| (name, v))
        })
        .collect())
}

pub fn parse_core_instance(name: &str) -> Option<(u32, u32)> {
    let (group, core) = name.split_once(',')?;
    Some((group.parse().ok()?, core.parse().ok()?))
}

/// Places each "group,core" instance at its logical index: group bases are the running sum of
/// `groups`. Missing instances stay `None`, so a cell never shows another processor's value.
pub fn per_core_from(items: &[(String, f64)], groups: &[u32]) -> Vec<Option<f32>> {
    let mut out = vec![None; groups.iter().sum::<u32>() as usize];
    for (name, v) in items {
        let Some((g, c)) = parse_core_instance(name) else { continue };
        let Some(&size) = groups.get(g as usize) else { continue };
        if c < size {
            let base: u32 = groups[..g as usize].iter().sum();
            out[(base + c) as usize] = Some(v.clamp(0.0, 100.0) as f32);
        }
    }
    out
}

/// Task Manager's "Speed": the nominal frequency scaled by "% Processor Performance".
pub fn cpu_mhz(freq_mhz: Option<f64>, perf_pct: Option<f64>) -> Option<f32> {
    let (f, p) = (freq_mhz?, perf_pct?);
    (f > 0.0).then(|| (f * p / 100.0) as f32)
}

pub fn memory_lists(modified: Option<f64>, standby: [Option<f64>; 3], free: Option<f64>) -> Option<MemoryLists> {
    let standby: f64 = standby.into_iter().sum::<Option<f64>>()?;
    Some(MemoryLists { modified: modified?.max(0.0) as u64, standby: standby.max(0.0) as u64, free: free?.max(0.0) as u64 })
}

pub fn total_from(items: &[(String, f64)]) -> Option<f32> {
    items.iter().find(|(n, _)| n == "_Total").map(|(_, v)| v.clamp(0.0, 100.0) as f32)
}

/// Engine types shown first on a GPU page, in Task Manager's order; the rest follow alphabetically.
const PREFERRED_ENGINES: [&str; 4] = ["3D", "Copy", "VideoEncode", "VideoDecode"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GpuInstance<'a> {
    pub pid: u32,
    /// "luid_0x…_0x…_phys_N"
    pub adapter: &'a str,
    /// "luid_0x…_0x…_phys_N_eng_M"
    pub engine: &'a str,
    pub engtype: &'a str,
}

pub fn parse_gpu_instance(name: &str) -> Option<GpuInstance<'_>> {
    let rest = name.strip_prefix("pid_")?;
    let (pid, rest) = rest.split_once('_')?;
    let (engine, engtype) = rest.split_once("_engtype_").unwrap_or((rest, ""));
    if !engine.starts_with("luid_") {
        return None;
    }
    let adapter = engine.split("_eng_").next()?;
    Some(GpuInstance { pid: pid.parse().ok()?, adapter, engine, engtype })
}

/// Display order of engine types: the preferred ones first, then alphabetical. Compares borrowed
/// strings only.
fn engine_order(a: &str, b: &str) -> std::cmp::Ordering {
    let rank = |t: &str| PREFERRED_ENGINES.iter().position(|p| *p == t).unwrap_or(PREFERRED_ENGINES.len());
    rank(a).cmp(&rank(b)).then_with(|| a.cmp(b))
}

/// System GPU % = busiest engine (summed over processes); per-process % = its busiest engine;
/// per adapter: busiest engine overall and per engine type. Borrows the instance names, so the
/// hundreds of instances a tick don't each allocate; strings are allocated only for the few
/// adapters and engine types in the result.
pub fn gpu_from(items: &[(String, f64)]) -> (Option<f32>, BTreeMap<u32, f32>, Vec<GpuSample>) {
    let mut per_engine: BTreeMap<(&str, &str, &str), f64> = BTreeMap::new();
    let mut per_pid: BTreeMap<u32, f32> = BTreeMap::new();
    for (name, value) in items {
        let Some(i) = parse_gpu_instance(name) else { continue };
        let v = value.max(0.0);
        *per_engine.entry((i.adapter, i.engine, i.engtype)).or_default() += v;
        let slot = per_pid.entry(i.pid).or_default();
        *slot = slot.max(v.min(100.0) as f32);
    }
    let system = per_engine.values().copied().reduce(f64::max).map(|v| v.min(100.0) as f32);
    let mut adapters: BTreeMap<&str, BTreeMap<&str, f32>> = BTreeMap::new();
    for ((adapter, _, engtype), v) in per_engine {
        let slot = adapters.entry(adapter).or_default().entry(engtype).or_default();
        *slot = slot.max(v.min(100.0) as f32);
    }
    let gpus = adapters
        .into_iter()
        .map(|(adapter, types)| {
            let mut types: Vec<(&str, f32)> = types.into_iter().collect();
            types.sort_by(|a, b| engine_order(a.0, b.0));
            GpuSample {
                key: adapter.to_ascii_lowercase(),
                util_pct: types.iter().map(|(_, v)| *v).fold(0.0, f32::max),
                engines: types.into_iter().map(|(t, v)| (t.to_string(), v)).collect(),
                ..Default::default()
            }
        })
        .collect();
    (system, per_pid, gpus)
}

/// "GPU Adapter Memory" bytes by lowercase adapter instance.
pub fn gpu_memory_from(items: &[(String, f64)]) -> BTreeMap<String, u64> {
    items.iter().map(|(n, v)| (n.to_ascii_lowercase(), v.max(0.0) as u64)).collect()
}

/// Throughput is a plain `f64`, so a read or write item that is absent (PDH omits items without
/// valid data, and non-finite ones are skipped) counts as 0 bytes/s for a disk that is otherwise
/// reported; a disk whose idle item is absent is dropped. Only `response`, which has an `Option`
/// to carry it, keeps "missing" distinct.
/// `response` is optional data: an instance without a value has no response time.
pub fn disks_from(
    idle: &[(String, f64)], read: &[(String, f64)], write: &[(String, f64)], response: &[(String, f64)],
) -> Vec<DiskSample> {
    let find = |set: &[(String, f64)], name: &str| set.iter().find(|(n, _)| n == name).map(|(_, v)| v.max(0.0));
    idle.iter()
        .filter(|(name, _)| name != "_Total")
        .map(|(name, idle_pct)| DiskSample {
            name: name.clone(),
            active_pct: (100.0 - idle_pct).clamp(0.0, 100.0) as f32,
            read_bps: find(read, name).unwrap_or(0.0),
            write_bps: find(write, name).unwrap_or(0.0),
            response_ms: find(response, name).map(|seconds| (seconds * 1000.0) as f32),
        })
        .collect()
}

/// (instance, sent, received) for every instance of the sent counter; a received item that is
/// absent counts as 0 (the tuple has no way to say missing), an absent sent item drops the adapter.
pub fn adapters_from(sent: &[(String, f64)], recv: &[(String, f64)]) -> Vec<(String, f64, f64)> {
    sent.iter()
        .map(|(name, s)| {
            let r = recv.iter().find(|(n, _)| n == name).map(|(_, v)| v.max(0.0)).unwrap_or(0.0);
            (name.clone(), s.max(0.0), r)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(v: &[(&str, f64)]) -> Vec<(String, f64)> {
        v.iter().map(|(n, x)| (n.to_string(), *x)).collect()
    }

    #[test]
    fn core_instances_parse_group_and_core() {
        assert_eq!(parse_core_instance("0,3"), Some((0, 3)));
        assert_eq!(parse_core_instance("1,63"), Some((1, 63)));
        assert_eq!(parse_core_instance("_Total"), None);
        assert_eq!(parse_core_instance("0,_Total"), None);
    }

    #[test]
    fn per_core_places_each_instance_at_its_logical_index() {
        // Two processor groups of 4 and 2. "0,1", "0,2" and "1,0" had no valid data this tick
        // (filtered by CStatus), "1,5" does not exist in group 1.
        let v = items(&[("0,0", 10.0), ("0,3", 120.0), ("1,1", 50.0), ("1,5", 99.0), ("_Total", 40.0), ("0,_Total", 30.0)]);
        assert_eq!(per_core_from(&v, &[4, 2]), vec![Some(10.0), None, None, Some(100.0), None, Some(50.0)]);
        assert_eq!(total_from(&v), Some(40.0));
        assert_eq!(per_core_from(&[], &[2]), vec![None, None], "no data still yields one slot per processor");
    }

    #[test]
    fn cpu_speed_is_the_nominal_frequency_scaled_by_performance() {
        assert_eq!(cpu_mhz(Some(3000.0), Some(120.0)), Some(3600.0));
        assert_eq!(cpu_mhz(Some(3000.0), None), None);
        assert_eq!(cpu_mhz(Some(0.0), Some(100.0)), None);
    }

    #[test]
    fn memory_lists_add_the_three_standby_priorities() {
        let gb = |n: f64| Some(n * 1_073_741_824.0);
        let l = memory_lists(gb(1.0), [gb(0.5), gb(2.0), gb(0.25)], gb(4.0)).unwrap();
        assert_eq!(l.modified, 1 << 30);
        assert_eq!(l.standby, (11 << 30) / 4);
        assert_eq!(l.free, 4 << 30);
        assert_eq!(memory_lists(gb(1.0), [gb(0.5), None, gb(0.25)], gb(4.0)), None);
    }

    #[test]
    fn gpu_instance_parses_pid_adapter_engine_and_type() {
        let n = "pid_1234_luid_0x00000000_0x0000D1B4_phys_0_eng_3_engtype_3D";
        let i = parse_gpu_instance(n).unwrap();
        assert_eq!(i.pid, 1234);
        assert_eq!(i.adapter, "luid_0x00000000_0x0000D1B4_phys_0");
        assert_eq!(i.engine, "luid_0x00000000_0x0000D1B4_phys_0_eng_3");
        assert_eq!(i.engtype, "3D");
        assert!(parse_gpu_instance("garbage").is_none());
    }

    #[test]
    fn gpu_totals_per_adapter_and_per_engine_type() {
        let v = items(&[
            ("pid_1_luid_0x0_0x1_phys_0_eng_0_engtype_3D", 30.0),
            ("pid_2_luid_0x0_0x1_phys_0_eng_0_engtype_3D", 25.0),
            ("pid_2_luid_0x0_0x1_phys_0_eng_5_engtype_VideoDecode", 40.0),
            ("pid_3_luid_0x0_0x2_phys_0_eng_1_engtype_Copy", 5.0),
            ("pid_3_luid_0x0_0x2_phys_0_eng_2_engtype_Copy", 7.0),
        ]);
        let (system, per_pid, gpus) = gpu_from(&v);
        assert_eq!(system, Some(55.0), "busiest engine summed over processes");
        assert_eq!(per_pid.get(&2), Some(&40.0));
        assert_eq!(gpus.len(), 2);
        assert_eq!(gpus[0].key, "luid_0x0_0x1_phys_0");
        assert_eq!(gpus[0].util_pct, 55.0);
        assert_eq!(gpus[0].engines, vec![("3D".to_string(), 55.0), ("VideoDecode".to_string(), 40.0)]);
        assert_eq!(gpus[1].engines, vec![("Copy".to_string(), 7.0)], "busiest engine of each type");
    }

    #[test]
    fn engine_types_sort_preferred_first_then_alphabetically() {
        let v = items(&[
            ("pid_1_luid_0x0_0x1_phys_0_eng_0_engtype_Zeta", 1.0),
            ("pid_1_luid_0x0_0x1_phys_0_eng_1_engtype_VideoDecode", 1.0),
            ("pid_1_luid_0x0_0x1_phys_0_eng_2_engtype_Alpha", 1.0),
            ("pid_1_luid_0x0_0x1_phys_0_eng_3_engtype_3D", 1.0),
            ("pid_1_luid_0x0_0x1_phys_0_eng_4_engtype_Copy", 1.0),
        ]);
        let names: Vec<_> = gpu_from(&v).2[0].engines.iter().map(|(t, _)| t.clone()).collect();
        assert_eq!(names, ["3D", "Copy", "VideoDecode", "Alpha", "Zeta"]);
    }

    #[test]
    fn gpu_adapter_memory_is_keyed_by_lowercase_adapter() {
        let m = gpu_memory_from(&items(&[("luid_0x00000000_0x0001569B_phys_0", 3.5e9)]));
        assert_eq!(m.get("luid_0x00000000_0x0001569b_phys_0"), Some(&3_500_000_000));
    }

    #[test]
    fn disks_join_counters_and_skip_total() {
        let idle = items(&[("0 C:", 75.0), ("_Total", 80.0), ("1 D:", 100.0)]);
        let read = items(&[("0 C:", 1000.0), ("1 D:", 0.0)]);
        let write = items(&[("0 C:", 500.0)]);
        let response = items(&[("0 C:", 0.0004), ("1 D:", -1.0)]);
        let disks = disks_from(&idle, &read, &write, &response);
        assert_eq!(disks.len(), 2);
        assert_eq!(disks[0].name, "0 C:");
        assert_eq!(disks[0].active_pct, 25.0);
        assert_eq!(disks[0].read_bps, 1000.0);
        assert_eq!(disks[0].write_bps, 500.0);
        let ms = disks[0].response_ms.expect("response for 0 C:");
        assert!((ms - 0.4).abs() < 1e-6, "seconds per transfer shown in ms");
        assert_eq!(disks[1].write_bps, 0.0);
        assert_eq!(disks[1].response_ms, Some(0.0), "negative values clamp to zero");
    }

    #[test]
    fn a_missing_response_counter_leaves_the_disks_with_no_response_time() {
        // Spec section 7: the response counter failing must not take the disks down with it.
        let idle = items(&[("0 C:", 75.0)]);
        let disks = disks_from(&idle, &items(&[("0 C:", 1.0)]), &items(&[("0 C:", 2.0)]), &[]);
        assert_eq!(disks.len(), 1);
        assert_eq!(disks[0].active_pct, 25.0);
        assert_eq!(disks[0].response_ms, None);
        let partial = disks_from(&items(&[("0 C:", 75.0), ("1 D:", 90.0)]), &[], &[], &items(&[("1 D:", 0.002)]));
        assert_eq!(partial[0].response_ms, None, "an instance missing from the response set is None");
        assert_eq!(partial[1].response_ms, Some(2.0));
    }

    #[test]
    fn adapters_pair_sent_and_received_by_instance() {
        let sent = items(&[("Intel[R] Ethernet", 100.0), ("Hyper-V Virtual Ethernet Adapter", 40.0)]);
        let recv = items(&[("Intel[R] Ethernet", 900.0)]);
        assert_eq!(
            adapters_from(&sent, &recv),
            vec![("Intel[R] Ethernet".to_string(), 100.0, 900.0), ("Hyper-V Virtual Ethernet Adapter".to_string(), 40.0, 0.0)]
        );
    }

    #[test]
    fn non_finite_counter_values_are_missing_not_zero() {
        assert_eq!(finite(1.5), Some(1.5));
        assert_eq!(finite(f64::NAN), None);
        assert_eq!(finite(f64::INFINITY), None);
        assert_eq!(finite(f64::NEG_INFINITY), None);
    }

    #[test]
    fn unknown_counter_path_is_none_not_panic() {
        let mut query = PDH_HQUERY::default();
        // SAFETY: opening a real-time query with no data source.
        assert_eq!(unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &mut query) }, 0);
        assert!(add(query, w!("\\No Such Object(*)\\Nope")).is_err());
        // SAFETY: closing the query opened above.
        unsafe { PdhCloseQuery(query) };
    }

    #[test]
    fn a_failed_response_counter_does_not_take_the_disks_down() {
        let mut sources = PdhSources::open().expect("open pdh");
        sources.counters.disk_response = Err(0xC0000BB8);
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let r = sources.read();
        let disks = r.disks.expect("disks are built without the response counter");
        assert!(!disks.is_empty());
        assert!(disks.iter().all(|d| d.response_ms.is_none()));
        assert!(r.errors.iter().any(|e| e.starts_with("pdh disk response: counter unavailable")), "{:?}", r.errors);
    }

    #[test]
    fn failed_counter_registration_is_reported_by_read() {
        let mut query = PDH_HQUERY::default();
        // SAFETY: opening a real-time query with no data source; owned by `sources` below.
        assert_eq!(unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &mut query) }, 0);
        // Dropping `sources` closes the query.
        let mut sources = PdhSources { query, counters: Counters::failed(0xC0000BB8), groups: vec![2] };
        let r = sources.read();
        assert!(r.cpu_pct.is_none() && r.disks.is_none() && r.adapters.is_none() && r.gpu_pct.is_none());
        assert!(r.gpus.is_none());
        assert!(r.cpu_mhz.is_none() && r.mem_lists.is_none());
        assert_eq!(r.per_core_pct, vec![None, None]);
        for label in ["cpu", "cpu frequency", "cpu performance", "memory modified", "memory free"] {
            let want = format!("pdh {label}: counter unavailable (add failed 0xc0000bb8)");
            assert!(r.errors.contains(&want), "missing {want:?} in {:?}", r.errors);
        }
    }
}
