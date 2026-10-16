//! In-memory history (spec §4.4): one hour of system totals, per-device series and the top apps
//! per metric. At most one sample per second is kept whatever the refresh rate (ruling P3-3),
//! and samples older than an hour are dropped. Every column is preallocated to MAX_SAMPLES and
//! reused, so steady-state pushes don't grow memory (spec §8). App keys are interned to small
//! ids so an hour fits in about 2 MB. Recording looks columns up by borrowed name: a key string
//! is allocated only the first time a device is seen (ruling PF12).
use negotium_model::{AppKey, AppRow, Metric, top_n};
use negotium_snapshot::SystemTotals;
use std::collections::{HashMap, VecDeque};
use std::mem::size_of;

pub const WINDOW_MS: u64 = 3_600_000;
pub const MIN_SPACING_MS: u64 = 950;
pub const MAX_SAMPLES: usize = (WINDOW_MS / 1000) as usize + 1;
pub const TOP_PER_METRIC: usize = 10;

/// Every app maps to this id once 65 535 distinct apps are live in the top lists (only under
/// pathological churn), so the id table stays bounded.
const OVERFLOW_ID: u16 = u16::MAX;
const OVERFLOW_NAME: &str = "Other apps";
const PCT_MISSING: u8 = u8::MAX;

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Series {
    Cpu,
    /// Logical processor ordinal.
    Core(u16),
    MemUsed,
    Committed,
    DiskBusiest,
    Gpu,
    NetTotal,
    DiskActive(String),
    DiskRead(String),
    DiskWrite(String),
    DiskResponse(String),
    NetSent(String),
    NetRecv(String),
    GpuUtil(String),
    /// (adapter key, engine type)
    GpuEngine(String, String),
    GpuDedicated(String),
    GpuShared(String),
    GpuTemp(String),
}

impl Series {
    /// Percent series are kept as whole percents in one byte each.
    pub fn is_pct(&self) -> bool {
        matches!(
            self,
            Series::Cpu
                | Series::Core(_)
                | Series::DiskBusiest
                | Series::Gpu
                | Series::DiskActive(_)
                | Series::GpuUtil(_)
                | Series::GpuEngine(..)
        )
    }
}

enum Data {
    Pct(VecDeque<u8>),
    Value(VecDeque<f32>),
}

struct Column {
    data: Data,
    /// Values present (not missing) among those retained.
    present: usize,
    /// The push that last wrote this column.
    generation: u64,
}

impl Column {
    /// A column for a series first seen now: `missing` gaps stand for the samples before it.
    fn new(pct: bool, missing: usize) -> Self {
        let data = if pct {
            let mut d = VecDeque::with_capacity(MAX_SAMPLES);
            d.resize(missing, PCT_MISSING);
            Data::Pct(d)
        } else {
            let mut d = VecDeque::with_capacity(MAX_SAMPLES);
            d.resize(missing, f32::NAN);
            Data::Value(d)
        };
        Self { data, present: 0, generation: 0 }
    }

    /// Unknown (None, NaN, infinite) is stored as missing, never as a number.
    fn push(&mut self, v: Option<f32>) {
        let v = v.filter(|v| v.is_finite());
        if v.is_some() {
            self.present += 1;
        }
        match &mut self.data {
            Data::Pct(d) => d.push_back(v.map_or(PCT_MISSING, |v| v.clamp(0.0, 100.0).round() as u8)),
            Data::Value(d) => d.push_back(v.unwrap_or(f32::NAN)),
        }
    }

    fn pop_front(&mut self) {
        let was_present = match &mut self.data {
            Data::Pct(d) => d.pop_front().is_some_and(|v| v != PCT_MISSING),
            Data::Value(d) => d.pop_front().is_some_and(|v| !v.is_nan()),
        };
        if was_present {
            self.present -= 1;
        }
    }

    fn get(&self, i: usize) -> Option<f32> {
        match &self.data {
            Data::Pct(d) => d.get(i).copied().filter(|&v| v != PCT_MISSING).map(f32::from),
            Data::Value(d) => d.get(i).copied().filter(|v| !v.is_nan()),
        }
    }

    fn bytes(&self) -> usize {
        size_of::<Self>()
            + match &self.data {
                Data::Pct(d) => d.capacity(),
                Data::Value(d) => d.capacity() * size_of::<f32>(),
            }
    }

    fn stale(&self, generation: u64) -> bool {
        self.present == 0 && self.generation != generation
    }
}

// Fixed series slots.
const F_CPU: usize = 0;
const F_MEM: usize = 1;
const F_COMMIT: usize = 2;
const F_DISK: usize = 3;
const F_GPU: usize = 4;
const F_NET: usize = 5;
const FIXED: usize = 6;
const FIXED_PCT: [bool; FIXED] = [true, false, false, true, true, false];

// Per-device series kinds, each a map from device name to column.
const N_DISK_ACTIVE: usize = 0;
const N_DISK_READ: usize = 1;
const N_DISK_WRITE: usize = 2;
const N_DISK_RESPONSE: usize = 3;
const N_NET_SENT: usize = 4;
const N_NET_RECV: usize = 5;
const N_GPU_UTIL: usize = 6;
const N_GPU_DEDICATED: usize = 7;
const N_GPU_SHARED: usize = 8;
const N_GPU_TEMP: usize = 9;
const NAMED: usize = 10;
const NAMED_PCT: [bool; NAMED] = [true, false, false, false, false, false, true, false, false, false];

/// All columns, grouped so a lookup can borrow the device name instead of building a `Series`.
#[derive(Default)]
struct Columns {
    fixed: [Option<Column>; FIXED],
    /// Indexed by logical processor ordinal.
    cores: Vec<Option<Column>>,
    named: [HashMap<String, Column>; NAMED],
    /// GPU key -> engine type -> column.
    engines: HashMap<String, HashMap<String, Column>>,
}

/// The column named `name` in `map`, created (with `missing` leading gaps) the first time only.
fn named_slot<'a>(map: &'a mut HashMap<String, Column>, name: &str, pct: bool, missing: usize) -> &'a mut Column {
    if !map.contains_key(name) {
        map.insert(name.to_owned(), Column::new(pct, missing));
    }
    map.get_mut(name).expect("inserted above")
}

impl Columns {
    fn get(&self, key: &Series) -> Option<&Column> {
        let named = |k: usize, n: &String| self.named[k].get(n.as_str());
        match key {
            Series::Cpu => self.fixed[F_CPU].as_ref(),
            Series::Core(i) => self.cores.get(*i as usize)?.as_ref(),
            Series::MemUsed => self.fixed[F_MEM].as_ref(),
            Series::Committed => self.fixed[F_COMMIT].as_ref(),
            Series::DiskBusiest => self.fixed[F_DISK].as_ref(),
            Series::Gpu => self.fixed[F_GPU].as_ref(),
            Series::NetTotal => self.fixed[F_NET].as_ref(),
            Series::DiskActive(n) => named(N_DISK_ACTIVE, n),
            Series::DiskRead(n) => named(N_DISK_READ, n),
            Series::DiskWrite(n) => named(N_DISK_WRITE, n),
            Series::DiskResponse(n) => named(N_DISK_RESPONSE, n),
            Series::NetSent(n) => named(N_NET_SENT, n),
            Series::NetRecv(n) => named(N_NET_RECV, n),
            Series::GpuUtil(n) => named(N_GPU_UTIL, n),
            Series::GpuEngine(g, e) => self.engines.get(g.as_str())?.get(e.as_str()),
            Series::GpuDedicated(n) => named(N_GPU_DEDICATED, n),
            Series::GpuShared(n) => named(N_GPU_SHARED, n),
            Series::GpuTemp(n) => named(N_GPU_TEMP, n),
        }
    }

    fn for_each_mut(&mut self, f: &mut dyn FnMut(&mut Column)) {
        self.fixed.iter_mut().flatten().for_each(&mut *f);
        self.cores.iter_mut().flatten().for_each(&mut *f);
        for map in &mut self.named {
            map.values_mut().for_each(&mut *f);
        }
        for map in self.engines.values_mut() {
            map.values_mut().for_each(&mut *f);
        }
    }

    /// Calls `f(column, bytes held by its key)` for every column.
    fn for_each(&self, f: &mut dyn FnMut(&Column, usize)) {
        self.fixed.iter().flatten().for_each(|c| f(c, 0));
        self.cores.iter().flatten().for_each(|c| f(c, 0));
        let key_bytes = |k: &String| k.capacity() + size_of::<String>() + 8;
        for map in &self.named {
            map.iter().for_each(|(k, c)| f(c, key_bytes(k)));
        }
        for (g, map) in &self.engines {
            map.iter().for_each(|(e, c)| f(c, key_bytes(g) + key_bytes(e)));
        }
    }

    /// Drops every column whose last value has aged out (and that was not written this push).
    fn drop_stale(&mut self, generation: u64) {
        for c in self.fixed.iter_mut().chain(self.cores.iter_mut()) {
            if c.as_ref().is_some_and(|c| c.stale(generation)) {
                *c = None;
            }
        }
        while self.cores.last().is_some_and(|c| c.is_none()) {
            self.cores.pop();
        }
        for map in &mut self.named {
            map.retain(|_, c| !c.stale(generation));
        }
        for map in self.engines.values_mut() {
            map.retain(|_, c| !c.stale(generation));
        }
        self.engines.retain(|_, m| !m.is_empty());
    }
}

#[derive(Clone, Copy)]
struct Top {
    ids: [u16; TOP_PER_METRIC],
    values: [f32; TOP_PER_METRIC],
    len: u8,
}

impl Default for Top {
    fn default() -> Self {
        Self { ids: [0; TOP_PER_METRIC], values: [0.0; TOP_PER_METRIC], len: 0 }
    }
}

pub struct History {
    times: VecDeque<u64>,
    /// `tops[i][metric as usize]`, aligned with `times`.
    tops: VecDeque<[Top; 5]>,
    columns: Columns,
    generation: u64,
    ids: HashMap<AppKey, u16>,
    names: Vec<String>,
    /// id -> key; `None` when the slot is free.
    keys: Vec<Option<AppKey>>,
    /// id -> number of top-list entries in retained samples that reference it.
    refs: Vec<u32>,
    free: Vec<u16>,
}

impl Default for History {
    fn default() -> Self {
        Self::new()
    }
}

impl History {
    pub fn new() -> Self {
        Self {
            times: VecDeque::with_capacity(MAX_SAMPLES),
            tops: VecDeque::with_capacity(MAX_SAMPLES),
            columns: Columns::default(),
            generation: 0,
            ids: HashMap::new(),
            names: Vec::new(),
            keys: Vec::new(),
            refs: Vec::new(),
            free: Vec::new(),
        }
    }

    /// Records one sample. Returns false (and records nothing) when it comes less than
    /// MIN_SPACING_MS after the last kept sample. A sample older than the last one is a timeline
    /// break (collector time restarted): the history is cleared and starts again from it, rather
    /// than refusing every sample until time catches up.
    pub fn push(&mut self, t_ms: u64, totals: &SystemTotals, rows: &[AppRow]) -> bool {
        if let Some(&last) = self.times.back() {
            if t_ms < last {
                *self = Self::new();
            } else if t_ms < last.saturating_add(MIN_SPACING_MS) {
                return false;
            }
        }
        let oldest = t_ms.saturating_sub(WINDOW_MS);
        while self.times.front().is_some_and(|&t| t < oldest) || self.times.len() >= MAX_SAMPLES {
            self.evict();
        }
        self.generation += 1;
        self.record_totals(totals);
        let generation = self.generation;
        self.columns.for_each_mut(&mut |col| {
            if col.generation != generation {
                col.push(None);
            }
        });
        // A device that left is dropped once its last value has aged out.
        self.columns.drop_stale(generation);

        let mut tops = [Top::default(); 5];
        for metric in Metric::ALL {
            let list = top_n(rows, metric, TOP_PER_METRIC);
            for (slot, &i) in list.iter().enumerate() {
                let id = self.intern(&rows[i]);
                let top = &mut tops[metric as usize];
                top.ids[slot] = id;
                top.values[slot] = rows[i].value(metric) as f32;
            }
            tops[metric as usize].len = list.len() as u8;
        }
        self.times.push_back(t_ms);
        self.tops.push_back(tops);
        true
    }

    /// Writes a value into a column once per push (a duplicate device name this tick: the first
    /// one wins, so columns stay aligned).
    fn write(col: &mut Column, generation: u64, v: Option<f32>) {
        if col.generation != generation {
            col.generation = generation;
            col.push(v);
        }
    }

    fn record_fixed(&mut self, slot: usize, v: Option<f32>) {
        let (missing, generation) = (self.times.len(), self.generation);
        let col = self.columns.fixed[slot].get_or_insert_with(|| Column::new(FIXED_PCT[slot], missing));
        Self::write(col, generation, v);
    }

    fn record_named(&mut self, kind: usize, name: &str, v: Option<f32>) {
        let (missing, generation) = (self.times.len(), self.generation);
        let col = named_slot(&mut self.columns.named[kind], name, NAMED_PCT[kind], missing);
        Self::write(col, generation, v);
    }

    fn record_totals(&mut self, t: &SystemTotals) {
        let (missing, generation) = (self.times.len(), self.generation);
        self.record_fixed(F_CPU, t.cpu_pct);
        for (i, v) in t.per_core_pct.iter().enumerate() {
            if self.columns.cores.len() <= i {
                self.columns.cores.resize_with(i + 1, || None);
            }
            let col = self.columns.cores[i].get_or_insert_with(|| Column::new(true, missing));
            Self::write(col, generation, *v);
        }
        self.record_fixed(F_MEM, t.memory.as_ref().map(|m| m.in_use() as f32));
        self.record_fixed(F_COMMIT, t.memory.as_ref().map(|m| m.commit_total as f32));
        self.record_fixed(F_DISK, t.busiest_disk_pct());
        self.record_fixed(F_GPU, t.gpu_pct);
        self.record_fixed(F_NET, t.net_bps.map(|v| v as f32));
        for d in t.disks.iter().flatten() {
            self.record_named(N_DISK_ACTIVE, &d.name, Some(d.active_pct));
            self.record_named(N_DISK_READ, &d.name, Some(d.read_bps as f32));
            self.record_named(N_DISK_WRITE, &d.name, Some(d.write_bps as f32));
            self.record_named(N_DISK_RESPONSE, &d.name, d.response_ms);
        }
        for a in t.adapters.iter().flatten() {
            self.record_named(N_NET_SENT, &a.name, Some(a.sent_bps as f32));
            self.record_named(N_NET_RECV, &a.name, Some(a.recv_bps as f32));
        }
        for g in t.gpus.iter().flatten() {
            // util_pct is NaN when unknown: Column::push stores that as missing.
            self.record_named(N_GPU_UTIL, &g.key, Some(g.util_pct));
            if !g.engines.is_empty() && !self.columns.engines.contains_key(g.key.as_str()) {
                self.columns.engines.insert(g.key.clone(), HashMap::new());
            }
            for (engtype, v) in &g.engines {
                let map = self.columns.engines.get_mut(g.key.as_str()).expect("inserted above");
                let col = named_slot(map, engtype, true, missing);
                Self::write(col, generation, Some(*v));
            }
            self.record_named(N_GPU_DEDICATED, &g.key, g.dedicated_used.map(|b| b as f32));
            self.record_named(N_GPU_SHARED, &g.key, g.shared_used.map(|b| b as f32));
            self.record_named(N_GPU_TEMP, &g.key, g.temp_c);
        }
    }

    fn evict(&mut self) {
        self.times.pop_front();
        if let Some(tops) = self.tops.pop_front() {
            for top in &tops {
                for &id in &top.ids[..top.len as usize] {
                    self.release(id);
                }
            }
        }
        self.columns.for_each_mut(&mut |col| col.pop_front());
    }

    fn intern(&mut self, row: &AppRow) -> u16 {
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
            None if self.names.len() < OVERFLOW_ID as usize => {
                self.names.push(String::new());
                self.keys.push(None);
                self.refs.push(0);
                (self.names.len() - 1) as u16
            }
            None => return OVERFLOW_ID,
        };
        let i = id as usize;
        self.names[i] = row.name.clone();
        self.keys[i] = Some(row.key.clone());
        self.refs[i] = 1;
        self.ids.insert(row.key.clone(), id);
        id
    }

    /// Drops one reference held by an evicted sample; an id no longer referenced is freed.
    fn release(&mut self, id: u16) {
        if id == OVERFLOW_ID {
            return;
        }
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

    fn name(&self, id: u16) -> &str {
        if id == OVERFLOW_ID { OVERFLOW_NAME } else { &self.names[id as usize] }
    }

    pub fn len(&self) -> usize {
        self.times.len()
    }

    pub fn is_empty(&self) -> bool {
        self.times.is_empty()
    }

    pub fn latest_t(&self) -> Option<u64> {
        self.times.back().copied()
    }

    pub fn has(&self, key: &Series) -> bool {
        self.columns.get(key).is_some()
    }

    /// (time, value) of every retained sample at or after `since_ms`; `None` where the series had
    /// no value (or is unknown).
    pub fn points<'a>(&'a self, key: &Series, since_ms: u64) -> impl Iterator<Item = (u64, Option<f32>)> + use<'a> {
        let col = self.columns.get(key);
        let start = self.times.partition_point(|&t| t < since_ms);
        (start..self.times.len()).map(move |i| (self.times[i], col.and_then(|c| c.get(i))))
    }

    /// Index of the sample nearest `t_ms` (the earlier one on a tie).
    fn nearest(&self, t_ms: u64) -> Option<usize> {
        let i = self.times.partition_point(|&t| t < t_ms);
        match (i.checked_sub(1), (i < self.times.len()).then_some(i)) {
            (Some(a), Some(b)) => Some(if t_ms - self.times[a] <= self.times[b] - t_ms { a } else { b }),
            (Some(a), None) => Some(a),
            (None, b) => b,
        }
    }

    pub fn top_at(&self, t_ms: u64, metric: Metric, max_gap_ms: u64) -> Option<(u64, Vec<(String, f32)>)> {
        let i = self.nearest(t_ms)?;
        if self.times[i].abs_diff(t_ms) > max_gap_ms {
            return None;
        }
        Some(self.top_of(i, metric))
    }

    /// Among the samples from `from_ms` to `to_ms` (inclusive), the one where `key` peaks (the
    /// earliest on a tie; the newest when there is no key or it has no value in any of them), with
    /// its top apps for `metric`. `None` when no sample lies in the range.
    pub fn top_at_max(&self, key: Option<&Series>, from_ms: u64, to_ms: u64, metric: Metric) -> Option<(u64, Vec<(String, f32)>)> {
        let (start, end) = (self.times.partition_point(|&t| t < from_ms), self.times.partition_point(|&t| t <= to_ms));
        if start >= end {
            return None;
        }
        let col = key.and_then(|k| self.columns.get(k));
        let mut best: Option<(usize, f32)> = None;
        for i in start..end {
            if let Some(v) = col.and_then(|c| c.get(i)).filter(|v| v.is_finite())
                && best.is_none_or(|(_, b)| v > b)
            {
                best = Some((i, v));
            }
        }
        Some(self.top_of(best.map_or(end - 1, |(i, _)| i), metric))
    }

    fn top_of(&self, i: usize, metric: Metric) -> (u64, Vec<(String, f32)>) {
        let top = &self.tops[i][metric as usize];
        (self.times[i], (0..top.len as usize).map(|k| (self.name(top.ids[k]).to_string(), top.values[k])).collect())
    }

    pub fn approx_bytes(&self) -> usize {
        let samples = self.times.capacity() * size_of::<u64>() + self.tops.capacity() * size_of::<[Top; 5]>();
        let mut columns = self.columns.cores.capacity() * size_of::<Option<Column>>();
        self.columns.for_each(&mut |c, key_bytes| columns += c.bytes() + key_bytes);
        let names: usize = self.names.iter().map(|n| n.capacity() + size_of::<String>()).sum::<usize>()
            + self.keys.capacity() * size_of::<Option<AppKey>>()
            + self.refs.capacity() * size_of::<u32>()
            + self.ids.capacity() * (size_of::<AppKey>() + size_of::<u16>() + 8);
        samples + columns + names
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use negotium_model::{AppKey, AppKind};
    use negotium_snapshot::{AdapterSample, DiskSample, GpuSample, MemoryTotals};

    fn row(pid: u32, name: &str, v: f32) -> AppRow {
        AppRow {
            key: AppKey::Background { pid, create_time: 0 }, name: name.into(), kind: AppKind::Background,
            processes: vec![], cpu_pct: v, private_bytes: (v * 1e6) as u64, disk_bps: v as f64 * 1e3,
            gpu_pct: Some(v), net_bps: Some(v as f64), hung_for_ms: None,
        }
    }
    fn totals(cpu: f32, cores: usize) -> SystemTotals {
        SystemTotals {
            logical_cpus: cores as u32, cpu_pct: Some(cpu), per_core_pct: vec![Some(cpu); cores],
            memory: Some(MemoryTotals { total: 32 << 30, used: 8 << 30, commit_total: 12 << 30, ..Default::default() }),
            ..Default::default()
        }
    }
    fn disk(name: &str, active: f32) -> DiskSample {
        DiskSample { name: name.into(), active_pct: active, read_bps: 1e6, write_bps: 2e6, response_ms: Some(0.5) }
    }
    /// A "typical machine" sample: 2 disks, 2 adapters, 1 GPU with 8 engine types.
    fn full(cores: usize) -> SystemTotals {
        let mut t = totals(50.0, cores);
        t.disks = Some(vec![disk("0 C:", 10.0), disk("1 D:", 20.0)]);
        t.net_bps = Some(1e6);
        t.adapters = Some(["Ethernet", "Wi-Fi"].map(|n| AdapterSample { name: n.into(), sent_bps: 1e5, recv_bps: 1e6, ..Default::default() }).to_vec());
        t.gpu_pct = Some(30.0);
        t.gpus = Some(vec![GpuSample {
            key: "luid_0x0_0x1_phys_0".into(), util_pct: 30.0,
            engines: (0..8).map(|i| (format!("type{i}"), i as f32)).collect(),
            dedicated_used: Some(1 << 30), shared_used: Some(1 << 28), temp_c: Some(60.0),
        }]);
        t
    }
    fn values(h: &History, key: &Series) -> Vec<Option<f32>> {
        h.points(key, 0).map(|(_, v)| v).collect()
    }

    #[test]
    fn at_most_one_sample_per_second_is_kept() {
        let mut h = History::new();
        assert!(h.push(0, &totals(1.0, 1), &[]));
        assert!(!h.push(500, &totals(2.0, 1), &[]), "a 0.5 s tick between two kept samples");
        assert!(h.push(1_000, &totals(3.0, 1), &[]));
        assert!(!h.push(1_000, &totals(4.0, 1), &[]), "an extra tick from a UI poke");
        assert_eq!(values(&h, &Series::Cpu), [Some(1.0), Some(3.0)]);
    }

    #[test]
    fn time_going_backwards_starts_a_new_timeline_instead_of_refusing_samples() {
        let mut h = History::new();
        for s in 100..=110u64 {
            assert!(h.push(s * 1_000, &totals(1.0, 1), &[row(1, "old", 9.0)]));
        }
        // A rebuilt collector without the shared epoch would restart near 0.
        assert!(h.push(2_000, &totals(7.0, 1), &[row(2, "new", 5.0)]), "the sample is recorded");
        assert_eq!(h.len(), 1, "the old timeline is cleared");
        assert_eq!(h.latest_t(), Some(2_000));
        assert_eq!(values(&h, &Series::Cpu), [Some(7.0)]);
        assert_eq!(h.top_at(105_000, Metric::Cpu, 2_500), None, "no apps from the old timeline");
        assert_eq!(h.top_at(2_000, Metric::Cpu, 2_500).unwrap().1, vec![("new".to_string(), 5.0)]);
        assert!(h.push(3_000, &totals(8.0, 1), &[]), "the new timeline goes on normally");
    }

    #[test]
    fn samples_older_than_an_hour_are_dropped() {
        let mut h = History::new();
        for t in 0..10u64 {
            h.push(t * 1_000, &totals(1.0, 1), &[]);
        }
        h.push(WINDOW_MS + 5_000, &totals(1.0, 1), &[]);
        assert_eq!(h.points(&Series::Cpu, 0).next().unwrap().0, 5_000, "everything before now - 1 h is gone");
        assert_eq!(h.latest_t(), Some(WINDOW_MS + 5_000));
    }

    #[test]
    fn never_more_than_max_samples() {
        let mut h = History::new();
        for t in 0..(MAX_SAMPLES as u64 + 50) {
            h.push(t * 1_000, &totals(1.0, 1), &[]);
        }
        assert_eq!(h.len(), MAX_SAMPLES);
    }

    #[test]
    fn a_device_that_appears_late_has_gaps_before_it_and_one_that_leaves_ages_out() {
        let mut h = History::new();
        h.push(0, &totals(1.0, 1), &[]);
        let mut with_usb = totals(1.0, 1);
        with_usb.disks = Some(vec![disk("0 C:", 5.0), disk("2 F:", 50.0)]);
        h.push(1_000, &with_usb, &[]);
        let mut without = totals(1.0, 1);
        without.disks = Some(vec![disk("0 C:", 6.0)]);
        h.push(2_000, &without, &[]);
        let usb = Series::DiskActive("2 F:".into());
        assert_eq!(values(&h, &usb), [None, Some(50.0), None]);
        assert!(h.has(&usb));
        // An hour later its last value has aged out: the column is gone.
        h.push(WINDOW_MS + 1_500, &without, &[]);
        assert!(!h.has(&usb));
        assert!(h.has(&Series::DiskActive("0 C:".into())));
    }

    #[test]
    fn a_core_without_data_stays_in_its_slot() {
        let mut h = History::new();
        let mut t = totals(10.0, 3);
        t.per_core_pct = vec![Some(10.0), None, Some(30.4)];
        h.push(0, &t, &[]);
        assert_eq!(values(&h, &Series::Core(1)), [None]);
        assert_eq!(values(&h, &Series::Core(2)), [Some(30.0)], "percent series keep whole percents");
    }

    #[test]
    fn device_series_are_recorded_for_every_kind() {
        let mut h = History::new();
        h.push(0, &full(2), &[]);
        assert_eq!(values(&h, &Series::DiskRead("1 D:".into())), [Some(1e6)]);
        assert_eq!(values(&h, &Series::DiskResponse("0 C:".into())), [Some(0.5)]);
        assert_eq!(values(&h, &Series::NetRecv("Wi-Fi".into())), [Some(1e6)]);
        assert_eq!(values(&h, &Series::GpuEngine("luid_0x0_0x1_phys_0".into(), "type7".into())), [Some(7.0)]);
        assert_eq!(values(&h, &Series::GpuTemp("luid_0x0_0x1_phys_0".into())), [Some(60.0)]);
        assert_eq!(values(&h, &Series::Committed), [Some((12u64 << 30) as f32)]);
        assert_eq!(values(&h, &Series::NetTotal), [Some(1e6)]);
        assert_eq!(values(&h, &Series::Gpu), [Some(30.0)]);
    }

    #[test]
    fn top_at_max_picks_the_peak_of_the_series_within_the_range() {
        let mut h = History::new();
        for (pid, (t, cpu, name)) in [(1_000u64, 5.0f32, "a"), (2_000, 70.0, "peak"), (3_000, 70.0, "tie"), (4_000, 10.0, "d")].into_iter().enumerate() {
            h.push(t, &totals(cpu, 1), &[row(pid as u32, name, 1.0)]);
        }
        let cpu = Some(&Series::Cpu);
        assert_eq!(h.top_at_max(cpu, 1_000, 4_000, Metric::Cpu), Some((2_000, vec![("peak".to_string(), 1.0)])), "earliest on a tie");
        assert_eq!(h.top_at_max(cpu, 3_001, 4_000, Metric::Cpu).unwrap().0, 4_000, "bounds are inclusive");
        assert_eq!(h.top_at_max(Some(&Series::Core(9)), 1_000, 3_500, Metric::Cpu).unwrap().0, 3_000, "no values: the newest sample");
        assert_eq!(h.top_at_max(None, 1_000, 3_500, Metric::Cpu).unwrap().0, 3_000, "no series: the newest sample");
        assert_eq!(h.top_at_max(cpu, 4_001, 9_000, Metric::Cpu), None, "no sample in the range");
    }

    #[test]
    fn points_since_skips_older_samples() {
        let mut h = History::new();
        for t in 0..5u64 {
            h.push(t * 1_000, &totals(t as f32, 1), &[]);
        }
        let got: Vec<(u64, Option<f32>)> = h.points(&Series::Cpu, 3_000).collect();
        assert_eq!(got, [(3_000, Some(3.0)), (4_000, Some(4.0))]);
        assert_eq!(h.points(&Series::DiskActive("none".into()), 0).count(), 5, "unknown series: all gaps");
        assert!(h.points(&Series::DiskActive("none".into()), 0).all(|(_, v)| v.is_none()));
    }

    #[test]
    fn top_at_finds_the_nearest_sample_within_the_gap_for_every_metric() {
        let mut h = History::new();
        h.push(1_000, &totals(10.0, 1), &[row(1, "chrome", 40.0), row(2, "code", 20.0)]);
        h.push(2_000, &totals(10.0, 1), &[row(1, "chrome", 30.0)]);
        assert_eq!(h.top_at(1_400, Metric::Cpu, 1_000), Some((1_000, vec![("chrome".to_string(), 40.0), ("code".to_string(), 20.0)])));
        assert_eq!(h.top_at(1_600, Metric::Gpu, 1_000), Some((2_000, vec![("chrome".to_string(), 30.0)])));
        assert_eq!(h.top_at(1_500, Metric::Memory, 1_000).unwrap().0, 1_000, "a tie goes to the earlier sample");
        assert_eq!(h.top_at(1_000, Metric::Network, 0).unwrap().1[0], ("chrome".to_string(), 40.0));
        assert_eq!(h.top_at(9_000, Metric::Cpu, 1_000), None, "nothing within the gap");
        assert_eq!(History::new().top_at(0, Metric::Cpu, u64::MAX), None);
    }

    #[test]
    fn name_table_stays_bounded_under_key_churn() {
        let mut h = History::new();
        for t in 0..(MAX_SAMPLES as u64 * 2) {
            h.push(t * 1_000, &totals(1.0, 1), &[row(t as u32 + 1, &format!("p{t}"), 10.0)]);
        }
        assert!(h.ids.len() <= MAX_SAMPLES, "{} live ids", h.ids.len());
        assert!(h.names.len() <= MAX_SAMPLES + 1, "{} name slots", h.names.len());
        let last = MAX_SAMPLES as u64 * 2 - 1;
        assert_eq!(h.top_at(last * 1_000, Metric::Cpu, 0).unwrap().1, vec![(format!("p{last}"), 10.0)]);
    }

    #[test]
    fn an_app_shared_across_lists_and_reappearing_keeps_its_name() {
        let mut h = History::new();
        h.push(0, &totals(1.0, 1), &[row(1, "chrome", 5.0)]);
        h.push(1_000, &totals(1.0, 1), &[]);
        h.push(2_000, &totals(1.0, 1), &[row(1, "chrome", 6.0)]);
        assert_eq!(h.top_at(0, Metric::Memory, 0).unwrap().1[0].0, "chrome");
        assert_eq!(h.top_at(2_000, Metric::Disk, 0).unwrap().1[0].0, "chrome");
        assert_eq!(h.ids.len(), 1);
    }

    fn fill_an_hour(cores: usize) -> History {
        let mut h = History::new();
        let rows: Vec<AppRow> = (1..=40).map(|i| row(i, &format!("app{i}"), i as f32)).collect();
        let t = full(cores);
        for s in 0..MAX_SAMPLES as u64 {
            h.push(s * 1_000, &t, &rows);
        }
        h
    }

    #[test]
    fn an_hour_on_a_typical_machine_fits_the_budget_with_every_list_full() {
        let h = fill_an_hour(16);
        assert_eq!(h.len(), MAX_SAMPLES);
        assert!(h.approx_bytes() <= 2_000_000, "{} bytes", h.approx_bytes());
    }

    #[test]
    fn an_hour_with_64_logical_processors_still_fits() {
        let h = fill_an_hour(64);
        assert!(h.approx_bytes() <= 2_000_000, "{} bytes", h.approx_bytes());
    }

    #[test]
    fn steady_state_pushes_do_not_grow_memory() {
        let mut h = fill_an_hour(16);
        let before = h.approx_bytes();
        let rows: Vec<AppRow> = (1..=40).map(|i| row(i, &format!("app{i}"), i as f32)).collect();
        for s in 0..600u64 {
            h.push((MAX_SAMPLES as u64 + s) * 1_000, &full(16), &rows);
        }
        assert_eq!(h.approx_bytes(), before);
    }

    #[test]
    fn unknown_values_are_stored_as_gaps_never_zero_or_nan() {
        let mut h = History::new();
        let mut t = full(1);
        t.gpus.as_mut().unwrap()[0].util_pct = f32::NAN;
        t.gpus.as_mut().unwrap()[0].temp_c = None;
        t.disks.as_mut().unwrap()[0].response_ms = None;
        t.cpu_pct = None;
        h.push(0, &t, &[]);
        assert_eq!(values(&h, &Series::GpuUtil("luid_0x0_0x1_phys_0".into())), [None]);
        assert_eq!(values(&h, &Series::GpuTemp("luid_0x0_0x1_phys_0".into())), [None]);
        assert_eq!(values(&h, &Series::DiskResponse("0 C:".into())), [None]);
        assert_eq!(values(&h, &Series::Cpu), [None]);
    }

    #[test]
    fn core_columns_follow_the_processor_count() {
        let mut h = History::new();
        h.push(0, &totals(1.0, 3), &[]);
        h.push(1_000, &totals(1.0, 0), &[]);
        assert_eq!(values(&h, &Series::Core(2)), [Some(1.0), None]);
        h.push(WINDOW_MS + 500, &totals(1.0, 0), &[]);
        assert!(!h.has(&Series::Core(0)) && !h.has(&Series::Core(2)));
    }
}
