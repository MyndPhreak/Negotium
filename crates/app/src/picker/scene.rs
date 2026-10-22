//! The overlay for one monitor: where the clear hole, outline, label card and hint bar go
//! (`layout`, pure), and the Direct2D drawing of that (`render`).
use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;
use windows::Win32::Graphics::DirectWrite::IDWriteFactory;

use super::label::{Label, Outline};
use super::target::Rect;
use super::win::Monitor;

pub const HINT: &str = "Click a window to end it · Shift+click: end now · Esc: cancel";

/// One monitor's overlay. All rects are monitor-local pixels.
pub struct Scene {
    pub size: (u32, u32),
    pub hole: Option<Rect>,
    pub outline: Outline,
    pub label: Option<(Rect, Label)>,
    pub hint: Option<Rect>,
    pub scale: f32,
}

/// All rects in monitor-local px. `cursor_here` puts the hint bar on this monitor.
pub fn layout(m: &Monitor, target: Option<(Rect, &Label)>, cursor_here: bool) -> Scene {
    let _ = (m, target, cursor_here);
    todo!()
}

pub fn render(rt: &ID2D1RenderTarget, text: &IDWriteFactory, scene: &Scene) -> windows::core::Result<()> {
    let _ = (rt, text, scene);
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Graphics::Direct2D::Common::{D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_PIXEL_FORMAT};
    use windows::Win32::Graphics::Direct2D::{
        D2D1CreateFactory, ID2D1Factory, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_FEATURE_LEVEL_DEFAULT,
        D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_SOFTWARE, D2D1_RENDER_TARGET_USAGE_NONE,
    };
    use windows::Win32::Graphics::DirectWrite::{DWriteCreateFactory, DWRITE_FACTORY_TYPE_SHARED};
    use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
    use windows::Win32::Graphics::Imaging::{
        CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICBitmap, IWICImagingFactory, WICBitmapCacheOnLoad,
        WICBitmapLockRead, WICRect,
    };
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};

    fn r(left: i32, top: i32, right: i32, bottom: i32) -> Rect {
        Rect { left, top, right, bottom }
    }

    fn mon(rect: Rect, dpi: u32) -> Monitor {
        Monitor { rect, dpi }
    }

    fn lbl(outline: Outline) -> Label {
        Label { title: "Notepad".to_string(), detail: "2 processes · 30 MB".to_string(), outline }
    }

    #[test]
    fn negative_origin_monitor_maps_target_into_local_space() {
        let m = mon(r(-1920, 0, 0, 1080), 96);
        let l = lbl(Outline::Accent);
        let s = layout(&m, Some((r(-1000, 100, -500, 400), &l)), false);
        assert_eq!(s.size, (1920, 1080));
        assert_eq!(s.hole, Some(r(920, 100, 1420, 400)));
        assert_eq!(s.outline, Outline::Accent);
    }

    #[test]
    fn target_on_another_monitor_leaves_no_hole_or_label() {
        let m = mon(r(0, 0, 1920, 1080), 96);
        let l = lbl(Outline::Red);
        let s = layout(&m, Some((r(2000, 100, 2500, 400), &l)), false);
        assert_eq!(s.hole, None);
        assert_eq!(s.label, None);
        assert_eq!(s.outline, Outline::None);
    }

    #[test]
    fn target_straddling_the_edge_is_clipped_to_the_monitor() {
        let m = mon(r(0, 0, 1920, 1080), 96);
        let l = lbl(Outline::Accent);
        let s = layout(&m, Some((r(1800, 100, 2500, 400), &l)), false);
        assert_eq!(s.hole, Some(r(1800, 100, 1920, 400)));
    }

    #[test]
    fn label_sits_below_the_target() {
        let m = mon(r(0, 0, 1920, 1080), 96);
        let l = lbl(Outline::Accent);
        let s = layout(&m, Some((r(100, 100, 500, 400), &l)), false);
        let (rect, label) = s.label.unwrap();
        assert_eq!(rect, r(100, 408, 520, 464));
        assert_eq!(label, l);
    }

    #[test]
    fn label_flips_inside_when_no_room_below() {
        let m = mon(r(0, 0, 1920, 1080), 96);
        let l = lbl(Outline::Accent);
        let s = layout(&m, Some((r(100, 100, 500, 1060), &l)), false);
        let (rect, _) = s.label.unwrap();
        assert_eq!(rect, r(100, 996, 520, 1052));
    }

    #[test]
    fn label_is_clamped_into_the_monitor() {
        let m = mon(r(0, 0, 1920, 1080), 96);
        let l = lbl(Outline::Accent);
        let s = layout(&m, Some((r(1800, 100, 1900, 400), &l)), false);
        let (rect, _) = s.label.unwrap();
        assert_eq!((rect.left, rect.right), (1500, 1920));
        // A target that is mostly off the top cannot push the card off-screen either.
        let s = layout(&m, Some((r(100, -500, 400, 20), &l)), false);
        let (rect, _) = s.label.unwrap();
        assert!(rect.left >= 0 && rect.top >= 0 && rect.bottom <= 1080);
    }

    #[test]
    fn hint_only_on_the_cursor_monitor() {
        let m = mon(r(0, 0, 1920, 1080), 96);
        assert_eq!(layout(&m, None, false).hint, None);
        assert_eq!(layout(&m, None, true).hint, Some(r(680, 10, 1240, 42)));
    }

    #[test]
    fn scale_is_dpi_over_96() {
        let m = mon(r(0, 0, 3000, 2000), 144);
        let l = lbl(Outline::Accent);
        let s = layout(&m, Some((r(100, 100, 500, 400), &l)), true);
        assert_eq!(s.scale, 1.5);
        assert_eq!(s.hint, Some(r(1080, 15, 1920, 63)));
        assert_eq!(s.label.unwrap().0, r(100, 412, 730, 496));
    }

    struct Offscreen {
        bitmap: IWICBitmap,
        rt: ID2D1RenderTarget,
        text: IDWriteFactory,
    }

    fn offscreen(w: u32, h: u32) -> Offscreen {
        // SAFETY: plain COM calls on live objects; the created objects are owned by the returned struct.
        unsafe {
            // S_FALSE / RPC_E_CHANGED_MODE only mean COM is already initialised on this thread.
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).unwrap();
            let bitmap = wic.CreateBitmap(w, h, &GUID_WICPixelFormat32bppPBGRA, WICBitmapCacheOnLoad).unwrap();
            let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).unwrap();
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: 96.0,
                dpiY: 96.0,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };
            let rt = d2d.CreateWicBitmapRenderTarget(&bitmap, &props).unwrap();
            let text: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).unwrap();
            Offscreen { bitmap, rt, text }
        }
    }

    /// BGRA pixels, row-major, `w * h * 4` bytes.
    fn pixels(o: &Offscreen, w: u32, h: u32) -> Vec<u8> {
        let rect = WICRect { X: 0, Y: 0, Width: w as i32, Height: h as i32 };
        // SAFETY: the lock covers the whole bitmap; the data is copied before the lock drops.
        unsafe {
            let lock = o.bitmap.Lock(&rect, WICBitmapLockRead.0 as u32).unwrap();
            let mut len = 0u32;
            let mut ptr = std::ptr::null_mut();
            lock.GetDataPointer(&mut len, &mut ptr).unwrap();
            std::slice::from_raw_parts(ptr, len as usize).to_vec()
        }
    }

    fn pixel(px: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * w + x) * 4) as usize;
        [px[i], px[i + 1], px[i + 2], px[i + 3]]
    }

    fn scene(outline: Outline) -> Scene {
        Scene { size: (200, 100), hole: Some(r(50, 20, 150, 80)), outline, label: None, hint: None, scale: 1.0 }
    }

    #[test]
    fn render_dims_outside_and_clears_the_hole() {
        let o = offscreen(200, 100);
        render(&o.rt, &o.text, &scene(Outline::None)).unwrap();
        let px = pixels(&o, 200, 100);
        assert_eq!(pixel(&px, 200, 10, 10)[3], 0x8C);
        assert_eq!(pixel(&px, 200, 100, 50)[3], 0);
    }

    #[test]
    fn render_draws_a_dashed_red_outline() {
        let o = offscreen(200, 100);
        render(&o.rt, &o.text, &scene(Outline::Red)).unwrap();
        let px = pixels(&o, 200, 100);
        // Row 21 is inside the 3 px stroke (rows 20..23), which is drawn inside the hole.
        let row: Vec<[u8; 4]> = (60..140).map(|x| pixel(&px, 200, x, 21)).collect();
        assert!(row.iter().any(|p| *p == [0x23, 0x11, 0xE8, 0xFF]), "no opaque red: {row:?}");
        assert!(row.iter().any(|p| p[3] == 0), "no gap in the dashes");
    }
}
