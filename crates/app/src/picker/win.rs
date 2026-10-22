//! Win32 side of the picker: lists top-level windows in z-order as plain `WindowInfo`
//! records, finds the shell process and lists monitors. Nothing here sends a window message,
//! so a hung window can never block the picker.
use super::target::{Rect, WindowInfo};

/// One display: its rectangle in virtual-screen pixels and its effective DPI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Monitor {
    pub rect: Rect,
    pub dpi: u32,
}

/// Unowned top-level windows, top of the z-order first. Visible, cloaked and minimised
/// windows are all listed; `classify` filters them.
pub fn top_level_windows() -> Vec<WindowInfo> {
    todo!()
}

/// Process id and create time (FILETIME, 100 ns units) of the shell window's process.
pub fn shell_pid() -> Option<(u32, i64)> {
    todo!()
}

pub fn monitors() -> Vec<Monitor> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, GetWindowRect, WINDOW_EX_STYLE, WS_OVERLAPPED, WS_VISIBLE,
    };
    use windows::core::{PCWSTR, w};

    fn make(title: PCWSTR, owner: Option<HWND>, x: i32) -> HWND {
        // SAFETY: plain window creation with a system class; destroyed by the test.
        unsafe {
            CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), title, WS_OVERLAPPED | WS_VISIBLE, x, 100, 200, 150, owner, None, None, None)
        }
        .expect("create test window")
    }

    fn destroy(hwnds: &[HWND]) {
        for h in hwnds {
            // SAFETY: our own test windows.
            unsafe {
                let _ = DestroyWindow(*h);
            }
        }
    }

    fn find(list: &[WindowInfo], h: HWND) -> Option<usize> {
        list.iter().position(|w| w.hwnd == h.0 as isize)
    }

    #[test]
    fn lists_a_visible_test_window_with_its_class_and_frame() {
        let h = make(w!("negotium-picker-a"), None, 100);
        let list = top_level_windows();
        let mut rect = RECT::default();
        // SAFETY: valid window and out pointer.
        unsafe { GetWindowRect(h, &mut rect).unwrap() };
        destroy(&[h]);
        let w = &list[find(&list, h).expect("listed")];
        assert_eq!(w.class, "Static");
        assert_eq!(w.title, "negotium-picker-a");
        assert_eq!(w.pid, std::process::id());
        assert!(w.create_time > 0);
        assert!(w.visible && !w.cloaked && !w.minimized && !w.hung && !w.uwp_unresolved);
        assert!(w.frame.right > w.frame.left && w.frame.bottom > w.frame.top);
        let slack = 16;
        assert!(w.frame.left >= rect.left - slack && w.frame.right <= rect.right + slack);
        assert!(w.frame.top >= rect.top - slack && w.frame.bottom <= rect.bottom + slack);
    }

    #[test]
    fn a_newer_window_is_listed_before_an_older_one() {
        let old = make(w!("negotium-picker-old"), None, 100);
        let new = make(w!("negotium-picker-new"), None, 120);
        let list = top_level_windows();
        destroy(&[old, new]);
        assert!(find(&list, new).expect("new listed") < find(&list, old).expect("old listed"));
    }

    #[test]
    fn owned_windows_are_not_listed() {
        let owner = make(w!("negotium-picker-owner"), None, 100);
        let owned = make(w!("negotium-picker-owned"), Some(owner), 120);
        let list = top_level_windows();
        destroy(&[owned, owner]);
        assert!(find(&list, owner).is_some());
        assert!(find(&list, owned).is_none());
    }

    #[test]
    fn monitors_cover_the_primary_monitor() {
        let list = monitors();
        assert!(!list.is_empty());
        assert!(list.iter().any(|m| m.rect.left <= 0 && m.rect.top <= 0 && m.rect.right > 0 && m.rect.bottom > 0));
        assert!(list.iter().all(|m| m.dpi >= 96));
    }
}
