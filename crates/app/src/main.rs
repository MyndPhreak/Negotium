#![windows_subsystem = "windows"]

use negotium::bridge::Bridge;
use negotium::controls::Controls;
use negotium::platform::{self, ShellEvent};
use negotium::renderer;
use negotium::settings;
use negotium::settingspage::Integrations;
use negotium::smooth::OnScreen;
use negotium::{MainWindow, NegotiumTray, worker};
use slint::{ComponentHandle, Model};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct ActiveAffinityTarget {
    name: String,
    /// The image name a remembered rule is saved under (view::Target::process_name).
    process_name: String,
    procs: Vec<(u32, i64)>,
}

struct ActivePriorityTarget {
    name: String,
    /// The image name a remembered rule is saved under (view::Target::process_name).
    process_name: String,
    procs: Vec<(u32, i64)>,
}

fn warn(msg: &str) {
    negotium::log::Log::new(negotium::log::Log::default_path()).warn(msg);
}

thread_local! {
    /// A restart as admin is waiting on its UAC prompt (UI thread only).
    static RESTART_PENDING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// The active settings on the UI thread, accessible for exit and geometry sync.
    static CURRENT_SETTINGS: RefCell<Option<Rc<RefCell<settings::Settings>>>> = const { RefCell::new(None) };
    /// A toast waiting for the main window to be shown (the picker shortcut was taken at start-up).
    static PENDING_TOAST: RefCell<Option<&'static str>> = const { RefCell::new(None) };
}

fn save_window_geometry(geo: settings::WindowGeometry, saver: &settings::Saver) {
    CURRENT_SETTINGS.with(|s| {
        if let Some(s) = s.borrow().as_ref() {
            s.borrow_mut().window_geometry = Some(geo);
            drop(saver.save(s.borrow().clone()));
        }
    });
}

/// Shows the main window from the tray or a second launch: un-hides, restores, focuses.
fn show_main(on_screen: &OnScreen, weak: &slint::Weak<MainWindow>) {
    if let Some(ui) = weak.upgrade() {
        if let Some(toast) = PENDING_TOAST.with(|t| t.borrow_mut().take()) {
            ui.set_toast(toast.into());
        }
        platform::bring_to_front(&ui);
        let maximized = CURRENT_SETTINGS.with(|s| {
            s.borrow().as_ref().and_then(|s| s.borrow().window_geometry).is_some_and(|g| g.maximized)
        });
        if maximized && let Some(hwnd) = platform::main_hwnd(&ui) {
            unsafe {
                let _ = windows::Win32::UI::WindowsAndMessaging::ShowWindow(hwnd, windows::Win32::UI::WindowsAndMessaging::SW_MAXIMIZE);
            }
        }
    }
    // Visible again (the worker's pace); graphs scroll unless the window is still minimised.
    on_screen.show();
}

/// How long an exit waits for pending settings writes (the file, the GPU marker) to land.
const EXIT_FLUSH: Duration = Duration::from_secs(1);

/// How long an exit waits for the worker to stop, which undoes the moderator's restraints.
const WORKER_EXIT_WAIT: Duration = Duration::from_secs(2);

/// How long `--exit-running` (the installer) waits for running instances to finish exiting.
const EXIT_RUNNING_WAIT: Duration = Duration::from_secs(10);

/// Every quit goes through here. The window mark is removed first, so a launch made while this
/// instance shuts down never hands its activation to a dying window: it finds no window, waits
/// for the lock and starts fresh. Pending settings writes land first (bounded), so the next
/// instance (a restart as admin) reads the new state.
fn begin_exit(controls: &Controls, weak: &slint::Weak<MainWindow>, saver: &settings::Saver) {
    if let Some(hwnd) = weak.upgrade().and_then(|ui| platform::main_hwnd(&ui)) {
        if let Some(geo) = platform::read_window_geometry(hwnd) {
            save_window_geometry(geo, saver);
        }
        platform::unmark_main_window(hwnd);
    }
    if RESTART_PENDING.with(|p| p.get()) {
        warn("exiting while the UAC prompt for 'Restart as admin' is open; an approved elevated instance may still start");
    }
    worker::stop_worker(controls, negotium_collector::netetw::stop_session);
    if !saver.flush(EXIT_FLUSH) {
        warn("settings writes were still running at exit; they may not have landed");
    }
    let _ = slint::quit_event_loop();
}

/// What a new tray icon needs besides the window: the settings writer (exit) and the Task
/// Manager handler with whether to offer it (the installed copy's redirect is on).
struct TrayWiring {
    saver: settings::Saver,
    integrations: Rc<Integrations>,
    taskmgr_shown: Rc<Cell<bool>>,
    settings: Rc<RefCell<settings::Settings>>,
    /// Whether this process runs as admin (the picker's admin checks and Explorer restart).
    elevated: bool,
}

fn make_tray(
    controls: &Arc<Controls>,
    on_screen: &Rc<OnScreen>,
    weak: &slint::Weak<MainWindow>,
    wiring: &TrayWiring,
) -> Result<NegotiumTray, slint::PlatformError> {
    let saver = &wiring.saver;
    let tray = NegotiumTray::new()?;
    {
        let (on_screen, weak) = (on_screen.clone(), weak.clone());
        tray.on_open(move || show_main(&on_screen, &weak));
    }
    {
        let ctx = negotium::picker::PickerCtx { elevated: wiring.elevated };
        tray.on_pick_window(move || negotium::picker::open(&ctx));
    }
    tray.set_taskmgr_shown(wiring.taskmgr_shown.get());
    {
        let integrations = wiring.integrations.clone();
        tray.on_open_task_manager(move || integrations.open_task_manager());
    }
    tray.set_aequilibrium_enabled(wiring.settings.borrow().moderator.aequilibrium_enabled);
    {
        let (controls, weak, saver, settings) = (controls.clone(), weak.clone(), saver.clone(), wiring.settings.clone());
        let tray_weak = tray.as_weak();
        tray.on_toggle_aequilibrium(move || {
            let new_val = !settings.borrow().moderator.aequilibrium_enabled;
            settings.borrow_mut().moderator.aequilibrium_enabled = new_val;
            controls.set_moderator_config(settings.borrow().moderator.clone());
            drop(saver.save(settings.borrow().clone()));
            if let Some(t) = tray_weak.upgrade() {
                t.set_aequilibrium_enabled(new_val);
            }
            if let Some(ui) = weak.upgrade() {
                ui.set_settings_aequilibrium_enabled(new_val);
            }
        });
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
    // Elevated, DLLs loaded by name come only from System32 and the install dir, never the
    // current directory or a user-controlled PATH folder (platform::restrict_dll_search); first,
    // before anything can load a library.
    if platform::restricts_dll_search(negotium::appdata::elevated())
        && let Err(e) = platform::restrict_dll_search()
    {
        warn(&format!("couldn't restrict the DLL search path: {e}"));
    }
    // Elevated (admin mode), nothing the user's environment says may choose what this process
    // loads or writes: every SLINT_* (the backend, font files) and NEGOTIUM_* variable is removed
    // first, before Slint or any other thread can read one (platform::untrusted_env). SystemRoot,
    // windir, SystemDrive and PATH are set from Windows itself (platform::trusted_env), so neither
    // this process, nor a child it starts (Explorer, Task Manager, msiexec, an elevated re-run),
    // nor COM expanding `%SystemRoot%` paths reaches a folder the user planted.
    if negotium::appdata::elevated() {
        for name in platform::untrusted_env(std::env::vars_os().map(|(name, _)| name)) {
            platform::remove_env_var(&name.to_string_lossy());
        }
        let windows = std::path::PathBuf::from(negotium_collector::windows_dir());
        for (name, value) in platform::trusted_env(&windows) {
            platform::set_env_var(name, &value);
        }
    }
    // Negotium's own arguments: started by the Ctrl+Shift+Esc redirect, Windows appends Task
    // Manager's command line, which is ignored (a normal launch).
    let args = negotium::integrate::own_args(std::env::args_os().skip(1));
    // `--exit-running` (the installer, before it replaces or removes the files): every running
    // instance of this exe in this session quits as from the tray's Exit; exit code 0 once they
    // all have, 1 if any is still running after the wait (Windows Installer then handles the file
    // in use). No window, no lock.
    if args.len() == 1 && args[0] == negotium::integrate::EXIT_RUNNING {
        let exe = std::env::current_exe().unwrap_or_default();
        let left = platform::exit_instances_of(&exe, EXIT_RUNNING_WAIT);
        if left > 0 {
            warn(&format!("{left} running Negotium didn't exit for the installer within {} s", EXIT_RUNNING_WAIT.as_secs()));
        }
        std::process::exit(i32::from(left > 0));
    }
    // `--relaunch` (the installer, once an update is installed; msiexec runs it elevated): start
    // the new version as the user gets it (the admin task, or a plain unelevated start through
    // the shell) and exit. Refused unless this is the installed exe. No window, no lock.
    if args.len() == 1 && args[0] == negotium::integrate::RELAUNCH {
        let mut log = negotium::log::Log::new(negotium::log::Log::default_path());
        let sid = negotium_actions::integration::current_user_sid().unwrap_or_default();
        let code = negotium::integrate::relaunch_command(
            &negotium::integrate::System::new(),
            sid,
            negotium_actions::integration::is_admin_member(),
            &|exe| platform::shell_start(exe).map_err(negotium::integrate::IntegrateError::Failed),
            &mut |line| log.warn(line),
        );
        std::process::exit(code);
    }
    // `--integrate <action>` (the installer and the Settings switches): no window, no renderer, no
    // instance lock, no settings; the exit code says whether it worked, a failure is logged. Never
    // for a start by the admin task (`--from-task` first): its arguments are anyone's, so it is a
    // normal launch (`integrate::requested`).
    if negotium::integrate::requested(&args) {
        // The registry guard then allows exactly the two integration values (and the startup
        // state keys, which this path never writes). The task is named by this process's user:
        // an administrator's SID is the same before and after the UAC prompt.
        negotium_actions::startup::registry::allow_system_writes();
        let mut log = negotium::log::Log::new(negotium::log::Log::default_path());
        let sid = negotium_actions::integration::current_user_sid().unwrap_or_else(|e| {
            log.warn(&format!("integrate: couldn't read this user's SID: {e}"));
            String::new()
        });
        let code = match negotium::integrate::elevate_request(&args) {
            // `--elevate --integrate …` (the installer's admin-mode step, as the installing user):
            // run here when already elevated, else in an elevated copy (one UAC prompt).
            Some(rest) => {
                let run_here = |rest: &[std::ffi::OsString]| {
                    let mut log = negotium::log::Log::new(negotium::log::Log::default_path());
                    negotium::integrate::command(rest, &negotium::integrate::System::new(), sid.clone(), &mut |line| {
                        log.warn(line)
                    })
                };
                negotium::integrate::elevated_command(
                    rest,
                    negotium_actions::is_elevated(),
                    &run_here,
                    &|command| negotium_actions::run_elevated(command, 0),
                    // Not elevated: only the Run value, which the registry guard allows.
                    &|action| {
                        let mut log = negotium::log::Log::new(negotium::log::Log::default_path());
                        negotium::integrate::command_unelevated(
                            action,
                            &negotium::integrate::System::new(),
                            sid.clone(),
                            &mut |line| log.warn(line),
                        )
                    },
                    &mut |line| log.warn(line),
                )
            }
            None => {
                negotium::integrate::command(&args, &negotium::integrate::System::new(), sid, &mut |line| log.warn(line))
            }
        };
        std::process::exit(code);
    }
    // Started by the admin task: `--from-task` (and a logon start's placeholder) set aside.
    let (from_task, args) = negotium::integrate::task_args(args);
    let has_arg = |flag: &str| args.iter().any(|a| a == flag);
    // Start with Windows: the Run value's `--tray`, or the admin task's logon trigger, starts
    // hidden in the tray (passed on through a hand-off).
    let in_tray = negotium::integrate::starts_in_tray(from_task, &args);
    let install = negotium::install::current();
    let install_installed = install.installed;
    let user_sid = negotium_actions::integration::current_user_sid().ok();
    let elevated = negotium_actions::is_elevated();
    let admin_member = negotium_actions::integration::is_admin_member();
    // Admin mode: a non-elevated launch of the installed copy starts the user's task (an elevated
    // copy, with this launch's page and arguments) and exits, before the window and before the
    // instance lock, so the elevated copy can claim it. A running instance is shown instead. A
    // task that is broken or won't start is logged and this launch carries on non-elevated; the
    // copy the task starts (`--from-task`) never starts it again.
    {
        let machine = negotium::integrate::System::new();
        let user_sid = || user_sid.clone();
        // A start in the tray leaves a running instance as it is. A hung one counts as none (it
        // would show nothing): the task is started instead.
        let activate_running = || {
            if in_tray {
                platform::responsive_main_window().is_some()
            } else {
                platform::activate_existing_instance(Duration::ZERO) == platform::Existing::Shown
            }
        };
        // The elevated copy the task started: its marked window, or its instance lock when none
        // was held before (a hung instance's lock is no sign of it).
        let lock_held_before = platform::instance_exists(platform::INSTANCE_MUTEX);
        let copy_appeared = || {
            platform::wait_until(negotium::integrate::HANDOFF_WAIT, Duration::from_millis(100), || {
                platform::responsive_main_window().is_some()
                    || (!lock_held_before && platform::instance_exists(platform::INSTANCE_MUTEX))
            })
        };
        let ctx = negotium::integrate::LaunchCtx {
            installed: install.installed,
            elevated,
            admin_member,
            from_task,
            user_sid: &user_sid,
            // Bounded: a hung Schedule service must not hold up the launch.
            read_task: &|sid| {
                let sid = sid.to_string();
                negotium::integrate::bounded(negotium::integrate::TASK_READ_BOUND, move || {
                    negotium::integrate::Machine::task_state(&negotium::integrate::System::new(), &sid)
                })
            },
            machine: &machine,
            activate_running: &activate_running,
            allow_foreground: &platform::allow_any_foreground,
            copy_appeared: &copy_appeared,
        };
        let passed: Vec<String> = args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
        if negotium::integrate::hand_off(&ctx, &passed, &mut |line| warn(line)) {
            return Ok(());
        }
    }
    // One instance per session. A second launch shows the running one (even from the tray) and
    // exits; a restart (as admin) waits for the old instance to finish quitting. A running one
    // that is hung (Not responding) can't show itself: this launch then runs beside it, without
    // the lock, so Ctrl+Shift+Esc still opens something that works.
    let restarted = has_arg("--restarted");
    let wait = if restarted { Duration::from_secs(10) } else { Duration::ZERO };
    let _instance = match platform::claim_instance(platform::INSTANCE_MUTEX, wait) {
        Some(lock) => Some(lock),
        None => {
            if in_tray && platform::responsive_main_window().is_some() {
                return Ok(());
            }
            match platform::activate_existing_instance(Duration::from_secs(5)) {
                platform::Existing::Shown => return Ok(()),
                platform::Existing::Hung => {
                    warn("a running Negotium isn't responding; starting another beside it");
                    None
                }
                // The holder has no window: it is shutting down (or never got one). Take over once
                // it has gone, so a launch during an exit doesn't end with nothing running.
                platform::Existing::None => match platform::claim_instance(platform::INSTANCE_MUTEX, Duration::from_secs(5)) {
                    Some(lock) => Some(lock),
                    None => return Ok(()),
                },
            }
        }
    };
    // The registry helpers refuse writes outside the test sandbox until the real app unlocks them,
    // so tests (which never run `main`) can't reach the user's startup entries.
    negotium_actions::startup::registry::allow_system_writes();
    // The installers earlier updates left behind (this version is installed now).
    if install_installed {
        negotium::update::clean_leftovers_in_background(elevated);
    }
    // Before the worker starts, so the collector and every action see the privilege.
    if elevated && let Err(e) = negotium_actions::enable_debug_privilege() {
        // Not fatal: actions on other users' processes then report access denied.
        warn(&format!("could not enable SeDebugPrivilege: {e:?}"));
    }
    // GPU (FemtoVG) by default, software once a stall has been recorded, SLINT_BACKEND over both
    // (crate::renderer; docs/benchmarks/README.md, "Renderer").
    let relaunched = has_arg(renderer::FALLBACK_ARG);
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
        let relaunch = renderer::SoftwareRelaunch::resolve(in_tray);
        renderer::Watchdog::arm(renderer::WATCHDOG_TIMEOUT, move || relaunch.run()).0
    });
    // Whether the tray icon may be created: not while the watchdog could still replace this
    // instance (it terminates without cleanup and would orphan the icon).
    let tray_allowed = {
        let watchdog = watchdog.clone();
        move || watchdog.as_ref().is_none_or(|w| w.resolved())
    };
    let ui = MainWindow::new()?;
    platform::set_renderer(running);
    platform::apply_accent(&ui);
    // Settings in effect for this run; the Settings page changes them and saves to `settings_path`.
    let settings = Rc::new(RefCell::new(settings));
    CURRENT_SETTINGS.with(|s| *s.borrow_mut() = Some(settings.clone()));
    // Copied out: no borrow is held while the theme is applied (window calls).
    let initial = settings.borrow().clone();
    negotium::settingspage::apply_settings(&ui, &initial);
    let controls = Arc::new(Controls::default());
    controls.set_refresh_ms(initial.refresh_ms);
    controls.set_moderator_config(initial.moderator.clone());
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
    // The tray icon, once made (below); the tray menu offers "Open Windows Task Manager" while the
    // installed copy's redirect is on.
    let tray: Rc<RefCell<Option<NegotiumTray>>> = Default::default();
    let taskmgr_shown: Rc<Cell<bool>> = Default::default();
    {
        let (settings, controls, saver, tray) = (settings.clone(), controls.clone(), saver.clone(), tray.clone());
        ui.on_settings_aequilibrium_toggled(move |on| {
            settings.borrow_mut().moderator.aequilibrium_enabled = on;
            controls.set_moderator_config(settings.borrow().moderator.clone());
            drop(saver.save(settings.borrow().clone()));
            if let Some(tray) = tray.borrow().as_ref() {
                tray.set_aequilibrium_enabled(on);
            }
        });
    }
    let redirect_shown = {
        // Weak: the tray's own menu handler holds these integrations, which hold this.
        let (tray, taskmgr_shown) = (Rc::downgrade(&tray), taskmgr_shown.clone());
        move |on: bool| {
            taskmgr_shown.set(on);
            if let Some(tray) = tray.upgrade()
                && let Some(t) = tray.borrow().as_ref()
            {
                t.set_taskmgr_shown(on);
            }
        }
    };
    // Ctrl+Alt+Shift+Esc opens the picker, only while the setting is on (the flag). A message-only
    // window that cannot be made reads as a taken shortcut.
    let shortcut = {
        let ctx = negotium::picker::PickerCtx { elevated };
        let api: Box<dyn negotium::shortcut::HotkeyApi> =
            match negotium::shortcut::Win32Hotkey::new(Box::new(move || negotium::picker::open(&ctx))) {
                Ok(h) => Box::new(h),
                Err(e) => {
                    warn(&format!("picker shortcut window unavailable: {e}"));
                    Box::new(negotium::shortcut::Unavailable)
                }
            };
        Rc::new(RefCell::new(negotium::shortcut::Shortcut::new(api)))
    };
    shortcut.borrow_mut().apply(initial.picker_shortcut);
    if shortcut.borrow().taken() {
        ui.set_settings_shortcut_taken(true);
        // Shown now when the window is up; from the tray, on the next `show_main`.
        if in_tray {
            PENDING_TOAST.with(|t| *t.borrow_mut() = Some(negotium::shortcut::TAKEN_TEXT));
        } else {
            ui.set_toast(negotium::shortcut::TAKEN_TEXT.into());
        }
    }
    let integrations = negotium::settingspage::SettingsWiring {
        settings: settings.clone(),
        saver: saver.clone(),
        controls: controls.clone(),
        gpu_marker: renderer::fallback_path(),
        fallback_recorded,
        running,
        tray_cpu_changed: Rc::new(negotium::tray::set_enabled),
        picker_shortcut_changed: Rc::new(move |on| {
            let registered = shortcut.borrow_mut().apply(on);
            if !negotium::shortcut::keep_pending_taken_toast(on, registered) {
                PENDING_TOAST.with(|t| *t.borrow_mut() = None);
            }
            registered
        }),
        integration: negotium::settingspage::Integration {
            install,
            installed_exe: negotium::install::program_files().map(|pf| pf.join("Negotium").join("negotium.exe")),
            // One registry value and the user's task, read on the UI thread when the page is
            // wired, each time it is shown and after each elevated command.
            redirect: Box::new(|| negotium::integrate::Machine::ifeo(&negotium::integrate::System::new())),
            run_value: Box::new(|| negotium::integrate::Machine::run_value(&negotium::integrate::System::new())),
            admin_member,
            read_task: Box::new({
                let sid = user_sid.clone();
                move |done| negotium::settingspage::read_task_in_background(sid.clone(), done)
            }),
            run_here: Box::new({
                let sid = user_sid.clone();
                move |action, done| negotium::settingspage::run_here_in_background(sid.clone(), action, done)
            }),
            elevated,
            // Admin mode just turned on here, non-elevated: the task starts the elevated copy
            // (which waits for this one's lock, `--restarted`) on the page shown, then this one
            // quits. If the task won't start, this copy carries on and says so.
            restart_elevated: Box::new({
                let (controls, weak, saver, sid) = (controls.clone(), ui.as_weak(), saver.clone(), user_sid.clone());
                move || {
                    let Some(sid) = sid.clone() else { return };
                    let args: Vec<String> =
                        negotium::controls::restart_args(controls.page()).split(' ').map(str::to_string).collect();
                    let (controls, weak, saver) = (controls.clone(), weak.clone(), saver.clone());
                    negotium::settingspage::in_background(
                        move || negotium::integrate::Machine::start_task(&negotium::integrate::System::new(), &sid, &args),
                        move |started| match started {
                            Ok(()) => begin_exit(&controls, &weak, &saver),
                            Err(e) => {
                                warn(&format!("admin mode: the task couldn't be started after turning it on: {e}"));
                                if let Some(ui) = weak.upgrade() {
                                    ui.set_toast("Admin mode is on. It takes effect when Negotium restarts.".into());
                                }
                            }
                        },
                    );
                }
            }),
            elevate: Box::new(negotium::settingspage::elevate_in_background),
            start_task_manager: Box::new(negotium::settingspage::start_task_manager_in_background),
            redirect_shown: Box::new(redirect_shown),
        },
    }
    .install(&ui);
    // Updates (installed copy only): "Check for updates", the banner, and the startup check once
    // the window is up (below). Accepting starts msiexec and quits shortly after, like the tray's
    // Exit.
    let updater = negotium::update::Updater::install(
        &ui,
        negotium::update::UpdateIo {
            installed: install_installed,
            check: Box::new(negotium::update::check_in_background),
            // Elevated (admin mode), the installer goes to an admin-only folder: msiexec then
            // starts without a prompt (negotium::update::secure).
            download: Box::new(move |release, done| negotium::update::download_in_background(release, elevated, done)),
            install: Box::new(move |job, done| negotium::update::install_in_background(job, elevated, done)),
            exit: Box::new({
                let (controls, weak, saver) = (controls.clone(), ui.as_weak(), saver.clone());
                move || begin_exit(&controls, &weak, &saver)
            }),
            log: Box::new(warn),
        },
    );
    let tray_wiring = Rc::new(TrayWiring { saver: saver.clone(), integrations, taskmgr_shown, settings: settings.clone(), elevated });
    let bridge = Bridge::install(&ui, controls.clone());
    // Program icons: extracted on the loader's own thread (shell and GDI calls, never in a frame);
    // each result comes back to the bridge's cache on the UI thread, which re-pushes the rows
    // showing them, gathered (bridge::ICON_BATCH). Fails only once the event loop is gone
    // (quitting): nothing left to show.
    bridge.borrow_mut().set_icon_loader(negotium::icons::IconLoader::spawn(negotium::icons::extract, |path, px| {
        let _ = slint::invoke_from_event_loop(move || negotium::bridge::current_icon_arrived(path, px));
    }));

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
    let worker = worker::spawn(controls.clone(), history, epoch, Some(tray_sink), move |frame| {
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
    ui.set_settings_pawnio_installed(negotium_collector::cputemp::pawnio_installed());
    ui.set_windows_sandbox_available(negotium_actions::runtask::is_windows_sandbox_available());

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
    let load_services = {
        let weak = weak.clone();
        move || {
            if weak.upgrade().is_some_and(|ui| negotium::services::begin_job(&ui)) {
                negotium::services::spawn_job(
                    weak.clone(),
                    negotium::services::list_job,
                );
            }
        }
    };
    // Every page change: a rail click, and the page a restart comes back on.
    let show_page = {
        let (weak, b, load_startup, load_services) = (weak.clone(), bridge.clone(), load_startup.clone(), load_services.clone());
        let integrations = tray_wiring.integrations.clone();
        move |page: negotium::controls::Page| {
            if let Some(ui) = weak.upgrade() {
                b.borrow_mut().set_page(&ui, page);
            }
            // Outside the bridge borrow.
            if page == negotium::controls::Page::Startup {
                load_startup();
            } else if page == negotium::controls::Page::Services {
                load_services();
            }
            // The Settings switches read the machine again (a change made outside Negotium).
            integrations.page_shown(page);
        }
    };
    {
        let show_page = show_page.clone();
        ui.on_select_page(move |i| show_page(negotium::controls::Page::from_index(i)));
    }
    // A restart (as admin) comes back on the page it left (spec §6); anything else opens on the
    // Overview.
    if let Some(page) = negotium::controls::page_from_os_args(args.iter().cloned()) {
        show_page(page);
    }
    ui.on_refresh_startup(load_startup.clone());
    {
        let (b, weak) = (bridge.clone(), weak.clone());
        ui.on_sort_startup(move |col| {
            if let Some(ui) = weak.upgrade() {
                b.borrow_mut().sort_startup(&ui, col);
            }
        });
    }
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
        ui.on_sort_history(move |col| {
            let (cur_col, cur_desc) = controls.history_sort();
            let desc = if col == cur_col { !cur_desc } else { true };
            controls.set_history_sort(col, desc);
            if let Some(ui) = weak.upgrade() {
                ui.set_history_sort_column(col);
                ui.set_history_sort_descending(desc);
            }
        });
    }
    {
        let controls = controls.clone();
        ui.on_reset_history(move || controls.request_reset_history());
    }
    ui.on_refresh_services(load_services.clone());
    {
        let b = bridge.clone();
        ui.on_search_services(move |text| b.borrow_mut().set_services_search(&text));
    }
    {
        let (b, weak) = (bridge.clone(), weak.clone());
        ui.on_sort_services(move |col| {
            if let Some(ui) = weak.upgrade() {
                b.borrow_mut().sort_services(&ui, col);
            }
        });
    }
    {
        let (b, weak) = (bridge.clone(), weak.clone());
        ui.on_service_action(move |action, id| {
            let Some(ui) = weak.upgrade() else { return };
            let Some(entry) = b.borrow().service_entry(id) else { return };
            if action.as_str() == "goto" {
                if let Some(pid) = entry.pid.filter(|&p| p > 0) {
                    b.borrow_mut().select_pid(&ui, pid);
                }
                return;
            }
            if !negotium::services::begin_job(&ui) {
                return;
            }
            let act = action.to_string();
            let name = entry.name.clone();
            negotium::services::spawn_job(
                weak.clone(),
                move || negotium::services::action_job(&name, &act, elevated),
            );
        });
    }
    {
        let weak = weak.clone();
        ui.on_run_new_task(move |cmd, cwd, mode, compat, eco_qos| {
            let owner = weak.upgrade().and_then(|ui| platform::main_hwnd(&ui)).map_or(0, |h| h.0 as isize);
            let opts = negotium_actions::runtask::RunTaskOptions {
                command: cmd.to_string(),
                working_dir: cwd.to_string(),
                privilege: negotium_actions::runtask::PrivilegeMode::from_index(mode),
                compat: negotium_actions::runtask::CompatLayer::from_index(compat),
                efficiency_mode: eco_qos,
            };
            let weak = weak.clone();
            std::thread::spawn(move || {
                if let Err(e) = negotium_actions::runtask::run_new_task(&opts, owner)
                    && !matches!(e, negotium_actions::ActionError::Cancelled)
                {
                    let msg = format!("Could not run task: {e}");
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = weak.upgrade() {
                            ui.set_toast(msg.into());
                        }
                    });
                }
            });
        });
    }
    {
        let weak = weak.clone();
        ui.on_browse_executable(move || {
            let owner = weak
                .upgrade()
                .and_then(|ui| platform::main_hwnd(&ui))
                .map_or(0, |h| h.0 as isize);
            let weak = weak.clone();
            std::thread::spawn(move || {
                if let Some(path) = negotium_actions::runtask::pick_executable(owner) {
                    let path_str = path.to_string_lossy();
                    let quoted_cmd = if path_str.contains(' ') {
                        format!("\"{path_str}\"")
                    } else {
                        path_str.into_owned()
                    };
                    let dir_str = path
                        .parent()
                        .map(|p| p.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = weak.upgrade() {
                            ui.set_run_task_command(quoted_cmd.into());
                            if ui.get_run_task_working_dir().trim().is_empty() {
                                ui.set_run_task_working_dir(dir_str.into());
                            }
                        }
                    });
                }
            });
        });
    }
    {
        let (controls, weak) = (controls.clone(), weak.clone());
        let copy_weak = weak.clone();
        ui.on_copy_windows_sandbox_cmd(move || {
            let res = negotium_actions::runtask::copy_to_clipboard(negotium_actions::runtask::ENABLE_SANDBOX_CMD);
            let weak = copy_weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = weak.upgrade() {
                    match res {
                        Ok(()) => ui.set_toast("Command copied to clipboard.".into()),
                        Err(e) => ui.set_toast(format!("Could not copy command: {e}").into()),
                    }
                }
            });
        });
        let term_weak = weak.clone();
        ui.on_enable_windows_sandbox_terminal(move || {
            let owner = term_weak.upgrade().and_then(|ui| platform::main_hwnd(&ui)).map_or(0, |h| h.0 as isize);
            let weak = term_weak.clone();
            std::thread::spawn(move || {
                if let Err(e) = negotium_actions::runtask::enable_windows_sandbox_in_terminal(owner)
                    && !matches!(e, negotium_actions::ActionError::Cancelled)
                {
                    let msg = format!("Could not launch terminal: {e}");
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = weak.upgrade() {
                            ui.set_toast(msg.into());
                        }
                    });
                }
            });
        });
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
    let exit_on_close = has_arg("--exit-on-close");
    // Whether the window is on screen: graphs scroll only then (negotium::smooth). Every path that
    // hides, shows, minimises or restores it goes through this.
    let on_screen = OnScreen::new(controls.clone(), &ui);
    // Started in the tray: the worker keeps the tray's pace from the start.
    if in_tray {
        on_screen.hide();
    }
    {
        let (controls, b, close_weak, settings) = (controls.clone(), bridge.clone(), weak.clone(), settings.clone());
        let (saver, on_screen) = (saver.clone(), on_screen.clone());
        ui.window().on_close_requested(move || {
            // Copied out: no borrow is held across `begin_exit` and its flush.
            let keep_in_tray = settings.borrow().keep_in_tray;
            match settings::close_action(keep_in_tray, exit_on_close) {
                settings::CloseAction::Exit => begin_exit(&controls, &close_weak, &saver),
                settings::CloseAction::HideToTray => {
                    if let Some(ui) = close_weak.upgrade()
                        && let Some(hwnd) = platform::main_hwnd(&ui)
                        && let Some(geo) = platform::read_window_geometry(hwnd)
                    {
                        save_window_geometry(geo, &saver);
                    }
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
    // NEGOTIUM_FRAME_STATS=<path>, times every frame for the bench: only in a debug build or with
    // the bench's own flag, and never elevated (the path is written at exit; framestats::path).
    let frame_stats_path = negotium::framestats::path(
        elevated,
        cfg!(debug_assertions) || has_arg(negotium::framestats::BENCH_ARG),
        std::env::var_os(negotium::framestats::ENV),
    );
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
    let active_affinity: Arc<Mutex<Option<ActiveAffinityTarget>>> = Default::default();
    let active_priority: Arc<Mutex<Option<ActivePriorityTarget>>> = Default::default();
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
        let (settings, controls, saver) = (settings.clone(), controls.clone(), saver.clone());
        let (active_affinity, active_priority) = (active_affinity.clone(), active_priority.clone());
        ui.on_row_action(move |action, id| {
            // Clone out of the borrow and release it before doing anything else.
            let (target, procs) = {
                let b = b.borrow();
                (b.target(id), b.all_procs().to_vec())
            };
            let Some(target) = target else { return };
            if action.as_str() == "set-affinity" {
                let root_proc = target.root.or_else(|| target.procs.first().copied());
                let Some((pid, ct)) = root_proc else { return; };
                match negotium_actions::get_process_affinity(pid, ct) {
                    Ok((proc_mask, sys_mask)) => {
                        let mut core_items = Vec::new();
                        let mut all_checked = true;
                        let mut any_checked = false;
                        for i in 0..64 {
                            if (sys_mask & (1u64 << i)) != 0 {
                                let checked = (proc_mask & (1u64 << i)) != 0;
                                if !checked {
                                    all_checked = false;
                                } else {
                                    any_checked = true;
                                }
                                core_items.push(negotium::CoreItem {
                                    id: i,
                                    name: format!("CPU {i}").into(),
                                    checked,
                                });
                            }
                        }
                        let has_rule = settings.borrow().moderator.rules.iter().any(|r| negotium_moderator::same_image(&r.name, &target.process_name) && r.affinity_mask.is_some());
                        *active_affinity.lock().unwrap() = Some(ActiveAffinityTarget {
                            name: target.name.clone(),
                            process_name: target.process_name.clone(),
                            procs: target.procs.clone(),
                        });
                        if let Some(ui) = weak.upgrade() {
                            ui.set_affinity_title(format!("Set CPU Affinity - {} (PID {})", target.name, pid).into());
                            ui.set_affinity_process_name(target.name.clone().into());
                            ui.set_affinity_cores(slint::ModelRc::new(slint::VecModel::from(core_items)));
                            ui.set_affinity_all_cores(all_checked);
                            ui.set_affinity_can_apply(any_checked);
                            ui.set_affinity_remember(has_rule);
                            ui.set_affinity_visible(true);
                        }
                    }
                    Err(err) => {
                        if let Some(ui) = weak.upgrade() {
                            ui.set_toast(err.message(&target.name, elevated).into());
                        }
                    }
                }
                return;
            }
            if action.as_str() == "set-priority" {
                let root_proc = target.root.or_else(|| target.procs.first().copied());
                let Some((pid, ct)) = root_proc else { return; };
                match negotium_actions::get_priority_class(pid, ct) {
                    Ok(priority) => {
                        let idx = match priority {
                            negotium_actions::PriorityClass::Realtime => 0,
                            negotium_actions::PriorityClass::High => 1,
                            negotium_actions::PriorityClass::AboveNormal => 2,
                            negotium_actions::PriorityClass::Normal => 3,
                            negotium_actions::PriorityClass::BelowNormal => 4,
                            negotium_actions::PriorityClass::Idle => 5,
                        };
                        let has_rule = settings.borrow().moderator.rules.iter().any(|r| negotium_moderator::same_image(&r.name, &target.process_name) && r.priority.is_some());
                        *active_priority.lock().unwrap() = Some(ActivePriorityTarget {
                            name: target.name.clone(),
                            process_name: target.process_name.clone(),
                            procs: target.procs.clone(),
                        });
                        if let Some(ui) = weak.upgrade() {
                            ui.set_priority_title(format!("Set CPU Priority - {} (PID {})", target.name, pid).into());
                            ui.set_priority_process_name(target.name.clone().into());
                            ui.set_priority_selected(idx);
                            ui.set_priority_remember(has_rule);
                            ui.set_priority_visible(true);
                        }
                    }
                    Err(err) => {
                        if let Some(ui) = weak.upgrade() {
                            ui.set_toast(err.message(&target.name, elevated).into());
                        }
                    }
                }
                return;
            }
            if action.as_str() == "toggle-aequilibrium-exclusion" {
                // The moderator matches image names, not an app card's friendly name.
                let image = &target.process_name;
                let mut s = settings.borrow_mut();
                let already_excluded = negotium_moderator::is_user_excluded(image, &s.moderator.exclusions);
                let toast_msg = if already_excluded {
                    s.moderator.exclusions.retain(|ex| !negotium_moderator::same_image(ex, image));
                    format!("Included {} in Aequilibrium.", target.name)
                } else {
                    s.moderator.exclusions.push(image.clone());
                    format!("Excluded {} from Aequilibrium.", target.name)
                };
                controls.set_moderator_config(s.moderator.clone());
                drop(saver.save(s.clone()));
                if let Some(ui) = weak.upgrade() {
                    ui.set_toast(toast_msg.into());
                }
                return;
            }
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
        ui.on_affinity_toggle_core(move |core_id| {
            let Some(ui) = weak.upgrade() else { return };
            let model = ui.get_affinity_cores();
            let mut all_checked = true;
            let mut any_checked = false;
            for idx in 0..model.row_count() {
                if let Some(mut item) = model.row_data(idx) {
                    if item.id == core_id {
                        item.checked = !item.checked;
                        model.set_row_data(idx, item.clone());
                    }
                    if item.checked {
                        any_checked = true;
                    } else {
                        all_checked = false;
                    }
                }
            }
            ui.set_affinity_all_cores(all_checked);
            ui.set_affinity_can_apply(any_checked);
        });
    }
    {
        let weak = weak.clone();
        ui.on_affinity_toggle_all(move |on| {
            let Some(ui) = weak.upgrade() else { return };
            let model = ui.get_affinity_cores();
            for idx in 0..model.row_count() {
                if let Some(mut item) = model.row_data(idx) {
                    item.checked = on;
                    model.set_row_data(idx, item);
                }
            }
            ui.set_affinity_all_cores(on);
            ui.set_affinity_can_apply(on && model.row_count() > 0);
        });
    }
    {
        let weak = weak.clone();
        let active = active_affinity.clone();
        ui.on_affinity_cancel(move || {
            active.lock().unwrap().take();
            if let Some(ui) = weak.upgrade() {
                ui.set_affinity_visible(false);
            }
        });
    }
    {
        let weak = weak.clone();
        let active = active_affinity.clone();
        let (settings, controls, saver) = (settings.clone(), controls.clone(), saver.clone());
        ui.on_affinity_apply(move |remember| {
            let Some(ui) = weak.upgrade() else { return };
            ui.set_affinity_visible(false);
            let Some(target) = active.lock().unwrap().take() else { return };
            let model = ui.get_affinity_cores();
            let mut new_mask = 0u64;
            let mut count = 0;
            for idx in 0..model.row_count() {
                if let Some(item) = model.row_data(idx)
                    && item.checked
                {
                    new_mask |= 1u64 << (item.id as u64);
                    count += 1;
                }
            }
            if new_mask == 0 {
                ui.set_toast("At least one CPU core must be selected.".into());
                return;
            }
            let mut applied = 0;
            let mut err = None;
            for &(pid, ct) in &target.procs {
                match negotium_actions::set_process_affinity(pid, ct, new_mask) {
                    Ok(_) => applied += 1,
                    Err(e) => err = Some(e),
                }
            }
            if remember {
                let mut s = settings.borrow_mut();
                if let Some(r) = s.moderator.rules.iter_mut().find(|r| negotium_moderator::same_image(&r.name, &target.process_name)) {
                    r.affinity_mask = Some(new_mask);
                } else {
                    s.moderator.rules.push(negotium_moderator::PersistentRule {
                        name: target.process_name.clone(),
                        priority: None,
                        affinity_mask: Some(new_mask),
                        cpu_sets: None,
                        efficiency_mode: None,
                    });
                }
                controls.set_moderator_config(s.moderator.clone());
                drop(saver.save(s.clone()));
            }
            let toast = if applied > 0 {
                if remember {
                    format!("Set CPU affinity for {} to {} core(s) (saved as rule).", target.name, count)
                } else {
                    format!("Set CPU affinity for {} to {} core(s).", target.name, count)
                }
            } else if let Some(e) = err {
                e.message(&target.name, elevated)
            } else {
                format!("No active process to set affinity for {}.", target.name)
            };
            ui.set_toast(toast.into());
        });
    }
    {
        let weak = weak.clone();
        let active = active_priority.clone();
        ui.on_priority_cancel(move || {
            active.lock().unwrap().take();
            if let Some(ui) = weak.upgrade() {
                ui.set_priority_visible(false);
            }
        });
    }
    {
        let weak = weak.clone();
        let active = active_priority.clone();
        let (settings, controls, saver) = (settings.clone(), controls.clone(), saver.clone());
        ui.on_priority_apply(move |selected_idx, remember| {
            let Some(ui) = weak.upgrade() else { return };
            ui.set_priority_visible(false);
            let Some(target) = active.lock().unwrap().take() else { return };
            let prio = match selected_idx {
                0 => negotium_actions::PriorityClass::Realtime,
                1 => negotium_actions::PriorityClass::High,
                2 => negotium_actions::PriorityClass::AboveNormal,
                3 => negotium_actions::PriorityClass::Normal,
                4 => negotium_actions::PriorityClass::BelowNormal,
                _ => negotium_actions::PriorityClass::Idle,
            };
            let mut applied = 0;
            let mut err = None;
            for &(pid, ct) in &target.procs {
                match negotium_actions::set_priority_class(pid, ct, prio) {
                    Ok(_) => applied += 1,
                    Err(e) => err = Some(e),
                }
            }
            if remember {
                let mut s = settings.borrow_mut();
                if let Some(r) = s.moderator.rules.iter_mut().find(|r| negotium_moderator::same_image(&r.name, &target.process_name)) {
                    r.priority = Some(prio);
                } else {
                    s.moderator.rules.push(negotium_moderator::PersistentRule {
                        name: target.process_name.clone(),
                        priority: Some(prio),
                        affinity_mask: None,
                        cpu_sets: None,
                        efficiency_mode: None,
                    });
                }
                controls.set_moderator_config(s.moderator.clone());
                drop(saver.save(s.clone()));
            }
            let toast = if applied > 0 {
                if remember {
                    format!("Set priority for {} to {} (saved as rule).", target.name, prio.display_name())
                } else {
                    format!("Set priority for {} to {}.", target.name, prio.display_name())
                }
            } else if let Some(e) = err {
                e.message(&target.name, elevated)
            } else {
                format!("No active process to set priority for {}.", target.name)
            };
            ui.set_toast(toast.into());
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

    // Started in the tray: the window is created far off screen, so the moment before it can be
    // hidden (below) shows nothing.
    let saved_geo = settings.borrow().window_geometry.map(platform::sanitize_geometry);
    if in_tray {
        let (x, y) = platform::OFF_SCREEN;
        ui.window().set_position(slint::PhysicalPosition::new(x, y));
    } else if let Some(geo) = saved_geo {
        ui.window().set_position(slint::PhysicalPosition::new(geo.x, geo.y));
        ui.window().set_size(slint::PhysicalSize::new(geo.width, geo.height));
    }
    ui.show()?;
    // The native HWND is created lazily once the event loop runs, so poll until it exists; then,
    // once: apply the chrome, hook the window (tray re-creation, activation by a second launch)
    // and mark it for that second launch. Every tick is also the renderer watchdog's heartbeat
    // (the UI thread is alive), and the tray icon waits until the watchdog has resolved.
    let chrome_timer = Rc::new(slint::Timer::default());
    {
        let weak = weak.clone();
        let controls = controls.clone();
        let handle = Rc::downgrade(&chrome_timer);
        let updater = updater.clone();
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
                    let (tray_allowed, settings, tray_wiring) = (tray_allowed.clone(), settings.clone(), tray_wiring.clone());
                    platform::install_window_hooks(hwnd, move |event| {
                        if let ShellEvent::Minimised(m) = event {
                            on_screen.on_size(m);
                            return;
                        }
                        // The hook runs inside the window procedure: act from the event loop instead.
                        let (tray, controls, weak, on_screen) = (tray.clone(), controls.clone(), weak.clone(), on_screen.clone());
                        let (tray_allowed, settings, tray_wiring) = (tray_allowed.clone(), settings.clone(), tray_wiring.clone());
                        slint::Timer::single_shot(Duration::ZERO, move || match event {
                            // Explorer restarted (perhaps ended from this very list): add the icon again.
                            // Before the watchdog resolves, the first creation below adds it.
                            ShellEvent::TaskbarCreated if tray_allowed() => {
                                drop(tray.borrow_mut().take());
                                *tray.borrow_mut() = make_tray(&controls, &on_screen, &weak, &tray_wiring)
                                    .map_err(|e| warn(&format!("tray icon could not be re-created: {e}")))
                                    .ok();
                            }
                            ShellEvent::TaskbarCreated => {}
                            ShellEvent::Activate => show_main(&on_screen, &weak),
                            // The installer is about to replace the files.
                            ShellEvent::Exit => begin_exit(&controls, &weak, &tray_wiring.saver),
                            ShellEvent::Minimised(_) => {} // handled above
                            ShellEvent::MovedOrResized => {
                                if let Some(ui) = weak.upgrade()
                                    && let Some(hwnd) = platform::main_hwnd(&ui)
                                    && let Some(geo) = platform::read_window_geometry(hwnd)
                                {
                                    CURRENT_SETTINGS.with(|s| {
                                        if let Some(s) = s.borrow().as_ref() {
                                            s.borrow_mut().window_geometry = Some(geo);
                                        }
                                    });
                                }
                            }
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
                // Started in the tray: the native window exists now (hooked and marked, so the tray
                // and a second launch can show it), and goes out of sight as closing it to the
                // tray would. It was created off screen, so the moment before this showed
                // nothing; hidden, it moves to the middle of the primary screen, where the tray
                // or a second launch shows it.
                if in_tray {
                    let _ = ui.hide();
                    let (x, y) = match saved_geo {
                        Some(geo) => (geo.x, geo.y),
                        None => platform::on_screen_position(platform::centered_position(hwnd), platform::primary_work_area()),
                    };
                    ui.window().set_position(slint::PhysicalPosition::new(x, y));
                    if let Some(geo) = saved_geo {
                        ui.window().set_size(slint::PhysicalSize::new(geo.width, geo.height));
                    }
                } else if let Some(geo) = saved_geo {
                    platform::apply_window_geometry(hwnd, geo);
                }
                // The window is up: the installed copy looks for an update, off the UI thread.
                updater.startup(settings.borrow().check_updates);
            }
            if !tray_allowed() {
                return;
            }
            *tray.borrow_mut() = make_tray(&controls, &on_screen, &weak, &tray_wiring)
                .map_err(|e| warn(&format!("tray icon could not be created: {e}")))
                .ok();
            if let Some(t) = handle.upgrade() {
                t.stop();
            }
        });
    }
    let result = slint::run_event_loop_until_quit();
    // Let the worker stop and undo its restraints (bounded). begin_exit normally asked already.
    controls.request_stop();
    let deadline = std::time::Instant::now() + WORKER_EXIT_WAIT;
    while !worker.is_finished() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    if let Some(path) = frame_stats_path {
        let _ = std::fs::write(path, frame_stats.borrow().summary_json());
    }
    result
}
