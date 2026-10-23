//! The Settings page's Rust side: shows the settings on the window and acts on the page's
//! choices. Each choice applies at once, the page shows the new state, and the file is saved off
//! the UI thread (`settings::Saver`).
use crate::controls::{Controls, header_click, sort_column_from_index, sort_column_index};
use negotium_model::{Metric, Sort, SortColumn};
use crate::install::{self, Install};
use crate::controls::Page;
use crate::integrate::{Action, TaskState};
use crate::settings::{BackdropChoice, Saver, Settings, ThemeChoice, refresh_from_index};
use crate::renderer::{self, Renderer};
use crate::{MainWindow, platform};
use slint::ComponentHandle;
use negotium_actions::ActionError;
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

/// Applies `s` to the window: the backdrop, the theme (colours, title bar, backdrop), always on
/// top, and what the Settings page shows. The refresh rate lives in `Controls`
/// (`Controls::set_refresh_ms`).
pub fn apply_settings(ui: &MainWindow, s: &Settings) {
    platform::set_backdrop(ui, s.backdrop);
    platform::apply_theme(ui, s.theme);
    show_settings(ui, s);
}

/// The window state that is only a property: always on top and the page's choices and switches.
fn show_settings(ui: &MainWindow, s: &Settings) {
    ui.set_on_top(s.always_on_top);
    ui.set_settings_refresh(s.refresh_index());
    ui.set_settings_theme(s.theme.index());
    ui.set_settings_backdrop(s.backdrop.index());
    ui.set_settings_keep_in_tray(s.keep_in_tray);
    ui.set_settings_gpu_rendering(s.gpu_rendering);
    ui.set_settings_smooth_graphs(s.smooth_graphs);
    ui.set_settings_tray_cpu(s.tray_cpu);
    ui.set_settings_picker_shortcut(s.picker_shortcut);
    ui.set_settings_group_by_type(s.group_by_type);
    ui.set_settings_check_updates(s.check_updates);
    ui.set_settings_start_with_windows(s.start_with_windows);
    ui.set_sort_column(sort_column_index(s.sort().column));
    ui.set_sort_descending(s.sort().descending);
    ui.set_col_pid(s.col_pid);
    ui.set_col_proc_name(s.col_proc_name);
    ui.set_col_status(s.col_status);
    ui.set_col_threads(s.col_threads);
    ui.set_col_handles(s.col_handles);
    ui.set_col_publisher(s.col_publisher);
    ui.set_settings_aequilibrium_enabled(s.moderator.aequilibrium_enabled);
    // Off: GraphClock rests the view on the newest sample at once (PF3).
    ui.global::<crate::GraphClock>().set_smooth(s.smooth_graphs);
}

/// What the page shows about this copy: the installed copy offers admin mode, the Ctrl+Shift+Esc
/// redirect, Start with Windows and updates; a development build shows them disabled.
pub fn show_install(ui: &MainWindow, install: &Install) {
    ui.set_settings_installed(install.installed);
    ui.set_version_text(install::version_line(install.installed).into());
}

/// Everything the page's handlers act on.
pub struct SettingsWiring {
    /// The settings in effect, shared with the close handler (keep in tray).
    pub settings: Rc<RefCell<Settings>>,
    pub saver: Saver,
    pub controls: Arc<Controls>,
    /// The renderer's automatic-fallback marker (`renderer::fallback_path`), and whether it
    /// existed at launch.
    pub gpu_marker: PathBuf,
    pub fallback_recorded: bool,
    /// The renderer this instance draws with: `Gpu` only if it was selected successfully.
    pub running: Renderer,
    /// The tray shows the CPU graph or the app icon (`crate::tray::set_enabled` in the app).
    pub tray_cpu_changed: Rc<dyn Fn(bool)>,
    /// Registers or unregisters the picker shortcut (`crate::shortcut`); returns whether it is now
    /// registered. The saved choice does not depend on the answer.
    pub picker_shortcut_changed: Rc<dyn Fn(bool) -> bool>,
    /// This copy, and what the Ctrl+Shift+Esc and admin mode switches and "Open Windows Task
    /// Manager" act on.
    pub integration: Integration,
}

/// What the Ctrl+Shift+Esc and admin mode switches and "Open Windows Task Manager" read and run.
/// The app passes the registry, Task Scheduler and the elevated `--integrate` command
/// (`main.rs`); tests pass fakes.
pub struct Integration {
    /// This copy (`install::current()` in the app).
    pub install: Install,
    /// Where the installed copy lives (`<Program Files>\Negotium\negotium.exe`), installed or
    /// not: a development build asks it to lift its redirect for "Open Windows Task Manager".
    pub installed_exe: Option<PathBuf>,
    /// The exe taskmgr.exe's redirect names (`integrate::Machine::ifeo`).
    pub redirect: Box<dyn Fn() -> Option<PathBuf>>,
    /// The HKCU Run value `Negotium` (`integrate::Machine::run_value`), one registry value:
    /// Start with Windows without admin mode.
    pub run_value: Box<dyn Fn() -> Option<String>>,
    /// The user is in Administrators (`integration::is_admin_member`): admin mode is offered
    /// only then.
    pub admin_member: bool,
    /// Reads this user's admin task (`integrate::Machine::task_state` and `task_logon`, Task
    /// Scheduler calls) off the UI thread and hands it back on it ([`read_task_in_background`]
    /// in the app); asked only for the installed copy of an administrator.
    pub read_task: Box<dyn Fn(TaskRead)>,
    /// Runs an integration action in this process, without elevation, off the UI thread; the
    /// outcome comes back on the UI thread ([`run_here_in_background`] in the app). Start with
    /// Windows without admin mode only: the Run value is the user's own (HKCU), so it needs no
    /// UAC prompt.
    pub run_here: Box<RunHere>,
    /// This instance runs elevated.
    pub elevated: bool,
    /// Starts the admin task with `--restarted` and the current page and, once it has started,
    /// quits this instance (`main.rs`): admin mode just turned on in a non-elevated copy, which
    /// the elevated copy replaces.
    pub restart_elevated: Box<dyn Fn()>,
    /// Runs the exe with the arguments elevated (one UAC prompt; the `isize` is the window's
    /// HWND, 0 for none) off the UI thread; the outcome comes back on the UI thread
    /// ([`elevate_in_background`] in the app). A command still running when the wait runs out
    /// is watched, and the [`LateExit`] runs on the UI thread once it exits.
    pub elevate: Box<Elevate>,
    /// Starts Windows' own Task Manager when no redirect of ours is in the way
    /// ([`start_task_manager_in_background`] in the app).
    pub start_task_manager: Box<dyn Fn()>,
    /// Told whether the redirect is ours each time it is read (the tray offers "Open Windows Task
    /// Manager" only then).
    pub redirect_shown: Box<dyn Fn(bool)>,
}

/// An elevated command's outcome, delivered on the UI thread.
pub type ElevatedDone = Box<dyn FnOnce(Result<(), ActionError>)>;
/// [`Integration::elevate`]: the exe, its arguments, the prompt's owner, then the outcome and
/// a late exit.
pub type Elevate = dyn Fn(PathBuf, Vec<String>, isize, ElevatedDone, LateExit);
/// An elevated command that outlasted its wait has exited (on the UI thread).
pub type LateExit = Box<dyn FnOnce()>;
/// The admin task, delivered on the UI thread.
pub type TaskRead = Box<dyn FnOnce(AdminTask)>;
/// [`Integration::run_here`]: the action, then its outcome.
pub type RunHere = dyn Fn(Action, ElevatedDone);

/// The user's admin task as Settings shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdminTask {
    pub state: TaskState,
    /// It has the logon trigger: Start with Windows in admin mode. `None`: it couldn't be read
    /// (the saved choice then stands).
    pub logon: Option<bool>,
}

/// The Ctrl+Shift+Esc and admin mode switches and "Open Windows Task Manager", wired. One
/// elevated command at a time: a click while a UAC prompt (or its command) is still open does
/// nothing.
pub struct Integrations {
    io: Integration,
    /// Start with Windows decides whether admin mode's task also starts at sign-in; it follows
    /// the machine (the Run value or the task's trigger) and is saved when that changes.
    settings: Rc<RefCell<Settings>>,
    saver: Saver,
    weak: slint::Weak<MainWindow>,
    busy: Cell<bool>,
    /// The admin mode row's last known state, shown while Task Scheduler is read again, and
    /// whether the task has the logon trigger; `task_known`: the task has been read once.
    admin_state: Cell<i32>,
    task_logon: Cell<Option<bool>>,
    task_known: Cell<bool>,
    /// A task read is running; `read_again`: another was asked for meanwhile.
    reading: Cell<bool>,
    read_again: Cell<bool>,
}

/// The admin mode row's state (`settings-admin-mode-state`): 0 off, 1 on, 2 needs repair, 3 not
/// available on this account.
pub fn admin_mode_state(admin_member: bool, task: &TaskState) -> i32 {
    match task {
        _ if !admin_member => 3,
        TaskState::Ready => 1,
        TaskState::Missing => 0,
        TaskState::Disabled | TaskState::Broken(_) => 2,
    }
}

impl Integrations {
    /// Whether the redirect names this copy; only the installed copy's redirect counts.
    fn redirect_is_ours(&self) -> bool {
        let install = &self.io.install;
        install.installed && (self.io.redirect)().is_some_and(|exe| crate::integrate::same_path(&exe, &install.exe))
    }

    /// The exe whose elevated open-task-manager lifts the redirect, when the redirect is in the
    /// way: this copy with its own redirect, or, for a development build, the installed copy
    /// whose redirect is on (only it may change it).
    fn redirect_lifter(&self) -> Option<PathBuf> {
        if self.redirect_is_ours() {
            return Some(self.io.install.exe.clone());
        }
        let installed = self.io.installed_exe.as_deref().filter(|_| !self.io.install.installed)?;
        let names = |exe: &Path| crate::integrate::same_path(exe, installed);
        (self.io.redirect)().is_some_and(|exe| names(&exe)).then(|| installed.to_path_buf())
    }

    /// Admin mode is on (a task exists, even one that needs repair): Start with Windows is then
    /// the task's logon trigger, changed elevated.
    fn admin_mode(&self) -> bool {
        self.io.admin_member && matches!(self.admin_state.get(), 1 | 2)
    }

    /// Whether Start with Windows is on, as the machine says: the task's trigger in admin mode,
    /// the Run value otherwise. `None` while it isn't known (a development build, the task not
    /// read yet for an administrator's installed copy, or a trigger Task Scheduler couldn't
    /// report): the saved choice stands meanwhile and is not overwritten.
    fn startup_on(&self) -> Option<bool> {
        let install = &self.io.install;
        if !install.installed || (self.io.admin_member && !self.task_known.get()) {
            return None;
        }
        if self.admin_mode() {
            return self.task_logon.get();
        }
        Some((self.io.run_value)().is_some_and(|v| crate::integrate::run_value_starts(&v, &install.exe)))
    }

    /// Shows Start with Windows as the machine has it; the saved choice follows (and is saved
    /// when it changed), so admin mode's `--logon` and the page agree with the machine.
    fn show_startup(&self) {
        let Some(on) = self.startup_on() else { return };
        if self.settings.borrow().start_with_windows != on {
            self.settings.borrow_mut().start_with_windows = on;
            drop(self.saver.save(self.settings.borrow().clone()));
        }
        if let Some(ui) = self.weak.upgrade() {
            ui.set_settings_start_with_windows(on);
        }
    }

    /// Reads the machine again and shows it: the switches, and the tray menu. The redirect and
    /// the Run value are one registry value each, read here; the admin task is read off the UI
    /// thread, and its rows show their last known state until the answer comes.
    pub fn show(self: &Rc<Self>) {
        let ours = self.redirect_is_ours();
        let admin = if !self.io.admin_member {
            admin_mode_state(false, &TaskState::Missing)
        } else if self.io.install.installed {
            self.read_task();
            self.admin_state.get()
        } else {
            0
        };
        if let Some(ui) = self.weak.upgrade() {
            ui.set_settings_hotkey(ours);
            ui.set_settings_admin_mode_state(admin);
        }
        self.show_startup();
        (self.io.redirect_shown)(ours);
    }

    /// Asks for the admin task's state; one read at a time, and one more if asked meanwhile (the
    /// answer must reflect the latest change).
    fn read_task(self: &Rc<Self>) {
        if self.reading.replace(true) {
            self.read_again.set(true);
            return;
        }
        let this = self.clone();
        (self.io.read_task)(Box::new(move |task| {
            this.reading.set(false);
            this.admin_state.set(admin_mode_state(true, &task.state));
            this.task_logon.set(task.logon);
            this.task_known.set(true);
            if let Some(ui) = this.weak.upgrade() {
                ui.set_settings_admin_mode_state(this.admin_state.get());
            }
            this.show_startup();
            if this.read_again.take() {
                this.read_task();
            }
        }));
    }

    /// A page was shown: the Settings page reads the machine again, so a change made outside
    /// Negotium (or by the installer) shows.
    pub fn page_shown(self: &Rc<Self>, page: Page) {
        if page == Page::Settings {
            self.show();
        }
    }

    /// The switch asked for `on`: the elevated hotkey-on/off. The switch shows the machine's
    /// state again once the command returns, so a declined UAC prompt (nothing changed) snaps it
    /// back.
    pub fn toggle_hotkey(self: &Rc<Self>, on: bool) {
        if !self.io.install.installed {
            self.show();
            return;
        }
        let action = if on { Action::HotkeyOn } else { Action::HotkeyOff };
        self.run_elevated(action, |e| e.message("Open on Ctrl+Shift+Esc", false), |_| {});
    }

    /// The admin mode switch asked for `on`: the elevated admin-on (with `--logon` while Start
    /// with Windows is on) or admin-off. Turning it on also repairs a broken task (it is
    /// recreated). As for the hotkey, the switch then shows what Task Scheduler says. Turned on
    /// in a non-elevated copy, the task then starts the elevated copy, which replaces this one:
    /// otherwise every later launch would just show this non-elevated window.
    pub fn toggle_admin_mode(self: &Rc<Self>, on: bool) {
        if !self.io.install.installed || !self.io.admin_member {
            self.show();
            return;
        }
        let action = if on {
            Action::AdminOn { logon: self.settings.borrow().start_with_windows }
        } else {
            Action::AdminOff
        };
        let restart = on && !self.io.elevated;
        // Off while Start with Windows is on: the elevated admin-off sets the Run value of the
        // account that approved the prompt; this user's own Run value is set here too, in case
        // that was another account (over-the-shoulder credentials).
        let startup = !on && self.settings.borrow().start_with_windows;
        self.run_elevated(action, |e| e.message("Admin mode", false), move |this| {
            if restart {
                (this.io.restart_elevated)();
            }
            if startup {
                this.run_command(Runner::Here, Action::StartupOn, |e| e.message("Start with Windows", false), |_| {});
            }
        });
    }

    /// The Start with Windows switch asked for `on`: startup-on/off, which picks the form
    /// (`integrate::run`). Without admin mode that is the Run value, run here without a prompt;
    /// with it, the task's logon trigger, run elevated. The switch then shows the machine again.
    pub fn toggle_start_with_windows(self: &Rc<Self>, on: bool) {
        // An administrator's task not read yet: the form (and so the runner) isn't known.
        if !self.io.install.installed || (self.io.admin_member && !self.task_known.get()) {
            self.show();
            return;
        }
        let action = if on { Action::StartupOn } else { Action::StartupOff };
        let failure = |e: &ActionError| e.message("Start with Windows", false);
        if self.admin_mode() {
            self.run_elevated(action, failure, |_| {});
        } else {
            self.run_command(Runner::Here, action, failure, |_| {});
        }
    }

    /// "Open Windows Task Manager" (Settings, the tray): with a Negotium redirect set, the
    /// elevated open-task-manager lifts it while Task Manager starts; otherwise Task Manager just
    /// starts.
    pub fn open_task_manager(self: &Rc<Self>) {
        if let Some(exe) = self.redirect_lifter() {
            let failure = |e: &ActionError| {
                format!("Couldn't open Windows Task Manager: {}.", e.to_string().trim_end_matches('.'))
            };
            self.run_command(Runner::Elevated(exe), Action::OpenTaskManager, failure, |_| {});
        } else if !self.busy.get() {
            (self.io.start_task_manager)();
        }
    }

    fn run_elevated(
        self: &Rc<Self>,
        action: Action,
        failure: fn(&ActionError) -> String,
        succeeded: impl FnOnce(&Rc<Self>) + 'static,
    ) {
        self.run_command(Runner::Elevated(self.io.install.exe.clone()), action, failure, succeeded);
    }

    /// Runs `action` (elevated as an exe, or here); the switches show the machine again once it
    /// returns (and again if an elevated one outlasted the wait, once it exits), and `succeeded`
    /// follows a success.
    fn run_command(
        self: &Rc<Self>,
        runner: Runner,
        action: Action,
        failure: fn(&ActionError) -> String,
        succeeded: impl FnOnce(&Rc<Self>) + 'static,
    ) {
        if self.busy.replace(true) {
            self.show();
            return;
        }
        let this = self.clone();
        let done: ElevatedDone = Box::new(move |result| {
            this.busy.set(false);
            this.show();
            match result {
                Ok(()) => succeeded(&this),
                Err(ActionError::Cancelled) => {}
                Err(e) => {
                    if let Some(ui) = this.weak.upgrade() {
                        ui.set_toast(failure(&e).into());
                    }
                }
            }
        });
        match runner {
            Runner::Elevated(exe) => {
                let owner = self.weak.upgrade().and_then(|ui| platform::main_hwnd(&ui)).map_or(0, |h| h.0 as isize);
                let late = self.clone();
                (self.io.elevate)(exe, action.to_args(), owner, done, Box::new(move || late.show()));
            }
            Runner::Here => (self.io.run_here)(action, done),
        }
    }
}

/// Where [`Integrations::run_command`] runs its action.
enum Runner {
    /// As this exe, elevated (a UAC prompt).
    Elevated(PathBuf),
    /// In this process, without elevation.
    Here,
}

/// A handler waiting for its value, type-erased ([`pending`]).
type Waiting = Box<dyn FnOnce(Box<dyn std::any::Any>)>;

thread_local! {
    /// Handlers waiting on the UI thread for work done elsewhere ([`Pending`]), by id.
    static WAITING: RefCell<std::collections::HashMap<u64, Waiting>> =
        RefCell::new(std::collections::HashMap::new());
    static NEXT_WAITING: Cell<u64> = const { Cell::new(0) };
}

/// A handler waiting on the UI thread for a `T` from another thread. [`Pending::deliver`] (from
/// any thread) runs it on the UI thread; dropped undelivered, it is forgotten there.
pub struct Pending<T> {
    id: Option<u64>,
    _value: std::marker::PhantomData<fn(T)>,
}

/// Registers `done` (on the UI thread) for a value another thread will deliver.
pub fn pending<T: Send + 'static>(done: impl FnOnce(T) + 'static) -> Pending<T> {
    let id = NEXT_WAITING.with(|n| n.replace(n.get() + 1));
    let handler: Box<dyn FnOnce(Box<dyn std::any::Any>)> = Box::new(move |value| {
        if let Ok(value) = value.downcast::<T>() {
            done(*value);
        }
    });
    WAITING.with(|w| w.borrow_mut().insert(id, handler));
    Pending { id: Some(id), _value: std::marker::PhantomData }
}

impl<T: Send + 'static> Pending<T> {
    /// Hands `value` to the handler on the UI thread. Fails only once the event loop is gone
    /// (quitting): nothing left to show.
    pub fn deliver(mut self, value: T) {
        if let Some(id) = self.id.take() {
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(done) = WAITING.with(|w| w.borrow_mut().remove(&id)) {
                    done(Box::new(value));
                }
            });
        }
    }
}

impl<T> Drop for Pending<T> {
    fn drop(&mut self) {
        if let Some(id) = self.id.take() {
            let _ = slint::invoke_from_event_loop(move || {
                WAITING.with(|w| w.borrow_mut().remove(&id));
            });
        }
    }
}

/// `work` on its own thread, then `done` with its result on the UI thread. Called on the UI
/// thread.
pub fn in_background<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static, done: impl FnOnce(T) + 'static) {
    let done = pending(done);
    std::thread::spawn(move || done.deliver(work()));
}

/// The app's [`Integration::read_task`]: user `sid`'s task (no SID: missing) from Task
/// Scheduler on its own thread, so a slow or hung Schedule service never holds up the window.
pub fn read_task_in_background(sid: Option<String>, done: TaskRead) {
    use crate::integrate::System;
    in_background(
        move || match sid {
            Some(sid) => {
                let (state, logon) = System::new().read_task(&sid);
                AdminTask { state, logon }
            }
            None => AdminTask { state: TaskState::Missing, logon: Some(false) },
        },
        done,
    );
}

/// The app's [`Integration::run_here`]: `action` through `integrate::run_unelevated` (the Run
/// value only) as this exe, for user `sid`, on its own thread, then `done` on the UI thread. A
/// failure is logged. Called on the UI thread; never by tests.
pub fn run_here_in_background(sid: Option<String>, action: Action, done: ElevatedDone) {
    in_background(
        move || {
            let machine = crate::integrate::System::new();
            let ctx = crate::integrate::Ctx {
                exe: std::env::current_exe().unwrap_or_default(),
                program_files: install::program_files(),
                user_sid: sid.unwrap_or_default(),
                machine: &machine,
                start_task_manager: &crate::integrate::start_task_manager,
            };
            crate::integrate::run_unelevated(action, &ctx).map_err(|e| {
                let command = action.to_args().join(" ");
                crate::log::Log::new(crate::log::Log::default_path()).warn(&format!("{command} failed: {e}"));
                ActionError::Other(e.to_string())
            })
        },
        done,
    );
}

/// The app's [`Integration::elevate`]: `negotium_actions::run_elevated_exe` on its own thread (it
/// waits for the UAC prompt and, bounded, the command), then `done` on the UI thread; `late` too
/// if the command outlasts the wait and exits later. A failure is logged (the command logs its
/// own reason). Called on the UI thread; never by tests.
pub fn elevate_in_background(exe: PathBuf, args: Vec<String>, owner: isize, done: ElevatedDone, late: LateExit) {
    let late = pending(move |()| late());
    in_background(
        move || {
            let command = args.join(" ");
            let result = negotium_actions::run_elevated_exe(&exe, &command, owner, move || late.deliver(()));
            if let Err(e) = &result
                && !matches!(e, ActionError::Cancelled)
            {
                crate::log::Log::new(crate::log::Log::default_path()).warn(&format!("{command} failed: {e}"));
            }
            result
        },
        done,
    );
}

/// The app's [`Integration::start_task_manager`]: Windows' own Task Manager through the shell (so
/// its "highest available" manifest gets its UAC prompt), on its own thread. A failure is logged.
/// Never called by tests.
pub fn start_task_manager_in_background() {
    std::thread::spawn(|| {
        let started = crate::integrate::task_manager_path()
            .ok_or_else(|| ActionError::Other("System32 could not be found".into()))
            .and_then(|path| negotium_actions::shell_open(&path, 0));
        if let Err(e) = started
            && !matches!(e, ActionError::Cancelled)
        {
            crate::log::Log::new(crate::log::Log::default_path())
                .warn(&format!("Windows Task Manager could not be started: {e}"));
        }
    });
}

impl SettingsWiring {
    /// Shows the GPU switch's launch state and connects the page's callbacks. Returns the
    /// Ctrl+Shift+Esc and Task Manager handlers, which the tray uses too.
    pub fn install(self, ui: &MainWindow) -> Rc<Integrations> {
        ui.set_settings_gpu_fallback(self.fallback_recorded);
        ui.set_settings_gpu_running(self.running == Renderer::Gpu);
        ui.set_settings_gpu_env_override(self.running == Renderer::FromEnv);
        show_install(ui, &self.integration.install);
        let integrations = Rc::new(Integrations {
            io: self.integration,
            settings: self.settings.clone(),
            saver: self.saver.clone(),
            weak: ui.as_weak(),
            busy: Cell::new(false),
            admin_state: Cell::new(0),
            task_logon: Cell::new(None),
            task_known: Cell::new(false),
            reading: Cell::new(false),
            read_again: Cell::new(false),
        });
        integrations.show();
        {
            let integrations = integrations.clone();
            ui.on_hotkey_toggled(move |on| integrations.toggle_hotkey(on));
        }
        {
            let integrations = integrations.clone();
            ui.on_open_windows_task_manager(move || integrations.open_task_manager());
        }
        {
            let integrations = integrations.clone();
            ui.on_admin_mode_toggled(move |on| integrations.toggle_admin_mode(on));
        }
        {
            let integrations = integrations.clone();
            ui.on_start_with_windows_toggled(move |on| integrations.toggle_start_with_windows(on));
        }
        // "Check for updates now" is the updater's (`crate::update::Updater`).
        let change = Rc::new(Change { settings: self.settings, saver: self.saver, weak: ui.as_weak() });
        {
            let change = change.clone();
            ui.on_check_updates_toggled(move |on| {
                change.apply(|s| s.check_updates = on);
            });
        }
        let controls = self.controls;
        {
            let (change, controls) = (change.clone(), controls.clone());
            ui.on_refresh_chosen(move |i| {
                let ms = refresh_from_index(i);
                change.apply(|s| s.refresh_ms = ms);
                controls.set_refresh_ms(ms);
            });
        }
        {
            let change = change.clone();
            ui.on_theme_chosen(move |i| {
                let theme = ThemeChoice::from_index(i);
                if let Some(ui) = change.apply(|s| s.theme = theme) {
                    platform::apply_theme(&ui, theme);
                }
            });
        }
        {
            let change = change.clone();
            ui.on_backdrop_chosen(move |i| {
                let backdrop = BackdropChoice::from_index(i);
                if let Some(ui) = change.apply(|s| s.backdrop = backdrop) {
                    platform::set_backdrop(&ui, backdrop);
                }
            });
        }
        {
            let change = change.clone();
            ui.on_always_on_top_toggled(move |on| {
                change.apply(|s| s.always_on_top = on);
            });
        }
        {
            let change = change.clone();
            ui.on_tray_toggled(move |on| {
                change.apply(|s| s.keep_in_tray = on);
            });
        }
        {
            let change = change.clone();
            ui.on_smooth_graphs_toggled(move |on| {
                change.apply(|s| s.smooth_graphs = on);
            });
        }
        {
            let (change, controls) = (change.clone(), controls.clone());
            ui.on_group_by_type_toggled(move |on| {
                change.apply(|s| s.group_by_type = on);
                controls.set_grouped(on);
            });
        }
        {
            let change = change.clone();
            ui.on_toggle_column(move |col| {
                change.apply(|s| match col {
                    0 => s.col_pid = !s.col_pid,
                    1 => s.col_proc_name = !s.col_proc_name,
                    2 => s.col_status = !s.col_status,
                    3 => s.col_threads = !s.col_threads,
                    4 => s.col_handles = !s.col_handles,
                    5 => s.col_publisher = !s.col_publisher,
                    _ => {}
                });
            });
        }
        // The meters and the column headers are one choice (spec "Meters and headers stay in
        // step"): a meter sorts by its metric, a metric header selects its meter.
        {
            let (change, controls) = (change.clone(), controls.clone());
            ui.on_select_metric(move |i| {
                let metric = Metric::ALL[(i.max(0) as usize).min(Metric::ALL.len() - 1)];
                controls.set_metric(metric);
                let sort = Sort::by(SortColumn::Metric(metric));
                controls.set_sort(sort);
                if let Some(ui) = change.apply(|s| s.set_sort(sort)) {
                    ui.set_selected_metric(i);
                }
            });
        }
        {
            let (change, controls) = (change.clone(), controls.clone());
            ui.on_sort_clicked(move |c| {
                let sort = header_click(controls.sort(), sort_column_from_index(c));
                controls.set_sort(sort);
                if let SortColumn::Metric(m) = sort.column {
                    controls.set_metric(m);
                }
                if let Some(ui) = change.apply(|s| s.set_sort(sort))
                    && let SortColumn::Metric(m) = sort.column
                {
                    ui.set_selected_metric(m as i32);
                }
            });
        }
        {
            let (change, controls, told) = (change.clone(), controls.clone(), self.tray_cpu_changed.clone());
            ui.on_tray_cpu_toggled(move |on| {
                change.apply(|s| s.tray_cpu = on);
                controls.set_tray_cpu(on);
                told(on);
            });
        }
        {
            let (change, told) = (change.clone(), self.picker_shortcut_changed.clone());
            ui.on_picker_shortcut_toggled(move |on| {
                // Saved even when another program holds the keys: the choice stays on.
                change.apply(|s| s.picker_shortcut = on);
                let registered = told(on);
                change.weak.upgrade().inspect(|ui| ui.set_settings_shortcut_taken(on && !registered));
            });
        }
        {
            let marker = self.gpu_marker;
            ui.on_gpu_rendering_toggled(move |on| {
                change.apply(|s| s.gpu_rendering = on);
                if !on {
                    return; // the saved choice is all it takes (`renderer::set_gpu_rendering`)
                }
                // Turning it on clears the fallback marker, so the next launch tries the GPU. The
                // explanation goes now; clearing is a file operation, so it runs off the UI
                // thread, and if it fails the explanation comes back (`marker_not_cleared`). The
                // saver counts it, so an exit or a restart waits for it (`Saver::flush`).
                change.weak.upgrade().inspect(|ui| ui.set_settings_gpu_fallback(false));
                // Written as the settings are (`Saver::writes`).
                let (marker, weak, writes) = (marker.clone(), change.weak.clone(), change.saver.writes().clone());
                drop(change.saver.run(move || {
                    if let Err(e) = renderer::set_gpu_rendering_with(&writes, &marker, on) {
                        crate::log::Log::new(crate::log::Log::default_path())
                            .warn(&format!("GPU rendering fallback marker not cleared: {e}"));
                        let _ = weak.upgrade_in_event_loop(|ui| marker_not_cleared(&ui));
                    }
                }));
            });
        }
        integrations
    }
}

/// The fallback marker could not be cleared: the next launch still draws with the CPU, so the
/// switch shows off again with its explanation.
pub fn marker_not_cleared(ui: &MainWindow) {
    ui.set_settings_gpu_fallback(true);
}

/// One settings change: update the shared settings, show them, save them.
struct Change {
    settings: Rc<RefCell<Settings>>,
    saver: Saver,
    weak: slint::Weak<MainWindow>,
}

impl Change {
    /// Returns the window (if it still exists) for any further effect of the change.
    fn apply(&self, f: impl FnOnce(&mut Settings)) -> Option<MainWindow> {
        f(&mut self.settings.borrow_mut());
        let s = self.settings.borrow().clone();
        let ui = self.weak.upgrade();
        if let Some(ui) = &ui {
            show_settings(ui, &s);
        }
        // The writer thread finishes on its own; overlapping saves keep the newest settings.
        drop(self.saver.save(s));
        ui
    }
}
