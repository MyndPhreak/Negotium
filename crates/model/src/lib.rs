//! Pure logic over snapshots: no Windows calls, fully unit-testable.
pub mod grouping;
pub mod rates;

pub use grouping::{AppKey, AppKind, SHELL_HOSTS, display_name, group};
pub use rates::{ProcRates, RateTracker};

use negotium_snapshot::Snapshot;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub struct ProcRow {
    pub pid: u32,
    pub create_time: i64,
    pub name: String,
    pub cpu_pct: f32,
    pub private_bytes: u64,
    pub disk_bps: f64,
    pub gpu_pct: Option<f32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AppRow {
    pub key: AppKey,
    pub name: String,
    pub kind: AppKind,
    pub processes: Vec<ProcRow>,
    pub cpu_pct: f32,
    pub private_bytes: u64,
    pub disk_bps: f64,
    pub gpu_pct: Option<f32>,
    /// Per-app network needs the SRUM source (Plan 4); `None` until then.
    pub net_bps: Option<f64>,
    pub hung_for_ms: Option<u64>,
}

pub struct Model {
    rates: RateTracker,
    system_root: String,
}

impl Default for Model {
    fn default() -> Self {
        Self::with_system_root(std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string()))
    }
}

impl Model {
    pub fn new() -> Self {
        Self::default()
    }

    /// For tests and callers that know the Windows directory.
    pub fn with_system_root(system_root: impl Into<String>) -> Self {
        Self { rates: RateTracker::new(), system_root: system_root.into() }
    }

    pub fn update(&mut self, snap: &Snapshot) -> Vec<AppRow> {
        let rates = self.rates.update(snap);
        let groups = group(&snap.processes, &snap.windows, &self.system_root);
        let gpu = snap.totals.gpu_pct_by_pid.as_ref();

        let mut rows: BTreeMap<AppKey, AppRow> = BTreeMap::new();
        for p in &snap.processes {
            let Some(key) = groups.get(&(p.pid, p.create_time)) else { continue };
            let r = rates.get(&(p.pid, p.create_time)).copied().unwrap_or_default();
            let proc_gpu = gpu.map(|m| m.get(&p.pid).copied().unwrap_or(0.0));
            let row = rows.entry(key.clone()).or_insert_with(|| AppRow {
                key: key.clone(),
                name: display_name(key, &snap.processes),
                kind: key.kind(),
                processes: Vec::new(),
                cpu_pct: 0.0,
                private_bytes: 0,
                disk_bps: 0.0,
                gpu_pct: gpu.map(|_| 0.0),
                net_bps: None,
                hung_for_ms: None,
            });
            row.cpu_pct = (row.cpu_pct + r.cpu_pct).min(100.0);
            row.private_bytes += p.private_working_set;
            row.disk_bps += r.disk_bps;
            if let (Some(total), Some(g)) = (row.gpu_pct.as_mut(), proc_gpu) {
                *total = (*total + g).min(100.0);
            }
            row.processes.push(ProcRow {
                pid: p.pid,
                create_time: p.create_time,
                name: p.name.clone(),
                cpu_pct: r.cpu_pct,
                private_bytes: p.private_working_set,
                disk_bps: r.disk_bps,
                gpu_pct: proc_gpu,
            });
        }
        rows.into_values().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use negotium_snapshot::{ProcessSample, Snapshot, SystemTotals, WindowSample};
    use std::collections::BTreeMap;

    fn p(pid: u32, parent: u32, cpu: u64, mem: u64) -> ProcessSample {
        ProcessSample {
            pid, parent_pid: parent, create_time: pid as i64, name: format!("app{pid}.exe"), session_id: 1,
            cpu_time_100ns: cpu, private_working_set: mem, disk_io_bytes: 0, threads: 1, handles: 0,
            image_path: None, package_family: None,
        }
    }

    #[test]
    fn rows_sum_their_processes() {
        let mut m = Model::with_system_root(r"C:\Windows");
        let mut gpu = BTreeMap::new();
        gpu.insert(2, 30.0);
        gpu.insert(3, 50.0);
        let snap = |t: u64, cpu2: u64, cpu3: u64| Snapshot {
            taken_at_ms: t,
            processes: vec![p(2, 1, cpu2, 100), p(3, 2, cpu3, 50)],
            windows: vec![WindowSample { pid: 2, hung: false, title: "App".into() }],
            totals: SystemTotals { logical_cpus: 1, gpu_pct_by_pid: Some(gpu.clone()), ..Default::default() },
            source_errors: vec![],
        };
        m.update(&snap(0, 0, 0));
        let rows = m.update(&snap(1000, 2_000_000, 3_000_000));
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.name, "app2");
        assert_eq!(row.processes.len(), 2);
        assert!((row.cpu_pct - 50.0).abs() < 0.01);
        assert_eq!(row.private_bytes, 150);
        assert_eq!(row.gpu_pct, Some(80.0));
        assert_eq!(row.net_bps, None);
    }
}
