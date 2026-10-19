//! Per-process CPU % and disk bytes/s from counter deltas between snapshots.
use negotium_snapshot::Snapshot;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ProcRates {
    pub cpu_pct: f32,
    pub disk_bps: f64,
}

#[derive(Default)]
pub struct RateTracker {
    prev: HashMap<(u32, i64), (u64, u64)>,
    prev_t_ms: Option<u64>,
}

impl RateTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn update(&mut self, snap: &Snapshot) -> HashMap<(u32, i64), ProcRates> {
        let elapsed_ms = self.prev_t_ms.map(|p| snap.taken_at_ms.saturating_sub(p)).unwrap_or(0);
        let cpus = snap.totals.logical_cpus.max(1) as f64;
        let capacity_100ns = elapsed_ms as f64 * 10_000.0 * cpus;

        let mut out = HashMap::with_capacity(snap.processes.len());
        let mut next = HashMap::with_capacity(snap.processes.len());
        for p in &snap.processes {
            let key = (p.pid, p.create_time);
            let rates = match self.prev.get(&key) {
                Some(&(cpu0, io0)) if elapsed_ms > 0 => {
                    let cpu_delta = p.cpu_time_100ns.saturating_sub(cpu0) as f64;
                    let io_delta = p.disk_io_bytes.saturating_sub(io0) as f64;
                    ProcRates {
                        cpu_pct: finite_clamp(cpu_delta / capacity_100ns * 100.0, 100.0) as f32,
                        disk_bps: finite_clamp(io_delta * 1000.0 / elapsed_ms as f64, f64::MAX),
                    }
                }
                _ => ProcRates::default(),
            };
            out.insert(key, rates);
            next.insert(key, (p.cpu_time_100ns, p.disk_io_bytes));
        }
        self.prev = next;
        self.prev_t_ms = Some(snap.taken_at_ms);
        out
    }
}

fn finite_clamp(v: f64, max: f64) -> f64 {
    if v.is_finite() { v.clamp(0.0, max) } else if v > 0.0 { max } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use negotium_snapshot::{ProcessSample, SystemTotals};

    fn proc(pid: u32, create: i64, cpu_100ns: u64, io: u64) -> ProcessSample {
        ProcessSample {
            pid, parent_pid: 0, create_time: create, name: format!("p{pid}.exe"), session_id: 1,
            cpu_time_100ns: cpu_100ns, private_working_set: 0, disk_io_bytes: io, threads: 1,
            handles: 0, image_path: None, package_family: None, description: None, publisher: None,
        }
    }

    fn snap(t_ms: u64, cpus: u32, procs: Vec<ProcessSample>) -> Snapshot {
        Snapshot { taken_at_ms: t_ms, processes: procs, totals: SystemTotals { logical_cpus: cpus, ..Default::default() }, ..Default::default() }
    }

    #[test]
    fn first_snapshot_has_zero_rates() {
        let mut t = RateTracker::new();
        let r = t.update(&snap(0, 2, vec![proc(1, 1, 500, 0)]));
        assert_eq!(r[&(1, 1)], ProcRates::default());
    }

    #[test]
    fn cpu_is_fraction_of_all_logical_processors() {
        let mut t = RateTracker::new();
        t.update(&snap(0, 2, vec![proc(1, 1, 0, 0)]));
        // 1000 ms on a 2-CPU box = 2000 ms capacity; 1000 ms used = 50 %.
        let r = t.update(&snap(1000, 2, vec![proc(1, 1, 10_000_000, 4096)]));
        assert!((r[&(1, 1)].cpu_pct - 50.0).abs() < 0.01);
        assert!((r[&(1, 1)].disk_bps - 4096.0).abs() < 0.01);
    }

    #[test]
    fn pid_reuse_is_a_new_process() {
        let mut t = RateTracker::new();
        t.update(&snap(0, 1, vec![proc(7, 100, 0, 0)]));
        let r = t.update(&snap(1000, 1, vec![proc(7, 200, 90_000_000, 0)]));
        assert_eq!(r[&(7, 200)].cpu_pct, 0.0);
    }

    #[test]
    fn backwards_counters_and_zero_elapsed_are_zero() {
        let mut t = RateTracker::new();
        t.update(&snap(1000, 1, vec![proc(1, 1, 5_000_000, 9000)]));
        let r = t.update(&snap(2000, 1, vec![proc(1, 1, 1_000_000, 10)]));
        assert_eq!(r[&(1, 1)], ProcRates::default());
        let r = t.update(&snap(2000, 1, vec![proc(1, 1, 9_000_000, 20)]));
        assert_eq!(r[&(1, 1)], ProcRates::default());
    }

    #[test]
    fn rates_are_clamped_and_finite() {
        let mut t = RateTracker::new();
        t.update(&snap(0, 0, vec![proc(1, 1, 0, 0)]));
        let r = t.update(&snap(10, 0, vec![proc(1, 1, u64::MAX / 2, 0)]));
        let cpu = r[&(1, 1)].cpu_pct;
        assert!(cpu.is_finite() && cpu == 100.0);
    }
}
