# Plan 1 (data core) — rulings and follow-ups

Carried out of the SDD ledger for `docs/superpowers/plans/2026-10-14-negotium-data-core.md` when the branch finished.

## Rulings made during execution

- Ruling: R1 — In Task 5 the implementer adds `pub mod memory; pub mod pdh;` to lib.rs in Step 1 (with the test-only pdh.rs) so the RED step fails to compile on missing items instead of silently skipping the unit tests — keeps TDD evidence honest — cost if wrong: none (ordering only).
- Ruling: R2 — If the Task 12 implementer cannot run elevated, it verifies the non-elevated refusal path, leaves the baseline JSON/README table marked "pending: run elevated", commits, and the human partner is asked to run the one command at the end — the plan says to ask the partner and that is not a stop condition — cost if wrong: baseline numbers arrive later than Plan 1's other results.
- Ruling: R3 — plan-mandated GetWindowTextLengthW/GetWindowTextW block on hung windows owned by the scanning process (measured 12 s), violating spec §4.2 "never block". Replace with InternalGetWindowText (no messaging; Taskmgr.exe itself imports it per research notes) into a fixed 512-unit buffer, fix the doc/SAFETY comments, and make the test assert every scan_windows() call returns in < 1 s — spec is binding over plan text — cost if wrong: titles longer than 511 units are truncated (display only).
- Ruling: R4 — fix all three plan-mandated grouping gaps now (spec §4.3 binding): (a) descendants of a packaged window root take Package(pfn), not Window; (b) spec rule 3 implemented generically: a windowless, non-system process collapses to its topmost same-image-name ancestor (valid create-time chain, depth cap) → Background{top}, no hardcoded host list; (c) rule 4 extended: session 0 OR image path under %SystemRoot%\ → Windows. Rule 4 is evaluated before rule 3 so same-name system chains stay in "Windows". Model gains a system_root (from the SystemRoot env var, fallback C:\Windows) via Model::with_system_root for tests; group() takes system_root — cost if wrong: unrelated same-named parent/child processes merge into one Background row; system binaries launched by users from System32 without a window land in "Windows".
- Ruling: R5 — a metric change re-sorts even while frozen (metric change takes precedence over the hover freeze): the user's click on a meter is an explicit request for that ordering and the pointer is on the meter, not the list — cost if wrong: one reorder under a pointer that drifted back onto the list within the same tick.
- Ruling: R6 — fix now with per-occurrence reference counts released on eviction plus a free list for id reuse; skip the name clone when unchanged; add a churn test asserting the tables stay bounded — spec §5.5 is binding over the plan's interning design — cost if wrong: small bookkeeping overhead per push.
- Ruling: R7 — address the README finding by measuring rather than rewording only: commit a small `crates/collector/examples/tick_breakdown.rs` that times the NtQuerySystemInformation call separately from parsing, plus PDH, window scan, identity and memory, under the same 60 × 1000 ms configuration as the headline run; README cites it and states only what it measured — Plan 2's performance decisions need this split and a reproducible harness — cost if wrong: one extra example file outside the plan's file list.
- Ruling: R8 — fix now (the human's first elevated run is the first real exercise of these paths): (a) a launched-target guard whose Drop posts WM_CLOSE when a window is known, waits up to 5 s, then OpenProcess(PROCESS_TERMINATE)+TerminateProcess, disarmed after a clean close; (b) process-list read errors in the running check and exit wait are errors, not "not running"/"exited"; (c) in the responsiveness probe only ERROR_TIMEOUT counts as a stall — any other SendMessageTimeoutW failure ends sampling with an error — "must not leave it running" constraint is binding; misreported timeouts would corrupt the baseline — cost if wrong: Ctrl+C during sampling still leaves Task Manager open (no console handler; documented).
- Ruling: R9 — Important 3 (ghost windows may hide hung windows; unverified) is deferred to Plan 2 as a required verification before Force close is built, not fixed in this branch — it needs an interactive hung-window experiment and possibly an undocumented API — cost if wrong: hung detection may miss frozen captioned apps until Plan 2.

## Measurements

- Task 11: measurement: collector tick median 18.7–19.1 ms (target ≤ 5 ms) on ~680 processes; NtQuerySystemInformation ≈15–17 ms, PDH ≈5.3 ms, window scan ≈1 ms. Spec §8 says targets are revisited against the baseline before pass/fail — carry to final summary and Plan 2 (own-CPU target ≤0.5 % of a core at 1 s implies ≤5 ms/tick).
- Task 11: measurement (tick_breakdown example, 674 procs): nt_call 14.96 ms median ≈ process_read 14.49 (parse ≈ 0, derived), pdh 6.11, windows 1.23, identity 0.13, memory 0.003, tick 22.35 — kernel process query dominates

## Notes for Plan 2

- packaged system components (Start/Search) show as App rows
- row key instability (tray flip, renamed children)
- PDH double formatting per tick
- spec §9 memory accuracy test missing
- HistorySample lacks per-disk/per-adapter series and time-based capacity
- Collector must be built on its thread
- per-core index != core id
- Hyper-V net double count
- resolve hung-pin vs frozen tension
- Ghost windows (R9): verify hung detection with an on-screen captioned window that receives input before building Force close.
- Bench: handle-ownership, warm-up and probe-error paths have only been compile-verified; first elevated run is their first real test.

## Deferred minor findings (triaged "can wait" by the final review)

- Task 1: minor (deferred): JSON round-trip test does not cover Some(gpu_pct_by_pid)/Some(memory)/Some(disks) (BTreeMap<u32,_> keys as strings)
- Task 1: minor (deferred): plan-mandated test lines exceed rustfmt width; repo has no rustfmt.toml
- Task 1: minor (deferred): license = "MIT" with no LICENSE file
- Task 2: minor (deferred): ImageName slice not bounds-checked against buffer (processes.rs:130-135)
- Task 2: minor (deferred): later-entry 8-byte alignment assumed, not asserted (processes.rs:125)
- Task 2: minor (deferred): layout test checks size only; add offset_of! asserts for read fields
- Task 2: minor (deferred): retry loop unbounded / len cast to u32 could truncate (unreachable in practice)
- Task 2: minor (deferred): cpu-growth integration test timing could flake under heavy load
- Task 2: minor (deferred): unused windows features in collector Cargo.toml until later tasks (plan-mandated)
- Task 3: minor (deferred): 1024-unit image path buffer; long paths cached as None (retry with 32768 on ERROR_INSUFFICIENT_BUFFER)
- Task 3: minor (deferred): PID-reuse race between enumeration and OpenProcess (query ignores create_time)
- Task 3: minor (deferred): opens() counts cache misses not OpenProcess calls; no doc comment
- Task 3: minor (deferred): manual CloseHandle, no RAII guard
- Task 3: minor (deferred): no packaged-process positive test; no access-denied-PID test
- Task 4: minor (deferred): cloaked / zero-size / WS_EX_TOOLWINDOW windows not filtered (may misclassify app roots)
- Task 4: minor (deferred): dev-dependency windows features duplicate [dependencies]; lib.rs mod/use ordering nit
- Task 5: minor (deferred): one SAFETY comment covers two unsafe blocks in read_array (pdh.rs:142-145; plan-mandated)
- Task 5: minor (deferred): read() failure path (None + error) untested; integration test doesn't assert errors empty
- Task 5: minor (deferred): CStatus filter can shorten per_core_pct so indices shift — Plan 2 per-core grid must not assume index == core
- Task 5: minor (deferred): invalid UTF-16 instance names become ""; net_bps double-counts vEthernet + physical NIC on Hyper-V hosts
- Task 6: minor (deferred): failure-isolation paths untested (no seam; pdh field is crate-private so Err(..) test is cheap)
- Task 6: minor (deferred): transient processes-read failure evicts whole identity cache (skip retain_alive on Err) — plan-mandated
- Task 6: minor (deferred): PDH open failure is permanent for the session (no retry) — plan-mandated
- Task 6: minor (deferred): Identity clone per process per tick (~2N String allocs) — check in Task 11 timing
- Task 6: minor (deferred): taken_at_ms stamped before reads; window pids may be absent from processes
- Task 6: minor (deferred): source_errors prefix convention undocumented; test sleep/slack; lib.rs mod ordering nit
- Task 7: minor (deferred): disk-rate clamp unreachable/untested; combined backwards+zero-elapsed test; finite_clamp NaN branch untested; serde dep unused (plan-mandated)
- Task 8: minor (deferred): depth-64 cap untested; no explicit self-parent test; console apps split (conhost owns window, shell parent is explorer); clamps untested; strip_exe misses ".Exe"; Model::update O(rows × processes) display_name scan
- Task 8: minor (deferred): host_root create-time/cycle guards untested; no shell-host stop in host_root; access-denied image_path in user session can't match rule 4
- Task 9: minor (deferred): no test for R5 (metric change while frozen); ranking.rs module doc still says "never while frozen"
- Task 9: minor (deferred): frozen test keeps only one old row; no hung-change-while-frozen test; no max-over-processes hung test
- Task 9: minor (deferred): Vec::contains in frozen path is O(n·m) (use HashSet)
- Task 9: minor (deferred): hung/metric changes bypass 2 s cap; newly hung app not pinned while frozen (plan-mandated)
- Task 9: minor (deferred): HungTracker keyed by pid only; transient empty window scan clears hung timers; empty row set resorts every call
- Task 10: minor (deferred): budget test exercises only CPU/Memory top lists (full load ≈2.4 MB computed; 64-core ≈3.1 MB); approx_bytes undercounts allocator/HashMap overhead and preallocation
- Task 10: minor (deferred): only Metric::Cpu asserted in tests; no truncation/totals/since threshold tests; at()/since() linear; equidistant → earlier sample; f32 storage for memory bytes
- Task 10: minor (deferred): churn test doesn't cover an app shared across lists or reappearing after release
- Task 11: minor (deferred): --timing without --ticks never prints; BufWriter not flushed explicitly; rough arg errors (Debug output, no flag name, no --help, --ticks 0 runs once); upper-median; accuracy test load-sensitive; unused snapshot dep in probe; README lacks observed accuracy value
- Task 11: minor (deferred): example asserts status >= 0 instead of regrowing; discards pdh result
- Task 12: minor (deferred): probe period 100 ms + RTT; CPU sampling includes post-launch warm-up; short runs report 0 % instead of no data; percentile test name says nearest-rank; commit subject claims baseline recorded; no CoInitializeEx; hard-coded System32 path
- Task 12: minor (deferred, TRIAGE BEFORE MERGE): Drop terminates by PID without re-checking existence when PostMessageW fails — PID reuse could kill an unrelated process from an elevated bench (fix: keep hProcess in the guard and terminate through it)
- Task 12: minor (deferred): pid-0 path bails without TerminateProcess on a non-null handle; WM_CLOSE to a recycled hwnd (pre-existing)
