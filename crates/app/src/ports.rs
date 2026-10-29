//! Ports page: rows from the open sockets joined with the snapshot's processes, with the view
//! filter, search, sorting and the count text. Pure data; the Windows reads live in the collector
//! (`negotium_collector::sockets`) and the worker.
use crate::view::Target;
use negotium_collector::sockets::{Protocol, Socket, SocketError, SocketKey, TcpState};
use negotium_snapshot::ProcessSample;
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

/// The socket tables are read again after this long (or on Refresh).
pub const PORTS_REFRESH: Duration = Duration::from_secs(2);

/// The kernel's own sockets (HTTP.sys and others) belong to PID 4; PID 0 is a socket that no
/// longer has an owner (a TCP connection in Time wait).
const SYSTEM_PID: u32 = 4;
const NO_OWNER_PID: u32 = 0;

/// What the Ports page knows of a socket's owner process.
#[derive(Clone, Debug, PartialEq)]
pub struct PortOwner {
    pub pid: u32,
    pub create_time: i64,
    /// Description, or the image name when there is none (as the Overview shows it).
    pub name: String,
    pub process_name: String,
    pub image_path: Option<String>,
    pub services: Vec<String>,
    /// Told to end but not closed yet (`negotium_actions::is_ending`): it keeps its sockets. Holds
    /// the row's second line ([`ending_text`]).
    pub ending: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PortView {
    #[default]
    Listening,
    All,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PortRow {
    pub id: i32,
    /// "Google Chrome", "System", "No owner", "Ended".
    pub app: String,
    /// Service names joined ", ", or "Kernel driver (for example HTTP.sys)".
    pub sub: String,
    /// Image path, empty for none.
    pub icon: String,
    pub pid: u32,
    /// "TCP" or "UDP".
    pub protocol: String,
    pub local_addr: String,
    pub local_port: u16,
    /// Empty when none.
    pub remote_addr: String,
    pub remote_port: Option<u16>,
    pub state: String,
    pub listening: bool,
    /// End task, End process tree, Go to process.
    pub actionable: bool,
    /// Open file location.
    pub can_locate: bool,
    /// Lowercased app, process name, services, addresses and state.
    pub search_text: String,
}

/// Row ids that stay the same across frames for the same socket. Ids only grow and are never
/// reused, so the selection, focus and scroll position survive a refresh. Identical sockets
/// (address reuse) are told apart by their order in the table: `nth` is 0 for the first.
#[derive(Default)]
pub struct PortIds {
    ids: HashMap<(SocketKey, usize), i32>,
    next: i32,
}

impl PortIds {
    pub fn id(&mut self, key: SocketKey) -> i32 {
        self.nth_id(key, 0)
    }

    pub fn nth_id(&mut self, key: SocketKey, nth: usize) -> i32 {
        let next = &mut self.next;
        *self.ids.entry((key, nth)).or_insert_with(|| {
            *next += 1;
            *next
        })
    }

    pub fn retain(&mut self, live: &HashSet<SocketKey>) {
        self.ids.retain(|(k, _), _| live.contains(k));
    }

    fn retain_nth(&mut self, live: &HashSet<(SocketKey, usize)>) {
        self.ids.retain(|k, _| live.contains(k));
    }
}

type SocketSource = Box<dyn FnMut() -> Result<Vec<Socket>, SocketError> + Send>;
type ServiceSource = Box<dyn FnMut() -> Result<HashMap<u32, Vec<String>>, String> + Send>;
/// Of the PIDs given, the ones stuck while they end.
type EndingSource = Box<dyn FnMut(&[u32]) -> HashSet<u32> + Send>;
/// The running network filter drivers, by name (`negotium_actions::drivers::network_filters`).
type FilterSource = Box<dyn FnMut() -> Vec<String> + Send>;
/// Sockets, services by PID, owners stuck while ending, network filter drivers.
pub type LastRead<'a> = (&'a [Socket], &'a HashMap<u32, Vec<String>>, &'a HashSet<u32>, &'a [String]);

/// Reads the socket tables at most every [`PORTS_REFRESH`] and keeps the last list.
pub struct PortReader {
    sockets: SocketSource,
    services: ServiceSource,
    ending: Option<EndingSource>,
    last_ending: HashSet<u32>,
    filters: Option<FilterSource>,
    last_filters: Vec<String>,
    read_at: Option<Instant>,
    last: Vec<Socket>,
    last_services: HashMap<u32, Vec<String>>,
    /// Why the latest socket read failed; kept until one succeeds.
    error: Option<String>,
    /// The latest services failure, until `take_services_failure` hands it out.
    services_failure: Option<String>,
    /// The services read failed last time: a failure now is the same streak.
    services_failing: bool,
    took: Option<Duration>,
}

impl PortReader {
    pub fn new(sockets: SocketSource, services: ServiceSource) -> Self {
        Self {
            sockets,
            services,
            ending: None,
            last_ending: HashSet::new(),
            filters: None,
            last_filters: Vec::new(),
            read_at: None,
            last: Vec::new(),
            last_services: HashMap::new(),
            error: None,
            services_failure: None,
            services_failing: false,
            took: None,
        }
    }

    pub fn live() -> Self {
        Self::new(
            Box::new(negotium_collector::sockets::list_sockets),
            Box::new(|| {
                let mut by_pid: HashMap<u32, Vec<String>> = HashMap::new();
                for s in negotium_actions::services::list_services().map_err(|e| e.to_string())? {
                    if let Some(pid) = s.pid.filter(|&p| p > 0) {
                        by_pid.entry(pid).or_default().push(s.name);
                    }
                }
                Ok(by_pid)
            }),
        )
        .with_ending(Box::new(|pids| pids.iter().copied().filter(|&p| negotium_actions::is_ending(p)).collect()))
        .with_filters(Box::new(negotium_actions::drivers::network_filters))
    }

    /// Also asks `ending` on each socket read which owners are stuck while they end.
    pub fn with_ending(mut self, ending: EndingSource) -> Self {
        self.ending = Some(ending);
        self
    }

    /// Also asks `filters` for the network filter drivers when an owner is first found stuck while
    /// ending, and keeps the answer while any stays stuck.
    pub fn with_filters(mut self, filters: FilterSource) -> Self {
        self.filters = Some(filters);
        self
    }

    /// The owners found stuck while ending at the latest socket read.
    pub fn ending(&self) -> &HashSet<u32> {
        &self.last_ending
    }

    /// The latest sockets, their services, the owners stuck while ending and the network filter
    /// drivers, without reading.
    pub fn last(&self) -> LastRead<'_> {
        (&self.last, &self.last_services, &self.last_ending, &self.last_filters)
    }

    /// Reads again when due (first call, PORTS_REFRESH passed, or `force`); otherwise the last list.
    pub fn sockets(&mut self, now: Instant, force: bool) -> (&[Socket], &HashMap<u32, Vec<String>>, Option<String>) {
        self.took = None;
        let due = force || self.read_at.is_none_or(|t| now.saturating_duration_since(t) >= PORTS_REFRESH);
        if due {
            let started = Instant::now();
            self.read_at = Some(now);
            match (self.sockets)() {
                Ok(list) => {
                    self.last = list;
                    self.error = None;
                    self.read_services();
                    self.read_ending();
                }
                // The last list and services stay; the failure shows until a read succeeds.
                Err(e) => self.error = Some(e.to_string()),
            }
            self.took = Some(started.elapsed());
        }
        (&self.last, &self.last_services, self.error.clone())
    }

    fn read_ending(&mut self) {
        let Some(ending) = self.ending.as_mut() else { return };
        let mut pids: Vec<u32> = self.last.iter().map(|s| s.pid).filter(|&p| p != SYSTEM_PID && p != NO_OWNER_PID).collect();
        pids.sort_unstable();
        pids.dedup();
        let was_stuck = !self.last_ending.is_empty();
        self.last_ending = ending(&pids);
        if self.last_ending.is_empty() {
            self.last_filters.clear();
        } else if !was_stuck && let Some(filters) = self.filters.as_mut() {
            self.last_filters = filters();
        }
    }

    fn read_services(&mut self) {
        match (self.services)() {
            Ok(map) => {
                self.last_services = map;
                self.services_failing = false;
            }
            Err(e) => {
                self.last_services.clear();
                if !self.services_failing {
                    self.services_failure = Some(e);
                }
                self.services_failing = true;
            }
        }
    }

    /// How long the latest `sockets` call spent reading, when it read.
    pub fn took(&self) -> Option<Duration> {
        self.took
    }

    /// The services failure to log: once per failure streak.
    pub fn take_services_failure(&mut self) -> Option<String> {
        self.services_failure.take()
    }
}

pub fn owners_from(processes: &[ProcessSample], services: &HashMap<u32, Vec<String>>) -> HashMap<u32, PortOwner> {
    processes
        .iter()
        .map(|p| {
            let name = p.description.as_deref().filter(|d| !d.is_empty()).unwrap_or(&p.name);
            let owner = PortOwner {
                pid: p.pid,
                create_time: p.create_time,
                name: name.to_string(),
                process_name: p.name.clone(),
                image_path: p.image_path.clone(),
                services: services.get(&p.pid).cloned().unwrap_or_default(),
                ending: None,
            };
            (p.pid, owner)
        })
        .collect()
}

/// The second line of a row whose owner is stuck while ending.
pub fn ending_text(filters: &[String]) -> String {
    if filters.is_empty() {
        "Ending: Windows did not finish closing it".to_string()
    } else {
        format!("Ending: a network filter driver likely holds it ({})", filters.join(", "))
    }
}

/// Marks the owners `ending` names as stuck while they end.
pub fn mark_ending(owners: &mut HashMap<u32, PortOwner>, ending: &HashSet<u32>, filters: &[String]) {
    for pid in ending {
        if let Some(o) = owners.get_mut(pid) {
            o.ending = Some(ending_text(filters));
        }
    }
}

pub fn build_port_rows(sockets: &[Socket], owners: &HashMap<u32, PortOwner>, ids: &mut PortIds) -> Vec<PortRow> {
    let mut live = HashSet::with_capacity(sockets.len());
    let mut seen: HashMap<SocketKey, usize> = HashMap::new();
    let rows = sockets
        .iter()
        .map(|s| {
            let key = s.key();
            let nth = seen.entry(key).or_default();
            let id = ids.nth_id(key, *nth);
            live.insert((key, *nth));
            *nth += 1;
            port_row(s, id, owners)
        })
        .collect();
    ids.retain_nth(&live);
    rows
}

fn port_row(s: &Socket, id: i32, owners: &HashMap<u32, PortOwner>) -> PortRow {
    let owner = owners.get(&s.pid).filter(|_| s.pid != SYSTEM_PID && s.pid != NO_OWNER_PID);
    let (app, sub, process_name) = match (s.pid, owner) {
        (SYSTEM_PID, _) => ("System".to_string(), "Kernel driver (for example HTTP.sys)".to_string(), "System".to_string()),
        (NO_OWNER_PID, _) => ("No owner".to_string(), String::new(), String::new()),
        (_, Some(PortOwner { ending: Some(note), name, process_name, .. })) => (name.clone(), note.clone(), process_name.clone()),
        (_, Some(o)) => (o.name.clone(), o.services.join(", "), o.process_name.clone()),
        (_, None) => ("Ended".to_string(), String::new(), String::new()),
    };
    let local_addr = address_text(&s.local);
    let remote_addr = s.remote.map(|a| address_text(&a)).unwrap_or_default();
    let state = state_label(s);
    let search_text = format!("{app} {process_name} {sub} {local_addr} {remote_addr} {state}").to_lowercase();
    PortRow {
        id,
        app,
        sub,
        icon: owner.and_then(|o| o.image_path.clone()).unwrap_or_default(),
        pid: s.pid,
        protocol: if s.protocol == Protocol::Tcp { "TCP" } else { "UDP" }.to_string(),
        local_addr,
        local_port: s.local.port(),
        remote_addr,
        remote_port: s.remote.map(|a| a.port()),
        state: state.to_string(),
        listening: is_listening(s),
        actionable: owner.is_some(),
        can_locate: owner.is_some_and(|o| o.image_path.is_some()),
        search_text,
    }
}

pub fn state_label(s: &Socket) -> &'static str {
    match s.state {
        None => "Open",
        Some(TcpState::Closed) => "Closed",
        Some(TcpState::Listen) => "Listening",
        Some(TcpState::SynSent) => "SYN sent",
        Some(TcpState::SynReceived) => "SYN received",
        Some(TcpState::Established) => "Established",
        Some(TcpState::FinWait1) => "FIN wait 1",
        Some(TcpState::FinWait2) => "FIN wait 2",
        Some(TcpState::CloseWait) => "Close wait",
        Some(TcpState::Closing) => "Closing",
        Some(TcpState::LastAck) => "Last ACK",
        Some(TcpState::TimeWait) => "Time wait",
        Some(TcpState::DeleteTcb) => "Delete TCB",
        Some(TcpState::Unknown(_)) => "Unknown",
    }
}

/// The address without its port (the port has its own column); an IPv6 scope id shows as `%12`.
pub fn address_text(a: &SocketAddr) -> String {
    match a {
        SocketAddr::V6(v6) if v6.scope_id() != 0 => format!("{}%{}", v6.ip(), v6.scope_id()),
        _ => a.ip().to_string(),
    }
}

/// A TCP socket in LISTEN, or any UDP socket (a bound UDP socket is what holds its port).
pub fn is_listening(s: &Socket) -> bool {
    s.protocol == Protocol::Udp || s.state == Some(TcpState::Listen)
}

pub fn filter_port_rows(rows: &[PortRow], view: PortView, query: &str) -> Vec<PortRow> {
    let q = query.trim().to_lowercase();
    // Digits only: a port or PID, exactly (80 is not 8080).
    let number = (!q.is_empty() && q.bytes().all(|b| b.is_ascii_digit())).then(|| q.parse::<u64>().ok());
    rows.iter()
        .filter(|r| view == PortView::All || r.listening)
        .filter(|r| match number {
            Some(n) => n.is_some_and(|n| u64::from(r.local_port) == n || r.remote_port.is_some_and(|p| u64::from(p) == n) || u64::from(r.pid) == n),
            None => q.is_empty() || r.search_text.contains(&q),
        })
        .cloned()
        .collect()
}

/// An address column's sort key: parsed (IPv4 before IPv6, numbers as numbers), none first.
fn addr_key(s: &str) -> Option<IpAddr> {
    s.split('%').next().and_then(|a| a.parse().ok())
}

/// Columns: 0 App, 1 PID, 2 Protocol, 3 Local address, 4 Local port, 5 Remote address,
/// 6 Remote port, 7 State. Ties: local port, protocol, PID, id.
pub fn sort_port_rows(rows: &mut [PortRow], col: i32, desc: bool) {
    rows.sort_by(|a, b| {
        let ord = match col {
            0 => a.app.to_lowercase().cmp(&b.app.to_lowercase()),
            1 => a.pid.cmp(&b.pid),
            2 => a.protocol.cmp(&b.protocol),
            3 => addr_key(&a.local_addr).cmp(&addr_key(&b.local_addr)),
            5 => addr_key(&a.remote_addr).cmp(&addr_key(&b.remote_addr)),
            6 => a.remote_port.cmp(&b.remote_port),
            7 => a.state.cmp(&b.state),
            _ => a.local_port.cmp(&b.local_port),
        };
        let ord = if desc { ord.reverse() } else { ord };
        ord.then(a.local_port.cmp(&b.local_port))
            .then_with(|| a.protocol.cmp(&b.protocol))
            .then(a.pid.cmp(&b.pid))
            .then(a.id.cmp(&b.id))
    });
}

pub fn count_text(n: usize, query: &str) -> String {
    let q = query.trim();
    let ports = if n == 1 { "port" } else { "ports" };
    match (q.is_empty(), n == 1) {
        (true, _) => format!("{n} {ports}"),
        (false, true) => format!("{n} {ports} matches {q}"),
        (false, false) => format!("{n} {ports} match {q}"),
    }
}

/// The action target of a row: its owner as a single process. None for the kernel, an ended
/// owner and an unowned socket.
pub fn port_target(row: &PortRow, owners: &HashMap<u32, PortOwner>) -> Option<Target> {
    let o = owners.get(&row.pid).filter(|_| row.actionable)?;
    Some(Target {
        name: o.name.clone(),
        process_name: o.process_name.clone(),
        root: Some((o.pid, o.create_time)),
        procs: vec![(o.pid, o.create_time)],
        hung_pid: None,
        image_path: o.image_path.clone(),
        critical: negotium_actions::is_critical(&o.process_name),
        group: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use negotium_collector::sockets::{Protocol, TcpState};
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddrV6};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn sa(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    fn tcp(local: &str, remote: Option<&str>, state: TcpState, pid: u32) -> Socket {
        Socket { protocol: Protocol::Tcp, local: sa(local), remote: remote.map(sa), state: Some(state), pid }
    }

    fn udp(local: &str, pid: u32) -> Socket {
        Socket { protocol: Protocol::Udp, local: sa(local), remote: None, state: None, pid }
    }

    fn owner(pid: u32, name: &str, process: &str, path: Option<&str>, services: &[&str]) -> PortOwner {
        PortOwner {
            pid,
            create_time: 1000 + pid as i64,
            name: name.into(),
            process_name: process.into(),
            image_path: path.map(String::from),
            services: services.iter().map(|s| s.to_string()).collect(),
            ending: None,
        }
    }

    fn owners(list: Vec<PortOwner>) -> HashMap<u32, PortOwner> {
        list.into_iter().map(|o| (o.pid, o)).collect()
    }

    fn one_row(s: Socket, owners: &HashMap<u32, PortOwner>) -> PortRow {
        build_port_rows(&[s], owners, &mut PortIds::default()).pop().expect("one row")
    }

    #[test]
    fn a_known_owner_names_the_row_and_allows_every_action() {
        let o = owners(vec![owner(200, "Node.js", "node.exe", Some("C:\\node\\node.exe"), &[])]);
        let r = one_row(tcp("0.0.0.0:5173", None, TcpState::Listen, 200), &o);
        assert_eq!((r.app.as_str(), r.sub.as_str(), r.icon.as_str()), ("Node.js", "", "C:\\node\\node.exe"));
        assert_eq!((r.pid, r.protocol.as_str(), r.local_addr.as_str(), r.local_port), (200, "TCP", "0.0.0.0", 5173));
        assert_eq!((r.remote_addr.as_str(), r.remote_port, r.state.as_str(), r.listening), ("", None, "Listening", true));
        assert!(r.actionable && r.can_locate);
    }

    #[test]
    fn an_owner_without_an_image_path_cannot_be_located() {
        let o = owners(vec![owner(200, "Node.js", "node.exe", None, &[])]);
        let r = one_row(tcp("0.0.0.0:5173", None, TcpState::Listen, 200), &o);
        assert!(r.actionable && !r.can_locate);
        assert_eq!(r.icon, "");
    }

    #[test]
    fn services_hosted_by_the_owner_show_under_its_name() {
        let o = owners(vec![owner(1420, "Host Process for Windows Services", "svchost.exe", None, &["RpcSs", "EventLog"])]);
        let r = one_row(udp("0.0.0.0:135", 1420), &o);
        assert_eq!(r.sub, "RpcSs, EventLog");
    }

    #[test]
    fn pid_4_is_the_system_without_actions() {
        let o = owners(vec![owner(4, "System", "System", None, &[])]);
        let r = one_row(tcp("0.0.0.0:80", None, TcpState::Listen, 4), &o);
        assert_eq!((r.app.as_str(), r.sub.as_str()), ("System", "Kernel driver (for example HTTP.sys)"));
        assert!(!r.actionable && !r.can_locate);
    }

    #[test]
    fn pid_0_has_no_owner_and_no_actions() {
        let r = one_row(tcp("127.0.0.1:50001", Some("127.0.0.1:5173"), TcpState::TimeWait, 0), &HashMap::new());
        assert_eq!((r.app.as_str(), r.sub.as_str()), ("No owner", ""));
        assert!(!r.actionable && !r.can_locate);
    }

    #[test]
    fn a_pid_missing_from_the_snapshot_is_ended_without_actions() {
        let r = one_row(tcp("0.0.0.0:3000", None, TcpState::Listen, 999), &HashMap::new());
        assert_eq!((r.app.as_str(), r.pid), ("Ended", 999));
        assert!(!r.actionable && !r.can_locate);
        assert_eq!(r.icon, "");
    }

    #[test]
    fn state_label_names_every_tcp_state_and_udp() {
        use TcpState::*;
        let states = [
            (Closed, "Closed"),
            (Listen, "Listening"),
            (SynSent, "SYN sent"),
            (SynReceived, "SYN received"),
            (Established, "Established"),
            (FinWait1, "FIN wait 1"),
            (FinWait2, "FIN wait 2"),
            (CloseWait, "Close wait"),
            (Closing, "Closing"),
            (LastAck, "Last ACK"),
            (TimeWait, "Time wait"),
            (DeleteTcb, "Delete TCB"),
            (Unknown(77), "Unknown"),
        ];
        for (state, label) in states {
            assert_eq!(state_label(&tcp("127.0.0.1:1", None, state, 1)), label, "{state:?}");
        }
        assert_eq!(state_label(&udp("127.0.0.1:1", 1)), "Open");
    }

    #[test]
    fn address_text_has_no_brackets_and_shows_a_scope_id() {
        let v6 = |ip: Ipv6Addr, scope| SocketAddr::V6(SocketAddrV6::new(ip, 1, 0, scope));
        let link_local = Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1);
        assert_eq!(address_text(&SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 1)), "0.0.0.0");
        assert_eq!(address_text(&SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 1)), "127.0.0.1");
        assert_eq!(address_text(&v6(Ipv6Addr::UNSPECIFIED, 0)), "::");
        assert_eq!(address_text(&v6(Ipv6Addr::LOCALHOST, 0)), "::1");
        assert_eq!(address_text(&v6(link_local, 12)), "fe80::1%12");
    }

    #[test]
    fn only_listening_tcp_and_udp_sockets_are_listening() {
        assert!(is_listening(&tcp("0.0.0.0:1", None, TcpState::Listen, 1)));
        for s in [TcpState::Established, TcpState::TimeWait, TcpState::SynSent] {
            assert!(!is_listening(&tcp("0.0.0.0:1", None, s, 1)), "{s:?}");
        }
        assert!(is_listening(&udp("0.0.0.0:1", 1)));
    }

    /// Ten sockets whose local ports are all different, so a result reads as a list of ports.
    fn fixture_rows() -> Vec<PortRow> {
        let o = owners(vec![
            owner(100, "Google Chrome", "chrome.exe", None, &[]),
            owner(80, "Web80", "web80.exe", None, &[]),
            owner(800, "Eight", "eight.exe", None, &[]),
            owner(1420, "Host Process", "svchost.exe", None, &["RpcSs", "EventLog"]),
            owner(200, "Node.js", "node.exe", None, &[]),
        ]);
        let sockets = [
            tcp("0.0.0.0:80", None, TcpState::Listen, 200),
            tcp("0.0.0.0:8080", None, TcpState::Listen, 100),
            tcp("0.0.0.0:180", None, TcpState::Listen, 100),
            tcp("127.0.0.1:50000", Some("10.0.0.9:80"), TcpState::Established, 100),
            tcp("[::]:5000", None, TcpState::Listen, 80),
            tcp("0.0.0.0:5001", None, TcpState::Listen, 800),
            tcp("127.0.0.1:50001", Some("127.0.0.1:5173"), TcpState::TimeWait, 0),
            tcp("127.0.0.1:5173", None, TcpState::Listen, 200),
            udp("0.0.0.0:5353", 1420),
            tcp("127.0.0.1:50002", Some("127.0.0.1:5173"), TcpState::Established, 200),
        ];
        build_port_rows(&sockets, &o, &mut PortIds::default())
    }

    fn local_ports(rows: &[PortRow]) -> Vec<u16> {
        rows.iter().map(|r| r.local_port).collect()
    }

    #[test]
    fn listening_view_hides_connections_and_keeps_tcp_listeners_and_udp() {
        let rows = fixture_rows();
        let got = local_ports(&filter_port_rows(&rows, PortView::Listening, ""));
        assert_eq!(got, [80, 8080, 180, 5000, 5001, 5173, 5353]);
        assert_eq!(filter_port_rows(&rows, PortView::All, "").len(), 10);
    }

    #[test]
    fn a_number_matches_a_port_or_pid_exactly() {
        let rows = fixture_rows();
        // Local port 80, remote port 80 and PID 80; not 8080, 180 or PID 800.
        let got = local_ports(&filter_port_rows(&rows, PortView::All, "80"));
        assert_eq!(got, [80, 50000, 5000]);
    }

    #[test]
    fn spaces_around_a_query_are_ignored_and_an_empty_query_filters_nothing() {
        let rows = fixture_rows();
        let want = local_ports(&filter_port_rows(&rows, PortView::All, "5173"));
        assert_eq!(want, [50001, 5173, 50002]);
        assert_eq!(local_ports(&filter_port_rows(&rows, PortView::All, " 5173 ")), want);
        assert_eq!(filter_port_rows(&rows, PortView::All, "").len(), 10);
        assert_eq!(filter_port_rows(&rows, PortView::All, "   ").len(), 10);
    }

    #[test]
    fn text_matches_app_process_service_address_and_state_ignoring_case() {
        let rows = fixture_rows();
        let all = |q: &str| local_ports(&filter_port_rows(&rows, PortView::All, q));
        assert_eq!(all("CHROME"), [8080, 180, 50000], "app name");
        assert_eq!(all("chrome.exe"), [8080, 180, 50000], "process name");
        assert_eq!(all("rpcss"), [5353], "service name");
        assert_eq!(all("127.0.0"), [50000, 50001, 5173, 50002], "address");
        assert_eq!(all("time"), [50001], "state");
    }

    #[test]
    fn the_view_still_applies_to_a_search() {
        let rows = fixture_rows();
        let got = local_ports(&filter_port_rows(&rows, PortView::Listening, "5173"));
        assert_eq!(got, [5173], "the Established and Time wait rows on 5173 stay hidden");
    }

    #[allow(clippy::too_many_arguments)] // one argument per column keeps the sort fixtures readable
    fn r(id: i32, app: &str, pid: u32, proto: &str, la: &str, lp: u16, ra: &str, rp: Option<u16>, state: &str) -> PortRow {
        PortRow {
            id,
            app: app.into(),
            pid,
            protocol: proto.into(),
            local_addr: la.into(),
            local_port: lp,
            remote_addr: ra.into(),
            remote_port: rp,
            state: state.into(),
            ..Default::default()
        }
    }

    fn sort_fixture() -> Vec<PortRow> {
        vec![
            r(1, "alpha", 30, "TCP", "10.0.0.2", 80, "9.9.9.9", Some(443), "Established"),
            r(2, "Bravo", 10, "UDP", "127.0.0.1", 5000, "", None, "Open"),
            r(3, "charlie", 20, "TCP", "::1", 9, "1.1.1.1", Some(53), "Listening"),
        ]
    }

    fn sorted_ids(col: i32, desc: bool) -> Vec<i32> {
        let mut rows = sort_fixture();
        sort_port_rows(&mut rows, col, desc);
        rows.iter().map(|r| r.id).collect()
    }

    #[test]
    fn every_column_sorts_ascending_and_descending() {
        // (column, ascending ids, descending ids); equal protocols fall to the local port.
        let expected = [
            (0, [1, 2, 3], [3, 2, 1]),
            (1, [2, 3, 1], [1, 3, 2]),
            (2, [3, 1, 2], [2, 3, 1]),
            (3, [1, 2, 3], [3, 2, 1]),
            (4, [3, 1, 2], [2, 1, 3]),
            (5, [2, 3, 1], [1, 3, 2]),
            (6, [2, 3, 1], [1, 3, 2]),
            (7, [1, 3, 2], [2, 3, 1]),
        ];
        for (col, asc, desc) in expected {
            assert_eq!(sorted_ids(col, false), asc, "column {col} ascending");
            assert_eq!(sorted_ids(col, true), desc, "column {col} descending");
        }
    }

    #[test]
    fn ports_sort_as_numbers_not_text() {
        let mut rows = vec![
            r(1, "a", 1, "TCP", "0.0.0.0", 5173, "", None, "Listening"),
            r(2, "a", 1, "TCP", "0.0.0.0", 10, "", None, "Listening"),
            r(3, "a", 1, "TCP", "0.0.0.0", 9, "", None, "Listening"),
        ];
        sort_port_rows(&mut rows, 4, false);
        assert_eq!(rows.iter().map(|r| r.local_port).collect::<Vec<_>>(), [9, 10, 5173]);
    }

    #[test]
    fn equal_values_fall_to_local_port_then_protocol_then_pid() {
        let mut rows = vec![
            r(1, "same", 5, "UDP", "0.0.0.0", 80, "", None, "Open"),
            r(2, "same", 9, "TCP", "0.0.0.0", 80, "", None, "Listening"),
            r(3, "same", 1, "TCP", "0.0.0.0", 22, "", None, "Listening"),
            r(4, "same", 3, "TCP", "0.0.0.0", 80, "", None, "Listening"),
        ];
        sort_port_rows(&mut rows, 0, false);
        assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), [3, 4, 2, 1]);
    }

    #[test]
    fn a_missing_remote_port_sorts_before_any_number() {
        let mut rows = vec![
            r(1, "a", 1, "TCP", "0.0.0.0", 1, "x", Some(0), "Established"),
            r(2, "a", 1, "TCP", "0.0.0.0", 2, "", None, "Listening"),
        ];
        sort_port_rows(&mut rows, 6, false);
        assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), [2, 1]);
    }

    #[test]
    fn the_count_text_counts_ports_and_names_the_query() {
        assert_eq!(count_text(1, ""), "1 port");
        assert_eq!(count_text(3, ""), "3 ports");
        assert_eq!(count_text(0, ""), "0 ports");
        assert_eq!(count_text(3, "5173"), "3 ports match 5173");
        assert_eq!(count_text(1, "5173"), "1 port matches 5173");
        assert_eq!(count_text(1, "  "), "1 port", "a blank query is no query");
    }

    fn key(pid: u32, port: u16) -> SocketKey {
        tcp(&format!("127.0.0.1:{port}"), None, TcpState::Listen, pid).key()
    }

    #[test]
    fn row_ids_are_stable_distinct_and_never_reused() {
        let mut ids = PortIds::default();
        let (a, b) = (ids.id(key(1, 80)), ids.id(key(1, 81)));
        assert_eq!(ids.id(key(1, 80)), a, "the same socket keeps its id");
        assert_ne!(a, b);
        ids.retain(&HashSet::from([key(1, 81)]));
        let again = ids.id(key(1, 80));
        assert!(again != a && again != b, "a returning socket gets a new id: {again}");
    }

    #[test]
    fn building_rows_keeps_ids_across_frames_and_drops_gone_sockets() {
        let mut ids = PortIds::default();
        let (s1, s2) = (udp("0.0.0.0:1000", 1), udp("0.0.0.0:2000", 1));
        let first = build_port_rows(&[s1, s2], &HashMap::new(), &mut ids);
        let second = build_port_rows(&[s2], &HashMap::new(), &mut ids);
        assert_eq!(second[0].id, first[1].id);
        let third = build_port_rows(&[s1, s2], &HashMap::new(), &mut ids);
        assert_ne!(third[0].id, first[0].id, "the socket that went away came back as a new row");
    }

    #[test]
    fn identical_sockets_get_their_own_rows_and_ids() {
        // Address reuse: one process with two UDP sockets on the same address and port.
        let mut ids = PortIds::default();
        let s = udp("0.0.0.0:5353", 1);
        let first = build_port_rows(&[s, s], &HashMap::new(), &mut ids);
        assert_eq!(first.len(), 2);
        assert_ne!(first[0].id, first[1].id, "each socket has its own row id");
        let second = build_port_rows(&[s, s], &HashMap::new(), &mut ids);
        assert_eq!((second[0].id, second[1].id), (first[0].id, first[1].id), "ids stay across frames");
        let third = build_port_rows(&[s], &HashMap::new(), &mut ids);
        assert_eq!(third[0].id, first[0].id, "the one left keeps the first id");
    }

    #[test]
    fn a_known_owner_is_an_action_target_for_its_one_process() {
        let o = owners(vec![owner(200, "Node.js", "node.exe", Some("C:\\node.exe"), &[])]);
        let row = one_row(tcp("0.0.0.0:5173", None, TcpState::Listen, 200), &o);
        let t = port_target(&row, &o).expect("a target");
        assert_eq!((t.name.as_str(), t.process_name.as_str()), ("Node.js", "node.exe"));
        assert_eq!((t.root, t.procs.clone()), (Some((200, 1200)), vec![(200, 1200)]));
        assert_eq!((t.image_path.as_deref(), t.critical, t.group, t.hung_pid), (Some("C:\\node.exe"), false, false, None));
    }

    #[test]
    fn a_critical_owner_is_marked_critical() {
        let o = owners(vec![owner(600, "Client Server Runtime Process", "csrss.exe", None, &[])]);
        let row = one_row(tcp("0.0.0.0:1", None, TcpState::Listen, 600), &o);
        assert!(port_target(&row, &o).expect("a target").critical);
    }

    #[test]
    fn system_unowned_and_ended_rows_have_no_target() {
        let o = owners(vec![owner(4, "System", "System", None, &[])]);
        for pid in [4, 0, 999] {
            let row = one_row(tcp("0.0.0.0:1", None, TcpState::Listen, pid), &o);
            assert_eq!(port_target(&row, &o), None, "pid {pid}");
        }
    }

    /// A reader whose sources count their reads. `socket_results` are handed out in order; the
    /// last one repeats.
    fn reader(
        socket_results: Vec<Result<Vec<Socket>, SocketError>>,
        services: Vec<Result<HashMap<u32, Vec<String>>, String>>,
    ) -> (PortReader, Arc<AtomicUsize>, Arc<AtomicUsize>) {
        let (sock_calls, svc_calls) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let (sc, vc) = (sock_calls.clone(), svc_calls.clone());
        let r = PortReader::new(
            Box::new(move || {
                let i = sc.fetch_add(1, Ordering::SeqCst);
                socket_results[i.min(socket_results.len() - 1)].clone()
            }),
            Box::new(move || {
                let i = vc.fetch_add(1, Ordering::SeqCst);
                services[i.min(services.len() - 1)].clone()
            }),
        );
        (r, sock_calls, svc_calls)
    }

    fn one_socket() -> Vec<Socket> {
        vec![udp("0.0.0.0:1", 1)]
    }

    #[test]
    fn the_reader_reads_the_first_time_and_then_every_two_seconds() {
        let (mut r, calls, _) = reader(vec![Ok(one_socket())], vec![Ok(HashMap::new())]);
        let t0 = Instant::now();
        assert_eq!(r.sockets(t0, false).0.len(), 1, "the first call reads");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        r.sockets(t0 + Duration::from_secs(1), false);
        assert_eq!(calls.load(Ordering::SeqCst), 1, "1 s later: the last list");
        r.sockets(t0 + PORTS_REFRESH, false);
        assert_eq!(calls.load(Ordering::SeqCst), 2, "2 s later: read again");
    }

    #[test]
    fn a_forced_call_reads_at_once() {
        let (mut r, calls, _) = reader(vec![Ok(one_socket())], vec![Ok(HashMap::new())]);
        let t0 = Instant::now();
        r.sockets(t0, false);
        r.sockets(t0 + Duration::from_millis(500), true);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(r.took().is_some(), "a call that read says how long it took");
        r.sockets(t0 + Duration::from_millis(600), false);
        assert_eq!(r.took(), None, "a call that did not read says nothing");
    }

    #[test]
    fn a_failed_read_keeps_the_last_list_and_reports_until_a_read_succeeds() {
        let err = SocketError::Api { table: "GetExtendedTcpTable (IPv4)", code: 87 };
        let (mut r, _, _) = reader(vec![Ok(one_socket()), Err(err), Ok(vec![])], vec![Ok(HashMap::new())]);
        let t0 = Instant::now();
        let (list, _, e) = r.sockets(t0, false);
        assert_eq!((list.len(), e), (1, None));
        let (list, _, e) = r.sockets(t0 + Duration::from_millis(100), true);
        assert_eq!((list.len(), e.as_deref()), (1, Some("GetExtendedTcpTable (IPv4) failed: 0x57")));
        let (_, _, e) = r.sockets(t0 + Duration::from_millis(200), false);
        assert!(e.is_some(), "the failure stands until a read succeeds");
        let (list, _, e) = r.sockets(t0 + Duration::from_millis(300), true);
        assert_eq!((list.len(), e), (0, None));
    }

    #[test]
    fn a_services_failure_gives_empty_services_no_error_and_one_log_per_streak() {
        let svc = HashMap::from([(1u32, vec!["RpcSs".to_string()])]);
        let (mut r, _, _) = reader(
            vec![Ok(one_socket())],
            vec![Ok(svc), Err("access denied".into()), Err("access denied".into()), Ok(HashMap::new()), Err("again".into())],
        );
        let t0 = Instant::now();
        let (_, s, _) = r.sockets(t0, false);
        assert_eq!(s.len(), 1);
        let (list, s, e) = r.sockets(t0 + Duration::from_millis(1), true);
        assert_eq!((list.len(), s.len(), e), (1, 0, None));
        assert_eq!(r.take_services_failure().as_deref(), Some("access denied"));
        r.sockets(t0 + Duration::from_millis(2), true);
        assert_eq!(r.take_services_failure(), None, "the same streak is logged once");
        r.sockets(t0 + Duration::from_millis(3), true);
        r.sockets(t0 + Duration::from_millis(4), true);
        assert_eq!(r.take_services_failure().as_deref(), Some("again"), "a new streak is logged again");
    }

    #[test]
    fn owners_use_the_description_then_the_image_name() {
        let p = |pid: u32, name: &str, desc: Option<&str>, path: Option<&str>| ProcessSample {
            pid,
            create_time: 50 + pid as i64,
            name: name.into(),
            description: desc.map(String::from),
            image_path: path.map(String::from),
            ..Default::default()
        };
        let services = HashMap::from([(3u32, vec!["Dnscache".to_string()])]);
        let got = owners_from(
            &[p(1, "a.exe", Some("Alpha"), Some("C:\\a.exe")), p(2, "b.exe", None, None), p(3, "c.exe", Some(""), None)],
            &services,
        );
        assert_eq!(got[&1].name, "Alpha");
        assert_eq!((got[&1].create_time, got[&1].image_path.as_deref(), got[&1].process_name.as_str()), (51, Some("C:\\a.exe"), "a.exe"));
        assert_eq!(got[&2].name, "b.exe");
        assert_eq!((got[&3].name.as_str(), got[&3].services.clone()), ("c.exe", vec!["Dnscache".to_string()]));
    }

    #[test]
    fn an_owner_stuck_while_ending_says_so_and_keeps_its_actions() {
        let mut o = owner(118960, "Node.js", "node.exe", Some("C:\node.exe"), &["Svc"]);
        o.ending = Some(ending_text(&["Portmaster".into()]));
        let o = owners(vec![o]);
        let r = one_row(tcp("0.0.0.0:8080", None, TcpState::Listen, 118960), &o);
        assert_eq!((r.app.as_str(), r.sub.as_str()), ("Node.js", "Ending: a network filter driver likely holds it (Portmaster)"));
        assert!(r.actionable && r.can_locate, "End task explains why it can't end it");
        assert_eq!(filter_port_rows(&[r], PortView::Listening, "ending").len(), 1, "the search finds it");
    }

    #[test]
    fn marking_sets_ending_only_on_the_owners_named() {
        let mut o = owners(vec![owner(1, "A", "a.exe", None, &[]), owner(2, "B", "b.exe", None, &[])]);
        mark_ending(&mut o, &HashSet::from([2, 99]), &["Portmaster".into()]);
        assert_eq!(o[&1].ending, None);
        assert_eq!(o[&2].ending.as_deref(), Some("Ending: a network filter driver likely holds it (Portmaster)"));
        assert!(!o.contains_key(&99));
    }

    #[test]
    fn the_ending_check_runs_on_each_socket_read_with_the_owner_pids() {
        let asked = Arc::new(std::sync::Mutex::new(Vec::<Vec<u32>>::new()));
        let log = asked.clone();
        let sockets = vec![
            tcp("0.0.0.0:80", None, TcpState::Listen, 7),
            udp("0.0.0.0:53", 7),
            udp("0.0.0.0:54", 3),
            tcp("0.0.0.0:445", None, TcpState::Listen, SYSTEM_PID),
            tcp("10.0.0.1:5000", Some("10.0.0.2:80"), TcpState::TimeWait, NO_OWNER_PID),
        ];
        let (r, _, _) = reader(vec![Ok(sockets), Err(SocketError::KeptGrowing { table: "t" })], vec![Ok(HashMap::new())]);
        let mut r = r.with_ending(Box::new(move |pids| {
            log.lock().unwrap().push(pids.to_vec());
            HashSet::from([7])
        }));
        let t0 = Instant::now();
        r.sockets(t0, false);
        assert_eq!(*asked.lock().unwrap(), vec![vec![3, 7]], "distinct owners, sorted, not PID 0 or 4");
        assert_eq!(r.ending(), &HashSet::from([7]));
        r.sockets(t0 + Duration::from_secs(1), false);
        assert_eq!(asked.lock().unwrap().len(), 1, "not asked between reads");
        r.sockets(t0 + PORTS_REFRESH, false);
        assert_eq!(asked.lock().unwrap().len(), 1, "a failed socket read asks nothing");
        assert_eq!(r.ending(), &HashSet::from([7]), "and keeps the last answer");
    }

    #[test]
    fn the_ending_line_names_the_network_filters_or_says_windows_did_not_finish() {
        assert_eq!(
            ending_text(&["NordVPN".into(), "Portmaster".into()]),
            "Ending: a network filter driver likely holds it (NordVPN, Portmaster)"
        );
        assert_eq!(ending_text(&[]), "Ending: Windows did not finish closing it");
    }

    #[test]
    fn the_filters_are_asked_once_while_owners_stay_stuck() {
        let (r, _, _) = reader(vec![Ok(vec![udp("0.0.0.0:53", 7)])], vec![Ok(HashMap::new())]);
        let answers = Arc::new(std::sync::Mutex::new(vec![HashSet::from([7]), HashSet::from([7]), HashSet::new(), HashSet::from([7])]));
        let filter_calls = Arc::new(AtomicUsize::new(0));
        let fc = filter_calls.clone();
        let mut r = r
            .with_ending(Box::new(move |_| answers.lock().unwrap().remove(0)))
            .with_filters(Box::new(move || {
                fc.fetch_add(1, Ordering::SeqCst);
                vec!["Portmaster".to_string()]
            }));
        let t0 = Instant::now();
        r.sockets(t0, true);
        assert_eq!((filter_calls.load(Ordering::SeqCst), r.last().3), (1, &["Portmaster".to_string()][..]));
        r.sockets(t0, true);
        assert_eq!(filter_calls.load(Ordering::SeqCst), 1, "still stuck: the last answer is kept");
        r.sockets(t0, true);
        assert!(r.last().3.is_empty(), "none stuck: no filters to show");
        r.sockets(t0, true);
        assert_eq!(filter_calls.load(Ordering::SeqCst), 2, "stuck again: asked again");
    }
}
