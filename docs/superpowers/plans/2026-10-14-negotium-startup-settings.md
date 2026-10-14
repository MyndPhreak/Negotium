# Negotium Plan 3b — Startup and Settings Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the Startup page (Run keys, Startup folders and packaged startup tasks, enabled/disabled through the same values Task Manager uses) and the Settings page (refresh rate, theme, always on top, keep running in the tray), persisted to a settings file, and make "Restart as admin" come back on the page it was started from.

**Architecture:** Startup data lives in a new `negotium-actions::startup` module: listing and toggling run on action threads (registry, file system, COM shell links, WinRT package names), never on the UI thread; the UI keeps the last list and shows it through the existing in-place model sync. Settings are a small serde struct in `crates/app/src/settings.rs`, loaded once at startup and saved off the UI thread; applying them touches `Controls` (refresh interval), window properties (always on top), the close handler (tray) and the colour scheme plus DWM chrome (theme). The rail from Plan 3a gains Startup and Settings; neither page needs a per-tick frame.

**Tech Stack:** Rust stable (MSVC, edition 2024), `slint` 1.18.1, `i-slint-backend-testing` 1.18.1 (dev), `windows` 0.62.2 (new features listed below), `serde`/`serde_json` 1.0. No new crates.

**Spec:** `docs/superpowers/specs/2026-10-14-negotium-design.md` — §4.2 (tick choices 0.5 / 1 / 2 / 5 s; 2 s hidden), §5 navigation, §5.3 (Startup), §5.4 (Settings), §5.5 (tray), §6 (restart as admin preserving the selected page), §7 (inline errors). Builds on `docs/superpowers/plans/2026-10-14-negotium-performance.md` (Plan 3a: rail, `controls::Page`, page-aware worker) — execute Plan 3a first.

**Out of scope (Plan 4):** the no-prompt admin task (Settings "Admin mode"), the Ctrl+Shift+Esc redirect, the installer, the CPU-temperature add-on (PawnIO), per-app network (SRUM), app icons. Settings shows those three rows disabled with "Coming in a later version" (ruling P3b-4).

**Carried / folded follow-ups:** restart-as-admin blocking the UI thread during UAC and passing no page (Plan 2 Task 11 minor) and the missing `SEE_MASK_NOASYNC` (Plan 2 Task 6 minor) — Task 8; title-bar dark mode applied once and never following system changes, and Mica coupled to the dark-mode call's success (Plan 2 Task 12 minors) — Task 2.

## Rulings on spec ambiguities

- **P3b-1 Packaged startup tasks.** Windows keeps an MSIX `windows.startupTask`'s state in `HKCU\Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\SystemAppData\<PackageFamilyName>\<TaskId>`, DWORD `State` (0 disabled, 1 disabled by user, 2 enabled, 3 disabled by policy, 4 enabled by policy); there is no StartupApproved value for them. Negotium lists every task key that has a `State`, toggles by writing 1 (disable) or 2 (enable), and shows policy states (3, 4) as fixed. Display name and publisher come from `PackageManager` (current user, no admin); a family that isn't installed shows its family name. Needs a one-time human check that Settings → Apps → Startup agrees after a toggle (Task 9).
- **P3b-2 StartupApproved format.** Values live under `{HKCU|HKLM}\Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\{Run|Run32|StartupFolder}`, named after the Run value or the shortcut's file name, `REG_BINARY`, 12 bytes: a little-endian DWORD of flags whose low bit means *disabled* (Windows writes `02 00 00 00` enabled and `03 00 00 00` disabled; some installers write `06`/`07`), then a little-endian `FILETIME` (UTC) of when it was disabled, all zero while enabled. A missing value — or one shorter than 4 bytes — means enabled. Negotium reads the low bit, and on a toggle keeps the other flag bits (`06`↔`07`, default `02`), writes the current FILETIME when disabling and zeros when enabling. HKCU `Run` → HKCU `…\Run`; HKLM `Run` → HKLM `…\Run`; HKLM `WOW6432Node\…\Run` → HKLM `…\Run32`; per-user Startup folder → HKCU `…\StartupFolder`; all-users Startup folder → HKLM `…\StartupFolder`. Negotium never edits or deletes a Run value or a shortcut.
- **P3b-3 Theme.** Negotium sets the colour scheme itself (light or dark) instead of leaving Slint to follow the system, so the title bar and Mica can be re-applied in the same step. "System" reads `HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize\AppsUseLightTheme` and re-reads it on `WM_SETTINGCHANGE`; the accent colour is re-read on `WM_SETTINGCHANGE` / `WM_DWMCOLORIZATIONCOLORCHANGED`.
- **P3b-4 Plan 4 settings.** "Admin mode", "Open on Ctrl+Shift+Esc" and "CPU temperature add-on" appear as disabled rows reading "Coming in a later version" (three static rows; trivially cheap).
- **P3b-5 Refresh rate.** The chosen rate applies while the window is visible; hidden in the tray the tick is 2 s, or the chosen rate if slower (spec §4.2). The history keeps at most one sample per second whatever the rate (Plan 3a ruling P3-3).
- **P3b-6 Startup list freshness.** The list is read on an action thread each time the Startup page opens, on Refresh, and after every toggle (which then shows the real state, success or not).
- **P3b-7 What restart preserves.** "Restart as admin" preserves the page (`--page=overview|performance|startup|settings`); the Performance device, range and the search text start fresh.

## Global Constraints

- Windows 11 x64 only; Rust stable, `x86_64-pc-windows-msvc`, edition 2024.
- No new crates. `crates/app` gains `serde.workspace = true` (already a workspace dependency). `negotium-actions` gains `negotium-collector` as a normal dependency (for `version_string`) and `windows` features `Win32_System_Com`, `Win32_Storage_FileSystem`, `Management_Deployment`, `ApplicationModel`, `Foundation`, `Foundation_Collections`.
- `unsafe` only in `negotium-collector`, `negotium-actions`, `negotium-bench`, and `crates/app/src/platform.rs`; every `unsafe` block has its own `// SAFETY:` comment.
- **The UI thread never collects data or performs an action.** Startup listing and toggling run on action threads (results come back through `slint::invoke_from_event_loop` and `bridge::with_bridge`); the settings file is written on a background thread. Allowed on the UI thread: reading the settings file once at startup before the window shows, and `platform.rs` window setup — including re-applying the colour scheme, title bar and Mica when the theme changes.
- Only the visible page's view is built each tick (spec §8): Startup and Settings build nothing per tick.
- Spec §7: failures are inline (toast or banner), never dialogs. Writing an HKLM value without admin shows the inline "Restart as admin" path.
- Settings file: `%LOCALAPPDATA%\Negotium\settings.json` (falls back to the temp directory if `LOCALAPPDATA` is unset), pretty JSON, written atomically (temp file + rename). Unknown fields are ignored and missing fields take defaults; an unreadable or invalid file is renamed to `settings.json.bad` and logged.
- **Tests never touch the user's real startup entries or settings.** Registry tests work only under `HKCU\Software\Negotium-Test\…` (the delete helper refuses any other path); folder and settings tests use fresh directories under `std::env::temp_dir()`. Read-only listing of the real startup lists is allowed. Tests never terminate real processes other than throwaway children, and never change real system settings.
- Slint lessons from Plans 2–3a (keep): icons as drawn `Path`s; `max-width: 16384px` on layouts holding repeaters (`window_opens_at_its_preferred_width` catches regressions); `changed` handlers run later in dirty order (call a public Slint function from Rust when Rust needs the effect at once); a disabled `FocusScope` ignores FocusOut; `ContextMenuArea` stays `enabled: false`.
- UI tests need debug info; run them in debug, never with `--release`.
- If a `windows` 0.62.2 or Slint 1.18 item path or signature differs from this plan, look it up (`https://microsoft.github.io/windows-docs-rs/doc/windows/`, `https://docs.slint.dev/latest/docs/rust/slint/`, or the crate source under `~/.cargo/registry/src`) and adapt without changing behaviour; note it in the report.
- Before running the app manually, `Stop-Process -Name negotium -ErrorAction SilentlyContinue`; stop any instance you started when done.

## Review Focus

1. **A corrupt, half-written or foreign settings file** (truncated JSON, a field of the wrong type, fields from a newer version, refresh 3 ms) → Negotium starts with defaults or the valid fields, sets a broken file aside as `settings.json.bad`, and snaps odd values to the offered choices. Tests in Task 1.
2. **Unusual Run commands** (unquoted paths with spaces, `%SystemRoot%`, `rundll32.exe …`, a target that no longer exists, an empty value) → the entry is listed under its value name, without a publisher, and can still be enabled or disabled. Tests in Task 4.
3. **An all-users entry without admin** → no dialog: the switch is disabled with "Needs admin" and an inline "Restart as admin" banner; a toggle that fails at write time shows an inline message and the switch shows the real state again. Tests in Tasks 4 and 7.
4. **StartupApproved values written by others** (`06`/`07` flags, values shorter than 12 bytes, a name differing only in case) → read correctly; a toggle keeps the other flag bits and the existing value. Tests in Task 4.
5. **The system theme flips while Theme = System** → colours, title bar and Mica follow; with Light or Dark chosen they don't. Tests in Task 2.

---

## File Structure

```
crates/app/Cargo.toml                    MODIFY: serde
crates/app/src/settings.rs               CREATE: Settings, load/save, Saver, theme and close rules
crates/app/src/startup.rs                CREATE: Startup page view model
crates/app/src/controls.rs               MODIFY: refresh rate; Page::{Startup, Settings}; page names; restart args
crates/app/src/worker.rs                 MODIFY: Startup/Settings build no frame
crates/app/src/bridge.rs                 MODIFY: Startup models; ignore frames for static pages
crates/app/src/platform.rs               MODIFY: system theme, settings-change event, apply_theme, Mica decoupled
crates/app/src/main.rs                   MODIFY: settings at startup, page handlers, startup jobs, restart on a thread
crates/app/src/lib.rs                    MODIFY: modules
crates/app/ui/rail.slint                 MODIFY: Startup and Settings entries
crates/app/ui/settingspage.slint         CREATE: Settings page (Choice, Toggle, SettingRow)
crates/app/ui/startuppage.slint          CREATE: Startup page
crates/app/ui/main.slint                 MODIFY: pages, theme function, always on top, window-level toast
crates/app/tests/ui.rs                   MODIFY: new UI tests
crates/collector/src/identity.rs         MODIFY: public version_string (FileDescription, CompanyName)
crates/collector/src/lib.rs              MODIFY: re-export
crates/actions/Cargo.toml                MODIFY: collector dependency, windows features
crates/actions/src/lib.rs                MODIFY: pub mod startup
crates/actions/src/elevation.rs          MODIFY: SEE_MASK_NOASYNC
crates/actions/src/startup/mod.rs        CREATE: entries, roots, list and toggle
crates/actions/src/startup/approved.rs   CREATE: StartupApproved codec (pure)
crates/actions/src/startup/command.rs    CREATE: executable from a Run command (pure)
crates/actions/src/startup/registry.rs   CREATE: registry helpers
crates/actions/src/startup/folders.rs    CREATE: Startup folders, shell links, COM apartment
crates/actions/src/startup/packaged.rs   CREATE: packaged startup tasks
crates/actions/tests/startup.rs          CREATE: sandboxed integration tests
docs/benchmarks/README.md                MODIFY: manual checks
```

---

### Task 1: Settings model and settings file

**Files:**
- Create: `crates/app/src/settings.rs`
- Modify: `crates/app/Cargo.toml`, `crates/app/src/lib.rs`
- Test: unit tests in `settings.rs`

**Interfaces:**
- Consumes: `crate::log::Log`.
- Produces (`negotium::settings`):
  - `REFRESH_CHOICES_MS: [u32; 4] = [500, 1000, 2000, 5000]`; `nearest_refresh(u32) -> u32`; `refresh_from_index(i32) -> u32`.
  - `#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)] #[serde(rename_all = "lowercase")] enum ThemeChoice { #[default] System, Light, Dark }` with `ALL`, `index(self) -> i32`, `from_index(i32) -> ThemeChoice`.
  - `#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)] #[serde(default)] struct Settings { refresh_ms: u32, theme: ThemeChoice, always_on_top: bool, keep_in_tray: bool }` (defaults 1000, System, false, true) with `normalized(self) -> Settings`, `refresh_index(&self) -> i32`.
  - `effective_dark(theme: ThemeChoice, system_dark: bool) -> bool`; `enum CloseAction { HideToTray, Exit }`, `close_action(keep_in_tray: bool, exit_on_close: bool) -> CloseAction`.
  - `default_path() -> PathBuf`; `#[derive(Debug, PartialEq)] enum Loaded { Defaults, File, Recovered(String) }`; `load(&Path) -> (Settings, Loaded)`; `save(&Path, &Settings) -> std::io::Result<()>`; `#[derive(Clone)] struct Saver` with `new(PathBuf) -> Saver`, `save(&self, Settings) -> std::thread::JoinHandle<()>` (newest wins).

- [ ] **Step 1: Write the failing tests** — the tests module of the new `crates/app/src/settings.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("negotium-settings-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.join("settings.json")
    }

    #[test]
    fn a_missing_file_means_defaults_and_the_tray_is_on_by_default() {
        let (s, how) = load(&temp("missing"));
        assert_eq!(how, Loaded::Defaults);
        assert_eq!(s, Settings::default());
        assert!(s.keep_in_tray, "spec 5.4: keep running in tray defaults on");
        assert_eq!((s.refresh_ms, s.theme, s.always_on_top), (1000, ThemeChoice::System, false));
    }

    #[test]
    fn settings_round_trip_as_readable_json() {
        let p = temp("roundtrip");
        let s = Settings { refresh_ms: 2000, theme: ThemeChoice::Dark, always_on_top: true, keep_in_tray: false };
        save(&p, &s).unwrap();
        assert!(std::fs::read_to_string(&p).unwrap().contains("\"theme\": \"dark\""));
        assert_eq!(load(&p), (s, Loaded::File));
        assert!(!p.with_extension("json.tmp").exists(), "the temporary file is renamed into place");
    }

    #[test]
    fn a_corrupt_file_is_set_aside_and_defaults_are_used() {
        let p = temp("corrupt");
        std::fs::write(&p, "{\"refresh_ms\": 20").unwrap();
        let (s, how) = load(&p);
        assert_eq!(s, Settings::default());
        assert!(matches!(how, Loaded::Recovered(_)), "{how:?}");
        assert!(p.with_extension("json.bad").exists());
        assert!(!p.exists());
        assert_eq!(load(&p).1, Loaded::Defaults, "the next start is clean");
    }

    #[test]
    fn a_field_of_the_wrong_type_is_a_corrupt_file() {
        let p = temp("wrongtype");
        std::fs::write(&p, r#"{"refresh_ms": "fast"}"#).unwrap();
        assert!(matches!(load(&p).1, Loaded::Recovered(_)));
    }

    #[test]
    fn unknown_fields_are_ignored_and_missing_ones_default() {
        let p = temp("partial");
        std::fs::write(&p, r#"{"theme": "light", "admin_mode": "no-prompt"}"#).unwrap();
        let (s, how) = load(&p);
        assert_eq!(how, Loaded::File);
        assert_eq!(s, Settings { theme: ThemeChoice::Light, ..Settings::default() });
    }

    #[test]
    fn odd_refresh_values_snap_to_the_nearest_choice() {
        assert_eq!(nearest_refresh(3), 500);
        assert_eq!(nearest_refresh(700), 500);
        assert_eq!(nearest_refresh(1600), 2000);
        assert_eq!(nearest_refresh(60_000), 5000);
        let p = temp("snap");
        std::fs::write(&p, r#"{"refresh_ms": 3}"#).unwrap();
        let (s, _) = load(&p);
        assert_eq!(s.refresh_ms, 500);
        assert_eq!(s.refresh_index(), 0);
        assert_eq!(refresh_from_index(3), 5000);
        assert_eq!(refresh_from_index(-1), 500);
    }

    #[test]
    fn the_newest_save_wins_when_saves_overlap() {
        let p = temp("overlap");
        let saver = Saver::new(p.clone());
        let a = saver.save(Settings { refresh_ms: 500, ..Settings::default() });
        let b = saver.save(Settings { refresh_ms: 5000, ..Settings::default() });
        a.join().unwrap();
        b.join().unwrap();
        assert_eq!(load(&p).0.refresh_ms, 5000);
    }

    #[test]
    fn theme_and_close_rules() {
        assert!(effective_dark(ThemeChoice::System, true));
        assert!(!effective_dark(ThemeChoice::System, false));
        assert!(!effective_dark(ThemeChoice::Light, true));
        assert!(effective_dark(ThemeChoice::Dark, false));
        assert_eq!(ThemeChoice::from_index(2), ThemeChoice::Dark);
        assert_eq!(ThemeChoice::from_index(9), ThemeChoice::Dark);
        assert_eq!(close_action(true, false), CloseAction::HideToTray);
        assert_eq!(close_action(false, false), CloseAction::Exit);
        assert_eq!(close_action(true, true), CloseAction::Exit, "--exit-on-close (benchmark) always exits");
    }
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p negotium --lib settings`
Expected: FAIL to compile (no `settings` module).

- [ ] **Step 3: Implement**

`crates/app/Cargo.toml`: add `serde.workspace = true` under `[dependencies]`.

`crates/app/src/settings.rs` (above the tests):
```rust
//! User settings (spec §5.4) in %LOCALAPPDATA%\Negotium\settings.json. A missing file means
//! defaults. An unreadable or invalid file is moved aside to settings.json.bad and defaults are
//! used: a bad file never stops Negotium from starting. Unknown fields are ignored and missing
//! ones take their defaults, so files from older or newer versions load.
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub const REFRESH_CHOICES_MS: [u32; 4] = [500, 1000, 2000, 5000];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeChoice {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemeChoice {
    pub const ALL: [ThemeChoice; 3] = [ThemeChoice::System, ThemeChoice::Light, ThemeChoice::Dark];

    pub fn index(self) -> i32 {
        self as i32
    }

    pub fn from_index(i: i32) -> ThemeChoice {
        Self::ALL[(i.max(0) as usize).min(Self::ALL.len() - 1)]
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Refresh interval while the window is visible, one of REFRESH_CHOICES_MS.
    pub refresh_ms: u32,
    pub theme: ThemeChoice,
    pub always_on_top: bool,
    /// Closing the window hides it to the tray (spec §5.5) instead of exiting.
    pub keep_in_tray: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { refresh_ms: 1000, theme: ThemeChoice::System, always_on_top: false, keep_in_tray: true }
    }
}

impl Settings {
    pub fn normalized(mut self) -> Settings {
        self.refresh_ms = nearest_refresh(self.refresh_ms);
        self
    }

    pub fn refresh_index(&self) -> i32 {
        REFRESH_CHOICES_MS.iter().position(|&c| c == nearest_refresh(self.refresh_ms)).unwrap_or(1) as i32
    }
}

pub fn nearest_refresh(ms: u32) -> u32 {
    REFRESH_CHOICES_MS.into_iter().min_by_key(|c| c.abs_diff(ms)).unwrap_or(1000)
}

pub fn refresh_from_index(i: i32) -> u32 {
    REFRESH_CHOICES_MS[(i.max(0) as usize).min(REFRESH_CHOICES_MS.len() - 1)]
}

pub fn effective_dark(theme: ThemeChoice, system_dark: bool) -> bool {
    match theme {
        ThemeChoice::System => system_dark,
        ThemeChoice::Light => false,
        ThemeChoice::Dark => true,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseAction {
    HideToTray,
    Exit,
}

/// What closing the window does. `--exit-on-close` (benchmark use) always exits.
pub fn close_action(keep_in_tray: bool, exit_on_close: bool) -> CloseAction {
    if keep_in_tray && !exit_on_close { CloseAction::HideToTray } else { CloseAction::Exit }
}

pub fn default_path() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("Negotium").join("settings.json")
}

#[derive(Debug, PartialEq)]
pub enum Loaded {
    Defaults,
    File,
    /// The file was unusable (the reason) and was renamed to settings.json.bad.
    Recovered(String),
}

pub fn load(path: &Path) -> (Settings, Loaded) {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Settings::default(), Loaded::Defaults),
        Err(e) => return (Settings::default(), set_aside(path, &e.to_string())),
    };
    match serde_json::from_str::<Settings>(&text) {
        Ok(s) => (s.normalized(), Loaded::File),
        Err(e) => (Settings::default(), set_aside(path, &e.to_string())),
    }
}

fn set_aside(path: &Path, reason: &str) -> Loaded {
    let _ = std::fs::rename(path, path.with_extension("json.bad"));
    Loaded::Recovered(reason.to_string())
}

/// Writes atomically: a temporary file next to the target, then a rename over it.
pub fn save(path: &Path, s: &Settings) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(s).map_err(std::io::Error::other)?)?;
    std::fs::rename(&tmp, path)
}

/// Saves off the UI thread. Overlapping saves write in order and the newest settings win.
#[derive(Clone)]
pub struct Saver {
    path: PathBuf,
    pending: Arc<Mutex<Option<Settings>>>,
    write: Arc<Mutex<()>>,
}

impl Saver {
    pub fn new(path: PathBuf) -> Self {
        Self { path, pending: Default::default(), write: Default::default() }
    }

    pub fn save(&self, s: Settings) -> std::thread::JoinHandle<()> {
        *self.pending.lock().unwrap_or_else(|e| e.into_inner()) = Some(s);
        let (path, pending, write) = (self.path.clone(), self.pending.clone(), self.write.clone());
        std::thread::spawn(move || {
            let _writing = write.lock().unwrap_or_else(|e| e.into_inner());
            let next = pending.lock().unwrap_or_else(|e| e.into_inner()).take();
            if let Some(s) = next
                && let Err(e) = save(&path, &s)
            {
                crate::log::Log::new(crate::log::Log::default_path()).warn(&format!("settings not saved: {e}"));
            }
        })
    }
}
```
`crates/app/src/lib.rs`: add `pub mod settings;`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p negotium --lib settings`
Expected: PASS (8 tests), warning-free.

- [ ] **Step 5: Commit**

```bash
git add crates/app/Cargo.toml crates/app/src
git commit -m "feat(app): settings with a forgiving settings.json"
```

---

### Task 2: Apply settings — refresh rate, theme with title bar and Mica, always on top, tray

**Files:**
- Modify: `crates/app/src/controls.rs`, `crates/app/src/platform.rs`, `crates/app/src/main.rs`, `crates/app/ui/main.slint`
- Test: unit tests in `controls.rs`, `platform.rs`; `crates/app/tests/ui.rs`

**Interfaces:**
- Consumes: Task 1 `settings::{Settings, ThemeChoice, effective_dark, close_action, CloseAction, load, Saver, default_path}`.
- Produces:
  - `Controls::set_refresh_ms(u32)` (wakes the worker); `Controls::interval()` = the refresh while visible, `max(refresh, 2 s)` while hidden.
  - `platform::{system_prefers_dark() -> bool, apply_theme(ui: &MainWindow, theme: ThemeChoice), ShellEvent::SettingsChanged}`; `apply_window_chrome` sets `Theme.mica` from the backdrop call alone.
  - Slint `MainWindow`: `in property <bool> on-top` (drives `always-on-top`), `public function set-dark(bool)` (sets `Palette.color-scheme` at once, so `Theme.dark` is current when Rust reads it next).
  - `main.rs` loads settings before the window, applies them, and keeps them in an `Rc<RefCell<Settings>>` and the path in `settings_path` (Task 3 adds the `Saver` and the page).

- [ ] **Step 1: Write the failing tests**

`controls.rs` tests:
```rust
    #[test]
    fn the_refresh_rate_applies_while_visible_and_never_speeds_up_the_tray() {
        let c = Controls::default();
        c.set_refresh_ms(500);
        assert!(c.wait(Duration::from_millis(20)), "a new rate takes effect at once");
        c.set_visible(true);
        assert_eq!(c.interval(), Duration::from_millis(500));
        c.set_visible(false);
        assert_eq!(c.interval(), Duration::from_secs(2), "hidden: 2 s (spec 4.2)");
        c.set_refresh_ms(5000);
        assert_eq!(c.interval(), Duration::from_secs(5), "hidden never ticks faster than chosen");
        c.set_visible(true);
        assert_eq!(c.interval(), Duration::from_secs(5));
    }
```

`platform.rs` tests — extend `shell_messages_reach_the_main_window_hook` to also send `WM_SETTINGCHANGE` and `WM_DWMCOLORIZATIONCOLORCHANGED`:
```rust
    #[test]
    fn shell_messages_reach_the_main_window_hook() {
        use windows::Win32::UI::WindowsAndMessaging::WM_SETTINGCHANGE;
        const WM_DWMCOLORIZATIONCOLORCHANGED: u32 = 0x0320;
        let hwnd = hidden_window();
        let seen = Rc::new(RefCell::new(Vec::new()));
        let s = seen.clone();
        assert!(install_window_hooks(hwnd, move |e| s.borrow_mut().push(e)));
        // SAFETY: synchronous sends to our own window on this thread.
        unsafe {
            SendMessageW(hwnd, activate_message(), Some(WPARAM(0)), Some(LPARAM(0)));
            SendMessageW(hwnd, taskbar_created_message(), Some(WPARAM(0)), Some(LPARAM(0)));
            SendMessageW(hwnd, WM_SETTINGCHANGE, Some(WPARAM(0)), Some(LPARAM(0)));
            SendMessageW(hwnd, WM_DWMCOLORIZATIONCOLORCHANGED, Some(WPARAM(0)), Some(LPARAM(0)));
            let _ = DestroyWindow(hwnd);
        }
        assert_eq!(
            *seen.borrow(),
            [ShellEvent::Activate, ShellEvent::TaskbarCreated, ShellEvent::SettingsChanged, ShellEvent::SettingsChanged]
        );
    }

    #[test]
    fn the_system_theme_can_be_read() {
        // Either answer is valid; reading must not fail or panic.
        let _ = system_prefers_dark();
    }
```

`crates/app/tests/ui.rs`:
```rust
// ---- Plan 3b: settings ----

#[test]
fn set_dark_switches_the_theme_at_once() {
    let ui = window();
    ui.invoke_set_dark(true);
    assert!(ui.global::<negotium::Theme>().get_dark());
    ui.invoke_set_dark(false);
    assert!(!ui.global::<negotium::Theme>().get_dark());
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p negotium`
Expected: FAIL to compile (no `set_refresh_ms`, `ShellEvent::SettingsChanged`, `system_prefers_dark`, `invoke_set_dark`).

- [ ] **Step 3: Implement**

`crates/app/src/controls.rs`:
```rust
use std::sync::atomic::AtomicU32;
// In `Controls`:
    /// Refresh interval while visible (Settings), milliseconds.
    refresh_ms: AtomicU32,
// In `Default`:
            refresh_ms: AtomicU32::new(1000),
// Methods (replace `interval`):
    pub fn set_refresh_ms(&self, ms: u32) {
        self.refresh_ms.store(ms.max(100), Ordering::Relaxed);
        self.poke();
    }
    /// The chosen rate while visible; hidden in the tray, 2 s or the chosen rate if slower (spec 4.2).
    pub fn interval(&self) -> Duration {
        let refresh = self.refresh_ms.load(Ordering::Relaxed).max(100) as u64;
        Duration::from_millis(if self.visible() { refresh } else { refresh.max(2000) })
    }
```

`crates/app/src/platform.rs`:
- `ShellEvent` gains:
```rust
    /// The system theme or accent colour may have changed (WM_SETTINGCHANGE, WM_DWMCOLORIZATIONCOLORCHANGED).
    SettingsChanged,
```
- in `subclass_proc`, before the final `DefSubclassProc`:
```rust
    } else if msg == WM_SETTINGCHANGE || msg == WM_DWMCOLORIZATIONCOLORCHANGED {
        // SAFETY: `hook` lives until WM_NCDESTROY.
        unsafe { (*hook)(ShellEvent::SettingsChanged) };
    }
```
  with `const WM_DWMCOLORIZATIONCOLORCHANGED: u32 = 0x0320;` (dwmapi.h; not in `windows` 0.62.2's WindowsAndMessaging) and `WM_SETTINGCHANGE` imported from `windows::Win32::UI::WindowsAndMessaging`. Both messages are broadcast to top-level windows; no message filter change is needed.
- add:
```rust
/// Whether Windows apps use the dark theme (Settings → Personalization → Colors).
pub fn system_prefers_dark() -> bool {
    let mut value = 1u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: `value`/`size` are valid for a REG_DWORD read.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut c_void),
            Some(&mut size),
        )
    };
    status == ERROR_SUCCESS && value == 0
}

/// Applies a theme choice: the colour scheme (immediately, through `set-dark`), then the title bar
/// and Mica to match (a no-op until the native window exists; the chrome timer applies it then).
pub fn apply_theme(ui: &MainWindow, theme: crate::settings::ThemeChoice) {
    ui.invoke_set_dark(crate::settings::effective_dark(theme, system_prefers_dark()));
    apply_window_chrome(ui);
}
```
- in `apply_window_chrome`, replace `ui.global::<Theme>().set_mica(applied);` with `ui.global::<Theme>().set_mica(mica_ok);` (Mica is shown whenever the backdrop applied, even if the dark-mode attribute failed; the return value stays `dark_ok && mica_ok`).

`crates/app/ui/main.slint`:
- `import { Palette } from "std-widgets.slint";`
- properties/functions in `MainWindow`:
```slint
    // Settings → Always on top.
    in property <bool> on-top;
    always-on-top: root.on-top;
    // Settings → Theme: Rust passes the effective scheme (crate::settings::effective_dark). A
    // function, not a `changed` handler, so Theme.dark is current as soon as it returns.
    public function set-dark(dark: bool) {
        Palette.color-scheme = dark ? ColorScheme.dark : ColorScheme.light;
    }
```

`crates/app/src/main.rs`:
- right after `MainWindow::new()?` and the accent read:
```rust
    let settings_path = negotium::settings::default_path();
    let (loaded, how) = negotium::settings::load(&settings_path);
    if let negotium::settings::Loaded::Recovered(reason) = &how {
        warn(&format!("settings.json was unusable and was set aside as settings.json.bad: {reason}"));
    }
    let settings = Rc::new(RefCell::new(loaded));
    platform::apply_theme(&ui, settings.borrow().theme);
    ui.set_on_top(settings.borrow().always_on_top);
```
- after `let controls = Arc::new(Controls::default());`: `controls.set_refresh_ms(settings.borrow().refresh_ms);`
- the close handler becomes:
```rust
    let exit_on_close = std::env::args().any(|a| a == "--exit-on-close");
    {
        let (controls, b, close_weak, settings) = (controls.clone(), bridge.clone(), weak.clone(), settings.clone());
        ui.window().on_close_requested(move || {
            use negotium::settings::{CloseAction, close_action};
            match close_action(settings.borrow().keep_in_tray, exit_on_close) {
                CloseAction::Exit => begin_exit(&controls, &close_weak),
                CloseAction::HideToTray => {
                    controls.set_visible(false);
                    // A hidden window cannot report pointer-leave, so release the hover freeze here.
                    b.borrow_mut().set_hover(false);
                }
            }
            slint::CloseRequestResponse::HideWindow
        });
    }
```
- in the window-hook event handler (`match event`), add (clone `settings` into the closures like `tray`):
```rust
                        ShellEvent::SettingsChanged => {
                            if let Some(ui) = weak.upgrade() {
                                if let Some((r, g, b)) = platform::read_accent() {
                                    ui.global::<negotium::Theme>().set_accent(slint::Color::from_rgb_u8(r, g, b));
                                }
                                if settings.borrow().theme == negotium::settings::ThemeChoice::System {
                                    platform::apply_theme(&ui, negotium::settings::ThemeChoice::System);
                                }
                            }
                        }
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p negotium`
Expected: all pass, warning-free.

- [ ] **Step 5: Look at it**

Run the app with Windows in dark mode, then switch Windows to light (Settings → Personalization → Colors → Choose your mode): Negotium's content, title bar and Mica follow within a second. Change the accent colour: meters and selection follow. Stop the instance.

- [ ] **Step 6: Commit**

```bash
git add crates/app
git commit -m "feat(app): apply refresh rate, theme (with title bar and Mica), always on top and tray settings"
```

---
### Task 3: Settings page

**Files:**
- Create: `crates/app/ui/settingspage.slint`
- Modify: `crates/app/ui/rail.slint`, `crates/app/ui/main.slint`, `crates/app/src/{controls,worker,bridge,main}.rs`
- Test: unit test in `controls.rs`; `crates/app/tests/ui.rs`

**Interfaces:**
- Consumes: Task 1 `settings::{Saver, ThemeChoice, refresh_from_index}`, Task 2 `platform::apply_theme`, `Controls::set_refresh_ms`, `MainWindow::on-top`; Plan 3a `controls::Page`, `Rail`, page host in `main.slint`.
- Produces:
  - `controls::Page` gains `Startup = 2` and `Settings = 3`; `Page::ALL` has all four; `from_index` maps 0–3 and anything else to `Overview`. The worker builds no frame for Startup or Settings; `Bridge::apply` ignores frames for them.
  - Slint components (exported from `settingspage.slint`): `Toggle` (`label: string`, `checked: bool` — set by the owner, never by the toggle itself — `interactive: bool`, callback `toggled(bool)` carrying the requested state; role switch), `Choice` (`options: [string]`, `selected: int`, `interactive`, callback `chosen(int)`; buttons have role radio-button and the option text as label), `SettingRow` (`title`, `description`, children on the right), `SettingsPage`.
  - `MainWindow`: `settings-refresh: int` (in-out; index into 0.5 / 1 / 2 / 5 s), `settings-theme: int` (in-out; System / Light / Dark), `settings-always-on-top: bool`, `settings-keep-in-tray: bool` (in-out); callbacks `refresh-chosen(int)`, `theme-chosen(int)`, `always-on-top-toggled(bool)`, `tray-toggled(bool)`. Rail entry "Settings page" selects page 3.

- [ ] **Step 1: Write the failing tests**

`controls.rs`: in `page_and_performance_selection_are_shared_and_wake_the_worker` replace `assert_eq!(Page::from_index(9), Page::Performance);` with:
```rust
        assert_eq!(Page::ALL.map(Page::index), [0, 1, 2, 3]);
        assert_eq!(Page::from_index(3), Page::Settings);
        assert_eq!(Page::from_index(9), Page::Overview, "unknown pages fall back to the Overview");
```

`crates/app/tests/ui.rs` (add `use i_slint_backend_testing::AccessibleRole;`):
```rust
/// The switch labelled `label` (a row title Text may carry the same words).
fn switch(ui: &MainWindow, label: &str) -> El {
    El::find_by_accessible_label(ui, label)
        .find(|e| e.accessible_role() == Some(AccessibleRole::Switch))
        .unwrap_or_else(|| panic!("no switch {label:?}"))
}

#[test]
fn the_settings_page_reports_each_choice() {
    let ui = shown_window();
    ui.set_page(3);
    ui.set_settings_keep_in_tray(true);
    let log = Rc::new(RefCell::new(Vec::<String>::new()));
    let l = log.clone();
    ui.on_refresh_chosen(move |i| l.borrow_mut().push(format!("refresh {i}")));
    let l = log.clone();
    ui.on_theme_chosen(move |i| l.borrow_mut().push(format!("theme {i}")));
    let l = log.clone();
    ui.on_always_on_top_toggled(move |on| l.borrow_mut().push(format!("top {on}")));
    let l = log.clone();
    ui.on_tray_toggled(move |on| l.borrow_mut().push(format!("tray {on}")));
    El::find_by_accessible_label(&ui, "2 s").next().expect("refresh choice").invoke_accessible_default_action();
    El::find_by_accessible_label(&ui, "Dark").next().expect("theme choice").invoke_accessible_default_action();
    switch(&ui, "Always on top").invoke_accessible_default_action();
    switch(&ui, "Keep running in the tray when closed").invoke_accessible_default_action();
    assert_eq!(*log.borrow(), ["refresh 2", "theme 2", "top true", "tray false"]);
}

#[test]
fn choices_and_switches_show_the_current_settings() {
    let ui = shown_window();
    ui.set_page(3);
    ui.set_settings_theme(1);
    ui.set_settings_always_on_top(true);
    let light = El::find_by_accessible_label(&ui, "Light").next().unwrap();
    assert_eq!(light.accessible_role(), Some(AccessibleRole::RadioButton));
    assert_eq!(light.accessible_checked(), Some(true));
    assert_eq!(El::find_by_accessible_label(&ui, "System").next().unwrap().accessible_checked(), Some(false));
    assert_eq!(switch(&ui, "Always on top").accessible_checked(), Some(true));
}

#[test]
fn later_version_settings_are_shown_disabled() {
    let ui = shown_window();
    ui.set_page(3);
    for label in ["Admin mode", "Open on Ctrl+Shift+Esc", "CPU temperature add-on"] {
        assert_eq!(switch(&ui, label).accessible_enabled(), Some(false), "{label}");
    }
}

#[test]
fn the_rail_reaches_settings() {
    let ui = window();
    let picked = Rc::new(RefCell::new(None));
    let p = picked.clone();
    ui.on_select_page(move |i| *p.borrow_mut() = Some(i));
    El::find_by_accessible_label(&ui, "Settings page").next().expect("rail item").invoke_accessible_default_action();
    assert_eq!(*picked.borrow(), Some(3));
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p negotium`
Expected: FAIL to compile (`Page::Settings`, `set_settings_*`, `on_refresh_chosen`, …).

- [ ] **Step 3: Implement**

`crates/app/src/controls.rs` — `Page`:
```rust
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Page {
    #[default]
    Overview = 0,
    Performance = 1,
    Startup = 2,
    Settings = 3,
}

impl Page {
    pub const ALL: [Page; 4] = [Page::Overview, Page::Performance, Page::Startup, Page::Settings];

    pub fn from_index(i: i32) -> Page {
        match i {
            1 => Page::Performance,
            2 => Page::Startup,
            3 => Page::Settings,
            _ => Page::Overview,
        }
    }

    pub fn index(self) -> i32 {
        self as i32
    }
}
```

`crates/app/src/worker.rs` — in `next_frame`'s `match controls.page()` add `Page::Startup | Page::Settings => None,` (nothing on those pages changes per tick).

`crates/app/src/bridge.rs` — in `apply`'s match add `Page::Startup | Page::Settings => {}`.

`crates/app/ui/settingspage.slint`:
```slint
import { ScrollView } from "std-widgets.slint";
import { Theme } from "theme.slint";

// An on/off switch. `checked` is owned by the caller: the switch only asks (`toggled` with the
// requested state), so a refused change never leaves it showing the wrong state.
export component Toggle inherits Rectangle {
    in property <string> label;
    in property <bool> checked;
    in property <bool> interactive: true;
    callback toggled(bool);
    width: 40px;
    height: 20px;
    border-radius: 10px;
    background: root.checked && root.interactive ? Theme.accent : transparent;
    border-width: fs.has-focus ? 2px : (root.checked && root.interactive ? 0px : 1px);
    border-color: fs.has-focus ? Theme.text : (root.interactive ? Theme.muted : Theme.card-border);
    accessible-role: switch;
    accessible-label: root.label;
    accessible-checkable: true;
    accessible-checked: root.checked;
    accessible-enabled: root.interactive;
    accessible-action-default => { root.flip(); }
    function flip() {
        if (root.interactive) {
            root.toggled(!root.checked);
        }
    }
    fs := FocusScope {
        enabled: root.interactive;
        key-pressed(event) => {
            if (event.text == " " || event.text == Key.Return) {
                root.flip();
                return accept;
            }
            return reject;
        }
    }
    TouchArea {
        enabled: root.interactive;
        clicked => { fs.focus(); root.flip(); }
    }
    Rectangle {
        x: root.checked ? root.width - 16px : 4px;
        y: 4px;
        width: 12px;
        height: 12px;
        border-radius: 6px;
        background: !root.interactive ? Theme.card-border : (root.checked ? #ffffff : Theme.muted);
    }
}

component ChoiceButton inherits Rectangle {
    in property <string> label;
    in property <bool> selected;
    in property <bool> interactive: true;
    callback activated();
    height: 30px;
    min-width: 56px;
    border-radius: 4px;
    background: root.selected ? Theme.accent : (ta.has-hover ? Theme.card : transparent);
    border-width: fs.has-focus ? 2px : 1px;
    border-color: fs.has-focus ? Theme.text : Theme.card-border;
    accessible-role: radio-button;
    accessible-label: root.label;
    accessible-checkable: true;
    accessible-checked: root.selected;
    accessible-enabled: root.interactive;
    accessible-action-default => { if (root.interactive) { root.activated(); } }
    fs := FocusScope {
        enabled: root.interactive;
        key-pressed(event) => {
            if (event.text == " " || event.text == Key.Return) {
                root.activated();
                return accept;
            }
            return reject;
        }
    }
    ta := TouchArea {
        enabled: root.interactive;
        clicked => { fs.focus(); root.activated(); }
    }
    HorizontalLayout {
        padding-left: 12px;
        padding-right: 12px;
        Text {
            text: root.label;
            color: root.selected ? #ffffff : Theme.text;
            font-size: 12px;
            vertical-alignment: center;
            horizontal-alignment: center;
        }
    }
}

// One choice out of a few, as a row of buttons.
export component Choice inherits HorizontalLayout {
    in property <[string]> options;
    in property <int> selected;
    in property <bool> interactive: true;
    callback chosen(int);
    spacing: 4px;
    alignment: start;
    max-width: 16384px; // see MetersRow: repeaters must not cap the window width
    for o[i] in root.options : ChoiceButton {
        label: o;
        selected: i == root.selected;
        interactive: root.interactive;
        activated => { root.chosen(i); }
    }
}

export component SettingRow inherits Rectangle {
    in property <string> title;
    in property <string> description;
    min-height: 64px;
    border-radius: Theme.radius;
    border-width: 1px;
    border-color: Theme.card-border;
    background: Theme.card;
    HorizontalLayout {
        padding: 12px;
        spacing: 16px;
        VerticalLayout {
            horizontal-stretch: 1;
            alignment: center;
            spacing: 2px;
            Text { text: root.title; color: Theme.text; font-size: 13px; font-weight: 600; }
            if root.description != "" : Text { text: root.description; color: Theme.muted; font-size: 11px; wrap: word-wrap; }
        }
        VerticalLayout {
            alignment: center;
            @children
        }
    }
}

export component SettingsPage inherits VerticalLayout {
    in property <int> refresh;
    in property <int> theme;
    in property <bool> always-on-top;
    in property <bool> keep-in-tray;
    in property <bool> interactive: true;
    callback refresh-chosen(int);
    callback theme-chosen(int);
    callback always-on-top-toggled(bool);
    callback tray-toggled(bool);
    max-width: 16384px;
    sv := ScrollView {
        content-width: self.visible-width;
        content-height: body.preferred-height;
        body := VerticalLayout {
            width: sv.visible-width;
            padding: 24px;
            spacing: 10px;
            alignment: start;
            Text { text: "Settings"; color: Theme.text; font-size: 24px; font-weight: 600; }
            SettingRow {
                title: "Refresh rate";
                description: "How often numbers update while the window is open. In the tray Negotium updates every 2 s, or slower if you choose a slower rate.";
                Choice {
                    options: ["0.5 s", "1 s", "2 s", "5 s"];
                    selected: root.refresh;
                    interactive: root.interactive;
                    chosen(i) => { root.refresh-chosen(i); }
                }
            }
            SettingRow {
                title: "Theme";
                description: "System follows Windows' light or dark mode.";
                Choice {
                    options: ["System", "Light", "Dark"];
                    selected: root.theme;
                    interactive: root.interactive;
                    chosen(i) => { root.theme-chosen(i); }
                }
            }
            SettingRow {
                title: "Always on top";
                description: "Keep Negotium above other windows.";
                Toggle {
                    label: "Always on top";
                    checked: root.always-on-top;
                    interactive: root.interactive;
                    toggled(on) => { root.always-on-top-toggled(on); }
                }
            }
            SettingRow {
                title: "Keep running in the tray when closed";
                description: "Closing the window keeps Negotium in the notification area: history keeps recording and reopening is instant.";
                Toggle {
                    label: "Keep running in the tray when closed";
                    checked: root.keep-in-tray;
                    interactive: root.interactive;
                    toggled(on) => { root.tray-toggled(on); }
                }
            }
            Text { text: "Coming in a later version"; color: Theme.muted; font-size: 12px; }
            SettingRow {
                title: "Admin mode";
                description: "Start with admin rights without a prompt.";
                Toggle { label: "Admin mode"; checked: false; interactive: false; }
            }
            SettingRow {
                title: "Open on Ctrl+Shift+Esc";
                description: "Open Negotium instead of Task Manager.";
                Toggle { label: "Open on Ctrl+Shift+Esc"; checked: false; interactive: false; }
            }
            SettingRow {
                title: "CPU temperature add-on";
                description: "Show the CPU temperature (installs the PawnIO driver from its publisher).";
                Toggle { label: "CPU temperature add-on"; checked: false; interactive: false; }
            }
        }
    }
}
```

`crates/app/ui/rail.slint` — after the Performance entry:
```slint
    RailButton {
        label: "Settings";
        icon: "M 3 5 L 17 5 M 3 10 L 17 10 M 3 15 L 17 15 M 7 3 L 7 7 M 13 8 L 13 12 M 9 13 L 9 17";
        selected: root.page == 3;
        interactive: root.interactive;
        activated => { root.select(3); }
    }
```

`crates/app/ui/main.slint`:
- `import { SettingsPage } from "settingspage.slint";`
- page comment: `// Page shown: 0 Overview, 1 Performance, 2 Startup, 3 Settings (crate::controls::Page).`
- properties and callbacks:
```slint
    // Settings page state (crate::settings::Settings): refresh index, theme index, switches.
    in-out property <int> settings-refresh: 1;
    in-out property <int> settings-theme: 0;
    in-out property <bool> settings-always-on-top;
    in-out property <bool> settings-keep-in-tray: true;
    callback refresh-chosen(int);
    callback theme-chosen(int);
    callback always-on-top-toggled(bool);
    callback tray-toggled(bool);
```
- in the page host, after the Performance page's `Rectangle`:
```slint
            Rectangle {
                x: 0;
                y: 0;
                width: parent.width;
                height: parent.height;
                visible: root.page == 3;
                SettingsPage {
                    refresh: root.settings-refresh;
                    theme: root.settings-theme;
                    always-on-top: root.settings-always-on-top;
                    keep-in-tray: root.settings-keep-in-tray;
                    interactive: !root.modal-open;
                    refresh-chosen(i) => { root.refresh-chosen(i); }
                    theme-chosen(i) => { root.theme-chosen(i); }
                    always-on-top-toggled(on) => { root.always-on-top-toggled(on); }
                    tray-toggled(on) => { root.tray-toggled(on); }
                }
            }
```

`crates/app/src/main.rs` — after the settings are applied (Task 2 block):
```rust
    let saver = negotium::settings::Saver::new(settings_path.clone());
    {
        let s = settings.borrow();
        ui.set_settings_refresh(s.refresh_index());
        ui.set_settings_theme(s.theme.index());
        ui.set_settings_always_on_top(s.always_on_top);
        ui.set_settings_keep_in_tray(s.keep_in_tray);
    }
```
and, once `controls` exists:
```rust
    // Settings: apply at once, show the new state, save off the UI thread.
    {
        let (settings, saver, controls, weak) = (settings.clone(), saver.clone(), controls.clone(), weak.clone());
        ui.on_refresh_chosen(move |i| {
            let ms = negotium::settings::refresh_from_index(i);
            settings.borrow_mut().refresh_ms = ms;
            controls.set_refresh_ms(ms);
            if let Some(ui) = weak.upgrade() {
                ui.set_settings_refresh(settings.borrow().refresh_index());
            }
            let _ = saver.save(settings.borrow().clone());
        });
    }
    {
        let (settings, saver, weak) = (settings.clone(), saver.clone(), weak.clone());
        ui.on_theme_chosen(move |i| {
            let theme = negotium::settings::ThemeChoice::from_index(i);
            settings.borrow_mut().theme = theme;
            if let Some(ui) = weak.upgrade() {
                platform::apply_theme(&ui, theme);
                ui.set_settings_theme(theme.index());
            }
            let _ = saver.save(settings.borrow().clone());
        });
    }
    {
        let (settings, saver, weak) = (settings.clone(), saver.clone(), weak.clone());
        ui.on_always_on_top_toggled(move |on| {
            settings.borrow_mut().always_on_top = on;
            if let Some(ui) = weak.upgrade() {
                ui.set_on_top(on);
                ui.set_settings_always_on_top(on);
            }
            let _ = saver.save(settings.borrow().clone());
        });
    }
    {
        let (settings, saver, weak) = (settings.clone(), saver.clone(), weak.clone());
        ui.on_tray_toggled(move |on| {
            settings.borrow_mut().keep_in_tray = on;
            if let Some(ui) = weak.upgrade() {
                ui.set_settings_keep_in_tray(on);
            }
            let _ = saver.save(settings.borrow().clone());
        });
    }
```
(`saver.save` returns the writer thread's handle; dropping it lets that thread finish on its own.)

- [ ] **Step 4: Run tests**

Run: `cargo test -p negotium`
Expected: all pass (including `window_opens_at_its_preferred_width`), warning-free.

- [ ] **Step 5: Look at it**

Run the app → Settings. Pick 0.5 s: the Overview numbers update twice a second. Pick Dark while Windows is light: content, title bar and Mica go dark; pick System: back to light. Always on top: the window stays above a focused Explorer window. Turn off "Keep running in the tray" and close the window: the process exits (check Task Manager). Start again: the settings are as you left them (`%LOCALAPPDATA%\Negotium\settings.json`). Restore the tray setting and stop the instance.

- [ ] **Step 6: Commit**

```bash
git add crates/app
git commit -m "feat(app): Settings page with refresh rate, theme, always on top and tray"
```

---
### Task 4: Startup — Run keys and the StartupApproved format

**Files:**
- Create: `crates/actions/src/startup/{mod,approved,command,registry}.rs`, `crates/actions/tests/startup.rs`
- Modify: `crates/actions/Cargo.toml`, `crates/actions/src/lib.rs`, `crates/collector/src/identity.rs`, `crates/collector/src/lib.rs`, `crates/collector/tests/identity.rs`
- Test: unit tests in `approved.rs`, `command.rs`; `crates/actions/tests/startup.rs`; `crates/collector/tests/identity.rs`

**Interfaces:**
- Consumes: `negotium_actions::ActionError`; collector's version-resource reader.
- Produces:
  - `negotium_collector::version_string(path: &str, key: &str) -> Option<String>` (first translation's `StringFileInfo` value, trimmed; `FileDescription`, `CompanyName`, …).
  - `negotium_actions::startup`:
    - `approved::{APPROVED_LEN = 12, is_enabled(&[u8]) -> bool, encode_approved(previous: Option<&[u8]>, enabled: bool, now_filetime: u64) -> [u8; 12], filetime_now() -> u64}` (re-exported from `startup`).
    - `command::{exe_from_command(command: &str, system_root: &str) -> Option<String>, expand_env(s: &str, lookup: impl Fn(&str) -> Option<String>) -> String}` (re-exported).
    - `pub mod registry` with `TEST_ROOT`, `Hive { CurrentUser, LocalMachine }` (`name()`), `RegKey { hive, path }` (`new`, `child`, `display`), `RegValue { Str, Binary, Dword, Other }` (`as_str`, `binary`, `dword`), `values(&RegKey) -> Result<Vec<(String, RegValue)>, ActionError>` (missing key = empty), `value(&RegKey, &str) -> Option<RegValue>`, `subkeys(&RegKey) -> Result<Vec<String>, ActionError>`, `set_binary`, `set_dword`, `set_string` (`-> Result<(), ActionError>`, creating the key), `delete_test_tree(&RegKey) -> Result<(), ActionError>` (refuses anything but a subkey of `HKCU\Software\Negotium-Test`).
    - `#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)] enum StartupSource { UserRun, MachineRun, MachineRun32 }` (Tasks 5–6 add `UserFolder`, `MachineFolder`, `Packaged`) with `needs_admin(self) -> bool`, `label(self) -> &'static str`.
    - `#[derive(Clone, Debug, PartialEq)] struct StartupEntry { source: StartupSource, key: String, name: String, publisher: Option<String>, enabled: bool, changeable: bool, command: String, location: String }`.
    - `#[derive(Clone, Debug)] struct StartupRoots { user_run, machine_run, machine_run32, user_approved, machine_approved: RegKey, system_root: String }` with `system() -> StartupRoots` and `under(base: &str) -> StartupRoots` (every list below `HKCU\<base>`: for tests).
    - `list_startup(&StartupRoots) -> (Vec<StartupEntry>, Vec<String>)` (entries, readable errors); `set_startup_enabled(&StartupRoots, &StartupEntry, enabled: bool) -> Result<(), ActionError>`.

- [ ] **Step 1: Write the failing tests**

`crates/collector/tests/identity.rs` — add:
```rust
#[test]
fn version_strings_read_company_name() {
    let company = negotium_collector::version_string(r"C:\Windows\System32\PING.EXE", "CompanyName");
    assert!(company.is_some_and(|c| !c.is_empty()));
    assert_eq!(negotium_collector::version_string(r"C:\Nope\missing.exe", "CompanyName"), None);
}
```

`crates/actions/src/startup/approved.rs` — tests (implementation in Step 3):
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_low_flag_bit_means_disabled_and_a_missing_or_short_value_means_enabled() {
        assert!(is_enabled(&[2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]));
        assert!(!is_enabled(&[3, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8]));
        assert!(is_enabled(&[6, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]));
        assert!(!is_enabled(&[7, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]));
        assert!(is_enabled(&[]));
        assert!(is_enabled(&[3, 0]), "too short to carry flags");
    }

    #[test]
    fn disabling_writes_the_flag_and_the_time_enabling_clears_both() {
        assert_eq!(
            encode_approved(None, false, 0x01D9_ABCD_1234_5678),
            [3, 0, 0, 0, 0x78, 0x56, 0x34, 0x12, 0xCD, 0xAB, 0xD9, 0x01]
        );
        assert_eq!(encode_approved(Some(&[3, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8]), true, 99), [2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn other_flag_bits_are_kept() {
        assert_eq!(encode_approved(Some(&[7, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1]), true, 5)[..4], [6, 0, 0, 0]);
        assert_eq!(encode_approved(Some(&[6, 0, 0, 0]), false, 5), [7, 0, 0, 0, 5, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(encode_approved(Some(&[1, 0, 0, 0]), true, 5)[..4], [2, 0, 0, 0], "no other bits: the default");
    }

    #[test]
    fn now_is_a_plausible_filetime() {
        assert!(filetime_now() > 0x01D0_0000_0000_0000, "after 2014");
    }
}
```

`crates/actions/src/startup/command.rs` — tests:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoted_unquoted_and_bare_commands() {
        assert_eq!(
            exe_from_command(r#""C:\Program Files\App\app.exe" --background"#, r"C:\Windows").as_deref(),
            Some(r"C:\Program Files\App\app.exe")
        );
        assert_eq!(
            exe_from_command(r"C:\Program Files\App\App.EXE /min", r"C:\Windows").as_deref(),
            Some(r"C:\Program Files\App\App.EXE"),
            "unquoted paths with spaces end at .exe"
        );
        assert_eq!(
            exe_from_command("rundll32.exe shell32.dll,Control_RunDLL", r"C:\Windows\").as_deref(),
            Some(r"C:\Windows\System32\rundll32.exe")
        );
        assert_eq!(exe_from_command(r#""C:\unterminated"#, r"C:\Windows").as_deref(), Some(r"C:\unterminated"));
        assert_eq!(exe_from_command("   ", r"C:\Windows"), None);
    }

    #[test]
    fn environment_variables_expand_and_unknown_ones_stay() {
        let lookup = |name: &str| (name.eq_ignore_ascii_case("windir")).then(|| r"C:\Windows".to_string());
        assert_eq!(expand_env(r"%windir%\system32\x.exe", lookup), r"C:\Windows\system32\x.exe");
        assert_eq!(expand_env(r"%NOPE%\x.exe", lookup), r"%NOPE%\x.exe");
        assert_eq!(expand_env("100%", lookup), "100%");
        assert_eq!(expand_env("%%", lookup), "%%");
        let real = exe_from_command(r"%SystemRoot%\system32\SecurityHealthSystray.exe", r"C:\Windows").unwrap();
        assert!(!real.contains('%') && real.ends_with(r"\system32\SecurityHealthSystray.exe"), "{real}");
    }
}
```

`crates/actions/tests/startup.rs`:
```rust
//! Startup listing and toggling, sandboxed: every key lives under HKCU\Software\Negotium-Test\…
//! and is removed afterwards. The user's real startup entries are only ever read.
use negotium_actions::startup::registry::{self, Hive, RegKey};
use negotium_actions::startup::{StartupEntry, StartupRoots, StartupSource, list_startup, set_startup_enabled};

const PING: &str = r"C:\Windows\System32\PING.EXE";

struct Sandbox {
    base: RegKey,
    roots: StartupRoots,
}

impl Sandbox {
    fn new(name: &str) -> Self {
        let base = RegKey::new(Hive::CurrentUser, &format!(r"{}\{name}-{}", registry::TEST_ROOT, std::process::id()));
        let _ = registry::delete_test_tree(&base);
        Self { roots: StartupRoots::under(&base.path), base }
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = registry::delete_test_tree(&self.base);
    }
}

fn find<'a>(entries: &'a [StartupEntry], key: &str) -> &'a StartupEntry {
    entries.iter().find(|e| e.key == key).unwrap_or_else(|| panic!("no {key:?} in {entries:?}"))
}

fn approved(key: &RegKey, name: &str) -> Vec<u8> {
    registry::value(key, name).and_then(|v| v.binary().map(<[u8]>::to_vec)).unwrap_or_else(|| panic!("no value {name:?}"))
}

#[test]
fn run_entries_are_listed_with_their_state_and_toggle_in_task_managers_format() {
    let sb = Sandbox::new("run");
    let r = &sb.roots;
    registry::set_string(&r.user_run, "Ping test", &format!("\"{PING}\" -n 1 127.0.0.1")).unwrap();
    registry::set_string(&r.user_run, "Missing app", r"C:\Nope\Missing App\missing.exe /x").unwrap();
    registry::set_string(&r.machine_run32, "Old app", r"C:\Nope\old.exe").unwrap();
    // A disabled state written by someone else, under a name that differs only in case.
    registry::set_binary(&r.user_approved.child("Run"), "missing APP", &[3, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8]).unwrap();

    let (entries, errors) = list_startup(r);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(entries.len(), 3);
    let ping = find(&entries, "Ping test").clone();
    assert!(ping.enabled && ping.changeable);
    assert_eq!(ping.source, StartupSource::UserRun);
    assert!(ping.publisher.is_some(), "CompanyName of PING.EXE");
    assert_ne!(ping.name, "Ping test", "named after the program's description");
    assert!(ping.location.ends_with(r"\UserRun"), "{}", ping.location);
    let missing = find(&entries, "Missing app");
    assert!(!missing.enabled, "StartupApproved names match case-insensitively");
    assert_eq!((missing.name.as_str(), missing.publisher.as_deref()), ("Missing app", None));
    let old = find(&entries, "Old app").clone();
    assert_eq!(old.source, StartupSource::MachineRun32);
    assert!(old.source.needs_admin());

    set_startup_enabled(r, &ping, false).unwrap();
    let bytes = approved(&r.user_approved.child("Run"), "Ping test");
    assert_eq!(bytes.len(), 12);
    assert_eq!(bytes[..4], [3, 0, 0, 0]);
    assert!(bytes[4..].iter().any(|&b| b != 0), "disabling records when");
    assert!(!find(&list_startup(r).0, "Ping test").enabled);
    set_startup_enabled(r, &ping, true).unwrap();
    assert_eq!(approved(&r.user_approved.child("Run"), "Ping test"), [2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    assert!(find(&list_startup(r).0, "Ping test").enabled);
    set_startup_enabled(r, &old, false).unwrap();
    assert_eq!(approved(&r.machine_approved.child("Run32"), "Old app")[0], 3, "32-bit HKLM entries use Run32");
}

#[test]
fn approved_values_with_other_flag_bits_keep_them() {
    let sb = Sandbox::new("flags");
    let r = &sb.roots;
    registry::set_string(&r.user_run, "Tool", r"C:\Nope\tool.exe").unwrap();
    registry::set_binary(&r.user_approved.child("Run"), "Tool", &[6, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]).unwrap();
    let tool = find(&list_startup(r).0, "Tool").clone();
    assert!(tool.enabled);
    set_startup_enabled(r, &tool, false).unwrap();
    assert_eq!(approved(&r.user_approved.child("Run"), "Tool")[0], 7);
    set_startup_enabled(r, &tool, true).unwrap();
    assert_eq!(approved(&r.user_approved.child("Run"), "Tool")[..4], [6, 0, 0, 0]);
}

#[test]
fn an_empty_command_is_listed_under_its_value_name() {
    let sb = Sandbox::new("blank");
    registry::set_string(&sb.roots.user_run, "Blank", "").unwrap();
    let (entries, _) = list_startup(&sb.roots);
    let blank = find(&entries, "Blank");
    assert_eq!((blank.name.as_str(), blank.command.as_str(), blank.publisher.as_deref()), ("Blank", "", None));
    assert!(blank.enabled);
}

#[test]
fn missing_lists_are_empty_not_errors() {
    let sb = Sandbox::new("empty");
    let (entries, errors) = list_startup(&sb.roots);
    assert!(entries.is_empty() && errors.is_empty(), "{entries:?} {errors:?}");
}

#[test]
fn the_real_lists_can_be_read_without_changing_them() {
    let (entries, _errors) = list_startup(&StartupRoots::system());
    for e in &entries {
        assert!(!e.name.is_empty() && !e.location.is_empty(), "{e:?}");
    }
}

#[test]
fn the_cleanup_helper_refuses_anything_outside_the_test_root() {
    assert!(registry::delete_test_tree(&RegKey::new(Hive::CurrentUser, r"Software\Microsoft")).is_err());
    assert!(registry::delete_test_tree(&RegKey::new(Hive::LocalMachine, &format!(r"{}\x", registry::TEST_ROOT))).is_err());
    assert!(registry::delete_test_tree(&RegKey::new(Hive::CurrentUser, registry::TEST_ROOT)).is_err(), "not the root itself");
    assert!(registry::delete_test_tree(&RegKey::new(Hive::CurrentUser, r"Software\Negotium-TestX\y")).is_err());
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p negotium-collector --test identity; cargo test -p negotium-actions`
Expected: FAIL to compile (no `version_string`, no `startup` module).

- [ ] **Step 3: Implement**

`crates/collector/src/identity.rs` — generalise the version reader (same body, `key` instead of the literal) and keep the old helper as a call:
```rust
/// A string value from the first translation of a file's version resource ("FileDescription",
/// "CompanyName", …), trimmed; `None` if the file, the resource or the value is missing or empty.
pub fn version_string(path: &str, key: &str) -> Option<String> {
    // body of the former `file_description`, with the query built as:
    // let query = HSTRING::from(format!("\\StringFileInfo\\{lang:04x}{cp:04x}\\{key}"));
}

fn file_description(path: &str) -> Option<String> {
    version_string(path, "FileDescription")
}
```
`crates/collector/src/lib.rs`: `pub use identity::{Identity, IdentityCache, version_string};`.

`crates/actions/Cargo.toml`: move `negotium-collector.workspace = true` from `[dev-dependencies]` to `[dependencies]`; add `windows` features `"Win32_System_Com", "Win32_Storage_FileSystem", "Management_Deployment", "ApplicationModel", "Foundation", "Foundation_Collections",` (the last four are used by Task 6; `Win32_System_Com`/`Win32_Storage_FileSystem` by Task 5 — adding them now keeps the feature list in one commit).

`crates/actions/src/lib.rs`: `pub mod startup;`.

`crates/actions/src/startup/approved.rs` (above the tests):
```rust
//! The StartupApproved values Task Manager reads and writes (ruling P3b-2). Each is REG_BINARY,
//! 12 bytes: a little-endian DWORD of flags whose low bit is set while the entry is disabled
//! (Windows writes 0x02 / 0x03; some installers 0x06 / 0x07), then a little-endian FILETIME (UTC)
//! of when it was disabled, zero while enabled. A missing value, or one too short to hold the
//! flags, means enabled.
pub const APPROVED_LEN: usize = 12;
const DEFAULT_FLAGS: u32 = 0x2;
const UNIX_EPOCH_AS_FILETIME: u64 = 116_444_736_000_000_000;

pub fn is_enabled(bytes: &[u8]) -> bool {
    bytes.len() < 4 || bytes[0] & 1 == 0
}

/// The value to write: `previous`'s other flag bits kept (0x02 when there are none), the low bit
/// set when disabling together with `now_filetime`, and zeros when enabling.
pub fn encode_approved(previous: Option<&[u8]>, enabled: bool, now_filetime: u64) -> [u8; APPROVED_LEN] {
    let old = previous
        .filter(|b| b.len() >= 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .unwrap_or(DEFAULT_FLAGS);
    let base = match old & !1 {
        0 => DEFAULT_FLAGS,
        other => other,
    };
    let mut out = [0u8; APPROVED_LEN];
    out[..4].copy_from_slice(&(if enabled { base } else { base | 1 }).to_le_bytes());
    if !enabled {
        out[4..].copy_from_slice(&now_filetime.to_le_bytes());
    }
    out
}

pub fn filetime_now() -> u64 {
    let since = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    UNIX_EPOCH_AS_FILETIME + (since.as_nanos() / 100) as u64
}
```

`crates/actions/src/startup/command.rs` (above the tests):
```rust
//! The program a Run command starts, for its name and publisher.

/// The quoted path, else the text up to and including ".exe", else the first word; environment
/// variables expanded; a bare file name taken from System32. `None` for an empty command.
pub fn exe_from_command(command: &str, system_root: &str) -> Option<String> {
    let c = command.trim();
    let raw = if let Some(rest) = c.strip_prefix('"') {
        rest.split('"').next().unwrap_or(rest)
    } else if let Some(end) = c.to_ascii_lowercase().find(".exe") {
        &c[..end + 4]
    } else {
        c.split_whitespace().next().unwrap_or("")
    };
    let path = expand_env(raw.trim(), |name| std::env::var(name).ok());
    if path.is_empty() {
        None
    } else if path.contains('\\') {
        Some(path)
    } else {
        Some(format!("{}\\System32\\{path}", system_root.trim_end_matches('\\')))
    }
}

/// Expands `%NAME%` through `lookup`; unknown or empty names stay as written.
pub fn expand_env(s: &str, lookup: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('%') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let name = &after[..end];
        match (!name.is_empty()).then(|| lookup(name)).flatten() {
            Some(value) => out.push_str(&value),
            None => {
                out.push('%');
                out.push_str(name);
                out.push('%');
            }
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}
```

`crates/actions/src/startup/registry.rs`:
```rust
//! Registry helpers for the startup lists: read a key's values and subkeys, write one value.
//! Paths are relative to a hive; a missing key reads as empty.
use crate::ActionError;
use windows::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS, ERROR_SUCCESS, WIN32_ERROR,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_SET_VALUE, REG_BINARY, REG_DWORD, REG_EXPAND_SZ,
    REG_OPTION_NON_VOLATILE, REG_SAM_FLAGS, REG_SZ, REG_VALUE_TYPE, RRF_NOEXPAND, RRF_RT_ANY, RegCloseKey,
    RegCreateKeyExW, RegDeleteTreeW, RegEnumKeyExW, RegEnumValueW, RegGetValueW, RegOpenKeyExW, RegSetValueExW,
};
use windows::core::{HSTRING, PCWSTR, PWSTR};

/// The only place `delete_test_tree` may delete from: tests' sandboxes.
pub const TEST_ROOT: &str = r"Software\Negotium-Test";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Hive {
    CurrentUser,
    LocalMachine,
}

impl Hive {
    fn hkey(self) -> HKEY {
        match self {
            Hive::CurrentUser => HKEY_CURRENT_USER,
            Hive::LocalMachine => HKEY_LOCAL_MACHINE,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Hive::CurrentUser => "HKCU",
            Hive::LocalMachine => "HKLM",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegKey {
    pub hive: Hive,
    pub path: String,
}

impl RegKey {
    pub fn new(hive: Hive, path: &str) -> Self {
        Self { hive, path: path.to_string() }
    }

    pub fn child(&self, name: &str) -> RegKey {
        RegKey { hive: self.hive, path: format!("{}\\{name}", self.path) }
    }

    pub fn display(&self) -> String {
        format!("{}\\{}", self.hive.name(), self.path)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum RegValue {
    Str(String),
    Binary(Vec<u8>),
    Dword(u32),
    Other,
}

impl RegValue {
    pub fn as_str(&self) -> Option<&str> {
        if let RegValue::Str(s) = self { Some(s) } else { None }
    }

    pub fn binary(&self) -> Option<&[u8]> {
        if let RegValue::Binary(b) = self { Some(b) } else { None }
    }

    pub fn dword(&self) -> Option<u32> {
        if let RegValue::Dword(d) = self { Some(*d) } else { None }
    }

    fn decode(kind: REG_VALUE_TYPE, data: &[u8]) -> RegValue {
        match kind {
            REG_SZ | REG_EXPAND_SZ => {
                let units: Vec<u16> = data.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
                RegValue::Str(String::from_utf16_lossy(&units[..end]))
            }
            REG_BINARY => RegValue::Binary(data.to_vec()),
            REG_DWORD if data.len() >= 4 => RegValue::Dword(u32::from_le_bytes([data[0], data[1], data[2], data[3]])),
            _ => RegValue::Other,
        }
    }
}

struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: opened by RegOpenKeyExW / RegCreateKeyExW and closed exactly once here.
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

fn error(code: WIN32_ERROR) -> ActionError {
    if code == ERROR_ACCESS_DENIED {
        ActionError::AccessDenied
    } else {
        let text = windows::core::Error::from(code.to_hresult()).message();
        ActionError::Other(text.trim().trim_end_matches('.').to_string())
    }
}

fn open(key: &RegKey, access: REG_SAM_FLAGS) -> Result<Option<Key>, ActionError> {
    let mut h = HKEY::default();
    let path = HSTRING::from(key.path.as_str());
    // SAFETY: opens a key; the handle is owned (and closed) by `Key`.
    let status = unsafe { RegOpenKeyExW(key.hive.hkey(), &path, None, access, &mut h) };
    match status {
        ERROR_SUCCESS => Ok(Some(Key(h))),
        ERROR_FILE_NOT_FOUND => Ok(None),
        e => Err(error(e)),
    }
}

pub fn values(key: &RegKey) -> Result<Vec<(String, RegValue)>, ActionError> {
    let Some(k) = open(key, KEY_READ)? else { return Ok(Vec::new()) };
    let mut out = Vec::new();
    let mut name = vec![0u16; 16_384]; // the registry's longest value name
    let mut data = vec![0u8; 64 * 1024];
    for index in 0u32.. {
        let mut name_len = name.len() as u32;
        let mut data_len = data.len() as u32;
        let mut kind = 0u32;
        // SAFETY: both buffers are writable for the lengths passed; the lengths are updated.
        let status = unsafe {
            RegEnumValueW(
                k.0, index, Some(PWSTR(name.as_mut_ptr())), &mut name_len, None, Some(&mut kind),
                Some(data.as_mut_ptr()), Some(&mut data_len),
            )
        };
        match status {
            ERROR_SUCCESS => out.push((
                String::from_utf16_lossy(&name[..name_len as usize]),
                RegValue::decode(REG_VALUE_TYPE(kind), &data[..data_len as usize]),
            )),
            ERROR_MORE_DATA => continue, // over 64 KB: not a startup entry
            ERROR_NO_MORE_ITEMS => break,
            e => return Err(error(e)),
        }
    }
    Ok(out)
}

pub fn value(key: &RegKey, name: &str) -> Option<RegValue> {
    let (path, name) = (HSTRING::from(key.path.as_str()), HSTRING::from(name));
    let flags = RRF_RT_ANY | RRF_NOEXPAND;
    let mut kind = REG_VALUE_TYPE::default();
    let mut size = 0u32;
    // SAFETY: size query with no buffer.
    let status = unsafe { RegGetValueW(key.hive.hkey(), &path, &name, flags, Some(&mut kind), None, Some(&mut size)) };
    if status != ERROR_SUCCESS {
        return None;
    }
    let mut data = vec![0u8; size as usize];
    // SAFETY: `data` has `size` writable bytes.
    let status = unsafe {
        RegGetValueW(key.hive.hkey(), &path, &name, flags, Some(&mut kind), Some(data.as_mut_ptr().cast()), Some(&mut size))
    };
    (status == ERROR_SUCCESS).then(|| RegValue::decode(kind, &data[..(size as usize).min(data.len())]))
}

pub fn subkeys(key: &RegKey) -> Result<Vec<String>, ActionError> {
    let Some(k) = open(key, KEY_READ)? else { return Ok(Vec::new()) };
    let mut out = Vec::new();
    let mut name = vec![0u16; 256]; // key names are at most 255 characters
    for index in 0u32.. {
        let mut len = name.len() as u32;
        // SAFETY: `name` is writable for `len` units; the other outputs are not requested.
        let status = unsafe { RegEnumKeyExW(k.0, index, Some(PWSTR(name.as_mut_ptr())), &mut len, None, None, None, None) };
        match status {
            ERROR_SUCCESS => out.push(String::from_utf16_lossy(&name[..len as usize])),
            ERROR_NO_MORE_ITEMS => break,
            e => return Err(error(e)),
        }
    }
    Ok(out)
}

fn set(key: &RegKey, name: &str, kind: REG_VALUE_TYPE, data: &[u8]) -> Result<(), ActionError> {
    let mut h = HKEY::default();
    let path = HSTRING::from(key.path.as_str());
    // SAFETY: creates or opens the key for writing; the handle is owned by `Key`.
    let status = unsafe {
        RegCreateKeyExW(key.hive.hkey(), &path, None, PCWSTR::null(), REG_OPTION_NON_VOLATILE, KEY_SET_VALUE, None, &mut h, None)
    };
    if status != ERROR_SUCCESS {
        return Err(error(status));
    }
    let h = Key(h);
    let name = HSTRING::from(name);
    // SAFETY: valid key handle; the data is passed as a slice.
    let status = unsafe { RegSetValueExW(h.0, &name, None, kind, Some(data)) };
    if status == ERROR_SUCCESS { Ok(()) } else { Err(error(status)) }
}

pub fn set_binary(key: &RegKey, name: &str, data: &[u8]) -> Result<(), ActionError> {
    set(key, name, REG_BINARY, data)
}

pub fn set_dword(key: &RegKey, name: &str, value: u32) -> Result<(), ActionError> {
    set(key, name, REG_DWORD, &value.to_le_bytes())
}

pub fn set_string(key: &RegKey, name: &str, value: &str) -> Result<(), ActionError> {
    let bytes: Vec<u8> = value.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect();
    set(key, name, REG_SZ, &bytes)
}

/// Deletes a test sandbox. Refuses anything but a subkey of HKCU\Software\Negotium-Test.
pub fn delete_test_tree(key: &RegKey) -> Result<(), ActionError> {
    let prefix = format!("{TEST_ROOT}\\");
    let inside = key.path.get(..prefix.len()).is_some_and(|p| p.eq_ignore_ascii_case(&prefix)) && key.path.len() > prefix.len();
    if key.hive != Hive::CurrentUser || !inside {
        return Err(ActionError::Other(format!("refusing to delete {}", key.display())));
    }
    let path = HSTRING::from(key.path.as_str());
    // SAFETY: deletes a key tree under HKCU that was checked to be a test sandbox.
    match unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, &path) } {
        ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => Ok(()),
        e => Err(error(e)),
    }
}
```

`crates/actions/src/startup/mod.rs`:
```rust
//! Startup apps (spec §5.3): Run-key entries (this task), the Startup folders (Task 5) and packaged
//! startup tasks (Task 6), with the enabled state Task Manager uses. Listing and toggling call
//! Windows: run them on an action thread. Only StartupApproved values (and packaged task states)
//! are ever written; Run values and shortcuts are never edited or deleted.
mod approved;
mod command;
pub mod registry;

pub use approved::{APPROVED_LEN, encode_approved, filetime_now, is_enabled};
pub use command::{exe_from_command, expand_env};

use crate::ActionError;
use negotium_collector::version_string;
use registry::{Hive, RegKey, RegValue};

const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN32: &str = r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Run";
const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StartupSource {
    UserRun,
    MachineRun,
    /// HKLM\SOFTWARE\WOW6432Node\…\Run (32-bit programs).
    MachineRun32,
}

impl StartupSource {
    /// Changing these entries writes under HKLM: admin only.
    pub fn needs_admin(self) -> bool {
        matches!(self, StartupSource::MachineRun | StartupSource::MachineRun32)
    }

    pub fn label(self) -> &'static str {
        match self {
            StartupSource::UserRun => "Registry (you)",
            StartupSource::MachineRun => "Registry (all users)",
            StartupSource::MachineRun32 => "Registry (all users, 32-bit)",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct StartupEntry {
    pub source: StartupSource,
    /// What identifies it for a toggle: the Run value name, the shortcut's file name, or
    /// "PackageFamilyName\TaskId".
    pub key: String,
    /// The program's description, else `key` (or the shortcut's name).
    pub name: String,
    pub publisher: Option<String>,
    pub enabled: bool,
    /// False when Windows policy fixes the state.
    pub changeable: bool,
    pub command: String,
    /// Where the entry lives: a registry key or a folder.
    pub location: String,
}

#[derive(Clone, Debug)]
pub struct StartupRoots {
    pub user_run: RegKey,
    pub machine_run: RegKey,
    pub machine_run32: RegKey,
    /// The StartupApproved parents; `Run`, `Run32` and `StartupFolder` are their children.
    pub user_approved: RegKey,
    pub machine_approved: RegKey,
    pub system_root: String,
}

impl StartupRoots {
    /// The real lists.
    pub fn system() -> Self {
        Self {
            user_run: RegKey::new(Hive::CurrentUser, RUN),
            machine_run: RegKey::new(Hive::LocalMachine, RUN),
            machine_run32: RegKey::new(Hive::LocalMachine, RUN32),
            user_approved: RegKey::new(Hive::CurrentUser, APPROVED),
            machine_approved: RegKey::new(Hive::LocalMachine, APPROVED),
            system_root: std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string()),
        }
    }

    /// Every list below HKCU\`base`, for tests: the user's real entries are never touched.
    pub fn under(base: &str) -> Self {
        let key = |name: &str| RegKey::new(Hive::CurrentUser, &format!("{base}\\{name}"));
        Self {
            user_run: key("UserRun"),
            machine_run: key("MachineRun"),
            machine_run32: key("MachineRun32"),
            user_approved: key("UserApproved"),
            machine_approved: key("MachineApproved"),
            system_root: std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string()),
        }
    }

    fn approved_key(&self, source: StartupSource) -> RegKey {
        match source {
            StartupSource::UserRun => self.user_approved.child("Run"),
            StartupSource::MachineRun => self.machine_approved.child("Run"),
            StartupSource::MachineRun32 => self.machine_approved.child("Run32"),
        }
    }
}

/// Enabled unless a StartupApproved value (names compare case-insensitively) says disabled.
fn enabled_in(states: &[(String, RegValue)], name: &str) -> bool {
    states.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).and_then(|(_, v)| v.binary()).is_none_or(is_enabled)
}

fn run_entries(roots: &StartupRoots, source: StartupSource, run: &RegKey) -> Result<Vec<StartupEntry>, ActionError> {
    let states = registry::values(&roots.approved_key(source))?;
    Ok(registry::values(run)?
        .into_iter()
        .filter(|(name, _)| !name.is_empty())
        .filter_map(|(name, v)| v.as_str().map(|c| (name, c.to_string())))
        .map(|(name, command)| {
            let exe = exe_from_command(&command, &roots.system_root);
            StartupEntry {
                source,
                name: exe.as_deref().and_then(|e| version_string(e, "FileDescription")).unwrap_or_else(|| name.clone()),
                publisher: exe.as_deref().and_then(|e| version_string(e, "CompanyName")),
                enabled: enabled_in(&states, &name),
                changeable: true,
                command,
                location: run.display(),
                key: name,
            }
        })
        .collect())
}

/// Every startup entry, and a readable line for each list that couldn't be read.
pub fn list_startup(roots: &StartupRoots) -> (Vec<StartupEntry>, Vec<String>) {
    let mut entries = Vec::new();
    let mut errors = Vec::new();
    let runs = [
        (StartupSource::UserRun, &roots.user_run),
        (StartupSource::MachineRun, &roots.machine_run),
        (StartupSource::MachineRun32, &roots.machine_run32),
    ];
    for (source, run) in runs {
        match run_entries(roots, source, run) {
            Ok(found) => entries.extend(found),
            Err(e) => errors.push(format!("startup: {} could not be read ({e:?})", run.display())),
        }
    }
    (entries, errors)
}

/// Enables or disables an entry the way Task Manager does. Writing an HKLM value without admin
/// fails with `ActionError::AccessDenied` (shown inline with the restart-as-admin hint).
pub fn set_startup_enabled(roots: &StartupRoots, entry: &StartupEntry, enabled: bool) -> Result<(), ActionError> {
    let key = roots.approved_key(entry.source);
    let previous = registry::value(&key, &entry.key);
    let bytes = encode_approved(previous.as_ref().and_then(RegValue::binary), enabled, filetime_now());
    registry::set_binary(&key, &entry.key, &bytes)
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p negotium-collector --test identity; cargo test -p negotium-actions`
Expected: all pass, warning-free. Afterwards `reg query HKCU\Software\Negotium-Test` shows no leftover sandbox (or "unable to find" — both fine).

- [ ] **Step 5: Commit**

```bash
git add crates/actions crates/collector
git commit -m "feat(actions): list Run-key startup entries and toggle them through StartupApproved"
```

---

### Task 5: Startup — Startup folders and shortcuts

**Files:**
- Create: `crates/actions/src/startup/folders.rs`
- Modify: `crates/actions/src/startup/mod.rs`, `crates/actions/tests/startup.rs`
- Test: `crates/actions/tests/startup.rs`

**Interfaces:**
- Consumes: Task 4's `startup` module.
- Produces:
  - `StartupSource` gains `UserFolder` ("Startup folder (you)", not admin) and `MachineFolder` ("Startup folder (all users)", admin); their StartupApproved values are under `…\StartupFolder`, named by file name (e.g. `Spotify.lnk`).
  - `StartupRoots` gains `user_folder: Option<PathBuf>`, `machine_folder: Option<PathBuf>` (`system()` reads `FOLDERID_Startup` / `FOLDERID_CommonStartup`; `under()` leaves them `None`) and `with_folders(self, user: PathBuf, machine: PathBuf) -> StartupRoots`.
  - `startup::{resolve_shortcut(&Path) -> Option<(String, String)> /* target, arguments */, write_shortcut(path: &Path, target: &str, args: &str) -> Result<(), ActionError>, ComApartment::enter() -> ComApartment}` (the COM guard: initialises an apartment for the calling thread, uninitialises on drop if it initialised one). `list_startup` holds one for its duration.

- [ ] **Step 1: Write the failing test** — in `crates/actions/tests/startup.rs`, replace `Sandbox` with one that also owns two temporary folders, and add the test:
```rust
struct Sandbox {
    base: RegKey,
    dir: std::path::PathBuf,
    roots: StartupRoots,
}

impl Sandbox {
    fn new(name: &str) -> Self {
        let base = RegKey::new(Hive::CurrentUser, &format!(r"{}\{name}-{}", registry::TEST_ROOT, std::process::id()));
        let _ = registry::delete_test_tree(&base);
        let dir = std::env::temp_dir().join(format!("negotium-startup-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (user, machine) = (dir.join("user"), dir.join("machine"));
        std::fs::create_dir_all(&user).unwrap();
        std::fs::create_dir_all(&machine).unwrap();
        Self { roots: StartupRoots::under(&base.path).with_folders(user, machine), base, dir }
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = registry::delete_test_tree(&self.base);
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn startup_folder_entries_resolve_shortcuts_and_toggle_under_startup_folder() {
    use negotium_actions::startup::write_shortcut;
    let sb = Sandbox::new("folders");
    let r = &sb.roots;
    let user = r.user_folder.clone().unwrap();
    write_shortcut(&user.join("Ping test.lnk"), PING, "-n 1 127.0.0.1").unwrap();
    std::fs::write(user.join("desktop.ini"), "[.ShellClassInfo]").unwrap();
    std::fs::write(user.join("notes.bat"), "@echo off").unwrap();
    std::fs::write(r.machine_folder.clone().unwrap().join("tool.bat"), "@echo off").unwrap();

    let (entries, errors) = list_startup(r);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(entries.len(), 3, "desktop.ini is not an entry: {entries:?}");
    let ping = find(&entries, "Ping test.lnk").clone();
    assert_eq!(ping.source, StartupSource::UserFolder);
    assert!(ping.command.to_ascii_uppercase().contains("PING.EXE") && ping.command.contains("-n 1"), "{}", ping.command);
    assert!(ping.publisher.is_some(), "the shortcut's target is read for the publisher");
    let notes = find(&entries, "notes.bat");
    assert_eq!(notes.name, "notes");
    let tool = find(&entries, "tool.bat");
    assert_eq!(tool.source, StartupSource::MachineFolder);
    assert!(tool.source.needs_admin());

    set_startup_enabled(r, &ping, false).unwrap();
    assert_eq!(approved(&r.user_approved.child("StartupFolder"), "Ping test.lnk")[0], 3);
    assert!(!find(&list_startup(r).0, "Ping test.lnk").enabled);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p negotium-actions --test startup`
Expected: FAIL to compile (no `with_folders`, `user_folder`, `write_shortcut`, `UserFolder`).

- [ ] **Step 3: Implement**

`crates/actions/src/startup/folders.rs`:
```rust
//! The per-user and all-users Startup folders: one entry per file (desktop.ini excepted); a
//! shortcut is resolved to its target for the name and publisher.
use super::{StartupEntry, StartupRoots, StartupSource, enabled_in};
use crate::ActionError;
use negotium_collector::version_string;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize,
    IPersistFile, STGM_READ,
};
use windows::Win32::UI::Shell::{FOLDERID_CommonStartup, FOLDERID_Startup, IShellLinkW, KNOWN_FOLDER_FLAG, SHGetKnownFolderPath, ShellLink};
use windows::core::{GUID, HSTRING, Interface};

/// A COM apartment for the calling thread (shell links and package names need one), released on
/// drop if this guard initialised it.
pub struct ComApartment(bool);

impl ComApartment {
    pub fn enter() -> Self {
        // SAFETY: plain per-thread initialisation; balanced in Drop when it succeeded.
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        Self(hr.is_ok())
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: balances the successful CoInitializeEx in `enter` on the same thread.
            unsafe { CoUninitialize() };
        }
    }
}

fn known_folder(id: &GUID) -> Option<PathBuf> {
    // SAFETY: returns a CoTaskMem string that is freed below.
    let p = unsafe { SHGetKnownFolderPath(id, KNOWN_FOLDER_FLAG(0), None) }.ok()?;
    // SAFETY: `p` is a valid NUL-terminated string until freed.
    let s = unsafe { p.to_string() }.ok();
    // SAFETY: allocated by SHGetKnownFolderPath; freed exactly once.
    unsafe { CoTaskMemFree(Some(p.0 as *const c_void)) };
    s.map(PathBuf::from)
}

/// (per-user, all-users) Startup folders.
pub(crate) fn startup_folders() -> (Option<PathBuf>, Option<PathBuf>) {
    (known_folder(&FOLDERID_Startup), known_folder(&FOLDERID_CommonStartup))
}

fn wide(units: &[u16]) -> String {
    let end = units.iter().position(|&c| c == 0).unwrap_or(units.len());
    String::from_utf16_lossy(&units[..end])
}

/// A shortcut's (target, arguments). Needs a COM apartment on this thread.
pub fn resolve_shortcut(path: &Path) -> Option<(String, String)> {
    // SAFETY: a COM object created and used on this thread only; buffers sized as passed.
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        link.cast::<IPersistFile>().ok()?.Load(&HSTRING::from(path.as_os_str()), STGM_READ).ok()?;
        let mut target = [0u16; 1024];
        link.GetPath(&mut target, std::ptr::null_mut(), 0).ok()?;
        let mut args = [0u16; 1024];
        let _ = link.GetArguments(&mut args);
        Some((wide(&target), wide(&args)))
    }
}

/// Writes a shortcut (tests build their Startup folders with it).
pub fn write_shortcut(path: &Path, target: &str, args: &str) -> Result<(), ActionError> {
    let _com = ComApartment::enter();
    let fail = |e: windows::core::Error| ActionError::from_win(&e);
    // SAFETY: a COM object created and used on this thread only.
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).map_err(fail)?;
        link.SetPath(&HSTRING::from(target)).map_err(fail)?;
        link.SetArguments(&HSTRING::from(args)).map_err(fail)?;
        link.cast::<IPersistFile>().map_err(fail)?.Save(&HSTRING::from(path.as_os_str()), true).map_err(fail)
    }
}

pub(crate) fn folder_entries(roots: &StartupRoots, source: StartupSource, dir: &Path) -> Result<Vec<StartupEntry>, ActionError> {
    let states = super::registry::values(&roots.approved_key(source))?;
    let Ok(read) = std::fs::read_dir(dir) else { return Ok(Vec::new()) };
    let mut out = Vec::new();
    for item in read.flatten() {
        let path = item.path();
        let file = item.file_name().to_string_lossy().into_owned();
        if !path.is_file() || file.eq_ignore_ascii_case("desktop.ini") {
            continue;
        }
        let shortcut = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("lnk"));
        let (target, args) = if shortcut {
            resolve_shortcut(&path).unwrap_or_default()
        } else {
            (path.to_string_lossy().into_owned(), String::new())
        };
        let exe = (!target.is_empty()).then_some(target.as_str());
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| file.clone());
        out.push(StartupEntry {
            source,
            name: exe.and_then(|e| version_string(e, "FileDescription")).unwrap_or(stem),
            publisher: exe.and_then(|e| version_string(e, "CompanyName")),
            enabled: enabled_in(&states, &file),
            changeable: true,
            command: if args.is_empty() { target.clone() } else { format!("{target} {args}") },
            location: dir.display().to_string(),
            key: file,
        });
    }
    out.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(out)
}
```

`crates/actions/src/startup/mod.rs`:
- `mod folders;` and `pub use folders::{ComApartment, resolve_shortcut, write_shortcut};`; `use std::path::PathBuf;`
- `StartupSource` gains `UserFolder`, `MachineFolder`; `needs_admin` matches `MachineRun | MachineRun32 | MachineFolder`; labels `"Startup folder (you)"`, `"Startup folder (all users)"`.
- `StartupRoots` gains
```rust
    pub user_folder: Option<PathBuf>,
    pub machine_folder: Option<PathBuf>,
```
  `system()` fills them from `folders::startup_folders()`; `under()` sets both `None`; add
```rust
    pub fn with_folders(mut self, user: PathBuf, machine: PathBuf) -> Self {
        self.user_folder = Some(user);
        self.machine_folder = Some(machine);
        self
    }
```
  and `approved_key` gains `StartupSource::UserFolder => self.user_approved.child("StartupFolder"), StartupSource::MachineFolder => self.machine_approved.child("StartupFolder"),`.
- `list_startup` starts with `let _com = ComApartment::enter();` and, after the Run loop:
```rust
    let folders = [
        (StartupSource::UserFolder, roots.user_folder.as_deref()),
        (StartupSource::MachineFolder, roots.machine_folder.as_deref()),
    ];
    for (source, dir) in folders {
        let Some(dir) = dir else { continue };
        match folders::folder_entries(roots, source, dir) {
            Ok(found) => entries.extend(found),
            Err(e) => errors.push(format!("startup: {} could not be read ({e:?})", dir.display())),
        }
    }
```
- `enabled_in` becomes `pub(crate)`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p negotium-actions`
Expected: all pass, warning-free; no `negotium-startup-test-*` folder left in `%TEMP%`.

- [ ] **Step 5: Commit**

```bash
git add crates/actions
git commit -m "feat(actions): Startup folder entries with shortcut targets"
```

---

### Task 6: Startup — packaged startup tasks

**Files:**
- Create: `crates/actions/src/startup/packaged.rs`
- Modify: `crates/actions/src/startup/mod.rs`, `crates/actions/tests/startup.rs`
- Test: unit tests in `packaged.rs`; `crates/actions/tests/startup.rs`

**Interfaces:**
- Consumes: Tasks 4–5 `startup` module, `ComApartment`.
- Produces:
  - `StartupSource::Packaged` ("Packaged app", not admin: the state lives under HKCU).
  - `StartupRoots` gains `packaged: RegKey` (`system()`: `HKCU\Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\SystemAppData`; `under()`: `HKCU\<base>\Packaged`).
  - `startup::packaged::{SYSTEM_APP_DATA, state_enabled(u32) -> bool, state_changeable(u32) -> bool, toggled_state(bool) -> u32, family_fallback_name(&str) -> String}`; packaged entries have `key = "<PackageFamilyName>\<TaskId>"`, `command = <TaskId>`, `location = "Packaged app"`; `set_startup_enabled` on a policy-fixed entry returns `Err(ActionError::Other("its state is set by a policy"))`.

- [ ] **Step 1: Write the failing tests**

`crates/actions/src/startup/packaged.rs` tests (implementation in Step 3):
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_states_follow_the_startup_task_api() {
        assert_eq!([0, 1, 2, 3, 4].map(state_enabled), [false, false, true, false, true]);
        assert_eq!([0, 1, 2, 3, 4].map(state_changeable), [true, true, true, false, false]);
        assert_eq!((toggled_state(true), toggled_state(false)), (2, 1), "Task Manager: enabled / disabled by user");
    }

    #[test]
    fn a_family_that_isnt_installed_is_named_after_itself() {
        assert_eq!(family_fallback_name("Contoso.App_8wekyb3d8bbwe"), "Contoso.App");
        assert_eq!(family_fallback_name("odd"), "odd");
    }
}
```

`crates/actions/tests/startup.rs` — add:
```rust
#[test]
fn packaged_tasks_are_listed_from_their_state_and_toggle_it() {
    let sb = Sandbox::new("packaged");
    let r = &sb.roots;
    registry::set_dword(&r.packaged.child(r"Contoso.App_8wekyb3d8bbwe\StartTask"), "State", 2).unwrap();
    registry::set_dword(&r.packaged.child(r"Contoso.Locked_abc\Boot"), "State", 3).unwrap();
    registry::set_string(&r.packaged.child(r"Contoso.NoTasks_xyz\Schemas"), "Version", "1").unwrap();

    let (entries, errors) = list_startup(r);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(entries.len(), 2, "a family without a State is no startup task: {entries:?}");
    let app = find(&entries, r"Contoso.App_8wekyb3d8bbwe\StartTask").clone();
    assert_eq!(app.source, StartupSource::Packaged);
    assert!(app.enabled && app.changeable && !app.source.needs_admin());
    assert_eq!((app.name.as_str(), app.publisher.as_deref(), app.location.as_str()), ("Contoso.App", None, "Packaged app"));
    let locked = find(&entries, r"Contoso.Locked_abc\Boot").clone();
    assert!(!locked.enabled && !locked.changeable, "disabled by policy");

    set_startup_enabled(r, &app, false).unwrap();
    let state = |key: &str| registry::value(&r.packaged.child(key), "State").and_then(|v| v.dword());
    assert_eq!(state(r"Contoso.App_8wekyb3d8bbwe\StartTask"), Some(1));
    assert!(!find(&list_startup(r).0, r"Contoso.App_8wekyb3d8bbwe\StartTask").enabled);
    set_startup_enabled(r, &app, true).unwrap();
    assert_eq!(state(r"Contoso.App_8wekyb3d8bbwe\StartTask"), Some(2));
    let refused = set_startup_enabled(r, &locked, true).unwrap_err();
    assert_eq!(refused.message("Contoso.Locked", false), "Couldn't change Contoso.Locked: its state is set by a policy.");
    assert_eq!(state(r"Contoso.Locked_abc\Boot"), Some(3), "untouched");
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p negotium-actions`
Expected: FAIL to compile (no `packaged` module, `StartupRoots::packaged`, `StartupSource::Packaged`).

- [ ] **Step 3: Implement**

`crates/actions/src/startup/packaged.rs` (above the tests):
```rust
//! Packaged-app startup tasks (MSIX `windows.startupTask`, ruling P3b-1). Windows keeps each task's
//! state in SYSTEM_APP_DATA\<PackageFamilyName>\<TaskId>, DWORD `State`, with the
//! Windows.ApplicationModel.StartupTaskState values: 0 disabled, 1 disabled by user, 2 enabled,
//! 3 disabled by policy, 4 enabled by policy.
use super::registry::{self, RegKey};
use super::{StartupEntry, StartupRoots, StartupSource};
use crate::ActionError;
use windows::Management::Deployment::PackageManager;
use windows::core::HSTRING;

pub const SYSTEM_APP_DATA: &str =
    r"Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\SystemAppData";

pub fn state_enabled(state: u32) -> bool {
    matches!(state, 2 | 4)
}

/// Policy states (3, 4) are fixed.
pub fn state_changeable(state: u32) -> bool {
    state <= 2
}

/// What Task Manager writes: 2 (enabled) or 1 (disabled by user).
pub fn toggled_state(enabled: bool) -> u32 {
    if enabled { 2 } else { 1 }
}

pub fn family_fallback_name(pfn: &str) -> String {
    pfn.split('_').next().unwrap_or(pfn).to_string()
}

/// (display name, publisher display name) of the current user's installed package of that family.
fn package_names(pfn: &str) -> Option<(String, String)> {
    let manager = PackageManager::new().ok()?;
    let packages = manager.FindPackagesByUserSecurityIdPackageFamilyName(&HSTRING::new(), &HSTRING::from(pfn)).ok()?;
    let first = packages.First().ok()?;
    if !first.HasCurrent().ok()? {
        return None;
    }
    let package = first.Current().ok()?;
    Some((package.DisplayName().ok()?.to_string(), package.PublisherDisplayName().ok()?.to_string()))
}

pub(crate) fn packaged_entries(roots: &StartupRoots) -> Result<Vec<StartupEntry>, ActionError> {
    let mut out = Vec::new();
    for pfn in registry::subkeys(&roots.packaged)? {
        let family = roots.packaged.child(&pfn);
        for task in registry::subkeys(&family).unwrap_or_default() {
            let Some(state) = registry::value(&family.child(&task), "State").and_then(|v| v.dword()) else { continue };
            let names = package_names(&pfn);
            out.push(StartupEntry {
                source: StartupSource::Packaged,
                key: format!("{pfn}\\{task}"),
                name: names.as_ref().map(|(n, _)| n.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| family_fallback_name(&pfn)),
                publisher: names.map(|(_, p)| p).filter(|p| !p.is_empty()),
                enabled: state_enabled(state),
                changeable: state_changeable(state),
                command: task,
                location: "Packaged app".to_string(),
            });
        }
    }
    Ok(out)
}

pub(crate) fn set_enabled(packaged: &RegKey, entry: &StartupEntry, enabled: bool) -> Result<(), ActionError> {
    if !entry.changeable {
        return Err(ActionError::Other("its state is set by a policy".into()));
    }
    let (pfn, task) = entry.key.split_once('\\').ok_or_else(|| ActionError::Other("unknown packaged task".into()))?;
    registry::set_dword(&packaged.child(pfn).child(task), "State", toggled_state(enabled))
}
```

`crates/actions/src/startup/mod.rs`:
- `pub mod packaged;`
- `StartupSource::Packaged` (label `"Packaged app"`; `needs_admin` false).
- `StartupRoots` gains `pub packaged: RegKey,` — `system()`: `RegKey::new(Hive::CurrentUser, packaged::SYSTEM_APP_DATA)`; `under()`: `key("Packaged")`.
- packaged tasks have no StartupApproved value: `approved_key` returns `Option<RegKey>` (`StartupSource::Packaged => None`, `Some(..)` for the others), and `run_entries` / `folder_entries` read their states with
```rust
    let states = match roots.approved_key(source) {
        Some(key) => registry::values(&key)?,
        None => Vec::new(),
    };
```
- `list_startup`, after the folders:
```rust
    match packaged::packaged_entries(roots) {
        Ok(found) => entries.extend(found),
        Err(e) => errors.push(format!("startup: {} could not be read ({e:?})", roots.packaged.display())),
    }
```
- `set_startup_enabled`:
```rust
pub fn set_startup_enabled(roots: &StartupRoots, entry: &StartupEntry, enabled: bool) -> Result<(), ActionError> {
    let Some(key) = roots.approved_key(entry.source) else {
        return packaged::set_enabled(&roots.packaged, entry, enabled);
    };
    let previous = registry::value(&key, &entry.key);
    let bytes = encode_approved(previous.as_ref().and_then(RegValue::binary), enabled, filetime_now());
    registry::set_binary(&key, &entry.key, &bytes)
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p negotium-actions`
Expected: all pass (including `the_real_lists_can_be_read_without_changing_them`, which now also reads the real packaged tasks and asks `PackageManager` for their names), warning-free.

- [ ] **Step 5: Commit**

```bash
git add crates/actions
git commit -m "feat(actions): packaged-app startup tasks"
```

---
### Task 7: Startup page

**Files:**
- Create: `crates/app/src/startup.rs`, `crates/app/ui/startuppage.slint`
- Modify: `crates/app/src/{lib,bridge,main}.rs`, `crates/app/ui/rail.slint`, `crates/app/ui/main.slint`
- Test: unit tests in `startup.rs`; `crates/app/tests/ui.rs`

**Interfaces:**
- Consumes: Tasks 4–6 `negotium_actions::startup::{StartupEntry, StartupRoots, StartupSource, list_startup, set_startup_enabled, registry}`; Task 3 `Toggle`; `ActionError::message`; `bridge::with_bridge`.
- Produces:
  - `negotium::startup::{StartupRow { id: i32, name, publisher, status, location, command: String, enabled: bool, can_change: bool, note: String }, startup_rows(&[StartupEntry], elevated: bool) -> Vec<StartupRow>, needs_admin_banner(&[StartupEntry], elevated: bool) -> bool, toggle_job(&StartupRoots, &StartupEntry, enabled: bool, elevated: bool) -> (Vec<StartupEntry>, Vec<String>, Option<String>)}`. `id` is the entry's index in the list it came from; rows are sorted by name (case-insensitive).
  - `Bridge::{set_startup(&mut self, ui: &MainWindow, entries: Vec<StartupEntry>, elevated: bool), startup_entry(&self, id: i32) -> Option<StartupEntry>, show_startup_pending(&self, id: i32, enabled: bool)}`.
  - Slint: exported struct `StartupData { id: int, name, publisher, status, location, command: string, enabled: bool, can-change: bool, note: string }`; component `StartupPage`; `MainWindow` gains `startup-rows: [StartupData]`, `startup-loading: bool` (in-out), `startup-admin-banner: bool`, callbacks `toggle-startup(int, bool)`, `refresh-startup()`; the toast moves to window level (visible on every page). Rail entry "Startup page" selects page 2. Row switches are labelled "Start <name> with Windows".

- [ ] **Step 1: Write the failing tests**

`crates/app/src/startup.rs` tests (implementation in Step 3):
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use negotium_actions::startup::registry::{self, Hive, RegKey};
    use negotium_actions::startup::StartupSource;

    fn entry(source: StartupSource, key: &str, name: &str, enabled: bool, changeable: bool) -> StartupEntry {
        StartupEntry {
            source, key: key.into(), name: name.into(), publisher: None, enabled, changeable,
            command: format!(r"C:\Apps\{key}.exe"), location: "x".into(),
        }
    }

    #[test]
    fn rows_are_sorted_by_name_and_explain_what_cannot_change() {
        let entries = vec![
            entry(StartupSource::UserRun, "zoom", "Zoom", true, true),
            entry(StartupSource::MachineRun, "old", "old tool", false, true),
            entry(StartupSource::Packaged, "pfn\\task", "Contoso", true, false),
        ];
        let rows = startup_rows(&entries, false);
        assert_eq!(rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["Contoso", "old tool", "Zoom"]);
        assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), [2, 1, 0], "ids point back into the entry list");
        assert_eq!((rows[0].can_change, rows[0].note.as_str()), (false, "Set by policy"));
        assert_eq!((rows[1].can_change, rows[1].note.as_str(), rows[1].status.as_str()), (false, "Needs admin", "Disabled"));
        assert_eq!((rows[2].can_change, rows[2].note.as_str(), rows[2].status.as_str()), (true, "", "Enabled"));
        assert_eq!(rows[1].location, "Registry (all users)");
        assert!(needs_admin_banner(&entries, false));
        let elevated = startup_rows(&entries, true);
        assert!(elevated[1].can_change && elevated[1].note.is_empty());
        assert!(!needs_admin_banner(&entries, true));
    }

    struct Sandbox(RegKey);

    impl Drop for Sandbox {
        fn drop(&mut self) {
            let _ = registry::delete_test_tree(&self.0);
        }
    }

    #[test]
    fn a_failed_toggle_reports_inline_and_the_list_shows_the_real_state() {
        let base = RegKey::new(Hive::CurrentUser, &format!(r"{}\app-toggle-{}", registry::TEST_ROOT, std::process::id()));
        let _sandbox = Sandbox(base.clone());
        let roots = StartupRoots::under(&base.path);
        registry::set_dword(&roots.packaged.child(r"Contoso.Locked_abc\Boot"), "State", 3).unwrap();
        registry::set_string(&roots.user_run, "Tool", r"C:\Nope\tool.exe").unwrap();
        let (entries, _) = list_startup(&roots);
        let locked = entries.iter().find(|e| e.key.starts_with("Contoso.Locked")).unwrap().clone();
        let (after, _, failure) = toggle_job(&roots, &locked, true, false);
        assert_eq!(failure.as_deref(), Some("Couldn't change Contoso.Locked: its state is set by a policy."));
        assert!(!after.iter().find(|e| e.key == locked.key).unwrap().enabled, "still disabled");
        let tool = entries.iter().find(|e| e.key == "Tool").unwrap().clone();
        let (after, _, failure) = toggle_job(&roots, &tool, false, false);
        assert_eq!(failure, None);
        assert!(!after.iter().find(|e| e.key == "Tool").unwrap().enabled);
    }
}
```

`crates/app/tests/ui.rs` (add `StartupData` to the `negotium::` import):
```rust
// ---- Plan 3b: startup ----

fn startup_row(id: i32, name: &str, enabled: bool, can_change: bool, note: &str) -> StartupData {
    StartupData {
        id, name: name.into(), publisher: "Contoso".into(), status: if enabled { "Enabled" } else { "Disabled" }.into(),
        location: "Registry (you)".into(), command: format!(r"C:\Apps\{name}.exe"), enabled, can_change, note: note.into(),
    }
}

#[test]
fn startup_rows_show_their_state_and_report_toggles() {
    let ui = shown_window();
    ui.set_page(2);
    ui.set_startup_rows(ModelRc::new(VecModel::from(vec![
        startup_row(0, "Discord", true, true, ""),
        startup_row(1, "Old tool", false, false, "Needs admin"),
    ])));
    let toggled = Rc::new(RefCell::new(Vec::new()));
    let t = toggled.clone();
    ui.on_toggle_startup(move |id, on| t.borrow_mut().push((id, on)));
    let discord = switch(&ui, "Start Discord with Windows");
    assert_eq!(discord.accessible_checked(), Some(true));
    discord.invoke_accessible_default_action();
    assert_eq!(*toggled.borrow(), [(0, false)]);
    let old = switch(&ui, "Start Old tool with Windows");
    assert_eq!(old.accessible_enabled(), Some(false), "an all-users entry without admin can't be switched");
    old.invoke_accessible_default_action();
    assert_eq!(toggled.borrow().len(), 1);
    assert!(El::find_by_accessible_label(&ui, "Needs admin").next().is_some());
}

#[test]
fn the_startup_admin_banner_offers_restart_inline() {
    let ui = shown_window();
    ui.set_page(2);
    ui.set_startup_admin_banner(true);
    let restarted = Rc::new(RefCell::new(false));
    let r = restarted.clone();
    ui.on_restart_admin(move || *r.borrow_mut() = true);
    El::find_by_accessible_label(&ui, "Restart as admin").next().expect("inline button").invoke_accessible_default_action();
    assert!(*restarted.borrow());
}

#[test]
fn the_bridge_shows_startup_entries_and_a_pending_toggle() {
    use negotium_actions::startup::{StartupEntry, StartupSource};
    use slint::Model;
    let (ui, _controls, bridge) = bridged();
    let entry = |key: &str, name: &str, source| StartupEntry {
        source, key: key.into(), name: name.into(), publisher: None, enabled: true, changeable: true,
        command: String::new(), location: String::new(),
    };
    let entries = vec![entry("z", "Zoom", StartupSource::UserRun), entry("a", "Audio", StartupSource::MachineRun)];
    bridge.borrow_mut().set_startup(&ui, entries, false);
    let rows = ui.get_startup_rows();
    assert_eq!(rows.row_data(0).unwrap().name, "Audio");
    assert!(ui.get_startup_admin_banner());
    assert!(!ui.get_startup_loading());
    assert_eq!(bridge.borrow().startup_entry(0).unwrap().key, "z");
    assert_eq!(bridge.borrow().startup_entry(7), None);
    bridge.borrow().show_startup_pending(0, false);
    let zoom = (0..rows.row_count()).map(|i| rows.row_data(i).unwrap()).find(|r| r.name == "Zoom").unwrap();
    assert!(!zoom.enabled, "the switch moves at once; the re-read shows the real state");
}

#[test]
fn the_rail_reaches_startup() {
    let ui = window();
    let picked = Rc::new(RefCell::new(None));
    let p = picked.clone();
    ui.on_select_page(move |i| *p.borrow_mut() = Some(i));
    El::find_by_accessible_label(&ui, "Startup page").next().expect("rail item").invoke_accessible_default_action();
    assert_eq!(*picked.borrow(), Some(2));
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p negotium`
Expected: FAIL to compile (no `startup` module, `StartupData`, `set_startup_rows`, …).

- [ ] **Step 3: Implement**

`crates/app/src/startup.rs` (above the tests):
```rust
//! Startup page view model (spec §5.3): name, publisher, status and location, sorted by name;
//! and the toggle job run on an action thread.
use negotium_actions::startup::{StartupEntry, StartupRoots, list_startup, set_startup_enabled};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct StartupRow {
    /// Index of the entry in the list the row came from.
    pub id: i32,
    pub name: String,
    pub publisher: String,
    pub status: String,
    pub location: String,
    pub command: String,
    pub enabled: bool,
    /// The switch works: not fixed by policy, and elevated when the entry is for all users.
    pub can_change: bool,
    /// Why it can't change ("Set by policy", "Needs admin"), or empty.
    pub note: String,
}

pub fn startup_rows(entries: &[StartupEntry], elevated: bool) -> Vec<StartupRow> {
    let mut rows: Vec<StartupRow> = entries
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let needs_admin = e.source.needs_admin() && !elevated;
            StartupRow {
                id: i as i32,
                name: e.name.clone(),
                publisher: e.publisher.clone().unwrap_or_default(),
                status: if e.enabled { "Enabled" } else { "Disabled" }.to_string(),
                location: e.source.label().to_string(),
                command: e.command.clone(),
                enabled: e.enabled,
                can_change: e.changeable && !needs_admin,
                note: if !e.changeable {
                    "Set by policy"
                } else if needs_admin {
                    "Needs admin"
                } else {
                    ""
                }
                .to_string(),
            }
        })
        .collect();
    rows.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.id.cmp(&b.id)));
    rows
}

/// The inline "Restart as admin" banner: some entry could change, but only with admin rights.
pub fn needs_admin_banner(entries: &[StartupEntry], elevated: bool) -> bool {
    !elevated && entries.iter().any(|e| e.source.needs_admin() && e.changeable)
}

/// Runs on an action thread: toggles, then re-reads the list so the page shows the real state
/// either way. Returns (entries, unreadable-list lines, the inline failure message).
pub fn toggle_job(
    roots: &StartupRoots, entry: &StartupEntry, enabled: bool, elevated: bool,
) -> (Vec<StartupEntry>, Vec<String>, Option<String>) {
    let failure = set_startup_enabled(roots, entry, enabled).err().map(|e| e.message(&entry.name, elevated));
    let (entries, errors) = list_startup(roots);
    (entries, errors, failure)
}
```
`crates/app/src/lib.rs`: add `pub mod startup;`.

`crates/app/src/bridge.rs` — `Bridge` gains `startup: Vec<StartupEntry>` and `startup_rows: Rc<VecModel<StartupData>>` (set on the window in `install`), plus:
```rust
    pub fn set_startup(&mut self, ui: &MainWindow, entries: Vec<StartupEntry>, elevated: bool) {
        let rows = crate::startup::startup_rows(&entries, elevated);
        sync_model(&*self.startup_rows, rows.iter().map(|r| StartupData {
            id: r.id, name: r.name.as_str().into(), publisher: r.publisher.as_str().into(), status: r.status.as_str().into(),
            location: r.location.as_str().into(), command: r.command.as_str().into(), enabled: r.enabled,
            can_change: r.can_change, note: r.note.as_str().into(),
        }).collect());
        ui.set_startup_admin_banner(crate::startup::needs_admin_banner(&entries, elevated));
        ui.set_startup_loading(false);
        self.startup = entries;
    }

    /// The entry behind a row id, as last listed.
    pub fn startup_entry(&self, id: i32) -> Option<StartupEntry> {
        usize::try_from(id).ok().and_then(|i| self.startup.get(i)).cloned()
    }

    /// Moves a switch at once; the re-read after the toggle then shows the real state.
    pub fn show_startup_pending(&self, id: i32, enabled: bool) {
        for i in 0..self.startup_rows.row_count() {
            if let Some(mut row) = self.startup_rows.row_data(i)
                && row.id == id
            {
                row.enabled = enabled;
                row.status = if enabled { "Enabled" } else { "Disabled" }.into();
                self.startup_rows.set_row_data(i, row);
            }
        }
    }
```
(imports: `use negotium_actions::startup::StartupEntry;` and `StartupData` in the `use crate::{…}` list).

`crates/app/ui/startuppage.slint`:
```slint
import { ListView, Button } from "std-widgets.slint";
import { Theme } from "theme.slint";
import { Toggle } from "settingspage.slint";

export struct StartupData {
    id: int, name: string, publisher: string, status: string, location: string, command: string,
    enabled: bool, can-change: bool, note: string,
}

component HeaderText inherits Text {
    color: Theme.muted;
    font-size: 11px;
}

export component StartupPage inherits VerticalLayout {
    in property <[StartupData]> rows;
    in property <bool> loading;
    in property <bool> admin-banner;
    in property <bool> interactive: true;
    callback toggle(int, bool);
    callback refresh();
    callback restart-admin();
    max-width: 16384px; // see MetersRow: repeaters must not cap the window width
    padding: 12px;
    spacing: 10px;
    HorizontalLayout {
        spacing: 12px;
        Text { text: "Startup apps"; color: Theme.text; font-size: 24px; font-weight: 600; horizontal-stretch: 1; }
        Button { text: "Refresh"; enabled: root.interactive && !root.loading; clicked => { root.refresh(); } }
    }
    // Inline, never a dialog (spec 7).
    if root.admin-banner : Rectangle {
        min-height: 40px;
        border-radius: Theme.radius;
        background: Theme.accent.with-alpha(0.15);
        HorizontalLayout {
            padding: 8px;
            spacing: 8px;
            Text { text: "Apps that start for all users can only be changed with admin rights."; color: Theme.text; vertical-alignment: center; wrap: word-wrap; horizontal-stretch: 1; }
            Button { text: "Restart as admin"; enabled: root.interactive; clicked => { root.restart-admin(); } }
        }
    }
    HorizontalLayout {
        padding-left: 8px;
        padding-right: 8px;
        spacing: 8px;
        HeaderText { text: "Name"; horizontal-stretch: 1; }
        HeaderText { text: "Publisher"; width: 150px; }
        HeaderText { text: "Status"; width: 64px; }
        HeaderText { text: "Location"; width: 160px; }
        Rectangle { width: 110px; }
    }
    if root.loading && root.rows.length == 0 : Text { text: "Reading startup apps…"; color: Theme.muted; }
    if !root.loading && root.rows.length == 0 : Text { text: "No apps start with Windows."; color: Theme.muted; }
    ListView {
        vertical-stretch: 1;
        for r in root.rows : Rectangle {
            height: 48px;
            border-radius: 4px;
            background: ta.has-hover ? Theme.card : transparent;
            accessible-role: list-item;
            accessible-label: r.name + ", " + r.status + ", " + r.location;
            ta := TouchArea { }
            HorizontalLayout {
                padding-left: 8px;
                padding-right: 8px;
                spacing: 8px;
                VerticalLayout {
                    horizontal-stretch: 1;
                    alignment: center;
                    Text { text: r.name; color: Theme.text; font-size: 12px; overflow: elide; }
                    Text { text: r.command; color: Theme.muted; font-size: 10px; overflow: elide; }
                }
                Text { text: r.publisher; width: 150px; color: Theme.text; font-size: 12px; vertical-alignment: center; overflow: elide; }
                Text { text: r.status; width: 64px; color: Theme.text; font-size: 12px; vertical-alignment: center; }
                Text { text: r.location; width: 160px; color: Theme.muted; font-size: 11px; vertical-alignment: center; overflow: elide; }
                HorizontalLayout {
                    width: 110px;
                    spacing: 6px;
                    alignment: end;
                    if r.note != "" : Text { text: r.note; color: Theme.muted; font-size: 10px; vertical-alignment: center; }
                    VerticalLayout {
                        alignment: center;
                        Toggle {
                            label: "Start " + r.name + " with Windows";
                            checked: r.enabled;
                            interactive: root.interactive && r.can-change;
                            toggled(on) => { root.toggle(r.id, on); }
                        }
                    }
                }
            }
        }
    }
}
```

`crates/app/ui/rail.slint` — between the Performance and Settings entries:
```slint
    RailButton {
        label: "Startup";
        icon: "M 10 2 L 10 9 M 5.5 5 A 7 7 0 1 0 14.5 5";
        selected: root.page == 2;
        interactive: root.interactive;
        activated => { root.select(2); }
    }
```

`crates/app/ui/main.slint`:
- `import { StartupPage, StartupData } from "startuppage.slint";`; add `StartupData` to the export list.
- properties/callbacks:
```slint
    in property <[StartupData]> startup-rows;
    in-out property <bool> startup-loading;
    in property <bool> startup-admin-banner;
    callback toggle-startup(int, bool);
    callback refresh-startup();
```
- page host: a `Rectangle { x: 0; y: 0; width: parent.width; height: parent.height; visible: root.page == 2; StartupPage { rows: root.startup-rows; loading: root.startup-loading; admin-banner: root.startup-admin-banner; interactive: !root.modal-open; toggle(id, on) => { root.toggle-startup(id, on); } refresh => { root.refresh-startup(); } restart-admin => { root.restart-admin(); } } }` before the Settings page.
- move the toast: delete `if root.toast != "" : Toast { … }` from the Overview's `VerticalLayout` and add, as the last child of the page host `Rectangle` (so it floats over every page):
```slint
            if root.toast != "" : Toast {
                x: 12px;
                width: parent.width - 24px;
                height: self.preferred-height;
                y: parent.height - self.height - 12px;
                text: root.toast;
                interactive: !root.modal-open;
                dismiss => { root.dismiss-toast(); }
            }
```

`crates/app/src/main.rs`:
- a reusable "read the startup list" action (before the page handlers):
```rust
    // Startup page: listing calls the registry, the file system, COM and WinRT, so it runs on an
    // action thread; the result comes back through the bridge.
    let load_startup = {
        let weak = weak.clone();
        move || {
            let Some(ui) = weak.upgrade() else { return };
            if ui.get_startup_loading() {
                return;
            }
            ui.set_startup_loading(true);
            let weak = weak.clone();
            std::thread::spawn(move || {
                let (entries, errors) = negotium_actions::startup::list_startup(&negotium_actions::startup::StartupRoots::system());
                for e in &errors {
                    warn(e);
                }
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = weak.upgrade() {
                        negotium::bridge::with_bridge(|b| b.set_startup(&ui, entries, elevated));
                    }
                });
            });
        }
    };
```
- in `on_select_page`, after `controls.set_page(page);`: `if page == negotium::controls::Page::Startup { load_startup(); }` (clone `load_startup` into the closure; it is `Clone` because it only captures a `Weak`).
- `ui.on_refresh_startup({ let load_startup = load_startup.clone(); move || load_startup() });`
- toggles:
```rust
    {
        let (b, weak) = (bridge.clone(), weak.clone());
        ui.on_toggle_startup(move |id, on| {
            let Some(entry) = b.borrow().startup_entry(id) else { return };
            b.borrow().show_startup_pending(id, on);
            let Some(ui) = weak.upgrade() else { return };
            ui.set_startup_loading(true);
            let weak = weak.clone();
            std::thread::spawn(move || {
                let roots = negotium_actions::startup::StartupRoots::system();
                let (entries, errors, failure) = negotium::startup::toggle_job(&roots, &entry, on, elevated);
                for e in &errors {
                    warn(e);
                }
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = weak.upgrade() {
                        negotium::bridge::with_bridge(|b| b.set_startup(&ui, entries, elevated));
                        if let Some(message) = failure {
                            ui.set_toast(message.into());
                        }
                    }
                });
            });
        });
    }
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p negotium`
Expected: all pass, warning-free; no `HKCU\Software\Negotium-Test\app-toggle-*` key left.

- [ ] **Step 5: Look at it**

Run the app (not elevated) → Startup. Check the list against Task Manager's Startup apps page: same apps, same Enabled/Disabled. Disable a per-user app that you don't need at logon (or a harmless test entry you created with `reg add HKCU\Software\Microsoft\Windows\CurrentVersion\Run /v NegotiumCheck /d "C:\Windows\System32\PING.EXE -n 1 127.0.0.1"`), press F5 in Task Manager's Startup apps: it shows Disabled too; enable it again in Task Manager and press Refresh in Negotium: Enabled. All-users entries show "Needs admin" with the banner. Remove the test entry afterwards (`reg delete HKCU\Software\Microsoft\Windows\CurrentVersion\Run /v NegotiumCheck /f` and the same name under `…\Explorer\StartupApproved\Run`). Stop the instance.

- [ ] **Step 6: Commit**

```bash
git add crates/app
git commit -m "feat(app): Startup page with enable/disable and the inline admin path"
```

---

### Task 8: Restart as admin returns to the same page

**Files:**
- Modify: `crates/app/src/controls.rs`, `crates/app/src/main.rs`, `crates/actions/src/elevation.rs`
- Test: unit tests in `controls.rs`

**Interfaces:**
- Consumes: `controls::Page` (Task 3), `negotium_actions::restart_as_admin`, `main.rs`'s `begin_exit` and `load_startup`.
- Produces: `Page::{name(self) -> &'static str, from_name(&str) -> Option<Page>}`; `controls::{restart_args(Page) -> String, page_from_args<I: IntoIterator<Item = String>>(I) -> Option<Page>}`. `restart_as_admin` passes `SEE_MASK_NOASYNC`. The UAC wait runs off the UI thread; a second click while a restart is pending does nothing.

- [ ] **Step 1: Write the failing test** (`controls.rs`)

```rust
    #[test]
    fn restart_arguments_carry_the_page() {
        for page in Page::ALL {
            assert_eq!(page_from_args(restart_args(page).split(' ').map(String::from)), Some(page));
        }
        assert_eq!(restart_args(Page::Startup), "--restarted --page=startup");
        assert_eq!(page_from_args(["negotium.exe".to_string(), "--page=SETTINGS".to_string()]), Some(Page::Settings));
        assert_eq!(page_from_args(["--page=nope".to_string()]), None);
        assert_eq!(page_from_args(Vec::<String>::new()), None);
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p negotium --lib controls`
Expected: FAIL to compile (no `restart_args`, `page_from_args`).

- [ ] **Step 3: Implement**

`crates/app/src/controls.rs`:
```rust
impl Page {
    pub fn name(self) -> &'static str {
        match self {
            Page::Overview => "overview",
            Page::Performance => "performance",
            Page::Startup => "startup",
            Page::Settings => "settings",
        }
    }

    pub fn from_name(name: &str) -> Option<Page> {
        Self::ALL.into_iter().find(|p| p.name().eq_ignore_ascii_case(name))
    }
}

/// Arguments for the elevated instance (spec §6): wait for this one to exit, open on `page`.
pub fn restart_args(page: Page) -> String {
    format!("--restarted --page={}", page.name())
}

/// The page asked for with `--page=<name>`, if any.
pub fn page_from_args<I: IntoIterator<Item = String>>(args: I) -> Option<Page> {
    args.into_iter().find_map(|a| a.strip_prefix("--page=").and_then(Page::from_name))
}
```

`crates/actions/src/elevation.rs` — in `restart_as_admin`'s `SHELLEXECUTEINFOW` add `fMask: SEE_MASK_NOASYNC,` (import `SEE_MASK_NOASYNC` from `windows::Win32::UI::Shell`): the caller exits right after, so the launch must complete inside the call.

`crates/app/src/main.rs`:
- after the bridge and handlers exist (and `load_startup` is defined):
```rust
    // A restart (as admin) comes back on the page it left (spec §6).
    if let Some(page) = negotium::controls::page_from_args(std::env::args()) {
        controls.set_page(page);
        ui.set_page(page.index());
        if page == negotium::controls::Page::Startup {
            load_startup();
        }
    }
```
- add, at file level, a UI-thread flag so a second click can't stack a second UAC prompt:
```rust
thread_local! {
    /// A restart-as-admin is waiting on its UAC prompt (UI thread only).
    static RESTART_PENDING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
```
- replace `on_restart_admin`:
```rust
    {
        let (weak, controls) = (weak.clone(), controls.clone());
        ui.on_restart_admin(move || {
            if RESTART_PENDING.with(|p| p.replace(true)) {
                return; // a UAC prompt is already up
            }
            let args = negotium::controls::restart_args(controls.page());
            let (weak, controls) = (weak.clone(), controls.clone());
            // ShellExecuteEx returns only once the UAC prompt is answered: wait off the UI thread.
            std::thread::spawn(move || {
                let ok = negotium_actions::restart_as_admin(&args).is_ok();
                let _ = slint::invoke_from_event_loop(move || {
                    RESTART_PENDING.with(|p| p.set(false));
                    if ok {
                        begin_exit(&controls, &weak);
                    } else if let Some(ui) = weak.upgrade() {
                        ui.set_toast("Restart as admin was cancelled.".into());
                    }
                });
            });
        });
    }
```

- [ ] **Step 4: Run tests**

Run: `cargo test --workspace`
Expected: all pass, warning-free.

- [ ] **Step 5: Look at it**

From a non-elevated start, open Startup and press "Restart as admin": the UAC prompt appears while the window stays responsive (drag it); press No: the toast says it was cancelled. Press it again and accept: the new elevated instance opens on Startup with the all-users switches enabled and no banner. Repeat from Performance (it reopens on Performance). Stop the instance.

- [ ] **Step 6: Commit**

```bash
git add crates/app crates/actions
git commit -m "feat(app): restart as admin returns to the same page without blocking the window"
```

---

### Task 9: Check and document

**Files:**
- Modify: `docs/benchmarks/README.md`
- Test: whole workspace

**Interfaces:**
- Consumes: everything above.
- Produces: the README section "Manual checks: Plan 3b"; no code changes unless a check fails.

- [ ] **Step 1: Run every test and lint**

Run: `cargo test --workspace` then `cargo clippy --workspace --all-targets -- -D warnings`
Expected: all pass, no warnings; `reg query HKCU\Software\Negotium-Test` finds nothing (or only an empty key).

- [ ] **Step 2: Add the README section**

Append to `docs/benchmarks/README.md`:
```markdown
## Manual checks: Plan 3b

1. Startup (not elevated): the list matches Task Manager → Startup apps (names, publishers, Enabled/Disabled). Toggling a per-user entry in either app shows the same state in the other after a refresh.
2. Packaged startup task (e.g. Windows Terminal → Settings → Launch on machine startup): Negotium lists it; disabling it in Negotium shows it disabled in Settings → Apps → Startup, and enabling it there shows it enabled after Refresh in Negotium.
3. All-users entries without admin: switches disabled with "Needs admin" and the inline banner; "Restart as admin" reopens Negotium on Startup with those switches enabled.
4. Settings: each refresh rate visibly changes the update pace; Theme System/Light/Dark switch content, title bar and Mica; with System, flipping Windows' mode follows within a second; Always on top holds above Explorer; with "Keep running in the tray" off, closing the window exits.
5. Settings persist across restarts; replacing `%LOCALAPPDATA%\Negotium\settings.json` with `{` starts Negotium with defaults and leaves `settings.json.bad`.
```

- [ ] **Step 3: Commit**

```bash
git add docs/benchmarks/README.md
git commit -m "docs(bench): Plan 3b manual checks"
```

---

## After this plan

- **Human partner:** run the Plan 3b manual checks, in particular #2 (ruling P3b-1 assumes Windows honours a `State` written directly; if Settings → Apps → Startup disagrees, the fallback is to show packaged tasks read-only with "Change in Settings → Apps → Startup").
- **Plan 4** unchanged: no-prompt admin task (the "Admin mode" row), Ctrl+Shift+Esc redirect (its row), installer, CPU-temperature add-on (its row and the CPU page note), per-app network (SRUM), app icons.
- **Deferred minors deliberately left out** (from the Plan 1 and Plan 2 follow-ups and not touched by this plan; Plan 3a's "After this plan" lists the rest): restart-as-admin has no owner window for the UAC prompt and its UAC-cancel message wording; concurrent action results overwrite the toast (now window-wide); focus is not restored after a modal closes; no Esc test on a show-confirm modal; the accent colour now follows system changes but Mica still flashes on the first frame before it applies; chrome polling timer uncapped if the HWND never appears.
- **New minors this plan accepts:** a settings file with one field of the wrong type loses every setting (set aside whole, by design); the Startup list re-reads on every visit (tens of milliseconds, off the UI thread); package names are looked up per packaged task on each read.

## Self-review (done while writing)

- **Spec coverage.** §5 rail entries → Tasks 3 and 7. §5.3: HKCU/HKLM Run keys (and the 32-bit HKLM key) → Task 4, per-user and all-users Startup folders → Task 5, packaged startup tasks → Task 6; enable/disable through StartupApproved → Tasks 4–5 (packaged: ruling P3b-1, Task 6); name, publisher, status, location, no impact rating → Task 7. §5.4 refresh rate (with §4.2's choices and 2 s hidden), theme system/light/dark, always on top, keep running in the tray (default on) → Tasks 1–3; Plan 4 items shown disabled → Task 3. §6 restart as admin preserving the page → Task 8. §7 inline failures and the inline admin path → Task 7.
- **Placeholder scan.** No TBD/TODO/"similar to"; every code step carries its code.
- **Type consistency.** Checked across tasks: `settings::{Settings, ThemeChoice, Saver, load, save, effective_dark, close_action, refresh_from_index}`, `platform::{apply_theme, system_prefers_dark, ShellEvent::SettingsChanged}`, `controls::{Page::{Startup, Settings}, set_refresh_ms, restart_args, page_from_args}`, `startup::{StartupSource, StartupEntry, StartupRoots::{system, under, with_folders, packaged}, list_startup, set_startup_enabled, registry::*, write_shortcut}`, app `startup::{startup_rows, needs_admin_banner, toggle_job}`, `Bridge::{set_startup, startup_entry, show_startup_pending}`, Slint `settings-*`, `startup-*`, `toggle-startup`, `refresh-startup`, `set-dark`, `on-top`.
- **Review Focus.** Each line has its test in the owning task (Tasks 1, 2, 4, 7).
