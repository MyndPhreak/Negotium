//! Live per-process network from an ETW real-time session on the Microsoft-Windows-Kernel-Network
//! provider (`{7DD42A49-5329-4832-8DFD-43D979153A88}`). Needs an elevated token: unelevated,
//! `StartTraceW` is denied and the collector never tries.
//!
//! The events counted, from the provider manifest (`wevtutil gp Microsoft-Windows-Kernel-Network
//! /ge /gm` for ids and keywords, `(Get-WinEvent -ListProvider Microsoft-Windows-Kernel-Network)
//! .Events` for the templates; every one is version 0):
//!
//! | id | task  | event          | keyword | payload (little-endian, packed)                         |
//! |----|-------|----------------|---------|---------------------------------------------------------|
//! | 10 | TCPIP | TCPv4 send     | 0x10    | PID u32 @0, size u32 @4, daddr 4 @8, saddr 4 @12, ...    |
//! | 11 | TCPIP | TCPv4 receive  | 0x10    | PID u32 @0, size u32 @4, daddr 4 @8, saddr 4 @12, ...    |
//! | 26 | TCPIP | TCPv6 send     | 0x20    | PID u32 @0, size u32 @4, daddr 16 @8, saddr 16 @24, ...  |
//! | 27 | TCPIP | TCPv6 receive  | 0x20    | PID u32 @0, size u32 @4, daddr 16 @8, saddr 16 @24, ...  |
//! | 42 | UDPIP | UDPv4 send     | 0x10    | PID u32 @0, size u32 @4, daddr 4 @8, saddr 4 @12, ...    |
//! | 43 | UDPIP | UDPv4 receive  | 0x10    | PID u32 @0, size u32 @4, daddr 4 @8, saddr 4 @12, ...    |
//! | 58 | UDPIP | UDPv6 send     | 0x20    | PID u32 @0, size u32 @4, daddr 16 @8, saddr 16 @24, ...  |
//! | 59 | UDPIP | UDPv6 receive  | 0x20    | PID u32 @0, size u32 @4, daddr 16 @8, saddr 16 @24, ...  |
//!
//! The rest of each payload (dport u16, sport u16, then startime/endtime u32 on the TCP sends,
//! seqnum u32, connid u32) is not read. IPv4 addresses are `win:UInt32` shown as `win:IPv4`, i.e.
//! an `in_addr` in network byte order: the four bytes are the dotted octets in order. IPv6
//! addresses are 16 raw bytes. `PID` is the process the traffic belongs to (the event header's
//! process is often System for receives). Not counted: connect, disconnect, accept, reconnect
//! and fail (12, 13, 15, 16, 17, 28, 29, 31, 32, 49: no bytes), retransmits (14, 30: not new data)
//! and the "copied on behalf of user" events 18/34 (already counted by the receive). Both keywords (`KERNEL_NETWORK_KEYWORD_IPV4` 0x10,
//! `KERNEL_NETWORK_KEYWORD_IPV6` 0x20) cover TCP and UDP alike.
//!
//! The callback adds into a `Mutex<HashMap<pid, NetBytes>>` that the collector swaps out each
//! tick (`take`). It allocates only for a PID the map doesn't hold: `take` zeroes the counts in
//! place and keeps every PID that had traffic, so a steady sender never re-enters the map; a PID
//! idle for a whole tick is dropped.
use negotium_snapshot::{NetRate, ProcessSample};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::c_void;
use std::mem::{offset_of, size_of};
use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Instant;
use windows::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, ERROR_SUCCESS, GetLastError, HANDLE, WIN32_ERROR};
use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows::Win32::System::Diagnostics::Etw::{
    CONTROLTRACE_HANDLE, CloseTrace, ControlTraceW, EVENT_CONTROL_CODE_ENABLE_PROVIDER, EVENT_RECORD,
    EVENT_TRACE_CONTROL, EVENT_TRACE_CONTROL_STOP, EVENT_TRACE_LOGFILEW,
    EVENT_TRACE_PROPERTIES, EVENT_TRACE_REAL_TIME_MODE, EnableTraceEx2, OpenTraceW, PROCESS_TRACE_MODE_EVENT_RECORD,
    PROCESS_TRACE_MODE_REAL_TIME, PROCESSTRACE_HANDLE, ProcessTrace, StartTraceW, TRACE_LEVEL_INFORMATION,
    WNODE_FLAG_TRACED_GUID,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::core::{GUID, PCWSTR, PWSTR};

/// The app's session. Fixed, so a session leaked by a crash is found and replaced at the next start.
pub const SESSION_NAME: &str = "Negotium-Net";

const PROVIDER: GUID = GUID::from_u128(0x7dd42a49_5329_4832_8dfd_43d979153a88);
const KEYWORD_IPV4: u64 = 0x10;
const KEYWORD_IPV6: u64 = 0x20;
/// What `OpenTraceW` returns on failure (`INVALID_PROCESSTRACE_HANDLE` on 64-bit Windows).
const INVALID_PROCESSTRACE: u64 = u64::MAX;

/// Bytes one process moved since the last `take`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NetBytes {
    pub sent: u64,
    pub recv: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Direction {
    Sent,
    Recv,
}

/// The process, direction and size of one send/receive event (table in the module docs). `None`
/// for any other event, a payload too short for its layout, and loopback traffic (either end in
/// 127.0.0.0/8, `::1`, or IPv4-mapped loopback).
pub(crate) fn parse(event_id: u16, payload: &[u8]) -> Option<(u32, Direction, u32)> {
    let (direction, v6) = match event_id {
        10 | 42 => (Direction::Sent, false),
        11 | 43 => (Direction::Recv, false),
        26 | 58 => (Direction::Sent, true),
        27 | 59 => (Direction::Recv, true),
        _ => return None,
    };
    let u32_at = |at: usize| payload.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let (pid, bytes) = (u32_at(0)?, u32_at(4)?);
    let loopback = if v6 {
        let addr = |at: usize| payload.get(at..at + 16).and_then(|b| <[u8; 16]>::try_from(b).ok()).map(Ipv6Addr::from);
        let (d, s) = (addr(8)?, addr(24)?);
        is_loopback_v6(d) || is_loopback_v6(s)
    } else {
        let addr = |at: usize| payload.get(at..at + 4).and_then(|b| <[u8; 4]>::try_from(b).ok()).map(Ipv4Addr::from);
        let (d, s) = (addr(8)?, addr(12)?);
        d.is_loopback() || s.is_loopback()
    };
    (!loopback).then_some((pid, direction, bytes))
}

fn is_loopback_v6(a: Ipv6Addr) -> bool {
    a.is_loopback() || a.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback())
}

/// The shared sums: written by the event callback, swapped out by the collector.
#[derive(Default)]
pub(crate) struct Sums(Mutex<HashMap<u32, NetBytes>>);

impl Sums {
    pub(crate) fn add(&self, pid: u32, direction: Direction, bytes: u32) {
        let mut map = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let entry = map.entry(pid).or_default();
        match direction {
            Direction::Sent => entry.sent = entry.sent.saturating_add(bytes as u64),
            Direction::Recv => entry.recv = entry.recv.saturating_add(bytes as u64),
        }
    }

    /// Bytes per PID since the last take (PIDs with traffic only), counts reset.
    pub(crate) fn take(&self) -> HashMap<u32, NetBytes> {
        let mut map = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let mut out = HashMap::with_capacity(map.len());
        // Idle since the last take: dropped (a PID that exited, or went quiet). The rest keep
        // their slot with zeroed counts, so the callback doesn't allocate for them again.
        map.retain(|pid, b| {
            if *b == NetBytes::default() {
                return false;
            }
            out.insert(*pid, std::mem::take(b));
            true
        });
        out
    }

    #[cfg(test)]
    fn capacity(&self) -> usize {
        self.0.lock().unwrap().capacity()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetFeedError {
    NotElevated,
    Os { call: &'static str, code: u32 },
}

impl std::fmt::Display for NetFeedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NetFeedError::NotElevated => write!(f, "needs admin rights"),
            NetFeedError::Os { code, .. } => write!(f, "couldn't start the network trace (error {code})"),
        }
    }
}

impl std::error::Error for NetFeedError {}

/// What the collector needs from a feed; a fake stands in for the session in tests.
pub(crate) trait TakeBytes {
    fn take(&self) -> HashMap<u32, NetBytes>;
    /// False once the session ended under the feed (stopped from outside, e.g. by another
    /// process starting a session of the same name).
    fn running(&self) -> bool;
}

/// `EVENT_TRACE_PROPERTIES` followed by room for the session name, where `StartTraceW` copies it
/// and the control calls write it back.
#[repr(C)]
struct Properties {
    props: EVENT_TRACE_PROPERTIES,
    name: [u16; 256],
}

impl Properties {
    fn new() -> Box<Self> {
        let mut p = Box::new(Properties { props: EVENT_TRACE_PROPERTIES::default(), name: [0; 256] });
        p.props.Wnode.BufferSize = size_of::<Properties>() as u32;
        p.props.LoggerNameOffset = offset_of!(Properties, name) as u32;
        p
    }
}

fn wide(name: &str) -> Vec<u16> {
    name.encode_utf16().chain(Some(0)).collect()
}

/// `ControlTraceW` on the session called `name` (or, when `session` is non-zero, that handle).
fn control(session: CONTROLTRACE_HANDLE, name: &[u16], code: EVENT_TRACE_CONTROL) -> WIN32_ERROR {
    let mut p = Properties::new();
    // SAFETY: `name` is NUL-terminated and `p` is a writable properties block of the size it
    // declares, with room for the name at LoggerNameOffset.
    unsafe { ControlTraceW(session, PCWSTR(name.as_ptr()), &mut p.props, code) }
}

/// Stops the session called `name`; the error code when there is none (or it can't be stopped).
pub(crate) fn stop_by_name(name: &str) -> WIN32_ERROR {
    control(CONTROLTRACE_HANDLE::default(), &wide(name), EVENT_TRACE_CONTROL_STOP)
}

/// Whether a session called `name` is running.
#[cfg(test)]
fn is_running(name: &str) -> bool {
    use windows::Win32::System::Diagnostics::Etw::EVENT_TRACE_CONTROL_QUERY;
    control(CONTROLTRACE_HANDLE::default(), &wide(name), EVENT_TRACE_CONTROL_QUERY) == ERROR_SUCCESS
}

/// The event callback. `UserContext` is the feed's `Sums`, kept alive by the consumer thread for
/// as long as `ProcessTrace` can call this. Never panics (it runs on an `extern "system"` frame).
unsafe extern "system" fn on_event(record: *mut EVENT_RECORD) {
    // SAFETY: ETW passes a valid record for the duration of the call.
    let Some(record) = (unsafe { record.as_ref() }) else { return };
    if record.EventHeader.ProviderId != PROVIDER || record.UserContext.is_null() || record.UserData.is_null() {
        return;
    }
    // SAFETY: UserData points to UserDataLength bytes of payload for the duration of the call.
    let payload = unsafe { std::slice::from_raw_parts(record.UserData as *const u8, record.UserDataLength as usize) };
    if let Some((pid, direction, bytes)) = parse(record.EventHeader.EventDescriptor.Id, payload) {
        // SAFETY: UserContext is `Arc::as_ptr` of the Sums the consumer thread holds (see start).
        let sums = unsafe { &*(record.UserContext as *const Sums) };
        sums.add(pid, direction, bytes);
    }
}

/// A running session plus its consumer thread (`negotium-netetw`). Dropping it stops the session
/// and joins the thread.
pub struct NetFeed {
    session: CONTROLTRACE_HANDLE,
    consumer: PROCESSTRACE_HANDLE,
    name: Vec<u16>,
    sums: Arc<Sums>,
    thread: Option<JoinHandle<()>>,
}

impl NetFeed {
    /// Stops any session already called `name` (one leaked by a crash, or a previous worker's),
    /// then starts a fresh one and its consumer.
    pub fn start(name: &str) -> Result<NetFeed, NetFeedError> {
        let mut wname = wide(name);
        let _ = stop_by_name(name);

        let mut p = Properties::new();
        p.props.Wnode.Flags = WNODE_FLAG_TRACED_GUID;
        p.props.Wnode.ClientContext = 1; // QueryPerformanceCounter timestamps
        p.props.LogFileMode = EVENT_TRACE_REAL_TIME_MODE;
        p.props.BufferSize = 64; // KB
        p.props.MinimumBuffers = 4;
        p.props.MaximumBuffers = 32;
        p.props.FlushTimer = 1; // seconds: events reach the consumer within a tick
        let mut session = CONTROLTRACE_HANDLE::default();
        // SAFETY: `wname` is NUL-terminated; `p` is a properties block of the size it declares
        // with room for the name, which StartTraceW copies there.
        let status = unsafe { StartTraceW(&mut session, PCWSTR(wname.as_ptr()), &mut p.props) };
        if status == ERROR_ACCESS_DENIED {
            return Err(NetFeedError::NotElevated);
        }
        if status != ERROR_SUCCESS {
            return Err(NetFeedError::Os { call: "StartTraceW", code: status.0 });
        }
        // Unelevated, StartTraceW may still succeed (a plain real-time session is allowed); it is
        // enabling the kernel provider that is denied. Either way the session is stopped again.
        let fail = |call: &'static str, code: u32| {
            let _ = control(session, &wide(name), EVENT_TRACE_CONTROL_STOP);
            Err(if code == ERROR_ACCESS_DENIED.0 { NetFeedError::NotElevated } else { NetFeedError::Os { call, code } })
        };

        // SAFETY: `session` was just started; PROVIDER is a valid GUID for the call's duration.
        let status = unsafe {
            EnableTraceEx2(
                session,
                &PROVIDER,
                EVENT_CONTROL_CODE_ENABLE_PROVIDER.0,
                TRACE_LEVEL_INFORMATION as u8,
                KEYWORD_IPV4 | KEYWORD_IPV6,
                0,
                0,
                None,
            )
        };
        if status != ERROR_SUCCESS {
            return fail("EnableTraceEx2", status.0);
        }

        let sums = Arc::new(Sums::default());
        let mut logfile = EVENT_TRACE_LOGFILEW {
            LoggerName: PWSTR(wname.as_mut_ptr()),
            Context: Arc::as_ptr(&sums) as *mut c_void,
            ..Default::default()
        };
        logfile.Anonymous1.ProcessTraceMode = PROCESS_TRACE_MODE_REAL_TIME | PROCESS_TRACE_MODE_EVENT_RECORD;
        logfile.Anonymous2.EventRecordCallback = Some(on_event);
        // SAFETY: `logfile` names a running real-time session and an EVENT_RECORD callback; it is
        // only read during the call. Context stays valid: the consumer thread owns a clone of `sums`.
        let consumer = unsafe { OpenTraceW(&mut logfile) };
        if consumer.Value == INVALID_PROCESSTRACE {
            // SAFETY: reads this thread's last error, set by OpenTraceW.
            return fail("OpenTraceW", unsafe { GetLastError() }.0);
        }

        let held = sums.clone();
        let thread = std::thread::Builder::new().name("negotium-netetw".into()).spawn(move || {
            // SAFETY: `consumer` is an open real-time trace; ProcessTrace blocks, calling on_event,
            // until the session stops or the handle is closed. `held` outlives every callback.
            let _ = unsafe { ProcessTrace(&[consumer], None, None) };
            drop(held);
        });
        match thread {
            Ok(thread) => Ok(NetFeed { session, consumer, name: wname, sums, thread: Some(thread) }),
            Err(_) => {
                // SAFETY: opened above and never processed.
                let _ = unsafe { CloseTrace(consumer) };
                fail("CreateThread", 0)
            }
        }
    }

    /// Bytes per PID since the last take.
    pub fn take(&self) -> HashMap<u32, NetBytes> {
        self.sums.take()
    }
}

impl TakeBytes for NetFeed {
    fn take(&self) -> HashMap<u32, NetBytes> {
        NetFeed::take(self)
    }

    fn running(&self) -> bool {
        self.thread.as_ref().is_some_and(|t| !t.is_finished())
    }
}

impl Drop for NetFeed {
    fn drop(&mut self) {
        // ProcessTrace returns once the session stops, so a finished consumer means it is already
        // gone. Its handle (a logger id) may then belong to a newer session: leave it alone.
        if TakeBytes::running(self) {
            let _ = control(self.session, &self.name, EVENT_TRACE_CONTROL_STOP);
        }
        // SAFETY: the consumer handle was opened in start and is closed exactly once, here.
        let _ = unsafe { CloseTrace(self.consumer) };
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Whether this process runs with an elevated token. The same check as `negotium-actions`, kept
/// here because the actions crate depends on this one.
pub fn is_elevated() -> bool {
    let mut token = HANDLE::default();
    // SAFETY: opens our own token for query; closed below.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.is_err() {
        return false;
    }
    let mut elevation = TOKEN_ELEVATION::default();
    let mut len = 0u32;
    // SAFETY: `elevation` is a writable TOKEN_ELEVATION of the size passed.
    let ok = unsafe {
        GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut c_void),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
    }
    .is_ok();
    // SAFETY: the token handle was opened above and is closed exactly once.
    let _ = unsafe { CloseHandle(token) };
    ok && elevation.TokenIsElevated != 0
}

/// The collector's side: the feed (only when elevated), the time of the last take, and a start
/// failure waiting to be reported once.
pub(crate) struct PerPidNet<F> {
    feed: Option<F>,
    error: Option<String>,
    last: Instant,
}

impl<F: TakeBytes> PerPidNet<F> {
    /// Starts the feed only when `elevated`; unelevated there is nothing to report (the Overview
    /// banner explains the missing column).
    pub(crate) fn new(elevated: bool, start: impl FnOnce() -> Result<F, NetFeedError>, now: Instant) -> Self {
        let (feed, error) = if !elevated {
            (None, None)
        } else {
            match start() {
                Ok(feed) => (Some(feed), None),
                Err(e) => (None, Some(format!("per-app network: {e}"))),
            }
        };
        PerPidNet { feed, error, last: now }
    }

    /// Rates since the previous step for the processes in this tick's list. `None` without a feed.
    pub(crate) fn step(
        &mut self, now: Instant, processes: &[ProcessSample], errors: &mut Vec<String>,
    ) -> Option<BTreeMap<u32, NetRate>> {
        errors.extend(self.error.take());
        if self.feed.as_ref().is_some_and(|f| !f.running()) {
            self.feed = None;
            errors.push("per-app network: the network trace stopped".into());
        }
        let feed = self.feed.as_ref()?;
        let bytes = feed.take();
        let secs = now.saturating_duration_since(self.last).as_secs_f64().max(1e-3);
        self.last = now;
        if bytes.is_empty() {
            return Some(BTreeMap::new());
        }
        // A PID that exited (or whose number was reused) is credited only to the process alive
        // now; one that lived less than a tick is never listed and its bytes are dropped.
        let alive: HashSet<u32> = processes.iter().map(|p| p.pid).collect();
        Some(
            bytes
                .into_iter()
                .filter(|(pid, _)| alive.contains(pid))
                .map(|(pid, b)| (pid, NetRate { sent_bps: b.sent as f64 / secs, recv_bps: b.recv as f64 / secs }))
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::time::Duration;

    fn v4(pid: u32, size: u32, daddr: [u8; 4], saddr: [u8; 4]) -> Vec<u8> {
        let mut p = Vec::new();
        p.extend(pid.to_le_bytes());
        p.extend(size.to_le_bytes());
        p.extend(daddr);
        p.extend(saddr);
        p.extend([0x01, 0xbb, 0xc3, 0x50]); // dport 443, sport 50000
        p.extend([0u8; 8]); // seqnum, connid
        p
    }

    fn v6(pid: u32, size: u32, daddr: Ipv6Addr, saddr: Ipv6Addr) -> Vec<u8> {
        let mut p = Vec::new();
        p.extend(pid.to_le_bytes());
        p.extend(size.to_le_bytes());
        p.extend(daddr.octets());
        p.extend(saddr.octets());
        p.extend([0x01, 0xbb, 0xc3, 0x50]);
        p.extend([0u8; 8]);
        p
    }

    const REMOTE: [u8; 4] = [93, 184, 216, 34];
    const LOCAL: [u8; 4] = [192, 168, 1, 20];

    fn remote6() -> Ipv6Addr {
        "2606:2800:220:1::1".parse().unwrap()
    }

    fn local6() -> Ipv6Addr {
        "fd00::20".parse().unwrap()
    }

    #[test]
    fn parses_tcp_and_udp_send_and_receive_for_v4_and_v6() {
        use Direction::{Recv, Sent};
        let cases = [
            (10, v4(100, 1500, REMOTE, LOCAL), (100, Sent, 1500)),
            (11, v4(101, 1400, LOCAL, REMOTE), (101, Recv, 1400)),
            (42, v4(102, 512, REMOTE, LOCAL), (102, Sent, 512)),
            (43, v4(103, 513, LOCAL, REMOTE), (103, Recv, 513)),
            (26, v6(104, 9000, remote6(), local6()), (104, Sent, 9000)),
            (27, v6(105, 9001, local6(), remote6()), (105, Recv, 9001)),
            (58, v6(106, 77, remote6(), local6()), (106, Sent, 77)),
            (59, v6(107, 78, local6(), remote6()), (107, Recv, 78)),
        ];
        for (id, payload, want) in cases {
            assert_eq!(parse(id, &payload), Some(want), "event {id}");
        }
        // The TCP sends carry two more fields; the layout up to the addresses is the same.
        let mut long = v4(108, 64, REMOTE, LOCAL);
        long.extend([0u8; 8]);
        assert_eq!(parse(10, &long), Some((108, Sent, 64)));
    }

    #[test]
    fn loopback_is_not_counted() {
        for addr in [[127, 0, 0, 1], [127, 5, 5, 5]] {
            assert_eq!(parse(10, &v4(1, 10, addr, LOCAL)), None, "{addr:?} as destination");
            assert_eq!(parse(11, &v4(1, 10, LOCAL, addr)), None, "{addr:?} as source");
            assert_eq!(parse(42, &v4(1, 10, addr, addr)), None);
        }
        for addr in [Ipv6Addr::LOCALHOST, "::ffff:127.0.0.1".parse().unwrap()] {
            assert_eq!(parse(26, &v6(1, 10, addr, local6())), None, "{addr} as destination");
            assert_eq!(parse(59, &v6(1, 10, local6(), addr)), None, "{addr} as source");
        }
        // A mapped non-loopback address is ordinary traffic.
        let mapped: Ipv6Addr = "::ffff:93.184.216.34".parse().unwrap();
        assert!(parse(26, &v6(1, 10, mapped, local6())).is_some());
    }

    #[test]
    fn short_or_unknown_events_are_ignored() {
        let p4 = v4(1, 10, REMOTE, LOCAL);
        let p6 = v6(1, 10, remote6(), local6());
        assert_eq!(parse(10, &p4[..15]), None, "v4 cut inside saddr");
        assert_eq!(parse(43, &p4[..16]).map(|e| e.0), Some(1), "v4 through saddr is enough");
        assert_eq!(parse(26, &p6[..39]), None, "v6 cut inside saddr");
        assert_eq!(parse(26, &p4), None, "a v4 payload under a v6 id");
        assert_eq!(parse(11, &[]), None);
        // Connect, disconnect, retransmit, accept, reconnect, fail, copy, UDP fail; unknown ids.
        for id in [12, 13, 14, 15, 16, 17, 18, 28, 29, 30, 31, 32, 34, 49, 0, 9, 44, 60, 1000] {
            assert_eq!(parse(id, &p6), None, "event {id}");
            assert_eq!(parse(id, &p4), None, "event {id}");
        }
    }

    #[test]
    fn take_returns_bytes_since_the_last_take_and_resets() {
        let sums = Sums::default();
        sums.add(7, Direction::Sent, 100);
        sums.add(7, Direction::Recv, 40);
        sums.add(7, Direction::Sent, 1);
        sums.add(9, Direction::Recv, 5);
        let first = sums.take();
        assert_eq!(first.len(), 2);
        assert_eq!(first[&7], NetBytes { sent: 101, recv: 40 });
        assert_eq!(first[&9], NetBytes { sent: 0, recv: 5 });
        assert!(sums.take().is_empty(), "nothing since the last take");
        sums.add(9, Direction::Sent, 3);
        assert_eq!(sums.take(), HashMap::from([(9, NetBytes { sent: 3, recv: 0 })]));
    }

    #[test]
    fn accumulating_events_does_not_allocate_per_event() {
        let sums = Sums::default();
        for pid in 1..=3 {
            sums.add(pid, Direction::Sent, 1);
        }
        let cap = sums.capacity();
        assert!(cap >= 3);
        for i in 0..10_000u32 {
            sums.add(1 + i % 3, if i % 2 == 0 { Direction::Sent } else { Direction::Recv }, 1500);
        }
        assert_eq!(sums.capacity(), cap, "known PIDs never grow the map");
        let _ = sums.take();
        for i in 0..10_000u32 {
            sums.add(1 + i % 3, Direction::Recv, 1500);
        }
        assert_eq!(sums.capacity(), cap, "a take keeps the PIDs that had traffic");
        assert_eq!(sums.take()[&1].recv, 3334 * 1500);
    }

    struct Fake(RefCell<Vec<HashMap<u32, NetBytes>>>, std::cell::Cell<bool>);

    impl Fake {
        fn new(takes: Vec<HashMap<u32, NetBytes>>) -> Self {
            Fake(RefCell::new(takes), std::cell::Cell::new(true))
        }
    }

    impl TakeBytes for Fake {
        fn take(&self) -> HashMap<u32, NetBytes> {
            self.0.borrow_mut().pop().unwrap_or_default()
        }

        fn running(&self) -> bool {
            self.1.get()
        }
    }

    fn proc(pid: u32) -> ProcessSample {
        ProcessSample {
            pid, parent_pid: 0, create_time: 1, name: "p".into(), session_id: 1, cpu_time_100ns: 0,
            private_working_set: 0, disk_io_bytes: 0, threads: 1, handles: 0, image_path: None,
            package_family: None, description: None,
        }
    }

    #[test]
    fn bytes_follow_the_pid_alive_at_take_time() {
        let t0 = Instant::now();
        let ticks = vec![HashMap::from([
            (10, NetBytes { sent: 1000, recv: 3000 }),
            (11, NetBytes { sent: 500, recv: 0 }), // exited before the take
        ])];
        let mut net = PerPidNet::new(true, || Ok(Fake::new(ticks)), t0);
        let mut errors = Vec::new();
        let rates = net.step(t0 + Duration::from_millis(500), &[proc(10), proc(12)], &mut errors).unwrap();
        assert_eq!(rates.keys().copied().collect::<Vec<_>>(), [10], "only live PIDs with traffic");
        assert_eq!(rates[&10], NetRate { sent_bps: 2000.0, recv_bps: 6000.0 }, "bytes over 0.5 s");
        assert!(errors.is_empty());
        let rates = net.step(t0 + Duration::from_millis(1500), &[proc(10)], &mut errors).unwrap();
        assert!(rates.is_empty(), "an idle feed gives an empty map, not None");
    }

    #[test]
    fn the_start_failure_is_reported_once() {
        let t0 = Instant::now();
        let mut net: PerPidNet<Fake> =
            PerPidNet::new(true, || Err(NetFeedError::Os { call: "StartTraceW", code: 1450 }), t0);
        let mut errors = Vec::new();
        for i in 1..=4 {
            assert_eq!(net.step(t0 + Duration::from_secs(i), &[proc(1)], &mut errors), None);
        }
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("couldn't start the network trace (error 1450)"), "{errors:?}");

        let mut unelevated: PerPidNet<Fake> = PerPidNet::new(false, || panic!("never started unelevated"), t0);
        let mut errors = Vec::new();
        assert_eq!(unelevated.step(t0 + Duration::from_secs(1), &[proc(1)], &mut errors), None);
        assert!(errors.is_empty(), "unelevated is not an error");
    }

    #[test]
    fn a_feed_whose_session_ended_is_reported_once_and_dropped() {
        let t0 = Instant::now();
        let mut net = PerPidNet::new(true, || Ok(Fake::new(Vec::new())), t0);
        let mut errors = Vec::new();
        assert_eq!(net.step(t0 + Duration::from_secs(1), &[proc(1)], &mut errors), Some(BTreeMap::new()));
        net.feed.as_ref().unwrap().1.set(false);
        for i in 2..=4 {
            assert_eq!(net.step(t0 + Duration::from_secs(i), &[proc(1)], &mut errors), None, "unknown, not zero");
        }
        assert_eq!(errors, ["per-app network: the network trace stopped"]);
    }

    #[test]
    fn errors_read_in_plain_english() {
        assert_eq!(NetFeedError::NotElevated.to_string(), "needs admin rights");
        assert_eq!(
            NetFeedError::Os { call: "StartTraceW", code: 5 }.to_string(),
            "couldn't start the network trace (error 5)"
        );
    }

    /// A name no other test or the app uses, so parallel tests and a running Negotium don't clash.
    fn test_name() -> String {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        format!("Negotium-Net-test-{}-{n}", std::process::id())
    }

    /// Stops the named session when the test ends, pass or fail.
    struct StopAtEnd(String);

    impl Drop for StopAtEnd {
        fn drop(&mut self) {
            let _ = stop_by_name(&self.0);
        }
    }

    #[test]
    fn unelevated_the_trace_is_refused_as_needing_admin() {
        let name = test_name();
        let _guard = StopAtEnd(name.clone());
        match NetFeed::start(&name) {
            Err(e) if !is_elevated() => assert_eq!(e, NetFeedError::NotElevated),
            Ok(feed) if is_elevated() => drop(feed),
            other => panic!("elevated: {}, start: {:?}", is_elevated(), other.err()),
        }
        assert!(!is_running(&name));
    }

    #[test]
    #[ignore = "needs an elevated terminal"]
    fn a_session_starts_and_stops() {
        let name = test_name();
        let _guard = StopAtEnd(name.clone());
        let feed = NetFeed::start(&name).expect("start");
        assert!(is_running(&name));
        std::thread::sleep(Duration::from_secs(1));
        let _ = feed.take();
        drop(feed);
        assert!(!is_running(&name), "dropping the feed stops the session");
    }

    #[test]
    #[ignore = "needs an elevated terminal"]
    fn a_leftover_session_is_replaced() {
        let name = test_name();
        let _guard = StopAtEnd(name.clone());
        std::mem::forget(NetFeed::start(&name).expect("first start"));
        assert!(is_running(&name), "the forgotten feed's session is left running");
        let feed = NetFeed::start(&name).expect("a leftover session of the same name is replaced");
        assert!(is_running(&name));
        drop(feed);
        assert!(!is_running(&name));
    }
}
