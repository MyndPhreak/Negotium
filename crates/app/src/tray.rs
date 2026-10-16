//! The tray icon (spec §5.5): the app icon or, with Settings > Show CPU in tray icon, a small CPU
//! graph redrawn for each history sample the worker sends, with "CPU 17% · Negotium" on hover.
//!
//! UI thread (rulings P3c-6, PF11): the pixels are drawn here, and Slint pushes a changed `icon`
//! or `tooltip` to the shell itself (Shell_NotifyIcon NIM_MODIFY), both on the UI thread. That is an
//! allowed exception to "the UI thread never blocks": a 32 × 32 icon takes well under a millisecond
//! once a second, and the shell call is bounded by PF2: while a window of explorer.exe is hung,
//! nothing is pushed (Shell_NotifyIcon would wait on Explorer); the newest look follows once it
//! answers again.
use crate::NegotiumTray;
use crate::trayicon::{self, Colors, Rgb};
use negotium_history::{History, Series};
use negotium_snapshot::{ProcessSample, WindowSample};
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
    pub points: Vec<(u64, Option<f32>)>,
}

impl TraySample {
    pub fn from_history(h: &History, end_ms: u64, cpu: Option<f32>) -> TraySample {
        TraySample { end_ms, cpu, points: h.points(&Series::Cpu, end_ms.saturating_sub(trayicon::SPAN_MS)).collect() }
    }
}

/// What the tray shows: a graph (RGBA pixels and their side) or the app icon, and the hover text.
#[derive(Clone, Debug, PartialEq)]
pub struct TrayLook {
    pub icon: Option<(Vec<u8>, u32)>,
    pub tooltip: String,
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
}

impl TrayView {
    pub fn new(enabled: bool, size: u32, colors: Colors) -> TrayView {
        TrayView { enabled, size, colors, last: None, shown: None }
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
                tooltip: tooltip(s.cpu),
            },
            None => TrayLook { icon: None, tooltip: "Negotium".into() },
        }
    }

    /// The look to push to the shell now, if any: none while Explorer is hung (PF2: its
    /// Shell_NotifyIcon would wait on it, so the look stays pending) or when the shell already
    /// shows it.
    pub fn take_update(&mut self, shell_hung: bool) -> Option<TrayLook> {
        if shell_hung {
            return None;
        }
        let look = self.look();
        if self.shown.as_ref() == Some(&look) {
            return None;
        }
        self.shown = Some(look.clone());
        Some(look)
    }

    /// A new tray (Explorer restarted) has been given nothing yet: the next update shows the look.
    pub fn forget_shown(&mut self) {
        self.shown = None;
    }
}

/// "CPU 17% · Negotium", or just "Negotium" while the CPU use is unknown.
pub fn tooltip(cpu: Option<f32>) -> String {
    match cpu.filter(|v| v.is_finite()) {
        Some(v) => format!("CPU {} · Negotium", crate::format::pct(v)),
        None => "Negotium".into(),
    }
}

/// The icon's colours for the user's accent (if it could be read) and the taskbar's mode.
pub fn colors_now(accent: Option<Rgb>, light_taskbar: bool) -> Colors {
    trayicon::colors(accent.unwrap_or(DEFAULT_ACCENT), light_taskbar)
}

/// Whether a snapshot shows a hung window of explorer.exe (the taskbar and its notification area),
/// from the snapshot's windows and processes (PF2).
pub fn explorer_hung(processes: &[ProcessSample], windows: &[WindowSample]) -> bool {
    windows
        .iter()
        .filter(|w| w.hung)
        .any(|w| processes.iter().any(|p| p.pid == w.pid && p.name.eq_ignore_ascii_case("explorer.exe")))
}

struct Live {
    view: TrayView,
    tray: Option<slint::Weak<NegotiumTray>>,
    /// Whether Explorer is hung now (the worker's latest snapshot, `Controls::shell_hung`).
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

/// A tray was (re)created: it shows the current look at once.
pub fn attach(tray: &NegotiumTray) {
    update(|live| {
        live.tray = Some(tray.as_weak());
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
        let look = live.view.take_update(hung);
        let retry = hung && !live.retrying;
        live.retrying |= retry;
        (look.map(|l| (l, tray)), retry)
    });
    if retry {
        slint::Timer::single_shot(HUNG_RETRY, || {
            update(|live| live.retrying = false);
        });
    }
    // Outside the borrow: setting the tray's properties may run Slint change handlers.
    if let Some((look, tray)) = shown {
        show(&tray, &look);
    }
}

fn show(tray: &NegotiumTray, look: &TrayLook) {
    if let Some((rgba, size)) = &look.icon {
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
    use negotium_snapshot::{ProcessSample, WindowSample};

    fn sample(cpu: f32) -> TraySample {
        TraySample { end_ms: 60_000, cpu: Some(cpu), points: (0..=60).map(|s| (s * 1_000, Some(cpu))).collect() }
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
        assert_eq!(look.tooltip, "CPU 17% · Negotium");
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
        let want = graph_of(30.0, 16, &colors_now(None, false), "CPU 30% · Negotium");
        assert_eq!(v.take_update(false), Some(want.clone()), "the first tray shows the graph");
        assert_eq!(v.take_update(false), None, "it already shows it");
        v.forget_shown();
        assert_eq!(v.take_update(false), Some(want), "the re-created tray shows the same graph");
    }

    #[test]
    fn flipping_the_switch_while_hidden_takes_effect_at_once() {
        // Hidden in the tray the worker still sends a sample every 2 s; the switch acts on the
        // shell right away, not with the next sample.
        let mut v = view(true);
        v.on_sample(sample(40.0));
        assert!(v.take_update(false).is_some_and(|l| l.icon.is_some()));
        v.set_enabled(false);
        assert_eq!(v.take_update(false), Some(app_icon()), "off: the app icon goes back at once");
        v.on_sample(sample(40.0));
        assert_eq!(v.take_update(false), None, "a sample still in flight changes nothing");
        v.set_enabled(true);
        assert_eq!(v.take_update(false), None, "on again: the app icon stays until a sample comes");
        v.on_sample(sample(60.0));
        assert_eq!(v.take_update(false), Some(graph_of(60.0, 16, &colors_now(None, false), "CPU 60% · Negotium")));
    }

    #[test]
    fn nothing_is_pushed_to_a_hung_explorer_and_the_newest_look_follows_once_it_recovers() {
        let mut v = view(true);
        v.on_sample(sample(10.0));
        assert_eq!(v.take_update(true), None, "Explorer hung: Shell_NotifyIcon would wait on it (PF2)");
        v.on_sample(sample(20.0));
        assert_eq!(v.take_update(true), None);
        let want = graph_of(20.0, 16, &colors_now(None, false), "CPU 20% · Negotium");
        assert_eq!(v.take_update(false), Some(want), "recovered: the newest graph, once");
        assert_eq!(v.take_update(false), None);
    }

    #[test]
    fn a_colour_change_redraws_the_last_sample() {
        let mut v = view(true);
        v.on_sample(sample(50.0));
        let dark = v.look().icon.unwrap().0;
        v.colors = colors_now(None, true);
        assert_ne!(v.look().icon.unwrap().0, dark, "a light taskbar gets a light plate");
        assert_eq!(v.look(), graph_of(50.0, 16, &colors_now(None, true), "CPU 50% · Negotium"));
    }

    #[test]
    fn an_unknown_cpu_reading_names_only_the_app() {
        assert_eq!(tooltip(None), "Negotium");
        assert_eq!(tooltip(Some(f32::NAN)), "Negotium");
        assert_eq!(tooltip(Some(99.6)), "CPU 100% · Negotium");
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

    fn process(pid: u32, name: &str) -> ProcessSample {
        ProcessSample {
            pid, parent_pid: 4, create_time: 1, name: name.into(), session_id: 1, cpu_time_100ns: 0,
            private_working_set: 0, disk_io_bytes: 0, threads: 1, handles: 1, image_path: None,
            package_family: None, description: None,
        }
    }

    fn window(pid: u32, hung: bool) -> WindowSample {
        WindowSample { pid, hung, title: String::new() }
    }

    #[test]
    fn only_a_hung_explorer_window_holds_the_tray_back() {
        let procs = [process(10, "explorer.exe"), process(20, "notepad.exe"), process(30, "EXPLORER.EXE")];
        assert!(!explorer_hung(&procs, &[]), "no windows");
        assert!(!explorer_hung(&procs, &[window(10, false), window(20, false)]), "nothing hung");
        assert!(!explorer_hung(&procs, &[window(20, true)]), "another app's hang doesn't involve the shell");
        assert!(explorer_hung(&procs, &[window(20, false), window(10, true)]), "the taskbar's process hangs");
        assert!(explorer_hung(&procs, &[window(30, true)]), "names compare without case");
        assert!(!explorer_hung(&procs, &[window(99, true)]), "a window of a process not listed");
    }
}
