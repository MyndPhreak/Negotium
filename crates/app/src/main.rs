#![windows_subsystem = "windows"]

use negotium::bridge::Bridge;
use negotium::controls::Controls;
use negotium::platform::{self, ShellEvent};
use negotium::{MainWindow, NegotiumTray, worker};
use negotium_model::Metric;
use slint::ComponentHandle;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

fn warn(msg: &str) {
    negotium::log::Log::new(negotium::log::Log::default_path()).warn(msg);
}

/// Shows the main window from the tray or a second launch: un-hides, restores, focuses.
fn show_main(controls: &Controls, weak: &slint::Weak<MainWindow>) {
    controls.set_visible(true);
    if let Some(ui) = weak.upgrade() {
        platform::bring_to_front(&ui);
    }
}

fn make_tray(controls: &Arc<Controls>, weak: &slint::Weak<MainWindow>) -> Result<NegotiumTray, slint::PlatformError> {
    let tray = NegotiumTray::new()?;
    {
        let (controls, weak) = (controls.clone(), weak.clone());
        tray.on_open(move || show_main(&controls, &weak));
    }
    {
        let controls = controls.clone();
        tray.on_exit(move || {
            controls.request_stop();
            let _ = slint::quit_event_loop();
        });
    }
    tray.show()?;
    Ok(tray)
}

fn main() -> Result<(), slint::PlatformError> {
    // One instance per session. A second launch shows the running one (even from the tray) and
    // exits; a restart (as admin) waits for the old instance to finish quitting.
    let restarted = std::env::args().any(|a| a == "--restarted");
    let wait = if restarted { Duration::from_secs(10) } else { Duration::ZERO };
    let _instance = match platform::claim_instance(platform::INSTANCE_MUTEX, wait) {
        Some(lock) => lock,
        None => {
            if platform::activate_existing_instance(Duration::from_secs(5)) {
                return Ok(());
            }
            // The holder has no window: it is shutting down (or never got one). Take over once it
            // has gone, so a launch during an exit doesn't end with nothing running.
            match platform::claim_instance(platform::INSTANCE_MUTEX, Duration::from_secs(5)) {
                Some(lock) => lock,
                None => return Ok(()),
            }
        }
    };
    // Before the worker starts, so the collector and every action see the privilege.
    let elevated = negotium_actions::is_elevated();
    if elevated && let Err(e) = negotium_actions::enable_debug_privilege() {
        // Not fatal: actions on other users' processes then report access denied.
        warn(&format!("could not enable SeDebugPrivilege: {e:?}"));
    }
    // Software rendering unless SLINT_BACKEND says otherwise: with winit + FemtoVG (OpenGL) the
    // window sometimes took 30 s to 5+ min to appear, and software opens faster on every launch
    // measured (docs/benchmarks/README.md, "Renderer").
    if std::env::var_os("SLINT_BACKEND").is_none()
        && let Err(e) =
            slint::BackendSelector::new().backend_name("winit".into()).renderer_name("software".into()).select()
    {
        warn(&format!("software renderer unavailable, using Slint's default: {e}"));
    }
    let ui = MainWindow::new()?;
    if let Some((r, g, b)) = platform::read_accent() {
        ui.global::<negotium::Theme>().set_accent(slint::Color::from_rgb_u8(r, g, b));
    }
    let controls = Arc::new(Controls::default());
    let bridge = Bridge::install(&ui, controls.clone());

    // Worker -> UI thread through a latest-frame slot: one wakeup is posted only when the slot was
    // empty, and the UI takes the newest frame. The sink must never panic, so the Result is ignored.
    let weak = ui.as_weak();
    let sink_ui = weak.clone();
    let latest: Arc<worker::Latest<negotium::view::Frame>> = Default::default();
    let _worker = worker::spawn(controls.clone(), move |frame| {
        if !latest.put(frame) {
            return;
        }
        let (weak, slot) = (sink_ui.clone(), latest.clone());
        let posted = slint::invoke_from_event_loop(move || {
            if let (Some(ui), Some(frame)) = (weak.upgrade(), slot.take()) {
                negotium::bridge::with_bridge(|b| b.apply(&ui, frame));
            }
        });
        if posted.is_err() {
            latest.take(); // no event loop (shutting down): keep the next put posting
        }
    });
    negotium::bridge::set_current(bridge.clone());

    {
        let controls = controls.clone();
        let weak = weak.clone();
        ui.on_select_metric(move |i| {
            controls.set_metric(Metric::ALL[(i.max(0) as usize).min(Metric::ALL.len() - 1)]);
            if let Some(ui) = weak.upgrade() {
                ui.set_selected_metric(i);
            }
        });
    }
    // The list freezes (rows keep their index) while the pointer is over it or a row menu is open.
    {
        let b = bridge.clone();
        ui.on_list_hover(move |h| b.borrow_mut().set_hover(h));
    }
    {
        let b = bridge.clone();
        ui.on_menu_open(move |open| {
            if open {
                b.borrow_mut().set_menu_open(true);
            } else {
                // Unfreezing can drop rows; do it after the row's own pointer handler has returned.
                let b = b.clone();
                slint::Timer::single_shot(std::time::Duration::ZERO, move || b.borrow_mut().set_menu_open(false));
            }
        });
    }
    {
        let b = bridge.clone();
        ui.on_row_key_focus(move |id, focused| negotium::bridge::row_focus(&b, id, focused));
    }
    {
        let b = bridge.clone();
        ui.on_clear_key_focus(move || b.borrow_mut().clear_key_focus());
    }
    {
        let b = bridge.clone();
        ui.on_search_changed(move |text| b.borrow_mut().set_search(&text));
    }
    {
        let b = bridge.clone();
        ui.on_toggle_row(move |id| b.borrow_mut().toggle(id));
    }
    {
        let b = bridge.clone();
        ui.on_open_tile(move |id| b.borrow_mut().open_tile(id));
    }
    {
        let b = bridge.clone();
        ui.on_dismiss_banner(move || b.borrow_mut().dismiss_banner());
    }

    // Close hides to tray (spec 5.5); the worker slows to 2 s while hidden. With --exit-on-close
    // (benchmark use only) it quits instead, so a WM_CLOSE from the bench ends the process.
    let exit_on_close = std::env::args().any(|a| a == "--exit-on-close");
    {
        let controls = controls.clone();
        let b = bridge.clone();
        ui.window().on_close_requested(move || {
            if exit_on_close {
                controls.request_stop();
                let _ = slint::quit_event_loop();
                return slint::CloseRequestResponse::HideWindow;
            }
            controls.set_visible(false);
            // A hidden window cannot report pointer-leave, so release the hover freeze here.
            b.borrow_mut().set_hover(false);
            slint::CloseRequestResponse::HideWindow
        });
    }

    // Optional render-time statistics for the benchmark (NEGOTIUM_FRAME_STATS=<path>).
    let frame_stats_path = std::env::var_os("NEGOTIUM_FRAME_STATS");
    let frame_stats: std::rc::Rc<std::cell::RefCell<negotium::framestats::FrameStats>> = Default::default();
    if frame_stats_path.is_some() {
        let stats = frame_stats.clone();
        // The software renderer has no notifier: log it and carry on; the run then reports 0 frames.
        if let Err(e) = ui.window().set_rendering_notifier(move |state, _| match state {
            slint::RenderingState::BeforeRendering => stats.borrow_mut().begin(),
            slint::RenderingState::AfterRendering => stats.borrow_mut().end(),
            _ => {}
        }) {
            warn(&format!("frame stats unavailable with this renderer ({e}); use SLINT_BACKEND=winit-femtovg"));
        }
    }

    // Row actions. The UI thread only plans; every job runs on its own thread.
    let pending: std::rc::Rc<std::cell::RefCell<Option<negotium::dispatch::Job>>> = Default::default();
    let start_job = {
        let weak = weak.clone();
        move |job: negotium::dispatch::Job| {
            let weak = weak.clone();
            std::thread::spawn(move || {
                let text = negotium::dispatch::run(job, elevated);
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = weak.upgrade() {
                        if let Some(details) = text.strip_prefix("details:") {
                            ui.invoke_show_details(details.into());
                        } else if !text.is_empty() {
                            ui.set_toast(text.into());
                        }
                    }
                });
            });
        }
    };
    {
        let b = bridge.clone();
        let weak = weak.clone();
        let pending = pending.clone();
        let start_job = start_job.clone();
        ui.on_row_action(move |action, id| {
            // Clone out of the borrow and release it before doing anything else.
            let (target, procs) = {
                let b = b.borrow();
                (b.target(id), b.all_procs().to_vec())
            };
            let Some(target) = target else { return };
            match negotium::dispatch::plan(action.as_str(), &target, &procs) {
                negotium::dispatch::Plan::Run(job) => start_job(job),
                negotium::dispatch::Plan::Confirm(text, job) => {
                    *pending.borrow_mut() = Some(job);
                    if let Some(ui) = weak.upgrade() {
                        ui.invoke_show_confirm(text.into());
                    }
                }
                negotium::dispatch::Plan::Nothing => {}
            }
        });
    }
    {
        let weak = weak.clone();
        ui.on_confirm(move |ok| {
            if let Some(ui) = weak.upgrade() {
                ui.set_confirm_visible(false);
            }
            if let Some(job) = pending.borrow_mut().take()
                && ok
            {
                start_job(job);
            }
        });
    }
    {
        let weak = weak.clone();
        ui.on_close_details(move || {
            if let Some(ui) = weak.upgrade() {
                ui.set_details_visible(false);
            }
        });
    }
    {
        let weak = weak.clone();
        ui.on_dismiss_toast(move || {
            if let Some(ui) = weak.upgrade() {
                ui.set_toast("".into());
            }
        });
    }
    {
        let weak = weak.clone();
        ui.on_restart_admin(move || {
            if negotium_actions::restart_as_admin("--restarted").is_ok() {
                let _ = slint::quit_event_loop();
            } else if let Some(ui) = weak.upgrade() {
                ui.set_toast("Restart as admin was cancelled.".into());
            }
        });
    }

    ui.show()?;
    // The native HWND is created lazily once the event loop runs, so poll until it exists; then,
    // once: apply the chrome, hook the window (tray re-creation, activation by a second launch),
    // mark it for that second launch, and only now create the tray, after the window exists.
    let tray: Rc<RefCell<Option<NegotiumTray>>> = Default::default();
    let chrome_timer = Rc::new(slint::Timer::default());
    {
        let weak = weak.clone();
        let controls = controls.clone();
        let handle = Rc::downgrade(&chrome_timer);
        chrome_timer.start(slint::TimerMode::Repeated, Duration::from_millis(50), move || {
            let Some(ui) = weak.upgrade() else { return };
            let Some(hwnd) = platform::main_hwnd(&ui) else { return };
            platform::apply_window_chrome(&ui);
            let hooked = {
                let (tray, controls, weak) = (tray.clone(), controls.clone(), weak.clone());
                platform::install_window_hooks(hwnd, move |event| {
                    // The hook runs inside the window procedure: act from the event loop instead.
                    let (tray, controls, weak) = (tray.clone(), controls.clone(), weak.clone());
                    slint::Timer::single_shot(Duration::ZERO, move || match event {
                        // Explorer restarted (perhaps ended from this very list): add the icon again.
                        ShellEvent::TaskbarCreated => {
                            drop(tray.borrow_mut().take());
                            *tray.borrow_mut() = make_tray(&controls, &weak)
                                .map_err(|e| warn(&format!("tray icon could not be re-created: {e}")))
                                .ok();
                        }
                        ShellEvent::Activate => show_main(&controls, &weak),
                    });
                })
            };
            if !hooked {
                warn("main window subclass failed: the tray icon won't survive an Explorer restart");
            }
            platform::mark_main_window(hwnd);
            *tray.borrow_mut() = make_tray(&controls, &weak)
                .map_err(|e| warn(&format!("tray icon could not be created: {e}")))
                .ok();
            if let Some(t) = handle.upgrade() {
                t.stop();
            }
        });
    }
    let result = slint::run_event_loop_until_quit();
    // Shutting down: stop a new launch from handing its activation to this dying instance; it
    // then waits for the lock and starts fresh.
    if let Some(hwnd) = platform::main_hwnd(&ui) {
        platform::unmark_main_window(hwnd);
    }
    if let Some(path) = frame_stats_path {
        let _ = std::fs::write(path, frame_stats.borrow().summary_json());
    }
    result
}
