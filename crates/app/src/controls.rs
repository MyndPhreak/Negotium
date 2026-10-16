//! State shared between the UI thread (writes) and the worker (reads).
use crate::perf::Range;
use negotium_model::Metric;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

/// Which page the window shows; the worker builds only that page's view.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Page {
    #[default]
    Overview = 0,
    Performance = 1,
    Startup = 2,
    Settings = 3,
}

impl Page {
    pub const ALL: [Page; 4] = [Page::Overview, Page::Performance, Page::Startup, Page::Settings];

    /// Unknown indexes fall back to the Overview.
    pub fn from_index(i: i32) -> Page {
        match i {
            1 => Page::Performance,
            2 => Page::Startup,
            3 => Page::Settings,
            _ => Page::Overview,
        }
    }

    pub fn index(self) -> i32 {
        self as i32
    }
}

pub struct Controls {
    metric: AtomicU8,
    page: AtomicU8,
    /// Selected Performance device id and graph range.
    perf: Mutex<(String, Range)>,
    frozen: AtomicBool,
    visible: AtomicBool,
    /// Refresh interval while visible (Settings), milliseconds.
    refresh_ms: AtomicU32,
    stop: AtomicBool,
    /// Expanded app row ids and whether a search is active: the worker builds child rows only
    /// for those (spec section 8: no per-process formatting for rows nobody can see).
    child_rows: Mutex<(HashSet<i32>, bool)>,
    /// Latched wake flag: a poke that lands while the worker is busy (not waiting) is
    /// remembered, so the next `wait` returns immediately instead of losing the wakeup.
    woken: Mutex<bool>,
    cv: Condvar,
}

impl Default for Controls {
    fn default() -> Self {
        Self {
            metric: AtomicU8::new(0),
            page: AtomicU8::new(Page::Overview as u8),
            perf: Mutex::new(("cpu".to_string(), Range::Minute)),
            frozen: AtomicBool::new(false),
            visible: AtomicBool::new(true),
            refresh_ms: AtomicU32::new(1000),
            stop: AtomicBool::new(false),
            child_rows: Mutex::new((HashSet::new(), false)),
            woken: Mutex::new(false),
            cv: Condvar::new(),
        }
    }
}

impl Controls {
    pub fn set_metric(&self, m: Metric) {
        self.metric.store(m as u8, Ordering::Relaxed);
        self.poke();
    }
    pub fn metric(&self) -> Metric {
        Metric::ALL[(self.metric.load(Ordering::Relaxed) as usize).min(Metric::ALL.len() - 1)]
    }
    pub fn set_page(&self, p: Page) {
        self.page.store(p as u8, Ordering::Relaxed);
        self.poke();
    }
    pub fn page(&self) -> Page {
        Page::from_index(self.page.load(Ordering::Relaxed) as i32)
    }
    pub fn select_device(&self, id: &str) {
        self.perf.lock().unwrap_or_else(|e| e.into_inner()).0 = id.to_string();
        self.poke();
    }
    /// Records the device a frame fell back to; the frame is already on screen, so no wakeup.
    pub fn adopt_device(&self, id: &str) {
        self.perf.lock().unwrap_or_else(|e| e.into_inner()).0 = id.to_string();
    }
    pub fn set_range(&self, r: Range) {
        self.perf.lock().unwrap_or_else(|e| e.into_inner()).1 = r;
        self.poke();
    }
    pub fn perf(&self) -> (String, Range) {
        self.perf.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    pub fn set_frozen(&self, f: bool) {
        self.frozen.store(f, Ordering::Relaxed);
    }
    pub fn frozen(&self) -> bool {
        self.frozen.load(Ordering::Relaxed)
    }
    pub fn set_visible(&self, v: bool) {
        self.visible.store(v, Ordering::Relaxed);
        self.poke();
    }
    pub fn visible(&self) -> bool {
        self.visible.load(Ordering::Relaxed)
    }
    /// Sets the refresh rate (Settings) and wakes the worker, so it takes effect at once. The
    /// history still keeps at most one sample per second (`negotium_history::MIN_SPACING_MS`).
    pub fn set_refresh_ms(&self, ms: u32) {
        self.refresh_ms.store(ms.max(100), Ordering::Relaxed);
        self.poke();
    }
    /// The chosen rate while visible; hidden in the tray, 2 s or the chosen rate if slower (spec 4.2).
    pub fn interval(&self) -> Duration {
        let refresh = self.refresh_ms.load(Ordering::Relaxed).max(100) as u64;
        Duration::from_millis(if self.visible() { refresh } else { refresh.max(2000) })
    }
    /// Shares which apps need child rows. Wakes the worker when more are needed (an expand, or a
    /// search starting), so they show without waiting for the next tick.
    pub fn set_child_rows(&self, expanded: &HashSet<i32>, searching: bool) {
        let more = {
            let mut cur = self.child_rows.lock().unwrap_or_else(|e| e.into_inner());
            let more = (searching && !cur.1) || expanded.iter().any(|id| !cur.0.contains(id));
            *cur = (expanded.clone(), searching);
            more
        };
        if more {
            self.poke();
        }
    }
    pub fn child_rows(&self) -> (HashSet<i32>, bool) {
        self.child_rows.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    pub fn request_stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        self.poke();
    }
    pub fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }
    pub fn poke(&self) {
        *self.woken.lock().unwrap_or_else(|e| e.into_inner()) = true;
        self.cv.notify_all();
    }
    /// Sleeps up to `d`; returns true if poked earlier (or a poke was already pending).
    pub fn wait(&self, d: Duration) -> bool {
        let guard = self.woken.lock().unwrap_or_else(|e| e.into_inner());
        let (mut guard, result) = self
            .cv
            .wait_timeout_while(guard, d, |woken| !*woken)
            .unwrap_or_else(|e| e.into_inner());
        let woke = !result.timed_out();
        *guard = false;
        woke
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    #[test]
    fn interval_depends_on_visibility() {
        let c = Controls::default();
        c.set_visible(true);
        assert_eq!(c.interval(), Duration::from_secs(1));
        c.set_visible(false);
        assert_eq!(c.interval(), Duration::from_secs(2));
    }

    #[test]
    fn the_refresh_rate_applies_while_visible_and_never_speeds_up_the_tray() {
        let c = Controls::default();
        c.set_refresh_ms(500);
        assert!(c.wait(Duration::from_millis(20)), "a new rate takes effect at once");
        c.set_visible(true);
        assert_eq!(c.interval(), Duration::from_millis(500));
        c.set_visible(false);
        assert_eq!(c.interval(), Duration::from_secs(2), "hidden: 2 s (spec 4.2)");
        c.set_refresh_ms(5000);
        assert_eq!(c.interval(), Duration::from_secs(5), "hidden never ticks faster than chosen");
        c.set_visible(true);
        assert_eq!(c.interval(), Duration::from_secs(5));
    }

    #[test]
    fn poke_wakes_a_waiter_early() {
        let c = Arc::new(Controls::default());
        let c2 = c.clone();
        let t = std::thread::spawn(move || {
            let start = Instant::now();
            let woke = c2.wait(Duration::from_secs(5));
            (woke, start.elapsed())
        });
        std::thread::sleep(Duration::from_millis(100));
        c.set_metric(Metric::Memory);
        let (woke, elapsed) = t.join().unwrap();
        assert!(woke && elapsed < Duration::from_secs(2));
        assert_eq!(c.metric(), Metric::Memory);
    }

    #[test]
    fn child_rows_are_shared_and_newly_needed_children_wake_the_worker() {
        let c = Controls::default();
        c.set_child_rows(&HashSet::from([7]), false);
        assert!(c.wait(Duration::from_millis(20)), "an expand asks for a frame with its children now");
        assert_eq!(c.child_rows(), (HashSet::from([7]), false));
        c.set_child_rows(&HashSet::new(), false);
        assert!(!c.wait(Duration::from_millis(20)), "collapsing needs nothing new");
        c.set_child_rows(&HashSet::new(), true);
        assert!(c.wait(Duration::from_millis(20)), "starting a search needs every app's children");
        assert_eq!(c.child_rows(), (HashSet::new(), true));
    }

    #[test]
    fn page_and_performance_selection_are_shared_and_wake_the_worker() {
        use crate::perf::Range;
        let c = Controls::default();
        assert_eq!(c.page(), Page::Overview);
        assert_eq!(c.perf(), ("cpu".to_string(), Range::Minute));
        c.set_page(Page::Performance);
        assert!(c.wait(Duration::from_millis(20)));
        assert_eq!(c.page(), Page::Performance);
        c.select_device("memory");
        assert!(c.wait(Duration::from_millis(20)));
        c.set_range(Range::Hour);
        assert!(c.wait(Duration::from_millis(20)));
        assert_eq!(c.perf(), ("memory".to_string(), Range::Hour));
        c.adopt_device("cpu");
        assert!(!c.wait(Duration::from_millis(20)), "a fallback adopted from a frame needs no new frame");
        assert_eq!(c.perf().0, "cpu");
        assert_eq!(Page::ALL.map(Page::index), [0, 1, 2, 3]);
        assert_eq!(Page::from_index(3), Page::Settings);
        assert_eq!(Page::from_index(9), Page::Overview, "unknown pages fall back to the Overview");
        assert_eq!(Page::from_index(-2), Page::Overview);
    }

    #[test]
    fn poke_before_wait_is_not_lost() {
        let c = Controls::default();
        c.poke();
        let start = Instant::now();
        assert!(c.wait(Duration::from_secs(5)));
        assert!(start.elapsed() < Duration::from_secs(1));
        // The latch is consumed: a second wait times out.
        assert!(!c.wait(Duration::from_millis(20)));
    }
}
