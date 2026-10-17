# Per-App Network Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fill the Network column, the "Top network users" tiles and the Performance network pages' hover list with live per-app network use while Negotium runs elevated.

**Architecture:** A new collector source (`crates/collector/src/netetw.rs`) runs an ETW real-time session on the Microsoft-Windows-Kernel-Network provider on its own thread, summing sent/received bytes per PID (loopback excluded); each tick the collector takes the sums, turns them into rates and puts them in `SystemTotals.net_by_pid`. The model maps PIDs to process and app rows (`net_bps`), after which the existing Network column, sorting, bars, tiles and history tops just work. The scrub gains the network list (with an upload/download split for the newest sample).

**Tech Stack:** Rust 2024, `windows` 0.62.2 (`Win32_System_Diagnostics_Etw`), Slint 1.18.1.

**Spec:** `docs/superpowers/specs/2026-10-17-negotium-app-network-design.md`

## Global Constraints

- Per-app network only while elevated; not elevated → Network shows "—" everywhere, no dialogs, the Overview banner explains (its text mentions network use).
- Upload + download summed in the list, tiles, sorting and history; the split only on the hover list for the newest sample. Loopback traffic (IPv4 127.0.0.0/8, IPv6 ::1, and IPv4-mapped loopback) is not counted.
- Per-app traffic is across all adapters; the network pages' hover list says so.
- If the trace session cannot start while elevated, the column stays "—", the reason is logged once (one `source_errors` entry, not every tick), the rest works.
- No leaked sessions: the session has a fixed name (`Negotium-Net`); at start a leftover session of that name is stopped and a new one started; the session is stopped on drop and on exit. Tests use unique names (`Negotium-Net-test-<pid>-<n>`) and always stop them.
- CPU budget: an elevated Overview bench run costs at most 0.5 % of a core more than the last app-list measurement; collector tick median ≤ 5 ms.
- Tests never touch the user's real settings, log or registry, never open the real window; tests that need elevation are `#[ignore = "needs an elevated terminal"]` and handed to the user.
- If `target\release\negotium.exe` is locked, build into a scratch `CARGO_TARGET_DIR`; never rename the exe aside; never close or kill the user's Negotium.

## Review Focus

1. **PID reuse and short-lived processes:** bytes for a PID that exited and was reused between two takes must not be credited to the new process forever; a process that lives less than one tick is simply dropped. (Task 2 test `bytes_follow_the_pid_alive_at_take_time`.)
2. **A leftover session from a crashed run** (same name still running): start must stop it and start fresh, not fail. (Task 2 elevated test `a_leftover_session_is_replaced`, `#[ignore]`.)
3. **Event-rate spikes** (a large download): per-event work stays a hash-map add under a short lock; no allocation per event. (Task 2 test `accumulating_events_does_not_allocate_per_event` — counts map growth only on new PIDs.)
4. **Not elevated or session failure:** `net_by_pid` is `None`, every Network value is "—" (not "0 b/s"), sorting by Network keeps calm ordering with all-equal values. (Task 1 test `no_feed_means_dash_not_zero`.)
5. **Malformed or unexpected events** (unknown event id, short payload): ignored, never panic on the event thread. (Task 2 test `short_or_unknown_events_are_ignored`.)

---

### Task 1: Snapshot and model carry per-process network

**Files:**
- Modify: `crates/snapshot/src/lib.rs`, `crates/model/src/lib.rs`, `crates/app/src/view.rs`
- Test: same files (unit)

**Interfaces:**
- Produces:
  - `#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)] pub struct NetRate { pub sent_bps: f64, pub recv_bps: f64 }` with `pub fn total(&self) -> f64` (in bits per second, like `SystemTotals.net_bps` — check `net.rs`/`format::bit_rate` for the unit the meter uses and match it exactly).
  - `SystemTotals.net_by_pid: Option<BTreeMap<u32, NetRate>>` (`#[serde(default)]`); `None` = no feed.
  - `ProcRow.net: Option<NetRate>`; `AppRow.net_bps: Option<f64>` = sum of its processes' totals when the feed exists (`Some(0.0)` for an app with no traffic), `None` without a feed. Add `AppRow.net: Option<NetRate>` (summed split) for Task 3's hover list.
  - view.rs: process rows show `format::bit_rate(total)` or "—"; app rows already map `net_bps`.

- [ ] **Step 1: Write failing tests:**
  - model: `apps_sum_their_processes_network` (two processes 1000+3000 bps sent/recv → app `net_bps == Some(4000.0)`, split summed), `no_feed_means_dash_not_zero` (`net_by_pid: None` → every `net_bps` is `None`; view shows "—" for app and process rows), `an_app_without_traffic_shows_zero_when_the_feed_runs` (`Some(map)` without its PIDs → `Some(0.0)`).
  - snapshot: round-trip JSON with `net_by_pid`; old JSON without the field still loads.
- [ ] **Step 2:** `cargo test -p negotium-snapshot -p negotium-model -p negotium` — fail.
- [ ] **Step 3:** Implement. Update the stale doc comment on `AppRow.net_bps` ("needs the SRUM source").
- [ ] **Step 4:** `cargo test --workspace` and clippy pass.
- [ ] **Step 5:** Commit `feat(model): per-process network rates reach app and process rows`.

### Task 2: The ETW network feed

**Files:**
- Create: `crates/collector/src/netetw.rs`
- Modify: `crates/collector/src/lib.rs`, `crates/collector/Cargo.toml` (`Win32_System_Diagnostics_Etw` feature), `crates/collector/src/collector.rs`
- Test: `crates/collector/src/netetw.rs` (unit + `#[ignore]` elevated)

**Interfaces:**
- Consumes: `NetRate`, `SystemTotals.net_by_pid` (Task 1).
- Produces:
  - `pub struct NetBytes { pub sent: u64, pub recv: u64 }`
  - `pub struct NetFeed` — owns the session handle, the `ProcessTrace` thread and the shared sums. `NetFeed::start(name: &str) -> Result<NetFeed, NetFeedError>` (stops a leftover session of that name first); `fn take(&self) -> HashMap<u32, NetBytes>` (bytes since the last take, then resets); `Drop` stops the session and joins the thread.
  - `pub enum NetFeedError { NotElevated, Os { call: &'static str, code: u32 } }` with `Display` in plain English ("needs admin rights", "couldn't start the network trace (error N)").
  - Pure, testable parsing: `pub(crate) fn parse(event_id: u16, payload: &[u8]) -> Option<(u32 /*pid*/, Direction, u32 /*bytes*/)>` returning `None` for loopback, unknown ids and short payloads; `enum Direction { Sent, Recv }`. Event ids and payload layouts come from the provider manifest (Microsoft-Windows-Kernel-Network, GUID `{7DD42A49-5329-4832-8DFD-43D979153A88}`): TCP send/recv IPv4 and IPv6, UDP send/recv IPv4 and IPv6. Verify the layouts against the manifest (`wevtutil gp Microsoft-Windows-Kernel-Network /ge /gm` output or TDH) and record the table you used in a doc comment.
  - `Collector`: holds `Option<NetFeed>` started in `with_epoch` only when the process is elevated; each tick converts `take()` into `NetRate` per PID with the tick's elapsed time and sets `totals.net_by_pid`; a start failure is pushed to `source_errors` once.

- [ ] **Step 1: Write failing tests:**
  - `parses_tcp_and_udp_send_and_receive_for_v4_and_v6` (hand-built payloads per the manifest table: pid, bytes and direction come out right).
  - `loopback_is_not_counted` (127.0.0.1 / 127.5.5.5 / ::1 / ::ffff:127.0.0.1 → `None`).
  - `short_or_unknown_events_are_ignored`.
  - `take_returns_bytes_since_the_last_take_and_resets` (feed the accumulator directly, no session).
  - `bytes_follow_the_pid_alive_at_take_time` (the collector's rate step drops PIDs not in the snapshot's process list).
  - `accumulating_events_does_not_allocate_per_event` (the map's capacity only grows on a new PID).
  - `the_start_failure_is_reported_once` (a collector with a failing feed constructor reports one `source_errors` line over several ticks).
  - Elevated, `#[ignore = "needs an elevated terminal"]`: `a_session_starts_and_stops` (a uniquely named session starts, `take()` works after 1 s, drop stops it — `ControlTraceW` query by name then fails) and `a_leftover_session_is_replaced` (start a uniquely named feed, `std::mem::forget` it so the session keeps running, `NetFeed::start` with the same name succeeds; stop it by name at the end).
- [ ] **Step 2:** Run — fail.
- [ ] **Step 3:** Implement: `StartTraceW` with `EVENT_TRACE_REAL_TIME_MODE`, `EnableTraceEx2` for the provider with the keywords for TCP/IP and UDP, `OpenTraceW` + `ProcessTrace` on a dedicated thread (named `negotium-netetw`), the callback calls `parse` and adds under a `Mutex<HashMap<u32, NetBytes>>`; `ControlTraceW(EVENT_TRACE_CONTROL_STOP)` by name before start and on drop; `CloseTrace` then join. Document every `unsafe` like the existing collector code.
- [ ] **Step 4:** `cargo test --workspace` and clippy pass (ignored tests compile).
- [ ] **Step 5:** Commit `feat(collector): live per-process network from the kernel network trace`.

### Task 3: Hover list, banner and wording

**Files:**
- Modify: `crates/app/src/scrub.rs`, `crates/app/src/perf.rs` (newest-sample split), `crates/app/src/worker.rs` (if the split is carried there), `crates/app/ui/overlays.slint` (banner text)
- Test: `crates/app/src/scrub.rs` (unit), `crates/app/tests/ui.rs`

**Interfaces:**
- Consumes: `AppRow.net`, `AppRow.net_bps` (Task 1); history tops for `Metric::Network` already record `value(Network)`.
- Produces:
  - `PerfFrame.net_split: Vec<(String, NetRate)>` — the newest sample's busiest apps (up to the scrub's row count) with their split, filled only for network pages.
  - scrub for network pages: rows from history like other metrics (`metric_text` total); when the hovered sample is the newest (`perf.window_end_ms`) and `net_split` has the app, its value reads `"<total>  ↑<sent> ↓<recv>"` (format with `format::bit_rate`, arrows as plain text); a footnote line `"All adapters"` via `ScrubView` (add `pub note: String`, shown under the rows; empty for other metrics); `empty` = `"Per-app network use needs admin rights."` when the frame says there is no feed (`PerfFrame.net_feed: bool`), else the usual "No app was busy at this moment."
  - Banner default text: `"Some processes need admin rights to show details, network use, or be ended."`

- [ ] **Step 1: Write failing tests:** replace `network_pages_explain_the_missing_per_app_data` with `network_pages_list_apps_from_history`, `the_newest_sample_shows_the_upload_download_split`, `older_samples_show_the_total_only`, `without_a_feed_network_pages_say_admin_is_needed`, `the_network_hover_notes_all_adapters`; ui test `the_admin_banner_mentions_network_use`.
- [ ] **Step 2:** Run — fail.
- [ ] **Step 3:** Implement; show `note` in the scrub popup in `perfpage.slint` (muted, 11 px).
- [ ] **Step 4:** `cargo test --workspace` and clippy pass.
- [ ] **Step 5:** Commit `feat(app): the network pages name their busiest apps`.

### Task 4: Measure and document

**Files:**
- Create: `.superpowers/run-net-bench.ps1` (git-ignored)
- Modify: `docs/benchmarks/README.md`

- [ ] **Step 1:** Write `.superpowers/run-net-bench.ps1` like `.superpowers/run-list-bench.ps1`: build release, then `negotium-bench negotium --runs 1 --sample-secs 60 --renderer gpu --smooth on --out docs\benchmarks\net-gpu-overview-on.json` and the same with `--smooth off` (`net-gpu-overview-off.json`) — the bench runs elevated, so the feed is on.
- [ ] **Step 2:** Hand it to the user (never run elevated yourself). When results are back, add a "Per-app network" section to `docs/benchmarks/README.md`: CPU against the app list's latest numbers (budget +0.5 % of a core), collector tick median (≤ 5 ms), and manual checks: the Network column fills while a download runs (browser or `curl` of a large file) and shows "—" when not elevated; sorting by Network; the Network meter's top tiles; the network page hover with the split on the newest sample and "All adapters"; a crash leaves no session (`logman query -ets` shows no `Negotium-Net` after killing Negotium and starting it again).
- [ ] **Step 3:** Commit `docs(bench): per-app network measurements and manual checks`.
