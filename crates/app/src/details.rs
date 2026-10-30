//! The Details popup's data: facts read once when it opens, and the live part the worker builds
//! each tick while it is open.
use crate::view::Target;
use negotium_actions::{Details, Signature};
use negotium_collector::sockets::Socket;
use negotium_model::AppRow;
use negotium_snapshot::Snapshot;
use std::collections::HashMap;

const DASH: &str = "\u{2014}";

/// What was read about one process of the app when the popup opened.
#[derive(Clone, Debug, PartialEq)]
pub struct ProcFacts {
    pub pid: u32,
    pub create_time: i64,
    pub name: String,
    pub role: Option<&'static str>,
    /// `None`: the process could not be opened (protected, access denied, exited).
    pub details: Option<Details>,
}

/// Everything the popup shows that does not change while it is open.
#[derive(Clone, Debug, PartialEq)]
pub struct Opened {
    pub name: String,
    pub publisher: Option<String>,
    pub version: Option<String>,
    pub signature: Signature,
    pub image_path: Option<String>,
    /// The root first, then by PID.
    pub procs: Vec<ProcFacts>,
    pub selected: usize,
}

/// The processes the worker reads live numbers for while the popup is open.
#[derive(Clone, Debug, PartialEq)]
pub struct DetailsWatch {
    pub procs: Vec<(u32, i64)>,
    pub selected: (u32, i64),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LiveProc {
    pub pid: u32,
    pub create_time: i64,
    /// Still running: the same PID with the same create time is in the snapshot.
    pub alive: bool,
    pub cpu_pct: f32,
    pub private_bytes: u64,
    pub disk_bps: f64,
    pub net_bps: Option<f64>,
    pub gpu_pct: Option<f32>,
    pub threads: u32,
    pub handles: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Connection {
    pub protocol: &'static str,
    pub local: String,
    pub remote: String,
    pub state: String,
}

/// The live part, built by the worker each tick while a watch is set.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DetailsFrame {
    pub procs: Vec<LiveProc>,
    /// Peak working set of the selected process.
    pub peak_bytes: Option<u64>,
    /// The selected process's parent, when it is still the process that started it.
    pub parent: Option<(u32, String)>,
    /// The selected process's create time (FILETIME), when it still runs.
    pub start_time: Option<i64>,
    pub connections: Vec<Connection>,
    pub services: Vec<String>,
}

/// A hint at what a process of a multi-process app does, from its command line.
pub fn role_hint(_command_line: Option<&str>, _is_root: bool) -> Option<&'static str> {
    None
}

pub fn build_details_frame(
    _watch: &DetailsWatch,
    _snap: &Snapshot,
    _rows: &[AppRow],
    _sockets: &[Socket],
    _services: &HashMap<u32, Vec<String>>,
    _peak: Option<u64>,
) -> DetailsFrame {
    DetailsFrame::default()
}

/// How long ago `start` was, both FILETIMEs: "2 h 5 min", "3 min 2 s", "45 s"; a dash when
/// `start` is in the future.
pub fn running_for(_start: i64, _now: i64) -> String {
    String::new()
}

/// The Identity section of the selected process as "Label: value" lines, for the clipboard.
pub fn copy_all_text(_o: &Opened, _f: &DetailsFrame) -> String {
    String::new()
}

/// Reads the facts for every process of `t`. Blocking; call on an action thread.
pub fn read_opened(t: &Target) -> Opened {
    Opened {
        name: t.name.clone(),
        publisher: None,
        version: None,
        signature: Signature::Unknown,
        image_path: t.image_path.clone(),
        procs: Vec::new(),
        selected: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use negotium_collector::sockets::{Protocol, TcpState};
    use negotium_model::{AppKey, AppKind, ProcRow};
    use negotium_snapshot::ProcessSample;

    fn sample(pid: u32, create_time: i64, parent_pid: u32, name: &str) -> ProcessSample {
        ProcessSample { pid, create_time, parent_pid, name: name.into(), threads: 7, handles: 99, ..Default::default() }
    }

    fn snap(processes: Vec<ProcessSample>) -> Snapshot {
        Snapshot { processes, ..Default::default() }
    }

    fn row(procs: &[(u32, i64, f32)]) -> AppRow {
        AppRow {
            key: AppKey::Background { pid: procs[0].0, create_time: procs[0].1 },
            name: "App".into(),
            kind: AppKind::Background,
            processes: procs
                .iter()
                .map(|&(pid, create_time, cpu_pct)| ProcRow {
                    pid,
                    parent_pid: 0,
                    create_time,
                    name: "app.exe".into(),
                    cpu_pct,
                    private_bytes: 1 << 20,
                    disk_bps: 0.0,
                    gpu_pct: None,
                    net: None,
                    hung_for_ms: None,
                })
                .collect(),
            cpu_pct: 0.0,
            private_bytes: 0,
            disk_bps: 0.0,
            gpu_pct: None,
            net_bps: None,
            net: None,
            hung_for_ms: None,
        }
    }

    fn watch(procs: &[(u32, i64)], selected: (u32, i64)) -> DetailsWatch {
        DetailsWatch { procs: procs.to_vec(), selected }
    }

    fn sock(pid: u32, protocol: Protocol, local: &str, remote: Option<&str>, state: Option<TcpState>) -> Socket {
        Socket { protocol, local: local.parse().unwrap(), remote: remote.map(|r| r.parse().unwrap()), state, pid }
    }

    #[test]
    fn roles_come_from_the_type_switch() {
        assert_eq!(role_hint(Some("x.exe --type=renderer --foo"), false), Some("Renderer"));
        assert_eq!(role_hint(Some("x.exe --type=gpu-process"), false), Some("GPU"));
        assert_eq!(role_hint(Some("x.exe --type=utility --utility-sub-type=x"), false), Some("Utility"));
        assert_eq!(role_hint(Some("x.exe --type=crashpad-handler"), false), Some("Crash reporter"));
        assert_eq!(role_hint(None, true), Some("Main"));
        assert_eq!(role_hint(Some("x.exe"), false), None);
    }

    #[test]
    fn live_numbers_follow_pid_and_create_time() {
        let s = snap(vec![sample(10, 2, 4, "app.exe")]);
        let f = build_details_frame(&watch(&[(10, 1)], (10, 1)), &s, &[row(&[(10, 2, 50.0)])], &[], &HashMap::new(), None);
        assert_eq!(f.procs.len(), 1);
        assert!(!f.procs[0].alive);
        assert_eq!(f.procs[0].cpu_pct, 0.0);
        assert_eq!(f.start_time, None);
    }

    #[test]
    fn threads_and_handles_come_from_the_snapshot() {
        let s = snap(vec![sample(10, 1, 4, "app.exe")]);
        let f = build_details_frame(&watch(&[(10, 1)], (10, 1)), &s, &[row(&[(10, 1, 12.5)])], &[], &HashMap::new(), Some(5));
        let p = &f.procs[0];
        assert!(p.alive);
        assert_eq!((p.threads, p.handles, p.cpu_pct, p.private_bytes), (7, 99, 12.5, 1 << 20));
        assert_eq!(f.peak_bytes, Some(5));
        assert_eq!(f.start_time, Some(1));
    }

    #[test]
    fn a_parent_younger_than_the_child_is_not_shown() {
        let young = snap(vec![sample(4, 9, 0, "System"), sample(10, 5, 4, "app.exe")]);
        let f = build_details_frame(&watch(&[(10, 5)], (10, 5)), &young, &[], &[], &HashMap::new(), None);
        assert_eq!(f.parent, None);
        let old = snap(vec![sample(4, 1, 0, "System"), sample(10, 5, 4, "app.exe")]);
        let f = build_details_frame(&watch(&[(10, 5)], (10, 5)), &old, &[], &[], &HashMap::new(), None);
        assert_eq!(f.parent, Some((4, "System".to_string())));
    }

    #[test]
    fn connections_are_only_the_selected_process() {
        let s = snap(vec![sample(10, 1, 0, "a.exe"), sample(11, 1, 0, "a.exe")]);
        let sockets = [
            sock(10, Protocol::Tcp, "0.0.0.0:80", None, Some(TcpState::Listen)),
            sock(10, Protocol::Tcp, "10.0.0.2:5000", Some("1.2.3.4:443"), Some(TcpState::Established)),
            sock(10, Protocol::Udp, "0.0.0.0:53", None, None),
            sock(11, Protocol::Tcp, "0.0.0.0:81", None, Some(TcpState::Listen)),
        ];
        let f = build_details_frame(&watch(&[(10, 1), (11, 1)], (10, 1)), &s, &[], &sockets, &HashMap::new(), None);
        assert_eq!(
            f.connections,
            vec![
                Connection { protocol: "TCP", local: "0.0.0.0:80".into(), remote: DASH.into(), state: "Listening".into() },
                Connection {
                    protocol: "TCP",
                    local: "10.0.0.2:5000".into(),
                    remote: "1.2.3.4:443".into(),
                    state: "Established".into()
                },
                Connection { protocol: "UDP", local: "0.0.0.0:53".into(), remote: DASH.into(), state: DASH.into() },
            ]
        );
    }

    #[test]
    fn services_are_the_selected_process() {
        let s = snap(vec![sample(10, 1, 0, "svchost.exe")]);
        let services = HashMap::from([(10, vec!["Dnscache".to_string()])]);
        let f = build_details_frame(&watch(&[(10, 1)], (10, 1)), &s, &[], &[], &services, None);
        assert_eq!(f.services, vec!["Dnscache".to_string()]);
        let f = build_details_frame(&watch(&[(10, 1)], (10, 1)), &s, &[], &[], &HashMap::new(), None);
        assert!(f.services.is_empty());
    }

    #[test]
    fn running_for_reads_as_hours_and_minutes() {
        const S: i64 = 10_000_000;
        assert_eq!(running_for(0, (2 * 3600 + 5 * 60 + 9) * S), "2 h 5 min");
        assert_eq!(running_for(0, (3 * 60 + 2) * S), "3 min 2 s");
        assert_eq!(running_for(0, 45 * S), "45 s");
        assert_eq!(running_for(10 * S, 0), DASH);
    }

    #[test]
    fn copy_all_lists_each_identity_field() {
        let details = Details {
            pid: 10,
            image_path: Some(r"C:\App\app.exe".into()),
            command_line: None,
            user: Some(r"PC\me".into()),
            priority: Some("Normal".into()),
            elevated: Some(false),
            virtualized: Some(false),
            architecture: None,
            efficiency: None,
        };
        let o = Opened {
            name: "App".into(),
            publisher: None,
            version: None,
            signature: Signature::Unknown,
            image_path: details.image_path.clone(),
            procs: vec![ProcFacts { pid: 10, create_time: 1, name: "app.exe".into(), role: Some("Main"), details: Some(details) }],
            selected: 0,
        };
        let text = copy_all_text(&o, &DetailsFrame::default());
        assert!(text.contains("Name: app.exe"), "{text}");
        assert!(text.contains("PID: 10"), "{text}");
        assert!(text.contains(r"Path: C:\App\app.exe"), "{text}");
        assert!(text.contains("Command line: \u{2014}"), "{text}");
        assert!(text.contains(r"User: PC\me"), "{text}");
        assert!(text.contains("Parent: \u{2014}"), "{text}");
    }
}
