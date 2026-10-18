# Installer, Updates and System Integration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Install Negotium per-machine from an MSI built and published by GitHub Actions, update it from GitHub Releases, and let the installed copy run as admin without a prompt, open on Ctrl+Shift+Esc and start with Windows.

**Architecture:** Every machine-level change goes through one place: `negotium.exe --integrate <action>` (a new `crates/app/src/integrate.rs`, run elevated via the existing ShellExecuteEx `runas` path when the UI needs it, and by the MSI's custom actions). The actions crate gains the Windows code: IFEO and Run registry values (through the existing default-deny write guard), Task Scheduler tasks under `\Negotium\`. The app learns whether it is the installed copy (`install.rs`); only that copy offers the integration switches and updates. Updates (`update.rs`) read the GitHub Releases API, download the release's MSI and run it. WiX source and two GitHub workflows package and publish.

**Tech Stack:** Rust 2024, `windows` 0.62.2 (Registry, TaskScheduler COM, WinHTTP), Slint 1.18.1, WiX via cargo-wix, GitHub Actions.

**Spec:** `docs/superpowers/specs/2026-10-17-negotium-installer-design.md` (with the admin-mode notes from `2026-10-17-negotium-app-network-design.md`).

## Global Constraints

- Repository: public `MyndPhreak/Negotium`. Creating it, pushing, tagging and publishing happen only after the user's explicit go-ahead at that moment (controller step, never a subagent).
- Install dir `C:\Program Files\Negotium` (per-machine); publisher "MyndPhreak"; Start menu shortcut "Negotium".
- Installer checkboxes, all off by default and shown on a first install only: "Open on Ctrl+Shift+Esc", "Admin mode (no prompt)", "Start with Windows". Upgrades keep the redirect, tasks and Run value; uninstall removes them (not settings/logs).
- Registry writes allowed by the guard: exactly `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options\taskmgr.exe` value `Debugger`, and `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` value `Negotium` (plus the existing allow-list). Values written point only at the installed exe.
- Admin task: folder `\Negotium\`, one task per user named by the user's SID, "Run with highest privileges", action = installed exe; only for accounts in Administrators; a broken task → non-elevated with the banner, logged, Settings "needs repair", never a launch loop.
- Start with Windows: Run value `"<install dir>\negotium.exe" --tray` without admin mode; a logon trigger on the user's admin task with admin mode (and the Run value removed).
- Updates: `GET https://api.github.com/repos/MyndPhreak/Negotium/releases/latest`; only an `.msi` asset of that release whose download URL host is `github.com` or `objects.githubusercontent.com` over HTTPS; startup check on by default (Settings "Check for updates on startup"), manual "Check for updates" always; a newer version → dismissable banner "Update to vX.Y.Z"; accept → download, run `msiexec /i <msi> /passive`, exit.
- Dev builds (not the installed copy): Settings says "Development build"; admin mode, Ctrl+Shift+Esc, Start with Windows and updates disabled with "Install Negotium to use this".
- Tests never touch the user's real settings, log, registry (IFEO, Run), Task Scheduler or the network; integration code is tested against fakes or unique throwaway names under test-only roots; elevated tests are `#[ignore = "needs an elevated terminal"]` and handed to the user.
- Never launch the real negotium.exe window in tests; scratch CARGO_TARGET_DIR outside the repo; never kill the user's Negotium.

## Review Focus

1. **Major upgrade removing the integration:** the old product's removal during an upgrade must not run the cleanup. (Task 7 static test on the `.wxs`: the cleanup custom action is conditioned on `REMOVE="ALL" AND NOT UPGRADINGPRODUCTCODE`; the real upgrade is a manual check in Task 9, since installing needs elevation.)
2. **Ctrl+Shift+Esc when the install is gone or moved:** the redirect must never point at a missing exe. Setting it checks the installed path exists; uninstall removes it before files go (custom action sequencing). (Task 3 test `the_redirect_only_ever_names_the_installed_exe`; Task 7 sequencing test on the `.wxs`.)
3. **Launch loop:** admin mode on, task broken → a non-elevated launch must not keep relaunching. The hand-off passes `--from-task`; an elevated-expected launch that isn't elevated never hands off again. (Task 4 test `a_failed_hand_off_runs_here_and_never_loops`.)
4. **Malicious or odd release data:** a release whose asset URL points elsewhere, a non-semver tag, a draft/prerelease, or an older version → no banner, no download. (Task 6 tests.)
5. **"Open Windows Task Manager" leaving the redirect lifted** if something fails mid-way: the redirect is restored even on error. (Task 5 test with a fake registry.)

---

### Task 1: Installed copy, versions and the Settings rows

**Files:**
- Create: `crates/app/src/install.rs`
- Modify: `crates/app/src/lib.rs`, `crates/app/src/settings.rs`, `crates/app/src/settingspage.rs`, `crates/app/ui/settingspage.slint`, `crates/app/ui/main.slint`
- Test: `install.rs` (unit), `crates/app/tests/ui.rs`

**Interfaces:**
- Produces:
  - `pub struct Install { pub exe: PathBuf, pub installed: bool }`, `pub fn detect(exe: &Path, program_files: &Path) -> Install` (installed when `exe` is `<program_files>\Negotium\negotium.exe`, case-insensitive), `pub fn current() -> Install` (real paths); `pub const VERSION: &str = env!("CARGO_PKG_VERSION")`.
  - Settings fields `check_updates: bool` (default true), `start_with_windows: bool` (default false), plus the state the switches show for admin mode and the redirect is read from the machine (Tasks 3–5), not stored.
  - Settings page rows (replace the three disabled "Coming in a later version" rows for admin mode and Ctrl+Shift+Esc; the CPU temperature row stays as it is): "Admin mode", "Open on Ctrl+Shift+Esc", "Start with Windows", "Check for updates on startup", a "Check for updates" button, a version line "Version X.Y.Z" or "Version X.Y.Z (development build)", and "Open Windows Task Manager". On a dev build the four integration/update rows are disabled with description "Install Negotium to use this". Slint properties/callbacks named `settings-installed`, `settings-admin-mode`, `settings-admin-mode-state` (0 off, 1 on, 2 needs repair, 3 not available on this account), `settings-hotkey`, `settings-start-with-windows`, `settings-check-updates`, `version-text`, callbacks `admin-mode-toggled(bool)`, `hotkey-toggled(bool)`, `start-with-windows-toggled(bool)`, `check-updates-toggled(bool)`, `check-updates-now()`, `open-windows-task-manager()` — wired to no-ops that later tasks fill.
- [ ] **Step 1: Write failing tests:** `detect_recognises_only_the_program_files_exe` (installed path, other case, `target\release`, a lookalike `Negotium2`); settings `new_fields_default_and_load_tolerantly`; ui `a_dev_build_disables_the_integration_rows_and_says_why`, `the_version_line_says_development_build_when_not_installed`.
- [ ] **Step 2:** Run — fail.
- [ ] **Step 3:** Implement; update the existing ui test that counts Settings titles.
- [ ] **Step 4:** `cargo test --workspace` + clippy.
- [ ] **Step 5:** Commit `feat(app): know the installed copy; Settings rows for installing features`.

### Task 2: The integration command and its machine primitives

**Files:**
- Create: `crates/app/src/integrate.rs`, `crates/actions/src/integration/{mod.rs,ifeo.rs,run.rs,tasks.rs}`
- Modify: `crates/actions/src/lib.rs`, `crates/actions/src/startup/registry.rs` (allow-list), `crates/actions/Cargo.toml` (TaskScheduler/COM features), `crates/app/src/main.rs` (dispatch `--integrate` before any window)

**Interfaces:**
- Produces:
  - `pub enum Action { HotkeyOn, HotkeyOff, AdminOn { logon: bool }, AdminOff, StartupOn, StartupOff, OpenTaskManager, Uninstall }` with `fn parse(args) -> Option<Action>` / `fn to_args(&self) -> Vec<String>` (`--integrate hotkey-on`, …, `--integrate admin-on --logon`).
  - `pub fn run(action: Action, ctx: &Ctx) -> Result<(), IntegrateError>` where `Ctx { exe: PathBuf, user_sid: String, machine: &dyn Machine }`; `trait Machine` (fake in tests, real impl in actions): `set_ifeo(Option<&Path>)`, `ifeo() -> Option<PathBuf>`, `set_run(Option<&str>)`, `run_value() -> Option<String>`, `create_task(sid, exe, logon: bool)`, `delete_task(sid)`, `task_state(sid) -> TaskState { Missing, Ready, Disabled, Broken }`, `delete_task_folder()`, `start_task(sid, args)`.
  - `main`: `negotium.exe --integrate …` runs the action without any UI and exits with 0 / non-zero (and logs to the normal log).
  - Real `Machine` in `negotium_actions::integration` using the registry guard (IFEO `Debugger`, HKCU Run `Negotium`) and Task Scheduler COM (folder `\Negotium\`, task named by SID, highest privileges, `InteractiveToken` logon for the user, optional logon trigger for that user).
- [ ] **Step 1: Write failing tests** (fake Machine): every Action's effects; `the_redirect_only_ever_names_the_installed_exe` (refuses when the exe doesn't exist or isn't installed); `uninstall_removes_redirect_run_value_and_every_task`; `args_round_trip`. Guard: `only_the_two_new_values_are_writable` (other IFEO images, other Run values refused). Elevated `#[ignore]`: create/delete a task in a unique test folder `\Negotium-test-<pid>\`; set/clear IFEO for a fake image name `negotium-test-<pid>.exe` (never `taskmgr.exe`).
- [ ] **Step 2:** Run — fail.
- [ ] **Step 3:** Implement. Make the real Machine's roots injectable (task folder, IFEO image name) so elevated tests never touch the real ones.
- [ ] **Step 4:** `cargo test --workspace` + clippy.
- [ ] **Step 5:** Commit `feat: one elevated integration command for redirect, admin task and startup`.

### Task 3: Ctrl+Shift+Esc and "Open Windows Task Manager"

**Files:** Modify `crates/app/src/settingspage.rs`, `crates/app/src/main.rs`, `crates/app/ui/tray.slint`, `crates/app/src/integrate.rs`; Test `integrate.rs`, `crates/app/tests/ui.rs`

**Interfaces:**
- Consumes: `Action::HotkeyOn/Off/OpenTaskManager`, `Machine` (Task 2), Settings rows (Task 1).
- Produces: Settings "Open on Ctrl+Shift+Esc" shows `machine.ifeo() == Some(installed exe)`; toggling runs `negotium.exe --integrate hotkey-on|off` elevated (existing `restart_as_admin`-style runas helper, generalised as `run_elevated(args) -> Result<(), ActionError>`; a cancelled UAC changes nothing and the switch snaps back). Launched with a trailing `taskmgr.exe` path argument (IFEO passes the original command line), Negotium ignores it. `OpenTaskManager`: clear the value, start `%SystemRoot%\System32\Taskmgr.exe`, wait until its process exists (≤ 5 s), restore the value — restore runs even on error. Tray menu gets "Open Windows Task Manager" (installed copy with the redirect on only).
- [ ] **Step 1: Write failing tests:** `the_taskmgr_argument_is_ignored`; `opening_task_manager_restores_the_redirect_even_on_error` (fake Machine whose start fails); ui `the_hotkey_switch_shows_the_machine_state`.
- [ ] **Step 2–4:** fail → implement → `cargo test --workspace` + clippy.
- [ ] **Step 5:** Commit `feat(app): open on Ctrl+Shift+Esc, and a way back to Windows Task Manager`.

### Task 4: Admin mode and the launch hand-off

**Files:** Modify `crates/app/src/main.rs`, `crates/app/src/settingspage.rs`, `crates/app/src/integrate.rs`; Test `integrate.rs` (unit), `crates/app/tests/ui.rs`

**Interfaces:**
- Consumes: `Action::AdminOn/Off`, `Machine::task_state/start_task` (Task 2).
- Produces:
  - `pub fn launch_plan(installed: bool, elevated: bool, admin_member: bool, task: TaskState, from_task: bool) -> Launch { RunHere, HandOff }` — HandOff only when installed, not elevated, an admin member, task Ready, and not `--from-task`.
  - main: on HandOff, start the task (passing the page and `--from-task`), then exit; if starting fails, run here (log once). The task's action passes `--from-task`.
  - Settings "Admin mode": state from `task_state` (Ready → on; Missing → off; Disabled/Broken → "needs repair", description "Turn on again to repair"; not an Administrators member → disabled "Needs an administrator account"); toggling runs `--integrate admin-on [--logon if start_with_windows]` / `admin-off` elevated.
- [ ] **Step 1: Write failing tests:** the launch_plan truth table incl. `a_failed_hand_off_runs_here_and_never_loops` (from_task = true while not elevated → RunHere); ui `admin_mode_needs_repair_when_the_task_is_broken`, `a_standard_account_cannot_turn_on_admin_mode`.
- [ ] **Step 2–4:** fail → implement → `cargo test --workspace` + clippy.
- [ ] **Step 5:** Commit `feat(app): admin mode starts every launch elevated without a prompt`.

### Task 5: Start with Windows

**Files:** Modify `crates/app/src/main.rs` (`--tray` starts hidden), `crates/app/src/settingspage.rs`, `crates/app/src/integrate.rs`; Test `integrate.rs`, `crates/app/tests/ui.rs`

**Interfaces:**
- Consumes: `Action::StartupOn/Off`, `AdminOn { logon }` (Task 2), admin mode (Task 4).
- Produces: `StartupOn` → admin mode on: recreate the user's task with a logon trigger and remove the Run value; off: set the Run value `"<exe>" --tray`. `StartupOff` → remove the logon trigger (recreate without) and the Run value. Turning admin mode on/off while Start with Windows is on moves the startup between the two forms. The Run value is HKCU, so StartupOn/Off without admin mode need no UAC (run in-process, not elevated). `--tray` launches start hidden in the tray (the existing hidden/tray path).
- [ ] **Step 1: Write failing tests:** `startup_uses_the_run_value_without_admin_mode_and_the_task_with_it`; `switching_admin_mode_moves_startup_between_forms`; `tray_argument_starts_hidden` (unit on the arg parse / initial visibility decision).
- [ ] **Step 2–4:** fail → implement → `cargo test --workspace` + clippy.
- [ ] **Step 5:** Commit `feat(app): start with Windows, hidden in the tray`.

### Task 6: Updates from GitHub Releases

**Files:** Create `crates/app/src/update.rs`; Modify `crates/app/src/main.rs`, `crates/app/src/settingspage.rs`, `crates/app/ui/main.slint` (banner), `crates/app/ui/overlays.slint`, `crates/actions/Cargo.toml` or `crates/app/Cargo.toml` (WinHTTP feature); Test `update.rs`, `crates/app/tests/ui.rs`

**Interfaces:**
- Produces:
  - `pub struct Release { pub version: semver-like (u64,u64,u64), pub msi_url: String, pub page_url: String }`; `pub fn parse_latest(json: &str, current: &str) -> Result<Option<Release>, UpdateError>` — `Some` only for a non-draft, non-prerelease release whose `tag_name` is `vX.Y.Z` newer than `current` with exactly one `.msi` asset on an allowed host (spec list) over HTTPS.
  - `trait Fetch { fn get(&self, url: &str) -> Result<Vec<u8>, UpdateError> }`, real impl over WinHTTP with a `User-Agent: Negotium/<version>` header and a 10 s timeout; `pub fn check(fetch: &dyn Fetch, current: &str) -> Result<Option<Release>, UpdateError>`; `pub fn download(fetch, release, dir) -> Result<PathBuf, UpdateError>` (to `%TEMP%\Negotium-update\Negotium-X.Y.Z.msi`).
  - Startup check (installed copy, setting on) on a background thread after the window shows; banner "Update to vX.Y.Z" with "Update" and "Dismiss"; Settings "Check for updates" shows "Checking…", then "You're up to date", the banner, or "Couldn't check for updates". "Update": download, run `msiexec /i "<msi>" /passive` (UAC by msiexec), then the app exits through `begin_exit`.
- [ ] **Step 1: Write failing tests** (fake Fetch, JSON fixtures): newer/older/equal, prerelease, draft, `v1.2` / `latest` tags, asset on a foreign host, `http://`, two MSI assets, missing asset, malformed JSON; `the_startup_check_never_runs_on_a_dev_build`; ui `the_update_banner_offers_the_version`.
- [ ] **Step 2–4:** fail → implement → `cargo test --workspace` + clippy.
- [ ] **Step 5:** Commit `feat(app): check GitHub Releases and update from the release's installer`.

### Task 7: The MSI

**Files:** Create `wix/main.wxs` (cargo-wix layout), `crates/app/Cargo.toml` `[package.metadata.wix]` as needed; Create `crates/app/tests/wix.rs` (static checks on the `.wxs`); Modify `README`/docs as needed.

**Interfaces:**
- Consumes: `negotium.exe --integrate …` (Tasks 2–5).
- Produces: per-machine MSI, `ProgramFiles64Folder\Negotium`, Start menu shortcut, ARP publisher "MyndPhreak", a fixed UpgradeCode (generate once, record in the `.wxs`), `MajorUpgrade` (downgrades blocked with a message). A first-install dialog page with the three checkboxes (properties `HOTKEY`, `ADMINMODE`, `STARTUP`, default unset). Deferred, impersonate-appropriate custom actions after InstallFiles running `[INSTALLFOLDER]negotium.exe --integrate …` for each ticked box, only on a first install (not when an older version is being upgraded — e.g. `NOT Installed AND NOT WIX_UPGRADE_DETECTED`); `--integrate uninstall` before RemoveFiles only when `REMOVE="ALL" AND NOT UPGRADINGPRODUCTCODE`; a running Negotium is closed first (CloseApplication or a custom action sending it the existing exit path).
- [ ] **Step 1: Write failing tests** (`wix.rs`, parse the `.wxs` as XML): the UpgradeCode is fixed and a GUID; the uninstall action's condition contains `REMOVE="ALL"` and `NOT UPGRADINGPRODUCTCODE`; the uninstall action is sequenced before `RemoveFiles`; install actions run after `InstallFiles`; the three checkbox properties default unset.
- [ ] **Step 2:** Run — fail.
- [ ] **Step 3:** Author the `.wxs`; build the MSI locally (`cargo wix` — unelevated build works; record the command and output). Installing is the user's manual check.
- [ ] **Step 4:** `cargo test --workspace` + clippy.
- [ ] **Step 5:** Commit `feat: an MSI for per-machine installs`.

### Task 8: GitHub workflows

**Files:** Create `.github/workflows/ci.yml`, `.github/workflows/release.yml`; Test: `actionlint` if available, else a careful read (no network runs).

**Interfaces:**
- `ci.yml`: on push and pull request: `windows-latest`, stable Rust, cache, `cargo test --workspace` (UI tests run headless with the testing backend), `cargo clippy --workspace --all-targets -- -D warnings`.
- `release.yml`: on tags `v*.*.*`: check the tag equals `v` + the workspace version (fail otherwise), build release, install WiX + cargo-wix, `cargo wix -p negotium --nocapture`, create the GitHub Release `vX.Y.Z` (not draft, not prerelease) with the MSI as the single `.msi` asset (`gh release create` with `GITHUB_TOKEN`, `permissions: contents: write`).
- [ ] **Step 1:** Write both workflows; run `actionlint` if installable locally (else note it).
- [ ] **Step 2:** Commit `ci: test on push and publish an MSI release on version tags`.

### Task 9: Publish (controller, with the user's go-ahead) and manual checks

- [ ] **Step 1:** Ask the user before each outward step: create `MyndPhreak/Negotium` (public) with `gh repo create`, add the remote, push `main`; then (after CI passes) bump the version if needed and push tag `v0.2.0` to publish the first release.
- [ ] **Step 2:** Hand the user the manual checks (record in `docs/benchmarks/README.md` or a new `docs/install.md`): install with each checkbox; Ctrl+Shift+Esc opens Negotium (cold and from the tray); "Open Windows Task Manager"; admin mode: Start menu launch lands elevated, the per-app Network column fills, no prompt; break the task (disable it in Task Scheduler) → non-elevated with the banner and "needs repair", no loop; Start with Windows with and without admin mode; update from an older installed version via the banner; uninstall removes the redirect, the task folder, the Run value and the Start menu shortcut while settings stay.
- [ ] **Step 3:** Commit the docs.
