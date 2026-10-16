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
    let (top, bottom) = (tinted_in(y0..y0 + 2), tinted_in(y0 + h - 2..y0 + h));
    assert!(top > 0, "the 100% sample reaches the graph's top rows");
    assert!(bottom > 0, "the area reaches the graph's bottom rows");
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

/// Every data path (a 1000 × 100 view box, see crate::graph) stretches to its box: the default
/// `fit: contain` keeps the 10:1 aspect and squashes the data into a band.
#[test]
fn every_data_path_stretches_to_fill_its_box() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui");
    let mut checked = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "slint") {
            continue;
        }
        let src = std::fs::read_to_string(&path).unwrap();
        let mut rest = src.as_str();
        while let Some(at) = rest.find("Path {") {
            let body_start = at + "Path {".len();
            let mut depth = 1;
            let mut end = body_start;
            for (i, c) in rest[body_start..].char_indices() {
                match c {
                    '{' => depth += 1,
                    '}' => depth -= 1,
                    _ => {}
                }
                if depth == 0 {
                    end = body_start + i;
                    break;
                }
            }
            let body = &rest[body_start..end];
            if body.contains("viewbox-width: 1000;") {
                checked += 1;
                assert!(body.contains("fit: fill;"), "{}: a data Path without fit: fill:\n{body}", path.display());
            }
            rest = &rest[end..];
        }
    }
    assert!(checked >= 5, "found only {checked} data paths");
}

/// Seeded pseudo-random samples, 1 s apart.
fn samples(n: usize, seed: u32) -> Vec<(u64, Option<f32>)> {
    let mut s = seed;
    (0..n)
        .map(|i| {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (i as u64 * 1000, Some((s >> 8) as f32 / 16_777_216.0 * 100.0))
        })
        .collect()
}

fn busy_graph(label: &str, n: usize, seed: u32) -> GraphData {
    let s = negotium::graph::shape(samples(n, seed), (n as u64 - 1) * 1000, n as u64 * 1000, 100.0, n);
    GraphData {
        label: label.into(), value: "12%".into(), scale: "100%".into(),
        line: s.line.into(), area: s.area.into(), line2: "".into(),
    }
}

/// Frame time while resizing the Performance page: 32 CPU cells, 1920 × 1080 at scale 1.5, the
/// software renderer (as the app uses), every frame a full repaint after a one-pixel resize (as
/// a window drag produces). Release only: `cargo test --release -p negotium --test render -- --ignored`.
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
    ui.set_perf_graphs(ModelRc::new(VecModel::from(vec![busy_graph("% Utilization", 300, 1)])));
    let cells: Vec<GraphData> = (0..32).map(|i| busy_graph(&format!("CPU {i}"), 60, i + 7)).collect();
    ui.set_perf_cells(ModelRc::new(VecModel::from(cells)));
    let devices: Vec<DeviceData> = (0..6)
        .map(|i| DeviceData {
            id: format!("d{i}").into(), title: "CPU".into(), value: "12%".into(),
            spark: negotium::graph::line(samples(60, i), 59_000, 60_000, 100.0, 60).into(),
        })
        .collect();
    ui.set_perf_devices(ModelRc::new(VecModel::from(devices)));
    let stats: Vec<StatData> = (0..12).map(|_| StatData { label: "Speed".into(), value: "3.4 GHz".into() }).collect();
    ui.set_perf_stats(ModelRc::new(VecModel::from(stats)));
    ui.show().unwrap();
    window.dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: 1.5 });

    let (w, h) = (1920u32, 1080u32);
    let mut buffer = vec![PremultipliedRgbaColor::default(); (w * h) as usize];
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
    let median = frames[frames.len() / 2];
    println!("Performance page resize frame at {w}x{h}, scale 1.5: median {median:.2} ms, min {:.2}, max {:.2}",
        frames[0], frames[frames.len() - 1]);
    assert!(median <= 16.0, "median frame {median:.2} ms is over 16 ms");
}
