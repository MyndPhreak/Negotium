# Installer, updates and system integration — rulings and follow-ups

Plan: `2026-10-17-negotium-installer.md`. Spec: `../specs/2026-10-17-negotium-installer-design.md`.

## Rulings made during execution
- Task 2 was split (2a core, 2b real machine) after a safety classifier stopped the first dispatch.
- `install::current()` and every elevated path take folders from Windows (known folders / GetSystemWindowsDirectoryW), never the environment.
- Hotkey-off and uninstall remove the IFEO value only when it names Negotium; another tool's redirect is left alone (turning ours on replaces it; Settings says so).
- The IFEO value is stored quoted; `parse_ifeo` reads quoted or unquoted.
- Admin mode and startup accept only account SIDs (S-1-5-21-…, S-1-12-1-…).
- Updater: allowed hosts include release-assets.githubusercontent.com; msiexec is started elevated by Negotium (a declined prompt keeps it running); the MSI's SHA-256 is checked against GitHub's digest when present (absent/unknown algorithm → logged, proceeds); elevated downloads use an admin-only `%ProgramData%\Negotium-update` (also created admin-only by the MSI); the MSI is held read-only-shared through the launch.
- Post-update relaunch is non-elevated unless admin mode is on (`--relaunch`: admin task when Ready, else the desktop shell).
- Elevated writes under %LOCALAPPDATA% are hardened (known folder, no reparse points, no rename/rotate, fixed log text); the elevated copy strips SLINT_*/NEGOTIUM_*/GLUTIN_* variables, resets SystemRoot/windir/SystemDrive/PATH, and restricts the DLL search.
- Static CRT; CI fails if the release exe imports the VC++ runtime. The MSI removes our IFEO value with reg.exe as a fallback on uninstall.

## Manual checks before the first release (Windows Sandbox or a spare PC)
- First install with each checkbox; Ctrl+Shift+Esc and Ctrl+Alt+Del → Task Manager with admin mode on and off; "Open Windows Task Manager".
- Admin mode: Start-menu launch lands elevated without a prompt; per-app Network column fills; disable the task → non-elevated, "needs repair", no loop.
- Start with Windows with and without admin mode (no window flash at sign-in).
- In-app update from an older version, elevated and not; relaunch non-elevated without admin mode.
- Uninstall (redirect, task folder, Run value, shortcut removed; settings kept) and an uninstall cancelled at the end.
- Elevated tests: `cargo test -p negotium-actions --lib integration -- --ignored --test-threads=1` (passed 2/2 on the user's machine) and `cargo test -p negotium --lib update::secure -- --ignored`.

## Follow-ups
- Pin GitHub Actions by commit SHA (release.yml first) and add Dependabot; run actionlint.
- Unverified on a real install: msiexec opening the held MSI; server-side AppSearch reading the user's HKCU; the post-InstallFinalize admin-mode UAC prompt; the off-screen tray start; RedirectionGuard against a pre-created update-folder junction.
- Optional hardening: reset ComSpec/PATHEXT/PSModulePath/ProgramFiles/ProgramData/TEMP when elevated; an elevated copy can't find tools on the user's PATH (intended).
- Minor: hung instance + elevated copy slower than 3 s can leave two copies; empty-file race in the guarded write; LocalSystem-step failures log to the system profile; a planted `*.msi.part` link in %TEMP% blocks that version's update; mixed-DPI centring; reason-string plural; `same_path` duplication.
