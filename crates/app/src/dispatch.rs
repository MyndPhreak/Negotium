//! Turns a row action into a job (pure), and runs jobs on action threads.
use crate::view::Target;
use negotium_actions::{
    ActionError, end_many, force_close, open_file_location, process_details, toggle_efficiency_mode,
};
use negotium_model::descendants_deepest_first;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobKind {
    EndMany,
    ForceClose,
    Efficiency,
    OpenLocation,
    Details,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    pub kind: JobKind,
    pub name: String,
    pub targets: Vec<(u32, i64)>,
    pub path: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Plan {
    Run(Job),
    Confirm(String, Job),
    Nothing,
}

pub fn plan(action: &str, t: &Target, all_procs: &[(u32, i64, u32)]) -> Plan {
    let job = |kind, targets: Vec<(u32, i64)>| Job {
        kind,
        name: t.name.clone(),
        targets,
        path: t.image_path.clone(),
    };
    // The Windows group spans every system process: nothing may be done to it as a whole.
    if t.group {
        return Plan::Nothing;
    }
    let first = t.root.or_else(|| t.procs.first().copied());
    let planned = match action {
        "end-task" => {
            let mut v: Vec<_> = t.procs.iter().copied().filter(|p| Some(*p) != t.root).collect();
            v.extend(t.root);
            job(JobKind::EndMany, v)
        }
        "end-tree" => match t.root {
            Some(root) => job(JobKind::EndMany, descendants_deepest_first(all_procs, root)),
            None => job(JobKind::EndMany, t.procs.clone()),
        },
        "force-close" => match t.hung_pid {
            Some(p) => job(JobKind::ForceClose, vec![p]),
            None => return Plan::Nothing,
        },
        "efficiency" => job(JobKind::Efficiency, t.procs.clone()),
        "open-location" if t.image_path.is_some() => job(JobKind::OpenLocation, vec![]),
        "details" => match first {
            Some(p) => job(JobKind::Details, vec![p]),
            None => return Plan::Nothing,
        },
        _ => return Plan::Nothing,
    };
    if t.critical && matches!(planned.kind, JobKind::EndMany | JobKind::ForceClose) {
        let text = format!(
            "Ending {} will make Windows unusable or shut it down, and unsaved work in every app will be lost.",
            t.name
        );
        return Plan::Confirm(text, planned);
    }
    Plan::Run(planned)
}

/// Notification for an end-process job: silent on full success, the plain message when the only
/// target failed, otherwise "Ended N of M" with the first failure's reason.
pub fn end_summary(name: &str, total: usize, failures: &[ActionError], elevated: bool) -> String {
    let Some(first) = failures.first() else { return String::new() };
    if total <= 1 {
        return first.message(name, elevated);
    }
    let reason = match first {
        ActionError::AccessDenied if elevated => "protected by Windows".to_string(),
        ActionError::AccessDenied => "access denied".to_string(),
        ActionError::AlreadyExited => "already exited".to_string(),
        ActionError::Critical => "Windows marks it as critical".to_string(),
        ActionError::Other(detail) => detail.trim_end_matches('.').to_string(),
    };
    let failed = failures.len();
    format!(
        "Ended {} of {total} processes in {name}; {failed} couldn't be ended: {reason}.",
        total.saturating_sub(failed)
    )
}

/// Blocking; call on an action thread. Returns the notification text ("" = silent success),
/// or `details:`-prefixed details text for [`JobKind::Details`].
pub fn run(job: Job, elevated: bool) -> String {
    let fail = |e: ActionError| e.message(&job.name, elevated);
    match job.kind {
        JobKind::EndMany => {
            let failures: Vec<_> = end_many(&job.targets).into_iter().filter_map(|(_, r)| r.err()).collect();
            end_summary(&job.name, job.targets.len(), &failures, elevated)
        }
        JobKind::ForceClose => match job.targets.first() {
            Some(&(pid, ct)) => match force_close(pid, ct) {
                Ok(()) | Err(ActionError::AlreadyExited) => String::new(),
                Err(e) => fail(e),
            },
            None => String::new(),
        },
        JobKind::Efficiency => match toggle_efficiency_mode(&job.targets) {
            Ok(true) => format!("Efficiency mode on for {}.", job.name),
            Ok(false) => format!("Efficiency mode off for {}.", job.name),
            Err(e) => fail(e),
        },
        JobKind::OpenLocation => match job.path.as_deref().map(open_file_location) {
            Some(Err(e)) => fail(e),
            _ => String::new(),
        },
        JobKind::Details => match job.targets.first() {
            Some(&(pid, ct)) => match process_details(pid, ct) {
                Ok(d) => format!(
                    "details:Name: {}\nPID: {}\nPath: {}\nCommand line: {}\nUser: {}\nPriority: {}",
                    job.name,
                    d.pid,
                    d.image_path.as_deref().unwrap_or("\u{2014}"),
                    d.command_line.as_deref().unwrap_or("\u{2014}"),
                    d.user.as_deref().unwrap_or("\u{2014}"),
                    d.priority.as_deref().unwrap_or("\u{2014}"),
                ),
                Err(e) => fail(e),
            },
            None => String::new(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(critical: bool) -> Target {
        Target {
            name: "App".into(), root: Some((10, 1)), procs: vec![(10, 1), (11, 2), (12, 3)],
            hung_pid: Some((11, 2)), image_path: Some(r"C:\App\app.exe".into()), critical, group: false,
        }
    }
    const ALL: &[(u32, i64, u32)] = &[(10, 1, 4), (11, 2, 10), (12, 3, 11), (13, 4, 4)];

    #[test]
    fn end_task_ends_children_before_root() {
        let Plan::Run(job) = plan("end-task", &target(false), ALL) else { panic!() };
        assert_eq!(job.kind, JobKind::EndMany);
        assert_eq!(job.targets.last(), Some(&(10, 1)));
        assert_eq!(job.targets.len(), 3);
    }

    #[test]
    fn end_tree_is_deepest_first() {
        let Plan::Run(job) = plan("end-tree", &target(false), ALL) else { panic!() };
        assert_eq!(job.targets, vec![(12, 3), (11, 2), (10, 1)]);
    }

    #[test]
    fn force_close_targets_the_hung_process() {
        let Plan::Run(job) = plan("force-close", &target(false), ALL) else { panic!() };
        assert_eq!((job.kind, job.targets), (JobKind::ForceClose, vec![(11, 2)]));
    }

    #[test]
    fn critical_targets_need_confirmation() {
        assert!(matches!(plan("end-task", &target(true), ALL), Plan::Confirm(text, _) if text.contains("unusable")));
        assert!(matches!(plan("details", &target(true), ALL), Plan::Run(_)));
    }

    #[test]
    fn the_windows_group_is_never_acted_on() {
        let mut t = target(true);
        t.group = true;
        t.root = None;
        for action in ["end-task", "end-tree", "force-close", "efficiency", "open-location", "details"] {
            assert_eq!(plan(action, &t, ALL), Plan::Nothing, "{action}");
        }
    }

    #[test]
    fn force_close_needs_a_hung_process() {
        let mut t = target(false);
        t.hung_pid = None;
        assert_eq!(plan("force-close", &t, ALL), Plan::Nothing);
    }

    #[test]
    fn critical_force_close_needs_confirmation() {
        assert!(matches!(plan("force-close", &target(true), ALL), Plan::Confirm(text, job)
            if text.contains("unusable") && job.kind == JobKind::ForceClose));
    }

    #[test]
    fn end_summary_single_failure_uses_the_plain_message() {
        let errs = [ActionError::AccessDenied];
        assert_eq!(
            end_summary("App", 1, &errs, false),
            "Couldn't change App: access denied. Restart Negotium as admin and try again."
        );
        assert_eq!(end_summary("App", 3, &[], false), "");
    }

    #[test]
    fn end_summary_counts_partial_failures() {
        let errs = [ActionError::AccessDenied];
        assert_eq!(
            end_summary("Chrome", 20, &errs, false),
            "Ended 19 of 20 processes in Chrome; 1 couldn't be ended: access denied."
        );
        let errs = [ActionError::AccessDenied, ActionError::Other("busy".into())];
        assert_eq!(
            end_summary("Chrome", 5, &errs, true),
            "Ended 3 of 5 processes in Chrome; 2 couldn't be ended: protected by Windows."
        );
        assert_eq!(
            end_summary("X", 2, &[ActionError::Other("busy".into())], true),
            "Ended 1 of 2 processes in X; 1 couldn't be ended: busy."
        );
    }

    #[test]
    fn end_summary_reports_processes_windows_marks_critical() {
        assert_eq!(
            end_summary("csrss.exe", 1, &[ActionError::Critical], true),
            "Windows marks this process as critical \u{2014} ending it would crash the PC."
        );
        assert_eq!(
            end_summary("Svc", 3, &[ActionError::Critical], true),
            "Ended 2 of 3 processes in Svc; 1 couldn't be ended: Windows marks it as critical."
        );
    }

    #[test]
    fn missing_path_means_nothing_to_open() {
        let mut t = target(false);
        t.image_path = None;
        assert!(matches!(plan("open-location", &t, ALL), Plan::Nothing));
        assert!(matches!(plan("unknown-action", &t, ALL), Plan::Nothing));
    }
}
