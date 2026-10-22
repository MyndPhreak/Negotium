//! Decides which top-level window under the cursor is the picker's target.
//!
//! Plain data only: a later task fills `WindowInfo` from Win32.

/// Window classes that belong to the shell and are never ended.
const SHELL_CLASSES: [&str; 4] = ["Shell_TrayWnd", "Shell_SecondaryTrayWnd", "Progman", "WorkerW"];
/// File Explorer folder window class.
const FOLDER_CLASS: &str = "CabinetWClass";

/// Screen rectangle in pixels. Left and top are inside, right and bottom are outside.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    fn contains(&self, (x, y): (i32, i32)) -> bool {
        self.left <= x && x < self.right && self.top <= y && y < self.bottom
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct WindowInfo {
    pub hwnd: isize,
    /// Process id after UWP resolution.
    pub pid: u32,
    pub create_time: i64,
    pub class: String,
    pub title: String,
    /// `DWMWA_EXTENDED_FRAME_BOUNDS`, screen pixels.
    pub frame: Rect,
    pub visible: bool,
    pub cloaked: bool,
    pub minimized: bool,
    pub hung: bool,
    /// `ApplicationFrameWindow` with no `CoreWindow` child.
    pub uwp_unresolved: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pickable {
    App,
    ExplorerInShell,
    Shell,
    /// Looked through: an overlay, or a window that is not on screen.
    PassThrough,
    /// On screen but not a target (no title, Negotium's own, unresolved UWP frame): it hides
    /// what is below it, so nothing is picked.
    Block,
}

pub struct Env {
    pub own_pid: u32,
    pub shell_pid: Option<u32>,
    pub overlays: Vec<isize>,
}

pub fn classify(w: &WindowInfo, env: &Env) -> Pickable {
    if env.overlays.contains(&w.hwnd) || !w.visible || w.cloaked || w.minimized {
        return Pickable::PassThrough;
    }
    // Before the title rule: the taskbars have no title.
    if SHELL_CLASSES.contains(&w.class.as_str()) {
        return Pickable::Shell;
    }
    if w.title.is_empty() || w.uwp_unresolved || w.pid == env.own_pid {
        return Pickable::Block;
    }
    if env.shell_pid == Some(w.pid) {
        return if w.class == FOLDER_CLASS { Pickable::ExplorerInShell } else { Pickable::Shell };
    }
    Pickable::App
}

/// `windows` in z-order, top first. Looks through overlays and windows not on screen; the first
/// other window whose frame contains `pt` decides: its kind, or `None` when it is `Block`.
/// Never returns `PassThrough` or `Block`.
pub fn pick_at<'a>(
    windows: &'a [WindowInfo],
    pt: (i32, i32),
    env: &Env,
) -> Option<(&'a WindowInfo, Pickable)> {
    windows
        .iter()
        .filter(|w| w.frame.contains(pt))
        .map(|w| (w, classify(w, env)))
        .find(|(_, p)| *p != Pickable::PassThrough)
        .filter(|(_, p)| *p != Pickable::Block)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWN: u32 = 10;
    const SHELL: u32 = 20;
    const OTHER: u32 = 30;

    fn win(hwnd: isize, pid: u32, class: &str, frame: Rect) -> WindowInfo {
        WindowInfo {
            hwnd,
            pid,
            create_time: 0,
            class: class.into(),
            title: "Title".into(),
            frame,
            visible: true,
            cloaked: false,
            minimized: false,
            hung: false,
            uwp_unresolved: false,
        }
    }

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> Rect {
        Rect { left, top, right, bottom }
    }

    fn app(hwnd: isize, frame: Rect) -> WindowInfo {
        win(hwnd, OTHER, "Notepad", frame)
    }

    fn env() -> Env {
        Env { own_pid: OWN, shell_pid: Some(SHELL), overlays: vec![99] }
    }

    #[test]
    fn classify_table() {
        let f = rect(0, 0, 10, 10);
        let base = || app(1, f);
        let rows: Vec<(&str, WindowInfo, Pickable)> = vec![
            ("overlay", WindowInfo { hwnd: 99, ..base() }, Pickable::PassThrough),
            ("invisible", WindowInfo { visible: false, ..base() }, Pickable::PassThrough),
            ("cloaked", WindowInfo { cloaked: true, ..base() }, Pickable::PassThrough),
            ("minimized", WindowInfo { minimized: true, ..base() }, Pickable::PassThrough),
            ("empty title", WindowInfo { title: String::new(), ..base() }, Pickable::Block),
            ("uwp unresolved", WindowInfo { uwp_unresolved: true, ..base() }, Pickable::Block),
            ("own pid", WindowInfo { pid: OWN, ..base() }, Pickable::Block),
            ("shell class", win(1, OTHER, "Shell_TrayWnd", f), Pickable::Shell),
            ("untitled shell class", WindowInfo { title: String::new(), ..win(1, SHELL, "Shell_TrayWnd", f) }, Pickable::Shell),
            ("explorer in shell", win(1, SHELL, "CabinetWClass", f), Pickable::ExplorerInShell),
            ("other shell window", win(1, SHELL, "Notepad", f), Pickable::Shell),
            ("plain app", base(), Pickable::App),
            ("folder in other process", win(1, OTHER, "CabinetWClass", f), Pickable::App),
        ];
        for (name, w, want) in rows {
            assert_eq!(classify(&w, &env()), want, "{name}");
        }
        for class in ["Shell_SecondaryTrayWnd", "Progman", "WorkerW"] {
            assert_eq!(classify(&win(1, OTHER, class, f), &env()), Pickable::Shell, "{class}");
        }
    }

    #[test]
    fn pick_at_takes_the_topmost_containing_window() {
        let ws = [app(1, rect(0, 0, 100, 100)), app(2, rect(0, 0, 100, 100))];
        let (w, p) = pick_at(&ws, (50, 50), &env()).unwrap();
        assert_eq!((w.hwnd, p), (1, Pickable::App));
    }

    #[test]
    fn an_untitled_taskbar_above_an_app_is_the_shell() {
        let mut taskbar = win(5, SHELL, "Shell_TrayWnd", rect(0, 0, 100, 100));
        taskbar.title = String::new();
        let mut second = win(6, SHELL, "Shell_SecondaryTrayWnd", rect(0, 0, 100, 100));
        second.title = String::new();
        for bar in [taskbar, second] {
            let ws = [bar.clone(), app(2, rect(0, 0, 100, 100))];
            let (w, p) = pick_at(&ws, (50, 50), &env()).unwrap();
            assert_eq!((w.hwnd, p), (bar.hwnd, Pickable::Shell), "{}", bar.class);
        }
    }

    #[test]
    fn negotiums_own_window_above_an_app_picks_nothing() {
        let own = win(3, OWN, "Window", rect(0, 0, 100, 100));
        let ws = [own, app(2, rect(0, 0, 100, 100))];
        assert!(pick_at(&ws, (50, 50), &env()).is_none());
    }

    #[test]
    fn an_untitled_window_above_an_app_picks_nothing() {
        let mut untitled = app(3, rect(0, 0, 100, 100));
        untitled.title = String::new();
        let ws = [untitled, app(2, rect(0, 0, 100, 100))];
        assert!(pick_at(&ws, (50, 50), &env()).is_none());
    }

    #[test]
    fn an_unresolved_uwp_frame_above_an_app_picks_nothing() {
        let mut frame = win(3, OTHER, "ApplicationFrameWindow", rect(0, 0, 100, 100));
        frame.uwp_unresolved = true;
        let ws = [frame, app(2, rect(0, 0, 100, 100))];
        assert!(pick_at(&ws, (50, 50), &env()).is_none());
    }

    #[test]
    fn hidden_windows_and_overlays_above_an_app_are_looked_through() {
        let f = rect(0, 0, 100, 100);
        let above = [
            WindowInfo { cloaked: true, ..app(3, f) },
            WindowInfo { visible: false, ..app(3, f) },
            WindowInfo { minimized: true, ..app(3, f) },
            app(99, f),
        ];
        for a in above {
            let ws = [a.clone(), app(2, f)];
            let (w, p) = pick_at(&ws, (50, 50), &env()).unwrap();
            assert_eq!((w.hwnd, p), (2, Pickable::App), "{a:?}");
        }
    }

    #[test]
    fn a_hidden_untitled_window_is_looked_through() {
        let hidden = WindowInfo { visible: false, title: String::new(), ..app(3, rect(0, 0, 100, 100)) };
        let ws = [hidden, app(2, rect(0, 0, 100, 100))];
        assert_eq!(pick_at(&ws, (50, 50), &env()).unwrap().0.hwnd, 2);
    }

    #[test]
    fn pick_at_looks_through_overlays_and_cloaked_windows() {
        let mut cloaked = app(1, rect(0, 0, 100, 100));
        cloaked.cloaked = true;
        let overlay = app(99, rect(0, 0, 100, 100));
        let ws = [overlay, cloaked, app(2, rect(0, 0, 100, 100))];
        let (w, p) = pick_at(&ws, (50, 50), &env()).unwrap();
        assert_eq!((w.hwnd, p), (2, Pickable::App));
    }

    #[test]
    fn pick_at_returns_shell_for_the_desktop() {
        let ws = [win(5, OTHER, "Progman", rect(0, 0, 1920, 1080))];
        let (w, p) = pick_at(&ws, (500, 500), &env()).unwrap();
        assert_eq!((w.hwnd, p), (5, Pickable::Shell));
    }

    #[test]
    fn pick_at_returns_none_when_nothing_contains_the_point() {
        let ws = [app(1, rect(0, 0, 10, 10))];
        assert!(pick_at(&ws, (50, 50), &env()).is_none());
    }

    #[test]
    fn frame_edges_are_inclusive_left_top_exclusive_right_bottom() {
        let r = rect(10, 20, 30, 40);
        assert!(r.contains((10, 20)));
        assert!(r.contains((29, 39)));
        assert!(!r.contains((30, 30)));
        assert!(!r.contains((20, 40)));
        assert!(!r.contains((9, 30)));
        assert!(!r.contains((20, 19)));
    }
}
