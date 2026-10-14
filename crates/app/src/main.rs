#![windows_subsystem = "windows"]

use negotium::bridge::Bridge;
use negotium::controls::Controls;
use negotium::{MainWindow, NegotiumTray, worker};
use negotium_model::Metric;
use slint::ComponentHandle;
use std::sync::Arc;

fn main() -> Result<(), slint::PlatformError> {
    let ui = MainWindow::new()?;
    let tray = NegotiumTray::new()?;
    let controls = Arc::new(Controls::default());
    let bridge = Bridge::install(&ui, controls.clone());

    // Worker -> UI thread. The sink must never panic, so the Result is ignored.
    let weak = ui.as_weak();
    let sink_ui = weak.clone();
    let _worker = worker::spawn(controls.clone(), move |frame| {
        let weak = sink_ui.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = weak.upgrade() {
                negotium::bridge::with_bridge(|b| b.apply(&ui, frame));
            }
        });
    });
    negotium::bridge::set_current(bridge.clone());

    {
        let controls = controls.clone();
        let weak = weak.clone();
        ui.on_select_metric(move |i| {
            controls.set_metric(Metric::ALL[(i.max(0) as usize).min(4)]);
            if let Some(ui) = weak.upgrade() {
                ui.set_selected_metric(i);
            }
        });
    }
    {
        let controls = controls.clone();
        ui.on_list_hover(move |h| controls.set_frozen(h));
    }
    {
        let b = bridge.clone();
        ui.on_search_changed(move |text| {
            b.borrow_mut().search = text.to_string();
            b.borrow().refresh_rows();
        });
    }
    {
        let b = bridge.clone();
        ui.on_toggle_row(move |id| {
            {
                let mut b = b.borrow_mut();
                if !b.expanded.remove(&id) {
                    b.expanded.insert(id);
                }
            }
            b.borrow().refresh_rows();
        });
    }
    {
        let b = bridge.clone();
        ui.on_open_tile(move |id| {
            {
                let mut b = b.borrow_mut();
                b.search.clear();
                b.expanded.insert(id);
            }
            b.borrow().refresh_rows();
        });
    }
    {
        let b = bridge.clone();
        ui.on_dismiss_banner(move || b.borrow_mut().dismiss_banner());
    }

    // Close hides to tray (spec 5.5); the worker slows to 2 s while hidden.
    {
        let controls = controls.clone();
        ui.window().on_close_requested(move || {
            controls.set_visible(false);
            slint::CloseRequestResponse::HideWindow
        });
    }
    {
        let controls = controls.clone();
        let weak = weak.clone();
        tray.on_open(move || {
            controls.set_visible(true);
            if let Some(ui) = weak.upgrade() {
                let _ = ui.show();
            }
        });
    }
    {
        let controls = controls.clone();
        tray.on_exit(move || {
            controls.request_stop();
            let _ = slint::quit_event_loop();
        });
    }

    ui.show()?;
    tray.show()?;
    slint::run_event_loop_until_quit()
}
