//! The supervised worker: one thread owns Collector -> Model -> Ranker -> History and emits
//! Frames. A panic anywhere in a tick rebuilds the pipeline after a backoff (spec section 7).
use crate::controls::{Controls, Page};
use crate::perf::{LastSeen, PerfInput, build_perf};
use crate::tray::TraySample;
use crate::view::{Frame, FrameOptions, IdMap, IdSeed, build_frame};
use negotium_collector::{Collector, SystemInfo};
use negotium_history::History;
use negotium_model::{Model, Ranker};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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

pub struct LiveSource {
    collector: Collector,
    info: SystemInfo,
    model: Model,
    ranker: Ranker,
    history: SharedHistory,
    ids: IdMap,
    /// The selected Performance device as last seen, for its page's grace period.
    seen: Option<LastSeen>,
    elevated: bool,
    log: crate::log::Log,
    tray: Option<TraySink>,
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
            model: Model::new(),
            ranker: Ranker::new(),
            history,
            ids: IdMap::with_seed(ids),
            seen: None,
            elevated: negotium_actions::is_elevated(),
            log: crate::log::Log::new(crate::log::Log::default_path()),
            tray: None,
        }
    }

    /// Sends the tray a CPU sample for every history sample kept while Controls::tray_cpu is on,
    /// visible or hidden.
    pub fn with_tray(mut self, sink: TraySink) -> Self {
        self.tray = Some(sink);
        self
    }
}

impl Source for LiveSource {
    fn next_frame(&mut self, controls: &Controls) -> Option<Frame> {
        let snap = self.collector.tick();
        for e in &snap.source_errors {
            self.log.warn(e);
        }
        let rows = self.model.update(&snap);
        // The tray's minute is read under the same lock as the push that kept the sample.
        let for_tray = controls.tray_cpu() && self.tray.is_some();
        let tray_sample = write_history(&self.history, |h| {
            let kept = h.push(snap.taken_at_ms, &snap.totals, &rows);
            (kept && for_tray).then(|| TraySample::from_history(h, snap.taken_at_ms, snap.totals.cpu_pct))
        });
        // Every tick, visible or hidden, so the tray never waits on a stale answer (crate::tray, PF2).
        controls.set_shell_hung(crate::platform::shell_tray_hung());
        if let (Some(sample), Some(tray)) = (tray_sample, &self.tray) {
            tray(sample);
        }
        if !controls.visible() {
            return None;
        }
        // Only the page on screen is built (spec section 8).
        match controls.page() {
            Page::Overview => {
                let metric = controls.metric();
                let order = self.ranker.order(&rows, metric, snap.taken_at_ms, controls.frozen());
                let (expanded, all_children) = controls.child_rows();
                let opts = FrameOptions { metric, elevated: self.elevated, expanded, all_children };
                let ids = &mut self.ids;
                Some(read_history(&self.history, |h| build_frame(ids, &snap, &rows, &order, h, &opts)))
            }
            Page::Performance => {
                let (device, range) = controls.perf();
                self.seen = LastSeen::track(self.seen.take(), &snap, &device);
                let perf = read_history(&self.history, |h| {
                    build_perf(&PerfInput { snap: &snap, history: h, info: &self.info, device: &device, range, last_seen: self.seen.as_ref() })
                });
                Some(Frame { page: Page::Performance, perf: Some(perf), ..Default::default() })
            }
            // Nothing on these pages changes per tick.
            Page::Startup | Page::Settings => None,
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
                    let source = LiveSource::new(history.clone(), epoch, ids.clone());
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
    fn only_the_visible_page_is_built_while_collection_keeps_running() {
        use crate::controls::Page;
        let mut source = LiveSource::new(SharedHistory::default(), Instant::now(), IdSeed::default());
        let dir = std::env::temp_dir().join(format!("negotium-worker-test-{}", std::process::id()));
        source.log = crate::log::Log::new(dir.join("negotium.log"));
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

    #[test]
    fn the_tray_gets_one_cpu_sample_per_kept_sample_even_while_hidden() {
        let got: Arc<Mutex<Vec<crate::tray::TraySample>>> = Default::default();
        let sink_got = got.clone();
        let mut source = LiveSource::new(SharedHistory::default(), Instant::now(), IdSeed::default())
            .with_tray(Arc::new(move |s| sink_got.lock().unwrap().push(s)));
        let dir = std::env::temp_dir().join(format!("negotium-worker-tray-{}", std::process::id()));
        source.log = crate::log::Log::new(dir.join("negotium.log"));
        let controls = Controls::default();
        controls.set_visible(false);
        source.next_frame(&controls);
        assert!(got.lock().unwrap().is_empty(), "off until Settings turns it on");
        controls.set_tray_cpu(true);
        std::thread::sleep(Duration::from_millis(negotium_history::MIN_SPACING_MS + 50));
        controls.set_shell_hung(true);
        assert!(source.next_frame(&controls).is_none(), "hidden in the tray: no frame");
        assert_eq!(controls.shell_hung(), crate::platform::shell_tray_hung(), "the taskbar's state is stored while hidden");
        let first = got.lock().unwrap().first().cloned().expect("but the tray gets the sample");
        assert_eq!(first.points.last().map(|p| p.0), Some(first.end_ms), "the minute up to this sample");
        source.next_frame(&controls); // too soon: the history keeps no sample
        assert_eq!(got.lock().unwrap().len(), 1, "one icon per kept sample");
        controls.set_tray_cpu(false);
        std::thread::sleep(Duration::from_millis(negotium_history::MIN_SPACING_MS + 50));
        source.next_frame(&controls);
        assert_eq!(got.lock().unwrap().len(), 1, "switched off: no more samples");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_restarted_worker_never_reuses_a_row_id_the_ui_may_still_hold() {
        let (history, ids) = (SharedHistory::default(), IdSeed::default());
        let dir = std::env::temp_dir().join(format!("negotium-worker-ids-{}", std::process::id()));
        let controls = Controls::default();
        let frame = |source: &mut LiveSource| {
            source.log = crate::log::Log::new(dir.join("negotium.log"));
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
