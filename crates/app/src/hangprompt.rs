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
const LOGICAL_SIZE: (f32, f32) = (420.0, 176.0);
const MARGIN: f32 = 16.0;

struct State {
    watch: HangWatch,
    prompt: Option<HangPrompt>,
    current: Option<HungApp>,
    epoch: Instant,
}

/// Starts polling. Keep the returned timer alive for as long as the prompt should work.
pub fn install(settings: Rc<RefCell<Settings>>, saver: Saver, controls: Arc<Controls>, elevated: bool) -> slint::Timer {
    let state = Rc::new(RefCell::new(State {
        watch: HangWatch::new(settings.borrow().hang_ignored.clone()),
        prompt: None,
        current: None,
        epoch: Instant::now(),
    }));
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, POLL, move || {
        let (enabled, apps) = (settings.borrow().hang_prompt, controls.hung_apps());
        let poll = {
            let mut s = state.borrow_mut();
            let now = s.epoch.elapsed().as_millis() as u64;
            s.watch.poll(now, enabled, &apps)
        };
        match poll {
            Poll::Keep => {
                // The prompted app keeps counting up.
                let s = state.borrow();
                if let (Some(p), Some(cur)) = (&s.prompt, &s.current)
                    && let Some(app) = apps.iter().find(|a| a.key() == cur.key())
                {
                    p.set_detail(detail(app).into());
                }
            }
            Poll::Hide => hide(&state),
            Poll::Show(app) => show(&state, &settings, &saver, elevated, app),
        }
    });
    timer
}

fn detail(app: &HungApp) -> String {
    format!("Not responding for {}s. Wait to give it more time, or end it now; unsaved work in it will be lost.", app.hung_ms / 1000)
}

fn hide(state: &Rc<RefCell<State>>) {
    let mut s = state.borrow_mut();
    s.current = None;
    if let Some(p) = &s.prompt {
        let _ = p.hide();
    }
}

fn show(state: &Rc<RefCell<State>>, settings: &Rc<RefCell<Settings>>, saver: &Saver, elevated: bool, app: HungApp) {
    let mut s = state.borrow_mut();
    if s.prompt.is_none() {
        match HangPrompt::new() {
            Ok(p) => {
                wire(&p, state, settings, saver, elevated);
                s.prompt = Some(p);
            }
            Err(_) => {
                s.watch.closed();
                return;
            }
        }
    }
    let Some(p) = &s.prompt else { return };
    p.set_app_name(app.name.clone().into());
    p.set_detail(detail(&app).into());
    p.set_status("".into());
    p.set_busy(false);
    // Restarting an app from an elevated Negotium would hand it admin rights it did not have.
    p.set_can_relaunch(!elevated);
    if p.show().is_ok() {
        // Bottom-right of the work area, above the taskbar.
        if let Some((_, _, right, bottom)) = platform::primary_work_area() {
            let scale = p.window().scale_factor();
            let (w, h, m) = ((LOGICAL_SIZE.0 * scale) as i32, (LOGICAL_SIZE.1 * scale) as i32, (MARGIN * scale) as i32);
            p.window().set_position(slint::PhysicalPosition::new(right - w - m, bottom - h - m));
        }
    }
    s.current = Some(app);
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
