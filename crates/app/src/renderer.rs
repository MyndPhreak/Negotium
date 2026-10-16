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
//!   that no frame was presented within [`WATCHDOG_TIMEOUT`], records the fallback, relaunches
//!   the exe with the software renderer and exits.
//!
//! The fallback is recorded in a marker file ([`fallback_path`]) and every later launch honours
//! it. Only an explicit user action clears it ([`clear_fallback`], for the Settings switch of a
//! later task), so the renderer never flips back and forth. Task 1 folds the marker into
//! settings.json (`gpu_rendering: false`) by redirecting the three functions below.
//! `SLINT_BACKEND` overrides both the default and the marker.
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

/// How long the main window has to present its first frame before the watchdog relaunches.
pub const WATCHDOG_TIMEOUT: Duration = Duration::from_secs(3);
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

pub fn record_fallback_at(path: &Path) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, "The GPU renderer stalled at startup; Negotium uses the software renderer.\n")
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

/// Waits on its own thread for the first frame; if none arrives within the timeout, runs the
/// stall callback (once, from that thread). Disarmed by [`Disarm::disarm`], or when every
/// [`Disarm`] has been dropped.
pub struct Watchdog;

/// Tells the watchdog the first frame arrived. Cheap to clone; disarming twice is harmless.
#[derive(Clone)]
pub struct Disarm(Sender<()>);

impl Disarm {
    pub fn disarm(&self) {
        let _ = self.0.send(());
    }
}

impl Watchdog {
    pub fn arm(timeout: Duration, on_stall: impl FnOnce() + Send + 'static) -> (Disarm, JoinHandle<()>) {
        let (tx, rx) = mpsc::channel::<()>();
        let thread = std::thread::Builder::new()
            .name("negotium-renderer-watchdog".into())
            .spawn(move || {
                if let Err(RecvTimeoutError::Timeout) = rx.recv_timeout(timeout) {
                    on_stall();
                }
            })
            .expect("spawn the renderer watchdog");
        (Disarm(tx), thread)
    }
}

/// The stall callback: records the fallback, relaunches this exe with the software renderer
/// and exits, which releases the single-instance lock the relaunch waits for. The stalled
/// instance has no tray icon yet (it is created after the first frame), so none is orphaned.
/// If the relaunch cannot start, this instance keeps waiting: the window may still appear, and
/// the next launch uses software anyway.
pub fn relaunch_in_software() {
    let log = |msg: &str| crate::log::Log::new(crate::log::Log::default_path()).warn(msg);
    if let Err(e) = record_fallback() {
        log(&format!("renderer fallback could not be recorded: {e}"));
    }
    log("the GPU renderer showed no frame within 3 s; relaunching with the software renderer");
    let spawned = std::env::current_exe().and_then(|exe| {
        std::process::Command::new(exe)
            .args(relaunch_args(std::env::args_os().skip(1)))
            .env("SLINT_BACKEND", SOFTWARE_BACKEND)
            .spawn()
    });
    match spawned {
        Ok(_) => std::process::exit(0),
        Err(e) => log(&format!("software relaunch failed: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

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

    fn counting() -> (Arc<AtomicUsize>, impl FnOnce() + Send + 'static) {
        let n = Arc::new(AtomicUsize::new(0));
        let m = n.clone();
        (n, move || {
            m.fetch_add(1, Ordering::SeqCst);
        })
    }

    #[test]
    fn a_first_frame_in_time_disarms_the_watchdog() {
        let (stalls, on_stall) = counting();
        let (disarm, thread) = Watchdog::arm(Duration::from_secs(30), on_stall);
        disarm.disarm();
        disarm.disarm(); // a second signal is harmless
        thread.join().unwrap(); // returns at once, not after 30 s
        assert_eq!(stalls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn no_frame_runs_the_stall_callback_exactly_once() {
        let (stalls, on_stall) = counting();
        let (disarm, thread) = Watchdog::arm(Duration::from_millis(20), on_stall);
        thread.join().unwrap();
        disarm.disarm(); // too late: nothing more happens
        assert_eq!(stalls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn dropping_every_disarm_disarms() {
        let (stalls, on_stall) = counting();
        let (disarm, thread) = Watchdog::arm(Duration::from_secs(30), on_stall);
        drop(disarm);
        thread.join().unwrap();
        assert_eq!(stalls.load(Ordering::SeqCst), 0);
    }
}
