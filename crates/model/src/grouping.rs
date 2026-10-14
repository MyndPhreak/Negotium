//! Process → app grouping rules (spec §4.3), applied in order per process:
//! 1. own package family → `Package` (unless the image is under %SystemRoot%, e.g.
//!    `SystemApps\StartMenuExperienceHost` → `Windows`);
//! 2. nearest window-owning ancestor (stopping at shell hosts and at parents created after
//!    the child); a packaged root yields `Package`, otherwise `Window`;
//! 3. session 0 or an image under %SystemRoot% → `Windows`;
//! 4. windowless same-name host chain collapses to its topmost ancestor → `Background`.
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

pub fn group(processes: &[ProcessSample], windows: &[WindowSample], system_root: &str) -> HashMap<(u32, i64), AppKey> {
    let by_pid: HashMap<u32, &ProcessSample> = processes.iter().map(|p| (p.pid, p)).collect();
    let window_pids: HashSet<u32> = windows.iter().map(|w| w.pid).collect();
    let system_prefix = format!("{}\\", system_root.trim_end_matches('\\')).to_ascii_lowercase();
    processes
        .iter()
        .filter(|p| p.pid != 0)
        .map(|p| {
            let system = p.session_id == 0 || is_under(p, &system_prefix);
            let key = if let Some(pfn) = &p.package_family {
                if is_under(p, &system_prefix) { AppKey::Windows } else { AppKey::Package(pfn.clone()) }
            } else if let Some(root) = window_root(p, &by_pid, &window_pids) {
                match &root.package_family {
                    Some(pfn) => AppKey::Package(pfn.clone()),
                    None => AppKey::Window { pid: root.pid, create_time: root.create_time },
                }
            } else if system {
                AppKey::Windows
            } else {
                let top = host_root(p, &by_pid);
                AppKey::Background { pid: top.pid, create_time: top.create_time }
            };
            ((p.pid, p.create_time), key)
        })
        .collect()
}

fn window_root<'a>(
    p: &'a ProcessSample,
    by_pid: &HashMap<u32, &'a ProcessSample>,
    window_pids: &HashSet<u32>,
) -> Option<&'a ProcessSample> {
    let mut cur = p;
    for _ in 0..MAX_ANCESTRY_DEPTH {
        if window_pids.contains(&cur.pid) {
            return Some(cur);
        }
        let parent = *by_pid.get(&cur.parent_pid)?;
        if parent.pid == cur.pid || parent.create_time > cur.create_time || is_shell_host(parent) {
            return None;
        }
        cur = parent;
    }
    None
}

/// Spec rule 3: a windowless multi-process host (Chromium, Electron) collapses to its
/// topmost ancestor with the same image name.
fn host_root<'a>(p: &'a ProcessSample, by_pid: &HashMap<u32, &'a ProcessSample>) -> &'a ProcessSample {
    let mut cur = p;
    for _ in 0..MAX_ANCESTRY_DEPTH {
        match by_pid.get(&cur.parent_pid) {
            Some(parent)
                if parent.pid != cur.pid
                    && parent.create_time <= cur.create_time
                    && parent.name.eq_ignore_ascii_case(&cur.name) =>
            {
                cur = parent
            }
            _ => break,
        }
    }
    cur
}

fn is_under(p: &ProcessSample, prefix_lower: &str) -> bool {
    p.image_path.as_deref().is_some_and(|path| path.to_ascii_lowercase().starts_with(prefix_lower))
}

fn is_shell_host(p: &ProcessSample) -> bool {
    SHELL_HOSTS.iter().any(|h| p.name.eq_ignore_ascii_case(h))
}

pub fn display_name(key: &AppKey, processes: &[ProcessSample]) -> String {
    let described = |p: &ProcessSample| p.description.as_deref().map(str::trim).filter(|d| !d.is_empty()).map(str::to_string);
    let process_name = |pid: u32, create_time: i64| {
        processes
            .iter()
            .find(|p| p.pid == pid && p.create_time == create_time)
            .map(|p| described(p).unwrap_or_else(|| strip_exe(&p.name)))
            .unwrap_or_else(|| format!("<pid {pid}>"))
    };
    match key {
        AppKey::Package(pfn) => processes
            .iter()
            .filter(|p| p.package_family.as_deref() == Some(pfn.as_str()))
            .find_map(described)
            .unwrap_or_else(|| pfn.split('_').next().unwrap_or(pfn).to_string()),
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

    const SYS: &str = r"C:\Windows";

    fn p(pid: u32, parent: u32, create: i64, name: &str, session: u32) -> ProcessSample {
        ProcessSample {
            pid, parent_pid: parent, create_time: create, name: name.into(), session_id: session,
            cpu_time_100ns: 0, private_working_set: 0, disk_io_bytes: 0, threads: 1, handles: 0,
            image_path: None, package_family: None, description: None,
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
        let g = group(&procs, &[win(10), win(20)], SYS);
        let chrome = AppKey::Window { pid: 20, create_time: 10 };
        assert_eq!(g[&(21, 11)], chrome);
        assert_eq!(g[&(22, 12)], chrome);
        assert_eq!(g[&(20, 10)], chrome);
        assert_eq!(g[&(10, 5)], AppKey::Window { pid: 10, create_time: 5 });
    }

    #[test]
    fn windowless_shell_children_are_background_not_explorer() {
        let procs = vec![p(10, 1, 5, "explorer.exe", 1), p(30, 10, 20, "tray.exe", 1)];
        let g = group(&procs, &[win(10)], SYS);
        assert_eq!(g[&(30, 20)], AppKey::Background { pid: 30, create_time: 20 });
    }

    #[test]
    fn reused_parent_pid_is_not_adopted() {
        // Parent PID 40 now belongs to a process created after the child.
        let procs = vec![p(40, 1, 999, "editor.exe", 1), p(41, 40, 100, "orphan.exe", 1)];
        let g = group(&procs, &[win(40)], SYS);
        assert_eq!(g[&(41, 100)], AppKey::Background { pid: 41, create_time: 100 });
    }

    #[test]
    fn parent_cycles_terminate() {
        let procs = vec![p(50, 51, 10, "a.exe", 1), p(51, 50, 10, "b.exe", 1)];
        let g = group(&procs, &[], SYS);
        assert_eq!(g[&(50, 10)], AppKey::Background { pid: 50, create_time: 10 });
        assert_eq!(g[&(51, 10)], AppKey::Background { pid: 51, create_time: 10 });
    }

    #[test]
    fn services_packages_and_idle() {
        let mut pkg = p(60, 1, 1, "WindowsTerminal.exe", 1);
        pkg.package_family = Some("Microsoft.WindowsTerminal_8wekyb3d8bbwe".into());
        let procs = vec![p(0, 0, 0, "System Idle Process", 0), p(4, 0, 0, "System", 0), p(70, 4, 2, "svchost.exe", 0), pkg];
        let g = group(&procs, &[win(60)], SYS);
        assert!(!g.contains_key(&(0, 0)));
        assert_eq!(g[&(4, 0)], AppKey::Windows);
        assert_eq!(g[&(70, 2)], AppKey::Windows);
        let key = AppKey::Package("Microsoft.WindowsTerminal_8wekyb3d8bbwe".into());
        assert_eq!(g[&(60, 1)], key);
        assert_eq!(display_name(&key, &procs), "Microsoft.WindowsTerminal");
        assert_eq!(display_name(&AppKey::Window { pid: 70, create_time: 2 }, &procs), "svchost");
        assert_eq!(display_name(&AppKey::Windows, &procs), "Windows");
    }

    #[test]
    fn packaged_window_owner_keeps_children_in_its_package() {
        let mut term = p(60, 1, 1, "WindowsTerminal.exe", 1);
        term.package_family = Some("Microsoft.WindowsTerminal_8wekyb3d8bbwe".into());
        let procs = vec![term, p(61, 60, 2, "pwsh.exe", 1)];
        let g = group(&procs, &[win(60)], SYS);
        assert_eq!(g[&(61, 2)], AppKey::Package("Microsoft.WindowsTerminal_8wekyb3d8bbwe".into()));
    }

    #[test]
    fn windowless_host_children_collapse_to_top_host() {
        let procs = vec![
            p(10, 1, 5, "explorer.exe", 1),
            p(80, 10, 20, "Discord.exe", 1),
            p(81, 80, 21, "Discord.exe", 1),
            p(82, 81, 22, "Discord.exe", 1),
        ];
        let g = group(&procs, &[win(10)], SYS);
        let top = AppKey::Background { pid: 80, create_time: 20 };
        assert_eq!(g[&(80, 20)], top);
        assert_eq!(g[&(81, 21)], top);
        assert_eq!(g[&(82, 22)], top);
    }

    #[test]
    fn user_session_system_binaries_join_windows() {
        let mut svc = p(90, 4, 3, "svchost.exe", 1);
        svc.image_path = Some(r"C:\WINDOWS\system32\svchost.exe".into());
        let mut tool = p(91, 4, 3, "tool.exe", 1);
        tool.image_path = Some(r"C:\Program Files\Tool\tool.exe".into());
        let g = group(&[svc, tool], &[], SYS);
        assert_eq!(g[&(90, 3)], AppKey::Windows);
        assert_eq!(g[&(91, 3)], AppKey::Background { pid: 91, create_time: 3 });
    }

    #[test]
    fn display_name_prefers_root_description() {
        let mut chrome = p(20, 1, 10, "chrome.exe", 1);
        chrome.description = Some("Google Chrome".into());
        let mut blank = p(21, 1, 11, "tool.exe", 1);
        blank.description = Some("   ".into());
        let procs = vec![chrome, blank];
        assert_eq!(display_name(&AppKey::Window { pid: 20, create_time: 10 }, &procs), "Google Chrome");
        assert_eq!(display_name(&AppKey::Background { pid: 21, create_time: 11 }, &procs), "tool");
    }

    #[test]
    fn packaged_system_components_join_windows() {
        let mut start = p(70, 1, 1, "StartMenuExperienceHost.exe", 1);
        start.package_family = Some("Microsoft.Windows.StartMenuExperienceHost_cw5n1h2txyewy".into());
        start.image_path = Some(r"C:\Windows\SystemApps\Microsoft.Windows.StartMenuExperienceHost_cw5n1h2txyewy\StartMenuExperienceHost.exe".into());
        let mut term = p(71, 1, 1, "WindowsTerminal.exe", 1);
        term.package_family = Some("Microsoft.WindowsTerminal_8wekyb3d8bbwe".into());
        term.image_path = Some(r"C:\Program Files\WindowsApps\Microsoft.WindowsTerminal\WindowsTerminal.exe".into());
        let g = group(&[start, term], &[win(70), win(71)], SYS);
        assert_eq!(g[&(70, 1)], AppKey::Windows);
        assert_eq!(g[&(71, 1)], AppKey::Package("Microsoft.WindowsTerminal_8wekyb3d8bbwe".into()));
    }
}
