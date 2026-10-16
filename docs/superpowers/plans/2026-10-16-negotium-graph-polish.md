# Negotium Plan 3c — Graph Polish Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Graph lines always reach both graph edges; graphs scroll smoothly between samples (Settings "Smooth graphs", on by default); the tray icon shows a live CPU graph (Settings "Show CPU in tray icon", on by default); and the CPU, GPU and memory cost of all this is measured and written down.

**Architecture:** Graph geometry stays on the worker. Each frame's paths are anchored on the newest *history sample* (not the tick) at x = 1000, and carry some history left of the window at negative x, clipped by the `Path`. Slint scrolls the view. A `GraphClock` global holds a short "run" for the view's right edge: where it starts, relative to the newest sample, and how fast it moves. A Slint `Timer` advances the clock only while the window is shown on a page with graphs, and only as often as a whole physical pixel of motion needs, at most about 60 times a second. Every data `Path` binds `viewbox-x` to the clock, so no path string is rebuilt between samples. The Rust side (`crate::smooth::Scroll`) decides each run so that the edge never jumps and stays about one sample interval behind real time. The tray graph is rasterised on the UI thread (`crate::trayicon`, pure) from a one-minute CPU sample that the worker sends for every kept history sample. It is shown through Slint 1.18's own `SystemTrayIcon.icon`, which already supports runtime changes through `NIM_MODIFY`. The bench gains run options, one-core CPU, GPU % and a VMMap-style memory breakdown.

**Tech Stack:** Rust stable (MSVC, edition 2024), `slint` 1.18.1 (`Timer`, `animation-tick()`, globals, `Path.viewbox-x`/`clip`, `SystemTrayIcon`), `i-slint-backend-testing` 1.18.1 (dev, mock time), `windows` 0.62.2, `serde`/`serde_json`. No new crates.

**Spec:** `docs/superpowers/specs/2026-10-14-negotium-design.md`: §4.2 (ticks 0.5/1/2/5 s; 2 s hidden), §4.4 (history ≈ 2 MB), §5 and §5.1 (60 s sparklines), §5.2 (graphs, ranges, scrub), §5.4 (Settings), §5.5 (tray), §8 (CPU ≤ 0.5 % of one core while visible, working set ≤ 60 MB). The plan builds on the rulings in `docs/superpowers/plans/2026-10-14-negotium-performance-followups.md` (P3-3 one sample a second, P3-5 max per column, absolute-time columns, `PAUSE_BREAK_MS` 10 s, the one-point stub, `fit: fill`) and `docs/superpowers/plans/2026-10-14-negotium-startup-settings-followups.md` (per-field tolerant settings, `SettingsWiring`, the renderer watchdog and tray gating).

**Out of scope:** recording history at the refresh rate (ruling P3c-1); memory optimisation beyond a cheap win (P3c-8); measuring the hidden (tray) CPU target; animating anything other than graphs.

## Rulings on spec ambiguities

- **P3c-1: Why the edges shift, and the fix.** `graph::trace` draws the newest column at `min(column end, end)`, where `end` is the tick time (`snap.taken_at_ms`: `PerfInput::end`, `view::meters`). The history keeps a sample only ≥ 950 ms after the last one (`negotium_history::MIN_SPACING_MS`, P3-3). So whenever a tick is not kept, the newest drawn sample is older than `end` and the right end sits inward by that age. At 0.5 s this happens on every other frame (≈ 0.5 s, about 6 px on a 700 px 1 min graph). At 1 s it happens after every poke (device, range or metric change, expand, search), which ticks early. On the left, `trace` drops every sample before `end − span`, so the line starts at the first sample inside the window, drawn at its column end. That is up to one sample interval plus one column inside the edge (about 2 % of the 1 min graph, 8 % at 5 s), and it hops as samples age out. **Fix:** anchor every graph's right edge on the newest history sample (`History::latest_t`). Also draw the history before the window (`graph::LEAD_MS`) at negative x, clipped by the `Path`, so the line enters from beyond the left edge. **Rejected:** recording history at the refresh rate. 0.5 s would double the 3601-sample preallocation, and the README budget table puts a typical machine at 1.52 MB and a large one at 2.06 MB, already at the ≈ 2 MB of §4.4. It would also still leave the 1 s poke case and the left edge.
- **P3c-2: What "smooth, ~60 fps" means.** The view moves continuously, and the timer redraws each time the widest scrolling graph on the page has moved a whole physical pixel (as built after Task 9 ruling P3c-3 step 1; it started at half a pixel). The interval runs from 16 ms (≈ 60 fps) to 1 s. Examples: 1 min graph 650 px wide ≈ 46 ms (22 fps); 5120 px window 16 ms (60 fps); Overview sparklines ≈ 200 ms; 1 h graph 1 s. Sub-pixel steps add little visible and cost a full frame each. The view trails real time by one sample interval plus 250 ms (`smooth::lag_ms`), so the newest point eases in from the right.
- **P3c-3: §8 and smooth graphs.** §8's ≤ 0.5 % of one core was written for a static window, and the README shows the collector tick alone at 13–19 ms per 1 s tick (≈ 1.3–1.9 % of one core). Smooth graphs therefore get a budget relative to "Smooth graphs off" on the same build: at most **+0.5 % of one core on the Overview** and **+1.5 % on the Performance page (1 min)**, GPU renderer, 1000 × 720, mean over 60 s. Over budget, Task 9 lowers the rate in fixed steps (whole-pixel steps, then a 30 fps cap) and re-measures. If it is still over, it stops and reports to the user; it does not turn the default off on its own.
- **P3c-4: Which graphs animate.** All of them on the visible page: the Overview sparklines, the Performance main graphs, the cells and the device-list sparklines. FemtoVG (the default renderer) repaints the whole window on every frame, so the cost follows the frame rate, not how many graphs move. The software renderer (fallback) repaints only changed areas. Task 9 measures it but does not gate on it.
- **P3c-5: Tray icon look.** A rounded plate in the taskbar's own tone (light or dark, from `SystemUsesLightTheme`) with a contrasting 1 px outline (2 px from 32 px up). On it, the last 60 s of CPU as a line in the accent colour, adjusted in tenths toward white or black until it reaches ≥ 3:1 contrast with the plate (WCAG's graphics minimum), with a 35 % fill of that colour under it. One column per pixel shows its largest value (P3-5). Size: `GetSystemMetricsForDpi(SM_CXSMICON, GetDpiForSystem())` (16 at 100 %, 20/24/32 at higher DPI), read once at start. Tooltip: "CPU 17% · Negotium", or "Negotium" when the value is unknown or the switch is off. It updates once per kept history sample: ≤ 1/s visible, every 2 s (or the slower chosen rate) hidden. It never animates.
- **P3c-6: Where the tray pixels are drawn.** On the UI thread. The Slint tray, `slint::Image` (not `Send`), the accent colour and the taskbar mode all live there (`platform::read_accent` already runs there on `SettingsChanged`). A 32 × 32 icon with 4 × 4 coverage samples is ≈ 16 k samples, well under a millisecond, once a second. The worker only sends plain data (`TraySample`).
- **P3c-7: Slint's tray supports runtime changes.** `i-slint-core-1.18.1/items/system_tray.rs` keeps an `icon_tracker` and a `tooltip_tracker` (`ChangeTracker`s run by every event-loop iteration). They call `system_tray/windows.rs::set_icon` (`create_hicon` + `Shell_NotifyIconW(NIM_MODIFY)`) and `set_tooltip`, and the new handle survives a TaskbarCreated re-add. No `Shell_NotifyIcon` code is needed in `platform.rs`; the icon is an `in property <image>` bound in `tray.slint`.
- **P3c-8: Memory.** Task 8 adds `negotium-bench memory`, a VMMap-style breakdown. Task 9 records the working set and private working set under the GPU and software renderers, idle on the Overview, and names the largest contributors in the README. A cheap win is applied only if it changes no behaviour, is ≤ 20 lines and saves ≥ 5 MB on the measured run. Anything else becomes a follow-up.
- **P3c-9: Measurement runs.** The bench starts Negotium with its own settings file (`NEGOTIUM_SETTINGS`), so no run reads or changes the user's settings. It needs the user's own Negotium closed, because the single-instance lock would hand the launch to theirs. The executor asks the user before any measurement run and never stops the user's instance itself.
- **P3c-10: Minimised.** The main window's existing subclass reports `WM_SIZE` (`SIZE_MINIMIZED` or any other) as `ShellEvent::Minimised(bool)`. Graphs scroll only while the window is shown: not hidden to the tray, not minimised.

## Global Constraints

- Windows 11 x64 only; Rust stable, `x86_64-pc-windows-msvc`, edition 2024; `slint` 1.18.1; `windows` 0.62.2.
- **No new crates.** `windows` features grow: `crates/app` gains `Win32_UI_HiDpi`; `crates/bench` gains `Win32_System_Memory` and `Win32_System_ProcessStatus`.
- `unsafe` only in `negotium-collector`, `negotium-actions`, `negotium-bench` and `crates/app/src/platform.rs`; every `unsafe` block has its own `// SAFETY:` comment.
- **The UI thread never blocks and never collects data.** Allowed on it, as before: `platform.rs` window setup. This now includes reading the tray icon size once at start and the taskbar mode with the accent on a theme change, plus Slint's own tray `NIM_MODIFY`. Rendering a tray icon (≤ 32 × 32) is allowed there (P3c-6). History is only ever `try_lock`ed on the UI thread.
- **Collection is unchanged:** the refresh rate drives the worker; history keeps at most one sample a second (P3-3) in its ≈ 2 MB preallocation; only the visible page's view is built.
- Graph rulings stay: max per column (P3-5) on an absolute grid, `PAUSE_BREAK_MS` = 10 s, a lone point gets a stub, every data path has a 1000 × 100 view box with `fit: fill`. New for every data path: `clip: true` and `viewbox-x` bound to `GraphClock`.
- Settings: new fields are per-field tolerant (`field!` in `settings::load`, `#[serde(default)]`), on by default, applied at once and saved through `SettingsWiring`/`Change::apply`. Switch labels are exactly **"Smooth graphs"** and **"Show CPU in tray icon"**.
- Slint lessons (keep): every control is a `FocusButton` (switches are `Toggle` in a `SettingRow`); `vertical-stretch: 0` on everything that must not take spare height; `fit: fill` on data paths; the fixed 14 px scrollbar gutter; **`changed` handlers run later**, so when Rust needs an effect in the same frame, call a public Slint function (here `scroll-from`); `max-width: 16384px` on layouts holding repeaters.
- **Tests:**
  - Never show the real window. Use the testing backend only.
  - Never create a `NegotiumTray` in a test: it registers a real notification-area icon.
  - Never touch the user's settings, log or registry. Use temp directories and injected logs.
  - Must pass while the user's `negotium.exe` runs: no single-instance mutex, no main-window mark, no default settings path.
  - UI tests need debug info; run them without `--release`.
  - If linking `negotium.exe` fails because the user runs it from `target\debug`, ask the user. Do not stop their process.
- **Never launch `negotium.exe`** except in Task 9's measurement runs, after the user has quit theirs and said go.
- Every task ends with `cargo test -p negotium` (plus `-p negotium-bench` when touched) and `cargo clippy --workspace --all-targets -- -D warnings` clean. Commit messages use the repository's `type(scope): summary` style and end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- If a `windows` 0.62.2 or Slint 1.18.1 item path or signature differs from this plan, look it up (`~/.cargo/registry/src`) and adapt without changing behaviour; note it in the report.

## Review Focus

1. **A refresh rate change while graphs scroll (0.5 s ↔ 5 s), and the 0.5 s rate where only every other frame brings a sample.** The line keeps reaching both edges, and the view eases to the new pace without jumping back or stalling. Tests: Task 1 `graphs_reach_both_edges_when_the_tick_is_newer_than_the_newest_sample`, Task 2 `a_new_refresh_rate_eases_in_without_a_jump`.
2. **Coming back after a long absence** (hours hidden in the tray, sleep/resume, a worker restart that clears history). The first frames show the current graph at once: no fast-forward through old data, no blank band at the right. Tests: Task 2 `after_a_long_absence_the_edge_starts_one_lag_behind_without_fast_forward`, `a_history_restart_puts_the_edge_on_the_new_samples`.
3. **Very wide or high-DPI windows (5120 px, 200 %), narrow windows and the 1 h range.** Scrolling never redraws more than about 60 times a second and redraws rarely when little moves. Test: Task 3 `graphs_are_redrawn_as_often_as_half_a_pixel_of_scrolling_needs_up_to_60_a_second`.
4. **Accent colours near the taskbar's own tone** (navy on a dark taskbar, pale yellow on a light one), and a taskbar mode change while running. The tray graph stays readable and is redrawn in the new colours. Tests: Task 6 `the_line_contrasts_with_the_plate_whatever_the_accent_and_taskbar`, Task 7 `a_colour_change_redraws_the_last_sample`.
5. **Explorer restarting, or the switch flipped, while the CPU tray icon is on and the window is hidden.** The re-created icon shows the graph again. Off brings back the app icon and stops the worker's samples; on again shows the graph from the next sample. Tests: Task 7 `a_re_created_tray_shows_the_last_graph`, `switching_off_brings_back_the_app_icon_and_ignores_samples`, `the_tray_gets_one_cpu_sample_per_kept_sample_even_while_hidden`.

---

## File Structure

```
crates/app/src/graph.rs           MODIFY: LEAD_MS, since(); history left of the window at negative x
crates/app/src/perf.rs            MODIFY: graphs anchored on the newest sample; window_end_ms / end_clock_ms
crates/app/src/view.rs            MODIFY: sparklines anchored on the newest sample; Frame::graph_end_ms
crates/app/src/smooth.rs          CREATE: the scrolling view's right edge (Scroll, lag_ms); set_shown
crates/app/src/bridge.rs          MODIFY: epoch, one Scroll per frame, scrub from the drawn edge
crates/app/src/scrub.rs           MODIFY: maps the pointer from the drawn right edge
crates/app/src/worker.rs          MODIFY: shared epoch; TraySink and tray samples
crates/app/src/controls.rs        MODIFY: tray_cpu flag
crates/app/src/settings.rs        MODIFY: smooth_graphs, tray_cpu; NEGOTIUM_SETTINGS override
crates/app/src/settingspage.rs    MODIFY: the two switches
crates/app/src/trayicon.rs        CREATE: tray CPU graph pixels and colours (pure)
crates/app/src/tray.rs            CREATE: TraySample, TrayView (pure) and the UI-thread tray glue
crates/app/src/platform.rs        MODIFY: ShellEvent::Minimised; tray_icon_px; taskbar_is_light
crates/app/src/main.rs            MODIFY: wiring
crates/app/src/lib.rs             MODIFY: modules smooth, trayicon, tray
crates/app/Cargo.toml             MODIFY: windows feature Win32_UI_HiDpi
crates/app/ui/graph.slint         MODIFY: GraphClock global; paths clip and scroll; span-ms
crates/app/ui/meters.slint        MODIFY: sparkline clip and scroll; spark-width
crates/app/ui/perfpage.slint      MODIFY: span-ms, graph-width; device sparks clip and scroll
crates/app/ui/main.slint          MODIFY: graph timer, scroll-from, scroll-follow; settings pass-through
crates/app/ui/settingspage.slint  MODIFY: "Smooth graphs" and "Show CPU in tray icon" rows
crates/app/ui/tray.slint          MODIFY: live icon and tooltip
crates/app/tests/ui.rs            MODIFY: new UI tests; helpers follow new fields
crates/app/tests/render.rs        MODIFY: pixel and source checks; scroll-frame bench
crates/bench/src/memmap.rs        CREATE: working set breakdown of a running process
crates/bench/src/main.rs          MODIFY: memory subcommand; run options; one-core CPU; GPU %
crates/bench/Cargo.toml           MODIFY: windows features
docs/benchmarks/README.md         MODIFY: Plan 3c measurements and manual checks
```

---

### Task 1: Graph lines reach both edges

**Files:**
- Modify: `crates/app/src/graph.rs` (`trace`, `time_to_x`, new `LEAD_MS`, `since`)
- Modify: `crates/app/src/perf.rs` (`PerfInput` methods, `build_perf`, `PerfFrame` docs)
- Modify: `crates/app/src/view.rs` (`meters`, new `graph_end`)
- Modify: `crates/app/ui/graph.slint`, `crates/app/ui/meters.slint`, `crates/app/ui/perfpage.slint` (`clip: true` on data paths)
- Test: unit tests in `graph.rs`, `perf.rs`, `view.rs`; `crates/app/tests/render.rs`

**Interfaces:**
- Consumes: `negotium_history::History::latest_t() -> Option<u64>`.
- Produces:
  - `graph::LEAD_MS: u64` (= `PAUSE_BREAK_MS` in this task; Task 3 widens it).
  - `graph::since(end: u64, span: u64) -> u64`: the earliest sample time a graph of `[end − span, end]` draws.
  - `graph::shape` and `graph::line` keep their signatures. They now also draw points in `[end − span − LEAD_MS, end − span)` at negative x.
  - `PerfFrame::window_end_ms` is the newest history sample (the right edge at rest); `end_clock_ms` is that sample's local clock.
  - `view::graph_end(snap: &Snapshot, history: &History) -> u64` (private; Task 3 uses it for `Frame::graph_end_ms`).

- [ ] **Step 1: Write the failing tests**

In `crates/app/src/graph.rs` `mod tests`, add:

```rust
    #[test]
    fn the_sample_before_the_window_carries_the_line_to_the_left_edge() {
        // One sample a second, half a second off the column grid; the window is [1.5 s, 61.5 s].
        let pts: Vec<(u64, Option<f32>)> = (0..=61u64).map(|i| (i * 1_000 + 500, Some(50.0))).collect();
        let s = shape(pts, 61_500, 60_000, 100.0, 60);
        let x = xs(&s.line);
        assert!(x[0] < 0.0 && x[1] > 0.0, "the line enters from beyond the left edge: {}", &s.line[..30]);
        assert_eq!(s.line.matches('M').count(), 1, "one unbroken line");
        assert!(s.area.starts_with(&format!("M {:.1} 100.0", x[0])), "the fill starts with it: {}", &s.area[..30]);
        assert_eq!(*x.last().unwrap(), 1000.0);
    }

    #[test]
    fn history_further_back_than_the_lead_in_is_not_drawn() {
        let (end, span) = (200_000u64, 60_000u64);
        // One stray sample just before `since`, then one a second from the window's start: the stray is
        // further back than a pause from the rest, so drawing it would start a second segment.
        let mut pts = vec![(since(end, span) - 1_000, Some(50.0))];
        pts.extend((end - span + 1_000..=end).step_by(1_000).map(|t| (t, Some(50.0))));
        let s = shape(pts, end, span, 100.0, 60);
        assert_eq!(s.line.matches('M').count(), 1, "{}", &s.line[..40]);
        assert_eq!(since(end, span), end - span - LEAD_MS);
        assert_eq!(since(5_000, 60_000), 0);
    }
```

In `crates/app/src/perf.rs` `mod tests`, add:

```rust
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
```

In `crates/app/src/view.rs` `mod tests`, add:

```rust
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
        let f = build_frame(&mut IdMap::new(), &s, &[], &[], &h, &opts(Metric::Cpu, true));
        let spark = &f.meters[0].spark;
        assert!(spark.starts_with("M 0.0 50.0"), "{spark}");
        assert!(spark.ends_with("L 1000.0 50.0"), "{spark}");
    }
```

In `crates/app/tests/render.rs`, replace `every_data_path_stretches_to_fill_its_box` with a shared scanner and two checks, and add a pixel check:

```rust
/// Every data path (a 1000 × 100 view box, see crate::graph) in ui/*.slint: (file, squeezed body).
fn data_paths() -> Vec<(String, String)> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui");
    let mut found = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "slint") {
            continue;
        }
        let src = squeezed(&std::fs::read_to_string(&path).unwrap());
        for body in path_bodies(&src) {
            if sets(body, "viewbox-width", &["1000", "1000.0"]) {
                found.push((path.display().to_string(), body.to_string()));
            }
        }
    }
    found
}

/// Every data path stretches to its box: the default `fit: contain` keeps the 10:1 aspect and
/// squashes the data into a band.
#[test]
fn every_data_path_stretches_to_fill_its_box() {
    let paths = data_paths();
    assert!(paths.len() >= 5, "found only {} data paths", paths.len());
    for (file, body) in &paths {
        assert!(sets(body, "fit", &["fill"]), "{file}: a data Path without fit: fill: {body}");
    }
}

/// Every data path clips to its box: frames draw history left of their window at negative x
/// (crate::graph::LEAD_MS), which must never paint outside the graph.
#[test]
fn every_data_path_clips_to_its_box() {
    for (file, body) in &data_paths() {
        assert!(sets(body, "clip", &["true"]), "{file}: a data Path without clip: true: {body}");
    }
}

/// A sparkline's history left of its window stays inside its meter: nothing is drawn in the gap
/// between two meters.
#[test]
fn a_sparklines_off_screen_history_is_clipped_to_its_meter() {
    software_backend();
    let ui = MainWindow::new().unwrap();
    ui.global::<Theme>().set_accent(slint::Color::from_rgb_u8(255, 0, 0));
    ui.window().set_size(PhysicalSize::new(1000, 720));
    let meter = |label: &str| negotium::MeterData {
        label: label.into(), value: "1%".into(), sub: "".into(), spark: "M -400 50 L 1400 50".into(), available: true,
    };
    let meters: Vec<negotium::MeterData> = ["CPU", "Memory", "Disk", "GPU", "Network"].into_iter().map(meter).collect();
    ui.set_meters(ModelRc::new(VecModel::from(meters)));
    ui.set_selected_metric(-1); // no meter is tinted by being selected
    ui.show().unwrap();
    let shot = ui.window().take_snapshot().expect("snapshot");
    let memory = ElementHandle::find_by_accessible_label(&ui, "Memory meter").next().expect("memory meter");
    let disk = ElementHandle::find_by_accessible_label(&ui, "Disk meter").next().expect("disk meter");
    let (m, mw, mh, d) = (memory.absolute_position(), memory.size().width, memory.size().height, disk.absolute_position());
    let tinted = |x0: f32, x1: f32| {
        (m.y.round() as u32..(m.y + mh).round() as u32)
            .flat_map(|y| (x0.round() as u32..x1.round() as u32).map(move |x| (x, y)))
            .filter(|&(x, y)| accent_tinted(shot.as_slice()[(y * shot.width() + x) as usize]))
            .count()
    };
    assert!(tinted(m.x + 12.0, m.x + mw - 12.0) > 0, "the line shows inside the meter");
    assert_eq!(tinted(m.x + mw + 1.0, d.x - 1.0), 0, "nothing is drawn between the meters");
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p negotium --lib graph::tests` then `cargo test -p negotium --lib perf::tests::graphs_reach` then `cargo test -p negotium --lib view::tests::meter_sparklines_reach` then `cargo test -p negotium --test render clip`
Expected: `graph::tests` fails to compile (no `since`/`LEAD_MS`). After the first `graph.rs` hunk below, the next three fail on their assertions: the main graph ends at `991.7` and starts at `8.3`; the spark ends at `991.7`; the two render checks find `clip` missing and tint in the gap. If the gap check passes before `clip: true` is added (Slint clipping on its own), keep it as a guard and say so in the report.

- [ ] **Step 3: Implement**

`crates/app/src/graph.rs`: add after `PAUSE_BREAK_MS`:

```rust
/// History drawn left of a graph's window, off the edge (the Path clips it): reaching one pause
/// back always takes in the sample before the window's start, so the line runs to the left edge
/// instead of starting at the first sample inside it.
pub const LEAD_MS: u64 = PAUSE_BREAK_MS;

/// The earliest sample time a graph of `[end - span, end]` draws (see [`LEAD_MS`]).
pub fn since(end: u64, span: u64) -> u64 {
    end.saturating_sub(span + LEAD_MS)
}
```

Replace the doc comment of `shape` with:

```rust
/// Draws `points` in the window `[end - span, end]` merged into columns of `span / buckets` ms
/// laid on an absolute time grid (column k covers `(k·w, (k+1)·w]`), so a sample stays in its column
/// while time advances and the graph scrolls instead of shimmering. A column shows its largest value
/// (ruling P3-5) at its end time (the newest column at `end`, the right edge); a column holding only
/// missing values breaks the line, and columns without any sample are simply skipped. History up to
/// [`LEAD_MS`] before the window is drawn too, at negative x (off the edge; the Path clips it), so
/// the line reaches the left edge. `end` is the newest sample: anchoring on a later moment would pull
/// the line's end in from the right edge. `max <= 0` draws values on the floor.
```

In `trace`, after `let start = end as i64 - span as i64;` add:

```rust
    // History left of the window is drawn too, off the edge (see LEAD_MS).
    let first = start - LEAD_MS as i64;
```

and change the filter `if t < start || t > end as i64 {` to `if t < first || t > end as i64 {`.

Replace `time_to_x` with:

```rust
/// Where collector time `t` is drawn on a graph of `[end - span, end]`: x in the view box, the
/// newest moment on the right edge; negative left of the window (history drawn off the edge, see
/// [`LEAD_MS`]). [`fraction_to_time`] is its inverse inside the window.
fn time_to_x(t: f64, end: u64, span: u64) -> f64 {
    (VIEW_W - (end as f64 - t) / span as f64 * VIEW_W).min(VIEW_W)
}
```

`crates/app/src/perf.rs`: replace the `impl PerfInput<'_>` block with:

```rust
impl PerfInput<'_> {
    /// This tick's collector time (the device grace period counts from it).
    fn end(&self) -> u64 {
        self.snap.taken_at_ms
    }

    /// The graphs' right edge: the newest history sample. At refresh rates under a second the
    /// history keeps one sample a second (ruling P3-3), so a tick can be newer; anchoring on the
    /// tick would pull every line's end in from the edge.
    fn anchor(&self) -> u64 {
        self.history.latest_t().unwrap_or(self.snap.taken_at_ms)
    }

    /// Local wall-clock time (ms since midnight) of the graphs' right edge.
    fn anchor_clock(&self) -> u64 {
        let day = crate::scrub::DAY_MS;
        let back = self.end().saturating_sub(self.anchor()) % day;
        (self.snap.totals.local_time_ms % day + day - back) % day
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

    /// The largest value of a series within the last `span_ms`, 0 when there is none.
    fn max_over(&self, key: &Series, span_ms: u64) -> f32 {
        self.history.points(key, self.anchor().saturating_sub(span_ms)).filter_map(|(_, v)| v).filter(|v| v.is_finite()).fold(0.0, f32::max)
    }

    fn spark(&self, key: &Series, max: f32) -> String {
        let end = self.anchor();
        graph::line(self.history.points(key, graph::since(end, SPARK_MS)), end, SPARK_MS, max, 60)
    }
}
```

In `build_perf`, change `window_end_ms: input.end(),` to `window_end_ms: input.anchor(),` and `end_clock_ms: input.snap.totals.local_time_ms,` to `end_clock_ms: input.anchor_clock(),`. Replace the two `PerfFrame` field docs:

```rust
    /// Graph window `[window_end_ms - span_ms, window_end_ms]` in collector time, for the scrub:
    /// `window_end_ms` is the newest history sample, where the graphs' right edge rests.
    pub window_end_ms: u64,
    pub span_ms: u64,
    /// Local wall-clock time of `window_end_ms`, ms since midnight.
    pub end_clock_ms: u64,
```

`crates/app/src/view.rs`: add above `meters`:

```rust
/// The graphs' right edge: the newest history sample. A tick can be newer (the history keeps one
/// sample a second, ruling P3-3); anchoring on it would pull every line's end in from the edge.
fn graph_end(snap: &Snapshot, history: &History) -> u64 {
    history.latest_t().unwrap_or(snap.taken_at_ms)
}
```

and in `meters` replace the four lines from `let span = …` to the `net_max` line with:

```rust
    let span = SPARK_SECONDS * 1000;
    let end = graph_end(snap, history);
    let since = graph::since(end, span);
    let spark = |key: Series, max: f32| graph::line(history.points(&key, since), end, span, max, SPARK_SECONDS as usize);
    // A zero total is no reading (as on the Memory page): "Unavailable", never 100% in use.
    let memory = t.memory.as_ref().filter(|m| m.total > 0);
    let mem_total = memory.map(|m| m.total as f32).unwrap_or(0.0);
    let net_max = history.points(&Series::NetTotal, end.saturating_sub(span)).filter_map(|(_, v)| v).fold(0.0f32, f32::max).max(t.net_bps.unwrap_or(0.0) as f32);
```

Slint: add `clip: true;` after `fit: fill;` in each of the three `Path`s of `crates/app/ui/graph.slint`, the `Path` in `Meter` (`crates/app/ui/meters.slint`) and the `Path` in `DeviceButton` (`crates/app/ui/perfpage.slint`).

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p negotium` then `cargo clippy --workspace --all-targets -- -D warnings`
Expected: all pass, including the existing graph, perf, view, scrub and render tests (their samples all lie inside their windows), and clippy is clean.

- [ ] **Step 5: Commit**

```bash
git add crates/app/src/graph.rs crates/app/src/perf.rs crates/app/src/view.rs crates/app/ui/graph.slint crates/app/ui/meters.slint crates/app/ui/perfpage.slint crates/app/tests/render.rs
git commit -m "fix(app): graph lines reach both edges at every refresh rate" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Where the scrolling view's edge is (`crate::smooth`)

**Files:**
- Create: `crates/app/src/smooth.rs`
- Modify: `crates/app/src/lib.rs` (`pub mod smooth;`)
- Test: unit tests in `smooth.rs`

**Interfaces:**
- Consumes: nothing.
- Produces (`negotium::smooth`):
  - `SCROLL_MARGIN_MS: u64 = 12_000`, `LAG_MARGIN_MS: f64 = 250.0`, `CATCH_UP_MS: f64 = 2_000.0`, `MIN_SPEED: f64 = 0.5`, `MAX_SPEED: f64 = 2.0`.
  - `fn lag_ms(refresh_ms: u64) -> f64`.
  - `#[derive(Clone, Copy, Debug, PartialEq)] struct Scroll { pub r0: f64, pub at: f64, pub speed: f64, pub newest: f64 }`, with:
    - `fn right_edge(&self, now: f64) -> f64`
    - `fn lead_ms(&self) -> f64`
    - `fn next(prev: Option<&Scroll>, now: f64, newest: f64, lag: f64) -> Scroll`
  - All times are collector ms as `f64`.

- [ ] **Step 1: Write the failing tests**: create `crates/app/src/smooth.rs` holding only the tests module, and add `pub mod smooth;` to `lib.rs` (after `pub mod settingspage;`):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// Frames every `refresh` ms (a tick takes 15 ms) from collector time `from`, each landing 5 ms
    /// after its tick; the history keeps a sample only 950 ms or more after the last (ruling P3-3).
    /// Returns (when each frame landed, its run).
    fn simulate(prev: Option<Scroll>, from: f64, refresh: u64, frames: usize) -> Vec<(f64, Scroll)> {
        let lag = lag_ms(refresh);
        let (mut prev, mut newest) = (prev, prev.map(|p| p.newest));
        let mut out = Vec::new();
        for i in 0..frames {
            let tick = from + i as f64 * (refresh + 15) as f64;
            if newest.is_none_or(|n| tick - n >= 950.0) {
                newest = Some(tick);
            }
            let now = tick + 5.0;
            let run = Scroll::next(prev.as_ref(), now, newest.unwrap(), lag);
            out.push((now, run));
            prev = Some(run);
        }
        out
    }

    #[test]
    fn the_view_trails_by_one_sample_interval_and_a_margin() {
        assert_eq!([500, 1_000, 2_000, 5_000].map(lag_ms), [1_250.0, 1_250.0, 2_250.0, 5_250.0]);
        assert!(SCROLL_MARGIN_MS as f64 >= 2.0 * lag_ms(5_000), "room for the lag and a catch-up");
    }

    #[test]
    fn the_edge_never_jumps_when_a_frame_lands() {
        for refresh in [500, 1_000, 2_000, 5_000] {
            for w in simulate(None, 0.0, refresh, 40).windows(2) {
                let ((_, a), (now, b)) = (w[0], w[1]);
                assert!((a.right_edge(now) - b.r0).abs() < 1e-6, "{refresh} ms: {} then {}", a.right_edge(now), b.r0);
            }
        }
    }

    #[test]
    fn the_edge_settles_one_sample_interval_behind_and_never_stalls() {
        for refresh in [500, 1_000, 2_000, 5_000] {
            let lag = lag_ms(refresh);
            let runs = simulate(None, 0.0, refresh, 60);
            for w in runs[30..].windows(2) {
                let ((t0, run), (t1, _)) = (w[0], w[1]);
                assert!((run.r0 - (t0 - lag)).abs() < 100.0, "{refresh} ms: edge {} vs {}", run.r0, t0 - lag);
                let mut last = run.right_edge(t0);
                let mut t = t0 + 10.0;
                while t < t1 {
                    let edge = run.right_edge(t);
                    assert!(edge > last && edge < run.newest, "{refresh} ms: stalled at {t}: {edge} (newest {})", run.newest);
                    last = edge;
                    t += 10.0;
                }
            }
        }
    }

    #[test]
    fn a_new_refresh_rate_eases_in_without_a_jump() {
        let before = simulate(None, 0.0, 1_000, 30);
        let (last_now, last) = *before.last().unwrap();
        // The user picks 5 s: frames now come 5 s apart and the view must fall 4 s further behind.
        let after = simulate(Some(last), last_now + 1_000.0, 5_000, 30);
        let (now, first) = after[0];
        assert!((last.right_edge(now) - first.r0).abs() < 1e-6, "no jump when the rate changes");
        assert!(first.speed > 0.0 && first.speed < 1.0, "it slows down to fall back: {}", first.speed);
        assert!(first.right_edge(now + 4_900.0) < first.newest, "without running into the newest sample before the next one is due");
        let (now, settled) = *after.last().unwrap();
        assert!((settled.r0 - (now - lag_ms(5_000))).abs() < 100.0, "{} vs {}", settled.r0, now - lag_ms(5_000));
    }

    #[test]
    fn the_first_frame_starts_one_lag_behind_and_never_passes_its_newest_sample() {
        let run = Scroll::next(None, 61_000.0, 60_000.0, 1_250.0);
        assert_eq!((run.r0, run.lead_ms(), run.speed), (59_750.0, -250.0, 1.0));
        assert_eq!(run.right_edge(70_000.0), 60_000.0);
    }

    #[test]
    fn after_a_long_absence_the_edge_starts_one_lag_behind_without_fast_forward() {
        // An hour hidden in the tray: the last run ended an hour ago.
        let old = Scroll::next(None, 61_000.0, 60_000.0, 1_250.0);
        let run = Scroll::next(Some(&old), 3_661_000.0, 3_660_000.0, 1_250.0);
        assert_eq!(run.r0, 3_659_750.0);
        assert_eq!(run.speed, 1.0, "no fast-forward through the hour");
    }

    #[test]
    fn a_history_restart_puts_the_edge_on_the_new_samples() {
        // Collector time restarted (the history was cleared): the old run is far ahead of the new samples.
        let old = Scroll::next(None, 3_600_000.0, 3_599_000.0, 1_250.0);
        let run = Scroll::next(Some(&old), 2_005.0, 2_000.0, 1_250.0);
        assert_eq!(run.r0, 755.0);
        assert!(run.right_edge(2_500.0) <= 2_000.0);
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p negotium --lib smooth::tests`
Expected: FAIL to compile (`Scroll`, `lag_ms` and the constants are undefined).

- [ ] **Step 3: Implement**: above the tests in `crates/app/src/smooth.rs`:

```rust
//! Smooth graphs (Settings > Smooth graphs): between samples the graphs scroll at display rate
//! instead of stepping once per refresh. Collection keeps the chosen refresh rate; only the view
//! moves, and Slint moves it (`GraphClock` in ui/graph.slint): each frame's paths put their newest
//! sample at x = 1000, and the view's right edge trails it. This module decides where that edge
//! is. It runs about one sample interval behind real time, so the next sample is in hand before
//! the edge gets there: the newest point eases in from the right and the line always reaches the
//! edge. Pure; the bridge calls it on the UI thread.

/// How far the view's right edge may trail a frame's newest sample. Frames draw this much history
/// beyond their window's left edge (crate::graph::LEAD_MS), so the view never runs out of line.
pub const SCROLL_MARGIN_MS: u64 = 12_000;
/// Extra delay beyond one sample interval: a tick takes time, and so does the frame after it.
pub const LAG_MARGIN_MS: f64 = 250.0;
/// The view closes a gap to where it should be over at least this long (and over at least two
/// lags, so a slow refresh rate can't make it overshoot).
pub const CATCH_UP_MS: f64 = 2_000.0;
pub const MIN_SPEED: f64 = 0.5;
pub const MAX_SPEED: f64 = 2.0;

/// How far behind real time the view's right edge runs at refresh interval `refresh_ms`: one
/// history sample interval (at least a second: the history keeps at most one sample a second,
/// ruling P3-3) plus [`LAG_MARGIN_MS`].
pub fn lag_ms(refresh_ms: u64) -> f64 {
    refresh_ms.max(1_000) as f64 + LAG_MARGIN_MS
}

/// The view's right edge (collector ms) from one frame to the next: a straight run that stops on
/// the frame's newest sample.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scroll {
    /// Where the right edge was when the frame landed.
    pub r0: f64,
    /// Collector time when the frame landed.
    pub at: f64,
    /// Graph ms per real ms.
    pub speed: f64,
    /// The frame's newest sample: the edge never passes it.
    pub newest: f64,
}

impl Scroll {
    /// The right edge at collector time `now`.
    pub fn right_edge(&self, now: f64) -> f64 {
        (self.r0 + self.speed * (now - self.at).max(0.0)).min(self.newest)
    }

    /// Where the run starts relative to the newest sample (≤ 0), for `GraphClock.lead-ms`.
    pub fn lead_ms(&self) -> f64 {
        self.r0 - self.newest
    }

    /// The run for a frame whose newest sample is `newest`, landing at collector time `now`, the
    /// view meant to trail real time by `lag`. The edge carries on from where `prev` has it, so
    /// nothing jumps, and its speed steers it toward `now - lag`; it is never so fast that it
    /// reaches the newest sample before the next one is due. Without a usable `prev` (the first
    /// frame, smoothing just turned on, a long absence, a history restart) the edge starts at
    /// `now - lag`. Either way it stays within SCROLL_MARGIN_MS of the newest sample: the history
    /// the frame's paths hold.
    pub fn next(prev: Option<&Scroll>, now: f64, newest: f64, lag: f64) -> Scroll {
        let (floor, target) = (newest - SCROLL_MARGIN_MS as f64, now - lag);
        let r0 = match prev.map(|p| p.right_edge(now)) {
            Some(edge) if (floor..=newest).contains(&edge) => edge,
            _ => target.clamp(floor, newest),
        };
        let horizon = CATCH_UP_MS.max(2.0 * lag);
        // The next sample is due one interval after this one.
        let due = (newest + (lag - LAG_MARGIN_MS) - now).max(1.0);
        let speed = (1.0 + (target - r0) / horizon).clamp(MIN_SPEED, MAX_SPEED).min((newest - r0) / due);
        Scroll { r0, at: now, speed, newest }
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p negotium --lib smooth::tests` then `cargo clippy --workspace --all-targets -- -D warnings`
Expected: 7 passed; clippy clean.

- [ ] **Step 5: Commit**

```bash
git add crates/app/src/smooth.rs crates/app/src/lib.rs
git commit -m "feat(app): a scrolling view edge that trails the newest sample without jumps" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Graphs scroll between samples

**Files:**
- Modify: `crates/app/ui/graph.slint` (`GraphClock`, `Graph.span-ms`, `viewbox-x`)
- Modify: `crates/app/ui/meters.slint` (`viewbox-x`, `MetersRow.spark-width`)
- Modify: `crates/app/ui/perfpage.slint` (`span-ms`, `graph-width`, device spark `viewbox-x`)
- Modify: `crates/app/ui/main.slint` (export `GraphClock`, `perf-span-ms`, `scroll-interval`, the graph `Timer`, `scroll-from`)
- Modify: `crates/app/src/graph.rs` (`LEAD_MS`)
- Modify: `crates/app/src/view.rs` (`Frame::graph_end_ms`)
- Modify: `crates/app/src/bridge.rs` (epoch, `scroll`, `scroll_to`, `perf-span-ms`)
- Modify: `crates/app/src/worker.rs` (`spawn` takes the epoch)
- Modify: `crates/app/src/main.rs` (shared epoch)
- Test: `crates/app/tests/ui.rs`, `crates/app/tests/render.rs`

**Interfaces:**
- Consumes: `smooth::{Scroll, lag_ms, SCROLL_MARGIN_MS}` (Task 2); `graph::LEAD_MS`, `view::graph_end` (Task 1).
- Produces:
  - Slint `export global GraphClock`:
    - `in property <bool> smooth: true`, `in property <bool> shown: true`
    - `out property <bool> live`
    - `in-out property <float> lead-ms`, `in-out property <float> speed: 1`
    - `in-out property <duration> anchor`, `in-out property <duration> now`
    - `out property <float> offset-ms`
    - Rust: `negotium::GraphClock`, with `get_offset_ms() -> f32`, `get_live() -> bool`, `set_smooth(bool)`, `set_shown(bool)`, `get_lead_ms() -> f32`, `get_speed() -> f32` and `get_now() -> i64`.
  - `MainWindow`: `public function scroll-from(lead-ms: float, speed: float)` (Rust `invoke_scroll_from(f32, f32)`); `in property <float> perf-span-ms: 60000`; `out property <duration> scroll-interval` (Rust `get_scroll_interval() -> i64`).
  - `Graph`: `in property <float> span-ms: 60000`. `PerformancePage`: `in property <float> span-ms`, `out property <length> graph-width`. `MetersRow`: `out property <length> spark-width`.
  - `Frame::graph_end_ms: u64`; `Bridge::set_epoch(&mut self, epoch: std::time::Instant)`.
  - `worker::spawn(controls, history, epoch: Instant, sink)`.

- [ ] **Step 1: Write the failing tests**

In `crates/app/tests/ui.rs`, change `bridged()` so the existing bridge tests keep their static mapping (their frame times are fictional, unrelated to a real clock):

```rust
fn bridged() -> (MainWindow, Arc<Controls>, Rc<RefCell<Bridge>>) {
    i_slint_backend_testing::init_no_event_loop();
    let ui = MainWindow::new().unwrap();
    // Frames in these tests carry made-up collector times: the graphs stand still (newest sample
    // on the edge) unless a test turns smoothing on with an epoch that matches its frames.
    ui.global::<negotium::GraphClock>().set_smooth(false);
    let controls = Arc::new(Controls::default());
    let bridge = Bridge::install(&ui, controls.clone());
    (ui, controls, bridge)
}
```

and append:

```rust
// ---- Plan 3c Task 3: smooth graphs ----

use negotium::GraphClock;

#[test]
fn the_graph_clock_runs_from_its_lead_to_the_newest_sample_and_stops_there() {
    let ui = shown_window();
    let clock = ui.global::<GraphClock>();
    assert!(clock.get_live(), "smooth graphs are on by default");
    ui.invoke_scroll_from(-1_000.0, 1.0);
    assert_eq!(clock.get_offset_ms(), -1_000.0);
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO); // the timer starts
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(400));
    let o = clock.get_offset_ms();
    assert!((o + 600.0).abs() < 1.0, "400 ms later the edge is 600 ms behind: {o}");
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(2_000));
    assert_eq!(clock.get_offset_ms(), 0.0, "it stops on the newest sample");
    let now = clock.get_now();
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(1_000));
    assert_eq!(clock.get_now(), now, "and the timer stops with it");
}

#[test]
fn the_graph_clock_stands_still_with_smooth_graphs_off_hidden_or_on_pages_without_graphs() {
    let ui = shown_window();
    let clock = ui.global::<GraphClock>();
    ui.invoke_scroll_from(-1_000.0, 1.0);
    clock.set_smooth(false);
    assert!(!clock.get_live());
    assert_eq!(clock.get_offset_ms(), 0.0, "off: the newest sample sits on the edge");
    clock.set_smooth(true);
    clock.set_shown(false);
    assert_eq!(clock.get_offset_ms(), 0.0, "hidden or minimised: no motion");
    clock.set_shown(true);
    ui.set_page(2);
    ui.invoke_scroll_from(-1_000.0, 1.0);
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO);
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(1_000));
    assert_eq!(clock.get_offset_ms(), -1_000.0, "the Startup page has no graphs: the timer doesn't run");
}

#[test]
fn graphs_are_redrawn_as_often_as_half_a_pixel_of_scrolling_needs_up_to_60_a_second() {
    let ui = shown_window(); // 1000 × 720
    let overview = ui.get_scroll_interval();
    assert!((100..=400).contains(&overview), "1 min sparklines, ~150 px wide: {overview} ms");
    ui.set_page(1);
    ui.set_perf_span_ms(60_000.0);
    let minute = ui.get_scroll_interval();
    assert!((25..=80).contains(&minute), "the 1 min main graph, ~650 px wide: {minute} ms");
    ui.set_perf_span_ms(3_600_000.0);
    assert_eq!(ui.get_scroll_interval(), 1_000, "the 1 h graph moves under a pixel a second");
    ui.set_perf_span_ms(60_000.0);
    ui.window().set_size(PhysicalSize::new(5_120, 1_440));
    assert_eq!(ui.get_scroll_interval(), 16, "never more than about 60 frames a second");
}

#[test]
fn landing_frames_continue_the_scrolling_view_from_where_it_is() {
    let (ui, controls, bridge) = bridged();
    let clock = ui.global::<GraphClock>();
    clock.set_smooth(true);
    controls.set_page(Page::Performance);
    // Collector time is now about 61 s.
    bridge.borrow_mut().set_epoch(std::time::Instant::now() - std::time::Duration::from_millis(61_000));
    bridge.borrow_mut().apply(&ui, scrub_frame(60_000, TWO_PM));
    let first = clock.get_lead_ms();
    assert!((first + 250.0).abs() < 50.0, "the first run starts 1.25 s behind now: {first}");
    bridge.borrow_mut().apply(&ui, scrub_frame(61_000, TWO_PM));
    let lead = clock.get_lead_ms();
    assert!((lead + 1_250.0).abs() < 50.0, "the next carries on from there, 1.25 s behind its newest sample: {lead}");
    assert!((0.5..=2.0).contains(&clock.get_speed()));
    assert_eq!(ui.get_perf_span_ms(), 60_000.0);
}

#[test]
fn overview_frames_scroll_the_sparklines_too() {
    let (ui, _controls, bridge) = bridged();
    let clock = ui.global::<GraphClock>();
    clock.set_smooth(true);
    bridge.borrow_mut().set_epoch(std::time::Instant::now() - std::time::Duration::from_millis(61_000));
    bridge.borrow_mut().apply(&ui, Frame { graph_end_ms: 60_000, ..frame(vec![]) });
    assert!((clock.get_lead_ms() + 250.0).abs() < 50.0, "{}", clock.get_lead_ms());
}
```

In `crates/app/tests/render.rs`, add:

```rust
/// Every data path scrolls with GraphClock (crate::smooth): one that didn't would stand still
/// while the others move.
#[test]
fn every_data_path_scrolls_with_the_graph_clock() {
    for (file, body) in &data_paths() {
        assert!(body.contains("viewbox-x:"), "{file}: a data Path without viewbox-x: {body}");
    }
}

/// Where across the "CPU graph" its tinted line is, as a fraction of the graph's width.
fn line_position(ui: &MainWindow) -> f32 {
    let shot = ui.window().take_snapshot().expect("snapshot");
    let graph = ElementHandle::find_by_accessible_label(ui, "CPU graph").next().expect("graph");
    let (p, s) = (graph.absolute_position(), graph.size());
    let y = (p.y + s.height * 3.0 / 8.0).round() as u32; // between two grid lines
    let xs: Vec<f32> = (p.x.round() as u32 + 2..(p.x + s.width).round() as u32 - 2)
        .filter(|&x| accent_tinted(shot.as_slice()[(y * shot.width() + x) as usize]))
        .map(|x| x as f32)
        .collect();
    assert!(!xs.is_empty(), "the line is drawn");
    (xs.iter().sum::<f32>() / xs.len() as f32 - p.x) / s.width
}

#[test]
fn a_graph_scrolls_with_the_graph_clock() {
    software_backend();
    let ui = MainWindow::new().unwrap();
    ui.global::<Theme>().set_accent(slint::Color::from_rgb_u8(255, 0, 0));
    ui.window().set_size(PhysicalSize::new(1000, 720));
    ui.set_page(1);
    // A vertical line half a minute before the newest sample (x = 500 of 1000).
    let g = GraphData {
        label: "CPU".into(), value: "".into(), scale: "".into(), line: "M 500 0 L 500 100".into(),
        area: "M 0 100".into(), line2: "M 0 100".into(),
    };
    ui.set_perf_graphs(ModelRc::new(VecModel::from(vec![g])));
    ui.show().unwrap();
    let at_rest = line_position(&ui);
    assert!((at_rest - 0.5).abs() < 0.02, "the newest sample on the edge: {at_rest}");
    // The view's right edge 15 s behind the newest sample: the line sits three quarters across.
    ui.invoke_scroll_from(-15_000.0, 1.0);
    let scrolled = line_position(&ui);
    assert!((scrolled - 0.75).abs() < 0.02, "{scrolled}");
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p negotium --test ui graph_clock` then `cargo test -p negotium --test render scroll`
Expected: FAIL to compile: `GraphClock`, `invoke_scroll_from`, `get_scroll_interval`, `set_perf_span_ms`, `set_epoch` and `Frame::graph_end_ms` don't exist.

- [ ] **Step 3: Implement the Slint side**

`crates/app/ui/graph.slint`: after the two `export struct` lines, add:

```slint
// The clock every time graph scrolls by (crate::smooth). Rust sets `smooth` (Settings > Smooth
// graphs) and `shown` (the window is neither hidden to the tray nor minimised);
// MainWindow.scroll-from starts a run when a frame lands, and MainWindow's timer advances `now`.
export global GraphClock {
    in property <bool> smooth: true;
    in property <bool> shown: true;
    out property <bool> live: smooth && shown;
    // The run (crate::smooth::Scroll): where the view's right edge started relative to the newest
    // sample (ms, ≤ 0), how many ms of graph time pass per ms, and when it started.
    in-out property <float> lead-ms;
    in-out property <float> speed: 1;
    in-out property <duration> anchor;
    in-out property <duration> now;
    // The view's right edge relative to the newest sample, ms (≤ 0): it never passes the newest
    // sample, and rests on it while graphs don't scroll.
    out property <float> offset-ms: live ? min(0, lead-ms + speed * ((now - anchor) / 1ms)) : 0;
}
```

In `Graph`, after `in property <bool> filled: true;` add:

```slint
    // The time the paths cover (crate::perf::Range), for the scrolling offset.
    in property <float> span-ms: 60000;
    // The view's left edge in path units: the paths put the newest sample at x = 1000 and the view
    // trails it by GraphClock.offset-ms (crate::smooth).
    property <float> scroll-x: GraphClock.offset-ms * 1000 / max(1, root.span-ms);
```

and add `viewbox-x: root.scroll-x;` before `viewbox-width: 1000;` in each of its three `Path`s.

`crates/app/ui/meters.slint`: add `import { GraphClock } from "graph.slint";` below the existing imports. In `Meter`'s `Path` add, before `viewbox-width: 1000;`:

```slint
            // A 60 s sparkline (crate::view::SPARK_SECONDS) scrolling with the graph clock.
            viewbox-x: GraphClock.offset-ms * 1000 / 60000;
```

In `MetersRow`, after `spacing: 8px;` add:

```slint
    // About how wide each sparkline is (a meter less its 10 px padding each side), for the graph
    // timer's interval.
    out property <length> spark-width: (self.width - (root.meters.length - 1) * 8px) / max(1, root.meters.length) - 20px;
```

`crates/app/ui/perfpage.slint`: change the graph import to `import { Graph, GraphData, ScrubRow, GraphClock } from "graph.slint";`. In `DeviceButton`'s `Path` add, before `viewbox-width: 1000;`:

```slint
                // A 60 s sparkline (crate::perf::SPARK_MS) scrolling with the graph clock.
                viewbox-x: GraphClock.offset-ms * 1000 / 60000;
```

In `PerformancePage`, after `in property <int> range;` add:

```slint
    // The time the main graphs and cells cover (the frame's span), for their scrolling offset.
    in property <float> span-ms: 60000;
    // About how wide the main graphs are, for the graph timer's interval.
    out property <length> graph-width: sv.content-w;
```

and add `span-ms: root.span-ms;` to the main `Graph { … }` (after `data: g;`) and to the cell `Graph { data: c; filled: false; vertical-stretch: 1; span-ms: root.span-ms; }`.

`crates/app/ui/main.slint`: change `import { GraphData, ScrubRow } from "graph.slint";` to `import { GraphData, ScrubRow, GraphClock } from "graph.slint";` and add `GraphClock` to the `export { … }` list. Name the two pages' components: `meters-row := MetersRow {` and `perf := PerformancePage {`, and add `span-ms: root.perf-span-ms;` to the `PerformancePage` block. After `in property <[ScrubRow]> scrub-rows;` add:

```slint
    // Smooth graphs (crate::smooth). The paths put each frame's newest sample at x = 1000, and
    // GraphClock moves the view's right edge from `lead-ms` toward it. This timer advances the
    // clock while the window is shown on a page with graphs, as often as the widest scrolling
    // graph there needs to move a whole physical pixel: at most about 60 times a second, far less
    // for the narrow sparklines and the long ranges, never less than once a second.
    in property <float> perf-span-ms: 60000;
    property <float> scroll-px: max(1, (root.page == 0 ? meters-row.spark-width : perf.graph-width) / 1phx);
    property <float> scroll-span-ms: root.page == 0 ? 60000 : root.perf-span-ms;
    out property <duration> scroll-interval: clamp(root.scroll-span-ms / (root.scroll-px * 1), 16, 1000) * 1ms;
    Timer {
        interval: root.scroll-interval;
        running: GraphClock.live && root.page <= 1 && GraphClock.offset-ms < 0;
        triggered => {
            GraphClock.now = animation-tick();
        }
    }
    // A frame's graphs landed (crate::smooth::Scroll): the view's right edge starts `lead-ms`
    // before their newest sample and moves `speed` ms of graph time per ms. A function, not a
    // `changed` handler: the new paths and their offset reach the same rendered frame.
    public function scroll-from(lead-ms: float, speed: float) {
        GraphClock.anchor = animation-tick();
        GraphClock.now = GraphClock.anchor;
        GraphClock.lead-ms = lead-ms;
        GraphClock.speed = speed;
    }
```

- [ ] **Step 4: Implement the Rust side**

`crates/app/src/graph.rs`: replace `LEAD_MS` and its doc with:

```rust
/// History drawn left of a graph's window, off the edge (the Path clips it). The scrolling view can
/// trail its newest sample by up to `crate::smooth::SCROLL_MARGIN_MS`, and one pause further back
/// always takes in the sample before that, so the line reaches the left edge wherever the view is.
pub const LEAD_MS: u64 = crate::smooth::SCROLL_MARGIN_MS + PAUSE_BREAK_MS;
```

`crates/app/src/view.rs`: add to `Frame`, after `pub perf: Option<crate::perf::PerfFrame>,`:

```rust
    /// The newest history sample: where the Overview sparklines' right edge rests (crate::smooth).
    pub graph_end_ms: u64,
```

and in `build_frame`'s `Frame { … }` literal add `graph_end_ms: graph_end(snap, history),`.

`crates/app/src/bridge.rs`: add the imports `use crate::smooth::{self, Scroll};`, `use crate::GraphClock;` and `use std::time::Instant;`. Add two fields to `Bridge`, after `scrub_rows`:

```rust
    /// Collector time counts from here (the worker's epoch), so the scrolling view can be placed
    /// against real time.
    epoch: Instant,
    /// The scrolling graphs' current run (crate::smooth); None while graphs don't scroll.
    scroll: Option<Scroll>,
```

and initialise them in `install` with `epoch: Instant::now(), scroll: None,`. Add these methods to `impl Bridge`:

```rust
    /// The worker's collector epoch (`worker::spawn`).
    pub fn set_epoch(&mut self, epoch: Instant) {
        self.epoch = epoch;
    }

    fn collector_now_ms(&self) -> f64 {
        self.epoch.elapsed().as_secs_f64() * 1_000.0
    }

    /// A frame's graphs landed with their newest sample at `newest_ms`: while graphs scroll, the
    /// view carries on from where it is (crate::smooth::Scroll) and Slint moves it until the next.
    fn scroll_to(&mut self, ui: &MainWindow, newest_ms: u64) {
        if !ui.global::<GraphClock>().get_live() {
            self.scroll = None;
            return;
        }
        let lag = smooth::lag_ms(self.controls.interval().as_millis() as u64);
        let run = Scroll::next(self.scroll.as_ref(), self.collector_now_ms(), newest_ms as f64, lag);
        ui.invoke_scroll_from(run.lead_ms() as f32, run.speed as f32);
        self.scroll = Some(run);
    }
```

In `apply_perf`, after `ui.set_perf_range(p.range.index());` add:

```rust
        ui.set_perf_span_ms(p.span_ms as f32);
        self.scroll_to(ui, p.window_end_ms);
```

In `apply_overview`, after `ui.set_show_admin_banner(…);` add `self.scroll_to(ui, frame.graph_end_ms);`.

`crates/app/src/worker.rs`: change `spawn` to take the epoch:

```rust
pub fn spawn(
    controls: Arc<Controls>,
    history: SharedHistory,
    epoch: Instant,
    sink: impl FnMut(Frame) + Send + 'static,
) -> std::thread::JoinHandle<u32> {
    let ids = IdSeed::default();
```

(the rest of the body unchanged; delete the old `let epoch = Instant::now();`).

`crates/app/src/main.rs`: replace `let _worker = worker::spawn(controls.clone(), history, move |frame| {` with:

```rust
    // Collector time counts from here, for the worker and for the bridge's scrolling view.
    let epoch = std::time::Instant::now();
    bridge.borrow_mut().set_epoch(epoch);
    let _worker = worker::spawn(controls.clone(), history, epoch, move |frame| {
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p negotium` then `cargo clippy --workspace --all-targets -- -D warnings`
Expected: all pass. The existing bridge and scrub tests keep their expectations (`bridged()` turns smoothing off), and `the_bridge_answers_a_scrub_…` still reads "13:59:30" / "13:59:31". Clippy is clean. If the interval bounds miss because the rail or padding differ from the estimate, print the measured widths (`ElementHandle` sizes of "CPU meter" and "% Utilization graph") and widen the bounds to match the formula. Do not change the formula.

- [ ] **Step 6: Commit**

```bash
git add crates/app/ui crates/app/src/graph.rs crates/app/src/view.rs crates/app/src/bridge.rs crates/app/src/worker.rs crates/app/src/main.rs crates/app/tests/ui.rs crates/app/tests/render.rs
git commit -m "feat(app): graphs scroll smoothly between samples, redrawn only as often as motion needs" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: The scrub follows the scrolling graph

**Files:**
- Modify: `crates/app/src/scrub.rs` (`scrub` takes the drawn right edge)
- Modify: `crates/app/src/bridge.rs` (`view_end_ms`, `follow_scrub`, `show_scrub`)
- Modify: `crates/app/ui/main.slint` (`scroll-follow` callback from the graph timer)
- Modify: `crates/app/src/main.rs` (wire `on_scroll_follow`)
- Test: `scrub.rs` unit tests, `crates/app/tests/ui.rs`

**Interfaces:**
- Consumes: `Scroll::right_edge` (Task 2); `Bridge::scroll`, `collector_now_ms`, the graph `Timer` (Task 3).
- Produces:
  - `scrub::scrub(history: &History, perf: &PerfFrame, view_end_ms: u64, fraction: f32) -> ScrubView`.
  - `Bridge::follow_scrub(&mut self, ui: &MainWindow)`.
  - Slint `MainWindow` `callback scroll-follow()` (Rust `on_scroll_follow`).

- [ ] **Step 1: Write the failing tests**

In `crates/app/src/scrub.rs` `mod tests`, add:

```rust
    #[test]
    fn a_scrolling_graph_maps_the_pointer_from_where_its_edge_is_drawn() {
        let p = perf(61_000, 60_000, TWO_PM, Metric::Cpu);
        // The view runs a second behind the newest sample: its right edge shows 60 s.
        let v = scrub(&history(), &p, 60_000, 1.0);
        assert_eq!(v.title, "Top apps at 13:59:59");
        assert_eq!(v.rows[0], ("chrome".to_string(), "60.0%".to_string()));
        assert_eq!(scrub(&history(), &p, 61_000, 1.0).title, "Top apps at 14:00:00");
    }
```

and change every existing call `scrub(<history>, &<perf>, <fraction>)` in that module to pass the perf frame's own `window_end_ms` third. The calls are:

```rust
scrub(&history(), &perf(61_000, 60_000, TWO_PM, Metric::Cpu), 61_000, 0.5)
scrub(&history(), &perf(61_000, 60_000, TWO_PM, Metric::Memory), 61_000, 1.0)
scrub(&history(), &perf(61_000, 3_600_000, TWO_PM, Metric::Cpu), 61_000, 0.1)
scrub(&h, &p, 610_000, 0.5)
scrub(&h, &p, 610_000, 1.0)
scrub(&h, &p, 3_600_000, at_column_end)
scrub(&h, &p, 3_600_000, 1_800_500.0 / 3_600_000.0)
scrub(&history(), &perf(61_000, 60_000, TWO_PM, Metric::Network), 61_000, 0.5)
```

In `crates/app/tests/ui.rs`, append:

```rust
// ---- Plan 3c Task 4: the scrub while graphs scroll ----

#[test]
fn a_scrub_maps_the_pointer_from_where_the_scrolling_graph_is_drawn() {
    use slint::Model;
    let (ui, controls, bridge) = bridged();
    ui.global::<GraphClock>().set_smooth(true);
    controls.set_page(Page::Performance);
    bridge.borrow_mut().set_history(scrub_history());
    bridge.borrow_mut().set_epoch(std::time::Instant::now() - std::time::Duration::from_millis(61_000));
    bridge.borrow_mut().apply(&ui, scrub_frame(60_000, TWO_PM - 1_000));
    bridge.borrow_mut().apply(&ui, scrub_frame(61_000, TWO_PM));
    // The view trails its newest sample (61 s) by 1.25 s: the right edge shows the sample at 60 s.
    bridge.borrow_mut().scrub(&ui, 1.0);
    assert_eq!(ui.get_scrub_title(), "Top apps at 13:59:59");
    assert_eq!(ui.get_scrub_rows().row_data(0).unwrap().value, "60.0%");
    ui.global::<GraphClock>().set_smooth(false);
    bridge.borrow_mut().scrub(&ui, 1.0);
    assert_eq!(ui.get_scrub_title(), "Top apps at 14:00:00", "standing still, the edge is the newest sample");
}

#[test]
fn the_graph_timer_asks_a_scrub_to_follow_and_leaves_it_alone_otherwise() {
    let ui = shown_window();
    ui.set_page(1);
    let follows = Rc::new(std::cell::Cell::new(0));
    let f = follows.clone();
    ui.on_scroll_follow(move || f.set(f.get() + 1));
    ui.set_scrub_x(0.5);
    ui.invoke_scroll_from(-5_000.0, 1.0);
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO);
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(1_000));
    assert!(follows.get() >= 1, "the content moved under a still pointer: the card follows");
    ui.set_scrub_x(-1.0);
    let before = follows.get();
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(1_000));
    assert_eq!(follows.get(), before, "no scrub, no follow-up");
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p negotium --lib scrub::tests` then `cargo test -p negotium --test ui scrub`
Expected: FAIL to compile (`scrub` takes three arguments; `on_scroll_follow` doesn't exist).

- [ ] **Step 3: Implement**

`crates/app/src/scrub.rs`: replace the head of `scrub` with:

```rust
/// The top apps at the moment under the pointer, `fraction` of the way across a Performance
/// graph whose right edge is drawn at collector time `view_end_ms`: `perf.window_end_ms` (the
/// newest sample) while graphs stand still, or where the scrolling view is now (crate::smooth).
pub fn scrub(history: &History, perf: &PerfFrame, view_end_ms: u64, fraction: f32) -> ScrubView {
    // The same x -> time mapping the graph is drawn with (absolute columns, the right edge at view_end_ms).
    let t = graph::fraction_to_time(fraction, view_end_ms, perf.span_ms);
```

(the rest unchanged).

`crates/app/src/bridge.rs`: add to `impl Bridge`:

```rust
    /// Collector time at the right edge of the Performance graphs as drawn: where the scrolling
    /// view is now (crate::smooth), else the newest sample.
    fn view_end_ms(&self, ui: &MainWindow, perf: &PerfFrame) -> u64 {
        match &self.scroll {
            Some(run) if ui.global::<GraphClock>().get_live() => run.right_edge(self.collector_now_ms()).max(0.0).round() as u64,
            _ => perf.window_end_ms,
        }
    }

    /// The graph timer moved the view under a still pointer: the card follows to the moment now
    /// under it.
    pub fn follow_scrub(&mut self, ui: &MainWindow) {
        self.show_scrub(ui);
    }
```

and in `show_scrub` replace

```rust
        let Ok(h) = history.try_lock() else {
            return;
        };
        let view = crate::scrub::scrub(&h, perf, f);
```

with

```rust
        let view_end = self.view_end_ms(ui, perf);
        let Ok(h) = history.try_lock() else {
            return;
        };
        let view = crate::scrub::scrub(&h, perf, view_end, f);
```

`crates/app/ui/main.slint`: add `callback scroll-follow();` next to `callback scrub(float);`, and make the graph timer's handler:

```slint
        triggered => {
            GraphClock.now = animation-tick();
            // The view moved under a still pointer: the scrub card follows (crate::bridge).
            if (root.scrub-x >= 0) {
                root.scroll-follow();
            }
        }
```

`crates/app/src/main.rs`: after the `ui.on_scrub(…)` block add:

```rust
    {
        let (b, weak) = (bridge.clone(), weak.clone());
        ui.on_scroll_follow(move || {
            if let Some(ui) = weak.upgrade() {
                b.borrow_mut().follow_scrub(&ui);
            }
        });
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p negotium` then `cargo clippy --workspace --all-targets -- -D warnings`
Expected: all pass; clippy clean.

- [ ] **Step 5: Commit**

```bash
git add crates/app/src/scrub.rs crates/app/src/bridge.rs crates/app/src/main.rs crates/app/ui/main.slint crates/app/tests/ui.rs
git commit -m "feat(app): the scrub reads the moment drawn under the pointer while graphs scroll" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: "Smooth graphs" switch; no motion while hidden or minimised

**Files:**
- Modify: `crates/app/src/settings.rs` (`smooth_graphs`)
- Modify: `crates/app/src/settingspage.rs` (show and handle the switch)
- Modify: `crates/app/ui/settingspage.slint`, `crates/app/ui/main.slint` (row, properties)
- Modify: `crates/app/src/smooth.rs` (`set_shown`)
- Modify: `crates/app/src/platform.rs` (`ShellEvent::Minimised`, `WM_SIZE`)
- Modify: `crates/app/src/main.rs` (shown on hide, show and minimise)
- Test: `settings.rs`, `platform.rs` unit tests; `crates/app/tests/ui.rs`

**Interfaces:**
- Consumes: `GraphClock.smooth` and `GraphClock.shown` (Task 3).
- Produces:
  - `Settings::smooth_graphs: bool` (default true; JSON `smooth_graphs`).
  - `smooth::set_shown(ui: &MainWindow, visible: bool, minimised: bool)`.
  - `platform::ShellEvent::Minimised(bool)`.
  - Slint `MainWindow`: `in-out property <bool> settings-smooth-graphs: true`, `callback smooth-graphs-toggled(bool)`. `SettingsPage`: `in property <bool> smooth-graphs`, `callback smooth-graphs-toggled(bool)`, row id `smooth-row`.

- [ ] **Step 1: Write the failing tests**

`crates/app/src/settings.rs` `mod tests`: add

```rust
    #[test]
    fn smooth_graphs_default_on_and_a_bad_value_loses_only_itself() {
        let dir = TempDir::new("smooth");
        let p = dir.file();
        std::fs::write(&p, r#"{"refresh_ms": 2000}"#).unwrap();
        assert!(load(&p).0.smooth_graphs, "a file that predates the switch: on");
        std::fs::write(&p, r#"{"smooth_graphs": "yes", "refresh_ms": 2000}"#).unwrap();
        let (s, how) = load(&p);
        assert_eq!(how, Loaded::Partial(vec!["smooth_graphs".to_string()]));
        assert!(s.smooth_graphs && s.refresh_ms == 2000);
        std::fs::write(&p, r#"{"smooth_graphs": false}"#).unwrap();
        assert!(!load(&p).0.smooth_graphs);
    }
```

and in `settings_round_trip_as_readable_json` add `smooth_graphs: false,` to the `Settings { … }` literal.

`crates/app/src/platform.rs` `mod tests`: add

```rust
    #[test]
    fn minimising_and_restoring_reach_the_hook() {
        use windows::Win32::UI::WindowsAndMessaging::{SIZE_MINIMIZED, SIZE_RESTORED, WM_SIZE};
        let hwnd = hidden_window();
        let seen = Rc::new(RefCell::new(Vec::new()));
        let s = seen.clone();
        assert!(install_window_hooks(hwnd, move |e| s.borrow_mut().push(e)));
        // SAFETY: synchronous sends to our own window on this thread.
        unsafe {
            SendMessageW(hwnd, WM_SIZE, Some(WPARAM(SIZE_MINIMIZED as usize)), Some(LPARAM(0)));
            SendMessageW(hwnd, WM_SIZE, Some(WPARAM(SIZE_RESTORED as usize)), Some(LPARAM(0)));
            let _ = DestroyWindow(hwnd);
        }
        let sizes: Vec<ShellEvent> = seen.borrow().iter().copied().filter(|e| matches!(e, ShellEvent::Minimised(_))).collect();
        assert_eq!(sizes, [ShellEvent::Minimised(true), ShellEvent::Minimised(false)]);
    }
```

`crates/app/tests/ui.rs`:

- In `apply_settings_sets_every_wired_property`, make the first literal `Settings { refresh_ms: 5000, theme: ThemeChoice::Dark, always_on_top: true, keep_in_tray: false, gpu_rendering: false, smooth_graphs: false }`.
  - After its `get_settings_gpu_rendering` assertion add `assert!(!ui.get_settings_smooth_graphs() && !ui.global::<negotium::GraphClock>().get_smooth(), "shown and applied");`.
  - After the second block's `get_settings_gpu_rendering` assertion add `assert!(ui.get_settings_smooth_graphs() && ui.global::<negotium::GraphClock>().get_smooth());`.
- In `a_choice_applies_at_once_shows_and_is_saved`, change the final literal to `Settings { refresh_ms: 500, theme: ThemeChoice::Dark, always_on_top: true, keep_in_tray: false, ..Settings::default() }`.
- In `a_control_reached_with_tab_is_scrolled_into_view`, add a handler next to the others:

```rust
    let l = log.clone();
    ui.on_smooth_graphs_toggled(move |_| l.borrow_mut().push("Smooth graphs".into()));
```

  and in `row_of` add the arm `"Smooth graphs" => "smooth-row",` before `_ => "gpu-row",`.
- In `screen_readers_hear_each_setting_once_with_its_description`, change the expected title count from `8` to `9`.
- Append:

```rust
// ---- Plan 3c Task 5: the Smooth graphs switch, hidden and minimised ----

#[test]
fn the_smooth_graphs_switch_applies_at_once_and_is_saved() {
    let dir = Scratch::new("smooth");
    let path = dir.0.join("settings.json");
    let (ui, _controls, _settings) = settings_wired(&dir, Settings::default(), false);
    let clock = ui.global::<GraphClock>();
    assert_eq!(switch(&ui, "Smooth graphs").accessible_checked(), Some(true));
    switch(&ui, "Smooth graphs").invoke_accessible_default_action();
    assert!(!clock.get_smooth() && !clock.get_live(), "graphs stop scrolling at once");
    assert_eq!(switch(&ui, "Smooth graphs").accessible_checked(), Some(false));
    saved(&path, "smooth graphs off", |s| !s.smooth_graphs);
}

#[test]
fn graphs_scroll_only_while_the_window_is_on_screen() {
    let ui = window();
    let clock = ui.global::<GraphClock>();
    negotium::smooth::set_shown(&ui, false, false);
    assert!(!clock.get_live(), "hidden to the tray");
    negotium::smooth::set_shown(&ui, true, true);
    assert!(!clock.get_live(), "minimised");
    negotium::smooth::set_shown(&ui, true, false);
    assert!(clock.get_live());
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p negotium --lib settings::tests::smooth` then `cargo test -p negotium --lib platform::tests::minimising` then `cargo test -p negotium --test ui smooth`
Expected: FAIL to compile (`smooth_graphs`, `ShellEvent::Minimised`, `set_shown` and the Slint properties don't exist).

- [ ] **Step 3: Implement**

`crates/app/src/settings.rs`: add to `Settings`, after `gpu_rendering`:

```rust
    /// Graphs scroll continuously between samples (crate::smooth).
    pub smooth_graphs: bool,
```

add `smooth_graphs: true` to `Default::default()`, and `field!(smooth_graphs);` after `field!(gpu_rendering);` in `load`.

`crates/app/src/smooth.rs`: change the module doc's last sentence to "Pure but for [`set_shown`]; the bridge calls it on the UI thread." and add, above the tests:

```rust
/// Graphs scroll only while the window is on screen: hidden to the tray or minimised, nothing
/// extra is drawn (`GraphClock.shown`).
pub fn set_shown(ui: &crate::MainWindow, visible: bool, minimised: bool) {
    use slint::ComponentHandle;
    ui.global::<crate::GraphClock>().set_shown(visible && !minimised);
}
```

`crates/app/src/settingspage.rs`: in `show_settings` add

```rust
    ui.set_settings_smooth_graphs(s.smooth_graphs);
    ui.global::<crate::GraphClock>().set_smooth(s.smooth_graphs);
```

and in `SettingsWiring::install`, before the `gpu_marker` block, add:

```rust
        {
            let change = change.clone();
            ui.on_smooth_graphs_toggled(move |on| {
                change.apply(|s| s.smooth_graphs = on);
            });
        }
```

`crates/app/ui/settingspage.slint`: add to `SettingsPage` `in property <bool> smooth-graphs;` and `callback smooth-graphs-toggled(bool);`, and insert after `theme-row`:

```slint
            smooth-row := SettingRow {
                title: "Smooth graphs";
                description: "Graphs scroll continuously between updates instead of stepping. Turn off to use a little less CPU.";
                Toggle {
                    label: smooth-row.title;
                    description: smooth-row.description;
                    checked: root.smooth-graphs;
                    interactive: root.interactive;
                    toggled(on) => { root.smooth-graphs-toggled(on); }
                    focus-gained => { sv.reveal(smooth-row.absolute-position.y, smooth-row.height); }
                }
            }
```

`crates/app/ui/main.slint`: next to the other settings properties add `in-out property <bool> settings-smooth-graphs: true;` and `callback smooth-graphs-toggled(bool);`. In the `SettingsPage { … }` block add `smooth-graphs: root.settings-smooth-graphs;` and `smooth-graphs-toggled(on) => { root.smooth-graphs-toggled(on); }`.

`crates/app/src/platform.rs`: add to `ShellEvent`:

```rust
    /// The main window was minimised (true) or shown at a size again (false): graphs scroll only
    /// while it is on screen. Sent on every WM_SIZE; the receiver drops repeats.
    Minimised(bool),
```

import `SIZE_MINIMIZED` and `WM_SIZE` from `WindowsAndMessaging`, and in `subclass_proc` add, before the `WM_SETTINGCHANGE` branch:

```rust
    } else if msg == WM_SIZE {
        // SAFETY: as above.
        unsafe { (*hook)(ShellEvent::Minimised(wparam.0 as u32 == SIZE_MINIMIZED)) };
```

`crates/app/src/main.rs`:

- `show_main` becomes:

```rust
fn show_main(controls: &Controls, weak: &slint::Weak<MainWindow>) {
    controls.set_visible(true);
    if let Some(ui) = weak.upgrade() {
        platform::bring_to_front(&ui);
        // Shown and restored: graphs may scroll again (crate::smooth).
        negotium::smooth::set_shown(&ui, true, false);
    }
}
```

- In the close handler's `HideToTray` arm, after `b.borrow_mut().set_hover(false);`, add:

```rust
                    if let Some(ui) = close_weak.upgrade() {
                        negotium::smooth::set_shown(&ui, false, false);
                    }
```

- In the chrome timer, add before `platform::install_window_hooks(hwnd, move |event| {`:

```rust
                    // WM_SIZE comes many times a second during a resize: only a change matters.
                    let last_minimised: Rc<std::cell::Cell<Option<bool>>> = Default::default();
```

  and as the hook closure's first statement:

```rust
                        if let ShellEvent::Minimised(m) = event
                            && last_minimised.replace(Some(m)) == Some(m)
                        {
                            return;
                        }
```

  Add this arm to the inner `match event`:

```rust
                            ShellEvent::Minimised(m) => {
                                if let Some(ui) = weak.upgrade() {
                                    negotium::smooth::set_shown(&ui, controls.visible(), m);
                                }
                            }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p negotium` then `cargo clippy --workspace --all-targets -- -D warnings`
Expected: all pass, including the Tab-order, settings-layout and screen-reader tests; clippy clean.

- [ ] **Step 5: Commit**

```bash
git add crates/app/src crates/app/ui crates/app/tests/ui.rs
git commit -m "feat(app): Smooth graphs setting; graphs never scroll while hidden or minimised" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: The tray icon's CPU graph pixels (`crate::trayicon`)

**Files:**
- Create: `crates/app/src/trayicon.rs`
- Modify: `crates/app/src/lib.rs` (`pub mod trayicon;`)
- Test: unit tests in `trayicon.rs`

**Interfaces:**
- Consumes: `graph::PAUSE_BREAK_MS`.
- Produces (`negotium::trayicon`):
  - `SPAN_MS: u64 = 60_000`; `MIN_CONTRAST: f32 = 3.0`; `type Rgb = (u8, u8, u8)`.
  - `#[derive(Clone, Copy, Debug, PartialEq)] struct Colors { pub plate: Rgb, pub frame: Rgb, pub line: Rgb, pub fill_alpha: f32 }`.
  - `fn colors(accent: Rgb, light_taskbar: bool) -> Colors`; `fn contrast(a: Rgb, b: Rgb) -> f32`.
  - `fn columns(points: &[(u64, Option<f32>)], end: u64, n: usize) -> Vec<Option<f32>>`.
  - `fn render(points: &[(u64, Option<f32>)], end: u64, size: u32, c: &Colors) -> Vec<u8>` (straight-alpha RGBA8, `size × size`, row-major).

- [ ] **Step 1: Write the failing tests**: create `crates/app/src/trayicon.rs` with only its tests module, and add `pub mod trayicon;` to `lib.rs` (after `pub mod startup;`):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const ACCENT: Rgb = (0x00, 0x78, 0xD4);

    fn flat(v: f32) -> Vec<(u64, Option<f32>)> {
        (0..=60u64).map(|s| (s * 1_000, Some(v))).collect()
    }

    fn pixel(img: &[u8], size: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * size + x) * 4) as usize;
        [img[i], img[i + 1], img[i + 2], img[i + 3]]
    }

    fn opaque(c: Rgb) -> [u8; 4] {
        [c.0, c.1, c.2, 255]
    }

    #[test]
    fn the_icon_has_the_size_windows_asks_for() {
        let c = colors(ACCENT, false);
        for size in [16, 20, 24, 32] {
            assert_eq!(render(&flat(40.0), 60_000, size, &c).len(), (size * size * 4) as usize, "{size} px");
        }
    }

    #[test]
    fn without_history_only_the_plate_and_its_outline_show() {
        let c = colors(ACCENT, false);
        let img = render(&[], 60_000, 32, &c);
        assert_eq!(pixel(&img, 32, 16, 16), opaque(c.plate));
        assert_eq!(pixel(&img, 32, 16, 0), opaque(c.frame), "the outline");
        assert_eq!(pixel(&img, 32, 0, 0)[3], 0, "rounded corners are transparent");
        assert!(img.chunks(4).all(|p| p[..3] != [c.line.0, c.line.1, c.line.2]), "no line");
    }

    #[test]
    fn full_and_idle_cpu_run_along_the_top_and_the_bottom() {
        let c = colors(ACCENT, false);
        let full = render(&flat(100.0), 60_000, 32, &c);
        assert_eq!(pixel(&full, 32, 16, 2), opaque(c.line), "100 % along the top of the graph");
        let idle = render(&flat(0.0), 60_000, 32, &c);
        assert_eq!(pixel(&idle, 32, 16, 29), opaque(c.line), "0 % along the bottom");
        assert_eq!(pixel(&idle, 32, 16, 16), opaque(c.plate), "nothing to fill under a 0 % line");
    }

    #[test]
    fn the_fill_shades_only_below_the_line() {
        let c = colors(ACCENT, false);
        let img = render(&flat(50.0), 60_000, 32, &c);
        assert_eq!(pixel(&img, 32, 16, 8), opaque(c.plate), "above the line: the plate");
        let fill = pixel(&img, 32, 16, 24);
        let blend = |p: u8, l: u8| p as f32 + (l as f32 - p as f32) * c.fill_alpha;
        let want = [blend(c.plate.0, c.line.0), blend(c.plate.1, c.line.1), blend(c.plate.2, c.line.2)];
        for (got, want) in fill[..3].iter().zip(want) {
            assert!((*got as f32 - want).abs() <= 1.0, "below the line: the fill {fill:?} vs {want}");
        }
        assert_eq!(fill[3], 255);
    }

    #[test]
    fn a_sloped_line_is_anti_aliased() {
        let c = colors(ACCENT, false);
        let ramp: Vec<(u64, Option<f32>)> = (0..=60u64).map(|s| (s * 1_000, Some(s as f32 * 100.0 / 60.0))).collect();
        let img = render(&ramp, 60_000, 16, &c);
        let fill = |p: u8, l: u8| (p as f32 + (l as f32 - p as f32) * c.fill_alpha).round() as u8;
        let known = [
            opaque(c.plate), opaque(c.line), opaque(c.frame),
            [fill(c.plate.0, c.line.0), fill(c.plate.1, c.line.1), fill(c.plate.2, c.line.2), 255],
        ];
        let mixed = img.chunks(4).filter(|p| p[3] == 255 && !known.iter().any(|k| k[..] == p[..])).count();
        assert!(mixed > 4, "edge pixels mix the line with what is under it: {mixed}");
    }

    #[test]
    fn the_line_contrasts_with_the_plate_whatever_the_accent_and_taskbar() {
        for accent in [(0x00, 0x78, 0xD4), (0x00, 0x1B, 0x3D), (0x1F, 0x1F, 0x1F), (0xFF, 0xF4, 0x9C), (0xFF, 0xFF, 0xFF), (0xE8, 0x11, 0x23)] {
            for light in [false, true] {
                let c = colors(accent, light);
                assert!(contrast(c.line, c.plate) >= MIN_CONTRAST, "{accent:?}, light taskbar {light}: {c:?}");
                assert!(contrast(c.frame, c.plate) >= 4.5, "the outline stands out: {c:?}");
            }
        }
        assert_eq!(colors(ACCENT, false).line, ACCENT, "an accent that already stands out is kept");
    }

    #[test]
    fn each_column_keeps_its_largest_value_and_gaps_stay_empty() {
        let pts = [(1_000, Some(5.0)), (2_000, Some(90.0)), (3_000, None), (59_000, Some(f32::NAN)), (60_000, Some(20.0)), (70_000, Some(99.0))];
        assert_eq!(columns(&pts, 60_000, 6), [Some(90.0), None, None, None, None, Some(20.0)], "spikes survive; unknown and future values are skipped");
        assert!(columns(&pts, 60_000, 0).is_empty());
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p negotium --lib trayicon::tests`
Expected: FAIL to compile (`render`, `colors`, `columns`, `contrast` are undefined).

- [ ] **Step 3: Implement**: above the tests:

```rust
//! The tray icon's CPU graph (Settings > Show CPU in tray icon): the last minute of CPU use as a
//! line with a translucent fill under it, on a plate in the taskbar's own tone ringed by a
//! contrasting outline, drawn anti-aliased at the size Windows uses for tray icons (ruling P3c-5).
//! Pure: no Windows calls (crate::tray shows it).
use crate::graph::PAUSE_BREAK_MS;

/// The time the icon covers, as the Overview sparklines do.
pub const SPAN_MS: u64 = 60_000;
/// WCAG's minimum contrast for graphics.
pub const MIN_CONTRAST: f32 = 3.0;
/// Coverage samples per pixel side: 4 × 4 give smooth edges even at 16 px.
const SS: u32 = 4;

pub type Rgb = (u8, u8, u8);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Colors {
    /// Inside the outline, in the taskbar's own tone.
    pub plate: Rgb,
    /// The outline, contrasting with the taskbar.
    pub frame: Rgb,
    /// The CPU line: the accent, adjusted to stand out from the plate.
    pub line: Rgb,
    /// Opacity of the fill under the line, in the line's colour.
    pub fill_alpha: f32,
}

const DARK_PLATE: Rgb = (0x20, 0x20, 0x20);
const LIGHT_PLATE: Rgb = (0xF3, 0xF3, 0xF3);
const DARK_FRAME: Rgb = (0x1A, 0x1A, 0x1A);
const LIGHT_FRAME: Rgb = (0xE6, 0xE6, 0xE6);

/// WCAG relative luminance.
fn luminance((r, g, b): Rgb) -> f32 {
    let channel = |c: u8| {
        let c = c as f32 / 255.0;
        if c <= 0.039_28 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
}

/// WCAG contrast ratio of two colours: 1 (none) to 21.
pub fn contrast(a: Rgb, b: Rgb) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    (m(a.0, b.0), m(a.1, b.1), m(a.2, b.2))
}

/// The icon's colours on a light or dark taskbar: the plate in the taskbar's tone, the outline in
/// the opposite one, and the accent moved toward the outline's tone, in tenths, until it stands out
/// from the plate by MIN_CONTRAST.
pub fn colors(accent: Rgb, light_taskbar: bool) -> Colors {
    let (plate, frame, toward) =
        if light_taskbar { (LIGHT_PLATE, DARK_FRAME, (0, 0, 0)) } else { (DARK_PLATE, LIGHT_FRAME, (255, 255, 255)) };
    let line = (0..=10)
        .map(|i| mix(accent, toward, i as f32 / 10.0))
        .find(|&c| contrast(c, plate) >= MIN_CONTRAST)
        .unwrap_or(toward);
    Colors { plate, frame, line, fill_alpha: 0.35 }
}

/// The largest known CPU value in each of `n` equal columns of [end - SPAN_MS, end] (ruling P3-5:
/// a spike is never averaged away); None where a column has no known value.
pub fn columns(points: &[(u64, Option<f32>)], end: u64, n: usize) -> Vec<Option<f32>> {
    let mut out: Vec<Option<f32>> = vec![None; n];
    let start = end as i64 - SPAN_MS as i64;
    for &(t, v) in points {
        let Some(v) = v.filter(|v| v.is_finite()) else { continue };
        let t = t as i64;
        if t < start || t > end as i64 {
            continue;
        }
        let i = (((t - start) as f64 / SPAN_MS as f64 * n as f64) as usize).min(n.saturating_sub(1));
        if let Some(slot) = out.get_mut(i) {
            *slot = Some(slot.map_or(v, |b| b.max(v)));
        }
    }
    out
}

/// Draws the icon: `size` × `size` pixels of straight-alpha RGBA8, row by row from the top.
pub fn render(points: &[(u64, Option<f32>)], end: u64, size: u32, c: &Colors) -> Vec<u8> {
    let s = size.max(1) as f32;
    let stroke = (s / 16.0).round().max(1.0);
    let radius = (s * 0.2).max(2.0);
    let (g0, gw) = (stroke, s - 2.0 * stroke); // the graph area, a square inside the outline
    let n = (gw.round() as usize).max(1); // one column per pixel
    let (col_w, col_ms) = (gw / n as f32, SPAN_MS as f32 / n as f32);
    // Runs of joined columns: consecutive known columns no further apart than a pause.
    let mut runs: Vec<Vec<(f32, f32)>> = Vec::new();
    let mut last: Option<usize> = None;
    for (i, v) in columns(points, end, n).into_iter().enumerate() {
        let Some(v) = v else { continue };
        if !last.is_some_and(|l| (i - l) as f32 * col_ms <= PAUSE_BREAK_MS as f32) {
            runs.push(Vec::new());
        }
        let point = (g0 + (i as f32 + 0.5) * col_w, g0 + gw * (1.0 - v.clamp(0.0, 100.0) / 100.0));
        runs.last_mut().expect("pushed above").push(point);
        last = Some(i);
    }
    let half = (s / 16.0 * 1.25).max(1.0) / 2.0; // half the line's width
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for py in 0..size {
        for px in 0..size {
            let (mut rgb, mut covered) = ([0f32; 3], 0f32);
            for sy in 0..SS {
                for sx in 0..SS {
                    let x = px as f32 + (sx as f32 + 0.5) / SS as f32;
                    let y = py as f32 + (sy as f32 + 0.5) / SS as f32;
                    if let Some(c) = shade(x, y, s, stroke, radius, &runs, half, c) {
                        (0..3).for_each(|i| rgb[i] += c[i]);
                        covered += 1.0;
                    }
                }
            }
            let channel = |i: usize| if covered > 0.0 { (rgb[i] / covered).round().clamp(0.0, 255.0) as u8 } else { 0 };
            out.extend_from_slice(&[channel(0), channel(1), channel(2), (covered / (SS * SS) as f32 * 255.0).round() as u8]);
        }
    }
    out
}

/// The opaque colour at (x, y), or None outside the icon's rounded square.
#[allow(clippy::too_many_arguments)]
fn shade(x: f32, y: f32, s: f32, stroke: f32, radius: f32, runs: &[Vec<(f32, f32)>], half: f32, c: &Colors) -> Option<[f32; 3]> {
    let rgb = |c: Rgb| [c.0 as f32, c.1 as f32, c.2 as f32];
    if !inside_rounded(x, y, s, radius) {
        return None;
    }
    if !inside_rounded(x - stroke, y - stroke, s - 2.0 * stroke, (radius - stroke).max(0.0)) {
        return Some(rgb(c.frame));
    }
    let mut out = rgb(c.plate);
    if runs.iter().any(|r| below(r, x, y)) {
        let line = rgb(c.line);
        (0..3).for_each(|i| out[i] += (line[i] - out[i]) * c.fill_alpha);
    }
    if runs.iter().any(|r| near(r, x, y, half)) {
        out = rgb(c.line);
    }
    Some(out)
}

/// Whether (x, y) lies in the square of side `side` at the origin with corners rounded by `r`.
fn inside_rounded(x: f32, y: f32, side: f32, r: f32) -> bool {
    if x < 0.0 || y < 0.0 || x > side || y > side {
        return false;
    }
    let dx = (r - x).max(x - (side - r)).max(0.0);
    let dy = (r - y).max(y - (side - r)).max(0.0);
    dx * dx + dy * dy <= r * r
}

/// Whether (x, y) is under the run's line, within the run's extent.
fn below(run: &[(f32, f32)], x: f32, y: f32) -> bool {
    run.windows(2).any(|w| {
        let ((x0, y0), (x1, y1)) = (w[0], w[1]);
        x >= x0 && x <= x1 && y >= y0 + (y1 - y0) * (x - x0) / (x1 - x0)
    })
}

/// Whether (x, y) is within `half` of the run's line; a lone point draws a dot.
fn near(run: &[(f32, f32)], x: f32, y: f32, half: f32) -> bool {
    if let [(px, py)] = run {
        return (x - px).hypot(y - py) <= half;
    }
    run.windows(2).any(|w| distance_to_segment(x, y, w[0], w[1]) <= half)
}

fn distance_to_segment(x: f32, y: f32, (x0, y0): (f32, f32), (x1, y1): (f32, f32)) -> f32 {
    let (dx, dy) = (x1 - x0, y1 - y0);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 { (((x - x0) * dx + (y - y0) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
    (x - (x0 + t * dx)).hypot(y - (y0 + t * dy))
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p negotium --lib trayicon::tests` then `cargo clippy --workspace --all-targets -- -D warnings`
Expected: 7 passed; clippy clean.

- [ ] **Step 5: Commit**

```bash
git add crates/app/src/trayicon.rs crates/app/src/lib.rs
git commit -m "feat(app): an anti-aliased CPU graph for the tray icon, readable on light and dark taskbars" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: The tray icon shows live CPU ("Show CPU in tray icon")

**Files:**
- Create: `crates/app/src/tray.rs`
- Modify: `crates/app/src/lib.rs` (`pub mod tray;`)
- Modify: `crates/app/src/controls.rs` (`tray_cpu`)
- Modify: `crates/app/src/worker.rs` (`TraySink`, `LiveSource::with_tray`, the sample after a kept push; `spawn` takes the sink)
- Modify: `crates/app/src/settings.rs` (`tray_cpu`)
- Modify: `crates/app/src/settingspage.rs` (switch; `SettingsWiring::tray_cpu_changed`)
- Modify: `crates/app/src/platform.rs` (`tray_icon_px`, `tray_icon_px_at`, `taskbar_is_light`, `light_taskbar_from`)
- Modify: `crates/app/Cargo.toml` (`Win32_UI_HiDpi`)
- Modify: `crates/app/ui/tray.slint`, `crates/app/ui/settingspage.slint`, `crates/app/ui/main.slint`
- Modify: `crates/app/src/main.rs` (wiring)
- Test: `tray.rs`, `worker.rs`, `settings.rs`, `platform.rs` unit tests; `crates/app/tests/ui.rs`

**Interfaces:**
- Consumes: `trayicon::{colors, render, Colors, Rgb, SPAN_MS}` (Task 6); `negotium_history::{History, Series}`; `worker::Latest`.
- Produces:
  - `tray::TraySample { pub end_ms: u64, pub cpu: Option<f32>, pub points: Vec<(u64, Option<f32>)> }` with `from_history(h: &History, end_ms: u64, cpu: Option<f32>) -> TraySample`.
  - `tray::TrayLook { pub icon: Option<(Vec<u8>, u32)>, pub tooltip: String }`.
  - `tray::TrayView { pub enabled: bool, pub size: u32, pub colors: Colors, .. }` with:
    - `new(enabled: bool, size: u32, colors: Colors) -> TrayView`
    - `on_sample(&mut self, TraySample)`
    - `set_enabled(&mut self, bool)`
    - `look(&self) -> TrayLook`
  - `tray::tooltip(cpu: Option<f32>) -> String`; `tray::DEFAULT_ACCENT: Rgb`; `tray::colors_now(accent: Option<Rgb>, light_taskbar: bool) -> Colors`.
  - UI-thread glue: `tray::install(TrayView)`, `tray::attach(&NegotiumTray)`, `tray::on_sample(TraySample)`, `tray::set_enabled(bool)`, `tray::set_colors(Colors)`.
  - `Controls::set_tray_cpu(&self, bool)`, `Controls::tray_cpu(&self) -> bool` (default false).
  - `worker::TraySink = Arc<dyn Fn(TraySample) + Send + Sync>`; `LiveSource::with_tray(self, TraySink) -> LiveSource`; `worker::spawn(controls, history, epoch, tray: Option<TraySink>, sink)`.
  - `Settings::tray_cpu: bool` (default true); `SettingsWiring::tray_cpu_changed: Rc<dyn Fn(bool)>`.
  - `platform::tray_icon_px() -> u32`, `tray_icon_px_at(dpi: u32) -> u32`, `taskbar_is_light() -> bool`, `light_taskbar_from(Option<u32>) -> bool`.
  - Slint `NegotiumTray`: `in property <bool> live`, `in property <image> live-icon`, `in property <string> live-tooltip`. `MainWindow`: `in-out property <bool> settings-tray-cpu: true`, `callback tray-cpu-toggled(bool)`. `SettingsPage`: `tray-cpu`, `tray-cpu-toggled`, row id `tray-cpu-row`.

- [ ] **Step 1: Write the failing tests**

Create `crates/app/src/tray.rs` with only its tests module, and add `pub mod tray;` to `lib.rs` (after `pub mod startup;`):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn sample(cpu: f32) -> TraySample {
        TraySample { end_ms: 60_000, cpu: Some(cpu), points: (0..=60).map(|s| (s * 1_000, Some(cpu))).collect() }
    }

    fn view(enabled: bool) -> TrayView {
        TrayView::new(enabled, 16, colors_now(None, false))
    }

    fn app_icon() -> TrayLook {
        TrayLook { icon: None, tooltip: "Negotium".into() }
    }

    #[test]
    fn with_the_switch_on_each_sample_draws_its_minute_and_names_the_cpu_use() {
        let mut v = view(true);
        assert_eq!(v.look(), app_icon(), "no sample yet: the app icon");
        v.on_sample(sample(17.2));
        let look = v.look();
        assert_eq!(look.tooltip, "CPU 17% · Negotium");
        let (rgba, size) = look.icon.expect("a graph");
        assert_eq!((size, rgba.len()), (16, 16 * 16 * 4));
        assert_eq!(rgba, trayicon::render(&sample(17.2).points, 60_000, 16, &v.colors));
    }

    #[test]
    fn switching_off_brings_back_the_app_icon_and_ignores_samples() {
        let mut v = view(true);
        v.on_sample(sample(50.0));
        v.set_enabled(false);
        assert_eq!(v.look(), app_icon());
        v.on_sample(sample(50.0));
        assert_eq!(v.look(), app_icon(), "samples that still arrive change nothing");
        v.set_enabled(true);
        assert_eq!(v.look(), app_icon(), "on again: the graph returns with the next sample");
        v.on_sample(sample(50.0));
        assert!(v.look().icon.is_some());
    }

    #[test]
    fn a_re_created_tray_shows_the_last_graph() {
        // Explorer restarted: main.rs makes a new tray and attaches it; it shows `look()` at once.
        let mut v = view(true);
        v.on_sample(sample(30.0));
        assert_eq!(v.look(), v.look(), "the look depends only on the last sample, size and colours");
        assert!(v.look().icon.is_some());
    }

    #[test]
    fn a_colour_change_redraws_the_last_sample() {
        let mut v = view(true);
        v.on_sample(sample(50.0));
        let dark = v.look().icon.unwrap().0;
        v.colors = colors_now(None, true);
        assert_ne!(v.look().icon.unwrap().0, dark, "a light taskbar gets a light plate");
    }

    #[test]
    fn an_unknown_cpu_reading_names_only_the_app() {
        assert_eq!(tooltip(None), "Negotium");
        assert_eq!(tooltip(Some(f32::NAN)), "Negotium");
        assert_eq!(tooltip(Some(99.6)), "CPU 100% · Negotium");
    }

    #[test]
    fn a_sample_holds_the_minute_up_to_its_end() {
        let mut h = negotium_history::History::new();
        for s in 0..=90u64 {
            let t = negotium_snapshot::SystemTotals { cpu_pct: Some(s as f32), ..Default::default() };
            h.push(s * 1_000, &t, &[]);
        }
        let s = TraySample::from_history(&h, 90_000, Some(90.0));
        assert_eq!(s.points.first(), Some(&(30_000, Some(30.0))));
        assert_eq!(s.points.last(), Some(&(90_000, Some(90.0))));
    }
}
```

`crates/app/src/worker.rs` `mod tests`: add

```rust
    #[test]
    fn the_tray_gets_one_cpu_sample_per_kept_sample_even_while_hidden() {
        let got: Arc<Mutex<Vec<crate::tray::TraySample>>> = Default::default();
        let sink_got = got.clone();
        let mut source = LiveSource::new(SharedHistory::default(), Instant::now(), IdSeed::default())
            .with_tray(Arc::new(move |s| sink_got.lock().unwrap().push(s)));
        let dir = std::env::temp_dir().join(format!("negotium-worker-tray-{}", std::process::id()));
        source.log = crate::log::Log::new(dir.join("negotium.log"));
        let controls = Controls::default();
        controls.set_visible(false);
        source.next_frame(&controls);
        assert!(got.lock().unwrap().is_empty(), "off until Settings turns it on");
        controls.set_tray_cpu(true);
        std::thread::sleep(Duration::from_millis(negotium_history::MIN_SPACING_MS + 50));
        assert!(source.next_frame(&controls).is_none(), "hidden in the tray: no frame");
        let first = got.lock().unwrap().first().cloned().expect("but the tray gets the sample");
        assert_eq!(first.points.last().map(|p| p.0), Some(first.end_ms), "the minute up to this sample");
        source.next_frame(&controls); // too soon: the history keeps no sample
        assert_eq!(got.lock().unwrap().len(), 1, "one icon per kept sample");
        controls.set_tray_cpu(false);
        std::thread::sleep(Duration::from_millis(negotium_history::MIN_SPACING_MS + 50));
        source.next_frame(&controls);
        assert_eq!(got.lock().unwrap().len(), 1, "switched off: no more samples");
        let _ = std::fs::remove_dir_all(dir);
    }
```

`crates/app/src/settings.rs` `mod tests`: add

```rust
    #[test]
    fn tray_cpu_defaults_on_and_a_bad_value_loses_only_itself() {
        let dir = TempDir::new("traycpu");
        let p = dir.file();
        std::fs::write(&p, r#"{"keep_in_tray": false}"#).unwrap();
        assert!(load(&p).0.tray_cpu, "a file that predates the switch: on");
        std::fs::write(&p, r#"{"tray_cpu": 1, "keep_in_tray": false}"#).unwrap();
        let (s, how) = load(&p);
        assert_eq!(how, Loaded::Partial(vec!["tray_cpu".to_string()]));
        assert!(s.tray_cpu && !s.keep_in_tray);
    }
```

and add `tray_cpu: false,` to the literal in `settings_round_trip_as_readable_json`.

`crates/app/src/platform.rs` `mod tests`: add

```rust
    #[test]
    fn tray_icons_follow_the_dpi_and_the_taskbar_mode_reads_safely() {
        assert_eq!(tray_icon_px_at(96), 16);
        assert_eq!(tray_icon_px_at(192), 2 * tray_icon_px_at(96));
        assert!(light_taskbar_from(Some(1)));
        assert!(!light_taskbar_from(Some(0)) && !light_taskbar_from(None) && !light_taskbar_from(Some(7)));
    }
```

`crates/app/tests/ui.rs`:

- Above `settings_wired_with`, add:

```rust
thread_local! {
    /// What the Settings page told the tray (`SettingsWiring::tray_cpu_changed`), per test thread.
    static TRAY_TOLD: RefCell<Vec<bool>> = const { RefCell::new(Vec::new()) };
}
```

  and add `tray_cpu_changed: Rc::new(|on| TRAY_TOLD.with(|t| t.borrow_mut().push(on))),` to its `SettingsWiring { … }` literal.
- In `apply_settings_sets_every_wired_property`, add `tray_cpu: false` to the first literal and assert `!ui.get_settings_tray_cpu()` after it and `ui.get_settings_tray_cpu()` after the second.
- In `a_control_reached_with_tab_is_scrolled_into_view` add:

```rust
    let l = log.clone();
    ui.on_tray_cpu_toggled(move |_| l.borrow_mut().push("Show CPU in tray icon".into()));
```

  and in `row_of` the arm `"Show CPU in tray icon" => "tray-cpu-row",`.
- In `screen_readers_hear_each_setting_once_with_its_description`, change the title count `9` to `10`.
- Append:

```rust
// ---- Plan 3c Task 7: Show CPU in tray icon ----

#[test]
fn the_tray_cpu_switch_saves_and_tells_the_worker_and_the_tray() {
    let dir = Scratch::new("traycpu");
    let path = dir.0.join("settings.json");
    let (ui, controls, _settings) = settings_wired(&dir, Settings::default(), false);
    controls.set_tray_cpu(true);
    assert_eq!(switch(&ui, "Show CPU in tray icon").accessible_checked(), Some(true));
    switch(&ui, "Show CPU in tray icon").invoke_accessible_default_action();
    assert!(!controls.tray_cpu(), "the worker stops sending samples");
    assert_eq!(TRAY_TOLD.with(|t| t.borrow().clone()), [false], "the tray shows the app icon again");
    assert_eq!(switch(&ui, "Show CPU in tray icon").accessible_checked(), Some(false));
    saved(&path, "tray cpu off", |s| !s.tray_cpu);
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p negotium --lib tray::tests` then `cargo test -p negotium --lib worker::tests::the_tray` then `cargo test -p negotium --test ui tray_cpu`
Expected: FAIL to compile (`TrayView`, `with_tray`, `set_tray_cpu`, `tray_cpu`, `tray_icon_px_at` and the Slint properties don't exist).

- [ ] **Step 3: Implement the tray module**: above the tests in `crates/app/src/tray.rs`:

```rust
//! The tray icon (spec §5.5): the app icon or, with Settings > Show CPU in tray icon, a small CPU
//! graph redrawn for each history sample the worker sends, with "CPU 17% · Negotium" on hover. The
//! pixels are drawn here, on the UI thread (ruling P3c-6): the Slint tray, its image type, the
//! accent colour and the taskbar mode all live on this thread, and a 32 × 32 icon takes well under
//! a millisecond once a second. Slint pushes a changed `icon` to the shell itself (NIM_MODIFY).
use crate::NegotiumTray;
use crate::trayicon::{self, Colors, Rgb};
use negotium_history::{History, Series};
use slint::ComponentHandle;
use std::cell::RefCell;

/// Windows' default accent, when the user's can't be read.
pub const DEFAULT_ACCENT: Rgb = (0x00, 0x78, 0xD4);

/// One history sample for the tray: the last minute of CPU use (crate::worker sends it).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TraySample {
    pub end_ms: u64,
    pub cpu: Option<f32>,
    pub points: Vec<(u64, Option<f32>)>,
}

impl TraySample {
    pub fn from_history(h: &History, end_ms: u64, cpu: Option<f32>) -> TraySample {
        TraySample { end_ms, cpu, points: h.points(&Series::Cpu, end_ms.saturating_sub(trayicon::SPAN_MS)).collect() }
    }
}

/// What the tray shows: a graph (RGBA pixels and their side) or the app icon, and the hover text.
#[derive(Clone, Debug, PartialEq)]
pub struct TrayLook {
    pub icon: Option<(Vec<u8>, u32)>,
    pub tooltip: String,
}

/// The tray's state, without Slint: the switch, the icon size and colours, and the last sample.
pub struct TrayView {
    pub enabled: bool,
    pub size: u32,
    pub colors: Colors,
    last: Option<TraySample>,
}

impl TrayView {
    pub fn new(enabled: bool, size: u32, colors: Colors) -> TrayView {
        TrayView { enabled, size, colors, last: None }
    }

    pub fn on_sample(&mut self, s: TraySample) {
        if self.enabled {
            self.last = Some(s);
        }
    }

    /// Off: the app icon at once. On again: the graph from the next sample.
    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
        self.last = None;
    }

    pub fn look(&self) -> TrayLook {
        match self.last.as_ref().filter(|_| self.enabled) {
            Some(s) => TrayLook {
                icon: Some((trayicon::render(&s.points, s.end_ms, self.size, &self.colors), self.size)),
                tooltip: tooltip(s.cpu),
            },
            None => TrayLook { icon: None, tooltip: "Negotium".into() },
        }
    }
}

/// "CPU 17% · Negotium", or just "Negotium" while the CPU use is unknown.
pub fn tooltip(cpu: Option<f32>) -> String {
    match cpu.filter(|v| v.is_finite()) {
        Some(v) => format!("CPU {} · Negotium", crate::format::pct(v)),
        None => "Negotium".into(),
    }
}

/// The icon's colours for the user's accent (if it could be read) and the taskbar's mode.
pub fn colors_now(accent: Option<Rgb>, light_taskbar: bool) -> Colors {
    trayicon::colors(accent.unwrap_or(DEFAULT_ACCENT), light_taskbar)
}

struct Live {
    view: TrayView,
    tray: Option<slint::Weak<NegotiumTray>>,
}

thread_local! {
    /// The tray as the UI thread knows it. `main` creates the Slint tray once the renderer watchdog
    /// has resolved, and again after Explorer restarts; it attaches each one here.
    static TRAY: RefCell<Option<Live>> = const { RefCell::new(None) };
}

/// Starts the tray state (UI thread, once, before the worker runs).
pub fn install(view: TrayView) {
    TRAY.with(|t| *t.borrow_mut() = Some(Live { view, tray: None }));
}

/// A tray was (re)created: it shows the current look at once.
pub fn attach(tray: &NegotiumTray) {
    update(|live| live.tray = Some(tray.as_weak()));
}

pub fn on_sample(s: TraySample) {
    update(|live| live.view.on_sample(s));
}

pub fn set_enabled(on: bool) {
    update(|live| live.view.set_enabled(on));
}

pub fn set_colors(colors: Colors) {
    update(|live| live.view.colors = colors);
}

fn update(f: impl FnOnce(&mut Live)) {
    let shown = TRAY.with(|t| {
        let mut t = t.borrow_mut();
        let live = t.as_mut()?;
        f(live);
        let tray = live.tray.as_ref().and_then(|w| w.upgrade())?;
        Some((live.view.look(), tray))
    });
    // Outside the borrow: setting the tray's properties may run Slint change handlers.
    if let Some((look, tray)) = shown {
        show(&tray, &look);
    }
}

fn show(tray: &NegotiumTray, look: &TrayLook) {
    if let Some((rgba, size)) = &look.icon {
        let pixels = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(rgba, *size, *size);
        tray.set_live_icon(slint::Image::from_rgba8(pixels));
    }
    tray.set_live(look.icon.is_some());
    tray.set_live_tooltip(look.tooltip.as_str().into());
}
```

- [ ] **Step 4: Implement the worker, controls, settings and platform parts**

`crates/app/src/controls.rs`: add the field `tray_cpu: AtomicBool,` (doc: `/// Settings > Show CPU in tray icon: the worker sends each kept sample's CPU minute to the tray.`), initialise it `tray_cpu: AtomicBool::new(false),`, and add:

```rust
    pub fn set_tray_cpu(&self, on: bool) {
        self.tray_cpu.store(on, Ordering::Relaxed);
    }
    pub fn tray_cpu(&self) -> bool {
        self.tray_cpu.load(Ordering::Relaxed)
    }
```

`crates/app/src/worker.rs`:

- Add `use crate::tray::TraySample;` and:

```rust
/// Where each kept history sample's tray graph goes (Settings > Show CPU in tray icon).
pub type TraySink = Arc<dyn Fn(TraySample) + Send + Sync>;
```

- Add the field `tray: Option<TraySink>,` to `LiveSource`, set `tray: None,` in `new`, and add:

```rust
    /// Sends the tray a CPU sample for every history sample kept while Controls::tray_cpu is on,
    /// visible or hidden.
    pub fn with_tray(mut self, sink: TraySink) -> Self {
        self.tray = Some(sink);
        self
    }
```

- In `next_frame` replace the `write_history(…)` line with:

```rust
        let kept = write_history(&self.history, |h| h.push(snap.taken_at_ms, &snap.totals, &rows));
        if kept
            && controls.tray_cpu()
            && let Some(tray) = &self.tray
        {
            tray(read_history(&self.history, |h| TraySample::from_history(h, snap.taken_at_ms, snap.totals.cpu_pct)));
        }
```

- `spawn` gains `tray: Option<TraySink>` after `epoch`. Its `supervise` call becomes:

```rust
            supervise(
                move || {
                    let source = LiveSource::new(history.clone(), epoch, ids.clone());
                    match &tray {
                        Some(t) => source.with_tray(t.clone()),
                        None => source,
                    }
                },
                controls,
                sink,
                move |m: &str| log.warn(m),
            )
```

`crates/app/src/settings.rs`: add to `Settings`, after `smooth_graphs`:

```rust
    /// The tray icon draws the CPU graph (crate::tray).
    pub tray_cpu: bool,
```

then add `tray_cpu: true` to `Default` and `field!(tray_cpu);` to `load`.

`crates/app/src/settingspage.rs`:

- In `show_settings` add `ui.set_settings_tray_cpu(s.tray_cpu);`.
- Add the field to `SettingsWiring`:

```rust
    /// The tray shows the CPU graph or the app icon (`crate::tray::set_enabled` in the app).
    pub tray_cpu_changed: Rc<dyn Fn(bool)>,
```

- In `install`, the refresh block moved `self.controls`; keep it for the new handler too. The start of `install`'s handlers becomes:

```rust
        let change = Rc::new(Change { settings: self.settings, saver: self.saver, weak: ui.as_weak() });
        let controls = self.controls;
        {
            let (change, controls) = (change.clone(), controls.clone());
            ui.on_refresh_chosen(move |i| {
                let ms = refresh_from_index(i);
                change.apply(|s| s.refresh_ms = ms);
                controls.set_refresh_ms(ms);
            });
        }
```

  and add before the GPU block:

```rust
        {
            let (change, controls, told) = (change.clone(), controls.clone(), self.tray_cpu_changed.clone());
            ui.on_tray_cpu_toggled(move |on| {
                change.apply(|s| s.tray_cpu = on);
                controls.set_tray_cpu(on);
                told(on);
            });
        }
```

`crates/app/Cargo.toml`: add `"Win32_UI_HiDpi",` to the `windows` features.

`crates/app/src/platform.rs`: import `windows::Win32::UI::HiDpi::{GetDpiForSystem, GetSystemMetricsForDpi}` and `SM_CXSMICON` from `WindowsAndMessaging`, then add:

```rust
/// `SystemUsesLightTheme`: 1 means the taskbar (and Start) are light. Missing means dark, the
/// default Windows taskbar.
pub fn light_taskbar_from(system_uses_light_theme: Option<u32>) -> bool {
    system_uses_light_theme == Some(1)
}

/// Whether the taskbar is light (Settings > Personalization > Colors > "Choose your mode").
pub fn taskbar_is_light() -> bool {
    light_taskbar_from(read_hkcu_dword(
        w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
        w!("SystemUsesLightTheme"),
    ))
}

/// The tray icon size at `dpi`: SM_CXSMICON (16 at 96 DPI, 24 at 144, 32 at 192); 16 if the call fails.
pub fn tray_icon_px_at(dpi: u32) -> u32 {
    // SAFETY: plain metric query.
    let px = unsafe { GetSystemMetricsForDpi(SM_CXSMICON, dpi) };
    if px > 0 { px as u32 } else { 16 }
}

/// The tray icon size at the system DPI, which the notification area uses. Read once at start:
/// the system DPI changes only at sign-in.
pub fn tray_icon_px() -> u32 {
    // SAFETY: plain query of the system DPI.
    tray_icon_px_at(unsafe { GetDpiForSystem() })
}
```

- [ ] **Step 5: Implement the Slint side and the wiring**

`crates/app/ui/tray.slint`:

```slint
export component NegotiumTray inherits SystemTrayIcon {
    // crate::tray: the CPU graph (Settings > Show CPU in tray icon) or the app icon, and the
    // hover text. Slint passes a changed icon or tooltip to the shell itself.
    in property <bool> live;
    in property <image> live-icon;
    in property <string> live-tooltip: "Negotium";
    icon: root.live ? root.live-icon : @image-url("../assets/negotium.svg");
    tooltip: root.live-tooltip;
    callback open();
    callback exit();
    // Left-click on the icon opens the window too.
    clicked => { root.open(); }
    Menu {
        MenuItem { title: "Open"; activated => { root.open(); } }
        MenuItem { title: "Exit"; activated => { root.exit(); } }
    }
}
```

`crates/app/ui/settingspage.slint`: add `in property <bool> tray-cpu;` and `callback tray-cpu-toggled(bool);` to `SettingsPage`, and insert after `tray-row`:

```slint
            tray-cpu-row := SettingRow {
                title: "Show CPU in tray icon";
                description: "The tray icon draws the last minute of CPU use; point at it to see the current value.";
                Toggle {
                    label: tray-cpu-row.title;
                    description: tray-cpu-row.description;
                    checked: root.tray-cpu;
                    interactive: root.interactive;
                    toggled(on) => { root.tray-cpu-toggled(on); }
                    focus-gained => { sv.reveal(tray-cpu-row.absolute-position.y, tray-cpu-row.height); }
                }
            }
```

`crates/app/ui/main.slint`: add `in-out property <bool> settings-tray-cpu: true;` and `callback tray-cpu-toggled(bool);`. In the `SettingsPage` block add `tray-cpu: root.settings-tray-cpu;` and `tray-cpu-toggled(on) => { root.tray-cpu-toggled(on); }`.

`crates/app/src/main.rs`:

- After `controls.set_refresh_ms(initial.refresh_ms);` add:

```rust
    // The tray icon's CPU graph (crate::tray): drawn on this thread from the worker's samples; its
    // size and colours are read here (window setup) and the colours again on a theme change.
    negotium::tray::install(negotium::tray::TrayView::new(
        initial.tray_cpu,
        platform::tray_icon_px(),
        negotium::tray::colors_now(platform::read_accent(), platform::taskbar_is_light()),
    ));
    controls.set_tray_cpu(initial.tray_cpu);
```

- Add `tray_cpu_changed: Rc::new(negotium::tray::set_enabled),` to the `SettingsWiring { … }` literal.
- Before `let _worker = worker::spawn(…)`, add:

```rust
    // Worker -> tray, like frames: one wakeup per empty slot, the newest sample wins.
    let tray_sink: worker::TraySink = {
        let slot: Arc<worker::Latest<negotium::tray::TraySample>> = Default::default();
        Arc::new(move |sample| {
            if !slot.put(sample) {
                return;
            }
            let posted = slot.clone();
            let sent = slint::invoke_from_event_loop(move || {
                if let Some(s) = posted.take() {
                    negotium::tray::on_sample(s);
                }
            });
            if sent.is_err() {
                slot.take(); // no event loop (shutting down): keep the next put posting
            }
        })
    };
```

  and pass `Some(tray_sink)` as `spawn`'s new fourth argument.
- In `make_tray`, after `tray.show()?;`, add `negotium::tray::attach(&tray);`.
- In the `ShellEvent::SettingsChanged` arm, after `platform::on_settings_changed(&ui, theme);`, add:

```rust
                                    negotium::tray::set_colors(negotium::tray::colors_now(
                                        platform::read_accent(),
                                        platform::taskbar_is_light(),
                                    ));
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p negotium` then `cargo clippy --workspace --all-targets -- -D warnings`
Expected: all pass; clippy clean. No test creates a `NegotiumTray`.

- [ ] **Step 7: Commit**

```bash
git add crates/app
git commit -m "feat(app): Show CPU in tray icon: a live CPU graph and tooltip in the tray, also while hidden" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: Where a process's memory goes (`negotium-bench memory`)

**Files:**
- Create: `crates/bench/src/memmap.rs`
- Modify: `crates/bench/src/main.rs` (`mod memmap;`, the `memory` subcommand, `memory` in Negotium reports)
- Modify: `crates/bench/Cargo.toml` (features `Win32_System_Memory`, `Win32_System_ProcessStatus`)
- Test: unit test in `memmap.rs`

**Interfaces:**
- Consumes: `running_pid(&str) -> Result<Option<u32>>` (existing in `main.rs`).
- Produces:
  - `memmap::Usage { pub resident_mb: f64, pub private_mb: f64 }`.
  - `memmap::MemoryReport { pub working_set_mb: f64, pub private_working_set_mb: f64, pub by_kind: Vec<(String, Usage)>, pub top: Vec<(String, Usage)> }`.
  - `memmap::measure(pid: u32, top: usize) -> anyhow::Result<MemoryReport>`.
  - CLI: `negotium-bench memory (--pid N | --name EXE) [--top N] [--out FILE]`. It needs no elevation for the user's own processes, and a `negotium` run's report gains `memory`.

- [ ] **Step 1: Write the failing test**: create `crates/bench/src/memmap.rs` with only:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_process_is_broken_down_by_what_backs_its_pages() {
        let r = measure(std::process::id(), 1_000).expect("measure this process");
        assert!(r.working_set_mb > 1.0 && r.private_working_set_mb > 0.0, "{r:?}");
        assert!(r.private_working_set_mb <= r.working_set_mb);
        let resident: f64 = r.by_kind.iter().map(|(_, u)| u.resident_mb).sum();
        assert!((resident - r.working_set_mb).abs() <= r.working_set_mb * 0.25, "the pages add up to the working set: {resident} vs {}", r.working_set_mb);
        assert!(r.top.iter().any(|(name, _)| name.eq_ignore_ascii_case("ntdll.dll")), "modules are named: {:?}", r.top);
        assert!(r.top.windows(2).all(|w| w[0].1.resident_mb >= w[1].1.resident_mb), "largest first");
    }
}
```

and add `mod memmap;` to `crates/bench/src/main.rs` (after `mod stats;`).

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p negotium-bench memmap`
Expected: FAIL to compile (`measure` undefined).

- [ ] **Step 3: Implement**

`crates/bench/Cargo.toml`: add `"Win32_System_Memory",` and `"Win32_System_ProcessStatus",` to the `windows` features.

`crates/bench/src/memmap.rs`, above the tests:

```rust
//! Where a process's memory goes: its working set and private working set, split by what backs
//! each resident page (an executable image, a mapped file, or private memory such as heaps,
//! stacks and graphics-driver allocations), much like VMMap's summary. Attaches to a running
//! process with the same user's rights; no elevation for one's own processes.
use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::HashMap;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Memory::{MEM_COMMIT, MEM_IMAGE, MEM_MAPPED, MEMORY_BASIC_INFORMATION, VirtualQueryEx};
use windows::Win32::System::ProcessStatus::{
    GetMappedFileNameW, K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX2,
    PSAPI_WORKING_SET_EX_INFORMATION, QueryWorkingSetEx,
};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ};

/// x64 pages.
const PAGE: usize = 4096;
/// Pages asked about per QueryWorkingSetEx call.
const CHUNK: usize = 4096;
const MB: f64 = 1_048_576.0;

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Usage {
    /// Resident (in the working set).
    pub resident_mb: f64,
    /// Resident and not shared with another process.
    pub private_mb: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct MemoryReport {
    pub working_set_mb: f64,
    pub private_working_set_mb: f64,
    /// Resident memory by what backs it: "image", "mapped", "private".
    pub by_kind: Vec<(String, Usage)>,
    /// The largest sources (one module, one mapped file, or all private memory), largest first.
    pub top: Vec<(String, Usage)>,
}

struct Process(HANDLE);

impl Drop for Process {
    fn drop(&mut self) {
        // SAFETY: the handle came from OpenProcess and is closed exactly once, here.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

pub fn measure(pid: u32, top: usize) -> Result<MemoryReport> {
    // SAFETY: plain call; the handle is owned by `Process`.
    let p = Process(unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, false, pid) }.with_context(|| format!("open pid {pid}"))?);
    let mut counters = PROCESS_MEMORY_COUNTERS_EX2 { cb: size_of::<PROCESS_MEMORY_COUNTERS_EX2>() as u32, ..Default::default() };
    // SAFETY: `counters` is a PROCESS_MEMORY_COUNTERS_EX2 whose `cb` says so.
    unsafe { K32GetProcessMemoryInfo(p.0, &mut counters as *mut _ as *mut PROCESS_MEMORY_COUNTERS, counters.cb) }
        .ok()
        .context("GetProcessMemoryInfo")?;
    // label -> (kind, resident bytes, private bytes)
    let mut sources: HashMap<String, (&'static str, u64, u64)> = HashMap::new();
    let mut addr = 0usize;
    loop {
        let mut mbi = MEMORY_BASIC_INFORMATION::default();
        // SAFETY: `mbi` is a valid out buffer of the size passed.
        let got = unsafe { VirtualQueryEx(p.0, Some(addr as *const _), &mut mbi, size_of::<MEMORY_BASIC_INFORMATION>()) };
        if got == 0 {
            break;
        }
        let (base, len) = (mbi.BaseAddress as usize, mbi.RegionSize);
        if mbi.State == MEM_COMMIT {
            let (resident, private) = resident_bytes(&p, base, len)?;
            if resident > 0 {
                let (kind, label) = label_of(&p, &mbi);
                let e = sources.entry(label).or_insert((kind, 0, 0));
                e.1 += resident;
                e.2 += private;
            }
        }
        match base.checked_add(len) {
            Some(next) if next > addr => addr = next,
            _ => break,
        }
    }
    let usage = |resident: u64, private: u64| Usage { resident_mb: resident as f64 / MB, private_mb: private as f64 / MB };
    let by_kind = ["image", "mapped", "private"]
        .into_iter()
        .map(|kind| {
            let (r, p) = sources.values().filter(|s| s.0 == kind).fold((0, 0), |(r, p), s| (r + s.1, p + s.2));
            (kind.to_string(), usage(r, p))
        })
        .collect();
    let mut all: Vec<(String, Usage)> = sources.into_iter().map(|(label, (_, r, p))| (label, usage(r, p))).collect();
    all.sort_by(|a, b| b.1.resident_mb.total_cmp(&a.1.resident_mb));
    all.truncate(top);
    Ok(MemoryReport {
        working_set_mb: counters.WorkingSetSize as f64 / MB,
        private_working_set_mb: counters.PrivateWorkingSetSize as f64 / MB,
        by_kind,
        top: all,
    })
}

/// Resident and resident-private bytes of the committed region at `base`.
fn resident_bytes(p: &Process, base: usize, len: usize) -> Result<(u64, u64)> {
    let pages = len / PAGE;
    let (mut resident, mut private) = (0u64, 0u64);
    for first in (0..pages).step_by(CHUNK) {
        let n = (pages - first).min(CHUNK);
        let mut info: Vec<PSAPI_WORKING_SET_EX_INFORMATION> = (0..n)
            .map(|i| PSAPI_WORKING_SET_EX_INFORMATION { VirtualAddress: (base + (first + i) * PAGE) as *mut _, ..Default::default() })
            .collect();
        // SAFETY: `info` holds `n` entries; the call fills in their attributes.
        unsafe { QueryWorkingSetEx(p.0, info.as_mut_ptr() as *mut _, (n * size_of::<PSAPI_WORKING_SET_EX_INFORMATION>()) as u32) }
            .context("QueryWorkingSetEx")?;
        for e in &info {
            // SAFETY: the attributes union read as its plain `Flags` word (bit 0 valid, bit 15 shared).
            let flags = unsafe { e.VirtualAttributes.Flags };
            if flags & 1 != 0 {
                resident += PAGE as u64;
                if flags & (1 << 15) == 0 {
                    private += PAGE as u64;
                }
            }
        }
    }
    Ok((resident, private))
}

/// The region's kind and the source it is counted under: an image's or mapped file's name, or
/// all private memory together.
fn label_of(p: &Process, mbi: &MEMORY_BASIC_INFORMATION) -> (&'static str, String) {
    let kind = if mbi.Type == MEM_IMAGE {
        "image"
    } else if mbi.Type == MEM_MAPPED {
        "mapped"
    } else {
        return ("private", "private (heaps, stacks, driver allocations)".into());
    };
    let mut buf = [0u16; 1024];
    // SAFETY: `buf` is a valid writable buffer; the call writes at most its length.
    let n = unsafe { GetMappedFileNameW(p.0, mbi.AllocationBase, &mut buf) } as usize;
    let name = if n == 0 {
        "unnamed".to_string()
    } else {
        let full = String::from_utf16_lossy(&buf[..n]);
        full.rsplit('\\').next().unwrap_or(&full).to_string()
    };
    (kind, if kind == "mapped" { format!("{name} (mapped)") } else { name })
}
```

`crates/bench/src/main.rs`:

- Directly after `let target = args.next().context(…)?;`, add:

```rust
    if target == "memory" {
        return memory_command(args);
    }
```

- Add this function:

```rust
/// `negotium-bench memory (--pid N | --name EXE) [--top N] [--out FILE]`: where a running
/// process's memory goes (crate::memmap). Needs no elevation for the user's own processes.
fn memory_command(mut args: impl Iterator<Item = String>) -> Result<()> {
    let (mut pid, mut top, mut out) = (None::<u32>, 12usize, None::<String>);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--pid" => pid = Some(args.next().context("--pid N")?.parse()?),
            "--name" => {
                let name = args.next().context("--name EXE")?.to_lowercase();
                pid = Some(running_pid(&name)?.with_context(|| format!("{name} is not running"))?);
            }
            "--top" => top = args.next().context("--top N")?.parse()?,
            "--out" => out = Some(args.next().context("--out FILE")?),
            other => bail!("unknown argument {other}"),
        }
    }
    let report = memmap::measure(pid.context("--pid N or --name EXE")?, top)?;
    let json = serde_json::to_string_pretty(&report)?;
    println!("{json}");
    if let Some(path) = out {
        std::fs::write(&path, json).with_context(|| format!("write {path}"))?;
    }
    Ok(())
}
```

- Add the field to `Report`:

```rust
    /// Where Negotium's memory went at the end of sampling (crate::memmap).
    #[serde(skip_serializing_if = "Option::is_none")]
    memory: Option<memmap::MemoryReport>,
```

- After the `sample(…)` call and before `target.close()?;`, add:

```rust
    let memory = if is_negotium {
        memmap::measure(target.pid, 12).map_err(|e| eprintln!("memory breakdown unavailable: {e:#}")).ok()
    } else {
        None
    };
```

  and add `memory,` to the `Report { … }` literal.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p negotium-bench` then `cargo clippy --workspace --all-targets -- -D warnings`
Expected: all pass; clippy clean.

- [ ] **Step 5: Commit**

```bash
git add crates/bench
git commit -m "feat(bench): a VMMap-style memory breakdown of a running process" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 9: Measure the cost and memory; apply the frame-rate ruling

**Files:**
- Modify: `crates/app/src/settings.rs` (`SETTINGS_ENV`, `path_from`, `default_path`)
- Modify: `crates/app/src/main.rs` (drop `NEGOTIUM_SETTINGS` from the environment after reading it)
- Modify: `crates/bench/src/main.rs` (`NegotiumRun`: `--renderer`, `--smooth`, `--tray-cpu`, `--page`; `cpu_one_core_pct`; `gpu_pct`)
- Modify: `crates/app/tests/render.rs` (a release-only scroll-frame bench)
- Modify: `docs/benchmarks/README.md` (Plan 3c measurements, rulings applied, manual checks)
- Modify, only if the ruling requires it: `crates/app/ui/main.slint` (`scroll-interval`) and its UI test bounds
- Test: `settings.rs` unit test, bench unit test

**Interfaces:**
- Consumes: `memmap::measure` (Task 8), the run report, `negotium_collector::PdhSources`.
- Produces:
  - `settings::SETTINGS_ENV: &str = "NEGOTIUM_SETTINGS"`; `settings::path_from(settings: Option<OsString>, local_app_data: Option<OsString>) -> PathBuf`.
  - Bench report fields `run`, `cpu_one_core_pct`, `gpu_pct`.
  - README tables.

- [ ] **Step 1: Write the failing tests**

`crates/app/src/settings.rs` `mod tests`:

```rust
    #[test]
    fn the_benchmark_can_name_its_own_settings_file() {
        use std::ffi::OsString;
        let local = || Some(OsString::from(r"C:\L"));
        assert_eq!(path_from(Some(r"C:\bench\s.json".into()), local()), PathBuf::from(r"C:\bench\s.json"));
        assert_eq!(path_from(None, local()), PathBuf::from(r"C:\L\Negotium\settings.json"));
        assert_eq!(path_from(Some(OsString::new()), local()), PathBuf::from(r"C:\L\Negotium\settings.json"), "an empty value is no value");
    }
```

`crates/bench/src/main.rs`: add at the end:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_negotium_run_starts_with_its_own_settings_renderer_and_page() {
        let mut run = NegotiumRun::default();
        assert_eq!(run.args(), "--exit-on-close");
        assert_eq!(serde_json::from_str::<serde_json::Value>(&run.settings_json()).unwrap(), serde_json::json!({ "smooth_graphs": true, "tray_cpu": true }));
        assert_eq!(run.backend().unwrap(), None);
        run.renderer = Some("software".into());
        run.smooth_graphs = false;
        run.page = Some("performance".into());
        assert_eq!(run.backend().unwrap(), Some("winit-software"));
        assert_eq!(run.args(), "--exit-on-close --page=performance");
        assert!(run.settings_json().contains("\"smooth_graphs\":false"));
        run.renderer = Some("vulkan".into());
        assert!(run.backend().is_err());
        assert!(on_off("on").unwrap() && !on_off("off").unwrap() && on_off("yes").is_err());
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p negotium --lib settings::tests::the_benchmark` then `cargo test -p negotium-bench`
Expected: FAIL to compile (`path_from`, `NegotiumRun`, `on_off` are undefined).

- [ ] **Step 3: Implement**

`crates/app/src/settings.rs`: replace `default_path` with:

```rust
/// Names a settings file to use instead of the user's: the benchmark starts Negotium with settings
/// of its own that way, never reading or changing the user's.
pub const SETTINGS_ENV: &str = "NEGOTIUM_SETTINGS";

/// The settings file: `%LOCALAPPDATA%\Negotium\settings.json` (the temp directory if
/// `LOCALAPPDATA` is unset), or the file [`SETTINGS_ENV`] names.
pub fn default_path() -> PathBuf {
    path_from(std::env::var_os(SETTINGS_ENV), std::env::var_os("LOCALAPPDATA"))
}

/// [`default_path`] from the two variables' values.
pub fn path_from(settings: Option<std::ffi::OsString>, local_app_data: Option<std::ffi::OsString>) -> PathBuf {
    if let Some(p) = settings.filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    local_app_data.map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("Negotium").join("settings.json")
}
```

`crates/app/src/main.rs`: after `platform::remove_env_var("SLINT_BACKEND");` add `platform::remove_env_var(settings::SETTINGS_ENV);` and extend the comment above it to say "and the bench's settings override".

`crates/bench/src/main.rs`:

- Add:

```rust
/// What a Negotium run starts with (ignored for other targets): its renderer, its own settings
/// file (never the user's) and its page.
#[derive(Clone, Debug, PartialEq, Serialize)]
struct NegotiumRun {
    /// "gpu" (FemtoVG) or "software"; None leaves Negotium's own choice.
    renderer: Option<String>,
    smooth_graphs: bool,
    tray_cpu: bool,
    /// `--page=<name>` (overview, performance, …).
    page: Option<String>,
}

impl Default for NegotiumRun {
    fn default() -> Self {
        Self { renderer: None, smooth_graphs: true, tray_cpu: true, page: None }
    }
}

impl NegotiumRun {
    /// SLINT_BACKEND for the chosen renderer (it overrides Negotium's default and fallback marker).
    fn backend(&self) -> Result<Option<&'static str>> {
        match self.renderer.as_deref() {
            None => Ok(None),
            Some("gpu") => Ok(Some("winit-femtovg")),
            Some("software") => Ok(Some("winit-software")),
            Some(other) => bail!("--renderer gpu|software, got {other}"),
        }
    }

    /// The run's settings file: defaults but for the two switches.
    fn settings_json(&self) -> String {
        serde_json::json!({ "smooth_graphs": self.smooth_graphs, "tray_cpu": self.tray_cpu }).to_string()
    }

    fn args(&self) -> String {
        match &self.page {
            Some(p) => format!("--exit-on-close --page={p}"),
            None => "--exit-on-close".to_string(),
        }
    }
}

fn on_off(v: &str) -> Result<bool> {
    match v {
        "on" => Ok(true),
        "off" => Ok(false),
        other => bail!("expected on or off, got {other}"),
    }
}
```

- In `main`, declare `let mut run = NegotiumRun::default();` before the flag loop, add the arms below, and extend the usage text with `[--renderer gpu|software] [--smooth on|off] [--tray-cpu on|off] [--page NAME]`:

```rust
            "--renderer" => run.renderer = Some(args.next().context("--renderer gpu|software")?),
            "--smooth" => run.smooth_graphs = on_off(&args.next().context("--smooth on|off")?)?,
            "--tray-cpu" => run.tray_cpu = on_off(&args.next().context("--tray-cpu on|off")?)?,
            "--page" => run.page = Some(args.next().context("--page NAME")?),
```

- Replace the `if is_negotium { … NEGOTIUM_FRAME_STATS … }` block and the `launch_args` line with:

```rust
    if is_negotium {
        let settings_path = std::env::temp_dir().join("negotium-bench-settings.json");
        std::fs::write(&settings_path, run.settings_json()).context("write the run's settings")?;
        let backend = run.backend()?;
        // SAFETY: single-threaded at this point; no other thread reads the environment.
        unsafe {
            std::env::set_var("NEGOTIUM_FRAME_STATS", &stats_path);
            std::env::set_var("NEGOTIUM_SETTINGS", &settings_path);
            if let Some(b) = backend {
                std::env::set_var("SLINT_BACKEND", b);
            }
        }
    }
    let launch_args = is_negotium.then(|| run.args());
```

  and change `launch(&path, launch_args)?` to `launch(&path, launch_args.as_deref())?`.
- Change `type Samples` to `(Vec<f64>, Vec<f64>, Vec<f64>, Vec<f64>, usize)` (cpu, private working set, GPU, responsiveness, timeouts). In `sample`, add `let mut pdh = negotium_collector::PdhSources::open().ok();` and `let mut gpu = Vec::new();` beside `cpu`/`ws`, and inside the `if let Some((t0, c0)) = prev {` block, after the CPU push:

```rust
                // The busiest GPU engine the process uses (PDH "GPU Engine", as Task Manager).
                if let Some(by_pid) = pdh.as_mut().and_then(|p| p.read().gpu_pct_by_pid) {
                    gpu.push(by_pid.get(&pid).copied().unwrap_or(0.0) as f64);
                }
```

  return `(cpu, ws, gpu, resp, timeouts)`, and destructure `let (cpu, ws, gpu, resp, timeouts) = sample(…)?;` in `main`.
- Add to `Report`:

```rust
    #[serde(skip_serializing_if = "Option::is_none")]
    run: Option<NegotiumRun>,
    /// `cpu_pct` × logical processors: the share of one core, spec §8's unit.
    cpu_one_core_pct: Summary,
    /// The busiest GPU engine's utilisation attributed to the process, %.
    gpu_pct: Summary,
```

  and to the literal:

```rust
        run: is_negotium.then(|| run.clone()),
        cpu_one_core_pct: summary(&cpu.iter().map(|c| c * logical_cpus() as f64).collect::<Vec<_>>()),
        gpu_pct: summary(&gpu),
```

`crates/app/tests/render.rs`: add a release-only bench of one scroll step under the software renderer:

```rust
/// Software-renderer cost of one scroll step (GraphClock moves, nothing else changes) at
/// 1920 × 1080, scale 1.5: partial repaint (the window's real mode) and full repaint. Reported, not
/// gated. Release only: `cargo test --release -p negotium --test render -- --ignored scroll_step --nocapture`.
#[test]
#[ignore = "bench: run in release with --ignored"]
fn scroll_step_cost_under_the_software_renderer() {
    use slint::platform::software_renderer::{MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType};
    use slint::platform::{Platform, WindowAdapter, WindowEvent};
    use std::rc::Rc;
    use std::time::Instant;
    if cfg!(debug_assertions) {
        panic!("frame times only mean something in a release build");
    }
    struct Software(Rc<MinimalSoftwareWindow>);
    impl Platform for Software {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            Ok(self.0.clone())
        }
    }
    let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
    slint::platform::set_platform(Box::new(Software(window.clone()))).expect("one backend per test thread");
    let ui = MainWindow::new().unwrap();
    ui.show().unwrap();
    window.dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: 1.5 });
    let (w, h) = (1920u32, 1080u32);
    ui.window().set_size(PhysicalSize::new(w, h));
    let mut buffer = vec![PremultipliedRgbaColor::default(); (w * h) as usize];
    let spark = negotium::graph::line(samples(82, 3, Load::Smooth), 81_000, 60_000, 100.0, 60);
    let meters: Vec<negotium::MeterData> = ["CPU", "Memory", "Disk", "GPU", "Network"]
        .into_iter()
        .map(|l| negotium::MeterData { label: l.into(), value: "12%".into(), sub: "".into(), spark: spark.clone().into(), available: true })
        .collect();
    ui.set_meters(ModelRc::new(VecModel::from(meters)));
    ui.set_perf_graphs(ModelRc::new(VecModel::from(vec![busy_graph("% Utilization", 300, 1, Load::Smooth)])));
    let cells: Vec<GraphData> = (0..32).map(|i| busy_graph(&format!("CPU {i}"), 60, i + 7, Load::Smooth)).collect();
    ui.set_perf_cells(ModelRc::new(VecModel::from(cells)));
    for (page, name) in [(0, "Overview"), (1, "Performance, 32 cells")] {
        ui.set_page(page);
        let mut step = |i: u32| {
            ui.invoke_scroll_from(-1_000.0 + i as f32 * 20.0, 1.0);
            window.request_redraw();
            let start = Instant::now();
            window.draw_if_needed(|r| {
                r.render(&mut buffer, w as usize);
            });
            start.elapsed().as_secs_f64() * 1e3
        };
        step(0); // first paint
        let mut frames: Vec<f64> = (1..40).map(&mut step).collect();
        frames.sort_by(f64::total_cmp);
        println!("{name}: scroll step, partial repaint: median {:.2} ms, p95 {:.2} ms", frames[frames.len() / 2], frames[frames.len() * 95 / 100]);
    }
}
```

(A full-repaint figure is the existing `performance_page_resize_frames_fit_in_16_ms` bench; record both.)

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p negotium` then `cargo test -p negotium-bench` then `cargo clippy --workspace --all-targets -- -D warnings`
Expected: all pass; clippy clean.

- [ ] **Step 5: Commit the tooling**

```bash
git add crates/app/src/settings.rs crates/app/src/main.rs crates/bench crates/app/tests/render.rs
git commit -m "feat(bench): runs with their own settings, renderer and page; one-core CPU and GPU %" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

- [ ] **Step 6: Run the headless scroll-step bench (needs nothing from the user)**

Run: `cargo test --release -p negotium --test render -- --ignored scroll_step --nocapture` then `cargo test --release -p negotium --test render -- --ignored performance_page_resize --nocapture`
Expected: two medians each; note them for the README.

- [ ] **Step 7: Stop and ask for the measurement runs**

Ask your human partner, in these words or close: "Task 9 needs about ten minutes of benchmark runs. They launch Negotium itself, so your running Negotium has to be closed first (tray → Exit), and they need an elevated terminal. Shall I go ahead, or would you rather run the commands yourself?" Wait for a clear yes. Never stop their instance yourself.

- [ ] **Step 8: Run the measurements** (elevated terminal, repository root, no Negotium running, desktop idle)

```
cargo build --release -p negotium -p negotium-bench
target\release\negotium-bench.exe negotium --runs 1 --sample-secs 60 --renderer gpu --smooth off --out docs\benchmarks\3c-gpu-overview-off.json
target\release\negotium-bench.exe negotium --runs 1 --sample-secs 60 --renderer gpu --smooth on --out docs\benchmarks\3c-gpu-overview-on.json
target\release\negotium-bench.exe negotium --runs 1 --sample-secs 60 --renderer gpu --smooth off --page performance --out docs\benchmarks\3c-gpu-perf-off.json
target\release\negotium-bench.exe negotium --runs 1 --sample-secs 60 --renderer gpu --smooth on --page performance --out docs\benchmarks\3c-gpu-perf-on.json
target\release\negotium-bench.exe negotium --runs 1 --sample-secs 60 --renderer software --smooth off --out docs\benchmarks\3c-sw-overview-off.json
target\release\negotium-bench.exe negotium --runs 1 --sample-secs 60 --renderer software --smooth on --out docs\benchmarks\3c-sw-overview-on.json
```

Expected: six JSON files, each with `cpu_one_core_pct`, `gpu_pct`, `private_working_set_mb` and `memory`, plus `frame_stats` for the GPU runs.

- [ ] **Step 9: Apply the pre-committed ruling (P3c-3)**

Let Δ = `cpu_one_core_pct.mean` (smooth on) − `cpu_one_core_pct.mean` (smooth off) for the GPU runs.

- **Within budget** (Δ ≤ 0.5 on the Overview and Δ ≤ 1.5 on Performance): keep the code as it is.
- **Over budget, first step (applied in Task 9; as built the factor is in `smooth::redraw_hz`, not `scroll-px`):** in `crates/app/ui/main.slint` change `root.scroll-px * 2` to `root.scroll-px * 1` (whole-pixel steps). In `graphs_are_redrawn_as_often_as_half_a_pixel_of_scrolling_needs_up_to_60_a_second`, change the bounds to `200..=800` and `50..=160` and the test name to `…as_often_as_a_pixel_…`. Rerun the four GPU runs.
- **Still over, second step:** also change the `16` floor to `33` (≈ 30 fps) and the 5120 px assertion to `33`. Rerun.
- **Still over:** stop and report the numbers to your human partner. Do not turn the default off.

Commit any change as `perf(app): fewer scroll redraws to meet the Plan 3c budget`.

- [ ] **Step 10: Write the README**: add to `docs/benchmarks/README.md`, after "Negotium vs Task Manager", a section with this structure (numbers from the JSON files; "—" where a renderer reports nothing):

```markdown
## Plan 3c: smooth graphs, tray CPU icon and memory

Measured with `negotium-bench` (release, elevated terminal, no other Negotium running), 1000 × 720
window, idle desktop, `--runs 1 --sample-secs 60` after the 5 s warm-up; each run starts Negotium
with its own settings file (`NEGOTIUM_SETTINGS`), so the user's settings are never read or changed.
CPU is the share of one core (`cpu_one_core_pct`), GPU the busiest engine attributed to the
process (`gpu_pct`), frames and render time from `frame_stats` (GPU renderer only).

| Renderer | Page | Smooth graphs | CPU % of one core, mean (max) | GPU %, mean | Frames / render ms mean |
|---|---|---|---|---|---|
| GPU | Overview | off | | | |
| GPU | Overview | on | | | |
| GPU | Performance (1 min) | off | | | |
| GPU | Performance (1 min) | on | | | |
| Software | Overview | off | | | — |
| Software | Overview | on | | | — |

Software renderer, one scroll step at 1920 × 1080 × 1.5 (`scroll_step_cost_under_the_software_renderer`):
Overview median … ms, Performance with 32 cells median … ms; full repaint (`performance_page_resize_frames_fit_in_16_ms`) median … ms.

Budget (ruling P3c-3): smooth graphs may add at most 0.5 % of one core on the Overview and 1.5 % on
the Performance page. Result: <within budget | the rate was lowered to … and re-measured>.

### Memory, idle on the Overview

| Renderer | Working set MB | Private working set MB | Image / mapped / private resident MB |
|---|---|---|---|
| GPU | | | |
| Software | | | |

Largest sources (from `memory.top`, GPU run): … (one line each: name, resident MB, private MB, what it is).
What this says against spec §8's 60 MB: … Cheap win applied: <none | what, saving>. Follow-ups: ….
```

When filling it in, replace every `…` and `<…>` with the measured value or the outcome. Then add the manual checks:

```markdown
## Manual checks: Plan 3c

1. 0.5 s refresh, Performance → CPU, 1 min: the line touches both graph edges on every update, with smoothing on and off; the same on the Overview sparklines.
2. Smooth graphs on: graphs, cells and sparklines glide; the newest point eases in from the right; nothing jumps when switching 1 s ↔ 5 s or device or range.
3. Hovering a graph with a still pointer while it scrolls: the card's time moves on with the line beneath the cursor.
4. Minimise, or close to the tray: Negotium's CPU in Task Manager drops back to its static level (no scrolling while unseen); restoring resumes smoothly.
5. Settings → Smooth graphs off: graphs step once per refresh; the setting survives a restart.
6. Tray: the CPU graph appears in the tray at 100 %, 150 % and 200 % scaling, sharp at each; readable on a light and a dark taskbar and after switching the Windows mode while running; hovering shows "CPU n% · Negotium"; it keeps updating every 2 s while the window is closed to the tray.
7. Settings → Show CPU in tray icon off: the app icon returns at once; on again: the graph returns within a second or two. Survives a restart.
8. Restart Explorer (Task Manager → Windows Explorer → Restart) with the CPU icon on: the icon comes back as a graph.
```

- [ ] **Step 11: Commit the measurements**

```bash
git add docs/benchmarks
git commit -m "docs(bench): Plan 3c cost and memory measurements, rulings applied, manual checks" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Self-review

- **Spec and request coverage:**
  - Edge shift: Task 1, with failing tests that pin the 0.5 s case (P3c-1 gives the evidence).
  - Smooth graphs on by default, with a Settings switch: Tasks 2, 3 and 5.
  - Data collection unchanged: P3c-1 and the Global Constraints.
  - No extra drawing when hidden or minimised: Task 5 (`GraphClock.shown`) and Task 3 (timer gated on page and offset).
  - Scrub consistency: Task 4.
  - Cost measured with a pre-committed ruling: Task 9 (P3c-3, P3c-4).
  - Tray CPU graph with tooltip, DPI size, light and dark taskbar, switch, updates while hidden, stop when off: Tasks 6 and 7.
  - `SystemTrayIcon` runtime change verified in source: P3c-7.
  - Memory measurement: Tasks 8 and 9.
- **Placeholder scan:** code steps carry full code. Task 9's README tables are filled with values measured during execution, and the step names the JSON field for each cell.
- **Type consistency:**
  - `Scroll::{next, right_edge, lead_ms}`, `smooth::lag_ms` and `SCROLL_MARGIN_MS` are used the same way in Tasks 2, 3 and 4.
  - `GraphClock` property names match across `graph.slint`, `main.slint` and the tests.
  - `scrub::scrub` has four arguments everywhere from Task 4 on.
  - `worker::spawn` gains `epoch` in Task 3 and `tray` in Task 7, and `main.rs` follows each change.
  - `TraySample`, `TrayView`, `TrayLook`, `colors_now`, `tray_cpu`, `tray_cpu_changed` and `TraySink` agree between Task 7's interfaces and its code.
  - `MemoryReport` in Task 8 is the type of Task 9's report field.
- **Review Focus:** each of the five lines names its test, in the task that owns the code.
