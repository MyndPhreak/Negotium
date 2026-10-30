//! The Details popup's data: facts read once when it opens, and the live part the worker builds
//! each tick while it is open.
use negotium_actions::{Architecture, Details, Signature};
use negotium_collector::sockets::{Protocol, Socket};
use negotium_model::{AppRow, ProcRow};
use negotium_snapshot::{ProcessSample, Snapshot};
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
pub fn role_hint(command_line: Option<&str>, is_root: bool) -> Option<&'static str> {
    if is_root {
        return Some("Main");
    }
    // Chromium and Electron name each child's job with --type=.
    let kind = command_line?.split_whitespace().find_map(|a| a.strip_prefix("--type="))?;
    match kind.trim_matches('"') {
        "renderer" => Some("Renderer"),
        "gpu-process" => Some("GPU"),
        "utility" => Some("Utility"),
        "crashpad-handler" => Some("Crash reporter"),
        _ => None,
    }
}

pub fn build_details_frame(
    watch: &DetailsWatch,
    snap: &Snapshot,
    rows: &[AppRow],
    sockets: &[Socket],
    services: &HashMap<u32, Vec<String>>,
    peak: Option<u64>,
) -> DetailsFrame {
    let samples: HashMap<(u32, i64), &ProcessSample> = snap.processes.iter().map(|p| ((p.pid, p.create_time), p)).collect();
    let rates: HashMap<(u32, i64), &ProcRow> =
        rows.iter().flat_map(|r| &r.processes).map(|p| ((p.pid, p.create_time), p)).collect();
    let procs = watch
        .procs
        .iter()
        .map(|&(pid, create_time)| {
            let Some(sample) = samples.get(&(pid, create_time)) else {
                return LiveProc { pid, create_time, ..Default::default() };
            };
            let rate = rates.get(&(pid, create_time));
            LiveProc {
                pid,
                create_time,
                alive: true,
                cpu_pct: rate.map_or(0.0, |r| r.cpu_pct),
                private_bytes: rate.map_or(sample.private_working_set, |r| r.private_bytes),
                disk_bps: rate.map_or(0.0, |r| r.disk_bps),
                net_bps: rate.and_then(|r| r.net).map(|n| n.total()),
                gpu_pct: rate.and_then(|r| r.gpu_pct),
                threads: sample.threads,
                handles: sample.handles,
            }
        })
        .collect();
    let (pid, create_time) = watch.selected;
    let selected = samples.get(&watch.selected);
    // A PID is reused once its process exits: only a parent older than the child started it.
    let parent = selected.and_then(|child| {
        snap.processes
            .iter()
            .find(|p| p.pid == child.parent_pid && p.create_time < child.create_time)
            .map(|p| (p.pid, p.name.clone()))
    });
    let connections = match selected {
        Some(_) => sockets.iter().filter(|s| s.pid == pid).map(connection).collect(),
        None => Vec::new(),
    };
    DetailsFrame {
        procs,
        peak_bytes: selected.and(peak),
        parent,
        start_time: selected.map(|_| create_time),
        connections,
        services: selected.and_then(|_| services.get(&pid)).cloned().unwrap_or_default(),
    }
}

fn connection(s: &Socket) -> Connection {
    let tcp = s.protocol == Protocol::Tcp;
    Connection {
        protocol: if tcp { "TCP" } else { "UDP" },
        local: s.local.to_string(),
        remote: s.remote.map_or_else(|| DASH.to_string(), |r| r.to_string()),
        state: if tcp { crate::ports::state_label(s).to_string() } else { DASH.to_string() },
    }
}

/// How long ago `start` was, both FILETIMEs: "2 h 5 min", "3 min 2 s", "45 s"; a dash when
/// `start` is in the future.
pub fn running_for(start: i64, now: i64) -> String {
    let Some(secs) = now.checked_sub(start).filter(|d| *d >= 0).map(|d| d / 10_000_000) else {
        return DASH.to_string();
    };
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    match (h, m) {
        (0, 0) => format!("{s} s"),
        (0, _) => format!("{m} min {s} s"),
        _ => format!("{h} h {m} min"),
    }
}

/// The Identity section of the selected process as "Label: value" lines, for the clipboard.
pub fn copy_all_text(o: &Opened, f: &DetailsFrame) -> String {
    let Some(p) = o.procs.get(o.selected) else { return String::new() };
    let d = p.details.as_ref();
    let field = |v: Option<&String>| v.map_or(DASH, |v| v.as_str()).to_string();
    let parent = f.parent.as_ref().map_or_else(|| DASH.to_string(), |(pid, name)| format!("{name} (PID {pid})"));
    [
        ("Name", p.name.clone()),
        ("PID", p.pid.to_string()),
        ("Path", field(d.and_then(|d| d.image_path.as_ref()))),
        ("Command line", field(d.and_then(|d| d.command_line.as_ref()))),
        ("User", field(d.and_then(|d| d.user.as_ref()))),
        ("Parent", parent),
    ]
    .iter()
    .map(|(label, value)| format!("{label}: {value}"))
    .collect::<Vec<_>>()
    .join("\n")
}

/// The header chips: (text, warn). Signature, architecture and admin always (when known);
/// efficiency mode and UAC virtualization only when on.
pub fn header_chips(o: &Opened) -> Vec<(String, bool)> {
    let mut chips = Vec::new();
    match &o.signature {
        Signature::Signed(name) if name.is_empty() => chips.push(("Signed".to_string(), false)),
        Signature::Signed(name) => chips.push((format!("Signed by {name}"), false)),
        Signature::NotSigned => chips.push(("Not signed".to_string(), true)),
        Signature::Unknown => {}
    }
    // The app's main process (listed first) speaks for the app.
    let Some(d) = o.procs.first().and_then(|p| p.details.as_ref()) else { return chips };
    if let Some(a) = d.architecture {
        let text = match a {
            Architecture::X86 => "32-bit",
            Architecture::X64 => "64-bit",
            Architecture::Arm64 => "ARM64",
            Architecture::X64OnArm64 => "x64 on ARM64",
            Architecture::X86OnArm64 => "x86 on ARM64",
            Architecture::Other => "Other architecture",
        };
        chips.push((text.to_string(), false));
    }
    match d.elevated {
        Some(true) => chips.push(("Runs as admin".to_string(), false)),
        Some(false) => chips.push(("Does not run as admin".to_string(), false)),
        None => {}
    }
    if d.efficiency == Some(true) {
        chips.push(("Efficiency mode on".to_string(), false));
    }
    if d.virtualized == Some(true) {
        chips.push(("UAC virtualization on".to_string(), false));
    }
    chips
}

/// The Identity section of the selected process: (label, value). `now` is a FILETIME.
pub fn identity_fields(o: &Opened, f: &DetailsFrame, now: i64) -> Vec<(String, String)> {
    let d = o.procs.get(o.selected).and_then(|p| p.details.as_ref());
    let text = |v: Option<&String>| v.map_or_else(|| DASH.to_string(), Clone::clone);
    vec![
        ("Path".to_string(), text(d.and_then(|d| d.image_path.as_ref()))),
        ("Command line".to_string(), text(d.and_then(|d| d.command_line.as_ref()))),
        ("Started".to_string(), f.start_time.and_then(local_time_text).unwrap_or_else(|| DASH.to_string())),
        ("Running for".to_string(), f.start_time.map_or_else(|| DASH.to_string(), |t| running_for(t, now))),
        (
            "Parent".to_string(),
            f.parent.as_ref().map_or_else(|| DASH.to_string(), |(pid, name)| format!("{name} (PID {pid})")),
        ),
        ("User".to_string(), text(d.and_then(|d| d.user.as_ref()))),
    ]
}

/// The Resources section of the selected process: (label, value).
pub fn resource_fields(o: &Opened, f: &DetailsFrame) -> Vec<(String, String)> {
    use crate::format;
    let selected = o.procs.get(o.selected).map(|p| (p.pid, p.create_time));
    let live = f.procs.iter().find(|p| p.alive && Some((p.pid, p.create_time)) == selected);
    let value = |make: &dyn Fn(&LiveProc) -> Option<String>| live.and_then(make).unwrap_or_else(|| DASH.to_string());
    vec![
        ("CPU".to_string(), value(&|p| Some(format::pct(p.cpu_pct)))),
        ("Memory".to_string(), value(&|p| Some(format::bytes(p.private_bytes as f64)))),
        ("Peak memory".to_string(), value(&|_| f.peak_bytes.map(|b| format::bytes(b as f64)))),
        ("Disk".to_string(), value(&|p| Some(format::byte_rate(p.disk_bps)))),
        ("Network".to_string(), value(&|p| p.net_bps.map(format::bit_rate))),
        ("GPU".to_string(), value(&|p| p.gpu_pct.map(format::pct))),
        ("Threads".to_string(), value(&|p| Some(p.threads.to_string()))),
        ("Handles".to_string(), value(&|p| Some(p.handles.to_string()))),
    ]
}

/// Now, as a FILETIME (100 ns since 1601), like process create times.
pub fn now_filetime() -> i64 {
    // SAFETY: plain Win32 call.
    let ft = unsafe { windows::Win32::System::SystemInformation::GetSystemTimeAsFileTime() };
    ((ft.dwHighDateTime as i64) << 32) | ft.dwLowDateTime as i64
}

/// A FILETIME as local date and time, e.g. "2026-10-29 14:25:01".
pub fn local_time_text(filetime: i64) -> Option<String> {
    use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
    let ft = FILETIME { dwLowDateTime: filetime as u32, dwHighDateTime: (filetime >> 32) as u32 };
    let (mut utc, mut local) = (SYSTEMTIME::default(), SYSTEMTIME::default());
    // SAFETY: valid in and out pointers.
    unsafe { FileTimeToSystemTime(&ft, &mut utc) }.ok()?;
    // SAFETY: valid in and out pointers; no time zone means the current one.
    unsafe { SystemTimeToTzSpecificLocalTime(None, &utc, &mut local) }.ok()?;
    Some(format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        local.wYear, local.wMonth, local.wDay, local.wHour, local.wMinute, local.wSecond
    ))
}

impl Opened {
    /// The app as an action target for the priority and affinity dialogs: every process, the
    /// main one first.
    pub fn target(&self) -> crate::view::Target {
        let procs: Vec<(u32, i64)> = self.procs.iter().map(|p| (p.pid, p.create_time)).collect();
        crate::view::Target {
            name: self.name.clone(),
            process_name: self.procs.first().map_or_else(|| self.name.clone(), |p| p.name.clone()),
            root: procs.first().copied(),
            procs,
            hung_pid: None,
            image_path: self.image_path.clone(),
            critical: false,
            group: false,
            hosts_services: false,
        }
    }
}

pub fn priority_text(p: negotium_actions::PriorityClass) -> &'static str {
    use negotium_actions::PriorityClass::*;
    match p {
        Realtime => "Realtime",
        High => "High",
        AboveNormal => "Above normal",
        Normal => "Normal",
        BelowNormal => "Below normal",
        Idle => "Low",
    }
}

/// "All CPUs" when the process may run on every CPU, else "N of M CPUs".
pub fn affinity_text(mask: u64, system: u64) -> String {
    if mask & system == system {
        "All CPUs".to_string()
    } else {
        format!("{} of {} CPUs", (mask & system).count_ones(), system.count_ones())
    }
}

/// Reads the facts for every process in `procs` (the root first, when there is one). Blocking;
/// call on an action thread.
pub fn read_opened(name: &str, root: Option<(u32, i64)>, procs: &[(u32, i64)], image_path: Option<&str>) -> Opened {
    let procs = procs
        .iter()
        .map(|&(pid, create_time)| {
            let details = negotium_actions::process_details(pid, create_time).ok();
            let file = details.as_ref().and_then(|d| d.image_path.as_deref()).and_then(|p| p.rsplit('\\').next());
            ProcFacts {
                pid,
                create_time,
                name: file.map_or_else(|| DASH.to_string(), str::to_string),
                role: role_hint(details.as_ref().and_then(|d| d.command_line.as_deref()), root == Some((pid, create_time))),
                details,
            }
        })
        .collect();
    Opened {
        name: name.to_string(),
        publisher: image_path.and_then(|p| negotium_collector::version_string(p, "CompanyName")),
        version: image_path.and_then(negotium_actions::file_version),
        signature: image_path.map_or(Signature::Unknown, negotium_actions::signature),
        image_path: image_path.map(str::to_string),
        procs,
        selected: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use negotium_collector::sockets::TcpState;
    use negotium_model::{AppKey, AppKind};

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

    fn opened(details: Details, signature: Signature) -> Opened {
        Opened {
            name: "App".into(),
            publisher: None,
            version: None,
            signature,
            image_path: details.image_path.clone(),
            procs: vec![ProcFacts { pid: 10, create_time: 1, name: "app.exe".into(), role: Some("Main"), details: Some(details) }],
            selected: 0,
        }
    }

    fn facts() -> Details {
        Details {
            pid: 10,
            image_path: None,
            command_line: None,
            user: None,
            priority: None,
            elevated: Some(true),
            virtualized: Some(false),
            architecture: Some(negotium_actions::Architecture::X86OnArm64),
            efficiency: Some(true),
        }
    }

    #[test]
    fn chips_name_signature_architecture_admin_and_what_is_on() {
        let chips = header_chips(&opened(facts(), Signature::Signed("Example Ltd".into())));
        assert_eq!(
            chips,
            vec![
                ("Signed by Example Ltd".to_string(), false),
                ("x86 on ARM64".to_string(), false),
                ("Runs as admin".to_string(), false),
                ("Efficiency mode on".to_string(), false),
            ]
        );
        let mut d = facts();
        (d.elevated, d.virtualized, d.efficiency, d.architecture) = (Some(false), Some(true), Some(false), None);
        assert_eq!(
            header_chips(&opened(d, Signature::NotSigned)),
            vec![
                ("Not signed".to_string(), true),
                ("Does not run as admin".to_string(), false),
                ("UAC virtualization on".to_string(), false),
            ]
        );
        assert!(header_chips(&opened(facts(), Signature::Unknown)).iter().all(|c| !c.0.contains("signed")));
    }

    #[test]
    fn identity_shows_dashes_for_what_windows_refused() {
        let o = opened(facts(), Signature::Unknown);
        let f = DetailsFrame { start_time: Some(0), ..Default::default() };
        let fields = identity_fields(&o, &f, 45 * 10_000_000);
        let get = |l: &str| fields.iter().find(|(k, _)| k == l).map(|(_, v)| v.clone());
        assert_eq!(get("Path").as_deref(), Some(DASH));
        assert_eq!(get("Command line").as_deref(), Some(DASH));
        assert_eq!(get("Parent").as_deref(), Some(DASH));
        assert_eq!(get("Running for").as_deref(), Some("45 s"));
    }

    #[test]
    fn resources_of_an_exited_process_are_dashes() {
        let o = opened(facts(), Signature::Unknown);
        let f = DetailsFrame { procs: vec![LiveProc { pid: 10, create_time: 1, ..Default::default() }], ..Default::default() };
        let fields = resource_fields(&o, &f);
        assert!(!fields.is_empty());
        assert!(fields.iter().all(|(_, v)| v == DASH), "{fields:?}");
    }

    #[test]
    fn affinity_reads_as_all_or_a_count() {
        assert_eq!(affinity_text(0xff, 0xff), "All CPUs");
        assert_eq!(affinity_text(0x0f, 0xff), "4 of 8 CPUs");
    }
}
