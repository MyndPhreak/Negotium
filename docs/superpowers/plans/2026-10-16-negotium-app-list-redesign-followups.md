# App list redesign — rulings and follow-ups

Plan: `2026-10-16-negotium-app-list-redesign.md`. Spec: `../specs/2026-10-16-negotium-app-list-redesign-design.md`.

## Rulings made during execution
- Task 4's page-down test asserted scroll position from `row_top`; Task 6 added the drawn-rect check once rows had real heights.
- Task 5 added `icon` / `has-icon` to `RowData` in applist.slint (the plan's file list omitted it).
- Task 4 also passed `collapsed` to `nav::navigate`, kept section ids out of `expanded`, and made exited placeholders keep `last_in_card` / `windows` / `expandable`.
- Task 6 guards expanding exited rows (Slint chevron, double-click, `Bridge::toggle`).
- Process rows show the PID (the spec says so; the plan had said PID-less).
- User decision: the top tiles glide only on the graph clock's redraws (no timer of their own).

## Measurements
See `docs/benchmarks/README.md`, "App list redesign". Overview, GPU, % of one core: Plan 3c 3.13 / 3.74 (smooth off / on); after the tile fixes 5.32 / ~7. The final fix wave (icon pushes coalesced, plain cards a single rectangle, flat number cells) is not measured yet.

## Follow-ups
- Measure the final fix wave with `.superpowers/run-list-bench.ps1`; recover the remaining Overview CPU gap (update-frame draw cost scales with visible cards). Bench frame counts include the launch burst; count after warm-up too.
- Slint 1.18.1's winit backend presents each lone redraw twice — worth an upstream issue.
- Minor items deferred by reviews (none blocking):
  - Task 1: compare_rows/compare_procs duplicate key+hung+direction logic (ranking.rs ~216-253)
  - Task 1: compare_procs tests thin (no Name/asc/Network-as-0); no hung-under-Name-sort test
  - Task 1: no captured RED run
  - Task 2: settings.rs imports controls::sort_column_index (persistence depends on UI-state module)
  - Task 2: two column<->index mappings (settings.rs:362-366, controls.rs:63-65) agree by convention
  - Task 2: launch wiring (saved sort -> selected-metric) untested
  - Task 2: import order nit settingspage.rs:487
  - Task 3: held heading count comes from fresh (live) while exited cards still show under it — confirm look in Task 6
  - Task 3: sections partition by key.kind() while AppRow also has kind (view.rs ~418)
  - Task 3: zero-process app would show "0 processes"
  - Task 4: open_section while frozen does not admit inserted expanded apps (cosmetic, unfreeze fixes)
  - Task 4: collapsed survives grouping off/on in a run (untested, acceptable)
  - Task 5: icon arrivals each run a full push_rows (not coalesced; windows rows counted as shown) bridge.rs:157-163
  - Task 5: HICON not RAII-guarded (leak only on a panic) icons.rs:323-326
  - Task 5: serial loader: one hanging path stalls later icons
  - Task 5: no log when the loader thread fails to spawn
  - Task 5: lib.rs module order; test name indirect
  - Task 6: sort headers' Space/Return and new Tab order untested
  - Task 6: exited-row chevron/double-click guard untested on Slint side
  - Task 6: height test measures slots only (not 46 px body / 26 px last strip)
  - Task 6: content-height clamp untested; card numbers hard-code 13px; process-row focus ring radius 4px mismatches strip shape
  - Task 6: every row still gets an empty slot + `if` (layout O(N) per scroll/frame) — Task 7 measures
  - Task 6: PID text not elided; no dedicated test for a scrolled-out focused Force close
  - Redraw fix: each live toggle starts a no-op 450 ms glide (one wasted tick); glide-px over-estimates (capped)
  - Redraw fix: strip_shift matches by id while tiles stay by slot (rank swaps may under-estimate → one-step jump)
  - Redraw fix: `changed live` glides use stale shift (up to 8 no-motion redraws on show/smooth-on)
  - Redraw fix: strip_shift ignores spacing/min-width (approximation); frame-count test has no lower bound; winit_frames model unverified vs real winit
  - Redraw fix: no test shows the real Bridge path actually glides (equal-redraw test would pass if tiles never glided)
  - Redraw fix: missing clock (answers 0) would freeze a glide (unreachable in app); coarse ~3-step glide (user-accepted); shrink pop at refresh <450 ms (pre-existing)
  - Final fix wave: `caseless` differs from `str::to_lowercase` only for a word-final Greek Σ; collapsing an app with the mouse while the current row is one of its processes is unverified.
