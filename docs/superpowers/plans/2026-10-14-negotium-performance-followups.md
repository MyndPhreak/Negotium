# Plan 3a (performance pages) — rulings and follow-ups

Carried out of the SDD ledger for `docs/superpowers/plans/2026-10-14-negotium-performance.md` when the branch finished.

## Rulings made during execution

- Ruling: PF1 (T11) — sync_current only syncs properties; ensure-row-visible is called only from list_key Move/Toggle and set_list_focus(true) — a wheel-scrolled focused list must not snap back every tick — cost if wrong: none.
- Ruling: PF2 (T2/T4/T7) — disk response counter is optional: disks built from (idle, read, write) as before, response passed as response.as_deref().unwrap_or(&[]); DiskSample::response_ms: Option<f32>; T7 shows "—" for None — spec §7 per-source independence binding — cost if wrong: none.
- Ruling: PF3 (T10, T4/worker) — show_scrub uses try_lock and keeps the current card on contention; LiveSource::new resets a poisoned history and calls clear_poison — UI thread must never block (Global Constraints) — cost if wrong: scrub card may lag one hover event under contention.
- Ruling: PF4 (T6) — Memory "In use" stat equals the composition bar's in-use segment (+compressed) when page lists exist, else `used`; update the test literal — one number per label — cost if wrong: differs slightly from Task Manager's "In use" if TM uses `used`.
- Ruling: PF5 (T8) — drop the duplicate AtomicU8 import in controls.rs; Frame literal at end of build_frame gets ..Default::default() (or explicit page/perf) — compile fix — cost: none.
- Ruling: PF6 (T1) — RegGetValueW pcbdata Some(&mut size as *mut u32); GetSystemFirmwareTable Some(buf.as_mut_slice()) per windows 0.62.2 — compile fix — cost: none.
- Ruling: PF7 (T10) — don't import ScrubRow in ui.rs — warning-free — cost: none.
- Ruling: PF8 (T6→T7) — PerfInput::max_over moves to T7 where first used — no dead_code warning — cost: none.
- Ruling: PF9 (T7) — disconnected adapters get pages, listed after connected ones (marked "Disconnected") — spec §5.2 "each network adapter" binding — cost if wrong: a longer device list on machines with many virtual adapters.
- Ruling: PF10 (T6) — CPU temperature shows "—" with a plain note; the "Install CPU temperature add-on" link arrives with Plan 4's add-on (a link to nothing would mislead) — cost if wrong: §7 link missing until Plan 4.
- Ruling: PF11 (T6) — Processes/Threads/Handles show "—" when the process read failed (procs empty) — spec §7 — cost: none.
- Ruling: PF12 (T4, T3, T7) — no per-tick String allocation: History::record looks series columns up by &str and allocates a key only on first sight; sorts compare borrowed strings — spec §8 — cost: none.
- Ruling: PF13 (T1, T8/T9, T11, T6) — remove verbatim duplication: T1 one generic get<T> for get_array/get_value; T8/T9 one shared Slint focusable-button component for meter-style focus/activation; T11 list menu and row menu share one component, list_focus/row_focus share a deferral helper; T6 view's Memory meter uses gb_of — review rubric — cost: none.
- Ruling: PF14 (T3) — GPU temperature integration test also asserts temps.len() <= gpus.len() and unique keys — no test that asserts nothing — cost: none.
- Ruling: PF15 (T9) — detail layout holding a repeater gets max-width: 16384px — Global Constraints — cost: none.
- Ruling: R1 — fix plan-mandated Important (GetUnicastIpAddressTable failure swallowed → return a warning from read_interfaces and push it to source_errors) plus Minors: unknown-instance refresh churn (remember still-unknown names after a refresh, exclude until REFRESH); deterministic tie-break on duplicate descriptions (prefer hardware); skipped non-finite read/write/recv items stay missing, not 0 (Option in the sample, or drop to None consistent with PF2) — spec §7 binding — cost if wrong: none.
- Ruling: R2 — finding 4 kept as documented (missing read/write item counts 0, disk/adapter missing its idle/sent item is dropped) — changing throughput fields to Option would ripple through Tasks 4-7 interfaces for a rare PDH glitch — cost if wrong: a one-tick 0 in a read/write graph.
- Ruling: R3 — fix plan-mandated Important: GPU samples seeded from DXGI info for adapters missing from PDH engine data (util None/"—"), so temperature and memory survive an engine-counter failure; temps with no matching sample attach to the seeded sample; plus Minors: stable Overview tie-break (prefer DXGI order) and skip re-querying adapters whose temperature query failed (report once) — spec §7 binding — cost if wrong: an extra GPU row with "—" utilization in a VM.
- Ruling: R4 — pause-break threshold PAUSE_BREAK_MS = 10 s instead of ~2 s — hidden tick is 2 s and Plan 3b offers a 5 s refresh, so 2 s would dot normal data; 10 s still breaks sleep/hibernate — cost if wrong: a 3–10 s collector stall draws as a straight segment
- Ruling: R5 — accept In use stat = in-use segment + compressed (matches Task Manager's 'In use (Compressed)' where the bar shows compressed separately) and the 301/61 column cap from absolute-time alignment — cost if wrong: stat and segment differ by the compressed amount
- Ruling: R6 — accept MemoryTotals::in_use()/available() in snapshot as the single memory source (history MemUsed and Overview meter follow it) — one consistent set of memory numbers across pages — cost if wrong: Overview memory % differs slightly from the OS 'used' figure by modified pages
- Ruling: R7 — main graph takes the spare height (min-height 180px, vertical-stretch 1; cells/stats/details vertical-stretch 0) with a layout test at a tall window; cell label elides but value never does (label/value HorizontalLayout, value in accessible label) with a long-engine-name test — spec §5.2 "large graph" + Review Focus 3 binding — cost if wrong: on short windows the graph shrinks to its 180px min and details scroll.
- Ruling: R8 — fix plan-mandated Important #1: hold the history lock only for push (short scope); take it again for the build; reset on poison only if a push was interrupted (pushing flag) — so a view-building panic never wipes history; Important #2: card flip decided in pixels and x clamped to the graph, rows capped to fit the graph height (or card hosted at page level), with a UI test at the minimum window; Minors folded: card accessible-label = scrub title; tests save/restore the previous panic hook; collector test uses checked_sub for the epoch — spec §4.4 history survives restarts — cost if wrong: none.
- Ruling: R9 — fix Important #1: after the missed-focus backstop re-asserts focus, a keypress never performs an action (map Action→Move(current)/Nothing) + test; also fold cheap Minors: key-menu ensures current row visible before opening; accessible-item-selected gated on list focus; shared row-height property — never end an app that wasn't highlighted — cost if wrong: one extra keypress after a missed focus edge.
- Ruling: R10 — fix pre-existing clippy lints so clippy -D warnings passes (separate 'chore: clippy clean' commit, scoped allows only) — the brief's Step 1 requires it — cost: none. Dispatched implementer ac051a2d32d0e0645 (sonnet) BASE 2ce5a0d
- Ruling: R11 — one fix wave: I1 monotonic id counter shared across restarts (old ids become inert exited placeholders) + restart test; I2 scrub resolves the whole column on the absolute grid and picks the sample with the page's main-series max + 1 h mid-column spike test; minors 3,4 (keep requested device ~5 s with "—" before falling back),5 (set_page leaving Overview clears key focus),6 (pct/pct1 non-finite → "—"),8 (forget fake handle); 7 documented in README (per-device cost; tops dominate) rather than restructured — R9–R11 safety and spec §5.2 binding — cost if wrong: a removed device's page lingers ~5 s showing "—".
- Ruling: cosmetic docs, fix with Plan 3b's docs task — cost: one paragraph renders merged
- Ruling: negligible, defer — cost: leftmost-edge scrub may name a sample just outside the drawn window
- Ruling: cosmetic, defer — cost: brief duplicate row

## User reports

- User report (2026-10-15): (1) the app window occasionally pops up — implementers' live checks launching negotium.exe; standing instructions now forbid launching the real app unless the brief's step requires it. (2) wants a sidebar — the navigation rail is Task 8. (3) CPU/RAM meter graphs "expand a second after the app opens" — layout jump on first frames; investigate (systematic-debugging) after Task 7, read-only first, fix carried into Task 8 or its own fix commit.
- User report (3) root cause: MetersRow and TopStrip lack vertical-stretch: 0, so spare height shifts to meters when tiles arrive (meter h110→303) — fix carried into Task 8 with a UI test (verified on a scratch copy, 21/21 UI tests pass)

## Final review

- Final review: dispatched (opus) over da06c88..91beceb; 290 tests pass
- Final review: With fixes — I1 worker restart renumbers row ids under surviving UI state (wrong-app risk); I2 scrub on 10 min/1 h reports column-end sample, not the drawn max; minors: README item 6 wording, one-tick device absence loses selection, leaving Overview keeps list key focus (Narrator path), format::pct NaN→"0%", history budget exceeds 2 MB on big machines, GPU temp test drops fake handle
- Final: fix wave (8 commits 91beceb..585e0a5; 299 tests, clippy/release clean); concerns: I2 sub-2.5 s columns fall back to nearest sample within 2.5 s; held device page shows '—' identity stats
- Final: re-review — all 8 findings addressed; no new Critical/Important
- Final review clean.

## Carried and deferred items

- Task 1: minor (deferred → carried to Task 2): pdh read_value/read_array map non-finite to 0 instead of missing (violates None-not-0); compressed_bytes computed only when PDH opens (move out of match)
- Task 1: minor (deferred → carried to Task 7): per_core_pct empty (not N Nones) when PdhSources::open fails — consumer must handle both or fill
- Task 1: minor (deferred): Processor Information instance may be "<NUMA node>,<index>" not "<group>,<index>" — multi-NUMA single-group machines could drop cores to None; verify; SMBIOS test gaps (max across modules, pre-2.7 struct, 0xFFFF); per-core history bytes doubled (Task 4 re-checks budget)
- Task 2: minor (deferred): join_adapters clones strings per tick (steady, not growth); live test doesn't assert a hardware adapter found; show_address ignores DadState / shows APIPA; test gaps (refresh failure, sockaddr_ip, show_address)
- Task 2: minor (deferred): net.rs module doc slightly stale about refresh triggers
- Task 3: minor (deferred): temp handles/adapter list never refreshed after TDR/hot-plug; per-tick String keys in GpuSample/gpu_memory_from and error clones (steady); adapter key hard-codes phys_0; coverage gaps (live PDH key == DXGI key, collector gpu_errors, read_checked failure); format.rs blank line
- Task 3: note: seeded GPU samples carry util_pct NaN (unknown) — carry to Tasks 4/7: history and perf view must treat NaN util as missing ('—'), never 0 or a NaN graph point
- Task 3: minor (deferred): GPU meter can show "Unavailable" with a temperature sub-label (deliberate); util_pct deserialize_with lacks serde(default) (absent field errors); temp open failures still reported every tick (log dedups)
- Task 4: carry → task sharing History behind a mutex (T8/T10): History survives worker restarts, so the collector clock must survive too (shared epoch / with_epoch) or push must treat t < last as a timeline break (clear + record); plus PF3 poison recovery
- Task 4: carry → Task 5: graph paths must break lines where consecutive samples are > ~2×MIN_SPACING_MS apart (sleep/resume), test a large jump
- Task 4: minor (deferred): top_n Vec per kept tick; intern drops freed buffer (clone_from); approx_bytes misses names capacity/free/bucket slack; column count unbounded with churning devices; no view test drives meters through filled History; test gaps (points after wrap, huge step, backwards t); no RED evidence
- Task 5: carry → Task 6 (graph.rs, before hour graphs reuse shape): (a) single-point segment emits a visible stub (L x+0.1 y) + test with ≥2 zero columns asserts L; (b) align columns to an absolute time grid floor(t/w) so graphs scroll without shimmer; (c) strengthen pause test (samples in different columns or exact string); (d) line-only variant so sparklines don't build area strings
- Task 5: minor (deferred): test slice &s.line[..80] may panic; lib.rs mod order
- Task 6: carry → Task 7 (perf.rs/graph.rs touched anyway): stub at right edge draws leftward (x-0.1) when x+0.1 > VIEW_W; drop redundant f64 cast graph.rs:209; Memory headline/graph/Available/In use from one consistent source (In use + Available = total); composition calls in_use() (no duplicate arithmetic); logical_cpus/uptime 0 → "—"; memory unavailable → EMPTY_PATH spark not stale floor line
- Task 6: minor (deferred): 1 s columns can flicker at sample-interval width; loose 2*301 test bound; per-cell shape over full hour span (perf); Device::id alloc per tick; test gaps (memory None, TenMinutes, compressed>in-use)
- Task 7: carry → Task 8 (small, view.rs/snapshot touched there): in_use() saturating adds; Overview memory meter available only when total > 0; probe prints in_use(); disk active_pct/rates NaN → "—" not "0%" if collector can emit NaN
- Task 7: minor (deferred): no-lists branch of in_use counts modified pages (steps if lists intermittent; narrow doc); no lists-bearing test for history MemUsed/Overview; disk id embeds volume letters (letter change → fallback); net/disk pages walk series twice; no RED evidence
- Task 8: carry → Task 9 (touches perf UI / focusbutton): FocusButton accessible default action gated by interactive (+UI test under confirm-visible); tiles onto FocusButton; stale-frame tests for old range and perf-frame-on-overview; page-switch freeze release moved into Bridge and tested
- Task 8: minor (deferred): stale perf content for ~1 tick on return to Performance; live visual check of rail/Performance page → human checklist
- Task 9: minor (deferred): tab-order test; details not keyboard-scrollable; duplicate SR text for composition legend; empty hosts add spacing; edge strokes half-clipped
- Task 9: minor (deferred): fixed 14px gutter even without scrollbar; cell-value test relies on implicit Text accessible label
- Task 10: minor (deferred): cursor line on every large graph at once (intentional); expanded state keyed by IdMap ids can attach to another app after a worker restart (pre-existing); no keyboard scrubbing
- Task 10: minor (deferred): card overlaps cursor line in a narrow middle band; rows 9-10 hidden at min height without indicator; quiet_expected_panics duplicated in worker.rs/ui.rs; UI-thread scrub holds guard (theoretical poison)
- Task 11: minor (deferred): window re-activation re-reveals current row (bends PF1); no test toggling while frozen with pending re-sort/new row; no UI test for re-Tab after current app vanished; focus-on-click untested; live Narrator check → human checklist
- Task 11: minor (deferred): on a re-asserting keypress Enter/arrows still toggle/move the post-reassert row (non-destructive)
