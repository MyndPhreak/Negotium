//! Builds the plain-data Frame the UI renders. Runs on the worker thread; `visible_rows`
//! runs on the UI thread (no Windows calls anywhere here).
use crate::format;
use negotium_history::History;
use negotium_model::{AppKey, AppRow, Metric, top_n};
use negotium_snapshot::Snapshot;
use std::collections::{HashMap, HashSet};

pub const SPARK_SECONDS: u64 = 60;
pub const TOP_TILES: usize = 5;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MeterView { pub label: &'static str, pub value: String, pub sub: String, pub spark: String, pub available: bool }

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TileView { pub id: i32, pub name: String, pub value: String, pub weight: f32, pub hung: bool }

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RowView {
    pub id: i32, pub parent: i32, pub depth: i32, pub name: String, pub search_text: String, pub count: String,
    pub cpu: String, pub mem: String, pub disk: String, pub gpu: String, pub net: String,
    pub bar: f32, pub hung: String, pub expandable: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Target {
    pub name: String,
    pub root: Option<(u32, i64)>,
    pub procs: Vec<(u32, i64)>,
    pub hung_pid: Option<(u32, i64)>,
    pub image_path: Option<String>,
    pub critical: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Frame {
    pub meters: Vec<MeterView>,
    pub tiles: Vec<TileView>,
    pub caption: String,
    pub rows: Vec<RowView>,
    pub targets: HashMap<i32, Target>,
    pub all_procs: Vec<(u32, i64, u32)>,
    pub admin_banner: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum RowKey {
    App(AppKey),
    Proc(u32, i64),
}

/// Stable small ids for rows across frames; ids of rows not seen in a frame are released.
#[derive(Default)]
pub struct IdMap {
    ids: HashMap<RowKey, i32>,
    next: i32,
}

impl IdMap {
    pub fn new() -> Self {
        Self::default()
    }
    fn id(&mut self, key: RowKey, seen: &mut HashSet<RowKey>) -> i32 {
        seen.insert(key.clone());
        if let Some(&id) = self.ids.get(&key) {
            return id;
        }
        self.next += 1;
        self.ids.insert(key, self.next);
        self.next
    }
    fn retain(&mut self, seen: &HashSet<RowKey>) {
        self.ids.retain(|k, _| seen.contains(k));
    }
}

pub fn spark_path(values: &[Option<f32>], max: f32) -> String {
    if values.is_empty() {
        return "M 0 100".to_string();
    }
    let n = values.len() as i64;
    let mut out = String::new();
    let mut pen_down = false;
    for (i, v) in values.iter().enumerate() {
        let x = SPARK_SECONDS as i64 - (n - 1 - i as i64);
        match v {
            Some(v) if max > 0.0 && v.is_finite() => {
                let y = (100.0 - (v / max * 100.0).clamp(0.0, 100.0)).round() as i64;
                out.push_str(&format!("{}{} {x} {y}", if out.is_empty() { "" } else { " " }, if pen_down { "L" } else { "M" }));
                pen_down = true;
            }
            Some(_) => {
                out.push_str(&format!("{}M {x} 100", if out.is_empty() { "" } else { " " }));
                pen_down = false;
            }
            None => pen_down = false,
        }
    }
    if out.is_empty() { "M 0 100".to_string() } else { out }
}

fn metric_text(row_value: f64, metric: Metric) -> String {
    match metric {
        Metric::Cpu | Metric::Gpu => format::pct1(row_value as f32),
        Metric::Memory => format::bytes(row_value),
        Metric::Disk => format::byte_rate(row_value),
        Metric::Network => format::bit_rate(row_value),
    }
}

fn caption(metric: Metric) -> &'static str {
    match metric {
        Metric::Cpu => "Top CPU users",
        Metric::Memory => "Top memory users",
        Metric::Disk => "Top disk users",
        Metric::Gpu => "Top GPU users",
        Metric::Network => "Top network users",
    }
}

fn meters(snap: &Snapshot, history: &History) -> Vec<MeterView> {
    let t = &snap.totals;
    let since = snap.taken_at_ms.saturating_sub(SPARK_SECONDS * 1000);
    let samples: Vec<_> = history.since(since).collect();
    let mem_total = t.memory.as_ref().map(|m| m.total as f32).unwrap_or(0.0);
    let net_series: Vec<Option<f32>> = samples.iter().map(|s| s.net_bps.map(|v| v as f32)).collect();
    let net_max = net_series.iter().flatten().fold(0.0f32, |a, b| a.max(*b)).max(t.net_bps.unwrap_or(0.0) as f32);
    vec![
        MeterView {
            label: "CPU",
            value: t.cpu_pct.map(format::pct).unwrap_or_default(),
            sub: String::new(),
            spark: spark_path(&samples.iter().map(|s| s.cpu_pct).collect::<Vec<_>>(), 100.0),
            available: t.cpu_pct.is_some(),
        },
        MeterView {
            label: "Memory",
            value: t.memory.as_ref().map(|m| format::pct(m.used as f32 / m.total.max(1) as f32 * 100.0)).unwrap_or_default(),
            sub: t.memory.as_ref().map(|m| {
                let gb = |b: u64| b as f64 / (1u64 << 30) as f64;
                format!("{:.1} / {:.1} GB", gb(m.used), gb(m.total))
            }).unwrap_or_default(),
            spark: spark_path(&samples.iter().map(|s| s.mem_used.map(|u| u as f32)).collect::<Vec<_>>(), mem_total),
            available: t.memory.is_some(),
        },
        MeterView {
            label: "Disk",
            value: t.busiest_disk_pct().map(format::pct).unwrap_or_default(),
            sub: String::new(),
            spark: spark_path(&samples.iter().map(|s| s.disk_active_pct).collect::<Vec<_>>(), 100.0),
            available: t.busiest_disk_pct().is_some(),
        },
        MeterView {
            label: "GPU",
            value: t.gpu_pct.map(format::pct).unwrap_or_default(),
            sub: String::new(),
            spark: spark_path(&samples.iter().map(|s| s.gpu_pct).collect::<Vec<_>>(), 100.0),
            available: t.gpu_pct.is_some(),
        },
        MeterView {
            label: "Network",
            value: t.net_bps.map(format::bit_rate).unwrap_or_default(),
            sub: String::new(),
            spark: spark_path(&net_series, net_max),
            available: t.net_bps.is_some(),
        },
    ]
}

pub fn build_frame(
    ids: &mut IdMap, snap: &Snapshot, rows: &[AppRow], order: &[usize], history: &History, metric: Metric, elevated: bool,
) -> Frame {
    let mut seen = HashSet::new();
    let hung_pids: HashSet<u32> = snap.windows.iter().filter(|w| w.hung).map(|w| w.pid).collect();
    let image_of: HashMap<(u32, i64), &str> = snap
        .processes
        .iter()
        .filter_map(|p| p.image_path.as_deref().map(|i| ((p.pid, p.create_time), i)))
        .collect();
    let max = rows.iter().map(|r| r.value(metric)).fold(0.0f64, f64::max);
    let frac = |v: f64| if max > 0.0 && v.is_finite() { (v / max).clamp(0.0, 1.0) as f32 } else { 0.0 };

    let mut out_rows = Vec::new();
    let mut targets = HashMap::new();
    for &i in order {
        let r = &rows[i];
        let id = ids.id(RowKey::App(r.key.clone()), &mut seen);
        let root = match r.key {
            AppKey::Window { pid, create_time } | AppKey::Background { pid, create_time } => Some((pid, create_time)),
            _ => None,
        };
        let procs: Vec<(u32, i64)> = r.processes.iter().map(|p| (p.pid, p.create_time)).collect();
        let hung_pid = r.processes.iter().find(|p| hung_pids.contains(&p.pid)).map(|p| (p.pid, p.create_time));
        targets.insert(id, Target {
            name: r.name.clone(),
            root,
            procs: procs.clone(),
            hung_pid,
            image_path: root.or(procs.first().copied()).and_then(|k| image_of.get(&k).map(|s| s.to_string())),
            critical: r.processes.iter().any(|p| negotium_actions::is_critical(&p.name)),
        });
        let search_text = std::iter::once(r.name.to_lowercase())
            .chain(r.processes.iter().map(|p| p.name.to_lowercase()))
            .collect::<Vec<_>>()
            .join(" ");
        out_rows.push(RowView {
            id, parent: -1, depth: 0, name: r.name.clone(), search_text,
            count: if r.processes.len() > 1 { format!("({})", r.processes.len()) } else { String::new() },
            cpu: format::pct1(r.cpu_pct), mem: format::bytes(r.private_bytes as f64), disk: format::byte_rate(r.disk_bps),
            gpu: r.gpu_pct.map(format::pct1).unwrap_or_else(|| "—".into()),
            net: r.net_bps.map(format::bit_rate).unwrap_or_else(|| "—".into()),
            bar: frac(r.value(metric)), hung: r.hung_for_ms.map(format::hung).unwrap_or_default(),
            expandable: r.processes.len() > 1,
        });
        if r.processes.len() > 1 {
            for p in &r.processes {
                let pid_key = (p.pid, p.create_time);
                let cid = ids.id(RowKey::Proc(p.pid, p.create_time), &mut seen);
                targets.insert(cid, Target {
                    name: p.name.clone(), root: Some(pid_key), procs: vec![pid_key],
                    hung_pid: hung_pids.contains(&p.pid).then_some(pid_key),
                    image_path: image_of.get(&pid_key).map(|s| s.to_string()),
                    critical: negotium_actions::is_critical(&p.name),
                });
                out_rows.push(RowView {
                    id: cid, parent: id, depth: 1, name: p.name.clone(), search_text: p.name.to_lowercase(), count: String::new(),
                    cpu: format::pct1(p.cpu_pct), mem: format::bytes(p.private_bytes as f64), disk: format::byte_rate(p.disk_bps),
                    gpu: p.gpu_pct.map(format::pct1).unwrap_or_else(|| "—".into()), net: "—".into(),
                    bar: 0.0, hung: String::new(), expandable: false,
                });
            }
        }
    }
    ids.retain(&seen);

    let tiles = top_n(rows, metric, TOP_TILES)
        .into_iter()
        .map(|i| {
            let r = &rows[i];
            let id = ids.ids.get(&RowKey::App(r.key.clone())).copied().unwrap_or(0);
            TileView { id, name: r.name.clone(), value: metric_text(r.value(metric), metric), weight: r.value(metric) as f32, hung: r.hung_for_ms.is_some() }
        })
        .collect();

    let all_procs = snap.processes.iter().map(|p| (p.pid, p.create_time, p.parent_pid)).collect();
    let admin_banner = !elevated && snap.processes.iter().any(|p| p.pid > 4 && p.image_path.is_none());
    Frame { meters: meters(snap, history), tiles, caption: caption(metric).to_string(), rows: out_rows, targets, all_procs, admin_banner }
}

pub fn visible_rows(rows: &[RowView], expanded: &HashSet<i32>, search: &str) -> Vec<RowView> {
    let needle = search.trim().to_lowercase();
    let matches_app = |r: &RowView| needle.is_empty() || r.search_text.contains(&needle);
    let app_ok: HashSet<i32> = rows.iter().filter(|r| r.depth == 0 && matches_app(r)).map(|r| r.id).collect();
    rows.iter()
        .filter(|r| {
            if r.depth == 0 {
                app_ok.contains(&r.id)
            } else {
                app_ok.contains(&r.parent)
                    && (expanded.contains(&r.parent) || (!needle.is_empty() && r.search_text.contains(&needle)))
            }
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use negotium_model::{AppKind, ProcRow};
    use negotium_snapshot::{MemoryTotals, SystemTotals};

    fn proc(pid: u32, parent: u32, cpu: f32, name: &str) -> ProcRow {
        ProcRow { pid, create_time: pid as i64, parent_pid: parent, name: name.into(), cpu_pct: cpu, private_bytes: 1024, disk_bps: 0.0, gpu_pct: None }
    }
    fn app(pid: u32, name: &str, cpu: f32, procs: Vec<ProcRow>, hung: Option<u64>) -> AppRow {
        AppRow {
            key: AppKey::Window { pid, create_time: pid as i64 }, name: name.into(), kind: AppKind::App,
            cpu_pct: cpu, private_bytes: procs.len() as u64 * 1024, disk_bps: 0.0, gpu_pct: None, net_bps: None,
            hung_for_ms: hung, processes: procs,
        }
    }
    fn snap(cpu: Option<f32>) -> Snapshot {
        Snapshot {
            taken_at_ms: 60_000,
            totals: SystemTotals { logical_cpus: 4, cpu_pct: cpu, memory: Some(MemoryTotals { total: 16 << 30, used: 8 << 30 }), ..Default::default() },
            ..Default::default()
        }
    }

    #[test]
    fn spark_path_right_aligns_samples_and_breaks_on_gaps() {
        assert_eq!(spark_path(&[Some(0.0), Some(50.0), Some(100.0)], 100.0), "M 58 100 L 59 50 L 60 0");
        assert_eq!(spark_path(&[Some(10.0), None, Some(10.0)], 100.0), "M 58 90 M 60 90");
        assert_eq!(spark_path(&[], 100.0), "M 0 100");
        assert_eq!(spark_path(&[Some(5.0)], 0.0), "M 60 100");
    }

    #[test]
    fn frame_has_five_meters_tiles_and_rows_in_order() {
        let rows = vec![
            app(1, "Chrome", 30.0, vec![proc(1, 0, 20.0, "chrome.exe"), proc(2, 1, 10.0, "chrome.exe")], None),
            app(3, "Editor", 5.0, vec![proc(3, 0, 5.0, "code.exe")], Some(14_400)),
        ];
        let mut ids = IdMap::new();
        let f = build_frame(&mut ids, &snap(Some(37.6)), &rows, &[1, 0], &History::new(10), Metric::Cpu, true);
        assert_eq!(f.meters.len(), 5);
        assert_eq!(f.meters[0].value, "38%");
        assert_eq!(f.meters[1].sub, "8.0 / 16.0 GB");
        assert_eq!(f.tiles.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["Chrome", "Editor"]);
        assert_eq!(f.caption, "Top CPU users");
        let names: Vec<_> = f.rows.iter().filter(|r| r.depth == 0).map(|r| r.name.clone()).collect();
        assert_eq!(names, ["Editor", "Chrome"], "rows follow the ranker order");
        let editor = f.rows.iter().find(|r| r.name == "Editor").unwrap();
        assert_eq!(editor.hung, "Not responding · 14s");
        assert_eq!(f.targets[&editor.id].root, Some((3, 3)));
        let chrome = f.rows.iter().find(|r| r.name == "Chrome").unwrap();
        assert_eq!(chrome.count, "(2)");
        assert!(chrome.expandable);
        assert!((chrome.bar - 1.0).abs() < 1e-6);
        assert!(!f.admin_banner);
    }

    #[test]
    fn degenerate_inputs_do_not_divide_by_zero() {
        let rows = vec![app(1, "Idle", 0.0, vec![proc(1, 0, 0.0, "idle.exe")], None)];
        let mut ids = IdMap::new();
        let f = build_frame(&mut ids, &snap(None), &rows, &[0], &History::new(10), Metric::Gpu, false);
        assert!(f.tiles.is_empty());
        assert!(f.rows.iter().all(|r| r.bar == 0.0));
        assert!(!f.meters[0].available, "CPU meter shows Unavailable when cpu_pct is None");
        assert!(!f.meters[3].available, "GPU meter shows Unavailable without GPU counters");
    }

    #[test]
    fn empty_disk_list_marks_disk_meter_unavailable() {
        let mut s = snap(Some(1.0));
        s.totals.disks = Some(vec![]);
        let mut ids = IdMap::new();
        let f = build_frame(&mut ids, &s, &[], &[], &History::new(10), Metric::Cpu, true);
        assert!(!f.meters[2].available);
    }

    #[test]
    fn ids_are_stable_across_frames() {
        let rows = vec![app(1, "Chrome", 30.0, vec![proc(1, 0, 30.0, "chrome.exe")], None)];
        let mut ids = IdMap::new();
        let a = build_frame(&mut ids, &snap(Some(1.0)), &rows, &[0], &History::new(10), Metric::Cpu, true);
        let b = build_frame(&mut ids, &snap(Some(1.0)), &rows, &[0], &History::new(10), Metric::Cpu, true);
        assert_eq!(a.rows[0].id, b.rows[0].id);
    }

    #[test]
    fn visible_rows_expand_and_search() {
        let rows = vec![
            RowView { id: 1, parent: -1, depth: 0, name: "Chrome".into(), search_text: "chrome chrome.exe gpu-helper.exe".into(), expandable: true, ..Default::default() },
            RowView { id: 2, parent: 1, depth: 1, name: "gpu-helper.exe".into(), search_text: "gpu-helper.exe".into(), ..Default::default() },
            RowView { id: 3, parent: -1, depth: 0, name: "Editor".into(), search_text: "editor code.exe".into(), ..Default::default() },
        ];
        let none = HashSet::new();
        assert_eq!(visible_rows(&rows, &none, "").iter().map(|r| r.id).collect::<Vec<_>>(), [1, 3]);
        assert_eq!(visible_rows(&rows, &HashSet::from([1]), "").iter().map(|r| r.id).collect::<Vec<_>>(), [1, 2, 3]);
        assert_eq!(visible_rows(&rows, &none, "HELPER").iter().map(|r| r.id).collect::<Vec<_>>(), [1, 2]);
        assert!(visible_rows(&rows, &none, "zzz").is_empty());
    }
}
