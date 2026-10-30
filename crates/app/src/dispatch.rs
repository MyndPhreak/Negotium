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
    Dump,
    Properties,
    SearchOnline,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    pub kind: JobKind,
    pub name: String,
    pub targets: Vec<(u32, i64)>,
    pub path: Option<String>,
    /// The app's root process, when it has one.
    pub root: Option<(u32, i64)>,
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
        root: t.root,
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
            Some(p) => {
                let mut rest: Vec<_> = t.procs.iter().copied().filter(|q| *q != p).collect();
                rest.sort_unstable();
                job(JobKind::Details, std::iter::once(p).chain(rest).collect())
            }
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

/// The Services page search for "Go to service(s)": the main process's PID, when the app hosts
/// services; `None` otherwise.
pub fn services_search(_t: &Target) -> Option<String> {
    None
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
        ActionError::Cancelled => "cancelled".to_string(),
        ActionError::Ending => "already ending".to_string(),
        ActionError::Other(detail) => detail.trim_end_matches('.').to_string(),
    };
    let failed = failures.len();
    format!(
        "Ended {} of {total} processes in {name}; {failed} couldn't be ended: {reason}.",
        total.saturating_sub(failed)
    )
}

/// What to do about a process stuck while ending, when it still holds sockets: name the network
/// filter drivers that likely hold it, else point to a restart. `None` when it holds no sockets.
pub fn ending_hint(holds_sockets: bool, filters: &[String]) -> Option<String> {
    if !holds_sockets {
        return None;
    }
    if filters.is_empty() {
        return Some("If it does not close, restart the computer.".to_string());
    }
    Some(format!(
        "A network filter driver likely holds it: {}. Restart that app or its service. If that does not help, restart the computer.",
        filters.join(", ")
    ))
}

/// `text` with `hint` after it.
pub fn with_hint(text: String, hint: Option<String>) -> String {
    match hint {
        Some(hint) if !text.is_empty() => format!("{text} {hint}"),
        _ => text,
    }
}

/// The hint for the stuck processes among `ending`: whether any holds a socket, and if so which
/// network filter drivers run. Reads the socket tables and the drivers; call off the UI thread.
fn stuck_hint(ending: &[u32]) -> Option<String> {
    if ending.is_empty() {
        return None;
    }
    let holds = negotium_collector::sockets::list_sockets().is_ok_and(|s| s.iter().any(|s| ending.contains(&s.pid)));
    let filters = if holds { negotium_actions::drivers::network_filters() } else { Vec::new() };
    ending_hint(holds, &filters)
}

/// What a finished job shows.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    Silent,
    Toast(String),
    Details(Box<crate::details::Opened>),
    /// A memory dump was written: the toast text and the folder its Open folder button opens.
    Dumped { text: String, file: std::path::PathBuf },
}

impl Outcome {
    /// The toast text, "" when there is none (the window picker shows only text).
    pub fn toast(self) -> String {
        match self {
            Outcome::Toast(text) | Outcome::Dumped { text, .. } => text,
            Outcome::Silent | Outcome::Details(_) => String::new(),
        }
    }
}

/// Blocking; call on an action thread.
pub fn run(job: Job, elevated: bool) -> Outcome {
    if job.kind == JobKind::Details {
        // The root's own failure (exited, access denied) is the toast; the others show dashes.
        return match job.targets.first().map(|&(pid, ct)| process_details(pid, ct)) {
            Some(Ok(_)) => Outcome::Details(Box::new(crate::details::read_opened(
                &job.name,
                job.root,
                &job.targets,
                job.path.as_deref(),
            ))),
            Some(Err(e)) => Outcome::Toast(e.message(&job.name, elevated)),
            None => Outcome::Silent,
        };
    }
    match run_text(job, elevated) {
        text if text.is_empty() => Outcome::Silent,
        text => Outcome::Toast(text),
    }
}

fn run_text(job: Job, elevated: bool) -> String {
    let fail = |e: ActionError| e.message(&job.name, elevated);
    match job.kind {
        JobKind::EndMany => {
            let results = end_many(&job.targets);
            let ending: Vec<u32> =
                results.iter().filter(|(_, r)| *r == Err(ActionError::Ending)).map(|&((pid, _), _)| pid).collect();
            let failures: Vec<_> = results.into_iter().filter_map(|(_, r)| r.err()).collect();
            with_hint(end_summary(&job.name, job.targets.len(), &failures, elevated), stuck_hint(&ending))
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
        JobKind::Details | JobKind::Dump | JobKind::Properties | JobKind::SearchOnline => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(critical: bool) -> Target {
        Target {
            name: "App".into(), process_name: "app.exe".into(), root: Some((10, 1)), procs: vec![(10, 1), (11, 2), (12, 3)],
            hung_pid: Some((11, 2)), image_path: Some(r"C:\App\app.exe".into()), critical, group: false,
            hosts_services: false,
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
    fn search_and_properties_need_a_path() {
        let mut t = target(false);
        let Plan::Run(job) = plan("search-online", &t, ALL) else { panic!("search-online") };
        assert_eq!(job.kind, JobKind::SearchOnline);
        let Plan::Run(job) = plan("properties", &t, ALL) else { panic!("properties") };
        assert_eq!(job.kind, JobKind::Properties);
        t.image_path = None;
        assert_eq!(plan("search-online", &t, ALL), Plan::Nothing);
        assert_eq!(plan("properties", &t, ALL), Plan::Nothing);
    }

    #[test]
    fn dump_targets_the_main_process() {
        let Plan::Run(job) = plan("dump", &target(false), ALL) else { panic!() };
        assert_eq!((job.kind, job.targets), (JobKind::Dump, vec![(10, 1)]));
    }

    #[test]
    fn the_windows_group_gets_none_of_the_new_actions() {
        let mut t = target(false);
        (t.group, t.hosts_services) = (true, true);
        for action in ["search-online", "properties", "dump"] {
            assert_eq!(plan(action, &t, ALL), Plan::Nothing, "{action}");
        }
        assert_eq!(services_search(&t), None);
    }

    #[test]
    fn goto_services_needs_a_service_host() {
        let mut t = target(false);
        assert_eq!(services_search(&t), None);
        t.hosts_services = true;
        assert_eq!(services_search(&t).as_deref(), Some("10"));
    }

    #[test]
    fn details_job_returns_every_process_root_first() {
        let mut t = target(false);
        t.procs = vec![(11, 2), (10, 1), (12, 3)];
        let Plan::Run(job) = plan("details", &t, ALL) else { panic!() };
        assert_eq!(job.kind, JobKind::Details);
        assert_eq!(job.targets, vec![(10, 1), (11, 2), (12, 3)]);
    }

    #[test]
    fn a_job_with_nothing_to_say_is_silent() {
        let job = Job { kind: JobKind::OpenLocation, name: "App".into(), targets: vec![], path: None, root: None };
        assert_eq!(run(job, false), Outcome::Silent);
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
    fn end_summary_reports_a_process_stuck_while_ending() {
        assert_eq!(
            end_summary("Node.js", 1, &[ActionError::Ending], true),
            "Node.js is already ending, but Windows did not finish closing it. It keeps its ports and files until it closes."
        );
        assert_eq!(
            end_summary("Node.js", 3, &[ActionError::Ending], true),
            "Ended 2 of 3 processes in Node.js; 1 couldn't be ended: already ending."
        );
    }

    #[test]
    fn a_stuck_process_with_sockets_names_the_network_filters() {
        assert_eq!(
            ending_hint(true, &["Portmaster".into()]).as_deref(),
            Some("A network filter driver likely holds it: Portmaster. Restart that app or its service. If that does not help, restart the computer.")
        );
        assert_eq!(
            ending_hint(true, &["NordVPN".into(), "Portmaster".into()]).as_deref(),
            Some("A network filter driver likely holds it: NordVPN, Portmaster. Restart that app or its service. If that does not help, restart the computer.")
        );
        assert_eq!(ending_hint(true, &[]).as_deref(), Some("If it does not close, restart the computer."));
        assert_eq!(ending_hint(false, &["Portmaster".into()]), None);
    }

    #[test]
    fn a_hint_follows_the_message() {
        assert_eq!(with_hint("A.".into(), Some("B.".into())), "A. B.");
        assert_eq!(with_hint("A.".into(), None), "A.");
        assert_eq!(with_hint(String::new(), Some("B.".into())), "", "no message, no hint");
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
