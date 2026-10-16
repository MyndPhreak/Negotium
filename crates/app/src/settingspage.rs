//! The Settings page's Rust side: shows the settings on the window and acts on the page's
//! choices. Each choice applies at once, the page shows the new state, and the file is saved off
//! the UI thread (`settings::Saver`).
use crate::controls::Controls;
use crate::settings::{Saver, Settings, ThemeChoice, refresh_from_index};
use crate::{MainWindow, platform, renderer};
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
}

impl SettingsWiring {
    /// Shows the GPU switch's launch state and connects the page's callbacks.
    pub fn install(self, ui: &MainWindow) {
        ui.set_settings_gpu_fallback(self.fallback_recorded);
        ui.set_settings_gpu_at_launch(self.settings.borrow().gpu_rendering && !self.fallback_recorded);
        let change = Rc::new(Change { settings: self.settings, saver: self.saver, weak: ui.as_weak() });
        {
            let (change, controls) = (change.clone(), self.controls);
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
            let marker = self.gpu_marker;
            ui.on_gpu_rendering_toggled(move |on| {
                if let Some(ui) = change.apply(|s| s.gpu_rendering = on)
                    && on
                {
                    // Turning it on clears the fallback marker: the next launch tries the GPU.
                    ui.set_settings_gpu_fallback(false);
                }
                // A file operation: off the UI thread. If it fails, the next launch shows the
                // fallback again, so the page never claims more than happened.
                let marker = marker.clone();
                std::thread::spawn(move || {
                    if let Err(e) = renderer::set_gpu_rendering_at(&marker, on) {
                        crate::log::Log::new(crate::log::Log::default_path())
                            .warn(&format!("GPU rendering fallback marker not cleared: {e}"));
                    }
                });
            });
        }
    }
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
