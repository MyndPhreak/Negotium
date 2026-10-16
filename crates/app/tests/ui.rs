use i_slint_backend_testing::ElementHandle;
use negotium::{DeviceData, GraphData, SegmentData, StatData};
use negotium::controls::Page;
use negotium::perf::{DeviceView, GraphView, PerfFrame, Range, StatView};
use negotium::worker::SharedHistory;
use negotium::ScrubRow;
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
fn moving_keyboard_focus_between_rows_never_unfreezes_the_list() {
    let (ui, controls, bridge) = bridged();
    bridge.borrow_mut().apply(&ui, frame(vec![row_view(1, "Editor", HUNG), row_view(2, "Game", HUNG)]));
    negotium::bridge::row_focus(&bridge, 1, true);
    assert!(controls.frozen());
    // A new hung app would rank first; it waits while the list is frozen.
    bridge.borrow_mut().apply(&ui, frame(vec![row_view(3, "Tool", HUNG), row_view(1, "Editor", HUNG), row_view(2, "Game", HUNG)]));
    // Tab from Editor's Force close to Game's: focus-out arrives before focus-in.
    negotium::bridge::row_focus(&bridge, 1, false);
    assert!(controls.frozen(), "the focus-out alone must not release the list");
    assert_eq!(shown_ids(&ui), [1, 2]);
    negotium::bridge::row_focus(&bridge, 2, true);
    slint::platform::update_timers_and_animations();
    assert!(controls.frozen());
    assert_eq!(shown_ids(&ui), [1, 2], "rows never moved under the focus");
    // Focus leaves the list for good: released on the next turn of the event loop.
    negotium::bridge::row_focus(&bridge, 2, false);
    slint::platform::update_timers_and_animations();
    assert!(!controls.frozen());
    assert_eq!(shown_ids(&ui), [3, 1, 2]);
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

// ---- Plan 3a: layout ----

#[test]
fn the_meters_do_not_grow_when_the_first_tiles_and_rows_arrive() {
    // The real app shows the window at its preferred size with meters only; the top-5 tiles and
    // the rows come a tick later. Spare height must go to the list from the first frame on, or
    // the meter cards visibly grow (110 -> 303 px) a second after the app opens.
    let filled = window(); // initialises the backend; lends its meters and rows
    let ui = MainWindow::new().unwrap();
    ui.set_meters(filled.get_meters());
    ui.show().unwrap();
    let measure = || {
        let cpu = El::find_by_accessible_label(&ui, "CPU meter").next().expect("cpu meter");
        let search = El::find_by_accessible_label(&ui, "Search apps and processes").next().expect("search box");
        (cpu.size().height, search.absolute_position().y)
    };
    let (meter_h, search_y) = measure();
    let tiles: Vec<negotium::TileData> = (0..5)
        .map(|i| negotium::TileData { id: i, name: format!("App {i}").into(), value: "5%".into(), weight: 1.0, hung: false })
        .collect();
    ui.set_tiles(ModelRc::new(VecModel::from(tiles)));
    ui.set_top_caption("Top CPU users".into());
    ui.set_rows(filled.get_rows());
    let (meter_h2, search_y2) = measure();
    assert_eq!((meter_h2, search_y2), (meter_h, search_y), "meter height and search box position stay put");
    assert!(meter_h < 130.0, "the meters keep their natural height: {meter_h}");
    let rail = El::find_by_accessible_label(&ui, "Performance page").next().expect("rail item");
    assert_eq!(rail.size().height, 56.0, "the rail gives no spare height to its buttons");
}

// ---- Plan 3a: pages ----

#[test]
fn the_rail_switches_pages() {
    let ui = window();
    let picked = Rc::new(RefCell::new(None));
    let p = picked.clone();
    ui.on_select_page(move |i| *p.borrow_mut() = Some(i));
    let perf = El::find_by_accessible_label(&ui, "Performance page").next().expect("rail item");
    assert_eq!(perf.accessible_checked(), Some(false));
    perf.invoke_accessible_default_action();
    assert_eq!(*picked.borrow(), Some(1));
    ui.set_page(1);
    assert_eq!(perf.accessible_checked(), Some(true));
    let overview = El::find_by_accessible_label(&ui, "Overview page").next().expect("rail item");
    assert_eq!(overview.accessible_checked(), Some(false));
}

fn device(id: &str, title: &str) -> DeviceData {
    DeviceData { id: id.into(), title: title.into(), value: "1%".into(), spark: "M 0 100".into() }
}

#[test]
fn clicking_a_device_selects_it() {
    let ui = shown_window();
    ui.set_page(1);
    ui.set_perf_devices(ModelRc::new(VecModel::from(vec![device("cpu", "CPU"), device("memory", "Memory")])));
    let picked = Rc::new(RefCell::new(None));
    let p = picked.clone();
    ui.on_select_device(move |id| *p.borrow_mut() = Some(id.to_string()));
    let memory = El::find_by_accessible_label(&ui, "Memory performance").next().expect("device button");
    assert_eq!(memory.size().height, 60.0, "the device list gives no spare height to its buttons");
    memory.invoke_accessible_default_action();
    assert_eq!(picked.borrow().as_deref(), Some("memory"));
    ui.set_perf_selected("memory".into());
    assert_eq!(memory.accessible_checked(), Some(true));
}

fn perf_frame(requested: &str, selected: &str) -> Frame {
    let devices = vec![
        DeviceView { id: "cpu".into(), title: "CPU".into(), ..Default::default() },
        DeviceView { id: "memory".into(), title: "Memory".into(), ..Default::default() },
    ];
    let title = if selected == "cpu" { "CPU" } else { "Memory" };
    let perf = PerfFrame { devices, requested: requested.into(), selected: selected.into(), title: title.into(), ..Default::default() };
    Frame { page: Page::Performance, perf: Some(perf), ..Default::default() }
}

#[test]
fn performance_frames_fill_the_page_and_leave_the_app_list_alone() {
    use slint::Model;
    let (ui, controls, bridge) = bridged();
    let mut b = bridge.borrow_mut();
    b.apply(&ui, frame(vec![row_view(1, "Editor", ""), row_view(2, "Browser", "")]));
    controls.set_page(Page::Performance);
    b.apply(&ui, perf_frame("cpu", "cpu"));
    assert_eq!(shown_ids(&ui), [1, 2], "the hidden Overview keeps its rows");
    assert_eq!(ui.get_perf_title(), "CPU");
    assert_eq!(ui.get_perf_devices().row_count(), 2);
    assert_eq!(ui.get_perf_selected(), "cpu");
}

#[test]
fn a_frame_for_another_page_or_device_is_dropped() {
    let (ui, controls, bridge) = bridged();
    let mut b = bridge.borrow_mut();
    controls.set_page(Page::Performance);
    controls.select_device("memory");
    b.apply(&ui, perf_frame("cpu", "cpu")); // built just before the click on Memory
    assert_eq!(ui.get_perf_title(), "", "a stale device's frame is not shown");
    b.apply(&ui, frame(vec![row_view(1, "Editor", "")])); // an Overview frame while Performance shows
    assert!(shown_ids(&ui).is_empty());
    controls.select_device("cpu");
    controls.set_range(Range::TenMinutes);
    b.apply(&ui, perf_frame("cpu", "cpu")); // built for 1 min, just before the click on 10 min
    assert_eq!(ui.get_perf_title(), "", "a stale range's frame is not shown");
    controls.set_range(Range::Minute);
    controls.set_page(Page::Overview);
    b.apply(&ui, perf_frame("cpu", "cpu")); // a Performance frame arriving after the user went back
    assert_eq!(ui.get_perf_title(), "", "a Performance frame is not shown while the Overview shows");
}

#[test]
fn a_vanished_device_falls_back_to_cpu_and_the_fallback_sticks() {
    let (ui, controls, bridge) = bridged();
    controls.set_page(Page::Performance);
    controls.select_device("disk:2 F:");
    bridge.borrow_mut().apply(&ui, perf_frame("disk:2 F:", "cpu"));
    assert_eq!(ui.get_perf_selected(), "cpu");
    assert_eq!(ui.get_perf_title(), "CPU");
    assert_eq!(controls.perf().0, "cpu", "a re-plugged disk doesn't snatch the page back");
}

#[test]
fn leaving_the_overview_releases_the_hover_freeze() {
    let (ui, controls, bridge) = bridged();
    let mut b = bridge.borrow_mut();
    b.apply(&ui, frame(vec![row_view(1, "Editor", "")]));
    b.set_hover(true);
    assert!(controls.frozen());
    // The hidden list can't report pointer-leave: switching pages must release it.
    b.set_page(&ui, Page::Performance);
    assert!(!controls.frozen(), "the hover freeze ends with the page switch");
    assert_eq!(controls.page(), Page::Performance);
    assert_eq!(ui.get_page(), 1);
    b.set_page(&ui, Page::Overview);
    assert_eq!((controls.page(), ui.get_page()), (Page::Overview, 0));
}

// A Narrator rail activation switches pages without moving keyboard focus: the hidden list (or a
// row's button) must neither stay focused nor keep the list frozen.
#[test]
fn leaving_the_overview_drops_the_lists_keyboard_focus() {
    use slint::platform::Key;
    let (ui, controls, bridge, actions) = wired_list(vec![row_view(7, "Editor", ""), row_view(9, "Browser", "")]);
    assert!(tab_until(&ui, || controls.frozen()), "Tab reaches the list");
    bridge.borrow_mut().set_page(&ui, Page::Performance);
    assert!(!controls.frozen(), "the switch itself releases the list, not a later focus-out");
    slint::platform::update_timers_and_animations();
    slint::platform::update_timers_and_animations();
    assert!(!controls.frozen(), "the hidden list is not left frozen");
    press_key(&ui, Key::Delete);
    assert!(!controls.frozen(), "a key does not freeze it again");
    assert!(actions.borrow().is_empty(), "keys no longer reach the hidden list: {:?}", actions.borrow());
    bridge.borrow_mut().set_page(&ui, Page::Overview);
    assert!(!controls.frozen());
}

#[test]
fn leaving_the_overview_drops_a_row_buttons_keyboard_focus() {
    use slint::platform::Key;
    let (ui, controls, bridge, actions) = wired_list(vec![row_view(7, "Editor", HUNG), row_view(9, "Browser", "")]);
    let log: FocusLog = Default::default();
    let (b, l) = (bridge.clone(), log.clone());
    ui.on_row_key_focus(move |id, focused| {
        l.borrow_mut().push((id, focused));
        negotium::bridge::row_focus(&b, id, focused);
    });
    assert!(tab_until(&ui, || log.borrow().last() == Some(&(7, true))), "Tab onto Editor's Force close");
    assert!(controls.frozen());
    bridge.borrow_mut().set_page(&ui, Page::Performance);
    assert!(!controls.frozen(), "the switch itself releases the list, not a later focus-out");
    slint::platform::update_timers_and_animations();
    slint::platform::update_timers_and_animations();
    assert!(!controls.frozen(), "the hidden list is not left frozen");
    press_key(&ui, Key::Space);
    assert!(!controls.frozen(), "a key does not freeze it again");
    assert!(actions.borrow().is_empty(), "Space no longer presses the hidden button: {:?}", actions.borrow());
    assert_eq!(log.borrow().last(), Some(&(7, false)), "the button really loses focus");
}

#[test]
fn an_open_modal_blocks_the_default_action_of_rail_and_tiles() {
    let ui = window();
    ui.set_tiles(ModelRc::new(VecModel::from(vec![negotium::TileData {
        id: 3, name: "Chrome".into(), value: "5%".into(), weight: 1.0, hung: false,
    }])));
    let pages = Rc::new(RefCell::new(Vec::new()));
    let p = pages.clone();
    ui.on_select_page(move |i| p.borrow_mut().push(i));
    let tiles = Rc::new(RefCell::new(Vec::new()));
    let t = tiles.clone();
    ui.on_open_tile(move |id| t.borrow_mut().push(id));
    ui.set_confirm_visible(true);
    El::find_by_accessible_label(&ui, "Performance page").next().expect("rail item").invoke_accessible_default_action();
    El::find_by_accessible_label(&ui, "Chrome tile").next().expect("tile").invoke_accessible_default_action();
    assert!(pages.borrow().is_empty(), "the rail ignores a screen reader while a modal is open");
    assert!(tiles.borrow().is_empty(), "so does a top-strip tile");
    ui.set_confirm_visible(false);
    El::find_by_accessible_label(&ui, "Performance page").next().unwrap().invoke_accessible_default_action();
    El::find_by_accessible_label(&ui, "Chrome tile").next().unwrap().invoke_accessible_default_action();
    assert_eq!((pages.borrow().clone(), tiles.borrow().clone()), (vec![1], vec![3]));
}

#[test]
fn a_clicked_tile_opens_and_takes_focus_so_space_opens_it_again() {
    let ui = shown_window();
    ui.set_tiles(ModelRc::new(VecModel::from(vec![negotium::TileData {
        id: 3, name: "Chrome".into(), value: "5%".into(), weight: 1.0, hung: false,
    }])));
    let tiles = Rc::new(RefCell::new(Vec::new()));
    let t = tiles.clone();
    ui.on_open_tile(move |id| t.borrow_mut().push(id));
    let tile = El::find_by_accessible_label(&ui, "Chrome tile").next().expect("tile");
    let (x, y) = centre(&tile);
    press(&ui, x, y, slint::platform::PointerEventButton::Left);
    assert_eq!(*tiles.borrow(), [3], "a click opens the tile");
    key(&ui, slint::platform::Key::Space);
    assert_eq!(*tiles.borrow(), [3, 3], "the click focused it: Space opens it again");
}

// ---- Plan 3a: Performance page ----

fn graph_data(label: &str) -> GraphData {
    GraphData {
        label: label.into(), value: "37%".into(), scale: "100%".into(),
        line: "M 1000.0 63.0".into(), area: "M 0 100".into(), line2: "M 0 100".into(),
    }
}

#[test]
fn range_buttons_pick_and_show_the_range() {
    let ui = shown_window();
    ui.set_page(1);
    let picked = Rc::new(RefCell::new(None));
    let p = picked.clone();
    ui.on_select_range(move |i| *p.borrow_mut() = Some(i));
    let ten = El::find_by_accessible_label(&ui, "10 min range").next().expect("range button");
    assert_eq!(ten.accessible_checked(), Some(false));
    ten.invoke_accessible_default_action();
    assert_eq!(*picked.borrow(), Some(1));
    ui.set_perf_range(1);
    assert_eq!(ten.accessible_checked(), Some(true));
    let minute = El::find_by_accessible_label(&ui, "1 min range").next().unwrap();
    assert_eq!(minute.accessible_checked(), Some(false));
}

#[test]
fn the_performance_page_shows_graphs_cells_details_and_composition() {
    let ui = shown_window();
    ui.set_page(1);
    ui.set_perf_graphs(ModelRc::new(VecModel::from(vec![graph_data("% Utilization")])));
    ui.set_perf_cells(ModelRc::new(VecModel::from(vec![graph_data("CPU 0"), graph_data("CPU 1")])));
    ui.set_perf_stats(ModelRc::new(VecModel::from(vec![StatData { label: "Speed".into(), value: "3.42 GHz".into() }])));
    ui.set_perf_segments(ModelRc::new(VecModel::from(vec![
        SegmentData { label: "In use".into(), value: "6.5 GB".into(), fraction: 0.4 },
        SegmentData { label: "Free".into(), value: "2.0 GB".into(), fraction: 0.6 },
    ])));
    for label in ["% Utilization graph", "CPU 0 graph", "CPU 1 graph", "Speed 3.42 GHz", "In use 6.5 GB", "Free 2.0 GB"] {
        assert!(El::find_by_accessible_label(&ui, label).next().is_some(), "missing {label}");
    }
}

#[test]
fn a_performance_frame_fills_graphs_and_details_through_the_bridge() {
    use slint::Model;
    let (ui, controls, bridge) = bridged();
    controls.set_page(Page::Performance);
    let mut f = perf_frame("cpu", "cpu");
    let p = f.perf.as_mut().unwrap();
    p.graphs = vec![GraphView { label: "% Utilization".into(), ..Default::default() }];
    p.cells = vec![GraphView::default(); 4];
    p.stats = vec![StatView { label: "Speed".into(), value: "3.42 GHz".into() }];
    p.note = "CPU temperature needs the add-on.".into();
    bridge.borrow_mut().apply(&ui, f);
    assert_eq!(ui.get_perf_graphs().row_count(), 1);
    assert_eq!(ui.get_perf_cells().row_count(), 4);
    assert_eq!(ui.get_perf_stats().row_data(0).unwrap().value, "3.42 GHz");
    assert_eq!(ui.get_perf_note(), "CPU temperature needs the add-on.");
    assert_eq!(ui.get_perf_range(), 0);
}

#[test]
fn the_performance_page_lays_out_graph_cells_details_and_ranges() {
    // Stands in for looking at the page: every key element is on screen, inside the window, with
    // a sane size, and the cells sit in a grid in logical-processor order.
    let ui = shown_window();
    ui.set_page(1);
    ui.set_perf_devices(ModelRc::new(VecModel::from(vec![device("cpu", "CPU"), device("memory", "Memory")])));
    ui.set_perf_title("CPU".into());
    ui.set_perf_graphs(ModelRc::new(VecModel::from(vec![graph_data("% Utilization")])));
    let cells: Vec<GraphData> = (0..16).map(|i| graph_data(&format!("CPU {i}"))).collect();
    ui.set_perf_cells(ModelRc::new(VecModel::from(cells)));
    let stats: Vec<StatData> = ["Utilization", "Speed", "Temperature", "Processes"]
        .iter()
        .map(|l| StatData { label: (*l).into(), value: "—".into() })
        .collect();
    ui.set_perf_stats(ModelRc::new(VecModel::from(stats)));
    ui.set_perf_segments(ModelRc::new(VecModel::from(vec![
        SegmentData { label: "In use".into(), value: "6.5 GB".into(), fraction: 0.4 },
        SegmentData { label: "Free".into(), value: "2.0 GB".into(), fraction: 0.6 },
    ])));
    let find = |label: &str| El::find_by_accessible_label(&ui, label).next().unwrap_or_else(|| panic!("missing {label}"));
    let inside = |label: &str| {
        let e = find(label);
        let (p, s) = (e.absolute_position(), e.size());
        let (x, y, w, h) = (p.x, p.y, s.width, s.height);
        assert!(w > 0.0 && h > 0.0, "{label} has no size: {w}x{h}");
        assert!(x >= 0.0 && x + w <= 1000.0, "{label} sticks out sideways: x {x} w {w}");
        (x, y, w, h)
    };
    let list = inside("CPU performance");
    let (gx, gy, gw, gh) = inside("% Utilization graph");
    assert!(gx > list.0 + list.2, "the graph sits right of the device list");
    assert!(gw > 500.0 && gh >= 150.0, "the main graph is large: {gw}x{gh}");
    assert!(gy + gh <= 720.0, "the main graph is in view");
    let ranges: Vec<_> = ["1 min range", "10 min range", "1 h range"].iter().map(|l| inside(l)).collect();
    assert!(ranges.windows(2).all(|w| w[0].1 == w[1].1 && w[0].0 + w[0].2 <= w[1].0), "range buttons in one row, in order: {ranges:?}");
    assert!(ranges.iter().all(|r| r.3 >= 24.0 && r.1 + r.3 <= gy), "range buttons sit above the graph: {ranges:?}");
    // Cells: cell n is processor n, laid out row by row.
    let cell: Vec<_> = (0..16).map(|i| inside(&format!("CPU {i} graph"))).collect();
    assert!(cell.iter().all(|c| c.2 >= 60.0 && c.3 >= 30.0), "cells are readable: {cell:?}");
    assert!(cell[0].1 > gy + gh, "the cells come below the main graph");
    assert!(cell[0].0 < cell[1].0 && cell[0].1 == cell[1].1, "cell 1 is right of cell 0");
    let cols = cell.iter().take_while(|c| c.1 == cell[0].1).count();
    assert!((2..16).contains(&cols), "the cells wrap into a grid: {cols} columns");
    assert!(cell[cols].1 > cell[0].1 && cell[cols].0 == cell[0].0, "processor {cols} starts the second row");
    // Composition bar: segments side by side, wider for the larger fraction.
    let (ux, uy, uw, _) = inside("In use 6.5 GB");
    let (fx, fy, fw, _) = inside("Free 2.0 GB");
    assert!(uy == fy && ux < fx && fw > uw, "segments share a row, in order and in proportion");
    // Details: below the cells.
    let speed = inside("Speed —");
    assert!(speed.1 > cell[15].1, "the details come after the cells");
    assert!(El::find_by_accessible_label(&ui, "Utilization —").next().is_some());
}

// ---- Task 9 fix round 1 ----

type Rect = (f32, f32, f32, f32);

fn rect_of(e: &El) -> Rect {
    let (p, s) = (e.absolute_position(), e.size());
    (p.x, p.y, s.width, s.height)
}

fn rect(ui: &MainWindow, label: &str) -> Rect {
    rect_of(&El::find_by_accessible_label(ui, label).next().unwrap_or_else(|| panic!("missing {label}")))
}

fn overlaps(a: Rect, b: Rect) -> bool {
    a.0 < b.0 + b.2 && b.0 < a.0 + a.2 && a.1 < b.1 + b.3 && b.1 < a.1 + a.3
}

fn cell(label: &str, value: &str) -> GraphData {
    GraphData { value: value.into(), ..graph_data(label) }
}

/// A Performance page at `w` × `h` with the given main graphs and cells, four stats and a note.
fn perf_window(w: u32, h: u32, graphs: &[&str], cells: Vec<GraphData>) -> MainWindow {
    // One testing backend per test thread; a test may build several windows.
    thread_local! { static BACKEND: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
    if !BACKEND.with(|b| b.replace(true)) {
        i_slint_backend_testing::init_no_event_loop();
    }
    let ui = MainWindow::new().unwrap();
    ui.window().set_size(PhysicalSize::new(w, h));
    ui.set_page(1);
    ui.set_perf_devices(ModelRc::new(VecModel::from(vec![device("cpu", "CPU"), device("gpu:0", "GPU 0")])));
    ui.set_perf_title("CPU".into());
    ui.set_perf_graphs(ModelRc::new(VecModel::from(graphs.iter().map(|g| graph_data(g)).collect::<Vec<_>>())));
    ui.set_perf_cells(ModelRc::new(VecModel::from(cells)));
    let stats: Vec<StatData> = ["Utilization", "Speed", "Temperature", "Processes"]
        .iter()
        .map(|l| StatData { label: (*l).into(), value: "1".into() })
        .collect();
    ui.set_perf_stats(ModelRc::new(VecModel::from(stats)));
    ui.set_perf_note("A note.".into());
    ui.show().unwrap();
    ui
}

fn cores(n: usize) -> Vec<GraphData> {
    (0..n).map(|i| cell(&format!("CPU {i}"), "37%")).collect()
}

#[test]
fn spare_height_goes_to_the_main_graph_only() {
    let measure = |h: u32| {
        let ui = perf_window(1000, h, &["% Utilization"], cores(16));
        (
            rect(&ui, "% Utilization graph"),
            rect(&ui, "CPU 0 37%"),
            rect(&ui, "1 min range"),
            rect(&ui, "Speed 1"),
        )
    };
    let (graph, cell, range, stat) = measure(720);
    assert_eq!(graph.3, 180.0, "content taller than the view: the graph keeps its minimum");
    let (tall_graph, tall_cell, tall_range, tall_stat) = measure(1100);
    assert!(tall_graph.3 > 180.0, "a tall window grows the main graph: {}", tall_graph.3);
    assert_eq!((tall_cell.2, tall_cell.3), (cell.2, cell.3), "cells keep their size");
    assert_eq!((tall_range.2, tall_range.3), (range.2, range.3), "range buttons keep their size");
    assert_eq!(tall_stat.3, stat.3, "details keep their size");
    let note = rect(&perf_window(1000, 1100, &["% Utilization"], cores(16)), "A note.");
    assert!(note.1 + note.3 <= 1100.0 - 12.0 + 0.5, "the content fills the view exactly, no more: {note:?}");
}

#[test]
fn spare_height_is_shared_by_two_main_graphs() {
    let ui = perf_window(1000, 1100, &["Active time", "Transfer rate"], cores(0));
    let (a, b) = (rect(&ui, "Active time graph"), rect(&ui, "Transfer rate graph"));
    assert!(a.3 > 180.0 && (a.3 - b.3).abs() < 1.0, "both grow alike: {a:?} {b:?}");
    assert!(a.1 + a.3 <= b.1, "and stay apart");
}

#[test]
fn a_cells_value_is_never_cut_off_by_a_long_label() {
    let long = "Video Processing Engine With A Rather Long Name";
    let ui = perf_window(760, 440, &["3D"], vec![cell("3D", "12.5%"), cell(long, "12.5%"), cell("Copy", "—"), cell(long, "—")]);
    for value in ["12.5%", "—"] {
        let texts: Vec<Rect> = El::find_by_accessible_label(&ui, value).map(|e| rect_of(&e)).collect();
        assert_eq!(texts.len(), 2, "one {value} per cell");
        assert!(texts[0].2 > 0.0 && texts[0].2 == texts[1].2, "{value} has its full width beside a long label: {texts:?}");
    }
    for (label, value) in [("3D", "12.5%"), (long, "12.5%"), ("Copy", "—"), (long, "—")] {
        let c = rect(&ui, &format!("{label} {value}"));
        let v: Vec<Rect> = El::find_by_accessible_label(&ui, value).map(|e| rect_of(&e)).collect();
        assert!(
            v.iter().any(|t| t.0 >= c.0 && t.0 + t.2 <= c.0 + c.2 + 0.5 && t.1 >= c.1 && t.1 < c.1 + c.3),
            "the value of {label} lies inside its cell {c:?}: {v:?}"
        );
    }
}

fn find_rect(ui: &MainWindow, label: &str) -> Option<Rect> {
    El::find_by_accessible_label(ui, label).next().map(|e| rect_of(&e))
}

/// Every element in view that has an accessible label, by label (first one wins); one tree walk.
fn labelled(ui: &MainWindow) -> std::collections::HashMap<String, Rect> {
    use i_slint_backend_testing::ElementRoot;
    let mut map = std::collections::HashMap::new();
    for e in ui.root_element().query_descendants().match_predicate(|e| e.accessible_label().is_some()).find_all() {
        map.entry(e.accessible_label().unwrap().to_string()).or_insert_with(|| rect_of(&e));
    }
    map
}

fn scroll_detail(ui: &MainWindow, dy: f32) {
    ui.window().dispatch_event(WindowEvent::PointerScrolled { position: LogicalPosition::new(600.0, 300.0), delta_x: 0.0, delta_y: dy });
}

/// Scrolls the detail area from top to bottom. At every position, the cells in view (the testing
/// API finds only what is not clipped away) must sit on one grid in index order without
/// overlapping, clear of the scrollbar gutter, and the sections in view must stack in order.
/// Returns the column count; panics unless every cell was seen.
fn scan_grid(ui: &MainWindow, labels: &[String], graphs: &[&str], width: f32) -> usize {
    scroll_detail(ui, 100_000.0); // to the top
    let mut seen = vec![false; labels.len()];
    let mut cols = None;
    let mut last = None;
    for _ in 0..200 {
        let view = labelled(ui);
        let find = |l: &str| view.get(l).copied();
        let cells: Vec<Option<Rect>> = labels.iter().map(|l| find(&format!("{l} graph"))).collect();
        let marks: Vec<Option<Rect>> = graphs.iter().map(|g| format!("{g} graph")).chain(["1 h range".into(), "A note.".into()]).map(|l| find(&l)).collect();
        let snapshot = (cells.clone(), marks);
        if last.as_ref() == Some(&snapshot) {
            break; // the bottom: scrolling moved nothing
        }
        let shown: Vec<(usize, Rect)> = cells.iter().enumerate().filter_map(|(i, c)| c.map(|c| (i, c))).collect();
        if let Some(&(i0, c0)) = shown.first() {
            let n = *cols.get_or_insert_with(|| shown.iter().take_while(|(_, c)| c.1 == c0.1).count().max(1));
            if shown.len() > n {
                // Seen a full row: its length is the column count (unless all cells fit one row).
                assert_eq!(n, shown.iter().take_while(|(_, c)| c.1 == c0.1).count().max(n));
            }
            let pitch = shown.iter().find(|(i, _)| *i == i0 + n).map(|(_, c)| c.1 - c0.1);
            for &(i, c) in &shown {
                for &(j, d) in &shown {
                    if i < j {
                        assert!(!overlaps(c, d), "cells {i} and {j} overlap: {c:?} {d:?}");
                    }
                    if i % n == j % n {
                        assert_eq!(c.0, d.0, "cells {i} and {j} share a column");
                    }
                    if i / n == j / n {
                        assert_eq!(c.1, d.1, "cells {i} and {j} share a row");
                    }
                }
                if let Some(p) = pitch {
                    let rows = (i / n) as f32 - (i0 / n) as f32;
                    assert!((c.1 - c0.1 - rows * p).abs() < 0.5, "cell {i} is in row {} below cell {i0}", i / n);
                }
                assert!(c.0 + c.2 <= width - 12.0 - 14.0 + 0.5, "cell {i} clears the scrollbar gutter: {c:?}");
                seen[i] = true;
            }
        }
        let mut sections: Vec<Rect> = ["1 h range"].iter().filter_map(|l| find(l)).collect();
        sections.extend(graphs.iter().filter_map(|g| find(&format!("{g} graph"))));
        if let (Some(&(_, a)), Some(&(_, b))) = (shown.first(), shown.last()) {
            sections.push((a.0, a.1, width, b.1 + b.3 - a.1));
        }
        sections.extend(["Speed 1", "A note."].iter().filter_map(|l| find(l)));
        for w in sections.windows(2) {
            assert!(w[0].1 + w[0].3 <= w[1].1, "sections stacked without overlap: {w:?}");
        }
        last = Some(snapshot);
        scroll_detail(ui, -60.0);
    }
    assert!(seen.iter().all(|s| *s), "every cell was reached by scrolling: {seen:?}");
    assert!(find_rect(ui, "A note.").is_some(), "the bottom of the page is reachable");
    cols.unwrap()
}

#[test]
fn sixty_four_cores_at_the_minimum_window_size_stay_in_order_without_overlap() {
    let ui = perf_window(760, 440, &["% Utilization"], cores(64));
    let labels: Vec<String> = (0..64).map(|i| format!("CPU {i}")).collect();
    let cols = scan_grid(&ui, &labels, &["% Utilization"], 760.0);
    assert!(cols >= 2, "{cols} columns");
    scroll_detail(&ui, 100_000.0);
    assert_eq!(rect(&ui, "% Utilization graph").3, 180.0, "the main graph keeps its minimum");
}

#[test]
fn eight_gpu_engines_lay_out_without_overlap() {
    let engines = ["3D", "Copy", "Video Decode", "Video Encode", "Video Processing", "Compute_0", "Compute_1", "Security"];
    let graphs = ["Utilization", "Dedicated GPU memory", "Shared GPU memory"];
    let ui = perf_window(760, 440, &graphs, engines.iter().map(|e| cell(e, "—")).collect());
    let labels: Vec<String> = engines.iter().map(|e| e.to_string()).collect();
    assert!(scan_grid(&ui, &labels, &graphs, 760.0) >= 2);
}

#[test]
fn the_column_count_does_not_flip_around_the_scrollbar_threshold() {
    // Sweep the height across the point where the content stops fitting (the scrollbar appears):
    // the columns depend on the width only, and every layout settles without a binding loop.
    for w in [760u32, 1000] {
        let mut seen = None;
        for h in (440..=1300).step_by(20) {
            let ui = perf_window(w, h, &["% Utilization"], cores(16));
            let view = labelled(&ui);
            let first = view["CPU 0 graph"];
            let cols = (0..16).take_while(|i| view.get(&format!("CPU {i} graph")).is_some_and(|c| c.1 == first.1)).count();
            slint::platform::update_timers_and_animations();
            assert_eq!(rect(&ui, "CPU 0 graph"), first, "settled at {w}x{h}");
            assert_eq!(*seen.get_or_insert(cols), cols, "column count changed at {w}x{h}");
        }
    }
}

#[test]
fn text_on_the_accent_contrasts_with_it() {
    let ui = window();
    let theme = ui.global::<negotium::Theme>();
    assert_eq!(theme.get_on_accent(), slint::Color::from_rgb_u8(255, 255, 255), "white on the default blue");
    theme.set_accent(slint::Color::from_rgb_u8(0xff, 0xd7, 0x00));
    assert_eq!(theme.get_on_accent(), slint::Color::from_rgb_u8(0, 0, 0), "dark on a light accent");
}

#[test]
fn hovering_a_graph_scrubs_and_leaving_ends_it() {
    let ui = shown_window();
    ui.set_page(1);
    ui.set_perf_graphs(ModelRc::new(VecModel::from(vec![graph_data("% Utilization")])));
    let got = Rc::new(RefCell::new(Vec::<f32>::new()));
    let g = got.clone();
    ui.on_scrub(move |f| g.borrow_mut().push(f));
    let graph = El::find_by_accessible_label(&ui, "% Utilization graph").next().expect("graph");
    let (x, y) = centre(&graph);
    move_pointer(&ui, x, y);
    slint::platform::update_timers_and_animations();
    let last = *got.borrow().last().expect("the scrub reports the pointer");
    assert!((last - 0.5).abs() < 0.05, "{last}");
    move_pointer(&ui, 5.0, 5.0);
    slint::platform::update_timers_and_animations();
    assert_eq!(got.borrow().last(), Some(&-1.0));
}

/// One sample a second from 1 s to 61 s; at second s, chrome uses s % CPU.
fn scrub_history() -> SharedHistory {
    use negotium_model::{AppKey, AppKind, AppRow};
    let history: SharedHistory = Default::default();
    for s in 1..=61u64 {
        let chrome = AppRow {
            key: AppKey::Background { pid: 1, create_time: 0 }, name: "chrome".into(), kind: AppKind::Background,
            processes: vec![], cpu_pct: s as f32, private_bytes: 0, disk_bps: 0.0, gpu_pct: None, net_bps: None, hung_for_ms: None,
        };
        history.lock().unwrap().push(s * 1_000, &Default::default(), &[chrome]);
    }
    history
}

/// A CPU frame whose graph window ends at collector time `end`, `clock` ms after midnight.
fn scrub_frame(end: u64, clock: u64) -> Frame {
    let mut f = perf_frame("cpu", "cpu");
    let p = f.perf.as_mut().unwrap();
    (p.window_end_ms, p.span_ms, p.end_clock_ms) = (end, 60_000, clock);
    f
}

const TWO_PM: u64 = 14 * 3_600_000;

#[test]
fn the_bridge_answers_a_scrub_from_the_shared_history_and_follows_new_frames() {
    use slint::Model;
    let (ui, controls, bridge) = bridged();
    controls.set_page(Page::Performance);
    bridge.borrow_mut().set_history(scrub_history());
    bridge.borrow_mut().apply(&ui, scrub_frame(60_000, TWO_PM));
    bridge.borrow_mut().scrub(&ui, 0.5);
    assert_eq!(ui.get_scrub_title(), "Top apps at 13:59:30");
    assert_eq!(ui.get_scrub_rows().row_data(0).unwrap().name, "chrome");
    assert!((ui.get_scrub_x() - 0.5).abs() < 1e-6);
    // A second later the window has moved under the still pointer: the card follows.
    bridge.borrow_mut().apply(&ui, scrub_frame(61_000, TWO_PM + 1_000));
    assert_eq!(ui.get_scrub_title(), "Top apps at 13:59:31");
    assert_eq!(ui.get_scrub_rows().row_data(0).unwrap().value, "31.0%");
    bridge.borrow_mut().scrub(&ui, -1.0);
    assert_eq!(ui.get_scrub_x(), -1.0);
}

#[test]
fn the_scrub_never_waits_for_the_worker_and_keeps_its_card_while_the_history_is_busy() {
    use slint::Model;
    let (ui, controls, bridge) = bridged();
    controls.set_page(Page::Performance);
    let history = scrub_history();
    bridge.borrow_mut().set_history(history.clone());
    bridge.borrow_mut().apply(&ui, scrub_frame(60_000, TWO_PM));
    bridge.borrow_mut().scrub(&ui, 0.5);
    {
        // The worker holds the lock (mid-tick). A blocking lock here would deadlock this thread.
        let _worker = history.lock().unwrap();
        bridge.borrow_mut().apply(&ui, scrub_frame(61_000, TWO_PM + 1_000));
        bridge.borrow_mut().scrub(&ui, 0.75);
        assert_eq!(ui.get_scrub_title(), "Top apps at 13:59:30", "the current card stays");
        assert_eq!(ui.get_scrub_rows().row_data(0).unwrap().value, "30.0%");
        assert!((ui.get_scrub_x() - 0.5).abs() < 1e-6);
    }
    bridge.borrow_mut().scrub(&ui, 0.75);
    assert_eq!(ui.get_scrub_title(), "Top apps at 13:59:46", "answered once the lock is free");
    assert!((ui.get_scrub_x() - 0.75).abs() < 1e-6);

    // A worker panic poisoned the lock: the card stays (no UI-thread panic) until the restarted
    // worker resets the history.
    quiet_expected_panics();
    let _ = std::panic::catch_unwind(|| {
        let _guard = history.lock().unwrap();
        panic!("test panic: worker panicked mid-push");
    });
    assert!(history.is_poisoned());
    bridge.borrow_mut().scrub(&ui, 0.25);
    assert_eq!(ui.get_scrub_title(), "Top apps at 13:59:46");
    bridge.borrow_mut().scrub(&ui, -1.0);
    assert_eq!(ui.get_scrub_x(), -1.0, "leaving the graph always hides the scrub");
}

#[test]
fn leaving_the_performance_page_ends_the_scrub() {
    let (ui, controls, bridge) = bridged();
    controls.set_page(Page::Performance);
    bridge.borrow_mut().set_history(scrub_history());
    bridge.borrow_mut().apply(&ui, scrub_frame(60_000, TWO_PM));
    bridge.borrow_mut().scrub(&ui, 0.5);
    assert!(ui.get_scrub_x() >= 0.0);
    bridge.borrow_mut().set_page(&ui, Page::Overview);
    assert_eq!(ui.get_scrub_x(), -1.0);
    bridge.borrow_mut().set_page(&ui, Page::Performance);
    bridge.borrow_mut().apply(&ui, scrub_frame(61_000, TWO_PM + 1_000));
    assert_eq!(ui.get_scrub_x(), -1.0, "no cursor comes back without a pointer over a graph");
}

/// Panics whose message starts with "test panic:" are expected by a test and not printed. The hook
/// is installed once and passes every other panic to the hook installed before it; it is never
/// removed, so tests running in parallel can't drop each other's.
fn quiet_expected_panics() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let p = info.payload();
            let text = p.downcast_ref::<&str>().copied().or_else(|| p.downcast_ref::<String>().map(String::as_str));
            if !text.is_some_and(|t| t.starts_with("test panic:")) {
                previous(info);
            }
        }));
    });
}

#[test]
fn the_scrub_card_stays_inside_the_graph_at_the_smallest_window() {
    let ui = perf_window(760, 440, &["% Utilization"], cores(4));
    let rows: Vec<ScrubRow> = (0..10)
        .map(|i| ScrubRow { name: format!("A rather long application name {i}").into(), value: "100.0%".into() })
        .collect();
    ui.set_scrub_rows(ModelRc::new(VecModel::from(rows)));
    let graph = rect(&ui, "% Utilization graph");
    let visible = (graph.0.max(0.0), graph.1.max(0.0), (graph.0 + graph.2).min(760.0), (graph.1 + graph.3).min(440.0));
    for f in [0.02f32, 0.59, 0.98] {
        let title = format!("Top apps at {f}");
        ui.set_scrub_title(title.as_str().into());
        let cursor = graph.0 + graph.2 * f;
        move_pointer(&ui, cursor, graph.1 + graph.3 / 2.0);
        slint::platform::update_timers_and_animations();
        ui.set_scrub_x(f);
        let card = rect(&ui, &title);
        let (l, t, r, b) = (card.0, card.1, card.0 + card.2, card.1 + card.3);
        assert!(l >= visible.0 && t >= visible.1 && r <= visible.2 && b <= visible.3,
            "at {f} the card {card:?} leaves the visible graph {visible:?}");
        assert!(r <= cursor || l >= cursor, "at {f} the card {card:?} covers the cursor at {cursor}");
        assert!(card.3 >= 34.0 + 5.0 * 16.0, "at {f} the card lists several apps: {card:?}");
    }
}

// ---- Plan 3a: keyboard navigation ----

type Actions = Rc<RefCell<Vec<(String, i32)>>>;

/// A shown window driven by a real Bridge, with the list wired the way main.rs wires it.
fn wired_list(rows: Vec<RowView>) -> (MainWindow, Arc<Controls>, Rc<RefCell<Bridge>>, Actions) {
    let (ui, controls, bridge) = bridged();
    bridge.borrow_mut().apply(&ui, frame(rows));
    let actions: Actions = Default::default();
    let a = actions.clone();
    ui.on_row_action(move |name, id| a.borrow_mut().push((name.to_string(), id)));
    let b = bridge.clone();
    ui.on_list_focus(move |f| negotium::bridge::list_focus(&b, f));
    let (b, weak) = (bridge.clone(), ui.as_weak());
    ui.on_list_key(move |text| weak.upgrade().is_some_and(|ui| negotium::bridge::handle_list_key(&b, &ui, &text)));
    let b = bridge.clone();
    ui.on_row_key_focus(move |id, f| negotium::bridge::row_focus(&b, id, f));
    let b = bridge.clone();
    ui.on_list_hover(move |h| b.borrow_mut().set_hover(h));
    let b = bridge.clone();
    ui.on_clear_key_focus(move || b.borrow_mut().clear_key_focus());
    ui.window().set_size(PhysicalSize::new(1000, 720));
    ui.show().unwrap();
    (ui, controls, bridge, actions)
}

/// A key, then two event-loop turns: one runs `changed` handlers, the next the zero-delay timers
/// they start (a deferred focus-out).
fn press_key(ui: &MainWindow, k: slint::platform::Key) {
    key(ui, k);
    slint::platform::update_timers_and_animations();
    slint::platform::update_timers_and_animations();
}

#[test]
fn tabbing_into_the_list_freezes_it_and_arrows_move_a_current_row_that_never_jumps() {
    use slint::platform::Key;
    let (ui, controls, bridge, actions) =
        wired_list(vec![row_view(7, "Editor", ""), row_view(9, "Browser", ""), row_view(11, "Tool", "")]);
    assert!(tab_until(&ui, || controls.frozen()), "Tab reaches the list");
    assert_eq!((ui.get_current_row(), ui.get_current_id()), (0, 7));
    press_key(&ui, Key::DownArrow);
    assert_eq!((ui.get_current_row(), ui.get_current_id()), (1, 9));
    // A new frame re-sorts and adds an app: nothing moves under the keyboard.
    bridge.borrow_mut().apply(&ui, frame(vec![row_view(12, "New", ""), row_view(11, "Tool", ""), row_view(9, "Browser", ""), row_view(7, "Editor", "")]));
    assert_eq!(shown_ids(&ui), [7, 9, 11]);
    assert_eq!((ui.get_current_row(), ui.get_current_id()), (1, 9));
    press_key(&ui, Key::Delete);
    assert_eq!(*actions.borrow(), [("end-task".to_string(), 9)], "Delete ends the current row's app");
    press_key(&ui, Key::Escape);
    assert!(!controls.frozen(), "Esc leaves the list and live ordering resumes");
    assert_eq!(shown_ids(&ui), [12, 11, 9, 7]);
}

#[test]
fn right_and_left_expand_enter_and_collapse_from_the_keyboard() {
    use slint::platform::Key;
    let mut browser = row_view(9, "Browser", "");
    browser.expandable = true;
    let child = |id: i32| RowView { id, parent: 9, depth: 1, name: format!("proc{id}"), actionable: true, ..Default::default() };
    let (ui, controls, _bridge, _) = wired_list(vec![browser, child(20), child(21), row_view(11, "Tool", "")]);
    assert!(tab_until(&ui, || controls.frozen()));
    press_key(&ui, Key::RightArrow);
    assert_eq!(shown_ids(&ui), [9, 20, 21, 11], "expanded in place");
    press_key(&ui, Key::RightArrow);
    assert_eq!(ui.get_current_id(), 20);
    press_key(&ui, Key::LeftArrow);
    assert_eq!(ui.get_current_id(), 9);
    press_key(&ui, Key::LeftArrow);
    assert_eq!(shown_ids(&ui), [9, 11]);
}

#[test]
fn a_modal_opened_from_the_keyboard_does_not_leave_the_list_frozen() {
    use slint::platform::Key;
    let (ui, controls, _bridge, _) = wired_list(vec![row_view(7, "csrss.exe", ""), row_view(9, "Browser", "")]);
    let weak = ui.as_weak();
    ui.on_row_action(move |_, _| {
        if let Some(ui) = weak.upgrade() {
            ui.invoke_show_confirm("Ending csrss.exe will crash Windows.".into());
        }
    });
    assert!(tab_until(&ui, || controls.frozen()));
    press_key(&ui, Key::Delete);
    assert!(ui.get_confirm_visible());
    ui.set_confirm_visible(false);
    slint::platform::update_timers_and_animations();
    assert!(!controls.frozen(), "closing the modal must not leave the list frozen");
}

#[test]
fn the_menu_key_on_the_list_opens_the_current_rows_menu_with_that_row_captured() {
    use slint::platform::Key;
    let (ui, controls, _bridge, actions) = wired_list(vec![row_view(7, "Editor", ""), row_view(9, "Browser", "")]);
    assert!(tab_until(&ui, || controls.frozen()));
    press_key(&ui, Key::DownArrow);
    key(&ui, Key::Menu);
    assert_eq!(ui.get_menu_target(), 9);
    key(&ui, Key::DownArrow);
    key(&ui, Key::Return);
    assert_eq!(*actions.borrow(), [("end-task".to_string(), 9)]);
}

#[test]
fn opening_a_tile_scrolls_its_app_into_view() {
    let rows: Vec<RowView> = (1..=40).map(|i| row_view(i, &format!("App{i}"), "")).collect();
    let (ui, _controls, bridge, _) = wired_list(rows);
    assert_eq!(ui.get_list_scroll_y(), 0.0);
    bridge.borrow_mut().open_tile(35);
    assert!(ui.get_list_scroll_y() < 0.0, "the list scrolled down to App35");
}

// Ruling PF1: frames only sync the current row; they never scroll the list.
#[test]
fn a_wheel_scrolled_focused_list_is_not_pulled_back_by_frames() {
    use slint::platform::Key;
    let rows = |cpu: &str| -> Vec<RowView> {
        (1..=40).map(|i| RowView { cpu: cpu.into(), ..row_view(i, &format!("App{i}"), "") }).collect()
    };
    let (ui, controls, bridge, _) = wired_list(rows("1.0%"));
    assert!(tab_until(&ui, || controls.frozen()));
    assert_eq!(ui.get_current_row(), 0);
    let list = El::find_by_accessible_label(&ui, "Apps").next().expect("the list");
    let (x, y) = centre(&list);
    ui.window().dispatch_event(WindowEvent::PointerScrolled { position: LogicalPosition::new(x, y), delta_x: 0.0, delta_y: -400.0 });
    slint::platform::update_timers_and_animations();
    let scrolled = ui.get_list_scroll_y();
    assert!(scrolled < -100.0, "the wheel scrolled the list: {scrolled}");
    for cpu in ["2.0%", "3.0%", "4.0%"] {
        bridge.borrow_mut().apply(&ui, frame(rows(cpu)));
        slint::platform::update_timers_and_animations();
        assert_eq!(ui.get_list_scroll_y(), scrolled, "a frame must not snap the view back to the current row");
    }
    // A navigation key brings the current row back into view.
    press_key(&ui, Key::DownArrow);
    assert_eq!(ui.get_current_row(), 1);
    assert!(ui.get_list_scroll_y() > scrolled, "Down scrolls to the current row: {}", ui.get_list_scroll_y());
}

// Review Focus 5: an exited row held in place is inert from the keyboard too.
#[test]
fn delete_and_the_menu_key_do_nothing_on_an_exited_current_row() {
    use slint::platform::Key;
    let (ui, controls, bridge, actions) = wired_list(vec![row_view(7, "Editor", ""), row_view(9, "Browser", "")]);
    let opened = Rc::new(RefCell::new(Vec::new()));
    let o = opened.clone();
    ui.on_menu_open(move |open| o.borrow_mut().push(open));
    assert!(tab_until(&ui, || controls.frozen()));
    press_key(&ui, Key::DownArrow);
    // Browser exits while it is the current row: it stays in place, marked exited.
    bridge.borrow_mut().apply(&ui, frame(vec![row_view(7, "Editor", "")]));
    assert_eq!(shown_ids(&ui), [7, 9]);
    assert_eq!((ui.get_current_row(), ui.get_current_id(), ui.get_current_actionable()), (1, 9, false));
    press_key(&ui, Key::Delete);
    key(&ui, Key::Menu);
    assert!(actions.borrow().is_empty(), "no action on an exited row: {:?}", actions.borrow());
    assert!(opened.borrow().is_empty(), "no menu on an exited row");
    assert_eq!(ui.get_current_id(), 9, "the current row does not jump to another app");
}

// Ruling R7: the Windows group is never a keyboard target either.
#[test]
fn the_windows_group_is_never_acted_on_from_the_keyboard() {
    use slint::platform::Key;
    let mut group = row_view(5, "Windows", "");
    group.actionable = false;
    group.expandable = true;
    let member = RowView { id: 6, parent: 5, depth: 1, name: "svchost.exe".into(), actionable: true, ..Default::default() };
    let (ui, controls, _bridge, actions) = wired_list(vec![group, member, row_view(9, "Browser", "")]);
    let opened = Rc::new(RefCell::new(Vec::new()));
    let o = opened.clone();
    ui.on_menu_open(move |open| o.borrow_mut().push(open));
    assert!(tab_until(&ui, || controls.frozen()));
    assert_eq!(ui.get_current_id(), 5);
    assert!(!ui.get_current_actionable());
    press_key(&ui, Key::Delete);
    key(&ui, Key::Menu);
    let shift: slint::SharedString = Key::Shift.into();
    ui.window().dispatch_event(WindowEvent::KeyPressed { text: shift.clone() });
    key(&ui, Key::F10);
    ui.window().dispatch_event(WindowEvent::KeyReleased { text: shift });
    press_key(&ui, Key::Return);
    assert_eq!(shown_ids(&ui), [5, 6, 9], "Enter only expands the group");
    assert_eq!(ui.get_current_id(), 5);
    assert!(actions.borrow().is_empty(), "nothing acts on the Windows group: {:?}", actions.borrow());
    assert!(opened.borrow().is_empty(), "its menu never opens");
}

// Review Focus 5: actions follow the current row's id, not the index it sat at.
#[test]
fn keyboard_actions_follow_the_current_rows_id_after_the_list_reorders() {
    use slint::platform::Key;
    let (ui, controls, bridge, actions) =
        wired_list(vec![row_view(7, "Editor", ""), row_view(9, "Browser", ""), row_view(11, "Tool", "")]);
    assert!(tab_until(&ui, || controls.frozen()));
    press_key(&ui, Key::DownArrow);
    assert_eq!((ui.get_current_row(), ui.get_current_id()), (1, 9));
    // Leave the list; it re-sorts and Browser moves to the end, Tool takes its index.
    press_key(&ui, Key::Escape);
    bridge.borrow_mut().apply(&ui, frame(vec![row_view(7, "Editor", ""), row_view(11, "Tool", ""), row_view(9, "Browser", "")]));
    assert_eq!(shown_ids(&ui), [7, 11, 9]);
    assert_eq!((ui.get_current_row(), ui.get_current_id()), (2, 9), "the current row followed its app");
    // Back into the list: Delete and the menu act on Browser, not on Tool at the old index.
    assert!(tab_until(&ui, || controls.frozen()));
    assert_eq!(ui.get_current_id(), 9);
    press_key(&ui, Key::Delete);
    key(&ui, Key::Menu);
    assert_eq!(ui.get_menu_target(), 9);
    key(&ui, Key::DownArrow);
    key(&ui, Key::Return);
    assert_eq!(*actions.borrow(), [("end-task".to_string(), 9), ("end-task".to_string(), 9)]);
}

#[test]
fn the_list_and_its_current_row_are_exposed_to_screen_readers() {
    use slint::platform::Key;
    let (ui, controls, _bridge, _) = wired_list(vec![row_view(7, "Editor", ""), row_view(9, "Browser", "")]);
    let list = El::find_by_accessible_label(&ui, "Apps").next().expect("the list");
    assert_eq!(list.accessible_role(), Some(i_slint_backend_testing::AccessibleRole::List));
    assert!(tab_until(&ui, || controls.frozen()));
    press_key(&ui, Key::DownArrow);
    let editor = El::find_by_accessible_label(&ui, "Editor, CPU 1.0%, memory ").next().expect("editor row");
    let browser = El::find_by_accessible_label(&ui, "Browser, CPU 1.0%, memory ").next().expect("browser row");
    assert_eq!(browser.accessible_role(), Some(i_slint_backend_testing::AccessibleRole::ListItem));
    assert_eq!(browser.accessible_item_selectable(), Some(true));
    assert_eq!((browser.accessible_item_selected(), editor.accessible_item_selected()), (Some(true), Some(false)));
}

#[test]
fn a_mouse_click_on_a_row_never_selects_it() {
    let (ui, controls, _bridge, _) = wired_list(vec![row_view(7, "Editor", ""), row_view(9, "Browser", "")]);
    let row = El::find_by_accessible_label(&ui, "Browser, CPU 1.0%, memory ").next().expect("browser row");
    let (x, y) = centre(&row);
    move_pointer(&ui, x, y);
    press(&ui, x, y, slint::platform::PointerEventButton::Left);
    slint::platform::update_timers_and_animations();
    slint::platform::update_timers_and_animations();
    assert_eq!(ui.get_current_row(), -1, "no current row from a click");
    move_pointer(&ui, 5.0, 5.0);
    slint::platform::update_timers_and_animations();
    slint::platform::update_timers_and_animations();
    assert!(!controls.frozen(), "a click gave the list no keyboard focus");
}


#[test]
fn any_modal_drops_the_list_freeze_even_without_a_focus_out() {
    // A modal shown without parking focus first: the disabled list never sees its focus-out.
    let (ui, controls, _bridge, _) = wired_list(vec![row_view(7, "Editor", ""), row_view(9, "Browser", "")]);
    assert!(tab_until(&ui, || controls.frozen()));
    ui.set_details_visible(true);
    slint::platform::update_timers_and_animations();
    slint::platform::update_timers_and_animations();
    ui.set_details_visible(false);
    slint::platform::update_timers_and_animations();
    slint::platform::update_timers_and_animations();
    assert!(!controls.frozen(), "modal-open clears the list's keyboard focus");
}

#[test]
fn tabbing_from_the_list_to_a_rows_force_close_never_unfreezes_the_list() {
    use slint::platform::Key;
    let (ui, controls, bridge, _) = wired_list(vec![row_view(7, "Editor", HUNG), row_view(9, "Browser", "")]);
    assert!(tab_until(&ui, || controls.frozen()), "Tab reaches the list");
    assert_eq!(ui.get_current_id(), 7);
    // A re-sort and a new app wait while the keyboard is in the list.
    bridge.borrow_mut().apply(&ui, frame(vec![row_view(3, "Game", HUNG), row_view(9, "Browser", ""), row_view(7, "Editor", HUNG)]));
    assert_eq!(shown_ids(&ui), [7, 9]);
    // Tab on to Editor's Force close: the list's focus-out arrives before the button's focus-in.
    key(&ui, Key::Tab);
    for _ in 0..3 {
        slint::platform::update_timers_and_animations();
        assert!(controls.frozen(), "the hand-off from the list to a row control never releases the freeze");
        assert_eq!(shown_ids(&ui), [7, 9]);
    }
    // Shift+Tab back to the list: still frozen, still the same current row.
    let shift: slint::SharedString = Key::Shift.into();
    ui.window().dispatch_event(WindowEvent::KeyPressed { text: shift.clone() });
    key(&ui, Key::Tab);
    ui.window().dispatch_event(WindowEvent::KeyReleased { text: shift });
    for _ in 0..3 {
        slint::platform::update_timers_and_animations();
        assert!(controls.frozen());
        assert_eq!(shown_ids(&ui), [7, 9]);
    }
    assert_eq!(ui.get_current_id(), 7);
}

// ---- Task 11 fix round 1 ----

#[test]
fn a_key_that_reasserts_a_missed_list_focus_never_acts_on_an_unhighlighted_row() {
    use slint::platform::Key;
    let (ui, controls, bridge, actions) = wired_list(vec![row_view(7, "Editor", ""), row_view(9, "Browser", "")]);
    assert!(tab_until(&ui, || controls.frozen()));
    press_key(&ui, Key::DownArrow);
    assert_eq!(ui.get_current_id(), 9);
    // A modal shown without parking focus: the disabled list misses its focus-out, the freeze ends.
    ui.set_details_visible(true);
    slint::platform::update_timers_and_animations();
    ui.set_details_visible(false);
    slint::platform::update_timers_and_animations();
    assert!(!controls.frozen());
    // The current app exits while the list is live: it is simply gone.
    bridge.borrow_mut().apply(&ui, frame(vec![row_view(7, "Editor", "")]));
    assert_eq!(shown_ids(&ui), [7]);
    press_key(&ui, Key::Delete);
    assert!(actions.borrow().is_empty(), "Delete ended an app the ring never showed: {:?}", actions.borrow());
    assert!(controls.frozen(), "the key re-asserted the list focus");
    assert_eq!((ui.get_current_row(), ui.get_current_id()), (0, 7), "the ring now shows on Editor");
    press_key(&ui, Key::Delete);
    assert_eq!(*actions.borrow(), [("end-task".to_string(), 7)], "once shown, Delete acts on it");
}

#[test]
fn the_menu_key_brings_a_scrolled_away_current_row_back_before_anchoring_its_menu() {
    use slint::platform::Key;
    let rows: Vec<RowView> = (1..=40).map(|i| row_view(i, &format!("App{i}"), "")).collect();
    let (ui, controls, _bridge, _) = wired_list(rows);
    assert!(tab_until(&ui, || controls.frozen()));
    let list = El::find_by_accessible_label(&ui, "Apps").next().expect("the list");
    let (x, y) = centre(&list);
    ui.window().dispatch_event(WindowEvent::PointerScrolled { position: LogicalPosition::new(x, y), delta_x: 0.0, delta_y: -400.0 });
    slint::platform::update_timers_and_animations();
    assert!(ui.get_list_scroll_y() < -100.0);
    key(&ui, Key::Menu);
    assert_eq!(ui.get_menu_target(), 1);
    assert_eq!(ui.get_list_scroll_y(), 0.0, "the current row (the first) is back in view under its menu");
}

#[test]
fn the_current_row_reports_selected_only_while_the_list_has_focus() {
    use slint::platform::Key;
    let (ui, controls, _bridge, _) = wired_list(vec![row_view(7, "Editor", ""), row_view(9, "Browser", "")]);
    let editor = || El::find_by_accessible_label(&ui, "Editor, CPU 1.0%, memory ").next().expect("editor row");
    assert_eq!(editor().accessible_item_selected(), Some(false));
    assert!(tab_until(&ui, || controls.frozen()));
    assert_eq!(editor().accessible_item_selected(), Some(true));
    press_key(&ui, Key::Escape);
    assert_eq!(ui.get_current_id(), 7, "the current row is kept for Tab back");
    assert_eq!(editor().accessible_item_selected(), Some(false), "not selected while the list is unfocused");
}

// ---- Plan 3b Task 0: the menu key with nothing focused ----

fn shift_f10(ui: &MainWindow) {
    use slint::platform::Key;
    let shift: slint::SharedString = Key::Shift.into();
    ui.window().dispatch_event(WindowEvent::KeyPressed { text: shift.clone() });
    key(ui, Key::F10);
    ui.window().dispatch_event(WindowEvent::KeyReleased { text: shift });
    slint::platform::update_timers_and_animations();
    slint::platform::update_timers_and_animations();
}

fn menu_key(ui: &MainWindow) {
    press_key(ui, slint::platform::Key::Menu);
}

/// With no control focused (at startup, or after Esc left the list), the first menu key only
/// shows the ring on the current row (rulings R9-R11: nothing acts on a row not yet seen
/// highlighted); the second opens that row's menu.
fn the_menu_key_with_nothing_focused_highlights_first_then_opens(press: fn(&MainWindow)) {
    use slint::platform::Key;
    let (ui, controls, _bridge, actions) = wired_list(vec![row_view(7, "Editor", ""), row_view(9, "Browser", "")]);
    let opened = Rc::new(RefCell::new(Vec::new()));
    let o = opened.clone();
    ui.on_menu_open(move |open| o.borrow_mut().push(open));
    for round in ["at startup", "after Esc in the list"] {
        assert!(!controls.frozen(), "{round}: not frozen before the key");
        press(&ui);
        assert!(controls.frozen(), "{round}: the first press focuses (and so freezes) the list");
        assert_eq!(ui.get_current_id(), 7, "{round}: the ring is on the first row");
        assert!(opened.borrow().is_empty(), "{round}: no menu on the first press");
        assert!(actions.borrow().is_empty(), "{round}: nothing acted on");
        press(&ui);
        assert_eq!(*opened.borrow(), [true, false], "{round}: the second press opens the menu");
        assert_eq!(ui.get_menu_target(), 7, "{round}: on the highlighted row");
        press_key(&ui, Key::Escape); // closes the menu
        press_key(&ui, Key::Escape); // leaves the list
        assert!(!controls.frozen(), "{round}: Esc leaves the list and live ordering resumes");
        assert!(actions.borrow().is_empty(), "{round}: nothing acted on");
        opened.borrow_mut().clear();
    }
}

#[test]
fn shift_f10_with_nothing_focused_highlights_the_list_then_opens_its_menu() {
    the_menu_key_with_nothing_focused_highlights_first_then_opens(shift_f10);
}

#[test]
fn the_menu_key_with_nothing_focused_highlights_the_list_then_opens_its_menu() {
    the_menu_key_with_nothing_focused_highlights_first_then_opens(menu_key);
}

// ---- Plan 3b: settings ----

#[test]
fn set_dark_switches_the_theme_at_once() {
    let ui = window();
    ui.invoke_set_dark(true);
    assert!(ui.global::<negotium::Theme>().get_dark());
    ui.invoke_set_dark(false);
    assert!(!ui.global::<negotium::Theme>().get_dark());
}

#[test]
fn a_system_theme_flip_changes_the_colours_only_with_theme_system() {
    use negotium::platform::apply_theme_with;
    use negotium::settings::ThemeChoice;
    let ui = window();
    let dark = |ui: &MainWindow| ui.global::<negotium::Theme>().get_dark();
    for (choice, expect) in [(ThemeChoice::Light, false), (ThemeChoice::Dark, true)] {
        for system_dark in [false, true, false] {
            apply_theme_with(&ui, choice, system_dark);
            assert_eq!(dark(&ui), expect, "{choice:?} ignores a system flip to dark = {system_dark}");
        }
    }
    for system_dark in [false, true, false] {
        apply_theme_with(&ui, ThemeChoice::System, system_dark);
        assert_eq!(dark(&ui), system_dark, "System follows the flip to dark = {system_dark}");
    }
}


// ---- Plan 3b Task 3: the Settings page ----

use i_slint_backend_testing::AccessibleRole;
use negotium::renderer::Renderer;
use negotium::settings::{Settings, ThemeChoice};

/// The switch labelled `label` (a row title Text may carry the same words).
fn switch(ui: &MainWindow, label: &str) -> El {
    El::find_by_accessible_label(ui, label)
        .find(|e| e.accessible_role() == Some(AccessibleRole::Switch))
        .unwrap_or_else(|| panic!("no switch {label:?}"))
}

/// The choice button labelled `label` (its Text child carries the same label, PF11).
fn radio(ui: &MainWindow, label: &str) -> El {
    El::find_by_accessible_label(ui, label)
        .find(|e| e.accessible_role() == Some(AccessibleRole::RadioButton))
        .unwrap_or_else(|| panic!("no choice {label:?}"))
}

/// The Text showing `label` (a switch may carry the same words). A row's title and description
/// are not read out (their control carries them): find those with [`row_texts`].
fn text(ui: &MainWindow, label: &str) -> Option<El> {
    El::find_by_accessible_label(ui, label).find(|e| e.accessible_role() == Some(AccessibleRole::Text))
}

/// The rects of every Settings row's `id` Text ("title" or "description"), top to bottom.
fn row_texts(ui: &MainWindow, id: &str) -> Vec<Rect> {
    use i_slint_backend_testing::ElementRoot;
    ui.root_element().query_descendants().match_id(format!("SettingRow::{id}")).find_all().iter().map(rect_of).collect()
}

#[test]
fn the_settings_page_reports_each_choice() {
    let ui = shown_window();
    ui.set_page(3);
    ui.set_settings_keep_in_tray(true);
    let log = Rc::new(RefCell::new(Vec::<String>::new()));
    let l = log.clone();
    ui.on_refresh_chosen(move |i| l.borrow_mut().push(format!("refresh {i}")));
    let l = log.clone();
    ui.on_theme_chosen(move |i| l.borrow_mut().push(format!("theme {i}")));
    let l = log.clone();
    ui.on_always_on_top_toggled(move |on| l.borrow_mut().push(format!("top {on}")));
    let l = log.clone();
    ui.on_tray_toggled(move |on| l.borrow_mut().push(format!("tray {on}")));
    let l = log.clone();
    ui.on_gpu_rendering_toggled(move |on| l.borrow_mut().push(format!("gpu {on}")));
    radio(&ui, "2 s").invoke_accessible_default_action();
    radio(&ui, "Dark").invoke_accessible_default_action();
    switch(&ui, "Always on top").invoke_accessible_default_action();
    switch(&ui, "Keep running in the tray when closed").invoke_accessible_default_action();
    switch(&ui, "Use GPU rendering").invoke_accessible_default_action();
    assert_eq!(*log.borrow(), ["refresh 2", "theme 2", "top true", "tray false", "gpu false"]);
    // The owner sets the state; a switch or choice never changes itself.
    assert_eq!(switch(&ui, "Keep running in the tray when closed").accessible_checked(), Some(true));
    assert_eq!(radio(&ui, "1 s").accessible_checked(), Some(true));
    assert_eq!(radio(&ui, "2 s").accessible_checked(), Some(false));
}

#[test]
fn choices_and_switches_show_the_current_settings() {
    let ui = shown_window();
    ui.set_page(3);
    ui.set_settings_theme(1);
    ui.set_on_top(true);
    let light = radio(&ui, "Light");
    assert_eq!(light.accessible_checked(), Some(true));
    assert_eq!(radio(&ui, "System").accessible_checked(), Some(false));
    assert_eq!(switch(&ui, "Always on top").accessible_checked(), Some(true));
    ui.set_on_top(false);
    assert_eq!(switch(&ui, "Always on top").accessible_checked(), Some(false), "one always-on-top state (PF16)");
}

#[test]
fn later_version_settings_are_shown_disabled() {
    let ui = shown_window();
    ui.set_page(3);
    assert!(text(&ui, "Coming in a later version").is_some());
    for label in ["Admin mode", "Open on Ctrl+Shift+Esc", "CPU temperature add-on"] {
        let s = switch(&ui, label);
        assert_eq!(s.accessible_enabled(), Some(false), "{label}");
        assert_eq!(s.accessible_checked(), Some(false), "{label}");
        assert_eq!(s.accessible_description().as_deref(), Some("Coming in a later version"), "{label}");
        assert!(s.computed_opacity() < 1.0, "{label}: the disabled switch is dimmed");
    }
    assert_eq!(switch(&ui, "Always on top").computed_opacity(), 1.0, "an offered switch is not");
}

#[test]
fn an_open_modal_disables_the_settings() {
    let ui = shown_window();
    ui.set_page(3);
    let picked = Rc::new(RefCell::new(Vec::new()));
    let p = picked.clone();
    ui.on_theme_chosen(move |i| p.borrow_mut().push(i));
    let p = picked.clone();
    ui.on_always_on_top_toggled(move |_| p.borrow_mut().push(-1));
    ui.set_details_visible(true);
    assert_eq!(radio(&ui, "Dark").accessible_enabled(), Some(false));
    assert_eq!(switch(&ui, "Always on top").accessible_enabled(), Some(false));
    radio(&ui, "Dark").invoke_accessible_default_action();
    switch(&ui, "Always on top").invoke_accessible_default_action();
    assert!(picked.borrow().is_empty(), "nothing beneath a modal acts");
}

#[test]
fn the_rail_reaches_settings() {
    let ui = window();
    let picked = Rc::new(RefCell::new(None));
    let p = picked.clone();
    ui.on_select_page(move |i| *p.borrow_mut() = Some(i));
    El::find_by_accessible_label(&ui, "Settings page").next().expect("rail item").invoke_accessible_default_action();
    assert_eq!(*picked.borrow(), Some(3));
    ui.set_page(3);
    assert_eq!(El::find_by_accessible_label(&ui, "Settings page").next().unwrap().accessible_checked(), Some(true));
}

#[test]
fn a_frame_never_lands_on_the_settings_page() {
    let (ui, controls, bridge) = bridged();
    controls.set_page(Page::Settings);
    let mut b = bridge.borrow_mut();
    b.apply(&ui, frame(vec![row_view(1, "Editor", "")]));
    b.apply(&ui, Frame { page: Page::Settings, ..Default::default() });
    assert!(shown_ids(&ui).is_empty(), "an Overview frame is dropped while Settings shows");
}

#[test]
fn apply_settings_sets_every_wired_property() {
    let ui = window();
    let s = Settings { refresh_ms: 5000, theme: ThemeChoice::Dark, always_on_top: true, keep_in_tray: false, gpu_rendering: false };
    negotium::settingspage::apply_settings(&ui, &s);
    assert!(ui.get_on_top());
    assert_eq!(ui.get_settings_refresh(), 3);
    assert_eq!(ui.get_settings_theme(), 2);
    assert!(!ui.get_settings_keep_in_tray());
    assert!(!ui.get_settings_gpu_rendering());
    assert!(ui.global::<negotium::Theme>().get_dark(), "the theme is applied, not just shown");
    let s = Settings { theme: ThemeChoice::Light, ..Settings::default() };
    negotium::settingspage::apply_settings(&ui, &s);
    assert!(!ui.get_on_top());
    assert_eq!(ui.get_settings_refresh(), 1);
    assert_eq!(ui.get_settings_theme(), 1);
    assert!(ui.get_settings_keep_in_tray());
    assert!(ui.get_settings_gpu_rendering());
    assert!(!ui.global::<negotium::Theme>().get_dark());
}

/// A fresh temp directory, removed when dropped.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("negotium-ui-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Waits (up to 5 s) until `f` holds.
fn eventually(what: &str, f: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !f() {
        assert!(std::time::Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Waits until the saved settings file holds what `f` expects.
fn saved(path: &std::path::Path, what: &str, f: impl Fn(&Settings) -> bool) {
    eventually(what, || matches!(negotium::settings::load(path), (s, negotium::settings::Loaded::File) if f(&s)));
}

/// The Settings page wired the way main.rs wires it, saving under `dir`; this instance draws with
/// `running`.
fn settings_wired_with(
    dir: &Scratch,
    s: Settings,
    fallback: bool,
    running: Renderer,
) -> (MainWindow, Arc<Controls>, Rc<RefCell<Settings>>) {
    let ui = shown_window();
    ui.set_page(3);
    let controls = Arc::new(Controls::default());
    let settings = Rc::new(RefCell::new(s));
    let marker = dir.0.join("renderer-fallback");
    if fallback {
        negotium::renderer::record_fallback_at(&marker).unwrap();
    }
    negotium::settingspage::apply_settings(&ui, &settings.borrow());
    negotium::settingspage::SettingsWiring {
        settings: settings.clone(),
        saver: negotium::settings::Saver::new(dir.0.join("settings.json")),
        controls: controls.clone(),
        gpu_marker: marker,
        fallback_recorded: fallback,
        running,
    }
    .install(&ui);
    (ui, controls, settings)
}

/// As [`settings_wired_with`], running what `decide` picks for `s` and `fallback`.
fn settings_wired(dir: &Scratch, s: Settings, fallback: bool) -> (MainWindow, Arc<Controls>, Rc<RefCell<Settings>>) {
    let running = negotium::renderer::decide(false, s.gpu_rendering, fallback, false).renderer;
    settings_wired_with(dir, s, fallback, running)
}

#[test]
fn a_choice_applies_at_once_shows_and_is_saved() {
    let dir = Scratch::new("apply");
    let path = dir.0.join("settings.json");
    let (ui, controls, settings) = settings_wired(&dir, Settings { theme: ThemeChoice::Light, ..Settings::default() }, false);

    radio(&ui, "0.5 s").invoke_accessible_default_action();
    assert_eq!(controls.interval(), std::time::Duration::from_millis(500), "the worker ticks faster at once");
    assert_eq!(radio(&ui, "0.5 s").accessible_checked(), Some(true));
    saved(&path, "refresh 500 ms", |s| s.refresh_ms == 500);

    radio(&ui, "Dark").invoke_accessible_default_action();
    assert!(ui.global::<negotium::Theme>().get_dark(), "the theme applies at once");
    assert_eq!(radio(&ui, "Dark").accessible_checked(), Some(true));
    saved(&path, "theme dark", |s| s.theme == ThemeChoice::Dark);

    switch(&ui, "Always on top").invoke_accessible_default_action();
    assert!(ui.get_on_top(), "the window goes on top at once");
    assert_eq!(switch(&ui, "Always on top").accessible_checked(), Some(true));
    saved(&path, "always on top", |s| s.always_on_top);

    switch(&ui, "Keep running in the tray when closed").invoke_accessible_default_action();
    assert!(!settings.borrow().keep_in_tray, "the close handler reads the shared settings");
    assert_eq!(switch(&ui, "Keep running in the tray when closed").accessible_checked(), Some(false));
    saved(&path, "every choice", |s| {
        *s == Settings { refresh_ms: 500, theme: ThemeChoice::Dark, always_on_top: true, keep_in_tray: false, gpu_rendering: true }
    });
}

const FALLBACK_NOTE: &str =
    "Switched to CPU rendering because the graphics driver didn't respond. Turn on to try the GPU again.";
const RESTART_NOTE: &str = "Takes effect when Negotium restarts.";

#[test]
fn the_gpu_switch_takes_effect_on_the_next_launch() {
    let dir = Scratch::new("gpu");
    let path = dir.0.join("settings.json");
    let (ui, _controls, settings) = settings_wired(&dir, Settings::default(), false);
    let gpu = || switch(&ui, "Use GPU rendering");
    assert_eq!(gpu().accessible_checked(), Some(true));
    assert!(text(&ui, RESTART_NOTE).is_none() && text(&ui, FALLBACK_NOTE).is_none(), "no note at launch");
    gpu().invoke_accessible_default_action();
    assert_eq!(gpu().accessible_checked(), Some(false));
    assert!(!settings.borrow().gpu_rendering);
    assert!(text(&ui, RESTART_NOTE).is_some(), "turning it off says when it applies");
    saved(&path, "gpu off", |s| !s.gpu_rendering);
    gpu().invoke_accessible_default_action();
    assert_eq!(gpu().accessible_checked(), Some(true));
    assert!(text(&ui, RESTART_NOTE).is_none(), "back to what this launch uses: nothing to restart for");
    saved(&path, "gpu on", |s| s.gpu_rendering);
}

#[test]
fn after_an_automatic_fallback_the_gpu_switch_is_off_and_turning_it_on_tries_again() {
    let dir = Scratch::new("fallback");
    let path = dir.0.join("settings.json");
    let marker = dir.0.join("renderer-fallback");
    let (ui, _controls, settings) = settings_wired(&dir, Settings::default(), true);
    let gpu = || switch(&ui, "Use GPU rendering");
    assert_eq!(gpu().accessible_checked(), Some(false), "the fallback shows the switch off");
    assert!(text(&ui, FALLBACK_NOTE).is_some(), "and says why");
    assert!(text(&ui, RESTART_NOTE).is_none());
    gpu().invoke_accessible_default_action();
    assert_eq!(gpu().accessible_checked(), Some(true));
    assert!(text(&ui, FALLBACK_NOTE).is_none(), "the explanation goes");
    assert!(text(&ui, RESTART_NOTE).is_some(), "the GPU is tried on the next launch");
    assert!(settings.borrow().gpu_rendering);
    eventually("the fallback marker is cleared", || !negotium::renderer::fallback_recorded_at(&marker));
    saved(&path, "gpu on", |s| s.gpu_rendering);
}

/// The rect of each setting control on the page, by label.
fn settings_rects(ui: &MainWindow) -> Vec<(String, Rect)> {
    let mut out = Vec::new();
    for l in ["0.5 s", "5 s", "System", "Dark"] {
        out.push((l.to_string(), rect_of(&radio(ui, l))));
    }
    for l in ["Always on top", "Keep running in the tray when closed", "Use GPU rendering", "Admin mode", "CPU temperature add-on"] {
        out.push((l.to_string(), rect_of(&switch(ui, l))));
    }
    out
}

/// The Settings page alone at `w` × `h`.
fn settings_window(w: u32, h: u32) -> MainWindow {
    thread_local! { static BACKEND: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
    if !BACKEND.with(|b| b.replace(true)) {
        i_slint_backend_testing::init_no_event_loop();
    }
    let ui = MainWindow::new().unwrap();
    ui.window().set_size(PhysicalSize::new(w, h));
    ui.set_page(3);
    ui.show().unwrap();
    ui
}

#[test]
fn a_tall_window_leaves_the_settings_their_size_and_place() {
    let measure = |h: u32| {
        let ui = settings_window(1000, h);
        (settings_rects(&ui), rect_of(&text(&ui, "Settings").expect("page title")))
    };
    let (short, short_title) = measure(1300);
    let (tall, tall_title) = measure(2400);
    assert_eq!(short_title, tall_title, "the title stays at the top");
    assert_eq!(short, tall, "no setting moves or grows in a taller window");
    for (label, r) in &tall {
        assert!(r.3 <= 30.0, "{label} keeps its fixed height: {r:?}");
    }
}

const REFRESH_DESC: &str = "How often numbers update while the window is open. In the tray Negotium updates every 2 s, or slower if you choose a slower rate.";

#[test]
fn settings_rows_wrap_without_overlap_at_the_smallest_window() {
    let ui = settings_window(760, 440);
    let theme = row_texts(&ui, "title")[1];
    let desc = row_texts(&ui, "description")[0];
    assert!(desc.3 > 20.0, "the long description wraps onto more lines: {desc:?}");
    assert!(desc.1 + desc.3 <= theme.1, "the next row starts below it: {desc:?} {theme:?}");
    let choice = rect_of(&radio(&ui, "5 s"));
    assert!(choice.0 >= desc.0 + desc.2, "the choices stay to the right of the text: {choice:?} {desc:?}");
    // Rail 76 px; the page's scrollbar gutter is 14 px.
    assert!(choice.0 + choice.2 <= 760.0 - 14.0, "and clear of the scrollbar gutter: {choice:?}");
}

// ---- Plan 3b Task 3, fix round 1 ----

/// The rect of the Settings row holding the control whose log entry is `entry`.
fn row_of(ui: &MainWindow, entry: &str) -> Rect {
    use i_slint_backend_testing::ElementRoot;
    let id = match entry {
        e if e.starts_with("refresh") => "refresh-row",
        e if e.starts_with("theme") => "theme-row",
        "Always on top" => "top-row",
        "Keep running in the tray when closed" => "tray-row",
        _ => "gpu-row",
    };
    let rows = ui.root_element().query_descendants().match_id(format!("SettingsPage::{id}")).find_all();
    rect_of(rows.first().unwrap_or_else(|| panic!("row {id}")))
}

/// Tabs (then Shift+Tabs) through the Settings page at the smallest window. Pressing Space after
/// each move tells which control has focus (the page is not wired, so nothing changes); each
/// one reached must lie inside the view, never below or above the fold.
#[test]
fn a_control_reached_with_tab_is_scrolled_into_view() {
    use slint::platform::Key;
    let ui = settings_window(760, 440);
    // The GPU row at its tallest: with the fallback explanation.
    ui.set_settings_gpu_fallback(true);
    let log = Rc::new(RefCell::new(Vec::<String>::new()));
    let l = log.clone();
    ui.on_refresh_chosen(move |i| l.borrow_mut().push(format!("refresh {i}")));
    let l = log.clone();
    ui.on_theme_chosen(move |i| l.borrow_mut().push(format!("theme {i}")));
    let l = log.clone();
    ui.on_always_on_top_toggled(move |_| l.borrow_mut().push("Always on top".into()));
    let l = log.clone();
    ui.on_tray_toggled(move |_| l.borrow_mut().push("Keep running in the tray when closed".into()));
    let l = log.clone();
    ui.on_gpu_rendering_toggled(move |_| l.borrow_mut().push("Use GPU rendering".into()));
    let l = log.clone();
    ui.on_select_page(move |i| l.borrow_mut().push(format!("page {i}")));
    let in_view = |e: &El| {
        let r = rect_of(e);
        r.1 >= 0.0 && r.1 + r.3 <= 440.0
    };
    // Space acts on the focused control: its log entry names it.
    let current = |ui: &MainWindow| {
        log.borrow_mut().clear();
        press_key(ui, Key::Space);
        log.borrow().last().cloned()
    };
    let mut seen: Vec<String> = Vec::new();
    for _ in 0..30 {
        press_key(&ui, Key::Tab);
        let Some(now) = current(&ui) else { continue };
        let control = match now.as_str() {
            "refresh 0" => radio(&ui, "0.5 s"),
            "refresh 1" => radio(&ui, "1 s"),
            "refresh 2" => radio(&ui, "2 s"),
            "refresh 3" => radio(&ui, "5 s"),
            "theme 0" => radio(&ui, "System"),
            "theme 1" => radio(&ui, "Light"),
            "theme 2" => radio(&ui, "Dark"),
            label if label.starts_with("page") => continue,
            label => switch(&ui, label),
        };
        assert!(in_view(&control), "{now} is in view when focused: {:?}", rect_of(&control));
        let row = row_of(&ui, &now);
        assert!(row.1 >= 0.0 && row.1 + row.3 <= 440.0, "{now}: its whole row is in view: {row:?}");
        seen.push(now.clone());
        if now == "Use GPU rendering" {
            break;
        }
    }
    assert_eq!(seen.last().map(String::as_str), Some("Use GPU rendering"), "Tab reaches the last enabled control: {seen:?}");
    assert!(seen.contains(&"Keep running in the tray when closed".to_string()), "{seen:?}");
    // And back up: the first choice comes back into view.
    let mut back = None;
    for _ in 0..30 {
        press_key(&ui, Key::Backtab);
        back = current(&ui);
        if back.as_deref() == Some("refresh 0") {
            break;
        }
    }
    assert_eq!(back.as_deref(), Some("refresh 0"), "Shift+Tab reaches the first choice");
    assert!(in_view(&radio(&ui, "0.5 s")), "the first choice is back in view");
    let row = row_of(&ui, "refresh 0");
    assert!(row.1 >= 0.0, "with its whole row: {row:?}");
}

#[test]
fn screen_readers_hear_each_setting_once_with_its_description() {
    let ui = shown_window();
    ui.set_page(3);
    let group = El::find_by_accessible_label(&ui, "Refresh rate")
        .find(|e| e.accessible_role() == Some(AccessibleRole::Groupbox))
        .expect("the refresh choices are a labelled group");
    assert_eq!(group.accessible_description().as_deref(), Some(REFRESH_DESC));
    let tray = switch(&ui, "Keep running in the tray when closed");
    assert!(tray.accessible_description().is_some_and(|d| d.starts_with("Closing the window keeps Negotium")));
    for label in ["Refresh rate", "Keep running in the tray when closed", "Dark"] {
        assert_eq!(El::find_by_accessible_label(&ui, label).count(), 1, "{label:?} is announced once");
    }
    assert_eq!(El::find_by_accessible_label(&ui, REFRESH_DESC).count(), 0, "a description is read with its control only");
    assert_eq!(row_texts(&ui, "title").len(), 8, "the titles are still drawn");
}

#[test]
fn a_marker_that_cannot_be_cleared_brings_the_fallback_back() {
    let dir = Scratch::new("stuck-marker");
    let (ui, _controls, _settings) = settings_wired(&dir, Settings::default(), true);
    switch(&ui, "Use GPU rendering").invoke_accessible_default_action();
    assert!(text(&ui, FALLBACK_NOTE).is_none());
    // What the clearing thread posts back when the removal fails.
    negotium::settingspage::marker_not_cleared(&ui);
    assert_eq!(switch(&ui, "Use GPU rendering").accessible_checked(), Some(false), "the next launch is still CPU");
    assert!(text(&ui, FALLBACK_NOTE).is_some(), "and the page says why again");
}

#[test]
fn the_restart_note_compares_with_the_renderer_in_use() {
    // Settings say GPU, but this instance draws with the CPU (the GPU could not be selected):
    // the switch is on and the GPU arrives only with a restart.
    let dir = Scratch::new("running-cpu");
    let (ui, ..) = settings_wired_with(&dir, Settings::default(), false, Renderer::Software);
    assert_eq!(switch(&ui, "Use GPU rendering").accessible_checked(), Some(true));
    assert!(text(&ui, RESTART_NOTE).is_some());
    switch(&ui, "Use GPU rendering").invoke_accessible_default_action();
    assert!(text(&ui, RESTART_NOTE).is_none(), "off matches what runs: no restart needed");
}

#[test]
fn slint_backend_overrides_the_gpu_switch_and_says_so() {
    let dir = Scratch::new("env");
    let (ui, ..) = settings_wired_with(&dir, Settings::default(), false, Renderer::FromEnv);
    assert!(text(&ui, "Overridden by SLINT_BACKEND.").is_some());
    switch(&ui, "Use GPU rendering").invoke_accessible_default_action();
    assert!(text(&ui, "Overridden by SLINT_BACKEND.").is_some(), "instead of the restart note");
    assert!(text(&ui, RESTART_NOTE).is_none());
}
