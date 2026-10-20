//! The tray icon (spec §5.5): the app icon or, with Settings > Show CPU in tray icon, a small CPU
//! graph redrawn for each history sample the worker sends, with "Negotium\nCPU: 17%" on hover.
//!
//! UI thread (rulings P3c-6, PF11): the pixels are drawn here, and Slint pushes a changed `icon`
//! or `tooltip` to the shell itself (Shell_NotifyIcon NIM_MODIFY), both on the UI thread. That is an
//! allowed exception to "the UI thread never blocks": a 32 × 32 icon takes well under a millisecond
//! once a second, and the shell call is bounded by PF2: while Explorer's taskbar window
//! (`Shell_TrayWnd`) is hung, nothing is pushed (Shell_NotifyIcon would wait on it); the newest
//! look follows once it answers again.
use crate::NegotiumTray;
use crate::trayicon::{self, Colors, Rgb};
use negotium_history::{History, Series};
use std::cell::RefCell;
use std::time::Duration;

/// Windows' default accent, when the user's can't be read.
pub const DEFAULT_ACCENT: Rgb = (0x00, 0x78, 0xD4);

/// How soon a look held back by a hung Explorer is tried again when no sample is coming to retry
/// it (the switch is off): the tray's own pace.
const HUNG_RETRY: Duration = Duration::from_secs(2);

/// One history sample for the tray: the last minute of CPU use (crate::worker sends it).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TraySample {
    pub end_ms: u64,
    pub cpu: Option<f32>,
    pub cpu_temp_c: Option<f32>,
    pub gpu: Option<f32>,
    pub gpu_temp_c: Option<f32>,
    pub mem_in_use: Option<u64>,
    pub mem_total: Option<u64>,
    pub points: Vec<(u64, Option<f32>)>,
}

impl TraySample {
    pub fn from_history(h: &History, end_ms: u64, cpu: Option<f32>) -> TraySample {
        TraySample {
            end_ms,
            cpu,
            cpu_temp_c: None,
            gpu: None,
            gpu_temp_c: None,
            mem_in_use: None,
            mem_total: None,
            points: h.points(&Series::Cpu, end_ms.saturating_sub(trayicon::SPAN_MS)).collect(),
        }
    }

    pub fn from_snapshot(h: &History, snap: &negotium_snapshot::Snapshot) -> TraySample {
        let (gpu, gpu_temp_c) = {
            let busiest = snap.totals.gpus.as_deref().and_then(crate::view::busiest_gpu);
            let util = snap.totals.gpu_pct.or_else(|| busiest.map(|g| g.util_pct));
            let temp = busiest.and_then(|g| g.temp_c);
            (util, temp)
        };
        let (mem_in_use, mem_total) = match snap.totals.memory.as_ref().filter(|m| m.total > 0) {
            Some(m) => (Some(m.in_use()), Some(m.total)),
            None => (None, None),
        };
        TraySample {
            end_ms: snap.taken_at_ms,
            cpu: snap.totals.cpu_pct,
            cpu_temp_c: snap.totals.cpu_temp_c,
            gpu,
            gpu_temp_c,
            mem_in_use,
            mem_total,
            points: h.points(&Series::Cpu, snap.taken_at_ms.saturating_sub(trayicon::SPAN_MS)).collect(),
        }
    }
}

/// What the tray shows: a graph (RGBA pixels and their side) or the app icon, and the hover text.
#[derive(Clone, Debug, PartialEq)]
pub struct TrayLook {
    pub icon: Option<(Vec<u8>, u32)>,
    pub tooltip: String,
}

/// One push to the shell: the look, and whether the tray's graph image needs these pixels. The
/// tray keeps the last ones it was given; setting an image again would send it to the shell even
/// when only the tooltip changed, as Slint compares images by buffer.
#[derive(Clone, Debug, PartialEq)]
pub struct TrayPush {
    pub look: TrayLook,
    pub set_icon: bool,
}

/// The tray's state, without Slint: the switch, the icon size and colours, the last sample and
/// what the shell was last given.
pub struct TrayView {
    pub enabled: bool,
    pub size: u32,
    pub colors: Colors,
    last: Option<TraySample>,
    /// The look last pushed to the shell; None for a tray that has been given nothing yet.
    shown: Option<TrayLook>,
    /// The graph pixels the tray's image holds (they stay while the app icon shows).
    icon_held: Option<(Vec<u8>, u32)>,
}

impl TrayView {
    pub fn new(enabled: bool, size: u32, colors: Colors) -> TrayView {
        TrayView { enabled, size, colors, last: None, shown: None, icon_held: None }
    }

    pub fn on_sample(&mut self, s: TraySample) {
        if self.enabled {
            self.last = Some(s);
        }
    }

    /// Off: the app icon at once. On again: the graph from the next sample.
    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
        self.last = None;
    }

    pub fn look(&self) -> TrayLook {
        match self.last.as_ref().filter(|_| self.enabled) {
            Some(s) => TrayLook {
                icon: Some((trayicon::render(&s.points, s.end_ms, self.size, &self.colors), self.size)),
                tooltip: tooltip(s),
            },
            None => TrayLook { icon: None, tooltip: "Negotium".into() },
        }
    }

    /// The look to push to the shell now, if any: none while Explorer is hung (PF2: its
    /// Shell_NotifyIcon would wait on it, so the look stays pending) or when the shell already
    /// shows it.
    pub fn take_update(&mut self, shell_hung: bool) -> Option<TrayPush> {
        if shell_hung {
            return None;
        }
        let look = self.look();
        if self.shown.as_ref() == Some(&look) {
            return None;
        }
        let set_icon = look.icon.is_some() && look.icon != self.icon_held;
        if set_icon {
            self.icon_held.clone_from(&look.icon);
        }
        self.shown = Some(look.clone());
        Some(TrayPush { look, set_icon })
    }

    /// A new tray (Explorer restarted) has been given nothing yet, not even graph pixels: the next
    /// update shows the whole look.
    pub fn forget_shown(&mut self) {
        self.shown = None;
        self.icon_held = None;
    }
}

/// Multi-line hover tooltip text for the tray icon.
///
/// Up to 127 UTF-16 code units (Windows Win32 `NOTIFYICONDATAW.szTip` buffer limit is 128 including null).
pub fn tooltip(s: &TraySample) -> String {
    let mut lines = Vec::with_capacity(4);
    lines.push("Negotium".to_string());

    if let Some(cpu) = s.cpu.filter(|v| v.is_finite()) {
        let mut line = format!("CPU: {}", crate::format::pct(cpu));
        if let Some(temp) = s.cpu_temp_c.filter(|t| t.is_finite()) {
            line.push_str(&format!(" ({})", crate::format::temp_c(temp)));
        }
        lines.push(line);
    }

    if let Some(gpu) = s.gpu.filter(|v| v.is_finite()) {
        let mut line = format!("GPU: {}", crate::format::pct(gpu));
        if let Some(temp) = s.gpu_temp_c.filter(|t| t.is_finite()) {
            line.push_str(&format!(" ({})", crate::format::temp_c(temp)));
        }
        lines.push(line);
    }

    if let (Some(used), Some(total)) = (s.mem_in_use, s.mem_total)
        && total > 0
    {
        let pct = used as f32 / total as f32 * 100.0;
        lines.push(format!("RAM: {} ({})", crate::format::gb_of(used, total), crate::format::pct(pct)));
    }

    if lines.len() == 1 {
        return lines.pop().unwrap();
    }

    truncate_utf16(&lines.join("\n"), 127)
}

fn truncate_utf16(s: &str, max_code_units: usize) -> String {
    let mut count = 0;
    let mut end_byte = 0;
    for (byte_idx, ch) in s.char_indices() {
        let units = ch.len_utf16();
        if count + units > max_code_units {
            break;
        }
        count += units;
        end_byte = byte_idx + ch.len_utf8();
    }
    s[..end_byte].to_string()
}

/// The icon's colours for the user's accent (if it could be read) and the taskbar's mode.
pub fn colors_now(accent: Option<Rgb>, light_taskbar: bool) -> Colors {
    trayicon::colors(accent.unwrap_or(DEFAULT_ACCENT), light_taskbar)
}

/// Whether a push held back by a hung Explorer needs a retry scheduled: one at a time, so a look
/// held back while the switch is off (no sample comes to retry it) still reaches the shell.
pub fn schedule_retry(shell_hung: bool, retrying: bool) -> bool {
    shell_hung && !retrying
}

struct Live {
    view: TrayView,
    tray: Option<slint::Weak<NegotiumTray>>,
    /// Whether Explorer's taskbar is hung now (the worker's latest tick, `Controls::shell_hung`).
    shell_hung: Box<dyn Fn() -> bool>,
    /// A retry is scheduled for a look held back by a hung Explorer.
    retrying: bool,
}

thread_local! {
    /// The tray as the UI thread knows it. `main` creates the Slint tray once the renderer watchdog
    /// has resolved, and again after Explorer restarts; it attaches each one here.
    static TRAY: RefCell<Option<Live>> = const { RefCell::new(None) };
}

/// Starts the tray state (UI thread, once, before the worker runs). `shell_hung` tells whether
/// Explorer is hung now.
pub fn install(view: TrayView, shell_hung: impl Fn() -> bool + 'static) {
    TRAY.with(|t| *t.borrow_mut() = Some(Live { view, tray: None, shell_hung: Box::new(shell_hung), retrying: false }));
}

/// A tray was (re)created for a taskbar whose icons are `size` px: it shows the current look at once.
pub fn attach(tray: &NegotiumTray, size: u32) {
    update(|live| {
        live.tray = Some(tray.as_weak());
        live.view.size = size;
        live.view.forget_shown();
    });
}

pub fn on_sample(s: TraySample) {
    update(|live| live.view.on_sample(s));
}

pub fn set_enabled(on: bool) {
    update(|live| live.view.set_enabled(on));
}

pub fn set_colors(colors: Colors) {
    update(|live| live.view.colors = colors);
}

fn update(f: impl FnOnce(&mut Live)) {
    TRAY.with(|t| t.borrow_mut().as_mut().map(f));
    refresh();
}

/// Pushes the current look to the attached tray unless it already shows it. While Explorer is
/// hung nothing is pushed, and a retry is kept scheduled until it answers again.
fn refresh() {
    let (shown, retry) = TRAY.with(|t| {
        let mut t = t.borrow_mut();
        let Some(live) = t.as_mut() else { return (None, false) };
        let Some(tray) = live.tray.as_ref().and_then(|w| w.upgrade()) else { return (None, false) };
        let hung = (live.shell_hung)();
        let push = live.view.take_update(hung);
        let retry = schedule_retry(hung, live.retrying);
        live.retrying |= retry;
        (push.map(|p| (p, tray)), retry)
    });
    if retry {
        slint::Timer::single_shot(HUNG_RETRY, || {
            update(|live| live.retrying = false);
        });
    }
    // Outside the borrow: setting the tray's properties may run Slint change handlers.
    if let Some((push, tray)) = shown {
        show(&tray, &push);
    }
}

fn show(tray: &NegotiumTray, push: &TrayPush) {
    let look = &push.look;
    if push.set_icon
        && let Some((rgba, size)) = &look.icon
    {
        // Straight (not premultiplied) alpha, as crate::trayicon draws it.
        let pixels = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(rgba, *size, *size);
        tray.set_live_icon(slint::Image::from_rgba8(pixels));
    }
    tray.set_live(look.icon.is_some());
    tray.set_live_tooltip(look.tooltip.as_str().into());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(cpu: f32) -> TraySample {
        TraySample {
            end_ms: 60_000,
            cpu: Some(cpu),
            cpu_temp_c: None,
            gpu: None,
            gpu_temp_c: None,
            mem_in_use: None,
            mem_total: None,
            points: (0..=60).map(|s| (s * 1_000, Some(cpu))).collect(),
        }
    }

    fn view(enabled: bool) -> TrayView {
        TrayView::new(enabled, 16, colors_now(None, false))
    }

    fn app_icon() -> TrayLook {
        TrayLook { icon: None, tooltip: "Negotium".into() }
    }

    /// The graph a tray of `size` px in `colors` must show for a flat `cpu` minute, built straight
    /// from the renderer (not from `look()`).
    fn graph_of(cpu: f32, size: u32, colors: &Colors, tooltip: &str) -> TrayLook {
        let points: Vec<(u64, Option<f32>)> = (0..=60).map(|s| (s * 1_000, Some(cpu))).collect();
        TrayLook { icon: Some((trayicon::render(&points, 60_000, size, colors), size)), tooltip: tooltip.into() }
    }

    #[test]
    fn with_the_switch_on_each_sample_draws_its_minute_and_names_the_cpu_use() {
        let mut v = view(true);
        assert_eq!(v.look(), app_icon(), "no sample yet: the app icon");
        v.on_sample(sample(17.2));
        let look = v.look();
        assert_eq!(look.tooltip, "Negotium\nCPU: 17%");
        let (rgba, size) = look.icon.expect("a graph");
        assert_eq!((size, rgba.len()), (16, 16 * 16 * 4));
        assert_eq!(rgba, trayicon::render(&sample(17.2).points, 60_000, 16, &v.colors));
    }

    #[test]
    fn switching_off_brings_back_the_app_icon_and_ignores_samples() {
        let mut v = view(true);
        v.on_sample(sample(50.0));
        v.set_enabled(false);
        assert_eq!(v.look(), app_icon());
        v.on_sample(sample(50.0));
        assert_eq!(v.look(), app_icon(), "samples that still arrive change nothing");
        v.set_enabled(true);
        assert_eq!(v.look(), app_icon(), "on again: the graph returns with the next sample");
        v.on_sample(sample(50.0));
        assert!(v.look().icon.is_some());
    }

    #[test]
    fn a_re_created_tray_shows_the_last_graph() {
        // Explorer restarted while the window was hidden: main.rs makes a new tray and attaches
        // it, which forgets what the old one showed; the new one gets the last graph at once.
        let mut v = view(true);
        v.on_sample(sample(30.0));
        let want = graph_of(30.0, 16, &colors_now(None, false), "Negotium\nCPU: 30%");
        assert_eq!(v.take_update(false).map(|p| p.look), Some(want.clone()), "the first tray shows the graph");
        assert_eq!(v.take_update(false).map(|p| p.look), None, "it already shows it");
        v.forget_shown();
        assert_eq!(v.take_update(false).map(|p| p.look), Some(want), "the re-created tray shows the same graph");
        // Re-created on a taskbar at another DPI (main.rs attaches it with the size read then).
        v.size = 24;
        v.forget_shown();
        let want = graph_of(30.0, 24, &colors_now(None, false), "Negotium\nCPU: 30%");
        assert_eq!(v.take_update(false).map(|p| p.look), Some(want), "drawn at the new size");
    }

    #[test]
    fn flipping_the_switch_while_hidden_takes_effect_at_once() {
        // Hidden in the tray the worker still sends a sample every 2 s; the switch acts on the
        // shell right away, not with the next sample.
        let mut v = view(true);
        v.on_sample(sample(40.0));
        assert!(v.take_update(false).is_some_and(|p| p.look.icon.is_some()));
        v.set_enabled(false);
        assert_eq!(v.take_update(false).map(|p| p.look), Some(app_icon()), "off: the app icon goes back at once");
        v.on_sample(sample(40.0));
        assert_eq!(v.take_update(false).map(|p| p.look), None, "a sample still in flight changes nothing");
        v.set_enabled(true);
        assert_eq!(v.take_update(false).map(|p| p.look), None, "on again: the app icon stays until a sample comes");
        v.on_sample(sample(60.0));
        assert_eq!(v.take_update(false).map(|p| p.look), Some(graph_of(60.0, 16, &colors_now(None, false), "Negotium\nCPU: 60%")));
    }

    #[test]
    fn nothing_is_pushed_to_a_hung_explorer_and_the_newest_look_follows_once_it_recovers() {
        let mut v = view(true);
        v.on_sample(sample(10.0));
        assert_eq!(v.take_update(true).map(|p| p.look), None, "Explorer hung: Shell_NotifyIcon would wait on it (PF2)");
        v.on_sample(sample(20.0));
        assert_eq!(v.take_update(true).map(|p| p.look), None);
        let want = graph_of(20.0, 16, &colors_now(None, false), "Negotium\nCPU: 20%");
        assert_eq!(v.take_update(false).map(|p| p.look), Some(want), "recovered: the newest graph, once");
        assert_eq!(v.take_update(false).map(|p| p.look), None);
    }

    #[test]
    fn a_colour_change_redraws_the_last_sample() {
        let mut v = view(true);
        v.on_sample(sample(50.0));
        let dark = v.look().icon.unwrap().0;
        v.colors = colors_now(None, true);
        assert_ne!(v.look().icon.unwrap().0, dark, "a light taskbar gets a light plate");
        assert_eq!(v.look(), graph_of(50.0, 16, &colors_now(None, true), "Negotium\nCPU: 50%"));
    }

    #[test]
    fn an_unknown_cpu_reading_names_only_the_app() {
        assert_eq!(tooltip(&TraySample::default()), "Negotium");
        assert_eq!(tooltip(&TraySample { cpu: Some(f32::NAN), ..Default::default() }), "Negotium");
        assert_eq!(tooltip(&sample(99.6)), "Negotium\nCPU: 100%");
    }

    #[test]
    fn rich_hardware_telemetry_tooltip() {
        let s = TraySample {
            cpu: Some(18.2),
            cpu_temp_c: Some(54.0),
            gpu: Some(5.1),
            gpu_temp_c: Some(49.0),
            mem_in_use: Some(8 << 30),
            mem_total: Some(16 << 30),
            ..Default::default()
        };
        assert_eq!(tooltip(&s), "Negotium\nCPU: 18% (54 °C)\nGPU: 5% (49 °C)\nRAM: 8.0 / 16.0 GB (50%)");

        // Omits temperatures when missing
        let no_temps = TraySample {
            cpu: Some(18.2),
            cpu_temp_c: None,
            gpu: Some(5.1),
            gpu_temp_c: None,
            mem_in_use: Some(8 << 30),
            mem_total: Some(16 << 30),
            ..Default::default()
        };
        assert_eq!(tooltip(&no_temps), "Negotium\nCPU: 18%\nGPU: 5%\nRAM: 8.0 / 16.0 GB (50%)");
    }

    #[test]
    fn a_sample_holds_the_minute_up_to_its_end() {
        let mut h = negotium_history::History::new();
        for s in 0..=90u64 {
            let t = negotium_snapshot::SystemTotals { cpu_pct: Some(s as f32), ..Default::default() };
            h.push(s * 1_000, &t, &[]);
        }
        let s = TraySample::from_history(&h, 90_000, Some(90.0));
        assert_eq!(s.points.first(), Some(&(30_000, Some(30.0))));
        assert_eq!(s.points.last(), Some(&(90_000, Some(90.0))));
    }

    #[test]
    fn the_graph_pixels_are_sent_only_when_they_change() {
        let mut v = view(true);
        v.on_sample(sample(30.0));
        assert!(v.take_update(false).is_some_and(|p| p.set_icon), "the first graph");
        // The same minute; only the reading in the tooltip differs.
        v.on_sample(TraySample { cpu: Some(31.0), ..sample(30.0) });
        let p = v.take_update(false).expect("the tooltip changed");
        assert_eq!(p.look.tooltip, "Negotium\nCPU: 31%");
        assert!(!p.set_icon, "the shell keeps the icon it has");
        v.set_enabled(false);
        assert!(v.take_update(false).is_some_and(|p| !p.set_icon && p.look.icon.is_none()), "off: the app icon");
        v.set_enabled(true);
        v.on_sample(sample(30.0));
        assert!(v.take_update(false).is_some_and(|p| !p.set_icon), "on again, same pixels: the held image shows");
        v.on_sample(sample(70.0));
        assert!(v.take_update(false).is_some_and(|p| p.set_icon), "new pixels are sent");
        v.forget_shown();
        assert!(v.take_update(false).is_some_and(|p| p.set_icon), "a re-created tray holds no image yet");
    }

    #[test]
    fn a_switch_turned_off_during_a_hang_is_retried_until_explorer_answers() {
        let mut v = view(true);
        v.on_sample(sample(40.0));
        assert!(v.take_update(false).is_some());
        let mut retrying = false;
        // The switch goes off while the taskbar is hung: nothing is pushed, a retry is scheduled.
        v.set_enabled(false);
        assert_eq!(v.take_update(true), None);
        assert!(schedule_retry(true, retrying));
        retrying = true;
        assert!(!schedule_retry(true, retrying), "one retry at a time");
        // The retry fires (clearing the flag) while still hung: it schedules the next.
        retrying = false;
        assert_eq!(v.take_update(true), None);
        assert!(schedule_retry(true, retrying));
        // The next retry finds Explorer answering: the app icon goes back, and no more retries.
        retrying = false;
        assert_eq!(v.take_update(false).map(|p| p.look), Some(app_icon()));
        assert!(!schedule_retry(false, retrying));
    }
}
