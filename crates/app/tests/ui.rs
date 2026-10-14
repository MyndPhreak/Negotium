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
        expandable: false, expanded: false, actionable: true,
    }, RowData {
        id: 9, depth: 0, name: "Browser".into(), count: "(3)".into(), cpu: "2.0%".into(), mem: "2.0 MB".into(),
        disk: "0 B/s".into(), gpu: "—".into(), net: "—".into(), bar: 0.2, hung: "".into(),
        expandable: true, expanded: false, actionable: true,
    }, RowData {
        id: 11, depth: 0, name: "Windows".into(), count: "(90)".into(), cpu: "3.0%".into(), mem: "3.0 MB".into(),
        disk: "0 B/s".into(), gpu: "—".into(), net: "—".into(), bar: 0.1, hung: "".into(),
        expandable: true, expanded: false, actionable: false,
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
fn rows_that_are_not_actionable_offer_no_end_task() {
    use slint::Model;
    let ui = window();
    // The test layout only leaves room for two rows: keep just the Windows group and Browser.
    let all = ui.get_rows();
    let keep: Vec<RowData> = [2, 1].iter().map(|&i| all.row_data(i).unwrap()).collect();
    ui.set_rows(ModelRc::new(VecModel::from(keep)));
    ui.window().set_size(PhysicalSize::new(1000, 720));
    ui.show().unwrap();
    let windows = El::find_by_accessible_label(&ui, "Windows, CPU 3.0%, memory 3.0 MB").next().expect("windows row");
    let (wx, wy) = centre(&windows);
    move_pointer(&ui, wx, wy);
    assert!(El::find_by_accessible_label(&ui, "End task Windows").next().is_none(), "no End task on the Windows group");
    // The control stays available on an ordinary row.
    let browser = El::find_by_accessible_label(&ui, BROWSER_ROW).next().expect("browser row");
    let (bx, by) = centre(&browser);
    move_pointer(&ui, bx, by);
    assert!(El::find_by_accessible_label(&ui, "End task Browser").next().is_some());
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
    assert_eq!(*events.borrow(), [true], "hovering the list reports true");
    // The modal opens while the pointer is still over the list: hover must be released.
    ui.set_confirm_visible(true);
    slint::platform::update_timers_and_animations(); // runs `changed` handlers
    assert_eq!(*events.borrow(), [true, false], "opening the modal releases the list hover");
    let after_open = events.borrow().len();
    // Move the pointer onto the modal (its Cancel button) and close it from there.
    let cancel = El::find_by_accessible_label(&ui, "Cancel").next().expect("cancel button");
    let (cx, cy) = centre(&cancel);
    move_pointer(&ui, cx, cy);
    ui.set_confirm_visible(false);
    slint::platform::update_timers_and_animations();
    assert!(events.borrow()[after_open - 1..].iter().all(|h| !h), "no hover reported while the modal covers the list: {:?}", events.borrow());
    // Back over the list and off it again: reports normally, never stuck.
    move_pointer(&ui, rx, ry);
    assert_eq!(events.borrow().last(), Some(&true), "re-hover after the modal: {:?}", events.borrow());
    move_pointer(&ui, rx, 5.0);
    assert_eq!(events.borrow().last(), Some(&false), "leaving reports false again: {:?}", events.borrow());
}

#[test]
fn window_opens_at_its_preferred_width() {
    // The real app creates the window before the first frame arrives, so every model is empty. Empty
    // repeaters in the meters row and top strip once derived a maximum width below min-width, which
    // clamped the window to min-width and made it ignore preferred-width.
    i_slint_backend_testing::init_no_event_loop();
    let ui = MainWindow::new().unwrap();
    ui.show().unwrap();
    let size = ui.window().size().to_logical(ui.window().scale_factor());
    assert!(size.width >= 980.0, "opened {}x{} logical", size.width, size.height);
    assert!(size.height >= 700.0, "opened {}x{} logical", size.width, size.height);
}

#[test]
fn clicking_a_tile_clears_the_search_box() {
    let ui = window();
    ui.set_tiles(ModelRc::new(VecModel::from(vec![negotium::TileData {
        id: 3, name: "Chrome".into(), value: "5%".into(), weight: 1.0, hung: false,
    }])));
    ui.set_search_text("chr".into());
    let tile = ElementHandle::find_by_accessible_label(&ui, "Chrome tile").next().expect("tile");
    tile.invoke_accessible_default_action();
    assert_eq!(ui.get_search_text(), "");
}
