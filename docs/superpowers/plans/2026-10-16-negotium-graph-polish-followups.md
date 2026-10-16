# Plan 3c (graph polish) — rulings and follow-ups

Carried out of the SDD ledger for `docs/superpowers/plans/2026-10-16-negotium-graph-polish.md` when the branch finished.

## User decisions

- Setup: branch feat/graph-polish from main 5873c10; spec docs/superpowers/specs/2026-10-14-negotium-design.md; user approved plan (smooth on by default, tray line+fill, edge fix, memory measured)
- Task 9: User decision (2026-10-16): option 1 — accept current cost; smooth graphs stay on everywhere (Performance +6.5 % of a core ≈ 0.3 % of total CPU while visible). Ruling: R10 — P3c-3 closes as 'over budget, accepted by user'; step 2 not applied; option 4 (cheaper cell redraw) noted as a follow-up

## Rulings made during execution

- Ruling: PF1 (T6,T7) — tray graph columns on the absolute grid via graph::column_of against the end column; test two renders 1 s apart shift by whole columns — P3-5 — cost: none
- Ruling: PF2 (T7) — skip the tray icon/tooltip update while the snapshot shows a hung explorer.exe window (Shell_NotifyIcon waits on Explorer; UI thread must not freeze) — spec §4/§1 — cost: tray graph pauses while Explorer is hung
- Ruling: PF3 (T3,T5) — when not scrolling, scroll-from(0,0) and keep a run resting on the newest sample, so restore/re-enable never jumps back — cost: none
- Ruling: PF4 (T2) — speed cap uses due = newest + lag − now (no ~20 ms pause) — cost: none
- Ruling: PF5 (T3,T4) — timing tests inject the clock or allow measured elapsed; no ±50 ms wall-clock slack — cost: none
- Ruling: PF6 (T4) — scrub returns early when the sample under the pointer is unchanged (no rebuild ~22×/s) — cost: none
- Ruling: PF7 (T6,T7,T3) — strengthen vacuous tests: re-created tray compares against an independently built expected look; AA test checks interior line pixels not outline corners; scroll source test asserts behaviour (offset changes over time) not property presence — cost: none
- Ruling: PF8 — T6: 1 px outline below 32 px (P3c-5); T5/T7: tall-window layout test includes the two new switches; T1: graph_end reuses PerfInput::anchor (no duplicate) — cost: none
- Ruling: PF9 (T1) — document in P3c-1 that the 1 h graph may fall short of its left edge by ≤ one 12 s column (history keeps exactly 1 h); don't grow history — cost: a ≤12 s gap at the 1 h left edge
- Ruling: PF10 (T9) — bench children run with LOCALAPPDATA pointed at a temp dir (own settings, marker and log), renderer default gpu; Steps 7–8 hand elevated commands to the user and never elevate — cost: none
- Ruling: PF11 — T9 also measures hidden-in-tray CPU with the tray graph on (spec target ≤0.2 % of a core at 2 s tick) and reports; record ruling that tray icon pixel drawing + Slint's Shell_NotifyIcon on the UI thread is an allowed exception (bounded by PF2); the platform test depending on the small-icon setting computes its expectation from GetSystemMetrics — cost: none
- Ruling: R1 — fix Important (clip horizontally only; floor/ceiling strokes keep full thickness; pixel test with floor line) + fold minors 2 (shared graph::end_of) and 4 (anchor_clock reuses scrub clock helper) — idle floor line is the common view — cost: none
- Ruling: R2 — redraw_hz clamps to [1, 60] (NaN-safe), test asserts 1.0 at 1 h; the half-pixel formula 2·width/span stands (the plan's '22 fps' figure was for ~700 px of drawn width; 1000 px gives ~33/s) — fold minors: NaN-safe clamp in right_edge, tolerance in float equality, a slightly-older-newest continuity test, doc note on ≤12 s catch-up at ≤2× — cost: none
- Ruling: R3 — fix Important (one owner: OnScreen holds Arc<Controls>; hide/show set controls.visible; hidden Cell removed) + minors 2 (positive control in timer test), 3 (OnScreen::on_size seam), 4 (show re-reads is_minimised), 5 (doc comment moved) — single source of truth for 'hidden' — cost: none; minor 6 (sub-turn flip) deferred
- Ruling: R4 — fix Important (pause break by real sample time > PAUSE_BREAK_MS and None as a break, matching main graphs; boundary tests 9 s/11 s) + minors 1 (inset value range by half stroke so 0/100% lines are full width at 16 px; test at 16 px), 2 (fill reaches both edges), 3 (line contrast also ≥3:1 vs outline or outline adjusted), 4 (light-taskbar render check) — consistency with main graphs; legibility at 16 px — cost: none
- Ruling: R5 — keep the brief's setting name tray_cpu (plan interfaces and Task 9 contract use it; my show_cpu_in_tray was a label, not a name) — cost: none. Review dispatched
- Ruling: R6 — gate PF2 on Shell_TrayWnd itself (platform::shell_tray_hung via FindWindowW + IsHungAppWindow, no messages; missing = not hung; pure seam tray_hung(Option<bool>)) replacing explorer_hung; fold minors 2 (icon NIM_MODIFY only when pixels change), 3 (re-read icon size from GetDpiForWindow(Shell_TrayWnd) on attach, or doc the limit), 4 (retry decision as a tested pure helper), 5 (assert shell_hung stored while hidden), 6 (build TraySample in the write closure) — PF2 must protect the call it exists for — cost: none
- Ruling: R7 — fix both Importants (ScratchDir RAII guard declared before Launched; any target whose exe name is negotium.exe gets isolation + tray message) + minors (warn on --renderer for non-Negotium targets; memory --name accepts name with or without .exe; fix set_var SAFETY wording) — PF10 'every mode' — cost: none
- Ruling: R8 — fix both Importants (validate --page against the app's names, reject whitespace; drift guard: after each run check scratch log has no invalid-fields warning and settings.json not renamed to .bad, fail loudly) + minors 4 (one close helper), 5 (record whether a renderer fallback happened from the scratch marker/log, report actual renderer) — measurements must be comparable — cost: none
- Ruling: R9 — fix wave before re-measuring: (1) app list scrollbar policy always-off + thin non-animated indicator bound to viewport; pixel/draw-count test; (2) GraphClock now/anchor from a Rust-supplied clock (no animation-tick() in ui/), source-scan + has_active_animations test; (3) P3c-3 step 1 whole-pixel steps (redraw_hz factor 2→1, tests updated). Then user re-runs the four GPU visible runs. 33 ms floor skipped (no effect at this size) unless still over — cost if wrong: thinner scroll indicator than Fluent's
- Ruling: R10 — P3c-3 closes as 'over budget, accepted by user'; step 2 not applied; option 4 (cheaper cell redraw) noted as a follow-up
- Ruling: R11 — one fix wave for minors 1 (vacuous clock test), 2 (timer interval from speed 1, not re-armed per frame), 3 (wheel over indicator scrolls), 4 (draw-count preconditions), 5 (auto-scale max includes visible lead-in), 6 (README Δ wording + pre-R9 source note), 7 (drop NEGOTIUM_SETTINGS override; bench writes scratch\Negotium\settings.json), 8 (worker test without wall time); 9 documented only
- Ruling: R12 — revert item 5 (e315708): original behaviour only clips a spike flat for ~1 s (values clamped to [0,100], nothing draws above the card), which is better than a ~22 s squashed graph; also rewrap smooth::install doc — cost: a just-left spike is clipped at the top for ~1 lag

## Measurements and final review

- Task 9: user ran measurements (9 JSONs in docs/benchmarks/3c-*.json). Results (cpu_one_core_pct mean / private WS MB):
-   gpu overview off 16.19 / 85 (1693 frames in 60 s even with smooth OFF); gpu overview on 12.96 / 71; gpu perf off 2.75 / 79 (231 frames); gpu perf on 16.09 / 82 (3061 frames, ~51 fps)
-   sw overview off 2.33 / 18.6; sw overview on 2.96 / 18.5
-   hidden gpu tray on 1.27 / 65.7; hidden tray off 0.93 / 64.0; hidden perf tray on 1.12 / 77.5
-   → Performance Δ +13.3 % of a core (budget 1.5) — over; Overview Δ negative because Overview redraws ~28 fps even with smooth off (anomaly, likely a bug); GPU renderer costs ~60 MB vs software ~19 MB; hidden tray graph +0.34 % (collector alone ~0.9 % hidden, above spec 0.2 % before 3c)
- Task 9: investigation — Overview redraw = fluent ListView scrollbar thumb `animate height 150ms` restarting on every row-count change (240 Hz monitor → ~30–36 frames per change); probe 40 frames vs 1 with policy always-off. Second bug: animation-tick() in graph timer and scroll-from sets active-animations flag → one extra identical repaint per timer tick (doubles fps). Perf smooth-on ≈ 2.8 ms CPU/frame (1.27 render, rest present/loop).
- Task 9: re-measure after R9 (cpu_one_core_pct mean / private MB / frames per 60 s): overview off 3.13 / 68 / 119; overview on 3.74 / 69 / 455; perf off 3.16 / 81 / 132; perf on 9.71 / 78 / 1542
-   → Overview fixed (16.2 → 3.1); Δ Overview +0.61 (budget 0.5, marginal); Δ Performance +6.55 (budget 1.5) still over; 33 ms floor has no effect at this size → per P3c-3 stop and report to user (never turn the default off)
- Final review: dispatched (opus) over 5873c10..c11dcc9
- Final: polish wave 9 commits c11dcc9..b78edf4 (item 3 premise wrong: real dead case was wheel while holding the bar; item 5 scale may stay high ≤22 s)
- Final: item 5 reverted (bd64692; perf.rs/view.rs equal c11dcc9 — controller verified), doc rewrap 00b99e5. Final review clean.

## Carried, live-check and deferred items

- Task 2: minor (deferred): overlong doc line in Scroll::next; a slightly older newest snaps back ≤100 ms (bounded)
- Task 3: carry → Task 4: move render.rs doc comment back above render_sparks; structural test asserts GraphClock.offset-ms; ui.rs a_run_cut_short calls smooth::install and asserts offset > -1000; LEAD_MS doc mentions scroll offset; scrub mapping must include offset (risk 5)
- Task 3: carry → Task 5 (MUST): wire GraphClock.shown via set_shown — until then the timer runs ~5 Hz while in tray
- Task 3: carry → Task 9: measure Performance with many core cells (Slint re-parses each Path per render); watch scroll-from anchor staleness (sub-pixel step back)
- Task 4: carry → Task 5: add a follow_scrub test where the edge advances (set_epoch) and the card changes under a still pointer; pin test edges so they don't depend on wall time
- Task 4: carry → Task 9: per-tick scrub query cost at 1 h (try_lock + top_at_max + ScrubView build) if profiling shows it
- Task 4: minor (deferred): Rust/Slint edge differ by ≤1 timer tick (known approximation); no RED captured
- Task 5: live check (human): Win+D / Win+M stop graph motion
- Task 6: carry → Task 7: drop test-only pub columns() or build runs() on it (shared grid code); live check: on a dark taskbar the 3:1-vs-both rule may push a bright accent to mid-grey — user judges look
- Task 7: live checks (human): Explorer restart re-creates live icon; light/dark flip; icon at high DPI; taskbar moved to other-DPI monitor keeps old size until next attach (documented)
- Follow-up: cheaper core-cell redraws on Performance (render cells only when a sample lands)
- Follow-up: lighter collection while hidden (collector ~0.9 % of a core hidden vs spec 0.2 %)
- Follow-up: GPU renderer memory (~65–80 MB private vs software ~19 MB; spec 60 MB)
