//! State shared between the UI thread (writes) and the worker (reads).
use negotium_model::Metric;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

pub struct Controls {
    metric: AtomicU8,
    frozen: AtomicBool,
    visible: AtomicBool,
    stop: AtomicBool,
    /// Latched wake flag: a poke that lands while the worker is busy (not waiting) is
    /// remembered, so the next `wait` returns immediately instead of losing the wakeup.
    woken: Mutex<bool>,
    cv: Condvar,
}

impl Default for Controls {
    fn default() -> Self {
        Self {
            metric: AtomicU8::new(0),
            frozen: AtomicBool::new(false),
            visible: AtomicBool::new(true),
            stop: AtomicBool::new(false),
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
    pub fn interval(&self) -> Duration {
        if self.visible() { Duration::from_secs(1) } else { Duration::from_secs(2) }
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
