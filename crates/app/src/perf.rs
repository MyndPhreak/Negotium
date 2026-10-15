//! Performance page view model (spec §5.2): the device list and the selected device's page.
//! Pure; built on the worker thread from the snapshot, the history and SystemInfo. Only the
//! selected device's page is built each tick (spec §8).
use crate::format;
use crate::graph::{self, EMPTY_PATH, Shape};
use negotium_collector::SystemInfo;
use negotium_history::{History, Series};
use negotium_model::Metric;
use negotium_snapshot::{MemoryTotals, Snapshot};

pub const MAIN_BUCKETS: usize = 300;
pub const CELL_BUCKETS: usize = 60;
pub const SPARK_MS: u64 = 60_000;
pub const CPU_TEMP_NOTE: &str = "CPU temperature needs the CPU temperature add-on, coming in a later version.";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Range {
    #[default]
    Minute,
    TenMinutes,
    Hour,
}

impl Range {
    pub const ALL: [Range; 3] = [Range::Minute, Range::TenMinutes, Range::Hour];

    pub fn span_ms(self) -> u64 {
        match self {
            Range::Minute => 60_000,
            Range::TenMinutes => 600_000,
            Range::Hour => 3_600_000,
        }
    }

    pub fn from_index(i: i32) -> Range {
        Self::ALL[(i.max(0) as usize).min(Self::ALL.len() - 1)]
    }

    pub fn index(self) -> i32 {
        self as i32
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Device {
    Cpu,
    Memory,
}

impl Device {
    pub fn id(&self) -> String {
        match self {
            Device::Cpu => "cpu".to_string(),
            Device::Memory => "memory".to_string(),
        }
    }

    pub fn parse(id: &str) -> Option<Device> {
        match id {
            "cpu" => Some(Device::Cpu),
            "memory" => Some(Device::Memory),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DeviceView {
    pub id: String,
    pub title: String,
    pub value: String,
    pub spark: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GraphView {
    pub label: String,
    pub value: String,
    /// Value at the top edge ("100%", "16.0 GB", "2.0 MB/s").
    pub scale: String,
    pub line: String,
    pub area: String,
    /// A second series on the same scale (write, send), or EMPTY_PATH.
    pub line2: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct StatView {
    pub label: String,
    pub value: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SegmentView {
    pub label: String,
    pub value: String,
    pub fraction: f32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PerfFrame {
    pub devices: Vec<DeviceView>,
    /// The device id this frame was asked for; the UI drops a frame asked for another one.
    pub requested: String,
    /// The device shown: `requested`, or "cpu" when that device is gone.
    pub selected: String,
    pub title: String,
    pub subtitle: String,
    pub note: String,
    pub graphs: Vec<GraphView>,
    pub cells: Vec<GraphView>,
    pub segments: Vec<SegmentView>,
    pub stats: Vec<StatView>,
    pub range: Range,
    /// Graph window `[window_end_ms - span_ms, window_end_ms]` in collector time, for the scrub.
    pub window_end_ms: u64,
    pub span_ms: u64,
    /// Local wall-clock time of `window_end_ms`, ms since midnight.
    pub end_clock_ms: u64,
    /// Which app ranking the scrub shows on this page (ruling P3-4).
    pub scrub_metric: Metric,
}

pub struct PerfInput<'a> {
    pub snap: &'a Snapshot,
    pub history: &'a History,
    pub info: &'a SystemInfo,
    pub device: &'a str,
    pub range: Range,
}

impl PerfInput<'_> {
    fn end(&self) -> u64 {
        self.snap.taken_at_ms
    }

    fn shape(&self, key: &Series, max: f32, buckets: usize) -> Shape {
        let span = self.range.span_ms();
        graph::shape(self.history.points(key, self.end().saturating_sub(span)), self.end(), span, max, buckets)
    }

    fn spark(&self, key: &Series, max: f32) -> String {
        graph::line(self.history.points(key, self.end().saturating_sub(SPARK_MS)), self.end(), SPARK_MS, max, 60)
    }
}

fn graph_view(label: &str, value: String, scale: String, shape: Shape) -> GraphView {
    GraphView { label: label.to_string(), value, scale, line: shape.line, area: shape.area, line2: EMPTY_PATH.to_string() }
}

fn stat(label: &str, value: impl Into<String>) -> StatView {
    StatView { label: label.to_string(), value: value.into() }
}

fn dash() -> String {
    "—".to_string()
}

/// Every device with a page, in display order.
pub fn devices(_snap: &Snapshot, _info: &SystemInfo) -> Vec<Device> {
    vec![Device::Cpu, Device::Memory]
}

fn device_view(d: &Device, input: &PerfInput) -> DeviceView {
    let t = &input.snap.totals;
    let (title, value, spark) = match d {
        Device::Cpu => ("CPU".to_string(), t.cpu_pct.map(format::pct).unwrap_or_else(dash), input.spark(&Series::Cpu, 100.0)),
        Device::Memory => {
            let m = t.memory.as_ref();
            let value = m
                .map(|m| format!("{} ({})", format::gb_of(m.used, m.total), format::pct(m.used as f32 / m.total.max(1) as f32 * 100.0)))
                .unwrap_or_else(dash);
            ("Memory".to_string(), value, input.spark(&Series::MemUsed, m.map_or(0.0, |m| m.total as f32)))
        }
    };
    DeviceView { id: d.id(), title, value, spark }
}

pub fn build_perf(input: &PerfInput) -> PerfFrame {
    let list = devices(input.snap, input.info);
    let device = Device::parse(input.device).filter(|d| list.contains(d)).unwrap_or(Device::Cpu);
    let mut f = PerfFrame {
        devices: list.iter().map(|d| device_view(d, input)).collect(),
        requested: input.device.to_string(),
        selected: device.id(),
        range: input.range,
        window_end_ms: input.end(),
        span_ms: input.range.span_ms(),
        end_clock_ms: input.snap.totals.local_time_ms,
        ..Default::default()
    };
    match &device {
        Device::Cpu => cpu_page(&mut f, input),
        Device::Memory => memory_page(&mut f, input),
    }
    f
}

fn cpu_page(f: &mut PerfFrame, input: &PerfInput) {
    let t = &input.snap.totals;
    let procs = &input.snap.processes;
    f.title = "CPU".into();
    f.subtitle = input.info.cpu_name.clone().unwrap_or_default();
    f.scrub_metric = Metric::Cpu;
    let util = t.cpu_pct.map(format::pct).unwrap_or_else(dash);
    f.graphs = vec![graph_view("% Utilization", util.clone(), "100%".into(), input.shape(&Series::Cpu, 100.0, MAIN_BUCKETS))];
    // One cell per logical processor even when the per-core vector is empty (PDH failed to open).
    let cores = (t.logical_cpus as usize).max(t.per_core_pct.len());
    f.cells = (0..cores)
        .map(|i| {
            let value = t.per_core_pct.get(i).copied().flatten().map(format::pct).unwrap_or_else(dash);
            graph_view(&format!("CPU {i}"), value, String::new(), input.shape(&Series::Core(i as u16), 100.0, CELL_BUCKETS))
        })
        .collect();
    // A failed process read leaves the list empty: unknown, never 0.
    let proc_stat = |n: Option<u64>| n.map(|n| n.to_string()).unwrap_or_else(dash);
    let known = !procs.is_empty();
    f.stats = vec![
        stat("Utilization", util),
        stat("Speed", t.cpu_mhz.map(format::ghz).unwrap_or_else(dash)),
        stat("Temperature", dash()),
        stat("Processes", proc_stat(known.then(|| procs.len() as u64))),
        stat("Threads", proc_stat(known.then(|| procs.iter().map(|p| p.threads as u64).sum()))),
        stat("Handles", proc_stat(known.then(|| procs.iter().map(|p| p.handles as u64).sum()))),
        stat("Up time", format::uptime(t.uptime_s)),
        stat("Logical processors", t.logical_cpus.to_string()),
    ];
    f.note = CPU_TEMP_NOTE.into();
}

fn memory_page(f: &mut PerfFrame, input: &PerfInput) {
    f.title = "Memory".into();
    f.scrub_metric = Metric::Memory;
    let Some(m) = input.snap.totals.memory.as_ref() else {
        f.graphs = vec![graph_view("Memory usage", dash(), String::new(), input.shape(&Series::MemUsed, 0.0, MAIN_BUCKETS))];
        f.stats = vec![stat("In use", dash())];
        return;
    };
    f.subtitle = format!("{} total", format::gb(m.total));
    f.graphs = vec![graph_view(
        "Memory usage",
        format::gb(m.used),
        format::gb(m.total),
        input.shape(&Series::MemUsed, m.total as f32, MAIN_BUCKETS),
    )];
    f.segments = composition(m);
    f.stats = vec![
        stat("In use", format::gb(in_use(m))),
        stat("Available", format::gb(m.total.saturating_sub(m.used))),
        stat("Committed", format::gb_of(m.commit_total, m.commit_limit)),
        stat("Cached", m.lists.map(|l| format::gb(l.standby + l.modified)).unwrap_or_else(dash)),
        stat("Compressed", m.compressed.map(|b| format::bytes(b as f64)).unwrap_or_else(dash)),
        stat("Speed", input.info.memory_speed_mts.map(|s| format!("{s} MT/s")).unwrap_or_else(dash)),
    ];
}

/// Memory in use as the "In use" stat shows it: the bar's in-use segment plus compressed memory
/// when the page lists exist, `used` otherwise.
fn in_use(m: &MemoryTotals) -> u64 {
    match m.lists {
        Some(l) => m.total.saturating_sub(l.modified + l.standby + l.free),
        None => m.used,
    }
}

/// The composition bar (spec §5.2): in use (less compressed), compressed, modified, standby and
/// free as fractions of physical memory; in use / available when the page lists are unknown.
pub fn composition(m: &MemoryTotals) -> Vec<SegmentView> {
    let total = m.total.max(1) as f64;
    let seg = |label: &str, b: u64| SegmentView { label: label.into(), value: format::gb(b), fraction: (b as f64 / total) as f32 };
    match m.lists {
        Some(l) => {
            let compressed = m.compressed.unwrap_or(0);
            let in_use = m.total.saturating_sub(l.modified + l.standby + l.free).saturating_sub(compressed);
            vec![
                seg("In use", in_use),
                seg("Compressed", compressed),
                seg("Modified", l.modified),
                seg("Standby", l.standby),
                seg("Free", l.free),
            ]
        }
        None => vec![seg("In use", m.used), seg("Available", m.total.saturating_sub(m.used))],
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use negotium_snapshot::{MemoryLists, MemoryTotals, ProcessSample, SystemTotals};

    fn proc(pid: u32, threads: u32, handles: u32) -> ProcessSample {
        ProcessSample {
            pid, parent_pid: 0, create_time: pid as i64, name: format!("p{pid}.exe"), session_id: 1, cpu_time_100ns: 0,
            private_working_set: 0, disk_io_bytes: 0, threads, handles, image_path: None, package_family: None,
            description: None,
        }
    }

    pub(crate) fn snapshot(t_ms: u64) -> Snapshot {
        Snapshot {
            taken_at_ms: t_ms,
            processes: vec![proc(4, 100, 2000), proc(8, 20, 300)],
            totals: SystemTotals {
                logical_cpus: 4,
                cpu_pct: Some(37.4),
                per_core_pct: vec![Some(10.0), None, Some(80.0), Some(60.0)],
                cpu_mhz: Some(3420.0),
                uptime_s: 93_784,
                local_time_ms: 12 * 3_600_000,
                memory: Some(MemoryTotals {
                    total: 16 << 30, used: 7 << 30, commit_total: 12 << 30, commit_limit: 24 << 30,
                    lists: Some(MemoryLists { modified: 1 << 30, standby: 6 << 30, free: 2 << 30 }),
                    compressed: Some(512 << 20),
                }),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    pub(crate) fn info() -> SystemInfo {
        SystemInfo { cpu_name: Some("Test CPU 9000".into()), memory_speed_mts: Some(6000), ..Default::default() }
    }

    pub(crate) fn build_with(s: &Snapshot, info: &SystemInfo, device: &str, range: Range) -> PerfFrame {
        let mut h = History::new();
        h.push(s.taken_at_ms, &s.totals, &[]);
        build_perf(&PerfInput { snap: s, history: &h, info, device, range })
    }

    fn build(s: &Snapshot, device: &str) -> PerfFrame {
        build_with(s, &info(), device, Range::Minute)
    }

    pub(crate) fn stat<'a>(f: &'a PerfFrame, label: &str) -> &'a str {
        &f.stats.iter().find(|s| s.label == label).unwrap_or_else(|| panic!("no {label:?} in {:?}", f.stats)).value
    }

    #[test]
    fn device_ids_round_trip_and_ranges_clamp() {
        for d in [Device::Cpu, Device::Memory] {
            assert_eq!(Device::parse(&d.id()), Some(d));
        }
        assert_eq!(Device::parse("bogus"), None);
        assert_eq!(Range::from_index(7), Range::Hour);
        assert_eq!(Range::from_index(-1), Range::Minute);
        assert_eq!(Range::ALL.map(Range::span_ms), [60_000, 600_000, 3_600_000]);
        assert_eq!(Range::TenMinutes.index(), 1);
    }

    #[test]
    fn the_device_list_starts_with_cpu_and_memory() {
        let f = build(&snapshot(60_000), "cpu");
        assert_eq!(f.devices.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(), ["cpu", "memory"]);
        assert_eq!(f.devices[0].value, "37%");
        assert_eq!(f.devices[1].value, "7.0 / 16.0 GB (44%)");
        assert_ne!(f.devices[0].spark, EMPTY_PATH);
    }

    #[test]
    fn an_unknown_or_vanished_device_falls_back_to_cpu() {
        let f = build(&snapshot(60_000), "disk:9 Z:");
        assert_eq!(f.requested, "disk:9 Z:");
        assert_eq!(f.selected, "cpu");
        assert_eq!(f.title, "CPU");
    }

    #[test]
    fn the_cpu_page_has_one_cell_per_logical_processor_in_place() {
        let s = snapshot(60_000);
        let f = build(&s, "cpu");
        assert_eq!((f.title.as_str(), f.subtitle.as_str()), ("CPU", "Test CPU 9000"));
        assert_eq!(f.graphs.len(), 1);
        assert_eq!((f.graphs[0].label.as_str(), f.graphs[0].value.as_str(), f.graphs[0].scale.as_str()), ("% Utilization", "37%", "100%"));
        assert_eq!(f.cells.iter().map(|c| c.label.as_str()).collect::<Vec<_>>(), ["CPU 0", "CPU 1", "CPU 2", "CPU 3"]);
        assert_eq!(f.cells[1].value, "—", "a processor without data this tick keeps its cell");
        assert_eq!(f.cells[2].value, "80%");
        assert_eq!(stat(&f, "Speed"), "3.42 GHz");
        assert_eq!(stat(&f, "Temperature"), "—");
        assert_eq!(stat(&f, "Processes"), "2");
        assert_eq!(stat(&f, "Threads"), "120");
        assert_eq!(stat(&f, "Handles"), "2300");
        assert_eq!(stat(&f, "Up time"), "1:02:03:04");
        assert_eq!(f.note, CPU_TEMP_NOTE);
        assert_eq!(f.scrub_metric, Metric::Cpu);
        assert_eq!((f.window_end_ms, f.span_ms, f.end_clock_ms), (60_000, 60_000, 12 * 3_600_000));
    }

    #[test]
    fn the_memory_page_shows_composition_and_details() {
        let f = build(&snapshot(60_000), "memory");
        assert_eq!(f.title, "Memory");
        assert_eq!(f.segments.iter().map(|s| s.label.as_str()).collect::<Vec<_>>(), ["In use", "Compressed", "Modified", "Standby", "Free"]);
        assert_eq!(f.segments[0].value, "6.5 GB");
        assert!((f.segments.iter().map(|s| s.fraction).sum::<f32>() - 1.0).abs() < 1e-4);
        assert_eq!(f.graphs[0].scale, "16.0 GB");
        assert_eq!(stat(&f, "Committed"), "12.0 / 24.0 GB");
        assert_eq!(stat(&f, "In use"), "7.0 GB", "in use plus compressed: 6.5 GB + 512 MB");
        assert_eq!(stat(&f, "Cached"), "7.0 GB");
        assert_eq!(stat(&f, "Compressed"), "512.0 MB");
        assert_eq!(stat(&f, "Speed"), "6000 MT/s");
        assert_eq!(f.scrub_metric, Metric::Memory);
    }

    #[test]
    fn memory_without_page_lists_falls_back_to_used_and_available() {
        let mut s = snapshot(60_000);
        s.totals.memory.as_mut().unwrap().lists = None;
        let f = build(&s, "memory");
        assert_eq!(f.segments.iter().map(|s| s.label.as_str()).collect::<Vec<_>>(), ["In use", "Available"]);
        assert_eq!(stat(&f, "Cached"), "—");
        assert_eq!(stat(&f, "In use"), "7.0 GB");
        assert_eq!(f.segments[0].value, "7.0 GB", "one number per label");
    }

    #[test]
    fn cpu_cells_survive_a_failed_counter_and_unreadable_processes_show_a_dash() {
        let mut s = snapshot(60_000);
        s.totals.per_core_pct.clear();
        s.processes.clear();
        let f = build(&s, "cpu");
        assert_eq!(f.cells.len(), 4, "one cell per logical processor");
        assert!(f.cells.iter().all(|c| c.value == "—"));
        assert_eq!(f.cells[3].label, "CPU 3");
        for l in ["Processes", "Threads", "Handles"] {
            assert_eq!(stat(&f, l), "—");
        }
    }

    #[test]
    fn the_hour_range_draws_only_recorded_history() {
        let f = build_with(&snapshot(4_000_000), &info(), "cpu", Range::Hour);
        assert_eq!(f.graphs[0].line, "M 1000.0 63.0 L 1000.1 63.0", "one second of data: one point at the right edge");
        assert_eq!(f.span_ms, 3_600_000);
    }
}
