//! The window picker's global shortcut, Ctrl+Alt+Shift+Esc. `Shortcut` holds the logic (the
//! setting is the flag: off never calls the API); `Win32Hotkey` is the `RegisterHotKey` side.
//! Not the Ctrl+Shift+Esc Task Manager redirect (`crate::integrate`).
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, RegisterHotKey, UnregisterHotKey, VK_ESCAPE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GWLP_USERDATA, GetWindowLongPtrW, HWND_MESSAGE, RegisterClassExW,
    SetWindowLongPtrW, WINDOW_EX_STYLE, WINDOW_STYLE, WM_HOTKEY, WNDCLASSEXW,
};
use windows::core::{HRESULT, w};

const MODIFIERS: HOT_KEY_MODIFIERS = HOT_KEY_MODIFIERS(MOD_ALT.0 | MOD_CONTROL.0 | MOD_SHIFT.0 | MOD_NOREPEAT.0);
/// The one hotkey of the message-only window.
const HOTKEY_ID: i32 = 1;

/// What the user is told when another program holds the keys (toast and Settings row).
pub const TAKEN_TEXT: &str = "Another program uses Ctrl+Alt+Shift+Esc";

/// Whether a queued "taken" toast is still true after a toggle: only while the shortcut is wanted
/// and not registered.
pub fn keep_pending_taken_toast(on: bool, registered: bool) -> bool {
    on && !registered
}

/// Registers and releases the shortcut with the system.
pub trait HotkeyApi {
    /// Asks the system for the keys; `false` when another program already holds them.
    fn register(&self) -> bool;
    fn unregister(&self);
}

/// Whether the shortcut is wanted and whether the system gave it.
pub struct Shortcut<A: HotkeyApi> {
    api: A,
    wanted: bool,
    registered: bool,
}

impl<A: HotkeyApi> Shortcut<A> {
    pub fn new(api: A) -> Self {
        Self { api, wanted: false, registered: false }
    }

    /// Registers or unregisters; returns `registered`. Never touches the saved setting. A taken
    /// shortcut is asked for again on the next `apply(true)`.
    pub fn apply(&mut self, wanted: bool) -> bool {
        self.wanted = wanted;
        if wanted && !self.registered {
            self.registered = self.api.register();
        } else if !wanted && self.registered {
            self.api.unregister();
            self.registered = false;
        }
        self.registered
    }

    /// Wanted, but another program holds the keys.
    pub fn taken(&self) -> bool {
        self.wanted && !self.registered
    }
}

/// A hotkey that cannot be registered: stands in when the message-only window could not be made,
/// so the setting reads as "taken" instead of the shortcut silently doing nothing.
pub struct Unavailable;

impl HotkeyApi for Unavailable {
    fn register(&self) -> bool {
        false
    }

    fn unregister(&self) {}
}

impl HotkeyApi for Box<dyn HotkeyApi> {
    fn register(&self) -> bool {
        (**self).register()
    }

    fn unregister(&self) {
        (**self).unregister()
    }
}

/// The real keys on a message-only window of the UI thread. The window gets WM_HOTKEY from the
/// thread's message loop (Slint's). Destroying it releases the keys.
pub struct Win32Hotkey {
    hwnd: HWND,
    /// Owned by the window (its user data); freed after the window is destroyed.
    on_press: *mut Box<dyn Fn()>,
}

impl Win32Hotkey {
    /// `on_press` runs on the UI thread each time the keys are pressed. Create AND drop it on that
    /// thread: `DestroyWindow` fails silently from another one.
    pub fn new(on_press: Box<dyn Fn()>) -> windows::core::Result<Self> {
        register_class()?;
        // SAFETY: the class is registered; the parent HWND_MESSAGE makes a message-only window.
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                CLASS,
                w!(""),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                Some(GetModuleHandleW(None)?.into()),
                None,
            )?
        };
        let on_press = Box::into_raw(Box::new(on_press));
        // SAFETY: `hwnd` is ours; the pointer stays valid until `drop` destroys the window first.
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, on_press as isize) };
        Ok(Self { hwnd, on_press })
    }
}

impl HotkeyApi for Win32Hotkey {
    fn register(&self) -> bool {
        // SAFETY: `hwnd` is this thread's live window.
        unsafe { RegisterHotKey(Some(self.hwnd), HOTKEY_ID, MODIFIERS, VK_ESCAPE.0 as u32) }.is_ok()
    }

    fn unregister(&self) {
        // SAFETY: as `register`; unregistering a key that is not held fails harmlessly.
        let _ = unsafe { UnregisterHotKey(Some(self.hwnd), HOTKEY_ID) };
    }
}

impl Drop for Win32Hotkey {
    fn drop(&mut self) {
        // SAFETY: the window is destroyed (no more messages reach `wndproc`) before the callback
        // it points to is freed.
        unsafe {
            let _ = DestroyWindow(self.hwnd);
            drop(Box::from_raw(self.on_press));
        }
    }
}

const CLASS: windows::core::PCWSTR = w!("NegotiumShortcutWindow");

fn register_class() -> windows::core::Result<()> {
    // SAFETY: the class struct is a local; the class name is static.
    unsafe {
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: GetModuleHandleW(None)?.into(),
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
    Ok(())
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_HOTKEY && wparam.0 == HOTKEY_ID as usize {
        // SAFETY: the user data is the callback `Win32Hotkey::new` stored; it outlives the window.
        let on_press = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const Box<dyn Fn()>;
        if !on_press.is_null() {
            // SAFETY: as above, non-null.
            unsafe { (*on_press)() };
        }
        return LRESULT(0);
    }
    // SAFETY: the arguments are the ones this window procedure received.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    /// Counts the calls; `free` is whether the system grants the keys.
    #[derive(Clone)]
    struct Fake {
        registers: Rc<Cell<u32>>,
        unregisters: Rc<Cell<u32>>,
        free: Rc<Cell<bool>>,
    }

    impl Fake {
        fn new(free: bool) -> Self {
            Self { registers: Rc::default(), unregisters: Rc::default(), free: Rc::new(Cell::new(free)) }
        }
    }

    impl HotkeyApi for Fake {
        fn register(&self) -> bool {
            self.registers.set(self.registers.get() + 1);
            self.free.get()
        }

        fn unregister(&self) {
            self.unregisters.set(self.unregisters.get() + 1);
        }
    }

    #[test]
    fn a_queued_taken_toast_is_kept_only_while_still_taken() {
        assert!(keep_pending_taken_toast(true, false));
        assert!(!keep_pending_taken_toast(true, true), "a retry registered");
        assert!(!keep_pending_taken_toast(false, false), "turned off");
        assert!(!keep_pending_taken_toast(false, true));
    }

    #[test]
    fn off_never_registers() {
        let fake = Fake::new(true);
        let mut s = Shortcut::new(fake.clone());
        assert!(!s.apply(false));
        assert!(!s.apply(false));
        assert_eq!((fake.registers.get(), fake.unregisters.get()), (0, 0), "the flag is off: the API is not touched");
        assert!(!s.taken());
    }

    #[test]
    fn on_registers_once() {
        let fake = Fake::new(true);
        let mut s = Shortcut::new(fake.clone());
        assert!(s.apply(true));
        assert!(s.apply(true));
        assert_eq!(fake.registers.get(), 1);
        assert!(!s.taken());
    }

    #[test]
    fn off_after_on_unregisters() {
        let fake = Fake::new(true);
        let mut s = Shortcut::new(fake.clone());
        s.apply(true);
        assert!(!s.apply(false));
        assert_eq!(fake.unregisters.get(), 1);
        s.apply(false);
        assert_eq!(fake.unregisters.get(), 1, "nothing left to release");
    }

    #[test]
    fn taken_when_register_fails() {
        let fake = Fake::new(false);
        let mut s = Shortcut::new(fake.clone());
        assert!(!s.apply(true), "not registered");
        assert!(s.taken());
        assert!(!s.apply(false));
        assert!(!s.taken(), "no longer wanted");
        assert_eq!(fake.unregisters.get(), 0, "a key never held is not released");
    }

    #[test]
    fn retry_on_next_apply_after_taken() {
        let fake = Fake::new(false);
        let mut s = Shortcut::new(fake.clone());
        s.apply(true);
        assert!(s.taken());
        fake.free.set(true);
        assert!(s.apply(true));
        assert!(!s.taken());
        assert_eq!(fake.registers.get(), 2);
    }
}
