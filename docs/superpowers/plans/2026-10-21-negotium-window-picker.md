# Window Picker ("End a window…") Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** From the tray menu or an opt-in Ctrl+Alt+Shift+Esc shortcut, dim every monitor, spotlight the window under the cursor with a dashed outline and a label, and end that window's whole app on click (confirm dialog; Shift+click skips it).

**Architecture:** A new `crates/app/src/picker/` module. Pure parts (which window is the target, what the label says, what a click does, where things are drawn) are unit-tested over plain data. Win32 parts are thin: z-ordered window enumeration, one per-pixel-alpha layered window per monitor drawn with Direct2D (CPU is enough: it redraws only when the target changes), a `TaskDialogIndirect` confirm, and `RegisterHotKey` on a message-only window. Everything runs on the UI thread except the process snapshot taken when the picker opens and the end job, which run on threads and come back through `slint::invoke_from_event_loop`, as the existing action jobs do. Ending reuses `dispatch::plan("end-task")` → `dispatch::run`.

**Tech Stack:** Rust 2024, `windows` 0.62.2 (WindowsAndMessaging, Gdi, Dwm, Direct2D, DirectWrite, Imaging for tests, Controls), Slint 1.18.1.

**Spec:** `docs/superpowers/specs/2026-10-21-negotium-window-picker-design.md`

## Summary

Task Manager often stays hidden behind a frozen window, and neither it nor Negotium lets you point at the bad window. This adds an `xkill`-style picker that ends the whole app that owns the clicked window, protects the shell, handles packaged (UWP) apps and File Explorer windows, and needs no main window.

## Global Constraints

- Shortcut: `MOD_CONTROL | MOD_ALT | MOD_SHIFT | MOD_NOREPEAT` + `VK_ESCAPE`. Setting `picker_shortcut: bool`, default `false`; the flag (golden rule 7). Off ⇒ `RegisterHotKey` is never called.
- In code, "hotkey" already means the Ctrl+Shift+Esc IFEO redirect. The new shortcut is always named `picker_shortcut` / `shortcut`, never `hotkey`.
- Tray item "End a window…" is always shown (spec goal 1; approved in the brainstorm — the documented exception to golden rule 7, because it does nothing until clicked).
- Dim: black at 55% alpha (`0x8C`). Outline: dashed, 3 px at 96 DPI scaled by monitor DPI, red `#E81123` when hung, accent `#4CC2FF` otherwise.
- Copy (ASD-STE100), exact:
  - Hint: `Click a window to end it · Shift+click: end now · Esc: cancel`
  - Tray item: `End a window…`
  - Settings row title: `Pick a window to end on Ctrl+Alt+Shift+Esc`; description: `Press the keys, then click a window to end its app. Shift+click ends it with no question.`
  - Shortcut taken: `Another program uses Ctrl+Alt+Shift+Esc`
  - Shell surface label: `Windows needs this — you cannot end it here`
  - Explorer label suffix: `File Explorer — restarts the taskbar`
  - Needs admin: `Needs admin — restart Negotium as admin`
  - Confirm: title `End {name}?`, body `This closes {n} processes. You lose unsaved work.` (`1 process` singular), buttons `End` / `Cancel`, default `Cancel`.
  - Explorer confirm body adds: `This also restarts the taskbar and the desktop. All File Explorer windows close.`
  - Explorer did not restart: `The taskbar did not start again. Press Ctrl+Shift+Esc, then run explorer.exe.`
- Shell window classes never targeted: `Shell_TrayWnd`, `Shell_SecondaryTrayWnd`, `Progman`, `WorkerW`. Folder window class: `CabinetWClass`. UWP frame: `ApplicationFrameWindow` hosting `Windows.UI.Core.CoreWindow`.
- Build and test with a scratch `CARGO_TARGET_DIR` outside the repo (the release exe may be locked by a running Negotium). Never kill the user's Negotium or Explorer in tests.
- Commits: `test(<crate>): add failing test for …` then `feat(<crate>): …`, trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Do not commit the unrelated `Cargo.lock` version bump in the working tree.

## Architectural Decisions

1. **Native layered windows, not Slint windows.** Slint has no multi-monitor, topmost, per-pixel-transparent window, and the software renderer's buffer is opaque (f1c6491). A native window behaves the same under both renderers.
2. **Direct2D into a DIB, `UpdateLayeredWindow`.** Direct2D gives premultiplied alpha, anti-aliased dashes and DirectWrite text; GDI writes alpha 0. The DC render target uses `D2D1_RENDER_TARGET_TYPE_DEFAULT`, so it uses the GPU when present. Redraw only on target change.
3. **The overlay takes activation** (no `WS_EX_NOACTIVATE`). The hotkey and the tray click both give Negotium foreground rights, so `SetForegroundWindow` on the overlay works; then Esc arrives as `WM_KEYDOWN` and the mouse is captured with `SetCapture`. Deviation from the spec's style list, recorded here.
4. **Own snapshot on open.** `worker.rs:296` builds nothing while hidden, so the picker runs `Collector::new().tick()` + `Model::update` on a thread when it opens. Clicks before it is ready are ignored and the label says the app name from the window's process only.
5. **Reuse `Target` building.** The `Target` construction inlined in `view::build_frame` (view.rs:391-413) moves to `pub fn target_of(...)` so the picker and the Overview build targets the same way.
6. **Result display.** `end_summary` is already `""` on full success; a non-empty result (failure) shows in a `TaskDialog` (the main window's toast is usually hidden). The shortcut-taken failure at start-up shows as the main window's toast the next time the window opens, and on the Settings row. **Deviation:** the spec asks for a tray notification; Slint's `SystemTrayIcon` has no notification API and Negotium has no `Shell_NotifyIcon` code. Follow-up named below.
7. **Explorer restart without admin.** Before ending a shell-hosted folder window, capture the shell's primary token (`OpenProcessToken` + `DuplicateTokenEx`) when Negotium is elevated, then start `%WINDIR%\explorer.exe` with `CreateProcessWithTokenW`. Not elevated: plain `CreateProcess`. Confirm a new `Shell_TrayWnd` within 10 s.

## Files

| File | Change |
|---|---|
| `crates/app/src/view.rs` | Extract `pub fn target_of(row, hung_pids, image_of) -> Target`; `build_frame` calls it |
| `crates/app/src/picker/mod.rs` | Create. `pub fn open(ctx: &PickerCtx)`; session state; wiring of the parts below |
| `crates/app/src/picker/apps.rs` | Create. Pure pid → app index over `AppRow`s |
| `crates/app/src/picker/target.rs` | Create. Pure `WindowInfo`, `Pickable`, `classify`, `pick_at` |
| `crates/app/src/picker/label.rs` | Create. Pure label text and outline colour |
| `crates/app/src/picker/click.rs` | Create. Pure click decision |
| `crates/app/src/picker/scene.rs` | Create. Pure per-monitor layout (`Scene`) + `render` with Direct2D |
| `crates/app/src/picker/win.rs` | Create. Win32: z-ordered enumeration, UWP resolve, monitors |
| `crates/app/src/picker/overlay.rs` | Create. Win32: layered windows, input, `TaskDialog` |
| `crates/app/src/shortcut.rs` | Create. `RegisterHotKey` behind a `HotkeyApi` seam |
| `crates/app/src/lib.rs` | `pub mod picker; pub mod shortcut;` |
| `crates/app/src/settings.rs` | `picker_shortcut: bool` (+ `field!`) |
| `crates/app/src/settingspage.rs` | Toggle wiring, `shortcut-taken` state |
| `crates/app/ui/settingspage.slint`, `main.slint` | New `picker-row` |
| `crates/app/ui/tray.slint`, `crates/app/src/main.rs` | `End a window…` item; `make_tray` handler; shortcut start-up |
| `crates/app/Cargo.toml` | `windows` features: `Win32_Graphics_Direct2D`, `Win32_Graphics_Direct2D_Common`, `Win32_Graphics_DirectWrite`, `Win32_Graphics_Dxgi_Common`, `Win32_Graphics_Imaging` |
| `crates/actions/src/shell.rs`, `lib.rs`, `Cargo.toml` | Create. `can_terminate`, `ShellToken`, `start_explorer` |

## Review Focus

1. **Target changes between button-down and button-up** (a window opens under the cursor): the app ended is the one highlighted at button-down. (Task 4 `the_target_is_fixed_at_button_down`.)
2. **The process exits or the pid is reused while the picker is open**: ending re-verifies pid + create time (`open_verified`), and an already-exited app is a silent success. (Task 4 `a_target_that_exited_ends_silently`; existing `end_many` tests.)
3. **Clicking the taskbar, desktop or Negotium itself** does nothing. (Task 2 table test.)
4. **A monitor at a different DPI or with negative coordinates** (left of primary): the outline and label land on the target. (Task 7 `negative_origin_monitor_maps_target_into_local_space`, `label_flips_inside_when_no_room_below`.)
5. **The picker gets stuck**: any Win32 failure while opening closes every overlay and releases capture; Esc always closes. (Task 8 manual steps M3, M4; `close_all` is called from every error path.)

---

## Phase 1 — Pure core (PR 1)

### Task 1: `target_of` and the pid → app index

**Files:**
- Modify: `crates/app/src/view.rs:391-413`
- Create: `crates/app/src/picker/mod.rs` (module decls only), `crates/app/src/picker/apps.rs`
- Modify: `crates/app/src/lib.rs`
- Test: `apps.rs` unit tests; existing `view.rs` / `tests/ui.rs` tests are the parity check

**Interfaces:**
- Produces:
  - `pub fn target_of(r: &AppRow, hung_pids: &HashSet<u32>, image_of: &HashMap<(u32, i64), String>) -> Target` in `view.rs` (exact body moved from `build_frame`).
  - `pub struct AppInfo { pub target: Target, pub process_count: usize, pub private_bytes: u64, pub hung: bool }`
  - `pub struct AppIndex { .. }` with `pub fn from_rows(rows: &[AppRow], hung_pids: &HashSet<u32>, image_of: &HashMap<(u32, i64), String>) -> Self` and `pub fn get(&self, pid: u32, create_time: i64) -> Option<&AppInfo>`.
  - `pub fn single_process(pid: u32, create_time: i64, name: &str) -> AppInfo` — fallback for a pid the snapshot does not know (`process_count` 1, `root` and `procs` that process).

- [ ] **Step 1: Write the failing tests** in `apps.rs`:
  - `finds_the_app_that_contains_a_child_process` — a `Window` row with procs `(10,1)` root and `(11,2)`: `get(11,2)` → target name of the row, `process_count == 2`.
  - `a_reused_pid_with_another_create_time_is_not_found` — `get(11, 99)` → `None`.
  - `the_windows_group_keeps_group_true` — an `AppKey::Windows` row → `target.group == true`.
  - `single_process_targets_only_that_process` — `procs == vec![(7,3)]`, `root == Some((7,3))`, `process_count == 1`.
- [ ] **Step 2:** `cargo test -p negotium --lib picker::apps` → FAIL (`todo!()` stubs).
- [ ] **Step 3:** Move the `Target { .. }` construction into `target_of`; implement `AppIndex` as a `HashMap<(u32, i64), usize>` into a `Vec<AppInfo>`.
- [ ] **Step 4:** `cargo test -p negotium` → all pass (view parity: existing Overview tests unchanged).
- [ ] **Step 5:** Commit (red, then green).

### Task 2: Which window is the target

**Files:** Create `crates/app/src/picker/target.rs` (unit tests inline).

**Interfaces:**
- Produces:
  ```rust
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub struct Rect { pub left: i32, pub top: i32, pub right: i32, pub bottom: i32 }
  #[derive(Clone, Debug, PartialEq)]
  pub struct WindowInfo {
      pub hwnd: isize,
      pub pid: u32,              // after UWP resolution (Task 6)
      pub create_time: i64,
      pub class: String,
      pub title: String,
      pub frame: Rect,           // DWMWA_EXTENDED_FRAME_BOUNDS, screen px
      pub visible: bool, pub cloaked: bool, pub minimized: bool,
      pub hung: bool,
      pub uwp_unresolved: bool,  // ApplicationFrameWindow with no CoreWindow child
  }
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub enum Pickable { App, ExplorerInShell, Shell, Skip }
  pub struct Env { pub own_pid: u32, pub shell_pid: Option<u32>, pub overlays: Vec<isize> }
  pub fn classify(w: &WindowInfo, env: &Env) -> Pickable;
  /// `windows` in z-order, top first. Returns the first window whose frame contains `pt` and is
  /// not `Skip`; overlays are ignored.
  pub fn pick_at<'a>(windows: &'a [WindowInfo], pt: (i32, i32), env: &Env) -> Option<(&'a WindowInfo, Pickable)>;
  ```
  Rules (in this order): overlay hwnd, invisible, cloaked, minimized, empty title, `uwp_unresolved`, `pid == own_pid` → `Skip`; shell class → `Shell`; `CabinetWClass` with `pid == shell_pid` → `ExplorerInShell`; any other window with `pid == shell_pid` → `Shell`; else `App`. A `CabinetWClass` in another process is `App`.

- [ ] **Step 1: Failing tests:**
  - `classify_table` — one row per rule above (11 rows), asserting the exact `Pickable`.
  - `pick_at_takes_the_topmost_containing_window` — two overlapping `App` windows, point in both → the first.
  - `pick_at_ignores_skipped_windows_above` — a cloaked window above an `App` → the `App`.
  - `pick_at_returns_shell_for_the_desktop` — only `Progman` under the point → `Some((_, Shell))`.
  - `frame_edges_are_inclusive_left_top_exclusive_right_bottom`.
- [ ] **Step 2:** `cargo test -p negotium --lib picker::target` → FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** PASS. **Step 5:** Commit pair.

### Task 3: The label and outline

**Files:** Create `crates/app/src/picker/label.rs`.

**Interfaces:**
- Consumes: `AppInfo` (Task 1), `Pickable` (Task 2).
- Produces:
  ```rust
  pub enum Outline { None, Accent, Red }
  pub struct Label { pub title: String, pub detail: String, pub outline: Outline }
  pub enum Info<'a> { Loading { exe: &'a str }, Ready(&'a AppInfo) }
  pub fn label(kind: Pickable, info: Info, needs_admin: bool, hung: bool) -> Label;
  ```
  `detail` joins with ` · `: `{n} processes` (`1 process`), memory via `crate::format::bytes`, `Not responding` when hung, `Needs admin — restart Negotium as admin` when `needs_admin`, `File Explorer — restarts the taskbar` for `ExplorerInShell`. `Shell` → title `Windows needs this — you cannot end it here`, empty detail, `Outline::None`.

- [ ] **Step 1: Failing tests:** `ready_app_shows_name_count_and_memory`; `one_process_is_singular`; `hung_app_is_red_and_says_not_responding`; `responding_app_is_accent`; `needs_admin_is_shown`; `explorer_in_shell_warns_about_the_taskbar`; `shell_surface_has_no_outline_and_the_shell_text`; `loading_shows_the_exe_name_only`.
- [ ] **Steps 2–5:** run → FAIL, implement, PASS, commit pair.

### Task 4: What a click does

**Files:** Create `crates/app/src/picker/click.rs`.

**Interfaces:**
- Consumes: `dispatch::{plan, Plan, Job}`, `AppInfo`, `Pickable`.
- Produces:
  ```rust
  pub struct Pressed { pub kind: Pickable, pub info: Option<AppInfo>, pub needs_admin: bool }
  pub enum Outcome {
      Ignore,                              // Shell / Skip / not ready / Windows group
      Tell(String),                        // needs admin: dialog with Close only
      Confirm { title: String, body: String, job: Job, restart_shell: bool },
      Run { job: Job, restart_shell: bool },
  }
  pub fn decide(pressed: &Pressed, shift: bool, all_procs: &[(u32, i64, u32)]) -> Outcome;
  ```
  `pressed` is captured at button-down and passed unchanged at button-up (Review Focus 1). `plan("end-task", ..)`: `Nothing` → `Ignore`; `Confirm(text, job)` (critical) → `Confirm` even with shift, body = that text; `Run(job)` → `Confirm` without shift, `Run` with shift. `ExplorerInShell` → always `Confirm` with the Explorer body and `restart_shell: true`. `needs_admin` → `Tell(..)` without shift, `Ignore` with shift.

- [ ] **Step 1: Failing tests:** `click_on_app_confirms_with_cancel_text`; `shift_click_runs_at_once`; `critical_always_confirms_even_with_shift`; `explorer_in_shell_always_confirms_and_restarts_the_shell`; `needs_admin_tells_on_click_and_ignores_shift_click`; `shell_and_not_ready_are_ignored`; `windows_group_is_ignored`; `the_target_is_fixed_at_button_down` (a `Pressed` built for app A, decided after the index changes → job targets A's procs); `a_target_that_exited_ends_silently` (`dispatch::end_summary` with only `AlreadyExited` failures → `""`, pins the existing contract the picker relies on); `job_ends_children_before_root`.
- [ ] **Steps 2–5:** run → FAIL, implement, PASS, commit pair.

## Phase 2 — Win32 building blocks (PR 2)

### Task 5: Terminate-rights probe and Explorer restart

**Files:** Create `crates/actions/src/shell.rs`; modify `crates/actions/src/lib.rs` (exports), `crates/actions/Cargo.toml` (features if missing). Test: `crates/actions/tests/actions.rs`.

**Interfaces:**
- Produces:
  - `pub fn can_terminate(pid: u32, create_time: i64) -> bool` — `open_verified` with `PROCESS_TERMINATE`; `AlreadyExited` → `true` (nothing to warn about).
  - `pub struct ShellToken(..)`; `pub fn capture_shell_token(shell_pid: u32) -> Result<Option<ShellToken>, ActionError>` — `Ok(None)` when Negotium is not elevated.
  - `pub fn start_explorer(token: Option<&ShellToken>) -> Result<(), ActionError>` — starts `windows_dir()\explorer.exe` (never a PATH lookup), with the token when `Some`.
  - `pub fn wait_for_taskbar(timeout: Duration) -> bool` — polls `FindWindowW("Shell_TrayWnd")`.

- [ ] **Step 1: Failing tests:** `can_terminate_own_child` (spawned `ping` → `true`); `can_terminate_an_exited_process_is_true`; `cannot_terminate_a_protected_process_when_not_elevated` (pid 4 `System` → `false`; `#[ignore]` when elevated is detected); `capture_shell_token_is_none_when_not_elevated`. `start_explorer` / `wait_for_taskbar` are **exempt from automated tests** (they would restart the user's shell); covered by manual step M6.
- [ ] **Steps 2–5:** run → FAIL, implement, PASS, commit pair.

### Task 6: Z-ordered enumeration and UWP resolution

**Files:** Create `crates/app/src/picker/win.rs`. Test: inline `#[cfg(test)]` using hidden test windows as in `platform.rs:1179-1260`.

**Interfaces:**
- Consumes: `WindowInfo`, `Rect` (Task 2); ghost handling copied from `collector::windows_scan` via a new `pub fn original_of_ghost(hwnd: HWND) -> Option<HWND>` exported there (reuse, not a copy).
- Produces:
  - `pub fn top_level_windows() -> Vec<WindowInfo>` — `EnumWindows` order (top first), unowned only. Never sends a window message (`InternalGetWindowText`, `GetClassNameW`, `DwmGetWindowAttribute`). `ApplicationFrameWindow` → `pid` of its `Windows.UI.Core.CoreWindow` child found with `FindWindowExW`; none → `uwp_unresolved: true`. `create_time` from `GetProcessTimes` (0 when the process cannot be opened; `AppIndex::get` then misses and `single_process` is used).
  - `pub fn shell_pid() -> Option<u32>` — process of `GetShellWindow()`.
  - `pub struct Monitor { pub rect: Rect, pub dpi: u32 }`; `pub fn monitors() -> Vec<Monitor>`.

- [ ] **Step 1: Failing tests:** `lists_a_visible_test_window_with_its_class_and_frame`; `a_newer_window_is_listed_before_an_older_one` (z-order); `owned_windows_are_not_listed`; `monitors_cover_the_primary_monitor`.
- [ ] **Steps 2–5:** run → FAIL, implement, PASS, commit pair.

## Phase 3 — The overlay and the tray item (PR 3, first user-visible change)

### Task 7: Scene layout and rendering

**Files:** Create `crates/app/src/picker/scene.rs`; modify `crates/app/Cargo.toml` (features listed in Files).

**Interfaces:**
- Consumes: `Rect`, `Monitor`, `Label`, `Outline`.
- Produces:
  ```rust
  pub struct Scene { pub size: (u32, u32), pub hole: Option<Rect>, pub outline: Outline,
                     pub label: Option<(Rect, Label)>, pub hint: Option<Rect>, pub scale: f32 }
  /// All rects in monitor-local px. `cursor_here` puts the hint bar on this monitor.
  pub fn layout(m: &Monitor, target: Option<(Rect, &Label)>, cursor_here: bool) -> Scene;
  pub fn render(rt: &ID2D1RenderTarget, text: &IDWriteFactory, scene: &Scene) -> windows::core::Result<()>;
  ```
  Label sits 8 px (scaled) below the target's bottom-left, flips inside the target's bottom when there is no room below, and is clamped into the monitor. Hint bar: centred, 10 px from the top. Dim everywhere except `hole`.

- [ ] **Step 1: Failing tests:** `negative_origin_monitor_maps_target_into_local_space`; `label_sits_below_the_target`; `label_flips_inside_when_no_room_below`; `label_is_clamped_into_the_monitor`; `hint_only_on_the_cursor_monitor`; `scale_is_dpi_over_96`; `render_dims_outside_and_clears_the_hole` (WIC bitmap render target, 200×100, hole 50..150: pixel (10,10) alpha `0x8C`, pixel (100,50) alpha `0`); `render_draws_a_dashed_red_outline` (sample along the top edge: both opaque red and clear pixels occur).
- [ ] **Steps 2–5:** run → FAIL, implement, PASS, commit pair.

### Task 8: Overlay windows, input, confirm, and the tray item

**Files:** Create `crates/app/src/picker/overlay.rs`; fill `crates/app/src/picker/mod.rs`; modify `crates/app/ui/tray.slint`, `crates/app/src/main.rs` (`make_tray`).

**Interfaces:**
- Consumes: everything above; `dispatch::run`; `negotium_actions::{can_terminate, capture_shell_token, start_explorer, wait_for_taskbar}`.
- Produces:
  - `pub struct PickerCtx { pub elevated: bool }`; `pub fn open(ctx: &PickerCtx)` — no-op when already open (spec goal 6).
  - `pub fn is_open() -> bool`.
  - Tray: `callback pick-window();` + `MenuItem { title: "End a window…"; activated => { root.pick-window(); } }` placed after "Open". `make_tray` wires `tray.on_pick_window(move || picker::open(&ctx))`.
  Behaviour: one `WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW` popup per monitor; crosshair cursor (`IDC_CROSS`); first overlay gets `SetForegroundWindow` + `SetCapture`. On open: spawn snapshot thread (`Collector::new().tick()`, `Model::with_system_root(windows_dir()).update`, `AppIndex::from_rows`), result via `invoke_from_event_loop`. `WM_MOUSEMOVE` → `pick_at` → redraw only if the picked hwnd or readiness changed. Button-down → capture `Pressed`; button-up → `decide`. `Confirm`/`Tell` → `TaskDialogIndirect` owned by the overlay (`TDF_POSITION_RELATIVE_TO_WINDOW`), Cancel default (`nDefaultButton = IDCANCEL`); Cancel returns to picking. `Run` → close overlays, then on an action thread: if `restart_shell`, `capture_shell_token` first, `dispatch::run(job)`, then `start_explorer` + `wait_for_taskbar(10 s)`. A non-empty result or a failed restart → `TaskDialog` with the text. `Esc` / right-click → `close_all()`. Every error path calls `close_all()` (releases capture, destroys all overlays).

- [ ] **Step 1: Failing test:** `tests/ui.rs` `tray_menu_offers_end_a_window` — the `NegotiumTray` menu has an item titled `End a window…` whose activation calls `pick-window`. No test creates `NegotiumTray` today; if the testing backend cannot create a `SystemTrayIcon`, assert on the `tray.slint` source instead (as `tests/wix.rs` reads `main.wxs`) and say so in the PR.
- [ ] **Step 2:** run → FAIL. **Step 3:** implement. **Step 4:** PASS + `cargo clippy -p negotium -- -D warnings`.
- [ ] **Step 5: Manual verification** (the Win32 overlay is exempt from automated tests: it needs a real desktop and input; record results in the PR):
  - M1. Tray → End a window…: every monitor dims; the window under the cursor stays bright with an accent dashed outline and the label; the hint is on the cursor's monitor.
  - M2. Hover the taskbar and the desktop: no hole, label `Windows needs this…`; clicking does nothing.
  - M3. Esc closes; right-click closes; no window keeps the capture afterwards (clicking the desktop works).
  - M4. With a second monitor left of the primary (negative x), outline and label land on the target.
  - M5. Start `notepad.exe`, hang a test app (`bench` hang helper or a `Sleep` app) → red outline, `Not responding`; click → dialog with Cancel default; Cancel returns to picking; End ends it; Shift+click ends with no dialog.
  - M6. Open a File Explorer folder → label says `restarts the taskbar`; Shift+click still shows the dialog; End → taskbar and desktop come back within 10 s; repeat with Negotium elevated and confirm in Task Manager's Details that the new `explorer.exe` is **not** elevated.
  - M7. Calculator (UWP): the label names Calculator; End closes Calculator only, other UWP apps (Settings) stay open.
  - M8. Not elevated, target an elevated app (an elevated terminal) → `Needs admin…`; click → dialog with Close only.
  - M9. Run with `gpu_rendering` off (software renderer): M1 looks the same.
- [ ] **Step 6:** Commit pair.

## Phase 4 — The shortcut (PR 4)

### Task 9: The `picker_shortcut` setting and Settings row

**Files:** `crates/app/src/settings.rs`, `crates/app/src/settingspage.rs`, `crates/app/ui/settingspage.slint`, `crates/app/ui/main.slint`. Test: `settings.rs` unit tests, `tests/ui.rs`.

**Interfaces:**
- Produces: `Settings::picker_shortcut: bool` (default `false`, in `field!` list); Slint `in property <bool> picker-shortcut; in property <bool> shortcut-taken; callback picker-shortcut-toggled(bool);` on the settings page, forwarded through `main.slint` as `settings-picker-shortcut` / `settings-shortcut-taken`; `picker-row := SettingRow` after `start-row`, shown in dev builds too (no `installed` gate). Description shows the taken text when `shortcut-taken`.
  `SettingsPage` (settingspage.rs) gains `pub picker_shortcut_changed: Rc<dyn Fn(bool) -> bool>` (returns whether the shortcut is now registered).

- [ ] **Step 1: Failing tests:** `picker_shortcut_defaults_off`; `picker_shortcut_round_trips`; `a_bad_picker_shortcut_value_takes_the_default_and_keeps_the_rest` (`Loaded::Partial(["picker_shortcut"])`); ui: `picker_row_is_shown_in_a_dev_build`; `picker_row_shows_the_taken_text`.
- [ ] **Steps 2–5:** run → FAIL, implement, PASS, commit pair.

### Task 10: Register the shortcut

**Files:** Create `crates/app/src/shortcut.rs`; modify `crates/app/src/main.rs`, `crates/app/src/settingspage.rs`.

**Interfaces:**
- Produces:
  ```rust
  pub trait HotkeyApi { fn register(&self) -> bool; fn unregister(&self); }
  pub struct Win32Hotkey { .. }   // message-only window (HWND_MESSAGE), id 1, WM_HOTKEY → callback
  impl Win32Hotkey { pub fn new(on_press: Box<dyn Fn()>) -> windows::core::Result<Self>; }
  pub struct Shortcut<A: HotkeyApi> { .. }
  impl<A: HotkeyApi> Shortcut<A> {
      pub fn new(api: A) -> Self;
      /// Registers or unregisters; returns `registered`. Never touches the saved setting.
      pub fn apply(&mut self, wanted: bool) -> bool;
      pub fn taken(&self) -> bool;   // wanted && !registered
  }
  ```
  Start-up: `apply(settings.picker_shortcut)`; when `taken()`, queue the toast `Another program uses Ctrl+Alt+Shift+Esc` for the next `show_main` and set `shortcut-taken`. Toggle on → `apply(true)`; saved `true` even when taken (spec goal 4). WM_HOTKEY → `picker::open` (no-op if open).

- [ ] **Step 1: Failing tests** (fake `HotkeyApi` counting calls):
  - `off_never_registers` — parity test for the flag: `apply(false)` from new → `register` called 0 times, `unregister` 0 times.
  - `on_registers_once` — `apply(true)` twice → `register` 1 time.
  - `off_after_on_unregisters`.
  - `taken_when_register_fails` — fake returns `false` → `taken() == true`, `apply` returned `false`.
  - `retry_on_next_apply_after_taken` — fail then succeed → `taken() == false`.
  - `Win32Hotkey` registration with the real key is exempt from automated tests (it would collide with the user's running Negotium); covered by M10–M12.
- [ ] **Steps 2–4:** run → FAIL, implement, PASS.
- [ ] **Step 5: Manual verification:** M10 setting off → Ctrl+Alt+Shift+Esc does nothing; M11 on → it opens the picker with the main window hidden in the tray and with a topmost frozen window on screen; M12 run a second program that registers the same key first (a tiny `RegisterHotKey` test exe from `crates/bench`), start Negotium → toast on next open and the Settings row text; setting still on after restart.
- [ ] **Step 6:** Commit pair.

---

## Test Plan

```
picker::apps
  - a child pid resolves to its app row with the row's name and process count
  - a reused pid (other create time) resolves to nothing
  - the Windows group keeps group = true
  - an unknown pid falls back to that single process
picker::target
  - classify: overlay, invisible, cloaked, minimized, untitled, unresolved UWP, own pid -> Skip
  - classify: Shell_TrayWnd / Shell_SecondaryTrayWnd / Progman / WorkerW -> Shell
  - classify: CabinetWClass in the shell process -> ExplorerInShell; in another process -> App
  - classify: other window of the shell process -> Shell; any other -> App
  - pick_at: topmost containing window wins; skipped windows above are passed over; desktop -> Shell
  - frame edges: left/top inclusive, right/bottom exclusive
picker::label
  - name, "N processes", memory; "1 process" singular
  - hung -> red + "Not responding"; responding -> accent
  - needs admin text; Explorer-in-shell text; shell surface text with no outline; loading shows exe only
picker::click
  - click -> Confirm (Cancel default text); shift -> Run
  - critical -> Confirm even with shift; Explorer-in-shell -> Confirm + restart_shell even with shift
  - needs admin -> Tell on click, Ignore on shift-click
  - shell, Skip, not ready, Windows group -> Ignore
  - target fixed at button-down; children ended before root; exited target ends silently
picker::scene
  - negative-origin monitor mapping; label below / flipped / clamped; hint only on cursor monitor; DPI scale
  - render: dim alpha 0x8C outside, alpha 0 in the hole; dashed red outline has gaps
picker::win
  - lists a test window with class and frame; z-order newest first; owned windows skipped; monitors cover primary
actions::shell (tests/actions.rs)
  - can terminate own child; exited -> true; System when not elevated -> false; no token when not elevated
settings
  - picker_shortcut default off; round-trips; bad value -> default + Partial
shortcut
  - off never registers (flag parity); on registers once; off after on unregisters; taken when register fails; retry clears taken
tests/ui.rs
  - tray menu offers "End a window…" and it calls pick-window
  - picker row shown in a dev build; shows the taken text
```

**Test-first exemptions (stated):** the layered overlay windows, input handling and `TaskDialog` (Task 8), the real `RegisterHotKey` (Task 10) and `start_explorer` / `wait_for_taskbar` (Task 5) need a real desktop, real input or would disturb the user's shell and running Negotium. They are kept thin over the tested pure modules and covered by manual steps M1–M12, recorded in each PR.

## Out of Scope

- Ending a single process of an app from the picker; keyboard window picking; a key picker for the shortcut.
- Exclusive full-screen games; the secure desktop (UAC, lock screen, Ctrl+Alt+Del).
- A tray balloon / Windows toast notification for the start-up shortcut failure (deviation 6). **Follow-up:** `Shell_NotifyIcon` `NIF_INFO` support for Negotium's tray.
- Animating the spotlight.

## Definition of Done

- [ ] Tray menu shows `End a window…` and it opens the picker (ui test; M1).
- [ ] With `picker_shortcut` off, `RegisterHotKey` is never called (`off_never_registers`; M10).
- [ ] With it on, Ctrl+Alt+Shift+Esc opens the picker while hidden in the tray (M11).
- [ ] A taken shortcut keeps the saved setting on and is reported on the Settings row and as a toast (`taken_when_register_fails`, `picker_row_shows_the_taken_text`; M12).
- [ ] Every monitor dims with alpha `0x8C`; the target is clear with a dashed outline, red when hung (`render_*`, label tests; M1, M5, M9).
- [ ] The taskbar, desktop and Negotium are never targeted (`classify_table`, `pick_at_returns_shell_for_the_desktop`; M2).
- [ ] Click confirms with Cancel default; Shift+click ends at once; critical and Explorer-in-shell always confirm (`click` tests; M5, M6).
- [ ] The whole app's processes end, children before root (`job_ends_children_before_root`; M5).
- [ ] A shell-hosted folder window restarts Explorer, not elevated (`explorer_in_shell_*`; M6).
- [ ] UWP apps end without ending `ApplicationFrameHost.exe` (M7).
- [ ] Needs-admin targets are flagged before the click (`needs_admin_*`, `cannot_terminate_a_protected_process_when_not_elevated`; M8).
- [ ] Esc / right-click always close and release capture (M3).
- [ ] `cargo test --workspace` and `cargo clippy --workspace -- -D warnings` pass.

## Acceptance Criteria

1. Choosing `End a window…` in the tray dims every monitor and spotlights the window under the cursor. → DoD 1, 5
2. With the shortcut setting off, pressing Ctrl+Alt+Shift+Esc has no effect; on, it opens the picker from anywhere. → DoD 2, 3
3. If another program owns the shortcut, Negotium tells the user and keeps their choice. → DoD 4
4. The shell and Negotium cannot be picked. → DoD 6
5. A click asks first (Cancel default) and Shift+click does not, except for critical processes and the shell-hosted File Explorer. → DoD 7
6. The ended unit is the whole app; for File Explorer in the shell, Explorer comes back unelevated; for UWP apps, only that app ends. → DoD 8, 9, 10
7. The user learns before clicking when admin rights are needed. → DoD 11
8. The picker can always be closed with Esc. → DoD 12

## Deviations from the spec

- Overlay windows take activation (no `WS_EX_NOACTIVATE`) so Esc arrives reliably (Decision 3).
- The start-up shortcut failure is shown as the main window's toast on next open, not a tray notification (Decision 6); follow-up named in Out of Scope.
- Result (left to the builder in the spec): silent on success, `TaskDialog` on failure (Decision 6).
