//! The text and outline colour the overlay draws for the hovered target.
use super::apps::AppInfo;
use super::target::Pickable;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outline {
    None,
    Accent,
    Red,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Label {
    pub title: String,
    pub detail: String,
    pub outline: Outline,
}

/// What is known about the app: only its exe name while the snapshot loads, else the full info.
pub enum Info<'a> {
    Loading { exe: &'a str },
    Ready(&'a AppInfo),
}

pub fn label(kind: Pickable, info: Info, needs_admin: bool, hung: bool) -> Label {
    let _ = (kind, info, needs_admin, hung);
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::Target;

    fn app(name: &str, count: usize, bytes: u64) -> AppInfo {
        AppInfo {
            target: Target { name: name.to_string(), ..Target::default() },
            process_count: count,
            private_bytes: bytes,
            hung: false,
        }
    }

    #[test]
    fn ready_app_shows_name_count_and_memory() {
        let a = app("Chrome", 3, 1024 * 1024);
        let l = label(Pickable::App, Info::Ready(&a), false, false);
        assert_eq!(l.title, "Chrome");
        assert_eq!(l.detail, "3 processes · 1.0 MB");
    }

    #[test]
    fn one_process_is_singular() {
        let a = app("Notepad", 1, 2048);
        let l = label(Pickable::App, Info::Ready(&a), false, false);
        assert_eq!(l.detail, "1 process · 2.0 KB");
    }

    #[test]
    fn hung_app_is_red_and_says_not_responding() {
        let a = app("Notepad", 1, 2048);
        let l = label(Pickable::App, Info::Ready(&a), false, true);
        assert_eq!(l.outline, Outline::Red);
        assert_eq!(l.detail, "1 process · 2.0 KB · Not responding");
    }

    #[test]
    fn responding_app_is_accent() {
        let a = app("Notepad", 1, 2048);
        assert_eq!(label(Pickable::App, Info::Ready(&a), false, false).outline, Outline::Accent);
    }

    #[test]
    fn needs_admin_is_shown() {
        let a = app("Notepad", 1, 2048);
        let l = label(Pickable::App, Info::Ready(&a), true, false);
        assert_eq!(l.detail, "1 process · 2.0 KB · Needs admin — restart Negotium as admin");
    }

    #[test]
    fn explorer_in_shell_warns_about_the_taskbar() {
        let a = app("File Explorer", 1, 2048);
        let l = label(Pickable::ExplorerInShell, Info::Ready(&a), false, false);
        assert_eq!(l.detail, "1 process · 2.0 KB · File Explorer — restarts the taskbar");
        assert_eq!(l.outline, Outline::Accent);
    }

    #[test]
    fn shell_surface_has_no_outline_and_the_shell_text() {
        let a = app("Explorer", 1, 2048);
        let l = label(Pickable::Shell, Info::Ready(&a), false, false);
        assert_eq!(l.title, "Windows needs this — you cannot end it here");
        assert_eq!(l.detail, "");
        assert_eq!(l.outline, Outline::None);
    }

    #[test]
    fn loading_shows_the_exe_name_only() {
        let l = label(Pickable::App, Info::Loading { exe: "foo.exe" }, false, false);
        assert_eq!(l.title, "foo.exe");
        assert_eq!(l.detail, "");
        assert_eq!(l.outline, Outline::Accent);
    }
}
