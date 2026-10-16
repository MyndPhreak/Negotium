//! The supervised worker: one thread owns Collector -> Model -> Ranker -> History and emits
//! Frames. A panic anywhere in a tick rebuilds the pipeline after a backoff (spec section 7).
use crate::controls::{Controls, Page};
use crate::perf::{PerfInput, build_perf};
use crate::view::{Frame, FrameOptions, IdMap, build_frame};
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

fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
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

/// The history, shared with the UI thread for the scrub (read under the lock, plain data).
pub type SharedHistory = Arc<Mutex<History>>;

pub struct LiveSource {
    collector: Collector,
    info: SystemInfo,
    model: Model,
    ranker: Ranker,
    history: SharedHistory,
    ids: IdMap,
    elevated: bool,
    log: crate::log::Log,
}

impl LiveSource {
    /// `history` and `epoch` outlive a restart after a panic, so the graphs keep their past. A
    /// panic while the history was locked may have left it half-written: it starts over empty.
    pub fn new(history: SharedHistory, epoch: Instant) -> Self {
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
            ids: IdMap::new(),
            elevated: negotium_actions::is_elevated(),
            log: crate::log::Log::new(crate::log::Log::default_path()),
        }
    }
}

impl Source for LiveSource {
    fn next_frame(&mut self, controls: &Controls) -> Option<Frame> {
        let snap = self.collector.tick();
        for e in &snap.source_errors {
            self.log.warn(e);
        }
        let rows = self.model.update(&snap);
        let mut history = self.history.lock().unwrap_or_else(|e| e.into_inner());
        history.push(snap.taken_at_ms, &snap.totals, &rows);
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
                Some(build_frame(&mut self.ids, &snap, &rows, &order, &history, &opts))
            }
            Page::Performance => {
                let (device, range) = controls.perf();
                let input = PerfInput { snap: &snap, history: &history, info: &self.info, device: &device, range };
                Some(Frame { page: Page::Performance, perf: Some(build_perf(&input)), ..Default::default() })
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

pub fn spawn(
    controls: Arc<Controls>,
    history: SharedHistory,
    sink: impl FnMut(Frame) + Send + 'static,
) -> std::thread::JoinHandle<u32> {
    let epoch = Instant::now();
    std::thread::Builder::new()
        .name("negotium-worker".into())
        .spawn(move || {
            let mut log = crate::log::Log::new(crate::log::Log::default_path());
            supervise(move || LiveSource::new(history.clone(), epoch), controls, sink, move |m: &str| log.warn(m))
        })
        .expect("spawn worker thread")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct Panicky(Arc<AtomicU32>);
    impl Source for Panicky {
        fn next_frame(&mut self, _: &Controls) -> Option<Frame> {
            if self.0.fetch_add(1, Ordering::SeqCst).is_multiple_of(2) {
                panic!("collector blew up");
            }
            Some(Frame::default())
        }
    }

    #[test]
    fn only_the_visible_page_is_built_while_collection_keeps_running() {
        use crate::controls::Page;
        let mut source = LiveSource::new(SharedHistory::default(), Instant::now());
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
    fn a_restart_keeps_the_history_but_recovers_one_poisoned_by_a_panic() {
        let history = SharedHistory::default();
        history.lock().unwrap().push(1_000, &Default::default(), &[]);
        let kept = LiveSource::new(history.clone(), Instant::now());
        assert_eq!(kept.history.lock().unwrap().len(), 1, "a healthy history survives the restart");
        drop(kept);

        let h = history.clone();
        std::panic::set_hook(Box::new(|_| {}));
        let _ = std::thread::spawn(move || {
            let _guard = h.lock().unwrap();
            panic!("panicked mid-push");
        })
        .join();
        let _ = std::panic::take_hook();
        assert!(history.is_poisoned());
        let _source = LiveSource::new(history.clone(), Instant::now());
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
        std::panic::set_hook(Box::new(|_| {}));
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
        let _ = std::panic::take_hook();
        assert!(restarts >= 1, "a panic must trigger a restart");
        assert!(restarts <= 3, "backoff must prevent a tight restart loop: {restarts}");
        assert!(frames.load(Ordering::SeqCst) >= 1, "frames continue after a restart");
        assert!(started.elapsed() < Duration::from_secs(5), "stop is honoured promptly");
        assert!(logged.lock().unwrap().iter().any(|m| m.contains("collector blew up")), "panics are logged");
    }
}
