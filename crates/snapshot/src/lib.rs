//! Plain data produced by the collector each tick. No Windows types, no handles.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProcessSample {
    pub pid: u32,
    pub parent_pid: u32,
    /// FILETIME units (100 ns since 1601); with `pid` it identifies a process uniquely.
    pub create_time: i64,
    pub name: String,
    pub session_id: u32,
    /// User + kernel time, 100 ns units.
    pub cpu_time_100ns: u64,
    /// Private working set in bytes (Task Manager's "Memory" column).
    pub private_working_set: u64,
    /// Read + write transfer bytes since process start.
    pub disk_io_bytes: u64,
    pub threads: u32,
    pub handles: u32,
    pub image_path: Option<String>,
    pub package_family: Option<String>,
    /// FileDescription from the executable's version resource (e.g. "Google Chrome").
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowSample {
    pub pid: u32,
    pub hung: bool,
    pub title: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DiskSample {
    pub name: String,
    pub active_pct: f32,
    pub read_bps: f64,
    pub write_bps: f64,
    /// Average time per transfer, milliseconds. `None` when the counter has no value for this disk.
    #[serde(default)]
    pub response_ms: Option<f32>,
}

/// One "Network Interface" instance, joined to the interface table (friendly name, hardware or
/// virtual, link speed, addresses). `name` is the PDH instance name: the key for its history.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AdapterSample {
    pub name: String,
    /// "Ethernet"
    pub alias: Option<String>,
    pub description: Option<String>,
    pub sent_bps: f64,
    pub recv_bps: f64,
    pub hardware: bool,
    pub connected: bool,
    /// Link speed in bits per second.
    pub link_bps: Option<u64>,
    pub addresses: Vec<String>,
}

/// The kernel's page lists (PDH `Memory` counter set), for the memory composition bar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MemoryLists {
    pub modified: u64,
    /// Standby cache, all priorities.
    pub standby: u64,
    /// Free and zeroed pages.
    pub free: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MemoryTotals {
    pub total: u64,
    pub used: u64,
    /// Committed bytes and the commit limit (RAM + page files).
    #[serde(default)]
    pub commit_total: u64,
    #[serde(default)]
    pub commit_limit: u64,
    #[serde(default)]
    pub lists: Option<MemoryLists>,
    /// Resident size of the memory compression store (the "Memory Compression" process).
    #[serde(default)]
    pub compressed: Option<u64>,
}

fn nan_if_null<'de, D: serde::Deserializer<'de>>(d: D) -> Result<f32, D::Error> {
    Ok(Option::<f32>::deserialize(d)?.unwrap_or(f32::NAN))
}

/// One GPU (adapter instance "luid_…_phys_N", lowercase). util_pct is its busiest engine; engines
/// holds the busiest engine of each type, in display order.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GpuSample {
    pub key: String,
    /// NaN when unknown (no engine data for this adapter); JSON null reads back as NaN.
    #[serde(deserialize_with = "nan_if_null")]
    pub util_pct: f32,
    pub engines: Vec<(String, f32)>,
    pub dedicated_used: Option<u64>,
    pub shared_used: Option<u64>,
    pub temp_c: Option<f32>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SystemTotals {
    pub logical_cpus: u32,
    pub cpu_pct: Option<f32>,
    /// Utility of each logical processor, indexed by its ordinal across processor groups (group 0's
    /// processors first). `None` for a processor whose counter had no valid data this tick, so an
    /// index always means the same processor.
    pub per_core_pct: Vec<Option<f32>>,
    /// Current speed: nominal frequency × "% Processor Performance".
    #[serde(default)]
    pub cpu_mhz: Option<f32>,
    #[serde(default)]
    pub uptime_s: u64,
    /// Local wall-clock time when the tick was taken, milliseconds since midnight.
    #[serde(default)]
    pub local_time_ms: u64,
    pub memory: Option<MemoryTotals>,
    pub disks: Option<Vec<DiskSample>>,
    pub gpu_pct: Option<f32>,
    pub gpu_pct_by_pid: Option<BTreeMap<u32, f32>>,
    #[serde(default)]
    pub gpus: Option<Vec<GpuSample>>,
    /// Physical network throughput: hardware interfaces only (a vEthernet or VPN sees the same bytes).
    pub net_bps: Option<f64>,
    #[serde(default)]
    pub adapters: Option<Vec<AdapterSample>>,
}

impl SystemTotals {
    /// Active time of the busiest physical disk, as Task Manager's Disk meter shows.
    pub fn busiest_disk_pct(&self) -> Option<f32> {
        self.disks.as_ref()?.iter().map(|d| d.active_pct).reduce(f32::max)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    /// Milliseconds since the collector started (monotonic).
    pub taken_at_ms: u64,
    pub processes: Vec<ProcessSample>,
    pub windows: Vec<WindowSample>,
    pub totals: SystemTotals,
    pub source_errors: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disk(name: &str, active: f32) -> DiskSample {
        DiskSample { name: name.into(), active_pct: active, read_bps: 0.0, write_bps: 0.0, response_ms: None }
    }

    #[test]
    fn snapshot_round_trips_through_json() {
        let snap = Snapshot {
            taken_at_ms: 1000,
            processes: vec![ProcessSample {
                pid: 42, parent_pid: 4, create_time: 7, name: "a.exe".into(), session_id: 1,
                cpu_time_100ns: 10, private_working_set: 20, disk_io_bytes: 30, threads: 2,
                handles: 3, image_path: Some(r"C:\a.exe".into()), package_family: None, description: None,
            }],
            windows: vec![WindowSample { pid: 42, hung: false, title: "A".into() }],
            totals: SystemTotals { logical_cpus: 8, cpu_pct: Some(12.5), ..Default::default() },
            source_errors: vec![],
        };
        let json = serde_json::to_string(&snap).unwrap();
        let back: Snapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(back, snap);
    }

    #[test]
    fn unknown_gpu_utilization_survives_json() {
        let g = GpuSample { key: "k".into(), util_pct: f32::NAN, ..Default::default() };
        let back: GpuSample = serde_json::from_str(&serde_json::to_string(&g).unwrap()).unwrap();
        assert!(back.util_pct.is_nan());
    }

    #[test]
    fn busiest_disk_is_max_active() {
        let t = SystemTotals { disks: Some(vec![disk("0 C:", 12.0), disk("1 D:", 80.0)]), ..Default::default() };
        assert_eq!(t.busiest_disk_pct(), Some(80.0));
    }

    #[test]
    fn busiest_disk_none_when_unavailable_or_empty() {
        assert_eq!(SystemTotals::default().busiest_disk_pct(), None);
        let t = SystemTotals { disks: Some(vec![]), ..Default::default() };
        assert_eq!(t.busiest_disk_pct(), None);
    }
}
