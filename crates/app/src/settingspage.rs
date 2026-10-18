//! The Settings page's Rust side: shows the settings on the window and acts on the page's
//! choices. Each choice applies at once, the page shows the new state, and the file is saved off
//! the UI thread (`settings::Saver`).
use crate::controls::{Controls, header_click, sort_column_from_index, sort_column_index};
use negotium_model::{Metric, Sort, SortColumn};
use crate::install::{self, Install};
use crate::settings::{Saver, Settings, ThemeChoice, refresh_from_index};
use crate::renderer::{self, Renderer};
use crate::{MainWindow, platform};
use slint::ComponentHandle;
use std::cell::RefCell;
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
}

impl SettingsWiring {
    /// Shows the GPU switch's launch state and connects the page's callbacks.
    pub fn install(self, ui: &MainWindow) {
        ui.set_settings_gpu_fallback(self.fallback_recorded);
        ui.set_settings_gpu_running(self.running == Renderer::Gpu);
        ui.set_settings_gpu_env_override(self.running == Renderer::FromEnv);
        show_install(ui, &install::current());
        // Filled in by the tasks that implement them; until then the page only asks.
        ui.on_admin_mode_toggled(|_| {});
        ui.on_hotkey_toggled(|_| {});
        ui.on_start_with_windows_toggled(|_| {});
        ui.on_check_updates_toggled(|_| {});
        ui.on_check_updates_now(|| {});
        ui.on_open_windows_task_manager(|| {});
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
