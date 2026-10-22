//! The window picker's global shortcut, Ctrl+Alt+Shift+Esc. `Shortcut` holds the logic (the
//! setting is the flag: off never calls the API); `Win32Hotkey` is the `RegisterHotKey` side.
//! Not the Ctrl+Shift+Esc Task Manager redirect (`crate::integrate`).

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
        let _ = wanted;
        todo!()
    }

    /// Wanted, but another program holds the keys.
    pub fn taken(&self) -> bool {
        todo!()
    }
}

/// The real keys on a message-only window of the UI thread.
pub struct Win32Hotkey {}

impl Win32Hotkey {
    /// `on_press` runs on the UI thread each time the keys are pressed.
    pub fn new(on_press: Box<dyn Fn()>) -> windows::core::Result<Self> {
        let _ = on_press;
        todo!()
    }
}

impl HotkeyApi for Win32Hotkey {
    fn register(&self) -> bool {
        todo!()
    }

    fn unregister(&self) {
        todo!()
    }
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
