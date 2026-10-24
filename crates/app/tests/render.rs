//! Rendering checks on real pixels (the Slint software renderer): data paths fill their graph,
//! and a release-only bench of Performance page frame times.
use i_slint_backend_testing::{ElementHandle, TestingBackend, TestingBackendOptions};
use negotium::{GraphData, MainWindow, Theme};
use slint::{ComponentHandle, ModelRc, PhysicalSize, VecModel};

/// The testing backend with a real software rasterizer, so `take_snapshot` draws pixels.
fn software_backend() {
    slint::platform::set_platform(Box::new(TestingBackend::new(TestingBackendOptions {
        renderer_name: Some("software".into()),
        ..Default::default()
    })))
    .expect("one backend per test thread");
}

/// A pixel tinted by the (pure red) test accent, even at the area fill's 16% alpha.
fn accent_tinted(p: slint::Rgba8Pixel) -> bool {
    p.r as i32 - p.g as i32 > 20 && p.r as i32 - p.b as i32 > 20
}

#[test]
fn a_graphs_data_fills_its_whole_height() {
    software_backend();
    let ui = MainWindow::new().unwrap();
    ui.global::<Theme>().set_accent(slint::Color::from_rgb_u8(255, 0, 0));
    ui.window().set_size(PhysicalSize::new(1000, 720));
    ui.set_page(1);
    // 0% over the left half, 100% over the right half (a step, so the 100% part is not just the
    // top-right corner the graph's rounded clip cuts off).
    let g = GraphData {
        label: "CPU".into(), value: "".into(), scale: "".into(), line: "".into(), line2: "".into(),
        area: "M 0 100 L 500 100 L 500 0 L 1000 0 L 1000 100 Z".into(),
    };
    ui.set_perf_graphs(ModelRc::new(VecModel::from(vec![g])));
    ui.show().unwrap();
    let graph = ElementHandle::find_by_accessible_label(&ui, "CPU graph").next().expect("graph");
    let (pos, size) = (graph.absolute_position(), graph.size());
    let shot = ui.window().take_snapshot().expect("snapshot");
    let (x0, y0) = (pos.x.round() as u32, pos.y.round() as u32);
    let (w, h) = (size.width.round() as u32, size.height.round() as u32);
    assert!(h > 100, "a tall graph ({h} px) is what shows the squash");
    let tinted_in = |rows: std::ops::Range<u32>| {
        rows.flat_map(|y| (x0 + 6..x0 + w - 6).map(move |x| (x, y)))
            .filter(|&(x, y)| accent_tinted(shot.as_slice()[(y * shot.width() + x) as usize]))
            .count()
    };
    let (top, bottom) = (tinted_in(y0..y0 + 4), tinted_in(y0 + h - 4..y0 + h));
    assert!(top > 0, "the 100% sample reaches the graph's top rows");
    assert!(bottom > 0, "the area reaches the graph's bottom rows");
}

/// A large graph's line along the floor or the ceiling keeps its full stroke inside the card: the
/// data is inset from the card's clip, which would otherwise trim it to half.
#[test]
fn a_graphs_floor_and_ceiling_lines_keep_their_full_stroke_inside_the_card() {
    software_backend();
    let ui = MainWindow::new().unwrap();
    ui.global::<Theme>().set_accent(slint::Color::from_rgb_u8(255, 0, 0));
    ui.window().set_size(PhysicalSize::new(1000, 720));
    ui.set_page(1);
    let line = |label: &str, path: &str| GraphData { label: label.into(), value: "".into(), scale: "".into(), line: path.into(), line2: "".into(), area: "".into() };
    ui.set_perf_graphs(ModelRc::new(VecModel::from(vec![line("Floor", "M 0 100 L 1000 100"), line("Ceiling", "M 0 0 L 1000 0")])));
    ui.show().unwrap();
    let shot = ui.window().take_snapshot().expect("snapshot");
    let rows = |label: &str| -> Vec<u32> {
        let e = ElementHandle::find_by_accessible_label(&ui, label).next().unwrap_or_else(|| panic!("missing {label}"));
        let (p, s) = (e.absolute_position(), e.size());
        let (x0, w) = (p.x.round() as u32 + 20, s.width.round() as u32 - 40);
        (p.y.round() as u32 + 1..(p.y + s.height).round() as u32 - 1)
            .filter(|&y| (x0..x0 + w).any(|x| accent_tinted(shot.as_slice()[(y * shot.width() + x) as usize])))
            .collect()
    };
    let (floor, ceiling) = (rows("Floor graph"), rows("Ceiling graph"));
    assert!(floor.len() >= 2, "floor rows {floor:?}");
    assert!(ceiling.len() >= 2, "ceiling rows {ceiling:?}");
}

/// The small multiples draw only their line: the area fill, under every cell, made each resize
/// frame of a many-core Performance page slow in the software renderer.
#[test]
fn graph_cells_draw_no_area_while_the_main_graph_does() {
    software_backend();
    let ui = MainWindow::new().unwrap();
    ui.global::<Theme>().set_accent(slint::Color::from_rgb_u8(255, 0, 0));
    ui.window().set_size(PhysicalSize::new(1000, 720));
    ui.set_page(1);
    let area_only = |label: &str| GraphData {
        label: label.into(), value: "".into(), scale: "".into(), line: "".into(), line2: "".into(),
        area: "M 0 100 L 0 0 L 1000 0 L 1000 100 Z".into(),
    };
    ui.set_perf_graphs(ModelRc::new(VecModel::from(vec![area_only("Main")])));
    ui.set_perf_cells(ModelRc::new(VecModel::from(vec![area_only("CPU 0")])));
    ui.show().unwrap();
    let shot = ui.window().take_snapshot().expect("snapshot");
    let tinted = |label: &str| {
        let e = ElementHandle::find_by_accessible_label(&ui, label).next().unwrap_or_else(|| panic!("missing {label}"));
        let (p, s) = (e.absolute_position(), e.size());
        let (x0, y0) = (p.x.round() as u32, p.y.round() as u32);
        let (w, h) = (s.width.round() as u32, s.height.round() as u32);
        (y0 + 4..y0 + h - 4)
            .flat_map(|y| (x0 + 4..x0 + w - 4).map(move |x| (x, y)))
            .filter(|&(x, y)| accent_tinted(shot.as_slice()[(y * shot.width() + x) as usize]))
            .count()
    };
    assert!(tinted("Main graph") > 0, "the main graph keeps its area");
    assert_eq!(tinted("CPU 0 graph"), 0, "a cell draws no area");
}

/// Slint source with `/* */` and `//` comments removed and all whitespace dropped, so the checks
/// below see `Path{`, `viewbox-width:1000;` and `fit:fill;` however the source is laid out.
fn squeezed(src: &str) -> String {
    let mut out = String::new();
    let mut rest = src;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix("/*") {
            rest = after.find("*/").map_or("", |end| &after[end + 2..]);
        } else if let Some(after) = rest.strip_prefix("//") {
            rest = after.find('\n').map_or("", |end| &after[end..]);
        } else {
            let c = rest.chars().next().unwrap();
            if !c.is_whitespace() {
                out.push(c);
            }
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}

fn is_ident(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_'
}

/// Whether `body` sets the property `name` to exactly one of `values` (squeezed source).
fn sets(body: &str, name: &str, values: &[&str]) -> bool {
    let key = format!("{name}:");
    body.match_indices(&key).any(|(at, _)| {
        let before_ok = body[..at].chars().next_back().is_none_or(|c| !is_ident(c));
        let value = body[at + key.len()..].split(';').next().unwrap_or("");
        before_ok && values.contains(&value)
    })
}

/// The bodies of every `Path { … }` element in squeezed source.
fn path_bodies(src: &str) -> Vec<&str> {
    let mut bodies = Vec::new();
    for (at, _) in src.match_indices("Path{") {
        if src[..at].chars().next_back().is_some_and(is_ident) {
            continue; // e.g. `MyPath{`
        }
        let body_start = at + "Path{".len();
        let mut depth = 1;
        for (i, c) in src[body_start..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => depth -= 1,
                _ => {}
            }
            if depth == 0 {
                bodies.push(&src[body_start..body_start + i]);
                break;
            }
        }
    }
    bodies
}

#[test]
fn the_path_scanner_sees_through_layout_and_comments() {
    let src = squeezed("Path
{
 viewbox-width : 1000.0 ;
 // fit: fill;
}
MyPath { viewbox-width: 1000; }");
    let bodies = path_bodies(&src);
    assert_eq!(bodies.len(), 1, "MyPath is not a Path");
    assert!(sets(bodies[0], "viewbox-width", &["1000", "1000.0"]));
    assert!(!sets(bodies[0], "fit", &["fill"]), "a commented-out fit does not count");
}

#[test]
fn block_comments_are_stripped_too() {
    let src = squeezed("Path { viewbox-width: 1000; /* fit: fill;\n still comment */ }\n/* Path { } */");
    let bodies = path_bodies(&src);
    assert_eq!(bodies.len(), 1, "a Path inside a block comment does not count");
    assert!(sets(bodies[0], "viewbox-width", &["1000"]));
    assert!(!sets(bodies[0], "fit", &["fill"]), "a fit inside a block comment does not count");
}

/// Every data path (a 1000 × 100 view box, see crate::graph) in ui/*.slint: (file, squeezed body).
fn data_paths() -> Vec<(String, String)> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui");
    let mut found = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "slint") {
            continue;
        }
        let src = squeezed(&std::fs::read_to_string(&path).unwrap());
        for body in path_bodies(&src) {
            if sets(body, "viewbox-width", &["1000", "1000.0"]) {
                found.push((path.display().to_string(), body.to_string()));
            }
        }
    }
    found
}

/// Every data path stretches to its box: the default `fit: contain` keeps the 10:1 aspect and
/// squashes the data into a band. Every data Path lives in the one `DataPath` component.
#[test]
fn every_data_path_stretches_to_fill_its_box() {
    let paths = data_paths();
    assert_eq!(paths.len(), 1, "data Paths belong in DataPath only: {:?}", paths.iter().map(|p| &p.0).collect::<Vec<_>>());
    for (file, body) in &paths {
        assert!(sets(body, "fit", &["fill"]), "{file}: a data Path without fit: fill: {body}");
    }
}

/// `DataPath` wraps its Path in a clip rectangle (history left of the window is drawn at negative x,
/// crate::graph::LEAD_MS, and must never paint outside the graph). The pixel tests below check it.
#[test]
fn the_data_path_sits_in_a_clip_rectangle() {
    let src = squeezed(&std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui/graph.slint")).unwrap());
    assert!(src.contains("exportcomponentDataPathinheritsRectangle{"), "DataPath is missing");
    assert!(src.contains("clip:true;Path{"), "the data Path is not directly inside a clip rectangle");
}

/// Every data path scrolls with GraphClock (crate::smooth): one that didn't would stand still
/// while the others move.
#[test]
fn every_data_path_scrolls_with_the_graph_clock() {
    for (file, body) in &data_paths() {
        assert!(body.contains("viewbox-x:"), "{file}: a data Path without viewbox-x: {body}");
        assert!(body.contains("GraphClock.offset-ms"), "{file}: viewbox-x is not bound to GraphClock.offset-ms: {body}");
    }
}

/// Where across the "CPU graph" its tinted line is, as a fraction of the graph's width.
fn line_position(ui: &MainWindow) -> f32 {
    let shot = ui.window().take_snapshot().expect("snapshot");
    let graph = ElementHandle::find_by_accessible_label(ui, "CPU graph").next().expect("graph");
    let (p, s) = (graph.absolute_position(), graph.size());
    let y = (p.y + s.height * 3.0 / 8.0).round() as u32; // between two grid lines
    let xs: Vec<f32> = (p.x.round() as u32 + 2..(p.x + s.width).round() as u32 - 2)
        .filter(|&x| accent_tinted(shot.as_slice()[(y * shot.width() + x) as usize]))
        .map(|x| x as f32)
        .collect();
    assert!(!xs.is_empty(), "the line is drawn");
    (xs.iter().sum::<f32>() / xs.len() as f32 - p.x) / s.width
}

/// The scrolling offset moves what is drawn (ruling PF7): the same paths, a later view.
#[test]
fn a_graph_scrolls_with_the_graph_clock() {
    software_backend();
    let ui = MainWindow::new().unwrap();
    ui.global::<Theme>().set_accent(slint::Color::from_rgb_u8(255, 0, 0));
    ui.window().set_size(PhysicalSize::new(1000, 720));
    ui.set_page(1);
    // A vertical line half a minute before the newest sample (x = 500 of 1000).
    let g = GraphData {
        label: "CPU".into(), value: "".into(), scale: "".into(), line: "M 500 0 L 500 100".into(),
        area: "M 0 100".into(), line2: "M 0 100".into(),
    };
    ui.set_perf_graphs(ModelRc::new(VecModel::from(vec![g])));
    ui.show().unwrap();
    let at_rest = line_position(&ui);
    assert!((at_rest - 0.5).abs() < 0.02, "the newest sample on the edge: {at_rest}");
    // The view's right edge 15 s behind the newest sample: the line sits three quarters across.
    ui.invoke_scroll_from(-15_000.0, 1.0);
    let scrolled = line_position(&ui);
    assert!((scrolled - 0.75).abs() < 0.02, "{scrolled}");
}

/// Meters (CPU, Memory, Disk, GPU, Network) drawn with these sparklines, rendered: per meter the
/// rows (absolute y) tinted by the accent away from its edges, and how many pixels beside the first
/// meter, in the gap to the second, are tinted.
fn render_sparks(sparks: [&str; 5]) -> (Vec<Vec<u32>>, usize) {
    software_backend();
    let ui = MainWindow::new().unwrap();
    ui.global::<Theme>().set_accent(slint::Color::from_rgb_u8(255, 0, 0));
    ui.window().set_size(PhysicalSize::new(1000, 720));
    let labels = ["CPU", "Memory", "Disk", "GPU", "Network"];
    let meters: Vec<negotium::MeterData> = labels
        .iter()
        .zip(sparks)
        .map(|(l, s)| negotium::MeterData { label: (*l).into(), value: "1%".into(), sub: "".into(), spark: s.into(), available: true })
        .collect();
    ui.set_meters(ModelRc::new(VecModel::from(meters)));
    ui.set_selected_metric(-1); // no meter is tinted by being selected
    ui.show().unwrap();
    let shot = ui.window().take_snapshot().expect("snapshot");
    let find = |l: &str| ElementHandle::find_by_accessible_label(&ui, &format!("{l} meter")).next().expect("meter");
    let tinted = |y0: f32, y1: f32, x0: f32, x1: f32| -> Vec<u32> {
        (y0.round() as u32..y1.round() as u32)
            .filter(|&y| (x0.round() as u32..x1.round() as u32).any(|x| accent_tinted(shot.as_slice()[(y * shot.width() + x) as usize])))
            .collect()
    };
    let rows = labels
        .iter()
        .map(|l| {
            let (e, s) = (find(l), find(l).size());
            let p = e.absolute_position();
            tinted(p.y, p.y + s.height, p.x + 12.0, p.x + s.width - 12.0)
        })
        .collect();
    let (first, second) = (find("CPU"), find("Memory"));
    let (p, s) = (first.absolute_position(), first.size());
    let gap = tinted(p.y, p.y + s.height, p.x + s.width + 1.0, second.absolute_position().x - 1.0).len();
    (rows, gap)
}

/// A line along the floor or the ceiling keeps its full stroke width (1.5 px: two pixel rows); a clip
/// at the path's own box would trim it to half.
#[test]
fn floor_and_ceiling_lines_keep_their_full_stroke() {
    let mid = "M 0 50 L 1000 50";
    let (rows, _) = render_sparks(["M 0 100 L 1000 100", "M 0 0 L 1000 0", mid, mid, mid]);
    assert!(rows[0].len() >= 2, "floor rows {:?}", rows[0]);
    assert!(rows[1].len() >= 2, "ceiling rows {:?}", rows[1]);
}

/// A sparkline's history left of its window stays inside its meter: nothing is drawn in the gap
/// between two meters.
#[test]
fn a_sparklines_off_screen_history_is_clipped_to_its_meter() {
    let (rows, gap) = render_sparks(["M -400 50 L 1400 50"; 5]);
    assert!(!rows[1].is_empty(), "the line shows inside the meter");
    assert_eq!(gap, 0, "nothing is drawn between the meters");
}

/// Bench data: `Smooth` is load-like (a slow wave with a little noise); `Noise` is uniform
/// noise, a jump of up to 100% on every sample, the stroke rasteriser's worst case.
#[derive(Clone, Copy, Debug)]
enum Load {
    Smooth,
    Noise,
}

/// Seeded samples, 1 s apart, in 0..100.
fn samples(n: usize, seed: u32, load: Load) -> Vec<(u64, Option<f32>)> {
    let mut s = seed;
    (0..n)
        .map(|i| {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let r = (s >> 8) as f32 / 16_777_216.0;
            let v = match load {
                Load::Smooth => 50.0 + 30.0 * (i as f32 / 7.0 + seed as f32).sin() + (r - 0.5) * 10.0,
                Load::Noise => r * 100.0,
            };
            (i as u64 * 1000, Some(v))
        })
        .collect()
}

fn busy_graph(label: &str, n: usize, seed: u32, load: Load) -> GraphData {
    let s = negotium::graph::shape(samples(n, seed, load), (n as u64 - 1) * 1000, n as u64 * 1000, 100.0, n);
    GraphData {
        label: label.into(), value: "12%".into(), scale: "100%".into(),
        line: s.line.into(), area: s.area.into(), line2: "".into(),
    }
}

/// Frame time while resizing the Performance page: 32 CPU cells, 1920 × 1080 at scale 1.5, the
/// software renderer (the fallback renderer), every frame a full repaint after a one-pixel resize
/// (as a window drag produces). Gated on load-like data; the worst case (uniform noise) is only
/// reported. Release only: `cargo test --release -p negotium --test render -- --ignored --nocapture`.
#[test]
#[ignore = "bench: run in release with --ignored"]
fn performance_page_resize_frames_fit_in_16_ms() {
    use negotium::{DeviceData, StatData};
    use slint::platform::software_renderer::{MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType};
    use slint::platform::{Platform, WindowAdapter, WindowEvent};
    use std::rc::Rc;
    use std::time::Instant;
    if cfg!(debug_assertions) {
        panic!("frame times only mean something in a release build");
    }

    struct Software(Rc<MinimalSoftwareWindow>);
    impl Platform for Software {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            Ok(self.0.clone())
        }
    }
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(Software(window.clone()))).expect("one backend per test thread");

    let ui = MainWindow::new().unwrap();
    ui.set_page(1);
    ui.set_perf_title("CPU".into());
    let stats: Vec<StatData> = (0..12).map(|_| StatData { label: "Speed".into(), value: "3.4 GHz".into() }).collect();
    ui.set_perf_stats(ModelRc::new(VecModel::from(stats)));
    ui.show().unwrap();
    window.dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: 1.5 });

    let (w, h) = (1920u32, 1080u32);
    let mut buffer = vec![PremultipliedRgbaColor::default(); (w * h) as usize];
    let mut p25_frame = |load: Load| {
        ui.set_perf_graphs(ModelRc::new(VecModel::from(vec![busy_graph("% Utilization", 300, 1, load)])));
        let cells: Vec<GraphData> = (0..32).map(|i| busy_graph(&format!("CPU {i}"), 60, i + 7, load)).collect();
        ui.set_perf_cells(ModelRc::new(VecModel::from(cells)));
        let devices: Vec<DeviceData> = (0..6)
            .map(|i| DeviceData {
                id: format!("d{i}").into(), title: "CPU".into(), subtitle: "".into(), value: "12%".into(),
                spark: negotium::graph::line(samples(60, i, load), 59_000, 60_000, 100.0, 60).into(),
            })
            .collect();
        ui.set_perf_devices(ModelRc::new(VecModel::from(devices)));
        let mut render = |w: u32, h: u32| {
            ui.window().set_size(PhysicalSize::new(w, h));
            window.request_redraw();
            let start = Instant::now();
            window.draw_if_needed(|r| {
                r.render(&mut buffer[..(w * h) as usize], w as usize);
            });
            start.elapsed().as_secs_f64() * 1e3
        };
        render(w, h); // warm up: first layout, glyph cache
        let mut frames: Vec<f64> = (0..40u32).map(|i| render(w - i % 2, h - i % 2)).collect();
        frames.sort_by(f64::total_cmp);
        let (median, p25) = (frames[frames.len() / 2], frames[frames.len() / 4]);
        println!("{load:?} data, resize frame at {w}x{h}, scale 1.5: median {median:.2} ms, p25 {p25:.2}, min {:.2}, max {:.2}",
            frames[0], frames[frames.len() - 1]);
        p25
    };
    // Gated on the 25th percentile: a frame can only be slowed by other load on the machine, so a
    // low percentile measures the renderer, and load spikes during the run don't fail it.
    let smooth = p25_frame(Load::Smooth);
    p25_frame(Load::Noise); // reported, not gated
    assert!(smooth <= 16.0, "25th-percentile frame {smooth:.2} ms on load-like data is over 16 ms");
}

/// Software-renderer cost of one scroll step (GraphClock moves, nothing else changes) at
/// 1920 x 1080, scale 1.5: partial repaint (the window's real mode). Reported, not gated. The
/// full-repaint figure is `performance_page_resize_frames_fit_in_16_ms`. Release only:
/// `cargo test --release -p negotium --test render -- --ignored scroll_step --nocapture`.
#[test]
#[ignore = "bench: run in release with --ignored"]
fn scroll_step_cost_under_the_software_renderer() {
    use slint::platform::software_renderer::{MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType};
    use slint::platform::{Platform, WindowAdapter, WindowEvent};
    use std::rc::Rc;
    use std::time::Instant;
    if cfg!(debug_assertions) {
        panic!("frame times only mean something in a release build");
    }
    struct Software(Rc<MinimalSoftwareWindow>);
    impl Platform for Software {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            Ok(self.0.clone())
        }
    }
    let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
    slint::platform::set_platform(Box::new(Software(window.clone()))).expect("one backend per test thread");
    let ui = MainWindow::new().unwrap();
    ui.show().unwrap();
    window.dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: 1.5 });
    let (w, h) = (1920u32, 1080u32);
    ui.window().set_size(PhysicalSize::new(w, h));
    let mut buffer = vec![PremultipliedRgbaColor::default(); (w * h) as usize];
    let spark = negotium::graph::line(samples(82, 3, Load::Smooth), 81_000, 60_000, 100.0, 60);
    let meters: Vec<negotium::MeterData> = ["CPU", "Memory", "Disk", "GPU", "Network"]
        .into_iter()
        .map(|l| negotium::MeterData { label: l.into(), value: "12%".into(), sub: "".into(), spark: spark.clone().into(), available: true })
        .collect();
    ui.set_perf_graphs(ModelRc::new(VecModel::from(vec![busy_graph("% Utilization", 300, 1, Load::Smooth)])));
    ui.set_meters(ModelRc::new(VecModel::from(meters)));
    let cells: Vec<GraphData> = (0..32).map(|i| busy_graph(&format!("CPU {i}"), 60, i + 7, Load::Smooth)).collect();
    ui.set_perf_cells(ModelRc::new(VecModel::from(cells)));
    for (page, name) in [(0, "Overview"), (1, "Performance, 32 cells")] {
        ui.set_page(page);
        let mut step = |i: u32| {
            ui.invoke_scroll_from(-1_000.0 + i as f32 * 20.0, 1.0);
            window.request_redraw();
            let start = Instant::now();
            window.draw_if_needed(|r| {
                r.render(&mut buffer, w as usize);
            });
            start.elapsed().as_secs_f64() * 1e3
        };
        step(0); // first paint
        let mut frames: Vec<f64> = (1..40).map(&mut step).collect();
        frames.sort_by(f64::total_cmp);
        println!("{name}: scroll step, partial repaint: median {:.2} ms, p95 {:.2} ms", frames[frames.len() / 2], frames[frames.len() * 95 / 100]);
    }
}

/// A software window on a clock the test moves (4 ms a step: a 250 Hz display), for counting the
/// frames a change costs. Each test thread installs its own.
mod frame_count {
    use slint::platform::software_renderer::{MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType};
    use slint::platform::{Platform, WindowAdapter};
    use std::cell::Cell;
    use std::rc::Rc;
    use std::time::Duration;

    thread_local! { static NOW_MS: Cell<u64> = const { Cell::new(0) }; }

    struct Mocked(Rc<MinimalSoftwareWindow>);
    impl Platform for Mocked {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            Ok(self.0.clone())
        }
        fn duration_since_start(&self) -> Duration {
            Duration::from_millis(NOW_MS.with(Cell::get))
        }
    }

    pub struct Frames {
        window: Rc<MinimalSoftwareWindow>,
        buffer: Vec<PremultipliedRgbaColor>,
        width: usize,
    }

    impl Frames {
        /// Installs the platform; create the window (`MainWindow::new`) after this.
        pub fn install(width: u32, height: u32) -> Frames {
            let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
            slint::platform::set_platform(Box::new(Mocked(window.clone()))).expect("one backend per test thread");
            Frames { window, buffer: vec![PremultipliedRgbaColor::default(); (width * height) as usize], width: width as usize }
        }

        /// Moves the clock `steps` times by 4 ms, drawing whatever is due; returns the frames drawn.
        pub fn run(&mut self, steps: u32) -> u32 {
            let mut drawn = 0;
            for _ in 0..steps {
                NOW_MS.with(|t| t.set(t.get() + 4));
                slint::platform::update_timers_and_animations();
                let (buffer, width) = (&mut self.buffer, self.width);
                if self.window.draw_if_needed(|r| {
                    r.render(buffer, width);
                }) {
                    drawn += 1;
                }
            }
            drawn
        }

        /// Like `run`, but returns the mocked time (ms) of every frame drawn.
        pub fn run_log(&mut self, steps: u32) -> Vec<u64> {
            let mut at = Vec::new();
            for _ in 0..steps {
                if self.run(1) == 1 {
                    at.push(now_ms());
                }
            }
            at
        }
    }

    /// The mocked clock, ms since the platform started.
    pub fn now_ms() -> u64 {
        NOW_MS.with(Cell::get)
    }

    /// The frames the winit backend presents for draws at `times` (one 4 ms step apart at the
    /// closest): Slint 1.18's timer-based frame throttle, started by a redraw request, ticks once
    /// more after the frame it asked for and requests another, identical one, so a draw with none
    /// in the next step costs two frames; draws in consecutive steps (an animation) one each.
    pub fn winit_frames(times: &[u64]) -> usize {
        times.len() + times.iter().filter(|&&t| !times.contains(&(t + 4))).count()
    }
}

fn app_row(id: i32, cpu: &str) -> negotium::RowData {
    negotium::RowData { id, name: format!("App {id}").into(), cpu: cpu.into(), actionable: true, ..Default::default() }
}

/// A frame that adds a row and changes a visible one costs one redraw, not an animation: the
/// Fluent scrollbar's thumb animated every change in the row count (150 ms, about 36 frames at
/// 240 Hz on the GPU renderer).
#[test]
fn a_row_change_in_the_app_list_draws_once_not_an_animation() {
    let mut frames = frame_count::Frames::install(1000, 720);
    let ui = MainWindow::new().unwrap();
    let rows = std::rc::Rc::new(VecModel::from((0..140).map(|i| app_row(i, "1.0%")).collect::<Vec<_>>()));
    ui.set_rows(ModelRc::from(rows.clone()));
    ui.show().unwrap();
    ui.window().set_size(PhysicalSize::new(1000, 720));
    frames.run(100); // first paint, and anything that settles after it
    rows.push(app_row(140, "1.0%"));
    slint::Model::set_row_data(&*rows, 3, app_row(3, "9.0%"));
    let drawn = frames.run(100); // 400 ms
    assert!(drawn <= 2, "{drawn} frames for one row change");
}

/// The Performance details view scrolls under a `ScrollIndicator`, because the Fluent thumb
/// animated for 150 ms whenever a frame re-evaluated the content height; only the device list keeps
/// the Fluent bar (a frame never changes its content height). A frame costs one redraw.
#[test]
fn a_performance_frame_draws_once() {
    use negotium::{DeviceData, StatData};
    let mut frames = frame_count::Frames::install(1000, 720);
    let ui = MainWindow::new().unwrap();
    ui.global::<negotium::GraphClock>().set_smooth(false);
    ui.set_page(1);
    let devices = |v: &str| -> Vec<DeviceData> {
        (0..20).map(|i| DeviceData { id: format!("d{i}").into(), title: format!("Disk {i}").into(), subtitle: "".into(), value: v.into(), spark: "M 0 50 L 1000 40".into() }).collect()
    };
    let stats = |v: &str| -> Vec<StatData> { (0..12).map(|_| StatData { label: "Speed".into(), value: v.into() }).collect() };
    let cells = |seed: u32| -> Vec<GraphData> { (0..64).map(|i| busy_graph(&format!("CPU {i}"), 60, i + seed, Load::Smooth)).collect() };
    let (d, s) = (std::rc::Rc::new(VecModel::from(devices("1%"))), std::rc::Rc::new(VecModel::from(stats("3.4 GHz"))));
    let (g, c) = (std::rc::Rc::new(VecModel::from(vec![busy_graph("% Utilization", 60, 1, Load::Smooth)])), std::rc::Rc::new(VecModel::from(cells(7))));
    ui.set_perf_devices(ModelRc::from(d.clone()));
    ui.set_perf_stats(ModelRc::from(s.clone()));
    ui.set_perf_graphs(ModelRc::from(g.clone()));
    ui.set_perf_cells(ModelRc::from(c.clone()));
    ui.show().unwrap();
    ui.window().set_size(PhysicalSize::new(1000, 720));
    frames.run(100);
    // Both views overflow, so the frame is counted with their scrollbars present: the details' indicator...
    let indicators = ElementHandle::find_by_element_id(&ui, "ScrollIndicator::thumb").filter(|t| t.size().width > 0.0).count();
    assert_eq!(indicators, 1, "sixty-four cells overflow the details: their indicator shows");
    // ...and the device list's Fluent bar: its last device lies below the list (or isn't built).
    let list = ElementHandle::find_by_element_id(&ui, "PerformancePage::device-list").next().expect("the device list");
    let list_bottom = list.absolute_position().y + list.size().height;
    let below = |label: &str| ElementHandle::find_by_accessible_label(&ui, label).next().is_none_or(|d| d.absolute_position().y + d.size().height > list_bottom);
    assert!(!below("Disk 0 performance"), "the first device is in view");
    let in_view = (0..slint::Model::row_count(&*d)).filter(|i| !below(&format!("Disk {i} performance"))).count();
    assert!(in_view < slint::Model::row_count(&*d), "{in_view} of twenty devices in view: they overflow the list");
    // The next second's frame, as crate::bridge::sync_model applies it: values change in place.
    use slint::Model;
    for (i, v) in devices("2%").into_iter().enumerate() { d.set_row_data(i, v); }
    for (i, v) in stats("3.5 GHz").into_iter().enumerate() { s.set_row_data(i, v); }
    for (i, v) in cells(9).into_iter().enumerate() { c.set_row_data(i, v); }
    g.set_row_data(0, busy_graph("% Utilization", 60, 2, Load::Smooth));
    let drawn = frames.run(100);
    assert!(drawn <= 2, "{drawn} frames for one Performance frame");
}

/// `animation-tick()` marks an animation as running whenever it is read, and the GPU backend then
/// draws an extra, identical frame: the graph clock reads Rust's clock instead (fix wave R9).
#[test]
fn no_slint_source_reads_the_animation_tick() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui");
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "slint") {
            let src = squeezed(&std::fs::read_to_string(&path).unwrap());
            assert!(!src.contains("animation-tick("), "{} calls animation-tick()", path.display());
        }
    }
}

/// An icon is drawn as straight alpha on either theme (icons::extract hands it over that way):
/// a uniform grey at 25% shows as a quarter of the grey over the card, not the grey added to it.
#[test]
fn an_icons_soft_pixels_blend_as_straight_alpha_on_dark_and_light() {
    software_backend();
    for dark in [true, false] {
        let ui = MainWindow::new().unwrap();
        ui.invoke_set_dark(dark);
        let mut px = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(32, 32);
        px.make_mut_slice().fill(slint::Rgba8Pixel { r: 200, g: 200, b: 200, a: 64 });
        let row = negotium::RowData {
            id: 1, kind: 1, name: "App".into(), actionable: true, has_icon: true, icon: slint::Image::from_rgba8(px),
            ..Default::default()
        };
        ui.set_rows(ModelRc::new(VecModel::from(vec![row])));
        ui.window().set_size(PhysicalSize::new(1000, 720));
        ui.show().unwrap();
        let icon = ElementHandle::find_by_element_id(&ui, "RowIcon::icon-image").next().expect("the icon");
        let (p, s) = (icon.absolute_position(), icon.size());
        let shot = ui.window().take_snapshot().expect("snapshot");
        let at = |x: f32, y: f32| shot.as_slice()[(y.round() as u32 * shot.width() + x.round() as u32) as usize];
        let (inside, beside) = (at(p.x + s.width / 2.0, p.y + s.height / 2.0), at(p.x - 3.0, p.y + s.height / 2.0));
        let straight = (200.0 * 0.25 + beside.r as f32 * 0.75).round() as i32;
        assert!((inside.r as i32 - straight).abs() <= 4, "dark {dark}: {inside:?} over {beside:?}, straight would be {straight}");
        ui.hide().unwrap();
    }
}

/// The Overview's draw times over `weights.len()` one-second frames through the real Bridge, on
/// the frame-count clock: meters with sparklines, the graphs' scroll run, and top tiles with the
/// given weights (none for an empty row). Returns each second's draw times.
fn overview_draws(weights: &[Vec<f32>]) -> Vec<Vec<u64>> {
    use negotium::view::{Frame, MeterView, TileView};
    let mut frames = frame_count::Frames::install(1000, 720);
    let ui = MainWindow::new().unwrap();
    let bridge = negotium::bridge::Bridge::install(&ui, std::sync::Arc::new(negotium::controls::Controls::default()));
    // The mocked clock, for the graphs and so the tiles (Bridge::install gave them the wall clock).
    negotium::smooth::install(&ui, || std::time::Duration::from_millis(frame_count::now_ms()));
    ui.show().unwrap();
    ui.window().set_size(PhysicalSize::new(1000, 720));
    let spark = negotium::graph::line(samples(82, 3, Load::Smooth), 81_000, 60_000, 100.0, 60);
    weights.iter().enumerate().map(|(sec, w)| {
        let f = Frame {
            meters: ["CPU", "Memory", "Disk", "GPU", "Network"].iter()
                .map(|l| MeterView { label: l, value: format!("{sec}%"), sub: String::new(), spark: spark.clone(), available: true }).collect(),
            tiles: w.iter().enumerate()
                .map(|(i, &w)| TileView { id: i as i32 + 1, name: format!("App {i}"), value: format!("{w:.1}%"), weight: w, hung: false }).collect(),
            graph_end_ms: 81_000 + sec as u64 * 1_000,
            ..Default::default()
        };
        bridge.borrow_mut().set_collector_now(Some(frame_count::now_ms() as f64));
        bridge.borrow_mut().apply(&ui, f);
        frames.run_log(250) // a second at 250 Hz
    }).collect()
}

/// With Smooth graphs on, the top tiles glide only on the redraws the scrolling graphs make: a
/// frame whose tiles change draws exactly when one whose tiles don't would, counted as the winit
/// backend presents them. Weights from a real run (crate::view's averaged CPU: edges move 10 to
/// 60 px a frame). A 450 ms `animate` drew at display rate (about 111 frames a second on a 240 Hz
/// screen), and a glide timer of the strip's own one redraw a step, each presented twice by the
/// winit frame throttle (61 a second at a step a pixel, 24 at eight steps a glide).
#[test]
fn the_top_tiles_glide_only_on_the_graphs_redraws() {
    let real: Vec<Vec<f32>> = vec![
        vec![5.390, 2.771, 2.060, 1.441, 1.170],
        vec![5.411, 2.954, 2.306, 1.683, 1.364],
        vec![5.456, 3.100, 2.553, 1.746, 1.587],
        vec![5.567, 3.258, 2.603, 1.859, 1.745],
        vec![5.500, 3.387, 2.655, 1.886, 1.818],
        vec![5.365, 3.506, 2.690, 1.946, 1.815],
    ];
    // The same tiles every frame: nothing for them to glide to after the first.
    let still: Vec<Vec<f32>> = vec![real[0].clone(); real.len()];
    // One window per thread: the frame-count platform is installed per thread.
    let run = |w: Vec<Vec<f32>>| std::thread::spawn(move || overview_draws(&w)).join().unwrap();
    let (gliding, resting) = (run(real), run(still));
    for sec in 1..gliding.len() {
        let (g, r) = (frame_count::winit_frames(&gliding[sec]), frame_count::winit_frames(&resting[sec]));
        assert_eq!(gliding[sec], resting[sec], "second {sec}: tiles gliding drew {g} frames presented, resting {r}");
    }
    assert!(frame_count::winit_frames(&gliding[2]) <= 14, "a second costs the graphs' few redraws: {:?}", gliding[2]);
}

/// Tiles laid out while Smooth graphs is off stay where they are, fully shown, when it is turned
/// on: no glide ran for them, so nothing may leave them at a glide's starting point (nothing).
#[test]
fn tiles_stay_shown_at_their_widths_when_smooth_graphs_turns_on() {
    use std::time::Duration;
    software_backend();
    let ui = MainWindow::new().unwrap();
    negotium::smooth::install(&ui, || Duration::from_millis(i_slint_backend_testing::get_mocked_time()));
    ui.global::<Theme>().set_accent(slint::Color::from_rgb_u8(255, 0, 0));
    ui.set_settings_smooth_graphs(false);
    let tile = |i: i32, w: f32| negotium::TileData { id: i, name: format!("App {i}").into(), value: "1%".into(), weight: w, hung: false, leaving: false };
    ui.set_tiles(ModelRc::new(VecModel::from(vec![tile(0, 1.0), tile(1, 3.0)])));
    ui.window().set_size(PhysicalSize::new(1000, 720));
    ui.show().unwrap();
    let rects = || -> Vec<(f32, f32, f32)> {
        (0..2).map(|i| {
            let t = ElementHandle::find_by_accessible_label(&ui, &format!("App {i} tile")).next().expect("tile");
            (t.absolute_position().x, t.absolute_position().y, t.size().width)
        }).collect()
    };
    rects(); // lays the tiles out, so their birth timers start
    for ms in [0, 1, 1000] {
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(ms));
    }
    let before = rects();
    assert!(before[1].2 > before[0].2 * 2.0, "three times the weight, wider: {before:?}");
    ui.set_settings_smooth_graphs(true);
    for ms in [0, 16, 100] {
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(ms));
        let now = rects();
        for (b, n) in before.iter().zip(&now) {
            assert!((b.2 - n.2).abs() < 1.0, "{ms} ms after turning smooth on: widths {before:?} became {now:?}");
        }
        let shot = ui.window().take_snapshot().expect("snapshot");
        for &(x, y, w) in &now {
            let p = shot.as_slice()[((y + 26.0).round() as u32 * shot.width() + (x + w - 4.0).round() as u32) as usize];
            assert!(accent_tinted(p), "{ms} ms after turning smooth on, a tile is drawn: {p:?}");
        }
    }
}

/// The real pipeline (Collector, Model, Ranker, TopPicker, build_frame through LiveSource) into the
/// real Bridge, on a software window driven at about 250 Hz of wall-clock time, one frame a second
/// for `NEGOTIUM_REAL_SECS` (10) seconds: the redraws a second, as negotium-bench counts them.
/// Machine-dependent and slow: run by hand (`--ignored --nocapture`). `NEGOTIUM_REAL_OFF` takes a
/// comma list of parts to leave out: tiles, meters, rows, icons, smooth.
#[test]
#[ignore]
fn real_pipeline_redraws_per_second() {
    use negotium::bridge::Bridge;
    use negotium::controls::Controls;
    use negotium::worker::{LiveSource, Source};
    use slint::Model;
    use slint::platform::software_renderer::{MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType};
    use slint::platform::{Platform, WindowAdapter};
    use std::rc::Rc;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    struct Real(Rc<MinimalSoftwareWindow>, Instant);
    impl Platform for Real {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            Ok(self.0.clone())
        }
        fn duration_since_start(&self) -> Duration {
            self.1.elapsed()
        }
    }
    let off = std::env::var("NEGOTIUM_REAL_OFF").unwrap_or_default();
    let off = |part: &str| off.split(',').any(|p| p == part);
    let secs: u64 = std::env::var("NEGOTIUM_REAL_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(10);
    let env = |k: &str, d: f32| std::env::var(k).ok().and_then(|s| s.parse().ok()).unwrap_or(d);
    let (w, h, scale) = (env("NEGOTIUM_REAL_W", 1600.0) as u32, env("NEGOTIUM_REAL_H", 1000.0) as u32, env("NEGOTIUM_REAL_SCALE", 1.0));
    let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
    slint::platform::set_platform(Box::new(Real(window.clone(), Instant::now()))).unwrap();
    let ui = MainWindow::new().unwrap();
    ui.set_settings_smooth_graphs(!off("smooth"));
    ui.global::<negotium::GraphClock>().set_smooth(!off("smooth"));
    let controls = Arc::new(Controls::default());
    let bridge = Bridge::install(&ui, controls.clone());
    let history: negotium::worker::SharedHistory = Default::default();
    bridge.borrow_mut().set_history(history.clone());
    let epoch = Instant::now();
    bridge.borrow_mut().set_epoch(epoch);
    type Arrived = Arc<Mutex<Vec<(String, Option<negotium::icons::IconPixels>)>>>;
    let icons: Arrived = Default::default();
    if !off("icons") {
        let icons = icons.clone();
        bridge.borrow_mut().set_icon_loader(negotium::icons::IconLoader::spawn(negotium::icons::extract, move |p, px| icons.lock().unwrap().push((p, px))));
    }
    let scratch = std::env::temp_dir().join(format!("negotium-real-pipeline-{}", std::process::id()));
    let mut source = LiveSource::new(history, epoch, Default::default()).with_log(negotium::log::Log::with_writes(scratch.join("negotium.log"), negotium::appdata::Writes::Plain));
    if off("tiles") { ui.set_tiles(ModelRc::new(VecModel::<negotium::TileData>::default())); }
    if off("meters") { ui.set_meters(ModelRc::new(VecModel::<negotium::MeterData>::default())); }
    if off("rows") { ui.set_rows(ModelRc::new(VecModel::<negotium::RowData>::default())); }
    ui.show().unwrap();
    window.dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged { scale_factor: scale });
    ui.window().set_size(PhysicalSize::new(w, h));
    let mut buffer = vec![PremultipliedRgbaColor::default(); (w * h) as usize];
    let start = Instant::now();
    let mut next_frame = start;
    let mut per_sec = vec![Vec::<u32>::new(); secs as usize + 1];
    let mut sec_start = start;
    while start.elapsed() < Duration::from_secs(secs) {
        let now = Instant::now();
        if now >= next_frame {
            if let Some(frame) = source.next_frame(&controls) {
                bridge.borrow_mut().apply(&ui, frame);
            }
            sec_start = now;
            next_frame += Duration::from_secs(1);
            println!("frame at {:>6} ms: scroll-interval {:?}, tiles {:?}", (now - start).as_millis(), ui.get_scroll_interval(),
                ui.get_tiles().iter().map(|t| (t.id, t.weight, t.leaving)).collect::<Vec<_>>());
        }
        for (p, px) in icons.lock().unwrap().drain(..) {
            negotium::bridge::icon_arrived(&bridge, p, px);
        }
        slint::platform::update_timers_and_animations();
        // As the winit backend: an active animation asks for a frame even when nothing is dirty.
        let animating = ui.window().has_active_animations();
        if animating {
            window.request_redraw();
        }
        if window.draw_if_needed(|r| { r.render(&mut buffer, w as usize); }) {
            let s = (start.elapsed().as_secs()) as usize;
            if s < per_sec.len() { per_sec[s].push((Instant::now() - sec_start).as_millis() as u32); }
        }
        std::thread::sleep(Duration::from_millis(4));
    }
    // Draws land on a ~5 ms loop here, not the 4 ms grid frame_count::winit_frames assumes: a
    // draw with none within 8 ms after it is isolated, and winit presents it twice.
    for (s, d) in per_sec.iter().enumerate() {
        let isolated = d.iter().filter(|&&t| !d.iter().any(|&u| u > t && u <= t + 8)).count();
        println!("second {s}: {} redraws, about {} presented by winit {:?}", d.len(), d.len() + isolated, d);
    }
    let _ = std::fs::remove_dir_all(&scratch);
}

// Std Buttons in a modal under a flipped palette (dark mode + White@100).
fn lum(p: slint::Rgba8Pixel) -> f32 { p.r as f32 * 0.299 + p.g as f32 * 0.587 + p.b as f32 * 0.114 }

fn button_ink_range(dark: bool, tint: Option<[u8; 3]>, opacity: Option<u8>) -> (f32, f32, f32) {
    software_backend();
    let ui = MainWindow::new().unwrap();
    ui.window().set_size(PhysicalSize::new(1000, 720));
    let s = negotium::settings::Settings {
        backdrop: negotium::settings::BackdropChoice::Acrylic,
        acrylic_tint: tint,
        acrylic_opacity: opacity,
        ..negotium::settings::Settings::default()
    };
    negotium::settingspage::apply_settings(&ui, &s);
    ui.global::<Theme>().set_mica(true);
    ui.invoke_set_dark(dark);
    ui.set_tint_picker_visible(true);
    ui.show().unwrap();
    let shot = ui.window().take_snapshot().expect("snapshot");
    let b = ElementHandle::find_by_accessible_label(&ui, "Cancel").next().expect("Cancel button");
    let (pos, size) = (b.absolute_position(), b.size());
    let (x0, y0) = (pos.x.round() as u32, pos.y.round() as u32);
    let (w, h) = (size.width.round() as u32, size.height.round() as u32);
    let mut lo = f32::MAX; let mut hi = f32::MIN; let mut edge = 0.0;
    for y in y0 + 2..y0 + h - 2 { for x in x0 + 2..x0 + w - 2 {
        let l = lum(shot.as_slice()[(y * shot.width() + x) as usize]);
        lo = lo.min(l); hi = hi.max(l);
    } }
    edge += lum(shot.as_slice()[((y0 + 3) * shot.width() + x0 + 3) as usize]);
    println!("PROBE dark={dark} tint={tint:?} op={opacity:?}: Cancel button lum min {lo:.0} max {hi:.0} corner fill {edge:.0}");
    (lo, hi, edge)
}

#[test]
fn probe_buttons_under_a_flipped_palette() {
    let (lo, hi, _) = button_ink_range(true, Some([255, 255, 255]), Some(100));
    assert!(hi - lo > 100.0, "dark + White@100: Cancel button ink range only {}", hi - lo);
}
#[test]
fn probe_buttons_control_dark_default() {
    let (lo, hi, _) = button_ink_range(true, None, None);
    assert!(hi - lo > 100.0, "dark default: range {}", hi - lo);
}
#[test]
fn probe_buttons_light_black() {
    let (lo, hi, _) = button_ink_range(false, Some([0, 0, 0]), Some(100));
    assert!(hi - lo > 100.0, "light + Black@100: range {}", hi - lo);
}
#[test]
fn probe_buttons_control_light_default() {
    let (lo, hi, _) = button_ink_range(false, None, None);
    assert!(hi - lo > 100.0, "light default: range {}", hi - lo);
}
#[test]
fn probe_sweep_0() { let (lo, hi, _) = button_ink_range(true, Some([255,255,255]), Some(43)); println!("SWEEP dark=true 255,255,255@43 range {}", hi - lo); }
#[test]
fn probe_sweep_1() { let (lo, hi, _) = button_ink_range(true, Some([255,255,255]), Some(50)); println!("SWEEP dark=true 255,255,255@50 range {}", hi - lo); }
#[test]
fn probe_sweep_2() { let (lo, hi, _) = button_ink_range(true, Some([255,255,255]), Some(70)); println!("SWEEP dark=true 255,255,255@70 range {}", hi - lo); }
#[test]
fn probe_sweep_3() { let (lo, hi, _) = button_ink_range(true, Some([200,200,200]), Some(100)); println!("SWEEP dark=true 200,200,200@100 range {}", hi - lo); }
#[test]
fn probe_sweep_4() { let (lo, hi, _) = button_ink_range(false, Some([0,0,0]), Some(50)); println!("SWEEP dark=false 0,0,0@50 range {}", hi - lo); }
#[test]
fn probe_sweep_5() { let (lo, hi, _) = button_ink_range(false, Some([0,0,0]), Some(70)); println!("SWEEP dark=false 0,0,0@70 range {}", hi - lo); }
#[test]
fn probe_sweep_6() { let (lo, hi, _) = button_ink_range(false, Some([60,60,60]), Some(100)); println!("SWEEP dark=false 60,60,60@100 range {}", hi - lo); }#[test]
fn std_buttons_stay_visible_when_a_white_tint_flips_the_dark_palette() {
    let (lo, hi, _) = button_ink_range(true, Some([255, 255, 255]), Some(100));
    assert!(hi - lo > 100.0, "dark + White@100: Cancel button ink range only {}", hi - lo);
}

#[test]
fn std_buttons_stay_visible_when_a_black_tint_flips_the_light_palette() {
    let (lo, hi, _) = button_ink_range(false, Some([0, 0, 0]), Some(100));
    assert!(hi - lo > 100.0, "light + Black@100: Cancel button ink range only {}", hi - lo);
}

#[test]
fn std_buttons_stay_visible_with_no_tint_in_dark_mode() {
    let (lo, hi, _) = button_ink_range(true, None, None);
    assert!(hi - lo > 100.0, "dark, no tint: Cancel button ink range only {}", hi - lo);
}

#[test]
fn std_buttons_stay_visible_with_no_tint_in_light_mode() {
    let (lo, hi, _) = button_ink_range(false, None, None);
    assert!(hi - lo > 100.0, "light, no tint: Cancel button ink range only {}", hi - lo);
}
