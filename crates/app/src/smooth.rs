//! Smooth graphs (Settings > Smooth graphs): between samples the graphs scroll at display rate
//! instead of stepping once per refresh. Collection keeps the chosen refresh rate; only the view
//! moves, and Slint moves it (`GraphClock` in ui/graph.slint): each frame's paths put their newest
//! sample at x = 1000, and the view's right edge trails it. This module decides where that edge
//! is. It runs about one sample interval behind real time, so the next sample is in hand before
//! the edge gets there: the newest point eases in from the right and the line always reaches the
//! edge. Pure; the bridge calls it on the UI thread.

/// How far the view's right edge may trail a frame's newest sample. Frames draw this much history
/// beyond their window's left edge (crate::graph::LEAD_MS), so the view never runs out of line.
pub const SCROLL_MARGIN_MS: u64 = 12_000;
/// Extra delay beyond one sample interval: a tick takes time, and so does the frame after it.
pub const LAG_MARGIN_MS: f64 = 250.0;
/// The view closes a gap to where it should be over at least this long (and over at least two
/// lags, so a slow refresh rate can't make it overshoot).
pub const CATCH_UP_MS: f64 = 2_000.0;
pub const MIN_SPEED: f64 = 0.5;
pub const MAX_SPEED: f64 = 2.0;

/// How far behind real time the view's right edge runs at refresh interval `refresh_ms`: one
/// history sample interval (at least a second: the history keeps at most one sample a second,
/// ruling P3-3) plus [`LAG_MARGIN_MS`].
pub fn lag_ms(refresh_ms: u64) -> f64 {
    refresh_ms.max(1_000) as f64 + LAG_MARGIN_MS
}

/// Redraws per second a graph `width_px` wide needs to scroll smoothly over `span_ms` at `speed`
/// (graph ms per real ms): half a pixel of motion is worth a redraw, within the ruled range of
/// 16 ms to 1 s between redraws (1 to 60 a second; zero or NaN gives the 1 s floor).
pub fn redraw_hz(width_px: f64, span_ms: f64, speed: f64) -> f64 {
    let hz = 2.0 * width_px * speed * 1_000.0 / span_ms.max(1.0);
    if hz.is_finite() { hz.clamp(1.0, 60.0) } else if hz > 0.0 { 60.0 } else { 1.0 }
}

/// The view's right edge (collector ms) from one frame to the next: a straight run that stops on
/// the frame's newest sample.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scroll {
    /// Where the right edge was when the frame landed.
    pub r0: f64,
    /// Collector time when the frame landed.
    pub at: f64,
    /// Graph ms per real ms.
    pub speed: f64,
    /// The frame's newest sample: the edge never passes it.
    pub newest: f64,
}

impl Scroll {
    /// The right edge at collector time `now`.
    pub fn right_edge(&self, now: f64) -> f64 {
        (self.r0 + self.speed * (now - self.at).max(0.0)).min(self.newest)
    }

    /// Where the run starts relative to the newest sample (≤ 0), for `GraphClock.lead-ms`.
    pub fn lead_ms(&self) -> f64 {
        self.r0 - self.newest
    }

    /// The run for a frame whose newest sample is `newest`, landing at collector time `now`, the
    /// view meant to trail real time by `lag`. The edge carries on from where `prev` has it, so
    /// nothing jumps, and its speed steers it toward `now - lag`; it is never so fast that it
    /// reaches the newest sample before the next one is due. Without a usable `prev` (the first
    /// frame, smoothing just turned on, a long absence, a history restart) the edge starts at
    /// `now - lag`. An absence of 12 s or less (within SCROLL_MARGIN_MS) is caught up at up to MAX_SPEED, 2x. Either way it stays within SCROLL_MARGIN_MS of the newest sample: the history
    /// the frame's paths hold.
    pub fn next(prev: Option<&Scroll>, now: f64, newest: f64, lag: f64) -> Scroll {
        let (floor, target) = (newest - SCROLL_MARGIN_MS as f64, now - lag);
        let r0 = match prev.map(|p| p.right_edge(now)) {
            Some(edge) if (floor..=newest).contains(&edge) => edge,
            _ => target.max(floor).min(newest),
        };
        let horizon = CATCH_UP_MS.max(2.0 * lag);
        // The next frame lands within one lag of this sample, so aim to reach it no sooner than that
        // (PF4): the edge then moves continuously whenever the cap applies, with no pause before a frame.
        let due = (newest + lag - now).max(1.0);
        let speed = (1.0 + (target - r0) / horizon).clamp(MIN_SPEED, MAX_SPEED).min((newest - r0) / due);
        Scroll { r0, at: now, speed, newest }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Frames every `refresh` ms (a tick takes 15 ms) from collector time `from`, each landing 5 ms
    /// after its tick; the history keeps a sample only 950 ms or more after the last (ruling P3-3).
    /// Returns (when each frame landed, its run).
    fn simulate(prev: Option<Scroll>, from: f64, refresh: u64, frames: usize) -> Vec<(f64, Scroll)> {
        let lag = lag_ms(refresh);
        let (mut prev, mut newest) = (prev, prev.map(|p| p.newest));
        let mut out = Vec::new();
        for i in 0..frames {
            let tick = from + i as f64 * (refresh + 15) as f64;
            if newest.is_none_or(|n| tick - n >= 950.0) {
                newest = Some(tick);
            }
            let now = tick + 5.0;
            let run = Scroll::next(prev.as_ref(), now, newest.unwrap(), lag);
            out.push((now, run));
            prev = Some(run);
        }
        out
    }

    #[test]
    fn the_view_trails_by_one_sample_interval_and_a_margin() {
        assert_eq!([500, 1_000, 2_000, 5_000].map(lag_ms), [1_250.0, 1_250.0, 2_250.0, 5_250.0]);
        assert!(SCROLL_MARGIN_MS as f64 >= 2.0 * lag_ms(5_000), "room for the lag and a catch-up");
    }

    #[test]
    fn the_edge_never_jumps_when_a_frame_lands() {
        for refresh in [500, 1_000, 2_000, 5_000] {
            for w in simulate(None, 0.0, refresh, 40).windows(2) {
                let ((_, a), (now, b)) = (w[0], w[1]);
                assert!((a.right_edge(now) - b.r0).abs() < 1e-6, "{refresh} ms: {} then {}", a.right_edge(now), b.r0);
            }
        }
    }

    #[test]
    fn the_edge_settles_one_sample_interval_behind_and_never_stalls() {
        for refresh in [500, 1_000, 2_000, 5_000] {
            let lag = lag_ms(refresh);
            let runs = simulate(None, 0.0, refresh, 60);
            for w in runs[30..].windows(2) {
                let ((t0, run), (t1, _)) = (w[0], w[1]);
                assert!((run.r0 - (t0 - lag)).abs() < 100.0, "{refresh} ms: edge {} vs {}", run.r0, t0 - lag);
                let mut last = run.right_edge(t0);
                let mut t = t0 + 10.0;
                while t < t1 {
                    let edge = run.right_edge(t);
                    assert!(edge > last && edge < run.newest, "{refresh} ms: stalled at {t}: {edge} (newest {})", run.newest);
                    last = edge;
                    t += 10.0;
                }
            }
        }
    }

    #[test]
    fn a_new_refresh_rate_eases_in_without_a_jump() {
        let before = simulate(None, 0.0, 1_000, 30);
        let (last_now, last) = *before.last().unwrap();
        // The user picks 5 s: frames now come 5 s apart and the view must fall 4 s further behind.
        let after = simulate(Some(last), last_now + 1_000.0, 5_000, 30);
        let (now, first) = after[0];
        assert!((last.right_edge(now) - first.r0).abs() < 1e-6, "no jump when the rate changes");
        assert!(first.speed > 0.0 && first.speed < 1.0, "it slows down to fall back: {}", first.speed);
        assert!(first.right_edge(now + 4_900.0) < first.newest, "without running into the newest sample before the next one is due");
        let (now, settled) = *after.last().unwrap();
        assert!((settled.r0 - (now - lag_ms(5_000))).abs() < 100.0, "{} vs {}", settled.r0, now - lag_ms(5_000));
    }

    #[test]
    fn the_first_frame_starts_one_lag_behind_and_never_passes_its_newest_sample() {
        let run = Scroll::next(None, 61_000.0, 60_000.0, 1_250.0);
        assert_eq!((run.r0, run.lead_ms(), run.speed), (59_750.0, -250.0, 1.0));
        assert_eq!(run.right_edge(70_000.0), 60_000.0);
    }

    #[test]
    fn after_a_long_absence_the_edge_starts_one_lag_behind_without_fast_forward() {
        // An hour hidden in the tray: the last run ended an hour ago.
        let old = Scroll::next(None, 61_000.0, 60_000.0, 1_250.0);
        let run = Scroll::next(Some(&old), 3_661_000.0, 3_660_000.0, 1_250.0);
        assert!((run.r0 - 3_659_750.0).abs() < 1e-6);
        assert_eq!(run.speed, 1.0, "no fast-forward through the hour");
    }

    #[test]
    fn a_history_restart_puts_the_edge_on_the_new_samples() {
        // Collector time restarted (the history was cleared): the old run is far ahead of the new samples.
        let old = Scroll::next(None, 3_600_000.0, 3_599_000.0, 1_250.0);
        let run = Scroll::next(Some(&old), 2_005.0, 2_000.0, 1_250.0);
        assert!((run.r0 - 755.0).abs() < 1e-6);
        assert!(run.right_edge(2_500.0) <= 2_000.0);
    }

    #[test]
    fn capped_motion_is_continuous_when_the_next_frame_is_late_or_early() {
        // A frame lands with the same newest sample as the last: the edge is close to it, the cap
        // applies, and the edge must still move every 10 ms until the next frame is due.
        let lag = 1_250.0;
        let run = Scroll { r0: 59_000.0, at: 60_300.0, speed: 1.5, newest: 60_000.0 };
        let capped = Scroll::next(Some(&run), 60_800.0, 60_000.0, lag);
        let free = 1.0 + (60_800.0 - lag - 59_750.0) / 2_500.0;
        assert!(capped.speed > 0.0 && capped.speed < free, "the cap binds: {} < {free}", capped.speed);
        let (mut last, mut t) = (capped.right_edge(60_800.0), 60_810.0);
        while t < 60_000.0 + lag {
            let edge = capped.right_edge(t);
            assert!(edge > last, "stalled at {t}");
            last = edge;
            t += 10.0;
        }
        assert!(last <= capped.newest);
    }

    #[test]
    fn a_faster_refresh_rate_eases_back_without_a_jump() {
        let before = simulate(None, 0.0, 5_000, 20);
        let (last_now, last) = *before.last().unwrap();
        let after = simulate(Some(last), last_now + 5_000.0, 500, 60);
        assert!((last.right_edge(after[0].0) - after[0].1.r0).abs() < 1e-6);
        for w in after.windows(2) {
            let ((_, a), (now, b)) = (w[0], w[1]);
            assert!((a.right_edge(now) - b.r0).abs() < 1e-6, "jump");
            assert!(b.speed >= 0.0 && b.speed <= MAX_SPEED);
        }
        let (now, settled) = *after.last().unwrap();
        assert!((settled.r0 - (now - lag_ms(500))).abs() < 100.0);
    }

    #[test]
    fn half_second_refresh_alternates_frames_with_and_without_a_new_sample() {
        let runs = simulate(None, 0.0, 500, 40);
        let new_samples = runs.windows(2).filter(|w| w[1].1.newest != w[0].1.newest).count();
        assert!((18..=21).contains(&new_samples), "about every other frame brings a sample: {new_samples}");
        for w in runs[10..].windows(2) {
            let ((_, a), (now, b)) = (w[0], w[1]);
            assert!(b.right_edge(now + 100.0) > b.r0, "moving on a sample-less frame too");
            assert!((a.right_edge(now) - b.r0).abs() < 1e-6);
        }
    }

    #[test]
    fn the_edge_is_always_in_range_whatever_comes_in() {
        let mut prev = None;
        for (now, newest) in [(1e3, 5e2), (9e5, 9e5), (1e3, 9e5), (5e4, 4e4), (5e4, 4e4), (10.0, 0.0)] {
            let run = Scroll::next(prev.as_ref(), now, newest, 1_250.0);
            assert!(run.r0 <= newest && run.r0 >= newest - SCROLL_MARGIN_MS as f64);
            assert!(run.speed <= MAX_SPEED);
            assert!(run.speed >= 0.0 && run.speed.is_finite());
            prev = Some(run);
        }
    }

    #[test]
    fn the_redraw_rate_never_exceeds_sixty_a_second_however_wide_the_graph_is() {
        for width in [100.0, 1_000.0, 5_120.0, 10_240.0, 1e6] {
            for span in [60_000.0, 3_600_000.0] {
                for speed in [MIN_SPEED, 1.0, MAX_SPEED] {
                    assert!((1.0..=60.0).contains(&redraw_hz(width, span, speed)));
                }
            }
        }
        assert_eq!(redraw_hz(1_000.0, 3_600_000.0, 1.0), 1.0, "an hour-wide view redraws once a second");
        assert_eq!([redraw_hz(0.0, 60_000.0, 1.0), redraw_hz(f64::NAN, 60_000.0, 1.0)], [1.0, 1.0]);
    }

    #[test]
    fn a_minute_at_a_thousand_pixels_redraws_about_thirty_three_times_a_second() {
        assert!((redraw_hz(1_000.0, 60_000.0, 1.0) - 33.3).abs() < 0.1);
    }

    #[test]
    fn huge_timestamps_and_nan_do_not_panic_or_lose_the_edge() {
        let big = 1e12;
        let a = Scroll::next(None, big + 1_000.0, big, 1_250.0);
        assert!((a.r0 - (big - 250.0)).abs() < 1e-3 && a.speed == 1.0);
        let b = Scroll::next(Some(&a), big + 1_500.0, big + 500.0, 1_250.0);
        assert!((a.right_edge(big + 1_500.0) - b.r0).abs() < 1e-3);
        let n = Scroll::next(None, f64::NAN, f64::NAN, 1_250.0);
        let _ = n.right_edge(0.0);
        let m = Scroll::next(Some(&n), 1_000.0, 500.0, 1_250.0);
        assert!(m.r0.is_finite() && m.speed.is_finite());
    }

    #[test]
    fn a_slightly_older_newest_sample_does_not_snap_the_edge_back() {
        let a = Scroll::next(None, 61_000.0, 60_000.0, 1_250.0);
        let now = 61_500.0;
        let before = a.right_edge(now);
        let b = Scroll::next(Some(&a), now, 59_900.0, 1_250.0);
        assert!(b.r0 <= 59_900.0 && before - b.r0 <= 100.0 + 1e-6, "snap back of {}", before - b.r0);
        assert!(b.speed >= 0.0);
    }
}
