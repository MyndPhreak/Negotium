//! The supervised worker: one thread owns Collector -> Model -> Ranker -> History and emits
//! Frames. A panic anywhere in a tick rebuilds the pipeline after a backoff (spec section 7).
use crate::controls::{Controls, Page};
use crate::perf::{LastSeen, PerfInput, build_perf};
use crate::tray::TraySample;
use crate::ports::{PortIds, PortReader};
use crate::view::{Frame, FrameOptions, IdMap, IdSeed, PortsFrame, TOP_TILES, build_frame};
use negotium_collector::{Collector, SystemInfo};
use negotium_history::History;
use negotium_model::{Model, Ranker, TopPicker};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A socket read longer than this is logged (spec: a refresh stays under 20 ms).
const PORTS_READ_BUDGET: Duration = Duration::from_millis(20);

pub trait Source {
    fn next_frame(&mut self, controls: &Controls) -> Option<Frame>;
}

pub fn backoff(restarts: u32) -> Duration {
    let ms = 500u64.saturating_mul(1u64 << restarts.saturating_sub(1).min(16));
    Duration::from_millis(ms.min(30_000))
}

pub(crate) fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_string())
}

/// Backoff sleep that only ends at the deadline or on stop. Unlike a bare `wait`, UI pokes
/// (metric change, visibility) do not cut it short, so a crash loop cannot be hurried.
fn sleep_unless_stopped(controls: &Controls, d: Duration) {
    let deadline = Instant::now() + d;
    while !controls.stopped() {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        controls.wait(left);
    }
}

pub fn supervise<S: Source, M: Fn() -> S>(
    make: M,
    controls: Arc<Controls>,
    mut sink: impl FnMut(Frame),
    mut log: impl FnMut(&str),
) -> u32 {
    let mut restarts = 0u32;
    while !controls.stopped() {
        let mut source = match catch_unwind(AssertUnwindSafe(&make)) {
            Ok(s) => s,
            Err(p) => {
                restarts += 1;
                log(&format!("worker restarted after panic: {}", panic_text(&*p)));
                sleep_unless_stopped(&controls, backoff(restarts));
                continue;
            }
        };
        loop {
            if controls.stopped() {
                return restarts;
            }
            match catch_unwind(AssertUnwindSafe(|| source.next_frame(&controls))) {
                Ok(Some(frame)) => sink(frame),
                Ok(None) => {}
                Err(p) => {
                    restarts += 1;
                    log(&format!("worker restarted after panic: {}", panic_text(&*p)));
                    sleep_unless_stopped(&controls, backoff(restarts));
                    break;
                }
            }
            controls.wait(controls.interval());
        }
    }
    restarts
}

/// Where each kept history sample's tray graph goes (Settings > Show CPU in tray icon).
pub type TraySink = Arc<dyn Fn(TraySample) + Send + Sync>;

/// The history, shared with the UI thread for the scrub (read under the lock, plain data).
pub type SharedHistory = Arc<Mutex<History>>;

/// Changes the shared history. This is the only place a panic can poison its lock, and a
/// poisoned lock therefore means a write was cut short: `LiveSource::new` starts the history over.
fn write_history<R>(history: &SharedHistory, f: impl FnOnce(&mut History) -> R) -> R {
    f(&mut history.lock().unwrap_or_else(|e| e.into_inner()))
}

/// Reads the shared history for view building. A panic in `f` leaves the history intact, so it
/// goes on (to the supervisor) only after the guard is released: the lock is not poisoned and the
/// restarted worker keeps the history.
fn read_history<R>(history: &SharedHistory, f: impl FnOnce(&History) -> R) -> R {
    let result = {
        let h = history.lock().unwrap_or_else(|e| e.into_inner());
        catch_unwind(AssertUnwindSafe(|| f(&h)))
    };
    result.unwrap_or_else(|p| std::panic::resume_unwind(p))
}

/// Offers the tick's sample to the history, which keeps at most one a second (P3-3). For a kept
/// sample while the tray graph is wanted (`for_tray`), returns the tray's minute up to it, read
/// under the same lock as the push that kept it.
fn keep_sample(history: &SharedHistory, snap: &negotium_snapshot::Snapshot, rows: &[negotium_model::AppRow], for_tray: bool) -> Option<TraySample> {
    write_history(history, |h| {
        let kept = h.push(snap.taken_at_ms, &snap.totals, rows);
        (kept && for_tray).then(|| TraySample::from_snapshot(h, snap))
    })
}

pub struct LiveSource {
    collector: Collector,
    info: SystemInfo,
    model: Model,
    ranker: Ranker,
    top: TopPicker,
    usage: negotium_model::UsageTracker,
    history: SharedHistory,
    ids: IdMap,
    /// The selected Performance device as last seen, for its page's grace period.
    seen: Option<LastSeen>,
    elevated: bool,
    log: crate::log::Log,
    tray: Option<TraySink>,
    moderator: negotium_moderator::Moderator,
    /// The Ports page: the socket tables, read only while that page is on screen.
    ports: PortReader,
    port_ids: PortIds,
}

impl LiveSource {
    /// `history`, `epoch` and `ids` outlive a restart after a panic, so the graphs keep their past
    /// and no row id the UI still holds comes to name another app. A poisoned lock means a write was
    /// cut short (see `write_history`): the possibly half-written history starts over empty.
    pub fn new(history: SharedHistory, epoch: Instant, ids: IdSeed) -> Self {
        if history.is_poisoned() {
            *history.lock().unwrap_or_else(|e| e.into_inner()) = History::new();
            history.clear_poison();
        }
        let collector = Collector::with_epoch(epoch);
        let info = collector.system_info().clone();
        Self {
            collector,
            info,
            model: Model::with_system_root(negotium_collector::windows_dir()),
            ranker: Ranker::new(),
            top: TopPicker::new(),
            usage: negotium_model::UsageTracker::new(),
            history,
            ids: IdMap::with_seed(ids),
            seen: None,
            elevated: negotium_actions::is_elevated(),
            log: crate::log::Log::new(crate::log::Log::default_path()),
            tray: None,
            moderator: negotium_moderator::Moderator::new(),
            ports: PortReader::live(),
            port_ids: PortIds::default(),
        }
    }

    /// Runs the per-process network feed (the app's own source only; see `net_session`).
    pub fn with_net_feed(mut self, name: &str) -> Self {
        self.collector.start_net_feed(name);
        self
    }

    /// Logs source errors to `log` instead of the user's log (tests driving the real pipeline).
    pub fn with_log(mut self, log: crate::log::Log) -> Self {
        self.log = log;
        self
    }

    /// Sends the tray a CPU sample for every history sample kept while Controls::tray_cpu is on,
    /// visible or hidden.
    pub fn with_tray(mut self, sink: TraySink) -> Self {
        self.tray = Some(sink);
        self
    }
}

impl LiveSource {
    /// Carries out the moderator's actions and logs its events. Win32 failures are ignored: the
    /// process may have exited or be out of reach, and the next tick sees whatever took effect.
    fn carry_out(&mut self, actions: Vec<negotium_moderator::ModeratorAction>, events: Vec<negotium_moderator::ModeratorEvent>) {
        use negotium_actions::PriorityClass;
        use negotium_moderator::{ModeratorAction, ModeratorEvent, RestraintMode};
        for action in actions {
            match action {
                ModeratorAction::Restrain { pid, create_time, name: _, mode } => {
                    let original_priority = negotium_actions::get_priority_class(pid, create_time).ok();
                    let original_efficiency = negotium_actions::is_efficiency_mode(pid, create_time).ok();
                    self.moderator.record_original_state(pid, create_time, original_priority, original_efficiency);
                    // A priority that couldn't be read couldn't be put back: leave it alone.
                    let lower_priority = original_priority.is_some();
                    match mode {
                        RestraintMode::Both => {
                            let _ = negotium_actions::set_power_throttling(pid, create_time, true);
                            if lower_priority {
                                let _ = negotium_actions::set_priority_class(pid, create_time, PriorityClass::BelowNormal);
                            }
                        }
                        RestraintMode::EcoQosOnly => {
                            let _ = negotium_actions::set_power_throttling(pid, create_time, true);
                        }
                        RestraintMode::BelowNormalOnly => {
                            if lower_priority {
                                let _ = negotium_actions::set_priority_class(pid, create_time, PriorityClass::BelowNormal);
                            }
                        }
                        RestraintMode::Idle => {
                            let _ = negotium_actions::set_power_throttling(pid, create_time, true);
                            if lower_priority {
                                let _ = negotium_actions::set_priority_class(pid, create_time, PriorityClass::Idle);
                            }
                        }
                    }
                }
                ModeratorAction::Restore { pid, create_time, name: _, reason: _, original_priority, original_efficiency } => {
                    if let Some(prio) = original_priority {
                        let _ = negotium_actions::set_priority_class(pid, create_time, prio);
                    }
                    let _ = negotium_actions::set_power_throttling(pid, create_time, original_efficiency.unwrap_or(false));
                }
                ModeratorAction::ApplyRule { pid, create_time, name: _, priority, affinity_mask, cpu_sets, efficiency_mode } => {
                    // Efficiency mode first: it sets the priority class too (Idle on, Normal off),
                    // and a priority the rule names explicitly must win.
                    if let Some(eff) = efficiency_mode {
                        let _ = negotium_actions::set_efficiency_mode(pid, create_time, eff);
                    }
                    if let Some(prio) = priority {
                        let _ = negotium_actions::set_priority_class(pid, create_time, prio);
                    }
                    if let Some(mask) = affinity_mask {
                        let _ = negotium_actions::set_process_affinity(pid, create_time, mask);
                    }
                    if let Some(sets) = &cpu_sets {
                        let _ = negotium_actions::set_process_cpu_sets(pid, create_time, sets);
                    }
                }
            }
        }

        for event in events {
            match event {
                ModeratorEvent::RestraintStarted { pid, name, cpu_pct, mode: _ } => {
                    self.log.info(&format!("Moderator: Restrained {name} (PID {pid}) using {cpu_pct:.1}% CPU"));
                }
                ModeratorEvent::RestraintRestored { pid, name, reason, duration_ms } => {
                    self.log.info(&format!("Moderator: Restored {name} (PID {pid}) after {duration_ms}ms (reason: {reason:?})"));
                }
                ModeratorEvent::RuleApplied { pid, name, summary } => {
                    self.log.info(&format!("Moderator: Applied rule to {name} (PID {pid}): {summary}"));
                }
            }
        }
    }
}

/// A restraint outlives the worker unless it is undone here: on exit, and on a restart after a
/// panic, whose fresh moderator knows nothing of the old one's restraints.
impl Drop for LiveSource {
    fn drop(&mut self) {
        let (actions, events) = self.moderator.restore_all(negotium_moderator::RestoreReason::Shutdown);
        self.carry_out(actions, events);
    }
}

impl Source for LiveSource {
    fn next_frame(&mut self, controls: &Controls) -> Option<Frame> {
        let snap = self.collector.tick();
        for e in errors_to_log(&snap.source_errors, controls.stopped()) {
            self.log.warn(e);
        }
        let rows = self.model.update(&snap);
        if controls.take_reset_history() {
            self.usage.reset();
        }
        self.usage.update(&snap, &rows);

        // Evaluate Moderator (Aequilibrium & rules)
        let mut proc_rates = std::collections::HashMap::with_capacity(snap.processes.len());
        for app in &rows {
            for p in &app.processes {
                proc_rates.insert((p.pid, p.create_time), p.cpu_pct);
            }
        }
        let foreground_pid = negotium_collector::foreground_process_id();
        let mod_config = controls.moderator_config();
        let (mod_actions, mod_events) = self.moderator.tick(&snap, &proc_rates, foreground_pid, &mod_config);

        self.carry_out(mod_actions, mod_events);
        let tray_sample = keep_sample(&self.history, &snap, &rows, controls.tray_cpu() && self.tray.is_some());
        // Every tick, visible or hidden, so the tray never waits on a stale answer (crate::tray, PF2).
        controls.set_shell_hung(crate::platform::shell_tray_hung());
        if let (Some(sample), Some(tray)) = (tray_sample, &self.tray) {
            tray(sample);
        }
        if !controls.visible() {
            return None;
        }
        // The Details popup shows over any page: its live part rides on whatever frame is built.
        let details = controls.details_watch().map(|watch| {
            self.ports.sockets(Instant::now(), false);
            let (sockets, services, _, _) = self.ports.last();
            let peak = negotium_actions::peak_working_set(watch.selected.0, watch.selected.1).ok();
            crate::details::build_details_frame(&watch, &snap, &rows, sockets, services, peak)
        });
        let frame = self.page_frame(controls, &snap, &rows);
        match (frame, details) {
            (Some(frame), details) => Some(Frame { details, ..frame }),
            (None, Some(details)) => Some(Frame { page: controls.page(), details: Some(details), ..Default::default() }),
            (None, None) => None,
        }
    }
}

impl LiveSource {
    /// The frame for the page on screen; `None` for a page that does not change per tick.
    fn page_frame(&mut self, controls: &Controls, snap: &negotium_snapshot::Snapshot, rows: &[negotium_model::AppRow]) -> Option<Frame> {
        // Only the page on screen is built (spec section 8).
        match controls.page() {
            Page::Overview => {
                let metric = controls.metric();
                let sort = controls.sort();
                let order = self.ranker.order(&rows, sort, snap.taken_at_ms, controls.frozen());
                let top = self.top.pick(&rows, metric, snap.taken_at_ms, TOP_TILES);
                let (expanded, all_children) = controls.child_rows();
                let resorted = self.ranker.resorted();
                let restrained: std::collections::HashSet<(u32, i64)> = snap.processes.iter()
                    .filter(|p| self.moderator.is_restrained(p.pid, p.create_time))
                    .map(|p| (p.pid, p.create_time))
                    .collect();
                let opts = FrameOptions { metric, elevated: self.elevated, expanded, all_children, sort, grouped: controls.grouped(), resorted, restrained };
                let ids = &mut self.ids;
                Some(read_history(&self.history, |h| build_frame(ids, &snap, &rows, &order, &top, h, &opts)))
            }
            Page::Performance => {
                let (device, range) = controls.perf();
                self.seen = LastSeen::track(self.seen.take(), &snap, &device);
                let perf = read_history(&self.history, |h| {
                    build_perf(&PerfInput { snap: &snap, history: h, info: &self.info, apps: &rows, device: &device, range, last_seen: self.seen.as_ref(), elevated: self.elevated })
                });
                Some(Frame { page: Page::Performance, perf: Some(perf), ..Default::default() })
            }
            Page::AppHistory => {
                let (sort_col, sort_desc) = controls.history_sort();
                Some(crate::view::build_history_frame(&self.usage, sort_col, sort_desc))
            }
            // Nothing on these pages changes per tick.
            Page::Startup | Page::Settings | Page::Services => None,
            Page::Ports => {
                let force = controls.take_ports_refresh();
                let error = self.ports.sockets(Instant::now(), force).2;
                let (sockets, services, ending, filters) = self.ports.last();
                let mut owners = crate::ports::owners_from(&snap.processes, services);
                crate::ports::mark_ending(&mut owners, ending, filters);
                let rows = crate::ports::build_port_rows(sockets, &owners, &mut self.port_ids);
                let targets = rows.iter().filter_map(|r| crate::ports::port_target(r, &owners).map(|t| (r.id, t))).collect();
                let all_procs = snap.processes.iter().map(|p| (p.pid, p.create_time, p.parent_pid)).collect();
                if let Some(took) = self.ports.took().filter(|t| *t > PORTS_READ_BUDGET) {
                    self.log.warn(&format!("ports read took {} ms", took.as_millis()));
                }
                if let Some(e) = self.ports.take_services_failure() {
                    self.log.warn(&format!("ports: couldn't list services: {e}"));
                }
                Some(Frame { page: Page::Ports, ports: Some(PortsFrame { rows, targets, all_procs, error }), ..Default::default() })
            }
        }
    }
}

/// A single-value mailbox between the worker and the UI thread. The worker overwrites it each
/// tick; only a put into an empty slot asks for a wakeup, so frames never pile up behind a
/// stalled UI thread: it takes just the newest when it gets to it.
pub struct Latest<T>(Mutex<Option<T>>);

impl<T> Default for Latest<T> {
    fn default() -> Self {
        Self(Mutex::new(None))
    }
}

impl<T> Latest<T> {
    /// Stores `value`, replacing any unread one. True if the slot was empty (post a wakeup).
    pub fn put(&self, value: T) -> bool {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).replace(value).is_none()
    }

    pub fn take(&self) -> Option<T> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).take()
    }
}

/// The source errors worth logging. While the app stops, the network trace ending is the exit's
/// own doing (`stop_worker` stops the session after the stop request), not news.
fn errors_to_log(errors: &[String], stopping: bool) -> impl Iterator<Item = &String> {
    errors.iter().filter(move |e| !(stopping && e.as_str() == negotium_collector::netetw::TRACE_STOPPED))
}

/// The app's per-process network trace session (`negotium_collector::netetw::session_name`, one
/// per logon session). Only the worker `spawn` starts runs it; the sources tests build go without.
pub fn net_session() -> String {
    negotium_collector::netetw::session_name()
}

/// Tells the worker to stop, then ends its network trace session with `stop_session`. The worker
/// thread isn't joined at exit, so its collector (and the feed's `Drop`) never goes away on its
/// own; stopping by name after the stop request leaves no session behind, and no new one starts
/// (a stopped supervisor builds no further source).
pub fn stop_worker(controls: &Controls, stop_session: impl FnOnce(&str)) {
    controls.request_stop();
    stop_session(&net_session());
}

/// Starts the supervised worker. Collector time counts from `epoch`, which the bridge shares to
/// place the scrolling graphs against real time (crate::smooth).
pub fn spawn(
    controls: Arc<Controls>,
    history: SharedHistory,
    epoch: Instant,
    tray: Option<TraySink>,
    sink: impl FnMut(Frame) + Send + 'static,
) -> std::thread::JoinHandle<u32> {
    let ids = IdSeed::default();
    std::thread::Builder::new()
        .name("negotium-worker".into())
        .spawn(move || {
            let mut log = crate::log::Log::new(crate::log::Log::default_path());
            supervise(
                move || {
                    let source = LiveSource::new(history.clone(), epoch, ids.clone()).with_net_feed(&net_session());
                    match &tray {
                        Some(t) => source.with_tray(t.clone()),
                        None => source,
                    }
                },
                controls,
                sink,
                move |m: &str| log.warn(m),
            )
        })
        .expect("spawn worker thread")
}

/// Installs, once, a panic hook that hides panics expected by a test (message starting with
/// "test panic:") and passes every other one to the hook installed before it. Never removed, so
/// tests running in parallel can't drop each other's.
#[cfg(test)]
pub(crate) fn quiet_expected_panics() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if !panic_text(info.payload()).starts_with("test panic:") {
                previous(info);
            }
        }));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct Panicky(Arc<AtomicU32>);
    impl Source for Panicky {
        fn next_frame(&mut self, _: &Controls) -> Option<Frame> {
            if self.0.fetch_add(1, Ordering::SeqCst).is_multiple_of(2) {
                panic!("test panic: collector blew up");
            }
            Some(Frame::default())
        }
    }

    #[test]
    fn exit_stops_the_worker_then_ends_the_network_session() {
        let controls = Controls::default();
        let mut stopped = Vec::new();
        stop_worker(&controls, |name| stopped.push((name.to_string(), controls.stopped())));
        assert_eq!(stopped, [(net_session(), true)], "after the stop request, by the app's name");
        assert!(net_session().starts_with("Negotium-Net-"), "{}", net_session());
    }

    #[test]
    fn a_trace_stop_is_not_logged_while_the_app_stops() {
        use negotium_collector::netetw::TRACE_STOPPED;
        let errors = vec!["memory: GlobalMemoryStatusEx failed".to_string(), TRACE_STOPPED.to_string()];
        assert_eq!(errors_to_log(&errors, false).collect::<Vec<_>>(), [&errors[0], &errors[1]]);
        assert_eq!(errors_to_log(&errors, true).collect::<Vec<_>>(), [&errors[0]], "other errors still logged");
    }

    #[test]
    fn only_the_spawned_worker_asks_for_the_network_feed() {
        // A test's source never starts a session, so running the suite elevated can't replace
        // the app's. (Unelevated `with_net_feed` starts nothing either; this pins the default.)
        let source = LiveSource::new(SharedHistory::default(), Instant::now(), IdSeed::default());
        assert!(!source.collector.has_net_feed());
    }

    #[test]
    fn only_the_visible_page_is_built_while_collection_keeps_running() {
        use crate::controls::Page;
        let mut source = LiveSource::new(SharedHistory::default(), Instant::now(), IdSeed::default());
        let dir = std::env::temp_dir().join(format!("negotium-worker-test-{}", std::process::id()));
        source.log = crate::log::Log::with_writes(dir.join("negotium.log"), crate::appdata::Writes::Plain);
        let controls = Controls::default();

        let f = source.next_frame(&controls).expect("visible: a frame");
        assert_eq!(f.page, Page::Overview);
        assert!(!f.meters.is_empty() && !f.rows.is_empty(), "the Overview is built");
        assert!(f.perf.is_none(), "no Performance view while the Overview shows");

        controls.set_page(Page::Performance);
        let f = source.next_frame(&controls).expect("visible: a frame");
        assert_eq!(f.page, Page::Performance);
        let perf = f.perf.expect("the Performance view is built");
        assert_eq!(perf.selected, "cpu");
        assert!(f.meters.is_empty() && f.tiles.is_empty() && f.rows.is_empty() && f.targets.is_empty(),
            "no Overview view while Performance shows");

        // Nothing on the Startup and Settings pages changes per tick: no frame (PF12), while
        // collection and history go on.
        for page in [Page::Startup, Page::Settings] {
            controls.set_page(page);
            let before = source.history.lock().unwrap().len();
            std::thread::sleep(Duration::from_millis(negotium_history::MIN_SPACING_MS + 50));
            assert!(source.next_frame(&controls).is_none(), "no frame is built for {page:?}");
            assert!(source.history.lock().unwrap().len() > before, "{page:?}: collection and history keep running");
        }
        controls.set_page(Page::Performance);

        controls.set_visible(false);
        let before = source.history.lock().unwrap().len();
        // History keeps one sample per MIN_SPACING_MS: wait so this tick's sample is kept.
        std::thread::sleep(Duration::from_millis(negotium_history::MIN_SPACING_MS + 50));
        assert!(source.next_frame(&controls).is_none(), "nothing is built while hidden in the tray");
        assert!(source.history.lock().unwrap().len() > before, "collection and history keep running while hidden");

        controls.set_visible(true);
        controls.set_page(Page::Overview);
        let f = source.next_frame(&controls).expect("visible again");
        assert!(f.perf.is_none() && !f.rows.is_empty(), "switching back stops building Performance");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A source on the Ports page whose socket reads are counted; each read gives the next result
    /// (the last repeats). The one socket is ours, so its owner is in the snapshot.
    fn ports_source(results: Vec<Result<Vec<negotium_collector::sockets::Socket>, negotium_collector::sockets::SocketError>>) -> (LiveSource, Arc<AtomicU32>) {
        let mut source = LiveSource::new(SharedHistory::default(), Instant::now(), IdSeed::default());
        let dir = std::env::temp_dir().join(format!("negotium-worker-ports-{}-{:?}", std::process::id(), std::thread::current().id()));
        source.log = crate::log::Log::with_writes(dir.join("negotium.log"), crate::appdata::Writes::Plain);
        let calls = Arc::new(AtomicU32::new(0));
        let c = calls.clone();
        source.ports = PortReader::new(
            Box::new(move || results[(c.fetch_add(1, Ordering::SeqCst) as usize).min(results.len() - 1)].clone()),
            Box::new(|| Ok(Default::default())),
        );
        (source, calls)
    }

    fn our_socket() -> negotium_collector::sockets::Socket {
        use negotium_collector::sockets::{Protocol, Socket};
        Socket { protocol: Protocol::Udp, local: "127.0.0.1:5353".parse().unwrap(), remote: None, state: None, pid: std::process::id() }
    }

    /// Our own process as the Overview sees it: (pid, create time).
    fn ourselves(source: &mut LiveSource) -> (u32, i64) {
        let f = source.next_frame(&Controls::default()).expect("visible: a frame");
        let me = std::process::id();
        f.all_procs.iter().find(|p| p.0 == me).map(|p| (p.0, p.1)).expect("we are in the snapshot")
    }

    fn watching(me: (u32, i64)) -> Controls {
        let c = Controls::default();
        c.set_details_watch(Some(crate::details::DetailsWatch { procs: vec![me], selected: me }));
        c
    }

    #[test]
    fn a_watch_adds_details_to_overview_frames() {
        let (mut source, _) = ports_source(vec![Ok(vec![our_socket()])]);
        let me = ourselves(&mut source);
        let f = source.next_frame(&watching(me)).expect("visible: a frame");
        let d = f.details.expect("a watch adds the details");
        assert_eq!(d.procs.len(), 1);
        assert!(d.procs[0].alive && d.procs[0].threads > 0);
        assert_eq!(d.connections.len(), 1, "our socket");
        assert!(d.peak_bytes.is_some_and(|b| b > 0));
    }

    #[test]
    fn a_watch_builds_a_frame_on_pages_that_have_none() {
        let (mut source, _) = ports_source(vec![Ok(vec![])]);
        let me = ourselves(&mut source);
        let controls = watching(me);
        controls.set_page(Page::Settings);
        let f = source.next_frame(&controls).expect("a watch: a frame on Settings");
        assert_eq!(f.page, Page::Settings);
        assert!(f.details.is_some());
        controls.set_details_watch(None);
        assert!(source.next_frame(&controls).is_none(), "no watch: Settings has no frame");
    }

    #[test]
    fn clearing_the_watch_stops_details() {
        let (mut source, _) = ports_source(vec![Ok(vec![])]);
        let me = ourselves(&mut source);
        let controls = watching(me);
        assert!(source.next_frame(&controls).and_then(|f| f.details).is_some());
        controls.set_details_watch(None);
        assert!(source.next_frame(&controls).and_then(|f| f.details).is_none());
    }

    #[test]
    fn sockets_are_not_read_without_ports_or_a_watch() {
        let (mut source, calls) = ports_source(vec![Ok(vec![])]);
        source.next_frame(&Controls::default());
        source.next_frame(&Controls::default());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn the_ports_page_builds_rows_and_targets_and_no_other_page() {
        let (mut source, _) = ports_source(vec![Ok(vec![our_socket()])]);
        let controls = Controls::default();
        controls.set_page(Page::Ports);
        let f = source.next_frame(&controls).expect("visible: a frame");
        assert_eq!(f.page, Page::Ports);
        let ports = f.ports.expect("the Ports page is built");
        assert_eq!(ports.rows.len(), 1);
        assert!(ports.rows[0].actionable, "our own process is in the snapshot: {:?}", ports.rows[0]);
        assert!(ports.targets.contains_key(&ports.rows[0].id), "an owned row has an action target");
        assert!(ports.error.is_none() && !ports.all_procs.is_empty());
        assert!(f.rows.is_empty() && f.meters.is_empty() && f.perf.is_none(), "no Overview or Performance view on Ports");
    }

    #[test]
    fn sockets_are_read_only_while_the_ports_page_is_on_screen() {
        let (mut source, calls) = ports_source(vec![Ok(vec![our_socket()])]);
        let controls = Controls::default();
        controls.set_page(Page::Ports);
        controls.set_visible(false);
        assert!(source.next_frame(&controls).is_none(), "hidden: no frame");
        assert_eq!(calls.load(Ordering::SeqCst), 0, "hidden: no read");
        controls.set_visible(true);
        controls.set_page(Page::Overview);
        let f = source.next_frame(&controls).expect("visible: a frame");
        assert!(f.ports.is_none());
        assert_eq!(calls.load(Ordering::SeqCst), 0, "another page: no read");
        controls.set_page(Page::Ports);
        let f = source.next_frame(&controls).expect("visible: a frame");
        assert_eq!(f.ports.map(|p| p.rows.len()), Some(1));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn two_ticks_read_the_sockets_once_unless_refresh_was_clicked() {
        let (mut source, calls) = ports_source(vec![Ok(vec![our_socket()])]);
        let controls = Controls::default();
        controls.set_page(Page::Ports);
        source.next_frame(&controls);
        source.next_frame(&controls);
        assert_eq!(calls.load(Ordering::SeqCst), 1, "within 2 s: the last list");
        controls.request_ports_refresh();
        source.next_frame(&controls);
        assert_eq!(calls.load(Ordering::SeqCst), 2, "Refresh reads at once");
        source.next_frame(&controls);
        assert_eq!(calls.load(Ordering::SeqCst), 2, "and is taken once");
    }

    #[test]
    fn a_failed_socket_read_gives_the_error_and_the_previous_rows() {
        use negotium_collector::sockets::SocketError;
        let (mut source, _) = ports_source(vec![Ok(vec![our_socket()]), Err(SocketError::Api { table: "GetExtendedUdpTable (IPv4)", code: 5 })]);
        let controls = Controls::default();
        controls.set_page(Page::Ports);
        let first = source.next_frame(&controls).and_then(|f| f.ports).expect("ports");
        assert_eq!(first.error, None);
        controls.request_ports_refresh();
        let second = source.next_frame(&controls).and_then(|f| f.ports).expect("ports");
        assert_eq!(second.error.as_deref(), Some("GetExtendedUdpTable (IPv4) failed: 0x5"));
        assert_eq!(second.rows, first.rows, "the previous rows stay");
    }

    #[test]
    fn the_tray_gets_one_cpu_sample_per_kept_sample_even_while_hidden() {
        let got: Arc<Mutex<Vec<crate::tray::TraySample>>> = Default::default();
        let sink_got = got.clone();
        let history = SharedHistory::default();
        let mut source = LiveSource::new(history.clone(), Instant::now(), IdSeed::default())
            .with_tray(Arc::new(move |s| sink_got.lock().unwrap().push(s)));
        let dir = std::env::temp_dir().join(format!("negotium-worker-tray-{}", std::process::id()));
        source.log = crate::log::Log::with_writes(dir.join("negotium.log"), crate::appdata::Writes::Plain);
        // An empty history keeps the next tick's sample whenever it lands (no wall-clock wait).
        let empty = || *history.lock().unwrap() = History::new();
        let controls = Controls::default();
        controls.set_visible(false);
        source.next_frame(&controls);
        assert!(got.lock().unwrap().is_empty(), "off until Settings turns it on");
        controls.set_tray_cpu(true);
        empty();
        controls.set_shell_hung(true);
        assert!(source.next_frame(&controls).is_none(), "hidden in the tray: no frame");
        assert_eq!(controls.shell_hung(), crate::platform::shell_tray_hung(), "the taskbar's state is stored while hidden");
        let first = got.lock().unwrap().first().cloned().expect("but the tray gets the sample");
        assert_eq!(first.points.last().map(|p| p.0), Some(first.end_ms), "the minute up to this sample");
        controls.set_tray_cpu(false);
        empty();
        source.next_frame(&controls);
        assert_eq!(got.lock().unwrap().len(), 1, "switched off: no more samples");
        let _ = std::fs::remove_dir_all(dir);
        // One icon per kept sample, at pinned collector times: a tick too soon after the last kept
        // sample (MIN_SPACING_MS) gets none.
        let (h, at) = (SharedHistory::default(), |t: u64| negotium_snapshot::Snapshot { taken_at_ms: t, ..Default::default() });
        let spacing = negotium_history::MIN_SPACING_MS;
        assert!(keep_sample(&h, &at(10_000), &[], true).is_some_and(|s| s.end_ms == 10_000), "kept: an icon");
        assert!(keep_sample(&h, &at(10_000 + spacing - 1), &[], true).is_none(), "too soon: not kept, no icon");
        assert!(keep_sample(&h, &at(10_000 + spacing), &[], false).is_none(), "kept, but the tray graph is off");
        assert!(keep_sample(&h, &at(10_000 + 2 * spacing), &[], true).is_some(), "the next kept sample: an icon");
    }

    #[test]
    fn a_restarted_worker_never_reuses_a_row_id_the_ui_may_still_hold() {
        let (history, ids) = (SharedHistory::default(), IdSeed::default());
        let dir = std::env::temp_dir().join(format!("negotium-worker-ids-{}", std::process::id()));
        let controls = Controls::default();
        let frame = |source: &mut LiveSource| {
            source.log = crate::log::Log::with_writes(dir.join("negotium.log"), crate::appdata::Writes::Plain);
            source.next_frame(&controls).expect("visible: a frame")
        };
        let a = frame(&mut LiveSource::new(history.clone(), Instant::now(), ids.clone()));
        // The supervisor's restart: a fresh source on the same history and id seed.
        let b = frame(&mut LiveSource::new(history.clone(), Instant::now(), ids.clone()));
        assert!(!a.targets.is_empty() && !b.targets.is_empty());
        let reused: Vec<&i32> = a.targets.keys().filter(|id| b.targets.contains_key(id)).collect();
        assert!(reused.is_empty(), "ids from before the restart were handed out again: {reused:?}");
        let _ = std::fs::remove_dir_all(dir);
    }

    fn one_sample() -> SharedHistory {
        let history = SharedHistory::default();
        write_history(&history, |h| h.push(1_000, &Default::default(), &[]));
        history
    }

    #[test]
    fn a_panic_while_building_a_view_keeps_the_history_across_the_restart() {
        quiet_expected_panics();
        let history = one_sample();
        let r = catch_unwind(|| read_history(&history, |_| -> Frame { panic!("test panic: view building") }));
        assert_eq!(panic_text(&*r.expect_err("the panic reaches the supervisor")), "test panic: view building");
        assert!(!history.is_poisoned(), "a read never poisons the lock");
        let source = LiveSource::new(history.clone(), Instant::now(), IdSeed::default());
        assert_eq!(source.history.lock().unwrap().len(), 1, "the restarted worker keeps the history");
    }

    #[test]
    fn a_panic_during_a_push_starts_the_history_over_on_restart() {
        quiet_expected_panics();
        let history = one_sample();
        let r = catch_unwind(|| write_history(&history, |_| panic!("test panic: mid-push")));
        assert!(r.is_err());
        assert!(history.is_poisoned());
        let _source = LiveSource::new(history.clone(), Instant::now(), IdSeed::default());
        assert!(!history.is_poisoned(), "the poison is cleared");
        assert!(history.lock().expect("lockable again").is_empty(), "a possibly half-written history starts over");
    }

    #[test]
    fn latest_slot_keeps_only_the_newest_and_wakes_once() {
        let slot = Latest::default();
        assert!(slot.put(1), "first value into an empty slot needs a wakeup");
        assert!(!slot.put(2), "the UI hasn't taken the first yet: no second wakeup");
        assert!(!slot.put(3));
        assert_eq!(slot.take(), Some(3), "the UI gets only the newest");
        assert_eq!(slot.take(), None);
        assert!(slot.put(4), "after a take the next value wakes the UI again");
    }

    #[test]
    fn backoff_doubles_and_caps() {
        assert_eq!(backoff(1), Duration::from_millis(500));
        assert_eq!(backoff(2), Duration::from_millis(1000));
        assert_eq!(backoff(20), Duration::from_secs(30));
    }

    #[test]
    fn panics_are_survived_with_backoff_and_frames_keep_flowing() {
        quiet_expected_panics();
        let controls = Arc::new(Controls::default());
        controls.set_visible(true);
        let calls = Arc::new(AtomicU32::new(0));
        let frames = Arc::new(AtomicU32::new(0));
        let (c2, calls2, frames2) = (controls.clone(), calls.clone(), frames.clone());
        let logged = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let logged2 = logged.clone();
        let started = std::time::Instant::now();
        let handle = std::thread::spawn(move || {
            supervise(
                move || Panicky(calls2.clone()),
                c2,
                move |_| { frames2.fetch_add(1, Ordering::SeqCst); },
                move |m: &str| logged2.lock().unwrap().push(m.to_string()),
            )
        });
        std::thread::sleep(Duration::from_millis(1800));
        controls.request_stop();
        let restarts = handle.join().unwrap();
        assert!(restarts >= 1, "a panic must trigger a restart");
        assert!(restarts <= 3, "backoff must prevent a tight restart loop: {restarts}");
        assert!(frames.load(Ordering::SeqCst) >= 1, "frames continue after a restart");
        assert!(started.elapsed() < Duration::from_secs(5), "stop is honoured promptly");
        assert!(logged.lock().unwrap().iter().any(|m| m.contains("collector blew up")), "panics are logged");
    }
}
