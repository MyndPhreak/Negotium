//! The history scrub (spec §5.2): the top apps at the moment under the pointer on a graph, from
//! the shared History. Runs on the UI thread on pointer moves: plain data, no Windows calls.
use crate::format;
use crate::graph;
use crate::perf::{MAIN_BUCKETS, PerfFrame};
use negotium_history::History;
use negotium_model::Metric;

pub const DAY_MS: u64 = 86_400_000;
/// Where graph columns are narrower than this, an empty column takes the nearest sample this close.
pub const MIN_GAP_MS: u64 = 2_500;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScrubView {
    pub title: String,
    /// (app name, value) largest first.
    pub rows: Vec<(String, String)>,
    /// Shown instead of rows when there are none.
    pub empty: String,
}

pub fn scrub(history: &History, perf: &PerfFrame, fraction: f32) -> ScrubView {
    // The same x -> time mapping the graph is drawn with (absolute columns, newest at x = 1000).
    let t = graph::fraction_to_time(fraction, perf.window_end_ms, perf.span_ms);
    let found = at_column(history, perf, t);
    let at = found.as_ref().map_or(t, |(sample_t, _)| *sample_t as i64);
    let title = format!("Top apps at {}", format::clock(clock_at(perf, at)));
    let (rows, empty) = match (&found, perf.scrub_metric) {
        (_, Metric::Network) => (Vec::new(), "Per-app network use isn't measured yet."),
        (None, _) => (Vec::new(), "No history for this moment."),
        (Some((_, apps)), metric) => (
            apps.iter().map(|(name, v)| (name.clone(), crate::view::metric_text(*v as f64, metric))).collect(),
            "No app was busy at this moment.",
        ),
    };
    ScrubView { title, rows, empty: empty.to_string() }
}

/// The sample the main graph draws at time `t`: the one where the page's main series peaks in the
/// column holding `t` (a column shows its largest value, ruling P3-5), on the graph's own grid.
/// Columns narrower than MIN_GAP_MS (the 1 min and 10 min ranges) are often empty between samples,
/// and the line joins across them: there the nearest sample within MIN_GAP_MS stands in. A wider
/// empty column is a real gap with no history. Before the collector started there is none.
fn at_column(history: &History, perf: &PerfFrame, t: i64) -> Option<(u64, Vec<(String, f32)>)> {
    let k = graph::column_of(t, perf.span_ms, MAIN_BUCKETS);
    let (first, last) = graph::column_bounds(k, perf.span_ms, MAIN_BUCKETS);
    let found = match u64::try_from(last.min(perf.window_end_ms as i64)) {
        Ok(last) => history.top_at_max(perf.scrub_series.as_ref(), first.max(0) as u64, last, perf.scrub_metric),
        Err(_) => return None,
    };
    let narrow = perf.span_ms / (MAIN_BUCKETS as u64) < MIN_GAP_MS;
    match found {
        None if narrow => u64::try_from(t).ok().and_then(|t| history.top_at(t, perf.scrub_metric, MIN_GAP_MS)),
        found => found,
    }
}

/// Local wall-clock time (ms since midnight) of collector time `t_ms`, counted back from the
/// window's end, wrapping past midnight.
pub fn clock_at(perf: &PerfFrame, t_ms: i64) -> u64 {
    clock_before(perf.end_clock_ms, (perf.window_end_ms as i64 - t_ms).rem_euclid(DAY_MS as i64) as u64)
}

/// The local wall-clock time (ms since midnight) `back_ms` before `clock_ms`, wrapping past midnight.
pub fn clock_before(clock_ms: u64, back_ms: u64) -> u64 {
    (clock_ms % DAY_MS + DAY_MS - back_ms % DAY_MS) % DAY_MS
}

#[cfg(test)]
mod tests {
    use super::*;
    use negotium_model::{AppKey, AppKind, AppRow};
    use negotium_snapshot::SystemTotals;

    fn app(name: &str, cpu: f32, bytes: u64) -> AppRow {
        AppRow {
            key: AppKey::Background { pid: name.len() as u32, create_time: 0 }, name: name.into(), kind: AppKind::Background,
            processes: vec![], cpu_pct: cpu, private_bytes: bytes, disk_bps: 0.0, gpu_pct: None, net_bps: None, hung_for_ms: None,
        }
    }

    /// One sample a second from 1 s to 61 s; at t the CPU leader uses t/1000 %.
    fn history() -> History {
        let mut h = History::new();
        for s in 1..=61u64 {
            h.push(s * 1_000, &SystemTotals::default(), &[app("chrome", s as f32, 40 << 20), app("code", 0.5, 1 << 20)]);
        }
        h
    }

    fn perf(end: u64, span: u64, clock: u64, metric: Metric) -> PerfFrame {
        PerfFrame { window_end_ms: end, span_ms: span, end_clock_ms: clock, scrub_metric: metric, ..Default::default() }
    }

    const TWO_PM: u64 = 14 * 3_600_000;

    #[test]
    fn names_the_moment_under_the_pointer_in_local_time() {
        let v = scrub(&history(), &perf(61_000, 60_000, TWO_PM, Metric::Cpu), 0.5);
        assert_eq!(v.title, "Top apps at 13:59:30");
        assert_eq!(v.rows, vec![("chrome".to_string(), "31.0%".to_string()), ("code".to_string(), "0.5%".to_string())]);
    }

    #[test]
    fn values_are_formatted_for_the_pages_metric() {
        let v = scrub(&history(), &perf(61_000, 60_000, TWO_PM, Metric::Memory), 1.0);
        assert_eq!(v.rows[0], ("chrome".to_string(), "40.0 MB".to_string()));
    }

    #[test]
    fn the_clock_wraps_past_midnight() {
        let p = perf(61_000, 60_000, 10_000, Metric::Cpu);
        assert_eq!(format::clock(clock_at(&p, 31_000)), "23:59:40");
    }

    #[test]
    fn an_empty_stretch_of_the_hour_says_so_instead_of_borrowing_another_moment() {
        let v = scrub(&history(), &perf(61_000, 3_600_000, TWO_PM, Metric::Cpu), 0.1);
        assert!(v.rows.is_empty());
        assert_eq!(v.empty, "No history for this moment.");
        assert!(v.title.starts_with("Top apps at 13:"), "{}", v.title);
    }

    #[test]
    fn a_gap_in_the_history_is_not_filled_from_its_edges() {
        // Asleep from 61 s to 600 s: ten minutes on screen, the middle of it has no samples.
        let mut h = history();
        for s in 600..=610u64 {
            h.push(s * 1_000, &SystemTotals::default(), &[app("late", 1.0, 0)]);
        }
        let p = perf(610_000, 600_000, TWO_PM, Metric::Cpu);
        let mid = scrub(&h, &p, 0.5);
        assert!(mid.rows.is_empty(), "{:?}", mid.rows);
        assert_eq!(mid.empty, "No history for this moment.");
        assert_eq!(mid.title, "Top apps at 13:55:00");
        assert_eq!(scrub(&h, &p, 1.0).rows[0].0, "late");
    }

    /// The 1 h graph draws each 12 s column at its end with the column's largest value: the scrub
    /// over that column names the spike's moment and apps, not the sample nearest the column end.
    #[test]
    fn an_hour_column_reports_its_spike_not_the_sample_at_its_end() {
        let mut h = History::new();
        for s in 1_790..=1_830u64 {
            let spike = s == 1_806;
            let totals = SystemTotals { cpu_pct: Some(if spike { 90.0 } else { 2.0 }), ..Default::default() };
            let apps = if spike { vec![app("spike", 85.0, 1 << 20)] } else { vec![app("idle", 1.0, 1 << 20)] };
            h.push(s * 1_000, &totals, &apps);
        }
        let mut p = perf(3_600_000, 3_600_000, TWO_PM, Metric::Cpu);
        p.scrub_series = Some(negotium_history::Series::Cpu);
        // Column 150 covers (1 800 s, 1 812 s] and is drawn at its end, 1 812 s.
        let at_column_end = 1_812_000.0 / 3_600_000.0;
        let v = scrub(&h, &p, at_column_end);
        assert_eq!(v.rows.first().map(|r| r.0.as_str()), Some("spike"), "{v:?}");
        assert_eq!(v.title, format!("Top apps at {}", format::clock(clock_at(&p, 1_806_000))));
        // Anywhere in the same column gives the same moment.
        assert_eq!(scrub(&h, &p, 1_800_500.0 / 3_600_000.0), v);
    }

    #[test]
    fn network_pages_explain_the_missing_per_app_data() {
        let v = scrub(&history(), &perf(61_000, 60_000, TWO_PM, Metric::Network), 0.5);
        assert!(v.rows.is_empty());
        assert_eq!(v.empty, "Per-app network use isn't measured yet.");
    }
}
