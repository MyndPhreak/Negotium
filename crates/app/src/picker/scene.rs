//! The overlay for one monitor: where the clear hole, outline, label card and hint bar go
//! (`layout`, pure), and the Direct2D drawing of that (`render`).
use windows::Win32::Graphics::Direct2D::Common::{D2D1_COLOR_F, D2D_RECT_F};
use windows::Win32::Graphics::Direct2D::{
    ID2D1RenderTarget, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_CAP_STYLE_FLAT, D2D1_DASH_STYLE_DASH,
    D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_LINE_JOIN_MITER, D2D1_ROUNDED_RECT, D2D1_STROKE_STYLE_PROPERTIES,
};
use windows::Win32::Graphics::DirectWrite::{
    IDWriteFactory, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_NORMAL,
    DWRITE_MEASURING_MODE_NATURAL, DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_CENTER,
    DWRITE_WORD_WRAPPING_NO_WRAP,
};

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
    let scale = m.dpi as f32 / 96.0;
    let px = |v: f32| (v * scale).round() as i32;
    let (w, h) = (m.rect.right - m.rect.left, m.rect.bottom - m.rect.top);

    let hole = target.and_then(|(t, _)| {
        let clipped = Rect {
            left: (t.left - m.rect.left).max(0),
            top: (t.top - m.rect.top).max(0),
            right: (t.right - m.rect.left).min(w),
            bottom: (t.bottom - m.rect.top).min(h),
        };
        (clipped.left < clipped.right && clipped.top < clipped.bottom).then_some(clipped)
    });

    let (outline, label) = match (hole, target) {
        (Some(hole), Some((_, l))) => {
            let (cw, ch, gap) = (px(420.0).min(w - px(16.0)).max(0), px(56.0), px(8.0));
            // Below the target's bottom-left; inside its bottom edge when the screen ends first.
            let below = hole.bottom + gap;
            let top = if below + ch <= h { below } else { hole.bottom - gap - ch };
            let left = hole.left.min(w - cw).max(0);
            let top = top.min(h - ch).max(0);
            (l.outline, Some((Rect { left, top, right: left + cw, bottom: top + ch }, l.clone())))
        }
        _ => (Outline::None, None),
    };

    let hint = cursor_here.then(|| {
        let (hw, hh) = (px(560.0).min(w - px(16.0)).max(0), px(32.0));
        let left = (w - hw) / 2;
        let top = px(10.0);
        Rect { left, top, right: left + hw, bottom: top + hh }
    });

    Scene { size: (w.max(0) as u32, h.max(0) as u32), hole, outline, label, hint, scale }
}

const CARD: D2D1_COLOR_F = D2D1_COLOR_F { r: 0x20 as f32 / 255.0, g: 0x20 as f32 / 255.0, b: 0x20 as f32 / 255.0, a: 1.0 };
const WHITE: D2D1_COLOR_F = D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 1.0 };
const GREY: D2D1_COLOR_F = D2D1_COLOR_F { r: 0xBB as f32 / 255.0, g: 0xBB as f32 / 255.0, b: 0xBB as f32 / 255.0, a: 1.0 };

fn rgb(r: u8, g: u8, b: u8) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r: r as f32 / 255.0, g: g as f32 / 255.0, b: b as f32 / 255.0, a: 1.0 }
}

fn rf(r: Rect) -> D2D_RECT_F {
    D2D_RECT_F { left: r.left as f32, top: r.top as f32, right: r.right as f32, bottom: r.bottom as f32 }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// Draws `scene` in physical pixels. `layout` already applied the DPI scale, so this resets the
/// target to 96 DPI; otherwise Direct2D would scale every coordinate a second time.
pub fn render(rt: &ID2D1RenderTarget, text: &IDWriteFactory, scene: &Scene) -> windows::core::Result<()> {
    let s = scene.scale;
    let (w, h) = (scene.size.0 as i32, scene.size.1 as i32);
    // SAFETY: every call is on live COM objects borrowed for this function; all pointers passed
    // point to locals that outlive the call. `EndDraw` runs even if a drawing step fails, so the
    // target is never left mid-draw.
    unsafe {
        rt.SetDpi(96.0, 96.0);
        rt.BeginDraw();
        let drawn = draw(rt, text, scene, s, w, h);
        let ended = rt.EndDraw(None, None);
        drawn.and(ended)
    }
}

/// # Safety
/// `rt` must be between `BeginDraw` and `EndDraw`.
unsafe fn draw(rt: &ID2D1RenderTarget, text: &IDWriteFactory, scene: &Scene, s: f32, w: i32, h: i32) -> windows::core::Result<()> {
    unsafe {
        rt.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));

        // Dim everything except the hole: four rects around it.
        let dim = rt.CreateSolidColorBrush(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0x8C as f32 / 255.0 }, None)?;
        let full = Rect { left: 0, top: 0, right: w, bottom: h };
        let dims = match scene.hole {
            None => vec![full],
            Some(o) => vec![
                Rect { bottom: o.top, ..full },
                Rect { top: o.bottom, ..full },
                Rect { top: o.top, bottom: o.bottom, right: o.left, ..full },
                Rect { top: o.top, bottom: o.bottom, left: o.right, ..full },
            ],
        };
        for d in dims.into_iter().filter(|d| d.left < d.right && d.top < d.bottom) {
            rt.FillRectangle(&rf(d), &dim);
        }

        let wide_enough = |o: Rect| (o.right - o.left) as f32 >= 3.0 * s && (o.bottom - o.top) as f32 >= 3.0 * s;
        // A hole thinner than the stroke would give an inverted inset rect, so it gets no outline.
        if let (Some(o), Some(colour)) = (scene.hole.filter(|o| wide_enough(*o)), outline_colour(scene.outline)) {
            let factory = rt.GetFactory()?;
            let style = factory.CreateStrokeStyle(
                &D2D1_STROKE_STYLE_PROPERTIES {
                    startCap: D2D1_CAP_STYLE_FLAT,
                    endCap: D2D1_CAP_STYLE_FLAT,
                    dashCap: D2D1_CAP_STYLE_FLAT,
                    lineJoin: D2D1_LINE_JOIN_MITER,
                    miterLimit: 10.0,
                    dashStyle: D2D1_DASH_STYLE_DASH,
                    dashOffset: 0.0,
                },
                None,
            )?;
            let width = 3.0 * s;
            // Stroke inside the hole so the whole line stays on the target.
            let mut line = rf(o);
            let inset = width / 2.0;
            line.left += inset;
            line.top += inset;
            line.right -= inset;
            line.bottom -= inset;
            let brush = rt.CreateSolidColorBrush(&colour, None)?;
            rt.DrawRectangle(&line, &brush, width, &style);
        }

        let radius = 6.0 * s;
        let white = rt.CreateSolidColorBrush(&WHITE, None)?;
        if let Some((card, label)) = &scene.label {
            let brush = rt.CreateSolidColorBrush(&CARD, None)?;
            rt.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: rf(*card), radiusX: radius, radiusY: radius }, &brush);
            let grey = rt.CreateSolidColorBrush(&GREY, None)?;
            let pad = 12.0 * s;
            let c = rf(*card);
            let title = format(text, 14.0 * s)?;
            let detail = format(text, 12.0 * s)?;
            rt.PushAxisAlignedClip(&c, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            let t = D2D_RECT_F { left: c.left + pad, top: c.top + 8.0 * s, right: c.right - pad, bottom: c.top + 30.0 * s };
            rt.DrawText(&wide(&label.title), &title, &t, &white, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
            let d = D2D_RECT_F { top: c.top + 30.0 * s, bottom: c.bottom - 6.0 * s, ..t };
            rt.DrawText(&wide(&label.detail), &detail, &d, &grey, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
            rt.PopAxisAlignedClip();
        }

        if let Some(bar) = scene.hint {
            let brush = rt.CreateSolidColorBrush(&D2D1_COLOR_F { a: 0.9, ..CARD }, None)?;
            rt.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: rf(bar), radiusX: radius, radiusY: radius }, &brush);
            let f = format(text, 13.0 * s)?;
            f.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
            f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            rt.DrawText(&wide(HINT), &f, &rf(bar), &white, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
        }
        Ok(())
    }
}

fn outline_colour(o: Outline) -> Option<D2D1_COLOR_F> {
    match o {
        Outline::None => None,
        Outline::Accent => Some(rgb(0x4C, 0xC2, 0xFF)),
        Outline::Red => Some(rgb(0xE8, 0x11, 0x23)),
    }
}

fn format(text: &IDWriteFactory, size: f32) -> windows::core::Result<windows::Win32::Graphics::DirectWrite::IDWriteTextFormat> {
    // SAFETY: `text` is a live factory; the wide-string literal is NUL-terminated by `w!`.
    unsafe {
        let f = text.CreateTextFormat(
            windows::core::w!("Segoe UI"),
            None,
            DWRITE_FONT_WEIGHT_NORMAL,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            size,
            windows::core::w!("en-us"),
        )?;
        f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
        Ok(f)
    }
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
        offscreen_dpi(w, h, 96.0)
    }

    fn offscreen_dpi(w: u32, h: u32, dpi: f32) -> Offscreen {
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
                dpiX: dpi,
                dpiY: dpi,
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
        assert!(row.contains(&[0x23, 0x11, 0xE8, 0xFF]), "no opaque red: {row:?}");
        assert!(row.iter().any(|p| p[3] == 0), "no gap in the dashes");
    }

    #[test]
    fn render_does_not_scale_twice_on_a_high_dpi_target() {
        let m = mon(r(0, 0, 300, 150), 144);
        let l = lbl(Outline::None);
        let sc = layout(&m, Some((r(75, 30, 225, 120), &l)), false);
        let hole = sc.hole.unwrap();
        assert_eq!(hole, r(75, 30, 225, 120));
        let o = offscreen_dpi(300, 150, 144.0);
        render(&o.rt, &o.text, &Scene { label: None, ..sc }).unwrap();
        let px = pixels(&o, 300, 150);
        // Just outside and just inside each hole edge, in physical pixels.
        assert_eq!(pixel(&px, 300, 74, 75)[3], 0x8C);
        assert_eq!(pixel(&px, 300, 75, 75)[3], 0);
        assert_eq!(pixel(&px, 300, 224, 75)[3], 0);
        assert_eq!(pixel(&px, 300, 225, 75)[3], 0x8C);
        assert_eq!(pixel(&px, 300, 150, 29)[3], 0x8C);
        assert_eq!(pixel(&px, 300, 150, 30)[3], 0);
    }

    #[test]
    fn render_draws_the_label_card_and_the_hint_bar() {
        let o = offscreen(600, 300);
        let l = lbl(Outline::Accent);
        let m = mon(r(0, 0, 600, 300), 96);
        let sc = layout(&m, Some((r(0, 0, 600, 300), &l)), true);
        let (card, _) = sc.label.clone().unwrap();
        let bar = sc.hint.unwrap();
        render(&o.rt, &o.text, &sc).unwrap();
        let px = pixels(&o, 600, 300);
        // The card's bottom-right corner area is clear of text and of the rounded corner.
        let p = pixel(&px, 600, (card.right - 10) as u32, (card.bottom - 10) as u32);
        assert_eq!(p, [0x20, 0x20, 0x20, 0xFF]);
        // The hole covers the monitor, so the bar is not over the dim. Its left padding is clear of the text.
        let a = pixel(&px, 600, (bar.left + 10) as u32, (bar.top + 16) as u32)[3];
        assert!(a.abs_diff(0xE6) <= 1, "hint alpha {a:#x}");
    }

    #[test]
    fn hint_bar_is_clamped_into_a_narrow_monitor() {
        let m = mon(r(0, 0, 400, 300), 96);
        let h = layout(&m, None, true).hint.unwrap();
        assert_eq!(h, r(8, 10, 392, 42));
    }

    #[test]
    fn a_hole_thinner_than_the_stroke_gets_no_outline() {
        let o = offscreen(200, 100);
        let sc = Scene { hole: Some(r(50, 20, 52, 80)), ..scene(Outline::Red) };
        render(&o.rt, &o.text, &sc).unwrap();
        let px = pixels(&o, 200, 100);
        assert!((0..100).all(|y| pixel(&px, 200, 51, y)[3] != 0xFF));
    }
}
