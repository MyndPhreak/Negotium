//! Which renderer Negotium starts with, and the safety net under the GPU one.
//!
//! The default is winit + FemtoVG (OpenGL): resizing a large window is smooth there, while the
//! software renderer repaints the whole window on every resize step. Two things can go wrong
//! with OpenGL:
//! - the GL driver fails with an error: Slint's winit backend then falls back to software
//!   in-process, even with FemtoVG selected by name (its window creation retries every built-in
//!   renderer when the selected one fails to build);
//! - the GL driver stalls (seen once in Plan 2: the window took 30 s to 5+ min to appear). That
//!   blocks the UI thread, so nothing in-process can rescue it. A [`Watchdog`] thread notices
//!   that the UI thread showed neither a frame nor a heartbeat for [`WATCHDOG_TIMEOUT`], records
//!   the fallback, relaunches the exe with the software renderer and terminates this process.
//!   A window that renders nothing because it is minimised (occluded) keeps its heartbeat, so it
//!   is not mistaken for a stall.
//!
//! The fallback is recorded in a marker file ([`fallback_path`]) and every later launch honours
//! it. Only an explicit user action clears it ([`clear_fallback`], for the Settings switch of a
//! later task), so the renderer never flips back and forth. Task 1 folds the marker into
//! settings.json (`gpu_rendering: false`) by redirecting the three functions below.
//! `SLINT_BACKEND` overrides both the default and the marker.
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How long the UI thread may show neither a first frame nor a heartbeat before the watchdog
/// relaunches. Generous, because the fallback it records sticks: a cold start at login, a virus
/// scan or font enumeration must not trip it.
pub const WATCHDOG_TIMEOUT: Duration = Duration::from_secs(10);
/// Marks a process started by the watchdog: it never arms a watchdog of its own.
pub const FALLBACK_ARG: &str = "--renderer-fallback";
/// Makes the relaunch wait for this instance's single-instance lock (as restart-as-admin does).
pub const RESTARTED_ARG: &str = "--restarted";
/// The `SLINT_BACKEND` value the relaunched process gets.
pub const SOFTWARE_BACKEND: &str = "winit-software";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Renderer {
    /// `SLINT_BACKEND` is set: Slint reads it, Negotium selects nothing.
    FromEnv,
    /// winit + FemtoVG.
    Gpu,
    /// winit + software.
    Software,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decision {
    pub renderer: Renderer,
    /// Whether a [`Watchdog`] guards the first frame.
    pub watchdog: bool,
}

/// The renderer for this launch. Only Negotium's own GPU choice is guarded: an explicit
/// `SLINT_BACKEND` is the user's (or the bench's) call, and a process the watchdog started must
/// never relaunch again.
pub fn decide(env_backend_set: bool, fallback_recorded: bool, relaunched_by_watchdog: bool) -> Decision {
    let renderer = if env_backend_set {
        Renderer::FromEnv
    } else if fallback_recorded || relaunched_by_watchdog {
        Renderer::Software
    } else {
        Renderer::Gpu
    };
    Decision { renderer, watchdog: renderer == Renderer::Gpu }
}

/// `%LOCALAPPDATA%\Negotium\renderer-fallback` (the temp directory if LOCALAPPDATA is unset).
pub fn fallback_path() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("Negotium").join("renderer-fallback")
}

/// Whether a fallback to software has been recorded.
pub fn fallback_recorded() -> bool {
    fallback_recorded_at(&fallback_path())
}

/// Records the fallback durably: later launches select software.
pub fn record_fallback() -> std::io::Result<()> {
    record_fallback_at(&fallback_path())
}

/// Forgets the fallback, so the next launch tries the GPU again. Only for an explicit user
/// action (the Settings switch, a later task); nothing calls it automatically.
pub fn clear_fallback() -> std::io::Result<()> {
    clear_fallback_at(&fallback_path())
}

pub fn fallback_recorded_at(path: &Path) -> bool {
    path.is_file()
}

/// Writes and flushes the marker; the file is closed on return (the watchdog then terminates the
/// process without any cleanup).
pub fn record_fallback_at(path: &Path) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut f = std::fs::File::create(path)?;
    f.write_all(b"The GPU renderer stalled at startup; Negotium uses the software renderer.\n")?;
    f.sync_all()
}

pub fn clear_fallback_at(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        r => r,
    }
}

/// The relaunch's arguments: this launch's own, plus [`RESTARTED_ARG`] and [`FALLBACK_ARG`]
/// (each once).
pub fn relaunch_args(args: impl IntoIterator<Item = OsString>) -> Vec<OsString> {
    let mut out: Vec<OsString> = args.into_iter().collect();
    for flag in [RESTARTED_ARG, FALLBACK_ARG] {
        if !out.iter().any(|a| a == flag) {
            out.push(flag.into());
        }
    }
    out
}

/// What the watchdog concluded once the timeout has passed without a first frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Not decided yet.
    Wait,
    /// The UI thread has been silent for the whole timeout: a stall.
    Fire,
    /// The UI thread has been beating for the whole timeout but drew nothing (minimised or
    /// hidden): it is healthy, and the watchdog stands down.
    Healthy,
}

/// The watchdog's decision, a pure function of time since arming and since the first and the
/// latest heartbeat (`None`: no heartbeat yet). A first frame is handled before this is asked.
pub fn verdict(
    since_arm: Duration,
    since_first_beat: Option<Duration>,
    since_last_beat: Option<Duration>,
    timeout: Duration,
) -> Verdict {
    if since_last_beat.unwrap_or(since_arm) >= timeout {
        Verdict::Fire
    } else if since_first_beat.is_some_and(|d| d >= timeout) {
        Verdict::Healthy
    } else {
        Verdict::Wait
    }
}

const ARMED: u8 = 0;
const PRESENTED: u8 = 1;
const STOOD_DOWN: u8 = 2;
const FIRED: u8 = 3;

struct Shared {
    state: AtomicU8,
    armed_at: Instant,
    /// Milliseconds since `armed_at`, plus one; 0 means no heartbeat yet.
    first_beat: AtomicU64,
    last_beat: AtomicU64,
}

impl Shared {
    fn since(&self, beat: &AtomicU64, now: Instant) -> Option<Duration> {
        let at = beat.load(Ordering::SeqCst).checked_sub(1)?;
        Some(now.saturating_duration_since(self.armed_at + Duration::from_millis(at)))
    }
}

/// Guards the first frame from its own thread. The UI thread reports a heartbeat
/// ([`Watchdog::heartbeat`]) and the first frame ([`Watchdog::presented`]); the state moves once,
/// out of Armed, by compare-and-swap, so a first frame and the stall callback can never both win.
#[derive(Clone)]
pub struct Watchdog(Arc<Shared>);

impl Watchdog {
    /// Starts the watchdog thread. `on_stall` runs at most once, from that thread. Dropping every
    /// handle stands the watchdog down.
    pub fn arm(timeout: Duration, on_stall: impl FnOnce() + Send + 'static) -> (Watchdog, JoinHandle<()>) {
        let shared = Arc::new(Shared {
            state: AtomicU8::new(ARMED),
            armed_at: Instant::now(),
            first_beat: AtomicU64::new(0),
            last_beat: AtomicU64::new(0),
        });
        let weak: Weak<Shared> = Arc::downgrade(&shared);
        let poll = (timeout / 20).clamp(Duration::from_millis(1), Duration::from_millis(100));
        let thread = std::thread::Builder::new()
            .name("negotium-renderer-watchdog".into())
            .spawn(move || {
                loop {
                    std::thread::sleep(poll);
                    let Some(w) = weak.upgrade() else { return };
                    if w.state.load(Ordering::SeqCst) != ARMED {
                        return;
                    }
                    let now = Instant::now();
                    let since_arm = now.saturating_duration_since(w.armed_at);
                    let to = match verdict(since_arm, w.since(&w.first_beat, now), w.since(&w.last_beat, now), timeout) {
                        Verdict::Wait => continue,
                        Verdict::Healthy => STOOD_DOWN,
                        Verdict::Fire => FIRED,
                    };
                    let won = w.state.compare_exchange(ARMED, to, Ordering::SeqCst, Ordering::SeqCst).is_ok();
                    drop(w);
                    if won && to == FIRED {
                        on_stall();
                    }
                    return;
                }
            })
            .expect("spawn the renderer watchdog");
        (Watchdog(shared), thread)
    }

    /// The UI thread is alive (called from a UI-thread timer).
    pub fn heartbeat(&self) {
        let at = self.0.armed_at.elapsed().as_millis() as u64 + 1;
        let _ = self.0.first_beat.compare_exchange(0, at, Ordering::SeqCst, Ordering::SeqCst);
        self.0.last_beat.store(at, Ordering::SeqCst);
    }

    /// The first frame was presented. False if the watchdog has already fired: this instance is
    /// being replaced and must not create anything (such as the tray icon) that would outlive it.
    pub fn presented(&self) -> bool {
        match self.0.state.compare_exchange(ARMED, PRESENTED, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => true,
            Err(state) => state != FIRED,
        }
    }

    /// No GL can stall (the software renderer is in use): stand down unless already fired.
    pub fn stand_down(&self) {
        let _ = self.0.state.compare_exchange(ARMED, STOOD_DOWN, Ordering::SeqCst, Ordering::SeqCst);
    }

    /// Whether the watchdog is settled and won't fire: a frame was presented or it stood down.
    /// Until then the tray icon waits, so a replaced instance leaves none behind.
    pub fn resolved(&self) -> bool {
        matches!(self.0.state.load(Ordering::SeqCst), PRESENTED | STOOD_DOWN)
    }
}

/// The stall callback: records the fallback, relaunches this exe with the software renderer and
/// terminates this process (no DLL detach: a wedged GL driver could hang an orderly exit), which
/// releases the single-instance lock the relaunch waits for. The stalled instance has no tray
/// icon yet (it is created once the watchdog has resolved), so none is orphaned.
/// If the relaunch cannot start, this instance keeps waiting: the window may still appear, and
/// the next launch uses software anyway.
pub fn relaunch_in_software() {
    let log = |msg: &str| crate::log::Log::new(crate::log::Log::default_path()).warn(msg);
    if let Err(e) = record_fallback() {
        log(&format!("renderer fallback could not be recorded: {e}"));
    }
    log(&format!(
        "the UI thread showed no frame and no heartbeat for {} s; relaunching with the software renderer",
        WATCHDOG_TIMEOUT.as_secs()
    ));
    let spawned = std::env::current_exe().and_then(|exe| {
        std::process::Command::new(exe)
            .args(relaunch_args(std::env::args_os().skip(1)))
            .env("SLINT_BACKEND", SOFTWARE_BACKEND)
            .spawn()
    });
    match spawned {
        // The marker and the log are written and closed above.
        Ok(_) => crate::platform::terminate_self(),
        Err(e) => log(&format!("software relaunch failed: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decides_gpu_with_a_watchdog_by_default() {
        assert_eq!(decide(false, false, false), Decision { renderer: Renderer::Gpu, watchdog: true });
    }

    #[test]
    fn a_recorded_fallback_or_a_watchdog_relaunch_selects_software_unguarded() {
        let software = Decision { renderer: Renderer::Software, watchdog: false };
        assert_eq!(decide(false, true, false), software);
        assert_eq!(decide(false, false, true), software);
        assert_eq!(decide(false, true, true), software);
    }

    #[test]
    fn slint_backend_overrides_everything_and_is_never_guarded() {
        for marker in [false, true] {
            for relaunched in [false, true] {
                assert_eq!(
                    decide(true, marker, relaunched),
                    Decision { renderer: Renderer::FromEnv, watchdog: false },
                    "marker {marker}, relaunched {relaunched}"
                );
            }
        }
    }

    fn temp_marker(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("negotium-renderer-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d.join("Negotium").join("renderer-fallback")
    }

    #[test]
    fn the_marker_is_written_read_and_cleared() {
        let path = temp_marker("roundtrip");
        assert!(!fallback_recorded_at(&path));
        record_fallback_at(&path).unwrap();
        assert!(fallback_recorded_at(&path), "recorded, creating the directory");
        record_fallback_at(&path).unwrap();
        assert!(fallback_recorded_at(&path), "recording twice is harmless");
        clear_fallback_at(&path).unwrap();
        assert!(!fallback_recorded_at(&path));
        clear_fallback_at(&path).unwrap(); // clearing when absent is fine
    }

    #[test]
    fn the_marker_lives_under_local_app_data() {
        let p = fallback_path();
        assert!(p.ends_with("Negotium/renderer-fallback"), "{}", p.display());
    }

    #[test]
    fn the_relaunch_adds_its_flags_once() {
        let args = |v: &[&str]| v.iter().map(OsString::from).collect::<Vec<_>>();
        assert_eq!(relaunch_args(args(&["--exit-on-close"])), args(&["--exit-on-close", RESTARTED_ARG, FALLBACK_ARG]));
        assert_eq!(relaunch_args(args(&[RESTARTED_ARG])), args(&[RESTARTED_ARG, FALLBACK_ARG]));
    }

    fn counting() -> (Arc<std::sync::atomic::AtomicUsize>, impl FnOnce() + Send + 'static) {
        let n = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let m = n.clone();
        (n, move || {
            m.fetch_add(1, Ordering::SeqCst);
        })
    }

    const T: Duration = Duration::from_secs(10);
    fn s(secs: u64) -> Duration {
        Duration::from_secs(secs)
    }

    #[test]
    fn the_verdict_waits_within_the_timeout() {
        assert_eq!(verdict(s(5), None, None, T), Verdict::Wait);
        assert_eq!(verdict(s(9), Some(s(8)), Some(s(1)), T), Verdict::Wait);
    }

    #[test]
    fn no_frame_and_no_heartbeat_fires() {
        assert_eq!(verdict(s(10), None, None, T), Verdict::Fire);
        // The loop beat early, then the first render stalled: silent for the whole timeout.
        assert_eq!(verdict(s(11), Some(s(11)), Some(s(10)), T), Verdict::Fire);
    }

    #[test]
    fn no_frame_with_a_heartbeat_does_not_fire() {
        // Minimised: the loop beats on but draws nothing.
        assert_eq!(verdict(s(10), Some(s(10)), Some(Duration::ZERO), T), Verdict::Healthy);
        // A late start: beating, but not yet for a whole timeout.
        assert_eq!(verdict(s(12), Some(s(4)), Some(Duration::ZERO), T), Verdict::Wait);
    }

    #[test]
    fn a_first_frame_in_time_resolves_the_watchdog() {
        let (stalls, on_stall) = counting();
        let (w, thread) = Watchdog::arm(Duration::from_secs(30), on_stall);
        assert!(!w.resolved());
        assert!(w.presented());
        assert!(w.presented(), "a second report is harmless");
        assert!(w.resolved());
        thread.join().unwrap(); // returns at its next poll, not after 30 s
        assert_eq!(stalls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn fire_wins_when_no_frame_arrives() {
        let (stalls, on_stall) = counting();
        let (w, thread) = Watchdog::arm(Duration::from_millis(20), on_stall);
        thread.join().unwrap();
        assert_eq!(stalls.load(Ordering::SeqCst), 1);
        assert!(!w.presented(), "a frame after the fire loses: this instance is being replaced");
        assert!(!w.resolved());
        w.stand_down();
        assert!(!w.resolved());
        assert_eq!(stalls.load(Ordering::SeqCst), 1, "fired exactly once");
    }

    #[test]
    fn a_frame_that_wins_after_the_timeout_keeps_the_watchdog_from_firing() {
        // The timeout has passed, but the first frame reaches the state first: no fire.
        let (stalls, on_stall) = counting();
        let (w, thread) = Watchdog::arm(Duration::from_millis(50), on_stall);
        assert!(w.presented());
        std::thread::sleep(Duration::from_millis(100));
        thread.join().unwrap();
        assert_eq!(stalls.load(Ordering::SeqCst), 0);
        assert!(w.resolved());
    }

    #[test]
    fn a_heartbeating_ui_without_frames_resolves_healthy() {
        let (stalls, on_stall) = counting();
        let (w, thread) = Watchdog::arm(Duration::from_millis(300), on_stall);
        while !thread.is_finished() {
            w.heartbeat();
            std::thread::sleep(Duration::from_millis(2));
        }
        thread.join().unwrap();
        assert_eq!(stalls.load(Ordering::SeqCst), 0);
        assert!(w.resolved(), "minimised but alive: the tray may be created");
    }

    #[test]
    fn standing_down_resolves_without_firing() {
        let (stalls, on_stall) = counting();
        let (w, thread) = Watchdog::arm(Duration::from_secs(30), on_stall);
        w.stand_down();
        assert!(w.resolved());
        thread.join().unwrap();
        assert_eq!(stalls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn dropping_every_handle_stands_down() {
        let (stalls, on_stall) = counting();
        let (w, thread) = Watchdog::arm(Duration::from_secs(30), on_stall);
        drop(w);
        thread.join().unwrap();
        assert_eq!(stalls.load(Ordering::SeqCst), 0);
    }
}
