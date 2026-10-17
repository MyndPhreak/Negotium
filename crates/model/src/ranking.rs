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

/// How long the top tiles' average remembers, as an exponential time constant (ms).
pub const TOP_AVERAGE_MS: f64 = 3000.0;
/// How far an app outside the top tiles must beat the weakest one, on average, to take its place.
pub const TOP_MARGIN: f64 = 1.2;

/// One top tile: the row and its recent average, which ranks it and sizes it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pick {
    pub index: usize,
    pub average: f64,
}

/// Picks the top tiles calmly: apps are ranked on a short exponential average, not the latest
/// reading, and an app outside the tiles must beat the weakest one by TOP_MARGIN to replace it,
/// so a spike or two apps sitting near the cutoff do not swap tiles every frame. A metric change
/// starts over from the current readings.
#[derive(Default)]
pub struct TopPicker {
    metric: Option<Metric>,
    last_ms: u64,
    average: HashMap<AppKey, f64>,
    members: Vec<AppKey>,
}

impl TopPicker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn pick(&mut self, rows: &[AppRow], metric: Metric, now_ms: u64, n: usize) -> Vec<Pick> {
        let fresh = self.metric != Some(metric);
        if fresh {
            self.average.clear();
            self.members.clear();
        }
        // A first sighting (a new app, or a metric change) starts at its reading.
        let alpha = 1.0 - (-(now_ms.saturating_sub(self.last_ms) as f64) / TOP_AVERAGE_MS).exp();
        let mut average = HashMap::with_capacity(rows.len());
        for r in rows {
            let v = r.value(metric);
            let a = self.average.get(&r.key).map_or(v, |&a| a + alpha * (v - a));
            average.insert(r.key.clone(), a);
        }
        self.average = average;
        self.metric = Some(metric);
        self.last_ms = now_ms;

        let index: HashMap<&AppKey, usize> = rows.iter().enumerate().map(|(i, r)| (&r.key, i)).collect();
        let avg = |i: usize| self.average[&rows[i].key];
        // An app idle now and whose average has faded to almost nothing leaves the tiles, as an
        // app with no reading never had one.
        let top = (0..rows.len()).map(avg).fold(0.0, f64::max);
        let eligible = |i: usize| rows[i].value(metric) > 0.0 || avg(i) > top * 0.01;
        let better = |a: usize, b: usize| avg(b).total_cmp(&avg(a)).then_with(|| rows[a].name.cmp(&rows[b].name));

        let mut members: Vec<usize> = self.members.iter().filter_map(|k| index.get(k).copied()).filter(|&i| eligible(i)).collect();
        let mut others: Vec<usize> = (0..rows.len()).filter(|i| !members.contains(i) && eligible(*i)).collect();
        others.sort_by(|&a, &b| better(a, b));
        let mut others = others.into_iter().peekable();
        while members.len() < n {
            match others.next() {
                Some(i) => members.push(i),
                None => break,
            }
        }
        // Challengers in rank order against the weakest member, each beating it by the margin.
        while let Some(&c) = others.peek() {
            let Some((w_pos, &w)) = members.iter().enumerate().min_by(|a, b| better(*b.1, *a.1)) else { break };
            if avg(c) > avg(w) * TOP_MARGIN {
                members[w_pos] = c;
                others.next();
            } else {
                break;
            }
        }
        members.sort_by(|&a, &b| better(a, b));
        self.members = members.iter().map(|&i| rows[i].key.clone()).collect();
        members.into_iter().map(|i| Pick { index: i, average: avg(i) }).collect()
    }
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

    fn picked(p: &mut TopPicker, rows: &[AppRow], now: u64, n: usize) -> Vec<String> {
        p.pick(rows, Metric::Cpu, now, n).iter().map(|k| rows[k.index].name.clone()).collect()
    }

    #[test]
    fn top_picker_ranks_on_the_average_so_a_one_frame_spike_does_not_reorder() {
        let mut p = TopPicker::new();
        let steady = vec![row(1, "a", 10.0, false), row(2, "b", 5.0, false)];
        assert_eq!(picked(&mut p, &steady, 0, 2), ["a", "b"]);
        assert_eq!(picked(&mut p, &steady, 10_000, 2), ["a", "b"]);
        // b spikes to 12 for one 500 ms frame: its average moves only part way.
        let spike = vec![row(1, "a", 10.0, false), row(2, "b", 12.0, false)];
        assert_eq!(picked(&mut p, &spike, 10_500, 2), ["a", "b"]);
        // Held for long, it overtakes.
        assert_eq!(picked(&mut p, &spike, 20_000, 2), ["b", "a"]);
    }

    #[test]
    fn top_picker_lets_a_challenger_in_only_by_the_margin() {
        let mut p = TopPicker::new();
        let rows = |c: f32| vec![row(1, "a", 10.0, false), row(2, "b", 5.0, false), row(3, "c", c, false)];
        assert_eq!(picked(&mut p, &rows(4.0), 0, 2), ["a", "b"]);
        // c settles just above b, but within the margin: b keeps its tile.
        assert_eq!(picked(&mut p, &rows(5.5), 60_000, 2), ["a", "b"]);
        // Clearly above b: c takes the tile.
        assert_eq!(picked(&mut p, &rows(7.0), 120_000, 2), ["a", "c"]);
    }

    #[test]
    fn top_picker_fills_free_tiles_without_a_margin_and_drops_faded_idle_apps() {
        let mut p = TopPicker::new();
        assert_eq!(picked(&mut p, &[row(1, "a", 10.0, false)], 0, 3), ["a"]);
        let rows = vec![row(1, "a", 10.0, false), row(2, "b", 0.5, false), row(3, "z", 0.0, false)];
        assert_eq!(picked(&mut p, &rows, 500, 3), ["a", "b"], "a free tile takes any app with a reading");
        let idle = vec![row(1, "a", 10.0, false), row(2, "b", 0.0, false)];
        assert_eq!(picked(&mut p, &idle, 600, 3), ["a", "b"], "just gone idle: still fading");
        assert_eq!(picked(&mut p, &idle, 60_000, 3), ["a"], "faded: gone");
    }

    #[test]
    fn top_picker_starts_over_on_a_metric_change() {
        let mut p = TopPicker::new();
        let mut a = row(1, "a", 10.0, false);
        a.private_bytes = 1;
        let mut b = row(2, "b", 1.0, false);
        b.private_bytes = 100;
        let rows = vec![a, b];
        assert_eq!(picked(&mut p, &rows, 0, 1), ["a"]);
        let mem: Vec<String> = p.pick(&rows, Metric::Memory, 500, 1).iter().map(|k| rows[k.index].name.clone()).collect();
        assert_eq!(mem, ["b"], "no margin or average carried over from CPU");
    }
}
