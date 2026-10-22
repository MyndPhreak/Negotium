# Negotium — Pick a window to end (design)

Date: 2026-10-21. A new feature on top of the actions in spec §5 of `2026-10-14-negotium-design.md`
(end task, force close) and the tray of the same spec. Mockups from the brainstorm are in
`.superpowers/brainstorm/7452-1792638195/content/overlay-style.html` (not committed). The chosen
look is style **A (spotlight)** with the **dashed outline from style C**.

## Why

When an app freezes, Windows Task Manager often stays hidden behind the frozen window or does not
come up at all. Then you have to find the app in a list by name. The user wants to *point at the
bad window* and end it, like `xkill` on Linux. Neither Task Manager nor Negotium can do that today.

Success: a window freezes, the user presses one shortcut (or uses the tray menu), clicks the frozen
window, and the whole app is gone, without opening Negotium's main window.

## Goals

### Starting the picker

1. **Tray menu.** A new item **"End a window…"** in the tray icon's right-click menu
   (`crates/app/ui/tray.slint`) starts the picker. It is always shown (my call: it does nothing until
   the user clicks it, so it needs no setting).
2. **Global shortcut.** **Ctrl+Alt+Shift+Esc** starts the picker from anywhere, also when the main
   window is hidden in the tray (my call on the key). It is **off by default** and turned on by a new
   switch in Settings, **"Pick a window to end on Ctrl+Alt+Shift+Esc"**. This is the feature flag
   (golden rule 7): with the switch off, Negotium registers no shortcut and behaves exactly as it does
   today.
3. The switch works in a development build too; unlike "Open on Ctrl+Shift+Esc" it needs no install,
   because it only needs `RegisterHotKey`, not a machine-wide registry value.
4. If Windows refuses the shortcut (another program owns it), the failure is never silent (my
   calls):
   - The saved setting **stays on**. Negotium tries again at the next start, so one boot where
     another program took the key first does not lose the user's choice.
   - The Settings row shows *"Another program uses Ctrl+Alt+Shift+Esc"* while the shortcut is not
     registered. This is runtime state, separate from the stored setting.
   - When the failure happens at start-up with no window on screen, the tray icon shows a
     notification with the same text, once per start.
5. Naming: in the code, "hotkey" already means the Ctrl+Shift+Esc IFEO redirect
   (`Action::HotkeyOn`, `Integrations::toggle_hotkey`). The new shortcut gets a different name,
   for example `picker_shortcut`, so the two are not confused.
6. Pressing the shortcut while the picker is already open does nothing.

### The overlay

7. The overlay covers **every monitor**, above all other windows, including always-on-top ones.
8. Everything is dimmed (about 55% black) except the **target window**: the top-level window under
   the cursor. The target stays at full brightness and gets a **dashed outline** around its visible
   frame.
9. The outline is **red** when the target app is not responding and **accent blue** when it is
   responding (my call).
10. A **label** next to the target shows: the app name, the number of processes, the memory use,
    and "Not responding" when that applies. When Negotium cannot end the app without admin rights,
    the label says so before the user clicks (goal 19).
11. A **hint bar** at the top of the monitor with the cursor says:
    *Click a window to end it · Shift+click: end now · Esc: cancel*.
12. The cursor is a crosshair while the picker is open.
13. The spotlight follows the cursor at once. Nothing animates, so the overlay redraws only when the
    target changes.
14. Windows that the picker **never targets** (no highlight, and a click does nothing):
    - the desktop, the taskbar, the Start menu and other shell surfaces. The rule (my call) is
      both: windows of the shell process (`GetShellWindow`'s process, normally `explorer.exe`)
      and shell window classes (`Shell_TrayWnd`, `Shell_SecondaryTrayWnd`, `Progman`, `WorkerW`).
      Anything in Negotium's `Windows` app group is also skipped. (`explorer.exe` itself is in an
      `AppKey::Window` group, not in `Windows`, so the group alone is not enough.) File Explorer
      folder windows: see goal 14a;
    - Negotium's own windows;
    - cloaked windows (on another virtual desktop, or hidden by DWM), minimised windows and
      windows with no title.

    Over such a place the screen stays fully dimmed and the label says
    *"Windows needs this — you cannot end it here"* for shell surfaces, or shows nothing.

14a. **File Explorer folder windows** (class `CabinetWClass`) are targeted (the user's decision):
    - If the folder window runs in its **own process** (Windows' "Launch folder windows in a
      separate process" option), the picker ends only that process. The taskbar is not touched.
    - If it runs in the **shell process** (the usual case), the confirm dialog always shows, also on
      Shift+click (my call), and warns: *"This also restarts the taskbar and the desktop. All File
      Explorer windows close."* After the shell process ends, Negotium starts `explorer.exe` again.
      When Negotium runs as admin, Explorer must start **without** admin rights (my call; for
      example through the shell's own token, taken before the shell ends, not a plain `CreateProcess` from the elevated
      process). If Explorer does not start again, the result message says so and tells the user
      to press Ctrl+Shift+Esc and run `explorer.exe`.
    - The label for such a window says *"File Explorer — restarts the taskbar"*.

### Ending the app

15. A **click** on the target shows a **confirm dialog** above the overlay:
    *"End Discord? This closes 6 processes. You lose unsaved work."* with **End** and **Cancel**,
    where Cancel is the default button (my call: the click was quick, the default must be safe).
16. **Shift+click** ends the app at once with no dialog.
17. "End" means the **whole app**: every process in the app group that Negotium already builds
    (`negotium_model::grouping`), ended through the existing `"end-task"` path
    (`dispatch::plan` → `run`). There is no new kill code.
18. After the app ends (or does not), the overlay closes and the user sees the result ("Ended 6 of
    6 processes" or the failure). The existing toast is inside the main window, which is usually
    hidden at this moment, so where it shows is left to the builder (below).
19. **Admin rights.** If Negotium is not elevated and the target process cannot be opened with
    terminate rights, the label says *"Needs admin — restart Negotium as admin"*. A click then
    shows the same text in the dialog with only a Close button. Shift+click does nothing.
20. **Cancel.** Esc or a right-click closes the picker with no change. Cancelling the confirm
    dialog returns to the picker, so the user can pick a different window.
21. **Critical processes** keep their existing protection: if `dispatch::plan` returns `Confirm`
    for a critical process, the dialog shows always, also on Shift+click.

## Non-goals

- Ending a single process of an app (a single Chrome tab process). The picker always ends the whole
  app; the main window's process list still does single processes.
- Picking a window with the keyboard (Tab through windows). Mouse only, for now.
- Exclusive full-screen games. The overlay cannot draw above an exclusive full-screen surface;
  borderless full-screen works.
- Picking windows on the secure desktop (UAC prompt, Ctrl+Alt+Del screen, lock screen).
- Changing the shortcut to a different key. A fixed key for now; a key picker is a follow-up.

## Anti-goals (a failure even if it works)

- Ending the wrong app. The target that is highlighted when the button goes down is the target that
  is ended, and its identity (pid + create time) is checked again before ending it.
- Ending the shell (taskbar, desktop) by mistake.
- The overlay getting stuck on screen. Any error closes it, and Esc always works.
- A shortcut that takes CPU or wakes the app when it is not used.

## Constraints

- Windows 10/11 x64, Rust, Slint 1.18 (winit + femtovg on GPU, winit + software on fallback).
  Everything in the app runs on the UI thread under `slint::run_event_loop_until_quit()`. winit
  pumps all Win32 messages on that thread, so native windows created there get their messages.
- The picker must work under **both** renderers, because the software fallback exists for machines
  where the GPU path stalls (`renderer::decide`, watchdog).
- Settings: a new field in `struct Settings` (`crates/app/src/settings.rs`) must also go into the
  `field!` list in `load_with`. Default `false`.
- UI text follows ASD-STE100.
- Test-first commits (`test(app): add failing test for …` then `feat(app): …`).

## How it is built (decided in the brainstorm)

- **Overlay rendering — my call:** a **native Win32 overlay**, one per-pixel-alpha window
  (`WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE`) per monitor, not a Slint
  window. Reasons:
  - Slint has no multi-monitor, per-pixel-transparent, topmost window today, and under the software
    renderer the buffer is opaque (commit f1c6491). A native window behaves the same under both
    renderers.
  - The overlay redraws only when the target changes (goal 13), a few times per second at most, so
    drawing on the CPU is fast enough. **The user's "GPU accelerated if necessary" is answered "not
    necessary".** If profiling shows a slow redraw on large 4K multi-monitor setups, Direct2D into
    the same layered window is the upgrade path.
  - One window per monitor (my call) keeps per-monitor DPI simple.
- **Confirm dialog — my call:** a native `TaskDialogIndirect`, topmost and owned by the overlay.
  The existing Slint `Modal` works only inside the main window. `Win32_UI_Controls` is already a
  feature of the `windows` crate here.
- **Shortcut:** `RegisterHotKey` on a message-only window on the UI thread (`MOD_CONTROL | MOD_ALT |
  MOD_SHIFT | MOD_NOREPEAT`, `VK_ESCAPE`). Registered when the setting turns on or at start-up when it
  is on; unregistered when it turns off.
- **Finding the target:** walk top-level windows in z-order (`EnumWindows`), skip the overlay
  windows, cloaked (`DWMWA_CLOAKED`) and minimised ones, and take the first whose
  `DWMWA_EXTENDED_FRAME_BOUNDS` contains the cursor. Hung state reuses the logic in
  `crates/collector/src/windows_scan.rs` (`IsHungAppWindow`, ghost → original).
- **Window → app — my call:** the main window's rows are not built while Negotium is hidden in the
  tray (`worker.rs:296`), so the picker cannot use them. When the picker opens, it takes **its own
  process snapshot** off the UI thread and runs `negotium_model::grouping::group` on it. Until that
  is ready (a fraction of a second) the label shows only the app name. Then: window → pid (+ create
  time) → the app group that contains that pid → `Target` → `dispatch::plan("end-task", …)`. Nothing
  runs while the picker is closed, so there is no idle CPU cost.
- **Packaged (UWP) apps — my call:** Settings, Calculator, Photos and similar apps draw inside a
  frame window owned by `ApplicationFrameHost.exe`, which hosts every such app. The picker resolves
  the frame to the process of its child `Windows.UI.Core.CoreWindow` and ends that app's package
  group. It never ends `ApplicationFrameHost.exe` itself. If no child is found (the app is
  suspended), the window is not targeted.

## Left to the builder

- How to show the result when the main window is hidden (goal 18): a Windows toast notification,
  the tray balloon, or a short line in the overlay before it closes. Pick one; the text is the same
  `end_summary` text the main window shows today.
- The exact drawing of the dashed outline, the label card and the hint bar, within the mockup's look.
- What to do when the snapshot does not yet know the target's pid (the app started less than one
  sample ago): fall back to that process and its descendants, and say "1 process" in the label.
- Module layout (for example `crates/app/src/picker/` with pure parts — target search over a list of
  window records, label text, what a click does — split from the Win32 parts so they can be unit
  tested).
- How the overlay handles a monitor being added or removed while it is open (closing the picker is
  fine).
