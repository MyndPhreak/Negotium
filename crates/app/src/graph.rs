//! Graph geometry: (time, value) series as Slint path commands for a 1000 × 100 view box. x is
//! time, so a graph always spans its whole range with the newest sample on the right edge; a
//! range with little history draws only where history exists (never stretched).
use std::fmt::Write;

/// Two consecutive samples further apart than this are not joined: after a sleep or hibernate the
/// line breaks instead of crossing the missing period. Well above the slowest normal tick (the
/// hidden-window 2 s interval, and the 5 s refreshes of sparse graphs), far below any real pause.
pub const PAUSE_BREAK_MS: u64 = 10_000;

/// History drawn left of a graph's window, off the edge (the Path clips it). The scrolling view can
/// trail its newest sample by up to `crate::smooth::SCROLL_MARGIN_MS`, and one pause further back
/// always takes in the sample before that, so the line reaches the left edge wherever the view is.
///
/// The 1 h graph can still fall short of its left edge by at most one 12 s column: history keeps
/// exactly 1 h (ruling PF9), so there is nothing older to draw there.
pub const LEAD_MS: u64 = crate::smooth::SCROLL_MARGIN_MS + PAUSE_BREAK_MS;

/// The graphs' right edge: the newest history sample, `fallback` (the tick) while there is none. At
/// refresh rates under a second the history keeps one sample a second (ruling P3-3), so a tick can
/// be newer; anchoring on it would pull every line's end in from the edge.
pub fn end_of(history: &negotium_history::History, fallback: u64) -> u64 {
    history.latest_t().unwrap_or(fallback)
}

/// The earliest sample time a graph of `[end - span, end]` draws (see [`LEAD_MS`]).
pub fn since(end: u64, span: u64) -> u64 {
    end.saturating_sub(span + LEAD_MS)
}

pub const VIEW_W: f64 = 1000.0;
/// A path that draws nothing (Slint needs a valid command string).
pub const EMPTY_PATH: &str = "M 0 100";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shape {
    pub line: String,
    /// The line closed down to the floor, for a translucent fill.
    pub area: String,
}

/// Width given to a one-point segment so it still draws (a bare `M` draws nothing).
const STUB: f64 = 0.1;

#[derive(Default)]
struct Pen {
    line: String,
    area: String,
    /// Build the filled outline too; sparklines only need the line.
    with_area: bool,
    down: bool,
    last_x: f64,
    last_y: f64,
    /// Points in the current segment.
    count: usize,
}

impl Pen {
    fn point(&mut self, x: f64, y: f64) {
        if !self.line.is_empty() {
            self.line.push(' ');
        }
        let _ = write!(self.line, "{} {x:.1} {y:.1}", if self.down { "L" } else { "M" });
        if self.with_area {
            if self.down {
                let _ = write!(self.area, " L {x:.1} {y:.1}");
            } else {
                if !self.area.is_empty() {
                    self.area.push(' ');
                }
                let _ = write!(self.area, "M {x:.1} 100.0 L {x:.1} {y:.1}");
            }
        }
        self.down = true;
        self.count += 1;
        self.last_x = x;
        self.last_y = y;
    }

    fn gap(&mut self) {
        if !self.down {
            return;
        }
        if self.count == 1 {
            // Toward the inside of the view box: a stub at the right edge would leave it.
            let x = if self.last_x + STUB > VIEW_W { self.last_x - STUB } else { self.last_x + STUB };
            let _ = write!(self.line, " L {x:.1} {:.1}", self.last_y);
        }
        if self.with_area {
            let _ = write!(self.area, " L {:.1} 100.0 Z", self.last_x);
        }
        self.down = false;
        self.count = 0;
    }

    fn finish(mut self) -> Shape {
        self.gap();
        let or_empty = |s: String| if s.is_empty() { EMPTY_PATH.to_string() } else { s };
        Shape { line: or_empty(self.line), area: or_empty(self.area) }
    }
}

/// Draws `points` in the window `[end - span, end]` merged into columns of `span / buckets` ms
/// laid on an absolute time grid (column k covers `(k·w, (k+1)·w]`), so a sample stays in its column
/// while time advances and the graph scrolls instead of shimmering. A column shows its largest value
/// (ruling P3-5) at its end time (the newest column at `end`, the right edge); a column holding only
/// missing values breaks the line, and columns without any sample are simply skipped. History up to
/// [`LEAD_MS`] before the window is drawn too, at negative x (off the edge; the Path clips it), so
/// the line reaches the left edge. `end` is the newest sample: anchoring on a later moment would pull
/// the line's end in from the right edge. `max <= 0` draws values on the floor.
pub fn shape(points: impl IntoIterator<Item = (u64, Option<f32>)>, end: u64, span: u64, max: f32, buckets: usize) -> Shape {
    trace(points, end, span, max, buckets, true)
}

/// Like [`shape`] but only the line (the area stays empty): for sparklines.
pub fn line(points: impl IntoIterator<Item = (u64, Option<f32>)>, end: u64, span: u64, max: f32, buckets: usize) -> String {
    trace(points, end, span, max, buckets, false).line
}

fn trace(points: impl IntoIterator<Item = (u64, Option<f32>)>, end: u64, span: u64, max: f32, buckets: usize, with_area: bool) -> Shape {
    let span = span.max(1);
    let buckets = buckets.max(1);
    // Negative right after start: the window then reaches back before collector time 0, and the
    // little history there is still drawn at the right edge.
    let start = end as i64 - span as i64;
    // History left of the window is drawn too, off the edge (see LEAD_MS).
    let first = start - LEAD_MS as i64;
    let mut pen = Pen { with_area, ..Pen::default() };
    let mut current: Option<(i64, Option<f32>)> = None;
    let mut prev_t: Option<i64> = None;
    let emit = |pen: &mut Pen, k: i64, best: Option<f32>| match best {
        Some(v) => {
            // The column's end time, but never past the newest moment.
            let col_end = ((k + 1) as f64 * span as f64 / buckets as f64).min(end as f64);
            let x = time_to_x(col_end, end, span);
            let y = if max > 0.0 { 100.0 - (v / max).clamp(0.0, 1.0) as f64 * 100.0 } else { 100.0 };
            pen.point(x, y);
        }
        None => pen.gap(),
    };
    for (t, v) in points {
        let t = t as i64;
        if t < first || t > end as i64 {
            continue;
        }
        if prev_t.is_some_and(|p| t - p > PAUSE_BREAK_MS as i64) {
            if let Some((cur, best)) = current.take() {
                emit(&mut pen, cur, best);
            }
            pen.gap();
        }
        prev_t = Some(t);
        let k = column_of(t, span, buckets);
        let v = v.filter(|v| v.is_finite());
        match &mut current {
            Some((cur, best)) if *cur == k => {
                if let Some(v) = v {
                    *best = Some(best.map_or(v, |b| b.max(v)));
                }
            }
            _ => {
                if let Some((cur, best)) = current.take() {
                    emit(&mut pen, cur, best);
                }
                current = Some((k, v));
            }
        }
    }
    if let Some((cur, best)) = current {
        emit(&mut pen, cur, best);
    }
    pen.finish()
}

/// The column of the absolute grid [`shape`] draws on that holds collector time `t`: column k
/// covers `(k·w, (k+1)·w]` with `w = span / buckets`.
pub fn column_of(t: i64, span: u64, buckets: usize) -> i64 {
    let (n, d) = (t * buckets.max(1) as i64, span.max(1) as i64);
    // ceil(n / d) - 1, for a `t` before collector time 0 too.
    n.div_euclid(d) - i64::from(n.rem_euclid(d) == 0)
}

/// The first and last whole millisecond of column `k` (see [`column_of`]), inclusive.
pub fn column_bounds(k: i64, span: u64, buckets: usize) -> (i64, i64) {
    let (span, buckets) = (span.max(1) as i64, buckets.max(1) as i64);
    ((k * span).div_euclid(buckets) + 1, ((k + 1) * span).div_euclid(buckets))
}

/// Where collector time `t` is drawn on a graph of `[end - span, end]`: x in the view box, the
/// newest moment on the right edge; negative left of the window (history drawn off the edge, see
/// [`LEAD_MS`]). [`fraction_to_time`] is its inverse inside the window.
fn time_to_x(t: f64, end: u64, span: u64) -> f64 {
    (VIEW_W - (end as f64 - t) / span as f64 * VIEW_W).min(VIEW_W)
}

/// The collector time under a pointer `fraction` of the way across a graph of `[end - span, end]`
/// (the inverse of where [`shape`] draws a time, so the scrub reads back the moment drawn under the
/// pointer); negative when that moment lies before the collector started.
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
    fn column_bounds_are_exactly_the_times_column_of_maps_to_the_column() {
        for (span, buckets) in [(60_000u64, 300usize), (600_000, 300), (3_600_000, 300), (60_000, 60), (1_000, 3)] {
            for k in [-2i64, -1, 0, 1, 7, 150, 299] {
                let (first, last) = column_bounds(k, span, buckets);
                assert!(first <= last, "{span}/{buckets} column {k}: {first}..{last}");
                assert_eq!((column_of(first, span, buckets), column_of(last, span, buckets)), (k, k));
                assert_eq!((column_of(first - 1, span, buckets), column_of(last + 1, span, buckets)), (k - 1, k + 1));
            }
        }
        assert_eq!(column_bounds(150, 3_600_000, 300), (1_800_001, 1_812_000));
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
        assert_eq!(s.line, "M 966.7 90.0 L 966.8 90.0 M 1000.0 90.0 L 999.9 90.0", "single points keep a visible stub");
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
        // Asleep for ten minutes between two samples one second apart each side; one column per second.
        let s = shape([(1_000_000, Some(10.0)), (1_001_000, Some(10.0)), (1_601_000, Some(10.0)), (1_602_000, Some(10.0))], 1_602_000, 3_600_000, 100.0, 3_600);
        assert_eq!(s.line, "M 832.8 90.0 L 833.1 90.0 M 999.7 90.0 L 1000.0 90.0");
        assert_eq!(s.area.matches('Z').count(), 2);
    }

    #[test]
    fn a_lone_point_gets_a_visible_stub_and_flat_data_draws_a_line() {
        let one = shape([(60_000, Some(5.0))], 60_000, 60_000, 100.0, 60);
        assert_eq!(one.line, "M 1000.0 95.0 L 999.9 95.0");
        let zeros = shape([(58_000, Some(0.0)), (59_000, Some(0.0)), (60_000, Some(0.0))], 60_000, 60_000, 100.0, 60);
        assert_eq!(zeros.line, "M 966.7 100.0 L 983.3 100.0 L 1000.0 100.0");
        assert_eq!(shape([(50_000, Some(0.0)), (60_000, Some(0.0))], 60_000, 60_000, 0.0, 60).line, "M 833.3 100.0 L 1000.0 100.0");
    }

    #[test]
    fn columns_stay_put_while_time_advances() {
        // The same samples a quarter second apart in frame time: same columns, shifted left smoothly.
        let pts: Vec<(u64, Option<f32>)> = (0..60u64).map(|i| (i * 1_000 + 500, Some(i as f32))).collect();
        let a = shape(pts.iter().copied().filter(|p| p.0 <= 60_000), 60_000, 60_000, 100.0, 60);
        let b = shape(pts.iter().copied().filter(|p| p.0 <= 60_000), 60_250, 60_000, 100.0, 60);
        let (xa, xb) = (xs(&a.line), xs(&b.line));
        assert_eq!(xa.len(), xb.len(), "no sample hops to another column");
        for (a, b) in xa.iter().zip(&xb).take(xa.len() - 1) {
            assert!((a - b - 250.0 / 60.0).abs() < 0.11, "{a} -> {b}");
        }
    }

    #[test]
    fn the_line_variant_matches_shape_without_building_the_area() {
        let pts = [(58_000, Some(0.0)), (59_000, Some(50.0)), (60_000, Some(100.0))];
        assert_eq!(line(pts, 60_000, 60_000, 100.0, 60), shape(pts, 60_000, 60_000, 100.0, 60).line);
        assert_eq!(trace(pts, 60_000, 60_000, 100.0, 60, false).area, EMPTY_PATH);
    }

    #[test]
    fn an_all_zero_series_draws_a_flat_line_at_the_bottom() {
        let s = shape([(59_000, Some(0.0)), (60_000, Some(0.0))], 60_000, 60_000, 0.0, 60);
        assert_eq!(s.line, "M 983.3 100.0 L 1000.0 100.0");
    }

    #[test]
    fn empty_or_flat_input_draws_safely() {
        assert_eq!(shape(std::iter::empty(), 60_000, 60_000, 100.0, 60), Shape { line: EMPTY_PATH.into(), area: EMPTY_PATH.into() });
        assert_eq!(shape([(60_000, Some(5.0))], 60_000, 60_000, 0.0, 60).line, "M 1000.0 100.0 L 999.9 100.0", "max 0 draws on the floor");
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
        assert!(xs(&s.line).len() <= 2 * 301, "at most a column more than asked, one point (plus a stub) each");
        assert!(s.line.contains(" 0.0"), "the one-second spike survives downsampling: {}", &s.line[..80]);
    }

    #[test]
    fn right_after_start_the_graph_still_ends_at_the_right_edge() {
        // Collector time is only 2 s old but the range is an hour.
        let s = shape([(1_000, Some(50.0)), (2_000, Some(50.0))], 2_000, 3_600_000, 100.0, 300);
        assert_eq!(s.line, "M 1000.0 50.0 L 999.9 50.0");
    }

    #[test]
    fn hover_fraction_maps_to_time() {
        assert_eq!(fraction_to_time(1.0, 60_000, 60_000), 60_000);
        assert_eq!(fraction_to_time(0.5, 60_000, 60_000), 30_000);
        assert_eq!(fraction_to_time(-3.0, 60_000, 60_000), 0);
        assert_eq!(fraction_to_time(0.1, 61_000, 3_600_000), -3_179_000, "before the collector started");
    }

    #[test]
    fn the_scrub_reads_back_the_time_each_column_was_drawn_at() {
        // An hour graph of 300 columns (12 s each), at a moment between column boundaries.
        let (end, span, buckets) = (4_005_500u64, 3_600_000u64, 300usize);
        let w = span / buckets as u64;
        let pts: Vec<(u64, Option<f32>)> = (406..=end / 1_000).map(|s| (s * 1_000, Some(50.0))).collect();
        let s = shape(pts, end, span, 100.0, buckets);
        for x in xs(&s.line) {
            let t = fraction_to_time((x / VIEW_W) as f32, end, span);
            // Path coordinates keep one decimal (span / 10 000 ms); a lone point's stub adds 0.1.
            let to_col_end = (t as u64 % w).min(w - t as u64 % w);
            assert!(t as u64 == end || to_col_end <= 2 * span / 10_000, "x {x} -> {t} ms, {to_col_end} ms off a column end");
        }
        assert_eq!(fraction_to_time((time_to_x(end as f64, end, span) / VIEW_W) as f32, end, span), end as i64, "right edge");
        for t in [410_000u64, 1_234_000, 3_999_000] {
            let back = fraction_to_time((time_to_x(t as f64, end, span) / VIEW_W) as f32, end, span);
            assert!((back - t as i64).abs() <= 1, "{t} -> {back}");
        }
    }

    #[test]
    fn scales_round_up_to_one_two_five() {
        assert_eq!(nice_max(0.0, 1_000.0), 1_000.0);
        assert_eq!(nice_max(1_300.0, 1_000.0), 2_000.0);
        assert_eq!(nice_max(7_100_000.0, 1.0), 10_000_000.0);
        assert_eq!(nice_max(f32::NAN, 5.0), 5.0);
    }

    #[test]
    fn the_sample_before_the_window_carries_the_line_to_the_left_edge() {
        // One sample a second, half a second off the column grid; the window is [1.5 s, 61.5 s].
        let pts: Vec<(u64, Option<f32>)> = (0..=61u64).map(|i| (i * 1_000 + 500, Some(50.0))).collect();
        let s = shape(pts, 61_500, 60_000, 100.0, 60);
        let x = xs(&s.line);
        assert!(x[0] < 0.0 && x[1] > 0.0, "the line enters from beyond the left edge: {}", &s.line[..30]);
        assert_eq!(s.line.matches('M').count(), 1, "one unbroken line");
        assert!(s.area.starts_with(&format!("M {:.1} 100.0", x[0])), "the fill starts with it: {}", &s.area[..30]);
        assert_eq!(*x.last().unwrap(), 1000.0);
    }

    #[test]
    fn history_further_back_than_the_lead_in_is_not_drawn() {
        let (end, span) = (200_000u64, 60_000u64);
        // One stray sample just before `since`, then one a second from the window's start: the stray is
        // further back than a pause from the rest, so drawing it would start a second segment.
        let mut pts = vec![(since(end, span) - 1_000, Some(50.0))];
        pts.extend((end - span + 1_000..=end).step_by(1_000).map(|t| (t, Some(50.0))));
        let s = shape(pts, end, span, 100.0, 60);
        assert_eq!(s.line.matches('M').count(), 1, "{}", &s.line[..40]);
        assert_eq!(since(end, span), end - span - LEAD_MS);
        assert_eq!(since(5_000, 60_000), 0);
    }

    #[test]
    fn the_scrub_maps_a_drawn_column_back_to_its_time() {
        let (end, span) = (61_000u64, 60_000u64);
        let pts: Vec<(u64, Option<f32>)> = (0..=61u64).map(|i| (i * 1_000, Some(50.0))).collect();
        let x = xs(&shape(pts, end, span, 100.0, 60).line);
        let drawn = x[x.len() - 11]; // the column ending 10 s before the newest sample
        let t = fraction_to_time((drawn / VIEW_W) as f32, end, span);
        assert!((t - 51_000).abs() <= 2, "{t}");
    }
}
