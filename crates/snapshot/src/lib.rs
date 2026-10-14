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
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MemoryTotals {
    pub total: u64,
    pub used: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SystemTotals {
    pub logical_cpus: u32,
    pub cpu_pct: Option<f32>,
    pub per_core_pct: Vec<f32>,
    pub memory: Option<MemoryTotals>,
    pub disks: Option<Vec<DiskSample>>,
    pub gpu_pct: Option<f32>,
    pub gpu_pct_by_pid: Option<BTreeMap<u32, f32>>,
    pub net_bps: Option<f64>,
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
        DiskSample { name: name.into(), active_pct: active, read_bps: 0.0, write_bps: 0.0 }
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
