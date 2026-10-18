//! Window setup on the UI thread (allowed by the Global Constraints): Mica backdrop, title-bar
//! theme, the colour scheme and the user's accent colour (re-applied when the system theme or
//! accent changes); the main-window hooks for tray re-creation and
//! activation; and the single-instance lock.
use crate::settings::{ThemeChoice, effective_dark};
use crate::{MainWindow, Theme};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::ComponentHandle;
use std::ffi::c_void;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, ERROR_SUCCESS, GetLastError, HANDLE, HWND, LPARAM, LRESULT, WAIT_ABANDONED,
    WAIT_OBJECT_0, WPARAM,
};
use windows::Win32::Graphics::Dwm::{
    DWMSBT_MAINWINDOW, DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute,
};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows::Win32::System::Threading::{
    CreateMutexW, GetCurrentProcess, GetCurrentProcessId, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_SYNCHRONIZE, QueryFullProcessImageNameW, ReleaseMutex, TerminateProcess, WaitForSingleObject,
};
use windows::Win32::UI::HiDpi::{GetDpiForSystem, GetDpiForWindow, GetSystemMetricsForDpi};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, ChangeWindowMessageFilterEx, EnumWindows, FindWindowW, GetPropW, GetWindowThreadProcessId, IsHungAppWindow, IsIconic,
    MSGFLT_ALLOW, PostMessageW, RegisterWindowMessageW, RemovePropW, SIZE_MINIMIZED, SM_CXSMICON, SW_RESTORE, SWP_FRAMECHANGED,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetForegroundWindow, SetPropW, SetWindowPos, ShowWindow,
    WM_DWMCOLORIZATIONCOLORCHANGED, WM_NCDESTROY, WM_SETTINGCHANGE, WM_SIZE,
};
use windows::core::{BOOL, HSTRING, PCWSTR, PWSTR, w};

/// `AccentColor` is stored as `0xAABBGGRR`.
pub fn accent_from_dword(v: u32) -> (u8, u8, u8) {
    ((v & 0xFF) as u8, ((v >> 8) & 0xFF) as u8, ((v >> 16) & 0xFF) as u8)
}

/// Reads a REG_DWORD under HKEY_CURRENT_USER; `None` if it is missing or of another type.
fn read_hkcu_dword(subkey: PCWSTR, value_name: PCWSTR) -> Option<u32> {
    let mut value = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: `value`/`size` are valid for a REG_DWORD read; both names are NUL-terminated literals.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            subkey,
            value_name,
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut c_void),
            Some(&mut size),
        )
    };
    (status == ERROR_SUCCESS).then_some(value)
}

pub fn read_accent() -> Option<(u8, u8, u8)> {
    read_hkcu_dword(w!("Software\\Microsoft\\Windows\\DWM"), w!("AccentColor")).map(accent_from_dword)
}

/// Sets `Theme.accent` to the user's accent colour; keeps the current one if it can't be read.
pub fn apply_accent(ui: &MainWindow) {
    if let Some((r, g, b)) = read_accent() {
        ui.global::<Theme>().set_accent(slint::Color::from_rgb_u8(r, g, b));
    }
}

/// `AppsUseLightTheme`: 0 means dark. Missing (older Windows, removed by policy) means light,
/// which is what Windows itself assumes.
pub fn dark_from(apps_use_light_theme: Option<u32>) -> bool {
    apps_use_light_theme == Some(0)
}

/// Whether Windows apps use the dark theme (Settings > Personalization > Colors).
pub fn system_prefers_dark() -> bool {
    dark_from(read_hkcu_dword(
        w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
        w!("AppsUseLightTheme"),
    ))
}

/// `SystemUsesLightTheme`: 1 means the taskbar (and Start) are light. Missing means dark, the
/// default Windows taskbar.
pub fn light_taskbar_from(system_uses_light_theme: Option<u32>) -> bool {
    system_uses_light_theme == Some(1)
}

/// Whether the taskbar is light (Settings > Personalization > Colors > "Choose your mode").
pub fn taskbar_is_light() -> bool {
    light_taskbar_from(read_hkcu_dword(
        w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
        w!("SystemUsesLightTheme"),
    ))
}

/// The tray icon size at `dpi`: SM_CXSMICON (16 at 96 DPI, 24 at 144, 32 at 192); 16 if the call fails.
pub fn tray_icon_px_at(dpi: u32) -> u32 {
    // SAFETY: plain metric query.
    let px = unsafe { GetSystemMetricsForDpi(SM_CXSMICON, dpi) };
    if px > 0 { px as u32 } else { 16 }
}

/// The DPI the tray icon is drawn for: the taskbar's own (`Shell_TrayWnd`, which Explorer draws
/// at the DPI of the monitor it is on), or the system DPI when that is unknown.
pub fn tray_dpi(taskbar_dpi: Option<u32>, system_dpi: u32) -> u32 {
    taskbar_dpi.filter(|&d| d > 0).unwrap_or(system_dpi)
}

/// The taskbar's window (Explorer's `Shell_TrayWnd`), if it exists. FindWindowW compares class
/// names only (no window text is asked for), so it sends no message and never waits on Explorer.
fn shell_tray_window() -> Option<HWND> {
    // SAFETY: a plain lookup by a NUL-terminated class name literal; no window name.
    unsafe { FindWindowW(w!("Shell_TrayWnd"), PCWSTR::null()) }.ok().filter(|h| !h.is_invalid())
}

/// The tray icon size for the taskbar as it is now (crate::tray reads it whenever a tray is
/// created: at start and after Explorer restarts). A taskbar moved to a monitor of another DPI
/// while Negotium runs keeps the old size until the next Explorer restart or launch.
pub fn tray_icon_px() -> u32 {
    // SAFETY: plain DPI queries; GetDpiForWindow reads the window's stored DPI without a message
    // (0 for an invalid handle, which `tray_dpi` replaces with the system DPI).
    let (taskbar, system) = unsafe { (shell_tray_window().map(|h| GetDpiForWindow(h)), GetDpiForSystem()) };
    tray_icon_px_at(tray_dpi(taskbar, system))
}

/// Whether the taskbar is hung, from `IsHungAppWindow` on `Shell_TrayWnd` if it was found: a
/// taskbar that is missing (Explorer restarting) is not hung.
pub fn tray_hung(found: Option<bool>) -> bool {
    found == Some(true)
}

/// Whether Explorer's taskbar thread is hung now (PF2: then Shell_NotifyIcon would wait on it).
/// Neither call sends a window message, so this never blocks on a hung Explorer itself.
pub fn shell_tray_hung() -> bool {
    // SAFETY: IsHungAppWindow reads the window's cached responsiveness; any handle value is safe.
    tray_hung(shell_tray_window().map(|h| unsafe { IsHungAppWindow(h) }.as_bool()))
}

/// Applies a theme choice: the colour scheme (immediately, through `set-dark`), then the title bar
/// and Mica to match (a no-op until the native window exists; the chrome timer applies it then).
pub fn apply_theme(ui: &MainWindow, theme: ThemeChoice) {
    apply_theme_with(ui, theme, system_prefers_dark());
}

/// [`apply_theme`] with the system's preference given, so a system flip can be tested. With
/// Light or Dark chosen the result ignores `system_dark` (`settings::effective_dark`).
pub fn apply_theme_with(ui: &MainWindow, theme: ThemeChoice, system_dark: bool) {
    ui.invoke_set_dark(effective_dark(theme, system_dark));
    apply_window_chrome(ui);
}

/// [`ShellEvent::SettingsChanged`]: the system theme or accent changed. The accent is re-read and
/// the chosen theme re-applied, which only moves the colours when it is System
/// (`settings::effective_dark`); the title bar and Mica are re-asserted either way.
pub fn on_settings_changed(ui: &MainWindow, theme: ThemeChoice) {
    apply_accent(ui);
    apply_theme(ui, theme);
}

fn hwnd_of(ui: &MainWindow) -> Option<HWND> {
    let handle = ui.window().window_handle();
    let raw = handle.window_handle().ok()?.as_raw();
    match raw {
        RawWindowHandle::Win32(h) => Some(HWND(h.hwnd.get() as *mut c_void)),
        _ => None,
    }
}

/// Applies the dark/light title bar (from `Theme.dark`) and the Mica backdrop. `Theme.mica`
/// follows the backdrop call alone: a failed dark-mode call leaves the title bar's colour wrong,
/// not the content's background. Returns true only if both DWM calls succeed.
pub fn apply_window_chrome(ui: &MainWindow) -> bool {
    let Some(hwnd) = hwnd_of(ui) else { return false };
    let dark = BOOL::from(ui.global::<Theme>().get_dark());
    let backdrop = DWMSBT_MAINWINDOW;
    // SAFETY: valid HWND of our own window; attribute payloads match their documented sizes.
    let dark_ok = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &dark as *const _ as *const c_void,
            std::mem::size_of::<BOOL>() as u32,
        )
    }
    .is_ok();
    // The caption keeps its old colours until the non-client area is redrawn: ask for that now
    // (no move, size, z-order or activation change).
    // SAFETY: plain call on our own window handle.
    let _ = unsafe {
        SetWindowPos(hwnd, None, 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE)
    };
    // SAFETY: as above; DWMSBT_MAINWINDOW is a DWM_SYSTEMBACKDROP_TYPE (i32).
    let mica_ok = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE,
            &backdrop as *const _ as *const c_void,
            std::mem::size_of_val(&backdrop) as u32,
        )
    }
    .is_ok();
    ui.global::<Theme>().set_mica(mica_ok);
    dark_ok && mica_ok
}

/// The main window's native handle, once it exists.
pub fn main_hwnd(ui: &MainWindow) -> Option<HWND> {
    hwnd_of(ui)
}

/// Shows the window (if hidden to the tray), restores it if minimised and brings it to the
/// foreground. `show()` alone does nothing for a minimised window.
pub fn bring_to_front(ui: &MainWindow) {
    let _ = ui.show();
    let Some(hwnd) = hwnd_of(ui) else { return };
    // SAFETY: plain calls on our own window handle.
    unsafe {
        if is_minimised(hwnd) {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        let _ = SetForegroundWindow(hwnd);
    }
}

/// Where a launch in the tray first creates the window: far off every screen, so the moment it
/// is on screen before it can be hidden shows nothing (Slint creates the native window only once
/// the event loop runs). [`centered_position`] puts it back once it is hidden.
pub const OFF_SCREEN: (i32, i32) = (-32000, -32000);

/// The top-left that centres a window of `size` (width, height) in `work` (left, top, right,
/// bottom), kept inside it when the window is larger.
pub fn centered(work: (i32, i32, i32, i32), size: (i32, i32)) -> (i32, i32) {
    let (left, top, right, bottom) = work;
    let x = left + ((right - left) - size.0) / 2;
    let y = top + ((bottom - top) - size.1) / 2;
    (x.max(left), y.max(top))
}

/// The position that centres `hwnd` in the primary monitor's work area (the window started off
/// screen, [`OFF_SCREEN`]), or `None` if Windows doesn't say.
pub fn centered_position(hwnd: HWND) -> Option<(i32, i32)> {
    use windows::Win32::Foundation::{POINT, RECT};
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromPoint};
    use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;
    let mut rect = RECT::default();
    let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    // SAFETY: plain queries on our own window and the primary monitor; the out-parameters are
    // writable locals, `info.cbSize` set as GetMonitorInfoW requires.
    unsafe {
        GetWindowRect(hwnd, &mut rect).ok()?;
        let monitor = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return None;
        }
    }
    let w = info.rcWork;
    Some(centered((w.left, w.top, w.right, w.bottom), (rect.right - rect.left, rect.bottom - rect.top)))
}

/// Where a window started off screen ([`OFF_SCREEN`]) goes once hidden: centred
/// ([`centered_position`]), else the primary work area's top-left, else the primary screen's
/// origin. Never left off screen, where a later Show would bring back a window nobody can see.
pub fn on_screen_position(centered: Option<(i32, i32)>, work: Option<(i32, i32, i32, i32)>) -> (i32, i32) {
    centered.or(work.map(|(left, top, _, _)| (left, top))).unwrap_or((0, 0))
}

/// The primary monitor's work area (left, top, right, bottom), from SPI_GETWORKAREA.
pub fn primary_work_area() -> Option<(i32, i32, i32, i32)> {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::{SPI_GETWORKAREA, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW};
    let mut rect = RECT::default();
    // SAFETY: SPI_GETWORKAREA writes one RECT to the writable local passed.
    unsafe {
        SystemParametersInfoW(SPI_GETWORKAREA, 0, Some(&mut rect as *mut RECT as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0))
    }
    .ok()?;
    Some((rect.left, rect.top, rect.right, rect.bottom))
}

/// Whether `hwnd` is minimised now: a launch from a shortcut set to "Minimized" starts so, before
/// the window hooks can hear WM_SIZE.
pub fn is_minimised(hwnd: HWND) -> bool {
    // SAFETY: a plain query; any handle value is safe to pass (an invalid one reads as false).
    unsafe { IsIconic(hwnd).as_bool() }
}

/// Messages the main window handles on top of what winit does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellEvent {
    /// Explorer (re)started: the tray icon must be added again. Slint's tray window is
    /// message-only, so it never receives this broadcast itself.
    TaskbarCreated,
    /// A second launch asked this instance to show itself.
    Activate,
    /// The system theme or accent colour may have changed: WM_SETTINGCHANGE for
    /// "ImmersiveColorSet" (light/dark mode) or WM_DWMCOLORIZATIONCOLORCHANGED (accent).
    SettingsChanged,
    /// The main window was minimised (true) or shown at a size again (false): graphs scroll only
    /// while it is on screen. Sent on every WM_SIZE; the receiver drops repeats.
    Minimised(bool),
    /// The installer asked this instance to exit ([`exit_instances_of`]), before it replaces the
    /// files: quit as the tray's Exit does.
    Exit,
}

// Cached: the subclass compares every message the main window receives against these.
pub fn taskbar_created_message() -> u32 {
    static MSG: OnceLock<u32> = OnceLock::new();
    // SAFETY: registers (or looks up) a well-known message name.
    *MSG.get_or_init(|| unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) })
}

pub fn activate_message() -> u32 {
    static MSG: OnceLock<u32> = OnceLock::new();
    // SAFETY: as above.
    *MSG.get_or_init(|| unsafe { RegisterWindowMessageW(w!("Negotium.Activate")) })
}

pub fn exit_message() -> u32 {
    static MSG: OnceLock<u32> = OnceLock::new();
    // SAFETY: as above.
    *MSG.get_or_init(|| unsafe { RegisterWindowMessageW(w!("Negotium.Exit")) })
}

/// Window property marking the main window, so a second launch can find it even while it is
/// hidden in the tray.
const MAIN_WINDOW_PROP: PCWSTR = w!("Negotium.MainWindow");
const SUBCLASS_ID: usize = 0x4E45_474F; // "NEGO"

type Hook = Box<dyn Fn(ShellEvent)>;

/// Subclasses the main window: `on_event` runs (on the UI thread, inside the window procedure)
/// for TaskbarCreated, the activation and exit messages and theme or accent changes (both
/// broadcast to top-level windows, so they need no UIPI exemption). Also lets TaskbarCreated,
/// activation and exit through UIPI, so a non-elevated Explorer, second launch or installer step
/// can still reach an elevated instance (anything in the session can then ask it to exit, as it
/// could close a non-elevated one). Returns false if subclassing failed.
pub fn install_window_hooks(hwnd: HWND, on_event: impl Fn(ShellEvent) + 'static) -> bool {
    let hook: *mut Hook = Box::into_raw(Box::new(Box::new(on_event)));
    // SAFETY: `hook` stays valid until WM_NCDESTROY, where the subclass frees it.
    let ok = unsafe { SetWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID, hook as usize) }.as_bool();
    if !ok {
        // SAFETY: not handed to the subclass; reclaimed exactly once.
        drop(unsafe { Box::from_raw(hook) });
        return false;
    }
    // SAFETY: plain calls on a window of this thread; failure only loses the UIPI exemption.
    unsafe {
        for msg in [taskbar_created_message(), activate_message(), exit_message()] {
            let _ = ChangeWindowMessageFilterEx(hwnd, msg, MSGFLT_ALLOW, None);
        }
    }
    true
}

/// Marks `hwnd` as this instance's main window for [`find_main_window`] in a later launch.
pub fn mark_main_window(hwnd: HWND) {
    // Only presence matters: any non-null value marks the window.
    let marker = HANDLE(std::ptr::dangling_mut::<c_void>());
    // SAFETY: sets a property on a window of this thread (removed again on WM_NCDESTROY).
    unsafe {
        let _ = SetPropW(hwnd, MAIN_WINDOW_PROP, Some(marker));
    }
}

unsafe extern "system" fn subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    let hook = data as *mut Hook;
    if msg == WM_NCDESTROY {
        // SAFETY: last message to this window: unhook, then free the hook exactly once.
        unsafe {
            let _ = RemoveWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID);
            let _ = RemovePropW(hwnd, MAIN_WINDOW_PROP);
            drop(Box::from_raw(hook));
        }
    } else if msg != 0 && msg == activate_message() {
        // SAFETY: `hook` lives until WM_NCDESTROY.
        unsafe { (*hook)(ShellEvent::Activate) };
        return LRESULT(0);
    } else if msg != 0 && msg == exit_message() {
        // SAFETY: as above.
        unsafe { (*hook)(ShellEvent::Exit) };
        return LRESULT(0);
    } else if msg != 0 && msg == taskbar_created_message() {
        // SAFETY: as above.
        unsafe { (*hook)(ShellEvent::TaskbarCreated) };
    } else if msg == WM_SIZE {
        // SAFETY: as above.
        unsafe { (*hook)(ShellEvent::Minimised(wparam.0 as u32 == SIZE_MINIMIZED)) };
    } else if (msg == WM_SETTINGCHANGE && is_colour_scheme_change(lparam)) || msg == WM_DWMCOLORIZATIONCOLORCHANGED {
        // SAFETY: as above.
        unsafe { (*hook)(ShellEvent::SettingsChanged) };
    }
    // SAFETY: forwards to the next procedure in the subclass chain.
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

/// Whether a WM_SETTINGCHANGE names the colour scheme ("ImmersiveColorSet"). It is broadcast
/// for many unrelated changes (locale, environment, policies); those name another area or none.
fn is_colour_scheme_change(lparam: LPARAM) -> bool {
    const AREA: &str = "ImmersiveColorSet";
    let p = lparam.0 as *const u16;
    if p.is_null() {
        return false;
    }
    // At most AREA's length plus its NUL is read: the compare stops at the first difference,
    // and a shorter string differs at its own NUL, so nothing past a terminator is ever read.
    for (i, want) in AREA.encode_utf16().chain(std::iter::once(0)).enumerate() {
        // SAFETY: trusted only because WM_SETTINGCHANGE is a system broadcast: the system
        // marshals its lParam into this process as a NUL-terminated UTF-16 string (or null),
        // valid while the message is handled. `i` never passes that string's NUL (see above).
        let c = unsafe { p.add(i).read() };
        if c != want {
            return false;
        }
    }
    true
}

/// Removes the mark set by [`mark_main_window`] (the instance is exiting).
pub fn unmark_main_window(hwnd: HWND) {
    // SAFETY: removes a property from a window of this thread; harmless if absent.
    unsafe {
        let _ = RemovePropW(hwnd, MAIN_WINDOW_PROP);
    }
}

/// The main window of a running instance (this process's included), visible or not.
pub fn find_main_window() -> Option<HWND> {
    find_main_window_where(|_| true)
}

/// The first marked window that `accept` takes (tests narrow it to their own window, so another
/// running instance can't answer).
fn find_main_window_where(accept: impl Fn(HWND) -> bool) -> Option<HWND> {
    main_windows_where(accept, true).into_iter().next()
}

/// The marked windows `accept` takes, in z-order; only the first with `first_only`.
fn main_windows_where(accept: impl Fn(HWND) -> bool, first_only: bool) -> Vec<HWND> {
    struct Search<'a> {
        accept: &'a dyn Fn(HWND) -> bool,
        first_only: bool,
        found: Vec<HWND>,
    }
    unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: lparam is the `&mut Search` passed below for the duration of EnumWindows.
        let search = unsafe { &mut *(lparam.0 as *mut Search) };
        // SAFETY: read-only property lookup on a handle supplied by EnumWindows.
        if !unsafe { GetPropW(hwnd, MAIN_WINDOW_PROP) }.is_invalid() && (search.accept)(hwnd) {
            search.found.push(hwnd);
            return (!search.first_only).into();
        }
        true.into()
    }
    let mut search = Search { accept: &accept, first_only, found: Vec::new() };
    // SAFETY: `search` outlives the synchronous enumeration.
    unsafe {
        let _ = EnumWindows(Some(visit), LPARAM(&mut search as *mut Search as isize));
    }
    search.found
}

/// Whether the instance in process `pid`, running `image`, is one [`exit_instances_of`] asks to
/// exit: another process (never this one) running `exe` itself. A dev build, or an image that
/// can't be read, is left alone.
fn asked_to_exit(pid: u32, me: u32, image: Option<&Path>, exe: &Path) -> bool {
    pid != me && image.is_some_and(|image| crate::integrate::same_path(image, exe))
}

/// The process owning `hwnd`, opened to wait for it, and its image.
fn window_process(hwnd: HWND) -> Option<(u32, OwnedHandle, Option<PathBuf>)> {
    let mut pid = 0u32;
    // SAFETY: plain call; a window that has gone leaves `pid` 0.
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if pid == 0 {
        return None;
    }
    // A limited query and SYNCHRONIZE: what an elevated instance's process also grants the same
    // user's non-elevated token.
    // SAFETY: plain call; the handle is owned (and closed) by the OwnedHandle below.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE, false, pid) }.ok()?;
    // SAFETY: a live handle this function just opened, owned by nothing else.
    let process = unsafe { OwnedHandle::from_raw_handle(process.0) };
    let mut buf = vec![0u16; 32 * 1024];
    let mut len = buf.len() as u32;
    // SAFETY: `buf` holds `len` u16s; the call writes at most that many and updates `len`.
    let image = unsafe {
        QueryFullProcessImageNameW(HANDLE(process.as_raw_handle()), PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len)
    }
    .ok()
    .map(|()| PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])));
    Some((pid, process, image))
}

/// Asks every running instance of `exe` in this session (its main window, on screen or hidden in
/// the tray) to exit the way the tray's Exit does ([`ShellEvent::Exit`]), then waits up to
/// `timeout` for those processes to end. Returns how many were still running then. This process,
/// dev builds and instances in other sessions are left alone (the installer runs this, as the
/// installed exe, before it replaces the files). Never called by tests: it reaches real windows.
pub fn exit_instances_of(exe: &Path, timeout: Duration) -> usize {
    // SAFETY: plain call.
    let me = unsafe { GetCurrentProcessId() };
    let mut asked = Vec::new();
    for hwnd in main_windows_where(|_| true, false) {
        let Some((pid, process, image)) = window_process(hwnd) else { continue };
        if !asked_to_exit(pid, me, image.as_deref(), exe) {
            continue;
        }
        // SAFETY: plain call; a window that has gone meanwhile only makes it fail.
        if unsafe { PostMessageW(Some(hwnd), exit_message(), WPARAM(0), LPARAM(0)) }.is_ok() {
            asked.push(process);
        }
    }
    let deadline = Instant::now() + timeout;
    asked
        .iter()
        .filter(|process| {
            let left = deadline.saturating_duration_since(Instant::now());
            // SAFETY: a live process handle opened with SYNCHRONIZE; waiting has no other effect.
            let waited = unsafe { WaitForSingleObject(HANDLE(process.as_raw_handle()), left.as_millis() as u32) };
            waited != WAIT_OBJECT_0
        })
        .count()
}

/// Asks the running instance to show itself, waiting up to `timeout` for its window to exist
/// (it may still be starting). Returns false if no window turned up.
pub fn activate_existing_instance(timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(hwnd) = find_main_window() {
            let mut pid = 0u32;
            // SAFETY: plain calls; the target may vanish meanwhile, which only makes them fail.
            unsafe {
                GetWindowThreadProcessId(hwnd, Some(&mut pid));
                // We are the foreground process (just launched); let the old instance take focus.
                let _ = AllowSetForegroundWindow(pid);
                let _ = PostMessageW(Some(hwnd), activate_message(), WPARAM(0), LPARAM(0));
            }
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Lets whichever process comes next take the foreground (this launch has it): admin mode's
/// task starts the elevated copy, which isn't this process's child.
pub fn allow_any_foreground() {
    // SAFETY: plain call; failure only means the started copy may open behind other windows.
    unsafe {
        let _ = AllowSetForegroundWindow(windows::Win32::UI::WindowsAndMessaging::ASFW_ANY);
    }
}

pub const INSTANCE_MUTEX: &str = r"Local\Negotium.SingleInstance";

/// Held for the life of the primary instance.
pub struct InstanceLock(HANDLE);

impl Drop for InstanceLock {
    fn drop(&mut self) {
        // SAFETY: this thread owns the mutex (created as owner, or acquired by the wait); the
        // handle is closed exactly once.
        unsafe {
            let _ = ReleaseMutex(self.0);
            let _ = CloseHandle(self.0);
        }
    }
}

/// Claims the per-session single-instance lock. `None` means another instance holds it. With a
/// nonzero `wait` (a restart, e.g. as admin, while the old instance is still exiting) it waits
/// that long for the holder to go.
pub fn claim_instance(name: &str, wait: Duration) -> Option<InstanceLock> {
    let name = HSTRING::from(name);
    let deadline = Instant::now() + wait;
    loop {
        // SAFETY: plain call; the returned handle is owned by InstanceLock or closed below.
        let created = unsafe { CreateMutexW(None, true, &name) };
        match created {
            // SAFETY: read immediately after the call it describes.
            Ok(h) if unsafe { GetLastError() } != ERROR_ALREADY_EXISTS => return Some(InstanceLock(h)),
            Ok(h) if wait.is_zero() => {
                // SAFETY: not handed out; closed exactly once.
                unsafe {
                    let _ = CloseHandle(h);
                }
                return None;
            }
            Ok(h) => {
                let left = deadline.saturating_duration_since(Instant::now());
                // SAFETY: valid mutex handle; a wait of 0 just polls.
                let r = unsafe { WaitForSingleObject(h, left.as_millis() as u32) };
                if r == WAIT_OBJECT_0 || r == WAIT_ABANDONED {
                    return Some(InstanceLock(h));
                }
                // SAFETY: not handed out; closed exactly once.
                unsafe {
                    let _ = CloseHandle(h);
                }
                return None;
            }
            // Created by an elevated instance whose DACL we can't open: it exists. Retry until it
            // is gone (a restart) or give up.
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            Err(_) => return None,
        }
    }
}

/// Ends this process at once, from any thread, without running DLL detach code. The renderer
/// watchdog uses it: on a wedged GL driver an orderly exit (`std::process::exit`) takes the loader
/// lock and runs opengl32's and the driver's DLL_PROCESS_DETACH, which can hang with the
/// single-instance lock still held. Anything that must survive (files) has to be closed first.
pub fn terminate_self() -> ! {
    // SAFETY: the pseudo-handle of the current process is always valid; terminating ourselves is
    // the intent, and the kernel releases our handles (the instance mutex) as the process ends.
    let _ = unsafe { TerminateProcess(GetCurrentProcess(), 0) };
    // TerminateProcess on the current process does not return; if it ever did, exit normally.
    std::process::exit(0)
}

/// Removes an environment variable from this process, so processes it starts don't inherit it.
/// Call it only while no other thread can be reading the environment (early in `main`).
pub fn remove_env_var(name: &str) {
    // SAFETY: called from `main` before any other thread is started; on Windows the environment
    // block is also guarded by the process environment lock.
    unsafe { std::env::remove_var(name) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accent_dword_is_abgr() {
        assert_eq!(accent_from_dword(0xFFD47800), (0x00, 0x78, 0xD4));
    }

    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::Duration;
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, SendMessageW, WINDOW_EX_STYLE, WS_OVERLAPPED,
    };

    /// A hidden top-level window of a system class, standing in for the main window.
    fn hidden_window() -> HWND {
        // SAFETY: plain window creation with a system class; destroyed by the test.
        unsafe {
            CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), w!("negotium-test"), WS_OVERLAPPED, 0, 0, 10, 10, None, None, None, None)
        }
        .expect("create test window")
    }

    #[test]
    fn shell_messages_reach_the_main_window_hook() {
        use windows::Win32::UI::WindowsAndMessaging::WM_DWMCOLORIZATIONCOLORCHANGED;
        let hwnd = hidden_window();
        let seen = Rc::new(RefCell::new(Vec::new()));
        let s = seen.clone();
        assert!(install_window_hooks(hwnd, move |e| s.borrow_mut().push(e)));
        // SAFETY: synchronous sends to our own window on this thread.
        unsafe {
            SendMessageW(hwnd, activate_message(), Some(WPARAM(0)), Some(LPARAM(0)));
            SendMessageW(hwnd, taskbar_created_message(), Some(WPARAM(0)), Some(LPARAM(0)));
            SendMessageW(hwnd, WM_DWMCOLORIZATIONCOLORCHANGED, Some(WPARAM(0)), Some(LPARAM(0)));
            SendMessageW(hwnd, exit_message(), Some(WPARAM(0)), Some(LPARAM(0)));
            let _ = DestroyWindow(hwnd);
        }
        assert_eq!(
            *seen.borrow(),
            [ShellEvent::Activate, ShellEvent::TaskbarCreated, ShellEvent::SettingsChanged, ShellEvent::Exit]
        );
    }

    #[test]
    fn only_other_instances_of_the_same_exe_are_asked_to_exit() {
        use std::path::Path;
        let exe = Path::new(r"C:\Program Files\Negotium\negotium.exe");
        let me = 10;
        assert!(asked_to_exit(20, me, Some(Path::new(r"c:\program files\negotium\NEGOTIUM.EXE")), exe));
        assert!(!asked_to_exit(me, me, Some(exe), exe), "not this process");
        let dev = Path::new(r"C:\dev\Negotium\target\release\negotium.exe");
        assert!(!asked_to_exit(20, me, Some(dev), exe), "a dev build");
        assert!(!asked_to_exit(20, me, None, exe), "an image that can't be read");
    }

    #[test]
    fn minimising_and_restoring_reach_the_hook() {
        use windows::Win32::UI::WindowsAndMessaging::{SIZE_MINIMIZED, SIZE_RESTORED, WM_SIZE};
        let hwnd = hidden_window();
        let seen = Rc::new(RefCell::new(Vec::new()));
        let s = seen.clone();
        assert!(install_window_hooks(hwnd, move |e| s.borrow_mut().push(e)));
        // SAFETY: synchronous sends to our own window on this thread.
        unsafe {
            SendMessageW(hwnd, WM_SIZE, Some(WPARAM(SIZE_MINIMIZED as usize)), Some(LPARAM(0)));
            SendMessageW(hwnd, WM_SIZE, Some(WPARAM(SIZE_RESTORED as usize)), Some(LPARAM(0)));
            let _ = DestroyWindow(hwnd);
        }
        let sizes: Vec<ShellEvent> = seen.borrow().iter().copied().filter(|e| matches!(e, ShellEvent::Minimised(_))).collect();
        assert_eq!(sizes, [ShellEvent::Minimised(true), ShellEvent::Minimised(false)]);
    }

    #[test]
    fn a_window_that_starts_minimised_reads_as_minimised() {
        use windows::Win32::UI::WindowsAndMessaging::WS_MINIMIZE;
        let hwnd = hidden_window();
        assert!(!is_minimised(hwnd));
        // SAFETY: as hidden_window, never shown (no WS_VISIBLE): minimised from creation, as a
        // launch from a shortcut set to "Minimized" leaves the main window.
        let minimised = unsafe {
            CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), w!("negotium-test"), WS_OVERLAPPED | WS_MINIMIZE, 0, 0, 10, 10, None, None, None, None)
        }
        .expect("create test window");
        assert!(is_minimised(minimised));
        // SAFETY: our own test windows.
        unsafe {
            let _ = DestroyWindow(hwnd);
            let _ = DestroyWindow(minimised);
        }
    }

    #[test]
    fn only_a_colour_scheme_setting_change_reaches_the_hook() {
        use windows::Win32::UI::WindowsAndMessaging::WM_SETTINGCHANGE;
        let hwnd = hidden_window();
        let seen = Rc::new(RefCell::new(Vec::new()));
        let s = seen.clone();
        assert!(install_window_hooks(hwnd, move |e| s.borrow_mut().push(e)));
        let send = |area: Option<PCWSTR>| {
            let lparam = LPARAM(area.map_or(0, |a| a.as_ptr() as isize));
            // SAFETY: synchronous send to our own window; `area` is a NUL-terminated literal.
            unsafe { SendMessageW(hwnd, WM_SETTINGCHANGE, Some(WPARAM(0)), Some(lparam)) };
        };
        send(None);
        send(Some(w!("intl")));
        send(Some(w!("Environment")));
        send(Some(w!("ImmersiveColorSetX")));
        assert!(seen.borrow().is_empty(), "unrelated or unnamed setting changes are ignored");
        send(Some(w!("ImmersiveColorSet")));
        assert_eq!(*seen.borrow(), [ShellEvent::SettingsChanged], "a colour scheme change reaches the hook");
        // SAFETY: our own test window.
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
    }

    #[test]
    fn apps_use_dark_only_when_the_light_flag_is_zero() {
        assert!(dark_from(Some(0)), "AppsUseLightTheme = 0: dark");
        assert!(!dark_from(Some(1)), "AppsUseLightTheme = 1: light");
        assert!(!dark_from(Some(7)), "any other value is not 0: light");
        assert!(!dark_from(None), "missing value (older Windows, policy): light, as Windows itself defaults");
    }

    #[test]
    fn tray_icons_follow_the_dpi_and_the_taskbar_mode_reads_safely() {
        use windows::Win32::UI::HiDpi::GetDpiForSystem;
        use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSMICON};
        // The machine's own small-icon size (PF11): GetSystemMetrics answers at the system DPI.
        // SAFETY: plain metric and DPI queries.
        let (small, system_dpi) = unsafe { (GetSystemMetrics(SM_CXSMICON) as u32, GetDpiForSystem()) };
        assert_eq!(tray_icon_px_at(system_dpi), small, "the size at the system DPI, as Windows reports it");
        // SAFETY: a plain DPI query on the taskbar's handle.
        let taskbar = shell_tray_window().map(|h| unsafe { GetDpiForWindow(h) });
        assert_eq!(tray_icon_px(), tray_icon_px_at(tray_dpi(taskbar, system_dpi)), "drawn for the taskbar's DPI");
        assert_eq!(tray_dpi(Some(144), 96), 144, "the taskbar's own DPI wins");
        assert_eq!(tray_dpi(None, 120), 120, "no taskbar: the system DPI");
        assert_eq!(tray_dpi(Some(0), 120), 120, "an unreadable DPI: the system DPI");
        // Scaled back to 96 DPI (16 on a default setup): Windows scales the metric with the DPI.
        let at_96 = (small * 96 + system_dpi / 2) / system_dpi;
        assert!(tray_icon_px_at(96).abs_diff(at_96) <= 1, "{} vs {at_96}", tray_icon_px_at(96));
        assert!(tray_icon_px_at(192).abs_diff(2 * tray_icon_px_at(96)) <= 1, "twice the DPI, twice the size");
        assert!(light_taskbar_from(Some(1)));
        assert!(!light_taskbar_from(Some(0)) && !light_taskbar_from(None) && !light_taskbar_from(Some(7)));
    }

    #[test]
    fn only_a_found_and_hung_taskbar_holds_the_tray_back() {
        assert!(tray_hung(Some(true)), "Shell_TrayWnd is hung");
        assert!(!tray_hung(Some(false)), "it answers");
        assert!(!tray_hung(None), "no taskbar (Explorer restarting): nothing to wait on");
        let _ = shell_tray_hung(); // the real query returns without blocking
    }

    #[test]
    fn a_window_is_centred_in_the_work_area() {
        assert_eq!(centered((0, 0, 1920, 1040), (1000, 600)), (460, 220));
        assert_eq!(centered((-1920, 0, 0, 1080), (920, 80)), (-1420, 500), "a monitor left of the primary");
        assert_eq!(centered((0, 0, 800, 600), (1000, 700)), (0, 0), "too large: its corner stays on screen");
        assert!(OFF_SCREEN.0 <= -30000 && OFF_SCREEN.1 <= -30000, "beyond any desktop");
    }

    #[test]
    fn a_window_started_off_screen_always_comes_back_on_screen() {
        assert_eq!(on_screen_position(Some((460, 220)), Some((0, 40, 1920, 1080))), (460, 220), "centred");
        assert_eq!(on_screen_position(None, Some((0, 40, 1920, 1080))), (0, 40), "the work area's corner (a taskbar on top)");
        assert_eq!(on_screen_position(None, None), (0, 0), "the primary screen's origin");
        assert_ne!(on_screen_position(None, None), OFF_SCREEN);
        let work = primary_work_area().expect("the primary work area");
        assert!(work.2 > work.0 && work.3 > work.1, "{work:?}");
    }

    #[test]
    fn a_hidden_main_window_is_found_by_its_mark() {
        let hwnd = hidden_window();
        mark_main_window(hwnd);
        let found = find_main_window_where(|h| h == hwnd);
        unmark_main_window(hwnd);
        let after = find_main_window_where(|h| h == hwnd);
        // SAFETY: our own test window.
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
        assert_eq!(found, Some(hwnd));
        assert_eq!(after, None, "an exiting instance is no longer found");
    }

    #[test]
    fn only_one_instance_holds_the_lock_and_a_restart_waits_for_it() {
        let name = format!(r"Local\Negotium.Test.{}", std::process::id());
        let first = claim_instance(&name, Duration::ZERO).expect("the first instance claims the lock");
        assert!(claim_instance(&name, Duration::ZERO).is_none(), "a second instance is turned away");
        let waiter = {
            let name = name.clone();
            std::thread::spawn(move || claim_instance(&name, Duration::from_secs(5)).is_some())
        };
        std::thread::sleep(Duration::from_millis(200));
        drop(first);
        assert!(waiter.join().unwrap(), "a restarted instance gets the lock once the old one exits");
    }
}
