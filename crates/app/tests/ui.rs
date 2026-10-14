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
        expandable: false, expanded: false, actionable: true, exited: false,
    }, RowData {
        id: 9, depth: 0, name: "Browser".into(), count: "(3)".into(), cpu: "2.0%".into(), mem: "2.0 MB".into(),
        disk: "0 B/s".into(), gpu: "—".into(), net: "—".into(), bar: 0.2, hung: "".into(),
        expandable: true, expanded: false, actionable: true, exited: false,
    }, RowData {
        id: 11, depth: 0, name: "Windows".into(), count: "(90)".into(), cpu: "3.0%".into(), mem: "3.0 MB".into(),
        disk: "0 B/s".into(), gpu: "—".into(), net: "—".into(), bar: 0.1, hung: "".into(),
        expandable: true, expanded: false, actionable: false, exited: false,
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

fn press(ui: &MainWindow, x: f32, y: f32, button: slint::platform::PointerEventButton) {
    let position = LogicalPosition::new(x, y);
    ui.window().dispatch_event(WindowEvent::PointerPressed { position, button });
    ui.window().dispatch_event(WindowEvent::PointerReleased { position, button });
}

fn key(ui: &MainWindow, k: slint::platform::Key) {
    let text: slint::SharedString = k.into();
    ui.window().dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
    ui.window().dispatch_event(WindowEvent::KeyReleased { text });
}

#[test]
fn context_menu_acts_on_the_row_it_was_opened_on_even_if_rows_move_underneath() {
    use slint::Model;
    let ui = shown_window();
    let actions = Rc::new(RefCell::new(Vec::new()));
    let a = actions.clone();
    ui.on_row_action(move |name, id| a.borrow_mut().push((name.to_string(), id)));
    let menu = Rc::new(RefCell::new(Vec::new()));
    let m = menu.clone();
    ui.on_menu_open(move |open| m.borrow_mut().push(open));

    // Right-click the Browser row (id 9, second row).
    let row = El::find_by_accessible_label(&ui, BROWSER_ROW).next().expect("browser row");
    let (rx, ry) = centre(&row);
    move_pointer(&ui, rx, ry);
    press(&ui, rx, ry, slint::platform::PointerEventButton::Right);
    assert_eq!(ui.get_menu_target(), 9, "the menu captures the row it opened on");
    // The freeze starts before the menu shows. (Under this popup fallback show() returns at once and
    // the freeze ends; the native Windows menu is modal and holds it until the menu closes.)
    assert_eq!(menu.borrow().first(), Some(&true), "the list is frozen before the menu shows: {:?}", menu.borrow());

    // While the menu is open a frame rewrites the rows by index: Editor now sits where Browser was.
    let rows = ui.get_rows();
    let (editor, browser) = (rows.row_data(0).unwrap(), rows.row_data(1).unwrap());
    rows.set_row_data(0, browser);
    rows.set_row_data(1, editor);

    // Choose "End task" (the first item) from the keyboard.
    key(&ui, slint::platform::Key::DownArrow);
    key(&ui, slint::platform::Key::Return);
    assert_eq!(*actions.borrow(), [("end-task".to_string(), 9)], "acts on the captured row, not the one now at that index");
}

#[test]
fn an_exited_row_is_marked_and_offers_no_actions() {
    use slint::Model;
    let ui = window();
    let rows = ui.get_rows();
    let mut browser = rows.row_data(1).unwrap();
    browser.exited = true;
    browser.actionable = false;
    rows.set_row_data(1, browser);
    ui.window().set_size(PhysicalSize::new(1000, 720));
    ui.show().unwrap();
    let row = El::find_by_accessible_label(&ui, "Browser, exited").next().expect("marked exited");
    let (x, y) = centre(&row);
    move_pointer(&ui, x, y);
    assert!(El::find_by_accessible_label(&ui, "End task Browser").next().is_none());
    let menu = Rc::new(RefCell::new(Vec::new()));
    let m = menu.clone();
    ui.on_menu_open(move |open| m.borrow_mut().push(open));
    press(&ui, x, y, slint::platform::PointerEventButton::Right);
    assert!(menu.borrow().is_empty(), "no context menu on an exited row");
}

// ---- Fix round 2 (R10) ----

use negotium::bridge::Bridge;
use negotium::controls::Controls;
use negotium::view::{Frame, RowView};
use std::sync::Arc;

fn row_view(id: i32, name: &str, hung: &str) -> RowView {
    RowView { id, parent: -1, depth: 0, name: name.into(), hung: hung.into(), cpu: "1.0%".into(), actionable: true, ..Default::default() }
}

fn frame(rows: Vec<RowView>) -> Frame {
    Frame { rows, ..Default::default() }
}

fn shown_ids(ui: &MainWindow) -> Vec<i32> {
    use slint::Model;
    ui.get_rows().iter().map(|r| r.id).collect()
}

fn bridged() -> (MainWindow, Arc<Controls>, Rc<RefCell<Bridge>>) {
    i_slint_backend_testing::init_no_event_loop();
    let ui = MainWindow::new().unwrap();
    let controls = Arc::new(Controls::default());
    let bridge = Bridge::install(&ui, controls.clone());
    (ui, controls, bridge)
}

const HUNG: &str = "Not responding · 9s";

#[test]
fn keyboard_focus_in_a_row_freezes_the_list_like_hover() {
    let (ui, controls, bridge) = bridged();
    let mut b = bridge.borrow_mut();
    b.apply(&ui, frame(vec![row_view(1, "Editor", HUNG), row_view(2, "Browser", "")]));
    b.set_key_focus(1, true); // Editor's Force close has keyboard focus
    assert!(controls.frozen(), "keyboard focus inside a row freezes the list");
    // Another hung app appears and ranks first: Editor's row must not move under the focus.
    b.apply(&ui, frame(vec![row_view(3, "Game", HUNG), row_view(1, "Editor", HUNG), row_view(2, "Browser", "")]));
    assert_eq!(shown_ids(&ui), [1, 2]);
    b.set_key_focus(1, false);
    assert!(!controls.frozen());
    assert_eq!(shown_ids(&ui), [3, 1, 2], "focus left: the list catches up");
}

#[test]
fn keyboard_freeze_ends_when_the_focused_button_goes_away() {
    let (ui, controls, bridge) = bridged();
    let mut b = bridge.borrow_mut();
    b.apply(&ui, frame(vec![row_view(1, "Editor", HUNG), row_view(2, "Browser", "")]));
    b.set_key_focus(1, true);
    assert!(controls.frozen());
    // Editor recovers: its Force close (the focused element) is destroyed without a focus-out.
    b.apply(&ui, frame(vec![row_view(2, "Browser", ""), row_view(1, "Editor", "")]));
    assert!(!controls.frozen(), "a vanished focus target must not hold the list frozen");
    assert_eq!(shown_ids(&ui), [2, 1]);
}

#[test]
fn a_hung_windows_group_auto_expands_only_while_the_list_is_not_frozen() {
    use slint::Model;
    let (ui, _controls, bridge) = bridged();
    let mut group = row_view(5, "Windows", HUNG);
    group.actionable = false;
    group.expandable = true;
    let child = RowView { id: 6, parent: 5, depth: 1, name: "SearchHost.exe".into(), hung: HUNG.into(), actionable: true, ..Default::default() };
    let mut b = bridge.borrow_mut();
    let mut calm = group.clone();
    calm.hung = String::new();
    b.apply(&ui, frame(vec![calm]));
    b.set_hover(true);
    // A member hangs while the pointer is over the list.
    b.apply(&ui, frame(vec![group.clone(), child.clone()]));
    let shown = ui.get_rows().row_data(0).unwrap();
    assert!(!shown.expanded, "no expand while frozen: its children could not appear anyway");
    assert_eq!(shown_ids(&ui), [5]);
    b.set_hover(false);
    assert!(ui.get_rows().row_data(0).unwrap().expanded, "applied as soon as the list unfreezes");
    assert_eq!(shown_ids(&ui), [5, 6]);
}

#[test]
fn keyboard_menu_on_a_focused_row_acts_on_that_row_not_on_a_stale_capture() {
    use slint::platform::Key;
    let ui = shown_window();
    let actions = Rc::new(RefCell::new(Vec::new()));
    let a = actions.clone();
    ui.on_row_action(move |name, id| a.borrow_mut().push((name.to_string(), id)));
    let focus = Rc::new(RefCell::new(Vec::new()));
    let f = focus.clone();
    ui.on_row_key_focus(move |id, has| f.borrow_mut().push((id, has)));

    // A stale capture: Browser's (id 9) menu was opened earlier and dismissed.
    let browser = El::find_by_accessible_label(&ui, BROWSER_ROW).next().expect("browser row");
    let (bx, by) = centre(&browser);
    press(&ui, bx, by, slint::platform::PointerEventButton::Right);
    key(&ui, Key::Escape);
    assert_eq!(ui.get_menu_target(), 9);
    move_pointer(&ui, 5.0, 5.0);

    // Tab to Editor's (id 7) always-visible Force close.
    for _ in 0..40 {
        if focus.borrow().last() == Some(&(7, true)) {
            break;
        }
        key(&ui, Key::Tab);
        slint::platform::update_timers_and_animations(); // runs `changed has-focus`
    }
    assert_eq!(focus.borrow().last(), Some(&(7, true)), "Force close on Editor has keyboard focus: {:?}", focus.borrow());
    actions.borrow_mut().clear();

    // Shift+F10 opens the row's menu from the keyboard; choose End task.
    let shift: slint::SharedString = Key::Shift.into();
    ui.window().dispatch_event(WindowEvent::KeyPressed { text: shift.clone() });
    key(&ui, Key::F10);
    ui.window().dispatch_event(WindowEvent::KeyReleased { text: shift });
    assert_eq!(ui.get_menu_target(), 7, "opening from the keyboard captures the focused row");
    key(&ui, Key::DownArrow);
    key(&ui, Key::Return);
    assert_eq!(*actions.borrow(), [("end-task".to_string(), 7)]);
}

// ---- Fix round 3 (R11) ----

/// A shown window driven by a real Bridge, wired the way main.rs wires it.
type FocusLog = Rc<RefCell<Vec<(i32, bool)>>>;

fn wired(rows: Vec<RowView>) -> (MainWindow, Arc<Controls>, FocusLog) {
    let (ui, controls, bridge) = bridged();
    bridge.borrow_mut().apply(&ui, frame(rows));
    let log: FocusLog = Default::default();
    let (b, l) = (bridge.clone(), log.clone());
    ui.on_row_key_focus(move |id, focused| {
        l.borrow_mut().push((id, focused));
        negotium::bridge::row_focus(&b, id, focused);
    });
    let b = bridge.clone();
    ui.on_list_hover(move |h| b.borrow_mut().set_hover(h));
    let b = bridge.clone();
    ui.on_clear_key_focus(move || b.borrow_mut().clear_key_focus());
    ui.window().set_size(PhysicalSize::new(1000, 720));
    ui.show().unwrap();
    (ui, controls, log)
}

fn tab_until(ui: &MainWindow, done: impl Fn() -> bool) -> bool {
    for _ in 0..40 {
        slint::platform::update_timers_and_animations();
        if done() {
            return true;
        }
        key(ui, slint::platform::Key::Tab);
    }
    slint::platform::update_timers_and_animations();
    done()
}

#[test]
fn a_modal_over_a_focused_force_close_does_not_leave_the_list_frozen() {
    let (ui, controls, log) = wired(vec![row_view(7, "Editor", HUNG), row_view(9, "Browser", "")]);
    assert!(tab_until(&ui, || log.borrow().last() == Some(&(7, true))), "Tab onto Editor's Force close");
    assert!(controls.frozen(), "keyboard focus on it freezes the list");
    // Space on a critical row (or Shift+F10 -> Details) opens a modal over the focused button.
    ui.invoke_show_confirm("End a critical process?".into());
    slint::platform::update_timers_and_animations();
    assert_eq!(log.borrow().last(), Some(&(7, false)), "the button really loses focus (no stale focus ring)");
    ui.set_confirm_visible(false);
    slint::platform::update_timers_and_animations();
    assert!(!controls.frozen(), "closing the modal must not leave the list frozen");
    let before = log.borrow().len();
    assert!(
        tab_until(&ui, || log.borrow()[before..].first() == Some(&(7, true))),
        "tabbing back onto the button reports focus again: {:?}",
        log.borrow()
    );
    assert!(controls.frozen(), "and freezes the list again");
}

#[test]
fn the_menu_key_opens_the_focused_rows_menu_with_that_row_captured() {
    use slint::platform::Key;
    let ui = shown_window();
    let actions = Rc::new(RefCell::new(Vec::new()));
    let a = actions.clone();
    ui.on_row_action(move |name, id| a.borrow_mut().push((name.to_string(), id)));
    let focus = Rc::new(RefCell::new(Vec::new()));
    let f = focus.clone();
    ui.on_row_key_focus(move |id, has| f.borrow_mut().push((id, has)));
    assert!(tab_until(&ui, || focus.borrow().last() == Some(&(7, true))), "Force close on Editor focused");
    actions.borrow_mut().clear();
    key(&ui, Key::Menu);
    assert_eq!(ui.get_menu_target(), 7);
    key(&ui, Key::DownArrow);
    key(&ui, Key::Return);
    assert_eq!(*actions.borrow(), [("end-task".to_string(), 7)]);
}

#[test]
fn any_modal_drops_the_keyboard_freeze_even_without_a_focus_out() {
    // A modal opened without parking focus first: the disabled button never sees its focus-out.
    let (ui, controls, log) = wired(vec![row_view(7, "Editor", HUNG), row_view(9, "Browser", "")]);
    assert!(tab_until(&ui, || log.borrow().last() == Some(&(7, true))));
    assert!(controls.frozen());
    ui.set_details_visible(true);
    slint::platform::update_timers_and_animations();
    ui.set_details_visible(false);
    slint::platform::update_timers_and_animations();
    assert!(!controls.frozen(), "modal-open clears the keyboard focus");
}

#[test]
fn a_key_pressed_inside_a_row_reasserts_a_missed_keyboard_freeze() {
    let (ui, controls, log) = wired(vec![row_view(7, "Editor", HUNG), row_view(9, "Browser", "")]);
    assert!(tab_until(&ui, || log.borrow().last() == Some(&(7, true))));
    ui.invoke_clear_key_focus(); // a focus edge that never arrived
    assert!(!controls.frozen());
    key(&ui, slint::platform::Key::Shift); // e.g. the start of Shift+F10, or Space
    assert!(controls.frozen(), "the capture phase re-freezes before the key acts");
}
