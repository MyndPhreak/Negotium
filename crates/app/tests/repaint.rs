//! The software renderer draws only what changed since its last frame. When Windows drops the
//! window's pixels (a GPU-heavy game can make the compositor do that), the main window asks for a
//! full repaint through `MainWindow.repaint-all`: flipping it must make the next frame cover the
//! whole window.
use negotium::MainWindow;
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType, Rgb565Pixel};
use slint::platform::{Platform, WindowAdapter};
use slint::{ComponentHandle, PhysicalSize};
use std::rc::Rc;

const W: u32 = 1000;
const H: u32 = 720;

/// One window that keeps its buffer between frames, as softbuffer's does on Windows.
struct Reused(Rc<MinimalSoftwareWindow>);

impl Platform for Reused {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

/// Renders one frame into `buf` and returns the area it drew: origin and size, in physical px.
fn draw(window: &MinimalSoftwareWindow, buf: &mut [Rgb565Pixel]) -> ((i32, i32), (u32, u32)) {
    slint::platform::update_timers_and_animations();
    let mut drawn = ((0, 0), (0, 0));
    window.draw_if_needed(|renderer| {
        let region = renderer.render(buf, W as usize);
        let (o, s) = (region.bounding_box_origin(), region.bounding_box_size());
        drawn = ((o.x, o.y), (s.width, s.height));
    });
    drawn
}

#[test]
fn flipping_repaint_all_redraws_the_whole_window() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
    slint::platform::set_platform(Box::new(Reused(window.clone()))).expect("one platform per test thread");
    let ui = MainWindow::new().unwrap();
    window.set_size(PhysicalSize::new(W, H));
    ui.show().unwrap();
    let mut buf = vec![Rgb565Pixel::default(); (W * H) as usize];
    let whole = ((0, 0), (W, H));
    assert_eq!(draw(&window, &mut buf), whole, "the first frame draws everything");
    assert_ne!(draw(&window, &mut buf), whole, "an unchanged frame does not redraw the whole window");

    ui.set_repaint_all(!ui.get_repaint_all());
    assert_eq!(draw(&window, &mut buf), whole, "after the flip the whole window is drawn again");
    assert_ne!(draw(&window, &mut buf), whole, "and only once");
}
