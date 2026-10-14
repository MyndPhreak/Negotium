# Negotium — Design Spec

**Date:** 2026-10-14
**Status:** Draft for review
**Owner:** mynd

## 1. Purpose

Negotium is a replacement for the Windows 11 Task Manager that is faster, easier to read, and looks like a modern Windows 11 app.

**Audience:** the author and people they share it with (GitHub releases). Not a commercial product.

**Primary jobs, in priority order:**

1. "Why is my PC slow right now?" — find what is hogging CPU, memory, disk, GPU or network and stop it.
2. "Something froze." — kill a hung app quickly and reliably.
3. "What is my system doing overall?" — totals, graphs, temperatures, and recent history.

**Success criteria:**

- Measurably faster than Task Manager on the same machine (see §8 targets; baseline measured first).
- A first-time user can identify the top resource hog within 2 seconds of the window appearing.
- A hung app can be force-closed in at most two clicks.

## 2. Constraints and non-goals

- **Windows 11 x64 only.**
- **Taskmgr.exe is not modified.** It is Microsoft-signed and protected by Windows; Negotium is a separate app that can optionally be launched by Ctrl+Shift+Esc.
- Negotium cannot auto-elevate as Task Manager does (`autoElevate` is reserved for Microsoft binaries). Elevation is on demand or via an opt-in scheduled task (§6).

**Non-goals for v1:** Services page, Users page, Details page (process details live in the app list's expand/Details action instead), startup impact ratings, persisted (on-disk) history, remote machines, plugins.

## 3. Background: what we learned from Taskmgr.exe 10.0.26100.9549

Reverse engineered with REA 6.4.0 + Ghidra 12.1.4. Evidence IDs refer to the REA ledger from that session.

- **UI stack:** WinUI 2 / `Windows.UI.Xaml` pages (`TaskManagerApp::*Page`) hosting the legacy DirectUI (`DUI70.dll`) grid via `DataGridNativeHWNDHost`. Two UI frameworks in one window.
- **Data engine:** `Wdc*Monitor` classes (`WdcProcessMonitor`, `WdcCpuMonitor`, `WdcMemoryMonitor`, `WdcDiskMonitor`, `WdcGpuMonitor`, `WdcNetworkMonitor`, …).
- **Data sources:**
  - Processes: `NtQuerySystemInformation`, `NtQueryInformationProcess`, `GetProcessTimes`.
  - CPU: kernel performance counter set **"Processor Information"** via `PcwCreateQuery`/`PcwAddQueryItem` (`WdcCpuMonitor::InitializePCWQuery`, `WdcProcessMonitor::InitializePCWQuery`).
  - Disk: counter set **"FileSystem Disk Activity"** via PCW (`rumdatasrcs.cpp`), plus PDH.
  - GPU: PDH `\GPU Engine(*)\Utilization Percentage`, `\GPU Engine(*)\Running Time`, `\GPU Process Memory(*)\…`; DXCore adapters; `D3DKMTQueryAdapterInfo`.
  - Per-process network: SRUM real-time stats (`srumapi.dll!SruRegisterRealTimeStats`, loaded dynamically; undocumented).
  - Network adapters: `GetIfEntry2`, `GetAdaptersAddresses`, `NsiGetAllParameters`.
  - Services: `EnumServicesStatusExW`, `SubscribeServiceChangeNotifications`.
  - Change notifications: WNF (`RtlSubscribeWnfStateChangeNotification`).
- **Actions:** `TerminateProcess`; Efficiency mode via `SetProcessInformation` (`DataMonitors::EcoMode`); dumps via `MiniDumpWriteDump`; wait chains via `GetThreadWaitChain`.
- **Hypothesis to verify with the benchmark (§8), not assumed:** data collection and UI refresh are coupled, and the hosted DirectUI grid adds layout cost.

## 4. Architecture

```
Collector thread ──snapshot──► Model ──diff──► UI thread (Slint)
                                  │                 │
                                  ▼                 ▼
                            History ring       Actions (on worker)
```

**Rule:** the UI thread never calls a Windows system API. All collection and all actions run on worker threads; the UI receives immutable data and sends action requests.

**Stack:** Rust (stable, MSVC toolchain) + Slint for UI (Skia or FemtoVG renderer, Fluent-style theme). `windows` crate for Win32/NT APIs.

### 4.1 Units

| Unit | Responsibility | Depends on | Tested by |
|---|---|---|---|
| `collector` | Calls Windows APIs once per tick; produces a `Snapshot` (plain data, no handles) | Windows APIs | Integration tests on real Windows |
| `model` | Groups processes into apps, computes rates/deltas, detects hung apps, ranks by metric, produces UI diffs | `Snapshot` only | Unit tests with recorded snapshots |
| `history` | Fixed-size ring buffer of totals + top apps per metric | `model` output | Unit tests |
| `actions` | End task / tree, force close, efficiency mode, startup toggle, elevate, open file location | Windows APIs | Integration tests on throwaway processes |
| `ui` | Slint screens; reads view models, emits action requests | `model`, `history` view models | Slint test backend + visual checklist |
| `sensors-cpu` | Optional CPU temperature via the PawnIO driver | PawnIO (if installed) | Integration test gated on driver presence |
| `bench` | Separate binary measuring Task Manager and Negotium identically | Windows APIs | Run manually / before releases |

Each unit lives in its own crate in a Cargo workspace so boundaries are enforced by the compiler.

### 4.2 Collector

- **Tick:** default 1 s; user choices 0.5 / 1 / 2 / 5 s. When the window is hidden (tray), tick is 2 s and only totals + top-N are kept.
- **Per tick:**
  - One `NtQuerySystemInformation(SystemProcessInformation)` call for all processes: CPU times, working set, private bytes, handle/thread counts, I/O counters, parent PID, create time. No per-process `OpenProcess` on the hot path.
  - CPU total and per-core from the **"Processor Information"** counter set (same source as Task Manager), consumed through the documented PerfLib V2 consumer API (`PerfOpenQueryHandle` / `PerfAddCounters` / `PerfQueryCounterData`).
  - Memory totals from `GlobalMemoryStatusEx` and performance information; memory composition (standby/modified/free) from `NtQuerySystemInformation(SystemMemoryListInformation)` where permitted.
  - Disk: per-physical-disk active time / throughput / response time from the PerfLib V2 disk counter sets; per-process disk rate from I/O counter deltas.
  - GPU: per-engine and per-process utilization from the `GPU Engine` / `GPU Process Memory` counter sets; adapter temperature from `D3DKMTQueryAdapterInfo` (adapter perf data).
  - Network adapters: `GetIfTable2` deltas.
  - Per-app network: behind a `NetworkAttribution` trait. **v1 implementation: SRUM real-time stats** (what Task Manager uses). Because it is undocumented, it is isolated and feature-detected; if unavailable the per-app Network column shows "—" and the system Network meter still works.
- **Expensive, cached-per-process data** (executable path, icon, publisher, command line, window titles) is fetched once per new PID on a low-priority worker and cached by (PID, create time).
- **Hung detection:** each tick, enumerate top-level visible windows (`EnumWindows`) and test `IsHungAppWindow`; record first-seen-hung time per window owner.

### 4.3 Model

- **App grouping**, applied in order:
  1. Packaged apps grouped by package family name.
  2. A process owning a visible top-level window is an app root; descendants join it (parent PID, validated by parent create time ≤ child create time to defeat PID reuse).
  3. Known multi-process hosts (Chromium browsers, Electron apps) collapse to their top browser/host process.
  4. Services and system processes → "Windows" group. Remaining processes → "Background", one row each.
- **Rates:** CPU % from CPU-time deltas ÷ (elapsed × logical processors); disk/network bytes/s from counter deltas.
- **Ranking:** by the selected metric; re-sort at most every 2 s; frozen while the pointer is over the list.
- **Output:** a diff (added/removed/changed rows + changed totals) consumed by the UI.

### 4.4 History

- In memory only. 1 hour at 1 s: system totals (CPU, per-core, memory, disk, GPU, network, temps) plus the top 10 apps per metric per sample. Budget ≈ 2 MB.
- Survives window close because the app keeps running in the tray (§5.5).

## 5. User interface

**Style:** native Windows 11 (Fluent): Mica backdrop (`DwmSetWindowAttribute` with `DWMWA_SYSTEMBACKDROP_TYPE`), the user's accent color, Segoe UI Variable, translucent tiles, rounded corners. Light and dark themes following the system setting by default.

**Navigation:** compact left rail: Overview · Performance · Startup · Settings.

### 5.1 Overview (main screen, stacked layout)

1. **Meters row:** CPU %, Memory (% and GB), Disk (busiest disk active time), GPU (busiest engine), Network (total throughput). Each has a 60 s sparkline. Clicking a meter sets the **selected metric**. GPU (and CPU when the add-on is installed) shows temperature.
2. **Top-5 map strip:** the five largest apps for the selected metric, widths proportional to value; name + value per tile; ⚠ on frozen apps. Clicking a tile scrolls to and expands that app in the list.
3. **App list:** columns Name (process count) · CPU · Memory · Disk · GPU · Network. Selected metric column highlighted, sorted, with inline bars. Search box filters by app or process name. Virtualized: only visible rows are built.
   - Expand an app → its processes.
   - Row actions (hover buttons + context menu): End task · End process tree · Efficiency mode · Open file location · Details (PID, command line, user, priority).
   - **Frozen apps** pin to the top in a red row with a live "Not responding · Ns" timer and a **Force close** button: posts `WM_CLOSE`, waits 3 s, then `TerminateProcess`.

### 5.2 Performance

One page per CPU, Memory, each disk, each GPU, each network adapter, with range buttons 1 min · 10 min · 1 h.

- CPU: large graph, per-core grid, speed, temperature, uptime, process/thread/handle counts.
- Memory: composition bar (in use, compressed, modified, standby, free), committed, speed.
- Disk: active time, read/write, response time.
- GPU: per-engine utilization, dedicated/shared memory, temperature.
- Network: send/receive, link speed, IP addresses.

**History scrub:** hovering a graph point shows "Top apps at HH:MM:SS" for that metric from the history buffer.

### 5.3 Startup

Lists entries from HKCU/HKLM `Run` keys, the per-user and all-users Startup folders, and packaged-app startup tasks. Enable/disable writes the same `StartupApproved` registry values Task Manager uses, so both tools stay in sync. Shows name, publisher, status, location. No impact rating in v1.

### 5.4 Settings

Refresh rate · Theme (system/light/dark) · Always on top · Keep running in tray when closed (default on) · Admin mode (normal / no-prompt) · Open on Ctrl+Shift+Esc · CPU temperature add-on (install/remove).

### 5.5 Tray behavior

Closing the window hides it to the tray; the collector continues at 2 s. Reopening is instant (window is hidden, not destroyed). Tray menu: Open · Exit.

## 6. Elevation and system integration

- **Default:** runs as the current user. Processes that need admin to read show "—" with a tooltip, and a single "Restart as admin" banner appears. Restart as admin relaunches with `ShellExecuteEx` + `runas`, preserving the selected page.
- **No-prompt admin (opt-in in Settings):** creates a Task Scheduler task with "Run with highest privileges" and launches it via the task. Settings explains the trade-off: anything that can start the task gets an elevated Negotium without a prompt.
- **Ctrl+Shift+Esc (opt-in):** sets `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options\taskmgr.exe\Debugger` to Negotium's path (the Process Explorer mechanism). Requires admin once. Negotium ignores the `taskmgr.exe` argument it receives and offers "Open Windows Task Manager" (which temporarily bypasses the redirect). Removed on uninstall and when the setting is turned off.
- **Critical processes:** ending `csrss`, `wininit`, `winlogon`, `lsass`, `smss`, `services`, `System` requires a confirmation that states the consequence.

## 7. Error handling

- Per-process access failures are data ("—" + reason), never dialogs.
- Action failures show an inline notification with a plain-English reason mapped from the Win32/NTSTATUS error (e.g. access denied, protected process, already exited).
- Each collector source is independent: one failing source marks its meter "Unavailable" and the rest continue. A panic in the collector thread is caught, logged, and the thread restarts with backoff.
- CPU temperature without the add-on shows "—" with an "Install CPU temperature add-on" link.
- Logs: rolling file in `%LOCALAPPDATA%\Negotium\logs`, warnings and errors only by default.

## 8. Performance targets and benchmark

**First step of implementation:** build `bench` and record Task Manager's baseline on the author's machine. Targets below are goals for a typical system with ~300 processes; they are revisited against the baseline before being treated as pass/fail.

| Metric | Target |
|---|---|
| Cold start → first frame | ≤ 250 ms |
| Tray → visible | ≤ 50 ms |
| Own CPU, window visible, 1 s tick | ≤ 0.5 % of one core |
| Own CPU, tray, 2 s tick | ≤ 0.2 % of one core |
| Working set | ≤ 60 MB |
| Collector tick (median) | ≤ 5 ms |
| UI frames during refresh/scroll | none > 16 ms |

`bench` measures both apps identically: process launch → first visible window (`WinEventHook` on `EVENT_OBJECT_SHOW`); CPU and working set sampled over 60 s; frame pacing from the app's own frame timer for Negotium and from DWM frame statistics for both.

## 9. Testing

- `model`: unit tests over recorded `Snapshot` fixtures — grouping rules, PID reuse, rate math, hung timers, ranking stability.
- `collector`: integration tests on Windows — a spawned test process burning one core is reported within ±5 percentage points of the "Processor Information" counter; memory within ±5 %.
- `actions`: integration tests on throwaway child processes — end, end tree, force close (hung test window), efficiency mode verified with `GetProcessInformation`.
- `ui`: Slint testing backend for selected-metric switching, tile → list navigation, row actions; a short manual visual checklist per release (light/dark, 100/150/200 % scale).
- `bench`: run before each release; regressions against the previous release block it.

## 10. Packaging and distribution

- MSI built with `cargo-wix`, published on GitHub Releases.
- Installer: per-machine install to `Program Files\Negotium`; Start menu shortcut; optional "Open on Ctrl+Shift+Esc" and "No-prompt admin" checkboxes (both off by default); uninstall removes the IFEO value and scheduled task.
- **Signing:** unsigned for the first releases (SmartScreen warning expected). Azure Trusted Signing to be added later.
- **CPU temperature add-on:** downloads and runs the official PawnIO installer from its publisher on explicit user action only; Negotium never bundles a kernel driver.

## 11. Risks and open items

| Risk | Mitigation |
|---|---|
| SRUM real-time stats is undocumented and may change | Isolated behind `NetworkAttribution`; feature-detected; graceful "—" |
| Slint lacks a built-in treemap and some Fluent details | Treemap strip is a simple 1-row proportional layout (no squarified algorithm needed for 5 tiles); Fluent styling done once in a shared theme |
| Accessibility in Slint is less mature than WinUI | Use Slint's accessibility properties on all interactive elements; test with Narrator before first public release |
| Matching Task Manager's numbers exactly | Use the same counter sets (§3); collector tests compare against them |
| PerfLib V2 / PCW behavior differs across Windows builds | Feature-detect counter sets at startup; per-source "Unavailable" state |
