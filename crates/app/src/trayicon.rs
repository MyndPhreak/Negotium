//! The tray icon's CPU graph (Settings > Show CPU in tray icon): the last minute of CPU use as a
//! line with a translucent fill under it, on a plate in the taskbar's own tone ringed by a
//! contrasting outline, drawn anti-aliased at the size Windows uses for tray icons (ruling P3c-5).
//! Pure: no Windows calls (crate::tray shows it).
use crate::graph::{PAUSE_BREAK_MS, column_of};

/// The time the icon covers, as the Overview sparklines do.
pub const SPAN_MS: u64 = 60_000;
/// WCAG's minimum contrast for graphics.
pub const MIN_CONTRAST: f32 = 3.0;
/// Coverage samples per pixel side: 4 × 4 give smooth edges even at 16 px.
const SS: u32 = 4;

pub type Rgb = (u8, u8, u8);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Colors {
    /// Inside the outline, in the taskbar's own tone.
    pub plate: Rgb,
    /// The outline, contrasting with the taskbar.
    pub frame: Rgb,
    /// The CPU line: the accent, adjusted to stand out from the plate.
    pub line: Rgb,
    /// Opacity of the fill under the line, in the line's colour.
    pub fill_alpha: f32,
}

const DARK_PLATE: Rgb = (0x20, 0x20, 0x20);
const LIGHT_PLATE: Rgb = (0xF3, 0xF3, 0xF3);
const DARK_FRAME: Rgb = (0x1A, 0x1A, 0x1A);
const LIGHT_FRAME: Rgb = (0xE6, 0xE6, 0xE6);

/// WCAG relative luminance.
fn luminance((r, g, b): Rgb) -> f32 {
    let channel = |c: u8| {
        let c = c as f32 / 255.0;
        if c <= 0.039_28 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
}

/// WCAG contrast ratio of two colours: 1 (none) to 21.
pub fn contrast(a: Rgb, b: Rgb) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    (m(a.0, b.0), m(a.1, b.1), m(a.2, b.2))
}

/// The icon's colours on a light or dark taskbar: the plate in the taskbar's tone, the outline in
/// the opposite one, and the accent moved toward the outline's tone, in tenths, until it stands out
/// from the plate by MIN_CONTRAST.
pub fn colors(accent: Rgb, light_taskbar: bool) -> Colors {
    let (plate, frame, toward) =
        if light_taskbar { (LIGHT_PLATE, DARK_FRAME, (0, 0, 0)) } else { (DARK_PLATE, LIGHT_FRAME, (255, 255, 255)) };
    let line = (0..=10)
        .map(|i| mix(accent, toward, i as f32 / 10.0))
        .find(|&c| contrast(c, plate) >= MIN_CONTRAST)
        .unwrap_or(toward);
    Colors { plate, frame, line, fill_alpha: 0.35 }
}

/// The largest known CPU value in each of the `n` newest columns of the absolute time grid that
/// spans SPAN_MS in `n` columns (`graph::column_of`: column k covers `(k·w, (k+1)·w]`), the last one
/// being the column holding `end` (ruling PF1: a sample keeps its column as time advances, so the
/// icon scrolls whole columns instead of shimmering). Ruling P3-5: a spike is never averaged away.
/// None where a column has no known value; later samples than `end` are ignored.
pub fn columns(points: &[(u64, Option<f32>)], end: u64, n: usize) -> Vec<Option<f32>> {
    let mut out: Vec<Option<f32>> = vec![None; n];
    if n == 0 {
        return out;
    }
    let newest = column_of(end as i64, SPAN_MS, n);
    let oldest = newest - n as i64 + 1;
    for &(t, v) in points {
        let Some(v) = v.filter(|v| v.is_finite()) else { continue };
        if t > end {
            continue;
        }
        let k = column_of(t as i64, SPAN_MS, n);
        if k < oldest {
            continue;
        }
        let slot = &mut out[(k - oldest) as usize];
        *slot = Some(slot.map_or(v, |b| b.max(v)));
    }
    out
}

/// Draws the icon: `size` × `size` pixels of straight-alpha RGBA8, row by row from the top.
pub fn render(points: &[(u64, Option<f32>)], end: u64, size: u32, c: &Colors) -> Vec<u8> {
    let s = size.max(1) as f32;
    let stroke = if size >= 32 { 2.0 } else { 1.0 }; // ruling P3c-5 / PF8
    let radius = (s * 0.2).max(2.0);
    let (g0, gw) = (stroke, s - 2.0 * stroke); // the graph area, a square inside the outline
    let n = (gw.round() as usize).max(1); // one column per pixel
    let (col_w, col_ms) = (gw / n as f32, SPAN_MS as f32 / n as f32);
    // Runs of joined columns: consecutive known columns no further apart than a pause.
    let mut runs: Vec<Vec<(f32, f32)>> = Vec::new();
    let mut last: Option<usize> = None;
    for (i, v) in columns(points, end, n).into_iter().enumerate() {
        let Some(v) = v else { continue };
        if !last.is_some_and(|l| (i - l) as f32 * col_ms <= PAUSE_BREAK_MS as f32) {
            runs.push(Vec::new());
        }
        let point = (g0 + (i as f32 + 0.5) * col_w, g0 + gw * (1.0 - v.clamp(0.0, 100.0) / 100.0));
        runs.last_mut().expect("pushed above").push(point);
        last = Some(i);
    }
    let half = (s / 16.0 * 1.25).max(1.0) / 2.0; // half the line's width
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for py in 0..size {
        for px in 0..size {
            let (mut rgb, mut covered) = ([0f32; 3], 0f32);
            for sy in 0..SS {
                for sx in 0..SS {
                    let x = px as f32 + (sx as f32 + 0.5) / SS as f32;
                    let y = py as f32 + (sy as f32 + 0.5) / SS as f32;
                    if let Some(c) = shade(x, y, s, stroke, radius, &runs, half, c) {
                        (0..3).for_each(|i| rgb[i] += c[i]);
                        covered += 1.0;
                    }
                }
            }
            let channel = |i: usize| if covered > 0.0 { (rgb[i] / covered).round().clamp(0.0, 255.0) as u8 } else { 0 };
            out.extend_from_slice(&[channel(0), channel(1), channel(2), (covered / (SS * SS) as f32 * 255.0).round() as u8]);
        }
    }
    out
}

/// The opaque colour at (x, y), or None outside the icon's rounded square.
#[allow(clippy::too_many_arguments)]
fn shade(x: f32, y: f32, s: f32, stroke: f32, radius: f32, runs: &[Vec<(f32, f32)>], half: f32, c: &Colors) -> Option<[f32; 3]> {
    let rgb = |c: Rgb| [c.0 as f32, c.1 as f32, c.2 as f32];
    if !inside_rounded(x, y, s, radius) {
        return None;
    }
    if !inside_rounded(x - stroke, y - stroke, s - 2.0 * stroke, (radius - stroke).max(0.0)) {
        return Some(rgb(c.frame));
    }
    let mut out = rgb(c.plate);
    if runs.iter().any(|r| below(r, x, y)) {
        let line = rgb(c.line);
        (0..3).for_each(|i| out[i] += (line[i] - out[i]) * c.fill_alpha);
    }
    if runs.iter().any(|r| near(r, x, y, half)) {
        out = rgb(c.line);
    }
    Some(out)
}

/// Whether (x, y) lies in the square of side `side` at the origin with corners rounded by `r`.
fn inside_rounded(x: f32, y: f32, side: f32, r: f32) -> bool {
    if x < 0.0 || y < 0.0 || x > side || y > side {
        return false;
    }
    let dx = (r - x).max(x - (side - r)).max(0.0);
    let dy = (r - y).max(y - (side - r)).max(0.0);
    dx * dx + dy * dy <= r * r
}

/// Whether (x, y) is under the run's line, within the run's extent.
fn below(run: &[(f32, f32)], x: f32, y: f32) -> bool {
    run.windows(2).any(|w| {
        let ((x0, y0), (x1, y1)) = (w[0], w[1]);
        x >= x0 && x <= x1 && y >= y0 + (y1 - y0) * (x - x0) / (x1 - x0)
    })
}

/// Whether (x, y) is within `half` of the run's line; a lone point draws a dot.
fn near(run: &[(f32, f32)], x: f32, y: f32, half: f32) -> bool {
    if let [(px, py)] = run {
        return (x - px).hypot(y - py) <= half;
    }
    run.windows(2).any(|w| distance_to_segment(x, y, w[0], w[1]) <= half)
}

fn distance_to_segment(x: f32, y: f32, (x0, y0): (f32, f32), (x1, y1): (f32, f32)) -> f32 {
    let (dx, dy) = (x1 - x0, y1 - y0);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 { (((x - x0) * dx + (y - y0) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
    (x - (x0 + t * dx)).hypot(y - (y0 + t * dy))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::column_of;

    const ACCENT: Rgb = (0x00, 0x78, 0xD4);

    fn flat(v: f32) -> Vec<(u64, Option<f32>)> {
        (0..=60u64).map(|s| (s * 1_000, Some(v))).collect()
    }

    fn pixel(img: &[u8], size: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * size + x) * 4) as usize;
        [img[i], img[i + 1], img[i + 2], img[i + 3]]
    }

    fn opaque(c: Rgb) -> [u8; 4] {
        [c.0, c.1, c.2, 255]
    }

    #[test]
    fn the_icon_has_the_size_windows_asks_for() {
        let c = colors(ACCENT, false);
        for size in [16, 20, 24, 32] {
            assert_eq!(render(&flat(40.0), 60_000, size, &c).len(), (size * size * 4) as usize, "{size} px");
        }
    }

    #[test]
    fn without_history_only_the_plate_and_its_outline_show() {
        let c = colors(ACCENT, false);
        let img = render(&[], 60_000, 32, &c);
        assert_eq!(pixel(&img, 32, 16, 16), opaque(c.plate));
        assert_eq!(pixel(&img, 32, 16, 0), opaque(c.frame), "the outline");
        assert_eq!(pixel(&img, 32, 0, 0)[3], 0, "rounded corners are transparent");
        assert!(img.chunks(4).all(|p| p[..3] != [c.line.0, c.line.1, c.line.2]), "no line");
    }

    #[test]
    fn full_and_idle_cpu_run_along_the_top_and_the_bottom() {
        let c = colors(ACCENT, false);
        let full = render(&flat(100.0), 60_000, 32, &c);
        assert_eq!(pixel(&full, 32, 16, 2), opaque(c.line), "100 % along the top of the graph");
        let idle = render(&flat(0.0), 60_000, 32, &c);
        assert_eq!(pixel(&idle, 32, 16, 29), opaque(c.line), "0 % along the bottom");
        assert_eq!(pixel(&idle, 32, 16, 16), opaque(c.plate), "nothing to fill under a 0 % line");
    }

    #[test]
    fn the_fill_shades_only_below_the_line() {
        let c = colors(ACCENT, false);
        let img = render(&flat(50.0), 60_000, 32, &c);
        assert_eq!(pixel(&img, 32, 16, 8), opaque(c.plate), "above the line: the plate");
        let fill = pixel(&img, 32, 16, 24);
        let blend = |p: u8, l: u8| p as f32 + (l as f32 - p as f32) * c.fill_alpha;
        let want = [blend(c.plate.0, c.line.0), blend(c.plate.1, c.line.1), blend(c.plate.2, c.line.2)];
        for (got, want) in fill[..3].iter().zip(want) {
            assert!((*got as f32 - want).abs() <= 1.0, "below the line: the fill {fill:?} vs {want}");
        }
        assert_eq!(fill[3], 255);
    }

    #[test]
    fn a_sloped_line_is_anti_aliased() {
        let c = colors(ACCENT, false);
        let ramp: Vec<(u64, Option<f32>)> = (0..=60u64).map(|s| (s * 1_000, Some(s as f32 * 100.0 / 60.0))).collect();
        let img = render(&ramp, 60_000, 16, &c);
        // Independently computed: the fill colour, and the range every blend of plate, fill and line lies in.
        let fill: [f32; 3] = [
            c.plate.0 as f32 + (c.line.0 as f32 - c.plate.0 as f32) * c.fill_alpha,
            c.plate.1 as f32 + (c.line.1 as f32 - c.plate.1 as f32) * c.fill_alpha,
            c.plate.2 as f32 + (c.line.2 as f32 - c.plate.2 as f32) * c.fill_alpha,
        ];
        let (plate, line) = ([c.plate.0, c.plate.1, c.plate.2], [c.line.0, c.line.1, c.line.2]);
        let mut mixed = 0;
        // Interior pixels only: inside the 1 px outline, away from the rounded corners.
        for y in 2..14 {
            for x in 2..14 {
                let p = pixel(&img, 16, x, y);
                assert_eq!(p[3], 255, "the interior is opaque");
                let is_pure = p[..3] == plate || p[..3] == line || (0..3).all(|i| (p[i] as f32 - fill[i]).abs() <= 1.0);
                if !is_pure {
                    mixed += 1;
                    for i in 0..3 {
                        let (lo, hi) = (plate[i].min(line[i]), plate[i].max(line[i]));
                        assert!(p[i] >= lo && p[i] <= hi, "a blend of plate, fill and line at ({x}, {y}): {p:?}");
                    }
                }
            }
        }
        assert!(mixed >= 8, "the sloped line's interior edge pixels are partly covered: {mixed}");
    }

    #[test]
    fn the_outline_is_one_pixel_below_32_and_two_at_32() {
        for (size, width) in [(16u32, 1u32), (20, 1), (24, 1), (32, 2)] {
            let c = colors(ACCENT, false);
            let img = render(&[], 60_000, size, &c);
            let mid = size / 2;
            for d in 0..width {
                assert_eq!(pixel(&img, size, mid, d), opaque(c.frame), "{size} px: outline row {d}");
                assert_eq!(pixel(&img, size, d, mid), opaque(c.frame), "{size} px: outline column {d}");
            }
            assert_eq!(pixel(&img, size, mid, width), opaque(c.plate), "{size} px: the plate starts after {width} px");
        }
    }

    #[test]
    fn the_graph_sits_on_the_time_grid_and_scrolls_whole_columns() {
        // A varied series, one sample a second.
        let pts: Vec<(u64, Option<f32>)> = (0..=300u64).map(|s| (s * 1_000, Some(((s * 37) % 100) as f32))).collect();
        let n = 28;
        let mut shifts = [0usize; 2];
        for end in (100_000..=130_000u64).step_by(1_000) {
            let (a, b) = (columns(&pts, end, n), columns(&pts, end + 1_000, n));
            let s = (column_of((end + 1_000) as i64, SPAN_MS, n) - column_of(end as i64, SPAN_MS, n)) as usize;
            assert!(s <= 1, "a second never moves the grid by more than a column at {n} columns");
            shifts[s] += 1;
            // Every older column of the earlier render is the same column, s places further left.
            for j in s..n - 1 {
                assert_eq!(b[j - s], a[j], "end {end}: column {j} moved by {s}");
            }
        }
        assert!(shifts[0] > 0 && shifts[1] > 0, "both a steady and a shifting second are covered: {shifts:?}");
    }

    #[test]
    fn a_render_changes_only_by_whole_columns_between_seconds() {
        // One spike on an otherwise idle series: a second later the picture is the same, moved by
        // whole pixels (one pixel per graph column) or not at all.
        let c = colors(ACCENT, false);
        let pts: Vec<(u64, Option<f32>)> = (0..=200u64).map(|s| (s * 1_000, Some(if s == 100 { 100.0 } else { 0.0 }))).collect();
        let n = 28; // 32 px less a 2 px outline each side
        let mut moved = [0; 2];
        for end in 118_000..=126_000u64 {
            if end % 1_000 != 0 {
                continue;
            }
            let (a, b) = (render(&pts, end, 32, &c), render(&pts, end + 1_000, 32, &c));
            let shift = (column_of((end + 1_000) as i64, SPAN_MS, n) - column_of(end as i64, SPAN_MS, n)) as u32;
            moved[shift as usize] += 1;
            // The spike has not reached an edge; compare the middle of the graph pixel by pixel.
            for y in 2..30 {
                for x in 9..=24 {
                    assert_eq!(pixel(&a, 32, x, y), pixel(&b, 32, x - shift, y), "end {end}, ({x}, {y}), shift {shift}");
                }
            }
        }
        assert!(moved[0] > 0 && moved[1] > 0, "a steady and a shifting second both occur: {moved:?}");
    }

    #[test]
    fn a_pause_breaks_the_line() {
        let c = colors(ACCENT, false);
        // Two bursts of 100 % with 40 s of nothing between them: the middle of the graph has no line.
        let pts: Vec<(u64, Option<f32>)> = (0..=10u64).chain(50..=60).map(|s| (s * 1_000, Some(100.0))).collect();
        let img = render(&pts, 60_000, 32, &c);
        assert_eq!(pixel(&img, 32, 6, 2), opaque(c.line));
        assert_eq!(pixel(&img, 32, 25, 2), opaque(c.line));
        assert_eq!(pixel(&img, 32, 16, 2), opaque(c.plate), "no line across the gap");
        assert_eq!(pixel(&img, 32, 16, 29), opaque(c.plate), "nor along the floor");
    }

    #[test]
    fn the_line_contrasts_with_the_plate_whatever_the_accent_and_taskbar() {
        for accent in [(0x00, 0x78, 0xD4), (0x00, 0x1B, 0x3D), (0x1F, 0x1F, 0x1F), (0xFF, 0xF4, 0x9C), (0xFF, 0xFF, 0xFF), (0xE8, 0x11, 0x23)] {
            for light in [false, true] {
                let c = colors(accent, light);
                assert!(contrast(c.line, c.plate) >= MIN_CONTRAST, "{accent:?}, light taskbar {light}: {c:?}");
                assert!(contrast(c.frame, c.plate) >= 4.5, "the outline stands out: {c:?}");
            }
        }
        // Near the taskbar's own tone: navy on a dark taskbar, pale yellow on a light one.
        let navy = colors((0x00, 0x1B, 0x3D), false);
        assert_ne!(navy.line, (0x00, 0x1B, 0x3D), "navy had to move");
        assert!(luminance(navy.line) > luminance(navy.plate), "toward white on a dark taskbar");
        let yellow = colors((0xFF, 0xF4, 0x9C), true);
        assert_ne!(yellow.line, (0xFF, 0xF4, 0x9C), "pale yellow had to move");
        assert!(luminance(yellow.line) < luminance(yellow.plate), "toward black on a light taskbar");
        // Independent anchors for the maths: black on white is 21:1, equal colours 1:1.
        assert!((contrast((0, 0, 0), (255, 255, 255)) - 21.0).abs() < 0.01);
        assert!((contrast(ACCENT, ACCENT) - 1.0).abs() < 1e-6);
        assert_eq!(colors(ACCENT, false).line, ACCENT, "an accent that already stands out is kept");
    }

    #[test]
    fn each_column_keeps_its_largest_value_and_gaps_stay_empty() {
        let pts = [(1_000, Some(5.0)), (2_000, Some(90.0)), (3_000, None), (59_000, Some(f32::NAN)), (60_000, Some(20.0)), (70_000, Some(99.0))];
        assert_eq!(columns(&pts, 60_000, 6), [Some(90.0), None, None, None, None, Some(20.0)], "spikes survive; unknown and future values are skipped");
        assert!(columns(&pts, 60_000, 0).is_empty());
    }
}
