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
