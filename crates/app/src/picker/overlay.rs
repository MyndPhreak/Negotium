//! Win32 side of the picker's overlays: one layered, topmost popup per monitor, drawn with
//! Direct2D into a DIB and shown with `UpdateLayeredWindow`; window messages turned into
//! `Input` for the session (`super::on_input`); mouse capture; and the task dialogs.
use std::ffi::c_void;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Foundation::{
    COLORREF, D2DERR_RECREATE_TARGET, HANDLE, HMODULE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM,
};
use windows::Win32::Graphics::Direct2D::Common::{D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_PIXEL_FORMAT};
use windows::Win32::Graphics::Direct2D::{
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_FEATURE_LEVEL_DEFAULT, D2D1_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_RENDER_TARGET_USAGE_NONE, D2D1CreateFactory, ID2D1DCRenderTarget, ID2D1Factory,
};
use windows::Win32::Graphics::DirectWrite::{DWRITE_FACTORY_TYPE_SHARED, DWriteCreateFactory, IDWriteFactory};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, CreateCompatibleDC, CreateDIBSection,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GdiFlush, HBITMAP, HDC, HGDIOBJ, SelectObject,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows::Win32::UI::Controls::{
    TASKDIALOG_BUTTON, TASKDIALOG_COMMON_BUTTON_FLAGS, TASKDIALOG_FLAGS, TASKDIALOGCONFIG, TD_WARNING_ICON,
    TDCBF_CANCEL_BUTTON, TDCBF_CLOSE_BUTTON, TDF_POSITION_RELATIVE_TO_WINDOW,
};
use windows::Win32::UI::HiDpi::{
    AreDpiAwarenessContextsEqual, DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    GetThreadDpiAwarenessContext, SetThreadDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow, GetCursorPos, IDC_CROSS, IDCANCEL, IDOK,
    LoadCursorW, MB_DEFBUTTON2, MB_ICONWARNING, MB_OK, MB_OKCANCEL, MESSAGEBOX_STYLE, MessageBoxW, RegisterClassExW,
    SW_SHOWNA, SetCursor, SetForegroundWindow, ShowWindow, ULW_ALPHA, UpdateLayeredWindow, WM_ACTIVATEAPP,
    WM_DISPLAYCHANGE, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_RBUTTONUP, WNDCLASSEXW,
    WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{BOOL, HRESULT, PCWSTR, s, w};

use super::scene::{Scene, render};
use super::win::Monitor;

/// What the user did, as the session sees it.
pub enum Input {
    Move,
    Down,
    Up { shift: bool },
    /// Esc, a right-click, another app taking the foreground, or a display change.
    Cancel,
}

const CLASS: PCWSTR = w!("NegotiumWindowPicker");
const TITLE: PCWSTR = w!("Negotium");
const VK_ESCAPE: usize = 0x1B;
const MK_SHIFT: usize = 0x0004;

/// The Direct2D and DirectWrite factories, made once per picker session.
pub struct Painter {
    d2d: ID2D1Factory,
    text: IDWriteFactory,
}

impl Painter {
    pub fn new() -> windows::core::Result<Self> {
        // SAFETY: factory creation has no preconditions; the factories are owned by the result.
        unsafe {
            Ok(Self {
                d2d: D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?,
                text: DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?,
            })
        }
    }
}

/// One monitor's overlay window and the 32-bpp DIB it is drawn into.
pub struct Overlay {
    hwnd: HWND,
    origin: POINT,
    size: SIZE,
    dc: HDC,
    bitmap: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u32,
    target: Option<ID2D1DCRenderTarget>,
}

impl Overlay {
    /// A hidden overlay covering `m`; shown by the first `paint`.
    pub fn create(m: &Monitor) -> windows::core::Result<Self> {
        register()?;
        let (w, h) = (m.rect.right - m.rect.left, m.rect.bottom - m.rect.top);
        let mut o = Overlay {
            hwnd: HWND::default(),
            origin: POINT { x: m.rect.left, y: m.rect.top },
            size: SIZE { cx: w, cy: h },
            dc: HDC::default(),
            bitmap: HBITMAP::default(),
            old: HGDIOBJ::default(),
            bits: std::ptr::null_mut(),
            target: None,
        };
        // SAFETY: each handle created here is stored in `o` at once, so Drop frees it even when a
        // later step fails. `bits` points into the DIB, which lives as long as `o`.
        unsafe {
            o.dc = CreateCompatibleDC(None);
            if o.dc.is_invalid() {
                return Err(windows::core::Error::from_thread());
            }
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h, // top-down
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut c_void = std::ptr::null_mut();
            o.bitmap = CreateDIBSection(Some(o.dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
            o.bits = bits.cast();
            o.old = SelectObject(o.dc, o.bitmap.into());
            o.hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
                CLASS,
                TITLE,
                WS_POPUP,
                o.origin.x,
                o.origin.y,
                w,
                h,
                None,
                None,
                GetModuleHandleW(None).ok().map(Into::into),
                None,
            )?;
        }
        Ok(o)
    }

    pub fn hwnd(&self) -> isize {
        self.hwnd.0 as isize
    }

    /// Draws `scene` and shows it. A lost device (`D2DERR_RECREATE_TARGET`) gets one new target.
    pub fn paint(&mut self, p: &Painter, scene: &Scene) -> windows::core::Result<()> {
        let drawn = match self.draw(p, scene) {
            Err(e) if e.code() == D2DERR_RECREATE_TARGET => {
                self.target = None;
                self.draw(p, scene)
            }
            r => r,
        };
        drawn?;
        let pixels = (self.size.cx.max(0) as usize) * (self.size.cy.max(0) as usize);
        // SAFETY: Direct2D has finished with the DIB (EndDraw) and GdiFlush completes any GDI
        // work on it; `bits` covers `pixels` u32 values of the DIB owned by `self`. The DCs
        // and window are owned by `self`; the structs passed are locals.
        unsafe {
            let _ = GdiFlush();
            // Fully clear pixels let clicks through to the window below, and capture alone does
            // not stop that without a button down. Alpha 1 (invisible) keeps the hole clickable.
            for px in std::slice::from_raw_parts_mut(self.bits, pixels) {
                if *px >> 24 == 0 {
                    *px = 0x0100_0000;
                }
            }
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            UpdateLayeredWindow(
                self.hwnd,
                None,
                Some(&self.origin),
                Some(&self.size),
                Some(self.dc),
                Some(&POINT::default()),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            )?;
            let _ = ShowWindow(self.hwnd, SW_SHOWNA);
        }
        Ok(())
    }

    fn draw(&mut self, p: &Painter, scene: &Scene) -> windows::core::Result<()> {
        let target = match &self.target {
            Some(t) => t.clone(),
            None => {
                let props = D2D1_RENDER_TARGET_PROPERTIES {
                    r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                    pixelFormat: D2D1_PIXEL_FORMAT {
                        format: DXGI_FORMAT_B8G8R8A8_UNORM,
                        alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                    },
                    dpiX: 0.0,
                    dpiY: 0.0,
                    usage: D2D1_RENDER_TARGET_USAGE_NONE,
                    minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
                };
                let rect = RECT { left: 0, top: 0, right: self.size.cx, bottom: self.size.cy };
                // SAFETY: `props` and `rect` are locals; `dc` holds the DIB for as long as `self`.
                let t = unsafe {
                    let t = p.d2d.CreateDCRenderTarget(&props)?;
                    t.BindDC(self.dc, &rect)?;
                    t
                };
                self.target = Some(t.clone());
                t
            }
        };
        render(&target, &p.text, scene)
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        self.target = None;
        // SAFETY: every handle was created by `create` and is released once, here; invalid
        // (never created) handles are skipped.
        unsafe {
            if !self.hwnd.is_invalid() {
                let _ = DestroyWindow(self.hwnd);
            }
            if !self.dc.is_invalid() {
                if !self.old.is_invalid() {
                    SelectObject(self.dc, self.old);
                }
                let _ = DeleteDC(self.dc);
            }
            if !self.bitmap.is_invalid() {
                let _ = DeleteObject(self.bitmap.into());
            }
        }
    }
}

/// Registers the overlay class once per process: crosshair cursor, `wndproc`.
fn register() -> windows::core::Result<()> {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.load(Ordering::Acquire) {
        return Ok(());
    }
    // SAFETY: the class struct is a local; the class name and cursor are static.
    unsafe {
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: GetModuleHandleW(None)?.into(),
            hCursor: LoadCursorW(None, IDC_CROSS)?,
            lpszClassName: CLASS,
            ..Default::default()
        };
        if RegisterClassExW(&class) == 0 {
            let e = windows::core::Error::from_thread();
            // ERROR_CLASS_ALREADY_EXISTS: registered before; nothing to do.
            if e.code() != HRESULT::from_win32(1410) {
                return Err(e);
            }
        }
    }
    DONE.store(true, Ordering::Release);
    Ok(())
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let input = match msg {
        WM_MOUSEMOVE => Some(Input::Move),
        WM_LBUTTONDOWN => Some(Input::Down),
        WM_LBUTTONUP => Some(Input::Up { shift: wparam.0 & MK_SHIFT != 0 }),
        // On the button-up, so the release does not reach the window below once the picker closes.
        WM_RBUTTONUP => Some(Input::Cancel),
        WM_KEYDOWN if wparam.0 == VK_ESCAPE => Some(Input::Cancel),
        WM_ACTIVATEAPP if wparam.0 == 0 => Some(Input::Cancel),
        WM_DISPLAYCHANGE => Some(Input::Cancel),
        _ => None,
    };
    match input {
        Some(i) => {
            super::on_input(hwnd.0 as isize, i);
            LRESULT(0)
        }
        // SAFETY: the arguments are the ones this window procedure received.
        None => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// The cursor in physical screen pixels.
pub fn cursor() -> (i32, i32) {
    let mut p = POINT::default();
    // SAFETY: `p` is a valid out pointer. On failure the origin is used: the picker shows nothing
    // there that a click could end by mistake (the target is fixed at button-down).
    let _ = unsafe { GetCursorPos(&mut p) };
    (p.x, p.y)
}

/// Brings `hwnd` to the foreground (so Esc reaches it) and captures the mouse to it.
pub fn grab(hwnd: isize) {
    windows::core::link!("user32.dll" "system" fn SetCapture(hwnd: HWND) -> HWND);
    let hwnd = HWND(hwnd as *mut c_void);
    // SAFETY: `hwnd` is an overlay of this thread. The crosshair is set at once rather than on
    // the next WM_SETCURSOR, which a captured mouse does not send.
    unsafe {
        let _ = SetForegroundWindow(hwnd);
        SetCapture(hwnd);
        if let Ok(c) = LoadCursorW(None, IDC_CROSS) {
            SetCursor(Some(c));
        }
    }
}

pub fn release() {
    windows::core::link!("user32.dll" "system" fn ReleaseCapture() -> BOOL);
    // SAFETY: no arguments; releasing when nothing is captured is harmless.
    unsafe {
        let _ = ReleaseCapture();
    }
}

/// Sets this thread to per-monitor-v2 DPI awareness while it lives, when it is not already
/// (winit normally makes the process per-monitor-v2), so monitor and window rects are physical.
pub struct DpiGuard(Option<DPI_AWARENESS_CONTEXT>);

impl DpiGuard {
    pub fn per_monitor_v2() -> Self {
        // SAFETY: plain Win32 calls on the current thread's DPI context.
        unsafe {
            if AreDpiAwarenessContextsEqual(GetThreadDpiAwarenessContext(), DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)
                .as_bool()
            {
                Self(None)
            } else {
                Self(Some(SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)))
            }
        }
    }

    /// Whether the thread was not per-monitor-v2 and had to be set.
    pub fn changed(&self) -> bool {
        self.0.is_some()
    }
}

impl Drop for DpiGuard {
    fn drop(&mut self) {
        if let Some(old) = self.0 {
            // SAFETY: restores the context saved by `per_monitor_v2` on this thread.
            unsafe {
                SetThreadDpiAwarenessContext(old);
            }
        }
    }
}

/// Asks "End …?" above `owner`, Cancel the default. `true` when the user chose End.
pub fn confirm(owner: isize, title: &str, body: &str) -> bool {
    let end = wide("End");
    let buttons = [TASKDIALOG_BUTTON { nButtonID: IDOK.0, pszButtonText: PCWSTR(end.as_ptr()) }];
    match task_dialog(Some(owner), title, body, &buttons, TDCBF_CANCEL_BUTTON, IDCANCEL.0, TD_WARNING_ICON) {
        Some(pressed) => pressed == IDOK.0,
        None => message_box(Some(owner), title, body, MB_OKCANCEL | MB_DEFBUTTON2 | MB_ICONWARNING) == IDOK.0,
    }
}

/// Shows `text` with a Close button only; above `owner` when given. Any thread.
pub fn tell(owner: Option<isize>, text: &str) {
    if task_dialog(owner, "", text, &[], TDCBF_CLOSE_BUTTON, 0, PCWSTR::null()).is_none() {
        message_box(owner, "", text, MB_OK);
    }
}

type TaskDialogIndirectFn = unsafe extern "system" fn(*const TASKDIALOGCONFIG, *mut i32, *mut i32, *mut BOOL) -> HRESULT;

/// The pressed button, or `None` when the task dialog is not available or failed.
fn task_dialog(
    owner: Option<isize>,
    instruction: &str,
    content: &str,
    buttons: &[TASKDIALOG_BUTTON],
    common: TASKDIALOG_COMMON_BUTTON_FLAGS,
    default: i32,
    icon: PCWSTR,
) -> Option<i32> {
    let (ctx, func) = comctl6()?;
    let (instruction, content) = (wide(instruction), wide(content));
    let mut config = TASKDIALOGCONFIG {
        cbSize: size_of::<TASKDIALOGCONFIG>() as u32,
        hwndParent: owner.map_or(HWND::default(), |h| HWND(h as *mut c_void)),
        dwFlags: if owner.is_some() { TDF_POSITION_RELATIVE_TO_WINDOW } else { TASKDIALOG_FLAGS(0) },
        dwCommonButtons: common,
        pszWindowTitle: TITLE,
        pszMainInstruction: if instruction.len() > 1 { PCWSTR(instruction.as_ptr()) } else { PCWSTR::null() },
        pszContent: PCWSTR(content.as_ptr()),
        cButtons: buttons.len() as u32,
        pButtons: buttons.as_ptr(),
        nDefaultButton: default,
        ..Default::default()
    };
    config.Anonymous1.pszMainIcon = icon;
    let mut pressed = 0i32;
    let mut cookie = 0usize;
    // SAFETY: `func` is comctl32 v6's TaskDialogIndirect, called inside the activation context it
    // was loaded from. Every string `config` points to outlives the call.
    unsafe {
        if !ActivateActCtx(ctx, &mut cookie).as_bool() {
            return None;
        }
        let hr = func(&config, &mut pressed, std::ptr::null_mut(), std::ptr::null_mut());
        let _ = DeactivateActCtx(0, cookie);
        hr.is_ok().then_some(pressed)
    }
}

fn message_box(owner: Option<isize>, instruction: &str, content: &str, style: MESSAGEBOX_STYLE) -> i32 {
    let text = if instruction.is_empty() { wide(content) } else { wide(&format!("{instruction}\n\n{content}")) };
    // SAFETY: `text` outlives the call; TITLE is static.
    unsafe { MessageBoxW(owner.map(|h| HWND(h as *mut c_void)), PCWSTR(text.as_ptr()), TITLE, style).0 }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

#[repr(C)]
struct ActCtxW {
    size: u32,
    flags: u32,
    source: PCWSTR,
    processor_architecture: u16,
    lang_id: u16,
    assembly_directory: PCWSTR,
    resource_name: PCWSTR,
    application_name: PCWSTR,
    module: HMODULE,
}

windows::core::link!("kernel32.dll" "system" fn CreateActCtxW(actctx: *const ActCtxW) -> HANDLE);
windows::core::link!("kernel32.dll" "system" fn ActivateActCtx(actctx: HANDLE, cookie: *mut usize) -> BOOL);
windows::core::link!("kernel32.dll" "system" fn DeactivateActCtx(flags: u32, cookie: usize) -> BOOL);

/// `ACTCTX_FLAG_RESOURCE_NAME_VALID`.
const RESOURCE_NAME_VALID: u32 = 0x8;
/// shell32.dll's manifest resource, which asks for Common Controls 6.
const SHELL32_COMCTL6_MANIFEST: usize = 124;

/// comctl32 v6's `TaskDialogIndirect` and the activation context to call it in.
///
/// negotium.exe has no manifest asking for Common Controls 6, so a plain import binds comctl32
/// v5, which has no task dialog (a static import would stop the exe from starting). The
/// activation context made from shell32.dll's own manifest loads v6. Kept for the process's life.
fn comctl6() -> Option<(HANDLE, TaskDialogIndirectFn)> {
    static LOADED: OnceLock<Option<(usize, usize)>> = OnceLock::new();
    let (ctx, func) = (*LOADED.get_or_init(load_comctl6))?;
    // SAFETY: `func` is the non-null address of TaskDialogIndirect, whose module is never freed.
    Some((HANDLE(ctx as *mut c_void), unsafe { std::mem::transmute::<usize, TaskDialogIndirectFn>(func) }))
}

fn load_comctl6() -> Option<(usize, usize)> {
    let shell32 = wide(&format!("{}\\System32\\shell32.dll", negotium_collector::windows_dir()));
    let actctx = ActCtxW {
        size: size_of::<ActCtxW>() as u32,
        flags: RESOURCE_NAME_VALID,
        source: PCWSTR(shell32.as_ptr()),
        processor_architecture: 0,
        lang_id: 0,
        assembly_directory: PCWSTR::null(),
        resource_name: PCWSTR(SHELL32_COMCTL6_MANIFEST as *const u16),
        application_name: PCWSTR::null(),
        module: HMODULE::default(),
    };
    let mut cookie = 0usize;
    // SAFETY: `actctx` and `shell32` outlive the calls; the context is deactivated before
    // returning and kept (never released) for later calls. The module is never freed.
    unsafe {
        let ctx = CreateActCtxW(&actctx);
        if ctx.is_invalid() {
            return None;
        }
        if !ActivateActCtx(ctx, &mut cookie).as_bool() {
            return None;
        }
        let func = LoadLibraryW(w!("comctl32.dll")).ok().and_then(|m| GetProcAddress(m, s!("TaskDialogIndirect")));
        let _ = DeactivateActCtx(0, cookie);
        func.map(|f| (ctx.0 as usize, f as usize))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::picker::label::Outline;
    use crate::picker::target::Rect;

    #[test]
    fn paints_the_dim_and_keeps_the_hole_clickable() {
        // A small overlay in the screen's corner, shown for a moment (win.rs's tests show windows too).
        let m = Monitor { rect: Rect { left: 0, top: 0, right: 64, bottom: 48 }, dpi: 96 };
        let hole = Rect { left: 10, top: 10, right: 30, bottom: 30 };
        let scene = Scene { size: (64, 48), hole: Some(hole), outline: Outline::None, label: None, hint: None, scale: 1.0 };
        let mut o = Overlay::create(&m).expect("overlay");
        o.paint(&Painter::new().expect("factories"), &scene).expect("paint");
        // SAFETY: `bits` covers the 64 x 48 DIB owned by `o`, which outlives the slice.
        let px = unsafe { std::slice::from_raw_parts(o.bits, 64 * 48) };
        assert_eq!(px[2 * 64 + 2], 0x8C00_0000, "premultiplied 55% black");
        assert_eq!(px[20 * 64 + 20], 0x0100_0000, "alpha 1, so a click in the hole reaches the overlay");
    }

    #[test]
    fn the_task_dialog_is_found_without_an_exe_manifest() {
        // The test exe, like negotium.exe, has no Common Controls 6 manifest.
        assert!(comctl6().is_some());
    }
}
