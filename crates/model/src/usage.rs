//! Cumulative per-app resource usage tracker since Negotium started (or since the last reset).
//! Tracks CPU time, Disk I/O bytes, Network data bytes, and peak memory across both currently
//! running and already-closed applications.
use crate::{AppKey, AppKind, AppRow};
use negotium_snapshot::Snapshot;
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppUsage {
    pub name: String,
    pub kind: AppKind,
    pub cpu_100ns: u64,
    pub disk_bytes: u64,
    pub net_bytes: u64,
    pub peak_mem_bytes: u64,
    pub icon_path: Option<String>,
    pub running: bool,
}

impl AppUsage {
    pub fn cpu_ms(&self) -> u64 {
        self.cpu_100ns / 10_000
    }
}

#[derive(Default)]
pub struct UsageTracker {
    started_at_ms: Option<u64>,
    last_at_ms: Option<u64>,
    prev_procs: HashMap<(u32, i64), (u64, u64)>,
    apps: HashMap<String, AppUsage>,
}

impl UsageTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Resets all accumulated history; the next snapshot establishes a fresh baseline.
    pub fn reset(&mut self) {
        self.started_at_ms = None;
        self.last_at_ms = None;
        self.prev_procs.clear();
        self.apps.clear();
    }

    /// Seconds elapsed since tracking started (or since the last reset).
    pub fn elapsed_s(&self, now_ms: u64) -> u64 {
        self.started_at_ms.map_or(0, |s| now_ms.saturating_sub(s) / 1000)
    }

    pub fn elapsed_s_now(&self) -> u64 {
        self.elapsed_s(self.last_at_ms.unwrap_or(0))
    }

    pub fn update(&mut self, snap: &Snapshot, rows: &[AppRow]) {
        let now = snap.taken_at_ms;
        self.started_at_ms.get_or_insert(now);
        let dt_s = self.last_at_ms.map_or(0.0, |prev| now.saturating_sub(prev) as f64 / 1000.0);
        self.last_at_ms = Some(now);

        let mut proc_deltas: HashMap<(u32, i64), (u64, u64)> = HashMap::with_capacity(snap.processes.len());
        let mut next_procs: HashMap<(u32, i64), (u64, u64)> = HashMap::with_capacity(snap.processes.len());
        let mut image_of: HashMap<(u32, i64), &str> = HashMap::with_capacity(snap.processes.len());

        for p in &snap.processes {
            let key = (p.pid, p.create_time);
            if let Some(&(prev_cpu, prev_disk)) = self.prev_procs.get(&key) {
                let d_cpu = p.cpu_time_100ns.saturating_sub(prev_cpu);
                let d_disk = p.disk_io_bytes.saturating_sub(prev_disk);
                proc_deltas.insert(key, (d_cpu, d_disk));
            }
            next_procs.insert(key, (p.cpu_time_100ns, p.disk_io_bytes));
            if let Some(img) = p.image_path.as_deref() {
                image_of.insert(key, img);
            }
        }
        self.prev_procs = next_procs;

        for app in self.apps.values_mut() {
            app.running = false;
        }

        for r in rows {
            if r.key == AppKey::Windows {
                continue;
            }
            let (mut d_cpu, mut d_disk) = (0u64, 0u64);
            for p in &r.processes {
                if let Some(&(c, d)) = proc_deltas.get(&(p.pid, p.create_time)) {
                    d_cpu = d_cpu.saturating_add(c);
                    d_disk = d_disk.saturating_add(d);
                }
            }
            let d_net = if dt_s > 0.0 {
                (r.net_bps.unwrap_or(0.0).max(0.0) * dt_s).round() as u64
            } else {
                0
            };
            let root = match r.key {
                AppKey::Window { pid, create_time } | AppKey::Background { pid, create_time } => Some((pid, create_time)),
                _ => None,
            };
            let icon = root
                .or_else(|| r.processes.first().map(|p| (p.pid, p.create_time)))
                .and_then(|k| image_of.get(&k).map(|s| (*s).to_string()));

            let entry = self.apps.entry(r.name.clone()).or_insert_with(|| AppUsage {
                name: r.name.clone(),
                kind: r.kind,
                cpu_100ns: 0,
                disk_bytes: 0,
                net_bytes: 0,
                peak_mem_bytes: 0,
                icon_path: None,
                running: true,
            });
            if r.kind == AppKind::App {
                entry.kind = AppKind::App;
            }
            entry.cpu_100ns = entry.cpu_100ns.saturating_add(d_cpu);
            entry.disk_bytes = entry.disk_bytes.saturating_add(d_disk);
            entry.net_bytes = entry.net_bytes.saturating_add(d_net);
            entry.peak_mem_bytes = entry.peak_mem_bytes.max(r.private_bytes);
            if entry.icon_path.is_none() && icon.is_some() {
                entry.icon_path = icon;
            }
            entry.running = true;
        }
    }

    pub fn entries(&self) -> Vec<AppUsage> {
        self.apps.values().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ProcRow;
    use negotium_snapshot::{ProcessSample, SystemTotals};

    fn sample(pid: u32, cpu_100ns: u64, disk_bytes: u64) -> ProcessSample {
        ProcessSample {
            pid,
            parent_pid: 1,
            create_time: pid as i64,
            name: format!("app{pid}.exe"),
            session_id: 1,
            cpu_time_100ns: cpu_100ns,
            private_working_set: 100 << 20,
            disk_io_bytes: disk_bytes,
            threads: 1,
            handles: 10,
            image_path: Some(format!(r"C:\app{pid}.exe")),
            package_family: None,
            description: None,
            publisher: None,
        }
    }

    fn app_row(pid: u32, name: &str, mem: u64, net_bps: Option<f64>) -> AppRow {
        AppRow {
            key: AppKey::Window { pid, create_time: pid as i64 },
            name: name.into(),
            kind: AppKind::App,
            processes: vec![ProcRow {
                pid,
                parent_pid: 1,
                create_time: pid as i64,
                name: format!("app{pid}.exe"),
                cpu_pct: 5.0,
                private_bytes: mem,
                disk_bps: 0.0,
                gpu_pct: None,
                net: None,
                hung_for_ms: None,
            }],
            cpu_pct: 5.0,
            private_bytes: mem,
            disk_bps: 0.0,
            gpu_pct: None,
            net_bps,
            net: None,
            hung_for_ms: None,
        }
    }

    #[test]
    fn accumulates_usage_since_start_and_retains_closed_apps() {
        let mut tracker = UsageTracker::new();
        let s1 = Snapshot {
            taken_at_ms: 1_000,
            processes: vec![sample(10, 50_000_000, 1_000_000)],
            totals: SystemTotals::default(),
            ..Default::default()
        };
        let r1 = vec![app_row(10, "Chrome", 200 << 20, Some(1000.0))];
        tracker.update(&s1, &r1);
        assert_eq!(tracker.elapsed_s(1_000), 0);

        let s2 = Snapshot {
            taken_at_ms: 3_000,
            processes: vec![sample(10, 75_000_000, 5_000_000)],
            totals: SystemTotals::default(),
            ..Default::default()
        };
        let r2 = vec![app_row(10, "Chrome", 350 << 20, Some(2000.0))];
        tracker.update(&s2, &r2);
        assert_eq!(tracker.elapsed_s(3_000), 2);

        // Chrome closes on tick 3; its cumulative history stays with running = false.
        let s3 = Snapshot { taken_at_ms: 4_000, ..Default::default() };
        tracker.update(&s3, &[]);
        let entries = tracker.entries();
        assert_eq!(entries.len(), 1);
        let chrome = &entries[0];
        assert_eq!(chrome.name, "Chrome");
        assert!(!chrome.running);
        assert_eq!(chrome.cpu_ms(), 2_500);
        assert_eq!(chrome.disk_bytes, 4_000_000);
        assert_eq!(chrome.net_bytes, 4_000);
        assert_eq!(chrome.peak_mem_bytes, 350 << 20);

        tracker.reset();
        assert!(tracker.entries().is_empty());
    }
}
