#![windows_subsystem = "windows"]

use negotium::bridge::Bridge;
use negotium::controls::Controls;
use negotium::platform::{self, ShellEvent};
use negotium::renderer;
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

/// Every quit goes through here. The window mark is removed first, so a launch made while this
/// instance shuts down never hands its activation to a dying window: it finds no window, waits
/// for the lock and starts fresh.
fn begin_exit(controls: &Controls, weak: &slint::Weak<MainWindow>) {
    if let Some(hwnd) = weak.upgrade().and_then(|ui| platform::main_hwnd(&ui)) {
        platform::unmark_main_window(hwnd);
    }
    controls.request_stop();
    let _ = slint::quit_event_loop();
}

fn make_tray(controls: &Arc<Controls>, weak: &slint::Weak<MainWindow>) -> Result<NegotiumTray, slint::PlatformError> {
    let tray = NegotiumTray::new()?;
    {
        let (controls, weak) = (controls.clone(), weak.clone());
        tray.on_open(move || show_main(&controls, &weak));
    }
    {
        let (controls, weak) = (controls.clone(), weak.clone());
        tray.on_exit(move || begin_exit(&controls, &weak));
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
    // GPU (FemtoVG) by default, software once a stall has been recorded, SLINT_BACKEND over both
    // (crate::renderer; docs/benchmarks/README.md, "Renderer").
    let relaunched = std::env::args_os().any(|a| a == renderer::FALLBACK_ARG);
    let decision =
        renderer::decide(std::env::var_os("SLINT_BACKEND").is_some(), renderer::fallback_recorded(), relaunched);
    let selector = slint::BackendSelector::new();
    let (selector, name) = match decision.renderer {
        // Selected now, while SLINT_BACKEND is still set: the selector reads it.
        renderer::Renderer::FromEnv => (selector, "SLINT_BACKEND"),
        renderer::Renderer::Gpu => (selector.backend_name("winit".into()).renderer_name("femtovg".into()), "femtovg"),
        renderer::Renderer::Software => {
            (selector.backend_name("winit".into()).renderer_name("software".into()), "software")
        }
    };
    if let Err(e) = selector.select() {
        warn(&format!("{name} renderer unavailable, using Slint's default: {e}"));
    }
    // The choice is made: keep SLINT_BACKEND (the user's, or the watchdog's winit-software) out of
    // the processes Negotium starts (Open file location, Run new task). No other thread runs yet.
    platform::remove_env_var("SLINT_BACKEND");
    // Armed before the window is created: creating it already probes OpenGL.
    let watchdog = decision
        .watchdog
        .then(|| renderer::Watchdog::arm(renderer::WATCHDOG_TIMEOUT, renderer::relaunch_in_software).0);
    // Whether the tray icon may be created: not while the watchdog could still replace this
    // instance (it terminates without cleanup and would orphan the icon).
    let tray_allowed = {
        let watchdog = watchdog.clone();
        move || watchdog.as_ref().is_none_or(|w| w.resolved())
    };
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
    // Kept across worker restarts; the UI thread reads it for the history scrub.
    let history: worker::SharedHistory = Default::default();
    bridge.borrow_mut().set_history(history.clone());
    let _worker = worker::spawn(controls.clone(), history, move |frame| {
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
    {
        let (weak, b) = (weak.clone(), bridge.clone());
        ui.on_select_page(move |i| {
            if let Some(ui) = weak.upgrade() {
                b.borrow_mut().set_page(&ui, negotium::controls::Page::from_index(i));
            }
        });
    }
    {
        let (controls, weak) = (controls.clone(), weak.clone());
        ui.on_select_device(move |id| {
            controls.select_device(&id);
            if let Some(ui) = weak.upgrade() {
                ui.set_perf_selected(id);
            }
        });
    }
    {
        let (b, weak) = (bridge.clone(), weak.clone());
        ui.on_scrub(move |f| {
            if let Some(ui) = weak.upgrade() {
                b.borrow_mut().scrub(&ui, f);
            }
        });
    }
    {
        let (controls, weak) = (controls.clone(), weak.clone());
        ui.on_select_range(move |i| {
            let range = negotium::perf::Range::from_index(i);
            controls.set_range(range);
            if let Some(ui) = weak.upgrade() {
                ui.set_perf_range(range.index());
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
    // Keyboard navigation through the list (ruling R4): it stays frozen while it has focus.
    {
        let b = bridge.clone();
        ui.on_list_focus(move |focused| negotium::bridge::list_focus(&b, focused));
    }
    {
        let (b, weak) = (bridge.clone(), weak.clone());
        ui.on_list_key(move |text| weak.upgrade().is_some_and(|ui| negotium::bridge::handle_list_key(&b, &ui, &text)));
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
        let close_weak = weak.clone();
        ui.window().on_close_requested(move || {
            if exit_on_close {
                begin_exit(&controls, &close_weak);
                return slint::CloseRequestResponse::HideWindow;
            }
            controls.set_visible(false);
            // A hidden window cannot report pointer-leave, so release the hover freeze here.
            b.borrow_mut().set_hover(false);
            slint::CloseRequestResponse::HideWindow
        });
    }

    // The rendering notifier reports the first presented frame to the watchdog and, with
    // NEGOTIUM_FRAME_STATS=<path>, times every frame for the bench.
    let frame_stats_path = std::env::var_os("NEGOTIUM_FRAME_STATS");
    let frame_stats: Rc<RefCell<negotium::framestats::FrameStats>> = Default::default();
    let notified = {
        let (stats, timed) = (frame_stats.clone(), frame_stats_path.is_some());
        let watchdog = watchdog.clone();
        let mut first = true;
        // Debug builds only: NEGOTIUM_SIMULATE_STALL=1 blocks the UI thread before the first frame
        // of a guarded launch, to watch the watchdog relaunch into software.
        #[cfg(debug_assertions)]
        let stall = watchdog.is_some() && std::env::var_os("NEGOTIUM_SIMULATE_STALL").is_some_and(|v| v == "1");
        #[cfg(not(debug_assertions))]
        let stall = false;
        ui.window().set_rendering_notifier(move |state, _| match state {
            slint::RenderingState::BeforeRendering => {
                if stall && first {
                    loop {
                        std::thread::sleep(Duration::from_secs(60));
                    }
                }
                if timed {
                    stats.borrow_mut().begin();
                }
            }
            slint::RenderingState::AfterRendering => {
                if timed {
                    stats.borrow_mut().end();
                }
                // AfterRendering comes before the frame is presented (swapped); the timer runs
                // once the event loop is back, after the present.
                if std::mem::take(&mut first)
                    && let Some(w) = watchdog.clone()
                {
                    slint::Timer::single_shot(Duration::ZERO, move || {
                        w.presented();
                    });
                }
            }
            _ => {}
        })
    };
    if let Err(e) = notified {
        // The software renderer has no notifier (FemtoVG fell back to it in-process, or software
        // was chosen): there is no GL to stall, so stand the watchdog down.
        if let Some(w) = &watchdog {
            w.stand_down();
        }
        if frame_stats_path.is_some() {
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
        let controls = controls.clone();
        ui.on_restart_admin(move || {
            if negotium_actions::restart_as_admin("--restarted").is_ok() {
                begin_exit(&controls, &weak);
            } else if let Some(ui) = weak.upgrade() {
                ui.set_toast("Restart as admin was cancelled.".into());
            }
        });
    }

    ui.show()?;
    // The native HWND is created lazily once the event loop runs, so poll until it exists; then,
    // once: apply the chrome, hook the window (tray re-creation, activation by a second launch)
    // and mark it for that second launch. Every tick is also the renderer watchdog's heartbeat
    // (the UI thread is alive), and the tray icon waits until the watchdog has resolved.
    let tray: Rc<RefCell<Option<NegotiumTray>>> = Default::default();
    let chrome_timer = Rc::new(slint::Timer::default());
    {
        let weak = weak.clone();
        let controls = controls.clone();
        let handle = Rc::downgrade(&chrome_timer);
        let mut chromed = false;
        let watchdog = watchdog.clone();
        chrome_timer.start(slint::TimerMode::Repeated, Duration::from_millis(50), move || {
            if let Some(w) = &watchdog {
                w.heartbeat();
            }
            let Some(ui) = weak.upgrade() else { return };
            if !chromed {
                let Some(hwnd) = platform::main_hwnd(&ui) else { return };
                platform::apply_window_chrome(&ui);
                let hooked = {
                    let (tray, controls, weak) = (tray.clone(), controls.clone(), weak.clone());
                    let tray_allowed = tray_allowed.clone();
                    platform::install_window_hooks(hwnd, move |event| {
                        // The hook runs inside the window procedure: act from the event loop instead.
                        let (tray, controls, weak) = (tray.clone(), controls.clone(), weak.clone());
                        let tray_allowed = tray_allowed.clone();
                        slint::Timer::single_shot(Duration::ZERO, move || match event {
                            // Explorer restarted (perhaps ended from this very list): add the icon again.
                            // Before the watchdog resolves, the first creation below adds it.
                            ShellEvent::TaskbarCreated if tray_allowed() => {
                                drop(tray.borrow_mut().take());
                                *tray.borrow_mut() = make_tray(&controls, &weak)
                                    .map_err(|e| warn(&format!("tray icon could not be re-created: {e}")))
                                    .ok();
                            }
                            ShellEvent::TaskbarCreated => {}
                            ShellEvent::Activate => show_main(&controls, &weak),
                        });
                    })
                };
                if !hooked {
                    warn("main window subclass failed: the tray icon won't survive an Explorer restart");
                }
                platform::mark_main_window(hwnd);
                chromed = true;
            }
            if !tray_allowed() {
                return;
            }
            *tray.borrow_mut() = make_tray(&controls, &weak)
                .map_err(|e| warn(&format!("tray icon could not be created: {e}")))
                .ok();
            if let Some(t) = handle.upgrade() {
                t.stop();
            }
        });
    }
    let result = slint::run_event_loop_until_quit();
    if let Some(path) = frame_stats_path {
        let _ = std::fs::write(path, frame_stats.borrow().summary_json());
    }
    result
}
