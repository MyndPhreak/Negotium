//! The Settings page's Rust side: shows the settings on the window and acts on the page's
//! choices. Each choice applies at once, the page shows the new state, and the file is saved off
//! the UI thread (`settings::Saver`).
use crate::controls::{Controls, header_click, sort_column_from_index, sort_column_index};
use negotium_model::{Metric, Sort, SortColumn};
use crate::install::{self, Install};
use crate::controls::Page;
use crate::integrate::{Action, TaskState};
use crate::settings::{Saver, Settings, ThemeChoice, refresh_from_index};
use crate::renderer::{self, Renderer};
use crate::{MainWindow, platform};
use slint::ComponentHandle;
use negotium_actions::ActionError;
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

/// Applies `s` to the window: the theme (colours, title bar, Mica), always on top, and what the
/// Settings page shows. The refresh rate lives in `Controls` (`Controls::set_refresh_ms`).
pub fn apply_settings(ui: &MainWindow, s: &Settings) {
    platform::apply_theme(ui, s.theme);
    show_settings(ui, s);
}

/// The window state that is only a property: always on top and the page's choices and switches.
fn show_settings(ui: &MainWindow, s: &Settings) {
    ui.set_on_top(s.always_on_top);
    ui.set_settings_refresh(s.refresh_index());
    ui.set_settings_theme(s.theme.index());
    ui.set_settings_keep_in_tray(s.keep_in_tray);
    ui.set_settings_gpu_rendering(s.gpu_rendering);
    ui.set_settings_smooth_graphs(s.smooth_graphs);
    ui.set_settings_tray_cpu(s.tray_cpu);
    ui.set_settings_group_by_type(s.group_by_type);
    ui.set_settings_check_updates(s.check_updates);
    ui.set_settings_start_with_windows(s.start_with_windows);
    ui.set_sort_column(sort_column_index(s.sort().column));
    ui.set_sort_descending(s.sort().descending);
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
    /// The user is in Administrators (`integration::is_admin_member`): admin mode is offered
    /// only then.
    pub admin_member: bool,
    /// Reads this user's admin task (`integrate::Machine::task_state`, a Task Scheduler call)
    /// off the UI thread and hands the state back on it ([`read_task_in_background`] in the
    /// app); asked only for the installed copy of an administrator.
    pub read_task: Box<dyn Fn(TaskRead)>,
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
/// The admin task's state, delivered on the UI thread.
pub type TaskRead = Box<dyn FnOnce(TaskState)>;

/// The Ctrl+Shift+Esc and admin mode switches and "Open Windows Task Manager", wired. One
/// elevated command at a time: a click while a UAC prompt (or its command) is still open does
/// nothing.
pub struct Integrations {
    io: Integration,
    /// Start with Windows decides whether admin mode's task also starts at sign-in.
    settings: Rc<RefCell<Settings>>,
    weak: slint::Weak<MainWindow>,
    busy: Cell<bool>,
    /// The admin mode row's last known state, shown while Task Scheduler is read again.
    admin_state: Cell<i32>,
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

    /// Reads the machine again and shows it: the switches, and the tray menu. The redirect is
    /// one registry value, read here; the admin task is read off the UI thread, and the row
    /// shows its last known state until the answer comes.
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
            this.admin_state.set(admin_mode_state(true, &task));
            if let Some(ui) = this.weak.upgrade() {
                ui.set_settings_admin_mode_state(this.admin_state.get());
            }
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
        self.run_elevated(action, |e| e.message("Admin mode", false), move |this| {
            if restart {
                (this.io.restart_elevated)();
            }
        });
    }

    /// "Open Windows Task Manager" (Settings, the tray): with a Negotium redirect set, the
    /// elevated open-task-manager lifts it while Task Manager starts; otherwise Task Manager just
    /// starts.
    pub fn open_task_manager(self: &Rc<Self>) {
        if let Some(exe) = self.redirect_lifter() {
            let failure = |e: &ActionError| {
                format!("Couldn't open Windows Task Manager: {}.", e.to_string().trim_end_matches('.'))
            };
            self.run_elevated_as(exe, Action::OpenTaskManager, failure, |_| {});
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
        self.run_elevated_as(self.io.install.exe.clone(), action, failure, succeeded);
    }

    /// Runs `action` as `exe`, elevated; the switches show the machine again once it returns
    /// (and again if it outlasted the wait, once it exits), and `succeeded` follows a success.
    fn run_elevated_as(
        self: &Rc<Self>,
        exe: PathBuf,
        action: Action,
        failure: fn(&ActionError) -> String,
        succeeded: impl FnOnce(&Rc<Self>) + 'static,
    ) {
        if self.busy.replace(true) {
            self.show();
            return;
        }
        let owner = self.weak.upgrade().and_then(|ui| platform::main_hwnd(&ui)).map_or(0, |h| h.0 as isize);
        let (this, late) = (self.clone(), self.clone());
        (self.io.elevate)(
            exe,
            action.to_args(),
            owner,
            Box::new(move |result| {
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
            }),
            Box::new(move || late.show()),
        );
    }
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
    in_background(
        move || match sid {
            Some(sid) => crate::integrate::Machine::task_state(&crate::integrate::System::new(), &sid),
            None => TaskState::Missing,
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
            weak: ui.as_weak(),
            busy: Cell::new(false),
            admin_state: Cell::new(0),
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
        // Filled in by the tasks that implement them; until then the page only asks.
        ui.on_start_with_windows_toggled(|_| {});
        ui.on_check_updates_toggled(|_| {});
        ui.on_check_updates_now(|| {});
        let change = Rc::new(Change { settings: self.settings, saver: self.saver, weak: ui.as_weak() });
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
                let (marker, weak) = (marker.clone(), change.weak.clone());
                drop(change.saver.run(move || {
                    if let Err(e) = renderer::set_gpu_rendering_at(&marker, on) {
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
