//! One-time window setup on the UI thread (allowed by the Global Constraints):
//! Mica backdrop, title-bar theme, and the user's accent colour.
use crate::{MainWindow, Theme};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::ComponentHandle;
use std::ffi::c_void;
use windows::Win32::Foundation::{ERROR_SUCCESS, HWND};
use windows::Win32::Graphics::Dwm::{
    DWMSBT_MAINWINDOW, DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute,
};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows::core::{BOOL, w};

/// `AccentColor` is stored as `0xAABBGGRR`.
pub fn accent_from_dword(v: u32) -> (u8, u8, u8) {
    ((v & 0xFF) as u8, ((v >> 8) & 0xFF) as u8, ((v >> 16) & 0xFF) as u8)
}

pub fn read_accent() -> Option<(u8, u8, u8)> {
    let mut value = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: `value`/`size` are valid for a REG_DWORD read.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\DWM"),
            w!("AccentColor"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut c_void),
            Some(&mut size),
        )
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

/// True once the native window exists (it is created lazily after `show()`).
pub fn has_window_handle(ui: &MainWindow) -> bool {
    hwnd_of(ui).is_some()
}

/// Applies the dark/light title bar and the Mica backdrop. Returns true (and sets
/// `Theme.mica`) only if both DWM calls succeed.
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
    let applied = dark_ok && mica_ok;
    ui.global::<Theme>().set_mica(applied);
    applied
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accent_dword_is_abgr() {
        assert_eq!(accent_from_dword(0xFFD47800), (0x00, 0x78, 0xD4));
    }
}
