//! Process → app grouping rules (spec §4.3), applied in order:
//! 1. packaged → package family; 2. nearest window-owning ancestor (stopping at shell
//! hosts and at parents created after the child); 3. session 0 → "Windows"; 4. Background.
use negotium_snapshot::{ProcessSample, WindowSample};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

pub const SHELL_HOSTS: &[&str] = &["explorer.exe"];
const MAX_ANCESTRY_DEPTH: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum AppKey {
    Package(String),
    Window { pid: u32, create_time: i64 },
    Windows,
    Background { pid: u32, create_time: i64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppKind {
    App,
    Windows,
    Background,
}

impl AppKey {
    pub fn kind(&self) -> AppKind {
        match self {
            AppKey::Package(_) | AppKey::Window { .. } => AppKind::App,
            AppKey::Windows => AppKind::Windows,
            AppKey::Background { .. } => AppKind::Background,
        }
    }
}

pub fn group(processes: &[ProcessSample], windows: &[WindowSample]) -> HashMap<(u32, i64), AppKey> {
    let by_pid: HashMap<u32, &ProcessSample> = processes.iter().map(|p| (p.pid, p)).collect();
    let window_pids: HashSet<u32> = windows.iter().map(|w| w.pid).collect();
    processes
        .iter()
        .filter(|p| p.pid != 0)
        .map(|p| {
            let key = if let Some(pfn) = &p.package_family {
                AppKey::Package(pfn.clone())
            } else if let Some((pid, create_time)) = window_root(p, &by_pid, &window_pids) {
                AppKey::Window { pid, create_time }
            } else if p.session_id == 0 {
                AppKey::Windows
            } else {
                AppKey::Background { pid: p.pid, create_time: p.create_time }
            };
            ((p.pid, p.create_time), key)
        })
        .collect()
}

fn window_root(
    p: &ProcessSample,
    by_pid: &HashMap<u32, &ProcessSample>,
    window_pids: &HashSet<u32>,
) -> Option<(u32, i64)> {
    let mut cur = p;
    for _ in 0..MAX_ANCESTRY_DEPTH {
        if window_pids.contains(&cur.pid) {
            return Some((cur.pid, cur.create_time));
        }
        let parent = by_pid.get(&cur.parent_pid)?;
        if parent.pid == cur.pid || parent.create_time > cur.create_time || is_shell_host(parent) {
            return None;
        }
        cur = parent;
    }
    None
}

fn is_shell_host(p: &ProcessSample) -> bool {
    SHELL_HOSTS.iter().any(|h| p.name.eq_ignore_ascii_case(h))
}

pub fn display_name(key: &AppKey, processes: &[ProcessSample]) -> String {
    let process_name = |pid: u32, create_time: i64| {
        processes
            .iter()
            .find(|p| p.pid == pid && p.create_time == create_time)
            .map(|p| strip_exe(&p.name))
            .unwrap_or_else(|| format!("<pid {pid}>"))
    };
    match key {
        AppKey::Package(pfn) => pfn.split('_').next().unwrap_or(pfn).to_string(),
        AppKey::Window { pid, create_time } | AppKey::Background { pid, create_time } => {
            process_name(*pid, *create_time)
        }
        AppKey::Windows => "Windows".to_string(),
    }
}

fn strip_exe(name: &str) -> String {
    name.strip_suffix(".exe").or_else(|| name.strip_suffix(".EXE")).unwrap_or(name).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: u32, parent: u32, create: i64, name: &str, session: u32) -> ProcessSample {
        ProcessSample {
            pid, parent_pid: parent, create_time: create, name: name.into(), session_id: session,
            cpu_time_100ns: 0, private_working_set: 0, disk_io_bytes: 0, threads: 1, handles: 0,
            image_path: None, package_family: None,
        }
    }
    fn win(pid: u32) -> WindowSample {
        WindowSample { pid, hung: false, title: format!("w{pid}") }
    }

    #[test]
    fn browser_children_join_the_window_owner() {
        let procs = vec![
            p(10, 1, 5, "explorer.exe", 1),
            p(20, 10, 10, "chrome.exe", 1),
            p(21, 20, 11, "chrome.exe", 1),
            p(22, 21, 12, "chrome.exe", 1),
        ];
        let g = group(&procs, &[win(10), win(20)]);
        let chrome = AppKey::Window { pid: 20, create_time: 10 };
        assert_eq!(g[&(21, 11)], chrome);
        assert_eq!(g[&(22, 12)], chrome);
        assert_eq!(g[&(20, 10)], chrome);
        assert_eq!(g[&(10, 5)], AppKey::Window { pid: 10, create_time: 5 });
    }

    #[test]
    fn windowless_shell_children_are_background_not_explorer() {
        let procs = vec![p(10, 1, 5, "explorer.exe", 1), p(30, 10, 20, "tray.exe", 1)];
        let g = group(&procs, &[win(10)]);
        assert_eq!(g[&(30, 20)], AppKey::Background { pid: 30, create_time: 20 });
    }

    #[test]
    fn reused_parent_pid_is_not_adopted() {
        // Parent PID 40 now belongs to a process created after the child.
        let procs = vec![p(40, 1, 999, "editor.exe", 1), p(41, 40, 100, "orphan.exe", 1)];
        let g = group(&procs, &[win(40)]);
        assert_eq!(g[&(41, 100)], AppKey::Background { pid: 41, create_time: 100 });
    }

    #[test]
    fn parent_cycles_terminate() {
        let procs = vec![p(50, 51, 10, "a.exe", 1), p(51, 50, 10, "b.exe", 1)];
        let g = group(&procs, &[]);
        assert_eq!(g[&(50, 10)], AppKey::Background { pid: 50, create_time: 10 });
        assert_eq!(g[&(51, 10)], AppKey::Background { pid: 51, create_time: 10 });
    }

    #[test]
    fn services_packages_and_idle() {
        let mut pkg = p(60, 1, 1, "WindowsTerminal.exe", 1);
        pkg.package_family = Some("Microsoft.WindowsTerminal_8wekyb3d8bbwe".into());
        let procs = vec![p(0, 0, 0, "System Idle Process", 0), p(4, 0, 0, "System", 0), p(70, 4, 2, "svchost.exe", 0), pkg];
        let g = group(&procs, &[win(60)]);
        assert!(!g.contains_key(&(0, 0)));
        assert_eq!(g[&(4, 0)], AppKey::Windows);
        assert_eq!(g[&(70, 2)], AppKey::Windows);
        let key = AppKey::Package("Microsoft.WindowsTerminal_8wekyb3d8bbwe".into());
        assert_eq!(g[&(60, 1)], key);
        assert_eq!(display_name(&key, &procs), "Microsoft.WindowsTerminal");
        assert_eq!(display_name(&AppKey::Window { pid: 70, create_time: 2 }, &procs), "svchost");
        assert_eq!(display_name(&AppKey::Windows, &procs), "Windows");
    }
}
