//! PDH counter sets, read as formatted arrays. Each counter is optional: a missing
//! counter set (e.g. no GPU counters in a VM) yields `None` for that reading only.
use negotium_snapshot::DiskSample;
use std::collections::BTreeMap;
use windows::Win32::System::Performance::{
    PDH_CSTATUS_NEW_DATA, PDH_FMT, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY,
    PDH_MORE_DATA, PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
    PdhOpenQueryW,
};
use windows::core::{PCWSTR, w};

/// `PDH_FMT_NOCAP100` from pdh.h; `windows` 0.62.2 does not expose it. Lets rates exceed 100.
const PDH_FMT_NOCAP100: u32 = 0x0000_8000;

pub struct PdhReadings {
    pub cpu_pct: Option<f32>,
    pub per_core_pct: Vec<f32>,
    pub disks: Option<Vec<DiskSample>>,
    pub net_bps: Option<f64>,
    pub gpu_pct: Option<f32>,
    pub gpu_pct_by_pid: Option<BTreeMap<u32, f32>>,
    pub errors: Vec<String>,
}

pub struct PdhSources {
    query: PDH_HQUERY,
    cpu: Result<PDH_HCOUNTER, u32>,
    disk_idle: Result<PDH_HCOUNTER, u32>,
    disk_read: Result<PDH_HCOUNTER, u32>,
    disk_write: Result<PDH_HCOUNTER, u32>,
    net: Result<PDH_HCOUNTER, u32>,
    gpu: Result<PDH_HCOUNTER, u32>,
}

impl PdhSources {
    pub fn open() -> Result<Self, u32> {
        let mut query = PDH_HQUERY::default();
        // SAFETY: opens a real-time query; closed in Drop.
        let status = unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &mut query) };
        if status != 0 {
            return Err(status);
        }
        let sources = Self {
            query,
            cpu: add(query, w!("\\Processor Information(*)\\% Processor Utility")),
            disk_idle: add(query, w!("\\PhysicalDisk(*)\\% Idle Time")),
            disk_read: add(query, w!("\\PhysicalDisk(*)\\Disk Read Bytes/sec")),
            disk_write: add(query, w!("\\PhysicalDisk(*)\\Disk Write Bytes/sec")),
            net: add(query, w!("\\Network Interface(*)\\Bytes Total/sec")),
            gpu: add(query, w!("\\GPU Engine(*)\\Utilization Percentage")),
        };
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
        let mut get = |label: &str, c: Result<PDH_HCOUNTER, u32>| -> Option<Vec<(String, f64)>> {
            let c = match c {
                Ok(c) => c,
                Err(status) => {
                    errors.push(format!("pdh {label}: counter unavailable (add failed {status:#010x})"));
                    return None;
                }
            };
            match read_array(c) {
                Ok(v) => Some(v),
                Err(e) => {
                    errors.push(format!("pdh {label}: {e:#010x}"));
                    None
                }
            }
        };
        let cpu = get("cpu", self.cpu);
        let idle = get("disk idle", self.disk_idle);
        let read = get("disk read", self.disk_read);
        let write = get("disk write", self.disk_write);
        let net = get("network", self.net);
        let gpu = get("gpu", self.gpu);

        let (gpu_pct, gpu_pct_by_pid) = match gpu {
            Some(v) => {
                let (sys, per) = gpu_from(&v);
                (sys, Some(per))
            }
            None => (None, None),
        };
        PdhReadings {
            cpu_pct: cpu.as_deref().and_then(total_from),
            per_core_pct: cpu.as_deref().map(per_core_from).unwrap_or_default(),
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

pub fn per_core_from(items: &[(String, f64)]) -> Vec<f32> {
    let mut cores: Vec<((u32, u32), f32)> = items
        .iter()
        .filter_map(|(n, v)| parse_core_instance(n).map(|k| (k, v.clamp(0.0, 100.0) as f32)))
        .collect();
    cores.sort_by_key(|(k, _)| *k);
    cores.into_iter().map(|(_, v)| v).collect()
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
    fn per_core_orders_by_group_then_core_and_clamps() {
        let v = items(&[("1,0", 50.0), ("0,10", 10.0), ("0,2", 120.0), ("_Total", 40.0), ("0,_Total", 30.0)]);
        assert_eq!(per_core_from(&v), vec![100.0, 10.0, 50.0]);
        assert_eq!(total_from(&v), Some(40.0));
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
        let failed = Err(0xC0000BB8u32);
        // Dropping `sources` closes the query.
        let mut sources = PdhSources {
            query,
            cpu: failed,
            disk_idle: failed,
            disk_read: failed,
            disk_write: failed,
            net: failed,
            gpu: failed,
        };
        let r = sources.read();
        assert!(r.cpu_pct.is_none() && r.disks.is_none() && r.net_bps.is_none() && r.gpu_pct.is_none());
        assert!(
            r.errors.iter().any(|e| e == "pdh cpu: counter unavailable (add failed 0xc0000bb8)"),
            "errors: {:?}",
            r.errors
        );
        assert_eq!(r.errors.iter().filter(|e| e.contains("counter unavailable")).count(), 6);
    }
}
