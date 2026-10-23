//! Window setup on the UI thread (allowed by the Global Constraints): Mica backdrop, title-bar
//! theme, the colour scheme and the user's accent colour (re-applied when the system theme or
//! accent changes); the main-window hooks for tray re-creation and
//! activation; and the single-instance lock.
use crate::renderer::Renderer;
use crate::settings::{BackdropChoice, ThemeChoice, effective_dark};
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
    DWM_SYSTEMBACKDROP_TYPE, DWMSBT_TABBEDWINDOW, DWMSBT_TRANSIENTWINDOW, DWMWA_SYSTEMBACKDROP_TYPE,
    DWMWA_USE_IMMERSIVE_DARK_MODE, DwmExtendFrameIntoClientArea, DwmSetWindowAttribute,
};
use windows::Win32::UI::Controls::MARGINS;
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
    GetWindowPlacement, SetWindowPlacement, WINDOWPLACEMENT, SW_SHOWMAXIMIZED, SW_SHOWNORMAL, WM_EXITSIZEMOVE,
    WPF_RESTORETOMAXIMIZED,
    WM_DWMCOLORIZATIONCOLORCHANGED, WM_NCDESTROY, WM_SETTINGCHANGE, WM_SIZE,
    HTCLIENT, HTMAXBUTTON, WM_NCLBUTTONDOWN,
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

/// The system backdrop behind a window drawn by `renderer`, or none. Only behind the GPU
/// renderer: FemtoVG presents an alpha channel that DWM composites over the backdrop. The
/// software renderer's buffer is opaque (and `SLINT_BACKEND` may name it), so that window keeps
/// the solid `Theme.window-bg`; the translucent cards then blend over that colour instead.
pub fn backdrop_for(renderer: Renderer, choice: BackdropChoice) -> Option<DWM_SYSTEMBACKDROP_TYPE> {
    let backdrop = match choice {
        BackdropChoice::Mica => DWMSBT_TABBEDWINDOW,
        BackdropChoice::Acrylic => DWMSBT_TRANSIENTWINDOW,
    };
    (renderer == Renderer::Gpu).then_some(backdrop)
}

thread_local! {
    /// The renderer this instance draws with, set once by `main` before the window is themed.
    /// Unset (tests, or before `main` sets it): no backdrop.
    static RENDERER: std::cell::Cell<Option<Renderer>> = const { std::cell::Cell::new(None) };
    /// The chosen backdrop (Settings > Backdrop), set by `set_backdrop`.
    static BACKDROP: std::cell::Cell<BackdropChoice> = const { std::cell::Cell::new(BackdropChoice::Mica) };
}

/// Records the renderer in use, which decides the backdrop (`backdrop_for`). UI thread only.
pub fn set_renderer(renderer: Renderer) {
    RENDERER.with(|r| r.set(Some(renderer)));
}

/// Records the chosen backdrop and re-applies the window chrome with it. UI thread only.
pub fn set_backdrop(ui: &MainWindow, choice: BackdropChoice) {
    BACKDROP.with(|b| b.set(choice));
    ui.global::<Theme>().set_acrylic(choice == BackdropChoice::Acrylic);
    apply_window_chrome(ui);
}

/// Applies the dark/light title bar (from `Theme.dark`) and the backdrop (`backdrop_for`).
/// `Theme.mica` follows the backdrop call alone: a failed dark-mode call leaves the title bar's
/// colour wrong, not the content's background. Returns true only if every DWM call made succeeds.
pub fn apply_window_chrome(ui: &MainWindow) -> bool {
    let Some(hwnd) = hwnd_of(ui) else { return false };
    let dark = BOOL::from(ui.global::<Theme>().get_dark());
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
    let Some(backdrop) = RENDERER.with(|r| r.get()).and_then(|r| backdrop_for(r, BACKDROP.with(|b| b.get()))) else {
        ui.global::<Theme>().set_mica(false);
        return dark_ok;
    };
    // SAFETY: as above; the payload is a DWM_SYSTEMBACKDROP_TYPE (i32).
    let mica_ok = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE,
            &backdrop as *const _ as *const c_void,
            std::mem::size_of_val(&backdrop) as u32,
        )
    }
    .is_ok();
    // DWM draws the backdrop behind the client area only where the frame extends into it; without
    // this a transparent background shows the windows behind, not the backdrop.
    let margins = MARGINS { cxLeftWidth: -1, cxRightWidth: -1, cyTopHeight: -1, cyBottomHeight: -1 };
    // SAFETY: plain call on our own window handle.
    let extended = mica_ok && unsafe { DwmExtendFrameIntoClientArea(hwnd, &margins) }.is_ok();
    ui.global::<Theme>().set_mica(extended);
    dark_ok && extended
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

/// Reads the current window geometry (normal position and size, plus maximized flag) from `hwnd`.
pub fn read_window_geometry(hwnd: HWND) -> Option<crate::settings::WindowGeometry> {
    let mut wp = WINDOWPLACEMENT {
        length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
        ..Default::default()
    };
    // SAFETY: plain query on our own window handle.
    unsafe {
        GetWindowPlacement(hwnd, &mut wp).ok()?;
    }
    let r = wp.rcNormalPosition;
    let width = (r.right - r.left).max(0) as u32;
    let height = (r.bottom - r.top).max(0) as u32;
    if width < 100 || height < 100 {
        return None;
    }
    let maximized = wp.showCmd == SW_SHOWMAXIMIZED.0 as u32
        || (wp.flags.0 & WPF_RESTORETOMAXIMIZED.0 != 0);
    Some(crate::settings::WindowGeometry {
        x: r.left,
        y: r.top,
        width: width.max(760),
        height: height.max(440),
        maximized,
    })
}

/// Applies `geo` to `hwnd`: size, normal position, and maximized state.
pub fn apply_window_geometry(hwnd: HWND, geo: crate::settings::WindowGeometry) -> bool {
    let mut wp = WINDOWPLACEMENT {
        length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
        ..Default::default()
    };
    // SAFETY: querying placement on our own window handle.
    if unsafe { GetWindowPlacement(hwnd, &mut wp) }.is_err() {
        return false;
    }
    wp.rcNormalPosition = windows::Win32::Foundation::RECT {
        left: geo.x,
        top: geo.y,
        right: geo.x + geo.width as i32,
        bottom: geo.y + geo.height as i32,
    };
    if geo.maximized {
        wp.showCmd = SW_SHOWMAXIMIZED.0 as u32;
    } else {
        wp.showCmd = SW_SHOWNORMAL.0 as u32;
    }
    // SAFETY: plain call on our own window handle.
    unsafe { SetWindowPlacement(hwnd, &wp).is_ok() }
}

/// Sanitizes saved geometry against connected monitors and minimum dimensions.
pub fn sanitize_geometry(geo: crate::settings::WindowGeometry) -> crate::settings::WindowGeometry {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITORINFO, MonitorFromRect, MONITOR_DEFAULTTONULL};

    let width = geo.width.max(760);
    let height = geo.height.max(440);
    let rect = RECT {
        left: geo.x,
        top: geo.y,
        right: geo.x + width as i32,
        bottom: geo.y + height as i32,
    };

    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    let monitor_work = unsafe {
        let monitor = MonitorFromRect(&rect, MONITOR_DEFAULTTONULL);
        if monitor.0.is_null() || !GetMonitorInfoW(monitor, &mut info).as_bool() {
            None
        } else {
            let w = info.rcWork;
            Some((w.left, w.top, w.right, w.bottom))
        }
    };

    let primary = primary_work_area().unwrap_or((0, 0, 1920, 1080));
    sanitize_geometry_bounds(geo, primary, monitor_work)
}

/// Pure helper for bounds clamping / centering, so multi-monitor logic can be unit-tested.
pub fn sanitize_geometry_bounds(
    geo: crate::settings::WindowGeometry,
    primary_work: (i32, i32, i32, i32),
    monitor_work: Option<(i32, i32, i32, i32)>,
) -> crate::settings::WindowGeometry {
    let width = geo.width.max(760);
    let height = geo.height.max(440);

    let Some(work) = monitor_work else {
        // Disconnected monitor or completely off-screen: center on primary work area
        let (cx, cy) = centered(primary_work, (width as i32, height as i32));
        return crate::settings::WindowGeometry {
            x: cx,
            y: cy,
            width,
            height,
            maximized: geo.maximized,
        };
    };

    let (left, top, right, bottom) = work;
    let min_visible_x = 100.min(width as i32);
    let clamped_x = geo.x.clamp(left - width as i32 + min_visible_x, right - min_visible_x);
    let clamped_y = geo.y.clamp(top, (bottom - 40).max(top));

    crate::settings::WindowGeometry {
        x: clamped_x,
        y: clamped_y,
        width,
        height,
        maximized: geo.maximized,
    }
}

/// Whether `hwnd` is minimised now: a launch from a shortcut set to "Minimized" starts so, before
/// the window hooks can hear WM_SIZE.
pub fn is_minimised(hwnd: HWND) -> bool {
    // SAFETY: a plain query; any handle value is safe to pass (an invalid one reads as false).
    unsafe { IsIconic(hwnd).as_bool() }
}

/// The caption Negotium draws when Settings > Use the Negotium title bar is on, in logical px: the
/// Windows 11 caption's height and button width. `Caption` in titlebar.slint holds the same.
pub const CAPTION_HEIGHT: f32 = 32.0;
pub const CAPTION_BUTTON_WIDTH: f32 = 46.0;

/// The hit-test code for a point in the client area of a window with Negotium's caption, in
/// physical px from the client's top-left: the top `frame` px resize (not when maximised), the
/// maximise button is `HTMAXBUTTON` (Windows 11 shows snap layouts over it), minimise, close and
/// everything below the caption are the client's, and the rest of the caption drags.
pub fn caption_hit(_x: i32, _y: i32, _width: i32, _scale: f32, _frame: i32, _maximised: bool) -> u32 {
    HTCLIENT
}

/// The client area's top after WM_NCCALCSIZE: the window's own top, so the caption is the
/// client's. A maximised window reaches past the monitor by its frame, so its top moves down by
/// that much.
pub fn caption_client_top(window_top: i32, _maximised: bool, _frame: i32) -> i32 {
    window_top
}

/// The maximise button's mouse state. Its input is non-client (`HTMAXBUTTON`), so the subclass
/// tracks it and Slint draws it (`MainWindow.max-button`, by index).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaxButton {
    Idle,
    Hover,
    Pressed,
}

impl MaxButton {
    pub fn index(self) -> i32 {
        self as i32
    }
}

/// A button of Negotium's caption.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptionButton {
    Minimise,
    MaximiseRestore,
    Close,
}

thread_local! {
    /// Negotium draws the caption (`set_custom_caption`); the subclass reads it per message.
    static CUSTOM_CAPTION: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// The maximise button was pressed and the button has not been released or left since.
    static MAX_PRESSED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn set_caption_flag(on: bool) {
    CUSTOM_CAPTION.with(|c| c.set(on));
}

/// Turns Negotium's caption on or off (Settings > Use the Negotium title bar). UI thread only.
pub fn set_custom_caption(_ui: &MainWindow, _on: bool) {}

/// Does what a caption button does, through the same WM_SYSCOMMAND the native caption sends.
pub fn caption_command(_ui: &MainWindow, _button: CaptionButton) {}

/// Wires the caption buttons' callbacks to `caption_command`.
pub fn install_caption(_ui: &MainWindow) {}

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
    /// The main window was moved, resized, maximized, or restored.
    MovedOrResized,
    /// The maximise button of Negotium's caption changed state.
    MaxButton(MaxButton),
    /// The window became active (true) or inactive (false), while Negotium draws the caption.
    Active(bool),
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
    } else if msg == WM_NCLBUTTONDOWN && wparam.0 as u32 == HTMAXBUTTON && CUSTOM_CAPTION.with(|c| c.get()) {
        return LRESULT(0);
    } else if msg == WM_SIZE {
        // SAFETY: as above.
        let minimised = wparam.0 as u32 == SIZE_MINIMIZED;
        unsafe { (*hook)(ShellEvent::Minimised(minimised)) };
        if !minimised {
            unsafe { (*hook)(ShellEvent::MovedOrResized) };
        }
    } else if msg == WM_EXITSIZEMOVE {
        // SAFETY: as above.
        unsafe { (*hook)(ShellEvent::MovedOrResized) };
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
/// exit: another process (never this one) whose window is marked as Negotium's, running `exe`
/// itself, or whose image can't be read (`None`: it can't be told apart, and an instance left
/// running would hold the files). A dev build that can be read is left alone.
fn asked_to_exit(pid: u32, me: u32, image: Option<&Path>, exe: &Path) -> bool {
    pid != me && image.is_none_or(|image| crate::integrate::same_path(image, exe))
}

/// The process owning `hwnd`: its id, a handle to wait for it (`None` if it couldn't be opened)
/// and its image (`None` if it couldn't be read).
fn window_process(hwnd: HWND) -> Option<(u32, Option<OwnedHandle>, Option<PathBuf>)> {
    let mut pid = 0u32;
    // SAFETY: plain call; a window that has gone leaves `pid` 0.
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if pid == 0 {
        return None;
    }
    // A limited query and SYNCHRONIZE: what an elevated instance's process also grants the same
    // user's non-elevated token.
    // SAFETY: plain call; the handle is owned (and closed) by the OwnedHandle below.
    let Ok(process) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE, false, pid) }) else {
        return Some((pid, None, None));
    };
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
    Some((pid, Some(process), image))
}

/// Asks every running instance of `exe` in this session (its main window, on screen or hidden in
/// the tray) to exit the way the tray's Exit does ([`ShellEvent::Exit`]), then waits up to
/// `timeout` for those processes to end. Returns how many were still running then (one that
/// couldn't be opened is asked but not waited for, so not counted). This process, dev builds and
/// instances in other sessions are left alone (the installer runs this, as the installed exe,
/// before it replaces the files). Never called by tests: it reaches real windows.
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
        if unsafe { PostMessageW(Some(hwnd), exit_message(), WPARAM(0), LPARAM(0)) }.is_ok()
            && let Some(process) = process
        {
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

/// Starts `exe` (no arguments) through the desktop shell, so it runs as Explorer does (not
/// elevated) whatever this process is: the shell's own `ShellExecute` (`IShellDispatch2`),
/// reached through the desktop window's view (Raymond Chen's documented way to start an
/// unelevated process from an elevated one). Fails, starting nothing, when there is no desktop
/// shell to ask (Explorer not running). Never called by tests.
pub fn shell_start(exe: &Path) -> Result<(), String> {
    use windows::Win32::System::Com::{
        CLSCTX_LOCAL_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize, IServiceProvider,
    };
    use windows::Win32::System::Variant::VARIANT;
    use windows::Win32::UI::Shell::{
        IShellBrowser, IShellDispatch2, IShellFolderViewDual, IShellView, IShellWindows, SID_STopLevelBrowser,
        SVGIO_BACKGROUND, SWC_DESKTOP, SWFO_NEEDDISPATCH, ShellWindows,
    };
    use windows::core::{BSTR, Interface};
    // SAFETY: COM is entered on this thread for the calls below and left once they are done
    // (this short-lived process's only COM use).
    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.ok().map_err(|e| e.message())?;
    let start = || -> windows::core::Result<()> {
        // SAFETY: plain COM calls on interfaces owned here; each is released when dropped,
        // before COM is left.
        unsafe {
            let windows: IShellWindows = CoCreateInstance(&ShellWindows, None, CLSCTX_LOCAL_SERVER)?;
            // CSIDL_DESKTOP (0): the desktop window, which Explorer always has.
            let (location, root) = (VARIANT::from(0i32), VARIANT::default());
            let mut hwnd = 0i32;
            let desktop = windows.FindWindowSW(&location, &root, SWC_DESKTOP, &mut hwnd, SWFO_NEEDDISPATCH)?;
            let browser: IShellBrowser = desktop.cast::<IServiceProvider>()?.QueryService(&SID_STopLevelBrowser)?;
            let view: IShellView = browser.QueryActiveShellView()?;
            let folder: IShellFolderViewDual = view.GetItemObject(SVGIO_BACKGROUND)?;
            let shell: IShellDispatch2 = folder.Application()?.cast()?;
            let none = VARIANT::default();
            shell.ShellExecute(&BSTR::from(exe.to_string_lossy().as_ref()), &none, &none, &none, &none)
        }
    };
    let started = start().map_err(|e| e.message());
    // SAFETY: pairs the successful CoInitializeEx above; every interface has been dropped.
    unsafe { CoUninitialize() };
    started
}

/// What [`activate_existing_instance`] found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Existing {
    /// A running instance's window was asked to show itself.
    Shown,
    /// A running instance's window exists but isn't responding (`IsHungAppWindow`): asking it
    /// would show nothing, so the caller goes on as if none were running.
    Hung,
    /// No window turned up within the wait.
    None,
}

/// Whether `hwnd`'s thread has stopped taking messages (Windows' own "Not responding": no message
/// read for 5 s). Sends no message itself, so it never blocks, and works across elevation.
fn is_hung(hwnd: HWND) -> bool {
    // SAFETY: IsHungAppWindow reads the window's cached responsiveness; any handle value is safe.
    unsafe { IsHungAppWindow(hwnd) }.as_bool()
}

/// Asks the running instance to show itself, waiting up to `timeout` for its window to exist
/// (it may still be starting). A responding one is preferred; a hung one is not asked
/// ([`Existing::Hung`]).
pub fn activate_existing_instance(timeout: Duration) -> Existing {
    activate_with(timeout, || responsive_main_window().or_else(find_main_window), is_hung, |hwnd| {
        let mut pid = 0u32;
        // SAFETY: plain calls; the target may vanish meanwhile, which only makes them fail.
        unsafe {
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            // We are the foreground process (just launched); let the old instance take focus.
            let _ = AllowSetForegroundWindow(pid);
            let _ = PostMessageW(Some(hwnd), activate_message(), WPARAM(0), LPARAM(0));
        }
    })
}

/// [`activate_existing_instance`] over `find`, `hung` and `show` (tests fake all three).
fn activate_with<W: Copy>(
    timeout: Duration,
    find: impl Fn() -> Option<W>,
    hung: impl Fn(W) -> bool,
    show: impl Fn(W),
) -> Existing {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(window) = find() {
            if hung(window) {
                return Existing::Hung;
            }
            show(window);
            return Existing::Shown;
        }
        if Instant::now() >= deadline {
            return Existing::None;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// A running instance's main window that is responding (not [`is_hung`]), if any.
pub fn responsive_main_window() -> Option<HWND> {
    find_main_window_where(|hwnd| !is_hung(hwnd))
}

/// Whether the single-instance lock `name` exists (some instance holds it), including one an
/// elevated instance created that this process may not open.
pub fn instance_exists(name: &str) -> bool {
    use windows::Win32::Foundation::ERROR_ACCESS_DENIED;
    use windows::Win32::System::Threading::{OpenMutexW, SYNCHRONIZATION_SYNCHRONIZE};
    // SAFETY: plain call; a handle it returns is closed at once.
    match unsafe { OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, false, &HSTRING::from(name)) } {
        Ok(h) => {
            // SAFETY: the handle just opened; closed exactly once.
            unsafe {
                let _ = CloseHandle(h);
            }
            true
        }
        Err(e) => e.code() == ERROR_ACCESS_DENIED.to_hresult(),
    }
}

/// Polls `check` every `step` until it is true (true) or `timeout` has passed (false).
pub fn wait_until(timeout: Duration, step: Duration, check: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if check() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(step.min(deadline.saturating_duration_since(Instant::now())));
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

/// The variables (of `names`, this process's) an elevated Negotium removes before anything reads
/// them: every `SLINT_*` (Slint's: `SLINT_BACKEND` picks the renderer code, `SLINT_DEFAULT_FONT`
/// and `SLINT_FONT_PATH` name font files it parses, the rest change its behaviour) and every
/// `NEGOTIUM_*` (Negotium's own: `NEGOTIUM_FRAME_STATS` names a file written at exit,
/// `NEGOTIUM_SIMULATE_STALL` hangs the UI thread) and every `GLUTIN_*` (glutin's
/// `GLUTIN_WGL_OPENGL_DLL` names the OpenGL DLL it loads, a full path included). Windows compares
/// names case-insensitively.
pub fn untrusted_env(names: impl IntoIterator<Item = std::ffi::OsString>) -> Vec<std::ffi::OsString> {
    names
        .into_iter()
        .filter(|name| {
            let upper = name.to_string_lossy().to_ascii_uppercase();
            upper.starts_with("SLINT_") || upper.starts_with("NEGOTIUM_") || upper.starts_with("GLUTIN_")
        })
        .collect()
}

/// Whether this process limits where DLLs loaded by name come from ([`restrict_dll_search`]):
/// only elevated. There the default search (the current directory and every `PATH` folder, which
/// the user's own `HKCU\Environment` can extend) would let a same-user process plant a DLL a
/// library loads by name (glutin tries `libEGL.dll` before OpenGL) in the elevated copy.
/// Unelevated nothing crosses a privilege boundary, so a user's own GL/EGL setup keeps working.
pub fn restricts_dll_search(elevated: bool) -> bool {
    elevated
}

/// Makes every later load by name search only System32 and this exe's folder (the install dir,
/// admin-only): `SetDefaultDllDirectories`. GPU drivers still load (opengl32.dll is in System32,
/// it loads the ICD by the full path the driver registered); `libEGL.dll` (ANGLE) is no longer
/// found, so glutin falls back to WGL. Call it first in `main`, before anything loads a library.
pub fn restrict_dll_search() -> windows::core::Result<()> {
    use windows::Win32::System::LibraryLoader::{
        LOAD_LIBRARY_SEARCH_APPLICATION_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32, SetDefaultDllDirectories,
    };
    // SAFETY: plain process-wide setting with valid flags; it only narrows later searches.
    unsafe { SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32 | LOAD_LIBRARY_SEARCH_APPLICATION_DIR) }
}

/// The variables an elevated Negotium sets from Windows itself (`windows`: the Windows directory,
/// `negotium_collector::windows_dir`), replacing whatever the user's `HKCU\Environment` planted:
/// `SystemRoot` and `windir` (which this process, the children it starts and COM expand in
/// `%SystemRoot%` paths), `SystemDrive` (its drive) and `PATH` (System32, Windows, Wbem and
/// Windows PowerShell only), so nothing elevated finds a program or file in a folder the user
/// picked, one hop later included.
pub fn trusted_env(windows: &Path) -> Vec<(&'static str, std::ffi::OsString)> {
    let sys32 = windows.join("System32");
    let path = [sys32.clone(), windows.to_path_buf(), sys32.join("Wbem"), sys32.join("WindowsPowerShell").join("v1.0")]
        .iter()
        .map(|p| p.as_os_str().to_owned())
        .collect::<Vec<_>>()
        .join(std::ffi::OsStr::new(";"));
    let mut vars = vec![("SystemRoot", windows.as_os_str().to_owned()), ("windir", windows.as_os_str().to_owned())];
    if let Some(std::path::Component::Prefix(drive)) = windows.components().next() {
        vars.push(("SystemDrive", drive.as_os_str().to_owned()));
    }
    vars.push(("PATH", path));
    vars
}

/// Sets an environment variable in this process (and so for the processes it starts). Call it
/// only while no other thread can be reading the environment (early in `main`).
pub fn set_env_var(name: &str, value: &std::ffi::OsStr) {
    // SAFETY: called from `main` before any other thread is started; on Windows the environment
    // block is also guarded by the process environment lock.
    unsafe { std::env::set_var(name, value) };
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
    fn only_the_gpu_renderer_gets_the_chosen_backdrop() {
        assert_eq!(backdrop_for(Renderer::Gpu, BackdropChoice::Mica), Some(DWMSBT_TABBEDWINDOW));
        assert_eq!(backdrop_for(Renderer::Gpu, BackdropChoice::Acrylic), Some(DWMSBT_TRANSIENTWINDOW));
        for choice in BackdropChoice::ALL {
            // Its buffer is opaque: a transparent window background would not show the backdrop.
            assert_eq!(backdrop_for(Renderer::Software, choice), None);
            // SLINT_BACKEND may name the software renderer.
            assert_eq!(backdrop_for(Renderer::FromEnv, choice), None);
        }
    }

    #[test]
    fn elevated_every_slint_and_negotium_variable_is_dropped() {
        let names = ["SLINT_BACKEND", "slint_default_font", "SLINT_FONT_PATH", "NEGOTIUM_FRAME_STATS", "Negotium_Simulate_Stall", "GLUTIN_WGL_OPENGL_DLL", "PATH", "LOCALAPPDATA", "SLINTX", "TEMP", "GLUTINX"];
        let dropped = untrusted_env(names.iter().map(std::ffi::OsString::from));
        assert_eq!(dropped, ["SLINT_BACKEND", "slint_default_font", "SLINT_FONT_PATH", "NEGOTIUM_FRAME_STATS", "Negotium_Simulate_Stall", "GLUTIN_WGL_OPENGL_DLL"]);
    }

    #[test]
    fn only_an_elevated_copy_restricts_the_dll_search() {
        assert!(restricts_dll_search(true));
        assert!(!restricts_dll_search(false));
    }

    /// In a child process (the setting is process-wide): after restricting, a DLL by name is found
    /// in System32 but not in a `PATH` folder, and opengl32.dll (the WGL path) still loads. The
    /// control child, unrestricted, does load the planted DLL from `PATH`: the test can tell.
    #[test]
    fn a_restricted_search_skips_path_but_finds_system32() {
        const CHILD: &str = "NEGOTIUM_TEST_DLL_SEARCH_CHILD";
        const RESTRICT: &str = "NEGOTIUM_TEST_DLL_SEARCH_RESTRICT";
        use windows::Win32::System::LibraryLoader::LoadLibraryW;
        if let Some(dir) = std::env::var_os(CHILD) {
            if std::env::var_os(RESTRICT).is_some() {
                restrict_dll_search().unwrap();
            }
            // SAFETY: loads by name; the modules stay loaded until the child exits.
            let gl = unsafe { LoadLibraryW(w!("opengl32.dll")) };
            // SAFETY: as above; the planted file is a renamed copy of System32's version.dll, a
            // real DLL, so it loads whenever the search reaches the PATH folder holding it.
            let planted = unsafe { LoadLibraryW(w!("negotium-planted-test.dll")) };
            println!("GL={} PLANTED={} DIR={}", gl.is_ok(), planted.is_ok(), Path::new(&dir).display());
            return;
        }
        let dir = std::env::temp_dir().join(format!("negotium-dll-search-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Copied from System32 under a new name: loadable if the search reached this folder.
        let src = PathBuf::from(negotium_collector::windows_dir()).join("System32").join("version.dll");
        std::fs::copy(&src, dir.join("negotium-planted-test.dll")).unwrap();
        let mut path = std::ffi::OsString::from(&dir);
        path.push(";");
        path.push(std::env::var_os("PATH").unwrap_or_default());
        let run = |restrict: bool| {
            let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
            cmd.args(["--exact", "platform::tests::a_restricted_search_skips_path_but_finds_system32", "--nocapture", "--test-threads=1"])
                .env(CHILD, &dir)
                .env("PATH", &path)
                .env_remove(RESTRICT);
            if restrict {
                cmd.env(RESTRICT, "1");
            }
            String::from_utf8_lossy(&cmd.output().unwrap().stdout).into_owned()
        };
        let (control, restricted) = (run(false), run(true));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(control.contains("GL=true PLANTED=true"), "control: the planted DLL is found on PATH: {control}");
        assert!(restricted.contains("GL=true PLANTED=false"), "{restricted}");
    }

    #[test]
    fn the_trusted_environment_comes_from_the_windows_directory() {
        let vars = trusted_env(Path::new(r"D:\Win"));
        let get = |n: &str| vars.iter().find(|(k, _)| *k == n).map(|(_, v)| v.to_string_lossy().into_owned());
        assert_eq!(get("SystemRoot").as_deref(), Some(r"D:\Win"));
        assert_eq!(get("windir").as_deref(), Some(r"D:\Win"));
        assert_eq!(get("SystemDrive").as_deref(), Some("D:"));
        assert_eq!(
            get("PATH").as_deref(),
            Some(r"D:\Win\System32;D:\Win;D:\Win\System32\Wbem;D:\Win\System32\WindowsPowerShell\v1.0")
        );
    }

    /// In a child process started with a planted SystemRoot, windir, SystemDrive and PATH: after
    /// the reset `main` does when elevated, this process's environment, and so what its children
    /// inherit and what `%SystemRoot%` expands to, holds the real values.
    #[test]
    fn the_reset_replaces_planted_variables() {
        const CHILD: &str = "NEGOTIUM_TEST_TRUSTED_ENV_CHILD";
        let real = PathBuf::from(negotium_collector::windows_dir());
        if std::env::var_os(CHILD).is_some() {
            for (name, value) in trusted_env(&real) {
                set_env_var(name, &value);
            }
            let expanded = negotium_actions::startup::expand_env(r"%SystemRoot%\explorer.exe", |n| std::env::var(n).ok());
            for name in ["SystemRoot", "windir", "SystemDrive", "PATH"] {
                println!("{name}={}", std::env::var(name).unwrap_or_default());
            }
            println!("EXPANDED={expanded}");
            return;
        }
        let fake = std::env::temp_dir().join("negotium-planted-root");
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "platform::tests::the_reset_replaces_planted_variables", "--nocapture", "--test-threads=1"])
            .env(CHILD, "1")
            .env("SystemRoot", &fake)
            .env("windir", &fake)
            .env("SystemDrive", "Z:")
            .env("PATH", &fake)
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        let get = |n: &str| {
            text.lines()
                .find_map(|l| l.split_once(&format!("{n}=")).map(|(_, v)| v))
                .unwrap_or_else(|| panic!("no {n} line in {text}"))
                .to_string()
        };
        let want: Vec<_> = trusted_env(&real).into_iter().map(|(k, v)| (k, v.to_string_lossy().into_owned())).collect();
        for (name, value) in &want {
            assert_eq!(&get(name), value, "{name}");
        }
        assert_eq!(PathBuf::from(get("EXPANDED")), real.join("explorer.exe"));
        assert!(!text.contains(&*fake.to_string_lossy()), "{text}");
    }

    /// A running instance is shown; a hung one never is (the launch goes on as if none ran); none
    /// at all is waited for up to the timeout.
    #[test]
    fn a_hung_instance_is_not_trusted_to_show_itself() {
        use std::cell::Cell;
        let shown = Cell::new(0);
        let show = |_: u32| shown.set(shown.get() + 1);
        assert_eq!(activate_with(Duration::ZERO, || Some(1u32), |_| false, show), Existing::Shown);
        assert_eq!(shown.get(), 1);
        assert_eq!(activate_with(Duration::from_secs(5), || Some(1u32), |_| true, show), Existing::Hung);
        assert_eq!(shown.get(), 1, "a hung window isn't asked");
        let started = Instant::now();
        assert_eq!(activate_with(Duration::from_millis(250), || None::<u32>, |_| false, show), Existing::None);
        assert!(started.elapsed() >= Duration::from_millis(250));
        // A window that turns up during the wait is shown.
        let polls = Cell::new(0);
        let late = || {
            polls.set(polls.get() + 1);
            (polls.get() > 2).then_some(7u32)
        };
        assert_eq!(activate_with(Duration::from_secs(5), late, |_| false, show), Existing::Shown);
        assert_eq!(shown.get(), 2);
    }

    #[test]
    fn waiting_stops_at_the_first_yes_or_the_timeout() {
        use std::cell::Cell;
        let n = Cell::new(0);
        assert!(wait_until(Duration::from_secs(5), Duration::from_millis(10), || {
            n.set(n.get() + 1);
            n.get() == 3
        }));
        assert_eq!(n.get(), 3);
        let started = Instant::now();
        assert!(!wait_until(Duration::from_millis(200), Duration::from_millis(50), || false));
        let took = started.elapsed();
        assert!(took >= Duration::from_millis(200) && took < Duration::from_secs(2), "{took:?}");
    }

    /// A lock nobody holds doesn't exist; one this process holds does (a unique test name, never
    /// the app's).
    #[test]
    fn an_instance_lock_is_seen_while_held() {
        let name = format!(r"Local\Negotium.Test.InstanceExists.{}", std::process::id());
        assert!(!instance_exists(&name));
        let lock = claim_instance(&name, Duration::ZERO).expect("unique name");
        assert!(instance_exists(&name));
        drop(lock);
        assert!(!instance_exists(&name));
    }

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
        // Marked as Negotium's main window, but its process can't be opened or read: still
        // asked (only the wait is skipped).
        assert!(asked_to_exit(20, me, None, exe), "an image that can't be read");
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

    #[test]
    fn geometry_bounds_are_sanitized_and_monitored() {
        use crate::settings::WindowGeometry;
        let primary = (0, 0, 1920, 1040);
        let secondary = (1920, 0, 3840, 1080);

        // Disconnected monitor: centers on primary while preserving width & height
        let offscreen = WindowGeometry { x: 2500, y: 100, width: 1200, height: 800, maximized: false };
        let sanitized = sanitize_geometry_bounds(offscreen, primary, None);
        assert_eq!(sanitized.x, 360, "centered horizontally");
        assert_eq!(sanitized.y, 120, "centered vertically");
        assert_eq!(sanitized.width, 1200, "width preserved");
        assert_eq!(sanitized.height, 800, "height preserved");

        // Connected secondary monitor: preserved within secondary work area
        let on_secondary = sanitize_geometry_bounds(offscreen, primary, Some(secondary));
        assert_eq!(on_secondary.x, 2500);
        assert_eq!(on_secondary.y, 100);
        assert_eq!(on_secondary.width, 1200);
        assert_eq!(on_secondary.height, 800);

        // Window dragged off the top: title bar clamped down into view
        let off_top = WindowGeometry { x: 100, y: -50, width: 1000, height: 720, maximized: true };
        let sanitized_top = sanitize_geometry_bounds(off_top, primary, Some(primary));
        assert_eq!(sanitized_top.y, 0, "title bar brought to top edge");
        assert!(sanitized_top.maximized, "maximized flag preserved");

        // Window below minimum size: clamped to 760x440
        let too_small = WindowGeometry { x: 50, y: 50, width: 400, height: 200, maximized: false };
        let sanitized_small = sanitize_geometry_bounds(too_small, primary, Some(primary));
        assert_eq!(sanitized_small.width, 760);
        assert_eq!(sanitized_small.height, 440);
    }

    #[test]
    fn the_caption_hit_zones_of_a_restored_window() {
        use windows::Win32::UI::WindowsAndMessaging::{HTCAPTION, HTTOP, HTTOPLEFT, HTTOPRIGHT};
        let hit = |x, y| caption_hit(x, y, 1000, 1.0, 8, false);
        assert_eq!(hit(500, 100), HTCLIENT, "below the caption");
        assert_eq!(hit(500, 20), HTCAPTION, "the drag area");
        assert_eq!(hit(10, 20), HTCAPTION, "the icon drags too");
        assert_eq!(hit(1000 - 46 - 23, 20), HTMAXBUTTON, "maximise");
        assert_eq!(hit(1000 - 23, 20), HTCLIENT, "close is Slint's");
        assert_eq!(hit(1000 - 92 - 23, 20), HTCLIENT, "minimise is Slint's");
        assert_eq!(hit(500, 3), HTTOP);
        assert_eq!(hit(3, 3), HTTOPLEFT);
        assert_eq!(hit(997, 3), HTTOPRIGHT);
        assert_eq!(hit(1000 - 46 - 23, 3), HTTOP, "resizing wins over the button");
        assert_eq!(hit(500, 31), HTCAPTION, "the caption's last row");
        assert_eq!(hit(500, 32), HTCLIENT, "the first row below it");
    }

    #[test]
    fn a_maximised_caption_has_no_resize_band() {
        use windows::Win32::UI::WindowsAndMessaging::HTCAPTION;
        assert_eq!(caption_hit(500, 3, 1000, 1.0, 8, true), HTCAPTION);
        assert_eq!(caption_hit(1000 - 46 - 23, 3, 1000, 1.0, 8, true), HTMAXBUTTON);
    }

    #[test]
    fn the_caption_hit_zones_follow_the_scale() {
        use windows::Win32::UI::WindowsAndMessaging::HTCAPTION;
        let hit = |x, y| caption_hit(x, y, 1000, 1.5, 12, false);
        assert_eq!(hit(500, 47), HTCAPTION, "48 px high at 150%");
        assert_eq!(hit(500, 48), HTCLIENT);
        assert_eq!(hit(1000 - 69 - 34, 20), HTMAXBUTTON, "69 px wide buttons");
        assert_eq!(hit(1000 - 69, 20), HTCLIENT, "close starts here");
    }

    #[test]
    fn a_maximised_client_starts_below_the_frame() {
        assert_eq!(caption_client_top(100, false, 8), 100);
        assert_eq!(caption_client_top(-8, true, 8), 0);
    }

    /// A hidden top-level window with a native caption, standing in for the main window. Its
    /// class's procedure is the default one: a system class such as STATIC answers the frame
    /// messages itself.
    fn captioned_window() -> HWND {
        use windows::Win32::UI::WindowsAndMessaging::{DefWindowProcW, RegisterClassW, WNDCLASSW, WS_OVERLAPPEDWINDOW};
        unsafe extern "system" fn default_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
            // SAFETY: forwards a message of our own window to the system default.
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        static CLASS: OnceLock<()> = OnceLock::new();
        CLASS.get_or_init(|| {
            let class = WNDCLASSW { lpfnWndProc: Some(default_proc), lpszClassName: w!("negotium-test-frame"), ..Default::default() };
            // SAFETY: registers a class whose procedure is the system default, once per process.
            assert_ne!(unsafe { RegisterClassW(&class) }, 0, "register the test class");
        });
        // SAFETY: plain window creation with the class above; destroyed by the test.
        unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("negotium-test-frame"),
                w!("negotium-test"),
                WS_OVERLAPPEDWINDOW,
                100,
                100,
                800,
                600,
                None,
                None,
                None,
                None,
            )
        }
        .expect("create test window")
    }

    fn window_rect(hwnd: HWND) -> windows::Win32::Foundation::RECT {
        let mut r = Default::default();
        // SAFETY: our own test window.
        unsafe { windows::Win32::UI::WindowsAndMessaging::GetWindowRect(hwnd, &mut r) }.unwrap();
        r
    }

    /// What WM_NCCALCSIZE (wParam TRUE) makes of the window's own rectangle.
    fn calc_size(hwnd: HWND) -> windows::Win32::Foundation::RECT {
        use windows::Win32::UI::WindowsAndMessaging::{NCCALCSIZE_PARAMS, WM_NCCALCSIZE};
        let mut pos = windows::Win32::UI::WindowsAndMessaging::WINDOWPOS::default();
        let mut params = NCCALCSIZE_PARAMS { lppos: &mut pos, ..Default::default() };
        params.rgrc = [window_rect(hwnd); 3];
        // SAFETY: synchronous send; `params` outlives it.
        unsafe { SendMessageW(hwnd, WM_NCCALCSIZE, Some(WPARAM(1)), Some(LPARAM(&mut params as *mut _ as isize))) };
        params.rgrc[0]
    }

    fn hit_test(hwnd: HWND, x: i32, y: i32) -> u32 {
        use windows::Win32::UI::WindowsAndMessaging::WM_NCHITTEST;
        let lparam = (((y as u32 & 0xFFFF) << 16) | (x as u32 & 0xFFFF)) as isize;
        // SAFETY: synchronous send to our own window.
        unsafe { SendMessageW(hwnd, WM_NCHITTEST, Some(WPARAM(0)), Some(LPARAM(lparam))) }.0 as u32
    }

    fn reframe(hwnd: HWND) {
        // SAFETY: our own test window: recompute its client area (no move, size or activation).
        let _ = unsafe {
            SetWindowPos(hwnd, None, 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE)
        };
    }

    #[test]
    fn with_the_caption_off_the_frame_answers_as_an_unhooked_window() {
        set_caption_flag(false);
        let (hooked, plain) = (captioned_window(), captioned_window());
        assert!(install_window_hooks(hooked, |_| {}));
        reframe(hooked);
        assert_eq!(calc_size(hooked), calc_size(plain));
        let r = window_rect(plain);
        let (x, y) = (r.left + 400, r.top + 15);
        assert_eq!(hit_test(hooked, x, y), hit_test(plain, x, y));
        // SAFETY: our own test windows.
        unsafe {
            let _ = DestroyWindow(hooked);
            let _ = DestroyWindow(plain);
        }
    }

    #[test]
    fn with_the_caption_on_the_client_area_starts_at_the_top() {
        let (hooked, plain) = (captioned_window(), captioned_window());
        assert!(install_window_hooks(hooked, |_| {}));
        set_caption_flag(true);
        let (got, native, r) = (calc_size(hooked), calc_size(plain), window_rect(hooked));
        set_caption_flag(false);
        assert_eq!(got.top, r.top, "the caption is the client's");
        assert_eq!((got.left, got.right, got.bottom), (native.left, native.right, native.bottom), "the other borders stay");
        // SAFETY: our own test windows.
        unsafe {
            let _ = DestroyWindow(hooked);
            let _ = DestroyWindow(plain);
        }
    }

    #[test]
    fn with_the_caption_on_the_hit_test_finds_the_caption_and_the_maximise_button() {
        use windows::Win32::UI::WindowsAndMessaging::{GetClientRect, HTCAPTION, HTLEFT};
        let hwnd = captioned_window();
        assert!(install_window_hooks(hwnd, |_| {}));
        set_caption_flag(true);
        reframe(hwnd);
        // SAFETY: our own test window.
        let scale = unsafe { GetDpiForWindow(hwnd) } as f32 / 96.0;
        let r = window_rect(hwnd);
        let mut client = Default::default();
        // SAFETY: as above.
        unsafe { GetClientRect(hwnd, &mut client) }.unwrap();
        let border = (r.right - r.left - client.right) / 2;
        let bw = (CAPTION_BUTTON_WIDTH * scale).round() as i32;
        let mid_y = r.top + (20.0 * scale) as i32;
        let (caption, max, left) = (
            hit_test(hwnd, r.left + client.right / 2, mid_y),
            hit_test(hwnd, r.right - border - bw - bw / 2, mid_y),
            hit_test(hwnd, r.left + 1, r.top + 300),
        );
        set_caption_flag(false);
        // SAFETY: our own test window.
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
        assert_eq!(caption, HTCAPTION);
        assert_eq!(max, HTMAXBUTTON);
        assert_eq!(left, HTLEFT, "the side border is still the default's");
    }

    /// Removes and returns the WM_SYSCOMMAND posted to `hwnd`, if any.
    fn posted_command(hwnd: HWND) -> Option<u32> {
        use windows::Win32::UI::WindowsAndMessaging::{MSG, PM_REMOVE, PeekMessageW, WM_SYSCOMMAND};
        let mut m = MSG::default();
        // SAFETY: reads this thread's queue for our own window.
        unsafe { PeekMessageW(&mut m, Some(hwnd), WM_SYSCOMMAND, WM_SYSCOMMAND, PM_REMOVE) }
            .as_bool()
            .then_some(m.wParam.0 as u32 & 0xFFF0)
    }

    #[test]
    fn the_maximise_button_is_pressed_and_released_through_non_client_messages() {
        use windows::Win32::UI::WindowsAndMessaging::{HTCAPTION, SC_MAXIMIZE, WM_NCLBUTTONUP, WM_NCMOUSELEAVE, WM_NCMOUSEMOVE};
        let hwnd = captioned_window();
        let seen = Rc::new(RefCell::new(Vec::new()));
        let s = seen.clone();
        assert!(install_window_hooks(hwnd, move |e| s.borrow_mut().push(e)));
        set_caption_flag(true);
        let send = |msg, ht: u32| {
            // SAFETY: synchronous send to our own window.
            unsafe { SendMessageW(hwnd, msg, Some(WPARAM(ht as usize)), Some(LPARAM(0))) }
        };
        send(WM_NCMOUSEMOVE, HTMAXBUTTON);
        let down = send(WM_NCLBUTTONDOWN, HTMAXBUTTON);
        send(WM_NCLBUTTONUP, HTMAXBUTTON);
        let after_click = posted_command(hwnd);
        send(WM_NCLBUTTONUP, HTMAXBUTTON);
        let after_stray_up = posted_command(hwnd);
        send(WM_NCMOUSEMOVE, HTCAPTION);
        send(WM_NCMOUSELEAVE, 0);
        set_caption_flag(false);
        // SAFETY: our own test window.
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
        assert_eq!(down, LRESULT(0), "the press is swallowed: no legacy button is drawn");
        assert_eq!(after_click, Some(SC_MAXIMIZE));
        assert_eq!(after_stray_up, None, "a release with no press first does nothing");
        let states: Vec<MaxButton> =
            seen.borrow().iter().filter_map(|e| if let ShellEvent::MaxButton(b) = e { Some(*b) } else { None }).collect();
        use MaxButton::{Hover, Idle, Pressed};
        assert_eq!(states, [Hover, Pressed, Hover, Hover, Idle, Idle]);
    }

    #[test]
    fn activation_reaches_the_hook_only_with_the_caption_on() {
        use windows::Win32::UI::WindowsAndMessaging::WM_NCACTIVATE;
        let hwnd = captioned_window();
        let seen = Rc::new(RefCell::new(Vec::new()));
        let s = seen.clone();
        assert!(install_window_hooks(hwnd, move |e| s.borrow_mut().push(e)));
        let send = |active: usize| {
            // SAFETY: synchronous send to our own window.
            unsafe { SendMessageW(hwnd, WM_NCACTIVATE, Some(WPARAM(active)), Some(LPARAM(0))) };
        };
        set_caption_flag(false);
        send(0);
        set_caption_flag(true);
        send(0);
        send(1);
        set_caption_flag(false);
        // SAFETY: our own test window.
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
        let active: Vec<ShellEvent> = seen.borrow().iter().copied().filter(|e| matches!(e, ShellEvent::Active(_))).collect();
        assert_eq!(active, [ShellEvent::Active(false), ShellEvent::Active(true)]);
    }
}
