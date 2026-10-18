//! The Settings page's Rust side: shows the settings on the window and acts on the page's
//! choices. Each choice applies at once, the page shows the new state, and the file is saved off
//! the UI thread (`settings::Saver`).
use crate::controls::{Controls, header_click, sort_column_from_index, sort_column_index};
use negotium_model::{Metric, Sort, SortColumn};
use crate::install::{self, Install};
use crate::integrate::Action;
use crate::settings::{Saver, Settings, ThemeChoice, refresh_from_index};
use crate::renderer::{self, Renderer};
use crate::{MainWindow, platform};
use slint::ComponentHandle;
use negotium_actions::ActionError;
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
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
    /// This copy, and what the Ctrl+Shift+Esc switch and "Open Windows Task Manager" act on.
    pub integration: Integration,
}

/// What the Ctrl+Shift+Esc switch and "Open Windows Task Manager" read and run. The app passes
/// the registry and the elevated `--integrate` command (`main.rs`); tests pass fakes.
pub struct Integration {
    /// This copy (`install::current()` in the app).
    pub install: Install,
    /// The exe taskmgr.exe's redirect names (`integrate::Machine::ifeo`).
    pub redirect: Box<dyn Fn() -> Option<PathBuf>>,
    /// Runs this exe with the arguments elevated (one UAC prompt; the `isize` is the window's
    /// HWND, 0 for none) off the UI thread; the outcome comes back on the UI thread
    /// ([`elevate_in_background`] in the app).
    pub elevate: Box<dyn Fn(Vec<String>, isize, ElevatedDone)>,
    /// Starts Windows' own Task Manager when no redirect of ours is in the way
    /// ([`start_task_manager_in_background`] in the app).
    pub start_task_manager: Box<dyn Fn()>,
    /// Told whether the redirect is ours each time it is read (the tray offers "Open Windows Task
    /// Manager" only then).
    pub redirect_shown: Box<dyn Fn(bool)>,
}

/// An elevated command's outcome, delivered on the UI thread.
pub type ElevatedDone = Box<dyn FnOnce(Result<(), ActionError>)>;

/// The Ctrl+Shift+Esc switch and "Open Windows Task Manager", wired. One elevated command at a
/// time: a click while a UAC prompt (or its command) is still open does nothing.
pub struct Integrations {
    io: Integration,
    weak: slint::Weak<MainWindow>,
    busy: Cell<bool>,
}

impl Integrations {
    /// Whether the redirect names this copy; only the installed copy's redirect counts.
    fn redirect_is_ours(&self) -> bool {
        let install = &self.io.install;
        install.installed && (self.io.redirect)().is_some_and(|exe| crate::integrate::same_path(&exe, &install.exe))
    }

    /// Reads the redirect again and shows it: the switch, and the tray menu.
    pub fn show(&self) {
        let ours = self.redirect_is_ours();
        if let Some(ui) = self.weak.upgrade() {
            ui.set_settings_hotkey(ours);
        }
        (self.io.redirect_shown)(ours);
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
        self.run_elevated(action, |e| e.message("Open on Ctrl+Shift+Esc", false));
    }

    /// "Open Windows Task Manager" (Settings, the tray): with our redirect set, the elevated
    /// open-task-manager lifts it while Task Manager starts; otherwise Task Manager just starts.
    pub fn open_task_manager(self: &Rc<Self>) {
        if self.redirect_is_ours() {
            self.run_elevated(Action::OpenTaskManager, |e| {
                format!("Couldn't open Windows Task Manager: {}.", e.to_string().trim_end_matches('.'))
            });
        } else if !self.busy.get() {
            (self.io.start_task_manager)();
        }
    }

    fn run_elevated(self: &Rc<Self>, action: Action, failure: fn(&ActionError) -> String) {
        if self.busy.replace(true) {
            self.show();
            return;
        }
        let owner = self.weak.upgrade().and_then(|ui| platform::main_hwnd(&ui)).map_or(0, |h| h.0 as isize);
        let this = self.clone();
        (self.io.elevate)(
            action.to_args(),
            owner,
            Box::new(move |result| {
                this.busy.set(false);
                this.show();
                if let Err(e) = result
                    && !matches!(e, ActionError::Cancelled)
                    && let Some(ui) = this.weak.upgrade()
                {
                    ui.set_toast(failure(&e).into());
                }
            }),
        );
    }
}

thread_local! {
    /// The elevated command's outcome handler, waiting on the UI thread (`Integrations` runs one
    /// command at a time).
    static ELEVATED_DONE: RefCell<Option<ElevatedDone>> = const { RefCell::new(None) };
}

/// The app's [`Integration::elevate`]: `negotium_actions::run_elevated` on its own thread (it
/// waits for the UAC prompt and the command), then `done` on the UI thread. A failure is logged
/// (the command logs its own reason). Called on the UI thread; never by tests.
pub fn elevate_in_background(args: Vec<String>, owner: isize, done: ElevatedDone) {
    ELEVATED_DONE.with(|d| *d.borrow_mut() = Some(done));
    std::thread::spawn(move || {
        let command = args.join(" ");
        let result = negotium_actions::run_elevated(&command, owner);
        if let Err(e) = &result
            && !matches!(e, ActionError::Cancelled)
        {
            crate::log::Log::new(crate::log::Log::default_path()).warn(&format!("{command} failed: {e}"));
        }
        // Fails only once the event loop is gone (quitting): nothing left to show.
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(done) = ELEVATED_DONE.with(|d| d.borrow_mut().take()) {
                done(result);
            }
        });
    });
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
        let integrations = Rc::new(Integrations { io: self.integration, weak: ui.as_weak(), busy: Cell::new(false) });
        integrations.show();
        {
            let integrations = integrations.clone();
            ui.on_hotkey_toggled(move |on| integrations.toggle_hotkey(on));
        }
        {
            let integrations = integrations.clone();
            ui.on_open_windows_task_manager(move || integrations.open_task_manager());
        }
        // Filled in by the tasks that implement them; until then the page only asks.
        ui.on_admin_mode_toggled(|_| {});
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
