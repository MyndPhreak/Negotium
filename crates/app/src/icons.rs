//! Program icons for the app list. Extraction calls the shell and GDI, so it runs on one loader
//! thread (never the UI thread, never in a frame); the UI thread keeps what came back by image
//! path, so each path is decoded at most once per run.
use negotium_actions::startup::ComApartment;
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};
use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::sync::mpsc;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDIBits,
    GetObjectW, HBITMAP, HDC,
};
use windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES;
use windows::Win32::UI::Shell::{SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON, SHGetFileInfoW};
use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO};
use windows::core::HSTRING;

/// Drawn at 24 px on cards and 16 px on process rows; 32 px keeps them sharp up to 133 % scaling.
pub const ICON_PX: u32 = 32;

/// An icon as extracted: `ICON_PX` square, straight (non-premultiplied) alpha, as
/// `Image::from_rgba8` takes it.
pub type IconPixels = SharedPixelBuffer<Rgba8Pixel>;

/// Where the cache sends a path it has not seen: the loader in the app, a recorder in tests.
pub trait RequestIcon {
    fn request(&self, path: &str);
}

/// One background thread that extracts icons in the order they were asked for. Dropping the
/// loader ends the thread once it has finished the icon in hand (never waited for).
pub struct IconLoader {
    tx: mpsc::Sender<String>,
}

impl IconLoader {
    /// `done` runs on the loader thread with each result; the app posts it to the UI thread.
    pub fn spawn(
        extract: impl Fn(&str) -> Option<IconPixels> + Send + 'static,
        done: impl Fn(String, Option<IconPixels>) + Send + 'static,
    ) -> IconLoader {
        let (tx, rx) = mpsc::channel::<String>();
        // Without a thread (spawn failed) requests go nowhere: rows simply show no icons.
        let _ = std::thread::Builder::new().name("negotium-icons".into()).spawn(move || {
            for path in rx {
                // A panicking extraction must not end the thread: that icon is just missing.
                let px = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| extract(&path))).ok().flatten();
                done(path, px);
            }
        });
        IconLoader { tx }
    }

    /// Queues `path`; never blocks.
    pub fn request(&self, path: &str) {
        let _ = self.tx.send(path.to_string());
    }
}

impl RequestIcon for IconLoader {
    fn request(&self, path: &str) {
        IconLoader::request(self, path);
    }
}

/// The icons the UI thread has, by image path. A path is requested the first time it is seen and
/// never again: a failure is kept as `None`, so it is not re-extracted every frame.
#[derive(Default)]
pub struct IconCache {
    loaded: HashMap<String, Option<Image>>,
    asked: HashSet<String>,
}

impl IconCache {
    /// The icon for `path` if it has arrived; on first sight it is requested from `loader`.
    pub fn get(&mut self, path: &str, loader: &(impl RequestIcon + ?Sized)) -> Option<Image> {
        if let Some(icon) = self.loaded.get(path) {
            return icon.clone();
        }
        if self.asked.insert(path.to_string()) {
            loader.request(path);
        }
        None
    }

    /// An extraction came back (`None`: the file has no icon, or is gone).
    pub fn put(&mut self, path: String, px: Option<IconPixels>) {
        self.asked.remove(&path);
        self.loaded.insert(path, px.map(Image::from_rgba8));
    }
}

thread_local! {
    /// The shell's icon lookup wants COM on the calling thread: entered once per thread (the
    /// loader's), left when the thread ends.
    static COM: ComApartment = ComApartment::enter();
}

/// The program icon of the file at `path`, `ICON_PX` square; `None` if the file is missing or has
/// no colour icon. Calls the shell and GDI: never call it on the UI thread.
pub fn extract(path: &str) -> Option<IconPixels> {
    if path.is_empty() {
        return None;
    }
    COM.with(|_| {});
    let mut info = SHFILEINFOW::default();
    // SAFETY: `info` is a valid SHFILEINFOW of the size passed; the path is a NUL-terminated
    // HSTRING that outlives the call. On success the icon in `info.hIcon` is ours to destroy.
    let found = unsafe {
        SHGetFileInfoW(
            &HSTRING::from(path),
            FILE_FLAGS_AND_ATTRIBUTES(0),
            Some(&mut info),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_LARGEICON,
        )
    };
    if found == 0 || info.hIcon.is_invalid() {
        return None;
    }
    let px = icon_rgba(info.hIcon);
    // SAFETY: the icon came from SHGetFileInfoW above and is destroyed exactly once.
    let _ = unsafe { DestroyIcon(info.hIcon) };
    px.map(|(w, h, rgba)| fit(&rgba, w, h, ICON_PX))
}

/// The bitmaps GetIconInfo hands over, deleted on every path out.
struct IconBitmaps(ICONINFO);

impl Drop for IconBitmaps {
    fn drop(&mut self) {
        for bm in [self.0.hbmColor, self.0.hbmMask] {
            if !bm.is_invalid() {
                // SAFETY: GetIconInfo created these copies for the caller to delete, once.
                let _ = unsafe { DeleteObject(bm.into()) };
            }
        }
    }
}

/// A memory DC for GetDIBits, deleted on drop.
struct MemoryDc(HDC);

impl Drop for MemoryDc {
    fn drop(&mut self) {
        // SAFETY: created by CreateCompatibleDC and deleted once.
        let _ = unsafe { DeleteDC(self.0) };
    }
}

/// The icon's pixels as straight-alpha RGBA at its own size. An icon whose colour bitmap has no
/// alpha at all (an old 24-bit one) takes its visibility from the mask. Monochrome icons (no
/// colour bitmap) are left out.
fn icon_rgba(icon: HICON) -> Option<(u32, u32, Vec<u8>)> {
    let mut info = ICONINFO::default();
    // SAFETY: `icon` is a valid icon handle and `info` a valid out-pointer.
    unsafe { GetIconInfo(icon, &mut info) }.ok()?;
    let bitmaps = IconBitmaps(info);
    if bitmaps.0.hbmColor.is_invalid() {
        return None;
    }
    let mut bm = BITMAP::default();
    // SAFETY: `bm` is a BITMAP of the size passed; the handle is a live bitmap.
    let got = unsafe {
        GetObjectW(bitmaps.0.hbmColor.into(), std::mem::size_of::<BITMAP>() as i32, Some(&mut bm as *mut BITMAP as *mut c_void))
    };
    let (w, h) = (u32::try_from(bm.bmWidth).ok()?, u32::try_from(bm.bmHeight).ok()?);
    if got == 0 || !(1..=256).contains(&w) || !(1..=256).contains(&h) {
        return None;
    }
    // SAFETY: a memory DC compatible with the screen, deleted by the guard.
    let dc = MemoryDc(unsafe { CreateCompatibleDC(None) });
    if dc.0.is_invalid() {
        return None;
    }
    let mut px = dib_bgra(dc.0, bitmaps.0.hbmColor, w, h)?;
    if px.as_chunks::<4>().0.iter().all(|p| p[3] == 0) {
        let mask = dib_bgra(dc.0, bitmaps.0.hbmMask, w, h)?;
        // The AND mask: black where the icon shows, white where the background does.
        for (p, m) in px.as_chunks_mut::<4>().0.iter_mut().zip(mask.as_chunks::<4>().0) {
            p[3] = if m[..3] == [0, 0, 0] { 255 } else { 0 };
        }
    }
    for p in px.as_chunks_mut::<4>().0 {
        p.swap(0, 2); // BGRA -> RGBA
    }
    Some((w, h, px))
}

/// `hbm`'s top `h` rows as top-down 32-bit BGRA.
fn dib_bgra(dc: HDC, hbm: HBITMAP, w: u32, h: u32) -> Option<Vec<u8>> {
    if hbm.is_invalid() {
        return None;
    }
    let mut bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w as i32,
            biHeight: -(h as i32), // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut px = vec![0u8; (w * h * 4) as usize];
    // SAFETY: `px` holds exactly `h` rows of `w` 32-bit pixels, the format `bmi` asks for; `hbm`
    // is not selected into any DC.
    let lines = unsafe { GetDIBits(dc, hbm, 0, h, Some(px.as_mut_ptr() as *mut c_void), &mut bmi, DIB_RGB_COLORS) };
    (lines == h as i32).then_some(px)
}

/// Scales straight-alpha RGBA `w`×`h` to `size` square: an area average weighted by alpha (so
/// transparent pixels lend no colour to the edges) when shrinking, nearest pixel when growing.
/// The shell's large icon follows the system DPI, so it is 32 px only at 100 %.
pub fn fit(rgba: &[u8], w: u32, h: u32, size: u32) -> IconPixels {
    let mut out = SharedPixelBuffer::<Rgba8Pixel>::new(size, size);
    if (w, h) == (size, size) {
        for (o, p) in out.make_mut_slice().iter_mut().zip(rgba.as_chunks::<4>().0) {
            *o = Rgba8Pixel { r: p[0], g: p[1], b: p[2], a: p[3] };
        }
        return out;
    }
    let span = |i: u32, n: u32| {
        let from = i * n / size;
        (from, ((i + 1) * n / size).max(from + 1))
    };
    for (i, o) in out.make_mut_slice().iter_mut().enumerate() {
        let (x, y) = (i as u32 % size, i as u32 / size);
        let ((x0, x1), (y0, y1)) = (span(x, w), span(y, h));
        let (mut c, mut a, mut n) = ([0u64; 3], 0u64, 0u64);
        for sy in y0..y1 {
            for sx in x0..x1 {
                let p = &rgba[((sy * w + sx) * 4) as usize..][..4];
                let pa = p[3] as u64;
                for (sum, v) in c.iter_mut().zip(p) {
                    *sum += *v as u64 * pa;
                }
                a += pa;
                n += 1;
            }
        }
        let ch = |k: usize| c[k].checked_div(a).unwrap_or(0) as u8;
        *o = Rgba8Pixel { r: ch(0), g: ch(1), b: ch(2), a: (a / n) as u8 };
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::time::Duration;

    #[derive(Default)]
    struct Recorder(RefCell<Vec<String>>);

    impl RequestIcon for Recorder {
        fn request(&self, path: &str) {
            self.0.borrow_mut().push(path.to_string());
        }
    }

    #[cfg(windows)]
    #[test]
    fn extracts_notepads_icon() {
        let px = extract(r"C:\Windows\System32\notepad.exe").expect("notepad has an icon");
        assert_eq!((px.width(), px.height()), (ICON_PX, ICON_PX));
        assert!(px.as_slice().iter().any(|p| p.a > 0), "some of it is visible");
        // Slint draws `Image::from_rgba8` as straight alpha. Premultiplied pixels never have a
        // channel above their alpha; a real icon's soft edge, kept straight, does. (In this one
        // test: two extractions at once on test threads sometimes come back empty.)
        let soft: Vec<_> = px.as_slice().iter().filter(|p| p.a > 0 && p.a < 255).collect();
        assert!(!soft.is_empty(), "a soft edge to look at");
        assert!(soft.iter().any(|p| p.r.max(p.g).max(p.b) > p.a), "straight alpha, not premultiplied: {} soft pixels", soft.len());
    }

    #[test]
    fn a_missing_file_has_no_icon() {
        let path = std::env::temp_dir().join("negotium-no-such-dir-7f3a").join("missing.exe");
        assert!(extract(path.to_str().unwrap()).is_none());
        assert!(extract("").is_none());
    }

    #[test]
    fn the_cache_requests_each_path_once() {
        let (mut cache, loader) = (IconCache::default(), Recorder::default());
        for _ in 0..3 {
            assert!(cache.get(r"C:\app.exe", &loader).is_none(), "not loaded yet");
        }
        assert_eq!(*loader.0.borrow(), [r"C:\app.exe"]);
        cache.put(r"C:\app.exe".into(), Some(SharedPixelBuffer::new(ICON_PX, ICON_PX)));
        assert!(cache.get(r"C:\app.exe", &loader).is_some());
        assert_eq!(loader.0.borrow().len(), 1, "a loaded icon is never asked for again");
    }

    #[test]
    fn a_failed_icon_is_remembered_as_missing() {
        let (mut cache, loader) = (IconCache::default(), Recorder::default());
        assert!(cache.get(r"C:\app.exe", &loader).is_none());
        cache.put(r"C:\app.exe".into(), None);
        assert!(cache.get(r"C:\app.exe", &loader).is_none());
        assert!(cache.get(r"C:\app.exe", &loader).is_none());
        assert_eq!(loader.0.borrow().len(), 1, "a failure is never re-requested");
    }

    #[test]
    fn the_loader_extracts_off_the_calling_thread() {
        let (seen_tx, seen) = mpsc::channel();
        let (done_tx, done) = mpsc::channel();
        let loader = IconLoader::spawn(
            move |path| {
                seen_tx.send((path.to_string(), std::thread::current().id())).unwrap();
                None
            },
            move |path, px| done_tx.send((path, px.is_some())).unwrap(),
        );
        loader.request(r"C:\app.exe");
        let (path, thread) = seen.recv_timeout(Duration::from_secs(5)).expect("extracted");
        assert_eq!(path, r"C:\app.exe");
        assert_ne!(thread, std::thread::current().id());
        assert_eq!(done.recv_timeout(Duration::from_secs(5)).unwrap(), (r"C:\app.exe".to_string(), false));
    }

    /// A 48 px icon (150 % system DPI) comes out at 32 px; transparent pixels lend no colour.
    #[test]
    fn a_larger_icon_is_scaled_down_by_area() {
        // 2×2 → 1×1: one opaque red pixel and three transparent white ones.
        let rgba = [255, 0, 0, 255, 255, 255, 255, 0, 255, 255, 255, 0, 255, 255, 255, 0];
        let px = fit(&rgba, 2, 2, 1);
        let p = px.as_slice()[0];
        assert_eq!((p.r, p.g, p.b, p.a), (255, 0, 0, 63));
        assert_eq!(fit(&vec![9; 48 * 48 * 4], 48, 48, ICON_PX).width(), ICON_PX);
    }
}
