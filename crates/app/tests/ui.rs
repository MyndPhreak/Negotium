use i_slint_backend_testing::ElementHandle;
use negotium::{MainWindow, MeterData, RowData};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::cell::RefCell;
use std::rc::Rc;

fn window() -> MainWindow {
    i_slint_backend_testing::init_no_event_loop();
    let ui = MainWindow::new().unwrap();
    let meters: Vec<MeterData> = ["CPU", "Memory", "Disk", "GPU", "Network"]
        .iter()
        .map(|l| MeterData { label: (*l).into(), value: "1%".into(), sub: "".into(), spark: "M 0 100".into(), available: true })
        .collect();
    ui.set_meters(ModelRc::new(VecModel::from(meters)));
    ui.set_rows(ModelRc::new(VecModel::from(vec![RowData {
        id: 7, depth: 0, name: "Editor".into(), count: "".into(), cpu: "1.0%".into(), mem: "1.0 MB".into(),
        disk: "0 B/s".into(), gpu: "—".into(), net: "—".into(), bar: 0.5, hung: "Not responding · 9s".into(),
        expandable: false, expanded: false,
    }, RowData {
        id: 9, depth: 0, name: "Browser".into(), count: "(3)".into(), cpu: "2.0%".into(), mem: "2.0 MB".into(),
        disk: "0 B/s".into(), gpu: "—".into(), net: "—".into(), bar: 0.2, hung: "".into(),
        expandable: true, expanded: false,
    }])));
    ui
}

#[test]
fn clicking_a_meter_selects_its_metric() {
    let ui = window();
    let picked = Rc::new(RefCell::new(None));
    let p = picked.clone();
    ui.on_select_metric(move |i| *p.borrow_mut() = Some(i));
    let meter = ElementHandle::find_by_accessible_label(&ui, "Memory meter").next().expect("memory meter");
    meter.invoke_accessible_default_action();
    assert_eq!(*picked.borrow(), Some(1));
}

#[test]
fn hung_rows_offer_force_close() {
    let ui = window();
    let action = Rc::new(RefCell::new(None));
    let a = action.clone();
    ui.on_row_action(move |name, id| *a.borrow_mut() = Some((name.to_string(), id)));
    let button = ElementHandle::find_by_accessible_label(&ui, "Force close Editor").next().expect("force close button");
    button.invoke_accessible_default_action();
    assert_eq!(*action.borrow(), Some(("force-close".to_string(), 7)));
}

#[test]
fn selected_meter_reports_checked() {
    let ui = window();
    ui.set_selected_metric(1);
    let memory = ElementHandle::find_by_accessible_label(&ui, "Memory meter").next().expect("memory meter");
    let cpu = ElementHandle::find_by_accessible_label(&ui, "CPU meter").next().expect("cpu meter");
    assert_eq!(memory.accessible_checkable(), Some(true));
    assert_eq!(memory.accessible_checked(), Some(true));
    assert_eq!(cpu.accessible_checked(), Some(false));
}

#[test]
fn expand_chevron_toggles_its_row() {
    let ui = window();
    let toggled = Rc::new(RefCell::new(None));
    let t = toggled.clone();
    ui.on_toggle_row(move |id| *t.borrow_mut() = Some(id));
    let chevron = ElementHandle::find_by_accessible_label(&ui, "Expand Browser").next().expect("expand chevron");
    chevron.invoke_accessible_default_action();
    assert_eq!(*toggled.borrow(), Some(9));
}

#[test]
fn open_modal_disables_controls_beneath() {
    let ui = window();
    let button = ElementHandle::find_by_accessible_label(&ui, "Force close Editor").next().expect("force close button");
    assert_eq!(button.accessible_enabled(), Some(true));
    ui.set_confirm_visible(true);
    assert!(ui.get_modal_open());
    assert_eq!(button.accessible_enabled(), Some(false));
}

use i_slint_backend_testing::ElementHandle as El;
use slint::platform::WindowEvent;
use slint::{LogicalPosition, PhysicalSize};

fn move_pointer(ui: &MainWindow, x: f32, y: f32) {
    ui.window().dispatch_event(WindowEvent::PointerMoved { position: LogicalPosition::new(x, y) });
}

fn centre(e: &El) -> (f32, f32) {
    let p = e.absolute_position();
    let s = e.size();
    (p.x + s.width / 2.0, p.y + s.height / 2.0)
}

fn shown_window() -> MainWindow {
    let ui = window();
    ui.window().set_size(PhysicalSize::new(1000, 720));
    ui.show().unwrap();
    ui
}

const BROWSER_ROW: &str = "Browser, CPU 2.0%, memory 2.0 MB";

#[test]
fn end_task_button_shows_on_row_hover_and_survives_pointer_moving_onto_it() {
    let ui = shown_window();
    let row = El::find_by_accessible_label(&ui, BROWSER_ROW).next().expect("browser row");
    assert!(El::find_by_accessible_label(&ui, "End task Browser").next().is_none(), "hidden before hover");
    let (rx, ry) = centre(&row);
    move_pointer(&ui, rx, ry);
    let button = El::find_by_accessible_label(&ui, "End task Browser").next().expect("End task appears on row hover");
    // Walk the pointer from the row onto the button in small steps, re-querying each time.
    let (bx, by) = centre(&button);
    for step in 0..=10 {
        let t = step as f32 / 10.0;
        move_pointer(&ui, rx + (bx - rx) * t, ry + (by - ry) * t);
        assert!(El::find_by_accessible_label(&ui, "End task Browser").next().is_some(), "button vanished at step {step}");
    }
    let button = El::find_by_accessible_label(&ui, "End task Browser").next().unwrap();
    let (bx2, by2) = centre(&button);
    move_pointer(&ui, bx2, by2);
    assert!(El::find_by_accessible_label(&ui, "End task Browser").next().is_some(), "button stays with pointer on it");
}

#[test]
fn list_hover_is_released_after_a_modal_opens_and_closes_over_the_list() {
    let ui = shown_window();
    let events = Rc::new(RefCell::new(Vec::new()));
    let e = events.clone();
    ui.on_list_hover(move |h| e.borrow_mut().push(h));
    let row = El::find_by_accessible_label(&ui, BROWSER_ROW).next().expect("browser row");
    let (rx, ry) = centre(&row);
    move_pointer(&ui, rx, ry);
    assert_eq!(events.borrow().last(), Some(&true), "hovering the list reports true");
    ui.set_confirm_visible(true);
    ui.set_confirm_visible(false);
    move_pointer(&ui, rx, 5.0); // off the list: over the meters/padding area
    assert_eq!(events.borrow().last(), Some(&false), "leaving the list reports false: {:?}", events.borrow());
    // The list is not stuck: hovering again and leaving again toggles normally.
    move_pointer(&ui, rx, ry);
    assert_eq!(events.borrow().last(), Some(&true), "re-hover after the modal reports true: {:?}", events.borrow());
    move_pointer(&ui, rx, 5.0);
    assert_eq!(events.borrow().last(), Some(&false), "and leaving reports false again: {:?}", events.borrow());
}
