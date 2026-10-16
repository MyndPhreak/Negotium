# Benchmarks

Machine: AMD Ryzen 9 7950X3D 16-Core Processor (16 cores, 32 logical processors), 95.2 GB RAM, Windows 11 Pro (NT 10.0.26300).

## Collector tick (negotium-probe --release --ticks 60 --timing)

```
collector tick over 60 ticks: first 36.59 ms, median 18.73 ms, p95 24.75 ms, max 36.59 ms; history 41783 bytes
```

A second run on the same machine: first 48.20 ms, median 19.09 ms, p95 26.51 ms, max 48.20 ms (history 40981 bytes).
Measured with about 680 processes running and a normal desktop workload (system CPU 20-30 %).

Target (spec §8): median ≤ 5 ms. **Not met: median is about 19 ms.**

### Breakdown by source

Command (release build, defaults: 60 iterations, 1000 ms apart, same cadence as the headline run; overridable with `--ticks N --interval-ms MS`):

```
cargo run -p negotium-collector --example tick_breakdown --release
```

Source: `crates/collector/examples/tick_breakdown.rs`. Each iteration times, in this order, one raw `NtQuerySystemInformation(5, ...)` call into a reused buffer (`nt_call`), `ProcessReader::read()` (`process_read`, call plus parse), `IdentityCache::get` for every process from that read on a persistent cache (`identity`), `PdhSources::read()` (`pdh`), `scan_windows()` (`windows`), `memory_totals()` (`memory`), and a whole `Collector::tick()` on a separate collector (`tick`). One run, 674 processes, times in ms:

| Column | First | Median | p95 | Max |
| --- | --- | --- | --- | --- |
| nt_call | 15.075 | 14.958 | 22.242 | 31.640 |
| process_read | 25.901 | 14.488 | 19.799 | 28.830 |
| identity | 11.337 | 0.127 | 0.630 | 11.337 |
| pdh | 4.744 | 6.109 | 7.590 | 26.880 |
| windows | 1.347 | 1.233 | 2.127 | 3.213 |
| memory | 0.003 | 0.003 | 0.004 | 0.005 |
| tick | 41.285 | 22.346 | 30.550 | 41.285 |

What the numbers support:

- The raw `NtQuerySystemInformation` call alone costs about 15 ms median, which is essentially all of `process_read` (14.5 ms median). Parse time derived as `process_read - nt_call` is -0.47 ms at the median, i.e. indistinguishable from zero within run-to-run noise. The two columns are separate calls, so this is a derived estimate, not a direct measurement; it says parsing is small relative to the call, not that it is exactly zero.
- PDH is the second-largest source at about 6 ms median; the window scan is about 1.2 ms; steady-state identity lookups are about 0.13 ms; memory totals are negligible.
- The first-iteration `process_read` (25.9 ms) and `identity` (11.3 ms) are slower: the first `process_read` is the reader's first call, and the first `identity` fills the cache with an `OpenProcess` query per process. The first-tick `Collector::tick()` (41.3 ms) includes both.
- The sum of the medians of `process_read`, `identity`, `pdh` and `windows` (about 22 ms) matches the `tick` median (22.3 ms) in this run. This run's `tick` median (22.3 ms) is higher than the headline probe runs (18.7 and 19.1 ms); absolute values vary between runs (the two probe runs also differ from each other by 0.4 ms). The ordering (process list, then PDH, then everything else) is the same.

The target stays unchanged; the process-list kernel call is the dominant cost on this machine and is the first place to look for savings.

## CPU accuracy

`cargo test -p negotium-collector --test accuracy`: one busy thread reports within ±35 % of 100 / logical_cpus percent. Tolerance unchanged.

## Task Manager baseline (Taskmgr.exe 10.0.26100.9549)

Raw data (not yet recorded): `2026-10-14-taskmgr-baseline.json`

The measurement needs an elevated terminal (Task Manager auto-elevates and UIPI blocks probing higher-integrity windows), so it has not been run yet. Close Task Manager, then run from the repository root in an elevated terminal and fill the table from the JSON:

```
cargo run -p negotium-bench --release -- taskmgr --runs 5 --sample-secs 60 --out docs/benchmarks/2026-10-14-taskmgr-baseline.json
```

The first 5 s after the window appears (`--warmup-secs`, default 5) are excluded from the CPU, working-set and responsiveness samples so the launch burst does not skew them; the `--sample-secs` window starts after the warm-up.

If interrupted with Ctrl+C during sampling, close Task Manager before re-running.

| Metric | Task Manager | Negotium target (spec §8) |
|---|---|---|
| Launch → first window (run 1 is cold, runs 2–5 warm; median of 5) | pending (run elevated) | ≤ 250 ms |
| Launch → first window, cold (`startup_ms[0]`) | pending (run elevated) | ≤ 250 ms |
| CPU while visible, mean / max | pending (run elevated) | ≤ 0.5 % of one core (= 0.0156 % of total on 32 logical processors) |
| Private working set, max | pending (run elevated) | ≤ 60 MB |
| UI responsiveness (WM_NULL), median / p95 / max | pending (run elevated) | no frame > 16 ms (Plan 2 measures frames) |
| Responsiveness timeouts (> 1 s) | pending (run elevated) | 0 |

## Negotium vs Task Manager

Run both from an elevated terminal (close both apps first):

    cargo build --release -p negotium -p negotium-bench
    target\release\negotium-bench.exe taskmgr --runs 5 --sample-secs 60 --out docs\benchmarks\taskmgr.json
    target\release\negotium-bench.exe negotium --runs 5 --sample-secs 60 --out docs\benchmarks\negotium.json

| Metric | Task Manager | Negotium | Target (spec §8) |
|---|---|---|---|
| Launch → first window, run 1 (cold) | pending | pending | ≤ 250 ms |
| Launch → first window, median of 5 | pending | pending | ≤ 250 ms |
| CPU while visible, mean / max (% of total) | pending | pending | ≤ 0.5 % of one core |
| Private working set, max | pending | pending | ≤ 60 MB |
| UI responsiveness (WM_NULL) median / p95 / max | pending | pending | — |
| Render time per frame, max / frames > 16 ms (excludes first paint; whole app lifetime) | n/a | pending | none > 16 ms |

Negotium renders with winit + FemtoVG (OpenGL) by default (see "Renderer" below). FemtoVG has a rendering notifier, so a default run reports frame stats without setting `SLINT_BACKEND`. The software renderer has no notifier. It is in use when forced with `SLINT_BACKEND=winit-software` or after a recorded fallback, and then the stats file reports 0 frames, the bench prints the stats as they are, and the app logs "frame stats unavailable". The bench still reports UI responsiveness through WM_NULL. Before a run, check that `%LOCALAPPDATA%\Negotium\renderer-fallback` does not exist, or set `SLINT_BACKEND=winit-femtovg` to be sure of the GPU renderer.

## Renderer: launch → window, FemtoVG vs software

During development the default winit + FemtoVG (OpenGL) renderer sometimes took from 30 s to more than 5 minutes to show the window, leaving processes behind; it reproduced on the baseline build. A/B measurement: release build, time from launch until the main window is visible (winit's "Winit Thread Event Target" helper window ignored), 90 s cap per launch, every negotium.exe stopped before each launch, configurations interleaved. Times in ms.

| Build | Renderer | Launches | Median |
|---|---|---|---|
| baseline (bf903db) | `SLINT_BACKEND=winit-femtovg` | 843, 530, 484, 562, 514, 547 | 539 |
| baseline (bf903db) | `SLINT_BACKEND=winit-software` | 78, 124, 78, 110, 79, 109 | 94 |
| software pinned | `SLINT_BACKEND=winit-femtovg` | 1009, 576, 701, 649, 529, 529 | 613 |
| software pinned | `SLINT_BACKEND=winit-software` | 154, 138, 110, 63, 62, 80 | 95 |
| software pinned | default (no `SLINT_BACKEND`) | 93, 95, 93, 64, 110, 63 | 93 |

No launch stalled in these runs (the stall is intermittent), and software opened the window faster on every launch (about 500 ms faster at the median). Plans 1 to 3a therefore pinned winit + software.

### Renderer: the default since Plan 3b

The software renderer repaints the whole window on every resize step: softbuffer reallocates its buffer, so Slint draws a new buffer each time. That makes resizing slow on a large display (5120 × 1440), while FemtoVG resizes smoothly. Since Plan 3b Negotium therefore selects **winit + FemtoVG** by default and accepts its slower start (about 550 ms against 95 ms). A safety net covers the GPU renderer (`crates/app/src/renderer.rs`):

- **GL errors:** if OpenGL is unavailable or fails, Slint's winit backend falls back to software in-process. This also happens when FemtoVG is selected by name, because window creation retries every built-in renderer when the chosen one fails.
- **GL stalls:** a stall blocks the UI thread, so a watchdog thread is armed before the window is created. It watches two signals from the UI thread. The first is the first presented frame: the rendering notifier's first `AfterRendering`, followed by a zero-delay timer so the signal fires only once the event loop is back after the present. The second is a heartbeat from the 50 ms window-setup timer. The watchdog fires only if the UI thread shows neither a frame nor a heartbeat for 10 s (`WATCHDOG_TIMEOUT`). A window that is alive but draws nothing, for example one that is minimised at startup, keeps beating, and after 10 s the watchdog stands down. When the watchdog fires, it writes the marker file `%LOCALAPPDATA%\Negotium\renderer-fallback` (the temp directory if `LOCALAPPDATA` is unset) and relaunches the exe with the same arguments plus `--restarted --renderer-fallback` and `SLINT_BACKEND=winit-software`. It then ends the process with `TerminateProcess`, because an orderly exit runs the GL driver's DLL detach code and could hang on a wedged driver. `--restarted` makes the new instance wait for the old one's single-instance lock. `--renderer-fallback` stops the new instance from arming a watchdog of its own, so there is at most one relaunch. A first frame and the watchdog settle the outcome by compare-and-swap, so only one of them can win. The tray icon is created only once the watchdog has resolved (a frame was presented, or it stood down), so a replaced instance leaves no icon behind. Negotium removes `SLINT_BACKEND` from its own environment after choosing the renderer, so the processes it starts do not inherit it.
- **No flip-flop:** while the marker exists, every launch selects software. Only an explicit user action clears it: `renderer::clear_fallback()`, for the Settings switch of a later task. Task 1 moves the marker into settings.json as `gpu_rendering: false`. To retry the GPU renderer by hand, delete the marker file.
- **Forcing a renderer:** `SLINT_BACKEND` overrides both the default and the marker, and the watchdog never guards an explicit choice. Use `SLINT_BACKEND=winit-femtovg` for the GPU renderer or `SLINT_BACKEND=winit-software` for the software renderer.
- **Testing the net:** in a debug build, `NEGOTIUM_SIMULATE_STALL=1` blocks the UI thread before the first frame of a guarded launch. After about 10 s a software window should replace it, and the marker should exist. Delete the marker afterwards.

## Manual check: frozen windows

1. `cargo run -p negotium-collector --bin negotium-hang --release`
2. In another terminal: `cargo run -p negotium-probe --release -- --ticks 30`
3. After ~8 s the window title shows "Not Responding"; click inside it once.
4. Expected: the probe lists `negotium-hang` with `NOT RESPONDING` on every tick after that, and no `dwm` app row appears.

## Collector tick after Plan 3a

Same machine and commands as above (`tick_breakdown --ticks 60`, `negotium-probe --ticks 60 --timing`). This run saw 486 processes (Plan 1: 674), so the process-dependent columns are not strictly comparable; `pdh` does not depend on the process count. Figures are from a single run, as medians of 60 ticks.

| Column | Plan 1 median (ms) | Plan 3a median (ms) |
|---|---|---|
| pdh | 6.11 | 5.78 |
| tick | 22.35 | 13.19 |
| history after 60 ticks (bytes, probe) | 41783 | 1827389 |

Plan 3a's history preallocates one hour of columns (budget about 2 MB, spec §4.4) and does not grow per tick, so its figure is capacity and is not comparable with Plan 1's much smaller one.

The probe's own headline for the same run: tick median 15.47 ms, p95 23.26 ms (Plan 1 headline: median 18.73 ms). Neither `pdh` (+3 ms limit) nor `tick` (+4 ms limit) grew against Plan 1.

Plan 3a adds 9 PDH counters (CPU speed, memory lists, disk response, per-adapter send/receive, GPU adapter memory), one interface-table read every 10 s, and one D3DKMT temperature query per GPU per tick.

### History memory budget

The history preallocates 3601 one-second samples (one hour) for everything it keeps, so its size depends on the machine's devices, not on how long Negotium runs. Figures are from `History::approx_bytes` after one sample:

| Part | Bytes |
|---|---|
| Top-10 app lists, 5 metrics (3601 × 5 × 64 B) | 1 152 320 (≈1.15 MB) |
| Sample times (3601 × 8 B) | 28 808 |
| One percent column (u8 per sample) / one value column (f32 per sample) | 3 601 / 14 404 (≈14.4 KB) |
| Fixed system series (CPU, memory used, committed, busiest disk, GPU, network total) plus app names | ≈54 000 |
| **No devices at all** | **1 235 479** |
| Per logical processor (one percent column) | ≈3 900 |
| Per disk (active time %, read, write, response time) | ≈47 200 |
| Per network adapter (send, receive) | ≈29 000 |
| Per GPU (utilization %, dedicated, shared, temperature), plus per engine type (one percent column) | ≈47 200, plus ≈3 700 |

The top lists dominate. A typical machine (16 logical processors, 2 disks, 2 adapters, 1 GPU with 8 engine types) comes to about 1.52 MB. A 64-processor machine with 4 disks, 8 adapters (virtual switches, VPNs) and 2 GPUs with 10 engine types each comes to 2 063 325 bytes, over the ≈2 MB budget of spec §4.4. That is accepted rather than restructured: it is a fixed size, set by the device count, and it does not grow with time.

## Manual checks: Plan 3a

1. Performance → each GPU: the discrete GPU shows a temperature that tracks a load (e.g. a game or `dxdiag` 3D test); an integrated GPU or one without a sensor shows "—". The Overview GPU meter shows the busiest GPU's temperature.
2. On a Hyper-V host (Default Switch present): download a large file; the Overview network meter matches the physical adapter's page, not double.
3. Performance → CPU: one cell per logical processor in Task Manager's order; Speed within a few percent of Task Manager's.
4. Performance → Memory: composition matches Task Manager's In use / Modified / Standby / Free within rounding.
5. 1 min / 10 min / 1 h: right after start the 1 h graph only fills its right edge; scrubbing its empty part says "No history for this moment."; a scrub card follows the pointer and shows the right time.
6. Keyboard: Tab into the list; Up/Down (and Home/End, Page Up/Page Down) move the highlight; Right expands a group, and on an expanded group moves to its first process; Left goes from a process back to its group, and collapses an expanded group; Enter or Space expands or collapses a group; Delete ends the highlighted app (critical processes ask first); the Menu key opens its menu; Esc leaves the list and the live order resumes; Esc also closes the menu or modal; repeat with Narrator.
7. Light and dark at 100 / 150 / 200 % scale: rail icons, graphs, cards, focus rings and the Performance page readable.
8. Scrub card: it flips to the other side of the pointer near the graph's edge, is not clipped at the smallest window size, and reads correctly in light and dark.
9. Narrator on the keyboard-navigated list: note what it announces for the list itself and for the current row.
10. Shift+F10 and the Menu key: the native context menu opens anchored at the highlighted row, not at the pointer or a window corner.
11. Meters do not change height when the window opens: open the window from the tray and from a cold start and watch the Overview meters.

## Manual checks: Plan 3b

1. Startup (not elevated): the list matches Task Manager → Startup apps (names, publishers, Enabled/Disabled). Toggling a per-user entry in either app shows the same state in the other after a refresh.
2. Packaged startup task (e.g. Windows Terminal → Settings → Launch on machine startup): Negotium lists it; disabling it in Negotium shows it disabled in Settings → Apps → Startup, and enabling it there shows it enabled after Refresh in Negotium.
3. All-users entries without admin: switches disabled with "Needs admin" and the inline banner; "Restart as admin" reopens Negotium on Startup with those switches enabled.
4. Settings: each refresh rate visibly changes the update pace; Theme System/Light/Dark switch content, title bar and Mica; with System, flipping Windows' mode follows within a second; Always on top holds above Explorer; with "Keep running in the tray" off, closing the window exits.
5. Settings persist across restarts; replacing `%LOCALAPPDATA%\Negotium\settings.json` with `{` starts Negotium with defaults and leaves `settings.json.bad`.
6. GPU renderer: it is the default and resizing is smooth. The debug build with `NEGOTIUM_SIMULATE_STALL=1` relaunches into software once and creates `%LOCALAPPDATA%\Negotium\renderer-fallback`. Settings → "Use GPU rendering" then shows off with its explanation; turning it on clears the marker after a restart. A minimised start must not trigger the fallback; the tray appears after about 10 s.
7. Graphs fill their boxes top to bottom.
8. Shift+F10 / the Menu key with nothing focused highlights a row first, and a second press opens the menu.
9. Task Manager's `StartupApproved\StartupFolder` value name matches Negotium's, which is the shortcut file name including `.lnk`.
10. A hidden (+h) shortcut in the Startup folder: does Task Manager list it, and does it run at sign-in? Negotium skips hidden and system files.
11. After disabling a packaged task in Settings or Task Manager, check whether `LastDisabledTime` is written. Negotium writes only `State`.
12. The UAC prompt for "Restart as admin" appears in front, not as a flashing taskbar button.
13. Theme change: the title bar repaints at once, and the Settings Tab order scrolls each focused row into view.
