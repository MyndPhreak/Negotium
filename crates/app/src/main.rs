#![windows_subsystem = "windows"]

use negotium::bridge::Bridge;
use negotium::controls::Controls;
use negotium::platform::{self, ShellEvent};
use negotium::renderer;
use negotium::settings;
use negotium::smooth::OnScreen;
use negotium::{MainWindow, NegotiumTray, worker};
use slint::ComponentHandle;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

fn warn(msg: &str) {
    negotium::log::Log::new(negotium::log::Log::default_path()).warn(msg);
}

/// Shows the main window from the tray or a second launch: un-hides, restores, focuses.
fn show_main(on_screen: &OnScreen, weak: &slint::Weak<MainWindow>) {
    if let Some(ui) = weak.upgrade() {
        platform::bring_to_front(&ui);
    }
    // Visible again (the worker's pace); graphs scroll unless the window is still minimised.
    on_screen.show();
}

thread_local! {
    /// A restart as admin is waiting on its UAC prompt (UI thread only).
    static RESTART_PENDING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// How long an exit waits for pending settings writes (the file, the GPU marker) to land.
const EXIT_FLUSH: Duration = Duration::from_secs(1);

/// Every quit goes through here. The window mark is removed first, so a launch made while this
/// instance shuts down never hands its activation to a dying window: it finds no window, waits
/// for the lock and starts fresh. Pending settings writes land first (bounded), so the next
/// instance (a restart as admin) reads the new state.
fn begin_exit(controls: &Controls, weak: &slint::Weak<MainWindow>, saver: &settings::Saver) {
    if let Some(hwnd) = weak.upgrade().and_then(|ui| platform::main_hwnd(&ui)) {
        platform::unmark_main_window(hwnd);
    }
    if RESTART_PENDING.with(|p| p.get()) {
        warn("exiting while the UAC prompt for 'Restart as admin' is open; an approved elevated instance may still start");
    }
    controls.request_stop();
    if !saver.flush(EXIT_FLUSH) {
        warn("settings writes were still running at exit; they may not have landed");
    }
    let _ = slint::quit_event_loop();
}

fn make_tray(
    controls: &Arc<Controls>,
    on_screen: &Rc<OnScreen>,
    weak: &slint::Weak<MainWindow>,
    saver: &settings::Saver,
) -> Result<NegotiumTray, slint::PlatformError> {
    let tray = NegotiumTray::new()?;
    {
        let (on_screen, weak) = (on_screen.clone(), weak.clone());
        tray.on_open(move || show_main(&on_screen, &weak));
    }
    {
        let (controls, weak, saver) = (controls.clone(), weak.clone(), saver.clone());
        tray.on_exit(move || begin_exit(&controls, &weak, &saver));
    }
    tray.show()?;
    // A new tray (first, or after Explorer restarted) shows the current graph at once, drawn for
    // the taskbar's DPI as it is now.
    negotium::tray::attach(&tray, platform::tray_icon_px());
    Ok(tray)
}

fn main() -> Result<(), slint::PlatformError> {
    // One instance per session. A second launch shows the running one (even from the tray) and
    // exits; a restart (as admin) waits for the old instance to finish quitting.
    let restarted = std::env::args_os().any(|a| a == "--restarted");
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
    // The registry helpers refuse writes outside the test sandbox until the real app unlocks them,
    // so tests (which never run `main`) can't reach the user's startup entries.
    negotium_actions::startup::registry::allow_system_writes();
    // Before the worker starts, so the collector and every action see the privilege.
    let elevated = negotium_actions::is_elevated();
    if elevated && let Err(e) = negotium_actions::enable_debug_privilege() {
        // Not fatal: actions on other users' processes then report access denied.
        warn(&format!("could not enable SeDebugPrivilege: {e:?}"));
    }
    // GPU (FemtoVG) by default, software once a stall has been recorded, SLINT_BACKEND over both
    // (crate::renderer; docs/benchmarks/README.md, "Renderer").
    let relaunched = std::env::args_os().any(|a| a == renderer::FALLBACK_ARG);
    // Read once, before the window shows (allowed on the UI thread).
    let settings_path = settings::default_path();
    let (settings, loaded) = settings::load(&settings_path);
    match &loaded {
        settings::Loaded::Recovered(why) => {
            warn(&format!("settings.json was unusable and was set aside as settings.json.bad: {why}"));
        }
        settings::Loaded::Partial(fields) => warn(&format!(
            "settings.json had invalid fields ({}); they use their defaults and the original is in settings.json.bad",
            fields.join(", ")
        )),
        settings::Loaded::PartialNotKept(fields, why) => warn(&format!(
            "settings.json had invalid fields ({}); they use their defaults, and the original could not be copied to settings.json.bad: {why}",
            fields.join(", ")
        )),
        settings::Loaded::Unreadable(why) => warn(&format!("settings.json could not be read, using defaults: {why}")),
        settings::Loaded::Defaults | settings::Loaded::File => {}
    }
    let fallback_recorded = renderer::fallback_recorded();
    let decision = renderer::decide(
        std::env::var_os("SLINT_BACKEND").is_some(),
        settings.gpu_rendering,
        fallback_recorded,
        relaunched,
    );
    let selector = slint::BackendSelector::new();
    let (selector, name) = match decision.renderer {
        // Selected now, while SLINT_BACKEND is still set: the selector reads it.
        renderer::Renderer::FromEnv => (selector, "SLINT_BACKEND"),
        renderer::Renderer::Gpu => (selector.backend_name("winit".into()).renderer_name("femtovg".into()), "femtovg"),
        renderer::Renderer::Software => {
            (selector.backend_name("winit".into()).renderer_name("software".into()), "software")
        }
    };
    // The renderer this instance draws with, for the Settings page. A failed selection leaves
    // Slint's default, which is not known to be the GPU.
    let running = match selector.select() {
        Ok(()) => decision.renderer,
        Err(e) => {
            warn(&format!("{name} renderer unavailable, using Slint's default: {e}"));
            match decision.renderer {
                renderer::Renderer::FromEnv => renderer::Renderer::FromEnv,
                _ => renderer::Renderer::Software,
            }
        }
    };
    // The choice is made: keep the user's SLINT_BACKEND out of the processes Negotium starts (Open
    // file location, Run new task, a restart). No other thread runs yet.
    platform::remove_env_var("SLINT_BACKEND");
    // Armed before the window is created: creating it already probes OpenGL.
    // The relaunch (exe path, arguments) is resolved now, so the stall path only spawns.
    let watchdog = decision.watchdog.then(|| {
        let relaunch = renderer::SoftwareRelaunch::resolve();
        renderer::Watchdog::arm(renderer::WATCHDOG_TIMEOUT, move || relaunch.run()).0
    });
    // Whether the tray icon may be created: not while the watchdog could still replace this
    // instance (it terminates without cleanup and would orphan the icon).
    let tray_allowed = {
        let watchdog = watchdog.clone();
        move || watchdog.as_ref().is_none_or(|w| w.resolved())
    };
    let ui = MainWindow::new()?;
    platform::apply_accent(&ui);
    // Settings in effect for this run; the Settings page changes them and saves to `settings_path`.
    let settings = Rc::new(RefCell::new(settings));
    // Copied out: no borrow is held while the theme is applied (window calls).
    let initial = settings.borrow().clone();
    negotium::settingspage::apply_settings(&ui, &initial);
    let controls = Arc::new(Controls::default());
    controls.set_refresh_ms(initial.refresh_ms);
    // The saved sort and grouping; a metric sort starts with its meter selected, like a click would.
    controls.set_sort(initial.sort());
    controls.set_grouped(initial.group_by_type);
    if let negotium_model::SortColumn::Metric(m) = initial.sort().column {
        controls.set_metric(m);
        ui.set_selected_metric(m as i32);
    }
    // The tray icon's CPU graph (crate::tray): drawn on this thread from the worker's samples; its
    // size and colours are read here (window setup), the size again whenever a tray is made and the
    // colours on a theme change. The shell is left alone while the worker finds the taskbar hung (PF2).
    {
        let controls = controls.clone();
        negotium::tray::install(
            negotium::tray::TrayView::new(
                initial.tray_cpu,
                platform::tray_icon_px(),
                negotium::tray::colors_now(platform::read_accent(), platform::taskbar_is_light()),
            ),
            move || controls.shell_hung(),
        );
    }
    controls.set_tray_cpu(initial.tray_cpu);
    // Shared with every exit path, which waits for its writes (`begin_exit`).
    let saver = settings::Saver::new(settings_path);
    negotium::settingspage::SettingsWiring {
        settings: settings.clone(),
        saver: saver.clone(),
        controls: controls.clone(),
        gpu_marker: renderer::fallback_path(),
        fallback_recorded,
        running,
        tray_cpu_changed: Rc::new(negotium::tray::set_enabled),
    }
    .install(&ui);
    let bridge = Bridge::install(&ui, controls.clone());

    // Worker -> UI thread through a latest-frame slot: one wakeup is posted only when the slot was
    // empty, and the UI takes the newest frame. The sink must never panic, so the Result is ignored.
    let weak = ui.as_weak();
    let sink_ui = weak.clone();
    let latest: Arc<worker::Latest<negotium::view::Frame>> = Default::default();
    // Kept across worker restarts; the UI thread reads it for the history scrub.
    let history: worker::SharedHistory = Default::default();
    bridge.borrow_mut().set_history(history.clone());
    // Collector time counts from here, for the worker and for the bridge's scrolling view.
    let epoch = std::time::Instant::now();
    bridge.borrow_mut().set_epoch(epoch);
    // Worker -> tray, like frames: one wakeup per empty slot, the newest sample wins.
    let tray_sink: worker::TraySink = {
        let slot: Arc<worker::Latest<negotium::tray::TraySample>> = Default::default();
        Arc::new(move |sample| {
            if !slot.put(sample) {
                return;
            }
            let posted = slot.clone();
            let sent = slint::invoke_from_event_loop(move || {
                if let Some(s) = posted.take() {
                    negotium::tray::on_sample(s);
                }
            });
            if sent.is_err() {
                slot.take(); // no event loop (shutting down): keep the next put posting
            }
        })
    };
    let _worker = worker::spawn(controls.clone(), history, epoch, Some(tray_sink), move |frame| {
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

    // Startup page: listing calls the registry, the file system, COM and WinRT, so it runs on an
    // action thread and the result comes back through the bridge (negotium::startup). The list is
    // read again when the page opens, on Refresh and after every toggle, so it shows the real state.
    let load_startup = {
        let weak = weak.clone();
        move || {
            if weak.upgrade().is_some_and(|ui| negotium::startup::begin_list(&ui)) {
                negotium::startup::spawn_job(
                    weak.clone(),
                    elevated,
                    || negotium::startup::list_job(&negotium_actions::startup::StartupRoots::system()),
                    || {},
                );
            }
        }
    };
    // Every page change: a rail click, and the page a restart comes back on.
    let show_page = {
        let (weak, b, load_startup) = (weak.clone(), bridge.clone(), load_startup.clone());
        move |page: negotium::controls::Page| {
            if let Some(ui) = weak.upgrade() {
                b.borrow_mut().set_page(&ui, page);
            }
            // Outside the bridge borrow.
            if page == negotium::controls::Page::Startup {
                load_startup();
            }
        }
    };
    {
        let show_page = show_page.clone();
        ui.on_select_page(move |i| show_page(negotium::controls::Page::from_index(i)));
    }
    // A restart (as admin) comes back on the page it left (spec §6); anything else opens on the
    // Overview.
    if let Some(page) = negotium::controls::page_from_os_args(std::env::args_os()) {
        show_page(page);
    }
    ui.on_refresh_startup(load_startup.clone());
    {
        let (b, weak, load_startup) = (bridge.clone(), weak.clone(), load_startup.clone());
        ui.on_toggle_startup(move |id, on| {
            let Some(ui) = weak.upgrade() else { return };
            // Refused while a list or toggle runs (PF4); the switch then keeps the listed state.
            let Some(entry) = negotium::startup::begin_toggle(&b.borrow(), &ui, id, on) else { return };
            negotium::startup::spawn_job(
                weak.clone(),
                elevated,
                move || {
                    let roots = negotium_actions::startup::StartupRoots::system();
                    negotium::startup::toggle_job(&roots, &entry, on, elevated)
                },
                // A panicked toggle may have changed the entry: read the real state again.
                load_startup.clone(),
            );
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
        let (b, weak) = (bridge.clone(), weak.clone());
        ui.on_scroll_follow(move || {
            if let Some(ui) = weak.upgrade() {
                b.borrow_mut().follow_scrub(&ui);
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

    // Close hides to tray (spec 5.5) while "Keep running in tray" is on; the worker then ticks every
    // 2 s (or slower, Controls::interval). Otherwise, or with --exit-on-close (benchmark use only),
    // it quits, so a WM_CLOSE from the bench ends the process.
    let exit_on_close = std::env::args_os().any(|a| a == "--exit-on-close");
    // Whether the window is on screen: graphs scroll only then (negotium::smooth). Every path that
    // hides, shows, minimises or restores it goes through this.
    let on_screen = OnScreen::new(controls.clone(), &ui);
    {
        let (controls, b, close_weak, settings) = (controls.clone(), bridge.clone(), weak.clone(), settings.clone());
        let (saver, on_screen) = (saver.clone(), on_screen.clone());
        ui.window().on_close_requested(move || {
            // Copied out: no borrow is held across `begin_exit` and its flush.
            let keep_in_tray = settings.borrow().keep_in_tray;
            match settings::close_action(keep_in_tray, exit_on_close) {
                settings::CloseAction::Exit => begin_exit(&controls, &close_weak, &saver),
                settings::CloseAction::HideToTray => {
                    // The worker slows to the tray pace and graphs stop scrolling.
                    on_screen.hide();
                    // A hidden window cannot report pointer-leave, so release the hover freeze here.
                    b.borrow_mut().set_hover(false);
                }
            }
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
        let (weak, controls, saver) = (weak.clone(), controls.clone(), saver.clone());
        ui.on_restart_admin(move || {
            if RESTART_PENDING.with(|p| p.replace(true)) {
                return; // a UAC prompt is already up
            }
            let args = negotium::controls::restart_args(controls.page());
            let (weak, controls, saver) = (weak.clone(), controls.clone(), saver.clone());
            // The UAC prompt's owner, so it comes to the front instead of flashing in the taskbar.
            let owner = weak.upgrade().and_then(|ui| platform::main_hwnd(&ui)).map_or(0, |h| h.0 as isize);
            // ShellExecuteEx returns only once the UAC prompt is answered: wait off the UI thread.
            // If the user exits Negotium (tray, Alt+F4 with the tray off) while the prompt is open,
            // `begin_exit` still runs and the process ends, yet the prompt belongs to ShellExecute's
            // own thread: an approval may still start the elevated instance, which waits for the
            // single-instance lock (`--restarted`) and then runs. `begin_exit` logs that case.
            std::thread::spawn(move || {
                let failure = negotium_actions::restart_as_admin(&args, owner).err();
                // Fails only once the event loop is gone (already quitting): nothing left to do.
                let _ = slint::invoke_from_event_loop(move || {
                    RESTART_PENDING.with(|p| p.set(false));
                    match failure {
                        None => begin_exit(&controls, &weak, &saver),
                        Some(e) => {
                            if !matches!(e, negotium_actions::ActionError::Cancelled) {
                                warn(&format!("restart as admin failed: {e}"));
                            }
                            if let Some(ui) = weak.upgrade() {
                                ui.set_toast(negotium::controls::restart_failure_text(&e).into());
                            }
                        }
                    }
                });
            });
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
                    let (tray, controls, weak, on_screen) = (tray.clone(), controls.clone(), weak.clone(), on_screen.clone());
                    let (tray_allowed, settings, saver) = (tray_allowed.clone(), settings.clone(), saver.clone());
                    platform::install_window_hooks(hwnd, move |event| {
                        if let ShellEvent::Minimised(m) = event {
                            on_screen.on_size(m);
                            return;
                        }
                        // The hook runs inside the window procedure: act from the event loop instead.
                        let (tray, controls, weak, on_screen) = (tray.clone(), controls.clone(), weak.clone(), on_screen.clone());
                        let (tray_allowed, settings, saver) = (tray_allowed.clone(), settings.clone(), saver.clone());
                        slint::Timer::single_shot(Duration::ZERO, move || match event {
                            // Explorer restarted (perhaps ended from this very list): add the icon again.
                            // Before the watchdog resolves, the first creation below adds it.
                            ShellEvent::TaskbarCreated if tray_allowed() => {
                                drop(tray.borrow_mut().take());
                                *tray.borrow_mut() = make_tray(&controls, &on_screen, &weak, &saver)
                                    .map_err(|e| warn(&format!("tray icon could not be re-created: {e}")))
                                    .ok();
                            }
                            ShellEvent::TaskbarCreated => {}
                            ShellEvent::Activate => show_main(&on_screen, &weak),
                            ShellEvent::Minimised(_) => {} // handled above
                            ShellEvent::SettingsChanged => {
                                if let Some(ui) = weak.upgrade() {
                                    let theme = settings.borrow().theme;
                                    platform::on_settings_changed(&ui, theme);
                                    // The taskbar's mode or the accent changed: redraw the tray graph.
                                    negotium::tray::set_colors(negotium::tray::colors_now(
                                        platform::read_accent(),
                                        platform::taskbar_is_light(),
                                    ));
                                }
                            }
                        });
                    })
                };
                if !hooked {
                    warn("main window subclass failed: the tray icon won't survive an Explorer restart");
                }
                // A launch that starts minimised (a shortcut set to "Minimized"), or a window that
                // first appeared after a watchdog stand-down, was minimised before the hooks could
                // hear WM_SIZE: read it now.
                on_screen.minimise(platform::is_minimised(hwnd));
                on_screen.apply();
                platform::mark_main_window(hwnd);
                chromed = true;
            }
            if !tray_allowed() {
                return;
            }
            *tray.borrow_mut() = make_tray(&controls, &on_screen, &weak, &saver)
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
