//! Builds the plain-data Frame the UI renders. Runs on the worker thread; `visible_rows`
//! runs on the UI thread (no Windows calls anywhere here).
use crate::format;
use crate::graph;
use negotium_history::{History, Series};
use negotium_model::{AppKey, AppRow, Metric, Pick, Sort};
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
    /// False for the "Windows" group row: it is never a target of any action.
    pub actionable: bool,
    /// Gone since the list froze: held in place (so no other row shifts under the pointer), inert.
    pub exited: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Target {
    pub name: String,
    pub root: Option<(u32, i64)>,
    pub procs: Vec<(u32, i64)>,
    pub hung_pid: Option<(u32, i64)>,
    pub image_path: Option<String>,
    pub critical: bool,
    /// The "Windows" group (every session-0 / system process): no action may target it.
    pub group: bool,
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
    /// The page this frame was built for; only that page's fields are filled.
    pub page: crate::controls::Page,
    pub perf: Option<crate::perf::PerfFrame>,
    /// The newest history sample: where the Overview sparklines' right edge rests (crate::smooth).
    pub graph_end_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum RowKey {
    App(AppKey),
    Proc(u32, i64),
}

/// The id counter shared by every IdMap of one run of the app. A worker restarted after a panic
/// builds a fresh IdMap, while the UI still holds ids from before (frozen rows, the keyboard-current
/// row, expanded apps, an open menu's target): drawing from the same counter, the new map never
/// gives one of those ids to another app, so they become exited and inert.
pub type IdSeed = std::sync::Arc<std::sync::atomic::AtomicI32>;

/// Stable small ids for rows across frames; ids of rows not seen in a frame are released (and
/// never handed out again: see [`IdSeed`]).
#[derive(Default)]
pub struct IdMap {
    ids: HashMap<RowKey, i32>,
    next: IdSeed,
}

impl IdMap {
    pub fn new() -> Self {
        Self::default()
    }
    /// A map drawing its ids from `seed`, shared with the maps before and after it.
    pub fn with_seed(seed: IdSeed) -> Self {
        Self { ids: HashMap::new(), next: seed }
    }
    fn id(&mut self, key: RowKey, seen: &mut HashSet<RowKey>) -> i32 {
        seen.insert(key.clone());
        if let Some(&id) = self.ids.get(&key) {
            return id;
        }
        // Only the worker thread draws ids; the atomic just lets the seed outlive a restart.
        let id = self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        self.ids.insert(key, id);
        id
    }
    fn retain(&mut self, seen: &HashSet<RowKey>) {
        self.ids.retain(|k, _| seen.contains(k));
    }
}

pub(crate) fn metric_text(row_value: f64, metric: Metric) -> String {
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

/// The busiest GPU; the first one (DXGI order) on a tie, so the label does not flip at idle. An
/// unknown (NaN) utilization never wins over a known one.
fn busiest_gpu(gpus: &[negotium_snapshot::GpuSample]) -> Option<&negotium_snapshot::GpuSample> {
    let mut best: Option<&negotium_snapshot::GpuSample> = None;
    for g in gpus {
        let beats = match best {
            None => true,
            Some(b) => g.util_pct > b.util_pct || (b.util_pct.is_nan() && !g.util_pct.is_nan()),
        };
        if beats {
            best = Some(g);
        }
    }
    best
}

fn meters(snap: &Snapshot, history: &History) -> Vec<MeterView> {
    let t = &snap.totals;
    let span = SPARK_SECONDS * 1000;
    let end = graph::end_of(history, snap.taken_at_ms);
    let since = graph::since(end, span);
    let spark = |key: Series, max: f32| graph::line(history.points(&key, since), end, span, max, SPARK_SECONDS as usize);
    // A zero total is no reading (as on the Memory page): "Unavailable", never 100% in use.
    let memory = t.memory.as_ref().filter(|m| m.total > 0);
    let mem_total = memory.map(|m| m.total as f32).unwrap_or(0.0);
    let net_max = history.points(&Series::NetTotal, end.saturating_sub(span)).filter_map(|(_, v)| v).fold(0.0f32, f32::max).max(t.net_bps.unwrap_or(0.0) as f32);
    vec![
        MeterView {
            label: "CPU",
            value: t.cpu_pct.map(format::pct).unwrap_or_default(),
            sub: String::new(),
            spark: spark(Series::Cpu, 100.0),
            available: t.cpu_pct.is_some(),
        },
        MeterView {
            label: "Memory",
            value: memory.map(|m| format::pct(m.in_use() as f32 / m.total as f32 * 100.0)).unwrap_or_default(),
            sub: memory.map(|m| format::gb_of(m.in_use(), m.total)).unwrap_or_default(),
            spark: spark(Series::MemUsed, mem_total),
            available: memory.is_some(),
        },
        MeterView {
            label: "Disk",
            value: t.busiest_disk_pct().map(format::pct).unwrap_or_default(),
            sub: String::new(),
            spark: spark(Series::DiskBusiest, 100.0),
            available: t.busiest_disk_pct().is_some(),
        },
        MeterView {
            label: "GPU",
            value: t.gpu_pct.map(format::pct).unwrap_or_default(),
            sub: t
                .gpus
                .as_ref()
                .and_then(|g| busiest_gpu(g))
                .and_then(|g| g.temp_c)
                .map(format::temp_c)
                .unwrap_or_default(),
            spark: spark(Series::Gpu, 100.0),
            available: t.gpu_pct.is_some(),
        },
        MeterView {
            label: "Network",
            value: t.net_bps.map(format::bit_rate).unwrap_or_default(),
            sub: String::new(),
            spark: spark(Series::NetTotal, net_max),
            available: t.net_bps.is_some(),
        },
    ]
}

/// What the UI currently needs in a frame besides the snapshot itself.
#[derive(Clone, Debug)]
pub struct FrameOptions {
    pub metric: Metric,
    pub elevated: bool,
    /// App row ids the user expanded: only these get child rows (and child targets).
    pub expanded: HashSet<i32>,
    /// Build every app's child rows (a search can match any process).
    pub all_children: bool,
    /// The list's sort and whether it is grouped under type headings (Controls).
    pub sort: Sort,
    pub grouped: bool,
}

/// `top` is the top tiles, picked by a TopPicker (negotium_model) that lasts across frames.
pub fn build_frame(
    ids: &mut IdMap, snap: &Snapshot, rows: &[AppRow], order: &[usize], top: &[Pick], history: &History, opts: &FrameOptions,
) -> Frame {
    let (metric, elevated) = (opts.metric, opts.elevated);
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
            group: r.key == AppKey::Windows,
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
            actionable: r.key != AppKey::Windows,
            exited: false,
        });
        // Child rows only where the UI can show them: expanded apps, everything while searching, and
        // a Windows group with a hung member (the group row itself is never actionable, so the hung
        // process's own row is the only way to force-close it).
        let children = opts.all_children
            || opts.expanded.contains(&id)
            || (r.key == AppKey::Windows && r.hung_for_ms.is_some());
        if r.processes.len() > 1 && children {
            for p in &r.processes {
                let pid_key = (p.pid, p.create_time);
                let cid = ids.id(RowKey::Proc(p.pid, p.create_time), &mut seen);
                targets.insert(cid, Target {
                    name: p.name.clone(), root: Some(pid_key), procs: vec![pid_key],
                    hung_pid: hung_pids.contains(&p.pid).then_some(pid_key),
                    image_path: image_of.get(&pid_key).map(|s| s.to_string()),
                    critical: negotium_actions::is_critical(&p.name),
                    group: false,
                });
                out_rows.push(RowView {
                    id: cid, parent: id, depth: 1, name: p.name.clone(), search_text: p.name.to_lowercase(), count: String::new(),
                    cpu: format::pct1(p.cpu_pct), mem: format::bytes(p.private_bytes as f64), disk: format::byte_rate(p.disk_bps),
                    gpu: p.gpu_pct.map(format::pct1).unwrap_or_else(|| "—".into()), net: "—".into(),
                    // Its own hung text, so Force close shows on (and targets) this process even
                    // when its app row is the never-actionable Windows group.
                    bar: 0.0, hung: p.hung_for_ms.map(format::hung).unwrap_or_default(), expandable: false, actionable: true,
                    exited: false,
                });
            }
        }
    }
    ids.retain(&seen);

    // Sized by the average that ranks them; the text is the live reading, as in the list.
    let tiles = top
        .iter()
        .map(|p| {
            let r = &rows[p.index];
            let id = ids.ids.get(&RowKey::App(r.key.clone())).copied().unwrap_or(0);
            TileView { id, name: r.name.clone(), value: metric_text(r.value(metric), metric), weight: p.average as f32, hung: r.hung_for_ms.is_some() }
        })
        .collect();

    let all_procs = snap.processes.iter().map(|p| (p.pid, p.create_time, p.parent_pid)).collect();
    let admin_banner = !elevated && snap.processes.iter().any(|p| p.pid > 4 && p.image_path.is_none());
    Frame {
        meters: meters(snap, history),
        tiles,
        caption: caption(metric).to_string(),
        rows: out_rows,
        targets,
        all_procs,
        admin_banner,
        page: crate::controls::Page::Overview,
        perf: None,
        graph_end_ms: graph::end_of(history, snap.taken_at_ms),
    }
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
    use negotium_model::TopPicker;

    /// build_frame with the tiles a fresh TopPicker picks: the current readings, ranked.
    fn test_frame(ids: &mut IdMap, snap: &Snapshot, rows: &[AppRow], order: &[usize], history: &History, opts: &FrameOptions) -> Frame {
        let top = TopPicker::new().pick(rows, opts.metric, snap.taken_at_ms, TOP_TILES);
        build_frame(ids, snap, rows, order, &top, history, opts)
    }
    use negotium_model::{AppKind, ProcRow};
    use negotium_snapshot::{MemoryTotals, SystemTotals};

    fn proc(pid: u32, parent: u32, cpu: f32, name: &str) -> ProcRow {
        ProcRow { pid, create_time: pid as i64, parent_pid: parent, name: name.into(), cpu_pct: cpu, private_bytes: 1024, disk_bps: 0.0, gpu_pct: None, hung_for_ms: None }
    }
    fn app(pid: u32, name: &str, cpu: f32, procs: Vec<ProcRow>, hung: Option<u64>) -> AppRow {
        AppRow {
            key: AppKey::Window { pid, create_time: pid as i64 }, name: name.into(), kind: AppKind::App,
            cpu_pct: cpu, private_bytes: procs.len() as u64 * 1024, disk_bps: 0.0, gpu_pct: None, net_bps: None,
            hung_for_ms: hung, processes: procs,
        }
    }
    /// Every app's child rows built (as while searching), so tests see the whole tree.
    fn opts(metric: Metric, elevated: bool) -> FrameOptions {
        FrameOptions { metric, elevated, expanded: HashSet::new(), all_children: true, sort: Sort::default(), grouped: true }
    }
    fn snap(cpu: Option<f32>) -> Snapshot {
        Snapshot {
            taken_at_ms: 60_000,
            totals: SystemTotals { logical_cpus: 4, cpu_pct: cpu, memory: Some(MemoryTotals { total: 16 << 30, used: 8 << 30, ..Default::default() }), ..Default::default() },
            ..Default::default()
        }
    }

    /// A worker restart builds a fresh IdMap while the UI still holds the old ids (frozen rows,
    /// the current row, a menu's target): no old id may come to name another app.
    #[test]
    fn a_restarted_id_map_never_gives_an_old_id_to_another_row() {
        let seed = IdSeed::default();
        let s = snap(Some(1.0));
        let before = vec![
            app(1, "Chrome", 30.0, vec![proc(1, 0, 20.0, "chrome.exe"), proc(2, 1, 10.0, "chrome.exe")], None),
            app(3, "Editor", 5.0, vec![proc(3, 0, 5.0, "code.exe")], None),
        ];
        let a = test_frame(&mut IdMap::with_seed(seed.clone()), &s, &before, &[0, 1], &History::new(), &opts(Metric::Cpu, true));
        // Restarted: a fresh map, and other apps rank first now.
        let after = vec![
            app(5, "Mail", 50.0, vec![proc(5, 0, 50.0, "mail.exe"), proc(6, 5, 1.0, "mail.exe")], None),
            app(3, "Editor", 5.0, vec![proc(3, 0, 5.0, "code.exe")], None),
            app(1, "Chrome", 1.0, vec![proc(1, 0, 1.0, "chrome.exe"), proc(2, 1, 0.0, "chrome.exe")], None),
        ];
        let b = test_frame(&mut IdMap::with_seed(seed.clone()), &s, &after, &[0, 1, 2], &History::new(), &opts(Metric::Cpu, true));
        assert!(!a.targets.is_empty() && !b.targets.is_empty());
        for (id, old) in &a.targets {
            if let Some(new) = b.targets.get(id) {
                assert_eq!((&new.name, new.root, &new.procs), (&old.name, old.root, &old.procs), "id {id} now names another row");
            }
        }
    }

    #[test]
    fn gpu_meter_shows_the_busiest_gpus_temperature() {
        use negotium_snapshot::GpuSample;
        let gpu = |key: &str, util: f32, temp: Option<f32>| GpuSample { key: key.into(), util_pct: util, temp_c: temp, ..Default::default() };
        let mut s = snap(Some(1.0));
        s.totals.gpu_pct = Some(80.0);
        s.totals.gpus = Some(vec![gpu("igpu", 5.0, None), gpu("dgpu", 80.0, Some(64.6))]);
        let mut ids = IdMap::new();
        let f = test_frame(&mut ids, &s, &[], &[], &History::new(), &opts(Metric::Cpu, true));
        assert_eq!(f.meters[3].sub, "65 °C");
        s.totals.gpus = Some(vec![gpu("dgpu", 80.0, None)]);
        let f = test_frame(&mut ids, &s, &[], &[], &History::new(), &opts(Metric::Cpu, true));
        assert_eq!(f.meters[3].sub, "", "no sensor: no label");
    }

    #[test]
    fn gpu_meter_temperature_does_not_flip_between_idle_gpus() {
        use negotium_snapshot::GpuSample;
        let gpu = |key: &str, util: f32, temp: f32| GpuSample { key: key.into(), util_pct: util, temp_c: Some(temp), ..Default::default() };
        let mut s = snap(Some(1.0));
        s.totals.gpus = Some(vec![gpu("first", 0.0, 40.0), gpu("second", 0.0, 70.0)]);
        let mut ids = IdMap::new();
        let f = test_frame(&mut ids, &s, &[], &[], &History::new(), &opts(Metric::Cpu, true));
        assert_eq!(f.meters[3].sub, "40 °C", "tie: the first GPU");
        s.totals.gpus = Some(vec![gpu("unknown", f32::NAN, 90.0), gpu("known", 0.0, 55.0)]);
        let f = test_frame(&mut ids, &s, &[], &[], &History::new(), &opts(Metric::Cpu, true));
        assert_eq!(f.meters[3].sub, "55 °C", "unknown utilization never beats a known one");
    }

    #[test]
    fn a_memory_total_of_zero_shows_the_meter_unavailable() {
        let mut s = snap(Some(1.0));
        s.totals.memory = Some(MemoryTotals::default());
        let f = test_frame(&mut IdMap::new(), &s, &[], &[], &History::new(), &opts(Metric::Cpu, true));
        assert!(!f.meters[1].available, "total 0 is unknown, not 100% in use: {:?}", f.meters[1]);
    }

    #[test]
    fn meter_sparklines_come_from_history_by_time() {
        let mut h = History::new();
        let mut s = snap(Some(0.0));
        for (t, cpu) in [(58_000u64, 0.0f32), (59_000, 50.0), (60_000, 100.0)] {
            s.taken_at_ms = t;
            s.totals.cpu_pct = Some(cpu);
            h.push(t, &s.totals, &[]);
        }
        let mut ids = IdMap::new();
        let f = test_frame(&mut ids, &s, &[], &[], &h, &opts(Metric::Cpu, true));
        assert_eq!(f.meters[0].spark, "M 966.7 100.0 L 983.3 50.0 L 1000.0 0.0");
    }

    #[test]
    fn an_idle_network_draws_a_flat_line() {
        let mut h = History::new();
        let mut s = snap(Some(1.0));
        s.totals.net_bps = Some(0.0);
        h.push(60_000, &s.totals, &[]);
        let mut ids = IdMap::new();
        let f = test_frame(&mut ids, &s, &[], &[], &h, &opts(Metric::Cpu, true));
        assert_eq!(f.meters[4].spark, "M 1000.0 100.0 L 999.9 100.0");
    }

    #[test]
    fn frame_has_five_meters_tiles_and_rows_in_order() {
        let rows = vec![
            app(1, "Chrome", 30.0, vec![proc(1, 0, 20.0, "chrome.exe"), proc(2, 1, 10.0, "chrome.exe")], None),
            app(3, "Editor", 5.0, vec![proc(3, 0, 5.0, "code.exe")], Some(14_400)),
        ];
        let mut ids = IdMap::new();
        let f = test_frame(&mut ids, &snap(Some(37.6)), &rows, &[1, 0], &History::new(), &opts(Metric::Cpu, true));
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
    fn windows_group_row_is_not_actionable_and_is_marked_as_a_group() {
        let mut win = app(4, "Windows", 1.0, vec![proc(4, 0, 1.0, "System"), proc(5, 4, 0.0, "svchost.exe")], None);
        win.key = AppKey::Windows;
        win.kind = AppKind::Windows;
        let rows = vec![win, app(9, "Editor", 5.0, vec![proc(9, 0, 5.0, "code.exe")], None)];
        let mut ids = IdMap::new();
        let f = test_frame(&mut ids, &snap(Some(1.0)), &rows, &[0, 1], &History::new(), &opts(Metric::Cpu, true));
        let windows = f.rows.iter().find(|r| r.name == "Windows" && r.depth == 0).unwrap();
        assert!(!windows.actionable);
        assert!(f.targets[&windows.id].group);
        let children: Vec<_> = f.rows.iter().filter(|r| r.parent == windows.id).collect();
        assert_eq!(children.len(), 2);
        for child in children {
            assert!(child.actionable, "individual system processes stay actionable (critical ones are confirmed)");
            assert!(!f.targets[&child.id].group);
        }
        let editor = f.rows.iter().find(|r| r.name == "Editor").unwrap();
        assert!(editor.actionable);
        assert!(!f.targets[&editor.id].group);
    }

    #[test]
    fn a_hung_member_of_the_windows_group_offers_force_close_on_its_own_row() {
        let mut hung_child = proc(6, 4, 0.0, "SearchHost.exe");
        hung_child.hung_for_ms = Some(9_000);
        let mut win = app(4, "Windows", 1.0, vec![proc(5, 4, 1.0, "StartMenuExperienceHost.exe"), hung_child], Some(9_000));
        win.key = AppKey::Windows;
        win.kind = AppKind::Windows;
        let mut s = snap(Some(1.0));
        s.windows = vec![negotium_snapshot::WindowSample { pid: 6, hung: true, title: "Search".into() }];
        let mut ids = IdMap::new();
        let f = test_frame(&mut ids, &s, &[win], &[0], &History::new(), &opts(Metric::Cpu, true));
        let group = f.rows.iter().find(|r| r.depth == 0).unwrap();
        assert_eq!(group.hung, "Not responding · 9s", "the group still shows the roll-up");
        assert!(!group.actionable, "R7: the group itself is never actionable");
        let child = f.rows.iter().find(|r| r.name == "SearchHost.exe").unwrap();
        assert_eq!(child.hung, "Not responding · 9s");
        assert!(child.actionable, "Force close shows on the hung child row");
        assert_eq!(f.targets[&child.id].hung_pid, Some((6, 6)));
        let healthy = f.rows.iter().find(|r| r.name == "StartMenuExperienceHost.exe").unwrap();
        assert_eq!(healthy.hung, "");
    }

    #[test]
    fn child_rows_are_built_only_for_expanded_apps_unless_searching() {
        let rows = vec![
            app(1, "Chrome", 30.0, vec![proc(1, 0, 20.0, "chrome.exe"), proc(2, 1, 10.0, "chrome.exe")], None),
            app(3, "Code", 5.0, vec![proc(3, 0, 5.0, "code.exe"), proc(4, 3, 0.0, "helper.exe")], None),
        ];
        let mut ids = IdMap::new();
        let collapsed = FrameOptions { metric: Metric::Cpu, elevated: true, expanded: HashSet::new(), all_children: false, sort: Sort::default(), grouped: true };
        let f = test_frame(&mut ids, &snap(Some(1.0)), &rows, &[0, 1], &History::new(), &collapsed);
        assert!(f.rows.iter().all(|r| r.depth == 0), "no child rows for collapsed apps");
        assert_eq!(f.targets.len(), 2, "and no child targets");
        let code = f.rows.iter().find(|r| r.name == "Code").unwrap().id;
        let one = FrameOptions { expanded: HashSet::from([code]), ..collapsed.clone() };
        let f = test_frame(&mut ids, &snap(Some(1.0)), &rows, &[0, 1], &History::new(), &one);
        let children: Vec<_> = f.rows.iter().filter(|r| r.depth == 1).collect();
        assert_eq!(children.len(), 2);
        assert!(children.iter().all(|r| r.parent == code && f.targets.contains_key(&r.id)));
        let searching = FrameOptions { all_children: true, ..collapsed };
        let f = test_frame(&mut ids, &snap(Some(1.0)), &rows, &[0, 1], &History::new(), &searching);
        assert_eq!(f.rows.iter().filter(|r| r.depth == 1).count(), 4, "a search can match any process");
    }

    #[test]
    fn a_hung_windows_group_builds_its_children_while_collapsed() {
        let mut hung_child = proc(6, 4, 0.0, "SearchHost.exe");
        hung_child.hung_for_ms = Some(9_000);
        let mut win = app(4, "Windows", 1.0, vec![proc(5, 4, 1.0, "StartMenuExperienceHost.exe"), hung_child], Some(9_000));
        win.key = AppKey::Windows;
        win.kind = AppKind::Windows;
        let collapsed = FrameOptions { metric: Metric::Cpu, elevated: true, expanded: HashSet::new(), all_children: false, sort: Sort::default(), grouped: true };
        let mut ids = IdMap::new();
        let f = test_frame(&mut ids, &snap(Some(1.0)), &[win], &[0], &History::new(), &collapsed);
        let child = f.rows.iter().find(|r| r.name == "SearchHost.exe").expect("built so it can be shown and force-closed");
        assert_eq!(child.hung, "Not responding · 9s");
    }

    #[test]
    fn degenerate_inputs_do_not_divide_by_zero() {
        let rows = vec![app(1, "Idle", 0.0, vec![proc(1, 0, 0.0, "idle.exe")], None)];
        let mut ids = IdMap::new();
        let f = test_frame(&mut ids, &snap(None), &rows, &[0], &History::new(), &opts(Metric::Gpu, false));
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
        let f = test_frame(&mut ids, &s, &[], &[], &History::new(), &opts(Metric::Cpu, true));
        assert!(!f.meters[2].available);
    }

    #[test]
    fn ids_are_stable_across_frames() {
        let rows = vec![app(1, "Chrome", 30.0, vec![proc(1, 0, 30.0, "chrome.exe")], None)];
        let mut ids = IdMap::new();
        let a = test_frame(&mut ids, &snap(Some(1.0)), &rows, &[0], &History::new(), &opts(Metric::Cpu, true));
        let b = test_frame(&mut ids, &snap(Some(1.0)), &rows, &[0], &History::new(), &opts(Metric::Cpu, true));
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

    #[test]
    fn meter_sparklines_reach_both_edges_between_kept_samples() {
        let mut h = History::new();
        let mut s = snap(Some(50.0));
        for t in (0..=60u64).map(|i| i * 1_000) {
            s.taken_at_ms = t;
            h.push(t, &s.totals, &[]);
        }
        // A 0.5 s refresh: this tick is half a second newer than the newest kept sample.
        s.taken_at_ms = 60_500;
        let f = test_frame(&mut IdMap::new(), &s, &[], &[], &h, &opts(Metric::Cpu, true));
        let spark = &f.meters[0].spark;
        assert!(spark.starts_with("M 0.0 50.0"), "{spark}");
        assert!(spark.ends_with("L 1000.0 50.0"), "{spark}");
    }
}
