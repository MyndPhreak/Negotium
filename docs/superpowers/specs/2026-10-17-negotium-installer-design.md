# Negotium — Installer, updates, admin mode and Ctrl+Shift+Esc (design)

Date: 2026-10-17. Second Plan 4 project (the last: the CPU temperature add-on). Amends spec §6
(elevation and system integration) and §10 (packaging and distribution) of
`2026-10-14-negotium-design.md`. The admin-mode behaviour agreed while designing per-app network
(`2026-10-17-negotium-app-network-design.md`, "Admin mode moves to the installer plan") is built
here.

## Why

Negotium only runs from `target\release` today. The user wants it installed like a real app,
updated from GitHub Releases ("check for updates on startup or manually"), able to run as admin
every launch without a prompt (so per-app network always works), and able to open on
Ctrl+Shift+Esc. Admin mode and Ctrl+Shift+Esc both need an exe only administrators can change,
which only an install to Program Files gives.

## Goals

### Repository and releases

1. A public GitHub repository **MyndPhreak/Negotium** holds the code. Creating it and pushing are
   done only with the user's explicit go-ahead at that step (outward-facing).
2. A GitHub Actions workflow builds and tests on every push, and on a version tag `vX.Y.Z` builds
   the release, packages the MSI, and publishes a GitHub Release `vX.Y.Z` with the MSI attached
   (my call: the user never builds installers by hand). The tag's version must equal the
   workspace version in `Cargo.toml`, or the release job fails.
3. Unsigned for now (SmartScreen warns; "More info → Run anyway"). Code signing is a later item.

### Installer (MSI)

4. Per-machine install to `C:\Program Files\Negotium` (admin-only writable), Start menu shortcut,
   Add/Remove Programs entry with version and publisher "MyndPhreak".
5. Two optional checkboxes on the install dialog, both **off** by default: "Open on Ctrl+Shift+Esc"
   and "Admin mode (no prompt)". The MSI does the same thing the Settings switches do (below).
6. Upgrades in place (a newer MSI replaces the older install; settings in
   `%LOCALAPPDATA%\Negotium` are kept). Uninstall removes the program, the Start menu shortcut,
   the Ctrl+Shift+Esc redirect and the admin-mode scheduled task; it leaves the user's settings and
   logs (my call — the usual Windows convention).
6a. Upgrades keep the machine's integration as it is (my call): the Ctrl+Shift+Esc redirect, the
   admin-mode tasks and "Start with Windows" survive an upgrade (the uninstall steps of goal 6 run
   only on a real uninstall, not the old product's removal during a major upgrade). The
   checkboxes appear only on a first install; the in-app updater runs the MSI with reduced UI
   (progress only, no checkboxes).
7. Installing or upgrading while Negotium runs: the installer asks it to exit (or the updater exits
   it first, below); it never leaves two versions running.

### Updates

8. On startup (and from Settings → "Check for updates") the **installed** Negotium asks GitHub's
   public API for the latest release of MyndPhreak/Negotium. If its version is newer than the
   running one, a dismissable banner offers "Update to vX.Y.Z". A Settings switch "Check for
   updates on startup" (default on) turns the startup check off; the manual button always works.
9. Accepting downloads that release's MSI (only an asset on that release of that repository, over
   HTTPS), runs it (one UAC prompt), and Negotium exits so the installer can replace it; the new
   version starts when the install finishes.
10. The check sends nothing about the user or the machine beyond what any HTTPS request to GitHub
    does; failures (offline, rate limit) are silent except a log line and, for the manual check, a
    short inline message ("Couldn't check for updates").

### Admin mode (from the per-app network design)

11. Settings → "Admin mode" (today a disabled "Coming in a later version" row) becomes a working
    switch on the installed copy, with the trade-off stated plainly: anything that can start the
    task gets an elevated Negotium without a prompt.
12. Turning it on shows one UAC prompt and creates a Task Scheduler task, "Run with highest
    privileges", for the current user, starting the installed exe in Program Files. Each user's
    task lives in a `\Negotium\` Task Scheduler folder, named after the user's SID, so uninstall
    removes every user's task by removing that folder (my call). The installer's checkbox creates
    the task for the account running the installer. Turning it off
    deletes the task (a UAC prompt if Windows needs one).
13. With admin mode on, every non-elevated launch of the installed Negotium (Start menu, the
    Ctrl+Shift+Esc redirect, the startup entry) starts the task instead and exits, so the user
    always lands in the elevated copy; the existing one-instance rule still holds.
14. A missing, disabled or failing task: Negotium carries on non-elevated, shows the banner, logs
    why, and Settings shows admin mode as needing repair (turning it on again recreates it). No
    dialogs and never a launch loop.
15. Only for accounts in the Administrators group; on a standard account the switch is disabled
    with a short note why.

### Start with Windows (the user's decision)

13a. Settings and the installer get "Start with Windows" (off by default). Negotium then starts at
     sign-in hidden in the tray (ready for Ctrl+Shift+Esc). Without admin mode it is a per-user
     Run entry (`HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, value `Negotium`, the
     installed exe with a `--tray` argument; the registry write guard's allow-list gains exactly
     this value). With admin mode on, the admin task gets a logon trigger instead (and the Run
     entry is removed), so sign-in starts the elevated copy directly with no hand-off. Uninstall
     removes the Run value. Installed copy only.

### Ctrl+Shift+Esc

16. Settings → "Open on Ctrl+Shift+Esc" (and the installer checkbox) sets the Image File Execution
    Options `Debugger` value for `taskmgr.exe` to the installed Negotium (the Process Explorer
    mechanism; machine-wide, one UAC prompt). Every way of starting Task Manager then opens
    Negotium (Ctrl+Shift+Esc, the taskbar menu, Ctrl+Alt+Del).
17. Launched this way, Negotium ignores the `taskmgr.exe` path it receives and behaves like a normal
    launch: an already-running instance comes forward; admin mode applies.
18. Settings and the tray menu get "Open Windows Task Manager", which briefly lifts the redirect to
    start the real Task Manager, then restores it (needs elevation to lift it).
19. Turning it off or uninstalling removes the value, restoring Windows' own Task Manager.
20. Only the installed copy can set it (a dev build shows the row disabled with "Install Negotium
    to use this").

### Dev builds

21. Builds run from `target\release` keep working as today; admin mode, Ctrl+Shift+Esc and updates
    are for the installed copy only, and a dev build's Settings says "Development build" with those
    rows disabled.

## Non-goals

- Code signing, winget/Store publishing, a portable zip, delta updates, a beta channel.
- Silent background updates (the user always clicks "Update").

## Anti-goals

- A Ctrl+Shift+Esc redirect pointing at an exe that can be missing or user-writable (Task Manager
  must never stop opening): only the installed Program Files path is ever written, and uninstall
  always removes it.
- A scheduled task that starts anything other than the installed, admin-only-writable exe.
- Downloading or running anything not attached to a MyndPhreak/Negotium GitHub release.
- Any registry or Task Scheduler write in tests against the real `taskmgr.exe` IFEO key or the
  real task name.

## Constraints

- Rust + `windows` 0.62.2; the existing default-deny registry write guard (`allow_system_writes()`
  only in main; allow-listed keys) is extended for exactly two values: the IFEO `taskmgr.exe`
  `Debugger` value and the HKCU Run value `Negotium`.
- The bench refuses to run while negotium.exe runs; the installed copy and dev builds share one
  instance mutex — the builder decides whether a dev build and the installed copy may run at once
  (a sensible default: same rule, one at a time).
- Tests never touch the user's real settings, log, registry (IFEO, Run), Task Scheduler or network;
  updater tests use a fake release source. Elevated tests are `#[ignore]` and handed to the user.
- Nothing is pushed to GitHub or published without the user's go-ahead at that moment.

## Left to the builder

- MSI tooling (cargo-wix / WiX v3 or WiX v4+ in CI), the upgrade code, UI flow for the two
  checkboxes, how the MSI runs the admin-mode and IFEO steps (custom actions calling Negotium with
  a flag, or WiX elements).
- Detecting "installed" (exe path under `%ProgramFiles%\Negotium` and the install's registry entry).
- The task's definition (Task Scheduler COM or `schtasks`), name, and how arguments (page) pass
  through it; how a non-elevated launcher brings an elevated window forward (UIPI).
- The HTTP client (WinHTTP via `windows`, or a small crate), version comparison (semver), and how
  the downloaded MSI is stored and launched.
- The workflow's exact steps and caching.
