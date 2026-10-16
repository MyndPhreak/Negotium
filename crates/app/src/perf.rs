//! Performance page view model (spec §5.2): the device list and the selected device's page.
//! Pure; built on the worker thread from the snapshot, the history and SystemInfo. Only the
//! selected device's page is built each tick (spec §8).
use crate::format;
use crate::graph::{self, EMPTY_PATH, Shape};
use negotium_collector::SystemInfo;
use negotium_history::{History, Series};
use negotium_model::Metric;
use negotium_snapshot::{AdapterSample, DiskSample, GpuSample, MemoryTotals, Snapshot};

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
    /// PhysicalDisk instance, e.g. "0 C:".
    Disk(String),
    /// Network Interface instance.
    Network(String),
    /// GPU key, "luid_…_phys_N".
    Gpu(String),
}

impl Device {
    pub fn id(&self) -> String {
        match self {
            Device::Cpu => "cpu".to_string(),
            Device::Memory => "memory".to_string(),
            Device::Disk(n) => format!("disk:{n}"),
            Device::Network(n) => format!("net:{n}"),
            Device::Gpu(k) => format!("gpu:{k}"),
        }
    }

    pub fn parse(id: &str) -> Option<Device> {
        match id {
            "cpu" => Some(Device::Cpu),
            "memory" => Some(Device::Memory),
            _ => {
                let (kind, name) = id.split_once(':')?;
                match kind {
                    "disk" => Some(Device::Disk(name.to_string())),
                    "net" => Some(Device::Network(name.to_string())),
                    "gpu" => Some(Device::Gpu(name.to_string())),
                    _ => None,
                }
            }
        }
    }
}

/// A device of the current snapshot, borrowed from it: the list is rebuilt every tick without
/// copying any device name.
#[derive(Clone, Copy)]
enum Entry<'a> {
    Cpu,
    Memory,
    Disk(&'a DiskSample),
    Network(&'a AdapterSample),
    Gpu(&'a GpuSample),
}

impl Entry<'_> {
    fn id(&self) -> String {
        match self {
            Entry::Cpu => "cpu".to_string(),
            Entry::Memory => "memory".to_string(),
            Entry::Disk(d) => format!("disk:{}", d.name),
            Entry::Network(a) => format!("net:{}", a.name),
            Entry::Gpu(g) => format!("gpu:{}", g.key),
        }
    }

    fn is(&self, d: &Device) -> bool {
        match (self, d) {
            (Entry::Cpu, Device::Cpu) | (Entry::Memory, Device::Memory) => true,
            (Entry::Disk(a), Device::Disk(n)) => a.name == *n,
            (Entry::Network(a), Device::Network(n)) => a.name == *n,
            (Entry::Gpu(g), Device::Gpu(k)) => g.key == *k,
            _ => false,
        }
    }

    /// Position of this kind of device in the list.
    fn rank(&self) -> u8 {
        match self {
            Entry::Cpu => 0,
            Entry::Memory => 1,
            Entry::Disk(_) => 2,
            Entry::Network(_) => 3,
            Entry::Gpu(_) => 4,
        }
    }

    fn device(&self) -> Device {
        match self {
            Entry::Cpu => Device::Cpu,
            Entry::Memory => Device::Memory,
            Entry::Disk(d) => Device::Disk(d.name.clone()),
            Entry::Network(a) => Device::Network(a.name.clone()),
            Entry::Gpu(g) => Device::Gpu(g.key.clone()),
        }
    }
}

/// "0 C:" → "Disk 0 (C:)"; "2" → "Disk 2"; anything else unchanged.
pub fn disk_title(name: &str) -> String {
    let mut parts = name.split_whitespace();
    match parts.next().map(|n| (n, n.parse::<u32>())) {
        Some((n, Ok(_))) => {
            let volumes: Vec<&str> = parts.collect();
            if volumes.is_empty() { format!("Disk {n}") } else { format!("Disk {n} ({})", volumes.join(" ")) }
        }
        _ => name.to_string(),
    }
}

const MIB: f32 = 1_048_576.0;

fn disk_number(name: &str) -> u32 {
    name.split_whitespace().next().and_then(|n| n.parse().ok()).unwrap_or(u32::MAX)
}

fn adapter_name(a: &AdapterSample) -> &str {
    a.alias.as_deref().unwrap_or(&a.name)
}

pub fn adapter_title(a: &AdapterSample) -> String {
    adapter_name(a).to_string()
}

/// Every device with a page, in display order: CPU, memory, disks by number, adapters (connected
/// ones first, hardware before virtual, then by name; disconnected ones last), GPUs in DXGI order
/// (unknown ones last). Sorts compare borrowed names.
fn entries<'a>(snap: &'a Snapshot, info: &SystemInfo) -> Vec<Entry<'a>> {
    let t = &snap.totals;
    let mut out = vec![Entry::Cpu, Entry::Memory];
    let mut disks: Vec<&DiskSample> = t.disks.iter().flatten().collect();
    disks.sort_by(|a, b| disk_number(&a.name).cmp(&disk_number(&b.name)).then_with(|| a.name.cmp(&b.name)));
    out.extend(disks.into_iter().map(Entry::Disk));
    let mut nets: Vec<&AdapterSample> = t.adapters.iter().flatten().collect();
    nets.sort_by(|a, b| {
        (!a.connected, !a.hardware, adapter_name(a), &a.name).cmp(&(!b.connected, !b.hardware, adapter_name(b), &b.name))
    });
    out.extend(nets.into_iter().map(Entry::Network));
    let dxgi = |key: &str| info.gpus.iter().position(|i| i.key == key).unwrap_or(usize::MAX);
    let mut gpus: Vec<&GpuSample> = t.gpus.iter().flatten().collect();
    gpus.sort_by_cached_key(|g| (dxgi(&g.key), g.key.as_str()));
    out.extend(gpus.into_iter().map(Entry::Gpu));
    out
}

/// Every device with a page, in display order.
pub fn devices(snap: &Snapshot, info: &SystemInfo) -> Vec<Device> {
    entries(snap, info).iter().map(Entry::device).collect()
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
    /// Graph window `[window_end_ms - span_ms, window_end_ms]` in collector time, for the scrub:
    /// `window_end_ms` is the newest history sample, where the graphs' right edge rests.
    pub window_end_ms: u64,
    pub span_ms: u64,
    /// Local wall-clock time of `window_end_ms` (that sample's clock), ms since midnight.
    pub end_clock_ms: u64,
    /// Which app ranking the scrub shows on this page (ruling P3-4).
    pub scrub_metric: Metric,
    /// The page's main series: within a graph column the scrub shows the sample where it peaks,
    /// the one that column draws (ruling P3-5).
    pub scrub_series: Option<Series>,
}

pub struct PerfInput<'a> {
    pub snap: &'a Snapshot,
    pub history: &'a History,
    pub info: &'a SystemInfo,
    pub device: &'a str,
    pub range: Range,
    /// The requested device as last seen (see [`LastSeen::track`]): keeps its page through a
    /// brief absence.
    pub last_seen: Option<&'a LastSeen>,
}

/// How long a selected device's page outlives the device: a counter that misses a tick or
/// re-enumerates must not throw the user back to the CPU page.
pub const DEVICE_GRACE_MS: u64 = 5_000;

/// A disk, adapter or GPU as last seen in a snapshot, kept by the worker across ticks.
#[derive(Clone, Debug, PartialEq)]
pub struct LastSeen {
    at_ms: u64,
    sample: Seen,
}

#[derive(Clone, Debug, PartialEq)]
enum Seen {
    Disk(DiskSample),
    Network(AdapterSample),
    Gpu(GpuSample),
}

impl LastSeen {
    /// This tick's record for `device`: its sample if the snapshot has it, else `prev` if that is
    /// the same device (it keeps the time it was last seen). CPU and memory are always present.
    pub fn track(prev: Option<LastSeen>, snap: &Snapshot, device: &str) -> Option<LastSeen> {
        let t = &snap.totals;
        let at_ms = snap.taken_at_ms;
        let now = match Device::parse(device)? {
            Device::Cpu | Device::Memory => return None,
            Device::Disk(n) => t.disks.iter().flatten().find(|d| d.name == n).map(|d| Seen::Disk(d.clone())),
            Device::Network(n) => t.adapters.iter().flatten().find(|a| a.name == n).map(|a| Seen::Network(a.clone())),
            Device::Gpu(k) => t.gpus.iter().flatten().find(|g| g.key == k).map(|g| Seen::Gpu(g.clone())),
        };
        match now {
            Some(sample) => Some(LastSeen { at_ms, sample }),
            None => prev.filter(|p| p.entry().id() == device),
        }
    }

    fn entry(&self) -> Entry<'_> {
        match &self.sample {
            Seen::Disk(d) => Entry::Disk(d),
            Seen::Network(a) => Entry::Network(a),
            Seen::Gpu(g) => Entry::Gpu(g),
        }
    }
}

impl PerfInput<'_> {
    /// This tick's collector time (the device grace period counts from it).
    fn end(&self) -> u64 {
        self.snap.taken_at_ms
    }

    /// The graphs' right edge (see [`graph::end_of`]).
    fn anchor(&self) -> u64 {
        graph::end_of(self.history, self.end())
    }

    /// Local wall-clock time (ms since midnight) of the graphs' right edge.
    fn anchor_clock(&self) -> u64 {
        crate::scrub::clock_before(self.snap.totals.local_time_ms, self.end().saturating_sub(self.anchor()))
    }

    fn shape(&self, key: &Series, max: f32, buckets: usize) -> Shape {
        let (end, span) = (self.anchor(), self.range.span_ms());
        graph::shape(self.history.points(key, graph::since(end, span)), end, span, max, buckets)
    }

    /// A small multiple's shape: the line only, since cells draw no area (Graph `filled: false`).
    fn cell_shape(&self, key: &Series, max: f32) -> Shape {
        let (end, span) = (self.anchor(), self.range.span_ms());
        let line = graph::line(self.history.points(key, graph::since(end, span)), end, span, max, CELL_BUCKETS);
        Shape { line, area: EMPTY_PATH.to_string() }
    }

    /// The largest value of a series a graph of the last `span_ms` draws, 0 when there is none:
    /// the window and the history before it a scrolling view shows ([`graph::since`]).
    fn max_over(&self, key: &Series, span_ms: u64) -> f32 {
        self.history.points(key, graph::since(self.anchor(), span_ms)).filter_map(|(_, v)| v).filter(|v| v.is_finite()).fold(0.0, f32::max)
    }

    fn spark(&self, key: &Series, max: f32) -> String {
        let end = self.anchor();
        graph::line(self.history.points(key, graph::since(end, SPARK_MS)), end, SPARK_MS, max, 60)
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

fn empty_shape() -> Shape {
    Shape { line: EMPTY_PATH.to_string(), area: EMPTY_PATH.to_string() }
}

/// A percentage that may be unknown (NaN, as a GPU seeded without engine data reports it).
fn known(v: f32) -> Option<f32> {
    v.is_finite().then_some(v)
}

fn device_view(e: &Entry, gpu_number: usize, input: &PerfInput) -> DeviceView {
    let t = &input.snap.totals;
    let (title, value, spark) = match e {
        Entry::Cpu => ("CPU".to_string(), t.cpu_pct.map(format::pct).unwrap_or_else(dash), input.spark(&Series::Cpu, 100.0)),
        Entry::Memory => {
            let m = t.memory.as_ref().filter(|m| m.total > 0);
            let value = m
                .map(|m| format!("{} ({})", format::gb_of(m.in_use(), m.total), format::pct(m.in_use() as f32 / m.total as f32 * 100.0)))
                .unwrap_or_else(dash);
            let spark = m.map_or_else(|| EMPTY_PATH.to_string(), |m| input.spark(&Series::MemUsed, m.total as f32));
            ("Memory".to_string(), value, spark)
        }
        Entry::Disk(d) => (disk_title(&d.name), format::pct(d.active_pct), input.spark(&Series::DiskActive(d.name.clone()), 100.0)),
        Entry::Network(a) => {
            let key = Series::NetRecv(a.name.clone());
            let max = graph::nice_max(input.max_over(&key, SPARK_MS) * 8.0, 100_000.0) / 8.0;
            let value = if a.connected {
                format!("S {} · R {}", format::bit_rate(a.sent_bps), format::bit_rate(a.recv_bps))
            } else {
                "Disconnected".to_string()
            };
            (adapter_title(a), value, input.spark(&key, max))
        }
        Entry::Gpu(g) => {
            let value = match (known(g.util_pct), g.temp_c) {
                (Some(u), Some(c)) => format!("{} ({})", format::pct(u), format::temp_c(c)),
                (Some(u), None) => format::pct(u),
                (None, Some(c)) => format::temp_c(c),
                (None, None) => dash(),
            };
            (format!("GPU {gpu_number}"), value, input.spark(&Series::GpuUtil(g.key.clone()), 100.0))
        }
    };
    DeviceView { id: e.id(), title, value, spark }
}

pub fn build_perf(input: &PerfInput) -> PerfFrame {
    let mut list = entries(input.snap, input.info);
    // The requested device; while it is briefly missing, its last-seen self with every value
    // unknown; after DEVICE_GRACE_MS the CPU. Never stale numbers.
    let wanted = Device::parse(input.device);
    let mut held = false;
    let selected = match wanted.and_then(|d| list.iter().position(|e| e.is(&d))) {
        Some(i) => i,
        None => match input.last_seen.filter(|s| s.entry().id() == input.device && input.end().saturating_sub(s.at_ms) <= DEVICE_GRACE_MS) {
            Some(seen) => {
                let ghost = seen.entry();
                let i = list.iter().position(|e| e.rank() > ghost.rank()).unwrap_or(list.len());
                list.insert(i, ghost);
                held = true;
                i
            }
            None => 0,
        },
    };
    let mut gpus = 0;
    let mut selected_gpu = 0;
    let devices = list
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let number = gpus;
            if matches!(e, Entry::Gpu(_)) {
                gpus += 1;
                if i == selected {
                    selected_gpu = number;
                }
            }
            device_view(e, number, input)
        })
        .collect();
    let mut f = PerfFrame {
        devices,
        requested: input.device.to_string(),
        selected: list[selected].id(),
        range: input.range,
        window_end_ms: input.anchor(),
        span_ms: input.range.span_ms(),
        end_clock_ms: input.anchor_clock(),
        ..Default::default()
    };
    match list[selected] {
        Entry::Cpu => cpu_page(&mut f, input),
        Entry::Memory => memory_page(&mut f, input),
        Entry::Disk(d) => disk_page(&mut f, input, d),
        Entry::Network(a) => net_page(&mut f, input, a),
        Entry::Gpu(g) => gpu_page(&mut f, input, g, selected_gpu),
    }
    if held {
        // Built from the last-seen sample for its names and history; none of its values is current.
        f.devices[selected].value = dash();
        for v in f.graphs.iter_mut().chain(f.cells.iter_mut()).map(|g| &mut g.value) {
            *v = dash();
        }
        for s in &mut f.stats {
            s.value = dash();
        }
    }
    f
}

fn cpu_page(f: &mut PerfFrame, input: &PerfInput) {
    let t = &input.snap.totals;
    let procs = &input.snap.processes;
    f.title = "CPU".into();
    f.subtitle = input.info.cpu_name.clone().unwrap_or_default();
    f.scrub_metric = Metric::Cpu;
    f.scrub_series = Some(Series::Cpu);
    let util = t.cpu_pct.map(format::pct).unwrap_or_else(dash);
    f.graphs = vec![graph_view("% Utilization", util.clone(), "100%".into(), input.shape(&Series::Cpu, 100.0, MAIN_BUCKETS))];
    // One cell per logical processor even when the per-core vector is empty (PDH failed to open).
    let cores = (t.logical_cpus as usize).max(t.per_core_pct.len());
    f.cells = (0..cores)
        .map(|i| {
            let value = t.per_core_pct.get(i).copied().flatten().map(format::pct).unwrap_or_else(dash);
            graph_view(&format!("CPU {i}"), value, String::new(), input.cell_shape(&Series::Core(i as u16), 100.0))
        })
        .collect();
    // A failed process read leaves the list empty: unknown, never 0.
    let proc_stat = |n: Option<u64>| n.map(|n| n.to_string()).unwrap_or_else(dash);
    let known = !procs.is_empty();
    f.stats = vec![
        stat("Utilization", util),
        stat("Speed", t.cpu_mhz.map(format::ghz).unwrap_or_else(dash)),
        stat("Temperature", dash()),
        stat("Processes", proc_stat(known.then_some(procs.len() as u64))),
        stat("Threads", proc_stat(known.then(|| procs.iter().map(|p| p.threads as u64).sum()))),
        stat("Handles", proc_stat(known.then(|| procs.iter().map(|p| p.handles as u64).sum()))),
        // A zero means the value was never read.
        stat("Up time", if t.uptime_s > 0 { format::uptime(t.uptime_s) } else { dash() }),
        stat("Logical processors", if t.logical_cpus > 0 { t.logical_cpus.to_string() } else { dash() }),
    ];
    f.note = CPU_TEMP_NOTE.into();
}

fn memory_page(f: &mut PerfFrame, input: &PerfInput) {
    f.title = "Memory".into();
    f.scrub_metric = Metric::Memory;
    f.scrub_series = Some(Series::MemUsed);
    let Some(m) = input.snap.totals.memory.as_ref().filter(|m| m.total > 0) else {
        f.graphs = vec![graph_view("Memory usage", dash(), String::new(), empty_shape())];
        f.stats = vec![stat("In use", dash())];
        return;
    };
    f.subtitle = format!("{} total", format::gb(m.total));
    f.graphs = vec![graph_view(
        "Memory usage",
        format::gb(m.in_use()),
        format::gb(m.total),
        input.shape(&Series::MemUsed, m.total as f32, MAIN_BUCKETS),
    )];
    f.segments = composition(m);
    f.stats = vec![
        stat("In use", format::gb(m.in_use())),
        stat("Available", format::gb(m.available())),
        stat("Committed", format::gb_of(m.commit_total, m.commit_limit)),
        stat("Cached", m.lists.map(|l| format::gb(l.standby + l.modified)).unwrap_or_else(dash)),
        stat("Compressed", m.compressed.map(|b| format::bytes(b as f64)).unwrap_or_else(dash)),
        stat("Speed", input.info.memory_speed_mts.map(|s| format!("{s} MT/s")).unwrap_or_else(dash)),
    ];
}

/// The composition bar (spec §5.2): in use (less compressed), compressed, modified, standby and
/// free as fractions of physical memory; in use / available when the page lists are unknown.
pub fn composition(m: &MemoryTotals) -> Vec<SegmentView> {
    let total = m.total.max(1) as f64;
    let seg = |label: &str, b: u64| SegmentView { label: label.into(), value: format::gb(b), fraction: (b as f64 / total) as f32 };
    match m.lists {
        Some(l) => {
            let compressed = m.compressed.unwrap_or(0);
            vec![
                seg("In use", m.in_use().saturating_sub(compressed)),
                seg("Compressed", compressed),
                seg("Modified", l.modified),
                seg("Standby", l.standby),
                seg("Free", l.free),
            ]
        }
        None => vec![seg("In use", m.in_use()), seg("Available", m.available())],
    }
}

fn disk_page(f: &mut PerfFrame, input: &PerfInput, d: &DiskSample) {
    let name = d.name.as_str();
    f.title = disk_title(name);
    f.scrub_metric = Metric::Disk;
    f.scrub_series = Some(Series::DiskActive(name.into()));
    f.note = "Top apps show each app's disk use across all disks.".into();
    let (read_key, write_key) = (Series::DiskRead(name.into()), Series::DiskWrite(name.into()));
    let span = input.range.span_ms();
    // Byte rates are shown in binary units: round the scale in MB/s so the label reads "2.0 MB/s".
    let peak = input.max_over(&read_key, span).max(input.max_over(&write_key, span));
    let scale = graph::nice_max(peak / MIB, 1.0) * MIB;
    let mut transfer = graph_view(
        "Transfer rate",
        format!("Read {} · Write {}", format::byte_rate(d.read_bps), format::byte_rate(d.write_bps)),
        format::byte_rate(scale as f64),
        input.shape(&read_key, scale, MAIN_BUCKETS),
    );
    transfer.line2 = input.shape(&write_key, scale, MAIN_BUCKETS).line;
    f.graphs = vec![
        graph_view("Active time", format::pct(d.active_pct), "100%".into(), input.shape(&Series::DiskActive(name.into()), 100.0, MAIN_BUCKETS)),
        transfer,
    ];
    f.stats = vec![
        stat("Active time", format::pct(d.active_pct)),
        stat("Read speed", format::byte_rate(d.read_bps)),
        stat("Write speed", format::byte_rate(d.write_bps)),
        stat("Average response time", d.response_ms.map(format::millis).unwrap_or_else(dash)),
    ];
}

fn net_page(f: &mut PerfFrame, input: &PerfInput, a: &AdapterSample) {
    let name = a.name.as_str();
    f.title = adapter_title(a);
    f.subtitle = a.description.clone().unwrap_or_else(|| name.to_string());
    f.scrub_metric = Metric::Network;
    f.scrub_series = Some(Series::NetRecv(name.into()));
    f.note = "Per-app network use arrives in a later version.".into();
    let (sent_key, recv_key) = (Series::NetSent(name.into()), Series::NetRecv(name.into()));
    let span = input.range.span_ms();
    // Network rates are shown in decimal bits: round the scale in bits, at least 100 Kb/s.
    let peak = input.max_over(&sent_key, span).max(input.max_over(&recv_key, span));
    let scale = graph::nice_max(peak * 8.0, 100_000.0) / 8.0;
    let mut g = graph_view(
        "Throughput",
        format!("Send {} · Receive {}", format::bit_rate(a.sent_bps), format::bit_rate(a.recv_bps)),
        format::bit_rate(scale as f64),
        input.shape(&recv_key, scale, MAIN_BUCKETS),
    );
    g.line2 = input.shape(&sent_key, scale, MAIN_BUCKETS).line;
    f.graphs = vec![g];
    f.stats = vec![
        stat("Send", format::bit_rate(a.sent_bps)),
        stat("Receive", format::bit_rate(a.recv_bps)),
        stat("Link speed", a.link_bps.map(|b| format::bit_rate(b as f64 / 8.0)).unwrap_or_else(dash)),
        stat("Adapter", if a.hardware { "Hardware" } else { "Virtual" }),
        stat("Status", if a.connected { "Connected" } else { "Disconnected" }),
    ];
    for addr in &a.addresses {
        f.stats.push(stat(if addr.contains(':') { "IPv6 address" } else { "IPv4 address" }, addr.clone()));
    }
}

/// Display order of the per-engine graphs: 3D, Copy, the video engines, then the rest by name.
fn engine_rank(engtype: &str) -> u8 {
    match engtype {
        "3D" => 0,
        "Copy" => 1,
        "VideoEncode" => 2,
        "VideoDecode" => 3,
        _ => 4,
    }
}

fn gpu_page(f: &mut PerfFrame, input: &PerfInput, g: &GpuSample, number: usize) {
    let key = g.key.as_str();
    let info = input.info.gpus.iter().find(|i| i.key == key);
    f.title = format!("GPU {number}");
    f.subtitle = info.map(|i| i.name.clone()).unwrap_or_default();
    f.scrub_metric = Metric::Gpu;
    f.scrub_series = Some(Series::GpuUtil(key.into()));
    let util = known(g.util_pct).map(format::pct).unwrap_or_else(dash);
    let memory = |used: Option<u64>, total: Option<u64>| match (used, total) {
        (Some(u), Some(t)) => format::gb_of(u, t),
        (Some(u), None) => format::gb(u),
        _ => dash(),
    };
    let dedicated_total = info.map(|i| i.dedicated_total).filter(|&t| t > 0);
    let shared_total = info.map(|i| i.shared_total).filter(|&t| t > 0);
    let dedicated = memory(g.dedicated_used, dedicated_total);
    let shared = memory(g.shared_used, shared_total);
    f.graphs = vec![graph_view("Utilization", util.clone(), "100%".into(), input.shape(&Series::GpuUtil(key.into()), 100.0, MAIN_BUCKETS))];
    if let Some(total) = dedicated_total {
        let shape = input.shape(&Series::GpuDedicated(key.into()), total as f32, MAIN_BUCKETS);
        f.graphs.push(graph_view("Dedicated GPU memory", dedicated.clone(), format::gb(total), shape));
    }
    if let Some(total) = shared_total {
        let shape = input.shape(&Series::GpuShared(key.into()), total as f32, MAIN_BUCKETS);
        f.graphs.push(graph_view("Shared GPU memory", shared.clone(), format::gb(total), shape));
    }
    // One graph per engine type, showing its busiest engine.
    let mut engines: Vec<&(String, f32)> = g.engines.iter().collect();
    engines.sort_by(|a, b| engine_rank(&a.0).cmp(&engine_rank(&b.0)).then_with(|| a.0.cmp(&b.0)));
    f.cells = engines
        .into_iter()
        .map(|(engtype, v)| {
            let shape = input.cell_shape(&Series::GpuEngine(key.into(), engtype.clone()), 100.0);
            graph_view(engtype, known(*v).map(format::pct).unwrap_or_else(dash), String::new(), shape)
        })
        .collect();
    f.stats = vec![
        stat("Utilization", util),
        stat("Dedicated GPU memory", dedicated),
        stat("Shared GPU memory", shared),
        stat("Temperature", g.temp_c.map(format::temp_c).unwrap_or_else(dash)),
    ];
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use negotium_snapshot::{AdapterSample, DiskSample, GpuSample, MemoryLists, MemoryTotals, ProcessSample, SystemTotals};

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
        build_perf(&PerfInput { snap: s, history: &h, info, device, range, last_seen: None })
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

    // The cells draw no area (Graph `filled: false`): building one every tick would be wasted.
    #[test]
    fn cells_carry_a_line_but_no_area_while_main_graphs_keep_theirs() {
        let f = build(&snapshot(60_000), "cpu");
        assert_ne!(f.graphs[0].area, EMPTY_PATH);
        assert!(f.cells.iter().any(|c| c.line != EMPTY_PATH), "some cell has a line");
        assert!(f.cells.iter().all(|c| c.area == EMPTY_PATH), "no cell has an area");
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
        assert_eq!(f.scrub_series, Some(Series::Cpu), "the scrub follows the main graph");
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
        assert_eq!(f.scrub_series, Some(Series::MemUsed));
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
        assert_eq!(f.graphs[0].line, "M 1000.0 63.0 L 999.9 63.0", "one second of data: one point at the right edge");
        assert_eq!(f.span_ms, 3_600_000);
    }

    use negotium_collector::gpu::GpuInfo;

    fn devices_snapshot() -> Snapshot {
        let mut s = snapshot(60_000);
        let disk = |name: &str, active: f32| DiskSample {
            name: name.into(), active_pct: active, read_bps: 1_048_576.0, write_bps: 2_097_152.0, response_ms: Some(0.4),
        };
        s.totals.disks = Some(vec![disk("1 D:", 3.0), disk("0 C:", 12.0)]);
        let adapter = |name: &str, alias: &str, hardware: bool, connected: bool| AdapterSample {
            name: name.into(), alias: Some(alias.into()), description: Some(name.replace('[', "(").replace(']', ")")),
            sent_bps: 12_500.0, recv_bps: 125_000.0, hardware, connected, link_bps: Some(1_000_000_000),
            addresses: vec!["192.168.1.20".into(), "2001:db8::5".into()],
        };
        s.totals.adapters = Some(vec![
            adapter("Hyper-V Virtual Ethernet Adapter", "vEthernet (Default Switch)", false, true),
            adapter("Bluetooth Device [Personal Area Network]", "Bluetooth Network Connection", false, false),
            adapter("Intel[R] Ethernet", "Ethernet", true, true),
        ]);
        let gpu = |key: &str, util: f32, temp: Option<f32>| GpuSample {
            key: key.into(), util_pct: util, engines: vec![("3D".into(), util), ("Copy".into(), 1.0)],
            dedicated_used: Some(3 << 30), shared_used: Some(1 << 28), temp_c: temp,
        };
        s.totals.gpus = Some(vec![gpu("luid_a", 5.0, None), gpu("luid_b", 40.0, Some(64.4))]);
        s
    }

    fn devices_info() -> SystemInfo {
        let g = |key: &str, name: &str| GpuInfo { key: key.into(), name: name.into(), dedicated_total: 24 << 30, shared_total: 32 << 30, ..Default::default() };
        SystemInfo { gpus: vec![g("luid_b", "NVIDIA GeForce RTX 4090"), g("luid_a", "AMD Radeon(TM) Graphics")], ..info() }
    }

    fn devices_build(device: &str) -> PerfFrame {
        build_with(&devices_snapshot(), &devices_info(), device, Range::Minute)
    }

    #[test]
    fn devices_are_listed_in_task_manager_order() {
        let f = devices_build("cpu");
        let ids: Vec<&str> = f.devices.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, [
            "cpu", "memory", "disk:0 C:", "disk:1 D:",
            "net:Intel[R] Ethernet", "net:Hyper-V Virtual Ethernet Adapter", "net:Bluetooth Device [Personal Area Network]",
            "gpu:luid_b", "gpu:luid_a",
        ], "connected adapters (hardware first), then disconnected ones; GPUs in DXGI order");
        let titles: Vec<&str> = f.devices.iter().map(|d| d.title.as_str()).collect();
        assert_eq!(&titles[2..], ["Disk 0 (C:)", "Disk 1 (D:)", "Ethernet", "vEthernet (Default Switch)", "Bluetooth Network Connection", "GPU 0", "GPU 1"]);
        assert_eq!(f.devices[6].value, "Disconnected");
        assert_eq!(f.devices[7].value, "40% (64 °C)");
        assert_eq!(f.devices[8].value, "5%");
        let ids: Vec<String> = devices(&devices_snapshot(), &devices_info()).iter().map(Device::id).collect();
        assert_eq!(ids.len(), 9);
        for d in [Device::Disk("0 C:".into()), Device::Network("x".into()), Device::Gpu("luid_b".into())] {
            assert_eq!(Device::parse(&d.id()), Some(d));
        }
    }

    #[test]
    fn unknown_gpus_sort_last() {
        let mut info = devices_info();
        info.gpus.retain(|g| g.key == "luid_a");
        let f = build_with(&devices_snapshot(), &info, "cpu", Range::Minute);
        let ids: Vec<&str> = f.devices.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(&ids[ids.len() - 2..], ["gpu:luid_a", "gpu:luid_b"]);
    }

    #[test]
    fn disk_titles_name_the_volumes() {
        assert_eq!(disk_title("0 C:"), "Disk 0 (C:)");
        assert_eq!(disk_title("1 D: E:"), "Disk 1 (D: E:)");
        assert_eq!(disk_title("2"), "Disk 2");
        assert_eq!(disk_title("weird"), "weird");
    }

    #[test]
    fn a_disk_page_shows_active_time_transfer_rate_and_response_time() {
        let f = devices_build("disk:0 C:");
        assert_eq!(f.title, "Disk 0 (C:)");
        assert_eq!(f.graphs.iter().map(|g| g.label.as_str()).collect::<Vec<_>>(), ["Active time", "Transfer rate"]);
        assert_ne!(f.graphs[1].line2, EMPTY_PATH, "write rate drawn as the second line");
        assert_eq!(f.graphs[1].scale, "2.0 MB/s");
        assert_eq!(stat(&f, "Active time"), "12%");
        assert_eq!(stat(&f, "Read speed"), "1.0 MB/s");
        assert_eq!(stat(&f, "Average response time"), "0.4 ms");
        assert_eq!(f.scrub_metric, Metric::Disk);
        assert_eq!(f.scrub_series, Some(Series::DiskActive("0 C:".into())));
    }

    #[test]
    fn a_disk_without_a_response_time_shows_a_dash_for_that_item_only() {
        let mut s = devices_snapshot();
        s.totals.disks.as_mut().unwrap()[1].response_ms = None;
        let f = build_with(&s, &devices_info(), "disk:0 C:", Range::Minute);
        assert_eq!(stat(&f, "Average response time"), "—");
        assert_eq!(stat(&f, "Active time"), "12%");
    }

    #[test]
    fn a_network_page_shows_throughput_link_speed_and_addresses() {
        let f = devices_build("net:Intel[R] Ethernet");
        assert_eq!((f.title.as_str(), f.subtitle.as_str()), ("Ethernet", "Intel(R) Ethernet"));
        assert_eq!(f.graphs[0].label, "Throughput");
        assert_ne!(f.graphs[0].line2, EMPTY_PATH);
        assert_eq!(stat(&f, "Send"), "100.0 Kb/s");
        assert_eq!(stat(&f, "Receive"), "1.0 Mb/s");
        assert_eq!(stat(&f, "Link speed"), "1.0 Gb/s");
        assert_eq!(stat(&f, "Adapter"), "Hardware");
        assert_eq!(stat(&f, "Status"), "Connected");
        assert_eq!(stat(&f, "IPv4 address"), "192.168.1.20");
        assert_eq!(stat(&f, "IPv6 address"), "2001:db8::5");
        assert_eq!(f.scrub_metric, Metric::Network);
        assert_eq!(f.scrub_series, Some(Series::NetRecv("Intel[R] Ethernet".into())));
    }

    #[test]
    fn a_disconnected_adapter_still_has_a_page() {
        let mut s = devices_snapshot();
        s.totals.adapters.as_mut().unwrap()[1].link_bps = None;
        let f = build_with(&s, &devices_info(), "net:Bluetooth Device [Personal Area Network]", Range::Minute);
        assert_eq!(f.selected, "net:Bluetooth Device [Personal Area Network]");
        assert_eq!(f.title, "Bluetooth Network Connection");
        assert_eq!(stat(&f, "Status"), "Disconnected");
        assert_eq!(stat(&f, "Link speed"), "—");
    }

    #[test]
    fn a_gpu_page_shows_engines_memory_and_temperature() {
        let f = devices_build("gpu:luid_b");
        assert_eq!((f.title.as_str(), f.subtitle.as_str()), ("GPU 0", "NVIDIA GeForce RTX 4090"));
        assert_eq!(f.graphs.iter().map(|g| g.label.as_str()).collect::<Vec<_>>(), ["Utilization", "Dedicated GPU memory", "Shared GPU memory"]);
        assert_eq!(f.cells.iter().map(|c| c.label.as_str()).collect::<Vec<_>>(), ["3D", "Copy"]);
        assert_eq!(stat(&f, "Temperature"), "64 °C");
        assert_eq!(stat(&f, "Dedicated GPU memory"), "3.0 / 24.0 GB");
        assert_eq!(f.scrub_metric, Metric::Gpu);
        assert_eq!(f.scrub_series, Some(Series::GpuUtil("luid_b".into())));
        let igpu = devices_build("gpu:luid_a");
        assert_eq!(igpu.title, "GPU 1");
        assert_eq!(stat(&igpu, "Temperature"), "—", "no sensor: a dash, not 0 °C");
    }

    #[test]
    fn engine_graphs_are_ordered_3d_copy_video_then_by_name() {
        let mut s = devices_snapshot();
        let g = &mut s.totals.gpus.as_mut().unwrap()[1];
        g.engines = ["Zeta", "VideoDecode", "Compute_0", "Copy", "VideoEncode", "3D"].iter().map(|n| (n.to_string(), 1.0)).collect();
        let f = build_with(&s, &devices_info(), "gpu:luid_b", Range::Minute);
        assert_eq!(f.cells.iter().map(|c| c.label.as_str()).collect::<Vec<_>>(), ["3D", "Copy", "VideoEncode", "VideoDecode", "Compute_0", "Zeta"]);
    }

    #[test]
    fn a_gpu_seeded_without_counters_shows_dashes_never_zero_or_nan() {
        let mut s = devices_snapshot();
        s.totals.gpus = Some(vec![GpuSample { key: "luid_b".into(), util_pct: f32::NAN, ..Default::default() }]);
        let f = build_with(&s, &devices_info(), "gpu:luid_b", Range::Minute);
        let row = f.devices.iter().find(|d| d.id == "gpu:luid_b").unwrap();
        assert_eq!(row.value, "—");
        assert_eq!(row.spark, EMPTY_PATH, "a NaN sample is a gap, not a graph point");
        assert_eq!(f.graphs[0].value, "—");
        assert_eq!(f.graphs[0].line, EMPTY_PATH);
        assert_eq!(stat(&f, "Utilization"), "—");
        assert_eq!(stat(&f, "Dedicated GPU memory"), "—");
        assert!(f.cells.is_empty(), "no engines");
        let mut engine_nan = devices_snapshot();
        engine_nan.totals.gpus.as_mut().unwrap()[1].engines = vec![("3D".into(), f32::NAN)];
        let f = build_with(&engine_nan, &devices_info(), "gpu:luid_b", Range::Minute);
        assert_eq!(f.cells[0].value, "—");
    }

    #[test]
    fn a_page_whose_device_vanished_falls_back_to_cpu() {
        for id in ["disk:0 C:", "net:Intel[R] Ethernet", "net:Bluetooth Device [Personal Area Network]", "gpu:luid_b"] {
            assert_eq!(devices_build(id).selected, id, "present: {id}");
            let mut s = devices_snapshot();
            s.totals.disks = Some(Vec::new());
            s.totals.adapters = None;
            s.totals.gpus = None;
            let f = build_with(&s, &devices_info(), id, Range::Minute);
            assert_eq!((f.selected.as_str(), f.requested.as_str(), f.title.as_str()), ("cpu", id, "CPU"), "gone: {id}");
            assert_eq!(f.devices.len(), 2);
        }
        assert_eq!(devices_build("disk:2 F:").selected, "cpu");
    }

    /// Builds the page for `device` at each snapshot in turn, tracking it the way the worker does.
    fn build_over(snaps: &[Snapshot], device: &str) -> PerfFrame {
        let (mut h, info, mut seen) = (History::new(), devices_info(), None);
        let mut last = None;
        for s in snaps {
            h.push(s.taken_at_ms, &s.totals, &[]);
            seen = LastSeen::track(seen, s, device);
            last = Some(build_perf(&PerfInput { snap: s, history: &h, info: &info, device, range: Range::Minute, last_seen: seen.as_ref() }));
        }
        last.expect("at least one snapshot")
    }

    fn without_devices(t_ms: u64) -> Snapshot {
        let mut s = devices_snapshot();
        s.taken_at_ms = t_ms;
        s.totals.disks = Some(Vec::new());
        s.totals.adapters = None;
        s.totals.gpus = None;
        s
    }

    fn with_devices(t_ms: u64) -> Snapshot {
        let mut s = devices_snapshot();
        s.taken_at_ms = t_ms;
        s
    }

    #[test]
    fn a_device_missing_for_a_tick_keeps_its_page_with_unknown_values_and_its_history() {
        for id in ["disk:0 C:", "net:Intel[R] Ethernet", "gpu:luid_b"] {
            let f = build_over(&[with_devices(58_000), with_devices(59_000), without_devices(60_000)], id);
            assert_eq!((f.selected.as_str(), f.requested.as_str()), (id, id), "kept through one missed tick: {id}");
            let row = f.devices.iter().find(|d| d.id == id).expect("still listed");
            assert_eq!(row.value, "—", "{id}");
            assert!(f.graphs.iter().all(|g| g.value == "—"), "{id}: {:?}", f.graphs);
            assert!(f.stats.iter().all(|s| s.value == "—"), "{id}: {:?}", f.stats);
            assert!(f.cells.iter().all(|c| c.value == "—"), "{id}");
            assert_ne!(f.graphs[0].line, EMPTY_PATH, "{id}: its history is still drawn");
        }
        let f = build_over(&[with_devices(58_000), without_devices(60_000)], "disk:0 C:");
        assert_eq!(f.title, "Disk 0 (C:)");
        let ids: Vec<&str> = f.devices.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, ["cpu", "memory", "disk:0 C:"], "listed where its kind goes");
    }

    #[test]
    fn a_device_gone_for_longer_falls_back_to_cpu() {
        for id in ["disk:0 C:", "net:Intel[R] Ethernet", "gpu:luid_b"] {
            let gone: Vec<Snapshot> = (60..=66).map(|s| without_devices(s * 1_000)).collect();
            let f = build_over(&[&[with_devices(59_000)][..], &gone].concat(), id);
            assert_eq!(f.selected, "cpu", "gone for 7 s: {id}");
            let back = build_over(&[&[with_devices(59_000)][..], &gone, &[with_devices(67_000)]].concat(), id);
            assert_eq!(back.selected, id, "and shown again once it is back: {id}");
        }
        assert_eq!(build_over(&[without_devices(60_000)], "disk:0 C:").selected, "cpu", "never seen: no grace");
    }

    #[test]
    fn memory_numbers_come_from_one_source_and_add_up() {
        let s = snapshot(60_000);
        let m = s.totals.memory.as_ref().unwrap();
        assert_eq!(m.in_use() + m.available(), m.total);
        let f = build(&s, "memory");
        assert_eq!(f.graphs[0].value, "7.0 GB");
        assert_eq!(f.devices[1].value, "7.0 / 16.0 GB (44%)");
        assert_eq!(stat(&f, "In use"), "7.0 GB");
        assert_eq!(stat(&f, "Available"), "9.0 GB");
        // Page lists that disagree with the OS's `used`: every number still follows the lists.
        let mut s = snapshot(60_000);
        s.totals.memory.as_mut().unwrap().used = 8 << 30;
        let f = build(&s, "memory");
        assert_eq!((f.graphs[0].value.as_str(), stat(&f, "In use")), ("7.0 GB", "7.0 GB"));
        assert_eq!(f.devices[1].value, "7.0 / 16.0 GB (44%)");
        let bar: f32 = f.segments.iter().map(|s| s.fraction).sum();
        assert!((bar - 1.0).abs() < 1e-4);
    }

    #[test]
    fn unavailable_memory_draws_nothing() {
        let mut h = History::new();
        let s = snapshot(1_000);
        h.push(1_000, &s.totals, &[]);
        let mut gone = snapshot(2_000);
        gone.totals.memory = None;
        h.push(2_000, &gone.totals, &[]);
        let info = info();
        let f = build_perf(&PerfInput { snap: &gone, history: &h, info: &info, device: "memory", range: Range::Minute, last_seen: None });
        assert_eq!((f.graphs[0].line.as_str(), f.graphs[0].area.as_str()), (EMPTY_PATH, EMPTY_PATH));
        assert_eq!(f.devices[1].spark, EMPTY_PATH);
        assert_eq!(f.devices[1].value, "—");
    }

    #[test]
    fn zero_uptime_and_zero_processors_are_unknown() {
        let mut s = snapshot(60_000);
        s.totals.uptime_s = 0;
        s.totals.logical_cpus = 0;
        s.totals.per_core_pct.clear();
        let f = build(&s, "cpu");
        assert_eq!(stat(&f, "Up time"), "—");
        assert_eq!(stat(&f, "Logical processors"), "—");
        assert!(f.cells.is_empty());
    }

    #[test]
    fn max_over_ignores_gaps_and_values_outside_the_span() {
        let mut h = History::new();
        for (t, v) in [(1_000u64, 9.0f32), (50_000, 3.0), (60_000, f32::NAN)] {
            let mut s = snapshot(t);
            s.totals.gpus = Some(vec![GpuSample { key: "k".into(), util_pct: v, ..Default::default() }]);
            h.push(t, &s.totals, &[]);
        }
        let s = snapshot(60_000);
        let info = info();
        let input = PerfInput { snap: &s, history: &h, info: &info, device: "cpu", range: Range::Minute, last_seen: None };
        assert_eq!(input.max_over(&Series::GpuUtil("k".into()), 30_000), 3.0);
        assert_eq!(input.max_over(&Series::GpuUtil("none".into()), 30_000), 0.0);
    }

    /// While graphs scroll, the view trails the newest sample and shows history from before the
    /// window's left edge (graph::LEAD_MS): a peak there must fit under the scale too.
    #[test]
    fn max_over_takes_in_the_history_a_scrolling_view_shows() {
        let mut h = History::new();
        let (end, span) = (60_000u64, 30_000u64);
        for (t, v) in [(end - span - 5_000, 9.0f32), (end - span + 1_000, 3.0), (end, 1.0)] {
            let mut s = snapshot(t);
            s.totals.gpus = Some(vec![GpuSample { key: "k".into(), util_pct: v, ..Default::default() }]);
            h.push(t, &s.totals, &[]);
        }
        let s = snapshot(end);
        let info = info();
        let input = PerfInput { snap: &s, history: &h, info: &info, device: "cpu", range: Range::Minute, last_seen: None };
        assert_eq!(input.max_over(&Series::GpuUtil("k".into()), span), 9.0, "the peak 5 s before the window is in view while scrolling");
    }

    /// A path's x coordinates.
    fn path_xs(path: &str) -> Vec<f64> {
        let t: Vec<&str> = path.split_whitespace().collect();
        t.chunks(3).filter(|c| c.len() == 3).map(|c| c[1].parse().unwrap()).collect()
    }

    #[test]
    fn graphs_reach_both_edges_when_the_tick_is_newer_than_the_newest_sample() {
        // At a 0.5 s refresh the history keeps one sample a second (ruling P3-3): every other tick
        // is half a second newer than the newest sample.
        let mut h = History::new();
        for i in 0..=60u64 {
            h.push(i * 1_000, &snapshot(i * 1_000).totals, &[]);
        }
        let s = snapshot(60_500);
        assert!(!h.push(s.taken_at_ms, &s.totals, &[]), "too soon after the last sample: not kept");
        let info = info();
        let f = build_perf(&PerfInput { snap: &s, history: &h, info: &info, device: "cpu", range: Range::Minute, last_seen: None });
        let main = path_xs(&f.graphs[0].line);
        assert_eq!(main.last(), Some(&1000.0), "the line ends on the right edge: {}", f.graphs[0].line);
        assert!(main[0] <= 0.0, "and starts on the left edge: {}", &f.graphs[0].line[..30]);
        assert_eq!(path_xs(&f.cells[0].line).last(), Some(&1000.0), "{}", f.cells[0].line);
        assert_eq!(path_xs(&f.devices[0].spark).last(), Some(&1000.0), "{}", f.devices[0].spark);
        assert_eq!(f.window_end_ms, 60_000, "the scrub maps the right edge to the newest sample");
        assert_eq!(f.end_clock_ms, 12 * 3_600_000 - 500, "at that sample's clock");
    }
}
