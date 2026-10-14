//! Window setup on the UI thread (allowed by the Global Constraints): Mica backdrop, title-bar
//! theme and the user's accent colour; the main-window hooks for tray re-creation and
//! activation; and the single-instance lock.
use crate::{MainWindow, Theme};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::ComponentHandle;
use std::ffi::c_void;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, ERROR_SUCCESS, GetLastError, HANDLE, HWND, LPARAM, LRESULT, WAIT_ABANDONED,
    WAIT_OBJECT_0, WPARAM,
};
use windows::Win32::Graphics::Dwm::{
    DWMSBT_MAINWINDOW, DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute,
};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows::Win32::System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, ChangeWindowMessageFilterEx, EnumWindows, GetPropW, GetWindowThreadProcessId, IsIconic,
    MSGFLT_ALLOW, PostMessageW, RegisterWindowMessageW, RemovePropW, SW_RESTORE, SetForegroundWindow, SetPropW,
    ShowWindow, WM_NCDESTROY,
};
use windows::core::{BOOL, HSTRING, PCWSTR, w};

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
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        let _ = SetForegroundWindow(hwnd);
    }
}

/// Messages the main window handles on top of what winit does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellEvent {
    /// Explorer (re)started: the tray icon must be added again. Slint's tray window is
    /// message-only, so it never receives this broadcast itself.
    TaskbarCreated,
    /// A second launch asked this instance to show itself.
    Activate,
}

pub fn taskbar_created_message() -> u32 {
    // SAFETY: registers (or looks up) a well-known message name.
    unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) }
}

pub fn activate_message() -> u32 {
    // SAFETY: as above.
    unsafe { RegisterWindowMessageW(w!("Negotium.Activate")) }
}

/// Window property marking the main window, so a second launch can find it even while it is
/// hidden in the tray.
const MAIN_WINDOW_PROP: PCWSTR = w!("Negotium.MainWindow");
const SUBCLASS_ID: usize = 0x4E45_474F; // "NEGO"

type Hook = Box<dyn Fn(ShellEvent)>;

/// Subclasses the main window: `on_event` runs (on the UI thread, inside the window procedure)
/// for TaskbarCreated and the activation message. Also lets both messages through UIPI, so a
/// non-elevated Explorer or second launch can still reach an elevated instance. Returns false
/// if subclassing failed.
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
        for msg in [taskbar_created_message(), activate_message()] {
            let _ = ChangeWindowMessageFilterEx(hwnd, msg, MSGFLT_ALLOW, None);
        }
    }
    true
}

/// Marks `hwnd` as this instance's main window for [`find_main_window`] in a later launch.
pub fn mark_main_window(hwnd: HWND) {
    // SAFETY: sets a property on a window of this thread (removed again on WM_NCDESTROY).
    unsafe {
        let _ = SetPropW(hwnd, MAIN_WINDOW_PROP, Some(HANDLE(1 as *mut c_void)));
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
    } else if msg != 0 && msg == taskbar_created_message() {
        // SAFETY: as above.
        unsafe { (*hook)(ShellEvent::TaskbarCreated) };
    }
    // SAFETY: forwards to the next procedure in the subclass chain.
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

/// The main window of a running instance (this process's included), visible or not.
pub fn find_main_window() -> Option<HWND> {
    unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: lparam is the `&mut Option<HWND>` passed below for the duration of EnumWindows.
        let found = unsafe { &mut *(lparam.0 as *mut Option<HWND>) };
        // SAFETY: read-only property lookup on a handle supplied by EnumWindows.
        if !unsafe { GetPropW(hwnd, MAIN_WINDOW_PROP) }.is_invalid() {
            *found = Some(hwnd);
            return false.into();
        }
        true.into()
    }
    let mut found: Option<HWND> = None;
    // SAFETY: `found` outlives the synchronous enumeration.
    unsafe {
        let _ = EnumWindows(Some(visit), LPARAM(&mut found as *mut Option<HWND> as isize));
    }
    found
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
        let hwnd = hidden_window();
        let seen = Rc::new(RefCell::new(Vec::new()));
        let s = seen.clone();
        assert!(install_window_hooks(hwnd, move |e| s.borrow_mut().push(e)));
        // SAFETY: synchronous sends to our own window on this thread.
        unsafe {
            SendMessageW(hwnd, activate_message(), Some(WPARAM(0)), Some(LPARAM(0)));
            SendMessageW(hwnd, taskbar_created_message(), Some(WPARAM(0)), Some(LPARAM(0)));
            let _ = DestroyWindow(hwnd);
        }
        assert_eq!(*seen.borrow(), [ShellEvent::Activate, ShellEvent::TaskbarCreated]);
    }

    #[test]
    fn a_hidden_main_window_is_found_by_its_mark() {
        let hwnd = hidden_window();
        mark_main_window(hwnd);
        let found = find_main_window();
        // SAFETY: our own test window.
        unsafe {
            let _ = RemovePropW(hwnd, MAIN_WINDOW_PROP);
            let _ = DestroyWindow(hwnd);
        }
        assert_eq!(found, Some(hwnd));
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
