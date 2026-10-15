//! In-memory history: totals plus the top apps per metric, one sample per tick.
//! App keys are interned to small ids so an hour fits in about 2 MB.
use negotium_model::{AppKey, AppRow, Metric, top_n};
use negotium_snapshot::SystemTotals;
use std::collections::{HashMap, VecDeque};
use std::mem::size_of;

pub const DEFAULT_CAPACITY: usize = 3600;
pub const TOP_PER_METRIC: usize = 10;

#[derive(Clone, Debug, PartialEq)]
pub struct HistorySample {
    pub t_ms: u64,
    pub cpu_pct: Option<f32>,
    pub per_core_pct: Vec<Option<f32>>,
    pub mem_used: Option<u64>,
    pub disk_active_pct: Option<f32>,
    pub gpu_pct: Option<f32>,
    pub net_bps: Option<f64>,
    /// `top[metric as usize]` = (interned app id, value), largest first.
    pub top: [Vec<(u32, f32)>; 5],
}

pub struct History {
    samples: VecDeque<HistorySample>,
    capacity: usize,
    ids: HashMap<AppKey, u32>,
    names: Vec<String>,
    /// id -> key; `None` when the slot is free.
    keys: Vec<Option<AppKey>>,
    /// id -> number of top-list entries in retained samples that reference it.
    refs: Vec<u32>,
    /// Released ids available for reuse.
    free: Vec<u32>,
}

impl History {
    pub fn new(capacity: usize) -> Self {
        Self {
            samples: VecDeque::with_capacity(capacity),
            capacity: capacity.max(1),
            ids: HashMap::new(),
            names: Vec::new(),
            keys: Vec::new(),
            refs: Vec::new(),
            free: Vec::new(),
        }
    }

    pub fn push(&mut self, t_ms: u64, totals: &SystemTotals, rows: &[AppRow]) {
        let mut top: [Vec<(u32, f32)>; 5] = Default::default();
        for metric in Metric::ALL {
            top[metric as usize] = top_n(rows, metric, TOP_PER_METRIC)
                .into_iter()
                .map(|i| (self.intern(&rows[i]), rows[i].value(metric) as f32))
                .collect();
        }
        if self.samples.len() == self.capacity {
            if let Some(old) = self.samples.pop_front() {
                self.release(&old);
            }
        }
        self.samples.push_back(HistorySample {
            t_ms,
            cpu_pct: totals.cpu_pct,
            per_core_pct: totals.per_core_pct.clone(),
            mem_used: totals.memory.as_ref().map(|m| m.used),
            disk_active_pct: totals.busiest_disk_pct(),
            gpu_pct: totals.gpu_pct,
            net_bps: totals.net_bps,
            top,
        });
    }

    fn intern(&mut self, row: &AppRow) -> u32 {
        if let Some(&id) = self.ids.get(&row.key) {
            let i = id as usize;
            if self.names[i] != row.name {
                self.names[i] = row.name.clone();
            }
            self.refs[i] += 1;
            return id;
        }
        let id = match self.free.pop() {
            Some(id) => id,
            None => {
                self.names.push(String::new());
                self.keys.push(None);
                self.refs.push(0);
                (self.names.len() - 1) as u32
            }
        };
        let i = id as usize;
        self.names[i] = row.name.clone();
        self.keys[i] = Some(row.key.clone());
        self.refs[i] = 1;
        self.ids.insert(row.key.clone(), id);
        id
    }

    /// Drops the references held by an evicted sample; ids no longer referenced are freed.
    fn release(&mut self, sample: &HistorySample) {
        for list in &sample.top {
            for &(id, _) in list {
                let i = id as usize;
                self.refs[i] -= 1;
                if self.refs[i] == 0 {
                    if let Some(key) = self.keys[i].take() {
                        self.ids.remove(&key);
                    }
                    self.names[i].clear();
                    self.free.push(id);
                }
            }
        }
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub fn since(&self, t_ms: u64) -> impl Iterator<Item = &HistorySample> {
        self.samples.iter().filter(move |s| s.t_ms >= t_ms)
    }

    pub fn at(&self, t_ms: u64) -> Option<&HistorySample> {
        self.samples.iter().min_by_key(|s| s.t_ms.abs_diff(t_ms))
    }

    pub fn top_at(&self, t_ms: u64, metric: Metric) -> Vec<(String, f32)> {
        self.at(t_ms)
            .map(|s| s.top[metric as usize].iter().map(|(id, v)| (self.names[*id as usize].clone(), *v)).collect())
            .unwrap_or_default()
    }

    pub fn approx_bytes(&self) -> usize {
        let samples: usize = self
            .samples
            .iter()
            .map(|s| {
                size_of::<HistorySample>()
                    + s.per_core_pct.capacity() * size_of::<Option<f32>>()
                    + s.top.iter().map(|t| t.capacity() * size_of::<(u32, f32)>()).sum::<usize>()
            })
            .sum();
        let names: usize = self.names.iter().map(|n| n.capacity() + size_of::<String>() + size_of::<AppKey>() + 8).sum();
        let slots = self.keys.len() * (size_of::<Option<AppKey>>() + size_of::<u32>());
        samples + names + slots
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use negotium_model::{AppKey, AppKind};

    fn row(pid: u32, name: &str, cpu: f32) -> AppRow {
        AppRow {
            key: AppKey::Background { pid, create_time: 0 }, name: name.into(), kind: AppKind::Background,
            processes: vec![], cpu_pct: cpu, private_bytes: pid as u64 * 100, disk_bps: 0.0, gpu_pct: None,
            net_bps: None, hung_for_ms: None,
        }
    }
    fn totals(cpu: f32, cores: usize) -> SystemTotals {
        SystemTotals { logical_cpus: cores as u32, cpu_pct: Some(cpu), per_core_pct: vec![Some(cpu); cores], ..Default::default() }
    }

    #[test]
    fn evicts_oldest_beyond_capacity() {
        let mut h = History::new(3);
        for t in 0..5 {
            h.push(t * 1000, &totals(t as f32, 2), &[]);
        }
        assert_eq!(h.len(), 3);
        assert_eq!(h.since(0).next().unwrap().t_ms, 2000);
    }

    #[test]
    fn at_returns_nearest_sample() {
        let mut h = History::new(10);
        h.push(1000, &totals(1.0, 1), &[]);
        h.push(2000, &totals(2.0, 1), &[]);
        assert_eq!(h.at(1400).unwrap().t_ms, 1000);
        assert_eq!(h.at(1600).unwrap().t_ms, 2000);
        assert_eq!(h.at(99_999).unwrap().t_ms, 2000);
        assert!(History::new(1).at(0).is_none());
    }

    #[test]
    fn top_at_resolves_names_and_interns_keys() {
        let mut h = History::new(10);
        h.push(1000, &totals(10.0, 1), &[row(1, "chrome", 40.0), row(2, "code", 20.0)]);
        h.push(2000, &totals(10.0, 1), &[row(1, "chrome", 30.0)]);
        assert_eq!(h.top_at(1000, Metric::Cpu), vec![("chrome".to_string(), 40.0), ("code".to_string(), 20.0)]);
        assert_eq!(h.top_at(2000, Metric::Cpu), vec![("chrome".to_string(), 30.0)]);
        assert_eq!(h.names.len(), 2);
    }

    #[test]
    fn one_hour_fits_the_memory_budget() {
        let mut h = History::new(DEFAULT_CAPACITY);
        let rows: Vec<AppRow> = (1..=40).map(|i| row(i, &format!("app{i}"), i as f32)).collect();
        for t in 0..DEFAULT_CAPACITY as u64 {
            h.push(t * 1000, &totals(50.0, 16), &rows);
        }
        assert_eq!(h.len(), DEFAULT_CAPACITY);
        assert!(h.approx_bytes() <= 2_500_000, "{} bytes", h.approx_bytes());
    }

    #[test]
    fn name_table_stays_bounded_under_key_churn() {
        let mut h = History::new(5);
        for t in 0..1000u64 {
            h.push(t * 1000, &totals(1.0, 1), &[row(t as u32 + 1, &format!("p{t}"), 10.0)]);
        }
        assert!(h.ids.len() <= 5, "{} live ids", h.ids.len());
        assert!(h.names.len() <= 6, "{} name slots", h.names.len());
        assert_eq!(h.top_at(999_000, Metric::Cpu), vec![("p999".to_string(), 10.0)]);
    }
}
