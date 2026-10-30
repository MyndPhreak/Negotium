//! The window picker: one dimmed overlay per monitor; a click on a window ends its app.
//! This file holds the session (what is hovered, the snapshot, what a click does) and runs the
//! end job; `overlay` is the Win32 side (windows, input, drawing, dialogs).
pub mod apps;
pub mod click;
pub mod label;
pub mod overlay;
pub mod scene;
pub mod target;
pub mod win;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

use apps::{AppIndex, AppInfo, single_process};
use click::{Outcome, Pressed, decide};
use label::{Info, label};
use overlay::{Input, Overlay, Painter};
use target::{Env, Pickable, Rect, WindowInfo, pick_at};
use win::Monitor;

use crate::dispatch::Job;

/// What the picker needs from the rest of the app.
pub struct PickerCtx {
    pub elevated: bool,
}

thread_local! {
    /// The open picker (UI thread only). Never borrowed across a dialog or a window destroy.
    static SESSION: RefCell<Option<Session>> = const { RefCell::new(None) };
    /// Counts opens, so a snapshot that arrives after its picker closed is dropped.
    static OPENS: Cell<u64> = const { Cell::new(0) };
}

/// The app index built from the picker's own snapshot (Decision 4).
struct Ready {
    index: AppIndex,
    all_procs: Vec<(u32, i64, u32)>,
}

/// The window under the cursor. Worked out once per target change, not per mouse move.
struct Hover {
    window: WindowInfo,
    kind: Pickable,
    /// The process image name, shown until the snapshot is ready.
    exe: String,
    needs_admin: bool,
}

struct Session {
    elevated: bool,
    open_id: u64,
    shell: Option<(u32, i64)>,
    env: Env,
    windows: Vec<WindowInfo>,
    monitors: Vec<Monitor>,
    overlays: Vec<Overlay>,
    painter: Painter,
    ready: Option<Ready>,
    hover: Option<Hover>,
    cursor_monitor: Option<usize>,
    pressed: Option<Pressed>,
    /// A dialog is open: pointer input is ignored until it closes.
    modal: bool,
}

/// What a handled input asks for once the session is no longer borrowed.
enum Step {
    Nothing,
    Close,
    Tell { owner: isize, text: String },
    Confirm { owner: isize, title: String, body: String, job: Job, restart_shell: bool },
    Run { job: Job, restart_shell: bool },
}

fn warn(msg: &str) {
    crate::log::Log::new(crate::log::Log::default_path()).warn(msg);
}

fn with_session<R>(f: impl FnOnce(&mut Session) -> R) -> Option<R> {
    SESSION.with(|s| s.try_borrow_mut().ok()?.as_mut().map(f))
}

/// Opens the picker on every monitor. Does nothing when it is already open (spec goal 6).
/// UI thread only. Any failure closes every overlay and is logged.
pub fn open(ctx: &PickerCtx) {
    if is_open() {
        return;
    }
    let open_id = OPENS.with(|n| {
        n.set(n.get() + 1);
        n.get()
    });
    match start(ctx, open_id) {
        Ok(session) => {
            let first = session.overlays[0].hwnd();
            SESSION.with(|s| *s.borrow_mut() = Some(session));
            overlay::grab(first);
            spawn_snapshot(open_id);
        }
        Err(e) => {
            overlay::release();
            warn(&format!("picker: could not open: {e}"));
        }
    }
}

/// Whether the picker is open on this thread.
pub fn is_open() -> bool {
    SESSION.with(|s| s.try_borrow().map_or(true, |s| s.is_some()))
}

/// Releases the mouse and destroys every overlay. Safe to call when closed.
pub fn close_all() {
    let session = SESSION.with(|s| s.try_borrow_mut().ok().and_then(|mut s| s.take()));
    overlay::release();
    // Dropped outside the borrow: destroying a window sends messages back into `on_input`.
    drop(session);
}

fn start(ctx: &PickerCtx, open_id: u64) -> windows::core::Result<Session> {
    let dpi = overlay::DpiGuard::per_monitor_v2();
    if dpi.changed() {
        warn("picker: the UI thread was not per-monitor-v2 DPI aware; the overlays set it themselves");
    }
    let monitors = win::monitors();
    if monitors.is_empty() {
        return Err(windows::core::Error::new(windows::core::HRESULT(-1), "no monitors"));
    }
    let painter = Painter::new()?;
    let overlays = monitors.iter().map(Overlay::create).collect::<windows::core::Result<Vec<_>>>()?;
    let shell = win::shell_pid();
    let env = Env {
        own_pid: std::process::id(),
        shell_pid: shell.map(|(p, _)| p),
        overlays: overlays.iter().map(Overlay::hwnd).collect(),
    };
    let mut s = Session {
        elevated: ctx.elevated,
        open_id,
        shell,
        env,
        windows: win::top_level_windows(),
        monitors,
        overlays,
        painter,
        ready: None,
        hover: None,
        cursor_monitor: None,
        pressed: None,
        modal: false,
    };
    s.track_cursor(true);
    s.paint()?;
    Ok(s)
}

/// Builds the app index on a thread and hands it to the session that asked for it.
fn spawn_snapshot(open_id: u64) {
    std::thread::spawn(move || {
        let snap = negotium_collector::Collector::new().tick();
        let rows = negotium_model::Model::with_system_root(negotium_collector::windows_dir()).update(&snap);
        let hung: HashSet<u32> = snap.windows.iter().filter(|w| w.hung).map(|w| w.pid).collect();
        let image_of: HashMap<(u32, i64), String> = snap
            .processes
            .iter()
            .filter_map(|p| p.image_path.clone().map(|i| ((p.pid, p.create_time), i)))
            .collect();
        let ready = Ready {
            index: AppIndex::from_rows(&rows, &hung, &image_of),
            all_procs: snap.processes.iter().map(|p| (p.pid, p.create_time, p.parent_pid)).collect(),
        };
        // Fails only once the event loop is gone (quitting): nothing left to show.
        let _ = slint::invoke_from_event_loop(move || snapshot_ready(open_id, ready));
    });
}

fn snapshot_ready(open_id: u64, ready: Ready) {
    let painted = with_session(|s| {
        if s.open_id != open_id {
            return Ok(());
        }
        s.ready = Some(ready);
        // The root may differ from the window's process now that the app is known.
        if let Some(h) = &s.hover {
            let needs_admin = s.needs_admin(&h.window, h.kind);
            if let Some(h) = s.hover.as_mut() {
                h.needs_admin = needs_admin;
            }
        }
        s.paint()
    });
    if let Some(Err(e)) = painted {
        warn(&format!("picker: could not draw: {e}"));
        close_all();
    }
}

/// Called by the overlay window procedure for every input it reports.
fn on_input(hwnd: isize, input: Input) {
    let step = with_session(|s| s.handle(hwnd, input)).unwrap_or(Step::Nothing);
    match step {
        Step::Nothing => {}
        Step::Close => close_all(),
        Step::Run { job, restart_shell } => run(job, restart_shell),
        Step::Tell { owner, text } => {
            overlay::release();
            overlay::tell(Some(owner), &text);
            back_to_picking();
        }
        Step::Confirm { owner, title, body, job, restart_shell } => {
            overlay::release();
            if overlay::confirm(owner, &title, &body) {
                run(job, restart_shell);
            } else {
                back_to_picking();
            }
        }
    }
}

/// After a dialog: the windows may have changed, so list them again and take the mouse back.
fn back_to_picking() {
    let first = with_session(|s| {
        s.modal = false;
        s.windows = win::top_level_windows();
        s.track_cursor(true);
        s.paint().map(|()| s.overlays[0].hwnd())
    });
    match first {
        Some(Ok(hwnd)) => overlay::grab(hwnd),
        Some(Err(e)) => {
            warn(&format!("picker: could not draw: {e}"));
            close_all();
        }
        None => {} // closed while the dialog was open
    }
}

/// Closes the picker and ends the app on an action thread; a failure is shown in a dialog.
fn run(job: Job, restart_shell: bool) {
    let Some((elevated, shell)) = with_session(|s| (s.elevated, s.shell)) else { return };
    close_all();
    std::thread::spawn(move || {
        let text = if restart_shell {
            end_with_restart(&mut LiveShell { job: Some(job), elevated, shell, old_taskbar: None }, elevated)
        } else {
            crate::dispatch::run(job, elevated).toast()
        };
        if !text.is_empty() {
            overlay::tell(None, &text);
        }
    });
}

impl Session {
    fn handle(&mut self, hwnd: isize, input: Input) -> Step {
        match input {
            Input::Cancel => Step::Close,
            _ if self.modal => Step::Nothing,
            Input::Move => {
                if !self.track_cursor(false) {
                    return Step::Nothing;
                }
                match self.paint() {
                    Ok(()) => Step::Nothing,
                    Err(e) => {
                        warn(&format!("picker: could not draw: {e}"));
                        Step::Close
                    }
                }
            }
            Input::Down => {
                self.pressed = self.hover.as_ref().map(|h| {
                    let info = self.info(h);
                    Pressed { kind: shown_kind(h.kind, info.as_ref()), info, needs_admin: h.needs_admin }
                });
                Step::Nothing
            }
            Input::Up { shift } => {
                let Some(pressed) = self.pressed.take() else { return Step::Nothing };
                let all_procs = self.ready.as_ref().map_or(&[][..], |r| &r.all_procs[..]);
                // The dialog goes on the monitor with the cursor; the captured overlay may be another.
                let owner = self.cursor_monitor.map_or(hwnd, |i| self.overlays[i].hwnd());
                match decide(&pressed, shift, all_procs) {
                    Outcome::Ignore => Step::Nothing,
                    Outcome::Tell(text) => {
                        self.modal = true;
                        Step::Tell { owner, text }
                    }
                    Outcome::Confirm { title, body, job, restart_shell } => {
                        self.modal = true;
                        Step::Confirm { owner, title, body, job, restart_shell }
                    }
                    Outcome::Run { job, restart_shell } => Step::Run { job, restart_shell },
                }
            }
        }
    }

    /// Picks the window under the cursor. Returns whether the target or the cursor's monitor
    /// changed, so the overlays need drawing; `force` works the target out again regardless.
    fn track_cursor(&mut self, force: bool) -> bool {
        let pt = overlay::cursor();
        let monitor = self.monitors.iter().position(|m| inside(m.rect, pt));
        let hit = pick_at(&self.windows, pt, &self.env).map(|(w, k)| (w.clone(), k));
        let same = hit.as_ref().map(|(w, _)| w.hwnd) == self.hover.as_ref().map(|h| h.window.hwnd);
        if !force && same && monitor == self.cursor_monitor {
            return false;
        }
        self.cursor_monitor = monitor;
        if force || !same {
            self.hover = hit.map(|(window, kind)| {
                let exe = if kind == Pickable::Shell { String::new() } else { win::image_name(window.pid) };
                let needs_admin = self.needs_admin(&window, kind);
                Hover { window, kind, exe, needs_admin }
            });
        }
        true
    }

    /// The app the window belongs to; `None` until the snapshot is ready.
    fn info(&self, h: &Hover) -> Option<AppInfo> {
        let ready = self.ready.as_ref()?;
        let (pid, ct) = (h.window.pid, h.window.create_time);
        Some(ready.index.get(pid, ct).cloned().unwrap_or_else(|| single_process(pid, ct, &h.exe)))
    }

    /// Not elevated and the target's root (the window's process until the snapshot is ready)
    /// cannot be opened with terminate rights.
    fn needs_admin(&self, w: &WindowInfo, kind: Pickable) -> bool {
        if self.elevated || !matches!(kind, Pickable::App | Pickable::ExplorerInShell) {
            return false;
        }
        let own = (w.pid, w.create_time);
        let root = self.ready.as_ref().and_then(|r| r.index.get(own.0, own.1)).and_then(|a| a.target.root);
        let (pid, ct) = root.unwrap_or(own);
        !negotium_actions::can_terminate(pid, ct)
    }

    fn paint(&mut self) -> windows::core::Result<()> {
        let target = self.hover.as_ref().map(|h| {
            let info = self.info(h);
            let kind = shown_kind(h.kind, info.as_ref());
            let l = match info {
                Some(info) => label(kind, Info::Ready(&info), h.needs_admin, h.window.hung || info.hung),
                None => label(kind, Info::Loading { exe: &h.exe }, h.needs_admin, h.window.hung),
            };
            (h.window.frame, l, kind == Pickable::Shell)
        });
        for (i, (m, o)) in self.monitors.iter().zip(self.overlays.iter_mut()).enumerate() {
            let mut sc = scene::layout(m, target.as_ref().map(|(r, l, _)| (*r, l)), self.cursor_monitor == Some(i));
            // A shell surface stays dimmed: only its label shows.
            if target.as_ref().is_some_and(|(_, _, shell)| *shell) {
                sc.hole = None;
            }
            o.paint(&self.painter, &sc)?;
        }
        Ok(())
    }
}

/// How the hovered window is shown and clicked once its app is known: a window of the Windows
/// group is treated as the shell (dimmed, shell label, click ignored), since no action may end it.
fn shown_kind(kind: Pickable, info: Option<&AppInfo>) -> Pickable {
    if info.is_some_and(|i| i.target.group) { Pickable::Shell } else { kind }
}

fn inside(r: Rect, (x, y): (i32, i32)) -> bool {
    r.left <= x && x < r.right && r.top <= y && y < r.bottom
}

const NO_ADMIN_RESTART: &str =
    "Negotium cannot start the taskbar again without admin rights, so it did not end File Explorer.";
const TASKBAR_DID_NOT_START: &str = "The taskbar did not start again. Press Ctrl+Shift+Esc, then run explorer.exe.";

/// The steps of ending the shell and starting the taskbar again; a seam so their order is tested.
trait ShellRestart {
    type Token;
    /// The shell's token (`capture_shell_token`); called only when Negotium is elevated.
    fn capture(&mut self) -> Option<Self::Token>;
    /// Runs the end job; returns its result text ("" on full success).
    fn run_job(&mut self) -> String;
    /// Waits up to 5 s for the old taskbar window to go.
    fn wait_old_taskbar_gone(&mut self);
    /// Whether a taskbar window exists within `timeout`.
    fn wait_taskbar(&mut self, timeout: Duration) -> bool;
    /// Starts `explorer.exe`, with the shell's token when given.
    fn start(&mut self, token: Option<&Self::Token>) -> bool;
}

/// Ends the shell-hosted app and makes sure a taskbar comes back. Returns the text to show
/// ("" when there is nothing to say). Elevated, the shell's token is taken before anything
/// ends: without it Explorer could only start elevated, so nothing is ended (Ruling I).
fn end_with_restart<S: ShellRestart>(s: &mut S, elevated: bool) -> String {
    let token = if elevated {
        match s.capture() {
            Some(t) => Some(t),
            None => return NO_ADMIN_RESTART.to_string(),
        }
    } else {
        None
    };
    let result = s.run_job();
    s.wait_old_taskbar_gone();
    // Windows restarts an ended shell itself (AutoRestartShell); a second explorer.exe would
    // only open a folder window.
    if s.wait_taskbar(Duration::from_secs(3)) {
        return result;
    }
    if s.start(token.as_ref()) && s.wait_taskbar(Duration::from_secs(10)) {
        return result;
    }
    if result.is_empty() { TASKBAR_DID_NOT_START.to_string() } else { format!("{result}\n\n{TASKBAR_DID_NOT_START}") }
}

/// `ShellRestart` on the real system.
struct LiveShell {
    job: Option<Job>,
    elevated: bool,
    shell: Option<(u32, i64)>,
    old_taskbar: Option<isize>,
}

impl ShellRestart for LiveShell {
    type Token = negotium_actions::ShellToken;

    fn capture(&mut self) -> Option<Self::Token> {
        let (pid, ct) = self.shell?;
        match negotium_actions::capture_shell_token(pid, ct) {
            Ok(t) => t,
            Err(e) => {
                warn(&format!("picker: could not take the shell's token: {e:?}"));
                None
            }
        }
    }

    fn run_job(&mut self) -> String {
        // Noted first: once the shell ends, any taskbar found is the new one.
        self.old_taskbar = win::taskbar();
        self.job.take().map_or_else(String::new, |job| crate::dispatch::run(job, self.elevated).toast())
    }

    fn wait_old_taskbar_gone(&mut self) {
        if let Some(old) = self.old_taskbar
            && !win::wait_window_gone(old, Duration::from_secs(5))
        {
            warn("picker: the old taskbar was still there 5 s after ending File Explorer");
        }
    }

    fn wait_taskbar(&mut self, timeout: Duration) -> bool {
        negotium_actions::wait_for_taskbar(timeout)
    }

    fn start(&mut self, token: Option<&Self::Token>) -> bool {
        match negotium_actions::start_explorer(token) {
            Ok(()) => true,
            Err(e) => {
                warn(&format!("picker: could not start explorer.exe: {e:?}"));
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_of_the_windows_group_is_shown_as_the_shell() {
        let mut windows = single_process(4, 1, "SearchHost.exe");
        windows.target.group = true;
        let app = single_process(10, 1, "notepad.exe");
        assert_eq!(shown_kind(Pickable::App, Some(&windows)), Pickable::Shell);
        assert_eq!(shown_kind(Pickable::ExplorerInShell, Some(&windows)), Pickable::Shell);
        assert_eq!(shown_kind(Pickable::App, Some(&app)), Pickable::App);
        assert_eq!(shown_kind(Pickable::App, None), Pickable::App);
        assert_eq!(shown_kind(Pickable::Shell, Some(&app)), Pickable::Shell);
    }

    /// Records each step; answers from its fields.
    #[derive(Default)]
    struct Fake {
        log: Vec<String>,
        token: Option<u8>,
        job: String,
        auto_restart: bool,
        starts: bool,
        comes_up: bool,
    }

    impl ShellRestart for Fake {
        type Token = u8;
        fn capture(&mut self) -> Option<u8> {
            self.log.push("capture".into());
            self.token
        }
        fn run_job(&mut self) -> String {
            self.log.push("run".into());
            self.job.clone()
        }
        fn wait_old_taskbar_gone(&mut self) {
            self.log.push("old gone".into());
        }
        fn wait_taskbar(&mut self, timeout: Duration) -> bool {
            self.log.push(format!("wait {}", timeout.as_secs()));
            if timeout == Duration::from_secs(3) { self.auto_restart } else { self.comes_up }
        }
        fn start(&mut self, token: Option<&u8>) -> bool {
            self.log.push(format!("start {token:?}"));
            self.starts
        }
    }

    fn fake() -> Fake {
        Fake { token: Some(7), starts: true, comes_up: true, ..Fake::default() }
    }

    #[test]
    fn elevated_without_the_shell_token_does_not_end_explorer() {
        let mut f = Fake { token: None, ..fake() };
        assert_eq!(end_with_restart(&mut f, true), NO_ADMIN_RESTART);
        assert_eq!(f.log, ["capture"]);
    }

    #[test]
    fn elevated_starts_explorer_with_the_shell_token_after_the_old_taskbar_goes() {
        let mut f = fake();
        assert_eq!(end_with_restart(&mut f, true), "");
        assert_eq!(f.log, ["capture", "run", "old gone", "wait 3", "start Some(7)", "wait 10"]);
    }

    #[test]
    fn not_elevated_takes_no_token_and_starts_a_plain_explorer() {
        let mut f = fake();
        assert_eq!(end_with_restart(&mut f, false), "");
        assert_eq!(f.log, ["run", "old gone", "wait 3", "start None", "wait 10"]);
    }

    #[test]
    fn windows_restarting_the_shell_itself_starts_no_second_explorer() {
        let mut f = Fake { auto_restart: true, ..fake() };
        assert_eq!(end_with_restart(&mut f, true), "");
        assert_eq!(f.log, ["capture", "run", "old gone", "wait 3"]);
    }

    #[test]
    fn a_taskbar_that_does_not_come_back_is_reported() {
        let mut f = Fake { comes_up: false, ..fake() };
        assert_eq!(end_with_restart(&mut f, false), TASKBAR_DID_NOT_START);
        let mut f = Fake { starts: false, ..fake() };
        assert_eq!(end_with_restart(&mut f, false), TASKBAR_DID_NOT_START);
        assert_eq!(f.log, ["run", "old gone", "wait 3", "start None"]);
    }

    #[test]
    fn a_job_failure_is_kept_beside_the_taskbar_text() {
        let mut f = Fake { job: "Access denied.".into(), comes_up: false, ..fake() };
        assert_eq!(end_with_restart(&mut f, false), format!("Access denied.\n\n{TASKBAR_DID_NOT_START}"));
        let mut f = Fake { job: "Access denied.".into(), ..fake() };
        assert_eq!(end_with_restart(&mut f, false), "Access denied.");
    }
}
