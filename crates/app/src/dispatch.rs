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
        "force-close" => match t.hung_pid.or(first) {
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
    if t.critical && planned.kind == JobKind::EndMany {
        let text = format!(
            "Ending {} will make Windows unusable or shut it down, and unsaved work in every app will be lost.",
            t.name
        );
        return Plan::Confirm(text, planned);
    }
    Plan::Run(planned)
}

/// Blocking; call on an action thread. Returns the notification text ("" = silent success),
/// or `details:`-prefixed details text for [`JobKind::Details`].
pub fn run(job: Job, elevated: bool) -> String {
    let fail = |e: ActionError| e.message(&job.name, elevated);
    match job.kind {
        JobKind::EndMany => {
            let failures: Vec<_> = end_many(&job.targets).into_iter().filter_map(|(_, r)| r.err()).collect();
            match failures.first() {
                None => String::new(),
                Some(e) => fail(e.clone()),
            }
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
            hung_pid: Some((11, 2)), image_path: Some(r"C:\App\app.exe".into()), critical,
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
    fn missing_path_means_nothing_to_open() {
        let mut t = target(false);
        t.image_path = None;
        assert!(matches!(plan("open-location", &t, ALL), Plan::Nothing));
        assert!(matches!(plan("unknown-action", &t, ALL), Plan::Nothing));
    }
}
