#![windows_subsystem = "windows"]

use negotium::bridge::Bridge;
use negotium::controls::Controls;
use negotium::{MainWindow, NegotiumTray, worker};
use negotium_model::Metric;
use slint::ComponentHandle;
use std::sync::Arc;

fn main() -> Result<(), slint::PlatformError> {
    let ui = MainWindow::new()?;
    if let Some((r, g, b)) = negotium::platform::read_accent() {
        ui.global::<negotium::Theme>().set_accent(slint::Color::from_rgb_u8(r, g, b));
    }
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
            controls.set_metric(Metric::ALL[(i.max(0) as usize).min(Metric::ALL.len() - 1)]);
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
                b.search.clear(); // the box itself is emptied by main.slint in the same click
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
            // A hidden window cannot report pointer-leave, so release the hover freeze here.
            controls.set_frozen(false);
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

    // Row actions. The UI thread only plans; every job runs on its own thread.
    let elevated = negotium_actions::is_elevated();
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
                            ui.set_details_text(details.into());
                            ui.set_details_visible(true);
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
                        ui.set_confirm_text(text.into());
                        ui.set_confirm_visible(true);
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
            if negotium_actions::restart_as_admin("").is_ok() {
                let _ = slint::quit_event_loop();
            } else if let Some(ui) = weak.upgrade() {
                ui.set_toast("Restart as admin was cancelled.".into());
            }
        });
    }

    ui.show()?;
    // The native HWND is created lazily once the event loop runs (and the GL renderer can take a
    // while to bring the window up), so poll until it exists, apply the chrome once, then stop.
    let chrome_timer = std::rc::Rc::new(slint::Timer::default());
    {
        let weak = weak.clone();
        let handle = std::rc::Rc::downgrade(&chrome_timer);
        chrome_timer.start(slint::TimerMode::Repeated, std::time::Duration::from_millis(50), move || {
            let Some(ui) = weak.upgrade() else { return };
            if !negotium::platform::has_window_handle(&ui) {
                return;
            }
            negotium::platform::apply_window_chrome(&ui);
            if let Some(t) = handle.upgrade() {
                t.stop();
            }
        });
    }
    tray.show()?;
    slint::run_event_loop_until_quit()
}
