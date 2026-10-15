//! PDH counter sets, read as formatted arrays. Each counter is optional: a missing
//! counter set (e.g. no GPU counters in a VM) yields `None` for that reading only.
use negotium_snapshot::{DiskSample, MemoryLists};
use std::collections::BTreeMap;
use windows::Win32::System::Performance::{
    PDH_CSTATUS_NEW_DATA, PDH_FMT, PDH_FMT_COUNTERVALUE, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_HCOUNTER,
    PDH_HQUERY, PDH_MORE_DATA, PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData,
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
    pub net_bps: Option<f64>,
    pub gpu_pct: Option<f32>,
    pub gpu_pct_by_pid: Option<BTreeMap<u32, f32>>,
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
    net: Counter,
    gpu: Counter,
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
            net: add(query, w!("\\Network Interface(*)\\Bytes Total/sec")),
            gpu: add(query, w!("\\GPU Engine(*)\\Utilization Percentage")),
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
            net: f,
            gpu: f,
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
        let net = get(&mut errors, "network", c.net, read_array);
        let gpu = get(&mut errors, "gpu", c.gpu, read_array);

        let (gpu_pct, gpu_pct_by_pid) = match gpu {
            Some(v) => {
                let (sys, per) = gpu_from(&v);
                (sys, Some(per))
            }
            None => (None, None),
        };
        PdhReadings {
            cpu_pct: cpu.as_deref().and_then(total_from),
            per_core_pct: per_core_from(cpu.as_deref().unwrap_or_default(), &self.groups),
            cpu_mhz: cpu_mhz(freq, perf),
            mem_lists: memory_lists(modified, standby, free),
            disks: match (idle, read, write) {
                (Some(i), Some(r), Some(w)) => Some(disks_from(&i, &r, &w)),
                _ => None,
            },
            net_bps: net.map(|v| v.iter().map(|(_, x)| x.max(0.0)).sum()),
            gpu_pct,
            gpu_pct_by_pid,
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
    Ok(if v.is_finite() { v } else { 0.0 })
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
        .map(|it| {
            // SAFETY: szName is a NUL-terminated string inside `buf`.
            let name = unsafe { it.szName.to_string() }.unwrap_or_default();
            // SAFETY: doubleValue is the active union member because we requested PDH_FMT_DOUBLE.
            let value = unsafe { it.FmtValue.Anonymous.doubleValue };
            (name, if value.is_finite() { value } else { 0.0 })
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

pub fn parse_gpu_instance(name: &str) -> Option<(u32, String)> {
    let rest = name.strip_prefix("pid_")?;
    let (pid, rest) = rest.split_once('_')?;
    let engine = rest.split("_engtype_").next()?;
    if !engine.starts_with("luid_") {
        return None;
    }
    Some((pid.parse().ok()?, engine.to_string()))
}

/// System GPU % = busiest engine (summed over processes); per-process % = its busiest engine.
pub fn gpu_from(items: &[(String, f64)]) -> (Option<f32>, BTreeMap<u32, f32>) {
    let mut per_engine: BTreeMap<String, f64> = BTreeMap::new();
    let mut per_pid: BTreeMap<u32, f32> = BTreeMap::new();
    for (name, value) in items {
        let Some((pid, engine)) = parse_gpu_instance(name) else { continue };
        let v = value.max(0.0);
        *per_engine.entry(engine).or_default() += v;
        let slot = per_pid.entry(pid).or_default();
        *slot = slot.max(v.min(100.0) as f32);
    }
    let system = per_engine.values().copied().reduce(f64::max).map(|v| v.min(100.0) as f32);
    (system, per_pid)
}

pub fn disks_from(idle: &[(String, f64)], read: &[(String, f64)], write: &[(String, f64)]) -> Vec<DiskSample> {
    let lookup = |set: &[(String, f64)], name: &str| {
        set.iter().find(|(n, _)| n == name).map(|(_, v)| v.max(0.0)).unwrap_or(0.0)
    };
    idle.iter()
        .filter(|(name, _)| name != "_Total")
        .map(|(name, idle_pct)| DiskSample {
            name: name.clone(),
            active_pct: (100.0 - idle_pct).clamp(0.0, 100.0) as f32,
            read_bps: lookup(read, name),
            write_bps: lookup(write, name),
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
    fn gpu_instance_parses_pid_and_engine() {
        let n = "pid_1234_luid_0x00000000_0x0000D1B4_phys_0_eng_3_engtype_3D";
        assert_eq!(parse_gpu_instance(n), Some((1234, "luid_0x00000000_0x0000D1B4_phys_0_eng_3".into())));
        assert_eq!(parse_gpu_instance("garbage"), None);
    }

    #[test]
    fn gpu_system_is_busiest_engine_and_process_is_its_max_engine() {
        let v = items(&[
            ("pid_1_luid_0x0_0x1_phys_0_eng_0_engtype_3D", 30.0),
            ("pid_2_luid_0x0_0x1_phys_0_eng_0_engtype_3D", 25.0),
            ("pid_2_luid_0x0_0x1_phys_0_eng_5_engtype_VideoDecode", 40.0),
        ]);
        let (system, per_pid) = gpu_from(&v);
        assert_eq!(system, Some(55.0));
        assert_eq!(per_pid.get(&1), Some(&30.0));
        assert_eq!(per_pid.get(&2), Some(&40.0));
    }

    #[test]
    fn disks_join_counters_and_skip_total() {
        let idle = items(&[("0 C:", 75.0), ("_Total", 80.0), ("1 D:", 100.0)]);
        let read = items(&[("0 C:", 1000.0), ("1 D:", 0.0)]);
        let write = items(&[("0 C:", 500.0)]);
        let disks = disks_from(&idle, &read, &write);
        assert_eq!(disks.len(), 2);
        assert_eq!(disks[0].name, "0 C:");
        assert_eq!(disks[0].active_pct, 25.0);
        assert_eq!(disks[0].read_bps, 1000.0);
        assert_eq!(disks[0].write_bps, 500.0);
        assert_eq!(disks[1].write_bps, 0.0);
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
    fn failed_counter_registration_is_reported_by_read() {
        let mut query = PDH_HQUERY::default();
        // SAFETY: opening a real-time query with no data source; owned by `sources` below.
        assert_eq!(unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &mut query) }, 0);
        // Dropping `sources` closes the query.
        let mut sources = PdhSources { query, counters: Counters::failed(0xC0000BB8), groups: vec![2] };
        let r = sources.read();
        assert!(r.cpu_pct.is_none() && r.disks.is_none() && r.net_bps.is_none() && r.gpu_pct.is_none());
        assert!(r.cpu_mhz.is_none() && r.mem_lists.is_none());
        assert_eq!(r.per_core_pct, vec![None, None]);
        for label in ["cpu", "cpu frequency", "cpu performance", "memory modified", "memory free"] {
            let want = format!("pdh {label}: counter unavailable (add failed 0xc0000bb8)");
            assert!(r.errors.contains(&want), "missing {want:?} in {:?}", r.errors);
        }
    }
}
