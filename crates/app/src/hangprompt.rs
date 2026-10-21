//! The "isn't responding" prompt: polls the hung apps the worker found (`Controls::hung_apps`) and
//! shows [`HangPrompt`] for one at a time (decisions in `crate::hangwatch`).
use crate::controls::Controls;
use crate::hangwatch::{HangWatch, HungApp, Poll, command_line_args};
use crate::settings::{Saver, Settings};
use crate::{HangPrompt, platform};
use slint::ComponentHandle;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

const POLL: Duration = Duration::from_secs(1);
/// How often the strip follows its ghost window.
const TRACK: Duration = Duration::from_millis(50);
/// How long a missing ghost is tolerated (in `TRACK` ticks) before falling back to the corner prompt.
const GHOST_LOST_TICKS: u32 = 20;
const CORNER_SIZE: (f32, f32) = (420.0, 176.0);
const STRIP_HEIGHT: f32 = 32.0;
const MARGIN: f32 = 16.0;

struct State {
    watch: HangWatch,
    /// The corner prompt, made on first use.
    prompt: Option<HangPrompt>,
    /// The strip over the hung window's title bar (Settings > experimental), made on first use.
    strip: Option<HangPrompt>,
    /// The ghost window the strip follows while it is shown.
    ghost: Option<isize>,
    ghost_lost: u32,
    current: Option<HungApp>,
    epoch: Instant,
}

/// The polling timer and the strip's tracking timer; keep both alive.
pub struct Timers(#[allow(dead_code)] slint::Timer, #[allow(dead_code)] slint::Timer);

type Shared = Rc<(Rc<RefCell<Settings>>, Saver, bool)>;

/// Starts polling. Keep the returned timers alive for as long as the prompt should work.
pub fn install(settings: Rc<RefCell<Settings>>, saver: Saver, controls: Arc<Controls>, elevated: bool) -> Timers {
    let state = Rc::new(RefCell::new(State {
        watch: HangWatch::new(settings.borrow().hang_ignored.clone()),
        prompt: None,
        strip: None,
        ghost: None,
        ghost_lost: 0,
        current: None,
        epoch: Instant::now(),
    }));
    let shared: Shared = Rc::new((settings, saver, elevated));
    let poll_timer = slint::Timer::default();
    {
        let (state, shared, controls) = (state.clone(), shared.clone(), controls.clone());
        poll_timer.start(slint::TimerMode::Repeated, POLL, move || {
            let (enabled, apps) = (shared.0.borrow().hang_prompt, controls.hung_apps());
            let poll = {
                let mut s = state.borrow_mut();
                let now = s.epoch.elapsed().as_millis() as u64;
                s.watch.poll(now, enabled, &apps)
            };
            match poll {
                Poll::Keep => {
                    // The prompted app keeps counting up.
                    let s = state.borrow();
                    if let Some(cur) = &s.current
                        && let Some(app) = apps.iter().find(|a| a.key() == cur.key())
                    {
                        for p in [&s.prompt, &s.strip].into_iter().flatten() {
                            p.set_detail(detail(app).into());
                            p.set_seconds(format!("{}s", app.hung_ms / 1000).into());
                        }
                    }
                }
                Poll::Hide => hide(&state),
                Poll::Show(app) => show(&state, &shared, app),
            }
        });
    }
    let track_timer = slint::Timer::default();
    {
        let state = state.clone();
        track_timer.start(slint::TimerMode::Repeated, TRACK, move || track(&state, &shared));
    }
    Timers(poll_timer, track_timer)
}

fn detail(app: &HungApp) -> String {
    format!("Not responding for {}s. Wait to give it more time, or end it now; unsaved work in it will be lost.", app.hung_ms / 1000)
}

fn hide(state: &Rc<RefCell<State>>) {
    let mut s = state.borrow_mut();
    s.current = None;
    s.ghost = None;
    for p in [&s.prompt, &s.strip].into_iter().flatten() {
        let _ = p.hide();
    }
}

fn show(state: &Rc<RefCell<State>>, shared: &Shared, app: HungApp) {
    let ghost = shared.0.borrow().hang_overlay.then(|| {
        negotium_collector::windows_scan::ghost_windows().into_iter().find(|g| g.pid == app.pid)
    });
    state.borrow_mut().current = Some(app.clone());
    if let Some(Some(g)) = ghost
        && show_strip(state, shared, &app, g)
    {
        return;
    }
    show_corner(state, shared, &app);
}

/// Makes the prompt window on first use and wires its buttons.
fn ensure(slot: &mut Option<HangPrompt>, compact: bool, state: &Rc<RefCell<State>>, shared: &Shared) -> bool {
    if slot.is_none() {
        let Ok(p) = HangPrompt::new() else { return false };
        p.set_compact(compact);
        wire(&p, state, &shared.0, &shared.1, shared.2);
        *slot = Some(p);
    }
    true
}

fn fill(p: &HangPrompt, app: &HungApp, elevated: bool) {
    p.set_app_name(app.name.clone().into());
    p.set_detail(detail(app).into());
    p.set_seconds(format!("{}s", app.hung_ms / 1000).into());
    p.set_status("".into());
    p.set_busy(false);
    // Restarting an app from an elevated Negotium would hand it admin rights it did not have.
    p.set_can_relaunch(!elevated);
}

fn show_corner(state: &Rc<RefCell<State>>, shared: &Shared, app: &HungApp) {
    let mut s = state.borrow_mut();
    if let Some(strip) = &s.strip {
        let _ = strip.hide();
    }
    s.ghost = None;
    if !ensure(&mut s.prompt, false, state, shared) {
        s.watch.closed();
        return;
    }
    let Some(p) = &s.prompt else { return };
    fill(p, app, shared.2);
    if p.show().is_ok() {
        platform::make_non_activating(p.window());
        // Bottom-right of the work area, above the taskbar.
        if let Some((_, _, right, bottom)) = platform::primary_work_area() {
            let scale = p.window().scale_factor();
            let (w, h, m) = ((CORNER_SIZE.0 * scale) as i32, (CORNER_SIZE.1 * scale) as i32, (MARGIN * scale) as i32);
            p.window().set_size(slint::PhysicalSize::new(w as u32, h as u32));
            p.window().set_position(slint::PhysicalPosition::new(right - w - m, bottom - h - m));
        }
    }
}

/// The strip over the ghost's title bar. Returns false when it can't be made or placed, so the
/// caller falls back to the corner prompt.
fn show_strip(state: &Rc<RefCell<State>>, shared: &Shared, app: &HungApp, ghost: negotium_collector::windows_scan::GhostWindow) -> bool {
    let mut s = state.borrow_mut();
    if !ensure(&mut s.strip, true, state, shared) {
        return false;
    }
    let Some(p) = &s.strip else { return false };
    fill(p, app, shared.2);
    if p.show().is_err() {
        return false;
    }
    platform::make_non_activating(p.window());
    let height = (STRIP_HEIGHT * p.window().scale_factor()) as i32;
    let placed = platform::place_above(p.window(), ghost.hwnd, ghost.rect, height);
    if !placed {
        let _ = p.hide();
        return false;
    }
    if let Some(corner) = &s.prompt {
        let _ = corner.hide();
    }
    s.ghost = Some(ghost.hwnd);
    s.ghost_lost = 0;
    true
}

/// Keeps the strip on its ghost window; when the ghost stays gone while the app is still hung,
/// falls back to the corner prompt.
fn track(state: &Rc<RefCell<State>>, shared: &Shared) {
    let fallback = {
        let mut s = state.borrow_mut();
        let (Some(ghost), Some(strip)) = (s.ghost, &s.strip) else { return };
        match negotium_collector::windows_scan::ghost_rect(ghost) {
            Some(rect) => {
                let height = (STRIP_HEIGHT * strip.window().scale_factor()) as i32;
                let (left, right) = (rect.0, rect.2);
                strip.window().set_size(slint::PhysicalSize::new((right - left).max(1) as u32, height as u32));
                platform::place_above(strip.window(), ghost, rect, height);
                s.ghost_lost = 0;
                None
            }
            None => {
                s.ghost_lost += 1;
                (s.ghost_lost >= GHOST_LOST_TICKS).then(|| s.current.clone()).flatten()
            }
        }
    };
    if let Some(app) = fallback {
        show_corner(state, shared, &app);
    }
}

fn wire(p: &HangPrompt, state: &Rc<RefCell<State>>, settings: &Rc<RefCell<Settings>>, saver: &Saver, elevated: bool) {
    {
        let state = state.clone();
        p.on_wait(move || {
            let key = state.borrow().current.as_ref().map(HungApp::key);
            if let Some(key) = key {
                let now = state.borrow().epoch.elapsed().as_millis() as u64;
                state.borrow_mut().watch.wait(key, now);
            }
            hide(&state);
        });
    }
    {
        let (state, settings, saver) = (state.clone(), settings.clone(), saver.clone());
        p.on_ignore_app(move || {
            let name = state.borrow().current.as_ref().map(|a| a.process_name.clone());
            if let Some(name) = name {
                let list = {
                    let mut s = state.borrow_mut();
                    s.watch.ignore(&name);
                    s.watch.ignored()
                };
                let snapshot = {
                    let mut st = settings.borrow_mut();
                    st.hang_ignored = list;
                    st.clone()
                };
                drop(saver.save(snapshot));
            }
            hide(&state);
        });
    }
    {
        let (state, handle) = (state.clone(), p.as_weak());
        p.on_force_quit(move || end(&state, &handle, false, elevated));
    }
    {
        let (state, handle) = (state.clone(), p.as_weak());
        p.on_relaunch(move || end(&state, &handle, true, elevated));
    }
}

/// Ends the prompted app on an action thread; on success the prompt closes (the next poll sees the
/// app gone and tidies the state), otherwise it shows why.
fn end(state: &Rc<RefCell<State>>, handle: &slint::Weak<HangPrompt>, relaunch: bool, elevated: bool) {
    let Some(app) = state.borrow().current.clone() else { return };
    if let Some(p) = handle.upgrade() {
        p.set_busy(true);
        p.set_status("".into());
    }
    let handle = handle.clone();
    std::thread::spawn(move || {
        let result = end_blocking(&app, relaunch, elevated);
        let _ = slint::invoke_from_event_loop(move || {
            let Some(p) = handle.upgrade() else { return };
            match result {
                Ok(()) => {
                    let _ = p.hide();
                }
                Err(message) => {
                    p.set_busy(false);
                    p.set_status(message.into());
                }
            }
        });
    });
}

/// Blocking. Reads how to restart the app first (the process is gone afterwards), ends it, then
/// starts it again from its image path and arguments.
fn end_blocking(app: &HungApp, relaunch: bool, elevated: bool) -> Result<(), String> {
    let details = relaunch.then(|| negotium_actions::process_details(app.pid, app.create_time).ok()).flatten();
    match negotium_actions::end_process(app.pid, app.create_time) {
        Ok(()) | Err(negotium_actions::ActionError::AlreadyExited) => {}
        Err(e) => return Err(e.message(&app.name, elevated)),
    }
    if let Some(d) = details {
        let Some(path) = d.image_path else { return Err(format!("{} ended, but its program file is unknown so it can't be restarted.", app.name)) };
        let mut cmd = std::process::Command::new(&path);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            if let Some(args) = d.command_line.as_deref().map(command_line_args).filter(|a| !a.is_empty()) {
                cmd.raw_arg(args);
            }
        }
        if let Some(dir) = std::path::Path::new(&path).parent() {
            cmd.current_dir(dir);
        }
        cmd.spawn().map_err(|e| format!("{} ended but could not be restarted: {e}.", app.name))?;
    }
    Ok(())
}
