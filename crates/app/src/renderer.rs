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
//!   is not mistaken for a stall, and time the machine spent asleep is not counted as silence
//!   ([`Clock`]). If the relaunch cannot start, the watchdog stands down and this instance
//!   carries on.
//!
//! The fallback is recorded in a marker file ([`fallback_path`]) and every later launch honours
//! it. Only an explicit user action clears it ([`set_gpu_rendering`]: the Settings page's "Use
//! GPU rendering" switch), so the renderer never flips back and forth. The marker stays the
//! durable record of an automatic fallback (the watchdog writes it from a background thread just before
//! terminating); settings.json holds the user's own choice (`gpu_rendering`). The GPU is used only
//! when the setting is on AND no marker exists. `SLINT_BACKEND` overrides all of it.
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
pub fn decide(
    env_backend_set: bool,
    gpu_rendering: bool,
    fallback_recorded: bool,
    relaunched_by_watchdog: bool,
) -> Decision {
    let renderer = if env_backend_set {
        Renderer::FromEnv
    } else if !gpu_rendering || fallback_recorded || relaunched_by_watchdog {
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
/// action ([`set_gpu_rendering`]); nothing calls it automatically.
pub fn clear_fallback() -> std::io::Result<()> {
    clear_fallback_at(&fallback_path())
}

/// The Settings switch "Use GPU rendering" (takes effect on the next launch). Turning it on
/// forgets an earlier automatic fallback, so the GPU is really tried again; turning it off needs
/// no marker, because the choice itself is saved as `Settings::gpu_rendering`.
pub fn set_gpu_rendering(on: bool) -> std::io::Result<()> {
    set_gpu_rendering_at(&fallback_path(), on)
}

pub fn set_gpu_rendering_at(marker: &Path, on: bool) -> std::io::Result<()> {
    if on { clear_fallback_at(marker) } else { Ok(()) }
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
/// (each once), and `--tray` when this launch started in the tray (a start by the admin task's
/// logon trigger has no arguments to say so, and the added flags would hide that).
pub fn relaunch_args(args: impl IntoIterator<Item = OsString>, in_tray: bool) -> Vec<OsString> {
    let mut out: Vec<OsString> = args.into_iter().collect();
    let tray = in_tray.then_some(crate::integrate::TRAY);
    for flag in [RESTARTED_ARG, FALLBACK_ARG].into_iter().chain(tray) {
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

/// A poll that comes this much later than its interval means the machine was asleep (or the
/// watchdog thread was starved): that interval is lost time, not silence, so a suspend early in
/// the launch can't trigger a permanent fallback.
pub const SUSPEND_SLACK: Duration = Duration::from_secs(2);

/// The watchdog's own sense of time: awake time since arming (each poll's interval, except one
/// that overshoots by more than [`SUSPEND_SLACK`], which counts as a single ordinary poll), and
/// when, in awake time, it first and last saw the heartbeat count change. Driven by the watchdog
/// thread with real instants; tests drive it with made-up ones.
pub struct Clock {
    prev: Instant,
    awake: Duration,
    beats: u64,
    first_beat: Option<Duration>,
    last_beat: Option<Duration>,
}

impl Clock {
    pub fn new(armed_at: Instant) -> Self {
        Self { prev: armed_at, awake: Duration::ZERO, beats: 0, first_beat: None, last_beat: None }
    }

    /// One poll at `now` (polls are `poll` apart), with `beats` heartbeats seen so far.
    pub fn observe(&mut self, now: Instant, beats: u64, poll: Duration, timeout: Duration) -> Verdict {
        let gap = now.saturating_duration_since(self.prev);
        self.prev = now;
        self.awake += if gap > poll + SUSPEND_SLACK { poll } else { gap };
        if beats != self.beats {
            self.beats = beats;
            self.first_beat.get_or_insert(self.awake);
            self.last_beat = Some(self.awake);
        }
        let since = |at: Option<Duration>| at.map(|at| self.awake - at);
        verdict(self.awake, since(self.first_beat), since(self.last_beat), timeout)
    }
}

const ARMED: u8 = 0;
const PRESENTED: u8 = 1;
const STOOD_DOWN: u8 = 2;
const FIRED: u8 = 3;

struct Shared {
    state: AtomicU8,
    /// Heartbeats so far; the watchdog thread notes when the count changes ([`Clock`]).
    beats: AtomicU64,
}

/// Guards the first frame from its own thread. The UI thread reports a heartbeat
/// ([`Watchdog::heartbeat`]) and the first frame ([`Watchdog::presented`]); the state moves once,
/// out of Armed, by compare-and-swap, so a first frame and the stall callback can never both win.
#[derive(Clone)]
pub struct Watchdog(Arc<Shared>);

impl Watchdog {
    /// Starts the watchdog thread. `on_stall` runs at most once, from that thread, and returns
    /// whether a replacement instance was started (production terminates instead of returning
    /// true). When it returns false the watchdog stands down: this instance carries on, and may
    /// create its tray icon if its window ever appears. Dropping every handle stands the
    /// watchdog down.
    pub fn arm(timeout: Duration, on_stall: impl FnOnce() -> bool + Send + 'static) -> (Watchdog, JoinHandle<()>) {
        let shared = Arc::new(Shared { state: AtomicU8::new(ARMED), beats: AtomicU64::new(0) });
        let weak: Weak<Shared> = Arc::downgrade(&shared);
        let poll = (timeout / 20).clamp(Duration::from_millis(1), Duration::from_millis(100));
        let mut clock = Clock::new(Instant::now());
        let thread = std::thread::Builder::new()
            .name("negotium-renderer-watchdog".into())
            .spawn(move || {
                loop {
                    std::thread::sleep(poll);
                    let Some(w) = weak.upgrade() else { return };
                    if w.state.load(Ordering::SeqCst) != ARMED {
                        return;
                    }
                    let to = match clock.observe(Instant::now(), w.beats.load(Ordering::SeqCst), poll, timeout) {
                        Verdict::Wait => continue,
                        Verdict::Healthy => STOOD_DOWN,
                        Verdict::Fire => FIRED,
                    };
                    let won = w.state.compare_exchange(ARMED, to, Ordering::SeqCst, Ordering::SeqCst).is_ok();
                    if won && to == FIRED && !on_stall() {
                        // No replacement is coming: never leave the state Fired, or no tray icon
                        // would ever be created.
                        w.state.store(STOOD_DOWN, Ordering::SeqCst);
                    }
                    return;
                }
            })
            .expect("spawn the renderer watchdog");
        (Watchdog(shared), thread)
    }

    /// The UI thread is alive (called from a UI-thread timer).
    pub fn heartbeat(&self) {
        self.0.beats.fetch_add(1, Ordering::SeqCst);
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

/// The stall callback, resolved when the watchdog is armed (before the window exists), so the
/// stall path only writes the marker and the log and spawns: [`SoftwareRelaunch::run`].
pub struct SoftwareRelaunch {
    exe: Result<PathBuf, String>,
    args: Vec<OsString>,
    marker: PathBuf,
    log: PathBuf,
}

impl SoftwareRelaunch {
    /// This exe, this launch's arguments plus [`RESTARTED_ARG`] and [`FALLBACK_ARG`] (and
    /// `--tray` for a launch that started in the tray), the marker at [`fallback_path`] and the
    /// default log.
    pub fn resolve(in_tray: bool) -> Self {
        Self {
            exe: std::env::current_exe().map_err(|e| e.to_string()),
            // Without a Task Manager command line the redirect appended (`integrate::own_args`),
            // which would hide the fallback flag from the relaunched instance.
            args: relaunch_args(crate::integrate::own_args(std::env::args_os().skip(1)), in_tray),
            marker: fallback_path(),
            log: crate::log::Log::default_path(),
        }
    }

    /// Records the fallback, relaunches the exe with the software renderer and terminates this
    /// process (no DLL detach: a wedged GL driver could hang an orderly exit), which releases the
    /// single-instance lock the relaunch waits for. The stalled instance has no tray icon yet (it
    /// is created once the watchdog has resolved), so none is orphaned.
    /// Returns false only if the relaunch could not start: this instance then carries on (the
    /// window may still appear), and the next launch uses software anyway.
    pub fn run(self) -> bool {
        let mut log = crate::log::Log::new(self.log);
        if let Err(e) = record_fallback_at(&self.marker) {
            log.warn(&format!("renderer fallback could not be recorded: {e}"));
        }
        log.warn(&format!(
            "the UI thread showed no frame and no heartbeat for {} s; relaunching with the software renderer",
            WATCHDOG_TIMEOUT.as_secs()
        ));
        let spawned = self.exe.and_then(|exe| relaunch_command(&exe, &self.args).spawn().map_err(|e| e.to_string()));
        match spawned {
            // The marker and the log are written and closed above.
            Ok(_) => crate::platform::terminate_self(),
            Err(e) => {
                log.warn(&format!("software relaunch failed ({e}); this instance carries on"));
                false
            }
        }
    }
}

/// The software relaunch: `exe` with `args` (which carry [`FALLBACK_ARG`], so `decide` selects
/// software). No SLINT_BACKEND: the relaunched instance would then count as overridden by the
/// environment (Settings: "Overridden by SLINT_BACKEND"); one inherited is removed as well.
fn relaunch_command(exe: &Path, args: &[OsString]) -> std::process::Command {
    let mut cmd = std::process::Command::new(exe);
    cmd.args(args).env_remove("SLINT_BACKEND");
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decides_gpu_with_a_watchdog_by_default() {
        assert_eq!(decide(false, true, false, false), Decision { renderer: Renderer::Gpu, watchdog: true });
    }

    #[test]
    fn turning_gpu_rendering_off_selects_software_unguarded() {
        let software = Decision { renderer: Renderer::Software, watchdog: false };
        assert_eq!(decide(false, false, false, false), software);
        assert_eq!(decide(false, false, true, true), software);
    }

    #[test]
    fn a_recorded_fallback_or_a_watchdog_relaunch_selects_software_unguarded() {
        let software = Decision { renderer: Renderer::Software, watchdog: false };
        assert_eq!(decide(false, true, true, false), software);
        assert_eq!(decide(false, true, false, true), software);
        assert_eq!(decide(false, true, true, true), software);
    }

    #[test]
    fn slint_backend_overrides_everything_and_is_never_guarded() {
        for gpu in [false, true] {
            for marker in [false, true] {
                for relaunched in [false, true] {
                    assert_eq!(
                        decide(true, gpu, marker, relaunched),
                        Decision { renderer: Renderer::FromEnv, watchdog: false },
                        "gpu {gpu}, marker {marker}, relaunched {relaunched}"
                    );
                }
            }
        }
    }

    /// A fresh temp directory holding a marker path, removed when dropped.
    struct TempMarker(PathBuf);

    impl TempMarker {
        fn path(&self) -> PathBuf {
            self.0.join("Negotium").join("renderer-fallback")
        }
    }

    impl Drop for TempMarker {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn temp_marker(name: &str) -> TempMarker {
        let d = std::env::temp_dir().join(format!("negotium-renderer-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        TempMarker(d)
    }

    #[test]
    fn the_marker_is_written_read_and_cleared() {
        let dir = temp_marker("roundtrip");
        let path = dir.path();
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
    fn the_gpu_switch_clears_the_marker_only_when_turned_on() {
        let dir = temp_marker("switch");
        let path = dir.path();
        record_fallback_at(&path).unwrap();
        set_gpu_rendering_at(&path, false).unwrap();
        assert!(fallback_recorded_at(&path), "turning it off leaves the record alone");
        set_gpu_rendering_at(&path, true).unwrap();
        assert!(!fallback_recorded_at(&path), "turning it on forgets the automatic fallback");
        set_gpu_rendering_at(&path, true).unwrap(); // nothing to clear is fine
    }

    #[test]
    fn the_marker_lives_under_local_app_data() {
        let p = fallback_path();
        assert!(p.ends_with("Negotium/renderer-fallback"), "{}", p.display());
    }

    #[test]
    fn the_relaunch_adds_its_flags_once() {
        let args = |v: &[&str]| v.iter().map(OsString::from).collect::<Vec<_>>();
        assert_eq!(relaunch_args(args(&["--exit-on-close"]), false), args(&["--exit-on-close", RESTARTED_ARG, FALLBACK_ARG]));
        assert_eq!(relaunch_args(args(&[RESTARTED_ARG]), false), args(&[RESTARTED_ARG, FALLBACK_ARG]));
    }

    /// A launch in the tray relaunches in the tray: the logon trigger's start (no arguments) would
    /// otherwise come back with the flags only, and show the window.
    #[test]
    fn a_relaunch_of_a_tray_start_stays_in_the_tray() {
        use crate::integrate::{TRAY, starts_in_tray, task_args};
        let args = |v: &[&str]| v.iter().map(OsString::from).collect::<Vec<_>>();
        for launch in [&["--from-task"][..], &["--from-task", "$(Arg0)"], &["--tray"]] {
            let relaunched = relaunch_args(args(launch), true);
            assert_eq!(relaunched.iter().filter(|a| *a == TRAY).count(), 1, "{launch:?}");
            let (from_task, rest) = task_args(relaunched);
            assert!(starts_in_tray(from_task, &rest), "{launch:?}");
        }
        assert!(!relaunch_args(args(&[]), false).iter().any(|a| a == TRAY));
    }

    fn counting() -> (Arc<std::sync::atomic::AtomicUsize>, impl FnOnce() -> bool + Send + 'static) {
        let n = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let m = n.clone();
        // `true`: the replacement started (production terminates instead of returning).
        (n, move || {
            m.fetch_add(1, Ordering::SeqCst);
            true
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

    const POLL: Duration = Duration::from_millis(100);

    /// Feeds `clock` polls `POLL` apart for `span`, with `beats` heartbeats each poll (0: silent),
    /// and returns the last verdict, stopping early at anything but Wait.
    fn run_for(clock: &mut Clock, now: &mut Instant, beats: &mut u64, per_poll: u64, span: Duration) -> Verdict {
        let end = *now + span;
        let mut v = Verdict::Wait;
        while *now < end {
            *now += POLL;
            *beats += per_poll;
            v = clock.observe(*now, *beats, POLL, T);
            if v != Verdict::Wait {
                break;
            }
        }
        v
    }

    #[test]
    fn the_clock_fires_after_a_timeout_of_silence() {
        let start = Instant::now();
        let (mut clock, mut now, mut beats) = (Clock::new(start), start, 0);
        assert_eq!(run_for(&mut clock, &mut now, &mut beats, 0, s(9)), Verdict::Wait);
        assert_eq!(run_for(&mut clock, &mut now, &mut beats, 0, s(2)), Verdict::Fire);
        assert_eq!(now - start, s(10), "fires once ten seconds of silence have passed");
    }

    #[test]
    fn a_suspend_is_lost_time_not_silence() {
        let start = Instant::now();
        let (mut clock, mut now, mut beats) = (Clock::new(start), start, 0);
        // Asleep 2 s into the launch, before the first frame or heartbeat.
        assert_eq!(run_for(&mut clock, &mut now, &mut beats, 0, s(2)), Verdict::Wait);
        now += s(3600);
        assert_eq!(clock.observe(now, beats, POLL, T), Verdict::Wait, "an hour asleep is not a stall");
        // Back awake, the UI thread beats: no fire, and it resolves healthy in awake time.
        assert_eq!(run_for(&mut clock, &mut now, &mut beats, 1, s(9)), Verdict::Wait);
        assert_eq!(run_for(&mut clock, &mut now, &mut beats, 1, s(2)), Verdict::Healthy);
    }

    #[test]
    fn real_silence_on_both_sides_of_a_suspend_still_fires() {
        let start = Instant::now();
        let (mut clock, mut now, mut beats) = (Clock::new(start), start, 0);
        assert_eq!(run_for(&mut clock, &mut now, &mut beats, 0, s(6)), Verdict::Wait);
        now += s(600);
        assert_eq!(clock.observe(now, beats, POLL, T), Verdict::Wait);
        // 6 s + one poll for the lost interval + this: ten awake seconds of silence.
        assert_eq!(run_for(&mut clock, &mut now, &mut beats, 0, s(5)), Verdict::Fire);
    }

    #[test]
    fn a_late_poll_within_the_slack_counts_in_full() {
        let start = Instant::now();
        let mut clock = Clock::new(start);
        // A busy machine delays every poll by 1.9 s: still ordinary time, so silence adds up.
        for ms in [2000, 4000, 6000, 8000, 9900] {
            assert_eq!(clock.observe(start + Duration::from_millis(ms), 0, POLL, T), Verdict::Wait, "{ms} ms");
        }
        assert_eq!(clock.observe(start + s(10), 0, POLL, T), Verdict::Fire);
        // Beyond the slack (2 s over the poll) the interval is lost instead.
        let mut clock = Clock::new(start);
        assert_eq!(clock.observe(start + s(9), 0, POLL, T), Verdict::Wait, "9 s: lost, counted as one poll");
        assert_eq!(clock.observe(start + Duration::from_millis(9500), 0, POLL, T), Verdict::Wait);
    }

    #[test]
    fn a_failed_relaunch_stands_the_watchdog_down() {
        let (w, thread) = Watchdog::arm(Duration::from_millis(20), || false);
        thread.join().unwrap();
        assert!(w.resolved(), "no replacement is coming: this instance carries on and may add its tray icon");
        assert!(w.presented(), "a later first frame is welcome");
    }

    #[test]
    fn the_relaunch_is_resolved_up_front_with_its_flags() {
        let r = SoftwareRelaunch::resolve(false);
        assert!(r.exe.is_ok());
        assert!(r.args.ends_with(&[OsString::from(RESTARTED_ARG), OsString::from(FALLBACK_ARG)]));
    }

    /// The relaunched instance selects software through `decide` (its `--renderer-fallback`), not
    /// through SLINT_BACKEND: an inherited SLINT_BACKEND would make it FromEnv, and Settings would
    /// then wrongly say the renderer was overridden.
    #[test]
    fn the_relaunched_instance_selects_software_without_slint_backend() {
        let exe = std::env::temp_dir().join("negotium.exe");
        let cmd = relaunch_command(&exe, &relaunch_args(std::iter::empty(), false));
        let env_backend_set = cmd.get_envs().any(|(k, v)| k == "SLINT_BACKEND" && v.is_some());
        assert!(!env_backend_set, "no SLINT_BACKEND is passed");
        assert!(
            cmd.get_envs().any(|(k, v)| k == "SLINT_BACKEND" && v.is_none()),
            "an inherited one is removed too"
        );
        let relaunched = cmd.get_args().any(|a| a == FALLBACK_ARG);
        for gpu_rendering in [false, true] {
            for marker in [false, true] {
                assert_eq!(
                    decide(env_backend_set, gpu_rendering, marker, relaunched),
                    Decision { renderer: Renderer::Software, watchdog: false },
                    "gpu {gpu_rendering}, marker {marker}"
                );
            }
        }
    }

    #[test]
    fn a_relaunch_that_cannot_start_records_the_fallback_logs_and_returns() {
        for (name, exe) in [
            ("missing", Ok(std::env::temp_dir().join("negotium-no-such-dir").join("missing.exe"))),
            ("unresolved", Err("current_exe failed".to_string())),
        ] {
            let dir = temp_marker(&format!("relaunch-{name}"));
            let relaunch = SoftwareRelaunch {
                exe,
                args: vec![],
                marker: dir.path(),
                log: dir.0.join("negotium.log"),
            };
            assert!(!relaunch.run(), "{name}: reports that no replacement started");
            assert!(fallback_recorded_at(&dir.path()), "{name}: later launches still use software");
            let log = std::fs::read_to_string(dir.0.join("negotium.log")).unwrap();
            assert!(log.contains("software relaunch failed"), "{name}: {log}");
        }
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
