# Negotium Plan 2 — Overview UI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the first runnable Negotium window: live meters, a top-5 map strip, a stable app list, row actions (end task, end process tree, force close, efficiency mode, open file location, details), close-to-tray, and the restart-as-admin banner, in a Fluent / Mica Slint UI fed by a supervised background collector.

**Architecture:** A supervised worker thread owns the `Collector`, `Model`, `Ranker` and `History`, and turns each tick into a plain-data `Frame` (formatted strings, fractions, sparkline paths, row ids, action targets). The UI thread only applies frames to Slint models and forwards user input; all Windows calls for data and actions run off the UI thread (actions on short-lived worker threads in `negotium-actions`). Slint renders the window (winit + FemtoVG), with Mica and accent colour applied once at window setup.

**Tech Stack:** Rust stable (MSVC, edition 2024), `slint` 1.18.1 (features `raw-window-handle-06` + defaults incl. `system-tray`), `slint-build` 1.18.1, `i-slint-backend-testing` 1.18.1 (dev), `raw-window-handle` 0.6.2, `windows` 0.62.2, `serde`/`serde_json` 1.0.

**Spec:** `docs/superpowers/specs/2026-10-14-negotium-design.md` — §4 (architecture, collector, model, history), §5.1 (Overview), §5.5 (tray), §6 (default elevation only), §7 (errors), §8 (benchmark), §9 (testing). Plan 1 follow-ups: `docs/superpowers/plans/2026-10-14-negotium-data-core-followups.md`.

**Out of scope (later plans):** Performance / Startup / Settings pages, history scrub, and the GPU temperature label on the GPU meter (Plan 3, together with the GPU performance page); Ctrl+Shift+Esc redirect, no-prompt admin task, installer, CPU-temperature add-on, per-app network (SRUM), single-instance enforcement (Plan 4); app icons in rows; collector tick-cost reduction (decided after the Task Manager baseline).

## Global Constraints

- Windows 11 x64 only; Rust stable, `x86_64-pc-windows-msvc`, edition 2024.
- New dependencies limited to: `slint = { version = "1.18.1", features = ["raw-window-handle-06"] }`, `slint-build = "1.18.1"`, `i-slint-backend-testing = "1.18.1"` (dev only), `raw-window-handle = "0.6.2"`. Existing: `windows = "0.62.2"`, `serde`, `serde_json`, `anyhow` (binaries only). Anything else requires changing this plan.
- `unsafe` only in `negotium-collector`, `negotium-actions`, `negotium-bench`, and `crates/app/src/platform.rs`; every `unsafe` block has its own `// SAFETY:` comment.
- **The UI thread never collects data or performs an action.** Data collection and model work run on the worker thread; actions run on spawned action threads. The only Windows calls allowed on the UI thread are one-time window setup in `platform.rs` (DWM attributes, accent colour read) at startup.
- A failing data source never panics; a panicking collector is restarted with backoff (spec §7).
- Ending or changing a process always verifies identity by `(pid, create_time)` before acting, so a reused PID is never touched.
- Spec §5.1 behaviour: list re-sorts at most every 2 s, never while the pointer is over the list (except on metric change, ruling R5), hung apps pin to the top with a live "Not responding · Ns" timer, Force close = `WM_CLOSE`, wait 3 s, then terminate.
- Spec §6: critical processes (`csrss`, `wininit`, `winlogon`, `lsass`, `smss`, `services`, `System`) require a confirmation that states the consequence.
- Spec §5: native Windows 11 look — Mica backdrop, user accent colour, Segoe UI Variable, light and dark following the system.
- If a `windows` 0.62.2 or Slint 1.18 item path or signature differs from this plan, look it up (`https://microsoft.github.io/windows-docs-rs/doc/windows/`, `https://docs.slint.dev/latest/docs/rust/slint/`) and adapt without changing behaviour; note it in the report.

## Review Focus

1. **Acting on a PID that was reused since the last tick** → nothing happens to the new process; the user sees "already exited". Test in Task 4.
2. **Acting on a process that exited between tick and click** → one plain notification, no error storm, no panic. Test in Task 4.
3. **A collector that panics on every tick** → the app stays up, restarts are spaced by backoff (no tight loop), and the last good frame stays on screen. Test in Task 8.
4. **Empty or degenerate data** (search with no matches, all values zero, no GPU counters, a single process) → no division by zero, empty top strip, bars at 0, meters show "Unavailable" where data is `None`. Tests in Task 7.
5. **Frequent row churn with 300+ rows** → list updates in place (no full model reset that would jump the scroll position). Test in Task 10.

---

## File Structure

```
crates/collector/src/windows_scan.rs     MODIFY: map ghost windows to the hung app (R9)
crates/collector/src/identity.rs         MODIFY: FileDescription → Identity.description
crates/collector/src/bin/hang.rs         CREATE: manual-test helper (captioned window that stops responding)
crates/snapshot/src/lib.rs               MODIFY: ProcessSample.description
crates/model/src/grouping.rs             MODIFY: packaged system components → Windows; display names
crates/model/src/lib.rs                  MODIFY: sticky app identity; ProcRow.parent_pid
crates/model/src/tree.rs                 CREATE: descendants_deepest_first
crates/actions/                          CREATE: negotium-actions
  src/lib.rs                             ActionError, verify-and-open helpers, re-exports
  src/terminate.rs                       end_process, end_many
  src/close.rs                           force_close
  src/efficiency.rs                      toggle_efficiency_mode
  src/details.rs                         process_details (command line, user, priority)
  src/elevation.rs                       is_elevated, restart_as_admin
  src/location.rs                        open_file_location
  tests/actions.rs                       integration tests on throwaway processes
crates/app/                              CREATE: negotium (the GUI binary + testable lib)
  Cargo.toml, build.rs
  assets/negotium.svg                    tray / window icon
  ui/theme.slint, ui/meters.slint, ui/topstrip.slint, ui/applist.slint,
  ui/overlays.slint, ui/tray.slint, ui/main.slint
  src/lib.rs                             module wiring + slint::include_modules!
  src/format.rs                          pure text formatting
  src/view.rs                            pure Frame building + visible_rows
  src/controls.rs                        shared atomics between UI and worker
  src/log.rs                             warnings log (spec §7)
  src/worker.rs                          supervised worker + LiveSource
  src/bridge.rs                          Frame → Slint models (in-place sync)
  src/dispatch.rs                        UI action → action thread → notification
  src/platform.rs                        HWND, Mica, dark title bar, accent colour
  src/framestats.rs                      optional render-time stats for the bench
  src/main.rs                            startup wiring
  tests/ui.rs                            Slint testing-backend tests
crates/bench/src/main.rs                 MODIFY: `negotium` target, --exit-on-close, frame stats
docs/benchmarks/README.md                MODIFY: Negotium column
```

---

### Task 1: Ghost windows map to the hung app (ruling R9)

**Files:**
- Modify: `crates/collector/src/windows_scan.rs`
- Create: `crates/collector/src/bin/hang.rs`
- Modify: `crates/collector/Cargo.toml` (new `[[bin]]`)
- Test: unit test in `windows_scan.rs`

**Interfaces:**
- Consumes: `scan_windows() -> Result<Vec<WindowSample>, String>` (existing).
- Produces: same signature; windows of class `Ghost` are reported as `WindowSample { pid: <hung app pid>, hung: true, title }` (via `HungWindowFromGhostWindow`), never as `dwm.exe`. Binary `negotium-hang [seconds-responsive]` for manual verification.

Background: when a hung, captioned window receives user input, Windows hides it and shows a "ghost" copy (class `Ghost`) owned by `dwm.exe`. Task Manager imports `HungWindowFromGhostWindow` from user32 for exactly this mapping (see `docs/research/taskmgr-26100-notes.md`). It is exported by user32 but not in the `windows` crate, so declare it.

- [ ] **Step 1: Write the failing unit test** (bottom of `windows_scan.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ghost_class_name_is_recognised() {
        assert!(is_ghost_class(&"Ghost".encode_utf16().collect::<Vec<_>>()));
        assert!(!is_ghost_class(&"Chrome_WidgetWin_1".encode_utf16().collect::<Vec<_>>()));
        assert!(!is_ghost_class(&[]));
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p negotium-collector --lib windows_scan`
Expected: FAIL to compile ("cannot find function `is_ghost_class`").

- [ ] **Step 3: Implement ghost mapping**

Add to `windows_scan.rs` (keep existing code; change `visit` as shown):
```rust
use windows::Win32::UI::WindowsAndMessaging::GetClassNameW;

#[link(name = "user32")]
unsafe extern "system" {
    /// Exported by user32 (used by Task Manager); returns the hung window a ghost stands in for.
    fn HungWindowFromGhostWindow(hwnd: HWND) -> HWND;
}

pub(crate) fn is_ghost_class(class: &[u16]) -> bool {
    class == "Ghost".encode_utf16().collect::<Vec<_>>().as_slice()
}
```

In `visit`, after the owner check and before reading the title, replace the PID/hung lines with:
```rust
        let mut class = [0u16; 64];
        let class_len = GetClassNameW(hwnd, &mut class).max(0) as usize;
        let (source, hung) = if is_ghost_class(&class[..class_len]) {
            let original = HungWindowFromGhostWindow(hwnd);
            if original.is_invalid() { (hwnd, false) } else { (original, true) }
        } else {
            (hwnd, IsHungAppWindow(hwnd).as_bool())
        };
        let mut pid = 0u32;
        GetWindowThreadProcessId(source, Some(&mut pid));
        out.push(WindowSample { pid, hung, title });
```
Keep the existing SAFETY comment on the block and extend it: `GetClassNameW` reads the window's class atom and `HungWindowFromGhostWindow` reads user32's ghost table; neither sends a window message.

`crates/collector/src/bin/hang.rs`:
```rust
//! Manual-test helper: shows a captioned window that responds for N seconds (default 3),
//! then stops processing messages for 60 s. Click it once it says "Not Responding" to make
//! Windows ghost it; `negotium-probe` should then list `negotium-hang` as NOT RESPONDING.
use std::time::{Duration, Instant};
use windows::Win32::UI::WindowsAndMessaging::{
    CW_USEDEFAULT, CreateWindowExW, DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage,
    WINDOW_EX_STYLE, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
};
use windows::core::w;

fn main() {
    let responsive: u64 = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(3);
    // SAFETY: creates a plain STATIC window owned by this thread.
    let _hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0), w!("STATIC"), w!("negotium-hang (click me once frozen)"),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE, CW_USEDEFAULT, CW_USEDEFAULT, 480, 200, None, None, None, None,
        )
    }
    .expect("create window");
    let until = Instant::now() + Duration::from_secs(responsive);
    let mut msg = MSG::default();
    while Instant::now() < until {
        // SAFETY: standard message pump on the owning thread.
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    std::thread::sleep(Duration::from_secs(60));
}
```

`crates/collector/Cargo.toml`: add
```toml
[[bin]]
name = "negotium-hang"
path = "src/bin/hang.rs"
test = false
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p negotium-collector`
Expected: all pass (existing + 1 new), warning-free.

- [ ] **Step 5: Record the manual check for the human partner**

Append to `docs/benchmarks/README.md` a section `## Manual check: frozen windows` with these exact steps:
```markdown
1. `cargo run -p negotium-collector --bin negotium-hang --release`
2. In another terminal: `cargo run -p negotium-probe --release -- --ticks 30`
3. After ~8 s the window title shows "Not Responding"; click inside it once.
4. Expected: the probe lists `negotium-hang` with `NOT RESPONDING` on every tick after that, and no `dwm` app row appears.
```

- [ ] **Step 6: Commit**

```bash
git add crates/collector docs/benchmarks/README.md
git commit -m "feat(collector): attribute ghost windows to the hung app"
```

---

### Task 2: Friendly app names from FileDescription

**Files:**
- Modify: `crates/snapshot/src/lib.rs` (field + every test literal)
- Modify: `crates/collector/src/identity.rs`, `crates/collector/src/collector.rs`, `crates/collector/Cargo.toml`
- Modify: `crates/model/src/grouping.rs` (`display_name`), test literals in `crates/model/src/{rates,grouping,lib}.rs`
- Test: `crates/collector/tests/identity.rs`, `crates/model/src/grouping.rs` tests

**Interfaces:**
- Produces: `ProcessSample.description: Option<String>` (`#[serde(default)]`); `Identity.description: Option<String>`; `display_name` prefers the root process's non-empty description, then the existing rules.

- [ ] **Step 1: Write the failing tests**

`crates/collector/tests/identity.rs` add (PING.EXE is used because it stays alive and, unlike Windows 11's `notepad.exe` stub, never hands off to another process):
```rust
#[test]
fn reads_file_description_of_a_system_binary() {
    let mut child = std::process::Command::new(r"C:\Windows\System32\PING.EXE")
        .args(["-n", "5", "127.0.0.1"])
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(300));
    let ct = ProcessReader::new().read().unwrap().into_iter().find(|p| p.pid == child.id()).unwrap().create_time;
    let id = IdentityCache::new().get(child.id(), ct).clone();
    child.kill().ok();
    child.wait().ok();
    assert_eq!(id.description.as_deref(), Some("TCP/IP Ping Command"));
}
```

`crates/model/src/grouping.rs` tests add:
```rust
    #[test]
    fn display_name_prefers_root_description() {
        let mut chrome = p(20, 1, 10, "chrome.exe", 1);
        chrome.description = Some("Google Chrome".into());
        let mut blank = p(21, 1, 11, "tool.exe", 1);
        blank.description = Some("   ".into());
        let procs = vec![chrome, blank];
        assert_eq!(display_name(&AppKey::Window { pid: 20, create_time: 10 }, &procs), "Google Chrome");
        assert_eq!(display_name(&AppKey::Background { pid: 21, create_time: 11 }, &procs), "tool");
    }
```

- [ ] **Step 2: Add the field everywhere and verify RED**

In `crates/snapshot/src/lib.rs` add to `ProcessSample` (after `package_family`):
```rust
    /// FileDescription from the executable's version resource (e.g. "Google Chrome").
    #[serde(default)]
    pub description: Option<String>,
```
Add `description: None,` to every `ProcessSample { .. }` literal (find them with `grep -rn "ProcessSample {" crates`: snapshot tests, collector.rs, model rates/grouping/lib tests).

Run: `cargo test -p negotium-collector --test identity` and `cargo test -p negotium-model grouping`
Expected: FAIL (no `description` on `Identity`; display_name ignores description).

- [ ] **Step 3: Implement**

`crates/collector/Cargo.toml`: add `"Win32_Storage_FileSystem"` to the `windows` features.

`crates/collector/src/identity.rs`:
- add `pub description: Option<String>` to `Identity`;
- in `query`, set `description: image_path.as_deref().and_then(file_description)` (compute `image_path` first, then build the struct);
- add:
```rust
use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
use windows::core::{HSTRING, PCWSTR};

/// FileDescription for the first translation in the version resource, trimmed; `None` if absent.
fn file_description(path: &str) -> Option<String> {
    let path = HSTRING::from(path);
    // SAFETY: size query on a NUL-terminated path.
    let size = unsafe { GetFileVersionInfoSizeW(&path, None) };
    if size == 0 {
        return None;
    }
    let mut data = vec![0u8; size as usize];
    // SAFETY: `data` has `size` writable bytes.
    unsafe { GetFileVersionInfoW(&path, None, size, data.as_mut_ptr().cast()) }.ok()?;
    let mut ptr = std::ptr::null_mut();
    let mut len = 0u32;
    // SAFETY: queries the translation table inside `data`; ptr points into `data`.
    if !unsafe { VerQueryValueW(data.as_ptr().cast(), windows::core::w!("\\VarFileInfo\\Translation"), &mut ptr, &mut len) }.as_bool() || len < 4 {
        return None;
    }
    // SAFETY: at least one (language, codepage) pair of u16 is present (len >= 4).
    let (lang, cp) = unsafe { (*(ptr as *const u16), *(ptr as *const u16).add(1)) };
    let key = HSTRING::from(format!("\\StringFileInfo\\{lang:04x}{cp:04x}\\FileDescription"));
    // SAFETY: queries a string value inside `data`; len is in UTF-16 units including NUL.
    if !unsafe { VerQueryValueW(data.as_ptr().cast(), PCWSTR(key.as_ptr()), &mut ptr, &mut len) }.as_bool() || len == 0 {
        return None;
    }
    // SAFETY: `ptr` points to `len` UTF-16 units inside `data`.
    let units = unsafe { std::slice::from_raw_parts(ptr as *const u16, len as usize) };
    let end = units.iter().position(|&c| c == 0).unwrap_or(units.len());
    let text = String::from_utf16_lossy(&units[..end]).trim().to_string();
    (!text.is_empty()).then_some(text)
}
```
`crates/collector/src/collector.rs`: set `description: id.description,` when building `ProcessSample`.

`crates/model/src/grouping.rs` `display_name`: for `Window` and `Background` keys, look up the root process; if `description` is `Some` and non-blank after trim, return it; otherwise the existing `strip_exe(name)`. For `Package`, if any process in `processes` with that `package_family` has a non-blank description, return the first one's; else the existing family-prefix rule.

- [ ] **Step 4: Run tests**

Run: `cargo test --workspace`
Expected: all pass, warning-free.

- [ ] **Step 5: Commit**

```bash
git add crates
git commit -m "feat: show apps by their FileDescription (e.g. Google Chrome)"
```

---

### Task 3: Model refinements — system packages, sticky identity, process tree

**Files:**
- Modify: `crates/model/src/grouping.rs`, `crates/model/src/lib.rs`
- Create: `crates/model/src/tree.rs`
- Test: unit tests in those files

**Interfaces:**
- Produces:
  - Rule change: a packaged process whose `image_path` is under `%SystemRoot%\` (e.g. `C:\Windows\SystemApps\…` StartMenuExperienceHost, SearchHost) → `AppKey::Windows`.
  - `ProcRow.parent_pid: u32`.
  - Sticky identity: a process that was last grouped under `AppKey::Window { pid, create_time }` keeps that key while both it and that root are still alive, even if the root loses its window (minimised to tray).
  - `pub fn descendants_deepest_first(procs: &[(u32, i64, u32)], root: (u32, i64)) -> Vec<(u32, i64)>` — `(pid, create_time, parent_pid)` triples in; root's descendants ordered deepest first, root last; a child counts only if `child.create_time >= parent.create_time`; cycle-safe.

- [ ] **Step 1: Write the failing tests**

`grouping.rs` tests add:
```rust
    #[test]
    fn packaged_system_components_join_windows() {
        let mut start = p(70, 1, 1, "StartMenuExperienceHost.exe", 1);
        start.package_family = Some("Microsoft.Windows.StartMenuExperienceHost_cw5n1h2txyewy".into());
        start.image_path = Some(r"C:\Windows\SystemApps\Microsoft.Windows.StartMenuExperienceHost_cw5n1h2txyewy\StartMenuExperienceHost.exe".into());
        let mut term = p(71, 1, 1, "WindowsTerminal.exe", 1);
        term.package_family = Some("Microsoft.WindowsTerminal_8wekyb3d8bbwe".into());
        term.image_path = Some(r"C:\Program Files\WindowsApps\Microsoft.WindowsTerminal\WindowsTerminal.exe".into());
        let g = group(&[start, term], &[win(70), win(71)], SYS);
        assert_eq!(g[&(70, 1)], AppKey::Windows);
        assert_eq!(g[&(71, 1)], AppKey::Package("Microsoft.WindowsTerminal_8wekyb3d8bbwe".into()));
    }
```

`crates/model/src/tree.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orders_descendants_deepest_first_with_root_last() {
        // 10 → 11 → 12, 10 → 13; 14 unrelated; 15 claims parent 10 but is older (reused PID)
        let procs = [(10, 100, 1), (11, 110, 10), (12, 120, 11), (13, 130, 10), (14, 140, 1), (15, 50, 10)];
        let order = descendants_deepest_first(&procs, (10, 100));
        assert_eq!(order.last(), Some(&(10, 100)));
        assert_eq!(order[0], (12, 120));
        assert_eq!(order.len(), 4);
        assert!(!order.contains(&(14, 140)) && !order.contains(&(15, 50)));
    }

    #[test]
    fn cycles_terminate() {
        let procs = [(1, 5, 2), (2, 5, 1)];
        let order = descendants_deepest_first(&procs, (1, 5));
        assert_eq!(order.last(), Some(&(1, 5)));
        assert!(order.len() <= 2);
    }
}
```

`lib.rs` tests add:
```rust
    #[test]
    fn app_keeps_its_row_when_its_window_closes() {
        let mut m = Model::with_system_root(r"C:\Windows");
        let snap = |t: u64, with_window: bool| Snapshot {
            taken_at_ms: t,
            processes: vec![p(2, 1, 0, 0), {
                let mut helper = p(3, 2, 0, 0);
                helper.name = "helper.exe".into();
                helper
            }],
            windows: if with_window { vec![WindowSample { pid: 2, hung: false, title: "App".into() }] } else { vec![] },
            totals: SystemTotals { logical_cpus: 1, ..Default::default() },
            source_errors: vec![],
        };
        let before = m.update(&snap(0, true));
        let after = m.update(&snap(1000, false));
        assert_eq!(before.len(), 1);
        assert_eq!(after.len(), 1, "minimising to the tray must not split or rename the app");
        assert_eq!(after[0].key, before[0].key);
        assert_eq!(after[0].processes.iter().map(|p| p.parent_pid).collect::<Vec<_>>(), vec![1, 2]);
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p negotium-model`
Expected: FAIL to compile (`tree` module, `parent_pid` missing) — add `pub mod tree;` and `pub use tree::descendants_deepest_first;` to lib.rs first so the failure is about missing items.

- [ ] **Step 3: Implement**

`grouping.rs` in `group`: compute `let system = p.session_id == 0 || is_under(p, &system_prefix);` once; rule 1 becomes `if let Some(pfn) = &p.package_family { if is_under(p, &system_prefix) { AppKey::Windows } else { AppKey::Package(pfn.clone()) } }` (keep the remaining order unchanged). Update the module doc's rule list accordingly.

`tree.rs` (above tests):
```rust
//! Process-tree ordering for "End process tree": children before parents.
use std::collections::{HashMap, HashSet};

pub fn descendants_deepest_first(procs: &[(u32, i64, u32)], root: (u32, i64)) -> Vec<(u32, i64)> {
    let mut children: HashMap<u32, Vec<(u32, i64)>> = HashMap::new();
    for &(pid, ct, parent) in procs {
        if pid != parent {
            children.entry(parent).or_default().push((pid, ct));
        }
    }
    let mut seen: HashSet<(u32, i64)> = HashSet::from([root]);
    let mut levels: Vec<Vec<(u32, i64)>> = vec![vec![root]];
    loop {
        let mut next = Vec::new();
        for &(pid, ct) in levels.last().unwrap() {
            for &(cpid, cct) in children.get(&pid).map(Vec::as_slice).unwrap_or(&[]) {
                if cct >= ct && seen.insert((cpid, cct)) {
                    next.push((cpid, cct));
                }
            }
        }
        if next.is_empty() {
            break;
        }
        levels.push(next);
    }
    levels.into_iter().rev().flatten().collect()
}
```

`lib.rs`:
- `ProcRow` gains `pub parent_pid: u32` (set from `p.parent_pid` in `update`).
- `Model` gains `sticky: HashMap<(u32, i64), AppKey>` (init empty in `with_system_root`).
- In `update`, after `let groups = group(...)`, make `groups` mutable and apply:
```rust
        // An app keeps its row while its root lives, even if its window closes
        // (minimise-to-tray), so rows don't change key, rank, or history id.
        let alive: std::collections::HashSet<(u32, i64)> =
            snap.processes.iter().map(|p| (p.pid, p.create_time)).collect();
        for (proc_key, key) in groups.iter_mut() {
            match key {
                AppKey::Window { .. } => {
                    self.sticky.insert(*proc_key, key.clone());
                }
                AppKey::Background { .. } => {
                    if let Some(prev @ AppKey::Window { pid, create_time }) = self.sticky.get(proc_key) {
                        if alive.contains(&(*pid, *create_time)) {
                            *key = prev.clone();
                        }
                    }
                }
                _ => {}
            }
        }
        self.sticky.retain(|k, _| alive.contains(k));
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p negotium-model` then `cargo test --workspace`
Expected: all pass, warning-free.

- [ ] **Step 5: Commit**

```bash
git add crates/model
git commit -m "feat(model): system packages under Windows, sticky app rows, process-tree order"
```

---

### Task 4: Actions crate — identity-checked termination and error messages

**Files:**
- Create: `crates/actions/Cargo.toml`, `crates/actions/src/lib.rs`, `crates/actions/src/terminate.rs`
- Modify: root `Cargo.toml` (`negotium-actions = { path = "crates/actions" }` in `[workspace.dependencies]`)
- Test: `crates/actions/tests/actions.rs`, unit tests in `lib.rs`

**Interfaces:**
- Produces (crate `negotium_actions`):
  - `pub enum ActionError { AlreadyExited, AccessDenied, Other(String) }` (derives `Clone, Debug, PartialEq, Eq`) with `pub fn message(&self, what: &str, elevated: bool) -> String`.
  - `pub const CRITICAL_PROCESSES: &[&str]` and `pub fn is_critical(name: &str) -> bool` (case-insensitive, `.exe` optional).
  - `pub fn end_process(pid: u32, create_time: i64) -> Result<(), ActionError>`
  - `pub fn end_many(targets: &[(u32, i64)]) -> Vec<((u32, i64), Result<(), ActionError>)>` (in the given order; already-exited targets are `Ok`).
  - `pub(crate) fn open_verified(pid: u32, create_time: i64, access: PROCESS_ACCESS_RIGHTS) -> Result<OwnedHandle, ActionError>` and `pub(crate) struct OwnedHandle(HANDLE)` closing on Drop.

- [ ] **Step 1: Create the crate**

`crates/actions/Cargo.toml`:
```toml
[package]
name = "negotium-actions"
edition.workspace = true
version.workspace = true
license.workspace = true
publish.workspace = true

[dependencies]
windows = { workspace = true, features = [
    "Win32_Foundation",
    "Win32_Security",
    "Win32_System_Threading",
    "Win32_UI_WindowsAndMessaging",
    "Win32_UI_Shell",
    "Win32_System_Registry",
] }

[dev-dependencies]
negotium-collector.workspace = true
```
Add `negotium-actions = { path = "crates/actions" }` and `negotium-collector` is already listed under `[workspace.dependencies]` in the root `Cargo.toml`.

- [ ] **Step 2: Write the failing tests**

Unit tests (bottom of `crates/actions/src/lib.rs`):
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn critical_names_match_with_or_without_exe() {
        assert!(is_critical("csrss.exe") && is_critical("LSASS") && is_critical("System"));
        assert!(!is_critical("chrome.exe"));
    }

    #[test]
    fn messages_are_plain_english() {
        assert_eq!(ActionError::AlreadyExited.message("Notepad", false), "Notepad already exited.");
        assert_eq!(
            ActionError::AccessDenied.message("svchost.exe", false),
            "Couldn't change svchost.exe: access denied. Restart Negotium as admin and try again."
        );
        assert_eq!(
            ActionError::AccessDenied.message("svchost.exe", true),
            "Couldn't change svchost.exe: Windows protects this process."
        );
    }
}
```

`crates/actions/tests/actions.rs`:
```rust
use negotium_actions::{ActionError, end_many, end_process};
use negotium_collector::ProcessReader;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

fn spawn_ping() -> (Child, i64) {
    let child = Command::new(r"C:\Windows\System32\PING.EXE")
        .args(["-n", "30", "127.0.0.1"])
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let ct = ProcessReader::new().read().unwrap().into_iter().find(|p| p.pid == child.id()).unwrap().create_time;
    (child, ct)
}

#[test]
fn ends_a_process_after_verifying_identity() {
    let (mut child, ct) = spawn_ping();
    assert_eq!(end_process(child.id(), ct), Ok(()));
    assert!(child.wait().unwrap().code().is_some());
}

#[test]
fn wrong_create_time_is_treated_as_already_exited_and_spares_the_process() {
    let (mut child, ct) = spawn_ping();
    assert_eq!(end_process(child.id(), ct + 1), Err(ActionError::AlreadyExited));
    assert!(child.try_wait().unwrap().is_none(), "the live process must not be touched");
    child.kill().ok();
    child.wait().ok();
}

#[test]
fn exited_process_reports_already_exited() {
    let (mut child, ct) = spawn_ping();
    child.kill().unwrap();
    child.wait().unwrap();
    assert_eq!(end_process(child.id(), ct), Err(ActionError::AlreadyExited));
}

#[test]
fn end_many_treats_already_exited_as_success() {
    let (mut a, act) = spawn_ping();
    let (mut b, bct) = spawn_ping();
    b.kill().unwrap();
    b.wait().unwrap();
    let results = end_many(&[(a.id(), act), (b.id(), bct)]);
    assert!(results.iter().all(|(_, r)| r.is_ok()), "{results:?}");
    a.wait().unwrap();
}
```

- [ ] **Step 3: Run to verify they fail**

Run: `cargo test -p negotium-actions`
Expected: FAIL to compile (`ActionError` etc. missing).

- [ ] **Step 4: Implement**

`crates/actions/src/lib.rs` (above tests):
```rust
//! Process actions. Every call verifies identity by (pid, create_time) before acting,
//! so a PID reused since the last tick is never touched. Call off the UI thread.
mod terminate;

pub use terminate::{end_many, end_process};

use windows::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, ERROR_INVALID_PARAMETER, FILETIME, HANDLE};
use windows::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, PROCESS_ACCESS_RIGHTS, PROCESS_QUERY_LIMITED_INFORMATION,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActionError {
    AlreadyExited,
    AccessDenied,
    Other(String),
}

impl ActionError {
    pub fn message(&self, what: &str, elevated: bool) -> String {
        match self {
            ActionError::AlreadyExited => format!("{what} already exited."),
            ActionError::AccessDenied if elevated => format!("Couldn't change {what}: Windows protects this process."),
            ActionError::AccessDenied => {
                format!("Couldn't change {what}: access denied. Restart Negotium as admin and try again.")
            }
            ActionError::Other(detail) => format!("Couldn't change {what}: {detail}."),
        }
    }

    pub(crate) fn from_win(e: &windows::core::Error) -> Self {
        let code = e.code();
        if code == ERROR_ACCESS_DENIED.to_hresult() {
            ActionError::AccessDenied
        } else if code == ERROR_INVALID_PARAMETER.to_hresult() {
            ActionError::AlreadyExited
        } else {
            ActionError::Other(e.message().trim().to_string())
        }
    }
}

pub const CRITICAL_PROCESSES: &[&str] = &["csrss", "wininit", "winlogon", "lsass", "smss", "services", "System"];

pub fn is_critical(name: &str) -> bool {
    let stem = name.strip_suffix(".exe").or_else(|| name.strip_suffix(".EXE")).unwrap_or(name);
    CRITICAL_PROCESSES.iter().any(|c| c.eq_ignore_ascii_case(stem))
}

pub(crate) struct OwnedHandle(pub(crate) HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: the handle came from OpenProcess and is closed exactly once here.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// Opens `pid` with `access` (plus query rights) and checks its creation time.
pub(crate) fn open_verified(pid: u32, create_time: i64, access: PROCESS_ACCESS_RIGHTS) -> Result<OwnedHandle, ActionError> {
    // SAFETY: plain Win32 call; the handle is owned by OwnedHandle.
    let handle = unsafe { OpenProcess(access | PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
        .map(OwnedHandle)
        .map_err(|e| ActionError::from_win(&e))?;
    let (mut created, mut exited, mut kernel, mut user) =
        (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    // SAFETY: valid handle and four writable FILETIMEs.
    unsafe { GetProcessTimes(handle.0, &mut created, &mut exited, &mut kernel, &mut user) }
        .map_err(|e| ActionError::from_win(&e))?;
    let actual = ((created.dwHighDateTime as i64) << 32) | created.dwLowDateTime as i64;
    if actual != create_time {
        return Err(ActionError::AlreadyExited);
    }
    Ok(handle)
}
```

`crates/actions/src/terminate.rs`:
```rust
use crate::{ActionError, open_verified};
use windows::Win32::System::Threading::{PROCESS_TERMINATE, TerminateProcess};

pub fn end_process(pid: u32, create_time: i64) -> Result<(), ActionError> {
    let handle = open_verified(pid, create_time, PROCESS_TERMINATE)?;
    // SAFETY: verified handle with PROCESS_TERMINATE.
    unsafe { TerminateProcess(handle.0, 1) }.map_err(|e| ActionError::from_win(&e))
}

pub fn end_many(targets: &[(u32, i64)]) -> Vec<((u32, i64), Result<(), ActionError>)> {
    targets
        .iter()
        .map(|&(pid, ct)| {
            let r = match end_process(pid, ct) {
                Err(ActionError::AlreadyExited) => Ok(()),
                other => other,
            };
            ((pid, ct), r)
        })
        .collect()
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p negotium-actions`
Expected: 2 unit + 4 integration tests pass, warning-free.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock crates/actions
git commit -m "feat(actions): identity-checked process termination with plain-English errors"
```

---

### Task 5: Actions — force close, efficiency mode, open file location

**Files:**
- Create: `crates/actions/src/close.rs`, `crates/actions/src/efficiency.rs`, `crates/actions/src/location.rs`
- Modify: `crates/actions/src/lib.rs` (mods + re-exports)
- Test: `crates/actions/tests/actions.rs`

**Interfaces:**
- Consumes: `open_verified`, `OwnedHandle`, `ActionError` (Task 4).
- Produces:
  - `pub const FORCE_CLOSE_GRACE: Duration = Duration::from_secs(3);`
  - `pub fn force_close(pid: u32, create_time: i64) -> Result<(), ActionError>` — posts `WM_CLOSE` to every visible top-level window of `pid`, waits up to `FORCE_CLOSE_GRACE` on the process handle, then terminates.
  - `pub fn toggle_efficiency_mode(targets: &[(u32, i64)]) -> Result<bool, ActionError>` — reads the first target's state, sets all targets to the opposite; returns the new state (`true` = on). On = `ProcessPowerThrottling` EXECUTION_SPEED on + `IDLE_PRIORITY_CLASS`; off = throttling cleared + `NORMAL_PRIORITY_CLASS`. Already-exited targets after the first are skipped.
  - `pub fn open_file_location(path: &str) -> Result<(), ActionError>` — `explorer.exe /select,"<path>"`.

- [ ] **Step 1: Write the failing tests** (append to `tests/actions.rs`)

```rust
use negotium_actions::{force_close, toggle_efficiency_mode};

#[test]
fn force_close_terminates_a_windowless_process_after_the_grace_period() {
    let (mut child, ct) = spawn_ping();
    let started = std::time::Instant::now();
    assert_eq!(force_close(child.id(), ct), Ok(()));
    assert!(child.wait().unwrap().code().is_some());
    assert!(started.elapsed() >= Duration::from_millis(2900), "should wait the 3 s grace first");
}

#[test]
fn efficiency_mode_toggles_on_then_off() {
    let (mut child, ct) = spawn_ping();
    assert_eq!(toggle_efficiency_mode(&[(child.id(), ct)]), Ok(true));
    assert_eq!(toggle_efficiency_mode(&[(child.id(), ct)]), Ok(false));
    child.kill().ok();
    child.wait().ok();
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p negotium-actions`
Expected: FAIL to compile.

- [ ] **Step 3: Implement**

`close.rs`:
```rust
use crate::{ActionError, open_verified};
use std::time::Duration;
use windows::Win32::Foundation::{HWND, LPARAM, WAIT_OBJECT_0, WPARAM};
use windows::Win32::System::Threading::{PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, TerminateProcess, WaitForSingleObject};
use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowThreadProcessId, IsWindowVisible, PostMessageW, WM_CLOSE};
use windows::core::BOOL;

pub const FORCE_CLOSE_GRACE: Duration = Duration::from_secs(3);

pub fn force_close(pid: u32, create_time: i64) -> Result<(), ActionError> {
    let handle = open_verified(pid, create_time, PROCESS_TERMINATE | PROCESS_SYNCHRONIZE)?;
    for hwnd in windows_of(pid) {
        // SAFETY: asks the window to close; PostMessage never blocks on a hung window.
        unsafe {
            let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }
    // SAFETY: verified handle opened with SYNCHRONIZE.
    if unsafe { WaitForSingleObject(handle.0, FORCE_CLOSE_GRACE.as_millis() as u32) } == WAIT_OBJECT_0 {
        return Ok(());
    }
    // SAFETY: verified handle opened with PROCESS_TERMINATE.
    unsafe { TerminateProcess(handle.0, 1) }.map_err(|e| ActionError::from_win(&e))
}

fn windows_of(pid: u32) -> Vec<HWND> {
    struct Search {
        pid: u32,
        found: Vec<HWND>,
    }
    unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: lparam is the `&mut Search` passed below for the duration of EnumWindows.
        let search = unsafe { &mut *(lparam.0 as *mut Search) };
        let mut owner = 0u32;
        // SAFETY: read-only queries on a handle supplied by EnumWindows.
        unsafe {
            GetWindowThreadProcessId(hwnd, Some(&mut owner));
            if owner == search.pid && IsWindowVisible(hwnd).as_bool() {
                search.found.push(hwnd);
            }
        }
        true.into()
    }
    let mut search = Search { pid, found: Vec::new() };
    // SAFETY: `search` outlives the synchronous enumeration.
    unsafe {
        let _ = EnumWindows(Some(visit), LPARAM(&mut search as *mut Search as isize));
    }
    search.found
}
```

`efficiency.rs`:
```rust
use crate::{ActionError, open_verified};
use std::ffi::c_void;
use std::mem::size_of;
use windows::Win32::System::Threading::{
    GetProcessInformation, IDLE_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
    PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_STATE, PROCESS_SET_INFORMATION,
    ProcessPowerThrottling, SetPriorityClass, SetProcessInformation,
};

pub fn toggle_efficiency_mode(targets: &[(u32, i64)]) -> Result<bool, ActionError> {
    let Some(&(pid, ct)) = targets.first() else { return Ok(false) };
    let enable = !is_on(pid, ct)?;
    set(pid, ct, enable)?;
    for &(pid, ct) in &targets[1..] {
        match set(pid, ct, enable) {
            Ok(()) | Err(ActionError::AlreadyExited) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(enable)
}

fn is_on(pid: u32, ct: i64) -> Result<bool, ActionError> {
    let handle = open_verified(pid, ct, Default::default())?;
    let mut state = PROCESS_POWER_THROTTLING_STATE { Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION, ..Default::default() };
    // SAFETY: `state` is a writable PROCESS_POWER_THROTTLING_STATE of the size passed.
    unsafe {
        GetProcessInformation(handle.0, ProcessPowerThrottling, &mut state as *mut _ as *mut c_void, size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32)
    }
    .map_err(|e| ActionError::from_win(&e))?;
    Ok(state.ControlMask & PROCESS_POWER_THROTTLING_EXECUTION_SPEED != 0
        && state.StateMask & PROCESS_POWER_THROTTLING_EXECUTION_SPEED != 0)
}

fn set(pid: u32, ct: i64, enable: bool) -> Result<(), ActionError> {
    let handle = open_verified(pid, ct, PROCESS_SET_INFORMATION)?;
    let state = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        StateMask: if enable { PROCESS_POWER_THROTTLING_EXECUTION_SPEED } else { 0 },
    };
    // SAFETY: `state` is a valid PROCESS_POWER_THROTTLING_STATE of the size passed.
    unsafe {
        SetProcessInformation(handle.0, ProcessPowerThrottling, &state as *const _ as *const c_void, size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32)
    }
    .map_err(|e| ActionError::from_win(&e))?;
    let class = if enable { IDLE_PRIORITY_CLASS } else { NORMAL_PRIORITY_CLASS };
    // SAFETY: verified handle opened with PROCESS_SET_INFORMATION.
    unsafe { SetPriorityClass(handle.0, class) }.map_err(|e| ActionError::from_win(&e))
}
```
(If `PROCESS_POWER_THROTTLING_CURRENT_VERSION` / `PROCESS_POWER_THROTTLING_EXECUTION_SPEED` are not exported by `windows` 0.62.2, define them locally as `u32` constants `1` and `1`.)

`location.rs`:
```rust
use crate::ActionError;
use std::process::Command;

/// Opens File Explorer with the file selected.
pub fn open_file_location(path: &str) -> Result<(), ActionError> {
    Command::new("explorer.exe")
        .arg(format!("/select,\"{path}\""))
        .spawn()
        .map(|_| ())
        .map_err(|e| ActionError::Other(e.to_string()))
}
```

`lib.rs`: add `mod close; mod efficiency; mod location;` and `pub use close::{FORCE_CLOSE_GRACE, force_close}; pub use efficiency::toggle_efficiency_mode; pub use location::open_file_location;`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p negotium-actions`
Expected: all pass (the force-close test takes ~3 s), warning-free.

- [ ] **Step 5: Commit**

```bash
git add crates/actions
git commit -m "feat(actions): force close, efficiency mode, open file location"
```

---

### Task 6: Actions — process details and elevation

**Files:**
- Create: `crates/actions/src/details.rs`, `crates/actions/src/elevation.rs`
- Modify: `crates/actions/src/lib.rs`
- Test: `crates/actions/tests/actions.rs`

**Interfaces:**
- Produces:
  - `pub struct Details { pub pid: u32, pub image_path: Option<String>, pub command_line: Option<String>, pub user: Option<String>, pub priority: Option<String> }` (derives `Clone, Debug, PartialEq`)
  - `pub fn process_details(pid: u32, create_time: i64) -> Result<Details, ActionError>` — fields that can't be read are `None` (never an error unless the process is gone or identity fails).
  - `pub fn is_elevated() -> bool`
  - `pub fn restart_as_admin(args: &str) -> Result<(), ActionError>` — relaunches the current exe with the `runas` verb; the caller quits on `Ok`.

- [ ] **Step 1: Write the failing tests** (append)

```rust
use negotium_actions::{is_elevated, process_details};

#[test]
fn details_include_command_line_user_and_priority() {
    let (mut child, ct) = spawn_ping();
    let d = process_details(child.id(), ct).unwrap();
    child.kill().ok();
    child.wait().ok();
    assert_eq!(d.pid, child.id());
    assert!(d.command_line.as_deref().unwrap_or("").contains("127.0.0.1"), "{d:?}");
    assert!(d.user.as_deref().unwrap_or("").contains('\\'), "DOMAIN\\user expected: {d:?}");
    assert_eq!(d.priority.as_deref(), Some("Normal"));
    assert!(d.image_path.as_deref().unwrap_or("").to_lowercase().ends_with("ping.exe"));
}

#[test]
fn elevation_query_does_not_panic() {
    let _ = is_elevated();
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p negotium-actions`
Expected: FAIL to compile.

- [ ] **Step 3: Implement**

`details.rs`:
```rust
use crate::{ActionError, OwnedHandle, open_verified};
use std::ffi::c_void;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Security::{GetTokenInformation, LookupAccountSidW, SID_NAME_USE, TOKEN_QUERY, TOKEN_USER, TokenUser};
use windows::Win32::System::Threading::{
    ABOVE_NORMAL_PRIORITY_CLASS, BELOW_NORMAL_PRIORITY_CLASS, GetPriorityClass, HIGH_PRIORITY_CLASS, IDLE_PRIORITY_CLASS,
    NORMAL_PRIORITY_CLASS, OpenProcessToken, PROCESS_NAME_WIN32, QueryFullProcessImageNameW, REALTIME_PRIORITY_CLASS,
};
use windows::core::PWSTR;

#[derive(Clone, Debug, PartialEq)]
pub struct Details {
    pub pid: u32,
    pub image_path: Option<String>,
    pub command_line: Option<String>,
    pub user: Option<String>,
    pub priority: Option<String>,
}

const PROCESS_COMMAND_LINE_INFORMATION: u32 = 60;

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQueryInformationProcess(process: HANDLE, class: u32, info: *mut c_void, len: u32, ret: *mut u32) -> i32;
}

pub fn process_details(pid: u32, create_time: i64) -> Result<Details, ActionError> {
    let h = open_verified(pid, create_time, Default::default())?;
    Ok(Details { pid, image_path: image_path(&h), command_line: command_line(&h), user: user(&h), priority: priority(&h) })
}

fn image_path(h: &OwnedHandle) -> Option<String> {
    let mut buf = vec![0u16; 32_768];
    let mut len = buf.len() as u32;
    // SAFETY: `buf` has `len` writable UTF-16 units.
    unsafe { QueryFullProcessImageNameW(h.0, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len) }.ok()?;
    Some(String::from_utf16_lossy(&buf[..len as usize]))
}

fn command_line(h: &OwnedHandle) -> Option<String> {
    let mut needed = 0u32;
    // SAFETY: size query; returns STATUS_INFO_LENGTH_MISMATCH with `needed` set.
    unsafe { NtQueryInformationProcess(h.0, PROCESS_COMMAND_LINE_INFORMATION, std::ptr::null_mut(), 0, &mut needed) };
    if needed == 0 {
        return None;
    }
    let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
    // SAFETY: `buf` is 8-byte aligned and at least `needed` bytes.
    let status = unsafe {
        NtQueryInformationProcess(h.0, PROCESS_COMMAND_LINE_INFORMATION, buf.as_mut_ptr().cast(), needed, &mut needed)
    };
    if status < 0 {
        return None;
    }
    // The buffer starts with a UNICODE_STRING { u16 len_bytes, u16 max, *const u16 } whose
    // buffer points into the same allocation.
    let base = buf.as_ptr() as *const u8;
    // SAFETY: the kernel wrote a UNICODE_STRING header at the start of `buf`.
    let (len_bytes, ptr) = unsafe { (*(base as *const u16), *(base.add(8) as *const *const u16)) };
    if ptr.is_null() || len_bytes == 0 {
        return None;
    }
    let start = ptr as usize;
    let end = start + len_bytes as usize;
    if start < base as usize || end > base as usize + buf.len() * 8 {
        return None;
    }
    // SAFETY: bounds checked against `buf` above.
    let units = unsafe { std::slice::from_raw_parts(ptr, len_bytes as usize / 2) };
    Some(String::from_utf16_lossy(units))
}

fn user(h: &OwnedHandle) -> Option<String> {
    let mut token = HANDLE::default();
    // SAFETY: opens the process token for query; wrapped in OwnedHandle below.
    unsafe { OpenProcessToken(h.0, TOKEN_QUERY, &mut token) }.ok()?;
    let token = OwnedHandle(token);
    let mut needed = 0u32;
    // SAFETY: size query.
    let _ = unsafe { GetTokenInformation(token.0, TokenUser, None, 0, &mut needed) };
    let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
    // SAFETY: `buf` is aligned and at least `needed` bytes.
    unsafe { GetTokenInformation(token.0, TokenUser, Some(buf.as_mut_ptr().cast()), needed, &mut needed) }.ok()?;
    // SAFETY: the buffer now holds a TOKEN_USER.
    let sid = unsafe { (*(buf.as_ptr() as *const TOKEN_USER)).User.Sid };
    let (mut name, mut domain) = ([0u16; 256], [0u16; 256]);
    let (mut name_len, mut domain_len) = (name.len() as u32, domain.len() as u32);
    let mut use_ = SID_NAME_USE::default();
    // SAFETY: buffers and lengths are valid; `sid` points into `buf`, which is alive.
    unsafe {
        LookupAccountSidW(None, sid, Some(PWSTR(name.as_mut_ptr())), &mut name_len, Some(PWSTR(domain.as_mut_ptr())), &mut domain_len, &mut use_)
    }
    .ok()?;
    Some(format!(
        "{}\\{}",
        String::from_utf16_lossy(&domain[..domain_len as usize]),
        String::from_utf16_lossy(&name[..name_len as usize])
    ))
}

fn priority(h: &OwnedHandle) -> Option<String> {
    // SAFETY: valid handle with query rights.
    let class = unsafe { GetPriorityClass(h.0) };
    let name = match class {
        c if c == IDLE_PRIORITY_CLASS.0 => "Low",
        c if c == BELOW_NORMAL_PRIORITY_CLASS.0 => "Below normal",
        c if c == NORMAL_PRIORITY_CLASS.0 => "Normal",
        c if c == ABOVE_NORMAL_PRIORITY_CLASS.0 => "Above normal",
        c if c == HIGH_PRIORITY_CLASS.0 => "High",
        c if c == REALTIME_PRIORITY_CLASS.0 => "Realtime",
        _ => return None,
    };
    Some(name.to_string())
}
```

`elevation.rs`:
```rust
use crate::{ActionError, OwnedHandle};
use std::ffi::c_void;
use std::mem::size_of;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::Win32::UI::Shell::{SHELLEXECUTEINFOW, ShellExecuteExW};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{HSTRING, PCWSTR, w};

pub fn is_elevated() -> bool {
    let mut token = HANDLE::default();
    // SAFETY: opens our own token for query.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.is_err() {
        return false;
    }
    let token = OwnedHandle(token);
    let mut elevation = TOKEN_ELEVATION::default();
    let mut len = 0u32;
    // SAFETY: `elevation` is a writable TOKEN_ELEVATION of the size passed.
    let ok = unsafe {
        GetTokenInformation(token.0, TokenElevation, Some(&mut elevation as *mut _ as *mut c_void), size_of::<TOKEN_ELEVATION>() as u32, &mut len)
    }
    .is_ok();
    ok && elevation.TokenIsElevated != 0
}

pub fn restart_as_admin(args: &str) -> Result<(), ActionError> {
    let exe = std::env::current_exe().map_err(|e| ActionError::Other(e.to_string()))?;
    let file = HSTRING::from(exe.as_os_str());
    let params = HSTRING::from(args);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    // SAFETY: `info` is initialised and its strings outlive the call.
    unsafe { ShellExecuteExW(&mut info) }.map_err(|e| ActionError::from_win(&e))
}
```

`lib.rs`: add `mod details; mod elevation;` and `pub use details::{Details, process_details}; pub use elevation::{is_elevated, restart_as_admin};`. Make `OwnedHandle`'s field `pub(crate)` constructor usable (it already is).

- [ ] **Step 4: Run tests**

Run: `cargo test -p negotium-actions`
Expected: all pass, warning-free.

- [ ] **Step 5: Commit**

```bash
git add crates/actions
git commit -m "feat(actions): process details and elevation helpers"
```

---

### Task 7: App crate — formatting and frame building (pure)

**Files:**
- Create: `crates/app/Cargo.toml`, `crates/app/src/lib.rs`, `crates/app/src/format.rs`, `crates/app/src/view.rs`
- Modify: root `Cargo.toml` (`[workspace.dependencies]`: `slint`, `slint-build`, `i-slint-backend-testing`, `raw-window-handle`, `negotium-actions`)
- Test: unit tests in `format.rs`, `view.rs`

**Interfaces:**
- Consumes: `negotium_model::{AppRow, AppKey, Metric, top_n}`, `negotium_history::History`, `negotium_snapshot::Snapshot`.
- Produces (crate `negotium`, lib):
  - `format::{bytes(f64) -> String, byte_rate(f64) -> String, bit_rate(f64) -> String, pct(f32) -> String, pct1(f32) -> String, hung(u64) -> String}`
  - `view::MeterView { label: &'static str, value: String, sub: String, spark: String, available: bool }`
  - `view::TileView { id: i32, name: String, value: String, weight: f32, hung: bool }`
  - `view::RowView { id: i32, parent: i32, depth: i32, name: String, search_text: String, count: String, cpu: String, mem: String, disk: String, gpu: String, net: String, bar: f32, hung: String, expandable: bool }` (`parent` = -1 for app rows)
  - `view::Target { name: String, root: Option<(u32, i64)>, procs: Vec<(u32, i64)>, hung_pid: Option<(u32, i64)>, image_path: Option<String>, critical: bool }`
  - `view::Frame { meters: Vec<MeterView>, tiles: Vec<TileView>, caption: String, rows: Vec<RowView>, targets: HashMap<i32, Target>, all_procs: Vec<(u32, i64, u32)>, admin_banner: bool }`
  - `view::IdMap` with `new()`; `view::build_frame(ids: &mut IdMap, snap: &Snapshot, rows: &[AppRow], order: &[usize], history: &History, metric: Metric, elevated: bool) -> Frame`
  - `view::visible_rows(rows: &[RowView], expanded: &HashSet<i32>, search: &str) -> Vec<RowView>`
  - `view::spark_path(values: &[Option<f32>], max: f32) -> String`

- [ ] **Step 1: Create the crate**

Root `Cargo.toml` `[workspace.dependencies]` add:
```toml
negotium-actions = { path = "crates/actions" }
slint = { version = "1.18.1", features = ["raw-window-handle-06"] }
slint-build = "1.18.1"
i-slint-backend-testing = "1.18.1"
raw-window-handle = "0.6.2"
```

`crates/app/Cargo.toml`:
```toml
[package]
name = "negotium"
edition.workspace = true
version.workspace = true
license.workspace = true
publish.workspace = true

[dependencies]
negotium-snapshot.workspace = true
negotium-collector.workspace = true
negotium-model.workspace = true
negotium-history.workspace = true
negotium-actions.workspace = true
slint.workspace = true
raw-window-handle.workspace = true
serde_json.workspace = true
windows = { workspace = true, features = ["Win32_Foundation", "Win32_Graphics_Dwm", "Win32_System_Registry"] }

[build-dependencies]
slint-build.workspace = true

[dev-dependencies]
i-slint-backend-testing.workspace = true
```

`crates/app/build.rs` (UI comes in Task 9; compile an empty-but-valid file now):
```rust
fn main() {
    slint_build::compile("ui/main.slint").expect("compile Slint UI");
}
```
`crates/app/ui/main.slint` (placeholder replaced in Task 9 — it must compile):
```slint
export component MainWindow inherits Window {
    title: "Negotium";
}
```
`crates/app/src/lib.rs`:
```rust
//! Negotium GUI: pure view building (format, view), the supervised worker, and Slint glue.
pub mod format;
pub mod view;

slint::include_modules!();
```
`crates/app/src/main.rs` (replaced in Task 10):
```rust
#![windows_subsystem = "windows"]

fn main() -> Result<(), slint::PlatformError> {
    use slint::ComponentHandle;
    let ui = negotium::MainWindow::new()?;
    ui.run()
}
```

- [ ] **Step 2: Write the failing tests**

`format.rs` tests:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_use_binary_units_with_one_decimal() {
        assert_eq!(bytes(0.0), "0 B");
        assert_eq!(bytes(512.0), "512 B");
        assert_eq!(bytes(1536.0), "1.5 KB");
        assert_eq!(bytes(3.9 * 1024.0 * 1024.0 * 1024.0), "3.9 GB");
        assert_eq!(byte_rate(2.0 * 1024.0 * 1024.0), "2.0 MB/s");
    }

    #[test]
    fn network_is_bits_per_second_in_decimal_units() {
        assert_eq!(bit_rate(0.0), "0 b/s");
        assert_eq!(bit_rate(500_000.0), "4.0 Mb/s");
        assert_eq!(bit_rate(125.0), "1.0 Kb/s");
    }

    #[test]
    fn percentages_and_hung_timer() {
        assert_eq!(pct(37.6), "38%");
        assert_eq!(pct1(18.24), "18.2%");
        assert_eq!(pct1(f32::NAN), "0.0%");
        assert_eq!(hung(14_400), "Not responding · 14s");
        assert_eq!(hung(65_000), "Not responding · 1m 05s");
    }
}
```

`view.rs` tests:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use negotium_model::{AppKind, ProcRow};
    use negotium_snapshot::{MemoryTotals, SystemTotals};

    fn proc(pid: u32, parent: u32, cpu: f32, name: &str) -> ProcRow {
        ProcRow { pid, create_time: pid as i64, parent_pid: parent, name: name.into(), cpu_pct: cpu, private_bytes: 1024, disk_bps: 0.0, gpu_pct: None }
    }
    fn app(pid: u32, name: &str, cpu: f32, procs: Vec<ProcRow>, hung: Option<u64>) -> AppRow {
        AppRow {
            key: AppKey::Window { pid, create_time: pid as i64 }, name: name.into(), kind: AppKind::App,
            cpu_pct: cpu, private_bytes: procs.len() as u64 * 1024, disk_bps: 0.0, gpu_pct: None, net_bps: None,
            hung_for_ms: hung, processes: procs,
        }
    }
    fn snap(cpu: Option<f32>) -> Snapshot {
        Snapshot {
            taken_at_ms: 60_000,
            totals: SystemTotals { logical_cpus: 4, cpu_pct: cpu, memory: Some(MemoryTotals { total: 16 << 30, used: 8 << 30 }), ..Default::default() },
            ..Default::default()
        }
    }

    #[test]
    fn spark_path_right_aligns_samples_and_breaks_on_gaps() {
        assert_eq!(spark_path(&[Some(0.0), Some(50.0), Some(100.0)], 100.0), "M 58 100 L 59 50 L 60 0");
        assert_eq!(spark_path(&[Some(10.0), None, Some(10.0)], 100.0), "M 58 90 M 60 90");
        assert_eq!(spark_path(&[], 100.0), "M 0 100");
        assert_eq!(spark_path(&[Some(5.0)], 0.0), "M 60 100");
    }

    #[test]
    fn frame_has_five_meters_tiles_and_rows_in_order() {
        let rows = vec![
            app(1, "Chrome", 30.0, vec![proc(1, 0, 20.0, "chrome.exe"), proc(2, 1, 10.0, "chrome.exe")], None),
            app(3, "Editor", 5.0, vec![proc(3, 0, 5.0, "code.exe")], Some(14_400)),
        ];
        let mut ids = IdMap::new();
        let f = build_frame(&mut ids, &snap(Some(37.6)), &rows, &[1, 0], &History::new(10), Metric::Cpu, true);
        assert_eq!(f.meters.len(), 5);
        assert_eq!(f.meters[0].value, "38%");
        assert_eq!(f.meters[1].sub, "8.0 / 16.0 GB");
        assert_eq!(f.tiles.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["Chrome", "Editor"]);
        assert_eq!(f.caption, "Top CPU users");
        let names: Vec<_> = f.rows.iter().filter(|r| r.depth == 0).map(|r| r.name.clone()).collect();
        assert_eq!(names, ["Editor", "Chrome"], "rows follow the ranker order");
        let editor = f.rows.iter().find(|r| r.name == "Editor").unwrap();
        assert_eq!(editor.hung, "Not responding · 14s");
        assert_eq!(f.targets[&editor.id].root, Some((3, 3)));
        let chrome = f.rows.iter().find(|r| r.name == "Chrome").unwrap();
        assert_eq!(chrome.count, "(2)");
        assert!(chrome.expandable);
        assert!((chrome.bar - 1.0).abs() < 1e-6);
        assert!(!f.admin_banner);
    }

    #[test]
    fn degenerate_inputs_do_not_divide_by_zero() {
        let rows = vec![app(1, "Idle", 0.0, vec![proc(1, 0, 0.0, "idle.exe")], None)];
        let mut ids = IdMap::new();
        let f = build_frame(&mut ids, &snap(None), &rows, &[0], &History::new(10), Metric::Gpu, false);
        assert!(f.tiles.is_empty());
        assert!(f.rows.iter().all(|r| r.bar == 0.0));
        assert!(!f.meters[0].available, "CPU meter shows Unavailable when cpu_pct is None");
        assert!(!f.meters[3].available, "GPU meter shows Unavailable without GPU counters");
    }

    #[test]
    fn ids_are_stable_across_frames() {
        let rows = vec![app(1, "Chrome", 30.0, vec![proc(1, 0, 30.0, "chrome.exe")], None)];
        let mut ids = IdMap::new();
        let a = build_frame(&mut ids, &snap(Some(1.0)), &rows, &[0], &History::new(10), Metric::Cpu, true);
        let b = build_frame(&mut ids, &snap(Some(1.0)), &rows, &[0], &History::new(10), Metric::Cpu, true);
        assert_eq!(a.rows[0].id, b.rows[0].id);
    }

    #[test]
    fn visible_rows_expand_and_search() {
        let rows = vec![
            RowView { id: 1, parent: -1, depth: 0, name: "Chrome".into(), search_text: "chrome chrome.exe gpu-helper.exe".into(), expandable: true, ..Default::default() },
            RowView { id: 2, parent: 1, depth: 1, name: "gpu-helper.exe".into(), search_text: "gpu-helper.exe".into(), ..Default::default() },
            RowView { id: 3, parent: -1, depth: 0, name: "Editor".into(), search_text: "editor code.exe".into(), ..Default::default() },
        ];
        let none = HashSet::new();
        assert_eq!(visible_rows(&rows, &none, "").iter().map(|r| r.id).collect::<Vec<_>>(), [1, 3]);
        assert_eq!(visible_rows(&rows, &HashSet::from([1]), "").iter().map(|r| r.id).collect::<Vec<_>>(), [1, 2, 3]);
        assert_eq!(visible_rows(&rows, &none, "HELPER").iter().map(|r| r.id).collect::<Vec<_>>(), [1, 2]);
        assert!(visible_rows(&rows, &none, "zzz").is_empty());
    }
}
```
(`RowView` derives `Default` for these literals.)

- [ ] **Step 3: Run to verify they fail**

Run: `cargo test -p negotium --lib`
Expected: FAIL to compile (functions/types missing).

- [ ] **Step 4: Implement `format.rs`**

```rust
//! Text formatting shared by meters, tiles and rows.

pub fn bytes(b: f64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if !b.is_finite() || b < 1024.0 {
        return format!("{} B", b.max(0.0).round() as u64);
    }
    let mut v = b;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    format!("{v:.1} {}", UNITS[u])
}

pub fn byte_rate(bps: f64) -> String {
    format!("{}/s", bytes(bps))
}

/// Network throughput in bits per second, decimal units (as Task Manager shows).
pub fn bit_rate(bytes_per_s: f64) -> String {
    let bits = (bytes_per_s.max(0.0)) * 8.0;
    if !bits.is_finite() || bits < 1000.0 {
        return format!("{} b/s", bits.round() as u64);
    }
    const UNITS: [&str; 4] = ["Kb/s", "Mb/s", "Gb/s", "Tb/s"];
    let mut v = bits / 1000.0;
    let mut u = 0;
    while v >= 1000.0 && u < UNITS.len() - 1 {
        v /= 1000.0;
        u += 1;
    }
    format!("{v:.1} {}", UNITS[u])
}

pub fn pct(p: f32) -> String {
    format!("{}%", if p.is_finite() { p.round().clamp(0.0, 100.0) } else { 0.0 })
}

pub fn pct1(p: f32) -> String {
    format!("{:.1}%", if p.is_finite() { p.clamp(0.0, 100.0) } else { 0.0 })
}

pub fn hung(ms: u64) -> String {
    let s = ms / 1000;
    if s < 60 { format!("Not responding · {s}s") } else { format!("Not responding · {}m {:02}s", s / 60, s % 60) }
}
```

- [ ] **Step 5: Implement `view.rs`**

```rust
//! Builds the plain-data Frame the UI renders. Runs on the worker thread; `visible_rows`
//! runs on the UI thread (no Windows calls anywhere here).
use crate::format;
use negotium_history::History;
use negotium_model::{AppKey, AppRow, Metric, top_n};
use negotium_snapshot::Snapshot;
use std::collections::{HashMap, HashSet};

pub const SPARK_SECONDS: u64 = 60;
pub const TOP_TILES: usize = 5;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MeterView { pub label: &'static str, pub value: String, pub sub: String, pub spark: String, pub available: bool }

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TileView { pub id: i32, pub name: String, pub value: String, pub weight: f32, pub hung: bool }

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RowView {
    pub id: i32, pub parent: i32, pub depth: i32, pub name: String, pub search_text: String, pub count: String,
    pub cpu: String, pub mem: String, pub disk: String, pub gpu: String, pub net: String,
    pub bar: f32, pub hung: String, pub expandable: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Target {
    pub name: String,
    pub root: Option<(u32, i64)>,
    pub procs: Vec<(u32, i64)>,
    pub hung_pid: Option<(u32, i64)>,
    pub image_path: Option<String>,
    pub critical: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Frame {
    pub meters: Vec<MeterView>,
    pub tiles: Vec<TileView>,
    pub caption: String,
    pub rows: Vec<RowView>,
    pub targets: HashMap<i32, Target>,
    pub all_procs: Vec<(u32, i64, u32)>,
    pub admin_banner: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum RowKey {
    App(AppKey),
    Proc(u32, i64),
}

/// Stable small ids for rows across frames; ids of rows not seen in a frame are released.
#[derive(Default)]
pub struct IdMap {
    ids: HashMap<RowKey, i32>,
    next: i32,
}

impl IdMap {
    pub fn new() -> Self {
        Self::default()
    }
    fn id(&mut self, key: RowKey, seen: &mut HashSet<RowKey>) -> i32 {
        seen.insert(key.clone());
        if let Some(&id) = self.ids.get(&key) {
            return id;
        }
        self.next += 1;
        self.ids.insert(key, self.next);
        self.next
    }
    fn retain(&mut self, seen: &HashSet<RowKey>) {
        self.ids.retain(|k, _| seen.contains(k));
    }
}

pub fn spark_path(values: &[Option<f32>], max: f32) -> String {
    if values.is_empty() {
        return "M 0 100".to_string();
    }
    let n = values.len() as i64;
    let mut out = String::new();
    let mut pen_down = false;
    for (i, v) in values.iter().enumerate() {
        let x = SPARK_SECONDS as i64 - (n - 1 - i as i64);
        match v {
            Some(v) if max > 0.0 && v.is_finite() => {
                let y = (100.0 - (v / max * 100.0).clamp(0.0, 100.0)).round() as i64;
                out.push_str(&format!("{}{} {x} {y}", if out.is_empty() { "" } else { " " }, if pen_down { "L" } else { "M" }));
                pen_down = true;
            }
            Some(_) => {
                out.push_str(&format!("{}M {x} 100", if out.is_empty() { "" } else { " " }));
                pen_down = false;
            }
            None => pen_down = false,
        }
    }
    if out.is_empty() { "M 0 100".to_string() } else { out }
}

fn metric_text(row_value: f64, metric: Metric) -> String {
    match metric {
        Metric::Cpu | Metric::Gpu => format::pct1(row_value as f32),
        Metric::Memory => format::bytes(row_value),
        Metric::Disk => format::byte_rate(row_value),
        Metric::Network => format::bit_rate(row_value),
    }
}

fn caption(metric: Metric) -> &'static str {
    match metric {
        Metric::Cpu => "Top CPU users",
        Metric::Memory => "Top memory users",
        Metric::Disk => "Top disk users",
        Metric::Gpu => "Top GPU users",
        Metric::Network => "Top network users",
    }
}

fn meters(snap: &Snapshot, history: &History) -> Vec<MeterView> {
    let t = &snap.totals;
    let since = snap.taken_at_ms.saturating_sub(SPARK_SECONDS * 1000);
    let samples: Vec<_> = history.since(since).collect();
    let mem_total = t.memory.as_ref().map(|m| m.total as f32).unwrap_or(0.0);
    let net_series: Vec<Option<f32>> = samples.iter().map(|s| s.net_bps.map(|v| v as f32)).collect();
    let net_max = net_series.iter().flatten().fold(0.0f32, |a, b| a.max(*b)).max(t.net_bps.unwrap_or(0.0) as f32);
    vec![
        MeterView {
            label: "CPU",
            value: t.cpu_pct.map(format::pct).unwrap_or_default(),
            sub: String::new(),
            spark: spark_path(&samples.iter().map(|s| s.cpu_pct).collect::<Vec<_>>(), 100.0),
            available: t.cpu_pct.is_some(),
        },
        MeterView {
            label: "Memory",
            value: t.memory.as_ref().map(|m| format::pct(m.used as f32 / m.total.max(1) as f32 * 100.0)).unwrap_or_default(),
            sub: t.memory.as_ref().map(|m| {
                let gb = |b: u64| b as f64 / (1u64 << 30) as f64;
                format!("{:.1} / {:.1} GB", gb(m.used), gb(m.total))
            }).unwrap_or_default(),
            spark: spark_path(&samples.iter().map(|s| s.mem_used.map(|u| u as f32)).collect::<Vec<_>>(), mem_total),
            available: t.memory.is_some(),
        },
        MeterView {
            label: "Disk",
            value: t.busiest_disk_pct().map(format::pct).unwrap_or_default(),
            sub: String::new(),
            spark: spark_path(&samples.iter().map(|s| s.disk_active_pct).collect::<Vec<_>>(), 100.0),
            available: t.disks.is_some(),
        },
        MeterView {
            label: "GPU",
            value: t.gpu_pct.map(format::pct).unwrap_or_default(),
            sub: String::new(),
            spark: spark_path(&samples.iter().map(|s| s.gpu_pct).collect::<Vec<_>>(), 100.0),
            available: t.gpu_pct.is_some(),
        },
        MeterView {
            label: "Network",
            value: t.net_bps.map(format::bit_rate).unwrap_or_default(),
            sub: String::new(),
            spark: spark_path(&net_series, net_max),
            available: t.net_bps.is_some(),
        },
    ]
}

pub fn build_frame(
    ids: &mut IdMap, snap: &Snapshot, rows: &[AppRow], order: &[usize], history: &History, metric: Metric, elevated: bool,
) -> Frame {
    let mut seen = HashSet::new();
    let hung_pids: HashSet<u32> = snap.windows.iter().filter(|w| w.hung).map(|w| w.pid).collect();
    let image_of: HashMap<(u32, i64), &str> = snap
        .processes
        .iter()
        .filter_map(|p| p.image_path.as_deref().map(|i| ((p.pid, p.create_time), i)))
        .collect();
    let max = rows.iter().map(|r| r.value(metric)).fold(0.0f64, f64::max);
    let frac = |v: f64| if max > 0.0 && v.is_finite() { (v / max).clamp(0.0, 1.0) as f32 } else { 0.0 };

    let mut out_rows = Vec::new();
    let mut targets = HashMap::new();
    for &i in order {
        let r = &rows[i];
        let id = ids.id(RowKey::App(r.key.clone()), &mut seen);
        let root = match r.key {
            AppKey::Window { pid, create_time } | AppKey::Background { pid, create_time } => Some((pid, create_time)),
            _ => None,
        };
        let procs: Vec<(u32, i64)> = r.processes.iter().map(|p| (p.pid, p.create_time)).collect();
        let hung_pid = r.processes.iter().find(|p| hung_pids.contains(&p.pid)).map(|p| (p.pid, p.create_time));
        targets.insert(id, Target {
            name: r.name.clone(),
            root,
            procs: procs.clone(),
            hung_pid,
            image_path: root.or(procs.first().copied()).and_then(|k| image_of.get(&k).map(|s| s.to_string())),
            critical: r.processes.iter().any(|p| negotium_actions::is_critical(&p.name)),
        });
        let search_text = std::iter::once(r.name.to_lowercase())
            .chain(r.processes.iter().map(|p| p.name.to_lowercase()))
            .collect::<Vec<_>>()
            .join(" ");
        out_rows.push(RowView {
            id, parent: -1, depth: 0, name: r.name.clone(), search_text,
            count: if r.processes.len() > 1 { format!("({})", r.processes.len()) } else { String::new() },
            cpu: format::pct1(r.cpu_pct), mem: format::bytes(r.private_bytes as f64), disk: format::byte_rate(r.disk_bps),
            gpu: r.gpu_pct.map(format::pct1).unwrap_or_else(|| "—".into()),
            net: r.net_bps.map(format::bit_rate).unwrap_or_else(|| "—".into()),
            bar: frac(r.value(metric)), hung: r.hung_for_ms.map(format::hung).unwrap_or_default(),
            expandable: r.processes.len() > 1,
        });
        if r.processes.len() > 1 {
            for p in &r.processes {
                let pid_key = (p.pid, p.create_time);
                let cid = ids.id(RowKey::Proc(p.pid, p.create_time), &mut seen);
                targets.insert(cid, Target {
                    name: p.name.clone(), root: Some(pid_key), procs: vec![pid_key],
                    hung_pid: hung_pids.contains(&p.pid).then_some(pid_key),
                    image_path: image_of.get(&pid_key).map(|s| s.to_string()),
                    critical: negotium_actions::is_critical(&p.name),
                });
                out_rows.push(RowView {
                    id: cid, parent: id, depth: 1, name: p.name.clone(), search_text: p.name.to_lowercase(), count: String::new(),
                    cpu: format::pct1(p.cpu_pct), mem: format::bytes(p.private_bytes as f64), disk: format::byte_rate(p.disk_bps),
                    gpu: p.gpu_pct.map(format::pct1).unwrap_or_else(|| "—".into()), net: "—".into(),
                    bar: 0.0, hung: String::new(), expandable: false,
                });
            }
        }
    }
    ids.retain(&seen);

    let tiles = top_n(rows, metric, TOP_TILES)
        .into_iter()
        .map(|i| {
            let r = &rows[i];
            let id = ids.ids.get(&RowKey::App(r.key.clone())).copied().unwrap_or(0);
            TileView { id, name: r.name.clone(), value: metric_text(r.value(metric), metric), weight: r.value(metric) as f32, hung: r.hung_for_ms.is_some() }
        })
        .collect();

    let all_procs = snap.processes.iter().map(|p| (p.pid, p.create_time, p.parent_pid)).collect();
    let admin_banner = !elevated && snap.processes.iter().any(|p| p.pid > 4 && p.image_path.is_none());
    Frame { meters: meters(snap, history), tiles, caption: caption(metric).to_string(), rows: out_rows, targets, all_procs, admin_banner }
}

pub fn visible_rows(rows: &[RowView], expanded: &HashSet<i32>, search: &str) -> Vec<RowView> {
    let needle = search.trim().to_lowercase();
    let matches_app = |r: &RowView| needle.is_empty() || r.search_text.contains(&needle);
    let app_ok: HashSet<i32> = rows.iter().filter(|r| r.depth == 0 && matches_app(r)).map(|r| r.id).collect();
    rows.iter()
        .filter(|r| {
            if r.depth == 0 {
                app_ok.contains(&r.id)
            } else {
                app_ok.contains(&r.parent)
                    && (expanded.contains(&r.parent) || (!needle.is_empty() && r.search_text.contains(&needle)))
            }
        })
        .cloned()
        .collect()
}
```

In `lib.rs`, the test literal `RowView { .., ..Default::default() }` requires `Default` (derived above).

- [ ] **Step 6: Run tests**

Run: `cargo test -p negotium --lib`
Expected: all format and view tests pass, warning-free.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml Cargo.lock crates/app
git commit -m "feat(app): pure formatting and frame building for the overview"
```

---

### Task 8: Controls and the supervised worker

**Files:**
- Create: `crates/app/src/controls.rs`, `crates/app/src/worker.rs`, `crates/app/src/log.rs`
- Modify: `crates/app/src/lib.rs`
- Test: unit tests in `controls.rs`, `worker.rs`, `log.rs`

**Interfaces:**
- Consumes: `view::{IdMap, build_frame, Frame}`, `negotium_collector::Collector`, `negotium_model::{Model, Ranker, Metric}`, `negotium_history::{History, DEFAULT_CAPACITY}`, `negotium_actions::is_elevated`.
- Produces:
  - `controls::Controls` (`Default`, shared via `Arc`) with `set_metric(Metric)`, `metric() -> Metric`, `set_frozen(bool)`, `frozen() -> bool`, `set_visible(bool)`, `visible() -> bool`, `interval() -> Duration` (1 s visible, 2 s hidden), `request_stop()`, `stopped() -> bool`, `poke()`, `wait(Duration) -> bool` (true if woken early).
  - `worker::Source` trait: `fn next_frame(&mut self, controls: &Controls) -> Option<Frame>` (`None` = nothing to show, e.g. hidden).
  - `worker::backoff(restarts: u32) -> Duration` = `min(500 ms × 2^(restarts−1), 30 s)`.
  - `worker::supervise<S: Source, M: Fn() -> S>(make: M, controls: Arc<Controls>, sink: impl FnMut(Frame), log: impl FnMut(&str)) -> u32` — runs until `controls.stopped()`, returns restart count. Catches panics from `make` and `next_frame`, logs `"worker restarted after panic: <message>"`, rebuilds the source after `backoff`.
  - `worker::LiveSource` implementing `Source` (built inside the worker thread; `Collector` is `!Send`); it logs each snapshot's `source_errors` through `log::Log` (deduplicated).
  - `worker::spawn(controls: Arc<Controls>, sink: impl FnMut(Frame) + Send + 'static) -> std::thread::JoinHandle<u32>` — logs to `log::Log::default_path()`.
  - `log::Log` (spec §7: rolling file in `%LOCALAPPDATA%\Negotium\logs`, warnings and errors only): `Log::new(path: PathBuf) -> Log`, `Log::default_path() -> PathBuf`, `warn(&mut self, msg: &str)` — appends `"<unix-seconds> WARN <msg>\n"`; each distinct message is written once per run; when the file exceeds `MAX_LOG_BYTES` (1 MiB) it is renamed to `negotium.log.1` (replacing any previous) and a new file is started. I/O errors are ignored (logging must never take the app down).

- [ ] **Step 1: Write the failing tests**

`controls.rs` tests:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    #[test]
    fn interval_depends_on_visibility() {
        let c = Controls::default();
        c.set_visible(true);
        assert_eq!(c.interval(), Duration::from_secs(1));
        c.set_visible(false);
        assert_eq!(c.interval(), Duration::from_secs(2));
    }

    #[test]
    fn poke_wakes_a_waiter_early() {
        let c = Arc::new(Controls::default());
        let c2 = c.clone();
        let t = std::thread::spawn(move || {
            let start = Instant::now();
            let woke = c2.wait(Duration::from_secs(5));
            (woke, start.elapsed())
        });
        std::thread::sleep(Duration::from_millis(100));
        c.set_metric(Metric::Memory);
        let (woke, elapsed) = t.join().unwrap();
        assert!(woke && elapsed < Duration::from_secs(2));
        assert_eq!(c.metric(), Metric::Memory);
    }
}
```

`worker.rs` tests:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct Panicky(Arc<AtomicU32>);
    impl Source for Panicky {
        fn next_frame(&mut self, _: &Controls) -> Option<Frame> {
            if self.0.fetch_add(1, Ordering::SeqCst) % 2 == 0 {
                panic!("collector blew up");
            }
            Some(Frame::default())
        }
    }

    #[test]
    fn backoff_doubles_and_caps() {
        assert_eq!(backoff(1), Duration::from_millis(500));
        assert_eq!(backoff(2), Duration::from_millis(1000));
        assert_eq!(backoff(20), Duration::from_secs(30));
    }

    #[test]
    fn panics_are_survived_with_backoff_and_frames_keep_flowing() {
        let controls = Arc::new(Controls::default());
        controls.set_visible(true);
        let calls = Arc::new(AtomicU32::new(0));
        let frames = Arc::new(AtomicU32::new(0));
        let (c2, calls2, frames2) = (controls.clone(), calls.clone(), frames.clone());
        let logged = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let logged2 = logged.clone();
        let started = std::time::Instant::now();
        let handle = std::thread::spawn(move || {
            supervise(
                move || Panicky(calls2.clone()),
                c2,
                move |_| { frames2.fetch_add(1, Ordering::SeqCst); },
                move |m: &str| logged2.lock().unwrap().push(m.to_string()),
            )
        });
        std::thread::sleep(Duration::from_millis(1800));
        controls.request_stop();
        let restarts = handle.join().unwrap();
        assert!(restarts >= 1, "a panic must trigger a restart");
        assert!(restarts <= 3, "backoff must prevent a tight restart loop: {restarts}");
        assert!(frames.load(Ordering::SeqCst) >= 1, "frames continue after a restart");
        assert!(started.elapsed() < Duration::from_secs(5), "stop is honoured promptly");
        assert!(logged.lock().unwrap().iter().any(|m| m.contains("collector blew up")), "panics are logged");
    }
}
```

`log.rs` tests:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("negotium-log-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn writes_each_distinct_message_once() {
        let dir = temp_dir("dedup");
        let mut log = Log::new(dir.join("negotium.log"));
        log.warn("pdh gpu: counter unavailable");
        log.warn("pdh gpu: counter unavailable");
        log.warn("worker restarted after panic: boom");
        let text = std::fs::read_to_string(dir.join("negotium.log")).unwrap();
        assert_eq!(text.lines().count(), 2);
        assert!(text.lines().all(|l| l.contains(" WARN ")));
    }

    #[test]
    fn rotates_when_too_large() {
        let dir = temp_dir("rotate");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("negotium.log"), vec![b'x'; MAX_LOG_BYTES as usize + 1]).unwrap();
        let mut log = Log::new(dir.join("negotium.log"));
        log.warn("after rotation");
        assert!(dir.join("negotium.log.1").exists());
        assert!(std::fs::read_to_string(dir.join("negotium.log")).unwrap().contains("after rotation"));
    }
}
```
(The panic message prints to stderr during this test; that is expected. Install a quiet panic hook inside the test only if the noise breaks "pristine output" — `std::panic::set_hook(Box::new(|_| {}))` at the start of this test.)

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p negotium --lib`
Expected: FAIL to compile.

- [ ] **Step 3: Implement `controls.rs`**

```rust
//! State shared between the UI thread (writes) and the worker (reads).
use negotium_model::Metric;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

pub struct Controls {
    metric: AtomicU8,
    frozen: AtomicBool,
    visible: AtomicBool,
    stop: AtomicBool,
    woken: Mutex<bool>,
    cv: Condvar,
}

impl Default for Controls {
    fn default() -> Self {
        Self { metric: AtomicU8::new(0), frozen: AtomicBool::new(false), visible: AtomicBool::new(true), stop: AtomicBool::new(false), woken: Mutex::new(false), cv: Condvar::new() }
    }
}

impl Controls {
    pub fn set_metric(&self, m: Metric) {
        self.metric.store(m as u8, Ordering::Relaxed);
        self.poke();
    }
    pub fn metric(&self) -> Metric {
        Metric::ALL[(self.metric.load(Ordering::Relaxed) as usize).min(Metric::ALL.len() - 1)]
    }
    pub fn set_frozen(&self, f: bool) {
        self.frozen.store(f, Ordering::Relaxed);
    }
    pub fn frozen(&self) -> bool {
        self.frozen.load(Ordering::Relaxed)
    }
    pub fn set_visible(&self, v: bool) {
        self.visible.store(v, Ordering::Relaxed);
        self.poke();
    }
    pub fn visible(&self) -> bool {
        self.visible.load(Ordering::Relaxed)
    }
    pub fn interval(&self) -> Duration {
        if self.visible() { Duration::from_secs(1) } else { Duration::from_secs(2) }
    }
    pub fn request_stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        self.poke();
    }
    pub fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }
    pub fn poke(&self) {
        *self.woken.lock().unwrap() = true;
        self.cv.notify_all();
    }
    /// Sleeps up to `d`; returns true if poked earlier.
    pub fn wait(&self, d: Duration) -> bool {
        let guard = self.woken.lock().unwrap();
        let (mut guard, result) = self.cv.wait_timeout_while(guard, d, |woken| !*woken).unwrap();
        let woke = !result.timed_out();
        *guard = false;
        woke
    }
}
```

- [ ] **Step 4: Implement `worker.rs`**

```rust
//! The supervised worker: one thread owns Collector → Model → Ranker → History and emits
//! Frames. A panic anywhere in a tick rebuilds the pipeline after a backoff (spec §7).
use crate::controls::Controls;
use crate::view::{Frame, IdMap, build_frame};
use negotium_collector::Collector;
use negotium_history::{DEFAULT_CAPACITY, History};
use negotium_model::{Model, Ranker};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::time::Duration;

pub trait Source {
    fn next_frame(&mut self, controls: &Controls) -> Option<Frame>;
}

pub fn backoff(restarts: u32) -> Duration {
    let ms = 500u64.saturating_mul(1u64 << restarts.saturating_sub(1).min(16));
    Duration::from_millis(ms.min(30_000))
}

fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_string())
}

pub fn supervise<S: Source, M: Fn() -> S>(
    make: M, controls: Arc<Controls>, mut sink: impl FnMut(Frame), mut log: impl FnMut(&str),
) -> u32 {
    let mut restarts = 0u32;
    while !controls.stopped() {
        let built = catch_unwind(AssertUnwindSafe(&make));
        let mut source = match built {
            Ok(s) => s,
            Err(p) => {
                restarts += 1;
                log(&format!("worker restarted after panic: {}", panic_text(&*p)));
                controls.wait(backoff(restarts));
                continue;
            }
        };
        loop {
            if controls.stopped() {
                return restarts;
            }
            match catch_unwind(AssertUnwindSafe(|| source.next_frame(&controls))) {
                Ok(Some(frame)) => sink(frame),
                Ok(None) => {}
                Err(p) => {
                    restarts += 1;
                    log(&format!("worker restarted after panic: {}", panic_text(&*p)));
                    controls.wait(backoff(restarts));
                    break;
                }
            }
            controls.wait(controls.interval());
        }
    }
    restarts
}

pub struct LiveSource {
    collector: Collector,
    model: Model,
    ranker: Ranker,
    history: History,
    ids: IdMap,
    elevated: bool,
    log: crate::log::Log,
}

impl LiveSource {
    pub fn new() -> Self {
        Self {
            collector: Collector::new(),
            model: Model::new(),
            ranker: Ranker::new(),
            history: History::new(DEFAULT_CAPACITY),
            ids: IdMap::new(),
            elevated: negotium_actions::is_elevated(),
            log: crate::log::Log::new(crate::log::Log::default_path()),
        }
    }
}

impl Default for LiveSource {
    fn default() -> Self {
        Self::new()
    }
}

impl Source for LiveSource {
    fn next_frame(&mut self, controls: &Controls) -> Option<Frame> {
        let snap = self.collector.tick();
        for e in &snap.source_errors {
            self.log.warn(e);
        }
        let rows = self.model.update(&snap);
        self.history.push(snap.taken_at_ms, &snap.totals, &rows);
        if !controls.visible() {
            return None;
        }
        let metric = controls.metric();
        let order = self.ranker.order(&rows, metric, snap.taken_at_ms, controls.frozen());
        Some(build_frame(&mut self.ids, &snap, &rows, &order, &self.history, metric, self.elevated))
    }
}

pub fn spawn(controls: Arc<Controls>, sink: impl FnMut(Frame) + Send + 'static) -> std::thread::JoinHandle<u32> {
    std::thread::Builder::new()
        .name("negotium-worker".into())
        .spawn(move || {
            let mut log = crate::log::Log::new(crate::log::Log::default_path());
            supervise(LiveSource::new, controls, sink, move |m: &str| log.warn(m))
        })
        .expect("spawn worker thread")
}
```

`log.rs` (above its tests):
```rust
//! Warnings-and-errors log (spec §7): %LOCALAPPDATA%\Negotium\logs\negotium.log,
//! rotated at MAX_LOG_BYTES, each distinct message written once per run.
use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;

pub const MAX_LOG_BYTES: u64 = 1024 * 1024;

pub struct Log {
    path: PathBuf,
    seen: HashSet<String>,
}

impl Log {
    pub fn new(path: PathBuf) -> Self {
        Self { path, seen: HashSet::new() }
    }

    pub fn default_path() -> PathBuf {
        let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        base.join("Negotium").join("logs").join("negotium.log")
    }

    pub fn warn(&mut self, msg: &str) {
        if !self.seen.insert(msg.to_string()) {
            return;
        }
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if std::fs::metadata(&self.path).map(|m| m.len() > MAX_LOG_BYTES).unwrap_or(false) {
            let _ = std::fs::rename(&self.path, self.path.with_extension("log.1"));
        }
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&self.path) {
            let _ = writeln!(f, "{secs} WARN {msg}");
        }
    }
}
```
`lib.rs`: add `pub mod controls; pub mod log; pub mod worker;`.

- [ ] **Step 5: Run tests**

Run: `cargo test -p negotium --lib`
Expected: all pass, output pristine (quiet hook if needed).

- [ ] **Step 6: Commit**

```bash
git add crates/app
git commit -m "feat(app): supervised worker with panic restart and shared controls"
```

---

### Task 9: Slint UI — theme, meters, top strip, app list, overlays, tray

**Files:**
- Create: `crates/app/assets/negotium.svg`, `crates/app/ui/{theme,meters,topstrip,applist,overlays,tray}.slint`
- Modify: `crates/app/ui/main.slint`
- Create: `crates/app/tests/ui.rs`

**Interfaces:**
- Produces (generated Rust in crate `negotium`): `MainWindow` with properties `meters: [MeterData]`, `selected-metric: int`, `tiles: [TileData]`, `top-caption: string`, `rows: [RowData]`, `show-admin-banner: bool`, `toast: string`, `confirm-text: string`, `confirm-visible: bool`, `details-text: string`, `details-visible: bool`; callbacks `select-metric(int)`, `open-tile(int)`, `toggle-row(int)`, `row-action(string, int)`, `list-hover(bool)`, `search-changed(string)`, `restart-admin()`, `dismiss-banner()`, `confirm(bool)`, `close-details()`, `dismiss-toast()`; structs `MeterData { label, value, sub, spark: string, available: bool }`, `TileData { id: int, name, value: string, weight: float, hung: bool }`, `RowData { id: int, depth: int, name, count, cpu, mem, disk, gpu, net: string, bar: float, hung: string, expandable: bool, expanded: bool }`; global `Theme` (`accent: color`, `mica: bool`); component `NegotiumTray` with callbacks `open()`, `exit()`.
- Row action names sent through `row-action`: `"end-task"`, `"end-tree"`, `"force-close"`, `"efficiency"`, `"open-location"`, `"details"`.

- [ ] **Step 1: Write the failing UI tests**

`crates/app/tests/ui.rs`:
```rust
use i_slint_backend_testing::ElementHandle;
use negotium::{MainWindow, MeterData, RowData};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::cell::RefCell;
use std::rc::Rc;

fn window() -> MainWindow {
    i_slint_backend_testing::init_no_event_loop();
    let ui = MainWindow::new().unwrap();
    let meters: Vec<MeterData> = ["CPU", "Memory", "Disk", "GPU", "Network"]
        .iter()
        .map(|l| MeterData { label: (*l).into(), value: "1%".into(), sub: "".into(), spark: "M 0 100".into(), available: true })
        .collect();
    ui.set_meters(ModelRc::new(VecModel::from(meters)));
    ui.set_rows(ModelRc::new(VecModel::from(vec![RowData {
        id: 7, depth: 0, name: "Editor".into(), count: "".into(), cpu: "1.0%".into(), mem: "1.0 MB".into(),
        disk: "0 B/s".into(), gpu: "—".into(), net: "—".into(), bar: 0.5, hung: "Not responding · 9s".into(),
        expandable: false, expanded: false,
    }])));
    ui
}

#[test]
fn clicking_a_meter_selects_its_metric() {
    let ui = window();
    let picked = Rc::new(RefCell::new(None));
    let p = picked.clone();
    ui.on_select_metric(move |i| *p.borrow_mut() = Some(i));
    let meter = ElementHandle::find_by_accessible_label(&ui, "Memory meter").next().expect("memory meter");
    meter.invoke_accessible_default_action();
    assert_eq!(*picked.borrow(), Some(1));
}

#[test]
fn hung_rows_offer_force_close() {
    let ui = window();
    let action = Rc::new(RefCell::new(None));
    let a = action.clone();
    ui.on_row_action(move |name, id| *a.borrow_mut() = Some((name.to_string(), id)));
    let button = ElementHandle::find_by_accessible_label(&ui, "Force close Editor").next().expect("force close button");
    button.invoke_accessible_default_action();
    assert_eq!(*action.borrow(), Some(("force-close".to_string(), 7)));
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p negotium --test ui`
Expected: FAIL to compile (no `MeterData`, `set_meters`, …).

- [ ] **Step 3: Write the UI files**

`crates/app/assets/negotium.svg`:
```svg
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32"><rect x="2" y="2" width="28" height="28" rx="7" fill="#0078d4"/><path d="M8 22 L13 14 L17 18 L24 9" fill="none" stroke="#fff" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"/></svg>
```

`ui/theme.slint`:
```slint
import { Palette } from "std-widgets.slint";

export global Theme {
    in-out property <color> accent: #0078d4;
    in-out property <bool> mica: false;
    out property <bool> dark: Palette.color-scheme == ColorScheme.dark;
    out property <brush> window-bg: mica ? transparent : (dark ? #202020 : #f3f3f3);
    out property <color> card: dark ? #ffffff0f : #ffffffb3;
    out property <color> card-border: dark ? #ffffff14 : #0000000f;
    out property <color> text: dark ? #ffffff : #1a1a1a;
    out property <color> muted: dark ? #ffffffa0 : #00000099;
    out property <color> danger-text: dark ? #ff99a4 : #c42b1c;
    out property <color> danger-bg: dark ? #442726 : #fde7e9;
    out property <length> radius: 8px;
}
```

`ui/meters.slint`:
```slint
import { Theme } from "theme.slint";

export struct MeterData { label: string, value: string, sub: string, spark: string, available: bool }

component Meter inherits Rectangle {
    in property <MeterData> data;
    in property <bool> selected;
    callback activated();
    min-width: 96px;
    border-radius: Theme.radius;
    background: selected ? Theme.accent.with-alpha(0.18) : Theme.card;
    border-width: 1px;
    border-color: selected ? Theme.accent : Theme.card-border;
    accessible-role: button;
    accessible-label: data.label + " meter";
    accessible-action-default => { root.activated(); }
    TouchArea { clicked => { root.activated(); } }
    VerticalLayout {
        padding: 10px;
        spacing: 2px;
        Text { text: data.label; color: Theme.muted; font-size: 12px; }
        Text { text: data.available ? data.value : "Unavailable"; color: Theme.text; font-size: data.available ? 20px : 14px; font-weight: 600; }
        Text { text: data.sub; color: Theme.muted; font-size: 11px; }
        Path {
            height: 26px;
            commands: data.spark;
            viewbox-width: 60;
            viewbox-height: 100;
            stroke: Theme.accent;
            stroke-width: 1.5px;
        }
    }
}

export component MetersRow inherits HorizontalLayout {
    in property <[MeterData]> meters;
    in property <int> selected;
    callback select(int);
    spacing: 8px;
    for m[i] in meters : Meter {
        data: m;
        selected: i == root.selected;
        activated => { root.select(i); }
    }
}
```

`ui/topstrip.slint`:
```slint
import { Theme } from "theme.slint";

export struct TileData { id: int, name: string, value: string, weight: float, hung: bool }

export component TopStrip inherits VerticalLayout {
    in property <[TileData]> tiles;
    in property <string> caption;
    callback open(int);
    spacing: 4px;
    Text { text: caption; color: Theme.muted; font-size: 11px; }
    HorizontalLayout {
        height: 52px;
        spacing: 4px;
        for t in tiles : Rectangle {
            horizontal-stretch: max(t.weight, 0.0001);
            min-width: 28px;
            border-radius: 6px;
            background: t.hung ? Theme.danger-bg : Theme.accent.with-alpha(0.28);
            accessible-role: button;
            accessible-label: t.name + " tile";
            accessible-action-default => { root.open(t.id); }
            TouchArea { clicked => { root.open(t.id); } }
            VerticalLayout {
                padding: 6px;
                Text { text: (t.hung ? "⚠ " : "") + t.name; color: Theme.text; font-size: 12px; overflow: elide; }
                Text { text: t.value; color: Theme.text; font-size: 12px; font-weight: 600; overflow: elide; }
            }
        }
    }
}
```

`ui/applist.slint`:
```slint
import { ListView, LineEdit, Button } from "std-widgets.slint";
import { Theme } from "theme.slint";

export struct RowData {
    id: int, depth: int, name: string, count: string,
    cpu: string, mem: string, disk: string, gpu: string, net: string,
    bar: float, hung: string, expandable: bool, expanded: bool,
}

component Cell inherits Text {
    in property <bool> strong;
    width: 84px;
    horizontal-alignment: right;
    vertical-alignment: center;
    font-size: 12px;
    color: Theme.text;
    font-weight: strong ? 600 : 400;
}

component HeaderCell inherits Text {
    width: 84px;
    horizontal-alignment: right;
    font-size: 11px;
    color: Theme.muted;
}

export component AppList inherits VerticalLayout {
    in property <[RowData]> rows;
    in property <int> metric;
    callback toggle(int);
    callback action(string, int);
    callback hover(bool);
    callback search(string);
    spacing: 6px;

    LineEdit {
        placeholder-text: "Search apps and processes";
        edited(text) => { root.search(text); }
    }
    HorizontalLayout {
        padding-left: 8px;
        padding-right: 8px;
        spacing: 8px;
        Text { text: "Name"; font-size: 11px; color: Theme.muted; horizontal-stretch: 1; }
        HeaderCell { text: "CPU"; }
        HeaderCell { text: "Memory"; }
        HeaderCell { text: "Disk"; }
        HeaderCell { text: "GPU"; }
        HeaderCell { text: "Network"; }
    }
    area := TouchArea {
        changed has-hover => { root.hover(self.has-hover); }
        ListView {
            for r in root.rows : Rectangle {
                height: 32px;
                background: r.hung != "" ? Theme.danger-bg : (row-touch.has-hover ? Theme.card : transparent);
                border-radius: 4px;
                row-touch := TouchArea {
                    double-clicked => { if (r.expandable) { root.toggle(r.id); } }
                }
                ContextMenuArea {
                    Menu {
                        MenuItem { title: "End task"; activated => { root.action("end-task", r.id); } }
                        MenuItem { title: "End process tree"; activated => { root.action("end-tree", r.id); } }
                        MenuItem { title: "Efficiency mode"; activated => { root.action("efficiency", r.id); } }
                        MenuSeparator { }
                        MenuItem { title: "Open file location"; activated => { root.action("open-location", r.id); } }
                        MenuItem { title: "Details"; activated => { root.action("details", r.id); } }
                    }
                    HorizontalLayout {
                        padding-left: 8px + r.depth * 20px;
                        padding-right: 8px;
                        spacing: 8px;
                        Text {
                            width: 14px;
                            text: r.expandable ? (r.expanded ? "▾" : "▸") : "";
                            color: Theme.muted;
                            vertical-alignment: center;
                            TouchArea { clicked => { if (r.expandable) { root.toggle(r.id); } } }
                        }
                        HorizontalLayout {
                            horizontal-stretch: 1;
                            spacing: 6px;
                            Text { text: r.name; color: Theme.text; font-size: 12px; vertical-alignment: center; overflow: elide; }
                            Text { text: r.count; color: Theme.muted; font-size: 12px; vertical-alignment: center; }
                            if r.hung != "" : Text { text: r.hung; color: Theme.danger-text; font-size: 11px; vertical-alignment: center; }
                            if r.hung != "" : Button {
                                text: "Force close";
                                accessible-label: "Force close " + r.name;
                                clicked => { root.action("force-close", r.id); }
                            }
                            if r.hung == "" && row-touch.has-hover && r.depth == 0 : Button {
                                text: "End task";
                                accessible-label: "End task " + r.name;
                                clicked => { root.action("end-task", r.id); }
                            }
                        }
                        Rectangle {
                            width: 60px;
                            Rectangle {
                                x: 0;
                                width: parent.width * r.bar;
                                height: 6px;
                                border-radius: 3px;
                                background: Theme.accent;
                            }
                        }
                        Cell { text: r.cpu; strong: root.metric == 0; }
                        Cell { text: r.mem; strong: root.metric == 1; }
                        Cell { text: r.disk; strong: root.metric == 2; }
                        Cell { text: r.gpu; strong: root.metric == 3; }
                        Cell { text: r.net; strong: root.metric == 4; }
                    }
                }
            }
        }
    }
}
```

`ui/overlays.slint`:
```slint
import { Button } from "std-widgets.slint";
import { Theme } from "theme.slint";

export component Banner inherits Rectangle {
    callback restart();
    callback dismiss();
    height: 40px;
    border-radius: Theme.radius;
    background: Theme.accent.with-alpha(0.15);
    HorizontalLayout {
        padding: 8px;
        spacing: 8px;
        Text { text: "Some processes need admin rights to show details or be ended."; color: Theme.text; vertical-alignment: center; horizontal-stretch: 1; }
        Button { text: "Restart as admin"; clicked => { root.restart(); } }
        Button { text: "Dismiss"; clicked => { root.dismiss(); } }
    }
}

export component Toast inherits Rectangle {
    in property <string> text;
    callback dismiss();
    height: 36px;
    border-radius: Theme.radius;
    background: Theme.card;
    border-width: 1px;
    border-color: Theme.card-border;
    HorizontalLayout {
        padding: 8px;
        Text { text: root.text; color: Theme.text; vertical-alignment: center; horizontal-stretch: 1; }
        Button { text: "OK"; clicked => { root.dismiss(); } }
    }
}

export component Modal inherits Rectangle {
    in property <string> title;
    in property <string> body;
    in property <string> confirm-label;
    in property <bool> show-cancel: true;
    callback done(bool);
    background: #00000066;
    TouchArea { }
    Rectangle {
        width: min(parent.width - 48px, 520px);
        height: layout.preferred-height;
        border-radius: Theme.radius;
        background: Theme.dark ? #2b2b2b : #ffffff;
        layout := VerticalLayout {
            padding: 16px;
            spacing: 12px;
            Text { text: root.title; color: Theme.text; font-size: 16px; font-weight: 600; }
            Text { text: root.body; color: Theme.text; wrap: word-wrap; }
            HorizontalLayout {
                alignment: end;
                spacing: 8px;
                if root.show-cancel : Button { text: "Cancel"; clicked => { root.done(false); } }
                Button { text: root.confirm-label; primary: true; clicked => { root.done(true); } }
            }
        }
    }
}
```

`ui/tray.slint`:
```slint
export component NegotiumTray inherits SystemTrayIcon {
    icon: @image-url("../assets/negotium.svg");
    tooltip: "Negotium";
    callback open();
    callback exit();
    Menu {
        MenuItem { title: "Open"; activated => { root.open(); } }
        MenuItem { title: "Exit"; activated => { root.exit(); } }
    }
}
```

`ui/main.slint`:
```slint
import { Theme } from "theme.slint";
import { MetersRow, MeterData } from "meters.slint";
import { TopStrip, TileData } from "topstrip.slint";
import { AppList, RowData } from "applist.slint";
import { Banner, Toast, Modal } from "overlays.slint";
export { NegotiumTray } from "tray.slint";
export { Theme, MeterData, TileData, RowData }

export component MainWindow inherits Window {
    title: "Negotium";
    icon: @image-url("../assets/negotium.svg");
    preferred-width: 1000px;
    preferred-height: 720px;
    min-width: 560px;
    min-height: 440px;
    default-font-family: "Segoe UI Variable Text";
    background: Theme.window-bg;

    in property <[MeterData]> meters;
    in property <int> selected-metric;
    in property <[TileData]> tiles;
    in property <string> top-caption;
    in property <[RowData]> rows;
    in property <bool> show-admin-banner;
    in property <string> toast;
    in property <string> confirm-text;
    in property <bool> confirm-visible;
    in property <string> details-text;
    in property <bool> details-visible;

    callback select-metric(int);
    callback open-tile(int);
    callback toggle-row(int);
    callback row-action(string, int);
    callback list-hover(bool);
    callback search-changed(string);
    callback restart-admin();
    callback dismiss-banner();
    callback confirm(bool);
    callback close-details();
    callback dismiss-toast();

    VerticalLayout {
        padding: 12px;
        spacing: 10px;
        if root.show-admin-banner : Banner {
            restart => { root.restart-admin(); }
            dismiss => { root.dismiss-banner(); }
        }
        MetersRow {
            meters: root.meters;
            selected: root.selected-metric;
            select(i) => { root.select-metric(i); }
        }
        TopStrip {
            tiles: root.tiles;
            caption: root.top-caption;
            open(id) => { root.open-tile(id); }
        }
        AppList {
            vertical-stretch: 1;
            rows: root.rows;
            metric: root.selected-metric;
            toggle(id) => { root.toggle-row(id); }
            action(name, id) => { root.row-action(name, id); }
            hover(h) => { root.list-hover(h); }
            search(text) => { root.search-changed(text); }
        }
        if root.toast != "" : Toast {
            text: root.toast;
            dismiss => { root.dismiss-toast(); }
        }
    }
    if root.confirm-visible : Modal {
        title: "End a critical system process?";
        body: root.confirm-text;
        confirm-label: "End process";
        done(ok) => { root.confirm(ok); }
    }
    if root.details-visible : Modal {
        title: "Details";
        body: root.details-text;
        confirm-label: "Close";
        show-cancel: false;
        done(ok) => { root.close-details(); }
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p negotium --test ui` then `cargo build -p negotium`
Expected: 2 UI tests pass; build warning-free. If a Slint item differs in 1.18 (e.g. `with-alpha`, `ContextMenuArea`/`Menu`, `default-font-family`, `SystemTrayIcon` properties), adapt per Global Constraints and note it.

- [ ] **Step 5: Commit**

```bash
git add crates/app
git commit -m "feat(app): Fluent Slint overview UI with tray component"
```

---

### Task 10: Bridge — live frames into the window

**Files:**
- Create: `crates/app/src/bridge.rs`
- Modify: `crates/app/src/lib.rs`, `crates/app/src/main.rs`
- Test: unit tests in `bridge.rs`

**Interfaces:**
- Consumes: `view::{Frame, RowView, MeterView, TileView, visible_rows}`, `controls::Controls`, `worker::spawn`, generated `MainWindow`, `MeterData`, `TileData`, `RowData`, `NegotiumTray`.
- Produces:
  - `bridge::RowStore<T>` trait (`len`, `get`, `set`, `push`, `pop`) implemented for `slint::VecModel<T>`, and `bridge::sync_model<T: Clone + PartialEq, S: RowStore<T>>(store: &S, items: Vec<T>)` — in-place diff: `set` only where changed, then `push`/`pop` the tail. The trait has no "clear/replace all" operation, so a full model reset (which would jump the list's scroll position) is impossible by construction.
  - `bridge::Bridge` (UI-thread state: models, expanded `HashSet<i32>`, search `String`, last `Frame`) with `install(ui: &MainWindow, controls: Arc<Controls>) -> Rc<RefCell<Bridge>>`, `apply(&mut self, ui: &MainWindow, frame: Frame)`, `refresh_rows(&self)` (re-applies expand/search to the last frame instantly), `target(id) -> Option<Target>`, `all_procs() -> &[(u32, i64, u32)]`.
  - `main.rs` wires: worker sink → `slint::invoke_from_event_loop` → `Bridge::apply`; meter click → `controls.set_metric` + immediate `selected-metric`; list hover → `controls.set_frozen`; search/toggle → `refresh_rows`; tile click → expand that app and clear search.

- [ ] **Step 1: Write the failing test** (bottom of `bridge.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// Records every operation so the test can prove updates are minimal and in place.
    #[derive(Default)]
    struct Recording {
        items: RefCell<Vec<i32>>,
        ops: RefCell<Vec<String>>,
    }

    impl RowStore<i32> for Recording {
        fn len(&self) -> usize { self.items.borrow().len() }
        fn get(&self, i: usize) -> Option<i32> { self.items.borrow().get(i).copied() }
        fn set(&self, i: usize, v: i32) { self.ops.borrow_mut().push(format!("set {i}")); self.items.borrow_mut()[i] = v; }
        fn push(&self, v: i32) { self.ops.borrow_mut().push("push".into()); self.items.borrow_mut().push(v); }
        fn pop(&self) { self.ops.borrow_mut().push("pop".into()); self.items.borrow_mut().pop(); }
    }

    #[test]
    fn sync_updates_only_changed_rows_in_place() {
        let store = Recording::default();
        *store.items.borrow_mut() = vec![1, 2, 3, 4];
        sync_model(&store, vec![1, 9, 3]);
        assert_eq!(*store.items.borrow(), [1, 9, 3]);
        assert_eq!(*store.ops.borrow(), ["set 1", "pop"], "unchanged rows are untouched");
        store.ops.borrow_mut().clear();
        sync_model(&store, vec![1, 9, 3, 5, 6]);
        assert_eq!(*store.ops.borrow(), ["push", "push"]);
    }

    #[test]
    fn vec_model_store_round_trips() {
        let model = VecModel::from(vec![1, 2]);
        sync_model(&model, vec![5, 2, 7]);
        assert_eq!((0..RowStore::len(&model)).map(|i| RowStore::get(&model, i).unwrap()).collect::<Vec<_>>(), [5, 2, 7]);
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p negotium --lib bridge`
Expected: FAIL to compile.

- [ ] **Step 3: Implement `bridge.rs`**

```rust
//! UI-thread glue: applies worker Frames to Slint models in place and keeps
//! expand/search state local so they react instantly. No Windows calls here.
use crate::controls::Controls;
use crate::view::{Frame, Target, visible_rows};
use crate::{MainWindow, MeterData, RowData, TileData};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

/// Minimal row storage the sync can use. Deliberately has no "replace all".
pub trait RowStore<T> {
    fn len(&self) -> usize;
    fn get(&self, i: usize) -> Option<T>;
    fn set(&self, i: usize, v: T);
    fn push(&self, v: T);
    fn pop(&self);
}

impl<T: Clone + 'static> RowStore<T> for VecModel<T> {
    fn len(&self) -> usize { self.row_count() }
    fn get(&self, i: usize) -> Option<T> { self.row_data(i) }
    fn set(&self, i: usize, v: T) { self.set_row_data(i, v) }
    fn push(&self, v: T) { VecModel::push(self, v) }
    fn pop(&self) { let n = self.row_count(); if n > 0 { self.remove(n - 1); } }
}

pub fn sync_model<T: Clone + PartialEq, S: RowStore<T>>(store: &S, items: Vec<T>) {
    let common = store.len().min(items.len());
    for (i, item) in items.iter().enumerate().take(common) {
        if store.get(i).as_ref() != Some(item) {
            store.set(i, item.clone());
        }
    }
    while store.len() > items.len() {
        store.pop();
    }
    for item in items.into_iter().skip(common) {
        store.push(item);
    }
}

pub struct Bridge {
    meters: Rc<VecModel<MeterData>>,
    tiles: Rc<VecModel<TileData>>,
    rows: Rc<VecModel<RowData>>,
    pub expanded: HashSet<i32>,
    pub search: String,
    last: Frame,
    banner_dismissed: bool,
    pub controls: Arc<Controls>,
    ui: slint::Weak<MainWindow>,
}

impl Bridge {
    pub fn install(ui: &MainWindow, controls: Arc<Controls>) -> Rc<RefCell<Bridge>> {
        let b = Bridge {
            meters: Rc::new(VecModel::default()),
            tiles: Rc::new(VecModel::default()),
            rows: Rc::new(VecModel::default()),
            expanded: HashSet::new(),
            search: String::new(),
            last: Frame::default(),
            banner_dismissed: false,
            controls,
            ui: ui.as_weak(),
        };
        ui.set_meters(ModelRc::from(b.meters.clone()));
        ui.set_tiles(ModelRc::from(b.tiles.clone()));
        ui.set_rows(ModelRc::from(b.rows.clone()));
        Rc::new(RefCell::new(b))
    }

    pub fn apply(&mut self, ui: &MainWindow, frame: Frame) {
        sync_model(&*self.meters, frame.meters.iter().map(|m| MeterData {
            label: m.label.into(), value: m.value.clone().into(), sub: m.sub.clone().into(), spark: m.spark.clone().into(), available: m.available,
        }).collect());
        sync_model(&*self.tiles, frame.tiles.iter().map(|t| TileData {
            id: t.id, name: t.name.clone().into(), value: t.value.clone().into(), weight: t.weight, hung: t.hung,
        }).collect());
        ui.set_top_caption(frame.caption.clone().into());
        ui.set_show_admin_banner(frame.admin_banner && !self.banner_dismissed);
        self.last = frame;
        self.refresh_rows();
    }

    pub fn refresh_rows(&self) {
        let visible = visible_rows(&self.last.rows, &self.expanded, &self.search);
        sync_model(&*self.rows, visible.into_iter().map(|r| RowData {
            id: r.id, depth: r.depth, name: r.name.into(), count: r.count.into(), cpu: r.cpu.into(), mem: r.mem.into(),
            disk: r.disk.into(), gpu: r.gpu.into(), net: r.net.into(), bar: r.bar, hung: r.hung.into(),
            expandable: r.expandable, expanded: self.expanded.contains(&r.id),
        }).collect());
    }

    pub fn dismiss_banner(&mut self) {
        self.banner_dismissed = true;
        if let Some(ui) = self.ui.upgrade() {
            ui.set_show_admin_banner(false);
        }
    }

    pub fn target(&self, id: i32) -> Option<Target> {
        self.last.targets.get(&id).cloned()
    }

    pub fn all_procs(&self) -> &[(u32, i64, u32)] {
        &self.last.all_procs
    }
}
```

`main.rs`:
```rust
#![windows_subsystem = "windows"]

use negotium::bridge::Bridge;
use negotium::controls::Controls;
use negotium::{MainWindow, NegotiumTray, worker};
use negotium_model::Metric;
use slint::ComponentHandle;
use std::sync::Arc;

fn main() -> Result<(), slint::PlatformError> {
    let ui = MainWindow::new()?;
    let tray = NegotiumTray::new()?;
    let controls = Arc::new(Controls::default());
    let bridge = Bridge::install(&ui, controls.clone());

    // Worker → UI thread.
    let weak = ui.as_weak();
    let sink_bridge_ui = weak.clone();
    let _worker = worker::spawn(controls.clone(), move |frame| {
        let weak = sink_bridge_ui.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = weak.upgrade() {
                negotium::bridge::with_bridge(|b| b.apply(&ui, frame));
            }
        });
    });
    negotium::bridge::set_current(bridge.clone());

    {
        let controls = controls.clone();
        let weak = weak.clone();
        ui.on_select_metric(move |i| {
            controls.set_metric(Metric::ALL[(i.max(0) as usize).min(4)]);
            if let Some(ui) = weak.upgrade() {
                ui.set_selected_metric(i);
            }
        });
    }
    {
        let controls = controls.clone();
        ui.on_list_hover(move |h| controls.set_frozen(h));
    }
    {
        let b = bridge.clone();
        ui.on_search_changed(move |text| {
            b.borrow_mut().search = text.to_string();
            b.borrow().refresh_rows();
        });
    }
    {
        let b = bridge.clone();
        ui.on_toggle_row(move |id| {
            {
                let mut b = b.borrow_mut();
                if !b.expanded.remove(&id) {
                    b.expanded.insert(id);
                }
            }
            b.borrow().refresh_rows();
        });
    }
    {
        let b = bridge.clone();
        ui.on_open_tile(move |id| {
            {
                let mut b = b.borrow_mut();
                b.search.clear();
                b.expanded.insert(id);
            }
            b.borrow().refresh_rows();
        });
    }
    {
        let b = bridge.clone();
        ui.on_dismiss_banner(move || b.borrow_mut().dismiss_banner());
    }

    // Close hides to tray (spec §5.5); the worker slows to 2 s while hidden.
    {
        let controls = controls.clone();
        ui.window().on_close_requested(move || {
            controls.set_visible(false);
            slint::CloseRequestResponse::HideWindow
        });
    }
    {
        let controls = controls.clone();
        let weak = weak.clone();
        tray.on_open(move || {
            controls.set_visible(true);
            if let Some(ui) = weak.upgrade() {
                let _ = ui.show();
            }
        });
    }
    {
        let controls = controls.clone();
        tray.on_exit(move || {
            controls.request_stop();
            let _ = slint::quit_event_loop();
        });
    }

    ui.show()?;
    tray.show()?;
    slint::run_event_loop_until_quit()
}
```
Add to `bridge.rs` the thread-local accessor used above:
```rust
thread_local! {
    static CURRENT: RefCell<Option<Rc<RefCell<Bridge>>>> = const { RefCell::new(None) };
}

pub fn set_current(b: Rc<RefCell<Bridge>>) {
    CURRENT.with(|c| *c.borrow_mut() = Some(b));
}

pub fn with_bridge(f: impl FnOnce(&mut Bridge)) {
    CURRENT.with(|c| {
        if let Some(b) = c.borrow().as_ref() {
            f(&mut b.borrow_mut());
        }
    });
}
```
`main.rs` needs `negotium-model` as a dependency of the app crate: add `negotium-model.workspace = true` (already listed in Task 7's manifest). `lib.rs`: add `pub mod bridge;`.

- [ ] **Step 4: Run tests and the app**

Run: `cargo test -p negotium` then `cargo run -p negotium --release`
Expected: tests pass; the window opens within ~1 s showing five meters, a top strip and a populated list that updates every second; clicking a meter re-ranks immediately; typing in search filters instantly; closing the window hides it and the tray icon's "Open" restores it; "Exit" quits. Capture a screenshot for the report:
```powershell
Add-Type -AssemblyName System.Windows.Forms,System.Drawing; $b=[System.Windows.Forms.Screen]::PrimaryScreen.Bounds; $bmp=New-Object System.Drawing.Bitmap $b.Width,$b.Height; [System.Drawing.Graphics]::FromImage($bmp).CopyFromScreen($b.Location,[System.Drawing.Point]::Empty,$b.Size); $bmp.Save("$env:TEMP\negotium-task10.png")
```
Read the PNG and describe what is visible in the report. Also check the hover-freeze manually: with the pointer resting over the list, rows must not reorder for > 2 s; if the outer `TouchArea`'s `has-hover` does not report while the pointer is over list rows, move hover reporting to the row `TouchArea`s (debounced: report `false` only when no row is hovered) and note it.

- [ ] **Step 5: Commit**

```bash
git add crates/app
git commit -m "feat(app): live overview — worker frames, metric switch, search, expand, tray"
```

---

### Task 11: Row actions — dispatch, confirmation, notifications, details

**Files:**
- Create: `crates/app/src/dispatch.rs`
- Modify: `crates/app/src/lib.rs`, `crates/app/src/main.rs`
- Test: unit tests in `dispatch.rs`

**Interfaces:**
- Consumes: `view::Target`, `negotium_actions::*`, `negotium_model::descendants_deepest_first`, `Bridge::{target, all_procs}`.
- Produces:
  - `dispatch::Plan { Run(Job), Confirm(String, Job), Nothing }`, `dispatch::Job { kind: JobKind, name: String, targets: Vec<(u32, i64)>, path: Option<String> }`, `JobKind { EndMany, ForceClose, Efficiency, OpenLocation, Details }`.
  - `dispatch::plan(action: &str, target: &Target, all_procs: &[(u32, i64, u32)]) -> Plan` (pure): `end-task` → root last (non-root procs first, then root); `end-tree` → `descendants_deepest_first` from root (or all procs if no root); `force-close` → `hung_pid` or root; `efficiency` → all procs; `open-location` → `image_path` (Nothing if `None`); `details` → root or first proc. `end-*` on a `critical` target → `Confirm` with text: `"Ending {name} will make Windows unusable or shut it down, and unsaved work in every app will be lost."`.
  - `dispatch::run(job: Job, elevated: bool) -> String` (blocking, call on an action thread): returns the user-facing notification text (empty string for success without message), or for `Details` the formatted details text prefixed with `"details:"`.
  - `main.rs`: `row-action` → `plan` → `Confirm` shows the modal (pending job kept on the UI thread) → `run` on `std::thread::spawn` → result back via `invoke_from_event_loop` → `toast` or details modal.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn target(critical: bool) -> Target {
        Target {
            name: "App".into(), root: Some((10, 1)), procs: vec![(10, 1), (11, 2), (12, 3)],
            hung_pid: Some((11, 2)), image_path: Some(r"C:\App\app.exe".into()), critical,
        }
    }
    const ALL: &[(u32, i64, u32)] = &[(10, 1, 4), (11, 2, 10), (12, 3, 11), (13, 4, 4)];

    #[test]
    fn end_task_ends_children_before_root() {
        let Plan::Run(job) = plan("end-task", &target(false), ALL) else { panic!() };
        assert_eq!(job.kind, JobKind::EndMany);
        assert_eq!(job.targets.last(), Some(&(10, 1)));
        assert_eq!(job.targets.len(), 3);
    }

    #[test]
    fn end_tree_is_deepest_first() {
        let Plan::Run(job) = plan("end-tree", &target(false), ALL) else { panic!() };
        assert_eq!(job.targets, vec![(12, 3), (11, 2), (10, 1)]);
    }

    #[test]
    fn force_close_targets_the_hung_process() {
        let Plan::Run(job) = plan("force-close", &target(false), ALL) else { panic!() };
        assert_eq!((job.kind, job.targets), (JobKind::ForceClose, vec![(11, 2)]));
    }

    #[test]
    fn critical_targets_need_confirmation() {
        assert!(matches!(plan("end-task", &target(true), ALL), Plan::Confirm(text, _) if text.contains("unusable")));
        assert!(matches!(plan("details", &target(true), ALL), Plan::Run(_)));
    }

    #[test]
    fn missing_path_means_nothing_to_open() {
        let mut t = target(false);
        t.image_path = None;
        assert!(matches!(plan("open-location", &t, ALL), Plan::Nothing));
        assert!(matches!(plan("unknown-action", &t, ALL), Plan::Nothing));
    }
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p negotium --lib dispatch`
Expected: FAIL to compile.

- [ ] **Step 3: Implement `dispatch.rs`**

```rust
//! Turns a row action into a job (pure), and runs jobs on action threads.
use crate::view::Target;
use negotium_actions::{
    ActionError, end_many, force_close, open_file_location, process_details, toggle_efficiency_mode,
};
use negotium_model::descendants_deepest_first;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobKind { EndMany, ForceClose, Efficiency, OpenLocation, Details }

#[derive(Clone, Debug, PartialEq)]
pub struct Job { pub kind: JobKind, pub name: String, pub targets: Vec<(u32, i64)>, pub path: Option<String> }

#[derive(Clone, Debug, PartialEq)]
pub enum Plan { Run(Job), Confirm(String, Job), Nothing }

pub fn plan(action: &str, t: &Target, all_procs: &[(u32, i64, u32)]) -> Plan {
    let job = |kind, targets: Vec<(u32, i64)>| Job { kind, name: t.name.clone(), targets, path: t.image_path.clone() };
    let first = t.root.or_else(|| t.procs.first().copied());
    let planned = match action {
        "end-task" => {
            let mut v: Vec<_> = t.procs.iter().copied().filter(|p| Some(*p) != t.root).collect();
            v.extend(t.root);
            job(JobKind::EndMany, v)
        }
        "end-tree" => match t.root {
            Some(root) => job(JobKind::EndMany, descendants_deepest_first(all_procs, root)),
            None => job(JobKind::EndMany, t.procs.clone()),
        },
        "force-close" => match t.hung_pid.or(first) {
            Some(p) => job(JobKind::ForceClose, vec![p]),
            None => return Plan::Nothing,
        },
        "efficiency" => job(JobKind::Efficiency, t.procs.clone()),
        "open-location" if t.image_path.is_some() => job(JobKind::OpenLocation, vec![]),
        "details" => match first {
            Some(p) => job(JobKind::Details, vec![p]),
            None => return Plan::Nothing,
        },
        _ => return Plan::Nothing,
    };
    if t.critical && planned.kind == JobKind::EndMany {
        let text = format!(
            "Ending {} will make Windows unusable or shut it down, and unsaved work in every app will be lost.",
            t.name
        );
        return Plan::Confirm(text, planned);
    }
    Plan::Run(planned)
}

pub fn run(job: Job, elevated: bool) -> String {
    let fail = |e: ActionError| e.message(&job.name, elevated);
    match job.kind {
        JobKind::EndMany => {
            let failures: Vec<_> = end_many(&job.targets).into_iter().filter_map(|(_, r)| r.err()).collect();
            match failures.first() {
                None => String::new(),
                Some(e) => fail(e.clone()),
            }
        }
        JobKind::ForceClose => match force_close(job.targets[0].0, job.targets[0].1) {
            Ok(()) | Err(ActionError::AlreadyExited) => String::new(),
            Err(e) => fail(e),
        },
        JobKind::Efficiency => match toggle_efficiency_mode(&job.targets) {
            Ok(true) => format!("Efficiency mode on for {}.", job.name),
            Ok(false) => format!("Efficiency mode off for {}.", job.name),
            Err(e) => fail(e),
        },
        JobKind::OpenLocation => match job.path.as_deref().map(open_file_location) {
            Some(Ok(())) => String::new(),
            Some(Err(e)) => fail(e),
            None => String::new(),
        },
        JobKind::Details => match process_details(job.targets[0].0, job.targets[0].1) {
            Ok(d) => format!(
                "details:Name: {}\nPID: {}\nPath: {}\nCommand line: {}\nUser: {}\nPriority: {}",
                job.name,
                d.pid,
                d.image_path.as_deref().unwrap_or("—"),
                d.command_line.as_deref().unwrap_or("—"),
                d.user.as_deref().unwrap_or("—"),
                d.priority.as_deref().unwrap_or("—"),
            ),
            Err(e) => fail(e),
        },
    }
}
```
`lib.rs`: `pub mod dispatch;`.

`main.rs` additions (before `ui.show()`):
```rust
    let elevated = negotium_actions::is_elevated();
    let pending: std::rc::Rc<std::cell::RefCell<Option<negotium::dispatch::Job>>> = Default::default();
    let start_job = {
        let weak = weak.clone();
        move |job: negotium::dispatch::Job| {
            let weak = weak.clone();
            std::thread::spawn(move || {
                let text = negotium::dispatch::run(job, elevated);
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = weak.upgrade() {
                        if let Some(details) = text.strip_prefix("details:") {
                            ui.set_details_text(details.into());
                            ui.set_details_visible(true);
                        } else if !text.is_empty() {
                            ui.set_toast(text.into());
                        }
                    }
                });
            });
        }
    };
    {
        let b = bridge.clone();
        let weak = weak.clone();
        let pending = pending.clone();
        let start_job = start_job.clone();
        ui.on_row_action(move |action, id| {
            let Some(target) = b.borrow().target(id) else { return };
            match negotium::dispatch::plan(action.as_str(), &target, b.borrow().all_procs()) {
                negotium::dispatch::Plan::Run(job) => start_job(job),
                negotium::dispatch::Plan::Confirm(text, job) => {
                    *pending.borrow_mut() = Some(job);
                    if let Some(ui) = weak.upgrade() {
                        ui.set_confirm_text(text.into());
                        ui.set_confirm_visible(true);
                    }
                }
                negotium::dispatch::Plan::Nothing => {}
            }
        });
    }
    {
        let weak = weak.clone();
        ui.on_confirm(move |ok| {
            if let Some(ui) = weak.upgrade() {
                ui.set_confirm_visible(false);
            }
            if let Some(job) = pending.borrow_mut().take() {
                if ok {
                    start_job(job);
                }
            }
        });
    }
    {
        let weak = weak.clone();
        ui.on_close_details(move || if let Some(ui) = weak.upgrade() { ui.set_details_visible(false) });
    }
    {
        let weak = weak.clone();
        ui.on_dismiss_toast(move || if let Some(ui) = weak.upgrade() { ui.set_toast("".into()) });
    }
    ui.on_restart_admin(move || {
        if negotium_actions::restart_as_admin("").is_ok() {
            let _ = slint::quit_event_loop();
        }
    });
```
Make `start_job` `Clone` (it captures only `Weak` and `bool`, so `#[derive]`-free closures are `Clone` when their captures are; if not, wrap it in `Rc`). Add `negotium-actions` to the app's dependencies (already in Task 7's manifest).

- [ ] **Step 4: Run tests and a manual action check**

Run: `cargo test -p negotium`, then `cargo run -p negotium --release`; start `C:\Windows\System32\PING.EXE -t 127.0.0.1` in a terminal, find "TCP/IP Ping Command" via search, right-click → Details (modal shows its command line), → Efficiency mode (toast "Efficiency mode on …"), → End task (row disappears within ~2 s). Record what happened in the report.
Expected: all tests pass; the manual flow behaves as listed.

- [ ] **Step 5: Commit**

```bash
git add crates/app
git commit -m "feat(app): row actions with confirmation, notifications and details"
```

---

### Task 12: Window chrome — Mica, dark title bar, accent colour

**Files:**
- Create: `crates/app/src/platform.rs`
- Modify: `crates/app/src/lib.rs`, `crates/app/src/main.rs`
- Test: unit test in `platform.rs`

**Interfaces:**
- Produces:
  - `platform::accent_from_dword(v: u32) -> (u8, u8, u8)` — `AccentColor` is `0xAABBGGRR`.
  - `platform::read_accent() -> Option<(u8, u8, u8)>` — `HKCU\Software\Microsoft\Windows\DWM\AccentColor`.
  - `platform::apply_window_chrome(ui: &MainWindow) -> bool` — returns true if Mica was applied: gets the HWND via `raw-window-handle`, sets `DWMWA_USE_IMMERSIVE_DARK_MODE` from `Theme.dark`, and `DWMWA_SYSTEMBACKDROP_TYPE = DWMSBT_MAINWINDOW`; sets `Theme.mica = true` only if both calls succeed.
  - `main.rs`: sets `Theme.accent` from `read_accent()` at startup; calls `apply_window_chrome` once via `slint::Timer::single_shot(Duration::from_millis(0), …)` after `ui.show()` (the HWND exists only after the window is created).

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accent_dword_is_abgr() {
        assert_eq!(accent_from_dword(0xFFD47800), (0x00, 0x78, 0xD4));
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p negotium --lib platform`
Expected: FAIL to compile.

- [ ] **Step 3: Implement**

```rust
//! One-time window setup on the UI thread (allowed by the Global Constraints):
//! Mica backdrop, title-bar theme, and the user's accent colour.
use crate::{MainWindow, Theme};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::ComponentHandle;
use std::ffi::c_void;
use windows::Win32::Foundation::{HWND, ERROR_SUCCESS};
use windows::Win32::Graphics::Dwm::{
    DWMSBT_MAINWINDOW, DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute,
};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows::core::{BOOL, w};

pub fn accent_from_dword(v: u32) -> (u8, u8, u8) {
    ((v & 0xFF) as u8, ((v >> 8) & 0xFF) as u8, ((v >> 16) & 0xFF) as u8)
}

pub fn read_accent() -> Option<(u8, u8, u8)> {
    let mut value = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: `value`/`size` are valid for a REG_DWORD read.
    let status = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, w!("Software\\Microsoft\\Windows\\DWM"), w!("AccentColor"), RRF_RT_REG_DWORD, None, Some(&mut value as *mut u32 as *mut c_void), Some(&mut size))
    };
    (status == ERROR_SUCCESS).then(|| accent_from_dword(value))
}

fn hwnd_of(ui: &MainWindow) -> Option<HWND> {
    let handle = ui.window().window_handle();
    let raw = handle.window_handle().ok()?.as_raw();
    match raw {
        RawWindowHandle::Win32(h) => Some(HWND(h.hwnd.get() as *mut c_void)),
        _ => None,
    }
}

pub fn apply_window_chrome(ui: &MainWindow) -> bool {
    let Some(hwnd) = hwnd_of(ui) else { return false };
    let dark = BOOL::from(ui.global::<Theme>().get_dark());
    let backdrop = DWMSBT_MAINWINDOW;
    // SAFETY: valid HWND of our own window; attribute payloads match their documented sizes.
    let dark_ok = unsafe {
        DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const _ as *const c_void, std::mem::size_of::<BOOL>() as u32)
    }
    .is_ok();
    // SAFETY: as above; DWMSBT_MAINWINDOW is a DWM_SYSTEMBACKDROP_TYPE (i32).
    let mica_ok = unsafe {
        DwmSetWindowAttribute(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, &backdrop as *const _ as *const c_void, std::mem::size_of_val(&backdrop) as u32)
    }
    .is_ok();
    let applied = dark_ok && mica_ok;
    ui.global::<Theme>().set_mica(applied);
    applied
}
```
`lib.rs`: `pub mod platform;`. `main.rs` (after `MainWindow::new()`):
```rust
    if let Some((r, g, b)) = negotium::platform::read_accent() {
        ui.global::<negotium::Theme>().set_accent(slint::Color::from_rgb_u8(r, g, b));
    }
```
and after `ui.show()?;`:
```rust
    {
        let weak = weak.clone();
        slint::Timer::single_shot(std::time::Duration::from_millis(0), move || {
            if let Some(ui) = weak.upgrade() {
                negotium::platform::apply_window_chrome(&ui);
            }
        });
    }
```

- [ ] **Step 4: Run tests and verify visually**

Run: `cargo test -p negotium`, then `cargo run -p negotium --release`, take a screenshot (same PowerShell snippet as Task 10, file `negotium-task12.png`) and read it.
Expected: tests pass; the window shows a Mica (desktop-tinted) background in the system theme with accent-coloured meter selection. **If the background renders black or opaque grey instead of Mica** (the renderer did not create a transparent surface), stop setting `Theme.mica` to true (keep the dark title bar), so the solid Fluent background is used, and record the finding and screenshot description in the report — the controller will rule on it.

- [ ] **Step 5: Commit**

```bash
git add crates/app
git commit -m "feat(app): Mica backdrop, themed title bar and accent colour"
```

---

### Task 13: Benchmark Negotium the same way as Task Manager

**Files:**
- Create: `crates/app/src/framestats.rs`
- Modify: `crates/app/src/lib.rs`, `crates/app/src/main.rs`, `crates/bench/src/main.rs`, `docs/benchmarks/README.md`
- Test: unit test in `framestats.rs`; build + non-elevated refusal check for the bench

**Interfaces:**
- Produces:
  - App CLI flag `--exit-on-close`: the window's close quits the app instead of hiding (bench use only).
  - Env var `NEGOTIUM_FRAME_STATS=<path>`: the app records each frame's render time with `Window::set_rendering_notifier` (`BeforeRendering` → `AfterRendering`) and, on exit, writes `{"frames": N, "render_ms": {median, p95, max, mean}, "over_16ms": K}` to `<path>`.
  - `framestats::FrameStats` with `begin()`, `end()`, `summary_json() -> String`.
  - Bench: target keyword `negotium` → `<bench exe dir>\negotium.exe` launched with `--exit-on-close`; when the target is Negotium, the bench sets `NEGOTIUM_FRAME_STATS` to a temp path before launch and embeds the file's JSON in the report as `frame_stats`.

- [ ] **Step 1: Write the failing test** (bottom of `framestats.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarises_and_counts_slow_frames() {
        let mut s = FrameStats::default();
        for ms in [2.0, 4.0, 20.0] {
            s.push_ms(ms);
        }
        let json: serde_json::Value = serde_json::from_str(&s.summary_json()).unwrap();
        assert_eq!(json["frames"], 3);
        assert_eq!(json["over_16ms"], 1);
        assert_eq!(json["render_ms"]["max"], 20.0);
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p negotium --lib framestats`
Expected: FAIL to compile.

- [ ] **Step 3: Implement**

`framestats.rs`:
```rust
//! Optional render-time statistics for the benchmark (NEGOTIUM_FRAME_STATS=<path>).
use std::time::Instant;

#[derive(Default)]
pub struct FrameStats {
    started: Option<Instant>,
    ms: Vec<f64>,
}

impl FrameStats {
    pub fn begin(&mut self) {
        self.started = Some(Instant::now());
    }
    pub fn end(&mut self) {
        if let Some(t) = self.started.take() {
            self.push_ms(t.elapsed().as_secs_f64() * 1000.0);
        }
    }
    pub fn push_ms(&mut self, ms: f64) {
        self.ms.push(ms);
    }
    pub fn summary_json(&self) -> String {
        let mut v = self.ms.clone();
        v.sort_by(f64::total_cmp);
        let at = |p: f64| if v.is_empty() { 0.0 } else { v[((v.len() - 1) as f64 * p).round() as usize] };
        let mean = if v.is_empty() { 0.0 } else { v.iter().sum::<f64>() / v.len() as f64 };
        serde_json::json!({
            "frames": v.len(),
            "render_ms": { "median": at(0.5), "p95": at(0.95), "max": at(1.0), "mean": mean },
            "over_16ms": v.iter().filter(|&&m| m > 16.0).count(),
        })
        .to_string()
    }
}
```
`lib.rs`: `pub mod framestats;`.

`main.rs`:
- parse `let exit_on_close = std::env::args().any(|a| a == "--exit-on-close");` and in `on_close_requested`, if `exit_on_close`, call `controls.request_stop(); let _ = slint::quit_event_loop();` and return `HideWindow`.
- if `std::env::var("NEGOTIUM_FRAME_STATS")` is set: create `Rc<RefCell<FrameStats>>`; `ui.window().set_rendering_notifier(move |state, _| match state { slint::RenderingState::BeforeRendering => stats.borrow_mut().begin(), slint::RenderingState::AfterRendering => stats.borrow_mut().end(), _ => {} })` (ignore the `Err` if the renderer doesn't support notifiers, and note it); after `run_event_loop_until_quit()` returns, write `summary_json()` to the path.

`crates/bench/src/main.rs`:
- target resolution: `"negotium"` → `std::env::current_exe()?.with_file_name("negotium.exe")`, launch parameters `--exit-on-close` (set `SHELLEXECUTEINFOW.lpParameters`; for `taskmgr` leave it null).
- before launching Negotium: `let stats_path = std::env::temp_dir().join("negotium-frame-stats.json");` remove any stale file, then `// SAFETY: single-threaded at this point; no other thread reads the environment.` `unsafe { std::env::set_var("NEGOTIUM_FRAME_STATS", &stats_path) };`
- after the final close: if the file exists, parse it with `serde_json::from_str::<serde_json::Value>` and add it to the report as `frame_stats` (add `frame_stats: Option<serde_json::Value>` to `Report`).

- [ ] **Step 4: Verify**

Run: `cargo test --workspace`; `cargo build --workspace --release`; `cargo run -p negotium-bench --release -- negotium --runs 1 --sample-secs 1` (non-elevated: must refuse cleanly with the elevated-terminal message).
Expected: tests pass, warning-free; refusal message printed.

- [ ] **Step 5: Add the Negotium column to the README**

In `docs/benchmarks/README.md` under the Task Manager baseline table, add a section:
```markdown
## Negotium vs Task Manager

Run both from an elevated terminal (close both apps first):

    cargo build --release -p negotium -p negotium-bench
    target\release\negotium-bench.exe taskmgr  --runs 5 --sample-secs 60 --out docs/benchmarks/taskmgr.json
    target\release\negotium-bench.exe negotium --runs 5 --sample-secs 60 --out docs/benchmarks/negotium.json

| Metric | Task Manager | Negotium | Target (spec §8) |
|---|---|---|---|
| Launch → first window, run 1 (cold) | pending | pending | ≤ 250 ms |
| Launch → first window, median of 5 | pending | pending | ≤ 250 ms |
| CPU while visible, mean / max (% of total) | pending | pending | ≤ 0.5 % of one core |
| Private working set, max | pending | pending | ≤ 60 MB |
| UI responsiveness (WM_NULL) median / p95 / max | pending | pending | — |
| Render time per frame, max / frames > 16 ms | n/a | pending | none > 16 ms |
```

- [ ] **Step 6: Commit**

```bash
git add crates/app crates/bench docs/benchmarks/README.md
git commit -m "feat(bench): measure Negotium alongside Task Manager, with frame stats"
```

---

## After this plan

- The human partner runs both benchmark commands elevated and the manual frozen-window check (Task 1 Step 5); results fill the README tables.
- Plan 3 (Performance, Startup, Settings, history scrub) builds on `Frame`, `History`, `Controls`, and the Slint theme. Carry: per-disk/per-adapter history series and time-based capacity; per-core index ≠ core id; Hyper-V network double count.
- Plan 4 adds single-instance handling, the Ctrl+Shift+Esc redirect, the no-prompt admin task, the installer, the CPU-temperature add-on and per-app network (SRUM).
- Tick cost (≈ 20 ms on ~680 processes) is revisited against the measured baselines before choosing an optimisation.
