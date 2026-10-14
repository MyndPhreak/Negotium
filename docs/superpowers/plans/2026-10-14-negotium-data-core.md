# Negotium Plan 1 — Data Core + Benchmark Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build Negotium's headless data core (collector → model → history), a console probe that prints live top apps, and a benchmark that records Task Manager's baseline.

**Architecture:** A Cargo workspace of small crates. `negotium-snapshot` holds plain data types. `negotium-collector` is the only crate (with `negotium-bench`) that calls Windows APIs; it produces a `Snapshot` per tick from one `NtQuerySystemInformation` call, PDH counter sets, `GlobalMemoryStatusEx`, and a top-level window scan. `negotium-model` is pure logic (rates, app grouping, hung tracking, ranking). `negotium-history` is a bounded ring buffer. `negotium-probe` and `negotium-bench` are binaries.

**Tech Stack:** Rust stable (MSVC, edition 2024), `windows` 0.62.2, `serde` 1.0.229, `serde_json` 1.0.154, `anyhow` 1.0.104.

**Spec:** `docs/superpowers/specs/2026-10-14-negotium-design.md` (sections 3, 4, 7, 8, 9 apply to this plan). Research: `docs/research/taskmgr-26100-notes.md`.

**Out of scope (later plans):** Slint UI, actions, tray, elevation, Performance/Startup/Settings pages, per-app network (SRUM spike), CPU temperature, installer. The collector runs inline in the probe here; the dedicated collector thread with panic-restart is built in Plan 2.

## Global Constraints

- Windows 11 x64 only; Rust stable, `x86_64-pc-windows-msvc`, edition 2024.
- Dependencies limited to: `windows = "0.62.2"`, `serde = "1.0.229"` (derive), `serde_json = "1.0.154"`, `anyhow = "1.0.104"` (binaries only). Adding any other crate requires changing this plan.
- `unsafe` code only in `negotium-collector` and `negotium-bench`; every `unsafe` block has a `// SAFETY:` comment.
- A failing data source never panics: its field becomes `None` and a message is pushed to `Snapshot::source_errors`.
- Per-process access failures are data (`None`), never errors.
- Numbers must match Task Manager's sources: CPU from the `Processor Information` counter set (`% Processor Utility`), GPU from `GPU Engine` counters, memory column = private working set.
- Performance targets (spec §8): collector tick median ≤ 5 ms; history ≈ 2 MB for 1 h at 1 s.
- If a `windows` 0.62.2 item path or signature differs from this plan, look it up at `https://microsoft.github.io/windows-docs-rs/doc/windows/` and adapt the call without changing behavior.

## Review Focus

1. **PID reuse between ticks** (same PID, different create time) → treated as a new process with 0 % CPU that tick, not a huge spike. Test in Task 7.
2. **Parent-PID cycles and reused parent PIDs** → grouping terminates and never adopts a process into a "parent" created after it. Test in Task 8.
3. **System Idle Process (PID 0), empty or invalid UTF-16 image names** → sensible names; PID 0 excluded from apps. Tests in Tasks 2 and 8.
4. **More than 64 logical processors** (processor groups, instances like `1,5`) → per-core list complete and ordered group-then-core. Test in Task 5.
5. **Timing jitter / counters going backwards / zero elapsed time** → CPU % clamped to [0, 100], never NaN or infinite. Test in Task 7.

---

## File Structure

```
Cargo.toml                         workspace + shared deps + release profile
rust-toolchain.toml                pins stable
crates/snapshot/                   negotium-snapshot: Snapshot & sample types (serde)
  src/lib.rs
crates/collector/                  negotium-collector: Windows APIs → Snapshot
  src/lib.rs                       module wiring + re-exports
  src/processes.rs                 NtQuerySystemInformation process list
  src/identity.rs                  per-(pid, create_time) image path + package family cache
  src/windows_scan.rs              top-level windows + IsHungAppWindow
  src/pdh.rs                       PDH counter sets (CPU, disk, network, GPU) + pure parsers
  src/memory.rs                    GlobalMemoryStatusEx + logical CPU count
  src/collector.rs                 Collector::tick assembling a Snapshot
  src/bin/burn.rs                  test helper: busy-loops one core
  tests/processes.rs, tests/identity.rs, tests/windows_scan.rs, tests/pdh.rs,
  tests/collector.rs, tests/accuracy.rs
crates/model/                      negotium-model: pure logic
  src/lib.rs                       Model, AppRow, ProcRow
  src/rates.rs                     CPU % and disk B/s from deltas
  src/grouping.rs                  AppKey, AppKind, process → app rules
  src/hung.rs                      hung-since tracking
  src/ranking.rs                   Metric, Ranker (stable ordering), top_n
crates/history/                    negotium-history: ring buffer
  src/lib.rs
crates/probe/                      negotium-probe: console live view + recorder
  src/main.rs
crates/bench/                      negotium-bench: Task Manager vs Negotium measurement
  src/main.rs
  src/stats.rs                     percentiles (pure)
docs/benchmarks/                   recorded results
```

---

### Task 1: Toolchain, workspace, and snapshot types

**Files:**
- Create: `rust-toolchain.toml`, `Cargo.toml`
- Create: `crates/snapshot/Cargo.toml`, `crates/snapshot/src/lib.rs`

**Interfaces:**
- Produces (crate `negotium_snapshot`):
  - `ProcessSample { pid: u32, parent_pid: u32, create_time: i64, name: String, session_id: u32, cpu_time_100ns: u64, private_working_set: u64, disk_io_bytes: u64, threads: u32, handles: u32, image_path: Option<String>, package_family: Option<String> }`
  - `WindowSample { pid: u32, hung: bool, title: String }`
  - `DiskSample { name: String, active_pct: f32, read_bps: f64, write_bps: f64 }`
  - `MemoryTotals { total: u64, used: u64 }`
  - `SystemTotals { logical_cpus: u32, cpu_pct: Option<f32>, per_core_pct: Vec<f32>, memory: Option<MemoryTotals>, disks: Option<Vec<DiskSample>>, gpu_pct: Option<f32>, gpu_pct_by_pid: Option<BTreeMap<u32, f32>>, net_bps: Option<f64> }` with `fn busiest_disk_pct(&self) -> Option<f32>`
  - `Snapshot { taken_at_ms: u64, processes: Vec<ProcessSample>, windows: Vec<WindowSample>, totals: SystemTotals, source_errors: Vec<String> }`
  - All derive `Clone, Debug, PartialEq, Serialize, Deserialize`; `SystemTotals` and `Snapshot` also derive `Default`.

- [ ] **Step 1: Install the Rust toolchain**

Run: `rustup default stable`
Then: `cargo --version`
Expected: prints `cargo 1.x.y` (any version ≥ 1.85).

- [ ] **Step 2: Create the workspace files**

`rust-toolchain.toml`:
```toml
[toolchain]
channel = "stable"
targets = ["x86_64-pc-windows-msvc"]
```

`Cargo.toml`:
```toml
[workspace]
resolver = "3"
members = ["crates/*"]

[workspace.package]
edition = "2024"
version = "0.1.0"
license = "MIT"
publish = false

[workspace.dependencies]
negotium-snapshot = { path = "crates/snapshot" }
negotium-collector = { path = "crates/collector" }
negotium-model = { path = "crates/model" }
negotium-history = { path = "crates/history" }
serde = { version = "1.0.229", features = ["derive"] }
serde_json = "1.0.154"
anyhow = "1.0.104"
windows = "0.62.2"

[profile.release]
lto = "thin"
codegen-units = 1
```

`crates/snapshot/Cargo.toml`:
```toml
[package]
name = "negotium-snapshot"
edition.workspace = true
version.workspace = true
license.workspace = true
publish.workspace = true

[dependencies]
serde.workspace = true

[dev-dependencies]
serde_json.workspace = true
```

- [ ] **Step 3: Write the failing tests**

`crates/snapshot/src/lib.rs` (tests only for now, types come in Step 5):
```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn disk(name: &str, active: f32) -> DiskSample {
        DiskSample { name: name.into(), active_pct: active, read_bps: 0.0, write_bps: 0.0 }
    }

    #[test]
    fn snapshot_round_trips_through_json() {
        let snap = Snapshot {
            taken_at_ms: 1000,
            processes: vec![ProcessSample {
                pid: 42, parent_pid: 4, create_time: 7, name: "a.exe".into(), session_id: 1,
                cpu_time_100ns: 10, private_working_set: 20, disk_io_bytes: 30, threads: 2,
                handles: 3, image_path: Some(r"C:\a.exe".into()), package_family: None,
            }],
            windows: vec![WindowSample { pid: 42, hung: false, title: "A".into() }],
            totals: SystemTotals { logical_cpus: 8, cpu_pct: Some(12.5), ..Default::default() },
            source_errors: vec![],
        };
        let json = serde_json::to_string(&snap).unwrap();
        let back: Snapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(back, snap);
    }

    #[test]
    fn busiest_disk_is_max_active() {
        let t = SystemTotals { disks: Some(vec![disk("0 C:", 12.0), disk("1 D:", 80.0)]), ..Default::default() };
        assert_eq!(t.busiest_disk_pct(), Some(80.0));
    }

    #[test]
    fn busiest_disk_none_when_unavailable_or_empty() {
        assert_eq!(SystemTotals::default().busiest_disk_pct(), None);
        let t = SystemTotals { disks: Some(vec![]), ..Default::default() };
        assert_eq!(t.busiest_disk_pct(), None);
    }
}
```

- [ ] **Step 4: Run tests to verify they fail**

Run: `cargo test -p negotium-snapshot`
Expected: FAIL to compile with "cannot find type `Snapshot`".

- [ ] **Step 5: Write the types above the test module**

```rust
//! Plain data produced by the collector each tick. No Windows types, no handles.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProcessSample {
    pub pid: u32,
    pub parent_pid: u32,
    /// FILETIME units (100 ns since 1601); with `pid` it identifies a process uniquely.
    pub create_time: i64,
    pub name: String,
    pub session_id: u32,
    /// User + kernel time, 100 ns units.
    pub cpu_time_100ns: u64,
    /// Private working set in bytes (Task Manager's "Memory" column).
    pub private_working_set: u64,
    /// Read + write transfer bytes since process start.
    pub disk_io_bytes: u64,
    pub threads: u32,
    pub handles: u32,
    pub image_path: Option<String>,
    pub package_family: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowSample {
    pub pid: u32,
    pub hung: bool,
    pub title: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DiskSample {
    pub name: String,
    pub active_pct: f32,
    pub read_bps: f64,
    pub write_bps: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MemoryTotals {
    pub total: u64,
    pub used: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SystemTotals {
    pub logical_cpus: u32,
    pub cpu_pct: Option<f32>,
    pub per_core_pct: Vec<f32>,
    pub memory: Option<MemoryTotals>,
    pub disks: Option<Vec<DiskSample>>,
    pub gpu_pct: Option<f32>,
    pub gpu_pct_by_pid: Option<BTreeMap<u32, f32>>,
    pub net_bps: Option<f64>,
}

impl SystemTotals {
    /// Active time of the busiest physical disk, as Task Manager's Disk meter shows.
    pub fn busiest_disk_pct(&self) -> Option<f32> {
        self.disks.as_ref()?.iter().map(|d| d.active_pct).reduce(f32::max)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    /// Milliseconds since the collector started (monotonic).
    pub taken_at_ms: u64,
    pub processes: Vec<ProcessSample>,
    pub windows: Vec<WindowSample>,
    pub totals: SystemTotals,
    pub source_errors: Vec<String>,
}
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p negotium-snapshot`
Expected: 3 passed.

- [ ] **Step 7: Commit**

```bash
git add rust-toolchain.toml Cargo.toml Cargo.lock crates/snapshot
git commit -m "feat: add workspace and snapshot data types"
```

---

### Task 2: Process list from NtQuerySystemInformation

**Files:**
- Create: `crates/collector/Cargo.toml`, `crates/collector/src/lib.rs`, `crates/collector/src/processes.rs`, `crates/collector/src/bin/burn.rs`
- Test: `crates/collector/tests/processes.rs` (integration) + unit tests in `processes.rs`

**Interfaces:**
- Produces (crate `negotium_collector`):
  - `pub struct RawProcess { pub pid: u32, pub parent_pid: u32, pub create_time: i64, pub name: String, pub session_id: u32, pub cpu_time_100ns: u64, pub private_working_set: u64, pub disk_io_bytes: u64, pub threads: u32, pub handles: u32 }`
  - `pub struct ProcessReader` with `pub fn new() -> Self` and `pub fn read(&mut self) -> Result<Vec<RawProcess>, i32>` (error = NTSTATUS)
  - Binary `negotium-burn [ms]`: busy-loops one core for `ms` milliseconds (default 10000).

- [ ] **Step 1: Create the crate manifest, burn helper, and lib**

`crates/collector/Cargo.toml`:
```toml
[package]
name = "negotium-collector"
edition.workspace = true
version.workspace = true
license.workspace = true
publish.workspace = true

[dependencies]
negotium-snapshot.workspace = true
windows = { workspace = true, features = [
    "Win32_Foundation",
    "Win32_System_Performance",
    "Win32_System_Threading",
    "Win32_System_SystemInformation",
    "Win32_Storage_Packaging_Appx",
    "Win32_UI_WindowsAndMessaging",
] }

[[bin]]
name = "negotium-burn"
path = "src/bin/burn.rs"
test = false
```

`crates/collector/src/bin/burn.rs`:
```rust
//! Test helper: keeps one core busy for the given milliseconds (default 10000).
use std::time::{Duration, Instant};

fn main() {
    let ms: u64 = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(10_000);
    let end = Instant::now() + Duration::from_millis(ms);
    let mut x: u64 = 1;
    while Instant::now() < end {
        x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        std::hint::black_box(x);
    }
}
```

`crates/collector/src/lib.rs`:
```rust
//! Windows data sources. The only crate besides `negotium-bench` that calls Windows APIs.
pub mod processes;

pub use processes::{ProcessReader, RawProcess};
```

- [ ] **Step 2: Write the failing tests**

`crates/collector/tests/processes.rs`:
```rust
use negotium_collector::ProcessReader;
use std::process::Command;
use std::time::Duration;

#[test]
fn includes_current_process() {
    let me = std::process::id();
    let list = ProcessReader::new().read().expect("read");
    let p = list.iter().find(|p| p.pid == me).expect("own pid listed");
    assert!(p.name.to_lowercase().ends_with(".exe"), "name was {}", p.name);
    assert!(p.create_time > 0);
    assert!(p.threads >= 1);
    assert!(p.private_working_set > 0);
}

#[test]
fn sees_child_with_parent_and_growing_cpu_time() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_negotium-burn")).arg("3000").spawn().unwrap();
    let mut reader = ProcessReader::new();
    std::thread::sleep(Duration::from_millis(300));
    let first = reader.read().unwrap().into_iter().find(|p| p.pid == child.id()).expect("child listed");
    assert_eq!(first.parent_pid, std::process::id());
    std::thread::sleep(Duration::from_millis(700));
    let second = reader.read().unwrap().into_iter().find(|p| p.pid == child.id()).expect("child listed");
    assert_eq!(second.create_time, first.create_time);
    assert!(second.cpu_time_100ns > first.cpu_time_100ns + 3_000_000, "cpu time should grow by >300 ms");
    child.kill().ok();
    child.wait().ok();
}

#[test]
fn includes_idle_and_system() {
    let list = ProcessReader::new().read().unwrap();
    let idle = list.iter().find(|p| p.pid == 0).expect("pid 0");
    assert_eq!(idle.name, "System Idle Process");
    assert!(list.iter().any(|p| p.pid == 4 && p.name == "System"));
}
```

Unit tests at the bottom of `crates/collector/src/processes.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn struct_layout_matches_x64_ntdll() {
        assert_eq!(size_of::<SystemProcessInformation>(), 0x100);
    }

    #[test]
    fn decode_names() {
        assert_eq!(decode_image_name(0, &[]), "System Idle Process");
        assert_eq!(decode_image_name(1234, &[]), "<pid 1234>");
        let units: Vec<u16> = "a.exe".encode_utf16().collect();
        assert_eq!(decode_image_name(5, &units), "a.exe");
        // Lone surrogate must not panic.
        assert_eq!(decode_image_name(6, &[0x61, 0xD800, 0x62]), "a\u{FFFD}b");
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p negotium-collector`
Expected: FAIL to compile ("file not found for module `processes`").

- [ ] **Step 4: Implement `processes.rs`**

```rust
//! Whole-system process list from a single NtQuerySystemInformation call.
//! No per-process handles are opened here.
use std::ffi::c_void;
use std::mem::size_of;

const SYSTEM_PROCESS_INFORMATION_CLASS: u32 = 5;
const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC000_0004_u32 as i32;
const INITIAL_BUFFER_BYTES: usize = 512 * 1024;

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQuerySystemInformation(class: u32, info: *mut c_void, len: u32, return_len: *mut u32) -> i32;
}

#[repr(C)]
struct UnicodeString {
    length_bytes: u16,
    maximum_length_bytes: u16,
    buffer: *const u16,
}

/// x64 layout of SYSTEM_PROCESS_INFORMATION (0x100 bytes); thread entries follow and are skipped.
#[repr(C)]
pub(crate) struct SystemProcessInformation {
    next_entry_offset: u32,
    number_of_threads: u32,
    working_set_private_size: i64,
    hard_fault_count: u32,
    number_of_threads_high_watermark: u32,
    cycle_time: u64,
    create_time: i64,
    user_time: i64,
    kernel_time: i64,
    image_name: UnicodeString,
    base_priority: i32,
    unique_process_id: usize,
    inherited_from_unique_process_id: usize,
    handle_count: u32,
    session_id: u32,
    unique_process_key: usize,
    peak_virtual_size: usize,
    virtual_size: usize,
    page_fault_count: u32,
    peak_working_set_size: usize,
    working_set_size: usize,
    quota_peak_paged_pool_usage: usize,
    quota_paged_pool_usage: usize,
    quota_peak_non_paged_pool_usage: usize,
    quota_non_paged_pool_usage: usize,
    pagefile_usage: usize,
    peak_pagefile_usage: usize,
    private_page_count: usize,
    read_operation_count: i64,
    write_operation_count: i64,
    other_operation_count: i64,
    read_transfer_count: i64,
    write_transfer_count: i64,
    other_transfer_count: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RawProcess {
    pub pid: u32,
    pub parent_pid: u32,
    pub create_time: i64,
    pub name: String,
    pub session_id: u32,
    pub cpu_time_100ns: u64,
    pub private_working_set: u64,
    pub disk_io_bytes: u64,
    pub threads: u32,
    pub handles: u32,
}

/// Reuses one 8-byte-aligned buffer across reads.
pub struct ProcessReader {
    buf: Vec<u64>,
}

impl Default for ProcessReader {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessReader {
    pub fn new() -> Self {
        Self { buf: vec![0; INITIAL_BUFFER_BYTES / 8] }
    }

    /// Returns every process, or the failing NTSTATUS.
    pub fn read(&mut self) -> Result<Vec<RawProcess>, i32> {
        loop {
            let len_bytes = (self.buf.len() * 8) as u32;
            let mut needed = 0u32;
            // SAFETY: `buf` is a writable, 8-byte-aligned buffer of `len_bytes` bytes.
            let status = unsafe {
                NtQuerySystemInformation(
                    SYSTEM_PROCESS_INFORMATION_CLASS,
                    self.buf.as_mut_ptr().cast(),
                    len_bytes,
                    &mut needed,
                )
            };
            if status == STATUS_INFO_LENGTH_MISMATCH {
                let want_words = (needed as usize + 64 * 1024).div_ceil(8);
                self.buf.resize(want_words.max(self.buf.len() * 2), 0);
                continue;
            }
            if status < 0 {
                return Err(status);
            }
            // SAFETY: the call succeeded, so `buf` holds a valid SystemProcessInformation chain.
            return Ok(unsafe { parse(self.buf.as_ptr().cast(), len_bytes as usize) });
        }
    }
}

/// # Safety
/// `base` must point to `len` bytes written by NtQuerySystemInformation(SystemProcessInformation).
unsafe fn parse(base: *const u8, len: usize) -> Vec<RawProcess> {
    let mut out = Vec::with_capacity(512);
    let mut offset = 0usize;
    while offset + size_of::<SystemProcessInformation>() <= len {
        // SAFETY: bounds checked above; entries are 8-byte aligned by the kernel.
        let info = unsafe { &*(base.add(offset) as *const SystemProcessInformation) };
        let pid = info.unique_process_id as u32;
        let name_units: &[u16] = if info.image_name.buffer.is_null() {
            &[]
        } else {
            // SAFETY: ImageName points inside the same buffer; length is in bytes.
            unsafe {
                std::slice::from_raw_parts(info.image_name.buffer, info.image_name.length_bytes as usize / 2)
            }
        };
        out.push(RawProcess {
            pid,
            parent_pid: info.inherited_from_unique_process_id as u32,
            create_time: info.create_time,
            name: decode_image_name(pid, name_units),
            session_id: info.session_id,
            cpu_time_100ns: (info.user_time.max(0) as u64).saturating_add(info.kernel_time.max(0) as u64),
            private_working_set: info.working_set_private_size.max(0) as u64,
            disk_io_bytes: (info.read_transfer_count.max(0) as u64)
                .saturating_add(info.write_transfer_count.max(0) as u64),
            threads: info.number_of_threads,
            handles: info.handle_count,
        });
        if info.next_entry_offset == 0 {
            break;
        }
        offset += info.next_entry_offset as usize;
    }
    out
}

pub(crate) fn decode_image_name(pid: u32, units: &[u16]) -> String {
    if units.is_empty() {
        return if pid == 0 { "System Idle Process".to_string() } else { format!("<pid {pid}>") };
    }
    String::from_utf16_lossy(units)
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p negotium-collector`
Expected: unit tests (2) and integration tests (3) pass.

- [ ] **Step 6: Commit**

```bash
git add crates/collector Cargo.lock
git commit -m "feat(collector): read all processes with one NtQuerySystemInformation call"
```

---

### Task 3: Process identity cache (image path, package family)

**Files:**
- Create: `crates/collector/src/identity.rs`
- Modify: `crates/collector/src/lib.rs`
- Test: `crates/collector/tests/identity.rs`

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces:
  - `pub struct Identity { pub image_path: Option<String>, pub package_family: Option<String> }` (derives `Clone, Debug, Default, PartialEq`)
  - `pub struct IdentityCache` with `pub fn new() -> Self`, `pub fn get(&mut self, pid: u32, create_time: i64) -> &Identity`, `pub fn retain_alive(&mut self, alive: &HashSet<(u32, i64)>)`, `pub fn opens(&self) -> u64` (number of uncached lookups, for tests)

- [ ] **Step 1: Write the failing tests**

`crates/collector/tests/identity.rs`:
```rust
use negotium_collector::{IdentityCache, ProcessReader};
use std::collections::HashSet;

fn own_create_time() -> i64 {
    let me = std::process::id();
    ProcessReader::new().read().unwrap().into_iter().find(|p| p.pid == me).unwrap().create_time
}

#[test]
fn resolves_own_image_path_and_no_package() {
    let mut cache = IdentityCache::new();
    let id = cache.get(std::process::id(), own_create_time()).clone();
    let expected = std::env::current_exe().unwrap().to_string_lossy().to_lowercase();
    assert_eq!(id.image_path.map(|p| p.to_lowercase()), Some(expected));
    assert_eq!(id.package_family, None);
}

#[test]
fn caches_until_process_is_gone() {
    let mut cache = IdentityCache::new();
    let key = (std::process::id(), own_create_time());
    cache.get(key.0, key.1);
    cache.get(key.0, key.1);
    assert_eq!(cache.opens(), 1);
    cache.retain_alive(&HashSet::new());
    cache.get(key.0, key.1);
    assert_eq!(cache.opens(), 2);
}

#[test]
fn inaccessible_pids_yield_empty_identity() {
    let mut cache = IdentityCache::new();
    assert_eq!(cache.get(0, 0).image_path, None);
    assert_eq!(cache.get(4, 0).image_path, None);
    assert_eq!(cache.get(u32::MAX - 3, 1).image_path, None);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p negotium-collector --test identity`
Expected: FAIL to compile ("no `IdentityCache` in the root").

- [ ] **Step 3: Implement `identity.rs` and export it**

```rust
//! Expensive per-process facts, fetched once per (pid, create_time) and cached.
use std::collections::{HashMap, HashSet};
use windows::Win32::Foundation::{CloseHandle, ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS, HANDLE};
use windows::Win32::Storage::Packaging::Appx::GetPackageFamilyName;
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::core::PWSTR;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Identity {
    pub image_path: Option<String>,
    pub package_family: Option<String>,
}

#[derive(Default)]
pub struct IdentityCache {
    entries: HashMap<(u32, i64), Identity>,
    opens: u64,
}

impl IdentityCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&mut self, pid: u32, create_time: i64) -> &Identity {
        let opens = &mut self.opens;
        self.entries.entry((pid, create_time)).or_insert_with(|| {
            *opens += 1;
            query(pid)
        })
    }

    /// Drops entries for processes that no longer exist.
    pub fn retain_alive(&mut self, alive: &HashSet<(u32, i64)>) {
        self.entries.retain(|k, _| alive.contains(k));
    }

    pub fn opens(&self) -> u64 {
        self.opens
    }
}

fn query(pid: u32) -> Identity {
    if pid == 0 || pid == 4 {
        return Identity::default();
    }
    // SAFETY: plain Win32 call; the handle is closed below.
    let Ok(handle) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }) else {
        return Identity::default();
    };
    let identity = Identity { image_path: image_path(handle), package_family: package_family(handle) };
    // SAFETY: `handle` came from OpenProcess and is closed exactly once.
    unsafe {
        let _ = CloseHandle(handle);
    }
    identity
}

fn image_path(handle: HANDLE) -> Option<String> {
    let mut buf = vec![0u16; 1024];
    let mut len = buf.len() as u32;
    // SAFETY: `buf` has `len` writable UTF-16 units; `len` is updated to the written length.
    unsafe { QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len) }.ok()?;
    Some(String::from_utf16_lossy(&buf[..len as usize]))
}

fn package_family(handle: HANDLE) -> Option<String> {
    let mut len = 0u32;
    // SAFETY: size query with no buffer; unpackaged processes return APPMODEL_ERROR_NO_PACKAGE.
    let first = unsafe { GetPackageFamilyName(handle, &mut len, None) };
    if first != ERROR_INSUFFICIENT_BUFFER || len == 0 {
        return None;
    }
    let mut buf = vec![0u16; len as usize];
    // SAFETY: `buf` holds `len` UTF-16 units including the terminator.
    let second = unsafe { GetPackageFamilyName(handle, &mut len, Some(PWSTR(buf.as_mut_ptr()))) };
    if second != ERROR_SUCCESS {
        return None;
    }
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..end]))
}
```

In `crates/collector/src/lib.rs` add:
```rust
pub mod identity;

pub use identity::{Identity, IdentityCache};
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p negotium-collector --test identity`
Expected: 3 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/collector
git commit -m "feat(collector): cache image path and package family per process"
```

---

### Task 4: Top-level window scan with hung detection

**Files:**
- Create: `crates/collector/src/windows_scan.rs`
- Modify: `crates/collector/src/lib.rs`
- Test: `crates/collector/tests/windows_scan.rs`

**Interfaces:**
- Consumes: `negotium_snapshot::WindowSample`.
- Produces: `pub fn scan_windows() -> Vec<WindowSample>` — visible, unowned, titled top-level windows with owning PID and `IsHungAppWindow` result.

- [ ] **Step 1: Write the failing test** (takes about 7 s: Windows flags a window as hung after 5 s without message processing)

`crates/collector/tests/windows_scan.rs`:
```rust
use negotium_collector::scan_windows;
use std::time::{Duration, Instant};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage, WINDOW_EX_STYLE,
    WS_POPUP, WS_VISIBLE,
};
use windows::core::{HSTRING, w};

#[test]
fn detects_a_window_that_stops_pumping_messages() {
    let title = format!("negotium-hang-test-{}", std::process::id());
    let thread_title = title.clone();
    std::thread::spawn(move || {
        // SAFETY: creates a plain STATIC window owned by this thread.
        let _hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0), w!("STATIC"), &HSTRING::from(thread_title.as_str()),
                WS_POPUP | WS_VISIBLE, -32000, -32000, 100, 100, None, None, None, None,
            )
        }
        .expect("create window");
        let pump_until = Instant::now() + Duration::from_millis(500);
        let mut msg = MSG::default();
        while Instant::now() < pump_until {
            // SAFETY: standard message pump on the owning thread.
            unsafe {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_secs(12)); // stop pumping: the window goes "Not responding"
    });

    let deadline = Instant::now() + Duration::from_secs(11);
    let mut seen_responsive = false;
    loop {
        let found = scan_windows().into_iter().find(|w| w.title == title);
        if let Some(w) = found {
            assert_eq!(w.pid, std::process::id());
            if !w.hung {
                seen_responsive = true;
            } else {
                assert!(seen_responsive, "window should be responsive before it hangs");
                return;
            }
        }
        assert!(Instant::now() < deadline, "window never reported hung");
        std::thread::sleep(Duration::from_millis(100));
    }
}
```

Add to `crates/collector/Cargo.toml`:
```toml
[dev-dependencies]
windows = { workspace = true, features = ["Win32_Foundation", "Win32_UI_WindowsAndMessaging"] }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p negotium-collector --test windows_scan`
Expected: FAIL to compile ("no `scan_windows` in the root").

- [ ] **Step 3: Implement `windows_scan.rs` and export it**

```rust
//! Top-level window enumeration used for app grouping and "Not responding" detection.
use negotium_snapshot::WindowSample;
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GW_OWNER, GetWindow, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
    IsHungAppWindow, IsWindowVisible,
};
use windows::core::BOOL;

/// Visible, unowned, titled top-level windows.
pub fn scan_windows() -> Vec<WindowSample> {
    let mut out: Vec<WindowSample> = Vec::with_capacity(64);
    // SAFETY: `out` outlives the synchronous EnumWindows call that receives its address.
    unsafe {
        let _ = EnumWindows(Some(visit), LPARAM(&mut out as *mut Vec<WindowSample> as isize));
    }
    out
}

unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: lparam is the `&mut Vec<WindowSample>` passed by scan_windows.
    let out = unsafe { &mut *(lparam.0 as *mut Vec<WindowSample>) };
    // SAFETY: all calls below only read window state for a handle supplied by EnumWindows.
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() {
            return true.into();
        }
        if GetWindow(hwnd, GW_OWNER).is_ok_and(|owner| !owner.is_invalid()) {
            return true.into();
        }
        let title = window_title(hwnd);
        if title.is_empty() {
            return true.into();
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        out.push(WindowSample { pid, hung: IsHungAppWindow(hwnd).as_bool(), title });
    }
    true.into()
}

/// # Safety
/// `hwnd` must be a window handle from EnumWindows. GetWindowText does not send
/// messages to other processes, so it cannot block on a hung window.
unsafe fn window_title(hwnd: HWND) -> String {
    let len = unsafe { GetWindowTextLengthW(hwnd) };
    if len <= 0 {
        return String::new();
    }
    let mut buf = vec![0u16; len as usize + 1];
    let copied = unsafe { GetWindowTextW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..copied.max(0) as usize])
}
```

In `crates/collector/src/lib.rs` add:
```rust
pub mod windows_scan;

pub use windows_scan::scan_windows;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p negotium-collector --test windows_scan`
Expected: 1 passed (about 7 s).

- [ ] **Step 5: Commit**

```bash
git add crates/collector
git commit -m "feat(collector): scan top-level windows and detect hung apps"
```

---

### Task 5: PDH counter sets and memory totals

**Files:**
- Create: `crates/collector/src/pdh.rs`, `crates/collector/src/memory.rs`
- Modify: `crates/collector/src/lib.rs`
- Test: unit tests in `pdh.rs`; `crates/collector/tests/pdh.rs`

**Interfaces:**
- Consumes: `negotium_snapshot::{DiskSample, MemoryTotals}`.
- Produces:
  - `pub struct PdhReadings { pub cpu_pct: Option<f32>, pub per_core_pct: Vec<f32>, pub disks: Option<Vec<DiskSample>>, pub net_bps: Option<f64>, pub gpu_pct: Option<f32>, pub gpu_pct_by_pid: Option<BTreeMap<u32, f32>>, pub errors: Vec<String> }`
  - `pub struct PdhSources` with `pub fn open() -> Result<Self, u32>` (primes rate counters) and `pub fn read(&mut self) -> PdhReadings`. Not `Send`: create and use on one thread.
  - Pure helpers (pub for tests): `parse_core_instance(&str) -> Option<(u32, u32)>`, `per_core_from(&[(String, f64)]) -> Vec<f32>`, `total_from(&[(String, f64)]) -> Option<f32>`, `parse_gpu_instance(&str) -> Option<(u32, String)>`, `gpu_from(&[(String, f64)]) -> (Option<f32>, BTreeMap<u32, f32>)`, `disks_from(idle, read, write) -> Vec<DiskSample>`
  - `pub fn memory_totals() -> Option<MemoryTotals>`, `pub fn logical_cpus() -> u32`

- [ ] **Step 1: Write the failing unit tests** (bottom of `crates/collector/src/pdh.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn items(v: &[(&str, f64)]) -> Vec<(String, f64)> {
        v.iter().map(|(n, x)| (n.to_string(), *x)).collect()
    }

    #[test]
    fn core_instances_parse_group_and_core() {
        assert_eq!(parse_core_instance("0,3"), Some((0, 3)));
        assert_eq!(parse_core_instance("1,63"), Some((1, 63)));
        assert_eq!(parse_core_instance("_Total"), None);
        assert_eq!(parse_core_instance("0,_Total"), None);
    }

    #[test]
    fn per_core_orders_by_group_then_core_and_clamps() {
        let v = items(&[("1,0", 50.0), ("0,10", 10.0), ("0,2", 120.0), ("_Total", 40.0), ("0,_Total", 30.0)]);
        assert_eq!(per_core_from(&v), vec![100.0, 10.0, 50.0]);
        assert_eq!(total_from(&v), Some(40.0));
    }

    #[test]
    fn gpu_instance_parses_pid_and_engine() {
        let n = "pid_1234_luid_0x00000000_0x0000D1B4_phys_0_eng_3_engtype_3D";
        assert_eq!(parse_gpu_instance(n), Some((1234, "luid_0x00000000_0x0000D1B4_phys_0_eng_3".into())));
        assert_eq!(parse_gpu_instance("garbage"), None);
    }

    #[test]
    fn gpu_system_is_busiest_engine_and_process_is_its_max_engine() {
        let v = items(&[
            ("pid_1_luid_0x0_0x1_phys_0_eng_0_engtype_3D", 30.0),
            ("pid_2_luid_0x0_0x1_phys_0_eng_0_engtype_3D", 25.0),
            ("pid_2_luid_0x0_0x1_phys_0_eng_5_engtype_VideoDecode", 40.0),
        ]);
        let (system, per_pid) = gpu_from(&v);
        assert_eq!(system, Some(55.0));
        assert_eq!(per_pid.get(&1), Some(&30.0));
        assert_eq!(per_pid.get(&2), Some(&40.0));
    }

    #[test]
    fn disks_join_counters_and_skip_total() {
        let idle = items(&[("0 C:", 75.0), ("_Total", 80.0), ("1 D:", 100.0)]);
        let read = items(&[("0 C:", 1000.0), ("1 D:", 0.0)]);
        let write = items(&[("0 C:", 500.0)]);
        let disks = disks_from(&idle, &read, &write);
        assert_eq!(disks.len(), 2);
        assert_eq!(disks[0].name, "0 C:");
        assert_eq!(disks[0].active_pct, 25.0);
        assert_eq!(disks[0].read_bps, 1000.0);
        assert_eq!(disks[0].write_bps, 500.0);
        assert_eq!(disks[1].write_bps, 0.0);
    }

    #[test]
    fn unknown_counter_path_is_none_not_panic() {
        let mut query = PDH_HQUERY::default();
        // SAFETY: opening a real-time query with no data source.
        assert_eq!(unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &mut query) }, 0);
        assert!(add(query, w!("\\No Such Object(*)\\Nope")).is_none());
        // SAFETY: closing the query opened above.
        unsafe { PdhCloseQuery(query) };
    }
}
```

`crates/collector/tests/pdh.rs`:
```rust
use negotium_collector::{PdhSources, logical_cpus, memory_totals};
use std::time::Duration;

#[test]
fn reads_cpu_and_per_core_after_priming() {
    let mut pdh = PdhSources::open().expect("open pdh");
    std::thread::sleep(Duration::from_millis(1100));
    let r = pdh.read();
    let cpu = r.cpu_pct.expect("cpu available on a real machine");
    assert!((0.0..=100.0).contains(&cpu));
    assert_eq!(r.per_core_pct.len() as u32, logical_cpus());
    assert!(r.disks.is_some());
    assert!(r.net_bps.is_some());
}

#[test]
fn memory_totals_are_sane() {
    let m = memory_totals().expect("memory");
    assert!(m.total > 0 && m.used <= m.total);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p negotium-collector pdh`
Expected: FAIL to compile ("file not found for module `pdh`" after adding the `pub mod` line in Step 3, or unresolved imports before).

- [ ] **Step 3: Implement `pdh.rs`, `memory.rs`, and exports**

`crates/collector/src/pdh.rs` (above the test module):
```rust
//! PDH counter sets, read as formatted arrays. Each counter is optional: a missing
//! counter set (e.g. no GPU counters in a VM) yields `None` for that reading only.
use negotium_snapshot::DiskSample;
use std::collections::BTreeMap;
use windows::Win32::System::Performance::{
    PDH_FMT, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_FMT_NOCAP100, PDH_HCOUNTER, PDH_HQUERY,
    PDH_MORE_DATA, PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
    PdhOpenQueryW,
};
use windows::core::{PCWSTR, w};

const PDH_CSTATUS_NEW_DATA: u32 = 1;

pub struct PdhReadings {
    pub cpu_pct: Option<f32>,
    pub per_core_pct: Vec<f32>,
    pub disks: Option<Vec<DiskSample>>,
    pub net_bps: Option<f64>,
    pub gpu_pct: Option<f32>,
    pub gpu_pct_by_pid: Option<BTreeMap<u32, f32>>,
    pub errors: Vec<String>,
}

pub struct PdhSources {
    query: PDH_HQUERY,
    cpu: Option<PDH_HCOUNTER>,
    disk_idle: Option<PDH_HCOUNTER>,
    disk_read: Option<PDH_HCOUNTER>,
    disk_write: Option<PDH_HCOUNTER>,
    net: Option<PDH_HCOUNTER>,
    gpu: Option<PDH_HCOUNTER>,
}

impl PdhSources {
    pub fn open() -> Result<Self, u32> {
        let mut query = PDH_HQUERY::default();
        // SAFETY: opens a real-time query; closed in Drop.
        let status = unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &mut query) };
        if status != 0 {
            return Err(status);
        }
        let sources = Self {
            query,
            cpu: add(query, w!("\\Processor Information(*)\\% Processor Utility")),
            disk_idle: add(query, w!("\\PhysicalDisk(*)\\% Idle Time")),
            disk_read: add(query, w!("\\PhysicalDisk(*)\\Disk Read Bytes/sec")),
            disk_write: add(query, w!("\\PhysicalDisk(*)\\Disk Write Bytes/sec")),
            net: add(query, w!("\\Network Interface(*)\\Bytes Total/sec")),
            gpu: add(query, w!("\\GPU Engine(*)\\Utilization Percentage")),
        };
        // SAFETY: valid query; first sample primes rate counters.
        unsafe { PdhCollectQueryData(query) };
        Ok(sources)
    }

    pub fn read(&mut self) -> PdhReadings {
        let mut errors = Vec::new();
        // SAFETY: valid query handle owned by self.
        let status = unsafe { PdhCollectQueryData(self.query) };
        if status != 0 {
            errors.push(format!("pdh collect: {status:#010x}"));
        }
        let mut get = |label: &str, c: Option<PDH_HCOUNTER>| -> Option<Vec<(String, f64)>> {
            let c = c?;
            match read_array(c) {
                Ok(v) => Some(v),
                Err(e) => {
                    errors.push(format!("pdh {label}: {e:#010x}"));
                    None
                }
            }
        };
        let cpu = get("cpu", self.cpu);
        let idle = get("disk idle", self.disk_idle);
        let read = get("disk read", self.disk_read);
        let write = get("disk write", self.disk_write);
        let net = get("network", self.net);
        let gpu = get("gpu", self.gpu);

        let (gpu_pct, gpu_pct_by_pid) = match gpu {
            Some(v) => {
                let (sys, per) = gpu_from(&v);
                (sys, Some(per))
            }
            None => (None, None),
        };
        PdhReadings {
            cpu_pct: cpu.as_deref().and_then(total_from),
            per_core_pct: cpu.as_deref().map(per_core_from).unwrap_or_default(),
            disks: match (idle, read, write) {
                (Some(i), Some(r), Some(w)) => Some(disks_from(&i, &r, &w)),
                _ => None,
            },
            net_bps: net.map(|v| v.iter().map(|(_, x)| x.max(0.0)).sum()),
            gpu_pct,
            gpu_pct_by_pid,
            errors,
        }
    }
}

impl Drop for PdhSources {
    fn drop(&mut self) {
        // SAFETY: query opened in `open`, closed once.
        unsafe { PdhCloseQuery(self.query) };
    }
}

fn add(query: PDH_HQUERY, path: PCWSTR) -> Option<PDH_HCOUNTER> {
    let mut counter = PDH_HCOUNTER::default();
    // SAFETY: valid query; English path is locale-independent.
    let status = unsafe { PdhAddEnglishCounterW(query, path, 0, &mut counter) };
    (status == 0).then_some(counter)
}

fn read_array(counter: PDH_HCOUNTER) -> Result<Vec<(String, f64)>, u32> {
    let format = PDH_FMT(PDH_FMT_DOUBLE.0 | PDH_FMT_NOCAP100.0);
    let mut size = 0u32;
    let mut count = 0u32;
    // SAFETY: size query with no buffer.
    let status = unsafe { PdhGetFormattedCounterArrayW(counter, format, &mut size, &mut count, None) };
    if status == 0 {
        return Ok(Vec::new());
    }
    if status != PDH_MORE_DATA {
        return Err(status);
    }
    let mut buf = vec![0u64; (size as usize).div_ceil(8)];
    let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
    // SAFETY: `buf` is at least `size` bytes and 8-byte aligned.
    let status = unsafe { PdhGetFormattedCounterArrayW(counter, format, &mut size, &mut count, Some(items)) };
    if status != 0 {
        return Err(status);
    }
    // SAFETY: PDH wrote `count` items into `buf`; names point into the same buffer.
    let slice = unsafe { std::slice::from_raw_parts(items, count as usize) };
    Ok(slice
        .iter()
        .filter(|it| it.FmtValue.CStatus <= PDH_CSTATUS_NEW_DATA)
        .map(|it| {
            // SAFETY: szName is a NUL-terminated string inside `buf`; doubleValue is the
            // active union member because we requested PDH_FMT_DOUBLE.
            let name = unsafe { it.szName.to_string() }.unwrap_or_default();
            let value = unsafe { it.FmtValue.Anonymous.doubleValue };
            (name, if value.is_finite() { value } else { 0.0 })
        })
        .collect())
}

pub fn parse_core_instance(name: &str) -> Option<(u32, u32)> {
    let (group, core) = name.split_once(',')?;
    Some((group.parse().ok()?, core.parse().ok()?))
}

pub fn per_core_from(items: &[(String, f64)]) -> Vec<f32> {
    let mut cores: Vec<((u32, u32), f32)> = items
        .iter()
        .filter_map(|(n, v)| parse_core_instance(n).map(|k| (k, v.clamp(0.0, 100.0) as f32)))
        .collect();
    cores.sort_by_key(|(k, _)| *k);
    cores.into_iter().map(|(_, v)| v).collect()
}

pub fn total_from(items: &[(String, f64)]) -> Option<f32> {
    items.iter().find(|(n, _)| n == "_Total").map(|(_, v)| v.clamp(0.0, 100.0) as f32)
}

pub fn parse_gpu_instance(name: &str) -> Option<(u32, String)> {
    let rest = name.strip_prefix("pid_")?;
    let (pid, rest) = rest.split_once('_')?;
    let engine = rest.split("_engtype_").next()?;
    if !engine.starts_with("luid_") {
        return None;
    }
    Some((pid.parse().ok()?, engine.to_string()))
}

/// System GPU % = busiest engine (summed over processes); per-process % = its busiest engine.
pub fn gpu_from(items: &[(String, f64)]) -> (Option<f32>, BTreeMap<u32, f32>) {
    let mut per_engine: BTreeMap<String, f64> = BTreeMap::new();
    let mut per_pid: BTreeMap<u32, f32> = BTreeMap::new();
    for (name, value) in items {
        let Some((pid, engine)) = parse_gpu_instance(name) else { continue };
        let v = value.max(0.0);
        *per_engine.entry(engine).or_default() += v;
        let slot = per_pid.entry(pid).or_default();
        *slot = slot.max(v.min(100.0) as f32);
    }
    let system = per_engine.values().copied().reduce(f64::max).map(|v| v.min(100.0) as f32);
    (system, per_pid)
}

pub fn disks_from(idle: &[(String, f64)], read: &[(String, f64)], write: &[(String, f64)]) -> Vec<DiskSample> {
    let lookup = |set: &[(String, f64)], name: &str| {
        set.iter().find(|(n, _)| n == name).map(|(_, v)| v.max(0.0)).unwrap_or(0.0)
    };
    idle.iter()
        .filter(|(name, _)| name != "_Total")
        .map(|(name, idle_pct)| DiskSample {
            name: name.clone(),
            active_pct: (100.0 - idle_pct).clamp(0.0, 100.0) as f32,
            read_bps: lookup(read, name),
            write_bps: lookup(write, name),
        })
        .collect()
}
```

`crates/collector/src/memory.rs`:
```rust
//! Physical memory totals and logical processor count.
use negotium_snapshot::MemoryTotals;
use std::mem::size_of;
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::{ALL_PROCESSOR_GROUPS, GetActiveProcessorCount};

pub fn memory_totals() -> Option<MemoryTotals> {
    let mut status = MEMORYSTATUSEX { dwLength: size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    // SAFETY: `status` is initialized with its size as the API requires.
    unsafe { GlobalMemoryStatusEx(&mut status) }.ok()?;
    Some(MemoryTotals { total: status.ullTotalPhys, used: status.ullTotalPhys.saturating_sub(status.ullAvailPhys) })
}

/// Logical processors across all processor groups (matters above 64 cores).
pub fn logical_cpus() -> u32 {
    // SAFETY: simple query with no pointers.
    let n = unsafe { GetActiveProcessorCount(ALL_PROCESSOR_GROUPS) };
    n.max(1)
}
```

In `crates/collector/src/lib.rs` add:
```rust
pub mod memory;
pub mod pdh;

pub use memory::{logical_cpus, memory_totals};
pub use pdh::{PdhReadings, PdhSources};
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p negotium-collector pdh` then `cargo test -p negotium-collector --test pdh`
Expected: 6 unit tests and 2 integration tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/collector
git commit -m "feat(collector): read CPU, disk, network, GPU counters and memory totals"
```

---

### Task 6: Collector::tick assembles a Snapshot

**Files:**
- Create: `crates/collector/src/collector.rs`
- Modify: `crates/collector/src/lib.rs`
- Test: `crates/collector/tests/collector.rs`

**Interfaces:**
- Consumes: `ProcessReader`, `IdentityCache`, `scan_windows`, `PdhSources`, `memory_totals`, `logical_cpus` (Tasks 2–5); `negotium_snapshot::*`.
- Produces: `pub struct Collector` with `pub fn new() -> Self` and `pub fn tick(&mut self) -> Snapshot`. Not `Send` (owns PDH handles).

- [ ] **Step 1: Write the failing test**

`crates/collector/tests/collector.rs`:
```rust
use negotium_collector::Collector;
use std::time::Duration;

#[test]
fn ticks_produce_complete_snapshots() {
    let mut c = Collector::new();
    let first = c.tick();
    std::thread::sleep(Duration::from_millis(1000));
    let second = c.tick();

    assert!(second.taken_at_ms >= first.taken_at_ms + 900);
    assert!(second.processes.len() > 20);
    assert!(!second.source_errors.iter().any(|e| e.starts_with("processes")), "{:?}", second.source_errors);

    let me = second.processes.iter().find(|p| p.pid == std::process::id()).expect("self");
    assert!(me.image_path.is_some());
    assert!(second.totals.logical_cpus >= 1);
    assert!(second.totals.memory.is_some());
    assert!(second.totals.cpu_pct.is_some());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p negotium-collector --test collector`
Expected: FAIL to compile ("no `Collector` in the root").

- [ ] **Step 3: Implement `collector.rs` and export it**

```rust
//! One tick = one consistent Snapshot. Each source fails independently.
use crate::{IdentityCache, PdhSources, ProcessReader, logical_cpus, memory_totals, scan_windows};
use negotium_snapshot::{ProcessSample, Snapshot, SystemTotals};
use std::collections::HashSet;
use std::time::Instant;

pub struct Collector {
    processes: ProcessReader,
    identity: IdentityCache,
    pdh: Result<PdhSources, u32>,
    started: Instant,
    logical_cpus: u32,
}

impl Default for Collector {
    fn default() -> Self {
        Self::new()
    }
}

impl Collector {
    pub fn new() -> Self {
        Self {
            processes: ProcessReader::new(),
            identity: IdentityCache::new(),
            pdh: PdhSources::open(),
            started: Instant::now(),
            logical_cpus: logical_cpus(),
        }
    }

    pub fn tick(&mut self) -> Snapshot {
        let taken_at_ms = self.started.elapsed().as_millis() as u64;
        let mut source_errors = Vec::new();

        let raw = self.processes.read().unwrap_or_else(|status| {
            source_errors.push(format!("processes: NTSTATUS {status:#010x}"));
            Vec::new()
        });
        let alive: HashSet<(u32, i64)> = raw.iter().map(|p| (p.pid, p.create_time)).collect();
        self.identity.retain_alive(&alive);
        let processes = raw
            .into_iter()
            .map(|p| {
                let id = self.identity.get(p.pid, p.create_time).clone();
                ProcessSample {
                    pid: p.pid,
                    parent_pid: p.parent_pid,
                    create_time: p.create_time,
                    name: p.name,
                    session_id: p.session_id,
                    cpu_time_100ns: p.cpu_time_100ns,
                    private_working_set: p.private_working_set,
                    disk_io_bytes: p.disk_io_bytes,
                    threads: p.threads,
                    handles: p.handles,
                    image_path: id.image_path,
                    package_family: id.package_family,
                }
            })
            .collect();

        let mut totals = SystemTotals { logical_cpus: self.logical_cpus, memory: memory_totals(), ..Default::default() };
        if totals.memory.is_none() {
            source_errors.push("memory: GlobalMemoryStatusEx failed".into());
        }
        match &mut self.pdh {
            Ok(pdh) => {
                let r = pdh.read();
                totals.cpu_pct = r.cpu_pct;
                totals.per_core_pct = r.per_core_pct;
                totals.disks = r.disks;
                totals.net_bps = r.net_bps;
                totals.gpu_pct = r.gpu_pct;
                totals.gpu_pct_by_pid = r.gpu_pct_by_pid;
                source_errors.extend(r.errors);
            }
            Err(status) => source_errors.push(format!("pdh open: {status:#010x}")),
        }

        Snapshot { taken_at_ms, processes, windows: scan_windows(), totals, source_errors }
    }
}
```

In `crates/collector/src/lib.rs` add:
```rust
mod collector;

pub use collector::Collector;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p negotium-collector --test collector`
Expected: 1 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/collector
git commit -m "feat(collector): assemble snapshots with per-source failure isolation"
```

---

### Task 7: Model — CPU % and disk rates from deltas

**Files:**
- Create: `crates/model/Cargo.toml`, `crates/model/src/lib.rs`, `crates/model/src/rates.rs`
- Test: unit tests in `rates.rs`

**Interfaces:**
- Consumes: `negotium_snapshot::Snapshot`.
- Produces:
  - `pub struct ProcRates { pub cpu_pct: f32, pub disk_bps: f64 }` (derives `Clone, Copy, Debug, Default, PartialEq`)
  - `pub struct RateTracker` with `pub fn new() -> Self` and `pub fn update(&mut self, snap: &Snapshot) -> HashMap<(u32, i64), ProcRates>`

- [ ] **Step 1: Create the crate**

`crates/model/Cargo.toml`:
```toml
[package]
name = "negotium-model"
edition.workspace = true
version.workspace = true
license.workspace = true
publish.workspace = true

[dependencies]
negotium-snapshot.workspace = true
serde.workspace = true
```

`crates/model/src/lib.rs`:
```rust
//! Pure logic over snapshots: no Windows calls, fully unit-testable.
pub mod rates;

pub use rates::{ProcRates, RateTracker};
```

- [ ] **Step 2: Write the failing tests** (bottom of `crates/model/src/rates.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use negotium_snapshot::{ProcessSample, SystemTotals};

    fn proc(pid: u32, create: i64, cpu_100ns: u64, io: u64) -> ProcessSample {
        ProcessSample {
            pid, parent_pid: 0, create_time: create, name: format!("p{pid}.exe"), session_id: 1,
            cpu_time_100ns: cpu_100ns, private_working_set: 0, disk_io_bytes: io, threads: 1,
            handles: 0, image_path: None, package_family: None,
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
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p negotium-model`
Expected: FAIL to compile ("cannot find type `RateTracker`").

- [ ] **Step 4: Implement `rates.rs`** (above the test module)

```rust
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
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p negotium-model`
Expected: 5 passed.

- [ ] **Step 6: Commit**

```bash
git add crates/model Cargo.lock
git commit -m "feat(model): compute per-process CPU and disk rates from deltas"
```

---

### Task 8: Model — group processes into apps and aggregate rows

**Files:**
- Create: `crates/model/src/grouping.rs`
- Modify: `crates/model/src/lib.rs`
- Test: unit tests in `grouping.rs` and `lib.rs`

**Interfaces:**
- Consumes: `RateTracker`, `ProcRates` (Task 7); `negotium_snapshot::*`.
- Produces:
  - `pub enum AppKey { Package(String), Window { pid: u32, create_time: i64 }, Windows, Background { pid: u32, create_time: i64 } }` (derives `Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize`)
  - `pub enum AppKind { App, Windows, Background }` (derives `Clone, Copy, Debug, PartialEq, Eq`)
  - `impl AppKey { pub fn kind(&self) -> AppKind }`
  - `pub const SHELL_HOSTS: &[&str] = &["explorer.exe"]`
  - `pub fn group(processes: &[ProcessSample], windows: &[WindowSample]) -> HashMap<(u32, i64), AppKey>` (PID 0 excluded)
  - `pub fn display_name(key: &AppKey, processes: &[ProcessSample]) -> String`
  - `pub struct ProcRow { pub pid: u32, pub create_time: i64, pub name: String, pub cpu_pct: f32, pub private_bytes: u64, pub disk_bps: f64, pub gpu_pct: Option<f32> }`
  - `pub struct AppRow { pub key: AppKey, pub name: String, pub kind: AppKind, pub processes: Vec<ProcRow>, pub cpu_pct: f32, pub private_bytes: u64, pub disk_bps: f64, pub gpu_pct: Option<f32>, pub net_bps: Option<f64>, pub hung_for_ms: Option<u64> }` (both derive `Clone, Debug, PartialEq`)
  - `pub struct Model` with `pub fn new() -> Self` and `pub fn update(&mut self, snap: &Snapshot) -> Vec<AppRow>` (`hung_for_ms` is `None` until Task 9; `net_bps` is `None` in this plan)

- [ ] **Step 1: Write the failing grouping tests** (bottom of `crates/model/src/grouping.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: u32, parent: u32, create: i64, name: &str, session: u32) -> ProcessSample {
        ProcessSample {
            pid, parent_pid: parent, create_time: create, name: name.into(), session_id: session,
            cpu_time_100ns: 0, private_working_set: 0, disk_io_bytes: 0, threads: 1, handles: 0,
            image_path: None, package_family: None,
        }
    }
    fn win(pid: u32) -> WindowSample {
        WindowSample { pid, hung: false, title: format!("w{pid}") }
    }

    #[test]
    fn browser_children_join_the_window_owner() {
        let procs = vec![
            p(10, 1, 5, "explorer.exe", 1),
            p(20, 10, 10, "chrome.exe", 1),
            p(21, 20, 11, "chrome.exe", 1),
            p(22, 21, 12, "chrome.exe", 1),
        ];
        let g = group(&procs, &[win(10), win(20)]);
        let chrome = AppKey::Window { pid: 20, create_time: 10 };
        assert_eq!(g[&(21, 11)], chrome);
        assert_eq!(g[&(22, 12)], chrome);
        assert_eq!(g[&(20, 10)], chrome);
        assert_eq!(g[&(10, 5)], AppKey::Window { pid: 10, create_time: 5 });
    }

    #[test]
    fn windowless_shell_children_are_background_not_explorer() {
        let procs = vec![p(10, 1, 5, "explorer.exe", 1), p(30, 10, 20, "tray.exe", 1)];
        let g = group(&procs, &[win(10)]);
        assert_eq!(g[&(30, 20)], AppKey::Background { pid: 30, create_time: 20 });
    }

    #[test]
    fn reused_parent_pid_is_not_adopted() {
        // Parent PID 40 now belongs to a process created after the child.
        let procs = vec![p(40, 1, 999, "editor.exe", 1), p(41, 40, 100, "orphan.exe", 1)];
        let g = group(&procs, &[win(40)]);
        assert_eq!(g[&(41, 100)], AppKey::Background { pid: 41, create_time: 100 });
    }

    #[test]
    fn parent_cycles_terminate() {
        let procs = vec![p(50, 51, 10, "a.exe", 1), p(51, 50, 10, "b.exe", 1)];
        let g = group(&procs, &[]);
        assert_eq!(g[&(50, 10)], AppKey::Background { pid: 50, create_time: 10 });
        assert_eq!(g[&(51, 10)], AppKey::Background { pid: 51, create_time: 10 });
    }

    #[test]
    fn services_packages_and_idle() {
        let mut pkg = p(60, 1, 1, "WindowsTerminal.exe", 1);
        pkg.package_family = Some("Microsoft.WindowsTerminal_8wekyb3d8bbwe".into());
        let procs = vec![p(0, 0, 0, "System Idle Process", 0), p(4, 0, 0, "System", 0), p(70, 4, 2, "svchost.exe", 0), pkg];
        let g = group(&procs, &[win(60)]);
        assert!(!g.contains_key(&(0, 0)));
        assert_eq!(g[&(4, 0)], AppKey::Windows);
        assert_eq!(g[&(70, 2)], AppKey::Windows);
        let key = AppKey::Package("Microsoft.WindowsTerminal_8wekyb3d8bbwe".into());
        assert_eq!(g[&(60, 1)], key);
        assert_eq!(display_name(&key, &procs), "Microsoft.WindowsTerminal");
        assert_eq!(display_name(&AppKey::Window { pid: 70, create_time: 2 }, &procs), "svchost");
        assert_eq!(display_name(&AppKey::Windows, &procs), "Windows");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p negotium-model grouping`
Expected: FAIL to compile ("cannot find function `group`").

- [ ] **Step 3: Implement `grouping.rs`** (above its tests)

```rust
//! Process → app grouping rules (spec §4.3), applied in order:
//! 1. packaged → package family; 2. nearest window-owning ancestor (stopping at shell
//! hosts and at parents created after the child); 3. session 0 → "Windows"; 4. Background.
use negotium_snapshot::{ProcessSample, WindowSample};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

pub const SHELL_HOSTS: &[&str] = &["explorer.exe"];
const MAX_ANCESTRY_DEPTH: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum AppKey {
    Package(String),
    Window { pid: u32, create_time: i64 },
    Windows,
    Background { pid: u32, create_time: i64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppKind {
    App,
    Windows,
    Background,
}

impl AppKey {
    pub fn kind(&self) -> AppKind {
        match self {
            AppKey::Package(_) | AppKey::Window { .. } => AppKind::App,
            AppKey::Windows => AppKind::Windows,
            AppKey::Background { .. } => AppKind::Background,
        }
    }
}

pub fn group(processes: &[ProcessSample], windows: &[WindowSample]) -> HashMap<(u32, i64), AppKey> {
    let by_pid: HashMap<u32, &ProcessSample> = processes.iter().map(|p| (p.pid, p)).collect();
    let window_pids: HashSet<u32> = windows.iter().map(|w| w.pid).collect();
    processes
        .iter()
        .filter(|p| p.pid != 0)
        .map(|p| {
            let key = if let Some(pfn) = &p.package_family {
                AppKey::Package(pfn.clone())
            } else if let Some((pid, create_time)) = window_root(p, &by_pid, &window_pids) {
                AppKey::Window { pid, create_time }
            } else if p.session_id == 0 {
                AppKey::Windows
            } else {
                AppKey::Background { pid: p.pid, create_time: p.create_time }
            };
            ((p.pid, p.create_time), key)
        })
        .collect()
}

fn window_root(
    p: &ProcessSample,
    by_pid: &HashMap<u32, &ProcessSample>,
    window_pids: &HashSet<u32>,
) -> Option<(u32, i64)> {
    let mut cur = p;
    for _ in 0..MAX_ANCESTRY_DEPTH {
        if window_pids.contains(&cur.pid) {
            return Some((cur.pid, cur.create_time));
        }
        let parent = by_pid.get(&cur.parent_pid)?;
        if parent.pid == cur.pid || parent.create_time > cur.create_time || is_shell_host(parent) {
            return None;
        }
        cur = parent;
    }
    None
}

fn is_shell_host(p: &ProcessSample) -> bool {
    SHELL_HOSTS.iter().any(|h| p.name.eq_ignore_ascii_case(h))
}

pub fn display_name(key: &AppKey, processes: &[ProcessSample]) -> String {
    let process_name = |pid: u32, create_time: i64| {
        processes
            .iter()
            .find(|p| p.pid == pid && p.create_time == create_time)
            .map(|p| strip_exe(&p.name))
            .unwrap_or_else(|| format!("<pid {pid}>"))
    };
    match key {
        AppKey::Package(pfn) => pfn.split('_').next().unwrap_or(pfn).to_string(),
        AppKey::Window { pid, create_time } | AppKey::Background { pid, create_time } => {
            process_name(*pid, *create_time)
        }
        AppKey::Windows => "Windows".to_string(),
    }
}

fn strip_exe(name: &str) -> String {
    name.strip_suffix(".exe").or_else(|| name.strip_suffix(".EXE")).unwrap_or(name).to_string()
}
```

- [ ] **Step 4: Run grouping tests to verify they pass**

Run: `cargo test -p negotium-model grouping`
Expected: 5 passed.

- [ ] **Step 5: Write the failing aggregation test** (bottom of `crates/model/src/lib.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use negotium_snapshot::{ProcessSample, Snapshot, SystemTotals, WindowSample};
    use std::collections::BTreeMap;

    fn p(pid: u32, parent: u32, cpu: u64, mem: u64) -> ProcessSample {
        ProcessSample {
            pid, parent_pid: parent, create_time: pid as i64, name: format!("app{pid}.exe"), session_id: 1,
            cpu_time_100ns: cpu, private_working_set: mem, disk_io_bytes: 0, threads: 1, handles: 0,
            image_path: None, package_family: None,
        }
    }

    #[test]
    fn rows_sum_their_processes() {
        let mut m = Model::new();
        let mut gpu = BTreeMap::new();
        gpu.insert(2, 30.0);
        gpu.insert(3, 50.0);
        let snap = |t: u64, cpu2: u64, cpu3: u64| Snapshot {
            taken_at_ms: t,
            processes: vec![p(2, 1, cpu2, 100), p(3, 2, cpu3, 50)],
            windows: vec![WindowSample { pid: 2, hung: false, title: "App".into() }],
            totals: SystemTotals { logical_cpus: 1, gpu_pct_by_pid: Some(gpu.clone()), ..Default::default() },
            source_errors: vec![],
        };
        m.update(&snap(0, 0, 0));
        let rows = m.update(&snap(1000, 2_000_000, 3_000_000));
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.name, "app2");
        assert_eq!(row.processes.len(), 2);
        assert!((row.cpu_pct - 50.0).abs() < 0.01);
        assert_eq!(row.private_bytes, 150);
        assert_eq!(row.gpu_pct, Some(80.0));
        assert_eq!(row.net_bps, None);
    }
}
```

- [ ] **Step 6: Run test to verify it fails**

Run: `cargo test -p negotium-model tests::rows_sum_their_processes`
Expected: FAIL to compile ("cannot find type `Model`").

- [ ] **Step 7: Implement `Model`, `AppRow`, `ProcRow` in `lib.rs`**

Replace `crates/model/src/lib.rs` content above its tests with:
```rust
//! Pure logic over snapshots: no Windows calls, fully unit-testable.
pub mod grouping;
pub mod rates;

pub use grouping::{AppKey, AppKind, SHELL_HOSTS, display_name, group};
pub use rates::{ProcRates, RateTracker};

use negotium_snapshot::Snapshot;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub struct ProcRow {
    pub pid: u32,
    pub create_time: i64,
    pub name: String,
    pub cpu_pct: f32,
    pub private_bytes: u64,
    pub disk_bps: f64,
    pub gpu_pct: Option<f32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AppRow {
    pub key: AppKey,
    pub name: String,
    pub kind: AppKind,
    pub processes: Vec<ProcRow>,
    pub cpu_pct: f32,
    pub private_bytes: u64,
    pub disk_bps: f64,
    pub gpu_pct: Option<f32>,
    /// Per-app network needs the SRUM source (Plan 4); `None` until then.
    pub net_bps: Option<f64>,
    pub hung_for_ms: Option<u64>,
}

#[derive(Default)]
pub struct Model {
    rates: RateTracker,
}

impl Model {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn update(&mut self, snap: &Snapshot) -> Vec<AppRow> {
        let rates = self.rates.update(snap);
        let groups = group(&snap.processes, &snap.windows);
        let gpu = snap.totals.gpu_pct_by_pid.as_ref();

        let mut rows: BTreeMap<AppKey, AppRow> = BTreeMap::new();
        for p in &snap.processes {
            let Some(key) = groups.get(&(p.pid, p.create_time)) else { continue };
            let r = rates.get(&(p.pid, p.create_time)).copied().unwrap_or_default();
            let proc_gpu = gpu.map(|m| m.get(&p.pid).copied().unwrap_or(0.0));
            let row = rows.entry(key.clone()).or_insert_with(|| AppRow {
                key: key.clone(),
                name: display_name(key, &snap.processes),
                kind: key.kind(),
                processes: Vec::new(),
                cpu_pct: 0.0,
                private_bytes: 0,
                disk_bps: 0.0,
                gpu_pct: gpu.map(|_| 0.0),
                net_bps: None,
                hung_for_ms: None,
            });
            row.cpu_pct = (row.cpu_pct + r.cpu_pct).min(100.0);
            row.private_bytes += p.private_working_set;
            row.disk_bps += r.disk_bps;
            if let (Some(total), Some(g)) = (row.gpu_pct.as_mut(), proc_gpu) {
                *total = (*total + g).min(100.0);
            }
            row.processes.push(ProcRow {
                pid: p.pid,
                create_time: p.create_time,
                name: p.name.clone(),
                cpu_pct: r.cpu_pct,
                private_bytes: p.private_working_set,
                disk_bps: r.disk_bps,
                gpu_pct: proc_gpu,
            });
        }
        rows.into_values().collect()
    }
}
```

- [ ] **Step 8: Run all model tests**

Run: `cargo test -p negotium-model`
Expected: 11 passed.

- [ ] **Step 9: Commit**

```bash
git add crates/model
git commit -m "feat(model): group processes into apps and aggregate app rows"
```

---

### Task 9: Model — hung tracking, metric ranking, top-N

**Files:**
- Create: `crates/model/src/hung.rs`, `crates/model/src/ranking.rs`
- Modify: `crates/model/src/lib.rs`
- Test: unit tests in `hung.rs` and `ranking.rs`

**Interfaces:**
- Consumes: `AppRow`, `AppKey`, `Model` (Task 8).
- Produces:
  - `pub struct HungTracker` with `pub fn new() -> Self` and `pub fn update(&mut self, windows: &[WindowSample], now_ms: u64) -> HashMap<u32, u64>` (pid → ms hung)
  - `Model::update` now fills `AppRow::hung_for_ms` (max over the app's processes)
  - `#[repr(usize)] pub enum Metric { Cpu = 0, Memory = 1, Disk = 2, Gpu = 3, Network = 4 }` with `pub const ALL: [Metric; 5]` (derives `Clone, Copy, Debug, PartialEq, Eq, Hash`)
  - `impl AppRow { pub fn value(&self, metric: Metric) -> f64 }`
  - `pub const RESORT_INTERVAL_MS: u64 = 2000`
  - `pub struct Ranker` with `pub fn new() -> Self` and `pub fn order(&mut self, rows: &[AppRow], metric: Metric, now_ms: u64, frozen: bool) -> Vec<usize>`
  - `pub fn top_n(rows: &[AppRow], metric: Metric, n: usize) -> Vec<usize>`

- [ ] **Step 1: Write the failing hung tests** (bottom of `crates/model/src/hung.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn w(pid: u32, hung: bool) -> WindowSample {
        WindowSample { pid, hung, title: "t".into() }
    }

    #[test]
    fn tracks_time_since_first_hung_and_clears_on_recovery() {
        let mut t = HungTracker::new();
        assert!(t.update(&[w(1, false)], 0).is_empty());
        assert_eq!(t.update(&[w(1, true)], 1000)[&1], 0);
        assert_eq!(t.update(&[w(1, true), w(1, false)], 15_000)[&1], 14_000);
        assert!(t.update(&[w(1, false)], 16_000).is_empty());
        assert_eq!(t.update(&[w(1, true)], 20_000)[&1], 0);
    }
}
```

- [ ] **Step 2: Implement `hung.rs`** (above its tests) and wire it into `Model`

```rust
//! Remembers when each process first had a hung window.
use negotium_snapshot::WindowSample;
use std::collections::{HashMap, HashSet};

#[derive(Default)]
pub struct HungTracker {
    since: HashMap<u32, u64>,
}

impl HungTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn update(&mut self, windows: &[WindowSample], now_ms: u64) -> HashMap<u32, u64> {
        let hung: HashSet<u32> = windows.iter().filter(|w| w.hung).map(|w| w.pid).collect();
        self.since.retain(|pid, _| hung.contains(pid));
        for pid in &hung {
            self.since.entry(*pid).or_insert(now_ms);
        }
        self.since.iter().map(|(pid, t)| (*pid, now_ms.saturating_sub(*t))).collect()
    }
}
```

In `crates/model/src/lib.rs`: add `pub mod hung;` and `pub use hung::HungTracker;`, add a `hung: HungTracker` field to `Model`, and in `Model::update` compute `let hung = self.hung.update(&snap.windows, snap.taken_at_ms);` before the loop; inside the loop after pushing the `ProcRow` add:
```rust
            if let Some(&ms) = hung.get(&p.pid) {
                row.hung_for_ms = Some(row.hung_for_ms.map_or(ms, |cur| cur.max(ms)));
            }
```

Add to the `lib.rs` test module:
```rust
    #[test]
    fn rows_report_hung_duration() {
        let mut m = Model::new();
        let snap = |t: u64, hung: bool| Snapshot {
            taken_at_ms: t,
            processes: vec![p(2, 1, 0, 0)],
            windows: vec![WindowSample { pid: 2, hung, title: "App".into() }],
            totals: SystemTotals { logical_cpus: 1, ..Default::default() },
            source_errors: vec![],
        };
        m.update(&snap(0, true));
        assert_eq!(m.update(&snap(3000, true))[0].hung_for_ms, Some(3000));
        assert_eq!(m.update(&snap(4000, false))[0].hung_for_ms, None);
    }
```

- [ ] **Step 3: Run hung tests to verify they pass**

Run: `cargo test -p negotium-model`
Expected: 13 passed.

- [ ] **Step 4: Write the failing ranking tests** (bottom of `crates/model/src/ranking.rs`)

```rust
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
```

- [ ] **Step 5: Run tests to verify they fail**

Run: `cargo test -p negotium-model ranking`
Expected: FAIL to compile ("cannot find type `Ranker`").

- [ ] **Step 6: Implement `ranking.rs`** (above its tests) and export it

```rust
//! Stable ordering for the app list: re-sorts at most every RESORT_INTERVAL_MS,
//! never while frozen (pointer over the list), immediately on metric change, and
//! immediately (when not frozen) if the set of hung apps changes. Hung apps lead.
use crate::{AppKey, AppRow};
use std::cmp::Ordering;
use std::collections::HashMap;

pub const RESORT_INTERVAL_MS: u64 = 2000;

#[repr(usize)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Metric {
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
```

In `crates/model/src/lib.rs` add: `pub mod ranking;` and `pub use ranking::{Metric, RESORT_INTERVAL_MS, Ranker, top_n};`

- [ ] **Step 7: Run all model tests**

Run: `cargo test -p negotium-model`
Expected: 19 passed.

- [ ] **Step 8: Commit**

```bash
git add crates/model
git commit -m "feat(model): track hung apps and rank rows stably by metric"
```

---

### Task 10: History ring buffer

**Files:**
- Create: `crates/history/Cargo.toml`, `crates/history/src/lib.rs`
- Test: unit tests in `lib.rs`

**Interfaces:**
- Consumes: `negotium_snapshot::SystemTotals`; `negotium_model::{AppKey, AppRow, Metric, top_n}`.
- Produces:
  - `pub const DEFAULT_CAPACITY: usize = 3600`, `pub const TOP_PER_METRIC: usize = 10`
  - `pub struct HistorySample { pub t_ms: u64, pub cpu_pct: Option<f32>, pub per_core_pct: Vec<f32>, pub mem_used: Option<u64>, pub disk_active_pct: Option<f32>, pub gpu_pct: Option<f32>, pub net_bps: Option<f64>, pub top: [Vec<(u32, f32)>; 5] }` (`top[metric as usize]` = interned app id + value)
  - `pub struct History` with `pub fn new(capacity: usize) -> Self`, `pub fn push(&mut self, t_ms: u64, totals: &SystemTotals, rows: &[AppRow])`, `pub fn len(&self) -> usize`, `pub fn is_empty(&self) -> bool`, `pub fn since(&self, t_ms: u64) -> impl Iterator<Item = &HistorySample>`, `pub fn at(&self, t_ms: u64) -> Option<&HistorySample>` (nearest), `pub fn top_at(&self, t_ms: u64, metric: Metric) -> Vec<(String, f32)>`, `pub fn approx_bytes(&self) -> usize`

- [ ] **Step 1: Create the crate manifest**

`crates/history/Cargo.toml`:
```toml
[package]
name = "negotium-history"
edition.workspace = true
version.workspace = true
license.workspace = true
publish.workspace = true

[dependencies]
negotium-snapshot.workspace = true
negotium-model.workspace = true
```

- [ ] **Step 2: Write the failing tests** (`crates/history/src/lib.rs`, tests only)

```rust
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
        SystemTotals { logical_cpus: cores as u32, cpu_pct: Some(cpu), per_core_pct: vec![cpu; cores], ..Default::default() }
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
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p negotium-history`
Expected: FAIL to compile ("cannot find type `History`").

- [ ] **Step 4: Implement the history buffer** (above the tests)

```rust
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
    pub per_core_pct: Vec<f32>,
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
}

impl History {
    pub fn new(capacity: usize) -> Self {
        Self { samples: VecDeque::with_capacity(capacity), capacity: capacity.max(1), ids: HashMap::new(), names: Vec::new() }
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
            self.samples.pop_front();
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
            self.names[id as usize] = row.name.clone();
            return id;
        }
        let id = self.names.len() as u32;
        self.ids.insert(row.key.clone(), id);
        self.names.push(row.name.clone());
        id
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
                    + s.per_core_pct.capacity() * size_of::<f32>()
                    + s.top.iter().map(|t| t.capacity() * size_of::<(u32, f32)>()).sum::<usize>()
            })
            .sum();
        let names: usize = self.names.iter().map(|n| n.capacity() + size_of::<String>() + size_of::<AppKey>() + 8).sum();
        samples + names
    }
}
```

Note: `top_at_resolves_names_and_interns_keys` reads the private `names` field; the test module is inside the crate, so that is allowed.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p negotium-history`
Expected: 4 passed.

- [ ] **Step 6: Commit**

```bash
git add crates/history Cargo.lock
git commit -m "feat(history): add bounded history with interned top apps"
```

---

### Task 11: Probe CLI, accuracy check, and tick timing

**Files:**
- Create: `crates/probe/Cargo.toml`, `crates/probe/src/main.rs`
- Create: `crates/collector/tests/accuracy.rs`
- Modify: `crates/collector/Cargo.toml` (dev-dependency on `negotium-model`)
- Create: `docs/benchmarks/README.md`

**Interfaces:**
- Consumes: `Collector` (Task 6), `Model`, `Ranker`, `Metric` (Tasks 8–9), `History` (Task 10).
- Produces: binary `negotium-probe [--metric cpu|memory|disk|gpu] [--ticks N] [--interval-ms MS] [--timing] [--record FILE.jsonl]`.

- [ ] **Step 1: Write the failing accuracy test**

Add to `crates/collector/Cargo.toml` under `[dev-dependencies]`:
```toml
negotium-model.workspace = true
```

`crates/collector/tests/accuracy.rs`:
```rust
//! One busy thread should show as ~100 / logical_cpus percent, like Task Manager.
use negotium_collector::Collector;
use negotium_model::Model;
use std::process::Command;
use std::time::Duration;

#[test]
fn busy_child_reports_one_core_of_cpu() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_negotium-burn")).arg("5000").spawn().unwrap();
    let mut collector = Collector::new();
    let mut model = Model::new();
    std::thread::sleep(Duration::from_millis(500));
    model.update(&collector.tick());
    std::thread::sleep(Duration::from_millis(2000));
    let snap = collector.tick();
    let rows = model.update(&snap);
    child.kill().ok();
    child.wait().ok();

    let proc_row = rows.iter().flat_map(|r| &r.processes).find(|p| p.pid == child.id()).expect("child row");
    let expected = 100.0 / snap.totals.logical_cpus as f32;
    assert!(
        (proc_row.cpu_pct - expected).abs() <= expected * 0.35,
        "got {} %, expected about {} %", proc_row.cpu_pct, expected
    );
    let system = snap.totals.cpu_pct.expect("system cpu");
    assert!(system + 5.0 >= proc_row.cpu_pct, "system {system} below process {}", proc_row.cpu_pct);
}
```

- [ ] **Step 2: Run the accuracy test**

Run: `cargo test -p negotium-collector --test accuracy`
Expected: PASS (all collector and model code exists). If it fails, stop and use superpowers:systematic-debugging; do not loosen the tolerance without recording why in `docs/benchmarks/README.md`.

- [ ] **Step 3: Create the probe crate**

`crates/probe/Cargo.toml`:
```toml
[package]
name = "negotium-probe"
edition.workspace = true
version.workspace = true
license.workspace = true
publish.workspace = true

[dependencies]
negotium-collector.workspace = true
negotium-model.workspace = true
negotium-history.workspace = true
negotium-snapshot.workspace = true
serde_json.workspace = true
anyhow.workspace = true
```

`crates/probe/src/main.rs`:
```rust
//! Console live view of the data core. Prints system totals and the top 10 apps
//! each tick; optionally records snapshots (JSON lines) and tick timings.
use anyhow::{Context, Result, bail};
use negotium_collector::Collector;
use negotium_history::{DEFAULT_CAPACITY, History};
use negotium_model::{Metric, Model, Ranker};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::time::{Duration, Instant};

struct Args {
    metric: Metric,
    ticks: Option<u64>,
    interval: Duration,
    timing: bool,
    record: Option<String>,
}

fn parse_args() -> Result<Args> {
    let mut args = Args { metric: Metric::Cpu, ticks: None, interval: Duration::from_millis(1000), timing: false, record: None };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--metric" => {
                args.metric = match it.next().as_deref() {
                    Some("cpu") => Metric::Cpu,
                    Some("memory") => Metric::Memory,
                    Some("disk") => Metric::Disk,
                    Some("gpu") => Metric::Gpu,
                    other => bail!("unknown metric {other:?} (use cpu|memory|disk|gpu)"),
                }
            }
            "--ticks" => args.ticks = Some(it.next().context("--ticks needs a number")?.parse()?),
            "--interval-ms" => args.interval = Duration::from_millis(it.next().context("--interval-ms needs a number")?.parse()?),
            "--timing" => args.timing = true,
            "--record" => args.record = Some(it.next().context("--record needs a path")?),
            other => bail!("unknown argument {other}"),
        }
    }
    Ok(args)
}

fn fmt_bytes(b: f64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    format!("{v:.1} {}", UNITS[u])
}

fn main() -> Result<()> {
    let args = parse_args()?;
    let mut collector = Collector::new();
    let mut model = Model::new();
    let mut ranker = Ranker::new();
    let mut history = History::new(DEFAULT_CAPACITY);
    let mut recorder = match &args.record {
        Some(path) => Some(BufWriter::new(File::create(path).with_context(|| format!("create {path}"))?)),
        None => None,
    };
    let mut tick_ms: Vec<f64> = Vec::new();
    let mut n = 0u64;

    loop {
        let started = Instant::now();
        let snap = collector.tick();
        tick_ms.push(started.elapsed().as_secs_f64() * 1000.0);
        let rows = model.update(&snap);
        history.push(snap.taken_at_ms, &snap.totals, &rows);
        let order = ranker.order(&rows, args.metric, snap.taken_at_ms, false);

        let t = &snap.totals;
        println!(
            "\n[{:>7} ms] CPU {:>5.1}%  Mem {}/{}  Disk {:>5.1}%  GPU {:>5.1}%  Net {}/s  ({} procs, {} apps)",
            snap.taken_at_ms,
            t.cpu_pct.unwrap_or(f32::NAN),
            fmt_bytes(t.memory.as_ref().map_or(0, |m| m.used) as f64),
            fmt_bytes(t.memory.as_ref().map_or(0, |m| m.total) as f64),
            t.busiest_disk_pct().unwrap_or(f32::NAN),
            t.gpu_pct.unwrap_or(f32::NAN),
            fmt_bytes(t.net_bps.unwrap_or(0.0)),
            snap.processes.len(),
            rows.len(),
        );
        for &i in order.iter().take(10) {
            let r = &rows[i];
            let hung = r.hung_for_ms.map(|ms| format!("  NOT RESPONDING {}s", ms / 1000)).unwrap_or_default();
            println!(
                "  {:<28} {:>3} procs  CPU {:>5.1}%  Mem {:>10}  Disk {:>10}/s  GPU {:>5.1}%{}",
                r.name, r.processes.len(), r.cpu_pct, fmt_bytes(r.private_bytes as f64), fmt_bytes(r.disk_bps),
                r.gpu_pct.unwrap_or(0.0), hung
            );
        }
        if !snap.source_errors.is_empty() {
            println!("  source errors: {:?}", snap.source_errors);
        }
        if let Some(w) = recorder.as_mut() {
            serde_json::to_writer(&mut *w, &snap)?;
            w.write_all(b"\n")?;
        }

        n += 1;
        if args.ticks.is_some_and(|max| n >= max) {
            break;
        }
        std::thread::sleep(args.interval.saturating_sub(started.elapsed()));
    }

    if args.timing {
        let mut sorted = tick_ms.clone();
        sorted.sort_by(f64::total_cmp);
        let pct = |p: f64| sorted[((sorted.len() - 1) as f64 * p).round() as usize];
        println!(
            "\ncollector tick over {} ticks: first {:.2} ms, median {:.2} ms, p95 {:.2} ms, max {:.2} ms; history {} bytes",
            sorted.len(), tick_ms[0], pct(0.5), pct(0.95), pct(1.0), history.approx_bytes()
        );
    }
    Ok(())
}
```

- [ ] **Step 4: Run the probe and check its output**

Run: `cargo run -p negotium-probe --release -- --ticks 5`
Expected: five blocks, each with a totals line (CPU %, memory, disk, GPU, network) and up to 10 app rows. CPU % near what Task Manager shows at the same time.

- [ ] **Step 5: Measure tick timing against the ≤ 5 ms target**

Run: `cargo run -p negotium-probe --release -- --ticks 60 --timing`
Expected: final line with first/median/p95/max tick times. Copy that line into `docs/benchmarks/README.md` (Step 6). If the median exceeds 5 ms, record it anyway and note which source dominates (re-run with a profiler or by timing sources); do not change the target.

- [ ] **Step 6: Write `docs/benchmarks/README.md`**

```markdown
# Benchmarks

Machine: <CPU model>, <logical processors>, <RAM>, Windows <build>. Fill in from `msinfo32`.

## Collector tick (negotium-probe --release --ticks 60 --timing)

<paste the final timing line here>

Target (spec §8): median ≤ 5 ms.

## Task Manager baseline

See Task 12.
```

- [ ] **Step 7: Commit**

```bash
git add crates/probe crates/collector docs/benchmarks/README.md Cargo.lock
git commit -m "feat(probe): add console live view, accuracy test, and tick timing"
```

---

### Task 12: Benchmark tool and Task Manager baseline

**Files:**
- Create: `crates/bench/Cargo.toml`, `crates/bench/src/main.rs`, `crates/bench/src/stats.rs`
- Create: `docs/benchmarks/2026-10-14-taskmgr-baseline.json` (generated)
- Modify: `docs/benchmarks/README.md`

**Interfaces:**
- Consumes: `negotium_collector::{ProcessReader, logical_cpus}`.
- Produces: binary `negotium-bench <taskmgr|PATH> [--runs N] [--sample-secs S] [--out FILE]`; `stats::percentile(sorted: &[f64], p: f64) -> f64`, `stats::summary(values: &[f64]) -> Summary { median, p95, max, mean }`.
- Measures per spec §8: launch → first visible window (5 ms polling), app CPU % and private working set over the sample window, and UI responsiveness as `WM_NULL` round-trip time every 100 ms. Must run elevated (Task Manager auto-elevates; UIPI blocks messages to higher-integrity windows).

- [ ] **Step 1: Write the failing stats tests**

`crates/bench/src/stats.rs`:
```rust
//! Percentile helpers for benchmark summaries.
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Summary {
    pub median: f64,
    pub p95: f64,
    pub max: f64,
    pub mean: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentile_uses_nearest_rank_on_sorted_input() {
        let v = [1.0, 2.0, 3.0, 4.0, 5.0];
        assert_eq!(percentile(&v, 0.5), 3.0);
        assert_eq!(percentile(&v, 1.0), 5.0);
        assert_eq!(percentile(&v, 0.0), 1.0);
    }

    #[test]
    fn summary_of_unsorted_values() {
        let s = summary(&[5.0, 1.0, 3.0]);
        assert_eq!(s, Summary { median: 3.0, p95: 5.0, max: 5.0, mean: 3.0 });
    }

    #[test]
    fn summary_of_empty_is_zero() {
        assert_eq!(summary(&[]), Summary { median: 0.0, p95: 0.0, max: 0.0, mean: 0.0 });
    }
}
```

`crates/bench/Cargo.toml`:
```toml
[package]
name = "negotium-bench"
edition.workspace = true
version.workspace = true
license.workspace = true
publish.workspace = true

[dependencies]
negotium-collector.workspace = true
serde.workspace = true
serde_json.workspace = true
anyhow.workspace = true
windows = { workspace = true, features = [
    "Win32_Foundation",
    "Win32_System_Threading",
    "Win32_UI_Shell",
    "Win32_UI_WindowsAndMessaging",
] }
```

`crates/bench/src/main.rs` (temporary, so the crate compiles for the test run):
```rust
mod stats;

fn main() {}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p negotium-bench`
Expected: FAIL to compile ("cannot find function `percentile`").

- [ ] **Step 3: Implement the stats functions** (in `stats.rs`, above the tests)

```rust
/// `sorted` must be ascending and non-empty; `p` in [0, 1].
pub fn percentile(sorted: &[f64], p: f64) -> f64 {
    sorted[((sorted.len() - 1) as f64 * p.clamp(0.0, 1.0)).round() as usize]
}

pub fn summary(values: &[f64]) -> Summary {
    if values.is_empty() {
        return Summary { median: 0.0, p95: 0.0, max: 0.0, mean: 0.0 };
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    Summary {
        median: percentile(&sorted, 0.5),
        p95: percentile(&sorted, 0.95),
        max: *sorted.last().unwrap(),
        mean: sorted.iter().sum::<f64>() / sorted.len() as f64,
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p negotium-bench`
Expected: 3 passed.

- [ ] **Step 5: Implement the benchmark binary**

Replace `crates/bench/src/main.rs`:
```rust
//! Measures an app the way spec §8 defines: launch → first window, CPU %, private
//! working set, and UI responsiveness (WM_NULL round-trip). Run from an elevated terminal.
mod stats;

use anyhow::{Context, Result, bail};
use negotium_collector::{ProcessReader, logical_cpus};
use serde::Serialize;
use stats::{Summary, summary};
use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, WPARAM};
use windows::Win32::System::Threading::GetProcessId;
use windows::Win32::UI::Shell::{IsUserAnAdmin, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GW_OWNER, GetWindow, GetWindowThreadProcessId, IsWindowVisible, PostMessageW, SMTO_NORMAL,
    SW_SHOWNORMAL, SendMessageTimeoutW, WM_CLOSE, WM_NULL,
};
use windows::core::{BOOL, HSTRING, PCWSTR};

const WINDOW_TIMEOUT: Duration = Duration::from_secs(15);
const RESPONSIVENESS_TIMEOUT_MS: u32 = 1000;

#[derive(Serialize)]
struct Report {
    target: String,
    logical_cpus: u32,
    startup_ms: Vec<f64>,
    startup: Summary,
    cpu_pct: Summary,
    private_working_set_mb: Summary,
    responsiveness_ms: Summary,
    responsiveness_timeouts: usize,
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let target = args.next().context("usage: negotium-bench <taskmgr|PATH> [--runs N] [--sample-secs S] [--out FILE]")?;
    let (mut runs, mut sample_secs, mut out) = (5u32, 60u64, None::<String>);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--runs" => runs = args.next().context("--runs N")?.parse()?,
            "--sample-secs" => sample_secs = args.next().context("--sample-secs S")?.parse()?,
            "--out" => out = Some(args.next().context("--out FILE")?),
            other => bail!("unknown argument {other}"),
        }
    }
    // SAFETY: no arguments; reads the current token.
    if !unsafe { IsUserAnAdmin() }.as_bool() {
        bail!("run from an elevated terminal: Task Manager auto-elevates and UIPI blocks probing higher-integrity windows");
    }
    let path = if target.eq_ignore_ascii_case("taskmgr") {
        PathBuf::from(r"C:\Windows\System32\Taskmgr.exe")
    } else {
        PathBuf::from(&target)
    };
    let exe_name = path.file_name().context("target has no file name")?.to_string_lossy().to_lowercase();

    let mut startup_ms = Vec::new();
    let mut last: Option<(u32, HWND)> = None;
    for run in 0..runs {
        if let Some((pid, hwnd)) = last.take() {
            close(pid, hwnd)?;
        }
        if running_pid(&exe_name).is_some() {
            bail!("{exe_name} is already running; close it first (Task Manager is single-instance)");
        }
        let t0 = Instant::now();
        let pid = launch(&path)?;
        let hwnd = wait_for_window(pid)?;
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        println!("run {}: first window after {ms:.0} ms", run + 1);
        startup_ms.push(ms);
        last = Some((pid, hwnd));
    }
    let (pid, hwnd) = last.context("--runs must be at least 1")?;
    println!("sampling pid {pid} for {sample_secs} s …");
    let (cpu, ws, resp, timeouts) = sample(pid, hwnd, Duration::from_secs(sample_secs))?;
    close(pid, hwnd)?;

    let report = Report {
        target: path.display().to_string(),
        logical_cpus: logical_cpus(),
        startup: summary(&startup_ms),
        startup_ms,
        cpu_pct: summary(&cpu),
        private_working_set_mb: summary(&ws),
        responsiveness_ms: summary(&resp),
        responsiveness_timeouts: timeouts,
    };
    let json = serde_json::to_string_pretty(&report)?;
    println!("{json}");
    if let Some(path) = out {
        std::fs::write(&path, json).with_context(|| format!("write {path}"))?;
    }
    Ok(())
}

fn running_pid(exe_name: &str) -> Option<u32> {
    ProcessReader::new().read().ok()?.into_iter().find(|p| p.name.to_lowercase() == exe_name).map(|p| p.pid)
}

fn launch(path: &PathBuf) -> Result<u32> {
    let file = HSTRING::from(path.as_os_str());
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpFile: PCWSTR(file.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    // SAFETY: `info` is fully initialized and `file` outlives the call.
    unsafe { ShellExecuteExW(&mut info) }.context("ShellExecuteExW")?;
    // SAFETY: hProcess was requested via SEE_MASK_NOCLOSEPROCESS; closed after reading the pid.
    let pid = unsafe { GetProcessId(info.hProcess) };
    unsafe {
        let _ = CloseHandle(info.hProcess);
    }
    if pid == 0 {
        bail!("could not read launched process id");
    }
    Ok(pid)
}

struct Search {
    pid: u32,
    found: Option<HWND>,
}

unsafe extern "system" fn find_window(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: lparam is the `&mut Search` passed by first_window.
    let search = unsafe { &mut *(lparam.0 as *mut Search) };
    let mut pid = 0u32;
    // SAFETY: read-only queries on a handle supplied by EnumWindows.
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == search.pid
            && IsWindowVisible(hwnd).as_bool()
            && !GetWindow(hwnd, GW_OWNER).is_ok_and(|o| !o.is_invalid())
        {
            search.found = Some(hwnd);
            return false.into();
        }
    }
    true.into()
}

fn first_window(pid: u32) -> Option<HWND> {
    let mut search = Search { pid, found: None };
    // SAFETY: `search` outlives the synchronous enumeration.
    unsafe {
        let _ = EnumWindows(Some(find_window), LPARAM(&mut search as *mut Search as isize));
    }
    search.found
}

fn wait_for_window(pid: u32) -> Result<HWND> {
    let deadline = Instant::now() + WINDOW_TIMEOUT;
    loop {
        if let Some(h) = first_window(pid) {
            return Ok(h);
        }
        if Instant::now() > deadline {
            bail!("no visible window for pid {pid} within {WINDOW_TIMEOUT:?}");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn sample(pid: u32, hwnd: HWND, duration: Duration) -> Result<(Vec<f64>, Vec<f64>, Vec<f64>, usize)> {
    let stop = Arc::new(AtomicBool::new(false));
    let hwnd_bits = hwnd.0 as usize;
    let stop_probe = stop.clone();
    let prober = std::thread::spawn(move || {
        let hwnd = HWND(hwnd_bits as *mut c_void);
        let (mut times, mut timeouts) = (Vec::new(), 0usize);
        while !stop_probe.load(Ordering::Relaxed) {
            let t = Instant::now();
            // SAFETY: WM_NULL has no side effects; bounded by the timeout.
            let r = unsafe {
                SendMessageTimeoutW(hwnd, WM_NULL, WPARAM(0), LPARAM(0), SMTO_NORMAL, RESPONSIVENESS_TIMEOUT_MS, None)
            };
            if r.0 == 0 {
                timeouts += 1;
                times.push(RESPONSIVENESS_TIMEOUT_MS as f64);
            } else {
                times.push(t.elapsed().as_secs_f64() * 1000.0);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        (times, timeouts)
    });

    let cpus = logical_cpus() as f64;
    let mut reader = ProcessReader::new();
    let (mut cpu, mut ws) = (Vec::new(), Vec::new());
    let mut prev: Option<(Instant, u64)> = None;
    let end = Instant::now() + duration;
    while Instant::now() < end {
        let now = Instant::now();
        let p = reader.read().ok().and_then(|l| l.into_iter().find(|p| p.pid == pid)).context("target exited during sampling")?;
        if let Some((t0, c0)) = prev {
            let capacity = now.duration_since(t0).as_secs_f64() * 10_000_000.0 * cpus;
            cpu.push((p.cpu_time_100ns.saturating_sub(c0) as f64 / capacity * 100.0).clamp(0.0, 100.0));
        }
        ws.push(p.private_working_set as f64 / (1024.0 * 1024.0));
        prev = Some((now, p.cpu_time_100ns));
        std::thread::sleep(Duration::from_secs(1));
    }
    stop.store(true, Ordering::Relaxed);
    let (resp, timeouts) = prober.join().expect("prober thread");
    Ok((cpu, ws, resp, timeouts))
}

fn close(pid: u32, hwnd: HWND) -> Result<()> {
    // SAFETY: asks the window to close; no pointers involved.
    unsafe { PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)) }.context("PostMessageW WM_CLOSE")?;
    let deadline = Instant::now() + Duration::from_secs(10);
    while ProcessReader::new().read().map(|l| l.iter().any(|p| p.pid == pid)).unwrap_or(false) {
        if Instant::now() > deadline {
            bail!("pid {pid} did not exit after WM_CLOSE");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}
```

- [ ] **Step 6: Build it**

Run: `cargo build -p negotium-bench --release`
Expected: builds with no errors. (If a `windows` item path differs, follow the Global Constraints rule.)

- [ ] **Step 7: Record the Task Manager baseline** — needs an **elevated** terminal; if the executor cannot elevate, ask the human partner to run this command and paste the output

Close Task Manager first, then run from the repository root in an elevated terminal:
```bash
cargo run -p negotium-bench --release -- taskmgr --runs 5 --sample-secs 60 --out docs/benchmarks/2026-10-14-taskmgr-baseline.json
```
Expected: five "first window after N ms" lines, then a JSON report; the file `docs/benchmarks/2026-10-14-taskmgr-baseline.json` exists.

- [ ] **Step 8: Add the baseline to `docs/benchmarks/README.md`**

Replace the "Task Manager baseline" section with a table filled from the JSON:
```markdown
## Task Manager baseline (Taskmgr.exe 10.0.26100.9549)

Raw data: `2026-10-14-taskmgr-baseline.json`

| Metric | Task Manager | Negotium target (spec §8) |
|---|---|---|
| Launch → first window, median of 5 | <startup.median> ms | ≤ 250 ms |
| CPU while visible, mean / max | <cpu_pct.mean> / <cpu_pct.max> % | ≤ 0.5 % of one core (= <0.5 / logical_cpus> % of total) |
| Private working set, max | <private_working_set_mb.max> MB | ≤ 60 MB |
| UI responsiveness (WM_NULL), median / p95 / max | <…> ms | no frame > 16 ms (Plan 2 measures frames) |
| Responsiveness timeouts (> 1 s) | <responsiveness_timeouts> | 0 |
```

- [ ] **Step 9: Commit**

```bash
git add crates/bench docs/benchmarks Cargo.lock
git commit -m "feat(bench): measure launch, CPU, memory, responsiveness; record Task Manager baseline"
```

---

## After this plan

- Plan 2 (Overview UI) builds on `Collector`, `Model`, `Ranker`, `top_n`, and `History`, adds the collector thread with panic-restart, and extends `negotium-bench` to measure Negotium itself against the baseline above.
