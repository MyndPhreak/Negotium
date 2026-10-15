//! Graph geometry: (time, value) series as Slint path commands for a 1000 × 100 view box. x is
//! time, so a graph always spans its whole range with the newest sample on the right edge; a
//! range with little history draws only where history exists (never stretched).
use std::fmt::Write;

/// Two consecutive samples further apart than this are not joined: after a sleep or hibernate the
/// line breaks instead of crossing the missing period. Well above the slowest normal tick (the
/// hidden-window 2 s interval, and the 5 s refreshes of sparse graphs), far below any real pause.
pub const PAUSE_BREAK_MS: u64 = 10_000;

pub const VIEW_W: f64 = 1000.0;
/// A path that draws nothing (Slint needs a valid command string).
pub const EMPTY_PATH: &str = "M 0 100";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shape {
    pub line: String,
    /// The line closed down to the floor, for a translucent fill.
    pub area: String,
}

#[derive(Default)]
struct Pen {
    line: String,
    area: String,
    down: bool,
    last_x: f64,
}

impl Pen {
    fn point(&mut self, x: f64, y: f64) {
        if !self.line.is_empty() {
            self.line.push(' ');
        }
        let _ = write!(self.line, "{} {x:.1} {y:.1}", if self.down { "L" } else { "M" });
        if self.down {
            let _ = write!(self.area, " L {x:.1} {y:.1}");
        } else {
            if !self.area.is_empty() {
                self.area.push(' ');
            }
            let _ = write!(self.area, "M {x:.1} 100.0 L {x:.1} {y:.1}");
        }
        self.down = true;
        self.last_x = x;
    }

    fn gap(&mut self) {
        if self.down {
            let _ = write!(self.area, " L {:.1} 100.0 Z", self.last_x);
            self.down = false;
        }
    }

    fn finish(mut self) -> Shape {
        self.gap();
        let or_empty = |s: String| if s.is_empty() { EMPTY_PATH.to_string() } else { s };
        Shape { line: or_empty(self.line), area: or_empty(self.area) }
    }
}

/// Draws `points` in the window `[end - span, end]` merged into at most `buckets` columns; a
/// column shows its largest value (ruling P3-5) at its right edge, a column holding only missing
/// values breaks the line, and columns without any sample are simply skipped. `max <= 0` draws
/// values on the floor.
pub fn shape(points: impl IntoIterator<Item = (u64, Option<f32>)>, end: u64, span: u64, max: f32, buckets: usize) -> Shape {
    let span = span.max(1);
    let buckets = buckets.max(1);
    // Negative right after start: the window then reaches back before collector time 0, and the
    // little history there is still drawn at the right edge.
    let start = end as i64 - span as i64;
    let mut pen = Pen::default();
    let mut current: Option<(usize, Option<f32>)> = None;
    let mut prev_t: Option<i64> = None;
    let emit = |pen: &mut Pen, b: usize, best: Option<f32>| match best {
        Some(v) => {
            let x = (b + 1) as f64 / buckets as f64 * VIEW_W;
            let y = if max > 0.0 { 100.0 - (v / max).clamp(0.0, 1.0) as f64 * 100.0 } else { 100.0 };
            pen.point(x, y);
        }
        None => pen.gap(),
    };
    for (t, v) in points {
        let t = t as i64;
        if t < start || t > end as i64 {
            continue;
        }
        if prev_t.is_some_and(|p| t - p > PAUSE_BREAK_MS as i64) {
            if let Some((cur, best)) = current.take() {
                emit(&mut pen, cur, best);
            }
            pen.gap();
        }
        prev_t = Some(t);
        // Columns are (start + k·w, start + (k+1)·w]: the newest sample lands in the last one.
        let b = (((t - start) as u64 * buckets as u64).div_ceil(span) as usize).saturating_sub(1).min(buckets - 1);
        let v = v.filter(|v| v.is_finite());
        match &mut current {
            Some((cur, best)) if *cur == b => {
                if let Some(v) = v {
                    *best = Some(best.map_or(v, |b| b.max(v)));
                }
            }
            _ => {
                if let Some((cur, best)) = current.take() {
                    emit(&mut pen, cur, best);
                }
                current = Some((b, v));
            }
        }
    }
    if let Some((cur, best)) = current {
        emit(&mut pen, cur, best);
    }
    pen.finish()
}

/// The collector time under a pointer `fraction` of the way across a graph of `[end - span, end]`;
/// negative when that moment lies before the collector started.
pub fn fraction_to_time(fraction: f32, end: u64, span: u64) -> i64 {
    let f = if fraction.is_finite() { fraction.clamp(0.0, 1.0) } else { 1.0 };
    end as i64 - span as i64 + (f as f64 * span as f64).round() as i64
}

/// A readable top-of-scale for rate graphs: the next 1, 2 or 5 × 10ⁿ at or above `v`, at least `floor`.
pub fn nice_max(v: f32, floor: f32) -> f32 {
    let v = if v.is_finite() { v.max(floor) } else { floor };
    if v <= 0.0 {
        return floor;
    }
    let mag = 10f32.powi(v.log10().floor() as i32);
    [1.0, 2.0, 5.0, 10.0].into_iter().map(|m| m * mag).find(|&s| s >= v).unwrap_or(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn xs(path: &str) -> Vec<f64> {
        let t: Vec<&str> = path.split_whitespace().collect();
        t.chunks(3).filter(|c| c.len() == 3).map(|c| c[1].parse().unwrap()).collect()
    }

    #[test]
    fn newest_sample_sits_on_the_right_edge_and_x_follows_time() {
        let s = shape([(58_000, Some(0.0)), (59_000, Some(50.0)), (60_000, Some(100.0))], 60_000, 60_000, 100.0, 60);
        assert_eq!(s.line, "M 966.7 100.0 L 983.3 50.0 L 1000.0 0.0");
        assert_eq!(s.area, "M 966.7 100.0 L 966.7 100.0 L 983.3 50.0 L 1000.0 0.0 L 1000.0 100.0 Z");
    }

    #[test]
    fn a_missing_value_breaks_the_line() {
        let s = shape([(58_000, Some(10.0)), (59_000, None), (60_000, Some(10.0))], 60_000, 60_000, 100.0, 60);
        assert_eq!(s.line, "M 966.7 90.0 M 1000.0 90.0");
        assert_eq!(s.area.matches('Z').count(), 2);
    }

    #[test]
    fn sparse_samples_are_joined_not_broken() {
        // 5 s refresh on a 1 min graph with 60 columns: most columns hold no sample at all.
        let s = shape([(50_000, Some(10.0)), (55_000, Some(20.0)), (60_000, Some(30.0))], 60_000, 60_000, 100.0, 60);
        assert_eq!(s.line.matches('M').count(), 1);
    }

    #[test]
    fn a_long_pause_breaks_the_line_instead_of_crossing_it() {
        // Asleep for ten minutes between two samples one second apart each side.
        let s = shape([(1_000_000, Some(10.0)), (1_001_000, Some(10.0)), (1_601_000, Some(10.0)), (1_602_000, Some(10.0))], 1_602_000, 3_600_000, 100.0, 300);
        assert_eq!(s.line.matches('M').count(), 2, "{}", s.line);
        assert_eq!(s.area.matches('Z').count(), 2);
    }

    #[test]
    fn an_all_zero_series_draws_a_flat_line_at_the_bottom() {
        let s = shape([(59_000, Some(0.0)), (60_000, Some(0.0))], 60_000, 60_000, 0.0, 60);
        assert_eq!(s.line, "M 983.3 100.0 L 1000.0 100.0");
    }

    #[test]
    fn empty_or_flat_input_draws_safely() {
        assert_eq!(shape(std::iter::empty(), 60_000, 60_000, 100.0, 60), Shape { line: EMPTY_PATH.into(), area: EMPTY_PATH.into() });
        assert_eq!(shape([(60_000, Some(5.0))], 60_000, 60_000, 0.0, 60).line, "M 1000.0 100.0", "max 0 draws on the floor");
        assert_eq!(shape([(1, Some(f32::NAN))], 60_000, 60_000, 100.0, 60).line, EMPTY_PATH);
    }

    #[test]
    fn a_minute_of_data_in_the_hour_view_only_fills_the_right_edge() {
        let end = 4_000_000u64;
        let pts: Vec<(u64, Option<f32>)> = (0..60).map(|i| (end - 59_000 + i * 1_000, Some(50.0))).collect();
        let s = shape(pts, end, 3_600_000, 100.0, 300);
        assert!(xs(&s.line).iter().all(|&x| x >= 980.0), "{}", s.line);
    }

    #[test]
    fn many_samples_collapse_to_at_most_one_point_per_column_keeping_spikes() {
        let end = 3_600_000u64;
        let pts: Vec<(u64, Option<f32>)> = (0..3_600u64).map(|i| (i * 1_000, Some(if i == 1_800 { 100.0 } else { 0.0 }))).collect();
        let s = shape(pts, end, 3_600_000, 100.0, 300);
        assert!(xs(&s.line).len() <= 300);
        assert!(s.line.contains(" 0.0"), "the one-second spike survives downsampling: {}", &s.line[..80]);
    }

    #[test]
    fn right_after_start_the_graph_still_ends_at_the_right_edge() {
        // Collector time is only 2 s old but the range is an hour.
        let s = shape([(1_000, Some(50.0)), (2_000, Some(50.0))], 2_000, 3_600_000, 100.0, 300);
        assert_eq!(s.line, "M 1000.0 50.0");
    }

    #[test]
    fn hover_fraction_maps_to_time() {
        assert_eq!(fraction_to_time(1.0, 60_000, 60_000), 60_000);
        assert_eq!(fraction_to_time(0.5, 60_000, 60_000), 30_000);
        assert_eq!(fraction_to_time(-3.0, 60_000, 60_000), 0);
        assert_eq!(fraction_to_time(0.1, 61_000, 3_600_000), -3_179_000, "before the collector started");
    }

    #[test]
    fn scales_round_up_to_one_two_five() {
        assert_eq!(nice_max(0.0, 1_000.0), 1_000.0);
        assert_eq!(nice_max(1_300.0, 1_000.0), 2_000.0);
        assert_eq!(nice_max(7_100_000.0, 1.0), 10_000_000.0);
        assert_eq!(nice_max(f32::NAN, 5.0), 5.0);
    }
}
