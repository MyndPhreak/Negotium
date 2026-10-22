//! Finds the app a window's process belongs to, and the `Target` the Overview would build for it.
use crate::view::{Target, target_of};
use negotium_model::AppRow;
use std::collections::{HashMap, HashSet};

/// An app as the picker sees it: its action target plus what the label shows.
#[derive(Clone, Debug, PartialEq)]
pub struct AppInfo {
    pub target: Target,
    pub process_count: usize,
    pub private_bytes: u64,
    pub hung: bool,
}

/// Every process of the snapshot, mapped to the app that holds it.
pub struct AppIndex {
    apps: Vec<AppInfo>,
    by_proc: HashMap<(u32, i64), usize>,
}

impl AppIndex {
    pub fn from_rows(rows: &[AppRow], hung_pids: &HashSet<u32>, image_of: &HashMap<(u32, i64), String>) -> Self {
        let mut apps = Vec::with_capacity(rows.len());
        let mut by_proc = HashMap::new();
        for r in rows {
            for p in &r.processes {
                by_proc.insert((p.pid, p.create_time), apps.len());
            }
            apps.push(AppInfo {
                target: target_of(r, hung_pids, image_of),
                process_count: r.processes.len(),
                private_bytes: r.private_bytes,
                hung: r.hung_for_ms.is_some(),
            });
        }
        Self { apps, by_proc }
    }

    /// The app holding this process; `None` for a pid the snapshot does not know, or one reused
    /// since (another create time).
    pub fn get(&self, pid: u32, create_time: i64) -> Option<&AppInfo> {
        self.by_proc.get(&(pid, create_time)).map(|&i| &self.apps[i])
    }
}

/// Fallback for a pid the snapshot does not know: the process alone.
pub fn single_process(pid: u32, create_time: i64, name: &str) -> AppInfo {
    let key = (pid, create_time);
    AppInfo {
        target: Target {
            name: name.to_string(),
            process_name: name.to_string(),
            root: Some(key),
            procs: vec![key],
            critical: negotium_actions::is_critical(name),
            ..Target::default()
        },
        process_count: 1,
        private_bytes: 0,
        hung: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use negotium_model::{AppKey, AppKind, ProcRow};

    fn proc(pid: u32, create_time: i64, name: &str) -> ProcRow {
        ProcRow {
            pid, parent_pid: 0, create_time, name: name.into(), cpu_pct: 0.0, private_bytes: 0,
            disk_bps: 0.0, gpu_pct: None, net: None, hung_for_ms: None,
        }
    }

    fn row(key: AppKey, kind: AppKind, name: &str, processes: Vec<ProcRow>) -> AppRow {
        AppRow {
            key, name: name.into(), kind, processes, cpu_pct: 0.0, private_bytes: 4096, disk_bps: 0.0,
            gpu_pct: None, net_bps: None, net: None, hung_for_ms: None,
        }
    }

    fn index(rows: &[AppRow]) -> AppIndex {
        AppIndex::from_rows(rows, &HashSet::new(), &HashMap::new())
    }

    #[test]
    fn finds_the_app_that_contains_a_child_process() {
        let rows = [row(
            AppKey::Window { pid: 10, create_time: 1 }, AppKind::App, "Editor",
            vec![proc(10, 1, "editor.exe"), proc(11, 2, "helper.exe")],
        )];
        let info = index(&rows).get(11, 2).cloned().expect("child is indexed");
        assert_eq!(info.target.name, "Editor");
        assert_eq!(info.process_count, 2);
        assert_eq!(info.private_bytes, 4096);
    }

    #[test]
    fn a_reused_pid_with_another_create_time_is_not_found() {
        let rows = [row(
            AppKey::Window { pid: 10, create_time: 1 }, AppKind::App, "Editor",
            vec![proc(10, 1, "editor.exe"), proc(11, 2, "helper.exe")],
        )];
        assert!(index(&rows).get(11, 99).is_none());
    }

    #[test]
    fn the_windows_group_keeps_group_true() {
        let rows = [row(AppKey::Windows, AppKind::Windows, "Windows", vec![proc(4, 1, "System")])];
        assert!(index(&rows).get(4, 1).expect("indexed").target.group);
    }

    #[test]
    fn single_process_targets_only_that_process() {
        let info = single_process(7, 3, "tool.exe");
        assert_eq!(info.target.procs, vec![(7, 3)]);
        assert_eq!(info.target.root, Some((7, 3)));
        assert_eq!(info.process_count, 1);
    }
}
