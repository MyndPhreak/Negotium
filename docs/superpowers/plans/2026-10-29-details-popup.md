# Fuller Details popup and context menu Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the six-line Details text box with a scrollable per-app popup (header, processes, identity, live resources, priority/affinity, network connections, services) and add Search online, Properties, Go to service(s) and Create memory dump file to the row menu.

**Architecture:** Facts that do not change while a process runs (path, command line, user, signature, version, architecture, elevation) are read once, on an action thread, for every process of the app when the popup opens. Live numbers come from the worker: while the popup is open, `Controls` holds a watch (the app's processes and the selected one) and the worker attaches a `DetailsFrame` to every frame it builds, whatever the page. The UI only formats and shows. The `details:` string prefix in `dispatch::run` is replaced by a typed `Outcome`.

**Tech Stack:** Rust, `windows` crate (Win32), Slint 1.18.1, `i_slint_backend_testing`.

**Spec:** `docs/superpowers/specs/2026-10-28-users-details-settings-design.md`, section 2.

## Summary

Today **Details** shows name, PID, path, command line, user and priority of the app's first process in a `Modal` ([dispatch.rs:161](../../../crates/app/src/dispatch.rs), [main.slint:667](../../../crates/app/ui/main.slint)). This plan makes it the one place for everything about an app, and adds the four Task Manager menu items the user kept. The work is one PR on `feat/details-popup`, cut from `main`.

## Global Constraints

- No feature flags in Negotium (user decision): everything ships on.
- Two commits per task: `Add failing tests for <area>` (with compiling stubs, failing on an assertion), then `Implement <area>`.
- UI text follows ASD-STE100. Record any departure as a deviation in the PR.
- A field Windows refuses shows `—` (U+2014). A section never disappears because one field failed.
- No layout needs a wider window than today: the popup is `min(parent.width - 48px, 560px)` wide and scrolls; UI tests run it at the 760 px minimum window width.
- Settings file shape does not change.
- Clippy: `cargo clippy -p negotium -p negotium-actions --all-targets -- -D warnings` stays clean.
- Every Win32 call checks identity first (`open_verified(pid, create_time, …)`), as today.

## Architectural Decisions

| Decision | Reason |
|---|---|
| Static facts read once per process at open, live numbers from the worker. | The worker already holds rates, threads, handles, GPU, network and sockets per tick. Re-opening 30 processes every tick to re-read a path is waste. |
| Watch held in `Controls` (`set_details_watch` / `details_watch`) and a `details: Option<DetailsFrame>` field on `Frame`, built on any page. | Same channel the Ports page uses (`request_ports_refresh`). The popup can open from Overview or Ports. |
| `bridge.apply` applies `frame.details` before its page check. | A frame for a page the user left is dropped today; the popup must still update on the page it was opened from. |
| Process list is the app's processes at open time. Processes that exit show "Exited" and their live cells show `—`. New processes are not added. | `Target` carries `procs`, not an `AppKey`; Ports targets have no key. Cost if wrong: a child started after opening is missing until the popup is reopened. |
| Network connections and services reuse `PortReader` (`sockets`, `last`) in the worker. | Same socket tables and service-by-PID map the Ports page reads, with its refresh interval. |
| Peak memory is read per tick for the selected process only (`peak_working_set`). | Not in the snapshot; one `GetProcessMemoryInfo` per tick is cheap. |
| Signature: embedded Authenticode first, then the system catalog. | Most Windows binaries are catalog-signed; embedded-only would mark them "Not signed". |
| Role hint from the command line (`--type=` of Chromium/Electron) and "Main" for the root. | The only reliable cheap source; dropped (no chip) when nothing matches. |
| `dispatch::run` returns `Outcome` instead of a prefixed `String`. | The popup needs typed data, the dump needs a folder for its toast button. |
| Change priority / affinity in the popup call the same functions the row menu uses, extracted from `on_row_action` into `open_affinity(&Target)` / `open_priority(&Target)`. | Spec: reuse today's dialogs unchanged. The affinity and priority overlays are declared after the details overlay in main.slint, so they draw on top. |
| Search online opens `https://www.bing.com/search?q=<file name>`. | Matches Task Manager. Cost if wrong: one constant. |
| Dump: `MiniDumpWriteDump` with `MiniDumpWithFullMemory | MiniDumpWithHandleData | MiniDumpWithThreadInfo | MiniDumpWithUnloadedModules`, to `%TEMP%\<process>_<pid>_<YYYYMMDD-HHMMSS>.dmp`, on an action thread. | Spec: Task Manager's place and name; the UI must not freeze. |
| "Go to service(s)" is enabled only for rows whose processes host services. The worker keeps a service-PID set refreshed at most every 10 s on the Overview. | Spec: shown only when the process hosts services. `MenuItem` has no `visible`; a disabled item is what Slint offers (ruling: deviation recorded). |
| Toast gains an optional action button (`toast-action` label, `toast-action-clicked()`), used for "Open folder". | Spec: notification with an Open folder button. |

## Files

| File | Change |
|---|---|
| `crates/actions/Cargo.toml` | Add `windows` features: `Win32_System_Diagnostics_Debug`, `Win32_System_ProcessStatus`, `Win32_System_SystemInformation`, `Win32_Security_WinTrust`, `Win32_Security_Cryptography`, `Win32_Security_Cryptography_Catalog`. |
| `crates/actions/src/details.rs` | Extend `Details` with elevated, virtualization, architecture; add `peak_working_set`. |
| `crates/actions/src/fileinfo.rs` (new) | `file_version`, `signature`. |
| `crates/actions/src/dump.rs` (new) | `dump_file_name`, `create_dump`. |
| `crates/actions/src/location.rs` | `show_properties`, `open_url`, `search_url`. |
| `crates/actions/src/lib.rs` | Export the above. |
| `crates/app/src/details.rs` (new) | Pure: `Opened`, `DetailsWatch`, `DetailsFrame`, `build_details_frame`, `role_hint`, formatting, `copy_all_text`. |
| `crates/app/src/dispatch.rs` | `Outcome`; `JobKind::{Dump, Properties, SearchOnline}`; Details job reads every process. |
| `crates/app/src/controls.rs` | `set_details_watch`, `details_watch`. |
| `crates/app/src/view.rs` | `Frame.details`; `RowView.hosts_services`; `Target.hosts_services`. |
| `crates/app/src/worker.rs` | Build `DetailsFrame` when watched; service-PID cache. |
| `crates/app/src/bridge.rs` | `apply_details`; `hosts_services` into row data. |
| `crates/app/src/main.rs` | Outcome handling; popup callbacks; extract `open_affinity` / `open_priority`; new menu actions; toast action. |
| `crates/app/ui/detailspopup.slint` (new) | `DetailsPopup` and its structs. |
| `crates/app/ui/main.slint` | Replace the details `Modal` with `DetailsPopup`; new properties and callbacks; toast action. |
| `crates/app/ui/overlays.slint` | Toast action button. |
| `crates/app/ui/applist.slint` | Four menu items; `hosts-services` on `RowMenu`, `RowData` and the list's current row. |
| `crates/app/tests/ui.rs` | UI tests below. |

## Review Focus

1. **PID reuse.** A watched process exits and its PID is reused by another process while the popup is open: the popup must not show the new process's numbers. Every lookup keys on `(pid, create_time)`.
2. **Parent PID reuse.** The parent shown must have a create time earlier than the child's; otherwise show `—`.
3. **Access denied on some processes of an app.** A protected child (for example an anti-cheat or audio host) must show dashes in its own fields while the others show values; the open must not fail as a whole.
4. **Dump of a protected or exited process.** Shows the usual `ActionError` message, leaves no zero-byte `.dmp` behind.
5. **Popup open while the user changes page or the app exits entirely.** Live numbers stop, the popup stays readable with "Exited", Close still works, and closing clears the watch so the worker stops reading sockets off the Ports page.

---

### Task 1: Static process facts in negotium-actions

**Files:**
- Modify: `crates/actions/src/details.rs`, `crates/actions/src/lib.rs`, `crates/actions/Cargo.toml`
- Test: `crates/actions/src/details.rs` (`#[cfg(test)]`, real OS, current process)

**Interfaces:**
- Produces:
  ```rust
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub enum Architecture { X86, X64, Arm64, X64OnArm64, X86OnArm64, Other }
  pub struct Details { pub pid: u32, pub image_path: Option<String>, pub command_line: Option<String>,
      pub user: Option<String>, pub priority: Option<String>,
      pub elevated: Option<bool>, pub virtualized: Option<bool>, pub architecture: Option<Architecture>,
      pub efficiency: Option<bool> }
  pub fn process_details(pid: u32, create_time: i64) -> Result<Details, ActionError>; // unchanged signature
  pub fn peak_working_set(pid: u32, create_time: i64) -> Result<u64, ActionError>;
  ```
  Architecture via `IsWow64Process2` (process machine + native machine); elevation via `TokenElevation`; virtualization via `TokenVirtualizationEnabled`; efficiency via the existing `is_efficiency_mode`; peak via `GetProcessMemoryInfo(...).PeakWorkingSetSize`.

- [ ] **Step 1: Write the failing tests** (`own_process_*` read the test process itself; its create time comes from `GetProcessTimes(GetCurrentProcess())`, helper `own_create_time()` in the test module)
  - `own_process_has_its_architecture`: `assert_eq!(d.architecture, Some(if cfg!(target_arch = "aarch64") { Architecture::Arm64 } else { Architecture::X64 }))`
  - `own_process_reports_elevation_as_the_token_says`: `assert_eq!(d.elevated, Some(crate::is_elevated()))`
  - `own_process_is_not_virtualized`: `assert_eq!(d.virtualized, Some(false))`
  - `own_process_has_a_peak_at_least_its_working_set`: `assert!(peak_working_set(pid, ct).unwrap() >= 1 << 20)`
  - `a_wrong_create_time_is_already_exited`: `assert_eq!(peak_working_set(pid, ct + 1), Err(ActionError::AlreadyExited))` (match whatever `open_verified` returns for a mismatch today; read it first).
- [ ] **Step 2: Run** `cargo test -p negotium-actions details` — Expected: the new tests FAIL on their assertions (stubs return `None` / `Ok(0)`).
- [ ] **Step 3: Implement** the fields and `peak_working_set`; add the Cargo features.
- [ ] **Step 4: Run** `cargo test -p negotium-actions details` — Expected: PASS.
- [ ] **Step 5: Commit** red then green.

### Task 2: File version and signature

**Files:**
- Create: `crates/actions/src/fileinfo.rs`; Modify: `crates/actions/src/lib.rs`
- Test: in `fileinfo.rs`

**Interfaces:**
- Produces:
  ```rust
  #[derive(Clone, Debug, PartialEq, Eq)]
  pub enum Signature { Signed(String), NotSigned, Unknown }
  pub fn file_version(path: &str) -> Option<String>;   // "10.0.26100.1" from VS_FIXEDFILEINFO
  pub fn signature(path: &str) -> Signature;           // blocking; call off the UI thread
  ```
  `Signed(name)` is the leaf signer's simple display name (`CertGetNameStringW`, `CERT_NAME_SIMPLE_DISPLAY_TYPE`). Embedded via `WinVerifyTrust(WINTRUST_ACTION_GENERIC_VERIFY_V2)`, `WTD_REVOKE_NONE`, `WTD_UI_NONE`; if that reports no signature, catalog via `CryptCATAdminAcquireContext2` + `CryptCATAdminCalcHashFromFileHandle2` + `CryptCATAdminEnumCatalogFromHash`, then verify the catalog member. Always close the trust state (`WTD_STATEACTION_CLOSE`). `Unknown` when the file can't be opened.

- [ ] **Step 1: Write the failing tests**
  - `notepad_has_a_version`: `assert!(file_version(&format!(r"{}\System32\notepad.exe", windir)).is_some_and(|v| v.split('.').count() == 4))` (`windir` from `GetWindowsDirectoryW`, as the collector does)
  - `a_catalog_signed_windows_file_is_signed_by_microsoft`: `assert!(matches!(signature(…\System32\notepad.exe), Signature::Signed(n) if n.contains("Microsoft")))`
  - `the_test_binary_is_not_signed`: `assert_eq!(signature(std::env::current_exe()…), Signature::NotSigned)`
  - `a_missing_file_is_unknown`: `assert_eq!(signature(r"C:\nope\nope.exe"), Signature::Unknown)` and `assert_eq!(file_version(r"C:\nope\nope.exe"), None)`
- [ ] **Step 2: Run** `cargo test -p negotium-actions fileinfo` — Expected: FAIL (stubs return `None`/`Unknown`).
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** — Expected: PASS.
- [ ] **Step 5: Commit** red then green.

### Task 3: Dump, Properties, Search online in negotium-actions

**Files:**
- Create: `crates/actions/src/dump.rs`; Modify: `crates/actions/src/location.rs`, `crates/actions/src/lib.rs`
- Test: in those files

**Interfaces:**
- Produces:
  ```rust
  pub fn dump_file_name(process: &str, pid: u32, local: (i32, u32, u32, u32, u32, u32)) -> String; // (y,mo,d,h,mi,s) → "notepad_1234_20261029-142501.dmp"; strips ".exe"; replaces any of \/:*?"<>| with '_'
  pub fn create_dump(pid: u32, create_time: i64, process: &str, dir: &std::path::Path) -> Result<std::path::PathBuf, ActionError>;
  pub fn search_url(file_name: &str) -> String;           // percent-encodes everything but unreserved chars
  pub fn open_url(url: &str) -> Result<(), ActionError>;  // ShellExecuteW "open"
  pub fn show_properties(path: &str) -> Result<(), ActionError>; // SHObjectProperties(SHOP_FILEPATH)
  ```
  `create_dump` opens with `PROCESS_QUERY_INFORMATION | PROCESS_VM_READ` through `open_verified`, writes to a temp name, and deletes the file on failure (Review Focus 4).

- [ ] **Step 1: Write the failing tests**
  - `dump_names_follow_task_manager`: `assert_eq!(dump_file_name("notepad.exe", 1234, (2026,10,29,14,25,1)), "notepad_1234_20261029-142501.dmp")`
  - `dump_names_have_no_path_characters`: `assert_eq!(dump_file_name(r"a:b\c.exe", 1, (2026,1,2,3,4,5)), "a_b_c_1_20260102-030405.dmp")`
  - `a_child_process_can_be_dumped`: spawn `cmd /c ping -n 30 127.0.0.1 >nul`, dump it to a fresh temp dir, `assert!(path.metadata().unwrap().len() > 0)`, kill the child, remove the dir.
  - `a_failed_dump_leaves_no_file`: `create_dump(child_pid, wrong_ct, …)` is `Err`, and the dir is empty.
  - `search_urls_are_encoded`: `assert_eq!(search_url("my app.exe"), "https://www.bing.com/search?q=my%20app.exe")`; `assert_eq!(search_url("a&b#.exe"), "https://www.bing.com/search?q=a%26b%23.exe")`
  - `open_url` and `show_properties` have no automated test (they open windows); covered by manual steps M3 and M4.
- [ ] **Step 2: Run** `cargo test -p negotium-actions dump search_url` — Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** — Expected: PASS.
- [ ] **Step 5: Commit** red then green.

### Task 4: Details model (pure) and typed job outcome

**Files:**
- Create: `crates/app/src/details.rs`; Modify: `crates/app/src/lib.rs`, `crates/app/src/dispatch.rs`
- Test: in those files

**Interfaces:**
- Consumes: Task 1 `Details`, `Architecture`; Task 2 `Signature`, `file_version`; Task 3 functions.
- Produces:
  ```rust
  // details.rs
  pub struct ProcFacts { pub pid: u32, pub create_time: i64, pub name: String, pub role: Option<&'static str>,
      pub details: Option<negotium_actions::Details> }          // None: process could not be opened
  pub struct Opened { pub name: String, pub publisher: Option<String>, pub version: Option<String>,
      pub signature: negotium_actions::Signature, pub image_path: Option<String>,
      pub procs: Vec<ProcFacts>, pub selected: usize }            // root first, then by pid
  pub struct DetailsWatch { pub procs: Vec<(u32, i64)>, pub selected: (u32, i64) }
  pub struct LiveProc { pub pid: u32, pub create_time: i64, pub alive: bool, pub cpu_pct: f32, pub private_bytes: u64,
      pub disk_bps: f64, pub net_bps: Option<f64>, pub gpu_pct: Option<f32>, pub threads: u32, pub handles: u32 }
  pub struct Connection { pub protocol: &'static str, pub local: String, pub remote: String, pub state: String }
  pub struct DetailsFrame { pub procs: Vec<LiveProc>, pub peak_bytes: Option<u64>, pub parent: Option<(u32, String)>,
      pub start_time: Option<i64>, pub connections: Vec<Connection>, pub services: Vec<String> }
  pub fn role_hint(command_line: Option<&str>, is_root: bool) -> Option<&'static str>;
  pub fn build_details_frame(watch: &DetailsWatch, snap: &Snapshot, rows: &[AppRow],
      sockets: &[Socket], services: &HashMap<u32, Vec<String>>, peak: Option<u64>) -> DetailsFrame;
  pub fn running_for(start_filetime: i64, now_filetime: i64) -> String;   // "2 h 5 min", "45 s"
  pub fn copy_all_text(o: &Opened, f: &DetailsFrame) -> String;            // Identity section as "Label: value" lines
  pub fn read_opened(t: &crate::view::Target, publisher: Option<String>) -> Opened; // blocking; action thread
  // dispatch.rs
  pub enum Outcome { Silent, Toast(String), Details(Box<crate::details::Opened>), Dumped { text: String, folder: std::path::PathBuf } }
  pub fn run(job: Job, elevated: bool) -> Outcome;
  ```

- [ ] **Step 1: Write the failing tests** (`details.rs` unit tests on hand-built snapshots; `dispatch.rs` tests)
  - `roles_come_from_the_type_switch`: `role_hint(Some("x.exe --type=renderer --foo"), false) == Some("Renderer")`; `--type=gpu-process` → `"GPU"`; `--type=utility` → `"Utility"`; `--type=crashpad-handler` → `"Crash reporter"`; `(None, true)` → `Some("Main")`; `(Some("x.exe"), false)` → `None`.
  - `live_numbers_follow_pid_and_create_time`: a watched `(10, 1)` and a snapshot holding `(10, 2)` → that proc `alive == false`, `cpu_pct == 0.0` (Review Focus 1).
  - `threads_and_handles_come_from_the_snapshot`: values copied for a live proc.
  - `a_parent_younger_than_the_child_is_not_shown`: parent pid 4 with create time 9, child create time 5 → `parent == None` (Review Focus 2); an older parent → `Some((4, "System"))`.
  - `connections_are_only_the_selected_process`: sockets for pids 10 and 11, selected 10 → only pid 10's rows; a TCP listener has remote `"—"` and state `"Listening"`; UDP has state `"—"`.
  - `services_are_the_selected_process`: map `{10: ["Dnscache"]}` → `["Dnscache"]`; no entry → empty.
  - `running_for_reads_as_hours_and_minutes`: 2 h 5 min apart → `"2 h 5 min"`; 45 s → `"45 s"`; negative → `"—"`.
  - `copy_all_lists_each_identity_field`: contains `"Path: C:\\App\\app.exe"`, `"Command line: —"` when unread, `"PID: 10"`.
  - `details_job_returns_every_process_root_first` (dispatch): `plan("details", &target(false), ALL)` targets equal `target.procs` with the root first.
  - Update existing dispatch tests from `String` to `Outcome` (e.g. silent success → `Outcome::Silent`).
- [ ] **Step 2: Run** `cargo test -p negotium --lib details dispatch` — Expected: new tests FAIL on assertions.
- [ ] **Step 3: Implement.** `read_opened` calls `process_details` per process (an error becomes `details: None`, Review Focus 3), `file_version` and `signature` on the root's image path.
- [ ] **Step 4: Run** — Expected: PASS.
- [ ] **Step 5: Commit** red then green.

### Task 5: Worker watch and live frames

**Files:**
- Modify: `crates/app/src/controls.rs`, `crates/app/src/view.rs`, `crates/app/src/worker.rs`, `crates/app/src/bridge.rs`
- Test: `crates/app/src/worker.rs` tests (use the existing `ports_source` pattern), `crates/app/src/controls.rs` tests

**Interfaces:**
- Consumes: Task 4 `DetailsWatch`, `DetailsFrame`, `build_details_frame`.
- Produces: `Controls::set_details_watch(&self, Option<DetailsWatch>)`, `Controls::details_watch(&self) -> Option<DetailsWatch>`; `Frame.details: Option<DetailsFrame>`; `Bridge::apply_details(&mut self, ui, DetailsFrame)` called by `apply` before the page check.

- [ ] **Step 1: Write the failing tests**
  - `a_watch_adds_details_to_overview_frames`: set a watch on the test's own pid; `next_frame` → `frame.details.is_some()`.
  - `a_watch_builds_a_frame_on_pages_that_have_none`: page `Settings` with a watch → `Some(frame)` with `details` set; without a watch → `None` (unchanged).
  - `clearing_the_watch_stops_details`: watch then `None` → `frame.details.is_none()`.
  - `sockets_are_not_read_without_ports_or_a_watch`: with the counting `ports_source`, an Overview tick and no watch → read count stays 0 (Review Focus 5).
- [ ] **Step 2: Run** `cargo test -p negotium --lib worker controls` — Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** — Expected: PASS.
- [ ] **Step 5: Commit** red then green.

### Task 6: The popup

**Files:**
- Create: `crates/app/ui/detailspopup.slint`; Modify: `crates/app/ui/main.slint`, `crates/app/src/main.rs`, `crates/app/src/bridge.rs`
- Test: `crates/app/tests/ui.rs`

**Interfaces:**
- Consumes: Task 4 `Opened`, `Outcome`, `copy_all_text`; Task 5 `apply_details`, `set_details_watch`.
- Produces (Slint, on `MainWindow`):
  ```slint
  export struct DetailsChip { text: string, warn: bool }
  export struct DetailsProc { pid: int, name: string, role: string, cpu: string, memory: string, alive: bool }
  export struct DetailsField { label: string, value: string }
  export struct DetailsConnection { protocol: string, local: string, remote: string, state: string }
  in-out property <bool> details-visible;
  in property <string> details-name; in property <string> details-subtitle;   // "Publisher · version"
  in property <image> details-icon; in property <[DetailsChip]> details-chips;
  in property <[DetailsProc]> details-procs; in-out property <int> details-selected;
  in property <[DetailsField]> details-identity; in property <[DetailsField]> details-resources;
  in property <[DetailsField]> details-settings;
  in property <[DetailsConnection]> details-connections; in property <[string]> details-services;
  callback details-select(int); callback details-copy-all(); callback details-change-priority();
  callback details-change-affinity(); callback details-open-location(); callback close-details();
  ```
  Section titles: "Processes in this app", "Identity", "Resources", "Settings", "Network connections", "Services". Settings note: "Applies to all processes in this app." Empty texts: "No network connections." and "No services run in this process." Copy all toast: "Details copied to clipboard." Chips: "Signed by <name>" / "Not signed" (warn) / no chip for `Unknown`; "64-bit", "32-bit", "ARM64", "x64 on ARM64", "x86 on ARM64"; "Runs as admin" / "Does not run as admin"; "Efficiency mode on" only when on; "UAC virtualization on" only when on.
  Escape and the Close button both call `close-details`, which clears the watch.

- [ ] **Step 1: Write the failing tests**
  - `details_popup_shows_every_section`: set `details-visible` and sample data; find each section title by accessible label.
  - `selecting_a_process_reports_its_index`: activate the second `DetailsProc` row → `details-select(1)` called.
  - `an_exited_process_reads_exited`: `alive: false` row shows text "Exited".
  - `details_popup_fits_the_narrowest_window`: window 760 × 900; popup width ≤ 712 px; the list scrolls (`ScrollView` viewport height < content height).
  - `escape_closes_the_details_popup`: key Escape → `close-details` called.
  - `change_buttons_report_their_callbacks`: activate "Change" beside Priority and Affinity, then "Open file location" → each callback once.
  - After a priority or affinity dialog applies, the Rust side re-reads the root's priority and affinity into `details-settings` (spec: the popup refreshes after); covered by manual step M6.
  - `empty_lists_say_so`: no connections, no services → both empty texts visible.
  - Update the existing details tests that drove the old `Modal`.
- [ ] **Step 2: Run** `cargo test -p negotium --test ui details` — Expected: FAIL.
- [ ] **Step 3: Implement** the component and the Rust side: on `Outcome::Details`, fill the static properties, set the watch (all procs, selected = root), show. `details-select` updates the watch's selected and the identity fields. Extract `open_affinity(&Target)` and `open_priority(&Target)` from `on_row_action`; the Change buttons call them with the target captured at open. The `ActionError` for the root stays a toast as today.
- [ ] **Step 4: Run** `cargo test -p negotium --test ui` — Expected: PASS (whole UI suite).
- [ ] **Step 5: Commit** red then green.

### Task 7: New menu items and the toast button

**Files:**
- Modify: `crates/app/ui/applist.slint`, `crates/app/ui/overlays.slint`, `crates/app/ui/main.slint`, `crates/app/src/view.rs`, `crates/app/src/worker.rs`, `crates/app/src/bridge.rs`, `crates/app/src/dispatch.rs`, `crates/app/src/main.rs`
- Test: `crates/app/src/dispatch.rs`, `crates/app/tests/ui.rs`

**Interfaces:**
- Consumes: Task 3 functions; Task 4 `Outcome::Dumped`.
- Produces: actions `"search-online"`, `"properties"`, `"goto-services"`, `"dump"`; `JobKind::{Dump, Properties, SearchOnline}`; `RowData.hosts-services: bool`, `RowMenu.hosts-services`, `AppList.current-hosts-services`; `Target.hosts_services: bool`; toast `toast-action: string`, `callback toast-action-clicked()`.
  Menu order: End task, End process tree, Efficiency mode | Set CPU affinity…, Set CPU priority… | Exclude from Aequilibrium | Open file location, Search online, Properties, Go to service(s) | Create memory dump file | Details.
  Toasts: "Creating a memory dump of <name>…" then "Memory dump of <name> saved." with **Open folder** (opens the folder with the file selected, via `open_file_location`).

- [ ] **Step 1: Write the failing tests**
  - `search_and_properties_need_a_path` (dispatch): with `image_path: None` → `Plan::Nothing` for both; with a path → `Plan::Run` of `SearchOnline` / `Properties`.
  - `dump_targets_the_main_process` (dispatch): `plan("dump", …)` targets `[root]`.
  - `the_windows_group_gets_none_of_the_new_actions` (dispatch): `group: true` → `Plan::Nothing` for all four.
  - `goto_services_needs_a_service_host` (dispatch): `hosts_services: false` → `Plan::Nothing`.
  - `the_row_menu_has_the_new_items_in_order` (ui): open the menu, read item titles in order.
  - `go_to_services_is_enabled_only_for_service_hosts` (ui): row with `hosts-services: false` → item disabled; `true` → enabled.
  - `go_to_services_filters_the_services_page_by_pid` (ui + bridge): activating it switches to Services with search text equal to the root PID.
  - `a_toast_action_shows_its_button` (ui): `toast-action: "Open folder"` → button found; activating it calls `toast-action-clicked`; empty → no button.
- [ ] **Step 2: Run** `cargo test -p negotium dispatch` and `cargo test -p negotium --test ui menu toast` — Expected: FAIL.
- [ ] **Step 3: Implement.** "goto-services" is handled in `on_row_action` like `set-affinity`: `set_page(Services)` and set `services-search-text` and `Bridge::set_services_search` to the PID.
- [ ] **Step 4: Run** `cargo test -p negotium` and clippy — Expected: PASS, clean.
- [ ] **Step 5: Commit** red then green.

---

## Test Plan

| File | Behaviours |
|---|---|
| `actions/src/details.rs` | own architecture; elevation matches the token; not virtualized; peak ≥ 1 MB; wrong create time fails |
| `actions/src/fileinfo.rs` | notepad has a 4-part version; notepad signed by Microsoft (catalog); test binary not signed; missing file Unknown/None |
| `actions/src/dump.rs`, `location.rs` | dump name format; path characters replaced; child process dumps to a non-empty file; failed dump leaves no file; search URL encoding |
| `app/src/details.rs` | role hints (5 cases); PID-reuse safety; threads/handles; parent age check; connections of the selected pid only; listener and UDP dashes; services; running time; copy-all text |
| `app/src/dispatch.rs` | details job root first; Outcome replaces strings; search/properties need a path; dump targets root; Windows group refused; goto-services needs a host |
| `app/src/worker.rs`, `controls.rs` | watch adds details on Overview; builds on frameless pages; cleared watch stops; no socket reads without Ports or a watch |
| `app/tests/ui.rs` | every section; select reports index; Exited text; fits at 760 px; Escape closes; Change callbacks; empty texts; menu item order; Go to services enabled state and navigation; toast action button |

**Not test-first, and what covers it instead:**
- `open_url`, `show_properties` and the Open folder button open real windows: manual steps M3, M4, M5.
- Live numbers updating on screen at the refresh rate: covered in logic by Task 5 tests, on screen by manual step M2.
- The icon in the header reuses the existing icon cache; no new test.

**Manual verification (release build into `try\`):**
- M1. Open Details on Chrome or Discord: every section shows, child processes list with roles, signature chip says Signed by the publisher.
- M2. Keep it open 10 s: CPU and memory change; select a renderer: Identity and Network connections change.
- M3. Search online opens the browser on the file name.
- M4. Properties opens the Windows file Properties dialog.
- M5. Create memory dump file on Notepad: toast, then Open folder selects the `.dmp` in `%TEMP%`.
- M6. Details on a protected process without admin: dashes, no failure; Change priority shows the usual admin-rights message.

## Out of Scope

- A separate Details page or a table of every process.
- Closing network connections; per-connection traffic.
- Wait chain, debugger, UAC virtualization toggle, Provide feedback.
- New menu items on the Ports page's menu (its Details opens the new popup; nothing else changes there).
- Adding processes that start after the popup opens.

## Definition of Done

- [ ] Details opens the new popup with all seven sections (`details_popup_shows_every_section`, M1).
- [ ] Every process of the app is listed, root first, with role hints where known (`details_job_returns_every_process_root_first`, `roles_come_from_the_type_switch`).
- [ ] Selecting a process changes Identity, Resources, connections and services (`selecting_a_process_reports_its_index`, `connections_are_only_the_selected_process`, M2).
- [ ] Live numbers come only from the matching `(pid, create_time)` (`live_numbers_follow_pid_and_create_time`).
- [ ] Unreadable fields show `—`; one failed process does not fail the popup (`copy_all_lists_each_identity_field`, M6).
- [ ] The popup fits a 760 px window and scrolls (`details_popup_fits_the_narrowest_window`).
- [ ] Closing clears the watch; sockets are not read off the Ports page without it (`clearing_the_watch_stops_details`, `sockets_are_not_read_without_ports_or_a_watch`).
- [ ] Change priority/affinity open today's dialogs over the popup (`change_buttons_report_their_callbacks`, M6).
- [ ] The menu has the four new items in the stated order; Go to service(s) is enabled only for service hosts and opens Services filtered by PID (`the_row_menu_has_the_new_items_in_order`, `go_to_services_*`).
- [ ] A dump is written to `%TEMP%` with Task Manager's name, never leaves an empty file, and its toast has Open folder (`dump_*`, `a_failed_dump_leaves_no_file`, `a_toast_action_shows_its_button`, M5).
- [ ] `cargo test -p negotium -p negotium-actions` passes; clippy clean.

## Acceptance Criteria

1. Choosing Details on an app shows one scrollable popup with the header, the app's processes, identity, live resources, settings, network connections and services of the selected process.
2. The popup never needs a wider window and never shows numbers from a different process that reused a PID.
3. The row menu offers Search online, Properties, Go to service(s) (only for service hosts) and Create memory dump file, and each does what Task Manager's item does.
4. A memory dump does not freeze the window and tells the user where it went.
