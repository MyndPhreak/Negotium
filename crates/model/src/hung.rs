//! Remembers when each process first had a hung window.
use negotium_snapshot::WindowSample;
use std::collections::{HashMap, HashSet};

#[derive(Default)]
pub struct HungTracker {
    since: HashMap<u32, u64>,
}

impl HungTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn update(&mut self, windows: &[WindowSample], now_ms: u64) -> HashMap<u32, u64> {
        let hung: HashSet<u32> = windows.iter().filter(|w| w.hung).map(|w| w.pid).collect();
        self.since.retain(|pid, _| hung.contains(pid));
        for pid in &hung {
            self.since.entry(*pid).or_insert(now_ms);
        }
        self.since.iter().map(|(pid, t)| (*pid, now_ms.saturating_sub(*t))).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(pid: u32, hung: bool) -> WindowSample {
        WindowSample { pid, hung, title: "t".into() }
    }

    #[test]
    fn tracks_time_since_first_hung_and_clears_on_recovery() {
        let mut t = HungTracker::new();
        assert!(t.update(&[w(1, false)], 0).is_empty());
        assert_eq!(t.update(&[w(1, true)], 1000)[&1], 0);
        assert_eq!(t.update(&[w(1, true), w(1, false)], 15_000)[&1], 14_000);
        assert!(t.update(&[w(1, false)], 16_000).is_empty());
        assert_eq!(t.update(&[w(1, true)], 20_000)[&1], 0);
    }
}
