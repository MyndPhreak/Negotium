//! Stable ordering for the app list. Re-sorts at most every RESORT_INTERVAL_MS; never
//! while frozen (pointer over the list) except on a metric change, which is an explicit
//! user action; immediately on metric change; immediately when not frozen if the set of
//! hung apps changes. Hung apps lead.
use crate::{AppKey, AppRow};
use std::cmp::Ordering;
use std::collections::HashMap;

pub const RESORT_INTERVAL_MS: u64 = 2000;

#[repr(usize)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Metric {
    #[default]
    Cpu = 0,
    Memory = 1,
    Disk = 2,
    Gpu = 3,
    Network = 4,
}

impl Metric {
    pub const ALL: [Metric; 5] = [Metric::Cpu, Metric::Memory, Metric::Disk, Metric::Gpu, Metric::Network];
}

impl AppRow {
    pub fn value(&self, metric: Metric) -> f64 {
        match metric {
            Metric::Cpu => self.cpu_pct as f64,
            Metric::Memory => self.private_bytes as f64,
            Metric::Disk => self.disk_bps,
            Metric::Gpu => self.gpu_pct.unwrap_or(0.0) as f64,
            Metric::Network => self.net_bps.unwrap_or(0.0),
        }
    }
}

#[derive(Default)]
pub struct Ranker {
    metric: Option<Metric>,
    order: Vec<AppKey>,
    hung: Vec<AppKey>,
    last_sort_ms: u64,
}

impl Ranker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn order(&mut self, rows: &[AppRow], metric: Metric, now_ms: u64, frozen: bool) -> Vec<usize> {
        let mut hung: Vec<AppKey> = rows.iter().filter(|r| r.hung_for_ms.is_some()).map(|r| r.key.clone()).collect();
        hung.sort();
        let due = now_ms.saturating_sub(self.last_sort_ms) >= RESORT_INTERVAL_MS;
        let resort = self.metric != Some(metric)
            || self.order.is_empty()
            || (!frozen && (due || hung != self.hung));

        let index: HashMap<&AppKey, usize> = rows.iter().enumerate().map(|(i, r)| (&r.key, i)).collect();
        let result: Vec<usize> = if resort {
            let mut all: Vec<usize> = (0..rows.len()).collect();
            all.sort_by(|&a, &b| compare(&rows[a], &rows[b], metric));
            self.last_sort_ms = now_ms;
            all
        } else {
            let mut kept: Vec<usize> = self.order.iter().filter_map(|k| index.get(k).copied()).collect();
            let mut fresh: Vec<usize> = (0..rows.len()).filter(|i| !self.order.contains(&rows[*i].key)).collect();
            fresh.sort_by(|&a, &b| compare(&rows[a], &rows[b], metric));
            kept.extend(fresh);
            kept
        };
        self.metric = Some(metric);
        self.order = result.iter().map(|&i| rows[i].key.clone()).collect();
        if resort {
            self.hung = hung;
        }
        result
    }
}

fn compare(a: &AppRow, b: &AppRow, metric: Metric) -> Ordering {
    b.hung_for_ms
        .is_some()
        .cmp(&a.hung_for_ms.is_some())
        .then_with(|| b.value(metric).total_cmp(&a.value(metric)))
        .then_with(|| a.name.cmp(&b.name))
}

/// Largest `n` rows with a positive value for `metric`.
pub fn top_n(rows: &[AppRow], metric: Metric, n: usize) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..rows.len()).filter(|&i| rows[i].value(metric) > 0.0).collect();
    idx.sort_by(|&a, &b| rows[b].value(metric).total_cmp(&rows[a].value(metric)).then_with(|| rows[a].name.cmp(&rows[b].name)));
    idx.truncate(n);
    idx
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AppKey, AppKind};

    fn row(pid: u32, name: &str, cpu: f32, hung: bool) -> AppRow {
        AppRow {
            key: AppKey::Background { pid, create_time: 0 }, name: name.into(), kind: AppKind::Background,
            processes: vec![], cpu_pct: cpu, private_bytes: 0, disk_bps: 0.0, gpu_pct: None, net_bps: None,
            hung_for_ms: hung.then_some(5000),
        }
    }
    fn names(rows: &[AppRow], order: &[usize]) -> Vec<String> {
        order.iter().map(|&i| rows[i].name.clone()).collect()
    }

    #[test]
    fn sorts_by_metric_desc_then_name() {
        let rows = vec![row(1, "b", 10.0, false), row(2, "a", 10.0, false), row(3, "c", 50.0, false)];
        let mut r = Ranker::new();
        assert_eq!(names(&rows, &r.order(&rows, Metric::Cpu, 0, false)), ["c", "a", "b"]);
    }

    #[test]
    fn holds_order_between_resorts_then_resorts() {
        let mut r = Ranker::new();
        let rows = vec![row(1, "a", 50.0, false), row(2, "b", 10.0, false)];
        r.order(&rows, Metric::Cpu, 0, false);
        let changed = vec![row(1, "a", 5.0, false), row(2, "b", 60.0, false)];
        assert_eq!(names(&changed, &r.order(&changed, Metric::Cpu, 1000, false)), ["a", "b"]);
        assert_eq!(names(&changed, &r.order(&changed, Metric::Cpu, 2000, false)), ["b", "a"]);
    }

    #[test]
    fn metric_change_resorts_immediately() {
        let mut r = Ranker::new();
        let mut rows = vec![row(1, "a", 50.0, false), row(2, "b", 10.0, false)];
        rows[1].private_bytes = 999;
        r.order(&rows, Metric::Cpu, 0, false);
        assert_eq!(names(&rows, &r.order(&rows, Metric::Memory, 10, false)), ["b", "a"]);
    }

    #[test]
    fn metric_change_resorts_even_while_frozen() {
        let mut r = Ranker::new();
        let mut rows = vec![row(1, "a", 50.0, false), row(2, "b", 10.0, false)];
        rows[1].private_bytes = 999;
        assert_eq!(names(&rows, &r.order(&rows, Metric::Cpu, 0, false)), ["a", "b"]);
        assert_eq!(names(&rows, &r.order(&rows, Metric::Memory, 10, true)), ["b", "a"]);
    }

    #[test]
    fn frozen_keeps_order_appends_new_and_drops_gone() {
        let mut r = Ranker::new();
        let rows = vec![row(1, "a", 50.0, false), row(2, "b", 10.0, false)];
        r.order(&rows, Metric::Cpu, 0, false);
        let next = vec![row(2, "b", 90.0, false), row(3, "new", 99.0, false)];
        assert_eq!(names(&next, &r.order(&next, Metric::Cpu, 10_000, true)), ["b", "new"]);
    }

    #[test]
    fn hung_apps_pin_to_top_and_trigger_resort() {
        let mut r = Ranker::new();
        let rows = vec![row(1, "a", 50.0, false), row(2, "b", 1.0, false)];
        r.order(&rows, Metric::Cpu, 0, false);
        let next = vec![row(1, "a", 50.0, false), row(2, "b", 1.0, true)];
        assert_eq!(names(&next, &r.order(&next, Metric::Cpu, 500, false)), ["b", "a"]);
    }

    #[test]
    fn top_n_skips_zero_and_limits() {
        let rows = vec![row(1, "a", 0.0, false), row(2, "b", 5.0, false), row(3, "c", 9.0, false)];
        assert_eq!(names(&rows, &top_n(&rows, Metric::Cpu, 1)), ["c"]);
        assert_eq!(names(&rows, &top_n(&rows, Metric::Cpu, 5)), ["c", "b"]);
    }
}
