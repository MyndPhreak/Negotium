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
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, ERROR_SUCCESS, GetLastError, HANDLE, WIN32_ERROR};
use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows::Win32::System::Diagnostics::Etw::{
    CONTROLTRACE_HANDLE, CloseTrace, ControlTraceW, EVENT_CONTROL_CODE_ENABLE_PROVIDER, EVENT_RECORD,
    EVENT_TRACE_CONTROL, EVENT_TRACE_CONTROL_QUERY, EVENT_TRACE_CONTROL_STOP, EVENT_TRACE_LOGFILEW,
    EVENT_TRACE_PROPERTIES, EVENT_TRACE_REAL_TIME_MODE, EnableTraceEx2, OpenTraceW, PROCESS_TRACE_MODE_EVENT_RECORD,
    PROCESS_TRACE_MODE_REAL_TIME, PROCESSTRACE_HANDLE, ProcessTrace, StartTraceW, TRACE_LEVEL_INFORMATION,
    WNODE_FLAG_TRACED_GUID,
};
use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows::Win32::System::Threading::{GetCurrentProcess, GetCurrentProcessId, OpenProcessToken};
use windows::core::{GUID, PCWSTR, PWSTR};

/// The start of the app's session name; see `session_name`.
const SESSION_PREFIX: &str = "Negotium-Net";

/// The app's session name: `Negotium-Net-<logon session id>`. Fixed per logon session, so a
/// session leaked by a crash is found and replaced at the next start. Trace sessions are
/// machine-wide while the single-instance lock is per logon session: with the id in the name,
/// another user's elevated Negotium (fast user switching, Remote Desktop) never stops ours.
pub fn session_name() -> String {
    let mut id = 0u32;
    // SAFETY: `id` is a writable u32; our own process id is always valid.
    match unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut id) } {
        Ok(()) => session_name_for(id),
        // Never seen for the current process; the bare prefix still names one session per machine.
        Err(_) => SESSION_PREFIX.to_string(),
    }
}

/// `session_name` for logon session `id`.
pub(crate) fn session_name_for(id: u32) -> String {
    format!("{SESSION_PREFIX}-{id}")
}

/// What a step reports when the feed's session ended under it. The app leaves it out of the log
/// while it is exiting: then its own exit stopped the session.
pub const TRACE_STOPPED: &str = "per-app network: the network trace stopped";
/// What a step reports when a retry brought the feed back.
pub const TRACE_RESTARTED: &str = "per-app network: the network trace restarted";
/// How long the collector waits before trying again to start a feed that stopped or failed.
pub(crate) const RETRY: Duration = Duration::from_secs(60);

const PROVIDER: GUID = GUID::from_u128(0x7dd42a49_5329_4832_8dfd_43d979153a88);
const KEYWORD_IPV4: u64 = 0x10;
const KEYWORD_IPV6: u64 = 0x20;
/// evntrace.h `EVENT_TRACE_USE_MS_FLUSH_TIMER` (Windows 8+; not in the windows crate): FlushTimer
/// counts milliseconds instead of seconds.
const EVENT_TRACE_USE_MS_FLUSH_TIMER: u32 = 0x10;
/// How often a partly filled buffer reaches the consumer. Below the shortest tick (500 ms), so
/// light traffic is credited to the tick it happened in instead of alternating 0 / double.
const FLUSH_MS: u32 = 200;
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

/// `parse` for the manifest's layouts, which are all version 0: a newer version of an event may
/// lay its payload out differently, so it is not read.
pub(crate) fn parse_versioned(event_id: u16, version: u8, payload: &[u8]) -> Option<(u32, Direction, u32)> {
    if version != 0 {
        return None;
    }
    parse(event_id, payload)
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
    /// The consumer thread could not be created (and Windows gave no error code).
    Thread,
}

impl std::fmt::Display for NetFeedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NetFeedError::NotElevated => write!(f, "needs admin rights"),
            NetFeedError::Os { code, .. } => write!(f, "couldn't start the network trace (error {code})"),
            NetFeedError::Thread => write!(f, "couldn't start the network trace's reader thread"),
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

/// Stops the trace session called `name`, if there is one. The app's exit path calls this with
/// `session_name()`: the worker's collector is not dropped when the process ends, so its feed's
/// `Drop` never runs. Harmless when no such session exists or it can't be stopped.
pub fn stop_session(name: &str) {
    let _ = stop_by_name(name);
}

/// The running session called `name`, as `ControlTraceW(QUERY)` reports it.
fn query(name: &str) -> Option<Box<Properties>> {
    let mut p = Properties::new();
    let wname = wide(name);
    // SAFETY: as in `control`.
    let status = unsafe {
        ControlTraceW(CONTROLTRACE_HANDLE::default(), PCWSTR(wname.as_ptr()), &mut p.props, EVENT_TRACE_CONTROL_QUERY)
    };
    (status == ERROR_SUCCESS).then_some(p)
}

/// Stops the session called `name` only if it is the one started with `guid`, so a newer session
/// of the same name (a later feed, or another process's) is left alone.
fn stop_if_ours(name: &str, guid: GUID) {
    if query(name).is_some_and(|p| p.props.Wnode.Guid == guid) {
        let _ = stop_by_name(name);
    }
}

/// Starts a real-time session called `name` with session GUID `guid` (no providers yet).
fn start_session(name: &str, guid: GUID) -> Result<CONTROLTRACE_HANDLE, WIN32_ERROR> {
    let wname = wide(name);
    let mut p = Properties::new();
    p.props.Wnode.Flags = WNODE_FLAG_TRACED_GUID;
    p.props.Wnode.ClientContext = 1; // QueryPerformanceCounter timestamps
    p.props.Wnode.Guid = guid;
    p.props.LogFileMode = EVENT_TRACE_REAL_TIME_MODE | EVENT_TRACE_USE_MS_FLUSH_TIMER;
    p.props.BufferSize = 64; // KB
    p.props.MinimumBuffers = 4;
    p.props.MaximumBuffers = 32;
    p.props.FlushTimer = FLUSH_MS;
    let mut session = CONTROLTRACE_HANDLE::default();
    // SAFETY: `wname` is NUL-terminated; `p` is a properties block of the size it declares
    // with room for the name, which StartTraceW copies there.
    let status = unsafe { StartTraceW(&mut session, PCWSTR(wname.as_ptr()), &mut p.props) };
    if status == ERROR_SUCCESS { Ok(session) } else { Err(status) }
}

/// Whether a session called `name` is running.
#[cfg(test)]
fn is_running(name: &str) -> bool {
    query(name).is_some()
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
    let event = &record.EventHeader.EventDescriptor;
    if let Some((pid, direction, bytes)) = parse_versioned(event.Id, event.Version, payload) {
        // SAFETY: UserContext is `Arc::as_ptr` of the Sums the consumer thread holds (see start).
        let sums = unsafe { &*(record.UserContext as *const Sums) };
        sums.add(pid, direction, bytes);
    }
}

/// A running session plus its consumer thread (`negotium-netetw`). Dropping it stops the session
/// and joins the thread.
pub struct NetFeed {
    consumer: PROCESSTRACE_HANDLE,
    name: String,
    /// The session GUID this feed started with: tells its session from a newer one of the name.
    guid: GUID,
    sums: Arc<Sums>,
    thread: Option<JoinHandle<()>>,
}

impl NetFeed {
    /// Stops any session already called `name` (one leaked by a crash, or a previous worker's),
    /// then starts a fresh one and its consumer.
    pub fn start(name: &str) -> Result<NetFeed, NetFeedError> {
        let mut wname = wide(name);
        let _ = stop_by_name(name);

        let guid = GUID::new().map_err(|e| NetFeedError::Os { call: "UuidCreate", code: e.code().0 as u32 })?;
        let session = match start_session(name, guid) {
            Ok(session) => session,
            Err(status) if status == ERROR_ACCESS_DENIED => return Err(NetFeedError::NotElevated),
            Err(status) => return Err(NetFeedError::Os { call: "StartTraceW", code: status.0 }),
        };
        // Unelevated, StartTraceW may still succeed (a plain real-time session is allowed); it is
        // enabling the kernel provider that is denied. Either way the session is stopped again.
        let stop = || {
            let _ = control(session, &wide(name), EVENT_TRACE_CONTROL_STOP);
        };
        let fail = |call: &'static str, code: u32| {
            stop();
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
            Ok(thread) => Ok(NetFeed { consumer, name: name.to_string(), guid, sums, thread: Some(thread) }),
            Err(e) => {
                // SAFETY: opened above and never processed.
                let _ = unsafe { CloseTrace(consumer) };
                match e.raw_os_error() {
                    Some(code) => fail("CreateThread", code as u32),
                    None => {
                        stop();
                        Err(NetFeedError::Thread)
                    }
                }
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
        // By name and only if the session is still this one, whether or not the consumer is still
        // attached: once the session was stopped (and maybe replaced by a newer one of the name,
        // which may even reuse the old handle's logger id) a stop by handle could end the newer
        // one. If ProcessTrace failed on its own, the session still runs and is ours: stopped too.
        stop_if_ours(&self.name, self.guid);
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

/// Starts a feed; kept to try again after the feed stopped or failed to start.
type Starter<F> = Box<dyn FnMut() -> Result<F, NetFeedError> + Send>;

/// The collector's side: the feed (only when elevated), the time of the last take, a start
/// failure waiting to be reported once, and when to try starting again (`RETRY` after a stop or
/// a failed start).
pub(crate) struct PerPidNet<F> {
    feed: Option<F>,
    error: Option<String>,
    last: Instant,
    /// `None` when the feed is never to run (unelevated, or `off`).
    start: Option<Starter<F>>,
    retry_at: Option<Instant>,
}

impl<F: TakeBytes> PerPidNet<F> {
    /// Starts the feed only when `elevated`; unelevated there is nothing to report (the Overview
    /// banner explains the missing column) and nothing is ever retried.
    pub(crate) fn new(
        elevated: bool, start: impl FnMut() -> Result<F, NetFeedError> + Send + 'static, now: Instant,
    ) -> Self {
        if !elevated {
            return Self::off(now);
        }
        let mut start: Starter<F> = Box::new(start);
        let (feed, error, retry_at) = match start() {
            Ok(feed) => (Some(feed), None, None),
            Err(e) => (None, Some(format!("per-app network: {e}")), Some(now + RETRY)),
        };
        PerPidNet { feed, error, last: now, start: Some(start), retry_at }
    }

    /// No feed: every step gives `None` and reports nothing.
    pub(crate) fn off(now: Instant) -> Self {
        PerPidNet { feed: None, error: None, last: now, start: None, retry_at: None }
    }

    pub(crate) fn is_on(&self) -> bool {
        self.feed.is_some()
    }

    /// Rates since the previous step for the processes in this tick's list. `None` without a feed.
    pub(crate) fn step(
        &mut self, now: Instant, processes: &[ProcessSample], errors: &mut Vec<String>,
    ) -> Option<BTreeMap<u32, NetRate>> {
        errors.extend(self.error.take());
        if self.feed.as_ref().is_some_and(|f| !f.running()) {
            self.feed = None;
            self.retry_at = Some(now + RETRY);
            errors.push(TRACE_STOPPED.into());
        }
        if self.feed.is_none() && self.retry_at.is_some_and(|at| now >= at) {
            let start = self.start.as_mut()?;
            // A failed retry stays quiet (the stop or the first failure was reported) and waits
            // another RETRY; a success is reported once.
            match start() {
                Ok(feed) => {
                    self.feed = Some(feed);
                    self.retry_at = None;
                    self.last = now;
                    errors.push(TRACE_RESTARTED.into());
                    // Nothing counted yet: a take now would divide a few bytes by almost no time.
                    return Some(BTreeMap::new());
                }
                Err(_) => self.retry_at = Some(now + RETRY),
            }
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
    use std::sync::atomic::{AtomicU32, Ordering};

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
    fn only_version_0_layouts_are_read() {
        let p = v4(5, 10, REMOTE, LOCAL);
        assert_eq!(parse_versioned(10, 0, &p), Some((5, Direction::Sent, 10)));
        assert_eq!(parse_versioned(10, 1, &p), None);
        assert_eq!(parse_versioned(27, 2, &v6(5, 10, local6(), remote6())), None);
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
            package_family: None, description: None, publisher: None,
        }
    }

    #[test]
    fn bytes_follow_the_pid_alive_at_take_time() {
        let t0 = Instant::now();
        let ticks = vec![HashMap::from([
            (10, NetBytes { sent: 1000, recv: 3000 }),
            (11, NetBytes { sent: 500, recv: 0 }), // exited before the take
        ])];
        let mut net = PerPidNet::new(true, move || Ok(Fake::new(ticks.clone())), t0);
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
        assert_eq!(unelevated.step(t0 + Duration::from_secs(3600), &[proc(1)], &mut errors), None, "nor retried");
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
        assert_eq!(errors, [TRACE_STOPPED]);
    }

    /// A starter whose calls succeed or fail in `results` order (then fail), counting its calls.
    fn starter(results: Vec<bool>, calls: Arc<AtomicU32>) -> impl FnMut() -> Result<Fake, NetFeedError> + Send + 'static {
        let mut results = results.into_iter();
        move || {
            calls.fetch_add(1, Ordering::SeqCst);
            match results.next() {
                Some(true) => Ok(Fake::new(Vec::new())),
                _ => Err(NetFeedError::Os { call: "StartTraceW", code: 1450 }),
            }
        }
    }

    #[test]
    fn a_stopped_feed_is_retried_each_minute_and_its_recovery_reported_once() {
        let t0 = Instant::now();
        let s = |secs| t0 + Duration::from_secs(secs);
        let calls = Arc::new(AtomicU32::new(0));
        let mut net = PerPidNet::new(true, starter(vec![true, false, true], calls.clone()), t0);
        let mut errors = Vec::new();
        assert!(net.step(s(1), &[proc(1)], &mut errors).is_some());
        net.feed.as_ref().unwrap().1.set(false);
        assert_eq!(net.step(s(2), &[proc(1)], &mut errors), None, "stopped: unknown, not zero");
        assert_eq!(net.step(s(61), &[proc(1)], &mut errors), None);
        assert_eq!(calls.load(Ordering::SeqCst), 1, "no retry before a minute has passed");
        assert_eq!(net.step(s(62), &[proc(1)], &mut errors), None, "the first retry fails");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(net.step(s(121), &[proc(1)], &mut errors), None);
        assert_eq!(calls.load(Ordering::SeqCst), 2, "a failed retry waits another minute");
        assert_eq!(net.step(s(122), &[proc(1)], &mut errors), Some(BTreeMap::new()), "back: an empty first tick");
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert!(net.step(s(123), &[proc(1)], &mut errors).is_some());
        assert_eq!(errors, ["per-app network: the network trace stopped", "per-app network: the network trace restarted"]);
    }

    #[test]
    fn a_failed_start_is_retried_quietly() {
        let t0 = Instant::now();
        let s = |secs| t0 + Duration::from_secs(secs);
        let calls = Arc::new(AtomicU32::new(0));
        let mut net = PerPidNet::new(true, starter(vec![false, false, true], calls.clone()), t0);
        let mut errors = Vec::new();
        assert_eq!(net.step(s(1), &[proc(1)], &mut errors), None);
        assert_eq!(net.step(s(60), &[proc(1)], &mut errors), None);
        assert_eq!(calls.load(Ordering::SeqCst), 2, "retried after a minute");
        assert_eq!(net.step(s(120), &[proc(1)], &mut errors), Some(BTreeMap::new()));
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert_eq!(errors.len(), 2, "the failure once, the recovery once: {errors:?}");
        assert!(errors[0].contains("error 1450"), "{errors:?}");
        assert_eq!(errors[1], "per-app network: the network trace restarted");
    }

    #[test]
    fn the_app_session_is_named_after_this_logon_session() {
        assert_eq!(session_name_for(0), "Negotium-Net-0");
        assert_eq!(session_name_for(3), "Negotium-Net-3");
        // The id the process list reports for this process.
        let pid = std::process::id();
        let ours = crate::ProcessReader::new().read().unwrap().into_iter().find(|p| p.pid == pid).expect("listed");
        assert_eq!(session_name(), session_name_for(ours.session_id));
        assert_ne!(session_name_for(1), session_name_for(2), "another user's session has another name");
    }

    #[test]
    fn errors_read_in_plain_english() {
        assert_eq!(NetFeedError::NotElevated.to_string(), "needs admin rights");
        assert_eq!(NetFeedError::Thread.to_string(), "couldn't start the network trace's reader thread");
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

    /// Starts a bare session (no provider) when Windows lets this token; `None` (test skipped)
    /// where even that needs admin rights.
    fn bare_session(name: &str, guid: GUID) -> Option<CONTROLTRACE_HANDLE> {
        match start_session(name, guid) {
            Ok(h) => Some(h),
            Err(e) if e == ERROR_ACCESS_DENIED && !is_elevated() => {
                eprintln!("skipped: this account may not start a trace session unelevated");
                None
            }
            Err(e) => panic!("StartTraceW: {e:?}"),
        }
    }

    #[test]
    fn the_session_flushes_more_often_than_the_shortest_tick() {
        let name = test_name();
        let _guard = StopAtEnd(name.clone());
        let Some(_) = bare_session(&name, GUID::new().unwrap()) else { return };
        let p = query(&name).expect("running");
        assert_ne!(p.props.LogFileMode & EVENT_TRACE_USE_MS_FLUSH_TIMER, 0, "{:#x}", p.props.LogFileMode);
        assert_eq!(p.props.FlushTimer, FLUSH_MS);
        const { assert!(FLUSH_MS < 500) };
    }

    #[test]
    fn stop_if_ours_leaves_a_newer_session_of_the_name_alone() {
        let name = test_name();
        let _guard = StopAtEnd(name.clone());
        let (old, new) = (GUID::new().unwrap(), GUID::new().unwrap());
        let Some(_) = bare_session(&name, old) else { return };
        assert_eq!(query(&name).unwrap().props.Wnode.Guid, old, "QUERY reports the session GUID");
        stop_session(&name);
        assert!(!is_running(&name));
        bare_session(&name, new).expect("restart");
        stop_if_ours(&name, old);
        assert!(is_running(&name), "the newer session survives the old feed's cleanup");
        stop_if_ours(&name, new);
        assert!(!is_running(&name));
    }

    #[test]
    fn stop_session_stops_a_session_by_name() {
        let name = test_name();
        let _guard = StopAtEnd(name.clone());
        let Some(_) = bare_session(&name, GUID::new().unwrap()) else { return };
        assert!(is_running(&name));
        stop_session(&name);
        assert!(!is_running(&name));
        stop_session(&name); // none left: harmless
    }

    #[test]
    #[ignore = "needs an elevated terminal"]
    fn stop_session_stops_a_running_feed() {
        let name = test_name();
        let _guard = StopAtEnd(name.clone());
        let feed = NetFeed::start(&name).expect("start");
        stop_session(&name);
        assert!(!is_running(&name));
        std::thread::sleep(Duration::from_millis(500));
        assert!(!TakeBytes::running(&feed), "the consumer ends with the session");
        drop(feed); // finished consumer, session gone: nothing to stop, nothing hangs
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
    fn the_live_feed_counts_this_processes_udp_sends() {
        let name = test_name();
        let _guard = StopAtEnd(name.clone());
        let feed = NetFeed::start(&name).expect("start");
        let _ = feed.take();
        // TEST-NET-1 (RFC 5737): routed off the machine, never loopback, nobody answers.
        let socket = std::net::UdpSocket::bind("0.0.0.0:0").expect("bind");
        for _ in 0..5 {
            let _ = socket.send_to(&[0u8; 512], "192.0.2.1:9");
        }
        std::thread::sleep(Duration::from_millis(500)); // more than FLUSH_MS
        let bytes = feed.take();
        let ours = bytes.get(&std::process::id()).copied().unwrap_or_default();
        assert!(ours.sent > 0, "no bytes counted for this process: {bytes:?}");
        drop(feed);
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
