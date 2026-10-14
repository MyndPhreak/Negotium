# Negotium Plan 3a — Performance Pages Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the left navigation rail and the Performance pages (CPU, Memory, every disk, every network adapter, every GPU) with 1 min / 10 min / 1 h graphs, the history scrub ("Top apps at HH:MM:SS"), the GPU temperature (on its page and on the Overview GPU meter), and full keyboard navigation through the app list.

**Architecture:** The collector gains per-device data (per-core by logical processor, CPU speed, memory composition, disk response time, per-adapter send/receive joined to the interface table, per-GPU engines, memory and temperature) plus static `SystemInfo`. `History` is rewritten around time: one sample per second at most, one hour kept, a preallocated column per device series, compact top-10 lists. The worker builds only the visible page each tick: the Overview `Frame` as before, or a `PerfFrame` for the one selected device, whose graphs are time-based Slint path strings from `graph.rs`. The history is shared behind a mutex so the UI thread can answer the scrub instantly without a collector tick. Keyboard navigation is a pure `nav.rs` state machine driven from one list-level `FocusScope`; keyboard focus in the list freezes it exactly like hover (rulings R9–R11).

**Tech Stack:** Rust stable (MSVC, edition 2024), `slint` 1.18.1, `i-slint-backend-testing` 1.18.1 (dev), `windows` 0.62.2 (new features listed below), `serde`/`serde_json` 1.0. No new crates.

**Spec:** `docs/superpowers/specs/2026-10-14-negotium-design.md` — §4.2 (collector sources), §4.4 (history), §5 navigation, §5.1 (GPU temperature on the GPU meter), §5.2 (Performance, history scrub), §7 (errors, CPU temperature "—"), §8 (performance). Earlier plans and their follow-ups: `docs/superpowers/plans/2026-10-14-negotium-overview-ui.md`, `docs/superpowers/plans/2026-10-14-negotium-data-core-followups.md`, `docs/superpowers/plans/2026-10-14-negotium-overview-ui-followups.md`.

**Out of scope:** Startup and Settings pages and restart-as-admin preserving the page (Plan 3b, `docs/superpowers/plans/2026-10-14-negotium-startup-settings.md`, which builds on this plan); no-prompt admin task, Ctrl+Shift+Esc redirect, installer, CPU-temperature add-on (PawnIO), per-app network (SRUM), app icons (Plan 4).

**Carried into this plan (Plan 1/2 follow-ups):** per-disk/per-adapter history series and time-based capacity (Task 4); per-core index ≠ core id (Task 1); Hyper-V network double count (Task 2); R4 full keyboard navigation through list rows (Task 11). Deferred minors folded in because this plan rewrites that code anyway: history budget measured at full load with every metric's top list filled, `at()`/`since()` linear, only `Metric::Cpu` asserted (Task 4); rapid UI pokes adding extra history samples (Task 4, one-per-second rule); index-based sparkline x-axis and the idle network sparkline that drew nothing (Task 5); tile click doesn't scroll to the app (Task 11).

## Rulings on spec ambiguities

- **P3-1 Memory composition source.** §4.2 names `NtQuerySystemInformation(SystemMemoryListInformation)` "where permitted". That class needs administrator rights, so the composition bar would be empty for the default (non-elevated) user. This plan reads the same kernel page lists through the PDH `Memory` counter set (`Modified Page List Bytes`, `Standby Cache {Core,Normal Priority,Reserve} Bytes`, `Free & Zero Page List Bytes`), which every user may read. "Compressed" is the resident size of the `Memory Compression` process. Cost if wrong: none for the numbers (same lists); the human partner may ask for the NT class as an elevated-only refinement.
- **P3-2 Network total.** The Overview network meter sums hardware interfaces only (`MIB_IF_ROW2` `HardwareInterface` flag): a Hyper-V vEthernet and a VPN tunnel carry traffic the physical NIC also counts. With no hardware interface at all (inside a VM, or the interface table unavailable) every interface counts.
- **P3-3 History resolution.** "1 hour at 1 s" is read as: at most one sample per second (ticks less than 950 ms after the last kept sample are not recorded), samples older than one hour dropped. At 0.5 s refresh every other tick is recorded; at 2 s and 5 s the hour holds fewer samples.
- **P3-4 Scrub per page.** The scrub shows the top apps for the page's metric (CPU→CPU, Memory→memory, disk pages→disk, GPU pages→GPU, network pages→network). Disk use is per app across all disks (Windows has no per-disk attribution); per-app network needs SRUM (Plan 4), so network pages say so instead of listing apps.
- **P3-5 Graph downsampling.** A graph shows at most 300 columns (60 for small multiples); each column draws its largest value, so short spikes stay visible on the 1 h range.
- **P3-6 GPU engines.** Per-engine graphs are per engine *type* (3D, Copy, VideoEncode, VideoDecode first, then the rest alphabetically); each shows the busiest engine of that type.
- **P3-7 Keyboard focus in the list.** Tab reaches the list once; arrows move a "current row" shown by a focus ring. While the list has keyboard focus it is frozen exactly like hover (R9–R11: rows keep their index, exits stay in place inert, new rows wait); Esc gives focus back so live ordering resumes. Mouse clicks never select rows (a click must not freeze the live list indefinitely).

## Global Constraints

- Windows 11 x64 only; Rust stable, `x86_64-pc-windows-msvc`, edition 2024.
- No new crates. Existing workspace dependencies only: `windows = "0.62.2"`, `slint = "1.18.1"` (features `raw-window-handle-06`), `slint-build`, `i-slint-backend-testing` (dev), `raw-window-handle`, `serde`, `serde_json`, `anyhow` (binaries only). New `windows` features: in `negotium-collector` `Win32_System_Registry`, `Win32_Graphics_Dxgi`, `Win32_Graphics_Dxgi_Common`, `Wdk_Graphics_Direct3D`, `Win32_NetworkManagement_IpHelper`, `Win32_NetworkManagement_Ndis`, `Win32_Networking_WinSock`.
- `unsafe` only in `negotium-collector`, `negotium-actions`, `negotium-bench`, and `crates/app/src/platform.rs`; every `unsafe` block has its own `// SAFETY:` comment.
- **The UI thread never collects data or performs an action.** Collection and model work run on the worker thread; actions on action threads. The UI thread may read the shared `History` under its mutex (plain data, no Windows calls) to answer the scrub; window setup in `platform.rs` stays the only Windows code on the UI thread.
- **Only the visible page's views are built each tick** (spec §8): the Overview frame only while the Overview shows, and only the selected device's Performance view while Performance shows; nothing while hidden in the tray. The collector, model and history keep running regardless.
- **No per-tick allocation growth** (spec §8): history columns are preallocated to `MAX_SAMPLES` and reused; memory for one hour stays at or below 2 000 000 bytes for a typical machine (16 logical processors, 2 disks, 2 adapters, 1 GPU with 8 engine types, all top lists full), measured by `History::approx_bytes`.
- Row identity rules from rulings R9–R11 keep holding: while the list is frozen (pointer over it, a row menu open, keyboard focus on a row control or on the list) every index keeps its row, exited rows stay in place inert, new rows wait; a context menu acts on the row captured when it opened.
- Spec §5: native Windows 11 look — Mica, accent colour, Segoe UI Variable, light and dark.
- Slint lessons from Plan 2 (keep): draw icons as `Path`s (glyph chevrons rendered as tofu); give every layout that holds a repeater `max-width: 16384px` (an empty repeater otherwise caps the window at its min-width; `window_opens_at_its_preferred_width` catches it); `changed` handlers run later, in dirty order — never rely on one having run before Rust reads a property; a disabled `FocusScope` ignores FocusOut (clear keyboard state explicitly when a modal opens); `ContextMenuArea` stays `enabled: false` and is opened only through a handler that captures the target row first.
- UI tests use `i-slint-backend-testing` and need debug info (`build.rs` sets `with_debug_info(PROFILE != release)`); run them in debug, never with `--release`.
- Tests never terminate real processes other than throwaway children they spawned, and never change real system settings.
- Every new value shown to the user that may be missing renders as "—" (or "Unavailable" on a meter), never as 0 or an error dialog (spec §7).
- If a `windows` 0.62.2 or Slint 1.18 item path or signature differs from this plan, look it up (`https://microsoft.github.io/windows-docs-rs/doc/windows/`, `https://docs.slint.dev/latest/docs/rust/slint/`, or the crate source under `~/.cargo/registry/src`) and adapt without changing behaviour; note it in the report.
- Before running the app manually, `Stop-Process -Name negotium -ErrorAction SilentlyContinue` (a tray-resident instance holds the single-instance lock), and stop any instance you started when done.

## Review Focus

1. **A device appears or disappears mid-session** (USB disk, VPN adapter, external GPU) → its page appears or disappears; a selected page whose device vanished falls back to CPU instead of showing stale numbers; its history column is dropped once its last value ages out. Tests in Task 4 (columns) and Task 7 (fallback).
2. **The 1 h range right after start** (only seconds of history) → the graph draws only the span that exists, at its true time on the right edge (never stretched across the hour); scrubbing over the empty part says there is no history for that moment instead of showing apps from another time. Tests in Task 5 and Task 10.
3. **Counters with no data** (no GPU counters in a VM, a GPU without a temperature sensor, a core whose counter had no valid data this tick) → "—" for that item only; per-core cells keep their position (cell *n* is always logical processor *n*). Tests in Task 1 and Task 7.
4. **Hyper-V or VPN hosts** → the network meter counts physical traffic once. Test in Task 2.
5. **Keyboard navigation while rows change** → the current row never jumps to another app (the list is frozen while focused), Delete on an exited row does nothing, and a modal opened from the keyboard never leaves the list frozen. Tests in Task 11.

---

## File Structure

```
crates/snapshot/src/lib.rs            MODIFY: per-core slots, CPU speed, uptime, local clock, memory composition,
                                              disk response time, per-adapter and per-GPU samples
crates/collector/Cargo.toml           MODIFY: windows features
crates/collector/src/lib.rs           MODIFY: module wiring
crates/collector/src/memory.rs        MODIFY: commit charge, processor groups, uptime, local clock
crates/collector/src/sysinfo.rs       CREATE: SystemInfo (CPU name, memory speed from SMBIOS, GPUs)
crates/collector/src/pdh.rs           MODIFY: counter table; per-core by logical index; CPU speed; memory lists;
                                              disk response; per-adapter network; per-GPU engines and memory
crates/collector/src/net.rs           CREATE: interface table (alias, hardware flag, link speed, addresses) + PDH join
crates/collector/src/gpu.rs           CREATE: DXGI adapters (name, memory sizes), D3DKMT temperature
crates/collector/src/collector.rs     MODIFY: fills the new totals; system_info(); with_epoch()
crates/collector/tests/pdh.rs         MODIFY
crates/collector/tests/collector.rs   MODIFY
crates/collector/tests/devices.rs     CREATE: interface table and GPU integration tests
crates/history/src/lib.rs             REWRITE: time-based retention, per-device series, compact top lists
crates/probe/src/main.rs              MODIFY: History::new()
crates/app/src/format.rs              MODIFY: temperature, GB, GHz, uptime, clock, milliseconds
crates/app/src/graph.rs               CREATE: time-based graph paths, scale rounding
crates/app/src/perf.rs                CREATE: Performance view model (device list + selected device)
crates/app/src/scrub.rs               CREATE: "Top apps at HH:MM:SS"
crates/app/src/nav.rs                 CREATE: keyboard row navigation (pure)
crates/app/src/view.rs                MODIFY: Frame.page/perf; sparklines via graph.rs; GPU temperature on its meter
crates/app/src/controls.rs            MODIFY: page, selected device, range
crates/app/src/worker.rs              MODIFY: page-aware frames; shared history; shared epoch
crates/app/src/bridge.rs              MODIFY: page dispatch; Performance models; scrub; list navigation
crates/app/src/main.rs                MODIFY: wiring
crates/app/src/lib.rs                 MODIFY: modules
crates/app/ui/rail.slint              CREATE: left navigation rail
crates/app/ui/graph.slint             CREATE: graph with scrub cursor and card
crates/app/ui/perfpage.slint          CREATE: Performance page
crates/app/ui/main.slint              MODIFY: rail + stacked pages
crates/app/ui/meters.slint            MODIFY: sparkline view box 1000 wide
crates/app/ui/applist.slint           MODIFY: list focus, current row, keyboard menu, ensure-visible
crates/app/tests/ui.rs                MODIFY: new UI tests
docs/benchmarks/README.md             MODIFY: tick cost after Plan 3a, manual checks
```

---

### Task 1: CPU and memory details in the snapshot

**Files:**
- Modify: `crates/snapshot/src/lib.rs`
- Modify: `crates/collector/Cargo.toml`, `crates/collector/src/lib.rs`, `crates/collector/src/memory.rs`, `crates/collector/src/pdh.rs`, `crates/collector/src/collector.rs`
- Create: `crates/collector/src/sysinfo.rs`
- Modify: `crates/history/src/lib.rs` (type change only), `crates/app/src/view.rs` (one test literal)
- Test: unit tests in `pdh.rs`, `sysinfo.rs`, `memory.rs`, `collector.rs`; `crates/collector/tests/{pdh,collector}.rs`

**Interfaces:**
- Consumes: existing `PdhSources`, `Collector`, `memory_totals()`, `logical_cpus()`.
- Produces:
  - `negotium_snapshot::SystemTotals` gains `per_core_pct: Vec<Option<f32>>` (was `Vec<f32>`; index = logical processor ordinal across processor groups, `None` = no valid data this tick), `cpu_mhz: Option<f32>`, `uptime_s: u64`, `local_time_ms: u64` (local wall-clock time of the tick, ms since midnight). All new fields `#[serde(default)]`.
  - `MemoryTotals` (now `Default`) gains `commit_total: u64`, `commit_limit: u64`, `lists: Option<MemoryLists>`, `compressed: Option<u64>`; new `#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)] pub struct MemoryLists { pub modified: u64, pub standby: u64, pub free: u64 }`.
  - `negotium_collector::{processor_groups() -> Vec<u32>, uptime_s() -> u64, local_time_ms() -> u64}` in `memory.rs`.
  - `negotium_collector::sysinfo::{SystemInfo, read_system_info, cpu_name, memory_speed_mts, parse_smbios_memory_speed}`; `SystemInfo { cpu_name: Option<String>, memory_speed_mts: Option<u32> }` (`Clone, Debug, Default, PartialEq`); re-exported as `negotium_collector::SystemInfo`.
  - `pdh::{per_core_from(items: &[(String, f64)], groups: &[u32]) -> Vec<Option<f32>>, cpu_mhz(freq_mhz: Option<f64>, perf_pct: Option<f64>) -> Option<f32>, memory_lists(modified: Option<f64>, standby: [Option<f64>; 3], free: Option<f64>) -> Option<MemoryLists>}`; `PdhReadings` gains `cpu_mhz: Option<f32>`, `mem_lists: Option<MemoryLists>`; its `per_core_pct` becomes `Vec<Option<f32>>`.
  - `Collector::system_info(&self) -> &SystemInfo`; `collector::compressed_bytes(&[ProcessSample]) -> Option<u64>` (crate-visible, unit-tested).

- [ ] **Step 1: Write the failing tests**

In `crates/collector/src/pdh.rs` tests, replace `per_core_orders_by_group_then_core_and_clamps` with:
```rust
    #[test]
    fn per_core_places_each_instance_at_its_logical_index() {
        // Two processor groups of 4 and 2. "0,1", "0,2" and "1,0" had no valid data this tick
        // (filtered by CStatus), "1,5" does not exist in group 1.
        let v = items(&[("0,0", 10.0), ("0,3", 120.0), ("1,1", 50.0), ("1,5", 99.0), ("_Total", 40.0), ("0,_Total", 30.0)]);
        assert_eq!(per_core_from(&v, &[4, 2]), vec![Some(10.0), None, None, Some(100.0), None, Some(50.0)]);
        assert_eq!(total_from(&v), Some(40.0));
        assert_eq!(per_core_from(&[], &[2]), vec![None, None], "no data still yields one slot per processor");
    }

    #[test]
    fn cpu_speed_is_the_nominal_frequency_scaled_by_performance() {
        assert_eq!(cpu_mhz(Some(3000.0), Some(120.0)), Some(3600.0));
        assert_eq!(cpu_mhz(Some(3000.0), None), None);
        assert_eq!(cpu_mhz(Some(0.0), Some(100.0)), None);
    }

    #[test]
    fn memory_lists_add_the_three_standby_priorities() {
        let gb = |n: f64| Some(n * 1_073_741_824.0);
        let l = memory_lists(gb(1.0), [gb(0.5), gb(2.0), gb(0.25)], gb(4.0)).unwrap();
        assert_eq!(l.modified, 1 << 30);
        assert_eq!(l.standby, (11 << 30) / 4);
        assert_eq!(l.free, 4 << 30);
        assert_eq!(memory_lists(gb(1.0), [gb(0.5), None, gb(0.25)], gb(4.0)), None);
    }
```

In the same tests module, replace `failed_counter_registration_is_reported_by_read` with:
```rust
    #[test]
    fn failed_counter_registration_is_reported_by_read() {
        let mut query = PDH_HQUERY::default();
        // SAFETY: opening a real-time query with no data source; owned by `sources` below.
        assert_eq!(unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &mut query) }, 0);
        // Dropping `sources` closes the query.
        let mut sources = PdhSources { query, counters: Counters::failed(0xC0000BB8), groups: vec![2] };
        let r = sources.read();
        assert!(r.cpu_pct.is_none() && r.disks.is_none() && r.net_bps.is_none() && r.gpu_pct.is_none());
        assert!(r.cpu_mhz.is_none() && r.mem_lists.is_none());
        assert_eq!(r.per_core_pct, vec![None, None]);
        for label in ["cpu", "cpu frequency", "cpu performance", "memory modified", "memory free"] {
            let want = format!("pdh {label}: counter unavailable (add failed 0xc0000bb8)");
            assert!(r.errors.contains(&want), "missing {want:?} in {:?}", r.errors);
        }
    }
```

Create `crates/collector/src/sysinfo.rs` with only its tests for now (the functions come in Step 3):
```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// One SMBIOS type-17 (Memory Device) structure: size in MB, speed, configured speed.
    fn device(size_mb: u16, speed: u16, configured: u16) -> Vec<u8> {
        let mut f = vec![0u8; 0x22];
        f[0] = 17;
        f[1] = 0x22;
        f[0x0C..0x0E].copy_from_slice(&size_mb.to_le_bytes());
        f[0x15..0x17].copy_from_slice(&speed.to_le_bytes());
        f[0x20..0x22].copy_from_slice(&configured.to_le_bytes());
        f.extend_from_slice(b"DIMM A\0\0");
        f
    }

    /// RawSMBIOSData: four version bytes, the table length, then the table.
    fn raw(structures: &[Vec<u8>]) -> Vec<u8> {
        let table = structures.concat();
        let mut out = vec![0, 3, 4, 0];
        out.extend_from_slice(&(table.len() as u32).to_le_bytes());
        out.extend(table);
        out
    }

    const END: [u8; 6] = [127, 4, 0, 0, 0, 0];

    #[test]
    fn memory_speed_prefers_configured_speed_over_rated_speed() {
        let r = raw(&[device(16384, 6400, 6000), device(16384, 6400, 6000), END.to_vec()]);
        assert_eq!(parse_smbios_memory_speed(&r), Some(6000));
    }

    #[test]
    fn memory_speed_falls_back_to_rated_speed_and_skips_empty_slots() {
        let r = raw(&[device(0, 4800, 4800), device(8192, 3200, 0), END.to_vec()]);
        assert_eq!(parse_smbios_memory_speed(&r), Some(3200));
    }

    #[test]
    fn memory_speed_is_none_without_devices_and_never_panics_on_truncation() {
        assert_eq!(parse_smbios_memory_speed(&raw(&[END.to_vec()])), None);
        assert_eq!(parse_smbios_memory_speed(&[]), None);
        let full = raw(&[device(8192, 3200, 3200)]);
        for cut in 0..full.len() {
            let _ = parse_smbios_memory_speed(&full[..cut]);
        }
    }
}
```

In `crates/collector/src/memory.rs` add:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn processor_groups_add_up_to_the_logical_processor_count() {
        assert_eq!(processor_groups().iter().sum::<u32>(), logical_cpus());
    }

    #[test]
    fn local_time_is_within_a_day_and_commit_is_reported() {
        assert!(local_time_ms() < 86_400_000);
        assert!(uptime_s() > 0);
        let m = memory_totals().unwrap();
        assert!(m.commit_limit > 0 && m.commit_total <= m.commit_limit);
    }
}
```

In `crates/collector/src/collector.rs` add:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn proc(name: &str, ws: u64) -> ProcessSample {
        ProcessSample {
            pid: 1, parent_pid: 0, create_time: 1, name: name.into(), session_id: 0, cpu_time_100ns: 0,
            private_working_set: ws, disk_io_bytes: 0, threads: 1, handles: 0, image_path: None,
            package_family: None, description: None,
        }
    }

    #[test]
    fn compressed_memory_is_the_memory_compression_process() {
        assert_eq!(compressed_bytes(&[proc("System", 5), proc("Memory Compression", 512)]), Some(512));
        assert_eq!(compressed_bytes(&[proc("System", 5)]), None);
    }
}
```

In `crates/collector/tests/pdh.rs` replace the per-core assertion line with:
```rust
    assert_eq!(r.per_core_pct.len() as u32, logical_cpus(), "one slot per logical processor");
    assert!(r.per_core_pct.iter().any(|c| c.is_some()));
    assert!(r.mem_lists.is_some(), "Memory counter set readable without admin");
```

In `crates/collector/tests/collector.rs` append inside `ticks_produce_complete_snapshots`:
```rust
    let t = &second.totals;
    assert_eq!(t.per_core_pct.len() as u32, t.logical_cpus);
    assert!(t.uptime_s > 0 && t.local_time_ms < 86_400_000);
    let m = t.memory.as_ref().unwrap();
    assert!(m.commit_limit > 0);
    let lists = m.lists.expect("memory composition");
    assert!(lists.modified + lists.standby + lists.free <= m.total);
    assert!(c.system_info().cpu_name.as_deref().is_some_and(|n| !n.is_empty()));
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p negotium-collector`
Expected: FAIL to compile (`per_core_from` takes one argument, no `cpu_mhz`, `memory_lists`, `Counters`, `sysinfo` items, `processor_groups`, `compressed_bytes`, …).

- [ ] **Step 3: Implement**

`crates/snapshot/src/lib.rs` — replace `MemoryTotals` and `SystemTotals` (keep `busiest_disk_pct`):
```rust
/// The kernel's page lists (PDH `Memory` counter set), for the memory composition bar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MemoryLists {
    pub modified: u64,
    /// Standby cache, all priorities.
    pub standby: u64,
    /// Free and zeroed pages.
    pub free: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MemoryTotals {
    pub total: u64,
    pub used: u64,
    /// Committed bytes and the commit limit (RAM + page files).
    #[serde(default)]
    pub commit_total: u64,
    #[serde(default)]
    pub commit_limit: u64,
    #[serde(default)]
    pub lists: Option<MemoryLists>,
    /// Resident size of the memory compression store (the "Memory Compression" process).
    #[serde(default)]
    pub compressed: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SystemTotals {
    pub logical_cpus: u32,
    pub cpu_pct: Option<f32>,
    /// Utility of each logical processor, indexed by its ordinal across processor groups (group 0's
    /// processors first). `None` for a processor whose counter had no valid data this tick, so an
    /// index always means the same processor.
    pub per_core_pct: Vec<Option<f32>>,
    /// Current speed: nominal frequency × "% Processor Performance".
    #[serde(default)]
    pub cpu_mhz: Option<f32>,
    #[serde(default)]
    pub uptime_s: u64,
    /// Local wall-clock time when the tick was taken, milliseconds since midnight.
    #[serde(default)]
    pub local_time_ms: u64,
    pub memory: Option<MemoryTotals>,
    pub disks: Option<Vec<DiskSample>>,
    pub gpu_pct: Option<f32>,
    pub gpu_pct_by_pid: Option<BTreeMap<u32, f32>>,
    pub net_bps: Option<f64>,
}
```

`crates/collector/Cargo.toml`: add `"Win32_System_Registry",` to the `windows` feature list.

`crates/collector/src/memory.rs` — replace `memory_totals` and add the helpers:
```rust
//! Physical memory totals, commit charge, processor topology, uptime and the local clock.
use negotium_snapshot::MemoryTotals;
use std::mem::size_of;
use windows::Win32::System::SystemInformation::{GetLocalTime, GetTickCount64, GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::{ALL_PROCESSOR_GROUPS, GetActiveProcessorCount, GetActiveProcessorGroupCount};

pub fn memory_totals() -> Option<MemoryTotals> {
    let mut status = MEMORYSTATUSEX { dwLength: size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    // SAFETY: `status` is initialized with its size as the API requires.
    unsafe { GlobalMemoryStatusEx(&mut status) }.ok()?;
    Some(MemoryTotals {
        total: status.ullTotalPhys,
        used: status.ullTotalPhys.saturating_sub(status.ullAvailPhys),
        // ullTotalPageFile is the commit limit; ullAvailPageFile what is left of it.
        commit_limit: status.ullTotalPageFile,
        commit_total: status.ullTotalPageFile.saturating_sub(status.ullAvailPageFile),
        lists: None,
        compressed: None,
    })
}

/// Logical processors across all processor groups (matters above 64 cores).
pub fn logical_cpus() -> u32 {
    // SAFETY: simple query with no pointers.
    let n = unsafe { GetActiveProcessorCount(ALL_PROCESSOR_GROUPS) };
    n.max(1)
}

/// Active logical processors in each processor group, in group order.
pub fn processor_groups() -> Vec<u32> {
    // SAFETY: simple query with no pointers.
    let count = unsafe { GetActiveProcessorGroupCount() };
    let groups: Vec<u32> = (0..count)
        .map(|g| {
            // SAFETY: simple query with no pointers.
            unsafe { GetActiveProcessorCount(g) }
        })
        .collect();
    if groups.iter().sum::<u32>() == 0 { vec![logical_cpus()] } else { groups }
}

/// Seconds since Windows started.
pub fn uptime_s() -> u64 {
    // SAFETY: simple query with no pointers.
    unsafe { GetTickCount64() } / 1000
}

/// Local wall-clock time, milliseconds since midnight.
pub fn local_time_ms() -> u64 {
    // SAFETY: simple query with no pointers.
    let t = unsafe { GetLocalTime() };
    ((t.wHour as u64 * 60 + t.wMinute as u64) * 60 + t.wSecond as u64) * 1000 + t.wMilliseconds as u64
}
```

`crates/collector/src/sysinfo.rs` — above the tests:
```rust
//! Facts about the machine that don't change while Negotium runs, read once at startup.
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
use windows::Win32::System::SystemInformation::{FIRMWARE_TABLE_PROVIDER, GetSystemFirmwareTable};
use windows::core::w;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SystemInfo {
    /// "AMD Ryzen 9 7950X 16-Core Processor".
    pub cpu_name: Option<String>,
    /// Configured memory speed in MT/s (SMBIOS), the fastest installed module.
    pub memory_speed_mts: Option<u32>,
}

pub fn read_system_info() -> SystemInfo {
    SystemInfo { cpu_name: cpu_name(), memory_speed_mts: memory_speed_mts() }
}

pub fn cpu_name() -> Option<String> {
    let mut buf = [0u16; 256];
    let mut size = (buf.len() * 2) as u32;
    // SAFETY: `buf` has `size` writable bytes for a REG_SZ value; `size` is updated to the bytes written.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!("HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0"),
            w!("ProcessorNameString"),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let units = &buf[..(size as usize / 2).min(buf.len())];
    let end = units.iter().position(|&c| c == 0).unwrap_or(units.len());
    let name = String::from_utf16_lossy(&units[..end]).trim().to_string();
    (!name.is_empty()).then_some(name)
}

pub fn memory_speed_mts() -> Option<u32> {
    let provider = FIRMWARE_TABLE_PROVIDER(u32::from_be_bytes(*b"RSMB"));
    // SAFETY: size query with no buffer.
    let size = unsafe { GetSystemFirmwareTable(provider, 0, None) };
    if size == 0 {
        return None;
    }
    let mut buf = vec![0u8; size as usize];
    // SAFETY: `buf` has `size` writable bytes.
    let got = unsafe { GetSystemFirmwareTable(provider, 0, Some(&mut buf)) };
    if got == 0 || got > size {
        return None;
    }
    parse_smbios_memory_speed(&buf[..got as usize])
}

/// Walks a RawSMBIOSData blob (4 version bytes, a u32 table length, the table) for type-17 Memory
/// Device structures and returns the highest configured speed (offset 0x20), falling back to the
/// rated speed (0x15), over installed modules (size at 0x0C nonzero). Never panics on short input.
pub fn parse_smbios_memory_speed(raw: &[u8]) -> Option<u32> {
    let len = u32::from_le_bytes(raw.get(4..8)?.try_into().ok()?) as usize;
    let table = raw.get(8..8usize.checked_add(len)?).or_else(|| raw.get(8..))?;
    let mut best: Option<u32> = None;
    let mut i = 0usize;
    while i + 4 <= table.len() {
        let (kind, flen) = (table[i], table[i + 1] as usize);
        if flen < 4 || i + flen > table.len() {
            break;
        }
        let f = &table[i..i + flen];
        if kind == 17 {
            let word = |o: usize| f.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
            let usable = |s: &u16| *s != 0 && *s != 0xFFFF;
            let installed = word(0x0C).is_some_and(|s| s != 0);
            let speed = word(0x20).filter(usable).or_else(|| word(0x15).filter(usable));
            if installed && let Some(s) = speed {
                best = Some(best.map_or(s as u32, |b| b.max(s as u32)));
            }
        }
        if kind == 127 {
            break;
        }
        // Skip the formatted area, then the string set, which ends with two NULs.
        let mut j = i + flen;
        while j + 1 < table.len() && !(table[j] == 0 && table[j + 1] == 0) {
            j += 1;
        }
        i = j + 2;
    }
    best
}
```

`crates/collector/src/lib.rs` — add `pub mod sysinfo;`, `pub use sysinfo::{SystemInfo, read_system_info};`, and extend the memory re-export to `pub use memory::{local_time_ms, logical_cpus, memory_totals, processor_groups, uptime_s};`.

`crates/collector/src/pdh.rs` — replace the struct definitions, `open`, `read` and `per_core_from`, and add the helpers (keep `add`, `read_array`, `parse_core_instance`, `total_from`, `parse_gpu_instance`, `gpu_from`, `disks_from`, `Drop`):
```rust
use negotium_snapshot::{DiskSample, MemoryLists};
use windows::Win32::System::Performance::{PDH_FMT_COUNTERVALUE, PdhGetFormattedCounterValue};
// (keep the existing imports)

pub struct PdhReadings {
    pub cpu_pct: Option<f32>,
    pub per_core_pct: Vec<Option<f32>>,
    pub cpu_mhz: Option<f32>,
    pub mem_lists: Option<MemoryLists>,
    pub disks: Option<Vec<DiskSample>>,
    pub net_bps: Option<f64>,
    pub gpu_pct: Option<f32>,
    pub gpu_pct_by_pid: Option<BTreeMap<u32, f32>>,
    pub errors: Vec<String>,
}

type Counter = Result<PDH_HCOUNTER, u32>;

/// Every counter of the query. A counter that could not be added stays `Err` and its reading is
/// `None` (with an error line) every tick; the others are unaffected.
pub(crate) struct Counters {
    cpu: Counter,
    cpu_freq: Counter,
    cpu_perf: Counter,
    mem_modified: Counter,
    mem_standby_core: Counter,
    mem_standby_normal: Counter,
    mem_standby_reserve: Counter,
    mem_free: Counter,
    disk_idle: Counter,
    disk_read: Counter,
    disk_write: Counter,
    net: Counter,
    gpu: Counter,
}

impl Counters {
    fn add_all(query: PDH_HQUERY) -> Self {
        Self {
            cpu: add(query, w!("\\Processor Information(*)\\% Processor Utility")),
            cpu_freq: add(query, w!("\\Processor Information(_Total)\\Processor Frequency")),
            cpu_perf: add(query, w!("\\Processor Information(_Total)\\% Processor Performance")),
            mem_modified: add(query, w!("\\Memory\\Modified Page List Bytes")),
            mem_standby_core: add(query, w!("\\Memory\\Standby Cache Core Bytes")),
            mem_standby_normal: add(query, w!("\\Memory\\Standby Cache Normal Priority Bytes")),
            mem_standby_reserve: add(query, w!("\\Memory\\Standby Cache Reserve Bytes")),
            mem_free: add(query, w!("\\Memory\\Free & Zero Page List Bytes")),
            disk_idle: add(query, w!("\\PhysicalDisk(*)\\% Idle Time")),
            disk_read: add(query, w!("\\PhysicalDisk(*)\\Disk Read Bytes/sec")),
            disk_write: add(query, w!("\\PhysicalDisk(*)\\Disk Write Bytes/sec")),
            net: add(query, w!("\\Network Interface(*)\\Bytes Total/sec")),
            gpu: add(query, w!("\\GPU Engine(*)\\Utilization Percentage")),
        }
    }

    #[cfg(test)]
    pub(crate) fn failed(status: u32) -> Self {
        let f = Err(status);
        Self {
            cpu: f, cpu_freq: f, cpu_perf: f, mem_modified: f, mem_standby_core: f, mem_standby_normal: f,
            mem_standby_reserve: f, mem_free: f, disk_idle: f, disk_read: f, disk_write: f, net: f, gpu: f,
        }
    }
}

pub struct PdhSources {
    query: PDH_HQUERY,
    counters: Counters,
    /// Active logical processors per processor group, for placing per-core instances.
    groups: Vec<u32>,
}

impl PdhSources {
    pub fn open() -> Result<Self, u32> {
        let mut query = PDH_HQUERY::default();
        // SAFETY: opens a real-time query; closed in Drop.
        let status = unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &mut query) };
        if status != 0 {
            return Err(status);
        }
        let sources = Self { query, counters: Counters::add_all(query), groups: crate::processor_groups() };
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
        let c = &self.counters;
        let cpu = get_array(&mut errors, "cpu", c.cpu);
        let freq = get_value(&mut errors, "cpu frequency", c.cpu_freq);
        let perf = get_value(&mut errors, "cpu performance", c.cpu_perf);
        let modified = get_value(&mut errors, "memory modified", c.mem_modified);
        let standby = [
            get_value(&mut errors, "memory standby core", c.mem_standby_core),
            get_value(&mut errors, "memory standby normal", c.mem_standby_normal),
            get_value(&mut errors, "memory standby reserve", c.mem_standby_reserve),
        ];
        let free = get_value(&mut errors, "memory free", c.mem_free);
        let idle = get_array(&mut errors, "disk idle", c.disk_idle);
        let read = get_array(&mut errors, "disk read", c.disk_read);
        let write = get_array(&mut errors, "disk write", c.disk_write);
        let net = get_array(&mut errors, "network", c.net);
        let gpu = get_array(&mut errors, "gpu", c.gpu);

        let (gpu_pct, gpu_pct_by_pid) = match gpu {
            Some(v) => {
                let (sys, per) = gpu_from(&v);
                (sys, Some(per))
            }
            None => (None, None),
        };
        PdhReadings {
            cpu_pct: cpu.as_deref().and_then(total_from),
            per_core_pct: per_core_from(cpu.as_deref().unwrap_or_default(), &self.groups),
            cpu_mhz: cpu_mhz(freq, perf),
            mem_lists: memory_lists(modified, standby, free),
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

fn get_array(errors: &mut Vec<String>, label: &str, c: Counter) -> Option<Vec<(String, f64)>> {
    let c = match c {
        Ok(c) => c,
        Err(status) => {
            errors.push(format!("pdh {label}: counter unavailable (add failed {status:#010x})"));
            return None;
        }
    };
    match read_array(c) {
        Ok(v) => Some(v),
        Err(e) => {
            errors.push(format!("pdh {label}: {e:#010x}"));
            None
        }
    }
}

fn get_value(errors: &mut Vec<String>, label: &str, c: Counter) -> Option<f64> {
    let c = match c {
        Ok(c) => c,
        Err(status) => {
            errors.push(format!("pdh {label}: counter unavailable (add failed {status:#010x})"));
            return None;
        }
    };
    match read_value(c) {
        Ok(v) => Some(v),
        Err(e) => {
            errors.push(format!("pdh {label}: {e:#010x}"));
            None
        }
    }
}

/// One formatted value of a single-instance counter (the `Memory` set, `_Total` instances).
fn read_value(counter: PDH_HCOUNTER) -> Result<f64, u32> {
    let mut value = PDH_FMT_COUNTERVALUE::default();
    let format = PDH_FMT(PDH_FMT_DOUBLE.0 | PDH_FMT_NOCAP100);
    // SAFETY: valid counter; `value` is a writable PDH_FMT_COUNTERVALUE.
    let status = unsafe { PdhGetFormattedCounterValue(counter, format, None, &mut value) };
    if status != 0 {
        return Err(status);
    }
    if value.CStatus > PDH_CSTATUS_NEW_DATA {
        return Err(value.CStatus);
    }
    // SAFETY: doubleValue is the active union member because we requested PDH_FMT_DOUBLE.
    let v = unsafe { value.Anonymous.doubleValue };
    Ok(if v.is_finite() { v } else { 0.0 })
}

/// Places each "group,core" instance at its logical index: group bases are the running sum of
/// `groups`. Missing instances stay `None`, so a cell never shows another processor's value.
pub fn per_core_from(items: &[(String, f64)], groups: &[u32]) -> Vec<Option<f32>> {
    let mut out = vec![None; groups.iter().sum::<u32>() as usize];
    for (name, v) in items {
        let Some((g, c)) = parse_core_instance(name) else { continue };
        let Some(&size) = groups.get(g as usize) else { continue };
        if c < size {
            let base: u32 = groups[..g as usize].iter().sum();
            out[(base + c) as usize] = Some(v.clamp(0.0, 100.0) as f32);
        }
    }
    out
}

/// Task Manager's "Speed": the nominal frequency scaled by "% Processor Performance".
pub fn cpu_mhz(freq_mhz: Option<f64>, perf_pct: Option<f64>) -> Option<f32> {
    let (f, p) = (freq_mhz?, perf_pct?);
    (f > 0.0).then(|| (f * p / 100.0) as f32)
}

pub fn memory_lists(modified: Option<f64>, standby: [Option<f64>; 3], free: Option<f64>) -> Option<MemoryLists> {
    let standby: f64 = standby.into_iter().sum::<Option<f64>>()?;
    Some(MemoryLists { modified: modified?.max(0.0) as u64, standby: standby.max(0.0) as u64, free: free?.max(0.0) as u64 })
}
```

`crates/collector/src/collector.rs` — add `info: SystemInfo` to `Collector` (set with `crate::read_system_info()` in `new`), the accessor, the helper, and fill the new fields in `tick`:
```rust
use crate::{SystemInfo, local_time_ms, read_system_info, uptime_s};

    pub fn system_info(&self) -> &SystemInfo {
        &self.info
    }

/// The memory compression store lives in the minimal process "Memory Compression".
pub(crate) fn compressed_bytes(processes: &[ProcessSample]) -> Option<u64> {
    processes.iter().find(|p| p.name == "Memory Compression").map(|p| p.private_working_set)
}
```
In `tick`, collect `processes` into a `let processes: Vec<ProcessSample> = …;` binding as today, then build totals as:
```rust
        let mut totals = SystemTotals {
            logical_cpus: self.logical_cpus,
            memory: memory_totals(),
            uptime_s: uptime_s(),
            local_time_ms: local_time_ms(),
            ..Default::default()
        };
        if totals.memory.is_none() {
            source_errors.push("memory: GlobalMemoryStatusEx failed".into());
        }
        match &mut self.pdh {
            Ok(pdh) => {
                let r = pdh.read();
                totals.cpu_pct = r.cpu_pct;
                totals.per_core_pct = r.per_core_pct;
                totals.cpu_mhz = r.cpu_mhz;
                totals.disks = r.disks;
                totals.net_bps = r.net_bps;
                totals.gpu_pct = r.gpu_pct;
                totals.gpu_pct_by_pid = r.gpu_pct_by_pid;
                if let Some(m) = totals.memory.as_mut() {
                    m.lists = r.mem_lists;
                    m.compressed = compressed_bytes(&processes);
                }
                source_errors.extend(r.errors);
            }
            Err(status) => source_errors.push(format!("pdh open: {status:#010x}")),
        }
```

Follow the type change through: in `crates/history/src/lib.rs` change `HistorySample::per_core_pct` to `Vec<Option<f32>>` and the test helper `totals` to `per_core_pct: vec![Some(cpu); cores]` (Task 4 rewrites this crate); in `crates/app/src/view.rs` tests change the literal to `memory: Some(MemoryTotals { total: 16 << 30, used: 8 << 30, ..Default::default() })`.

- [ ] **Step 4: Run tests**

Run: `cargo test --workspace`
Expected: all pass, warning-free (`cargo clippy --workspace --all-targets` clean).

- [ ] **Step 5: Commit**

```bash
git add crates/snapshot crates/collector crates/history crates/app/src/view.rs
git commit -m "feat(collector): per-core by logical processor, CPU speed, memory composition, commit, uptime"
```

---
### Task 2: Disk response time and per-adapter network (Hyper-V double count)

**Files:**
- Modify: `crates/snapshot/src/lib.rs`, `crates/collector/Cargo.toml`, `crates/collector/src/{lib,pdh,collector}.rs`
- Create: `crates/collector/src/net.rs`, `crates/collector/tests/devices.rs`
- Modify: `crates/collector/tests/pdh.rs`
- Test: unit tests in `pdh.rs` and `net.rs`; `crates/collector/tests/devices.rs`

**Interfaces:**
- Consumes: Task 1's `Counters`, `get_array`, `PdhReadings`.
- Produces:
  - `DiskSample` gains `#[serde(default)] pub response_ms: f32` (average time per transfer).
  - `#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)] pub struct AdapterSample { pub name: String /* PDH instance, the stable key */, pub alias: Option<String> /* "Ethernet" */, pub description: Option<String>, pub sent_bps: f64, pub recv_bps: f64, pub hardware: bool, pub connected: bool, pub link_bps: Option<u64> /* bits per second */, pub addresses: Vec<String> }`; `SystemTotals` gains `#[serde(default)] pub adapters: Option<Vec<AdapterSample>>`. `SystemTotals::net_bps` now means physical throughput (ruling P3-2).
  - `pdh::{disks_from(idle, read, write, response), adapters_from(sent: &[(String, f64)], recv: &[(String, f64)]) -> Vec<(String, f64, f64)>}`; `PdhReadings::net_bps` is replaced by `adapters: Option<Vec<(String, f64, f64)>>` (name, sent, received bytes/s).
  - `negotium_collector::net::{IfInfo, NetTable, pdh_instance_name, join_adapters, hardware_total, REFRESH, MIN_REFRESH}`:
    - `IfInfo { alias: String, description: String, hardware: bool, connected: bool, link_bps: Option<u64>, addresses: Vec<String> }`
    - `NetTable::new()`, `NetTable::from_interfaces(Vec<IfInfo>, Instant) -> NetTable`, `get(&self, instance: &str) -> Option<&IfInfo>`, `needs_refresh<'a>(&self, instances: impl IntoIterator<Item = &'a str>, now: Instant) -> bool`, `refresh(&mut self, now: Instant) -> Result<(), String>`
    - `pdh_instance_name(description: &str) -> String`, `join_adapters(raw: &[(String, f64, f64)], table: &NetTable) -> Vec<AdapterSample>`, `hardware_total(adapters: &[AdapterSample]) -> f64`.

- [ ] **Step 1: Write the failing tests**

In `pdh.rs` tests replace `disks_join_counters_and_skip_total` with:
```rust
    #[test]
    fn disks_join_counters_and_skip_total() {
        let idle = items(&[("0 C:", 75.0), ("_Total", 80.0), ("1 D:", 100.0)]);
        let read = items(&[("0 C:", 1000.0), ("1 D:", 0.0)]);
        let write = items(&[("0 C:", 500.0)]);
        let response = items(&[("0 C:", 0.0004), ("1 D:", -1.0)]);
        let disks = disks_from(&idle, &read, &write, &response);
        assert_eq!(disks.len(), 2);
        assert_eq!(disks[0].name, "0 C:");
        assert_eq!(disks[0].active_pct, 25.0);
        assert_eq!(disks[0].read_bps, 1000.0);
        assert_eq!(disks[0].write_bps, 500.0);
        assert!((disks[0].response_ms - 0.4).abs() < 1e-6, "seconds per transfer shown in ms");
        assert_eq!(disks[1].write_bps, 0.0);
        assert_eq!(disks[1].response_ms, 0.0, "negative values clamp to zero");
    }

    #[test]
    fn adapters_pair_sent_and_received_by_instance() {
        let sent = items(&[("Intel[R] Ethernet", 100.0), ("Hyper-V Virtual Ethernet Adapter", 40.0)]);
        let recv = items(&[("Intel[R] Ethernet", 900.0)]);
        assert_eq!(
            adapters_from(&sent, &recv),
            vec![("Intel[R] Ethernet".to_string(), 100.0, 900.0), ("Hyper-V Virtual Ethernet Adapter".to_string(), 40.0, 0.0)]
        );
    }
```
Also, in `failed_counter_registration_is_reported_by_read`, change `r.net_bps.is_none()` to `r.adapters.is_none()`.

Create `crates/collector/src/net.rs` with its tests (implementation in Step 3):
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn info(alias: &str, description: &str, hardware: bool) -> IfInfo {
        IfInfo {
            alias: alias.into(), description: description.into(), hardware, connected: true,
            link_bps: Some(1_000_000_000), addresses: vec!["192.168.1.20".into()],
        }
    }
    fn sample(name: &str, hardware: bool, sent: f64, recv: f64) -> AdapterSample {
        AdapterSample { name: name.into(), hardware, sent_bps: sent, recv_bps: recv, connected: true, ..Default::default() }
    }

    #[test]
    fn pdh_instance_names_mangle_the_description_like_perflib() {
        assert_eq!(pdh_instance_name("Intel(R) Ethernet Connection (7) I219-V"), "Intel[R] Ethernet Connection [7] I219-V");
        assert_eq!(pdh_instance_name("Realtek USB GbE #2"), "Realtek USB GbE _2");
        assert_eq!(pdh_instance_name("Wi-Fi a/b\\c"), "Wi-Fi a_b_c");
    }

    #[test]
    fn hyper_v_and_vpn_traffic_counts_once() {
        // The physical NIC carries everything; the vEthernet and the VPN tunnel see it again.
        let adapters = [
            sample("Intel[R] Ethernet", true, 400.0, 600.0),
            sample("Hyper-V Virtual Ethernet Adapter", false, 400.0, 600.0),
            sample("WireGuard Tunnel", false, 100.0, 200.0),
        ];
        assert_eq!(hardware_total(&adapters), 1000.0);
    }

    #[test]
    fn without_any_hardware_interface_every_interface_counts() {
        let vm = [sample("Microsoft Hyper-V Network Adapter", false, 10.0, 20.0), sample("Other", false, 1.0, 2.0)];
        assert_eq!(hardware_total(&vm), 33.0);
        assert_eq!(hardware_total(&[]), 0.0);
    }

    #[test]
    fn join_fills_interface_details_and_tolerates_unknown_instances() {
        let table = NetTable::from_interfaces(vec![info("Ethernet", "Intel(R) Ethernet", true)], Instant::now());
        let joined = join_adapters(
            &[("Intel[R] Ethernet".into(), 1.0, 2.0), ("Mystery Adapter".into(), 3.0, 4.0)],
            &table,
        );
        assert_eq!(joined[0].alias.as_deref(), Some("Ethernet"));
        assert_eq!(joined[0].description.as_deref(), Some("Intel(R) Ethernet"));
        assert!(joined[0].hardware && joined[0].connected);
        assert_eq!(joined[0].link_bps, Some(1_000_000_000));
        assert_eq!(joined[0].addresses, ["192.168.1.20"]);
        assert_eq!(joined[1].alias, None);
        assert!(!joined[1].hardware && joined[1].connected, "unknown interfaces are shown, not counted as hardware");
    }

    #[test]
    fn the_interface_table_is_reread_on_age_or_on_an_unknown_instance_but_not_every_tick() {
        let t0 = Instant::now();
        let table = NetTable::from_interfaces(vec![info("Ethernet", "Intel(R) Ethernet", true)], t0);
        assert!(NetTable::new().needs_refresh(["x"], t0), "never read");
        assert!(!table.needs_refresh(["Intel[R] Ethernet"], t0 + Duration::from_secs(1)));
        assert!(!table.needs_refresh(["New Adapter"], t0 + Duration::from_secs(1)), "unknown, but read just now");
        assert!(table.needs_refresh(["New Adapter"], t0 + MIN_REFRESH));
        assert!(table.needs_refresh(["Intel[R] Ethernet"], t0 + REFRESH));
    }
}
```

Create `crates/collector/tests/devices.rs`:
```rust
use negotium_collector::PdhSources;
use negotium_collector::net::{NetTable, join_adapters};
use std::time::{Duration, Instant};

#[test]
fn live_network_instances_join_the_interface_table() {
    let mut pdh = PdhSources::open().expect("open pdh");
    std::thread::sleep(Duration::from_millis(1100));
    let raw = pdh.read().adapters.expect("Network Interface counters on a real machine");
    let mut table = NetTable::new();
    table.refresh(Instant::now()).expect("GetIfTable2");
    let joined = join_adapters(&raw, &table);
    assert_eq!(joined.len(), raw.len());
    if !raw.is_empty() {
        assert!(joined.iter().any(|a| a.alias.is_some()), "at least one PDH instance matches an interface: {joined:?}");
    }
}
```

In `crates/collector/tests/pdh.rs` replace `assert!(r.net_bps.is_some());` with `assert!(r.adapters.is_some());`.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p negotium-collector`
Expected: FAIL to compile (`disks_from` arity, no `adapters_from`, no `net` module, no `PdhReadings::adapters`).

- [ ] **Step 3: Implement**

`crates/snapshot/src/lib.rs`: add `#[serde(default)] pub response_ms: f32,` to `DiskSample` (update the `disk()` test helper with `response_ms: 0.0`), add `AdapterSample` as specified in Interfaces with this doc comment, and `#[serde(default)] pub adapters: Option<Vec<AdapterSample>>,` to `SystemTotals`:
```rust
/// One "Network Interface" instance, joined to the interface table (friendly name, hardware or
/// virtual, link speed, addresses). `name` is the PDH instance name: the key for its history.
```
and change the `net_bps` doc to `/// Physical network throughput: hardware interfaces only (a vEthernet or VPN sees the same bytes).`

`crates/collector/Cargo.toml`: add `"Win32_NetworkManagement_IpHelper", "Win32_NetworkManagement_Ndis", "Win32_Networking_WinSock",` to the `windows` features.

`pdh.rs`:
- in `Counters` replace `net` with `disk_response`, `net_sent`, `net_recv` (also in `failed`):
```rust
            disk_response: add(query, w!("\\PhysicalDisk(*)\\Avg. Disk sec/Transfer")),
            net_sent: add(query, w!("\\Network Interface(*)\\Bytes Sent/sec")),
            net_recv: add(query, w!("\\Network Interface(*)\\Bytes Received/sec")),
```
- `PdhReadings`: replace `pub net_bps: Option<f64>,` with `pub adapters: Option<Vec<(String, f64, f64)>>,`
- in `read`:
```rust
        let response = get_array(&mut errors, "disk response", c.disk_response);
        let sent = get_array(&mut errors, "network sent", c.net_sent);
        let recv = get_array(&mut errors, "network received", c.net_recv);
        // …
            disks: match (idle, read, write, response) {
                (Some(i), Some(r), Some(w), Some(t)) => Some(disks_from(&i, &r, &w, &t)),
                _ => None,
            },
            adapters: match (sent, recv) {
                (Some(s), Some(r)) => Some(adapters_from(&s, &r)),
                _ => None,
            },
```
- replace `disks_from` and add `adapters_from`:
```rust
pub fn disks_from(
    idle: &[(String, f64)], read: &[(String, f64)], write: &[(String, f64)], response: &[(String, f64)],
) -> Vec<DiskSample> {
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
            response_ms: (lookup(response, name) * 1000.0) as f32,
        })
        .collect()
}

/// (instance, sent, received) for every instance of the sent counter.
pub fn adapters_from(sent: &[(String, f64)], recv: &[(String, f64)]) -> Vec<(String, f64, f64)> {
    sent.iter()
        .map(|(name, s)| {
            let r = recv.iter().find(|(n, _)| n == name).map(|(_, v)| v.max(0.0)).unwrap_or(0.0);
            (name.clone(), s.max(0.0), r)
        })
        .collect()
}
```

`crates/collector/src/net.rs` (above the tests):
```rust
//! Network interfaces: friendly name, hardware or virtual, link speed and addresses from the
//! interface table, joined to PDH "Network Interface" instances by their mangled description.
//! The table is read every REFRESH, or sooner (at most every MIN_REFRESH) when PDH reports an
//! instance it doesn't know — never every tick.
use negotium_snapshot::AdapterSample;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::NetworkManagement::IpHelper::{
    FreeMibTable, GetIfTable2, GetUnicastIpAddressTable, MIB_IF_TABLE2, MIB_UNICASTIPADDRESS_TABLE,
};
use windows::Win32::NetworkManagement::Ndis::MediaConnectStateConnected;
use windows::Win32::Networking::WinSock::{AF_INET, AF_INET6, AF_UNSPEC, SOCKADDR_INET};

pub const REFRESH: Duration = Duration::from_secs(10);
pub const MIN_REFRESH: Duration = Duration::from_secs(2);

/// `MIB_IF_ROW2.InterfaceAndOperStatusFlags` bits.
const HARDWARE_INTERFACE: u8 = 0x01;
const FILTER_INTERFACE: u8 = 0x02;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct IfInfo {
    pub alias: String,
    pub description: String,
    pub hardware: bool,
    pub connected: bool,
    pub link_bps: Option<u64>,
    /// IPv4 first, then IPv6; loopback and IPv6 link-local omitted.
    pub addresses: Vec<String>,
}

/// PDH names a "Network Interface" instance after the adapter description with `(`→`[`,
/// `)`→`]`, and `#`, `/`, `\` → `_`.
pub fn pdh_instance_name(description: &str) -> String {
    description
        .chars()
        .map(|c| match c {
            '(' => '[',
            ')' => ']',
            '#' | '/' | '\\' => '_',
            c => c,
        })
        .collect()
}

#[derive(Default)]
pub struct NetTable {
    by_instance: HashMap<String, IfInfo>,
    read_at: Option<Instant>,
}

impl NetTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_interfaces(list: Vec<IfInfo>, read_at: Instant) -> Self {
        Self {
            by_instance: list.into_iter().map(|i| (pdh_instance_name(&i.description), i)).collect(),
            read_at: Some(read_at),
        }
    }

    pub fn get(&self, instance: &str) -> Option<&IfInfo> {
        self.by_instance.get(instance)
    }

    pub fn needs_refresh<'a>(&self, instances: impl IntoIterator<Item = &'a str>, now: Instant) -> bool {
        let Some(at) = self.read_at else { return true };
        let age = now.saturating_duration_since(at);
        age >= REFRESH || (age >= MIN_REFRESH && instances.into_iter().any(|i| !self.by_instance.contains_key(i)))
    }

    /// Re-reads the interface table. The time is recorded even on failure, so a failing table
    /// is retried on the refresh schedule, not every tick.
    pub fn refresh(&mut self, now: Instant) -> Result<(), String> {
        self.read_at = Some(now);
        let list = read_interfaces()?;
        *self = Self::from_interfaces(list, now);
        Ok(())
    }
}

pub fn join_adapters(raw: &[(String, f64, f64)], table: &NetTable) -> Vec<AdapterSample> {
    raw.iter()
        .map(|(name, sent, recv)| {
            let info = table.get(name);
            AdapterSample {
                name: name.clone(),
                alias: info.map(|i| i.alias.clone()),
                description: info.map(|i| i.description.clone()),
                sent_bps: *sent,
                recv_bps: *recv,
                hardware: info.is_some_and(|i| i.hardware),
                connected: info.is_none_or(|i| i.connected),
                link_bps: info.and_then(|i| i.link_bps),
                addresses: info.map(|i| i.addresses.clone()).unwrap_or_default(),
            }
        })
        .collect()
}

/// Physical traffic counted once (ruling P3-2): hardware interfaces only; every interface when
/// none is hardware (inside a VM, or without the interface table).
pub fn hardware_total(adapters: &[AdapterSample]) -> f64 {
    let any_hardware = adapters.iter().any(|a| a.hardware);
    adapters
        .iter()
        .filter(|a| a.hardware || !any_hardware)
        .map(|a| a.sent_bps.max(0.0) + a.recv_bps.max(0.0))
        .sum()
}

fn wide(units: &[u16]) -> String {
    let end = units.iter().position(|&c| c == 0).unwrap_or(units.len());
    String::from_utf16_lossy(&units[..end])
}

fn show_address(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => !v4.is_loopback(),
        IpAddr::V6(v6) => !v6.is_loopback() && (v6.segments()[0] & 0xffc0) != 0xfe80,
    }
}

/// # Safety
/// `a.si_family` must describe the active member, as the IP helper API guarantees.
unsafe fn sockaddr_ip(a: &SOCKADDR_INET) -> Option<IpAddr> {
    // SAFETY: the caller guarantees si_family selects the initialised member.
    unsafe {
        if a.si_family == AF_INET {
            Some(IpAddr::V4(Ipv4Addr::from(a.Ipv4.sin_addr.S_un.S_addr.to_ne_bytes())))
        } else if a.si_family == AF_INET6 {
            Some(IpAddr::V6(Ipv6Addr::from(a.Ipv6.sin6_addr.u.Byte)))
        } else {
            None
        }
    }
}

fn read_interfaces() -> Result<Vec<IfInfo>, String> {
    let mut by_luid: HashMap<u64, IfInfo> = HashMap::new();
    let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
    // SAFETY: GetIfTable2 allocates the table; it is freed with FreeMibTable below.
    let status = unsafe { GetIfTable2(&mut table) };
    if status != ERROR_SUCCESS {
        return Err(format!("network: GetIfTable2 failed {:#010x}", status.0));
    }
    // SAFETY: on success `table` points to NumEntries consecutive rows.
    let rows = unsafe { std::slice::from_raw_parts((*table).Table.as_ptr(), (*table).NumEntries as usize) };
    for row in rows {
        let flags = row.InterfaceAndOperStatusFlags._bitfield;
        if flags & FILTER_INTERFACE != 0 {
            continue;
        }
        let link = row.ReceiveLinkSpeed;
        // SAFETY: NET_LUID_LH is a union whose `Value` covers the whole 64 bits.
        let luid = unsafe { row.InterfaceLuid.Value };
        by_luid.insert(luid, IfInfo {
            alias: wide(&row.Alias),
            description: wide(&row.Description),
            hardware: flags & HARDWARE_INTERFACE != 0,
            connected: row.MediaConnectState == MediaConnectStateConnected,
            link_bps: (link != 0 && link != u64::MAX).then_some(link),
            addresses: Vec::new(),
        });
    }
    // SAFETY: allocated by GetIfTable2, freed exactly once.
    unsafe { FreeMibTable(table as *const _) };

    let mut addresses: *mut MIB_UNICASTIPADDRESS_TABLE = std::ptr::null_mut();
    // SAFETY: as above, freed with FreeMibTable.
    if unsafe { GetUnicastIpAddressTable(AF_UNSPEC, &mut addresses) } == ERROR_SUCCESS {
        // SAFETY: on success `addresses` points to NumEntries consecutive rows.
        let rows = unsafe { std::slice::from_raw_parts((*addresses).Table.as_ptr(), (*addresses).NumEntries as usize) };
        for row in rows {
            // SAFETY: as for the interface LUID above.
            let luid = unsafe { row.InterfaceLuid.Value };
            // SAFETY: rows from GetUnicastIpAddressTable carry a valid si_family.
            let ip = unsafe { sockaddr_ip(&row.Address) };
            if let (Some(info), Some(ip)) = (by_luid.get_mut(&luid), ip)
                && show_address(&ip)
            {
                info.addresses.push(ip.to_string());
            }
        }
        // SAFETY: allocated by GetUnicastIpAddressTable, freed exactly once.
        unsafe { FreeMibTable(addresses as *const _) };
    }
    let mut list: Vec<IfInfo> = by_luid.into_values().collect();
    for i in &mut list {
        i.addresses.sort_by_key(|a| a.contains(':'));
    }
    Ok(list)
}
```

`crates/collector/src/lib.rs`: add `pub mod net;`.

`crates/collector/src/collector.rs`: add `net: crate::net::NetTable` (`NetTable::new()` in `new`), and in the `Ok(pdh)` arm replace `totals.net_bps = r.net_bps;` with:
```rust
                if let Some(raw) = r.adapters {
                    let now = std::time::Instant::now();
                    if self.net.needs_refresh(raw.iter().map(|(n, _, _)| n.as_str()), now)
                        && let Err(e) = self.net.refresh(now)
                    {
                        source_errors.push(e);
                    }
                    let adapters = crate::net::join_adapters(&raw, &self.net);
                    totals.net_bps = Some(crate::net::hardware_total(&adapters));
                    totals.adapters = Some(adapters);
                }
```
(`self.pdh` and `self.net` are distinct fields; if the borrow checker objects to `&mut self.pdh` being matched while `self.net` is used, bind `let r = match &mut self.pdh { Ok(p) => Ok(p.read()), Err(s) => Err(*s) };` first and handle `r` afterwards.)

- [ ] **Step 4: Run tests**

Run: `cargo test --workspace`
Expected: all pass, warning-free.

- [ ] **Step 5: Commit**

```bash
git add crates/snapshot crates/collector
git commit -m "feat(collector): per-adapter network with interface details; count Hyper-V/VPN traffic once; disk response time"
```

---

### Task 3: Per-GPU engines, memory and temperature; GPU temperature on the meter

**Files:**
- Modify: `crates/snapshot/src/lib.rs`, `crates/collector/Cargo.toml`, `crates/collector/src/{lib,pdh,collector,sysinfo}.rs`
- Create: `crates/collector/src/gpu.rs`
- Modify: `crates/collector/tests/devices.rs`
- Modify: `crates/app/src/format.rs`, `crates/app/src/view.rs`
- Test: unit tests in `pdh.rs`, `gpu.rs`, `format.rs`, `view.rs`; `tests/devices.rs`

**Interfaces:**
- Consumes: Task 1's `Counters`, `SystemInfo`, `read_system_info`.
- Produces:
  - `#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)] pub struct GpuSample { pub key: String, pub util_pct: f32, pub engines: Vec<(String, f32)>, pub dedicated_used: Option<u64>, pub shared_used: Option<u64>, pub temp_c: Option<f32> }`; `SystemTotals` gains `#[serde(default)] pub gpus: Option<Vec<GpuSample>>`. `key` is the lowercase adapter instance `"luid_0x00000000_0x0001569b_phys_0"`.
  - `pdh::{GpuInstance<'a> { pid: u32, adapter: &'a str, engine: &'a str, engtype: &'a str }, parse_gpu_instance(&str) -> Option<GpuInstance<'_>>, gpu_from(items) -> (Option<f32>, BTreeMap<u32, f32>, Vec<GpuSample>), gpu_memory_from(items) -> BTreeMap<String, u64>}`; `PdhReadings` gains `gpus: Option<Vec<GpuSample>>`.
  - `negotium_collector::gpu::{GpuInfo { key: String, luid: (i32, u32), name: String, dedicated_total: u64, shared_total: u64 }, adapter_key(high: i32, low: u32) -> String, list_gpus() -> Vec<GpuInfo>, GpuTemps::{open(&[GpuInfo]) -> GpuTemps, read(&self) -> Vec<(String, f32)>}, deci_celsius(u32) -> Option<f32>}`.
  - `SystemInfo` gains `pub gpus: Vec<GpuInfo>` (DXGI order).
  - `negotium::format::temp_c(f32) -> String` ("62 °C"). The Overview GPU meter's `sub` shows the temperature of the busiest GPU, empty when unknown.

- [ ] **Step 1: Write the failing tests**

In `pdh.rs` tests replace `gpu_instance_parses_pid_and_engine` and `gpu_system_is_busiest_engine_and_process_is_its_max_engine` with:
```rust
    #[test]
    fn gpu_instance_parses_pid_adapter_engine_and_type() {
        let n = "pid_1234_luid_0x00000000_0x0000D1B4_phys_0_eng_3_engtype_3D";
        let i = parse_gpu_instance(n).unwrap();
        assert_eq!(i.pid, 1234);
        assert_eq!(i.adapter, "luid_0x00000000_0x0000D1B4_phys_0");
        assert_eq!(i.engine, "luid_0x00000000_0x0000D1B4_phys_0_eng_3");
        assert_eq!(i.engtype, "3D");
        assert!(parse_gpu_instance("garbage").is_none());
    }

    #[test]
    fn gpu_totals_per_adapter_and_per_engine_type() {
        let v = items(&[
            ("pid_1_luid_0x0_0x1_phys_0_eng_0_engtype_3D", 30.0),
            ("pid_2_luid_0x0_0x1_phys_0_eng_0_engtype_3D", 25.0),
            ("pid_2_luid_0x0_0x1_phys_0_eng_5_engtype_VideoDecode", 40.0),
            ("pid_3_luid_0x0_0x2_phys_0_eng_1_engtype_Copy", 5.0),
            ("pid_3_luid_0x0_0x2_phys_0_eng_2_engtype_Copy", 7.0),
        ]);
        let (system, per_pid, gpus) = gpu_from(&v);
        assert_eq!(system, Some(55.0), "busiest engine summed over processes");
        assert_eq!(per_pid.get(&2), Some(&40.0));
        assert_eq!(gpus.len(), 2);
        assert_eq!(gpus[0].key, "luid_0x0_0x1_phys_0");
        assert_eq!(gpus[0].util_pct, 55.0);
        assert_eq!(gpus[0].engines, vec![("3D".to_string(), 55.0), ("VideoDecode".to_string(), 40.0)]);
        assert_eq!(gpus[1].engines, vec![("Copy".to_string(), 7.0)], "busiest engine of each type");
    }

    #[test]
    fn gpu_adapter_memory_is_keyed_by_lowercase_adapter() {
        let m = gpu_memory_from(&items(&[("luid_0x00000000_0x0001569B_phys_0", 3.5e9)]));
        assert_eq!(m.get("luid_0x00000000_0x0001569b_phys_0"), Some(&3_500_000_000));
    }
```
Also extend `failed_counter_registration_is_reported_by_read` with `assert!(r.gpus.is_none());`.

Create `crates/collector/src/gpu.rs` with tests (implementation in Step 3):
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapter_keys_match_pdh_instances() {
        assert_eq!(adapter_key(0, 0x0001_569B), "luid_0x00000000_0x0001569b_phys_0");
        assert_eq!(adapter_key(-1, 1), "luid_0xffffffff_0x00000001_phys_0");
    }

    #[test]
    fn temperature_is_tenths_of_a_degree_and_zero_means_no_sensor() {
        assert_eq!(deci_celsius(625), Some(62.5));
        assert_eq!(deci_celsius(0), None);
        assert_eq!(deci_celsius(5000), None, "implausible values are dropped");
    }
}
```

Append to `crates/collector/tests/devices.rs`:
```rust
#[test]
fn gpu_temperatures_are_plausible_for_whatever_reports_one() {
    let gpus = negotium_collector::gpu::list_gpus();
    let temps = negotium_collector::gpu::GpuTemps::open(&gpus).read();
    for (key, c) in temps {
        assert!(gpus.iter().any(|g| g.key == key));
        assert!((5.0..=120.0).contains(&c), "{key}: {c} °C");
    }
}
```

In `crates/app/src/format.rs` tests add:
```rust
    #[test]
    fn temperatures_round_to_whole_degrees() {
        assert_eq!(temp_c(62.4), "62 °C");
        assert_eq!(temp_c(f32::NAN), "—");
    }
```

In `crates/app/src/view.rs` tests add:
```rust
    #[test]
    fn gpu_meter_shows_the_busiest_gpus_temperature() {
        use negotium_snapshot::GpuSample;
        let gpu = |key: &str, util: f32, temp: Option<f32>| GpuSample { key: key.into(), util_pct: util, temp_c: temp, ..Default::default() };
        let mut s = snap(Some(1.0));
        s.totals.gpu_pct = Some(80.0);
        s.totals.gpus = Some(vec![gpu("igpu", 5.0, None), gpu("dgpu", 80.0, Some(64.6))]);
        let mut ids = IdMap::new();
        let f = build_frame(&mut ids, &s, &[], &[], &History::new(10), &opts(Metric::Cpu, true));
        assert_eq!(f.meters[3].sub, "65 °C");
        s.totals.gpus = Some(vec![gpu("dgpu", 80.0, None)]);
        let f = build_frame(&mut ids, &s, &[], &[], &History::new(10), &opts(Metric::Cpu, true));
        assert_eq!(f.meters[3].sub, "", "no sensor: no label");
    }
```
(Task 4 changes `History::new(10)` to `History::new()` everywhere in these tests.)

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p negotium-collector; cargo test -p negotium --lib`
Expected: FAIL to compile (no `GpuInstance` fields, `gpu_from` arity, no `gpu_memory_from`, no `gpu` module, no `GpuSample`, no `temp_c`).

- [ ] **Step 3: Implement**

`crates/snapshot/src/lib.rs`: add `GpuSample` (doc: `/// One GPU (adapter instance "luid_…_phys_N", lowercase). util_pct is its busiest engine; engines holds the busiest engine of each type, in display order.`) and `#[serde(default)] pub gpus: Option<Vec<GpuSample>>,` in `SystemTotals`.

`crates/collector/Cargo.toml`: add `"Win32_Graphics_Dxgi", "Win32_Graphics_Dxgi_Common", "Wdk_Graphics_Direct3D",` to the `windows` features.

`pdh.rs`:
- `Counters` gains `gpu_dedicated: add(query, w!("\\GPU Adapter Memory(*)\\Dedicated Usage"))` and `gpu_shared: add(query, w!("\\GPU Adapter Memory(*)\\Shared Usage"))` (and `failed`).
- `PdhReadings` gains `pub gpus: Option<Vec<GpuSample>>,`.
- in `read`:
```rust
        let dedicated = get_array(&mut errors, "gpu dedicated memory", c.gpu_dedicated).map(|v| gpu_memory_from(&v));
        let shared = get_array(&mut errors, "gpu shared memory", c.gpu_shared).map(|v| gpu_memory_from(&v));
        let (gpu_pct, gpu_pct_by_pid, gpus) = match gpu {
            Some(v) => {
                let (sys, per, mut gpus) = gpu_from(&v);
                for g in &mut gpus {
                    g.dedicated_used = dedicated.as_ref().and_then(|m| m.get(&g.key).copied());
                    g.shared_used = shared.as_ref().and_then(|m| m.get(&g.key).copied());
                }
                (sys, Some(per), Some(gpus))
            }
            None => (None, None, None),
        };
```
  and `gpus` in the returned `PdhReadings`.
- replace `parse_gpu_instance` and `gpu_from`, add `gpu_memory_from`:
```rust
/// Engine types shown first on a GPU page, in Task Manager's order; the rest follow alphabetically.
const PREFERRED_ENGINES: [&str; 4] = ["3D", "Copy", "VideoEncode", "VideoDecode"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GpuInstance<'a> {
    pub pid: u32,
    /// "luid_0x…_0x…_phys_N"
    pub adapter: &'a str,
    /// "luid_0x…_0x…_phys_N_eng_M"
    pub engine: &'a str,
    pub engtype: &'a str,
}

pub fn parse_gpu_instance(name: &str) -> Option<GpuInstance<'_>> {
    let rest = name.strip_prefix("pid_")?;
    let (pid, rest) = rest.split_once('_')?;
    let (engine, engtype) = rest.split_once("_engtype_").unwrap_or((rest, ""));
    if !engine.starts_with("luid_") {
        return None;
    }
    let adapter = engine.split("_eng_").next()?;
    Some(GpuInstance { pid: pid.parse().ok()?, adapter, engine, engtype })
}

/// System GPU % = busiest engine (summed over processes); per-process % = its busiest engine;
/// per adapter: busiest engine overall and per engine type. Borrows the instance names, so the
/// hundreds of instances a tick don't each allocate.
pub fn gpu_from(items: &[(String, f64)]) -> (Option<f32>, BTreeMap<u32, f32>, Vec<GpuSample>) {
    let mut per_engine: BTreeMap<(&str, &str, &str), f64> = BTreeMap::new();
    let mut per_pid: BTreeMap<u32, f32> = BTreeMap::new();
    for (name, value) in items {
        let Some(i) = parse_gpu_instance(name) else { continue };
        let v = value.max(0.0);
        *per_engine.entry((i.adapter, i.engine, i.engtype)).or_default() += v;
        let slot = per_pid.entry(i.pid).or_default();
        *slot = slot.max(v.min(100.0) as f32);
    }
    let system = per_engine.values().copied().reduce(f64::max).map(|v| v.min(100.0) as f32);
    let mut adapters: BTreeMap<&str, BTreeMap<&str, f32>> = BTreeMap::new();
    for ((adapter, _, engtype), v) in per_engine {
        let slot = adapters.entry(adapter).or_default().entry(engtype).or_default();
        *slot = slot.max(v.min(100.0) as f32);
    }
    let gpus = adapters
        .into_iter()
        .map(|(adapter, types)| {
            let mut engines: Vec<(String, f32)> = types.into_iter().map(|(t, v)| (t.to_string(), v)).collect();
            engines.sort_by_key(|(t, _)| {
                (PREFERRED_ENGINES.iter().position(|p| p == t).unwrap_or(PREFERRED_ENGINES.len()), t.clone())
            });
            GpuSample {
                key: adapter.to_ascii_lowercase(),
                util_pct: engines.iter().map(|(_, v)| *v).fold(0.0, f32::max),
                engines,
                ..Default::default()
            }
        })
        .collect();
    (system, per_pid, gpus)
}

/// "GPU Adapter Memory" bytes by lowercase adapter instance.
pub fn gpu_memory_from(items: &[(String, f64)]) -> BTreeMap<String, u64> {
    items.iter().map(|(n, v)| (n.to_ascii_lowercase(), v.max(0.0) as u64)).collect()
}
```

`crates/collector/src/gpu.rs` (above the tests):
```rust
//! GPU adapters from DXGI (name, memory sizes, LUID) and their temperature from the kernel
//! adapter performance data (D3DKMTQueryAdapterInfo, KMTQAITYPE_ADAPTERPERFDATA), as Task Manager.
use std::ffi::c_void;
use std::mem::size_of;
use windows::Wdk::Graphics::Direct3D::{
    D3DKMT_ADAPTER_PERFDATA, D3DKMT_CLOSEADAPTER, D3DKMT_OPENADAPTERFROMLUID, D3DKMT_QUERYADAPTERINFO,
    D3DKMTCloseAdapter, D3DKMTOpenAdapterFromLuid, D3DKMTQueryAdapterInfo, KMTQAITYPE_ADAPTERPERFDATA,
};
use windows::Win32::Foundation::LUID;
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIFactory1};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GpuInfo {
    /// Same key as `GpuSample::key`.
    pub key: String,
    /// (HighPart, LowPart) of the adapter LUID.
    pub luid: (i32, u32),
    pub name: String,
    pub dedicated_total: u64,
    pub shared_total: u64,
}

pub fn adapter_key(high: i32, low: u32) -> String {
    format!("luid_0x{:08x}_0x{low:08x}_phys_0", high as u32)
}

/// Hardware adapters in DXGI order, one per LUID (the software rasterizer is skipped).
pub fn list_gpus() -> Vec<GpuInfo> {
    // SAFETY: plain factory creation; COM initialisation is not required for DXGI.
    let Ok(factory) = (unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }) else { return Vec::new() };
    let mut out: Vec<GpuInfo> = Vec::new();
    for i in 0u32.. {
        // SAFETY: enumeration on a valid factory; fails with DXGI_ERROR_NOT_FOUND past the end.
        let Ok(adapter) = (unsafe { factory.EnumAdapters1(i) }) else { break };
        // SAFETY: plain query on a valid adapter.
        let Ok(desc) = (unsafe { adapter.GetDesc1() }) else { continue };
        if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
            continue;
        }
        let key = adapter_key(desc.AdapterLuid.HighPart, desc.AdapterLuid.LowPart);
        if out.iter().any(|g| g.key == key) {
            continue;
        }
        let end = desc.Description.iter().position(|&c| c == 0).unwrap_or(desc.Description.len());
        out.push(GpuInfo {
            key,
            luid: (desc.AdapterLuid.HighPart, desc.AdapterLuid.LowPart),
            name: String::from_utf16_lossy(&desc.Description[..end]).trim().to_string(),
            dedicated_total: desc.DedicatedVideoMemory as u64,
            shared_total: desc.SharedSystemMemory as u64,
        });
    }
    out
}

/// Adapter temperature is reported in tenths of a degree; 0 means the driver has no sensor.
pub fn deci_celsius(raw: u32) -> Option<f32> {
    (raw > 0 && raw < 2000).then(|| raw as f32 / 10.0)
}

/// Kernel adapter handles opened once; queried every tick; closed on drop.
pub struct GpuTemps {
    handles: Vec<(String, u32)>,
}

impl GpuTemps {
    pub fn open(gpus: &[GpuInfo]) -> Self {
        let mut handles = Vec::new();
        for g in gpus {
            let mut open = D3DKMT_OPENADAPTERFROMLUID {
                AdapterLuid: LUID { LowPart: g.luid.1, HighPart: g.luid.0 },
                hAdapter: 0,
            };
            // SAFETY: `open` is a valid in/out struct; the handle is closed in Drop.
            if unsafe { D3DKMTOpenAdapterFromLuid(&mut open) }.is_ok() {
                handles.push((g.key.clone(), open.hAdapter));
            }
        }
        Self { handles }
    }

    pub fn read(&self) -> Vec<(String, f32)> {
        self.handles
            .iter()
            .filter_map(|(key, h)| {
                let mut perf = D3DKMT_ADAPTER_PERFDATA { PhysicalAdapterIndex: 0, ..Default::default() };
                let mut query = D3DKMT_QUERYADAPTERINFO {
                    hAdapter: *h,
                    Type: KMTQAITYPE_ADAPTERPERFDATA,
                    pPrivateDriverData: &mut perf as *mut _ as *mut c_void,
                    PrivateDriverDataSize: size_of::<D3DKMT_ADAPTER_PERFDATA>() as u32,
                };
                // SAFETY: valid adapter handle; `perf` is the documented output for this query type
                // and outlives the call.
                let ok = unsafe { D3DKMTQueryAdapterInfo(&mut query) }.is_ok();
                if ok { deci_celsius(perf.Temperature).map(|c| (key.clone(), c)) } else { None }
            })
            .collect()
    }
}

impl Drop for GpuTemps {
    fn drop(&mut self) {
        for (_, h) in &self.handles {
            let close = D3DKMT_CLOSEADAPTER { hAdapter: *h };
            // SAFETY: closes a handle opened in `open`, exactly once.
            unsafe {
                let _ = D3DKMTCloseAdapter(&close);
            }
        }
    }
}
```

`crates/collector/src/lib.rs`: add `pub mod gpu;`.

`crates/collector/src/sysinfo.rs`: add `pub gpus: Vec<crate::gpu::GpuInfo>,` (doc: `/// Hardware GPUs in DXGI order.`) to `SystemInfo` and `gpus: crate::gpu::list_gpus()` to `read_system_info`.

`crates/collector/src/collector.rs`: add `gpu_temps: crate::gpu::GpuTemps`, built in `new` after `info` with `GpuTemps::open(&info.gpus)` (build `info` into a local first), and in the `Ok(pdh)` arm:
```rust
                totals.gpus = r.gpus;
                if let Some(gpus) = totals.gpus.as_mut() {
                    for (key, c) in self.gpu_temps.read() {
                        if let Some(g) = gpus.iter_mut().find(|g| g.key == key) {
                            g.temp_c = Some(c);
                        }
                    }
                }
```

`crates/app/src/format.rs`:
```rust
pub fn temp_c(c: f32) -> String {
    if c.is_finite() { format!("{:.0} °C", c) } else { "—".to_string() }
}
```

`crates/app/src/view.rs`, in `meters`, the GPU meter's `sub`:
```rust
            sub: t
                .gpus
                .as_ref()
                .and_then(|g| g.iter().max_by(|a, b| a.util_pct.total_cmp(&b.util_pct)))
                .and_then(|g| g.temp_c)
                .map(format::temp_c)
                .unwrap_or_default(),
```

- [ ] **Step 4: Run tests**

Run: `cargo test --workspace`
Expected: all pass, warning-free.

- [ ] **Step 5: Commit**

```bash
git add crates/snapshot crates/collector crates/app/src
git commit -m "feat(collector): per-GPU engines, memory and temperature; GPU temperature on the GPU meter"
```

---
### Task 4: History by time, with per-device series

**Files:**
- Rewrite: `crates/history/src/lib.rs`
- Modify: `crates/app/src/view.rs` (meter sparklines read series), `crates/app/src/worker.rs`, `crates/probe/src/main.rs`
- Test: unit tests in `crates/history/src/lib.rs`; existing `view.rs` tests

**Interfaces:**
- Consumes: Tasks 1–3 `SystemTotals` (per-core slots, memory commit, disks with response, adapters, GPUs).
- Produces (`negotium_history`):
  - consts `WINDOW_MS: u64 = 3_600_000`, `MIN_SPACING_MS: u64 = 950`, `MAX_SAMPLES: usize = 3_601`, `TOP_PER_METRIC: usize = 10`. (`DEFAULT_CAPACITY` and `HistorySample` are removed.)
  - `#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)] pub enum Series { Cpu, Core(u16), MemUsed, Committed, DiskBusiest, Gpu, NetTotal, DiskActive(String), DiskRead(String), DiskWrite(String), DiskResponse(String), NetSent(String), NetRecv(String), GpuUtil(String), GpuEngine(String, String), GpuDedicated(String), GpuShared(String), GpuTemp(String) }` with `is_pct(&self) -> bool`. Device names are the snapshot keys (`DiskSample::name`, `AdapterSample::name`, `GpuSample::key`; engine = engine type).
  - `History::new() -> History` (also `Default`), `push(&mut self, t_ms: u64, totals: &SystemTotals, rows: &[AppRow]) -> bool` (false = not recorded: under `MIN_SPACING_MS` since the last kept sample), `len`, `is_empty`, `latest_t(&self) -> Option<u64>`, `has(&self, key: &Series) -> bool`, `points<'a>(&'a self, key: &Series, since_ms: u64) -> impl Iterator<Item = (u64, Option<f32>)> + use<'a>`, `top_at(&self, t_ms: u64, metric: Metric, max_gap_ms: u64) -> Option<(u64, Vec<(String, f32)>)>` (nearest sample within the gap: its time and top apps, largest first), `approx_bytes(&self) -> usize`.

- [ ] **Step 1: Write the failing tests** — the `tests` module of the new `crates/history/src/lib.rs`:

```rust
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
        DiskSample { name: name.into(), active_pct: active, read_bps: 1e6, write_bps: 2e6, response_ms: 0.5 }
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
    fn a_device_that_appears_late_is_backfilled_and_one_that_leaves_ages_out() {
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
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p negotium-history`
Expected: FAIL to compile (no `Series`, `History::new()` arity, no `points`, `has`, `latest_t`, `top_at` arity…).

- [ ] **Step 3: Implement** — the rest of `crates/history/src/lib.rs`:

```rust
//! In-memory history (spec §4.4): one hour of system totals, per-device series and the top apps
//! per metric. At most one sample per second is kept whatever the refresh rate (ruling P3-3),
//! and samples older than an hour are dropped. Every column is preallocated to MAX_SAMPLES and
//! reused, so steady-state pushes don't grow memory (spec §8). App keys are interned to small
//! ids so an hour fits in about 2 MB.
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
    fn new(pct: bool, missing: usize, generation: u64) -> Self {
        let data = if pct {
            let mut d = VecDeque::with_capacity(MAX_SAMPLES);
            d.resize(missing, PCT_MISSING);
            Data::Pct(d)
        } else {
            let mut d = VecDeque::with_capacity(MAX_SAMPLES);
            d.resize(missing, f32::NAN);
            Data::Value(d)
        };
        Self { data, present: 0, generation }
    }

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
    columns: HashMap<Series, Column>,
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
            columns: HashMap::new(),
            generation: 0,
            ids: HashMap::new(),
            names: Vec::new(),
            keys: Vec::new(),
            refs: Vec::new(),
            free: Vec::new(),
        }
    }

    /// Records one sample. Returns false (and records nothing) when it comes less than
    /// MIN_SPACING_MS after the last kept sample.
    pub fn push(&mut self, t_ms: u64, totals: &SystemTotals, rows: &[AppRow]) -> bool {
        if self.times.back().is_some_and(|&last| t_ms < last.saturating_add(MIN_SPACING_MS)) {
            return false;
        }
        let oldest = t_ms.saturating_sub(WINDOW_MS);
        while self.times.front().is_some_and(|&t| t < oldest) || self.times.len() >= MAX_SAMPLES {
            self.evict();
        }
        self.generation += 1;
        self.record_totals(totals);
        let generation = self.generation;
        for col in self.columns.values_mut() {
            if col.generation != generation {
                col.push(None);
            }
        }
        // A device that left is dropped once its last value has aged out.
        self.columns.retain(|_, c| c.present > 0 || c.generation == generation);

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

    fn record(&mut self, key: Series, v: Option<f32>) {
        let (missing, generation) = (self.times.len(), self.generation);
        let col = self.columns.entry(key).or_insert_with_key(|k| Column::new(k.is_pct(), missing, generation.wrapping_sub(1)));
        if col.generation == generation {
            return; // a duplicate device name this tick: the first one wins, columns stay aligned
        }
        col.generation = generation;
        col.push(v);
    }

    fn record_totals(&mut self, t: &SystemTotals) {
        self.record(Series::Cpu, t.cpu_pct);
        for (i, v) in t.per_core_pct.iter().enumerate() {
            self.record(Series::Core(i as u16), *v);
        }
        self.record(Series::MemUsed, t.memory.as_ref().map(|m| m.used as f32));
        self.record(Series::Committed, t.memory.as_ref().map(|m| m.commit_total as f32));
        self.record(Series::DiskBusiest, t.busiest_disk_pct());
        self.record(Series::Gpu, t.gpu_pct);
        self.record(Series::NetTotal, t.net_bps.map(|v| v as f32));
        for d in t.disks.iter().flatten() {
            self.record(Series::DiskActive(d.name.clone()), Some(d.active_pct));
            self.record(Series::DiskRead(d.name.clone()), Some(d.read_bps as f32));
            self.record(Series::DiskWrite(d.name.clone()), Some(d.write_bps as f32));
            self.record(Series::DiskResponse(d.name.clone()), Some(d.response_ms));
        }
        for a in t.adapters.iter().flatten() {
            self.record(Series::NetSent(a.name.clone()), Some(a.sent_bps as f32));
            self.record(Series::NetRecv(a.name.clone()), Some(a.recv_bps as f32));
        }
        for g in t.gpus.iter().flatten() {
            self.record(Series::GpuUtil(g.key.clone()), Some(g.util_pct));
            for (engtype, v) in &g.engines {
                self.record(Series::GpuEngine(g.key.clone(), engtype.clone()), Some(*v));
            }
            self.record(Series::GpuDedicated(g.key.clone()), g.dedicated_used.map(|b| b as f32));
            self.record(Series::GpuShared(g.key.clone()), g.shared_used.map(|b| b as f32));
            self.record(Series::GpuTemp(g.key.clone()), g.temp_c);
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
        for col in self.columns.values_mut() {
            col.pop_front();
        }
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
        self.columns.contains_key(key)
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
        let t = self.times[i];
        if t.abs_diff(t_ms) > max_gap_ms {
            return None;
        }
        let top = &self.tops[i][metric as usize];
        let apps = (0..top.len as usize).map(|k| (self.name(top.ids[k]).to_string(), top.values[k])).collect();
        Some((t, apps))
    }

    pub fn approx_bytes(&self) -> usize {
        let samples = self.times.capacity() * size_of::<u64>() + self.tops.capacity() * size_of::<[Top; 5]>();
        let columns: usize = self.columns.values().map(|c| c.bytes() + size_of::<Series>() + 16).sum();
        let names: usize = self.names.iter().map(|n| n.capacity() + size_of::<String>()).sum::<usize>()
            + self.keys.capacity() * size_of::<Option<AppKey>>()
            + self.refs.capacity() * size_of::<u32>()
            + self.ids.capacity() * (size_of::<AppKey>() + size_of::<u16>() + 8);
        samples + columns + names
    }
}
```

Callers:
- `crates/app/src/worker.rs`: `use negotium_history::History;` and `history: History::new(),`.
- `crates/probe/src/main.rs`: `use negotium_history::History;` and `let mut history = History::new();`.
- `crates/app/src/view.rs`: in `meters`, replace the `samples` vector with series reads (Task 5 replaces `spark_path` itself):
```rust
use negotium_history::{History, Series};

fn meters(snap: &Snapshot, history: &History) -> Vec<MeterView> {
    let t = &snap.totals;
    let since = snap.taken_at_ms.saturating_sub(SPARK_SECONDS * 1000);
    let series = |key: Series| -> Vec<Option<f32>> { history.points(&key, since).map(|(_, v)| v).collect() };
    let mem_total = t.memory.as_ref().map(|m| m.total as f32).unwrap_or(0.0);
    let net_series = series(Series::NetTotal);
    let net_max = net_series.iter().flatten().fold(0.0f32, |a, b| a.max(*b)).max(t.net_bps.unwrap_or(0.0) as f32);
    // …the five MeterViews as before, with:
    //   CPU:     spark_path(&series(Series::Cpu), 100.0)
    //   Memory:  spark_path(&series(Series::MemUsed), mem_total)
    //   Disk:    spark_path(&series(Series::DiskBusiest), 100.0)
    //   GPU:     spark_path(&series(Series::Gpu), 100.0)
    //   Network: spark_path(&net_series, net_max)
```
  and in its tests replace every `History::new(10)` with `History::new()`.

- [ ] **Step 4: Run tests**

Run: `cargo test --workspace`
Expected: all pass, warning-free. Note the two budget figures from the test output (`cargo test -p negotium-history -- --nocapture` after temporarily printing is not needed: if a budget assertion fails, its message prints the byte count).

- [ ] **Step 5: Commit**

```bash
git add crates/history crates/app/src crates/probe
git commit -m "feat(history): one hour by time with per-device series and compact top lists"
```

---

### Task 5: Time-based graph geometry; Overview sparklines use it

**Files:**
- Create: `crates/app/src/graph.rs`
- Modify: `crates/app/src/lib.rs`, `crates/app/src/view.rs`, `crates/app/ui/meters.slint`
- Test: unit tests in `graph.rs`, `view.rs`

**Interfaces:**
- Consumes: `History::points`, `Series` (Task 4).
- Produces (`negotium::graph`): `VIEW_W: f64 = 1000.0`, `EMPTY_PATH: &str = "M 0 100"`, `#[derive(Clone, Debug, Default, PartialEq)] pub struct Shape { pub line: String, pub area: String }`, `shape(points: impl IntoIterator<Item = (u64, Option<f32>)>, end: u64, span: u64, max: f32, buckets: usize) -> Shape` (the window may reach back before collector time 0; the newest sample is still on the right edge), `fraction_to_time(fraction: f32, end: u64, span: u64) -> i64` (negative before the collector started), `nice_max(v: f32, floor: f32) -> f32`. Paths are for a `viewbox-width: 1000; viewbox-height: 100` Slint `Path`. `view::spark_path` is removed; meter sparks use `shape(..).line` over the last 60 s with 60 buckets.

- [ ] **Step 1: Write the failing tests** (bottom of new `crates/app/src/graph.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn xs(path: &str) -> Vec<f64> {
        let t: Vec<&str> = path.split_whitespace().collect();
        t.chunks(3).filter(|c| c.len() == 3).map(|c| c[1].parse().unwrap()).collect()
    }

    #[test]
    fn newest_sample_sits_on_the_right_edge_and_x_follows_time() {
        let s = shape([(58_000, Some(0.0)), (59_000, Some(50.0)), (60_000, Some(100.0))], 60_000, 60_000, 100.0, 60);
        assert_eq!(s.line, "M 966.7 100.0 L 983.3 50.0 L 1000.0 0.0");
        assert_eq!(s.area, "M 966.7 100.0 L 966.7 100.0 L 983.3 50.0 L 1000.0 0.0 L 1000.0 100.0 Z");
    }

    #[test]
    fn a_missing_value_breaks_the_line() {
        let s = shape([(58_000, Some(10.0)), (59_000, None), (60_000, Some(10.0))], 60_000, 60_000, 100.0, 60);
        assert_eq!(s.line, "M 966.7 90.0 M 1000.0 90.0");
        assert_eq!(s.area.matches('Z').count(), 2);
    }

    #[test]
    fn sparse_samples_are_joined_not_broken() {
        // 5 s refresh on a 1 min graph with 60 columns: most columns hold no sample at all.
        let s = shape([(50_000, Some(10.0)), (55_000, Some(20.0)), (60_000, Some(30.0))], 60_000, 60_000, 100.0, 60);
        assert_eq!(s.line.matches('M').count(), 1);
    }

    #[test]
    fn empty_or_flat_input_draws_safely() {
        assert_eq!(shape(std::iter::empty(), 60_000, 60_000, 100.0, 60), Shape { line: EMPTY_PATH.into(), area: EMPTY_PATH.into() });
        assert_eq!(shape([(60_000, Some(5.0))], 60_000, 60_000, 0.0, 60).line, "M 1000.0 100.0", "max 0 draws on the floor");
        assert_eq!(shape([(1, Some(f32::NAN))], 60_000, 60_000, 100.0, 60).line, EMPTY_PATH);
    }

    #[test]
    fn a_minute_of_data_in_the_hour_view_only_fills_the_right_edge() {
        let end = 4_000_000u64;
        let pts: Vec<(u64, Option<f32>)> = (0..60).map(|i| (end - 59_000 + i * 1_000, Some(50.0))).collect();
        let s = shape(pts, end, 3_600_000, 100.0, 300);
        assert!(xs(&s.line).iter().all(|&x| x >= 980.0), "{}", s.line);
    }

    #[test]
    fn many_samples_collapse_to_at_most_one_point_per_column_keeping_spikes() {
        let end = 3_600_000u64;
        let pts: Vec<(u64, Option<f32>)> = (0..3_600u64).map(|i| (i * 1_000, Some(if i == 1_800 { 100.0 } else { 0.0 }))).collect();
        let s = shape(pts, end, 3_600_000, 100.0, 300);
        assert!(xs(&s.line).len() <= 300);
        assert!(s.line.contains(" 0.0"), "the one-second spike survives downsampling: {}", &s.line[..80]);
    }

    #[test]
    fn right_after_start_the_graph_still_ends_at_the_right_edge() {
        // Collector time is only 2 s old but the range is an hour.
        let s = shape([(1_000, Some(50.0)), (2_000, Some(50.0))], 2_000, 3_600_000, 100.0, 300);
        assert_eq!(s.line, "M 1000.0 50.0");
    }

    #[test]
    fn hover_fraction_maps_to_time() {
        assert_eq!(fraction_to_time(1.0, 60_000, 60_000), 60_000);
        assert_eq!(fraction_to_time(0.5, 60_000, 60_000), 30_000);
        assert_eq!(fraction_to_time(-3.0, 60_000, 60_000), 0);
        assert_eq!(fraction_to_time(0.1, 61_000, 3_600_000), -3_179_000, "before the collector started");
    }

    #[test]
    fn scales_round_up_to_one_two_five() {
        assert_eq!(nice_max(0.0, 1_000.0), 1_000.0);
        assert_eq!(nice_max(1_300.0, 1_000.0), 2_000.0);
        assert_eq!(nice_max(7_100_000.0, 1.0), 10_000_000.0);
        assert_eq!(nice_max(f32::NAN, 5.0), 5.0);
    }
}
```

In `view.rs` tests replace `spark_path_right_aligns_samples_and_breaks_on_gaps` with:
```rust
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
        let f = build_frame(&mut ids, &s, &[], &[], &h, &opts(Metric::Cpu, true));
        assert_eq!(f.meters[0].spark, "M 966.7 100.0 L 983.3 50.0 L 1000.0 0.0");
    }

    #[test]
    fn an_idle_network_draws_a_flat_line() {
        let mut h = History::new();
        let mut s = snap(Some(1.0));
        s.totals.net_bps = Some(0.0);
        h.push(60_000, &s.totals, &[]);
        let mut ids = IdMap::new();
        let f = build_frame(&mut ids, &s, &[], &[], &h, &opts(Metric::Cpu, true));
        assert_eq!(f.meters[4].spark, "M 1000.0 100.0");
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p negotium --lib`
Expected: FAIL to compile (no `graph` module).

- [ ] **Step 3: Implement**

`crates/app/src/graph.rs` (above the tests):
```rust
//! Graph geometry: (time, value) series as Slint path commands for a 1000 × 100 view box. x is
//! time, so a graph always spans its whole range with the newest sample on the right edge; a
//! range with little history draws only where history exists (never stretched).
use std::fmt::Write;

pub const VIEW_W: f64 = 1000.0;
/// A path that draws nothing (Slint needs a valid command string).
pub const EMPTY_PATH: &str = "M 0 100";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shape {
    pub line: String,
    /// The line closed down to the floor, for a translucent fill.
    pub area: String,
}

#[derive(Default)]
struct Pen {
    line: String,
    area: String,
    down: bool,
    last_x: f64,
}

impl Pen {
    fn point(&mut self, x: f64, y: f64) {
        if !self.line.is_empty() {
            self.line.push(' ');
        }
        let _ = write!(self.line, "{} {x:.1} {y:.1}", if self.down { "L" } else { "M" });
        if self.down {
            let _ = write!(self.area, " L {x:.1} {y:.1}");
        } else {
            if !self.area.is_empty() {
                self.area.push(' ');
            }
            let _ = write!(self.area, "M {x:.1} 100.0 L {x:.1} {y:.1}");
        }
        self.down = true;
        self.last_x = x;
    }

    fn gap(&mut self) {
        if self.down {
            let _ = write!(self.area, " L {:.1} 100.0 Z", self.last_x);
            self.down = false;
        }
    }

    fn finish(mut self) -> Shape {
        self.gap();
        let or_empty = |s: String| if s.is_empty() { EMPTY_PATH.to_string() } else { s };
        Shape { line: or_empty(self.line), area: or_empty(self.area) }
    }
}

/// Draws `points` in the window `[end - span, end]` merged into at most `buckets` columns; a
/// column shows its largest value (ruling P3-5) at its right edge, a column holding only missing
/// values breaks the line, and columns without any sample are simply skipped. `max <= 0` draws
/// values on the floor.
pub fn shape(points: impl IntoIterator<Item = (u64, Option<f32>)>, end: u64, span: u64, max: f32, buckets: usize) -> Shape {
    let span = span.max(1);
    let buckets = buckets.max(1);
    // Negative right after start: the window then reaches back before collector time 0, and the
    // little history there is still drawn at the right edge.
    let start = end as i64 - span as i64;
    let mut pen = Pen::default();
    let mut current: Option<(usize, Option<f32>)> = None;
    let emit = |pen: &mut Pen, b: usize, best: Option<f32>| match best {
        Some(v) => {
            let x = (b + 1) as f64 / buckets as f64 * VIEW_W;
            let y = if max > 0.0 { 100.0 - (v / max).clamp(0.0, 1.0) as f64 * 100.0 } else { 100.0 };
            pen.point(x, y);
        }
        None => pen.gap(),
    };
    for (t, v) in points {
        let t = t as i64;
        if t < start || t > end as i64 {
            continue;
        }
        // Columns are (start + k·w, start + (k+1)·w]: the newest sample lands in the last one.
        let b = (((t - start) as u64 * buckets as u64).div_ceil(span) as usize).saturating_sub(1).min(buckets - 1);
        let v = v.filter(|v| v.is_finite());
        match &mut current {
            Some((cur, best)) if *cur == b => {
                if let Some(v) = v {
                    *best = Some(best.map_or(v, |b| b.max(v)));
                }
            }
            _ => {
                if let Some((cur, best)) = current.take() {
                    emit(&mut pen, cur, best);
                }
                current = Some((b, v));
            }
        }
    }
    if let Some((cur, best)) = current {
        emit(&mut pen, cur, best);
    }
    pen.finish()
}

/// The collector time under a pointer `fraction` of the way across a graph of `[end - span, end]`;
/// negative when that moment lies before the collector started.
pub fn fraction_to_time(fraction: f32, end: u64, span: u64) -> i64 {
    let f = if fraction.is_finite() { fraction.clamp(0.0, 1.0) } else { 1.0 };
    end as i64 - span as i64 + (f as f64 * span as f64).round() as i64
}

/// A readable top-of-scale for rate graphs: the next 1, 2 or 5 × 10ⁿ at or above `v`, at least `floor`.
pub fn nice_max(v: f32, floor: f32) -> f32 {
    let v = if v.is_finite() { v.max(floor) } else { floor };
    if v <= 0.0 {
        return floor;
    }
    let mag = 10f32.powi(v.log10().floor() as i32);
    [1.0, 2.0, 5.0, 10.0].into_iter().map(|m| m * mag).find(|&s| s >= v).unwrap_or(v)
}
```

`crates/app/src/lib.rs`: add `pub mod graph;`.

`crates/app/src/view.rs`: delete `spark_path`; in `meters` replace `series` and every `spark_path(..)` with:
```rust
    let span = SPARK_SECONDS * 1000;
    let now = snap.taken_at_ms;
    let since = now.saturating_sub(span);
    let spark = |key: Series, max: f32| graph::shape(history.points(&key, since), now, span, max, SPARK_SECONDS as usize).line;
    let net_max = history.points(&Series::NetTotal, since).filter_map(|(_, v)| v).fold(0.0f32, f32::max).max(t.net_bps.unwrap_or(0.0) as f32);
    //   CPU spark: spark(Series::Cpu, 100.0)        Memory: spark(Series::MemUsed, mem_total)
    //   Disk:      spark(Series::DiskBusiest, 100.0) GPU:   spark(Series::Gpu, 100.0)
    //   Network:   spark(Series::NetTotal, net_max)
```
with `use crate::graph;` at the top.

`crates/app/ui/meters.slint`: in the meter's `Path`, change `viewbox-width: 60;` to `viewbox-width: 1000;` and add `fill: transparent;`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p negotium`
Expected: all pass (lib and UI tests), warning-free.

- [ ] **Step 5: Commit**

```bash
git add crates/app
git commit -m "feat(app): time-based graph paths; Overview sparklines by time, idle network drawn flat"
```

---
### Task 6: Performance view model — device list, CPU and Memory pages

**Files:**
- Create: `crates/app/src/perf.rs`
- Modify: `crates/app/src/lib.rs`, `crates/app/src/format.rs`, `crates/model/src/ranking.rs` (`Metric: Default`)
- Test: unit tests in `perf.rs`, `format.rs`

**Interfaces:**
- Consumes: `graph::{shape, Shape, EMPTY_PATH}` (Task 5), `History::points`, `Series` (Task 4), `SystemInfo` (Tasks 1, 3), snapshot fields from Tasks 1–3.
- Produces (`negotium::perf`):
  - consts `MAIN_BUCKETS = 300`, `CELL_BUCKETS = 60`, `SPARK_MS: u64 = 60_000`, `CPU_TEMP_NOTE: &str`.
  - `#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)] pub enum Range { #[default] Minute, TenMinutes, Hour }` with `ALL`, `span_ms(self) -> u64`, `from_index(i32) -> Range` (clamped), `index(self) -> i32`.
  - `#[derive(Clone, Debug, PartialEq, Eq, Hash)] pub enum Device { Cpu, Memory }` (Task 7 adds `Disk(String)`, `Network(String)`, `Gpu(String)`) with `id(&self) -> String` (`"cpu"`, `"memory"`) and `parse(&str) -> Option<Device>`.
  - `#[derive(Clone, Debug, Default, PartialEq)]` structs `DeviceView { id, title, value, spark: String }`, `GraphView { label, value, scale, line, area, line2: String }`, `StatView { label, value: String }`, `SegmentView { label: String, value: String, fraction: f32 }`, and `PerfFrame { devices: Vec<DeviceView>, requested: String, selected: String, title: String, subtitle: String, note: String, graphs: Vec<GraphView>, cells: Vec<GraphView>, segments: Vec<SegmentView>, stats: Vec<StatView>, range: Range, window_end_ms: u64, span_ms: u64, end_clock_ms: u64, scrub_metric: Metric }`.
  - `pub struct PerfInput<'a> { pub snap: &'a Snapshot, pub history: &'a History, pub info: &'a SystemInfo, pub device: &'a str, pub range: Range }`; `devices(snap: &Snapshot, info: &SystemInfo) -> Vec<Device>`; `build_perf(input: &PerfInput) -> PerfFrame`; `composition(m: &MemoryTotals) -> Vec<SegmentView>`.
  - `negotium::format::{gb(u64) -> String, gb_of(used: u64, total: u64) -> String, ghz(mhz: f32) -> String, uptime(secs: u64) -> String}`.
  - `negotium_model::Metric` derives `Default` (`Cpu`).

- [ ] **Step 1: Write the failing tests**

`crates/app/src/format.rs` tests:
```rust
    #[test]
    fn sizes_speeds_and_uptime() {
        assert_eq!(gb(7 << 30), "7.0 GB");
        assert_eq!(gb_of(7 << 30, 16 << 30), "7.0 / 16.0 GB");
        assert_eq!(ghz(3420.0), "3.42 GHz");
        assert_eq!(ghz(0.0), "—");
        assert_eq!(uptime(93_784), "1:02:03:04");
        assert_eq!(uptime(59), "0:00:00:59");
    }
```

`crates/app/src/perf.rs` — tests first (implementation in Step 3):
```rust
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
    }

    #[test]
    fn the_hour_range_draws_only_recorded_history() {
        let f = build_with(&snapshot(4_000_000), &info(), "cpu", Range::Hour);
        assert_eq!(f.graphs[0].line, "M 1000.0 63.0", "one second of data: one point at the right edge");
        assert_eq!(f.span_ms, 3_600_000);
    }
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p negotium --lib`
Expected: FAIL to compile (no `perf` module, no `format::gb` …).

- [ ] **Step 3: Implement**

`crates/model/src/ranking.rs`: change `#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]` on `Metric` to add `Default` and mark `#[default] Cpu = 0,`.

`crates/app/src/format.rs`:
```rust
const GIB: f64 = (1u64 << 30) as f64;

pub fn gb(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / GIB)
}

pub fn gb_of(used: u64, total: u64) -> String {
    format!("{:.1} / {:.1} GB", used as f64 / GIB, total as f64 / GIB)
}

pub fn ghz(mhz: f32) -> String {
    if mhz.is_finite() && mhz > 0.0 { format!("{:.2} GHz", mhz / 1000.0) } else { "—".to_string() }
}

/// Task Manager's "Up time": days:hours:minutes:seconds.
pub fn uptime(secs: u64) -> String {
    format!("{}:{:02}:{:02}:{:02}", secs / 86_400, secs / 3_600 % 24, secs / 60 % 60, secs % 60)
}
```

`crates/app/src/perf.rs` (above the tests):
```rust
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
        graph::shape(self.history.points(key, self.end().saturating_sub(SPARK_MS)), self.end(), SPARK_MS, max, 60).line
    }

    /// Largest value of `key` within the last `span` ms.
    fn max_over(&self, key: &Series, span: u64) -> f32 {
        self.history.points(key, self.end().saturating_sub(span)).filter_map(|(_, v)| v).fold(0.0, f32::max)
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
    f.cells = t
        .per_core_pct
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let shape = input.shape(&Series::Core(i as u16), 100.0, CELL_BUCKETS);
            graph_view(&format!("CPU {i}"), v.map(format::pct).unwrap_or_else(dash), String::new(), shape)
        })
        .collect();
    f.stats = vec![
        stat("Utilization", util),
        stat("Speed", t.cpu_mhz.map(format::ghz).unwrap_or_else(dash)),
        stat("Temperature", dash()),
        stat("Processes", procs.len().to_string()),
        stat("Threads", procs.iter().map(|p| p.threads as u64).sum::<u64>().to_string()),
        stat("Handles", procs.iter().map(|p| p.handles as u64).sum::<u64>().to_string()),
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
        stat("In use", format::gb(m.used)),
        stat("Available", format::gb(m.total.saturating_sub(m.used))),
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
```

`crates/app/src/lib.rs`: add `pub mod perf;`.

- [ ] **Step 4: Run tests**

Run: `cargo test --workspace`
Expected: all pass, warning-free.

- [ ] **Step 5: Commit**

```bash
git add crates/app/src crates/model/src/ranking.rs
git commit -m "feat(app): Performance view model with the CPU and Memory pages"
```

---

### Task 7: Performance view model — disk, network and GPU pages

**Files:**
- Modify: `crates/app/src/perf.rs`, `crates/app/src/format.rs`
- Test: unit tests in `perf.rs`, `format.rs`

**Interfaces:**
- Consumes: Task 6's `perf` items; `AdapterSample`, `DiskSample`, `GpuSample`, `GpuInfo`.
- Produces: `Device` gains `Disk(String)` (`"disk:<instance>"`), `Network(String)` (`"net:<instance>"`), `Gpu(String)` (`"gpu:<key>"`); `devices()` lists CPU, Memory, disks by disk number, connected adapters (hardware first, then by title), GPUs in DXGI order (unknown GPUs last); `perf::{disk_title(&str) -> String, adapter_title(&AdapterSample) -> String}`; `format::millis(f32) -> String` ("0.4 ms").

- [ ] **Step 1: Write the failing tests**

`format.rs` tests: add `assert_eq!(millis(0.4), "0.4 ms"); assert_eq!(millis(f32::NAN), "—");` to `sizes_speeds_and_uptime`.

`perf.rs` tests — add:
```rust
    use negotium_collector::gpu::GpuInfo;
    use negotium_snapshot::{AdapterSample, DiskSample, GpuSample};

    fn devices_snapshot() -> Snapshot {
        let mut s = snapshot(60_000);
        let disk = |name: &str, active: f32| DiskSample { name: name.into(), active_pct: active, read_bps: 1_048_576.0, write_bps: 2_097_152.0, response_ms: 0.4 };
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

    #[test]
    fn devices_are_listed_in_task_manager_order() {
        let f = build_with(&devices_snapshot(), &devices_info(), "cpu", Range::Minute);
        let ids: Vec<&str> = f.devices.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, [
            "cpu", "memory", "disk:0 C:", "disk:1 D:",
            "net:Intel[R] Ethernet", "net:Hyper-V Virtual Ethernet Adapter",
            "gpu:luid_b", "gpu:luid_a",
        ], "disconnected adapters have no page; hardware adapters first; GPUs in DXGI order");
        let titles: Vec<&str> = f.devices.iter().map(|d| d.title.as_str()).collect();
        assert_eq!(&titles[2..], ["Disk 0 (C:)", "Disk 1 (D:)", "Ethernet", "vEthernet (Default Switch)", "GPU 0", "GPU 1"]);
        assert_eq!(f.devices[6].value, "40% (64 °C)");
        assert_eq!(f.devices[7].value, "5%");
        for d in [Device::Disk("0 C:".into()), Device::Network("x".into()), Device::Gpu("luid_b".into())] {
            assert_eq!(Device::parse(&d.id()), Some(d));
        }
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
        let f = build_with(&devices_snapshot(), &devices_info(), "disk:0 C:", Range::Minute);
        assert_eq!(f.title, "Disk 0 (C:)");
        assert_eq!(f.graphs.iter().map(|g| g.label.as_str()).collect::<Vec<_>>(), ["Active time", "Transfer rate"]);
        assert_ne!(f.graphs[1].line2, EMPTY_PATH, "write rate drawn as the second line");
        assert_eq!(f.graphs[1].scale, "2.0 MB/s");
        assert_eq!(stat(&f, "Active time"), "12%");
        assert_eq!(stat(&f, "Read speed"), "1.0 MB/s");
        assert_eq!(stat(&f, "Average response time"), "0.4 ms");
        assert_eq!(f.scrub_metric, Metric::Disk);
    }

    #[test]
    fn a_network_page_shows_throughput_link_speed_and_addresses() {
        let f = build_with(&devices_snapshot(), &devices_info(), "net:Intel[R] Ethernet", Range::Minute);
        assert_eq!((f.title.as_str(), f.subtitle.as_str()), ("Ethernet", "Intel(R) Ethernet"));
        assert_eq!(f.graphs[0].label, "Throughput");
        assert_ne!(f.graphs[0].line2, EMPTY_PATH);
        assert_eq!(stat(&f, "Send"), "100.0 Kb/s");
        assert_eq!(stat(&f, "Receive"), "1.0 Mb/s");
        assert_eq!(stat(&f, "Link speed"), "1.0 Gb/s");
        assert_eq!(stat(&f, "Adapter"), "Hardware");
        assert_eq!(stat(&f, "IPv4 address"), "192.168.1.20");
        assert_eq!(stat(&f, "IPv6 address"), "2001:db8::5");
        assert_eq!(f.scrub_metric, Metric::Network);
    }

    #[test]
    fn a_gpu_page_shows_engines_memory_and_temperature() {
        let f = build_with(&devices_snapshot(), &devices_info(), "gpu:luid_b", Range::Minute);
        assert_eq!((f.title.as_str(), f.subtitle.as_str()), ("GPU 0", "NVIDIA GeForce RTX 4090"));
        assert_eq!(f.graphs.iter().map(|g| g.label.as_str()).collect::<Vec<_>>(), ["Utilization", "Dedicated GPU memory", "Shared GPU memory"]);
        assert_eq!(f.cells.iter().map(|c| c.label.as_str()).collect::<Vec<_>>(), ["3D", "Copy"]);
        assert_eq!(stat(&f, "Temperature"), "64 °C");
        assert_eq!(stat(&f, "Dedicated GPU memory"), "3.0 / 24.0 GB");
        assert_eq!(f.scrub_metric, Metric::Gpu);
        let igpu = build_with(&devices_snapshot(), &devices_info(), "gpu:luid_a", Range::Minute);
        assert_eq!(igpu.title, "GPU 1");
        assert_eq!(stat(&igpu, "Temperature"), "—", "no sensor: a dash, not 0 °C");
    }

    #[test]
    fn a_page_whose_device_was_unplugged_falls_back_to_cpu() {
        let f = build_with(&devices_snapshot(), &devices_info(), "disk:2 F:", Range::Minute);
        assert_eq!(f.selected, "cpu");
        let f = build_with(&devices_snapshot(), &devices_info(), "net:Bluetooth Device [Personal Area Network]", Range::Minute);
        assert_eq!(f.selected, "cpu", "a disconnected adapter has no page");
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p negotium --lib perf`
Expected: FAIL to compile (no `Device::Disk`, `disk_title`, `format::millis` …).

- [ ] **Step 3: Implement**

`format.rs`:
```rust
pub fn millis(ms: f32) -> String {
    if ms.is_finite() { format!("{:.1} ms", ms.max(0.0)) } else { "—".to_string() }
}
```

`perf.rs` — extend `Device`, its `id`/`parse`, `devices`, `device_view` and `build_perf`; add the helpers and pages. Imports gain `negotium_snapshot::{AdapterSample, GpuSample}`.
```rust
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

pub fn adapter_title(a: &AdapterSample) -> String {
    a.alias.clone().unwrap_or_else(|| a.name.clone())
}

pub fn devices(snap: &Snapshot, info: &SystemInfo) -> Vec<Device> {
    let t = &snap.totals;
    let mut out = vec![Device::Cpu, Device::Memory];
    let mut disks: Vec<&str> = t.disks.iter().flatten().map(|d| d.name.as_str()).collect();
    disks.sort_by_key(|n| (disk_number(n), n.to_string()));
    out.extend(disks.into_iter().map(|n| Device::Disk(n.to_string())));
    let mut nets: Vec<&AdapterSample> = t.adapters.iter().flatten().filter(|a| a.connected).collect();
    nets.sort_by_key(|a| (!a.hardware, adapter_title(a)));
    out.extend(nets.into_iter().map(|a| Device::Network(a.name.clone())));
    let mut gpus: Vec<&GpuSample> = t.gpus.iter().flatten().collect();
    gpus.sort_by_key(|g| (info.gpus.iter().position(|i| i.key == g.key).unwrap_or(usize::MAX), g.key.clone()));
    out.extend(gpus.into_iter().map(|g| Device::Gpu(g.key.clone())));
    out
}

/// "GPU n" numbering follows the device list.
fn gpu_number(list: &[Device], key: &str) -> usize {
    list.iter().filter(|d| matches!(d, Device::Gpu(_))).position(|d| *d == Device::Gpu(key.to_string())).unwrap_or(0)
}

fn disk<'a>(input: &'a PerfInput, name: &str) -> Option<&'a negotium_snapshot::DiskSample> {
    input.snap.totals.disks.iter().flatten().find(|d| d.name == name)
}

fn adapter<'a>(input: &'a PerfInput, name: &str) -> Option<&'a AdapterSample> {
    input.snap.totals.adapters.iter().flatten().find(|a| a.name == name)
}

fn gpu<'a>(input: &'a PerfInput, key: &str) -> Option<&'a GpuSample> {
    input.snap.totals.gpus.iter().flatten().find(|g| g.key == key)
}
```

`device_view` takes the list (`fn device_view(d: &Device, list: &[Device], input: &PerfInput) -> DeviceView`; update the call in `build_perf` to `device_view(d, &list, input)`) and gains arms:
```rust
        Device::Disk(name) => (
            disk_title(name),
            disk(input, name).map(|d| format::pct(d.active_pct)).unwrap_or_else(dash),
            input.spark(&Series::DiskActive(name.clone()), 100.0),
        ),
        Device::Network(name) => {
            let a = adapter(input, name);
            let key = Series::NetRecv(name.clone());
            let max = graph::nice_max(input.max_over(&key, SPARK_MS) * 8.0, 100_000.0) / 8.0;
            (
                a.map(adapter_title).unwrap_or_else(|| name.clone()),
                a.map(|a| format!("S {} · R {}", format::bit_rate(a.sent_bps), format::bit_rate(a.recv_bps))).unwrap_or_else(dash),
                input.spark(&key, max),
            )
        }
        Device::Gpu(key) => {
            let g = gpu(input, key);
            let value = match g {
                Some(g) => match g.temp_c {
                    Some(c) => format!("{} ({})", format::pct(g.util_pct), format::temp_c(c)),
                    None => format::pct(g.util_pct),
                },
                None => dash(),
            };
            (format!("GPU {}", gpu_number(list, key)), value, input.spark(&Series::GpuUtil(key.clone()), 100.0))
        }
```

`build_perf`'s match gains:
```rust
        Device::Disk(name) => disk_page(&mut f, input, name),
        Device::Network(name) => net_page(&mut f, input, name),
        Device::Gpu(key) => gpu_page(&mut f, input, key, gpu_number(&list, key)),
```

Pages:
```rust
fn disk_page(f: &mut PerfFrame, input: &PerfInput, name: &str) {
    let d = disk(input, name);
    f.title = disk_title(name);
    f.scrub_metric = Metric::Disk;
    f.note = "Top apps show each app's disk use across all disks.".into();
    let (active, read, write, response) = d.map_or((0.0, 0.0, 0.0, 0.0), |d| (d.active_pct, d.read_bps, d.write_bps, d.response_ms));
    let (read_key, write_key) = (Series::DiskRead(name.into()), Series::DiskWrite(name.into()));
    let span = input.range.span_ms();
    // Byte rates are shown in binary units: round the scale in MB/s so the label reads "2.0 MB/s".
    let peak = input.max_over(&read_key, span).max(input.max_over(&write_key, span));
    let scale = graph::nice_max(peak / MIB, 1.0) * MIB;
    let mut transfer = graph_view(
        "Transfer rate",
        format!("Read {} · Write {}", format::byte_rate(read), format::byte_rate(write)),
        format::byte_rate(scale as f64),
        input.shape(&read_key, scale, MAIN_BUCKETS),
    );
    transfer.line2 = input.shape(&write_key, scale, MAIN_BUCKETS).line;
    f.graphs = vec![
        graph_view("Active time", format::pct(active), "100%".into(), input.shape(&Series::DiskActive(name.into()), 100.0, MAIN_BUCKETS)),
        transfer,
    ];
    f.stats = vec![
        stat("Active time", format::pct(active)),
        stat("Read speed", format::byte_rate(read)),
        stat("Write speed", format::byte_rate(write)),
        stat("Average response time", format::millis(response)),
    ];
}

fn net_page(f: &mut PerfFrame, input: &PerfInput, name: &str) {
    let a = adapter(input, name);
    f.title = a.map(adapter_title).unwrap_or_else(|| name.to_string());
    f.subtitle = a.and_then(|a| a.description.clone()).unwrap_or_else(|| name.to_string());
    f.scrub_metric = Metric::Network;
    f.note = "Per-app network use arrives in a later version.".into();
    let (sent, recv) = a.map_or((0.0, 0.0), |a| (a.sent_bps, a.recv_bps));
    let (sent_key, recv_key) = (Series::NetSent(name.into()), Series::NetRecv(name.into()));
    let span = input.range.span_ms();
    // Network rates are shown in decimal bits: round the scale in bits, at least 100 Kb/s.
    let peak = input.max_over(&sent_key, span).max(input.max_over(&recv_key, span));
    let scale = graph::nice_max(peak * 8.0, 100_000.0) / 8.0;
    let mut g = graph_view(
        "Throughput",
        format!("Send {} · Receive {}", format::bit_rate(sent), format::bit_rate(recv)),
        format::bit_rate(scale as f64),
        input.shape(&recv_key, scale, MAIN_BUCKETS),
    );
    g.line2 = input.shape(&sent_key, scale, MAIN_BUCKETS).line;
    f.graphs = vec![g];
    f.stats = vec![
        stat("Send", format::bit_rate(sent)),
        stat("Receive", format::bit_rate(recv)),
        stat("Link speed", a.and_then(|a| a.link_bps).map(|b| format::bit_rate(b as f64 / 8.0)).unwrap_or_else(dash)),
        stat("Adapter", if a.is_some_and(|a| a.hardware) { "Hardware" } else { "Virtual" }),
    ];
    for addr in a.map(|a| a.addresses.as_slice()).unwrap_or_default() {
        f.stats.push(stat(if addr.contains(':') { "IPv6 address" } else { "IPv4 address" }, addr.clone()));
    }
}

fn gpu_page(f: &mut PerfFrame, input: &PerfInput, key: &str, number: usize) {
    let g = gpu(input, key);
    let info = input.info.gpus.iter().find(|i| i.key == key);
    f.title = format!("GPU {number}");
    f.subtitle = info.map(|i| i.name.clone()).unwrap_or_default();
    f.scrub_metric = Metric::Gpu;
    let util = g.map(|g| format::pct(g.util_pct)).unwrap_or_else(dash);
    let memory = |used: Option<u64>, total: Option<u64>| match (used, total) {
        (Some(u), Some(t)) if t > 0 => format::gb_of(u, t),
        (Some(u), _) => format::gb(u),
        _ => dash(),
    };
    let dedicated_total = info.map(|i| i.dedicated_total).filter(|&t| t > 0);
    let shared_total = info.map(|i| i.shared_total).filter(|&t| t > 0);
    let dedicated = memory(g.and_then(|g| g.dedicated_used), dedicated_total);
    let shared = memory(g.and_then(|g| g.shared_used), shared_total);
    f.graphs = vec![graph_view("Utilization", util.clone(), "100%".into(), input.shape(&Series::GpuUtil(key.into()), 100.0, MAIN_BUCKETS))];
    if let Some(total) = dedicated_total {
        let shape = input.shape(&Series::GpuDedicated(key.into()), total as f32, MAIN_BUCKETS);
        f.graphs.push(graph_view("Dedicated GPU memory", dedicated.clone(), format::gb(total), shape));
    }
    if let Some(total) = shared_total {
        let shape = input.shape(&Series::GpuShared(key.into()), total as f32, MAIN_BUCKETS);
        f.graphs.push(graph_view("Shared GPU memory", shared.clone(), format::gb(total), shape));
    }
    f.cells = g
        .map(|g| g.engines.as_slice())
        .unwrap_or_default()
        .iter()
        .map(|(engtype, v)| {
            let shape = input.shape(&Series::GpuEngine(key.into(), engtype.clone()), 100.0, CELL_BUCKETS);
            graph_view(engtype, format::pct(*v), String::new(), shape)
        })
        .collect();
    f.stats = vec![
        stat("Utilization", util),
        stat("Dedicated GPU memory", dedicated),
        stat("Shared GPU memory", shared),
        stat("Temperature", g.and_then(|g| g.temp_c).map(format::temp_c).unwrap_or_else(dash)),
    ];
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p negotium --lib`
Expected: all pass, warning-free.

- [ ] **Step 5: Commit**

```bash
git add crates/app/src
git commit -m "feat(app): disk, network and GPU performance pages"
```

---
### Task 8: Navigation rail and page-aware frames

**Files:**
- Create: `crates/app/ui/rail.slint`, `crates/app/ui/perfpage.slint`
- Modify: `crates/app/ui/main.slint`, `crates/app/src/{controls,view,worker,bridge,main}.rs`
- Test: unit tests in `controls.rs`; `crates/app/tests/ui.rs`

**Interfaces:**
- Consumes: `perf::{PerfFrame, DeviceView, Range, build_perf, PerfInput}` (Tasks 6–7), `Collector::system_info()` (Task 1).
- Produces:
  - `controls::Page` (`#[repr(u8)] #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)] enum Page { #[default] Overview = 0, Performance = 1 }` with `ALL`, `from_index(i32) -> Page` (clamped), `index(self) -> i32`). Plan 3b appends `Startup = 2, Settings = 3`.
  - `Controls::{set_page(Page), page() -> Page, select_device(&str), adopt_device(&str), set_range(Range), perf() -> (String, Range)}`; `set_page`, `select_device`, `set_range` wake the worker; `adopt_device` does not.
  - `view::Frame` gains `pub page: Page` and `pub perf: Option<PerfFrame>`. The worker builds the Overview fields only on the Overview and `perf` only on Performance.
  - `Bridge::apply` drops a frame built for another page, or for a device or range the user has since left; on a fallback (`selected != requested`) it adopts `selected` into `Controls`. `Bridge` keeps `last_perf: Option<PerfFrame>` (used by Task 10).
  - Slint: `MainWindow` properties `page: int` (in-out), `perf-devices: [DeviceData]`, `perf-selected: string` (in-out), `perf-title: string`, `perf-subtitle: string`; callbacks `select-page(int)`, `select-device(string)`; exported struct `DeviceData { id: string, title: string, value: string, spark: string }`. Rail items have accessible labels "Overview page", "Performance page" (checkable, checked when shown); device buttons "<title> performance".

- [ ] **Step 1: Write the failing tests**

`crates/app/src/controls.rs` tests:
```rust
    #[test]
    fn page_and_performance_selection_are_shared_and_wake_the_worker() {
        use crate::perf::Range;
        let c = Controls::default();
        assert_eq!(c.page(), Page::Overview);
        assert_eq!(c.perf(), ("cpu".to_string(), Range::Minute));
        c.set_page(Page::Performance);
        assert!(c.wait(Duration::from_millis(20)));
        assert_eq!(c.page(), Page::Performance);
        c.select_device("memory");
        assert!(c.wait(Duration::from_millis(20)));
        c.set_range(Range::Hour);
        assert!(c.wait(Duration::from_millis(20)));
        assert_eq!(c.perf(), ("memory".to_string(), Range::Hour));
        c.adopt_device("cpu");
        assert!(!c.wait(Duration::from_millis(20)), "a fallback adopted from a frame needs no new frame");
        assert_eq!(c.perf().0, "cpu");
        assert_eq!(Page::from_index(9), Page::Performance);
        assert_eq!(Page::from_index(-2), Page::Overview);
    }
```

`crates/app/tests/ui.rs` — add (new imports at the top of the file: `use negotium::DeviceData; use negotium::controls::Page; use negotium::perf::{DeviceView, PerfFrame};`):
```rust
// ---- Plan 3a: pages ----

#[test]
fn the_rail_switches_pages() {
    let ui = window();
    let picked = Rc::new(RefCell::new(None));
    let p = picked.clone();
    ui.on_select_page(move |i| *p.borrow_mut() = Some(i));
    let perf = El::find_by_accessible_label(&ui, "Performance page").next().expect("rail item");
    assert_eq!(perf.accessible_checked(), Some(false));
    perf.invoke_accessible_default_action();
    assert_eq!(*picked.borrow(), Some(1));
    ui.set_page(1);
    assert_eq!(perf.accessible_checked(), Some(true));
    let overview = El::find_by_accessible_label(&ui, "Overview page").next().expect("rail item");
    assert_eq!(overview.accessible_checked(), Some(false));
}

fn device(id: &str, title: &str) -> DeviceData {
    DeviceData { id: id.into(), title: title.into(), value: "1%".into(), spark: "M 0 100".into() }
}

#[test]
fn clicking_a_device_selects_it() {
    let ui = shown_window();
    ui.set_page(1);
    ui.set_perf_devices(ModelRc::new(VecModel::from(vec![device("cpu", "CPU"), device("memory", "Memory")])));
    let picked = Rc::new(RefCell::new(None));
    let p = picked.clone();
    ui.on_select_device(move |id| *p.borrow_mut() = Some(id.to_string()));
    let memory = El::find_by_accessible_label(&ui, "Memory performance").next().expect("device button");
    memory.invoke_accessible_default_action();
    assert_eq!(picked.borrow().as_deref(), Some("memory"));
    ui.set_perf_selected("memory".into());
    assert_eq!(memory.accessible_checked(), Some(true));
}

fn perf_frame(requested: &str, selected: &str) -> Frame {
    let devices = vec![
        DeviceView { id: "cpu".into(), title: "CPU".into(), ..Default::default() },
        DeviceView { id: "memory".into(), title: "Memory".into(), ..Default::default() },
    ];
    let title = if selected == "cpu" { "CPU" } else { "Memory" };
    let perf = PerfFrame { devices, requested: requested.into(), selected: selected.into(), title: title.into(), ..Default::default() };
    Frame { page: Page::Performance, perf: Some(perf), ..Default::default() }
}

#[test]
fn performance_frames_fill_the_page_and_leave_the_app_list_alone() {
    use slint::Model;
    let (ui, controls, bridge) = bridged();
    let mut b = bridge.borrow_mut();
    b.apply(&ui, frame(vec![row_view(1, "Editor", ""), row_view(2, "Browser", "")]));
    controls.set_page(Page::Performance);
    b.apply(&ui, perf_frame("cpu", "cpu"));
    assert_eq!(shown_ids(&ui), [1, 2], "the hidden Overview keeps its rows");
    assert_eq!(ui.get_perf_title(), "CPU");
    assert_eq!(ui.get_perf_devices().row_count(), 2);
    assert_eq!(ui.get_perf_selected(), "cpu");
}

#[test]
fn a_frame_for_another_page_or_device_is_dropped() {
    let (ui, controls, bridge) = bridged();
    let mut b = bridge.borrow_mut();
    controls.set_page(Page::Performance);
    controls.select_device("memory");
    b.apply(&ui, perf_frame("cpu", "cpu")); // built just before the click on Memory
    assert_eq!(ui.get_perf_title(), "", "a stale device's frame is not shown");
    b.apply(&ui, frame(vec![row_view(1, "Editor", "")])); // an Overview frame while Performance shows
    assert!(shown_ids(&ui).is_empty());
}

#[test]
fn a_vanished_device_falls_back_to_cpu_and_the_fallback_sticks() {
    let (ui, controls, bridge) = bridged();
    controls.set_page(Page::Performance);
    controls.select_device("disk:2 F:");
    bridge.borrow_mut().apply(&ui, perf_frame("disk:2 F:", "cpu"));
    assert_eq!(ui.get_perf_selected(), "cpu");
    assert_eq!(ui.get_perf_title(), "CPU");
    assert_eq!(controls.perf().0, "cpu", "a re-plugged disk doesn't snatch the page back");
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p negotium`
Expected: FAIL to compile (no `Page`, `set_page`, `select_device`, `Frame::page`, `DeviceData`, `set_perf_devices`, …).

- [ ] **Step 3: Implement**

`crates/app/src/controls.rs` — add the enum and fields (and their initial values in `Default`):
```rust
use crate::perf::Range;
use std::sync::atomic::AtomicU8;

/// Which page the window shows; the worker builds only that page's view.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Page {
    #[default]
    Overview = 0,
    Performance = 1,
}

impl Page {
    pub const ALL: [Page; 2] = [Page::Overview, Page::Performance];

    pub fn from_index(i: i32) -> Page {
        Self::ALL[(i.max(0) as usize).min(Self::ALL.len() - 1)]
    }

    pub fn index(self) -> i32 {
        self as i32
    }
}

// In `Controls`:
    page: AtomicU8,
    /// Selected Performance device id and graph range.
    perf: Mutex<(String, Range)>,
// In `Default`:
            page: AtomicU8::new(Page::Overview as u8),
            perf: Mutex::new(("cpu".to_string(), Range::Minute)),
// Methods:
    pub fn set_page(&self, p: Page) {
        self.page.store(p as u8, Ordering::Relaxed);
        self.poke();
    }
    pub fn page(&self) -> Page {
        Page::from_index(self.page.load(Ordering::Relaxed) as i32)
    }
    pub fn select_device(&self, id: &str) {
        self.perf.lock().unwrap_or_else(|e| e.into_inner()).0 = id.to_string();
        self.poke();
    }
    /// Records the device a frame fell back to; the frame is already on screen, so no wakeup.
    pub fn adopt_device(&self, id: &str) {
        self.perf.lock().unwrap_or_else(|e| e.into_inner()).0 = id.to_string();
    }
    pub fn set_range(&self, r: Range) {
        self.perf.lock().unwrap_or_else(|e| e.into_inner()).1 = r;
        self.poke();
    }
    pub fn perf(&self) -> (String, Range) {
        self.perf.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
```

`crates/app/src/view.rs` — `Frame` gains:
```rust
    /// The page this frame was built for; only that page's fields are filled.
    pub page: crate::controls::Page,
    pub perf: Option<crate::perf::PerfFrame>,
```

`crates/app/src/worker.rs` — `LiveSource` gains `info: negotium_collector::SystemInfo` (in `new`: `let collector = Collector::new(); let info = collector.system_info().clone();`), and `next_frame` becomes:
```rust
    fn next_frame(&mut self, controls: &Controls) -> Option<Frame> {
        let snap = self.collector.tick();
        for e in &snap.source_errors {
            self.log.warn(e);
        }
        let rows = self.model.update(&snap);
        self.history.push(snap.taken_at_ms, &snap.totals, &rows);
        if !controls.visible() {
            return None;
        }
        // Only the page on screen is built (spec section 8).
        match controls.page() {
            Page::Overview => {
                let metric = controls.metric();
                let order = self.ranker.order(&rows, metric, snap.taken_at_ms, controls.frozen());
                let (expanded, all_children) = controls.child_rows();
                let opts = FrameOptions { metric, elevated: self.elevated, expanded, all_children };
                Some(build_frame(&mut self.ids, &snap, &rows, &order, &self.history, &opts))
            }
            Page::Performance => {
                let (device, range) = controls.perf();
                let input = PerfInput { snap: &snap, history: &self.history, info: &self.info, device: &device, range };
                Some(Frame { page: Page::Performance, perf: Some(build_perf(&input)), ..Default::default() })
            }
        }
    }
```
(imports: `use crate::controls::{Controls, Page}; use crate::perf::{PerfInput, build_perf};`).

`crates/app/src/bridge.rs`:
- imports: `use crate::controls::{Controls, Page}; use crate::perf::PerfFrame; use crate::{DeviceData, MainWindow, MeterData, RowData, TileData};`
- `Bridge` gains `perf_devices: Rc<VecModel<DeviceData>>` and `last_perf: Option<PerfFrame>`; `install` creates them and calls `ui.set_perf_devices(ModelRc::from(b.perf_devices.clone()));`.
- rename the current `apply` to `fn apply_overview(&mut self, ui: &MainWindow, frame: Frame)` (body unchanged) and add:
```rust
    pub fn apply(&mut self, ui: &MainWindow, frame: Frame) {
        // Built for a page the user has since left: the next frame is already on its way.
        if frame.page != self.controls.page() {
            return;
        }
        match frame.page {
            Page::Overview => self.apply_overview(ui, frame),
            Page::Performance => {
                if let Some(perf) = frame.perf {
                    self.apply_perf(ui, perf);
                }
            }
        }
    }

    fn apply_perf(&mut self, ui: &MainWindow, p: PerfFrame) {
        let (device, range) = self.controls.perf();
        if p.requested != device || p.range != range {
            return; // asked for before the user picked another device or range
        }
        if p.selected != device {
            self.controls.adopt_device(&p.selected); // that device is gone: CPU stands in, and stays
        }
        sync_model(&*self.perf_devices, p.devices.iter().map(|d| DeviceData {
            id: d.id.as_str().into(), title: d.title.as_str().into(), value: d.value.as_str().into(), spark: d.spark.as_str().into(),
        }).collect());
        ui.set_perf_selected(p.selected.as_str().into());
        ui.set_perf_title(p.title.as_str().into());
        ui.set_perf_subtitle(p.subtitle.as_str().into());
        self.last_perf = Some(p);
    }
```

`crates/app/src/main.rs` — after the `on_select_metric` block:
```rust
    {
        let (controls, weak, b) = (controls.clone(), weak.clone(), bridge.clone());
        ui.on_select_page(move |i| {
            let page = negotium::controls::Page::from_index(i);
            controls.set_page(page);
            if page != negotium::controls::Page::Overview {
                // The hidden list can't report pointer-leave: release the hover freeze here.
                b.borrow_mut().set_hover(false);
            }
            if let Some(ui) = weak.upgrade() {
                ui.set_page(page.index());
            }
        });
    }
    {
        let (controls, weak) = (controls.clone(), weak.clone());
        ui.on_select_device(move |id| {
            controls.select_device(&id);
            if let Some(ui) = weak.upgrade() {
                ui.set_perf_selected(id);
            }
        });
    }
```

`crates/app/ui/rail.slint`:
```slint
import { Theme } from "theme.slint";

// One rail entry: a drawn icon (glyphs render as tofu in Segoe UI Variable) over a short label.
component RailButton inherits Rectangle {
    in property <string> label;
    in property <string> icon;
    in property <bool> selected;
    in property <bool> interactive: true;
    callback activated();
    height: 56px;
    border-radius: Theme.radius;
    background: root.selected ? Theme.accent.with-alpha(0.18) : (ta.has-hover ? Theme.card : transparent);
    border-width: fs.has-focus ? 2px : 0px;
    border-color: Theme.text;
    accessible-role: button;
    accessible-label: root.label + " page";
    accessible-checkable: true;
    accessible-checked: root.selected;
    accessible-action-default => { root.activated(); }
    fs := FocusScope {
        enabled: root.interactive;
        key-pressed(event) => {
            if (event.text == " " || event.text == Key.Return) {
                root.activated();
                return accept;
            }
            return reject;
        }
    }
    ta := TouchArea {
        enabled: root.interactive;
        clicked => { fs.focus(); root.activated(); }
    }
    if root.selected : Rectangle {
        x: 0;
        y: (parent.height - 16px) / 2;
        width: 3px;
        height: 16px;
        border-radius: 1.5px;
        background: Theme.accent;
    }
    VerticalLayout {
        alignment: center;
        spacing: 4px;
        HorizontalLayout {
            alignment: center;
            Path {
                width: 20px;
                height: 20px;
                viewbox-width: 20;
                viewbox-height: 20;
                commands: root.icon;
                stroke: root.selected ? Theme.accent : Theme.text;
                stroke-width: 1.5px;
                fill: transparent;
            }
        }
        Text { text: root.label; color: Theme.text; font-size: 10px; horizontal-alignment: center; }
    }
}

export component Rail inherits VerticalLayout {
    in property <int> page;
    in property <bool> interactive: true;
    callback select(int);
    width: 76px;
    padding: 6px;
    spacing: 4px;
    alignment: start;
    RailButton {
        label: "Overview";
        icon: "M 2 2 L 8 2 L 8 8 L 2 8 Z M 12 2 L 18 2 L 18 8 L 12 8 Z M 2 12 L 8 12 L 8 18 L 2 18 Z M 12 12 L 18 12 L 18 18 L 12 18 Z";
        selected: root.page == 0;
        interactive: root.interactive;
        activated => { root.select(0); }
    }
    RailButton {
        label: "Performance";
        icon: "M 1 11 L 5 11 L 8 4 L 12 16 L 15 9 L 19 9";
        selected: root.page == 1;
        interactive: root.interactive;
        activated => { root.select(1); }
    }
}
```

`crates/app/ui/perfpage.slint` (first version; Task 9 fills in the detail area):
```slint
import { ListView } from "std-widgets.slint";
import { Theme } from "theme.slint";

export struct DeviceData { id: string, title: string, value: string, spark: string }

component DeviceButton inherits Rectangle {
    in property <DeviceData> data;
    in property <bool> selected;
    in property <bool> interactive: true;
    callback activated();
    height: 60px;
    border-radius: Theme.radius;
    background: root.selected ? Theme.accent.with-alpha(0.18) : (ta.has-hover ? Theme.card : transparent);
    border-width: fs.has-focus ? 2px : 0px;
    border-color: Theme.text;
    accessible-role: button;
    accessible-label: root.data.title + " performance";
    accessible-checkable: true;
    accessible-checked: root.selected;
    accessible-action-default => { root.activated(); }
    fs := FocusScope {
        enabled: root.interactive;
        key-pressed(event) => {
            if (event.text == " " || event.text == Key.Return) {
                root.activated();
                return accept;
            }
            return reject;
        }
    }
    ta := TouchArea {
        enabled: root.interactive;
        clicked => { fs.focus(); root.activated(); }
    }
    HorizontalLayout {
        padding: 8px;
        spacing: 10px;
        Rectangle {
            width: 56px;
            border-width: 1px;
            border-color: Theme.card-border;
            Path {
                width: parent.width;
                height: parent.height;
                viewbox-width: 1000;
                viewbox-height: 100;
                commands: root.data.spark;
                stroke: Theme.accent;
                stroke-width: 1.5px;
                fill: transparent;
            }
        }
        VerticalLayout {
            alignment: center;
            Text { text: root.data.title; color: Theme.text; font-size: 13px; font-weight: 600; overflow: elide; }
            Text { text: root.data.value; color: Theme.muted; font-size: 11px; overflow: elide; }
        }
    }
}

export component PerformancePage inherits HorizontalLayout {
    in property <[DeviceData]> devices;
    in property <string> selected;
    in property <string> title;
    in property <string> subtitle;
    in property <bool> interactive: true;
    callback select-device(string);
    max-width: 16384px; // see MetersRow: repeaters must not cap the window width
    padding: 12px;
    spacing: 12px;
    ListView {
        width: 220px;
        for d in root.devices : DeviceButton {
            data: d;
            selected: d.id == root.selected;
            interactive: root.interactive;
            activated => { root.select-device(d.id); }
        }
    }
    VerticalLayout {
        horizontal-stretch: 1;
        alignment: start;
        spacing: 8px;
        HorizontalLayout {
            spacing: 12px;
            Text { text: root.title; color: Theme.text; font-size: 24px; font-weight: 600; }
            Text { text: root.subtitle; color: Theme.muted; font-size: 13px; vertical-alignment: bottom; overflow: elide; horizontal-stretch: 1; }
        }
    }
}
```

`crates/app/ui/main.slint` — replace the file:
```slint
import { Theme } from "theme.slint";
import { MetersRow, MeterData } from "meters.slint";
import { TopStrip, TileData } from "topstrip.slint";
import { AppList, RowData } from "applist.slint";
import { Banner, Toast, Modal } from "overlays.slint";
import { Rail } from "rail.slint";
import { PerformancePage, DeviceData } from "perfpage.slint";
export { NegotiumTray } from "tray.slint";
export { Theme, MeterData, TileData, RowData, DeviceData }

export component MainWindow inherits Window {
    title: "Negotium";
    icon: @image-url("../assets/negotium.svg");
    preferred-width: 1000px;
    preferred-height: 720px;
    min-width: 760px;
    min-height: 440px;
    default-font-family: "Segoe UI Variable Text";
    background: Theme.window-bg;

    // Page shown: 0 Overview, 1 Performance (crate::controls::Page).
    in-out property <int> page: 0;
    in property <[MeterData]> meters;
    in property <int> selected-metric;
    in property <[TileData]> tiles;
    in property <string> top-caption;
    in property <[RowData]> rows;
    in property <bool> show-admin-banner;
    in-out property <string> search-text;
    in property <string> toast;
    in-out property <string> confirm-text;
    in-out property <bool> confirm-visible;
    in-out property <string> details-text;
    in-out property <bool> details-visible;
    in property <[DeviceData]> perf-devices;
    in-out property <string> perf-selected: "cpu";
    in property <string> perf-title;
    in property <string> perf-subtitle;
    out property <bool> modal-open: confirm-visible || details-visible;
    out property <int> menu-target: list.menu-target;

    callback select-page(int);
    callback select-metric(int);
    callback open-tile(int);
    callback toggle-row(int);
    callback row-action(string, int);
    callback list-hover(bool);
    callback menu-open(bool);
    callback row-key-focus(int, bool);
    callback clear-key-focus();
    callback search-changed(string);
    callback restart-admin();
    callback dismiss-banner();
    callback confirm(bool);
    callback close-details();
    callback dismiss-toast();
    callback select-device(string);

    // has-hover only re-evaluates on the next pointer event, so release the list freeze explicitly
    // when a modal opens and re-evaluate it when the modal closes.
    // A modal disables every control beneath it; a disabled Button ignores its focus-out, so the
    // keyboard focus (and its list freeze) is dropped explicitly as well.
    changed modal-open => {
        root.list-hover(!root.modal-open && list.hovered);
        if (root.modal-open) {
            root.clear-key-focus();
        }
    }

    // Open a modal only through these: focus is parked first, while the controls beneath are
    // still enabled, so a focused row button gets a real focus-out (no stale focus ring, and
    // tabbing back onto it reports focus again).
    public function show-confirm(text: string) {
        parking.focus();
        root.confirm-text = text;
        root.confirm-visible = true;
    }
    public function show-details(text: string) {
        parking.focus();
        root.details-text = text;
        root.details-visible = true;
    }
    parking := FocusScope {
        width: 0px;
        height: 0px;
        focus-on-click: false;
        focus-on-tab-navigation: false;
    }

    HorizontalLayout {
        Rail {
            page: root.page;
            interactive: !root.modal-open;
            select(p) => { root.select-page(p); }
        }
        // Pages are stacked, not laid out (an invisible element still takes layout space): a hidden
        // page keeps its state (scroll position, search text, selection) and gets no input.
        Rectangle {
            horizontal-stretch: 1;
            Rectangle {
                x: 0;
                y: 0;
                width: parent.width;
                height: parent.height;
                visible: root.page == 0;
                VerticalLayout {
                    padding: 12px;
                    spacing: 10px;
                    if root.show-admin-banner : Banner {
                        interactive: !root.modal-open;
                        restart => { root.restart-admin(); }
                        dismiss => { root.dismiss-banner(); }
                    }
                    MetersRow {
                        meters: root.meters;
                        selected: root.selected-metric;
                        interactive: !root.modal-open;
                        select(i) => { root.select-metric(i); }
                    }
                    TopStrip {
                        tiles: root.tiles;
                        caption: root.top-caption;
                        interactive: !root.modal-open;
                        // Opening a tile clears the filter, so the box must be emptied together with it.
                        open(id) => { root.search-text = ""; root.open-tile(id); }
                    }
                    list := AppList {
                        vertical-stretch: 1;
                        rows: root.rows;
                        metric: root.selected-metric;
                        search-text <=> root.search-text;
                        interactive: !root.modal-open;
                        toggle(id) => { root.toggle-row(id); }
                        action(name, id) => { root.row-action(name, id); }
                        hover(h) => { root.list-hover(h); }
                        menu-open(open) => { root.menu-open(open); }
                        row-focus(id, focused) => { root.row-key-focus(id, focused); }
                        search(text) => { root.search-changed(text); }
                    }
                    if root.toast != "" : Toast {
                        text: root.toast;
                        interactive: !root.modal-open;
                        dismiss => { root.dismiss-toast(); }
                    }
                }
            }
            Rectangle {
                x: 0;
                y: 0;
                width: parent.width;
                height: parent.height;
                visible: root.page == 1;
                PerformancePage {
                    devices: root.perf-devices;
                    selected: root.perf-selected;
                    title: root.perf-title;
                    subtitle: root.perf-subtitle;
                    interactive: !root.modal-open;
                    select-device(id) => { root.select-device(id); }
                }
            }
        }
    }
    if root.confirm-visible : Modal {
        title: "End a critical system process?";
        body: root.confirm-text;
        confirm-label: "End process";
        cancel-primary: true;
        done(ok) => { root.confirm(ok); }
    }
    if root.details-visible : Modal {
        title: "Details";
        body: root.details-text;
        confirm-label: "Close";
        show-cancel: false;
        done(ok) => { root.close-details(); }
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p negotium` (debug; the UI tests need debug info)
Expected: all pass, including every pre-existing UI test (in particular `window_opens_at_its_preferred_width`), warning-free.

- [ ] **Step 5: Look at it**

Run: `Stop-Process -Name negotium -ErrorAction SilentlyContinue; cargo run -p negotium`. Click Performance in the rail: the device list shows CPU, Memory, each disk, each connected adapter and each GPU with live values; clicking one changes the title. Click Overview: the list is where you left it (scroll position, search). Close to the tray and Exit from the tray menu.

- [ ] **Step 6: Commit**

```bash
git add crates/app
git commit -m "feat(app): navigation rail; the worker builds only the visible page"
```

---
### Task 9: Performance page — graphs, per-core grid, details, composition, ranges

**Files:**
- Create: `crates/app/ui/graph.slint`
- Modify: `crates/app/ui/perfpage.slint`, `crates/app/ui/main.slint`, `crates/app/src/bridge.rs`, `crates/app/src/main.rs`
- Test: `crates/app/tests/ui.rs`

**Interfaces:**
- Consumes: `PerfFrame`, `GraphView`, `StatView`, `SegmentView`, `Range` (Tasks 6–7); `Bridge::apply_perf` (Task 8).
- Produces:
  - Slint structs exported from `main.slint`: `GraphData { label, value, scale, line, area, line2: string }`, `StatData { label, value: string }`, `SegmentData { label: string, value: string, fraction: float }`; component `Graph` (accessible label "<label> graph").
  - `MainWindow` properties `perf-note: string`, `perf-graphs: [GraphData]`, `perf-cells: [GraphData]`, `perf-stats: [StatData]`, `perf-segments: [SegmentData]`, `perf-range: int` (in-out; 0 = 1 min, 1 = 10 min, 2 = 1 h); callback `select-range(int)`. Range buttons are labelled "1 min range", "10 min range", "1 h range" (checkable). A stat block is labelled "<label> <value>"; a composition segment "<label> <value>".
  - `Bridge::apply_perf` fills those models in place (`sync_model`).

- [ ] **Step 1: Write the failing tests** (`crates/app/tests/ui.rs`; add `GraphData, SegmentData, StatData` to the `negotium::` import and `GraphView, StatView` to the `negotium::perf` import)

```rust
fn graph_data(label: &str) -> GraphData {
    GraphData {
        label: label.into(), value: "37%".into(), scale: "100%".into(),
        line: "M 1000.0 63.0".into(), area: "M 0 100".into(), line2: "M 0 100".into(),
    }
}

#[test]
fn range_buttons_pick_and_show_the_range() {
    let ui = shown_window();
    ui.set_page(1);
    let picked = Rc::new(RefCell::new(None));
    let p = picked.clone();
    ui.on_select_range(move |i| *p.borrow_mut() = Some(i));
    let ten = El::find_by_accessible_label(&ui, "10 min range").next().expect("range button");
    assert_eq!(ten.accessible_checked(), Some(false));
    ten.invoke_accessible_default_action();
    assert_eq!(*picked.borrow(), Some(1));
    ui.set_perf_range(1);
    assert_eq!(ten.accessible_checked(), Some(true));
    let minute = El::find_by_accessible_label(&ui, "1 min range").next().unwrap();
    assert_eq!(minute.accessible_checked(), Some(false));
}

#[test]
fn the_performance_page_shows_graphs_cells_details_and_composition() {
    let ui = shown_window();
    ui.set_page(1);
    ui.set_perf_graphs(ModelRc::new(VecModel::from(vec![graph_data("% Utilization")])));
    ui.set_perf_cells(ModelRc::new(VecModel::from(vec![graph_data("CPU 0"), graph_data("CPU 1")])));
    ui.set_perf_stats(ModelRc::new(VecModel::from(vec![StatData { label: "Speed".into(), value: "3.42 GHz".into() }])));
    ui.set_perf_segments(ModelRc::new(VecModel::from(vec![
        SegmentData { label: "In use".into(), value: "6.5 GB".into(), fraction: 0.4 },
        SegmentData { label: "Free".into(), value: "2.0 GB".into(), fraction: 0.6 },
    ])));
    for label in ["% Utilization graph", "CPU 0 graph", "CPU 1 graph", "Speed 3.42 GHz", "In use 6.5 GB", "Free 2.0 GB"] {
        assert!(El::find_by_accessible_label(&ui, label).next().is_some(), "missing {label}");
    }
}

#[test]
fn a_performance_frame_fills_graphs_and_details_through_the_bridge() {
    use slint::Model;
    let (ui, controls, bridge) = bridged();
    controls.set_page(Page::Performance);
    let mut f = perf_frame("cpu", "cpu");
    let p = f.perf.as_mut().unwrap();
    p.graphs = vec![GraphView { label: "% Utilization".into(), ..Default::default() }];
    p.cells = vec![GraphView::default(); 4];
    p.stats = vec![StatView { label: "Speed".into(), value: "3.42 GHz".into() }];
    p.note = "CPU temperature needs the add-on.".into();
    bridge.borrow_mut().apply(&ui, f);
    assert_eq!(ui.get_perf_graphs().row_count(), 1);
    assert_eq!(ui.get_perf_cells().row_count(), 4);
    assert_eq!(ui.get_perf_stats().row_data(0).unwrap().value, "3.42 GHz");
    assert_eq!(ui.get_perf_note(), "CPU temperature needs the add-on.");
    assert_eq!(ui.get_perf_range(), 0);
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p negotium --test ui`
Expected: FAIL to compile (no `GraphData`, `set_perf_graphs`, `on_select_range`, …).

- [ ] **Step 3: Implement**

`crates/app/ui/graph.slint`:
```slint
import { Theme } from "theme.slint";

export struct GraphData { label: string, value: string, scale: string, line: string, area: string, line2: string }

// A time graph: grid, translucent area, line and an optional second line (write / send). The
// paths come from crate::graph for a 1000 × 100 view box, newest sample on the right edge.
export component Graph inherits Rectangle {
    in property <GraphData> data;
    border-radius: 4px;
    border-width: 1px;
    border-color: Theme.card-border;
    background: Theme.card;
    clip: true;
    accessible-role: image;
    accessible-label: root.data.label + " graph";
    for i in 3 : Rectangle {
        x: 0;
        y: root.height * (i + 1) / 4;
        width: root.width;
        height: 1px;
        background: Theme.card-border;
    }
    Path {
        x: 0;
        y: 0;
        width: root.width;
        height: root.height;
        viewbox-width: 1000;
        viewbox-height: 100;
        commands: root.data.area;
        fill: Theme.accent.with-alpha(0.16);
        stroke: transparent;
    }
    Path {
        x: 0;
        y: 0;
        width: root.width;
        height: root.height;
        viewbox-width: 1000;
        viewbox-height: 100;
        commands: root.data.line;
        fill: transparent;
        stroke: Theme.accent;
        stroke-width: 1.5px;
    }
    Path {
        x: 0;
        y: 0;
        width: root.width;
        height: root.height;
        viewbox-width: 1000;
        viewbox-height: 100;
        commands: root.data.line2;
        fill: transparent;
        stroke: Theme.muted;
        stroke-width: 1.5px;
    }
}
```

`crates/app/ui/perfpage.slint` — replace the file (DeviceButton unchanged from Task 8, repeated here so the file is complete):
```slint
import { ListView, ScrollView } from "std-widgets.slint";
import { Theme } from "theme.slint";
import { Graph, GraphData } from "graph.slint";

export struct DeviceData { id: string, title: string, value: string, spark: string }
export struct StatData { label: string, value: string }
export struct SegmentData { label: string, value: string, fraction: float }

component DeviceButton inherits Rectangle {
    in property <DeviceData> data;
    in property <bool> selected;
    in property <bool> interactive: true;
    callback activated();
    height: 60px;
    border-radius: Theme.radius;
    background: root.selected ? Theme.accent.with-alpha(0.18) : (ta.has-hover ? Theme.card : transparent);
    border-width: fs.has-focus ? 2px : 0px;
    border-color: Theme.text;
    accessible-role: button;
    accessible-label: root.data.title + " performance";
    accessible-checkable: true;
    accessible-checked: root.selected;
    accessible-action-default => { root.activated(); }
    fs := FocusScope {
        enabled: root.interactive;
        key-pressed(event) => {
            if (event.text == " " || event.text == Key.Return) {
                root.activated();
                return accept;
            }
            return reject;
        }
    }
    ta := TouchArea {
        enabled: root.interactive;
        clicked => { fs.focus(); root.activated(); }
    }
    HorizontalLayout {
        padding: 8px;
        spacing: 10px;
        Rectangle {
            width: 56px;
            border-width: 1px;
            border-color: Theme.card-border;
            Path {
                width: parent.width;
                height: parent.height;
                viewbox-width: 1000;
                viewbox-height: 100;
                commands: root.data.spark;
                stroke: Theme.accent;
                stroke-width: 1.5px;
                fill: transparent;
            }
        }
        VerticalLayout {
            alignment: center;
            Text { text: root.data.title; color: Theme.text; font-size: 13px; font-weight: 600; overflow: elide; }
            Text { text: root.data.value; color: Theme.muted; font-size: 11px; overflow: elide; }
        }
    }
}

component RangeButton inherits Rectangle {
    in property <string> label;
    in property <bool> selected;
    in property <bool> interactive: true;
    callback activated();
    width: 64px;
    height: 28px;
    border-radius: 4px;
    background: root.selected ? Theme.accent : (ta.has-hover ? Theme.card : transparent);
    border-width: fs.has-focus ? 2px : 1px;
    border-color: fs.has-focus ? Theme.text : Theme.card-border;
    accessible-role: button;
    accessible-label: root.label + " range";
    accessible-checkable: true;
    accessible-checked: root.selected;
    accessible-action-default => { root.activated(); }
    fs := FocusScope {
        enabled: root.interactive;
        key-pressed(event) => {
            if (event.text == " " || event.text == Key.Return) {
                root.activated();
                return accept;
            }
            return reject;
        }
    }
    ta := TouchArea {
        enabled: root.interactive;
        clicked => { fs.focus(); root.activated(); }
    }
    Text {
        text: root.label;
        color: root.selected ? #ffffff : Theme.text;
        font-size: 12px;
        horizontal-alignment: center;
        vertical-alignment: center;
    }
}

export component PerformancePage inherits HorizontalLayout {
    in property <[DeviceData]> devices;
    in property <string> selected;
    in property <string> title;
    in property <string> subtitle;
    in property <string> note;
    in property <[GraphData]> graphs;
    in property <[GraphData]> cells;
    in property <[StatData]> stats;
    in property <[SegmentData]> segments;
    in property <int> range;
    in property <bool> interactive: true;
    callback select-device(string);
    callback select-range(int);
    max-width: 16384px; // see MetersRow: repeaters must not cap the window width
    padding: 12px;
    spacing: 12px;

    // Composition segments shade from the full accent colour down.
    pure function segment-alpha(i: int) -> float {
        if (i == 0) { return 1.0; }
        if (i == 1) { return 0.75; }
        if (i == 2) { return 0.5; }
        if (i == 3) { return 0.3; }
        return 0.12;
    }

    ListView {
        width: 220px;
        for d in root.devices : DeviceButton {
            data: d;
            selected: d.id == root.selected;
            interactive: root.interactive;
            activated => { root.select-device(d.id); }
        }
    }
    sv := ScrollView {
        horizontal-stretch: 1;
        content-width: self.visible-width;
        content-height: detail.preferred-height;
        detail := VerticalLayout {
            width: sv.visible-width;
            alignment: start;
            spacing: 10px;
            HorizontalLayout {
                spacing: 12px;
                Text { text: root.title; color: Theme.text; font-size: 24px; font-weight: 600; }
                Text { text: root.subtitle; color: Theme.muted; font-size: 13px; vertical-alignment: bottom; overflow: elide; horizontal-stretch: 1; }
            }
            HorizontalLayout {
                spacing: 6px;
                alignment: start;
                RangeButton { label: "1 min"; selected: root.range == 0; interactive: root.interactive; activated => { root.select-range(0); } }
                RangeButton { label: "10 min"; selected: root.range == 1; interactive: root.interactive; activated => { root.select-range(1); } }
                RangeButton { label: "1 h"; selected: root.range == 2; interactive: root.interactive; activated => { root.select-range(2); } }
            }
            for g in root.graphs : VerticalLayout {
                spacing: 2px;
                HorizontalLayout {
                    Text { text: g.label; color: Theme.muted; font-size: 12px; horizontal-stretch: 1; }
                    Text { text: g.scale; color: Theme.muted; font-size: 11px; }
                }
                Graph { height: 180px; data: g; }
                Text { text: g.value; color: Theme.text; font-size: 14px; font-weight: 600; }
            }
            if root.segments.length > 0 : VerticalLayout {
                spacing: 6px;
                Text { text: "Memory composition"; color: Theme.muted; font-size: 12px; }
                HorizontalLayout {
                    height: 22px;
                    spacing: 2px;
                    max-width: 16384px;
                    for s[i] in root.segments : Rectangle {
                        horizontal-stretch: max(s.fraction, 0.0001);
                        min-width: 2px;
                        border-radius: 2px;
                        background: Theme.accent.with-alpha(root.segment-alpha(i));
                        accessible-role: text;
                        accessible-label: s.label + " " + s.value;
                    }
                }
                HorizontalLayout {
                    spacing: 14px;
                    alignment: start;
                    max-width: 16384px;
                    for s[i] in root.segments : HorizontalLayout {
                        spacing: 4px;
                        Rectangle {
                            width: 10px;
                            border-radius: 2px;
                            background: Theme.accent.with-alpha(root.segment-alpha(i));
                        }
                        Text { text: s.label + " " + s.value; color: Theme.text; font-size: 11px; }
                    }
                }
            }
            // Small multiples (per-core, per-engine), wrapped into as many columns as fit.
            cells-host := Rectangle {
                property <int> cols: max(1, floor(sv.visible-width / 120px));
                property <length> cw: (sv.visible-width - (self.cols - 1) * 6px) / self.cols;
                height: ceil(root.cells.length / self.cols) * 70px;
                for c[i] in root.cells : VerticalLayout {
                    x: mod(i, cells-host.cols) * (cells-host.cw + 6px);
                    y: floor(i / cells-host.cols) * 70px;
                    width: cells-host.cw;
                    height: 64px;
                    Text { text: c.label + "  " + c.value; color: Theme.muted; font-size: 10px; overflow: elide; }
                    Graph { data: c; vertical-stretch: 1; }
                }
            }
            stats-host := Rectangle {
                property <int> cols: max(1, floor(sv.visible-width / 180px));
                height: ceil(root.stats.length / self.cols) * 44px;
                for s[i] in root.stats : Rectangle {
                    x: mod(i, stats-host.cols) * 180px;
                    y: floor(i / stats-host.cols) * 44px;
                    width: 172px;
                    height: 40px;
                    accessible-role: text;
                    accessible-label: s.label + " " + s.value;
                    VerticalLayout {
                        Text { text: s.label; color: Theme.muted; font-size: 11px; }
                        Text { text: s.value; color: Theme.text; font-size: 14px; font-weight: 600; overflow: elide; }
                    }
                }
            }
            if root.note != "" : Text { text: root.note; color: Theme.muted; font-size: 11px; wrap: word-wrap; }
        }
    }
}
```
(If Slint reports a binding loop on `cells-host`/`stats-host` heights, it comes from `sv.visible-width` depending on the content height through the vertical scrollbar; then compute `cols` from `root.width - 220px - 3 * 12px` instead. Note it in the report.)

`crates/app/ui/main.slint`:
- imports: `import { PerformancePage, DeviceData, StatData, SegmentData } from "perfpage.slint";` and `import { GraphData } from "graph.slint";`; export line `export { Theme, MeterData, TileData, RowData, DeviceData, GraphData, StatData, SegmentData }`.
- properties after `perf-subtitle`:
```slint
    in property <string> perf-note;
    in property <[GraphData]> perf-graphs;
    in property <[GraphData]> perf-cells;
    in property <[StatData]> perf-stats;
    in property <[SegmentData]> perf-segments;
    // Graph range: 0 = 1 min, 1 = 10 min, 2 = 1 h (crate::perf::Range).
    in-out property <int> perf-range: 0;
```
- callback `callback select-range(int);`
- on the `PerformancePage` element add:
```slint
                    note: root.perf-note;
                    graphs: root.perf-graphs;
                    cells: root.perf-cells;
                    stats: root.perf-stats;
                    segments: root.perf-segments;
                    range: root.perf-range;
                    select-range(i) => { root.select-range(i); }
```

`crates/app/src/bridge.rs` — `Bridge` gains `perf_graphs`, `perf_cells` (`Rc<VecModel<GraphData>>`), `perf_stats` (`Rc<VecModel<StatData>>`), `perf_segments` (`Rc<VecModel<SegmentData>>`), created and set on the window in `install` like `perf_devices`; in `apply_perf`, after the title:
```rust
        let graph = |g: &GraphView| GraphData {
            label: g.label.as_str().into(), value: g.value.as_str().into(), scale: g.scale.as_str().into(),
            line: g.line.as_str().into(), area: g.area.as_str().into(), line2: g.line2.as_str().into(),
        };
        sync_model(&*self.perf_graphs, p.graphs.iter().map(graph).collect());
        sync_model(&*self.perf_cells, p.cells.iter().map(graph).collect());
        sync_model(&*self.perf_stats, p.stats.iter().map(|s| StatData { label: s.label.as_str().into(), value: s.value.as_str().into() }).collect());
        sync_model(&*self.perf_segments, p.segments.iter().map(|s| SegmentData {
            label: s.label.as_str().into(), value: s.value.as_str().into(), fraction: s.fraction,
        }).collect());
        ui.set_perf_note(p.note.as_str().into());
        ui.set_perf_range(p.range.index());
```
(imports: `use crate::perf::{GraphView, PerfFrame}; use crate::{DeviceData, GraphData, MainWindow, MeterData, RowData, SegmentData, StatData, TileData};`).

`crates/app/src/main.rs` — after `on_select_device`:
```rust
    {
        let (controls, weak) = (controls.clone(), weak.clone());
        ui.on_select_range(move |i| {
            let range = negotium::perf::Range::from_index(i);
            controls.set_range(range);
            if let Some(ui) = weak.upgrade() {
                ui.set_perf_range(range.index());
            }
        });
    }
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p negotium`
Expected: all pass, warning-free.

- [ ] **Step 5: Look at it**

Run the app (`Stop-Process -Name negotium -ErrorAction SilentlyContinue; cargo run -p negotium`). On each Performance page check: the graph fills from the right as time passes; 10 min and 1 h show the same recent data compressed to the right edge (never stretched); CPU shows one cell per logical processor; Memory shows the composition bar; disks show two graphs; GPU pages show engine cells, memory graphs and (on a GPU with a sensor) the temperature. Check light and dark (Windows Settings → Personalization → Colors). Stop the instance afterwards.

- [ ] **Step 6: Commit**

```bash
git add crates/app
git commit -m "feat(app): Performance page graphs, per-core and per-engine grids, details and ranges"
```

---
### Task 10: History scrub — "Top apps at HH:MM:SS"

**Files:**
- Create: `crates/app/src/scrub.rs`
- Modify: `crates/app/src/{lib,format,view,worker,bridge,main}.rs`, `crates/collector/src/collector.rs`
- Modify: `crates/app/ui/graph.slint`, `crates/app/ui/perfpage.slint`, `crates/app/ui/main.slint`
- Test: unit tests in `scrub.rs`, `format.rs`; `crates/collector/tests/collector.rs`; `crates/app/tests/ui.rs`

**Interfaces:**
- Consumes: `History::top_at` (Task 4), `graph::fraction_to_time` (Task 5), `PerfFrame { window_end_ms, span_ms, end_clock_ms, scrub_metric }` (Task 6), `Bridge::last_perf` (Task 8).
- Produces:
  - `Collector::with_epoch(started: Instant) -> Collector` (`new()` = `with_epoch(Instant::now())`): collector time is measured from a start the worker keeps across restarts.
  - `worker::SharedHistory = Arc<Mutex<History>>`; `worker::LiveSource::new(history: SharedHistory, epoch: Instant)`; `worker::spawn(controls: Arc<Controls>, history: SharedHistory, sink) -> JoinHandle<u32>`. The history (and its time base) survive a worker restart.
  - `negotium::scrub::{ScrubView { title: String, rows: Vec<(String, String)>, empty: String }, scrub(history: &History, perf: &PerfFrame, fraction: f32) -> ScrubView, clock_at(perf: &PerfFrame, t_ms: i64) -> u64, DAY_MS, MIN_GAP_MS}` (a moment before the collector started has no history).
  - `negotium::format::clock(ms_since_midnight: u64) -> String` ("13:59:30"); `view::metric_text` becomes `pub(crate)`.
  - `Bridge::{set_history(&mut self, SharedHistory), scrub(&mut self, ui: &MainWindow, fraction: f32)}`; a scrub in progress is recomputed on every Performance frame (the window moves under a still pointer).
  - Slint: exported struct `ScrubRow { name: string, value: string }`; `Graph` gains `scrubbable: bool`, `scrub-x: float` (-1 = none), `scrub-title`, `scrub-empty: string`, `scrub-rows: [ScrubRow]`, callback `scrub(float)` (pointer fraction 0..1 while hovering, -1 on leave); `MainWindow` gains `scrub-x: float` (default -1), `scrub-title`, `scrub-empty: string`, `scrub-rows: [ScrubRow]`, callback `scrub(float)`.

- [ ] **Step 1: Write the failing tests**

`format.rs` tests: add
```rust
    #[test]
    fn wall_clock_times() {
        assert_eq!(clock(50_370_000), "13:59:30");
        assert_eq!(clock(86_399_999), "23:59:59");
        assert_eq!(clock(0), "00:00:00");
    }
```

`crates/app/src/scrub.rs` tests (implementation in Step 3):
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use negotium_model::{AppKey, AppKind, AppRow};
    use negotium_snapshot::SystemTotals;

    fn app(name: &str, cpu: f32, bytes: u64) -> AppRow {
        AppRow {
            key: AppKey::Background { pid: name.len() as u32, create_time: 0 }, name: name.into(), kind: AppKind::Background,
            processes: vec![], cpu_pct: cpu, private_bytes: bytes, disk_bps: 0.0, gpu_pct: None, net_bps: None, hung_for_ms: None,
        }
    }

    /// One sample a second from 1 s to 61 s; at t the CPU leader uses t/1000 %.
    fn history() -> History {
        let mut h = History::new();
        for s in 1..=61u64 {
            h.push(s * 1_000, &SystemTotals::default(), &[app("chrome", s as f32, 40 << 20), app("code", 0.5, 1 << 20)]);
        }
        h
    }

    fn perf(end: u64, span: u64, clock: u64, metric: Metric) -> PerfFrame {
        PerfFrame { window_end_ms: end, span_ms: span, end_clock_ms: clock, scrub_metric: metric, ..Default::default() }
    }

    const TWO_PM: u64 = 14 * 3_600_000;

    #[test]
    fn names_the_moment_under_the_pointer_in_local_time() {
        let v = scrub(&history(), &perf(61_000, 60_000, TWO_PM, Metric::Cpu), 0.5);
        assert_eq!(v.title, "Top apps at 13:59:30");
        assert_eq!(v.rows, vec![("chrome".to_string(), "31.0%".to_string()), ("code".to_string(), "0.5%".to_string())]);
    }

    #[test]
    fn values_are_formatted_for_the_pages_metric() {
        let v = scrub(&history(), &perf(61_000, 60_000, TWO_PM, Metric::Memory), 1.0);
        assert_eq!(v.rows[0], ("chrome".to_string(), "40.0 MB".to_string()));
    }

    #[test]
    fn the_clock_wraps_past_midnight() {
        let p = perf(61_000, 60_000, 10_000, Metric::Cpu);
        assert_eq!(format::clock(clock_at(&p, 31_000)), "23:59:40");
    }

    #[test]
    fn an_empty_stretch_of_the_hour_says_so_instead_of_borrowing_another_moment() {
        let v = scrub(&history(), &perf(61_000, 3_600_000, TWO_PM, Metric::Cpu), 0.1);
        assert!(v.rows.is_empty());
        assert_eq!(v.empty, "No history for this moment.");
        assert!(v.title.starts_with("Top apps at 13:"), "{}", v.title);
    }

    #[test]
    fn network_pages_explain_the_missing_per_app_data() {
        let v = scrub(&history(), &perf(61_000, 60_000, TWO_PM, Metric::Network), 0.5);
        assert!(v.rows.is_empty());
        assert_eq!(v.empty, "Per-app network use isn't measured yet.");
    }
}
```

`crates/collector/tests/collector.rs`: add
```rust
#[test]
fn a_shared_epoch_carries_collector_time_across_a_rebuilt_collector() {
    let epoch = std::time::Instant::now() - Duration::from_secs(5);
    let snap = negotium_collector::Collector::with_epoch(epoch).tick();
    assert!(snap.taken_at_ms >= 5_000);
}
```

`crates/app/tests/ui.rs` (add `ScrubRow` to the `negotium::` import and `use negotium::worker::SharedHistory;`):
```rust
#[test]
fn hovering_a_graph_scrubs_and_leaving_ends_it() {
    let ui = shown_window();
    ui.set_page(1);
    ui.set_perf_graphs(ModelRc::new(VecModel::from(vec![graph_data("% Utilization")])));
    let got = Rc::new(RefCell::new(Vec::<f32>::new()));
    let g = got.clone();
    ui.on_scrub(move |f| g.borrow_mut().push(f));
    let graph = El::find_by_accessible_label(&ui, "% Utilization graph").next().expect("graph");
    let (x, y) = centre(&graph);
    move_pointer(&ui, x, y);
    slint::platform::update_timers_and_animations();
    let last = *got.borrow().last().expect("the scrub reports the pointer");
    assert!((last - 0.5).abs() < 0.05, "{last}");
    move_pointer(&ui, 5.0, 5.0);
    slint::platform::update_timers_and_animations();
    assert_eq!(got.borrow().last(), Some(&-1.0));
}

#[test]
fn the_bridge_answers_a_scrub_from_the_shared_history_and_follows_new_frames() {
    use negotium_model::{AppKey, AppKind, AppRow};
    use slint::Model;
    let (ui, controls, bridge) = bridged();
    controls.set_page(Page::Performance);
    let history: SharedHistory = Default::default();
    for s in 1..=61u64 {
        let chrome = AppRow {
            key: AppKey::Background { pid: 1, create_time: 0 }, name: "chrome".into(), kind: AppKind::Background,
            processes: vec![], cpu_pct: s as f32, private_bytes: 0, disk_bps: 0.0, gpu_pct: None, net_bps: None, hung_for_ms: None,
        };
        history.lock().unwrap().push(s * 1_000, &Default::default(), &[chrome]);
    }
    bridge.borrow_mut().set_history(history);
    let mut f = perf_frame("cpu", "cpu");
    let p = f.perf.as_mut().unwrap();
    (p.window_end_ms, p.span_ms, p.end_clock_ms) = (60_000, 60_000, 14 * 3_600_000);
    bridge.borrow_mut().apply(&ui, f.clone());
    bridge.borrow_mut().scrub(&ui, 0.5);
    assert_eq!(ui.get_scrub_title(), "Top apps at 13:59:30");
    assert_eq!(ui.get_scrub_rows().row_data(0).unwrap().name, "chrome");
    assert!((ui.get_scrub_x() - 0.5).abs() < 1e-6);
    // A second later the window has moved under the still pointer: the card follows.
    let p = f.perf.as_mut().unwrap();
    (p.window_end_ms, p.end_clock_ms) = (61_000, 14 * 3_600_000 + 1_000);
    bridge.borrow_mut().apply(&ui, f);
    assert_eq!(ui.get_scrub_title(), "Top apps at 13:59:31");
    assert_eq!(ui.get_scrub_rows().row_data(0).unwrap().value, "31.0%");
    bridge.borrow_mut().scrub(&ui, -1.0);
    assert_eq!(ui.get_scrub_x(), -1.0);
}
```
(This test needs `negotium-model` as a dev-dependency of `crates/app`: add `negotium-model.workspace = true` under `[dev-dependencies]` if `cargo test` reports it missing — it is already a normal dependency, which integration tests can use.)

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p negotium-collector --test collector; cargo test -p negotium`
Expected: FAIL to compile (no `with_epoch`, `scrub` module, `format::clock`, `SharedHistory`, `set_history`, `on_scrub`, …).

- [ ] **Step 3: Implement**

`crates/collector/src/collector.rs`: split `new` into
```rust
    pub fn new() -> Self {
        Self::with_epoch(Instant::now())
    }

    /// Collector time (`Snapshot::taken_at_ms`) counts from `started`. A restarted worker passes
    /// the same instant, so time never goes backwards for the history it keeps.
    pub fn with_epoch(started: Instant) -> Self {
        // (the former body of `new`, with `started` in place of `Instant::now()`)
    }
```

`crates/app/src/format.rs`:
```rust
/// "HH:MM:SS" for a time of day in milliseconds since midnight.
pub fn clock(ms: u64) -> String {
    let s = ms / 1000 % 86_400;
    format!("{:02}:{:02}:{:02}", s / 3_600, s / 60 % 60, s % 60)
}
```

`crates/app/src/view.rs`: `fn metric_text` → `pub(crate) fn metric_text`.

`crates/app/src/scrub.rs` (above the tests):
```rust
//! The history scrub (spec §5.2): the top apps at the moment under the pointer on a graph, from
//! the shared History. Runs on the UI thread on pointer moves: plain data, no Windows calls.
use crate::format;
use crate::graph;
use crate::perf::{MAIN_BUCKETS, PerfFrame};
use negotium_history::History;
use negotium_model::Metric;

pub const DAY_MS: u64 = 86_400_000;
/// A sample is "at" the pointer when it is this close, or within one graph column if wider.
pub const MIN_GAP_MS: u64 = 2_500;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScrubView {
    pub title: String,
    /// (app name, value) largest first.
    pub rows: Vec<(String, String)>,
    /// Shown instead of rows when there are none.
    pub empty: String,
}

pub fn scrub(history: &History, perf: &PerfFrame, fraction: f32) -> ScrubView {
    let t = graph::fraction_to_time(fraction, perf.window_end_ms, perf.span_ms);
    let gap = MIN_GAP_MS.max(perf.span_ms / MAIN_BUCKETS as u64);
    // Before the collector started there is no history, however close the first sample is.
    let found = u64::try_from(t).ok().and_then(|t| history.top_at(t, perf.scrub_metric, gap));
    let at = found.as_ref().map_or(t, |(sample_t, _)| *sample_t as i64);
    let title = format!("Top apps at {}", format::clock(clock_at(perf, at)));
    let (rows, empty) = match (&found, perf.scrub_metric) {
        (_, Metric::Network) => (Vec::new(), "Per-app network use isn't measured yet."),
        (None, _) => (Vec::new(), "No history for this moment."),
        (Some((_, apps)), metric) => (
            apps.iter().map(|(name, v)| (name.clone(), crate::view::metric_text(*v as f64, metric))).collect(),
            "No app was busy at this moment.",
        ),
    };
    ScrubView { title, rows, empty: empty.to_string() }
}

/// Local wall-clock time (ms since midnight) of collector time `t_ms`, counted back from the
/// window's end, wrapping past midnight.
pub fn clock_at(perf: &PerfFrame, t_ms: i64) -> u64 {
    let back = (perf.window_end_ms as i64 - t_ms).rem_euclid(DAY_MS as i64) as u64;
    (perf.end_clock_ms % DAY_MS + DAY_MS - back) % DAY_MS
}
```
`crates/app/src/lib.rs`: add `pub mod scrub;`.

`crates/app/src/worker.rs`:
```rust
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// The history, shared with the UI thread for the scrub (read under the lock, plain data).
pub type SharedHistory = Arc<Mutex<History>>;

pub struct LiveSource {
    collector: Collector,
    model: Model,
    ranker: Ranker,
    history: SharedHistory,
    ids: IdMap,
    elevated: bool,
    info: negotium_collector::SystemInfo,
    log: crate::log::Log,
}

impl LiveSource {
    /// `history` and `epoch` outlive a restart after a panic, so the graphs keep their past.
    pub fn new(history: SharedHistory, epoch: Instant) -> Self {
        let collector = Collector::with_epoch(epoch);
        let info = collector.system_info().clone();
        Self {
            collector,
            model: Model::new(),
            ranker: Ranker::new(),
            history,
            ids: IdMap::new(),
            elevated: negotium_actions::is_elevated(),
            info,
            log: crate::log::Log::new(crate::log::Log::default_path()),
        }
    }
}
```
Delete `impl Default for LiveSource`. In `next_frame`, take the lock once and use it for the push and the frame:
```rust
        let rows = self.model.update(&snap);
        let mut history = self.history.lock().unwrap_or_else(|e| e.into_inner());
        history.push(snap.taken_at_ms, &snap.totals, &rows);
        // …then pass `&history` where `&self.history` was used in both page arms.
```
and `spawn` becomes:
```rust
pub fn spawn(
    controls: Arc<Controls>,
    history: SharedHistory,
    sink: impl FnMut(Frame) + Send + 'static,
) -> std::thread::JoinHandle<u32> {
    let epoch = Instant::now();
    std::thread::Builder::new()
        .name("negotium-worker".into())
        .spawn(move || {
            let mut log = crate::log::Log::new(crate::log::Log::default_path());
            supervise(move || LiveSource::new(history.clone(), epoch), controls, sink, move |m: &str| log.warn(m))
        })
        .expect("spawn worker thread")
}
```

`crates/app/ui/graph.slint` — at file level, next to `GraphData`:
```slint
export struct ScrubRow { name: string, value: string }
```
and inside `Graph`, after the three `Path`s:
```slint
    // History scrub (only the large graphs take part).
    in property <bool> scrubbable: false;
    // Fraction across the graph of the scrub cursor, -1 when none.
    in property <float> scrub-x: -1;
    in property <string> scrub-title;
    in property <string> scrub-empty;
    in property <[ScrubRow]> scrub-rows;
    callback scrub(float);

    ta := TouchArea {
        enabled: root.scrubbable;
        // mouse-x follows the pointer while it hovers, so every move over the graph reports.
        changed mouse-x => {
            if (self.has-hover) {
                root.scrub(clamp(self.mouse-x / self.width, 0, 1));
            }
        }
        changed has-hover => {
            root.scrub(self.has-hover ? clamp(self.mouse-x / self.width, 0, 1) : -1);
        }
    }
    if root.scrub-x >= 0 : Rectangle {
        x: root.width * root.scrub-x;
        y: 0;
        width: 1px;
        height: root.height;
        background: Theme.text;
    }
    if root.scrub-x >= 0 && ta.has-hover : Rectangle {
        width: 210px;
        height: card.preferred-height;
        x: root.scrub-x > 0.6 ? root.width * root.scrub-x - self.width - 8px : root.width * root.scrub-x + 8px;
        y: 8px;
        border-radius: 6px;
        border-width: 1px;
        border-color: Theme.card-border;
        background: Theme.dark ? #2b2b2b : #ffffff;
        card := VerticalLayout {
            padding: 8px;
            spacing: 2px;
            Text { text: root.scrub-title; color: Theme.text; font-size: 12px; font-weight: 600; }
            for r in root.scrub-rows : HorizontalLayout {
                spacing: 8px;
                Text { text: r.name; color: Theme.text; font-size: 11px; overflow: elide; horizontal-stretch: 1; }
                Text { text: r.value; color: Theme.text; font-size: 11px; }
            }
            if root.scrub-rows.length == 0 : Text { text: root.scrub-empty; color: Theme.muted; font-size: 11px; wrap: word-wrap; }
        }
    }
```

`crates/app/ui/perfpage.slint`: import `ScrubRow` from `graph.slint`; `PerformancePage` gains
```slint
    in property <float> scrub-x: -1;
    in property <string> scrub-title;
    in property <string> scrub-empty;
    in property <[ScrubRow]> scrub-rows;
    callback scrub(float);
```
and the large graph becomes
```slint
                Graph {
                    height: 180px;
                    data: g;
                    scrubbable: true;
                    scrub-x: root.scrub-x;
                    scrub-title: root.scrub-title;
                    scrub-empty: root.scrub-empty;
                    scrub-rows: root.scrub-rows;
                    scrub(f) => { root.scrub(f); }
                }
```

`crates/app/ui/main.slint`: import `ScrubRow` from `graph.slint` and add it to the export list; add
```slint
    in property <float> scrub-x: -1;
    in property <string> scrub-title;
    in property <string> scrub-empty;
    in property <[ScrubRow]> scrub-rows;
    callback scrub(float);
```
and on `PerformancePage`: `scrub-x: root.scrub-x; scrub-title: root.scrub-title; scrub-empty: root.scrub-empty; scrub-rows: root.scrub-rows; scrub(f) => { root.scrub(f); }`.

`crates/app/src/bridge.rs` — `Bridge` gains `history: Option<SharedHistory>`, `scrub_at: Option<f32>`, `scrub_rows: Rc<VecModel<ScrubRow>>` (set on the window in `install`), plus:
```rust
    pub fn set_history(&mut self, history: SharedHistory) {
        self.history = Some(history);
    }

    /// The pointer is `fraction` of the way across a Performance graph (-1: it left).
    pub fn scrub(&mut self, ui: &MainWindow, fraction: f32) {
        self.scrub_at = (fraction.is_finite() && fraction >= 0.0).then(|| fraction.min(1.0));
        self.show_scrub(ui);
    }

    fn show_scrub(&self, ui: &MainWindow) {
        let (Some(f), Some(perf), Some(history)) = (self.scrub_at, self.last_perf.as_ref(), self.history.as_ref()) else {
            ui.set_scrub_x(-1.0);
            return;
        };
        let view = {
            let h = history.lock().unwrap_or_else(|e| e.into_inner());
            crate::scrub::scrub(&h, perf, f)
        };
        ui.set_scrub_title(view.title.into());
        ui.set_scrub_empty(view.empty.into());
        sync_model(&*self.scrub_rows, view.rows.into_iter().map(|(name, value)| ScrubRow { name: name.into(), value: value.into() }).collect());
        ui.set_scrub_x(f);
    }
```
and at the end of `apply_perf`, after `self.last_perf = Some(p);`, call `self.show_scrub(ui);`. Imports: `use crate::worker::SharedHistory;` and add `ScrubRow` to the `use crate::{…}` list.

`crates/app/src/main.rs`: before `worker::spawn`:
```rust
    let history: negotium::worker::SharedHistory = Default::default();
    bridge.borrow_mut().set_history(history.clone());
```
pass `history` as `worker::spawn(controls.clone(), history, move |frame| { … })`, and wire the scrub:
```rust
    {
        let (b, weak) = (bridge.clone(), weak.clone());
        ui.on_scrub(move |f| {
            if let Some(ui) = weak.upgrade() {
                b.borrow_mut().scrub(&ui, f);
            }
        });
    }
```

- [ ] **Step 4: Run tests**

Run: `cargo test --workspace`
Expected: all pass, warning-free.

- [ ] **Step 5: Look at it**

Run the app; on the CPU page hover the main graph: a cursor line and a card "Top apps at HH:MM:SS" with up to ten apps follow the pointer; the card flips to the left in the right part of the graph; on 1 h shortly after start the empty left part says "No history for this moment."; a network page's card explains per-app network isn't measured yet. Stop the instance afterwards.

- [ ] **Step 6: Commit**

```bash
git add crates/app crates/collector
git commit -m "feat(app): history scrub shows the top apps at the moment under the pointer"
```

---
### Task 11: Keyboard navigation through the app list (ruling R4)

**Files:**
- Create: `crates/app/src/nav.rs`
- Modify: `crates/app/src/{lib,bridge,main}.rs`, `crates/app/ui/applist.slint`, `crates/app/ui/main.slint`
- Test: unit tests in `nav.rs`; `crates/app/tests/ui.rs`

**Interfaces:**
- Consumes: `Bridge` freeze machinery (`update_frozen`, `RowLayout`, `toggle`, `focus_generation`, `row_focus`), `view::RowView`.
- Produces:
  - `negotium::nav::{NavKey { Up, Down, Home, End, PageUp, PageDown, Left, Right, Enter, Delete }, NavKey::from_text(&str) -> Option<NavKey>, NavOutcome { Move(i32), Toggle(i32), Action(&'static str, i32), Nothing }, navigate(rows: &[RowView], expanded: &HashSet<i32>, current: Option<i32>, key: NavKey) -> NavOutcome, PAGE_ROWS}`.
  - `Bridge::{set_list_focus(&mut self, bool), list_key(&mut self, NavKey) -> NavOutcome}`; free functions `bridge::list_focus(&Rc<RefCell<Bridge>>, bool)` (a focus-out is applied on the next turn of the event loop unless the list regained focus) and `bridge::handle_list_key(&Rc<RefCell<Bridge>>, &MainWindow, &str) -> bool` (Delete goes through the window's `row-action`, exactly like a click). While the list has keyboard focus it is frozen (ruling P3-7); `clear_key_focus` (a modal opened) also drops the list focus. `open_tile` scrolls the opened app into view.
  - Slint: `AppList` gains `current-row: int`, `current-id: int`, `current-actionable: bool`, `out scroll-y: length`, callbacks `list-focus(bool)`, `list-key(string) -> bool`, `public function ensure-visible(int)`; the list-level focus stop is labelled "Apps" (role list); rows report `accessible-item-selected` for the current row. `MainWindow` gains the same three `in` properties, `out property <length> list-scroll-y`, callbacks `list-focus(bool)`, `list-key(string) -> bool`, and `public function ensure-row-visible(int)`.

- [ ] **Step 1: Write the failing tests**

`crates/app/src/nav.rs` tests (implementation in Step 3):
```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: i32, parent: i32, expandable: bool, actionable: bool, exited: bool) -> RowView {
        RowView {
            id, parent, depth: if parent < 0 { 0 } else { 1 }, name: format!("r{id}"), expandable, actionable, exited,
            ..Default::default()
        }
    }

    /// A (expanded: a1, a2), B, C (exited), W (the never-actionable Windows group).
    fn rows() -> Vec<RowView> {
        vec![
            row(1, -1, true, true, false),
            row(2, 1, false, true, false),
            row(3, 1, false, true, false),
            row(4, -1, false, true, false),
            row(5, -1, false, false, true),
            row(6, -1, true, false, false),
        ]
    }

    fn go(current: i32, key: NavKey, expanded: &[i32]) -> NavOutcome {
        navigate(&rows(), &expanded.iter().copied().collect(), Some(current), key)
    }

    #[test]
    fn without_a_current_row_any_key_starts_at_the_top() {
        let none = HashSet::new();
        assert_eq!(navigate(&rows(), &none, None, NavKey::Delete), NavOutcome::Move(1));
        assert_eq!(navigate(&rows(), &none, Some(99), NavKey::Down), NavOutcome::Move(1), "the current row went away");
        assert_eq!(navigate(&[], &none, None, NavKey::Down), NavOutcome::Nothing);
    }

    #[test]
    fn arrows_home_end_and_paging_move_and_clamp() {
        assert_eq!(go(1, NavKey::Down, &[1]), NavOutcome::Move(2));
        assert_eq!(go(1, NavKey::Up, &[1]), NavOutcome::Nothing);
        assert_eq!(go(1, NavKey::End, &[1]), NavOutcome::Move(6));
        assert_eq!(go(6, NavKey::Home, &[1]), NavOutcome::Move(1));
        assert_eq!(go(1, NavKey::PageDown, &[1]), NavOutcome::Move(6));
        assert_eq!(go(6, NavKey::PageUp, &[1]), NavOutcome::Move(1));
        assert_eq!(go(6, NavKey::Down, &[1]), NavOutcome::Nothing);
    }

    #[test]
    fn right_expands_then_enters_and_left_returns_then_collapses() {
        assert_eq!(go(1, NavKey::Right, &[]), NavOutcome::Toggle(1));
        assert_eq!(go(1, NavKey::Right, &[1]), NavOutcome::Move(2));
        assert_eq!(go(2, NavKey::Left, &[1]), NavOutcome::Move(1));
        assert_eq!(go(1, NavKey::Left, &[1]), NavOutcome::Toggle(1));
        assert_eq!(go(1, NavKey::Left, &[]), NavOutcome::Nothing);
        assert_eq!(go(4, NavKey::Right, &[]), NavOutcome::Nothing);
        assert_eq!(go(1, NavKey::Enter, &[]), NavOutcome::Toggle(1));
        assert_eq!(go(4, NavKey::Enter, &[]), NavOutcome::Nothing);
    }

    #[test]
    fn delete_ends_only_rows_that_can_be_acted_on() {
        assert_eq!(go(4, NavKey::Delete, &[]), NavOutcome::Action("end-task", 4));
        assert_eq!(go(2, NavKey::Delete, &[1]), NavOutcome::Action("end-task", 2), "a single process");
        assert_eq!(go(5, NavKey::Delete, &[]), NavOutcome::Nothing, "an exited row held in place is inert");
        assert_eq!(go(6, NavKey::Delete, &[]), NavOutcome::Nothing, "the Windows group is never a target");
    }

    #[test]
    fn slint_key_texts_map_to_navigation_keys() {
        use slint::platform::Key;
        let text = |k: Key| slint::SharedString::from(k);
        assert_eq!(NavKey::from_text(&text(Key::DownArrow)), Some(NavKey::Down));
        assert_eq!(NavKey::from_text(&text(Key::PageUp)), Some(NavKey::PageUp));
        assert_eq!(NavKey::from_text(&text(Key::Return)), Some(NavKey::Enter));
        assert_eq!(NavKey::from_text(" "), Some(NavKey::Enter));
        assert_eq!(NavKey::from_text(&text(Key::Delete)), Some(NavKey::Delete));
        assert_eq!(NavKey::from_text("a"), None);
    }
}
```

`crates/app/tests/ui.rs` — add:
```rust
// ---- Plan 3a: keyboard navigation ----

type Actions = Rc<RefCell<Vec<(String, i32)>>>;

/// A shown window driven by a real Bridge, with the list wired the way main.rs wires it.
fn wired_list(rows: Vec<RowView>) -> (MainWindow, Arc<Controls>, Rc<RefCell<Bridge>>, Actions) {
    let (ui, controls, bridge) = bridged();
    bridge.borrow_mut().apply(&ui, frame(rows));
    let actions: Actions = Default::default();
    let a = actions.clone();
    ui.on_row_action(move |name, id| a.borrow_mut().push((name.to_string(), id)));
    let b = bridge.clone();
    ui.on_list_focus(move |f| negotium::bridge::list_focus(&b, f));
    let (b, weak) = (bridge.clone(), ui.as_weak());
    ui.on_list_key(move |text| weak.upgrade().is_some_and(|ui| negotium::bridge::handle_list_key(&b, &ui, &text)));
    let b = bridge.clone();
    ui.on_row_key_focus(move |id, f| negotium::bridge::row_focus(&b, id, f));
    let b = bridge.clone();
    ui.on_list_hover(move |h| b.borrow_mut().set_hover(h));
    let b = bridge.clone();
    ui.on_clear_key_focus(move || b.borrow_mut().clear_key_focus());
    ui.window().set_size(PhysicalSize::new(1000, 720));
    ui.show().unwrap();
    (ui, controls, bridge, actions)
}

/// A key, then two event-loop turns: one runs `changed` handlers, the next the zero-delay timers
/// they start (a deferred focus-out).
fn press_key(ui: &MainWindow, k: slint::platform::Key) {
    key(ui, k);
    slint::platform::update_timers_and_animations();
    slint::platform::update_timers_and_animations();
}

#[test]
fn tabbing_into_the_list_freezes_it_and_arrows_move_a_current_row_that_never_jumps() {
    use slint::platform::Key;
    let (ui, controls, bridge, actions) =
        wired_list(vec![row_view(7, "Editor", ""), row_view(9, "Browser", ""), row_view(11, "Tool", "")]);
    assert!(tab_until(&ui, || controls.frozen()), "Tab reaches the list");
    assert_eq!((ui.get_current_row(), ui.get_current_id()), (0, 7));
    press_key(&ui, Key::DownArrow);
    assert_eq!((ui.get_current_row(), ui.get_current_id()), (1, 9));
    // A new frame re-sorts and adds an app: nothing moves under the keyboard.
    bridge.borrow_mut().apply(&ui, frame(vec![row_view(12, "New", ""), row_view(11, "Tool", ""), row_view(9, "Browser", ""), row_view(7, "Editor", "")]));
    assert_eq!(shown_ids(&ui), [7, 9, 11]);
    assert_eq!((ui.get_current_row(), ui.get_current_id()), (1, 9));
    press_key(&ui, Key::Delete);
    assert_eq!(*actions.borrow(), [("end-task".to_string(), 9)], "Delete ends the current row's app");
    press_key(&ui, Key::Escape);
    assert!(!controls.frozen(), "Esc leaves the list and live ordering resumes");
    assert_eq!(shown_ids(&ui), [12, 11, 9, 7]);
}

#[test]
fn right_and_left_expand_enter_and_collapse_from_the_keyboard() {
    use slint::platform::Key;
    let mut browser = row_view(9, "Browser", "");
    browser.expandable = true;
    let child = |id: i32| RowView { id, parent: 9, depth: 1, name: format!("proc{id}"), actionable: true, ..Default::default() };
    let (ui, controls, _bridge, _) = wired_list(vec![browser, child(20), child(21), row_view(11, "Tool", "")]);
    assert!(tab_until(&ui, || controls.frozen()));
    press_key(&ui, Key::RightArrow);
    assert_eq!(shown_ids(&ui), [9, 20, 21, 11], "expanded in place");
    press_key(&ui, Key::RightArrow);
    assert_eq!(ui.get_current_id(), 20);
    press_key(&ui, Key::LeftArrow);
    assert_eq!(ui.get_current_id(), 9);
    press_key(&ui, Key::LeftArrow);
    assert_eq!(shown_ids(&ui), [9, 11]);
}

#[test]
fn a_modal_opened_from_the_keyboard_does_not_leave_the_list_frozen() {
    use slint::platform::Key;
    let (ui, controls, _bridge, _) = wired_list(vec![row_view(7, "csrss.exe", ""), row_view(9, "Browser", "")]);
    let weak = ui.as_weak();
    ui.on_row_action(move |_, _| {
        if let Some(ui) = weak.upgrade() {
            ui.invoke_show_confirm("Ending csrss.exe will crash Windows.".into());
        }
    });
    assert!(tab_until(&ui, || controls.frozen()));
    press_key(&ui, Key::Delete);
    assert!(ui.get_confirm_visible());
    ui.set_confirm_visible(false);
    slint::platform::update_timers_and_animations();
    assert!(!controls.frozen(), "closing the modal must not leave the list frozen");
}

#[test]
fn the_menu_key_on_the_list_opens_the_current_rows_menu_with_that_row_captured() {
    use slint::platform::Key;
    let (ui, controls, _bridge, actions) = wired_list(vec![row_view(7, "Editor", ""), row_view(9, "Browser", "")]);
    assert!(tab_until(&ui, || controls.frozen()));
    press_key(&ui, Key::DownArrow);
    key(&ui, Key::Menu);
    assert_eq!(ui.get_menu_target(), 9);
    key(&ui, Key::DownArrow);
    key(&ui, Key::Return);
    assert_eq!(*actions.borrow(), [("end-task".to_string(), 9)]);
}

#[test]
fn opening_a_tile_scrolls_its_app_into_view() {
    let rows: Vec<RowView> = (1..=40).map(|i| row_view(i, &format!("App{i}"), "")).collect();
    let (ui, _controls, bridge, _) = wired_list(rows);
    assert_eq!(ui.get_list_scroll_y(), 0.0);
    bridge.borrow_mut().open_tile(35);
    assert!(ui.get_list_scroll_y() < 0.0, "the list scrolled down to App35");
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p negotium`
Expected: FAIL to compile (no `nav` module, `on_list_focus`, `on_list_key`, `get_current_row`, `get_list_scroll_y`, `bridge::list_focus`, `handle_list_key`).

- [ ] **Step 3: Implement**

`crates/app/src/nav.rs` (above the tests):
```rust
//! Keyboard navigation through the app list (ruling R4): one list-level focus stop and a
//! "current row" moved by the keys. Pure: it decides; the bridge applies.
use crate::view::RowView;
use std::collections::HashSet;

pub const PAGE_ROWS: usize = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavKey {
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Left,
    Right,
    Enter,
    Delete,
}

impl NavKey {
    /// The navigation key a Slint key event's text stands for (Space acts like Enter).
    pub fn from_text(text: &str) -> Option<NavKey> {
        use slint::platform::Key;
        let is = |k: Key| text == slint::SharedString::from(k).as_str();
        let key = if is(Key::UpArrow) {
            NavKey::Up
        } else if is(Key::DownArrow) {
            NavKey::Down
        } else if is(Key::Home) {
            NavKey::Home
        } else if is(Key::End) {
            NavKey::End
        } else if is(Key::PageUp) {
            NavKey::PageUp
        } else if is(Key::PageDown) {
            NavKey::PageDown
        } else if is(Key::LeftArrow) {
            NavKey::Left
        } else if is(Key::RightArrow) {
            NavKey::Right
        } else if is(Key::Return) || text == " " {
            NavKey::Enter
        } else if is(Key::Delete) {
            NavKey::Delete
        } else {
            return None;
        };
        Some(key)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavOutcome {
    /// Make this row current.
    Move(i32),
    /// Expand or collapse this app (and make it current).
    Toggle(i32),
    /// Act on this row, exactly as its button or menu would.
    Action(&'static str, i32),
    Nothing,
}

pub fn navigate(rows: &[RowView], expanded: &HashSet<i32>, current: Option<i32>, key: NavKey) -> NavOutcome {
    let Some(first) = rows.first() else { return NavOutcome::Nothing };
    let Some(i) = current.and_then(|id| rows.iter().position(|r| r.id == id)) else {
        // No current row yet, or it went away: any key starts at the top.
        return NavOutcome::Move(first.id);
    };
    let r = &rows[i];
    let last = rows.len() - 1;
    let to = |j: usize| if j == i { NavOutcome::Nothing } else { NavOutcome::Move(rows[j].id) };
    match key {
        NavKey::Up => to(i.saturating_sub(1)),
        NavKey::Down => to((i + 1).min(last)),
        NavKey::Home => to(0),
        NavKey::End => to(last),
        NavKey::PageUp => to(i.saturating_sub(PAGE_ROWS)),
        NavKey::PageDown => to((i + PAGE_ROWS).min(last)),
        NavKey::Right if r.expandable && !expanded.contains(&r.id) => NavOutcome::Toggle(r.id),
        NavKey::Right if r.expandable => match rows.get(i + 1) {
            Some(c) if c.parent == r.id => NavOutcome::Move(c.id),
            _ => NavOutcome::Nothing,
        },
        NavKey::Left if r.depth > 0 => NavOutcome::Move(r.parent),
        NavKey::Left if r.expandable && expanded.contains(&r.id) => NavOutcome::Toggle(r.id),
        NavKey::Enter if r.expandable => NavOutcome::Toggle(r.id),
        NavKey::Delete if r.actionable && !r.exited => NavOutcome::Action("end-task", r.id),
        _ => NavOutcome::Nothing,
    }
}
```
`crates/app/src/lib.rs`: add `pub mod nav;`.

`crates/app/src/bridge.rs`:
- `Bridge` gains
```rust
    /// The keyboard-current row (by id). Kept while focus is elsewhere, so Tab back resumes there.
    current: Option<i32>,
    /// The list itself has keyboard focus: it stays frozen, like on hover (ruling P3-7).
    list_focused: bool,
    /// Bumped by every list focus change; a deferred list focus-out applies only if none came after.
    list_generation: u64,
```
  (initialised `None`, `false`, `0` in `install`).
- `update_frozen`: `let frozen = self.hover || self.menu_open || self.key_focus.is_some() || self.list_focused;`
- `clear_key_focus`: also `self.list_generation += 1; self.list_focused = false;` (before `update_frozen`).
- `push_rows`: call `self.sync_current();` at the end.
- `open_tile`: after `self.push_rows();`:
```rust
        if let (Some(i), Some(ui)) = (self.layout.rows().iter().position(|r| r.id == id), self.ui.upgrade()) {
            ui.invoke_ensure_row_visible(i as i32);
        }
```
- new methods and free functions:
```rust
    pub fn set_list_focus(&mut self, focused: bool) {
        self.list_focused = focused;
        if focused && !self.layout.rows().iter().any(|r| Some(r.id) == self.current) {
            self.current = self.layout.rows().first().map(|r| r.id);
        }
        self.update_frozen();
        self.sync_current();
    }

    /// A navigation key while the list has focus. Moves and expand/collapse happen here; an action
    /// is returned for the caller to run through the window's row-action handler.
    pub fn list_key(&mut self, key: NavKey) -> NavOutcome {
        let outcome = navigate(self.layout.rows(), &self.expanded, self.current, key);
        match outcome {
            NavOutcome::Move(id) => {
                self.current = Some(id);
                self.sync_current();
            }
            NavOutcome::Toggle(id) => {
                self.current = Some(id);
                self.toggle(id);
            }
            NavOutcome::Action(..) | NavOutcome::Nothing => {}
        }
        outcome
    }

    /// Tells the list which row is current (its index moves when rows expand or collapse) and
    /// keeps it in view while the list has focus.
    fn sync_current(&self) {
        let Some(ui) = self.ui.upgrade() else { return };
        let rows = self.layout.rows();
        let index = self.current.and_then(|id| rows.iter().position(|r| r.id == id));
        ui.set_current_row(index.map_or(-1, |i| i as i32));
        ui.set_current_id(self.current.unwrap_or(-1));
        ui.set_current_actionable(index.is_some_and(|i| rows[i].actionable && !rows[i].exited));
        if let Some(i) = index
            && self.list_focused
        {
            ui.invoke_ensure_row_visible(i as i32);
        }
    }

/// The list gained or lost keyboard focus (the UI's `list-focus`). A focus-out is applied on the
/// next turn of the event loop and dropped if the list regained focus meanwhile; a focus moving
/// from the list to a row button meanwhile keeps the list frozen through `key_focus`.
pub fn list_focus(bridge: &Rc<RefCell<Bridge>>, focused: bool) {
    let generation = {
        let mut b = bridge.borrow_mut();
        b.list_generation += 1;
        if focused {
            b.set_list_focus(true);
            return;
        }
        b.list_generation
    };
    let b = bridge.clone();
    slint::Timer::single_shot(std::time::Duration::ZERO, move || {
        let mut b = b.borrow_mut();
        if b.list_generation == generation {
            b.set_list_focus(false);
        }
    });
}

/// A key pressed while the list itself has focus (the UI's `list-key`). An action (Delete → End
/// task) runs through the window's row-action handler, like a click, after the bridge borrow is
/// released (that handler borrows it again). Returns true when it was a navigation key.
pub fn handle_list_key(bridge: &Rc<RefCell<Bridge>>, ui: &MainWindow, text: &str) -> bool {
    let Some(key) = NavKey::from_text(text) else { return false };
    let outcome = bridge.borrow_mut().list_key(key);
    if let NavOutcome::Action(name, id) = outcome {
        ui.invoke_row_action(name.into(), id);
    }
    true
}
```
  (imports: `use crate::nav::{NavKey, NavOutcome, navigate};`).

`crates/app/ui/applist.slint` — the `AppList` component becomes (structs, `Cell`, `HeaderCell`, the search box and header row unchanged; the row body inside `row-touch` unchanged):
```slint
export component AppList inherits VerticalLayout {
    in property <[RowData]> rows;
    in property <int> metric;
    in property <bool> interactive: true;
    in-out property <string> search-text;
    // Keyboard navigation (ruling P3-7): the current row's index and id, and whether it can be acted on.
    in property <int> current-row: -1;
    in property <int> current-id: -1;
    in property <bool> current-actionable;
    out property <bool> hovered: area.has-hover;
    out property <length> scroll-y: lv.content-y;
    // The row a context menu was opened on. Captured when it opens; every menu item acts on it,
    // never on whatever row data sits at that index by the time an item is chosen.
    out property <int> menu-target: -1;
    callback toggle(int);
    callback action(string, int);
    callback hover(bool);
    // True when a row's context menu opens, false once it has closed: the list stays frozen meanwhile.
    callback menu-open(bool);
    // Keyboard focus entered (true) or left (false) a control inside row `id`: the list stays
    // frozen meanwhile, like on hover, so the focused control can't move to another app.
    callback row-focus(int, bool);
    callback search(string);
    // The list itself gained (true) or lost (false) keyboard focus: frozen meanwhile, like on hover.
    callback list-focus(bool);
    // A key while the list itself has focus; true when it was a navigation key.
    callback list-key(string) -> bool;
    spacing: 6px;

    // Scrolls so row `i` is fully visible (rows are 32px tall).
    public function ensure-visible(i: int) {
        if (i < 0) {
            return;
        }
        if (i * 32px < -lv.content-y) {
            lv.content-y = -(i * 32px);
        } else if ((i + 1) * 32px > -lv.content-y + lv.visible-height) {
            lv.content-y = -((i + 1) * 32px - lv.visible-height);
        }
    }

    // The current row's menu from the keyboard (Menu key, Shift+F10) while the list has focus.
    // Same capture rule as a row's own menu: the target is fixed before the menu shows.
    function open-key-menu() {
        if (root.current-id >= 0 && root.current-actionable && root.interactive) {
            root.menu-target = root.current-id;
            root.menu-open(true);
            key-menu.show({ x: 24px, y: (root.current-row + 1) * 32px + lv.content-y });
            root.menu-open(false);
        }
    }

    LineEdit { /* unchanged */ }
    HorizontalLayout { /* header, unchanged */ }
    list-keys := FocusScope {
        vertical-stretch: 1;
        enabled: root.interactive;
        focus-on-click: false;
        accessible-role: list;
        accessible-label: "Apps";
        changed has-focus => { root.list-focus(self.has-focus); }
        key-pressed(e) => {
            // Keys bubbling up from a focused row control are not list navigation.
            if (!self.has-focus) {
                return reject;
            }
            if (e.text == Key.Escape) {
                self.clear-focus();
                return accept;
            }
            if (e.text == Key.Menu || (e.text == Key.F10 && e.modifiers.shift)) {
                root.open-key-menu();
                return accept;
            }
            return root.list-key(e.text) ? accept : reject;
        }
        key-menu := ContextMenuArea {
            // Opened only by open-key-menu(), after the capture.
            enabled: false;
            Menu {
                MenuItem { title: "End task"; enabled: root.current-actionable; activated => { root.action("end-task", root.menu-target); } }
                MenuItem { title: "End process tree"; enabled: root.current-actionable; activated => { root.action("end-tree", root.menu-target); } }
                MenuItem { title: "Efficiency mode"; enabled: root.current-actionable; activated => { root.action("efficiency", root.menu-target); } }
                MenuSeparator { }
                MenuItem { title: "Open file location"; enabled: root.current-actionable; activated => { root.action("open-location", root.menu-target); } }
                MenuItem { title: "Details"; enabled: root.current-actionable; activated => { root.action("details", root.menu-target); } }
            }
            area := TouchArea {
                enabled: root.interactive;
                changed has-hover => { root.hover(self.has-hover); }
                lv := ListView {
                    for r[i] in root.rows : Rectangle {
                        height: 32px;
                        background: r.hung != "" ? Theme.danger-bg : (row-touch.has-hover ? Theme.card : transparent);
                        border-radius: 4px;
                        // The keyboard-current row, while the list has focus.
                        border-width: (i == root.current-row && list-keys.has-focus) ? 2px : 0px;
                        border-color: Theme.text;
                        opacity: r.exited ? 0.5 : 1.0;
                        accessible-role: list-item;
                        accessible-item-selectable: true;
                        accessible-item-selected: i == root.current-row;
                        accessible-label: r.exited ? r.name + ", exited"
                            : r.name + (r.hung != "" ? ", " + r.hung : "") + ", CPU " + r.cpu + ", memory " + r.mem;
                        // (open-menu function, menu := ContextMenuArea, row-keys := FocusScope and
                        // row-touch := TouchArea with the row body: unchanged from Plan 2)
                    }
                }
            }
        }
    }
}
```
(`content-y` is the ListView's scroll offset; Slint 1.18 deprecates the older `viewport-y` name, so don't use it.)

`crates/app/ui/main.slint`:
- properties: `in property <int> current-row: -1; in property <int> current-id: -1; in property <bool> current-actionable; out property <length> list-scroll-y: list.scroll-y;`
- callbacks: `callback list-focus(bool); callback list-key(string) -> bool;`
- function: `public function ensure-row-visible(i: int) { list.ensure-visible(i); }`
- on `list := AppList`: `current-row: root.current-row; current-id: root.current-id; current-actionable: root.current-actionable; list-focus(f) => { root.list-focus(f); } list-key(k) => { return root.list-key(k); }`

`crates/app/src/main.rs` — next to the `on_row_key_focus` block:
```rust
    {
        let b = bridge.clone();
        ui.on_list_focus(move |focused| negotium::bridge::list_focus(&b, focused));
    }
    {
        let (b, weak) = (bridge.clone(), weak.clone());
        ui.on_list_key(move |text| weak.upgrade().is_some_and(|ui| negotium::bridge::handle_list_key(&b, &ui, &text)));
    }
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p negotium`
Expected: all pass — the new tests and every Plan 2 focus/freeze test (R9–R11) — warning-free.

- [ ] **Step 5: Look at it**

Run the app. Tab from the search box: a focus ring appears on the first row and the list stops re-sorting; arrows, Home/End, PageUp/PageDown move it (scrolling as needed); Right/Left expand and collapse; Delete asks to end the app (critical processes ask for confirmation); the Menu key opens that row's menu; Esc releases the list and it re-sorts again. Tab past the list to a hung row's Force close and back: the list never re-sorts in between. Turn on Narrator briefly and check it reads "Apps, list" and the current row. Stop the instance.

- [ ] **Step 6: Commit**

```bash
git add crates/app
git commit -m "feat(app): keyboard navigation through the app list; tile click scrolls to its app"
```

---

### Task 12: Measure, check and document

**Files:**
- Modify: `docs/benchmarks/README.md`
- Test: whole workspace

**Interfaces:**
- Consumes: everything above.
- Produces: README sections "Collector tick after Plan 3a" and "Manual checks: Plan 3a"; no code changes unless a check fails.

- [ ] **Step 1: Run every test and lint**

Run: `cargo test --workspace` then `cargo clippy --workspace --all-targets -- -D warnings`
Expected: all pass; no warnings.

- [ ] **Step 2: Measure the collector tick and the history size**

Run: `cargo run -p negotium-collector --example tick_breakdown --release -- --ticks 60` and `cargo run -p negotium-probe --release -- --ticks 60 --timing`
Record the `pdh` and `tick` medians and the history bytes. Compare with the Plan 1 figures in the README (pdh ≈ 6 ms, tick ≈ 22 ms median on ~680 processes). If `pdh` grew by more than 3 ms or `tick` by more than 4 ms, say so in the report with the numbers (the human partner decides; spec §8 targets are revisited against the baseline).

- [ ] **Step 3: Add the README sections**

Append to `docs/benchmarks/README.md`:
```markdown
## Collector tick after Plan 3a

Same machine and commands as above (`tick_breakdown --ticks 60`, `negotium-probe --ticks 60 --timing`).

| Column | Plan 1 median (ms) | Plan 3a median (ms) |
|---|---|---|
| pdh | 6.11 | <measured> |
| tick | 22.35 | <measured> |
| history after 60 ticks (bytes, probe) | — | <measured> |

Plan 3a adds 9 PDH counters (CPU speed, memory lists, disk response, per-adapter send/receive, GPU adapter memory), one interface-table read every 10 s, and one D3DKMT temperature query per GPU per tick.

## Manual checks: Plan 3a

1. Performance → each GPU: the discrete GPU shows a temperature that tracks a load (e.g. a game or `dxdiag` 3D test); an integrated GPU or one without a sensor shows "—". The Overview GPU meter shows the busiest GPU's temperature.
2. On a Hyper-V host (Default Switch present): download a large file; the Overview network meter matches the physical adapter's page, not double.
3. Performance → CPU: one cell per logical processor in Task Manager's order; Speed within a few percent of Task Manager's.
4. Performance → Memory: composition matches Task Manager's In use / Modified / Standby / Free within rounding.
5. 1 min / 10 min / 1 h: right after start the 1 h graph only fills its right edge; scrubbing its empty part says "No history for this moment."; a scrub card follows the pointer and shows the right time.
6. Keyboard: Tab into the list, arrows, Right/Left, Delete, Menu key, Esc (see Task 11 Step 5); repeat with Narrator.
7. Light and dark, 100 / 150 / 200 % scale: rail icons, graphs, cards and focus rings readable.
```
Replace each `<measured>` with the number from Step 2 before committing.

- [ ] **Step 4: Commit**

```bash
git add docs/benchmarks/README.md
git commit -m "docs(bench): Plan 3a collector cost and manual checks"
```

---

## After this plan

- **Plan 3b** (`docs/superpowers/plans/2026-10-14-negotium-startup-settings.md`) adds the Startup and Settings pages, the refresh-rate / theme / always-on-top / tray settings, and restart-as-admin preserving the page; it extends `controls::Page` and the rail built here.
- **Human partner:** run Task 12's manual checks (GPU temperature on the RTX-class GPU, Hyper-V network, Narrator).
- **Plan 4** unchanged: no-prompt admin task, Ctrl+Shift+Esc redirect, installer, CPU-temperature add-on (the CPU page's "Temperature" stat and note are where it lands), per-app network via SRUM (the network scrub and the Network column), app icons.
- **Deferred minors deliberately left out** (from the Plan 1 and Plan 2 follow-ups; none touches code this plan rewrites):
  - Plan 1 collector: JSON round-trip coverage of optional maps; `ImageName` bounds/alignment asserts and `offset_of!` layout tests; unbounded read retry; 1024-unit image path buffer; PID-reuse race in identity query; `opens()` counts misses; no RAII handle in identity; no packaged/denied-PID identity tests; cloaked/tool windows not filtered; one SAFETY comment over two blocks in `read_array`; PDH open failure permanent for the session; identity clone per process per tick; `taken_at_ms` stamped before reads; transient process-read failure evicts the identity cache; invalid UTF-16 instance names become "".
  - Plan 1 model: disk-rate clamp/NaN branches untested; depth-64 cap and self-parent untested; console apps split under conhost; `strip_exe` misses ".Exe"; `Model::update` O(rows × processes) name scan; host-root guards untested; R5 (metric change while frozen) test and stale ranking doc; frozen-path `Vec::contains`; hung changes bypass the 2 s cap; HungTracker keyed by pid only; empty row set resorts every call.
  - Plan 1 bench/probe: arg-parsing rough edges, BufWriter flush, upper-median, probe warm-up, `CoInitializeEx`, hard-coded System32, and the "triage before merge" bench Drop terminating by PID (still open — flag to the human partner before the next elevated bench run).
  - Plan 2: ghost-window decision not unit-testable; version info read per process not per path; packaged-root children keyed Package; sticky-identity gaps and their tests; exit-time check via FILETIME; AccessDenied on a just-exited process; `is_critical` ".Exe"; efficiency half-applied / Off forces Normal / UWP close / first-target access-denied; details buffers and tests; idle-network and index-based sparkline (fixed here); child rows have no bar; `visible_rows` clones; thin view tests; worker restart backoff tests, sink outside `catch_unwind`, IdMap restart, no healthy-period backoff reset; UI header not inset for the scrollbar; tile width proportionality and tiny tiles; modal wheel scroll and uncapped Details body; modal FocusScope sibling and focus restoration after a modal; worker not joined on exit; two Bridge access paths; stale has-hover after close-to-tray; duplicated max-width magic number; concurrent action results overwrite the toast; Details labels row with one PID; end-tree critical gate uses the row flag; chrome timer uncapped; Mica coupled to the dark-mode call (Plan 3b touches this); first-frame flash; bench frame-stats rough edges; no Esc test on a show-confirm modal.

## Self-review (done while writing)

- **Spec coverage.** §5 rail → Task 8 (Startup and Settings entries: Plan 3b). §5.2: one page per CPU and Memory → Task 6, per disk / adapter / GPU → Task 7; ranges 1 min · 10 min · 1 h → Tasks 6 and 9; CPU large graph, per-core grid, speed, temperature ("—" + add-on note, §7), uptime, process/thread/handle counts → Task 6; Memory composition (in use, compressed, modified, standby, free), committed, speed → Tasks 1 and 6; Disk active time, read/write, response time → Tasks 2 and 7; GPU per-engine, dedicated/shared memory, temperature → Tasks 3 and 7; Network send/receive, link speed, IP addresses → Tasks 2 and 7; history scrub → Task 10. §5.1 GPU temperature on the meter → Task 3. §4.4 history (totals, per-core, memory, disk, GPU, network, temperatures, top 10 apps per metric, ≈2 MB) → Task 4. §8 only the visible page built → Task 8; no allocation growth → Task 4. Carries: per-core index → Task 1; Hyper-V double count → Task 2; per-device series and time-based capacity → Task 4; R4 keyboard rows → Task 11; R9–R11 freeze rules → Task 11 tests plus the untouched Plan 2 tests.
- **Placeholder scan.** No TBD/TODO/"similar to"; the only fill-ins are Task 12's measured numbers, taken in its Step 2.
- **Type consistency.** Checked across tasks: `SystemTotals::{per_core_pct: Vec<Option<f32>>, cpu_mhz, uptime_s, local_time_ms, adapters, gpus}`, `MemoryTotals::{commit_total, commit_limit, lists, compressed}`, `Series::*`, `History::{new, push -> bool, points, top_at(t, metric, gap)}`, `graph::{shape, fraction_to_time -> i64, nice_max, EMPTY_PATH}`, `perf::{Range, Device, PerfFrame, PerfInput, build_perf}`, `controls::{Page, select_device, adopt_device, set_range, perf}`, `worker::SharedHistory`, `scrub::{scrub, clock_at(i64)}`, `nav::{NavKey, NavOutcome, navigate}`, `bridge::{list_focus, handle_list_key}`, Slint `perf-*`, `scrub-*`, `current-*`, `list-*` names.
- **Review Focus.** Each line has its test in the owning task (Tasks 1, 2, 4, 5, 7, 10, 11).
