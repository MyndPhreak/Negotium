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

/// Meters (CPU, Memory, Disk, GPU, Network) drawn with these sparklines, rendered: per meter the
/// rows (absolute y) tinted by the accent away from its edges, and how many pixels beside the first
/// meter, in the gap to the second, are tinted.
/// Every data path scrolls with GraphClock (crate::smooth): one that didn't would stand still
/// while the others move.
#[test]
fn every_data_path_scrolls_with_the_graph_clock() {
    for (file, body) in &data_paths() {
        assert!(body.contains("viewbox-x:"), "{file}: a data Path without viewbox-x: {body}");
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
                id: format!("d{i}").into(), title: "CPU".into(), value: "12%".into(),
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
