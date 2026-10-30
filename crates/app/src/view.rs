//! Builds the plain-data Frame the UI renders. Runs on the worker thread; `visible_rows`
//! runs on the UI thread (no Windows calls anywhere here).
use crate::format;
use crate::graph;
use negotium_history::{History, Series};
use negotium_model::{AppKey, AppKind, AppRow, Metric, Pick, ProcRow, Sort, compare_procs};
use negotium_snapshot::Snapshot;
use std::collections::{HashMap, HashSet};

pub const SPARK_SECONDS: u64 = 60;
pub const TOP_TILES: usize = 5;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MeterView { pub label: &'static str, pub value: String, pub sub: String, pub spark: String, pub available: bool }

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TileView { pub id: i32, pub name: String, pub value: String, pub weight: f32, pub hung: bool }

/// What a list row draws as: a section heading, an app's card, or a process row inside an
/// expanded card.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RowKind {
    Section,
    #[default]
    App,
    Proc,
}

/// Section heading ids: fixed, at the top of the range, where `IdSeed` (counting up from 1) never
/// reaches, so the UI can hold a heading's id (current row, collapsed set) like any other.
pub const SECTION_APPS: i32 = i32::MAX - 2;
pub const SECTION_BACKGROUND: i32 = i32::MAX - 1;
pub const SECTION_WINDOWS: i32 = i32::MAX;

/// Row heights by kind, the one source of truth: applist.slint mirrors them (a test keeps them
/// equal), and scrolling to a row sums them. An app's 50 px is its 46 px card and a 4 px gap.
pub const ROW_H_SECTION: f32 = 32.0;
pub const ROW_H_APP: f32 = 50.0;
pub const ROW_H_PROC: f32 = 30.0;

pub fn row_height(kind: RowKind) -> f32 {
    match kind {
        RowKind::Section => ROW_H_SECTION,
        RowKind::App => ROW_H_APP,
        RowKind::Proc => ROW_H_PROC,
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RowView {
    pub id: i32, pub parent: i32, pub depth: i32, pub name: String, pub search_text: String,
    /// "(N)": an app's process count (its accessible label), a section's apps on show.
    pub count: String,
    pub cpu: String, pub mem: String, pub disk: String, pub gpu: String, pub net: String,
    pub hung: String, pub expandable: bool,
    /// False for the "Windows" group row and section headings: never a target of any action.
    pub actionable: bool,
    /// A process of it hosts Windows services: Go to service(s) applies.
    pub hosts_services: bool,
    /// Gone since the list froze: held in place (so no other row shifts under the pointer), inert.
    pub exited: bool,
    pub kind: RowKind,
    /// An app card's second line: its process count, or its hang.
    pub sub: String,
    /// Each column's bar, 0..=1, indexed by `Metric`; all zero on process rows and headings.
    pub bars: [f32; 5],
    /// The program image the row's icon comes from; empty when unknown.
    pub icon: String,
    /// The Windows group: draws the Windows glyph instead of a program icon.
    pub windows: bool,
    /// The last process row of an expanded card: the card closes below it.
    pub last_in_card: bool,
    /// A process row's PID, shown muted after its name; empty on cards and headings.
    pub pid: String,
    /// Optional table columns (shown when enabled in Settings).
    pub col_pid: String,
    pub col_proc_name: String,
    pub col_status: String,
    pub col_threads: String,
    pub col_handles: String,
    pub col_publisher: String,
    pub restrained: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AppHistoryView {
    pub name: String,
    pub icon: String,
    pub running: bool,
    pub cpu_time: String,
    pub cpu_frac: f32,
    pub net_data: String,
    pub net_frac: f32,
    pub disk_data: String,
    pub disk_frac: f32,
    pub peak_mem: String,
    pub mem_frac: f32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Target {
    pub name: String,
    /// The primary process's image name (e.g. `chrome.exe`): what moderator rules and exclusions
    /// match, unlike `name`, which on an app card is the friendly file description.
    pub process_name: String,
    pub root: Option<(u32, i64)>,
    pub procs: Vec<(u32, i64)>,
    pub hung_pid: Option<(u32, i64)>,
    pub image_path: Option<String>,
    pub critical: bool,
    /// The "Windows" group (every session-0 / system process): no action may target it.
    pub group: bool,
    /// One of its processes hosts Windows services (Go to service(s)).
    pub hosts_services: bool,
}

/// Builds the action target for an app row; the Overview and the window picker share it.
pub fn target_of<S: AsRef<str>>(r: &AppRow, hung_pids: &HashSet<u32>, image_of: &HashMap<(u32, i64), S>) -> Target {
    let root = match r.key {
        AppKey::Window { pid, create_time } | AppKey::Background { pid, create_time } => Some((pid, create_time)),
        _ => None,
    };
    let procs: Vec<(u32, i64)> = r.processes.iter().map(|p| (p.pid, p.create_time)).collect();
    let hung_pid = r.processes.iter().find(|p| hung_pids.contains(&p.pid)).map(|p| (p.pid, p.create_time));
    let primary_key = root.or(procs.first().copied());
    let image_path = primary_key.and_then(|k| image_of.get(&k).map(|s| s.as_ref().to_string()));
    let primary_proc = primary_key
        .and_then(|k| r.processes.iter().find(|p| (p.pid, p.create_time) == k))
        .or_else(|| r.processes.first());
    Target {
        name: r.name.clone(),
        process_name: primary_proc.map_or_else(|| r.name.clone(), |p| p.name.clone()),
        root,
        procs,
        hung_pid,
        image_path,
        critical: r.processes.iter().any(|p| negotium_actions::is_critical(&p.name)),
        group: r.key == AppKey::Windows,
        hosts_services: false,
    }
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
    pub history_rows: Vec<AppHistoryView>,
    pub history_elapsed_s: u64,
    pub ports: Option<PortsFrame>,
    /// The open Details popup's live part, on whatever page the frame is for.
    pub details: Option<crate::details::DetailsFrame>,
}

/// The Ports page for one tick: every socket as a row, and the action target of each row that has one.
#[derive(Clone, Debug, Default)]
pub struct PortsFrame {
    pub rows: Vec<crate::ports::PortRow>,
    pub targets: HashMap<i32, Target>,
    /// Every process (pid, create time, parent pid): End process tree needs the descendants.
    pub all_procs: Vec<(u32, i64, u32)>,
    /// Why the sockets could not be read; the rows are then the last list.
    pub error: Option<String>,
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
/// never handed out again: see [`IdSeed`]). Also what else `build_frame` keeps about rows from one
/// frame to the next: the order of each expanded card's processes.
#[derive(Default)]
pub struct IdMap {
    ids: HashMap<RowKey, i32>,
    next: IdSeed,
    procs: ProcOrder,
}

impl IdMap {
    pub fn new() -> Self {
        Self::default()
    }
    /// A map drawing its ids from `seed`, shared with the maps before and after it.
    pub fn with_seed(seed: IdSeed) -> Self {
        Self { ids: HashMap::new(), next: seed, procs: ProcOrder::default() }
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
pub(crate) fn busiest_gpu(gpus: &[negotium_snapshot::GpuSample]) -> Option<&negotium_snapshot::GpuSample> {
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
            sub: t.cpu_temp_c.map(format::temp_c).unwrap_or_default(),
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
    /// The apps were re-sorted for this frame (`Ranker::resorted`): processes inside expanded
    /// cards re-sort with them, and otherwise hold their order (`ProcOrder`).
    pub resorted: bool,
    pub restrained: HashSet<(u32, i64)>,
}

/// The order of the processes inside each expanded card, kept across frames under the Ranker's
/// calm rules: they re-sort only when the apps do (at most every RESORT_INTERVAL_MS, never while
/// frozen, at once on a sort change) or when the card is newly shown; otherwise each keeps its
/// place and new processes go at the end, sorted among themselves, as new apps do while frozen.
#[derive(Default)]
struct ProcOrder {
    /// Each app's process order as last shown, by (pid, create_time).
    held: HashMap<AppKey, Vec<(u32, i64)>>,
    /// This frame's, becoming `held` at its end; apps whose processes were not built drop out, so
    /// a card opened again sorts afresh.
    next: HashMap<AppKey, Vec<(u32, i64)>>,
}

impl ProcOrder {
    fn order<'a>(&mut self, app: &AppKey, procs: &'a [ProcRow], sort: Sort, resort: bool) -> Vec<&'a ProcRow> {
        let mut sorted: Vec<&ProcRow> = procs.iter().collect();
        sorted.sort_by(|a, b| compare_procs(a, b, sort));
        let out = match self.held.get(app) {
            Some(held) if !resort => {
                let at: HashMap<(u32, i64), usize> = held.iter().enumerate().map(|(i, &k)| (k, i)).collect();
                // Held ones in their place; new ones (sorted) after them.
                let (mut kept, fresh): (Vec<&ProcRow>, Vec<&ProcRow>) = sorted.into_iter().partition(|p| at.contains_key(&(p.pid, p.create_time)));
                kept.sort_by_key(|p| at[&(p.pid, p.create_time)]);
                kept.extend(fresh);
                kept
            }
            _ => sorted,
        };
        self.next.insert(app.clone(), out.iter().map(|p| (p.pid, p.create_time)).collect());
        out
    }

    fn finish(&mut self) {
        self.held = std::mem::take(&mut self.next);
    }
}

/// `top` is the top tiles, picked by a TopPicker (negotium_model) that lasts across frames.
pub fn build_frame(
    ids: &mut IdMap, snap: &Snapshot, rows: &[AppRow], order: &[usize], top: &[Pick], history: &History, opts: &FrameOptions,
) -> Frame {
    let (metric, elevated) = (opts.metric, opts.elevated);
    let mut seen = HashSet::new();
    let hung_pids: HashSet<u32> = snap.windows.iter().filter(|w| w.hung).map(|w| w.pid).collect();
    let sample_of: HashMap<(u32, i64), &negotium_snapshot::ProcessSample> = snap
        .processes
        .iter()
        .map(|p| ((p.pid, p.create_time), p))
        .collect();
    let image_of: HashMap<(u32, i64), &str> = snap
        .processes
        .iter()
        .filter_map(|p| p.image_path.as_deref().map(|i| ((p.pid, p.create_time), i)))
        .collect();
    // Bars: CPU and GPU of the whole machine, Memory of physical memory, Disk and Network of the
    // busiest app this frame (they have no meaningful machine total).
    let mem_total = snap.totals.memory.as_ref().map_or(0.0, |m| m.total as f64);
    let busiest = |m: Metric| rows.iter().map(|r| r.value(m)).filter(|v| v.is_finite()).fold(0.0f64, f64::max);
    let (disk_max, net_max) = (busiest(Metric::Disk), busiest(Metric::Network));
    let frac = |v: f64, of: f64| if of > 0.0 && v.is_finite() { (v / of).clamp(0.0, 1.0) as f32 } else { 0.0 };
    let bars = |r: &AppRow| {
        let mut b = [0.0; 5];
        b[Metric::Cpu as usize] = frac(r.cpu_pct as f64, 100.0);
        b[Metric::Memory as usize] = frac(r.private_bytes as f64, mem_total);
        b[Metric::Disk as usize] = frac(r.disk_bps, disk_max);
        b[Metric::Gpu as usize] = frac(r.value(Metric::Gpu), 100.0);
        b[Metric::Network as usize] = frac(r.value(Metric::Network), net_max);
        b
    };

    // Grouped: each type's heading, then its apps in the ranker's order. Empty sections are left
    // out here; `visible_rows` drops the ones a search or a collapse empties.
    enum Entry {
        Section(i32, &'static str),
        App(usize),
    }
    let sequence: Vec<Entry> = if opts.grouped {
        let mut seq = Vec::with_capacity(order.len() + 3);
        for (kind, id, name) in [
            (AppKind::App, SECTION_APPS, "Apps"),
            (AppKind::Background, SECTION_BACKGROUND, "Background processes"),
            (AppKind::Windows, SECTION_WINDOWS, "Windows processes"),
        ] {
            let at = seq.len();
            seq.extend(order.iter().copied().filter(|&i| rows[i].key.kind() == kind).map(Entry::App));
            if seq.len() > at {
                seq.insert(at, Entry::Section(id, name));
            }
        }
        seq
    } else {
        order.iter().copied().map(Entry::App).collect()
    };

    let mut out_rows = Vec::new();
    let mut targets = HashMap::new();
    for entry in sequence {
        let i = match entry {
            Entry::App(i) => i,
            Entry::Section(id, name) => {
                out_rows.push(RowView {
                    id, parent: -1, depth: 0, name: name.into(), kind: RowKind::Section, expandable: true, actionable: false,
                    ..Default::default()
                });
                continue;
            }
        };
        let r = &rows[i];
        let id = ids.id(RowKey::App(r.key.clone()), &mut seen);
        let target = target_of(r, &hung_pids, &image_of);
        let procs = target.procs.clone();
        let image_path = target.image_path.clone();
        let primary_key = target.root.or(procs.first().copied());
        let primary_proc = primary_key
            .and_then(|k| r.processes.iter().find(|p| (p.pid, p.create_time) == k))
            .or_else(|| r.processes.first());
        targets.insert(id, target);
        let col_pid = if r.key == AppKey::Windows {
            "—".into()
        } else {
            primary_proc.map(|p| p.pid.to_string()).unwrap_or_else(|| "—".into())
        };
        let col_proc_name = if r.key == AppKey::Windows {
            "—".into()
        } else {
            primary_proc.map(|p| p.name.clone()).unwrap_or_else(|| "—".into())
        };
        let has_restrained = procs.iter().any(|k| opts.restrained.contains(k));
        let col_status = if r.hung_for_ms.is_some() {
            "Not responding".into()
        } else if has_restrained {
            "Restrained (Aequilibrium)".into()
        } else {
            "Running".into()
        };
        let total_threads: u64 = procs.iter().filter_map(|k| sample_of.get(k)).map(|s| s.threads as u64).sum();
        let total_handles: u64 = procs.iter().filter_map(|k| sample_of.get(k)).map(|s| s.handles as u64).sum();
        let col_threads = if total_threads > 0 { total_threads.to_string() } else { "—".into() };
        let col_handles = if total_handles > 0 { total_handles.to_string() } else { "—".into() };
        let col_publisher = if r.key == AppKey::Windows {
            "Microsoft Corporation".into()
        } else {
            primary_key
                .and_then(|k| sample_of.get(&k))
                .and_then(|s| s.publisher.clone())
                .or_else(|| procs.iter().filter_map(|k| sample_of.get(k).and_then(|s| s.publisher.clone())).next())
                .unwrap_or_else(|| "—".into())
        };
        let search_text = std::iter::once(r.name.to_lowercase())
            .chain(r.processes.iter().map(|p| p.name.to_lowercase()))
            .chain(r.processes.iter().map(|p| p.pid.to_string()))
            .chain(procs.iter().filter_map(|k| sample_of.get(k).and_then(|s| s.publisher.as_ref())).map(|pub_s| pub_s.to_lowercase()))
            .collect::<Vec<_>>()
            .join(" ");
        let hung = r.hung_for_ms.map(format::hung).unwrap_or_default();
        let sub = match r.processes.len() {
            _ if !hung.is_empty() => hung.clone(),
            1 => "1 process".into(),
            n => format!("{n} processes"),
        };
        out_rows.push(RowView {
            id, parent: -1, depth: 0, name: r.name.clone(), search_text,
            count: if r.processes.len() > 1 { format!("({})", r.processes.len()) } else { String::new() },
            cpu: format::pct1(r.cpu_pct), mem: format::bytes(r.private_bytes as f64), disk: format::byte_rate(r.disk_bps),
            gpu: r.gpu_pct.map(format::pct1).unwrap_or_else(|| "—".into()),
            net: r.net_bps.map(format::bit_rate).unwrap_or_else(|| "—".into()),
            hung, expandable: r.processes.len() > 1,
            actionable: r.key != AppKey::Windows,
            hosts_services: false,
            exited: false,
            kind: RowKind::App, sub, bars: bars(r), icon: image_path.unwrap_or_default(),
            windows: r.key == AppKey::Windows, last_in_card: false, pid: String::new(),
            col_pid, col_proc_name, col_status, col_threads, col_handles, col_publisher,
            restrained: has_restrained,
        });
        // Child rows only where the UI can show them: expanded apps, everything while searching, and
        // a Windows group with a hung member (the group row itself is never actionable, so the hung
        // process's own row is the only way to force-close it).
        let children = opts.all_children
            || opts.expanded.contains(&id)
            || (r.key == AppKey::Windows && r.hung_for_ms.is_some());
        if r.processes.len() > 1 && children {
            // Inside the card, processes follow the list's sort too, as calmly as the apps.
            let sorted = ids.procs.order(&r.key, &r.processes, opts.sort, opts.resorted);
            let last = sorted.len() - 1;
            for (n, p) in sorted.into_iter().enumerate() {
                let pid_key = (p.pid, p.create_time);
                let cid = ids.id(RowKey::Proc(p.pid, p.create_time), &mut seen);
                targets.insert(cid, Target {
                    name: p.name.clone(), process_name: p.name.clone(), root: Some(pid_key), procs: vec![pid_key],
                    hung_pid: hung_pids.contains(&p.pid).then_some(pid_key),
                    image_path: image_of.get(&pid_key).map(|s| s.to_string()),
                    critical: negotium_actions::is_critical(&p.name),
                    group: false,
                    hosts_services: false,
                });
                let sample = sample_of.get(&pid_key).copied();
                let proc_pub = sample.and_then(|s| s.publisher.clone());
                let mut proc_search = format!("{} {}", p.name.to_lowercase(), p.pid);
                if let Some(ref pub_s) = proc_pub {
                    proc_search.push(' ');
                    proc_search.push_str(&pub_s.to_lowercase());
                }
                out_rows.push(RowView {
                    id: cid, parent: id, depth: 1, name: p.name.clone(), search_text: proc_search, count: String::new(),
                    cpu: format::pct1(p.cpu_pct), mem: format::bytes(p.private_bytes as f64), disk: format::byte_rate(p.disk_bps),
                    gpu: p.gpu_pct.map(format::pct1).unwrap_or_else(|| "—".into()),
                    net: p.net.map(|n| format::bit_rate(n.total())).unwrap_or_else(|| "—".into()),
                    // Its own hung text, so Force close shows on (and targets) this process even
                    // when its app row is the never-actionable Windows group.
                    hung: p.hung_for_ms.map(format::hung).unwrap_or_default(), expandable: false, actionable: true, hosts_services: false,
                    exited: false, kind: RowKind::Proc, sub: String::new(), bars: [0.0; 5],
                    icon: image_of.get(&pid_key).map(|s| s.to_string()).unwrap_or_default(), windows: false,
                    last_in_card: n == last, pid: p.pid.to_string(),
                    col_pid: p.pid.to_string(),
                    col_proc_name: p.name.clone(),
                    col_status: if p.hung_for_ms.is_some() {
                        "Not responding".into()
                    } else if opts.restrained.contains(&pid_key) {
                        "Restrained (Aequilibrium)".into()
                    } else {
                        "Running".into()
                    },
                    col_threads: sample.filter(|s| s.threads > 0).map(|s| s.threads.to_string()).unwrap_or_else(|| "—".into()),
                    col_handles: sample.filter(|s| s.handles > 0).map(|s| s.handles.to_string()).unwrap_or_else(|| "—".into()),
                    col_publisher: proc_pub.unwrap_or_else(|| "—".into()),
                    restrained: opts.restrained.contains(&pid_key),
                });
            }
        }
    }
    ids.retain(&seen);
    ids.procs.finish();

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
        history_rows: Vec::new(),
        history_elapsed_s: 0,
        ports: None,
        details: None,
    }
}

pub fn build_history_frame(usage: &negotium_model::UsageTracker, sort_col: i32, sort_desc: bool) -> Frame {
    let mut entries = usage.entries();
    entries.sort_by(|a, b| {
        let ord = match sort_col {
            0 => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            2 => a.net_bytes.cmp(&b.net_bytes),
            3 => a.disk_bytes.cmp(&b.disk_bytes),
            4 => a.peak_mem_bytes.cmp(&b.peak_mem_bytes),
            _ => a.cpu_100ns.cmp(&b.cpu_100ns),
        };
        let ord = if sort_desc { ord.reverse() } else { ord };
        ord.then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    let max_cpu = entries.iter().map(|e| e.cpu_100ns).max().unwrap_or(0).max(1) as f64;
    let max_net = entries.iter().map(|e| e.net_bytes).max().unwrap_or(0).max(1) as f64;
    let max_disk = entries.iter().map(|e| e.disk_bytes).max().unwrap_or(0).max(1) as f64;
    let max_mem = entries.iter().map(|e| e.peak_mem_bytes).max().unwrap_or(0).max(1) as f64;
    let history_rows = entries
        .into_iter()
        .map(|e| {
            let cpu_secs = e.cpu_100ns / 10_000_000;
            let cpu_ms = (e.cpu_100ns / 10_000) % 1000;
            let cpu_time = if cpu_secs >= 3600 {
                format!("{}:{:02}:{:02}", cpu_secs / 3600, (cpu_secs / 60) % 60, cpu_secs % 60)
            } else if cpu_secs >= 60 {
                format!("{}:{:02}", cpu_secs / 60, cpu_secs % 60)
            } else if cpu_secs > 0 || cpu_ms >= 100 {
                format!("{}.{} s", cpu_secs, cpu_ms / 100)
            } else {
                "0 s".into()
            };
            AppHistoryView {
                name: e.name,
                icon: e.icon_path.unwrap_or_default(),
                running: e.running,
                cpu_time,
                cpu_frac: (e.cpu_100ns as f64 / max_cpu).clamp(0.0, 1.0) as f32,
                net_data: format::bytes(e.net_bytes as f64),
                net_frac: (e.net_bytes as f64 / max_net).clamp(0.0, 1.0) as f32,
                disk_data: format::bytes(e.disk_bytes as f64),
                disk_frac: (e.disk_bytes as f64 / max_disk).clamp(0.0, 1.0) as f32,
                peak_mem: format::bytes(e.peak_mem_bytes as f64),
                mem_frac: (e.peak_mem_bytes as f64 / max_mem).clamp(0.0, 1.0) as f32,
            }
        })
        .collect();
    Frame {
        page: crate::controls::Page::AppHistory,
        history_rows,
        history_elapsed_s: usage.elapsed_s_now(),
        ..Default::default()
    }
}

/// The rows the list shows: apps matching `search` (by name or a process's name), the processes
/// of expanded apps (and, while searching, the matching ones), each under its section heading.
/// A heading counts the apps it lets through, shows just itself while `collapsed` holds its id,
/// and goes when no app of its matches. The last process shown in a card closes it (a search may
/// hide the one built last).
pub fn visible_rows(rows: &[RowView], expanded: &HashSet<i32>, collapsed: &HashSet<i32>, search: &str) -> Vec<RowView> {
    let needle = search.trim().to_lowercase();
    let matches = |r: &RowView| needle.is_empty() || r.search_text.contains(&needle);
    // Apps that match, and how many under each heading; open ones show their cards.
    let mut counts: HashMap<i32, usize> = HashMap::new();
    let mut shown_apps = HashSet::new();
    let mut section = None;
    for r in rows {
        if r.kind == RowKind::Section {
            section = Some(r.id);
        } else if r.depth == 0 && matches(r) {
            if let Some(s) = section {
                *counts.entry(s).or_default() += 1;
            }
            if !section.is_some_and(|s| collapsed.contains(&s)) {
                shown_apps.insert(r.id);
            }
        }
    }
    let mut out: Vec<RowView> = rows.iter()
        .filter_map(|r| {
            if r.kind == RowKind::Section {
                let n = counts.get(&r.id).copied().unwrap_or(0);
                let count = if n == 0 || r.id == SECTION_WINDOWS { String::new() } else { format!("({n})") };
                return (n > 0).then(|| RowView { count, ..r.clone() });
            }
            let show = if r.depth == 0 {
                shown_apps.contains(&r.id)
            } else {
                shown_apps.contains(&r.parent) && (expanded.contains(&r.parent) || (!needle.is_empty() && r.search_text.contains(&needle)))
            };
            show.then(|| r.clone())
        })
        .collect();
    close_cards(&mut out);
    out
}

/// Marks the last process row of each card in `rows` as shown, and only that one: the card closes
/// below it. Worked out from the order on screen, which a search or a frozen list (rows held in
/// place, an exited one among them) makes differ from the order a frame was built in.
pub fn close_cards(rows: &mut [RowView]) {
    for i in 0..rows.len() {
        if rows[i].kind == RowKind::Proc {
            rows[i].last_in_card = rows.get(i + 1).is_none_or(|n| n.depth == 0 || n.parent != rows[i].parent);
        }
    }
}

/// Marks every target, and its row, whose processes include a service host (Go to service(s)).
pub fn mark_service_hosts(frame: &mut Frame, service_pids: &HashSet<u32>) {
    for target in frame.targets.values_mut() {
        target.hosts_services = target.procs.iter().any(|(pid, _)| service_pids.contains(pid));
    }
    for row in &mut frame.rows {
        row.hosts_services = frame.targets.get(&row.id).is_some_and(|t| t.hosts_services);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_whose_processes_host_services_are_marked() {
        let mut frame = Frame {
            rows: vec![RowView { id: 1, ..Default::default() }, RowView { id: 2, ..Default::default() }],
            targets: HashMap::from([
                (1, Target { procs: vec![(10, 1), (11, 1)], ..Default::default() }),
                (2, Target { procs: vec![(20, 1)], ..Default::default() }),
            ]),
            ..Default::default()
        };
        mark_service_hosts(&mut frame, &HashSet::from([11]));
        assert!(frame.targets[&1].hosts_services && !frame.targets[&2].hosts_services);
        assert!(frame.rows[0].hosts_services && !frame.rows[1].hosts_services);
    }
    use negotium_model::TopPicker;

    /// build_frame with the tiles a fresh TopPicker picks: the current readings, ranked.
    fn test_frame(ids: &mut IdMap, snap: &Snapshot, rows: &[AppRow], order: &[usize], history: &History, opts: &FrameOptions) -> Frame {
        let top = TopPicker::new().pick(rows, opts.metric, snap.taken_at_ms, TOP_TILES);
        build_frame(ids, snap, rows, order, &top, history, opts)
    }
    use negotium_model::{AppKind, ProcRow};
    use negotium_snapshot::{MemoryTotals, NetRate, SystemTotals};

    fn proc(pid: u32, parent: u32, cpu: f32, name: &str) -> ProcRow {
        ProcRow { pid, create_time: pid as i64, parent_pid: parent, name: name.into(), cpu_pct: cpu, private_bytes: 1024, disk_bps: 0.0, gpu_pct: None, net: None, hung_for_ms: None }
    }
    fn app(pid: u32, name: &str, cpu: f32, procs: Vec<ProcRow>, hung: Option<u64>) -> AppRow {
        AppRow {
            key: AppKey::Window { pid, create_time: pid as i64 }, name: name.into(), kind: AppKind::App,
            cpu_pct: cpu, private_bytes: procs.len() as u64 * 1024, disk_bps: 0.0, gpu_pct: None, net_bps: None, net: None,
            hung_for_ms: hung, processes: procs,
        }
    }
    /// Every app's child rows built (as while searching), so tests see the whole tree.
    fn opts(metric: Metric, elevated: bool) -> FrameOptions {
        FrameOptions { metric, elevated, expanded: HashSet::new(), all_children: true, sort: Sort::default(), grouped: true, resorted: true, restrained: HashSet::new() }
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
        let names: Vec<_> = f.rows.iter().filter(|r| r.kind == RowKind::App).map(|r| r.name.clone()).collect();
        assert_eq!(names, ["Editor", "Chrome"], "rows follow the ranker order");
        let editor = f.rows.iter().find(|r| r.name == "Editor").unwrap();
        assert_eq!(editor.hung, "Not responding · 14s");
        assert_eq!(f.targets[&editor.id].root, Some((3, 3)));
        let chrome = f.rows.iter().find(|r| r.name == "Chrome").unwrap();
        assert_eq!(chrome.count, "(2)");
        assert!(chrome.expandable);
        assert!((chrome.bars[Metric::Cpu as usize] - 0.3).abs() < 1e-6, "30% of the machine");
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
        let group = f.rows.iter().find(|r| r.kind == RowKind::App).unwrap();
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
        let collapsed = FrameOptions { metric: Metric::Cpu, elevated: true, expanded: HashSet::new(), all_children: false, sort: Sort::default(), grouped: true, resorted: true, restrained: HashSet::new() };
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
        let collapsed = FrameOptions { metric: Metric::Cpu, elevated: true, expanded: HashSet::new(), all_children: false, sort: Sort::default(), grouped: true, resorted: true, restrained: HashSet::new() };
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
        // Disk and Network against a busiest app of 0: empty, not NaN. Memory has a total.
        assert!(f.rows.iter().all(|r| r.bars.iter().all(|b| b.is_finite()) && r.bars[Metric::Disk as usize] == 0.0 && r.bars[Metric::Network as usize] == 0.0));
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
        let chrome = |f: &Frame| f.rows.iter().find(|r| r.name == "Chrome").unwrap().id;
        assert_eq!(chrome(&a), chrome(&b));
    }

    #[test]
    fn visible_rows_expand_and_search() {
        let rows = vec![
            RowView { id: 1, parent: -1, depth: 0, name: "Chrome".into(), search_text: "chrome chrome.exe gpu-helper.exe".into(), expandable: true, ..Default::default() },
            RowView { id: 2, parent: 1, depth: 1, name: "gpu-helper.exe".into(), search_text: "gpu-helper.exe".into(), ..Default::default() },
            RowView { id: 3, parent: -1, depth: 0, name: "Editor".into(), search_text: "editor code.exe".into(), ..Default::default() },
        ];
        let none = HashSet::new();
        assert_eq!(visible_rows(&rows, &none, &none, "").iter().map(|r| r.id).collect::<Vec<_>>(), [1, 3]);
        assert_eq!(visible_rows(&rows, &HashSet::from([1]), &none, "").iter().map(|r| r.id).collect::<Vec<_>>(), [1, 2, 3]);
        assert_eq!(visible_rows(&rows, &none, &none, "HELPER").iter().map(|r| r.id).collect::<Vec<_>>(), [1, 2]);
        assert!(visible_rows(&rows, &none, &none, "zzz").is_empty());
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

    fn bg(pid: u32, name: &str, cpu: f32, procs: Vec<ProcRow>) -> AppRow {
        AppRow { key: AppKey::Background { pid, create_time: pid as i64 }, kind: AppKind::Background, ..app(pid, name, cpu, procs, None) }
    }
    fn windows_group(procs: Vec<ProcRow>) -> AppRow {
        AppRow { key: AppKey::Windows, kind: AppKind::Windows, ..app(4, "Windows", 1.0, procs, None) }
    }
    fn sample(pid: u32, image: &str) -> negotium_snapshot::ProcessSample {
        negotium_snapshot::ProcessSample {
            pid, parent_pid: 0, create_time: pid as i64, name: String::new(), session_id: 1, cpu_time_100ns: 0,
            private_working_set: 0, disk_io_bytes: 0, threads: 1, handles: 1, image_path: Some(image.into()),
            package_family: None, description: None, publisher: None,
        }
    }
    fn names(rows: &[RowView]) -> Vec<(RowKind, &str)> {
        rows.iter().map(|r| (r.kind, r.name.as_str())).collect()
    }
    /// Editor, Mail (apps), Updater (background) and the Windows group, ranked Windows, Mail,
    /// Updater, Editor.
    fn mixed() -> (Vec<AppRow>, Vec<usize>) {
        let rows = vec![
            app(1, "Editor", 1.0, vec![proc(1, 0, 1.0, "code.exe"), proc(2, 1, 0.0, "helper.exe")], None),
            app(3, "Mail", 2.0, vec![proc(3, 0, 2.0, "mail.exe")], None),
            bg(5, "Updater", 3.0, vec![proc(5, 0, 3.0, "updater.exe")]),
            windows_group(vec![proc(4, 0, 1.0, "System"), proc(6, 4, 0.0, "svchost.exe")]),
        ];
        (rows, vec![3, 1, 2, 0])
    }

    #[test]
    fn grouped_frames_put_apps_under_their_section_in_sort_order() {
        use RowKind::{App, Section};
        let (rows, order) = mixed();
        let flat = FrameOptions { all_children: false, ..opts(Metric::Cpu, true) };
        let f = test_frame(&mut IdMap::new(), &snap(Some(1.0)), &rows, &order, &History::new(), &flat);
        assert_eq!(names(&f.rows), [
            (Section, "Apps"), (App, "Mail"), (App, "Editor"),
            (Section, "Background processes"), (App, "Updater"),
            (Section, "Windows processes"), (App, "Windows"),
        ]);
        let ids: Vec<i32> = f.rows.iter().filter(|r| r.kind == Section).map(|r| r.id).collect();
        assert_eq!(ids, [SECTION_APPS, SECTION_BACKGROUND, SECTION_WINDOWS]);
        for s in f.rows.iter().filter(|r| r.kind == Section) {
            assert!(s.depth == 0 && !s.actionable && s.expandable, "{s:?}");
        }
        assert!(f.rows.iter().find(|r| r.name == "Windows").unwrap().windows, "the Windows group draws the Windows glyph");
        assert!(f.rows.iter().filter(|r| r.name != "Windows").all(|r| !r.windows));
        let ungrouped = FrameOptions { grouped: false, ..flat };
        let f = test_frame(&mut IdMap::new(), &snap(Some(1.0)), &rows, &order, &History::new(), &ungrouped);
        assert_eq!(names(&f.rows), [(App, "Windows"), (App, "Mail"), (App, "Updater"), (App, "Editor")], "no sections, the ranker's order");
    }

    #[test]
    fn search_hides_empty_sections_and_counts_matches() {
        let (rows, order) = mixed();
        let f = test_frame(&mut IdMap::new(), &snap(Some(1.0)), &rows, &order, &History::new(), &opts(Metric::Cpu, true));
        let none = HashSet::new();
        let shown = visible_rows(&f.rows, &none, &none, "UPDAT");
        assert_eq!(names(&shown), [(RowKind::Section, "Background processes"), (RowKind::App, "Updater")]);
        assert_eq!(shown[0].count, "(1)");
        let all = visible_rows(&f.rows, &none, &none, "");
        let count = |id: i32| all.iter().find(|r| r.id == id).unwrap().count.clone();
        assert_eq!((count(SECTION_APPS), count(SECTION_BACKGROUND), count(SECTION_WINDOWS)), ("(2)".into(), "(1)".into(), String::new()));
        // A process name matches too: its app's section and card come with it.
        let shown = visible_rows(&f.rows, &none, &none, "svchost");
        assert_eq!(names(&shown), [(RowKind::Section, "Windows processes"), (RowKind::App, "Windows"), (RowKind::Proc, "svchost.exe")]);
        let shown = visible_rows(&f.rows, &none, &none, "system");
        assert_eq!(names(&shown)[2], (RowKind::Proc, "System"));
        assert!(shown[2].last_in_card, "the only process shown closes the card, though built first");
        assert!(visible_rows(&f.rows, &none, &none, "zzz").is_empty(), "no section heading without apps");
    }

    #[test]
    fn a_collapsed_section_shows_its_heading_only() {
        use RowKind::{App, Proc, Section};
        let (rows, order) = mixed();
        let f = test_frame(&mut IdMap::new(), &snap(Some(1.0)), &rows, &order, &History::new(), &opts(Metric::Cpu, true));
        let editor = f.rows.iter().find(|r| r.name == "Editor").unwrap().id;
        let expanded = HashSet::from([editor]);
        let open = visible_rows(&f.rows, &expanded, &HashSet::new(), "");
        assert_eq!(names(&open)[..5], [(Section, "Apps"), (App, "Mail"), (App, "Editor"), (Proc, "code.exe"), (Proc, "helper.exe")]);
        let shown = visible_rows(&f.rows, &expanded, &HashSet::from([SECTION_APPS]), "");
        assert_eq!(names(&shown), [
            (Section, "Apps"), (Section, "Background processes"), (App, "Updater"), (Section, "Windows processes"), (App, "Windows"),
        ]);
        assert_eq!(shown[0].count, "(2)", "the heading still counts its apps");
    }

    #[test]
    fn network_cells_show_the_rate_or_a_dash() {
        let mut with = proc(2, 1, 0.0, "a2.exe");
        with.net = Some(NetRate { sent_bps: 1000.0, recv_bps: 3000.0 });
        let mut idle = proc(3, 1, 0.0, "a3.exe");
        idle.net = Some(NetRate::default());
        let mut a = app(1, "A", 1.0, vec![with, idle, proc(4, 1, 0.0, "a4.exe")], None);
        a.net_bps = Some(4000.0);
        let b = app(5, "B", 1.0, vec![proc(5, 0, 0.0, "b.exe")], None);
        let f = test_frame(&mut IdMap::new(), &snap(Some(1.0)), &[a, b], &[0, 1], &History::new(), &opts(Metric::Cpu, true));
        let net = |name: &str| f.rows.iter().find(|r| r.name == name).unwrap().net.to_string();
        assert_eq!(net("A"), format::bit_rate(4000.0));
        assert_eq!(net("a2.exe"), format::bit_rate(4000.0));
        assert_eq!(net("a3.exe"), "0 b/s");
        assert_eq!(net("a4.exe"), "—", "no feed for that process: a dash");
        assert_eq!(net("B"), "—");
    }

    #[test]
    fn bars_follow_the_spec() {
        const GB: u64 = 1 << 30;
        let mut a = app(1, "A", 12.7, vec![proc(1, 0, 12.7, "a.exe"), proc(2, 1, 0.0, "a2.exe")], None);
        (a.private_bytes, a.disk_bps, a.gpu_pct, a.net_bps) = (6 * GB, 50.0, Some(40.0), Some(1000.0));
        let mut b = app(3, "B", f32::NAN, vec![proc(3, 0, 0.0, "b.exe")], None);
        (b.disk_bps, b.net_bps) = (200.0, None);
        let mut c = app(4, "C", 150.0, vec![proc(4, 0, 150.0, "c.exe")], None);
        c.private_bytes = 200 * GB;
        let rows = vec![a, b, c];
        let mut s = snap(Some(1.0));
        s.totals.memory = Some(MemoryTotals { total: 96 * GB, used: 8 * GB, ..Default::default() });
        let flat = FrameOptions { grouped: false, ..opts(Metric::Cpu, true) };
        let f = test_frame(&mut IdMap::new(), &s, &rows, &[0, 1, 2], &History::new(), &flat);
        let bars = |name: &str| f.rows.iter().find(|r| r.name == name).unwrap().bars;
        let close = |got: [f32; 5], want: [f32; 5]| got.iter().zip(want).all(|(g, w)| (g - w).abs() < 1e-6);
        // Indexed by Metric: CPU, Memory, Disk, GPU, Network.
        assert!(close(bars("A"), [0.127, 0.0625, 0.25, 0.4, 1.0]), "{:?}", bars("A"));
        assert!(close(bars("B"), [0.0, 1024.0 / (96 * GB) as f32, 1.0, 0.0, 0.0]), "NaN and none are 0: {:?}", bars("B"));
        assert!(close(bars("C"), [1.0, 1.0, 0.0, 0.0, 0.0]), "never past the track: {:?}", bars("C"));
        assert!(f.rows.iter().filter(|r| r.kind == RowKind::Proc).all(|r| r.bars == [0.0; 5]), "process rows have no bars");
        s.totals.memory = None;
        let f = test_frame(&mut IdMap::new(), &s, &rows, &[0, 1, 2], &History::new(), &flat);
        assert!(f.rows.iter().all(|r| r.bars[Metric::Memory as usize] == 0.0), "no memory total: no memory bar");
    }

    #[test]
    fn an_apps_sub_line_is_its_process_count_or_its_hang() {
        let many: Vec<ProcRow> = (10..141).map(|pid| proc(pid, 10, 0.0, "chrome.exe")).collect();
        let rows = vec![
            app(10, "Chrome", 1.0, many, None),
            app(3, "Editor", 1.0, vec![proc(3, 0, 1.0, "code.exe")], None),
            app(5, "Frozen", 1.0, vec![proc(5, 0, 1.0, "frozen.exe")], Some(14_400)),
        ];
        let f = test_frame(&mut IdMap::new(), &snap(Some(1.0)), &rows, &[0, 1, 2], &History::new(), &opts(Metric::Cpu, true));
        let app_row = |name: &str| f.rows.iter().find(|r| r.name == name && r.kind == RowKind::App).unwrap().clone();
        assert_eq!(app_row("Chrome").sub, "131 processes");
        assert_eq!(app_row("Chrome").count, "(131)", "kept for the accessible label");
        assert_eq!(app_row("Editor").sub, "1 process");
        assert_eq!(app_row("Frozen").sub, "Not responding · 14s");
    }

    #[test]
    fn process_rows_carry_their_pid_and_cards_and_headings_none() {
        let rows = vec![app(10, "Chrome", 1.0, vec![proc(10, 0, 1.0, "chrome.exe"), proc(4242, 10, 0.0, "helper.exe")], None)];
        let f = test_frame(&mut IdMap::new(), &snap(Some(1.0)), &rows, &[0], &History::new(), &opts(Metric::Cpu, true));
        let pids = |kind: RowKind| f.rows.iter().filter(|r| r.kind == kind).map(|r| r.pid.clone()).collect::<Vec<_>>();
        let mut procs = pids(RowKind::Proc);
        procs.sort();
        assert_eq!(procs, ["10", "4242"]);
        assert!(pids(RowKind::App).iter().chain(&pids(RowKind::Section)).all(String::is_empty), "only process rows show a PID");
    }

    #[test]
    fn process_rows_follow_the_sort_and_the_last_closes_the_card() {
        use negotium_model::SortColumn;
        let rows = vec![app(1, "Chrome", 35.0, vec![proc(1, 0, 5.0, "b.exe"), proc(2, 1, 20.0, "c.exe"), proc(3, 1, 10.0, "a.exe")], None)];
        let procs = |sort: Sort| {
            let o = FrameOptions { sort, ..opts(Metric::Cpu, true) };
            let f = test_frame(&mut IdMap::new(), &snap(Some(1.0)), &rows, &[0], &History::new(), &o);
            f.rows.iter().filter(|r| r.kind == RowKind::Proc).map(|r| (r.name.clone(), r.last_in_card)).collect::<Vec<_>>()
        };
        let p = |n: &str, last: bool| (n.to_string(), last);
        assert_eq!(procs(Sort::default()), [p("c.exe", false), p("a.exe", false), p("b.exe", true)], "CPU, high to low");
        assert_eq!(procs(Sort::by(SortColumn::Name)), [p("a.exe", false), p("b.exe", false), p("c.exe", true)], "Name, A to Z");
        let up = Sort { descending: false, ..Sort::default() };
        assert_eq!(procs(up), [p("b.exe", false), p("a.exe", false), p("c.exe", true)], "CPU, low to high");
    }

    /// An expanded card's processes keep the Ranker's calm rules: between re-sorts each keeps its
    /// place and a new one goes at the end; a re-sort (due, or a sort change) puts them in order.
    #[test]
    fn process_rows_hold_their_order_between_resorts() {
        use negotium_model::SortColumn;
        let chrome = |procs: Vec<ProcRow>| vec![app(1, "Chrome", 35.0, procs, None)];
        let mut ids = IdMap::new();
        let mut procs = |rows: &[AppRow], sort: Sort, resorted: bool| {
            let o = FrameOptions { sort, resorted, ..opts(Metric::Cpu, true) };
            let top = TopPicker::new().pick(rows, o.metric, 0, TOP_TILES);
            let f = build_frame(&mut ids, &snap(Some(1.0)), rows, &[0], &top, &History::new(), &o);
            f.rows.iter().filter(|r| r.kind == RowKind::Proc).map(|r| r.name.clone()).collect::<Vec<_>>()
        };
        let first = chrome(vec![proc(1, 0, 5.0, "b.exe"), proc(2, 1, 20.0, "c.exe"), proc(3, 1, 10.0, "a.exe")]);
        assert_eq!(procs(&first, Sort::default(), true), ["c.exe", "a.exe", "b.exe"]);
        // b now leads and d is new, but the apps were not re-sorted: the order holds, d goes last.
        let busy = chrome(vec![proc(1, 0, 50.0, "b.exe"), proc(2, 1, 1.0, "c.exe"), proc(3, 1, 10.0, "a.exe"), proc(4, 1, 99.0, "d.exe")]);
        assert_eq!(procs(&busy, Sort::default(), false), ["c.exe", "a.exe", "b.exe", "d.exe"], "held between re-sorts");
        assert_eq!(procs(&busy, Sort::default(), true), ["d.exe", "b.exe", "a.exe", "c.exe"], "a re-sort orders them");
        assert_eq!(procs(&busy, Sort::by(SortColumn::Name), true), ["a.exe", "b.exe", "c.exe", "d.exe"], "a sort change orders them at once");
    }

    #[test]
    fn a_card_opened_again_sorts_its_processes_afresh() {
        let rows = vec![app(1, "Chrome", 35.0, vec![proc(1, 0, 5.0, "b.exe"), proc(2, 1, 20.0, "c.exe")], None)];
        let mut ids = IdMap::new();
        let mut frame = |rows: &[AppRow], all_children: bool| {
            let o = FrameOptions { all_children, resorted: false, ..opts(Metric::Cpu, true) };
            let f = build_frame(&mut ids, &snap(Some(1.0)), rows, &[0], &[], &History::new(), &o);
            f.rows.iter().filter(|r| r.kind == RowKind::Proc).map(|r| r.name.clone()).collect::<Vec<_>>()
        };
        assert_eq!(frame(&rows, true), ["c.exe", "b.exe"], "newly shown: sorted, though the apps were not re-sorted");
        assert!(frame(&rows, false).is_empty(), "closed");
        let swapped = vec![app(1, "Chrome", 35.0, vec![proc(1, 0, 50.0, "b.exe"), proc(2, 1, 20.0, "c.exe")], None)];
        assert_eq!(frame(&swapped, true), ["b.exe", "c.exe"], "opened again: sorted afresh");
    }

    #[test]
    fn icons_are_the_apps_root_image_and_each_processes_own() {
        let rows = vec![
            app(1, "Chrome", 1.0, vec![proc(2, 1, 0.0, "helper.exe"), proc(1, 0, 1.0, "chrome.exe")], None),
            bg(7, "Unknown", 1.0, vec![proc(7, 0, 1.0, "x.exe")]),
        ];
        let mut s = snap(Some(1.0));
        s.processes = vec![sample(1, r"C:\chrome.exe"), sample(2, r"C:\helper.exe")];
        let f = test_frame(&mut IdMap::new(), &s, &rows, &[0, 1], &History::new(), &opts(Metric::Cpu, true));
        let icon = |name: &str| f.rows.iter().find(|r| r.name == name).unwrap().icon.clone();
        assert_eq!(icon("Chrome"), r"C:\chrome.exe", "the root process, not the first");
        assert_eq!(icon("helper.exe"), r"C:\helper.exe");
        assert_eq!(icon("Unknown"), "", "no image path: no icon");
    }

    #[test]
    fn a_frozen_list_keeps_section_rows_in_place() {
        use crate::bridge::RowLayout;
        let (rows, order) = mixed();
        let flat = FrameOptions { all_children: false, ..opts(Metric::Cpu, true) };
        let mut ids = IdMap::new();
        let none = HashSet::new();
        let a = test_frame(&mut ids, &snap(Some(1.0)), &rows, &order, &History::new(), &flat);
        let mut layout = RowLayout::default();
        layout.on_frame(&a.rows, visible_rows(&a.rows, &none, &none, ""));
        layout.set_frozen(true, &a.rows, visible_rows(&a.rows, &none, &none, ""));
        let before: Vec<i32> = layout.rows().iter().map(|r| r.id).collect();
        // Mail (in Apps) exits.
        let rest: Vec<AppRow> = rows.iter().filter(|r| r.name != "Mail").cloned().collect();
        let b = test_frame(&mut ids, &snap(Some(1.0)), &rest, &[2, 1, 0], &History::new(), &flat);
        layout.on_frame(&b.rows, visible_rows(&b.rows, &none, &none, ""));
        let shown = layout.rows();
        assert_eq!(shown.iter().map(|r| r.id).collect::<Vec<_>>(), before, "every index keeps its row");
        let mail = shown.iter().find(|r| r.name == "Mail").unwrap();
        assert!(mail.exited && !mail.actionable && mail.kind == RowKind::App, "the exited card stays in place, inert");
        let headings: Vec<_> = shown.iter().filter(|r| r.kind == RowKind::Section).map(|r| (r.name.as_str(), r.exited)).collect();
        assert_eq!(headings, [("Apps", false), ("Background processes", false), ("Windows processes", false)]);
        assert_eq!(shown[0].count, "(1)", "the heading counts what is live");
    }

    #[test]
    fn row_heights_are_one_per_kind() {
        assert_eq!([RowKind::Section, RowKind::App, RowKind::Proc].map(row_height), [ROW_H_SECTION, ROW_H_APP, ROW_H_PROC]);
        assert_eq!((ROW_H_SECTION, ROW_H_APP, ROW_H_PROC), (32.0, 50.0, 30.0));
    }

    /// The Slint list mirrors the Rust row heights (ensure-visible and the menu anchor sum them).
    #[test]
    fn row_heights_match_the_list() {
        let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/ui/applist.slint")).unwrap();
        for (name, h) in [("section-height", ROW_H_SECTION), ("app-height", ROW_H_APP), ("proc-height", ROW_H_PROC)] {
            let line = src.lines().find(|l| l.contains(&format!("{name}:"))).unwrap_or_else(|| panic!("applist.slint declares no {name}"));
            assert!(line.contains(&format!("{h}px")), "{name} must be {h}px: {line}");
        }
    }
}
