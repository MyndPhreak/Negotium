use i_slint_backend_testing::ElementHandle;
use negotium::{DeviceData, GraphData, SegmentData, StartupData, StatData};
use negotium::appdata::Writes;
use negotium::controls::Page;
use negotium::perf::{DeviceView, GraphView, PerfFrame, Range, StatView};
use negotium::worker::SharedHistory;
use negotium::ScrubRow;
use negotium::{MainWindow, MeterData, RowData};
use slint::Model as _;
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
        disk: "0 B/s".into(), gpu: "—".into(), net: "—".into(), kind: 1, hung: "Not responding · 9s".into(),
        expandable: false, expanded: false, actionable: true, exited: false, ..Default::default()
    }, RowData {
        id: 9, depth: 0, name: "Browser".into(), count: "(3)".into(), cpu: "2.0%".into(), mem: "2.0 MB".into(),
        disk: "0 B/s".into(), gpu: "—".into(), net: "—".into(), kind: 1, hung: "".into(),
        expandable: true, expanded: false, actionable: true, exited: false, ..Default::default()
    }, RowData {
        id: 11, depth: 0, name: "Windows".into(), count: "(90)".into(), cpu: "3.0%".into(), mem: "3.0 MB".into(),
        disk: "0 B/s".into(), gpu: "—".into(), net: "—".into(), kind: 1, hung: "".into(),
        expandable: true, expanded: false, actionable: false, exited: false, ..Default::default()
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
        id: 3, name: "Chrome".into(), value: "5%".into(), weight: 1.0, hung: false, leaving: false,
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
    // Frames in these tests carry made-up collector times: the graphs stand still (newest sample
    // on the edge) unless a test turns smoothing on with an epoch that matches its frames.
    ui.global::<negotium::GraphClock>().set_smooth(false);
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

/// Records what the bridge asks the icon loader for (the real one extracts on its own thread).
struct AskedIcons(Rc<RefCell<Vec<String>>>);

impl negotium::icons::RequestIcon for AskedIcons {
    fn request(&self, path: &str) {
        self.0.borrow_mut().push(path.to_string());
    }
}

fn icon_px() -> Option<negotium::icons::IconPixels> {
    Some(slint::SharedPixelBuffer::new(negotium::icons::ICON_PX, negotium::icons::ICON_PX))
}

#[test]
fn rows_ask_for_their_icon_once_and_show_it_when_it_arrives() {
    use slint::Model;
    let (ui, _controls, bridge) = bridged();
    let asked = Rc::new(RefCell::new(Vec::new()));
    bridge.borrow_mut().set_icon_loader(AskedIcons(asked.clone()));
    let editor = RowView { icon: r"C:\editor.exe".into(), ..row_view(1, "Editor", "") };
    // The Windows group draws its own glyph; a row without an image path has nothing to ask for.
    let group = RowView { icon: r"C:\Windows\explorer.exe".into(), windows: true, actionable: false, ..row_view(2, "Windows", "") };
    let rows = vec![editor, group, row_view(3, "Unknown", "")];
    bridge.borrow_mut().apply(&ui, frame(rows.clone()));
    bridge.borrow_mut().apply(&ui, frame(rows.clone()));
    assert_eq!(*asked.borrow(), [r"C:\editor.exe"], "one request, only for a real program image");
    assert!(!ui.get_rows().row_data(0).unwrap().has_icon, "not here yet");
    negotium::bridge::icon_arrived(&bridge, r"C:\editor.exe".into(), icon_px());
    i_slint_backend_testing::mock_elapsed_time(negotium::bridge::ICON_BATCH);
    let shown = ui.get_rows().row_data(0).unwrap();
    assert!(shown.has_icon, "the row updates once its icon lands");
    let size = shown.icon.size();
    assert_eq!((size.width, size.height), (32, 32));
    assert!(ui.get_rows().iter().skip(1).all(|r| !r.has_icon));
    bridge.borrow_mut().apply(&ui, frame(rows));
    assert_eq!(asked.borrow().len(), 1, "never asked for again");
}

/// A launch brings dozens of icons one by one: they are gathered and the rows pushed once, not
/// once per icon (each push redraws the list).
#[test]
fn icon_arrivals_are_gathered_into_one_push() {
    use slint::Model;
    let (ui, _controls, bridge) = bridged();
    bridge.borrow_mut().set_icon_loader(AskedIcons(Default::default()));
    let rows = vec![
        RowView { icon: r"C:\editor.exe".into(), ..row_view(1, "Editor", "") },
        RowView { icon: r"C:rowser.exe".into(), ..row_view(2, "Browser", "") },
        RowView { icon: r"C:\Windows\explorer.exe".into(), windows: true, actionable: false, ..row_view(3, "Windows", "") },
    ];
    bridge.borrow_mut().apply(&ui, frame(rows));
    let has_icon = || ui.get_rows().iter().map(|r| r.has_icon).collect::<Vec<_>>();
    negotium::bridge::icon_arrived(&bridge, r"C:\editor.exe".into(), icon_px());
    negotium::bridge::icon_arrived(&bridge, r"C:rowser.exe".into(), icon_px());
    assert_eq!(has_icon(), [false, false, false], "stored, not pushed yet");
    i_slint_backend_testing::mock_elapsed_time(negotium::bridge::ICON_BATCH);
    assert_eq!(has_icon(), [true, true, false], "both in one push");
    // A path only the Windows group shows (it draws its own glyph) needs no push at all.
    assert!(!bridge.borrow_mut().put_icon(r"C:\Windows\explorer.exe".into(), icon_px()));
}

/// Slint's empty image never equals itself, which would make every iconless row look changed on
/// every frame: rows without an icon must still compare equal, so `sync_model` leaves them be.
#[test]
fn a_row_without_an_icon_is_unchanged_by_an_identical_frame() {
    use slint::Model;
    let (ui, _controls, bridge) = bridged();
    let mut b = bridge.borrow_mut();
    let rows = vec![row_view(1, "Editor", "")];
    b.apply(&ui, frame(rows.clone()));
    let before = ui.get_rows().row_data(0).unwrap();
    b.apply(&ui, frame(rows));
    assert_eq!(ui.get_rows().row_data(0).unwrap(), before);
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
        .map(|i| negotium::TileData { id: i, name: format!("App {i}").into(), value: "5%".into(), weight: 1.0, hung: false, leaving: false })
        .collect();
    ui.set_tiles(ModelRc::new(VecModel::from(tiles)));
    ui.set_top_caption("Top CPU users".into());
    ui.set_rows(filled.get_rows());
    let (meter_h2, search_y2) = measure();
    assert_eq!((meter_h2, search_y2), (meter_h, search_y), "meter height and search box position stay put");
    assert!(meter_h < 130.0, "the meters keep their natural height: {meter_h}");
    let rail = El::find_by_accessible_label(&ui, "Performance page").next().expect("rail item");
    assert_eq!(rail.size().height, 48.0, "the rail gives no spare height to its buttons");
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
    DeviceData { id: id.into(), title: title.into(), subtitle: "".into(), value: "1%".into(), spark: "M 0 100".into() }
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
        id: 3, name: "Chrome".into(), value: "5%".into(), weight: 1.0, hung: false, leaving: false,
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
        id: 3, name: "Chrome".into(), value: "5%".into(), weight: 1.0, hung: false, leaving: false,
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
    // The page as tall as under a native caption: Negotium's caption takes the window's top rows.
    ui.window().set_size(PhysicalSize::new(1000, 720 + negotium::platform::CAPTION_HEIGHT as u32));
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
    // One testing backend per test thread (`page_window`); a test may build several windows.
    let ui = page_window(1, w, h);
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
            processes: vec![], cpu_pct: s as f32, private_bytes: 0, disk_bps: 0.0, gpu_pct: None, net_bps: None, net: None, hung_for_ms: None,
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
fn the_network_scrub_card_notes_all_adapters() {
    let (ui, controls, bridge) = bridged();
    controls.set_page(Page::Performance);
    bridge.borrow_mut().set_history(scrub_history());
    let mut net = scrub_frame(60_000, TWO_PM);
    let p = net.perf.as_mut().unwrap();
    (p.scrub_metric, p.net_feed) = (negotium_model::Metric::Network, negotium::perf::NetFeedState::On);
    bridge.borrow_mut().apply(&ui, net);
    bridge.borrow_mut().scrub(&ui, 0.5);
    assert_eq!(ui.get_scrub_note(), "All adapters");
    bridge.borrow_mut().apply(&ui, scrub_frame(61_000, TWO_PM + 1_000));
    assert_eq!(ui.get_scrub_note(), "", "a CPU page has no note");
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

/// The app list's scroll indicator (fix wave R9: the Fluent scrollbar's animated thumb is off):
/// there only while the rows overflow, it follows the wheel and can be dragged.
#[test]
fn the_app_lists_scroll_indicator_follows_the_wheel_and_drags_the_list() {
    let thumb = |ui: &MainWindow| El::find_by_element_id(ui, "ScrollIndicator::thumb").next();
    let rows = |n: i32| frame((1..=n).map(|i| row_view(i, &format!("App{i}"), "")).collect());
    let (ui, _controls, bridge, _) = wired_list(rows(3).rows);
    assert!(thumb(&ui).is_none(), "three rows fit: no indicator");
    bridge.borrow_mut().apply(&ui, rows(60));
    let list = rect_of(&El::find_by_element_id(&ui, "AppList::lv").next().expect("the list view"));
    let t = thumb(&ui).expect("sixty rows overflow: an indicator");
    let (top, size) = (t.absolute_position(), t.size());
    assert!(size.width <= 4.0 && top.x + size.width >= list.0 + list.2 - 4.0, "thin, on the right edge: {top:?} {size:?} in {list:?}");
    assert!(size.height >= 24.0 && size.height < list.3 / 2.0, "sized to the visible share: {size:?}");
    let (x, y) = (list.0 + list.2 / 2.0, list.1 + list.3 / 2.0);
    ui.window().dispatch_event(WindowEvent::PointerScrolled { position: LogicalPosition::new(x, y), delta_x: 0.0, delta_y: -400.0 });
    slint::platform::update_timers_and_animations();
    assert!(ui.get_list_scroll_y() < -100.0, "the wheel still scrolls the list");
    let moved = thumb(&ui).unwrap().absolute_position();
    assert!(moved.y > top.y + 10.0, "the indicator follows: {} then {}", top.y, moved.y);
    // The wheel over the indicator's own strip scrolls the list too, and stops at the top.
    let strip = (list.0 + list.2 - 3.0, list.1 + list.3 - 20.0);
    let before = ui.get_list_scroll_y();
    ui.window().dispatch_event(WindowEvent::PointerScrolled { position: LogicalPosition::new(strip.0, strip.1), delta_x: 0.0, delta_y: -100.0 });
    slint::platform::update_timers_and_animations();
    assert!(ui.get_list_scroll_y() < before - 50.0, "over the indicator: {before} then {}", ui.get_list_scroll_y());
    ui.window().dispatch_event(WindowEvent::PointerScrolled { position: LogicalPosition::new(strip.0, strip.1), delta_x: 0.0, delta_y: 100_000.0 });
    slint::platform::update_timers_and_animations();
    assert_eq!(ui.get_list_scroll_y(), 0.0, "clamped at the top");
    // Held, the bar has every pointer event (nothing falls through to the list): the wheel still scrolls.
    let (at, held) = (LogicalPosition::new(strip.0, strip.1), slint::platform::PointerEventButton::Left);
    ui.window().dispatch_event(WindowEvent::PointerPressed { position: at, button: held });
    ui.window().dispatch_event(WindowEvent::PointerScrolled { position: at, delta_x: 0.0, delta_y: -100.0 });
    slint::platform::update_timers_and_animations();
    assert_eq!(ui.get_list_scroll_y(), -100.0, "the wheel over a held bar");
    ui.window().dispatch_event(WindowEvent::PointerReleased { position: at, button: held });
    // Dragged to the bottom: the list shows its last rows.
    let (tx, ty) = centre(&thumb(&ui).unwrap());
    let button = slint::platform::PointerEventButton::Left;
    ui.window().dispatch_event(WindowEvent::PointerPressed { position: LogicalPosition::new(tx, ty), button });
    // A drag is many moves: the list's height estimate settles as its last rows are created.
    for dy in [100.0, 199.0, 200.0] {
        ui.window().dispatch_event(WindowEvent::PointerMoved { position: LogicalPosition::new(tx, list.1 + list.3 + dy) });
    }
    ui.window().dispatch_event(WindowEvent::PointerReleased { position: LogicalPosition::new(tx, list.1 + list.3 + 200.0), button });
    let bottom = rect_of(&thumb(&ui).unwrap());
    assert!((bottom.1 + bottom.3 - (list.1 + list.3)).abs() <= 3.0, "the indicator at the bottom: {bottom:?} in {list:?}");
    assert!(text(&ui, "App60").is_some_and(|e| rect_of(&e).1 < list.1 + list.3), "the last row is in view");
}

#[test]
fn the_performance_details_indicator_appears_on_overflow_and_follows_the_wheel() {
    let thumbs = |ui: &MainWindow| El::find_by_element_id(ui, "ScrollIndicator::thumb").filter(|t| t.size().width > 0.0).collect::<Vec<_>>();
    let ui = shown_window();
    ui.set_page(1);
    ui.set_perf_devices(ModelRc::new(VecModel::from(vec![device("cpu", "CPU")])));
    ui.set_perf_graphs(ModelRc::new(VecModel::from(vec![graph_data("% Utilization")])));
    assert!(thumbs(&ui).is_empty(), "a short page needs no indicator");
    let cells: Vec<GraphData> = (0..64).map(|i| graph_data(&format!("CPU {i}"))).collect();
    ui.set_perf_cells(ModelRc::new(VecModel::from(cells)));
    slint::platform::update_timers_and_animations();
    let found = thumbs(&ui);
    assert_eq!(found.len(), 1, "sixty-four cells overflow the page: one indicator");
    let (top, size) = (found[0].absolute_position(), found[0].size());
    assert!(size.width <= 4.0 && size.height >= 24.0, "thin and sized to the visible share: {size:?}");
    assert!(top.x > 700.0, "on the details' right edge: {top:?}");
    ui.window().dispatch_event(WindowEvent::PointerScrolled { position: LogicalPosition::new(600.0, 400.0), delta_x: 0.0, delta_y: -400.0 });
    slint::platform::update_timers_and_animations();
    let moved = thumbs(&ui)[0].absolute_position();
    assert!(moved.y > top.y + 10.0, "the indicator follows content-y: {} then {}", top.y, moved.y);
    // The wheel over the indicator's own strip scrolls the details too, within their bounds.
    let x = moved.x + 2.0;
    ui.window().dispatch_event(WindowEvent::PointerScrolled { position: LogicalPosition::new(x, 300.0), delta_x: 0.0, delta_y: -100_000.0 });
    slint::platform::update_timers_and_animations();
    let bottom = thumbs(&ui)[0].absolute_position();
    assert!(bottom.y > moved.y + 10.0, "over the indicator: {} then {}", moved.y, bottom.y);
    ui.window().dispatch_event(WindowEvent::PointerScrolled { position: LogicalPosition::new(x, 300.0), delta_x: 0.0, delta_y: 100_000.0 });
    slint::platform::update_timers_and_animations();
    assert_eq!(thumbs(&ui)[0].absolute_position().y, top.y, "back at the top, and no further");
    // Held, the bar has every pointer event (nothing falls through to the details): the wheel still scrolls.
    let (at, held) = (LogicalPosition::new(x, 700.0), slint::platform::PointerEventButton::Left);
    ui.window().dispatch_event(WindowEvent::PointerPressed { position: at, button: held });
    ui.window().dispatch_event(WindowEvent::PointerScrolled { position: at, delta_x: 0.0, delta_y: -400.0 });
    slint::platform::update_timers_and_animations();
    assert!(thumbs(&ui)[0].absolute_position().y > top.y + 10.0, "the wheel over a held bar");
    ui.window().dispatch_event(WindowEvent::PointerReleased { position: at, button: held });
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

// ---- App list redesign Task 4: sections collapse, and the list scrolls by real row tops ----

fn heading(id: i32, name: &str) -> RowView {
    RowView { id, parent: -1, depth: 0, kind: negotium::view::RowKind::Section, name: name.into(), expandable: true, ..Default::default() }
}

fn shown_expanded(ui: &MainWindow, id: i32) -> bool {
    use slint::Model;
    ui.get_rows().iter().find(|r| r.id == id).is_some_and(|r| r.expanded)
}

#[test]
fn collapsing_a_section_hides_its_cards_and_reopens_on_a_hang() {
    use negotium::view::{SECTION_APPS, SECTION_BACKGROUND};
    let (ui, controls, bridge) = bridged();
    let rows = |hung: &str| frame(vec![
        heading(SECTION_APPS, "Apps"), row_view(1, "Editor", ""),
        heading(SECTION_BACKGROUND, "Background processes"), row_view(2, "Updater", hung), row_view(3, "Agent", ""),
    ]);
    let (open, closed) = ([SECTION_APPS, 1, SECTION_BACKGROUND, 2, 3], [SECTION_APPS, 1, SECTION_BACKGROUND]);
    let mut b = bridge.borrow_mut();
    b.apply(&ui, rows(""));
    assert_eq!(shown_ids(&ui), open);
    assert!(shown_expanded(&ui, SECTION_BACKGROUND), "sections start open");
    b.toggle(SECTION_BACKGROUND);
    assert_eq!(shown_ids(&ui), closed, "its cards go; the heading stays");
    assert!(!shown_expanded(&ui, SECTION_BACKGROUND));
    assert!(!controls.child_rows().0.contains(&SECTION_BACKGROUND), "a section is never published to the worker");
    b.apply(&ui, rows(""));
    assert_eq!(shown_ids(&ui), closed, "it stays closed across frames");
    b.apply(&ui, rows(HUNG));
    assert_eq!(shown_ids(&ui), open, "a hung app reopens its section");
    b.toggle(SECTION_BACKGROUND);
    b.apply(&ui, rows(HUNG));
    assert_eq!(shown_ids(&ui), closed, "closed again during the same hang: it stays closed");
    b.apply(&ui, rows(""));
    b.apply(&ui, rows(HUNG));
    assert_eq!(shown_ids(&ui), open, "a new hang reopens it");
    b.toggle(SECTION_BACKGROUND);
    b.open_tile(3);
    assert_eq!(shown_ids(&ui), open, "opening a tile opens its app's section");
}

#[test]
fn left_and_right_on_a_heading_close_and_open_its_section_while_the_list_has_focus() {
    use negotium::view::{SECTION_APPS, SECTION_BACKGROUND};
    use slint::platform::Key;
    let (ui, controls, _bridge, _) = wired_list(vec![
        heading(SECTION_APPS, "Apps"), row_view(1, "Editor", ""),
        heading(SECTION_BACKGROUND, "Background processes"), row_view(2, "Updater", ""), row_view(3, "Agent", ""),
    ]);
    assert!(tab_until(&ui, || controls.frozen()));
    assert_eq!(ui.get_current_id(), SECTION_APPS);
    press_key(&ui, Key::DownArrow);
    press_key(&ui, Key::DownArrow);
    assert_eq!(ui.get_current_id(), SECTION_BACKGROUND);
    press_key(&ui, Key::LeftArrow);
    assert_eq!(shown_ids(&ui), [SECTION_APPS, 1, SECTION_BACKGROUND], "closed in place, though the list is frozen");
    press_key(&ui, Key::RightArrow);
    assert_eq!(shown_ids(&ui), [SECTION_APPS, 1, SECTION_BACKGROUND, 2, 3]);
    press_key(&ui, Key::RightArrow);
    assert_eq!(ui.get_current_id(), 2, "into the open section");
}

/// Closing a section with the mouse while the keyboard-current row is a card inside it: the
/// current row moves to that heading rather than being lost with the card.
#[test]
fn closing_a_section_with_the_mouse_moves_the_current_row_out_of_it_to_its_heading() {
    use negotium::view::{SECTION_APPS, SECTION_BACKGROUND};
    use slint::platform::Key;
    let (ui, controls, bridge, _) = wired_list(vec![
        heading(SECTION_APPS, "Apps"), row_view(1, "Editor", ""),
        heading(SECTION_BACKGROUND, "Background processes"), row_view(2, "Updater", ""), row_view(3, "Agent", ""),
    ]);
    assert!(tab_until(&ui, || controls.frozen()));
    for _ in 0..4 {
        press_key(&ui, Key::DownArrow);
    }
    assert_eq!(ui.get_current_id(), 3);
    bridge.borrow_mut().toggle(SECTION_BACKGROUND); // a click on the heading
    assert_eq!(shown_ids(&ui), [SECTION_APPS, 1, SECTION_BACKGROUND]);
    assert_eq!((ui.get_current_row(), ui.get_current_id()), (2, SECTION_BACKGROUND), "the heading is current now");
    // Closing another section leaves the current row where it is.
    bridge.borrow_mut().toggle(SECTION_APPS);
    assert_eq!(ui.get_current_id(), SECTION_BACKGROUND);
}

/// The scroll position ensure-visible sets from the real row tops, and the card as drawn there.
#[test]
fn page_down_reveals_the_card_it_lands_on() {
    use negotium::view::{RowKind, SECTION_APPS, visible_rows};
    use slint::platform::Key;
    use std::collections::HashSet;
    let proc = |id: i32| RowView { id, parent: 2, depth: 1, kind: RowKind::Proc, name: format!("proc{id}"), actionable: true, ..Default::default() };
    let mut rows = vec![heading(SECTION_APPS, "Apps")];
    for i in 1..=40 {
        rows.push(RowView { expandable: i == 2, ..row_view(i, &format!("App{i}"), "") });
        if i == 2 {
            rows.extend([proc(101), proc(102)]);
        }
    }
    let (ui, controls, bridge, _) = wired_list(rows.clone());
    bridge.borrow_mut().toggle(2);
    assert!(tab_until(&ui, || controls.frozen()));
    assert_eq!(ui.get_current_id(), SECTION_APPS);
    press_key(&ui, Key::PageDown);
    press_key(&ui, Key::PageDown);
    let shown = visible_rows(&rows, &HashSet::from([2]), &HashSet::new(), "");
    assert_eq!(ui.get_current_id(), shown[20].id);
    let (y, h) = negotium::bridge::row_top(&shown, 20);
    assert_eq!((y, h), (32.0 + 50.0 * 2.0 + 30.0 * 2.0 + 50.0 * 15.0, 50.0), "heading, two cards, two processes, fifteen cards");
    assert_eq!(ui.get_current_row_y(), y);
    let visible = rect_of(&El::find_by_element_id(&ui, "AppList::lv").next().expect("the list view")).3;
    assert!(y + h > visible, "the target starts out of view");
    assert_eq!(ui.get_list_scroll_y(), -(y + h - visible), "the card's bottom edge sits on the list's");
    // Drawn where Rust summed it: the whole card inside the list's visible rect, flush with its bottom.
    let list = rect_of(&El::find_by_element_id(&ui, "AppList::lv").next().unwrap());
    let card = rect(&ui, &format!("{}, CPU 1.0%, memory ", shown[20].name));
    assert!(card.1 >= list.1 - 0.5 && card.1 + card.3 <= list.1 + list.3 + 0.5, "the card {card:?} lies within the list {list:?}");
    assert!((card.1 + card.3 - (list.1 + list.3)).abs() <= 1.0, "its bottom edge on the list's: {card:?} in {list:?}");
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
    // Tall enough for every row: the testing backend finds only what the page's view shows.
    let ui = settings_window(1000, 2600);
    ui.set_settings_keep_in_tray(true);
    let log = Rc::new(RefCell::new(Vec::<String>::new()));
    let l = log.clone();
    ui.on_refresh_chosen(move |i| l.borrow_mut().push(format!("refresh {i}")));
    let l = log.clone();
    ui.on_theme_chosen(move |i| l.borrow_mut().push(format!("theme {i}")));
    let l = log.clone();
    ui.on_backdrop_chosen(move |i| l.borrow_mut().push(format!("backdrop {i}")));
    let l = log.clone();
    ui.on_always_on_top_toggled(move |on| l.borrow_mut().push(format!("top {on}")));
    let l = log.clone();
    ui.on_tray_toggled(move |on| l.borrow_mut().push(format!("tray {on}")));
    let l = log.clone();
    ui.on_gpu_rendering_toggled(move |on| l.borrow_mut().push(format!("gpu {on}")));
    radio(&ui, "2 s").invoke_accessible_default_action();
    radio(&ui, "Dark").invoke_accessible_default_action();
    radio(&ui, "Acrylic").invoke_accessible_default_action();
    switch(&ui, "Always on top").invoke_accessible_default_action();
    switch(&ui, "Keep running in the tray when closed").invoke_accessible_default_action();
    switch(&ui, "Use GPU rendering").invoke_accessible_default_action();
    assert_eq!(*log.borrow(), ["refresh 2", "theme 2", "backdrop 1", "top true", "tray false", "gpu false"]);
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
    // Tall enough for the last row: the testing backend finds only what the page's view shows.
    let ui = settings_window(1000, 2600);
    assert!(text(&ui, "Coming in a later version").is_some());
    let s = switch(&ui, "CPU temperature add-on");
    assert_eq!(s.accessible_enabled(), Some(false));
    assert_eq!(s.accessible_checked(), Some(false));
    assert_eq!(s.accessible_description().as_deref(), Some("Coming in a later version"));
    assert!(s.computed_opacity() < 1.0, "the disabled switch is dimmed");
    assert_eq!(switch(&ui, "Always on top").computed_opacity(), 1.0, "an offered switch is not");
}

const PICKER_TITLE: &str = "Pick a window to end on Ctrl+Alt+Shift+Esc";
const PICKER_TEXT: &str = "Press the keys, then click a window to end its app. Shift+click ends it with no question.";
const PICKER_TAKEN: &str = "Another program uses Ctrl+Alt+Shift+Esc";

#[test]
fn picker_row_is_shown_in_a_dev_build() {
    let ui = settings_window(1000, 1900);
    negotium::settingspage::show_install(&ui, &negotium::install::Install { exe: "target\\release\\negotium.exe".into(), installed: false });
    let s = switch(&ui, PICKER_TITLE);
    assert_eq!(s.accessible_enabled(), Some(true), "no installed gate");
    assert_eq!(s.accessible_checked(), Some(false), "off by default");
    assert_eq!(s.accessible_description().as_deref(), Some(PICKER_TEXT));
    ui.set_settings_picker_shortcut(true);
    assert_eq!(switch(&ui, PICKER_TITLE).accessible_checked(), Some(true));
}

#[test]
fn picker_row_shows_the_taken_text() {
    let ui = settings_window(1000, 1900);
    ui.set_settings_picker_shortcut(true);
    ui.set_settings_shortcut_taken(true);
    assert_eq!(switch(&ui, PICKER_TITLE).accessible_description().as_deref(), Some(PICKER_TAKEN));
}

#[test]
fn the_picker_switch_saves_even_when_the_keys_are_taken() {
    let dir = Scratch::new("pickershortcut");
    let path = dir.0.join("settings.json");
    SHORTCUT_TOLD.with(|t| t.borrow_mut().clear());
    SHORTCUT_FREE.with(|f| f.set(false));
    let (ui, _controls, settings) = settings_wired_with(&dir, Settings::default(), false, Renderer::Software);
    ui.invoke_picker_shortcut_toggled(true);
    assert!(settings.borrow().picker_shortcut, "the choice is kept (spec goal 4)");
    saved(&path, "picker shortcut on", |s| s.picker_shortcut);
    assert_eq!(SHORTCUT_TOLD.with(|t| t.borrow().clone()), vec![true]);
    assert!(ui.get_settings_shortcut_taken(), "taken: on and not registered");
    SHORTCUT_FREE.with(|f| f.set(true));
    ui.invoke_picker_shortcut_toggled(false);
    assert!(!ui.get_settings_shortcut_taken());
    saved(&path, "picker shortcut off", |s| !s.picker_shortcut);
}

const INSTALL_ROWS: [&str; 4] = ["Admin mode", "Open on Ctrl+Shift+Esc", "Start with Windows", "Check for updates on startup"];

#[test]
fn a_dev_build_disables_the_integration_rows_and_says_why() {
    let ui = settings_window(1000, 2600);
    negotium::settingspage::show_install(&ui, &negotium::install::Install { exe: "target\release\negotium.exe".into(), installed: false });
    for label in INSTALL_ROWS {
        let s = switch(&ui, label);
        assert_eq!(s.accessible_enabled(), Some(false), "{label}");
        assert_eq!(s.accessible_checked(), Some(false), "{label}");
        assert!(s.accessible_description().is_some_and(|d| d == "Install Negotium to use this"), "{label}: {:?}", s.accessible_description());
        assert!(s.computed_opacity() < 1.0, "{label}: dimmed");
    }
    let button = El::find_by_accessible_label(&ui, "Check for updates").find(|e| e.accessible_role() == Some(AccessibleRole::Button)).unwrap();
    assert_eq!(button.accessible_enabled(), Some(false), "no updates for a dev build");
    let tm = El::find_by_accessible_label(&ui, "Open Windows Task Manager").find(|e| e.accessible_role() == Some(AccessibleRole::Button)).unwrap();
    assert_eq!(tm.accessible_enabled(), Some(true), "Task Manager opens from any build");
}

#[test]
fn an_installed_copy_offers_the_integration_rows_and_reports_them() {
    let ui = settings_window(1000, 2600);
    negotium::settingspage::show_install(&ui, &negotium::install::Install { exe: "x".into(), installed: true });
    let log = Rc::new(RefCell::new(Vec::<String>::new()));
    let l = log.clone();
    ui.on_admin_mode_toggled(move |on| l.borrow_mut().push(format!("admin {on}")));
    let l = log.clone();
    ui.on_hotkey_toggled(move |on| l.borrow_mut().push(format!("hotkey {on}")));
    let l = log.clone();
    ui.on_start_with_windows_toggled(move |on| l.borrow_mut().push(format!("start {on}")));
    let l = log.clone();
    ui.on_check_updates_toggled(move |on| l.borrow_mut().push(format!("updates {on}")));
    let l = log.clone();
    ui.on_check_updates_now(move || l.borrow_mut().push("now".to_string()));
    let l = log.clone();
    ui.on_open_windows_task_manager(move || l.borrow_mut().push("taskmgr".to_string()));
    for label in INSTALL_ROWS {
        assert_eq!(switch(&ui, label).accessible_enabled(), Some(true), "{label}");
        switch(&ui, label).invoke_accessible_default_action();
    }
    for label in ["Check for updates", "Open Windows Task Manager"] {
        El::find_by_accessible_label(&ui, label).find(|e| e.accessible_role() == Some(AccessibleRole::Button)).unwrap().invoke_accessible_default_action();
    }
    assert_eq!(*log.borrow(), ["admin true", "hotkey true", "start true", "updates false", "now", "taskmgr"]);
}

#[test]
fn the_version_line_says_development_build_when_not_installed() {
    let ui = settings_window(1000, 2600);
    let v = negotium::install::VERSION;
    negotium::settingspage::show_install(&ui, &negotium::install::Install { exe: "x".into(), installed: false });
    assert_eq!(ui.get_version_text().as_str(), format!("Version {v} (development build)"));
    assert!(text(&ui, &format!("Version {v} (development build)")).is_some(), "the page shows it");
    negotium::settingspage::show_install(&ui, &negotium::install::Install { exe: "x".into(), installed: true });
    assert_eq!(ui.get_version_text().as_str(), format!("Version {v}"));
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
    let s = Settings { refresh_ms: 5000, theme: ThemeChoice::Dark, always_on_top: true, keep_in_tray: false, gpu_rendering: false, smooth_graphs: false, tray_cpu: false, ..Settings::default() };
    negotium::settingspage::apply_settings(&ui, &s);
    assert!(!ui.get_settings_tray_cpu());
    assert!(ui.get_on_top());
    assert_eq!(ui.get_settings_refresh(), 3);
    assert_eq!(ui.get_settings_theme(), 2);
    assert!(!ui.get_settings_keep_in_tray());
    assert!(!ui.get_settings_gpu_rendering());
    assert!(!ui.get_settings_smooth_graphs() && !ui.global::<negotium::GraphClock>().get_smooth(), "shown and applied");
    assert!(ui.global::<negotium::Theme>().get_dark(), "the theme is applied, not just shown");
    let s = Settings { theme: ThemeChoice::Light, ..Settings::default() };
    negotium::settingspage::apply_settings(&ui, &s);
    assert!(!ui.get_on_top());
    assert_eq!(ui.get_settings_refresh(), 1);
    assert_eq!(ui.get_settings_theme(), 1);
    assert!(ui.get_settings_keep_in_tray());
    assert!(ui.get_settings_gpu_rendering());
    assert!(ui.get_settings_smooth_graphs() && ui.global::<negotium::GraphClock>().get_smooth());
    assert!(ui.get_settings_tray_cpu());
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
    eventually(what, || matches!(negotium::settings::load_with(&Writes::Plain, path), (s, negotium::settings::Loaded::File) if f(&s)));
}

thread_local! {
    /// What the Settings page told the picker shortcut, and whether its keys are free, per test thread.
    static SHORTCUT_TOLD: RefCell<Vec<bool>> = const { RefCell::new(Vec::new()) };
    static SHORTCUT_FREE: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

thread_local! {
    /// What the Settings page told the tray (`SettingsWiring::tray_cpu_changed`), per test thread.
    static TRAY_TOLD: RefCell<Vec<bool>> = const { RefCell::new(Vec::new()) };
}

/// The Settings page wired the way main.rs wires it, saving under `dir`; this instance draws with
/// `running` and is a development build that never runs anything elevated.
fn settings_wired_with(
    dir: &Scratch,
    s: Settings,
    fallback: bool,
    running: Renderer,
) -> (MainWindow, Arc<Controls>, Rc<RefCell<Settings>>) {
    let (ui, controls, settings, _) = settings_wired_integrated(dir, s, fallback, running, FakeIntegration::new(false).0);
    (ui, controls, settings)
}

/// [`settings_wired_with`] with `integration` behind the Ctrl+Shift+Esc switch and "Open Windows
/// Task Manager".
fn settings_wired_integrated(
    dir: &Scratch,
    s: Settings,
    fallback: bool,
    running: Renderer,
    integration: negotium::settingspage::Integration,
) -> (MainWindow, Arc<Controls>, Rc<RefCell<Settings>>, Rc<negotium::settingspage::Integrations>) {
    let ui = shown_window();
    // Tall enough for every row: the testing backend finds only what the page's view shows.
    ui.window().set_size(PhysicalSize::new(1000, 2600));
    ui.set_page(3);
    let controls = Arc::new(Controls::default());
    let settings = Rc::new(RefCell::new(s));
    let marker = dir.0.join("renderer-fallback");
    if fallback {
        negotium::renderer::record_fallback_with(&Writes::Plain, &marker).unwrap();
    }
    negotium::settingspage::apply_settings(&ui, &settings.borrow());
    let integrations = negotium::settingspage::SettingsWiring {
        settings: settings.clone(),
        saver: negotium::settings::Saver::with_writes(dir.0.join("settings.json"), Writes::Plain),
        controls: controls.clone(),
        gpu_marker: marker,
        fallback_recorded: fallback,
        running,
        tray_cpu_changed: Rc::new(|on| TRAY_TOLD.with(|t| t.borrow_mut().push(on))),
        picker_shortcut_changed: Rc::new(|on| {
            SHORTCUT_TOLD.with(|t| t.borrow_mut().push(on));
            SHORTCUT_FREE.with(|f| f.get())
        }),
        integration,
    }
    .install(&ui);
    (ui, controls, settings, integrations)
}

/// The installed exe the fake integration reports for an installed copy.
const INSTALLED_EXE: &str = r"C:\Program Files\Negotium\negotium.exe";

/// The machine behind a fake [`negotium::settingspage::Integration`]: the redirect it reads, what
/// it was asked to run (elevated commands, plain Task Manager starts), the elevated command still
/// waiting for its outcome, and what the tray was told.
#[derive(Clone, Default)]
struct FakeIntegration {
    redirect: Rc<RefCell<Option<std::path::PathBuf>>>,
    /// The user's admin task (none set: missing), and whether it has the logon trigger.
    task: Rc<RefCell<Option<negotium::integrate::TaskState>>>,
    logon: Rc<std::cell::Cell<bool>>,
    /// Task Scheduler can't report the trigger.
    logon_unreadable: Rc<std::cell::Cell<bool>>,
    /// The HKCU Run value.
    run: Rc<RefCell<Option<String>>>,
    /// How often the task was asked for, and the reads not yet answered (`read`).
    task_reads: Rc<std::cell::Cell<usize>>,
    reads: Rc<RefCell<Vec<negotium::settingspage::TaskRead>>>,
    /// The late-exit handler of each elevated command.
    late: Rc<RefCell<Vec<negotium::settingspage::LateExit>>>,
    /// How often the elevated copy was asked to replace this one.
    restarts: Rc<std::cell::Cell<usize>>,
    asked: Rc<RefCell<Vec<String>>>,
    /// The exe each elevated command ran as.
    exes: Rc<RefCell<Vec<std::path::PathBuf>>>,
    pending: Rc<RefCell<Option<negotium::settingspage::ElevatedDone>>>,
    tray: Rc<RefCell<Vec<bool>>>,
}

impl FakeIntegration {
    /// An administrator's copy: installed, or a development build.
    fn new(installed: bool) -> (negotium::settingspage::Integration, FakeIntegration) {
        FakeIntegration::for_account(installed, true)
    }

    fn for_account(installed: bool, admin_member: bool) -> (negotium::settingspage::Integration, FakeIntegration) {
        FakeIntegration::with(installed, admin_member, false)
    }

    fn with(installed: bool, admin_member: bool, elevated: bool) -> (negotium::settingspage::Integration, FakeIntegration) {
        let fake = FakeIntegration::default();
        let f = fake.clone();
        let exe = if installed { INSTALLED_EXE } else { DEV_EXE };
        let integration = negotium::settingspage::Integration {
            install: negotium::install::Install { exe: exe.into(), installed },
            installed_exe: Some(INSTALLED_EXE.into()),
            redirect: Box::new({
                let r = f.redirect.clone();
                move || r.borrow().clone()
            }),
            run_value: Box::new({
                let r = f.run.clone();
                move || r.borrow().clone()
            }),
            admin_member,
            // Like the app's: the answer comes later (`read`), never during the call.
            read_task: Box::new({
                let (count, reads) = (f.task_reads.clone(), f.reads.clone());
                move |done| {
                    count.set(count.get() + 1);
                    reads.borrow_mut().push(done);
                }
            }),
            // Run here (no UAC): recorded as `here ...`, its outcome given by `finish` like an
            // elevated command's.
            run_here: Box::new({
                let (asked, pending) = (f.asked.clone(), f.pending.clone());
                move |action, done| {
                    asked.borrow_mut().push(format!("here {}", action.to_args().join(" ")));
                    assert!(pending.borrow_mut().replace(done).is_none(), "one command at a time");
                }
            }),
            elevated,
            restart_elevated: Box::new({
                let restarts = f.restarts.clone();
                move || restarts.set(restarts.get() + 1)
            }),
            elevate: Box::new({
                let (asked, exes, pending, late) = (f.asked.clone(), f.exes.clone(), f.pending.clone(), f.late.clone());
                move |exe, args, _owner, done, exited_late| {
                    asked.borrow_mut().push(args.join(" "));
                    exes.borrow_mut().push(exe);
                    late.borrow_mut().push(exited_late);
                    assert!(pending.borrow_mut().replace(done).is_none(), "one elevated command at a time");
                }
            }),
            start_task_manager: Box::new({
                let asked = f.asked.clone();
                move || asked.borrow_mut().push("start Task Manager".into())
            }),
            redirect_shown: Box::new({
                let tray = f.tray.clone();
                move |on| tray.borrow_mut().push(on)
            }),
        };
        (integration, fake)
    }

    /// The elevated command returns `result`.
    fn finish(&self, result: Result<(), negotium_actions::ActionError>) {
        let done = self.pending.borrow_mut().take().expect("an elevated command is running");
        done(result);
    }

    fn set_redirect(&self, exe: Option<&str>) {
        *self.redirect.borrow_mut() = exe.map(Into::into);
    }

    fn set_task(&self, state: negotium::integrate::TaskState) {
        *self.task.borrow_mut() = Some(state);
    }

    /// Task Scheduler answers every read asked for (and any asked meanwhile).
    fn read(&self) {
        loop {
            let reads: Vec<_> = self.reads.borrow_mut().drain(..).collect();
            if reads.is_empty() {
                return;
            }
            for done in reads {
                let state = self.task.borrow().clone().unwrap_or(negotium::integrate::TaskState::Missing);
                let logon = (!self.logon_unreadable.get()).then(|| self.logon.get());
                done(negotium::settingspage::AdminTask { state, logon });
            }
        }
    }
}

const STARTUP: &str = "Start with Windows";

/// The Run value the installed copy writes.
fn our_run_value() -> String {
    format!("\"{INSTALLED_EXE}\" --tray")
}

/// Without admin mode Start with Windows is the Run value: shown from the machine, changed in
/// this process (HKCU, no UAC prompt), and the saved choice follows the machine.
#[test]
fn start_with_windows_is_the_run_value_without_admin_mode() {
    let dir = Scratch::new("startup-run");
    let path = dir.0.join("settings.json");
    let (integration, fake) = FakeIntegration::new(true);
    let (ui, _, settings, integrations) = settings_wired_integrated(&dir, Settings::default(), false, Renderer::Software, integration);
    ui.window().set_size(PhysicalSize::new(1000, 2600)); // every row on screen
    fake.read();
    let checked = || switch(&ui, STARTUP).accessible_checked();
    assert_eq!(checked(), Some(false));

    switch(&ui, STARTUP).invoke_accessible_default_action();
    assert_eq!(*fake.asked.borrow(), ["here --integrate startup-on"], "run here, not elevated");
    assert!(fake.exes.borrow().is_empty());
    assert_eq!(checked(), Some(false), "not on until the machine says so");
    *fake.run.borrow_mut() = Some(our_run_value());
    fake.finish(Ok(()));
    assert_eq!(checked(), Some(true));
    assert!(settings.borrow().start_with_windows);
    saved(&path, "start with windows", |s| s.start_with_windows);

    // A failure says so, and the switch stays with the machine.
    switch(&ui, STARTUP).invoke_accessible_default_action();
    assert_eq!(fake.asked.borrow().last().map(String::as_str), Some("here --integrate startup-off"));
    fake.finish(Err(negotium_actions::ActionError::Other("access denied".into())));
    assert_eq!(checked(), Some(true));
    assert_eq!(ui.get_toast(), "Couldn't change Start with Windows: access denied.");

    // Removed outside Negotium: the switch and the saved choice follow when Settings is shown.
    *fake.run.borrow_mut() = None;
    integrations.page_shown(Page::Settings);
    fake.read();
    assert_eq!(checked(), Some(false));
    assert!(!settings.borrow().start_with_windows);
    // Another program's value under our name doesn't count.
    *fake.run.borrow_mut() = Some(r#""C:\Tools\other.exe" --tray"#.into());
    integrations.show();
    fake.read();
    assert_eq!(checked(), Some(false));
}

/// With admin mode on, Start with Windows is the task's logon trigger: shown from the task (read
/// off the UI thread) and changed elevated. Admin mode turned off or on takes it along.
#[test]
fn start_with_windows_is_the_tasks_trigger_in_admin_mode() {
    use negotium::integrate::TaskState;
    let dir = Scratch::new("startup-task");
    let (integration, fake) = FakeIntegration::new(true);
    fake.set_task(TaskState::Ready);
    // A stray Run value doesn't count in admin mode.
    *fake.run.borrow_mut() = Some(our_run_value());
    let (ui, _, settings, integrations) = settings_wired_integrated(&dir, Settings::default(), false, Renderer::Software, integration);
    ui.window().set_size(PhysicalSize::new(1000, 2600)); // every row on screen
    let checked = || switch(&ui, STARTUP).accessible_checked();
    assert_eq!(checked(), Some(false), "the saved choice until the task is read");
    fake.read();
    assert_eq!(checked(), Some(false), "no logon trigger");

    switch(&ui, STARTUP).invoke_accessible_default_action();
    assert_eq!(*fake.asked.borrow(), ["--integrate startup-on"]);
    assert_eq!(*fake.exes.borrow(), [std::path::PathBuf::from(INSTALLED_EXE)], "elevated");
    fake.logon.set(true);
    *fake.run.borrow_mut() = None;
    fake.finish(Ok(()));
    assert_eq!(checked(), Some(false), "until Task Scheduler answers");
    fake.read();
    assert_eq!(checked(), Some(true));
    assert!(settings.borrow().start_with_windows);

    // Admin mode off: the command moves the startup to the Run value.
    switch(&ui, ADMIN).invoke_accessible_default_action();
    assert_eq!(fake.asked.borrow().last().map(String::as_str), Some("--integrate admin-off"));
    fake.set_task(TaskState::Missing);
    fake.logon.set(false);
    *fake.run.borrow_mut() = Some(our_run_value());
    fake.finish(Ok(()));
    // This user's own Run value is set here too (the prompt may have been another account's).
    assert_eq!(fake.asked.borrow().last().map(String::as_str), Some("here --integrate startup-on"));
    fake.finish(Ok(()));
    fake.read();
    assert_eq!(checked(), Some(true), "now the Run value");
    // And on again: the task takes it back (--logon).
    switch(&ui, ADMIN).invoke_accessible_default_action();
    assert_eq!(fake.asked.borrow().last().map(String::as_str), Some("--integrate admin-on --logon"));
    fake.set_task(TaskState::Ready);
    fake.logon.set(true);
    *fake.run.borrow_mut() = None;
    fake.finish(Ok(()));
    fake.read();
    assert_eq!(checked(), Some(true), "the trigger again");
    // Off in admin mode (even one that needs repair): elevated.
    fake.set_task(TaskState::Broken("its exe is gone".into()));
    integrations.show();
    fake.read();
    switch(&ui, STARTUP).invoke_accessible_default_action();
    assert_eq!(fake.asked.borrow().last().map(String::as_str), Some("--integrate startup-off"));
    assert_eq!(fake.exes.borrow().len(), 4, "every one elevated");
}

/// Task Scheduler couldn't report the trigger (or the task at all): the saved choice stands, is
/// not overwritten, and a repair keeps the startup (--logon).
#[test]
fn an_unreadable_trigger_keeps_the_saved_start_with_windows() {
    use negotium::integrate::TaskState;
    let dir = Scratch::new("startup-unknown");
    let path = dir.0.join("settings.json");
    let (integration, fake) = FakeIntegration::new(true);
    fake.logon_unreadable.set(true);
    fake.set_task(TaskState::Broken("it couldn't be read: the service is not running".into()));
    let s = Settings { start_with_windows: true, ..Settings::default() };
    let (ui, _, settings, integrations) = settings_wired_integrated(&dir, s, false, Renderer::Software, integration);
    ui.window().set_size(PhysicalSize::new(1000, 2600)); // every row on screen
    fake.read();
    integrations.show();
    fake.read();
    assert_eq!(switch(&ui, STARTUP).accessible_checked(), Some(true), "the saved choice");
    assert!(settings.borrow().start_with_windows);
    assert!(!path.exists(), "nothing was saved");
    // Repairing admin mode keeps the startup.
    switch(&ui, ADMIN).invoke_accessible_default_action();
    assert_eq!(*fake.asked.borrow(), ["--integrate admin-on --logon"]);
    // Ready, trigger still unreadable: the same.
    fake.set_task(TaskState::Ready);
    fake.finish(Ok(()));
    fake.read();
    assert_eq!(switch(&ui, STARTUP).accessible_checked(), Some(true));
    assert!(settings.borrow().start_with_windows);
    // Readable again: the machine wins.
    fake.logon_unreadable.set(false);
    integrations.show();
    fake.read();
    assert_eq!(switch(&ui, STARTUP).accessible_checked(), Some(false));
    assert!(!settings.borrow().start_with_windows);
}

/// Before an administrator's task has been read, the form (Run value or trigger) isn't known:
/// the switch does nothing and snaps back.
#[test]
fn start_with_windows_waits_for_the_first_task_read() {
    let dir = Scratch::new("startup-early");
    let (integration, fake) = FakeIntegration::new(true);
    let (ui, _, _, _) = settings_wired_integrated(&dir, Settings::default(), false, Renderer::Software, integration);
    ui.window().set_size(PhysicalSize::new(1000, 2600)); // every row on screen
    switch(&ui, STARTUP).invoke_accessible_default_action();
    assert!(fake.asked.borrow().is_empty(), "nothing runs");
    assert_eq!(switch(&ui, STARTUP).accessible_checked(), Some(false), "snapped back");
    fake.read();
    switch(&ui, STARTUP).invoke_accessible_default_action();
    assert_eq!(*fake.asked.borrow(), ["here --integrate startup-on"]);
}

#[test]
fn a_dev_build_never_reads_or_changes_start_with_windows() {
    let dir = Scratch::new("startup-dev");
    let (integration, fake) = FakeIntegration::new(false);
    *fake.run.borrow_mut() = Some(our_run_value());
    let (ui, _, settings, integrations) = settings_wired_integrated(&dir, Settings::default(), false, Renderer::Software, integration);
    ui.window().set_size(PhysicalSize::new(1000, 2600)); // every row on screen
    assert_eq!(switch(&ui, STARTUP).accessible_checked(), Some(false));
    integrations.toggle_start_with_windows(true);
    assert!(fake.asked.borrow().is_empty());
    assert!(!settings.borrow().start_with_windows, "a dev build doesn't take the installed copy's state");
}

/// A development build's exe.
const DEV_EXE: &str = r"C:\Users\me\DEV\Negotium\target\release\negotium.exe";

const HOTKEY: &str = "Open on Ctrl+Shift+Esc";

fn task_manager_button(ui: &MainWindow) -> El {
    El::find_by_accessible_label(ui, "Open Windows Task Manager").find(|e| e.accessible_role() == Some(AccessibleRole::Button)).unwrap()
}

#[test]
fn the_hotkey_switch_shows_the_machine_state() {
    let dir = Scratch::new("hotkey");
    let (integration, fake) = FakeIntegration::new(true);
    let (ui, _, _, integrations) = settings_wired_integrated(&dir, Settings::default(), false, Renderer::Software, integration);
    ui.window().set_size(PhysicalSize::new(1000, 2600)); // every row on screen
    let checked = || switch(&ui, HOTKEY).accessible_checked();
    assert_eq!(checked(), Some(false), "no redirect");
    for (redirect, on) in [
        (Some(INSTALLED_EXE.to_string()), true),
        (Some(INSTALLED_EXE.to_uppercase()), true),
        (Some(r"C:\Tools\procexp64.exe".to_string()), false),
        (Some(r"C:\Users\me\DEV\Negotium\target\release\negotium.exe".to_string()), false),
        (None, false),
    ] {
        fake.set_redirect(redirect.as_deref());
        integrations.show();
        assert_eq!(checked(), Some(on), "{redirect:?}");
        assert_eq!(fake.tray.borrow().last(), Some(&on), "the tray offers Task Manager only with our redirect");
    }

    // On: the elevated hotkey-on; the switch shows what the machine then says.
    switch(&ui, HOTKEY).invoke_accessible_default_action();
    assert_eq!(*fake.asked.borrow(), ["--integrate hotkey-on"]);
    assert_eq!(checked(), Some(false), "not on until the command has set it");
    switch(&ui, HOTKEY).invoke_accessible_default_action();
    task_manager_button(&ui).invoke_accessible_default_action();
    assert_eq!(fake.asked.borrow().len(), 1, "nothing else runs while the UAC prompt is open");
    fake.set_redirect(Some(INSTALLED_EXE));
    fake.finish(Ok(()));
    assert_eq!(checked(), Some(true));

    // A declined UAC prompt changes nothing: the switch snaps back, quietly.
    switch(&ui, HOTKEY).invoke_accessible_default_action();
    assert_eq!(fake.asked.borrow().last().map(String::as_str), Some("--integrate hotkey-off"));
    fake.finish(Err(negotium_actions::ActionError::Cancelled));
    assert_eq!(checked(), Some(true));
    assert_eq!(ui.get_toast(), "");

    // A failed command says so.
    switch(&ui, HOTKEY).invoke_accessible_default_action();
    fake.finish(Err(negotium_actions::ActionError::Other("the elevated command failed (exit code 1)".into())));
    assert_eq!(checked(), Some(true));
    assert_eq!(ui.get_toast(), "Couldn't change Open on Ctrl+Shift+Esc: the elevated command failed (exit code 1).");
    ui.set_toast("".into());

    // Off, approved.
    switch(&ui, HOTKEY).invoke_accessible_default_action();
    fake.set_redirect(None);
    fake.finish(Ok(()));
    assert_eq!(checked(), Some(false));
}

#[test]
fn open_windows_task_manager_lifts_only_our_redirect() {
    let dir = Scratch::new("taskmgr");
    let (integration, fake) = FakeIntegration::new(true);
    let (ui, _, _, integrations) = settings_wired_integrated(&dir, Settings::default(), false, Renderer::Software, integration);
    ui.window().set_size(PhysicalSize::new(1000, 2600)); // every row on screen
    // No redirect, or another tool's: Task Manager just starts.
    for redirect in [None, Some(r"C:\Tools\procexp64.exe")] {
        fake.set_redirect(redirect);
        task_manager_button(&ui).invoke_accessible_default_action();
    }
    assert_eq!(*fake.asked.borrow(), ["start Task Manager", "start Task Manager"]);
    // Ours: the elevated open-task-manager (the tray asks the same way).
    fake.set_redirect(Some(INSTALLED_EXE));
    integrations.open_task_manager();
    assert_eq!(fake.asked.borrow().last().map(String::as_str), Some("--integrate open-task-manager"));
    fake.finish(Err(negotium_actions::ActionError::Other("Taskmgr.exe could not be started".into())));
    assert_eq!(ui.get_toast(), "Couldn't open Windows Task Manager: Taskmgr.exe could not be started.");
    assert_eq!(switch(&ui, HOTKEY).accessible_checked(), Some(true), "the redirect is still ours");
}

#[test]
fn a_dev_build_never_runs_the_hotkey_command() {
    let dir = Scratch::new("hotkey-dev");
    let (integration, fake) = FakeIntegration::new(false);
    // Even with the installed copy's redirect on, a dev build's switch is off and asks nothing.
    fake.set_redirect(Some(INSTALLED_EXE));
    let (ui, _, _, integrations) = settings_wired_integrated(&dir, Settings::default(), false, Renderer::Software, integration);
    ui.window().set_size(PhysicalSize::new(1000, 2600)); // every row on screen
    assert_eq!(switch(&ui, HOTKEY).accessible_checked(), Some(false));
    integrations.toggle_hotkey(true);
    assert!(!ui.get_settings_hotkey());
    assert!(fake.asked.borrow().is_empty());
    assert_eq!(fake.tray.borrow().last(), Some(&false));
}

/// A development build can't change the installed copy's redirect itself (`--integrate` refuses
/// any exe but the installed one), so "Open Windows Task Manager" asks the installed copy.
#[test]
fn a_dev_build_asks_the_installed_copy_to_lift_its_redirect() {
    let dir = Scratch::new("taskmgr-dev");
    let (integration, fake) = FakeIntegration::new(false);
    let (ui, _, _, _) = settings_wired_integrated(&dir, Settings::default(), false, Renderer::Software, integration);
    ui.window().set_size(PhysicalSize::new(1000, 2600)); // every row on screen
    // No redirect, another tool's, or one naming this build: Task Manager just starts.
    for redirect in [None, Some(r"C:\Tools\procexp64.exe"), Some(DEV_EXE)] {
        fake.set_redirect(redirect);
        task_manager_button(&ui).invoke_accessible_default_action();
    }
    assert_eq!(*fake.asked.borrow(), ["start Task Manager"; 3]);
    assert!(fake.exes.borrow().is_empty());
    // The installed copy's (however it is spelled): it runs open-task-manager, elevated.
    fake.set_redirect(Some(&INSTALLED_EXE.to_uppercase()));
    task_manager_button(&ui).invoke_accessible_default_action();
    assert_eq!(fake.asked.borrow().last().map(String::as_str), Some("--integrate open-task-manager"));
    assert_eq!(*fake.exes.borrow(), [std::path::PathBuf::from(INSTALLED_EXE)]);
    fake.finish(Ok(()));
    assert_eq!(ui.get_toast(), "");
    assert_eq!(switch(&ui, HOTKEY).accessible_checked(), Some(false), "a dev build's switch stays off");
}

const ADMIN: &str = "Admin mode";

#[test]
fn the_admin_mode_switch_shows_the_task_and_runs_the_elevated_command() {
    use negotium::integrate::TaskState;
    let dir = Scratch::new("admin");
    let (integration, fake) = FakeIntegration::new(true);
    let (ui, _, settings, integrations) = settings_wired_integrated(&dir, Settings::default(), false, Renderer::Software, integration);
    ui.window().set_size(PhysicalSize::new(1000, 2600)); // every row on screen
    fake.read();
    let checked = || switch(&ui, ADMIN).accessible_checked();
    assert_eq!(checked(), Some(false), "no task");
    assert_eq!(switch(&ui, ADMIN).accessible_enabled(), Some(true));
    fake.set_task(TaskState::Ready);
    integrations.show();
    fake.read();
    assert_eq!(checked(), Some(true));
    assert_eq!(ui.get_settings_admin_mode_state(), 1);

    // Off: the elevated admin-off; the switch follows the task, not the click.
    switch(&ui, ADMIN).invoke_accessible_default_action();
    assert_eq!(*fake.asked.borrow(), ["--integrate admin-off"]);
    assert_eq!(*fake.exes.borrow(), [std::path::PathBuf::from(INSTALLED_EXE)]);
    switch(&ui, HOTKEY).invoke_accessible_default_action();
    assert_eq!(fake.asked.borrow().len(), 1, "one elevated command at a time");
    fake.set_task(TaskState::Missing);
    fake.finish(Ok(()));
    fake.read();
    assert_eq!(checked(), Some(false));

    // On; a declined prompt snaps it back, quietly.
    switch(&ui, ADMIN).invoke_accessible_default_action();
    assert_eq!(fake.asked.borrow().last().map(String::as_str), Some("--integrate admin-on"));
    fake.finish(Err(negotium_actions::ActionError::Cancelled));
    fake.read();
    assert_eq!(checked(), Some(false));
    assert_eq!(ui.get_toast(), "");

    // With Start with Windows on, the task also starts at sign-in; a failure says so.
    settings.borrow_mut().start_with_windows = true;
    switch(&ui, ADMIN).invoke_accessible_default_action();
    assert_eq!(fake.asked.borrow().last().map(String::as_str), Some("--integrate admin-on --logon"));
    fake.finish(Err(negotium_actions::ActionError::Other("the elevated command failed (exit code 1)".into())));
    fake.read();
    assert_eq!(checked(), Some(false));
    assert_eq!(ui.get_toast(), "Couldn't change Admin mode: the elevated command failed (exit code 1).");
    assert_eq!(fake.restarts.get(), 0, "nothing turned on: this copy stays");
}

/// Task Scheduler is never asked on the UI thread: the row shows the last known state until the
/// (background) answer comes, and a read asked for meanwhile is made once the first returns.
#[test]
fn the_admin_task_is_read_off_the_ui_thread() {
    use negotium::integrate::TaskState;
    let dir = Scratch::new("admin-async");
    let (integration, fake) = FakeIntegration::new(true);
    fake.set_task(TaskState::Ready);
    let (ui, _, _, integrations) = settings_wired_integrated(&dir, Settings::default(), false, Renderer::Software, integration);
    assert_eq!(fake.task_reads.get(), 1, "asked for when wired");
    assert_eq!(ui.get_settings_admin_mode_state(), 0, "not known yet");
    integrations.show();
    integrations.page_shown(Page::Settings);
    assert_eq!(fake.task_reads.get(), 1, "one read at a time");
    fake.read();
    assert_eq!(ui.get_settings_admin_mode_state(), 1);
    assert_eq!(fake.task_reads.get(), 2, "and one more for the shows meanwhile");
    // Shown again: the last known state stays until the new answer.
    fake.set_task(TaskState::Disabled);
    integrations.show();
    assert_eq!(ui.get_settings_admin_mode_state(), 1);
    fake.read();
    assert_eq!(ui.get_settings_admin_mode_state(), 2);

    // The app's reader runs the read on another thread.
    let (tx, rx) = std::sync::mpsc::channel();
    negotium::settingspage::in_background(move || tx.send(std::thread::current().id()).unwrap(), |_| {});
    let worker = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    assert_ne!(worker, std::thread::current().id());
}

/// Turned on in a running non-elevated copy, admin mode hands over to the elevated copy at once
/// (otherwise every later launch would just show this window); an elevated copy stays.
#[test]
fn turning_admin_mode_on_hands_over_to_the_elevated_copy() {
    hands_over_when_turned_on(false);
}

#[test]
fn an_elevated_copy_keeps_running_when_admin_mode_turns_on() {
    hands_over_when_turned_on(true);
}

fn hands_over_when_turned_on(elevated: bool) {
    use negotium::integrate::TaskState;
    let dir = Scratch::new("admin-restart");
    let (integration, fake) = FakeIntegration::with(true, true, elevated);
    let (_ui, _, _, integrations) = settings_wired_integrated(&dir, Settings::default(), false, Renderer::Software, integration);
    fake.read();
    integrations.toggle_admin_mode(true);
    fake.set_task(TaskState::Ready);
    fake.finish(Ok(()));
    assert_eq!(fake.restarts.get(), usize::from(!elevated), "elevated: {elevated}");
    // Off never restarts.
    integrations.toggle_admin_mode(false);
    fake.finish(Ok(()));
    assert_eq!(fake.restarts.get(), usize::from(!elevated), "elevated: {elevated}");
}

/// A command that outlasts the wait reports a failure, and the switches are read again when it
/// finally exits.
#[test]
fn a_command_that_outlasts_the_wait_refreshes_the_switches_when_it_exits() {
    let dir = Scratch::new("hotkey-late");
    let (integration, fake) = FakeIntegration::new(true);
    let (ui, _, _, integrations) = settings_wired_integrated(&dir, Settings::default(), false, Renderer::Software, integration);
    ui.window().set_size(PhysicalSize::new(1000, 2600)); // every row on screen
    integrations.toggle_hotkey(true);
    fake.finish(Err(negotium_actions::ActionError::Other("the elevated command didn't finish within 30 seconds".into())));
    assert_eq!(switch(&ui, HOTKEY).accessible_checked(), Some(false));
    fake.set_redirect(Some(INSTALLED_EXE));
    let late = fake.late.borrow_mut().pop().expect("a late-exit handler");
    late();
    assert_eq!(switch(&ui, HOTKEY).accessible_checked(), Some(true));
}

#[test]
fn admin_mode_needs_repair_when_the_task_is_broken() {
    use negotium::integrate::TaskState;
    let dir = Scratch::new("admin-repair");
    let (integration, fake) = FakeIntegration::new(true);
    fake.set_task(TaskState::Broken("its exe is gone".into()));
    let (ui, _, _, integrations) = settings_wired_integrated(&dir, Settings::default(), false, Renderer::Software, integration);
    ui.window().set_size(PhysicalSize::new(1000, 2600)); // every row on screen
    for state in [TaskState::Broken("its exe is gone".into()), TaskState::Disabled] {
        fake.set_task(state.clone());
        integrations.show();
        fake.read();
        assert_eq!(ui.get_settings_admin_mode_state(), 2, "{state:?}");
        let s = switch(&ui, ADMIN);
        assert_eq!(s.accessible_checked(), Some(false), "{state:?}");
        assert_eq!(s.accessible_enabled(), Some(true), "{state:?}: it can be repaired");
        assert_eq!(s.accessible_description().as_deref(), Some("Turn on again to repair"), "{state:?}");
        assert!(text(&ui, "Needs repair. Turn on again to repair").is_some(), "{state:?}: the row says so");
    }
    // Turning it on again recreates the task.
    switch(&ui, ADMIN).invoke_accessible_default_action();
    assert_eq!(*fake.asked.borrow(), ["--integrate admin-on"]);
    fake.set_task(TaskState::Ready);
    fake.finish(Ok(()));
    fake.read();
    assert_eq!(switch(&ui, ADMIN).accessible_checked(), Some(true));
    assert_eq!(switch(&ui, ADMIN).accessible_description().as_deref(), Some(ADMIN_DESCRIPTION));
}

const ADMIN_DESCRIPTION: &str =
    "Start with admin rights without a prompt. Anything that can start Negotium can then open it with admin rights, without a prompt.";

#[test]
fn a_standard_account_cannot_turn_on_admin_mode() {
    use negotium::integrate::TaskState;
    let dir = Scratch::new("admin-standard");
    let (integration, fake) = FakeIntegration::for_account(true, false);
    fake.set_task(TaskState::Ready);
    let (ui, _, _, integrations) = settings_wired_integrated(&dir, Settings::default(), false, Renderer::Software, integration);
    ui.window().set_size(PhysicalSize::new(1000, 2600)); // every row on screen
    assert_eq!(ui.get_settings_admin_mode_state(), 3);
    let s = switch(&ui, ADMIN);
    assert_eq!(s.accessible_enabled(), Some(false));
    assert_eq!(s.accessible_checked(), Some(false));
    assert_eq!(s.accessible_description().as_deref(), Some("Needs an administrator account"));
    assert!(s.computed_opacity() < 1.0, "dimmed");
    assert!(text(&ui, "Needs an administrator account").is_some());
    integrations.toggle_admin_mode(true);
    assert!(fake.asked.borrow().is_empty(), "nothing runs");
    assert_eq!(fake.task_reads.get(), 0, "Task Scheduler isn't even asked");
    // The hotkey stays available to a standard account's installed copy.
    assert_eq!(switch(&ui, HOTKEY).accessible_enabled(), Some(true));
}

#[test]
fn a_dev_build_never_reads_or_runs_admin_mode() {
    let dir = Scratch::new("admin-dev");
    let (integration, fake) = FakeIntegration::new(false);
    fake.set_task(negotium::integrate::TaskState::Ready);
    let (ui, _, _, integrations) = settings_wired_integrated(&dir, Settings::default(), false, Renderer::Software, integration);
    ui.window().set_size(PhysicalSize::new(1000, 2600)); // every row on screen
    assert_eq!(switch(&ui, ADMIN).accessible_checked(), Some(false));
    assert_eq!(switch(&ui, ADMIN).accessible_description().as_deref(), Some("Install Negotium to use this"));
    integrations.toggle_admin_mode(true);
    assert!(fake.asked.borrow().is_empty());
    assert_eq!(fake.task_reads.get(), 0);
}

#[test]
fn the_switches_read_the_machine_when_the_settings_page_is_shown() {
    use negotium::integrate::TaskState;
    let dir = Scratch::new("integration-refresh");
    let (integration, fake) = FakeIntegration::new(true);
    let (ui, _, _, integrations) = settings_wired_integrated(&dir, Settings::default(), false, Renderer::Software, integration);
    ui.window().set_size(PhysicalSize::new(1000, 2600)); // every row on screen
    fake.read();
    assert_eq!(switch(&ui, HOTKEY).accessible_checked(), Some(false));
    assert_eq!(switch(&ui, ADMIN).accessible_checked(), Some(false));
    // Changed outside Negotium (the installer, another copy).
    fake.set_redirect(Some(INSTALLED_EXE));
    fake.set_task(TaskState::Ready);
    integrations.page_shown(Page::Overview);
    fake.read();
    assert_eq!(switch(&ui, HOTKEY).accessible_checked(), Some(false), "only the Settings page reads it");
    assert_eq!(switch(&ui, ADMIN).accessible_checked(), Some(false));
    integrations.page_shown(Page::Settings);
    fake.read();
    assert_eq!(switch(&ui, HOTKEY).accessible_checked(), Some(true));
    assert_eq!(switch(&ui, ADMIN).accessible_checked(), Some(true));
    assert_eq!(fake.tray.borrow().last(), Some(&true));
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
        *s == Settings { refresh_ms: 500, theme: ThemeChoice::Dark, always_on_top: true, keep_in_tray: false, ..Settings::default() }
    });
}

#[test]
fn the_backdrop_choice_shows_and_is_saved() {
    use negotium::settings::BackdropChoice;
    let dir = Scratch::new("backdrop");
    let path = dir.0.join("settings.json");
    let (ui, _controls, _settings) = settings_wired(&dir, Settings::default(), false);
    assert_eq!(radio(&ui, "Mica").accessible_checked(), Some(true), "Mica by default");

    radio(&ui, "Acrylic").invoke_accessible_default_action();
    assert_eq!(radio(&ui, "Acrylic").accessible_checked(), Some(true));
    saved(&path, "backdrop acrylic", |s| s.backdrop == BackdropChoice::Acrylic);

    negotium::settingspage::apply_settings(&ui, &Settings::default());
    assert_eq!(ui.get_settings_backdrop(), 0, "apply_settings shows the saved backdrop");
    negotium::settingspage::apply_settings(&ui, &Settings { backdrop: BackdropChoice::Acrylic, ..Settings::default() });
    assert_eq!(ui.get_settings_backdrop(), 1);
}

#[test]
fn acrylic_is_tinted_and_mica_is_not() {
    use negotium::settings::BackdropChoice;
    let ui = window();
    let theme = ui.global::<negotium::Theme>();
    negotium::platform::set_backdrop(&ui, BackdropChoice::Acrylic);
    assert!(theme.get_acrylic());
    theme.set_mica(true); // what apply_window_chrome sets once the backdrop is on
    ui.invoke_set_dark(false);
    let tint = theme.get_window_bg().color();
    assert!(tint.alpha() > 0 && tint.alpha() < 255, "part-opaque over Acrylic: {tint:?}");
    // Grey text on the backdrop can only be as dark as this layer lets it: below ~75% the labels
    // break up over a busy background (seen in light mode at 45%).
    assert!(tint.alpha() >= 191, "light mode keeps grey text readable: {tint:?}");
    ui.invoke_set_dark(true);
    let tint = theme.get_window_bg().color();
    assert_eq!((tint.red(), tint.green(), tint.blue()), (0, 0, 0), "dark mode darkens with black: {tint:?}");
    assert!(tint.alpha() > 0 && tint.alpha() < 255, "and stays part-opaque: {tint:?}");
    ui.invoke_set_dark(false);
    negotium::platform::set_backdrop(&ui, BackdropChoice::Mica);
    assert!(!theme.get_acrylic());
    assert_eq!(theme.get_window_bg().color().alpha(), 0, "Mica shows through untouched");
    theme.set_mica(false);
    assert_eq!(theme.get_window_bg().color().alpha(), 255, "no backdrop: solid");
}

// ---- Acrylic tint colour and opacity ----

/// The light and dark Acrylic tints before Tint colour and Tint opacity existed (window-bg on
/// 682ec58, Acrylic with the backdrop on, captured from a run).
const LIGHT_TINT: (u8, u8, u8, u8) = (243, 243, 243, 204);
const DARK_TINT: (u8, u8, u8, u8) = (0, 0, 0, 115);
const BLUE: [u8; 3] = [0, 120, 212];

fn rgba(c: slint::Color) -> (u8, u8, u8, u8) {
    (c.red(), c.green(), c.blue(), c.alpha())
}

/// What `Color.with-alpha(opacity)` gives a colour's alpha.
fn alpha_of(opacity: f32) -> u8 {
    slint::Color::from_rgb_u8(0, 0, 0).with_alpha(opacity).alpha()
}

/// Acrylic with `tint` and `opacity` as saved.
fn tinted(tint: Option<[u8; 3]>, opacity: Option<u8>) -> Settings {
    use negotium::settings::BackdropChoice;
    Settings { backdrop: BackdropChoice::Acrylic, acrylic_tint: tint, acrylic_opacity: opacity, ..Settings::default() }
}

/// `ui`'s window-bg once `s` is applied as at startup, the backdrop is on (`mica`, as
/// apply_window_chrome sets it with the GPU renderer) and the mode is `dark`.
fn window_bg_with(ui: &MainWindow, s: &Settings, mica: bool, dark: bool) -> (u8, u8, u8, u8) {
    negotium::settingspage::apply_settings(ui, s);
    let theme = ui.global::<negotium::Theme>();
    theme.set_mica(mica);
    ui.invoke_set_dark(dark);
    rgba(theme.get_window_bg().color())
}

#[test]
fn an_unset_tint_looks_as_before_in_light_mode() {
    let ui = window();
    assert_eq!(window_bg_with(&ui, &tinted(None, None), true, false), LIGHT_TINT);
}

#[test]
fn an_unset_tint_looks_as_before_in_dark_mode() {
    let ui = window();
    assert_eq!(window_bg_with(&ui, &tinted(None, None), true, true), DARK_TINT);
}

#[test]
fn a_tint_colour_alone_keeps_the_light_modes_opacity() {
    let ui = window();
    assert_eq!(window_bg_with(&ui, &tinted(Some(BLUE), None), true, false), (0, 120, 212, LIGHT_TINT.3));
}

#[test]
fn a_tint_colour_alone_keeps_the_dark_modes_opacity() {
    let ui = window();
    assert_eq!(window_bg_with(&ui, &tinted(Some(BLUE), None), true, true), (0, 120, 212, DARK_TINT.3));
}

#[test]
fn a_tint_opacity_alone_keeps_the_light_modes_colour() {
    let ui = window();
    assert_eq!(window_bg_with(&ui, &tinted(None, Some(30)), true, false), (243, 243, 243, alpha_of(0.30)));
}

#[test]
fn a_tint_opacity_alone_keeps_the_dark_modes_colour() {
    let ui = window();
    assert_eq!(window_bg_with(&ui, &tinted(None, Some(30)), true, true), (0, 0, 0, alpha_of(0.30)));
}

#[test]
fn a_tint_colour_and_opacity_look_the_same_in_both_modes() {
    let ui = window();
    let s = tinted(Some(BLUE), Some(30));
    let light = window_bg_with(&ui, &s, true, false);
    assert_eq!(light, (0, 120, 212, alpha_of(0.30)));
    assert_eq!(window_bg_with(&ui, &s, true, true), light);
}

#[test]
fn a_tint_opacity_of_0_is_clear_and_100_is_opaque() {
    let ui = window();
    assert_eq!(window_bg_with(&ui, &tinted(Some(BLUE), Some(0)), true, false).3, 0);
    assert_eq!(window_bg_with(&ui, &tinted(Some(BLUE), Some(100)), true, false).3, 255);
}

#[test]
fn mica_ignores_the_tint() {
    use negotium::settings::BackdropChoice;
    let ui = window();
    let s = Settings { backdrop: BackdropChoice::Mica, ..tinted(Some(BLUE), Some(30)) };
    for dark in [false, true] {
        assert_eq!(window_bg_with(&ui, &s, true, dark).3, 0, "dark = {dark}: Mica shows through untouched");
    }
}

#[test]
fn the_window_without_a_backdrop_ignores_the_tint() {
    let ui = window();
    for dark in [false, true] {
        assert_eq!(window_bg_with(&ui, &tinted(Some(BLUE), Some(30)), false, dark).3, 255, "dark = {dark}: solid");
    }
}

/// The std Slider labelled `label` (its `accessible-label` is set on the instance; its value reads
/// as "45", "83.137").
fn slider(ui: &MainWindow, label: &str) -> El {
    El::find_by_accessible_label(ui, label)
        .find(|e| e.accessible_role() == Some(AccessibleRole::Slider))
        .unwrap_or_else(|| panic!("no slider {label:?}"))
}

/// Settings wired as main.rs wires them, with `s` in light mode and the backdrop on.
fn tint_wired(dir: &Scratch, s: Settings) -> MainWindow {
    let (ui, _controls, _settings) = settings_wired(dir, Settings { theme: ThemeChoice::Light, ..s }, false);
    ui.global::<negotium::Theme>().set_mica(true);
    ui
}

fn window_bg(ui: &MainWindow) -> (u8, u8, u8, u8) {
    rgba(ui.global::<negotium::Theme>().get_window_bg().color())
}

#[test]
fn the_tint_rows_show_only_for_acrylic() {
    let dir = Scratch::new("tintrows");
    let (ui, _controls, _settings) = settings_wired(&dir, Settings::default(), false);
    let shown = |label: &str| El::find_by_accessible_label(&ui, label).next().is_some();
    assert!(!shown("Tint colour") && !shown("Tint opacity"), "Mica: no tint rows");
    radio(&ui, "Acrylic").invoke_accessible_default_action();
    assert!(shown("Tint colour"), "Acrylic: the tint colours");
    slider(&ui, "Tint opacity");
}

#[test]
fn a_tint_swatch_applies_at_once_and_is_saved() {
    let dir = Scratch::new("tintswatch");
    let path = dir.0.join("settings.json");
    let ui = tint_wired(&dir, tinted(None, None));
    assert_eq!(radio(&ui, "Default colour").accessible_checked(), Some(true), "unset: the default is chosen");
    radio(&ui, "Blue").invoke_accessible_default_action();
    assert_eq!(window_bg(&ui), (0, 120, 212, LIGHT_TINT.3), "the tint changes at once");
    assert_eq!(radio(&ui, "Blue").accessible_checked(), Some(true));
    assert_eq!(radio(&ui, "Default colour").accessible_checked(), Some(false));
    saved(&path, "tint blue", |s| s.acrylic_tint == Some(BLUE));
}

#[test]
fn default_colour_returns_to_the_modes_tint_and_is_saved() {
    let dir = Scratch::new("tintdefault");
    let path = dir.0.join("settings.json");
    let ui = tint_wired(&dir, tinted(Some(BLUE), None));
    assert_eq!(radio(&ui, "Blue").accessible_checked(), Some(true), "the saved tint is shown");
    radio(&ui, "Default colour").invoke_accessible_default_action();
    assert_eq!(window_bg(&ui), LIGHT_TINT);
    assert_eq!(radio(&ui, "Default colour").accessible_checked(), Some(true));
    assert_eq!(radio(&ui, "Blue").accessible_checked(), Some(false));
    saved(&path, "tint unset", |s| s.acrylic_tint.is_none());
}

#[test]
fn the_opacity_slider_shows_the_modes_opacity_while_unset() {
    let dir = Scratch::new("tintslidermode");
    let ui = tint_wired(&dir, tinted(None, None));
    assert_eq!(slider(&ui, "Tint opacity").accessible_value().as_deref(), Some("80"), "light");
    ui.invoke_set_dark(true);
    assert_eq!(slider(&ui, "Tint opacity").accessible_value().as_deref(), Some("45"), "dark");
}

#[test]
fn moving_the_opacity_slider_applies_at_once_and_is_saved() {
    let dir = Scratch::new("tintslidermove");
    let path = dir.0.join("settings.json");
    let ui = tint_wired(&dir, tinted(None, None));
    slider(&ui, "Tint opacity").set_accessible_value("30");
    assert_eq!(window_bg(&ui), (243, 243, 243, alpha_of(0.30)), "the opacity changes at once");
    saved(&path, "opacity 30", |s| s.acrylic_opacity == Some(30));
}

#[test]
fn the_tint_controls_report_each_choice() {
    let ui = settings_window(1000, 2600);
    ui.set_settings_backdrop(1);
    let log = Rc::new(RefCell::new(Vec::<String>::new()));
    let l = log.clone();
    ui.on_tint_default_chosen(move || l.borrow_mut().push("default".to_string()));
    let l = log.clone();
    ui.on_tint_chosen(move |c| l.borrow_mut().push(format!("tint {:?}", (c.red(), c.green(), c.blue(), c.alpha()))));
    let l = log.clone();
    ui.on_tint_opacity_chosen(move |v| l.borrow_mut().push(format!("opacity {v}")));
    for label in ["Default colour", "Black", "White", "Blue", "Purple", "Green", "Red"] {
        radio(&ui, label).invoke_accessible_default_action();
    }
    slider(&ui, "Tint opacity").set_accessible_value("30");
    assert_eq!(
        *log.borrow(),
        [
            "default",
            "tint (0, 0, 0, 255)",
            "tint (255, 255, 255, 255)",
            "tint (0, 120, 212, 255)",
            "tint (116, 77, 169, 255)",
            "tint (16, 124, 16, 255)",
            "tint (196, 43, 28, 255)",
            "opacity 30",
        ]
    );
}

#[test]
fn a_saved_tint_and_a_clicked_tint_look_the_same() {
    let dir = Scratch::new("tintequal");
    let ui = tint_wired(&dir, tinted(Some(BLUE), Some(30)));
    let at_startup = window_bg(&ui);
    radio(&ui, "Default colour").invoke_accessible_default_action();
    slider(&ui, "Tint opacity").set_accessible_value("50");
    assert_ne!(window_bg(&ui), at_startup, "moved away first");
    radio(&ui, "Blue").invoke_accessible_default_action();
    slider(&ui, "Tint opacity").set_accessible_value("30");
    assert_eq!(window_bg(&ui), at_startup);
}

/// The tint picker, opened from "Custom colour" over Settings wired with `tint`.
fn tint_picker(dir: &Scratch, tint: [u8; 3]) -> (MainWindow, std::path::PathBuf) {
    let ui = tint_wired(dir, tinted(Some(tint), None));
    radio(&ui, "Custom colour").invoke_accessible_default_action();
    assert!(ui.get_tint_picker_visible(), "the picker opens");
    (ui, dir.0.join("settings.json"))
}

fn rgb_of(c: slint::Color) -> [u8; 3] {
    [c.red(), c.green(), c.blue()]
}

#[test]
fn custom_colour_opens_the_picker_over_the_page() {
    let dir = Scratch::new("tintpickeropen");
    let ui = tint_wired(&dir, tinted(Some(BLUE), None));
    radio(&ui, "Custom colour").invoke_accessible_default_action();
    assert!(ui.get_tint_picker_visible());
    assert!(ui.get_modal_open(), "the picker is a modal");
    assert_eq!(radio(&ui, "Blue").accessible_enabled(), Some(false), "the page beneath is blocked");
}

#[test]
fn the_picker_starts_from_the_tint_in_effect() {
    let dir = Scratch::new("tintpickerseed");
    let (ui, _path) = tint_picker(&dir, BLUE);
    // #0078d4: value 212 / 255, shown in whole percent.
    assert_eq!(slider(&ui, "Brightness").accessible_value().as_deref(), Some("83"));
}

#[test]
fn a_click_in_the_picker_then_apply_saves_that_colour() {
    let dir = Scratch::new("tintpickerclick");
    let (ui, path) = tint_picker(&dir, BLUE);
    let (x, y) = centre(&El::find_by_accessible_label(&ui, "Hue and saturation").next().expect("hue and saturation area"));
    move_pointer(&ui, x, y);
    press(&ui, x, y, slint::platform::PointerEventButton::Left);
    button(&ui, "Apply").expect("Apply button").invoke_accessible_default_action();
    let expected = rgb_of(slint::Color::from_hsva(180.0, 0.5, 0.83, 1.0));
    saved(&path, "the clicked colour", |s| s.acrylic_tint == Some(expected));
    assert!(!ui.get_tint_picker_visible(), "Apply closes the picker");
    for label in ["Default colour", "Black", "White", "Blue", "Purple", "Green", "Red"] {
        assert_eq!(radio(&ui, label).accessible_checked(), Some(false), "{label}");
    }
    assert_eq!(radio(&ui, "Custom colour").accessible_checked(), Some(true));
}

#[test]
fn the_picker_keys_move_hue_and_saturation_and_return_applies() {
    use slint::platform::Key;
    // Not fully saturated, so Up has room: hue 210, saturation 0.4, brightness 63%.
    let seed = [96, 128, 160];
    let dir = Scratch::new("tintpickerkeys");
    let (ui, path) = tint_picker(&dir, seed);
    let hsv = slint::Color::from_rgb_u8(seed[0], seed[1], seed[2]).to_hsva();
    key(&ui, Key::RightArrow);
    key(&ui, Key::UpArrow);
    key(&ui, Key::Return);
    let expected = rgb_of(slint::Color::from_hsva(hsv.hue + 10.0, hsv.saturation + 0.1, 0.63, 1.0));
    saved(&path, "the keyed colour", |s| s.acrylic_tint == Some(expected));
    assert!(!ui.get_tint_picker_visible(), "Return applies and closes");
}

#[test]
fn cancel_closes_the_picker_and_changes_nothing() {
    let dir = Scratch::new("tintpickercancel");
    let (ui, _path) = tint_picker(&dir, BLUE);
    let before = window_bg(&ui);
    let (x, y) = centre(&El::find_by_accessible_label(&ui, "Hue and saturation").next().expect("hue and saturation area"));
    move_pointer(&ui, x, y);
    press(&ui, x, y, slint::platform::PointerEventButton::Left);
    button(&ui, "Cancel").expect("Cancel button").invoke_accessible_default_action();
    assert!(!ui.get_tint_picker_visible());
    assert_eq!(window_bg(&ui), before);
    assert_eq!(radio(&ui, "Blue").accessible_checked(), Some(true), "the saved tint stands");
}

#[test]
fn escape_closes_the_picker_and_changes_nothing() {
    let dir = Scratch::new("tintpickeresc");
    let (ui, _path) = tint_picker(&dir, BLUE);
    let before = window_bg(&ui);
    key(&ui, slint::platform::Key::RightArrow);
    key(&ui, slint::platform::Key::Escape);
    assert!(!ui.get_tint_picker_visible());
    assert_eq!(window_bg(&ui), before);
    assert_eq!(radio(&ui, "Blue").accessible_checked(), Some(true), "the saved tint stands");
}

/// Clicks the middle of the picker's brightness slider (it moves to 50), then Right: the slider,
/// not the picker, has the keys (51).
fn focus_brightness(ui: &MainWindow) {
    let (x, y) = centre(&slider(ui, "Brightness"));
    move_pointer(ui, x, y);
    press(ui, x, y, slint::platform::PointerEventButton::Left);
    key(ui, slint::platform::Key::RightArrow);
    assert_eq!(slider(ui, "Brightness").accessible_value().as_deref(), Some("51"), "the slider has focus");
}

#[test]
fn return_applies_while_the_brightness_slider_has_focus() {
    let dir = Scratch::new("tintpickerslideret");
    let (ui, path) = tint_picker(&dir, BLUE);
    focus_brightness(&ui);
    key(&ui, slint::platform::Key::Return);
    let hsv = slint::Color::from_rgb_u8(BLUE[0], BLUE[1], BLUE[2]).to_hsva();
    let expected = rgb_of(slint::Color::from_hsva(hsv.hue, hsv.saturation, 0.51, 1.0));
    saved(&path, "the slider's brightness", |s| s.acrylic_tint == Some(expected));
    assert!(!ui.get_tint_picker_visible());
}

#[test]
fn escape_cancels_while_the_brightness_slider_has_focus() {
    let dir = Scratch::new("tintpickerslideresc");
    let (ui, _path) = tint_picker(&dir, BLUE);
    let before = window_bg(&ui);
    focus_brightness(&ui);
    key(&ui, slint::platform::Key::Escape);
    assert!(!ui.get_tint_picker_visible());
    assert_eq!(window_bg(&ui), before);
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

#[test]
fn clicking_a_metric_header_selects_its_meter_and_clicking_again_flips() {
    let dir = Scratch::new("sortclick");
    let path = dir.0.join("settings.json");
    let (ui, controls, _settings) = settings_wired(&dir, Settings::default(), false);
    assert_eq!((ui.get_sort_column(), ui.get_sort_descending(), ui.get_selected_metric()), (0, true, 0));
    ui.invoke_sort_clicked(1);
    assert_eq!((ui.get_selected_metric(), ui.get_sort_column(), ui.get_sort_descending()), (1, 1, true));
    assert_eq!(controls.metric(), negotium_model::Metric::Memory, "the meter selection follows");
    ui.invoke_sort_clicked(1);
    assert_eq!((ui.get_selected_metric(), ui.get_sort_column(), ui.get_sort_descending()), (1, 1, false));
    assert!(!controls.sort().descending);
    ui.invoke_sort_clicked(-1);
    assert_eq!((ui.get_selected_metric(), ui.get_sort_column(), ui.get_sort_descending()), (1, -1, false));
    assert_eq!(controls.sort().column, negotium_model::SortColumn::Name);
    assert_eq!(controls.metric(), negotium_model::Metric::Memory, "Name leaves the meter where it was");
    saved(&path, "sorted by name, A to Z", |s| s.sort_column == "name" && !s.sort_descending);
}

#[test]
fn selecting_a_meter_sorts_by_it_descending() {
    let dir = Scratch::new("metersort");
    let path = dir.0.join("settings.json");
    let (ui, controls, _settings) = settings_wired(&dir, Settings::default(), false);
    ui.invoke_sort_clicked(-1);
    ui.invoke_select_metric(3);
    assert_eq!((ui.get_selected_metric(), ui.get_sort_column(), ui.get_sort_descending()), (3, 3, true));
    assert_eq!(controls.sort(), negotium_model::Sort::by(negotium_model::SortColumn::Metric(negotium_model::Metric::Gpu)));
    saved(&path, "sorted by GPU", |s| s.sort_column == "gpu" && s.sort_descending);
}

#[test]
fn the_group_by_type_switch_shows_and_reports_its_state() {
    let dir = Scratch::new("grouping");
    let path = dir.0.join("settings.json");
    let (ui, controls, settings) = settings_wired(&dir, Settings::default(), false);
    assert_eq!(switch(&ui, "Group apps by type").accessible_checked(), Some(true));
    switch(&ui, "Group apps by type").invoke_accessible_default_action();
    assert_eq!(switch(&ui, "Group apps by type").accessible_checked(), Some(false));
    assert!(!settings.borrow().group_by_type && !controls.grouped());
    saved(&path, "grouping off", |s| !s.group_by_type);
    negotium::settingspage::apply_settings(&ui, &Settings { group_by_type: true, ..Settings::default() });
    assert_eq!(switch(&ui, "Group apps by type").accessible_checked(), Some(true), "launch settings are shown");
}

/// The rect of each setting control on the page, by label.
fn settings_rects(ui: &MainWindow) -> Vec<(String, Rect)> {
    let mut out = Vec::new();
    for l in ["0.5 s", "5 s", "System", "Dark"] {
        out.push((l.to_string(), rect_of(&radio(ui, l))));
    }
    for l in ["Smooth graphs", "Show CPU in tray icon", "Always on top", "Keep running in the tray when closed", "Use GPU rendering", "Admin mode", "CPU temperature add-on"] {
        out.push((l.to_string(), rect_of(&switch(ui, l))));
    }
    out
}

/// The Settings page alone at `w` × `h`.
fn settings_window(w: u32, h: u32) -> MainWindow {
    page_window(3, w, h)
}

/// The window at `w` × `h` showing `page`; may be called more than once per test.
fn page_window(page: i32, w: u32, h: u32) -> MainWindow {
    thread_local! { static BACKEND: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
    if !BACKEND.with(|b| b.replace(true)) {
        i_slint_backend_testing::init_no_event_loop();
    }
    let ui = MainWindow::new().unwrap();
    ui.window().set_size(PhysicalSize::new(w, h));
    ui.set_page(page);
    ui.show().unwrap();
    ui
}

#[test]
fn a_tall_window_leaves_the_settings_their_size_and_place() {
    let measure = |h: u32| {
        let ui = settings_window(1000, h);
        (settings_rects(&ui), rect_of(&text(&ui, "Settings").expect("page title")))
    };
    let (short, short_title) = measure(2600);
    let (tall, tall_title) = measure(3400);
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
    // Appearance comes first: Theme, then Backdrop (a long description), then Smooth graphs.
    let next = row_texts(&ui, "title")[2];
    let desc = row_texts(&ui, "description")[1];
    assert!(desc.3 > 20.0, "the long description wraps onto more lines: {desc:?}");
    assert!(desc.1 + desc.3 <= next.1, "the next row starts below it: {desc:?} {next:?}");
    let choice = rect_of(&radio(&ui, "Acrylic"));
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
        "Smooth graphs" => "smooth-row",
        "Show CPU in tray icon" => "tray-cpu-row",
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
    ui.on_smooth_graphs_toggled(move |_| l.borrow_mut().push("Smooth graphs".into()));
    let l = log.clone();
    ui.on_tray_cpu_toggled(move |_| l.borrow_mut().push("Show CPU in tray icon".into()));
    let l = log.clone();
    ui.on_select_page(move |i| l.borrow_mut().push(format!("page {i}")));
    // Space types into the search box on the way: match the way the app does.
    ui.on_setting_matches(|q, t, d| negotium::settings::setting_matches(&q, &t, &d));
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
    for _ in 0..60 {
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
    assert!(seen.contains(&"Smooth graphs".to_string()), "{seen:?}");
    assert!(seen.contains(&"Show CPU in tray icon".to_string()), "{seen:?}");
    // And back up: the first choice comes back into view.
    let mut back = None;
    for _ in 0..60 {
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
    // Tall enough for every row: the testing backend finds only what the page's view shows.
    let ui = settings_window(1000, 2600);
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
    assert_eq!(row_texts(&ui, "title").len(), 18, "the titles are still drawn");
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

// ---- Plan 3b: startup ----

fn startup_row(id: i32, name: &str, enabled: bool, can_change: bool, note: &str) -> StartupData {
    StartupData {
        id, name: name.into(), publisher: "Contoso".into(), status: if enabled { "Enabled" } else { "Disabled" }.into(),
        location: "Registry (you)".into(), command: format!(r"C:\Apps\{name}.exe").into(), enabled, can_change, note: note.into(),
    }
}

#[test]
fn startup_rows_show_their_state_and_report_toggles() {
    let ui = shown_window();
    ui.set_page(2);
    ui.set_startup_rows(ModelRc::new(VecModel::from(vec![
        startup_row(0, "Discord", true, true, ""),
        startup_row(1, "Old tool", false, false, "Needs admin"),
    ])));
    let toggled = Rc::new(RefCell::new(Vec::new()));
    let t = toggled.clone();
    ui.on_toggle_startup(move |id, on| t.borrow_mut().push((id, on)));
    let discord = switch(&ui, "Start Discord with Windows");
    assert_eq!(discord.accessible_checked(), Some(true));
    discord.invoke_accessible_default_action();
    assert_eq!(*toggled.borrow(), [(0, false)]);
    let old = switch(&ui, "Start Old tool with Windows");
    assert_eq!(old.accessible_enabled(), Some(false), "an all-users entry without admin can't be switched");
    old.invoke_accessible_default_action();
    assert_eq!(toggled.borrow().len(), 1);
    assert!(El::find_by_accessible_label(&ui, "Needs admin").next().is_some());
}

const STARTUP_BANNER: &str = "Apps that start for all users can only be changed with admin rights.";

#[test]
fn the_startup_admin_banner_offers_restart_inline() {
    let ui = shown_window();
    ui.set_page(2);
    ui.set_startup_admin_banner(true);
    let restarted = Rc::new(RefCell::new(false));
    let r = restarted.clone();
    ui.on_restart_admin(move || *r.borrow_mut() = true);
    El::find_by_accessible_label(&ui, "Restart as admin").next().expect("inline button").invoke_accessible_default_action();
    assert!(*restarted.borrow());
    // The Overview's banner is the same component; only it can be dismissed.
    assert!(text(&ui, STARTUP_BANNER).is_some());
    assert_eq!(El::find_by_accessible_label(&ui, "Dismiss").count(), 0, "the Startup banner has no Dismiss");
    ui.set_page(0);
    ui.set_show_admin_banner(true);
    assert_eq!(El::find_by_accessible_label(&ui, "Dismiss").count(), 1);
    assert!(text(&ui, OVERVIEW_BANNER).is_some(), "the Overview keeps its text");
}

const OVERVIEW_BANNER: &str = "Some processes need admin rights to show details, network use, or be ended.";

#[test]
fn the_admin_banner_mentions_network_use() {
    let ui = shown_window();
    ui.set_page(0);
    ui.set_show_admin_banner(true);
    assert!(text(&ui, OVERVIEW_BANNER).is_some(), "per-app network needs admin too");
}

#[test]
fn the_bridge_shows_startup_entries_and_a_pending_toggle() {
    use negotium_actions::startup::{StartupEntry, StartupSource};
    use slint::Model;
    let (ui, _controls, bridge) = bridged();
    let entry = |key: &str, name: &str, source| StartupEntry {
        source, key: key.into(), name: name.into(), publisher: None, enabled: true, changeable: true,
        command: String::new(), location: String::new(),
    };
    let entries = vec![entry("z", "Zoom", StartupSource::UserRun), entry("a", "Audio", StartupSource::MachineRun)];
    bridge.borrow_mut().set_startup(&ui, entries, false);
    let rows = ui.get_startup_rows();
    assert_eq!(rows.row_data(0).unwrap().name, "Audio");
    assert!(ui.get_startup_admin_banner());
    assert!(!ui.get_startup_loading());
    assert_eq!(bridge.borrow().startup_entry(0).unwrap().key, "z");
    assert_eq!(bridge.borrow().startup_entry(7), None);
    bridge.borrow().show_startup_pending(0, false);
    let zoom = (0..rows.row_count()).map(|i| rows.row_data(i).unwrap()).find(|r| r.name == "Zoom").unwrap();
    assert!(!zoom.enabled, "the switch moves at once; the re-read shows the real state");
    assert_eq!(zoom.status, "Disabled");
}

fn startup_entries() -> Vec<negotium_actions::startup::StartupEntry> {
    use negotium_actions::startup::{StartupEntry, StartupSource};
    vec![StartupEntry {
        source: StartupSource::UserRun, key: "z".into(), name: "Zoom".into(), publisher: None, enabled: true,
        changeable: true, command: String::new(), location: String::new(),
    }]
}

// PF4: while a list or toggle job runs, a toggle starts nothing, so an older list can't land last.
#[test]
fn a_toggle_while_the_list_is_read_starts_nothing() {
    use slint::Model;
    let (ui, _controls, bridge) = bridged();
    bridge.borrow_mut().set_startup(&ui, startup_entries(), false);
    assert!(negotium::startup::begin_list(&ui), "a list starts");
    assert!(ui.get_startup_loading());
    assert!(!negotium::startup::begin_list(&ui), "not a second one while it runs");
    assert_eq!(negotium::startup::begin_toggle(&bridge.borrow(), &ui, 0, false), None, "no toggle while listing");
    assert!(ui.get_startup_rows().row_data(0).unwrap().enabled, "the switch keeps the real state");
    // The list lands: now a toggle starts, moves the switch and holds off the next job.
    bridge.borrow_mut().set_startup(&ui, startup_entries(), false);
    let entry = negotium::startup::begin_toggle(&bridge.borrow(), &ui, 0, false).expect("a toggle starts");
    assert_eq!(entry.key, "z");
    assert!(!ui.get_startup_rows().row_data(0).unwrap().enabled);
    assert!(ui.get_startup_loading());
    assert_eq!(negotium::startup::begin_toggle(&bridge.borrow(), &ui, 0, true), None, "nor a second toggle");
    assert!(!negotium::startup::begin_list(&ui), "nor a list");
    // An unknown row starts nothing either.
    bridge.borrow_mut().set_startup(&ui, startup_entries(), false);
    assert_eq!(negotium::startup::begin_toggle(&bridge.borrow(), &ui, 5, true), None);
    assert!(!ui.get_startup_loading());
}

#[test]
fn a_job_result_lands_through_the_bridge_with_its_failure_inline() {
    use slint::Model;
    let (ui, _controls, bridge) = bridged();
    negotium::bridge::set_current(bridge.clone());
    ui.set_startup_loading(true);
    let outcome = |read_error, failure: Option<&str>| JobOutcome {
        entries: Some(startup_entries()), read_error, failure: failure.map(Into::into), panicked: false,
    };
    negotium::startup::deliver(&ui, outcome(false, Some("Couldn't change Zoom: access denied.")), false);
    assert_eq!(ui.get_startup_rows().row_count(), 1);
    assert!(!ui.get_startup_loading());
    assert!(!ui.get_startup_read_note());
    assert_eq!(ui.get_toast(), "Couldn't change Zoom: access denied.");
    negotium::startup::deliver(&ui, outcome(true, None), false);
    assert!(ui.get_startup_read_note(), "an unreadable list is noted");
    negotium::startup::deliver(&ui, outcome(false, None), false);
    assert!(!ui.get_startup_read_note(), "and the note goes once everything reads");
}

use negotium::startup::JobOutcome;

// Fix round 1 (2a): a job that failed outright still clears the loading state, undoes the
// pending switch and says so.
#[test]
fn a_failed_job_restores_the_listed_state_and_frees_the_page() {
    use slint::Model;
    let (ui, _controls, bridge) = bridged();
    negotium::bridge::set_current(bridge.clone());
    bridge.borrow_mut().set_startup(&ui, startup_entries(), false);
    negotium::startup::begin_toggle(&bridge.borrow(), &ui, 0, false).expect("a toggle starts");
    assert!(!ui.get_startup_rows().row_data(0).unwrap().enabled);
    let failed = JobOutcome { entries: None, read_error: false, failure: Some(negotium::startup::JOB_FAILED.into()), panicked: true };
    negotium::startup::deliver(&ui, failed, false);
    assert!(!ui.get_startup_loading(), "the next job may start");
    let row = ui.get_startup_rows().row_data(0).unwrap();
    assert!(row.enabled && row.status == "Enabled", "the switch shows the last listed state again");
    assert_eq!(ui.get_toast(), negotium::startup::JOB_FAILED);
}

const READ_NOTE: &str = "Some startup apps couldn't be read; details are in the log.";

// Fix round 1 (1, 2b): an unreadable list is noted on the page; a running job over a shown list
// says "Updating…".
#[test]
fn the_page_notes_unreadable_lists_and_a_running_update() {
    let ui = startup_window(1000, 720, 3);
    assert!(text(&ui, READ_NOTE).is_none());
    ui.set_startup_read_note(true);
    let note = rect_of(&text(&ui, READ_NOTE).expect("the read note"));
    let header = rect_of(&text(&ui, "Name").expect("header"));
    let first = rect_of(&El::find_by_accessible_label(&ui, "App00, Enabled, Registry (you)").next().expect("row"));
    assert!(note.1 >= header.1 + header.3 && note.1 + note.3 <= first.1, "under the header, above the list: {note:?}");
    assert!(text(&ui, "Updating…").is_none());
    ui.set_startup_loading(true);
    assert!(text(&ui, "Updating…").is_some(), "a job runs over the shown list");
    ui.set_startup_rows(ModelRc::new(VecModel::from(Vec::<StartupData>::new())));
    assert!(text(&ui, "Updating…").is_none(), "with nothing shown the empty state says it instead");
    assert!(text(&ui, "Reading startup apps…").is_some());
}

/// The Overview's list of apps (below its search box and header).
fn apps_list(ui: &MainWindow) -> Rect {
    rect_of(&El::find_by_accessible_label(ui, "Apps").next().expect("apps list"))
}

/// The rect of the element with id `id` ("Component::id").
fn by_id(ui: &MainWindow, id: &str) -> Rect {
    use i_slint_backend_testing::ElementRoot;
    let found = ui.root_element().query_descendants().match_id(id).find_all();
    rect_of(found.first().unwrap_or_else(|| panic!("no element {id}")))
}

// Fix round 1 (2d): the toast never hides the end of a list, and the keyboard reaches its OK.
#[test]
fn the_toast_leaves_the_lists_clear_and_takes_tab_and_space() {
    use slint::platform::Key;
    let ui = startup_window(1000, 720, 20);
    let dismissed = Rc::new(RefCell::new(0));
    let d = dismissed.clone();
    ui.on_dismiss_toast(move || *d.borrow_mut() += 1);
    // Hidden pages are left out of the accessibility tree: measure each while it shows.
    ui.set_page(0);
    let full_apps = apps_list(&ui);
    ui.set_page(2);
    let full = (by_id(&ui, "StartupPage::sv"), full_apps);
    ui.set_toast("Couldn't change App19: access denied.".into());
    for (n, page) in [0, 2].into_iter().enumerate() {
        ui.set_page(page);
        let ok = rect_of(&El::find_by_accessible_label(&ui, "OK").next().expect("the toast's OK"));
        let toast = by_id(&ui, "MainWindow::toast-host");
        assert!(toast.3 > 30.0 && ok.1 >= toast.1);
        let list = if page == 0 { apps_list(&ui) } else { by_id(&ui, "StartupPage::sv") };
        assert!(list.1 + list.3 <= toast.1, "page {page}: the list ends above the toast: {list:?} {toast:?}");
        let before = if page == 0 { full.1 } else { full.0 };
        assert_eq!(before.3 - list.3, toast.3 + 12.0, "page {page}: the list gives up exactly the toast's room");
        let mut reached = false;
        for _ in 0..80 {
            press_key(&ui, Key::Tab);
            press_key(&ui, Key::Space);
            if *dismissed.borrow() == n + 1 {
                reached = true;
                break;
            }
        }
        assert!(reached, "page {page}: Tab reaches the toast's OK and Space dismisses");
    }
    ui.set_toast("".into());
    assert_eq!(by_id(&ui, "StartupPage::sv"), full.0, "no toast, no room kept");

    // The Performance and Settings pages scroll too: in a short window, scrolled to the end, their
    // last item clears the toast.
    let ui = perf_window(1000, 480, &["% Utilization"], cores(16));
    ui.set_toast("Couldn't change App19: access denied.".into());
    for (page, last) in [(1, "A note."), (3, "SettingsPage::temp-row")] {
        ui.set_page(page);
        scroll_detail(&ui, -100_000.0); // to the end
        let toast = by_id(&ui, "MainWindow::toast-host");
        assert!(toast.3 > 30.0, "page {page}: the toast shows");
        let end = if page == 1 { rect(&ui, last) } else { by_id(&ui, last) };
        assert!(end.1 + end.3 <= toast.1, "page {page}: the end scrolls clear of the toast: {end:?} {toast:?}");
    }
}

// PF12: nothing on the Startup page changes per tick; a frame meant for it touches nothing.
#[test]
fn a_frame_on_the_startup_page_leaves_it_alone() {
    use slint::Model;
    let (ui, controls, bridge) = bridged();
    bridge.borrow_mut().set_page(&ui, Page::Startup);
    bridge.borrow_mut().set_startup(&ui, startup_entries(), false);
    bridge.borrow_mut().apply(&ui, Frame { page: Page::Startup, rows: vec![row_view(1, "X", "")], ..Default::default() });
    assert_eq!(controls.page(), Page::Startup);
    assert_eq!(ui.get_rows().row_count(), 0, "no Overview rows built");
    assert_eq!(ui.get_startup_rows().row_count(), 1);
}

#[test]
fn the_rail_reaches_startup() {
    let ui = window();
    let picked = Rc::new(RefCell::new(None));
    let p = picked.clone();
    ui.on_select_page(move |i| *p.borrow_mut() = Some(i));
    El::find_by_accessible_label(&ui, "Startup page").next().expect("rail item").invoke_accessible_default_action();
    assert_eq!(*picked.borrow(), Some(2));
}

#[test]
fn screen_readers_get_a_labelled_list_and_each_switchs_details() {
    let ui = shown_window();
    ui.set_page(2);
    let mut policy = startup_row(1, "Locked", true, false, "Set by policy");
    policy.publisher = "".into();
    ui.set_startup_rows(ModelRc::new(VecModel::from(vec![startup_row(0, "Discord", true, true, ""), policy])));
    assert!(
        El::find_by_accessible_label(&ui, "Startup apps").any(|e| e.accessible_role() == Some(AccessibleRole::List)),
        "a labelled list"
    );
    let item = El::find_by_accessible_label(&ui, "Discord, Enabled, Registry (you)").next().expect("a labelled item");
    assert_eq!(item.accessible_role(), Some(AccessibleRole::ListItem));
    assert_eq!(switch(&ui, "Start Discord with Windows").accessible_description().as_deref(), Some("Contoso, Registry (you)"));
    let locked = switch(&ui, "Start Locked with Windows");
    assert_eq!(locked.accessible_description().as_deref(), Some("Registry (you). Set by policy"));
    assert_eq!(locked.accessible_enabled(), Some(false));
    assert!(text(&ui, "Set by policy").is_some(), "the reason is read out");
    assert_eq!(El::find_by_accessible_label(&ui, "Discord").count(), 0, "the name is read with its item and switch only");
}

/// The Startup page alone at `w` × `h` with `n` rows (every third set by policy) and the banner.
fn startup_window(w: u32, h: u32, n: i32) -> MainWindow {
    let ui = page_window(2, w, h);
    let rows = (0..n).map(|i| {
        startup_row(i, &format!("App{i:02}"), i % 2 == 0, i % 3 != 0, if i % 3 == 0 { "Set by policy" } else { "" })
    });
    ui.set_startup_rows(ModelRc::new(VecModel::from(rows.collect::<Vec<_>>())));
    ui.set_startup_admin_banner(true);
    ui
}

// PF3: spare height goes to the list only.
#[test]
fn a_tall_window_leaves_the_startup_header_and_banner_their_size() {
    let measure = |h: u32, n: i32| {
        let ui = startup_window(1000, h, n);
        let mut out = vec![
            rect_of(&text(&ui, "Startup apps").expect("title")),
            rect_of(&text(&ui, STARTUP_BANNER).expect("banner")),
            rect_of(&El::find_by_accessible_label(&ui, "Restart as admin").next().expect("banner button")),
            rect_of(&text(&ui, "Publisher").expect("header")),
        ];
        if n == 0 {
            out.push(rect_of(&text(&ui, "No apps start with Windows.").expect("empty state")));
            ui.set_startup_loading(true);
            out.push(rect_of(&text(&ui, "Reading startup apps…").expect("loading state")));
        } else {
            out.push(rect_of(&switch(&ui, "Start App01 with Windows")));
            out.push(rect_of(&El::find_by_accessible_label(&ui, "App00, Enabled, Registry (you)").next().expect("first row")));
        }
        (out, by_id(&ui, "StartupPage::sv"))
    };
    for n in [0, 3] {
        let ((short, short_list), (tall, tall_list)) = (measure(720, n), measure(1100, n));
        assert_eq!(short, tall, "with {n} rows nothing above the list, nor the rows, moves or grows");
        assert_eq!(tall_list.3 - short_list.3, 380.0, "with {n} rows the list takes all the spare height");
    }
    let header = measure(1100, 3).0[3];
    assert!(header.3 < 20.0, "the header keeps its text height: {header:?}");
    let ui = startup_window(1000, 1100, 3);
    let first = rect_of(&switch(&ui, "Start App01 with Windows"));
    assert!(first.1 - (header.1 + header.3) < 80.0, "the rows follow the header: {first:?}");
}

// PF19: "Set by policy" fits beside its switch at the smallest window.
#[test]
fn the_policy_note_fits_beside_its_switch() {
    let ui = startup_window(760, 440, 1);
    let note = rect_of(&text(&ui, "Set by policy").expect("note"));
    let toggle = rect_of(&switch(&ui, "Start App00 with Windows"));
    let location = rect_of(&text(&ui, "Location").expect("header"));
    assert!(note.2 > 40.0, "the note has its width: {note:?}");
    assert!(note.0 >= location.0 + location.2, "the note stays out of the location column: {note:?} {location:?}");
    assert!(note.0 + note.2 <= toggle.0, "and clear of the switch: {note:?} {toggle:?}");
    assert!(toggle.0 + toggle.2 <= 760.0 - 14.0, "the switch clears the scrollbar gutter: {toggle:?}");
}

#[test]
fn a_startup_switch_reached_with_tab_is_scrolled_into_view_and_space_toggles_it() {
    use slint::platform::Key;
    let ui = startup_window(760, 440, 20);
    // The list's view: below the column header, above the page's bottom padding.
    let header = rect_of(&text(&ui, "Name").expect("header"));
    let row_in_view = |ui: &MainWindow, id: i32| {
        let label = format!("App{id:02}, {}, Registry (you)", if id % 2 == 0 { "Enabled" } else { "Disabled" });
        let r = rect_of(&El::find_by_accessible_label(ui, &label).next().unwrap_or_else(|| panic!("row {label}")));
        assert!(r.1 >= header.1 + header.3 && r.1 + r.3 <= 440.0 - 12.0 + 0.5, "App{id:02}: its whole row is in view: {r:?}");
    };
    let log = Rc::new(RefCell::new(Vec::<(i32, bool)>::new()));
    let l = log.clone();
    ui.on_toggle_startup(move |id, on| l.borrow_mut().push((id, on)));
    // Space acts on the focused control: the toggle it reports names it.
    let current = |ui: &MainWindow| {
        log.borrow_mut().clear();
        press_key(ui, Key::Space);
        log.borrow().last().copied()
    };
    let mut seen = Vec::new();
    for _ in 0..60 {
        press_key(&ui, Key::Tab);
        let Some((id, on)) = current(&ui) else { continue };
        let r = rect_of(&switch(&ui, &format!("Start App{id:02} with Windows")));
        assert!(r.1 >= 0.0 && r.1 + r.3 <= 440.0, "App{id:02} is in view when focused: {r:?}");
        row_in_view(&ui, id);
        assert_eq!(on, id % 2 != 0, "Space asks for the other state");
        seen.push(id);
        if id == 19 {
            break;
        }
    }
    let changeable: Vec<i32> = (0..20).filter(|i| i % 3 != 0).collect();
    assert_eq!(seen, changeable, "Tab reaches every switch that can change, in order, and skips the rest");
    // And back up to the first.
    let mut back = None;
    for _ in 0..60 {
        press_key(&ui, Key::Backtab);
        back = current(&ui);
        if back.map(|b| b.0) == Some(1) {
            break;
        }
    }
    assert_eq!(back.map(|b| b.0), Some(1), "Shift+Tab reaches the first switch");
    row_in_view(&ui, 1);
}

#[test]
fn the_toast_shows_on_every_page() {
    // A click on its OK reaches it on every page (a hidden page gets no input).
    let ui = shown_window();
    let dismissed = Rc::new(RefCell::new(0));
    let d = dismissed.clone();
    ui.on_dismiss_toast(move || *d.borrow_mut() += 1);
    ui.set_toast("Couldn't change Zoom: access denied.".into());
    for page in [0, 1, 2, 3] {
        ui.set_page(page);
        let toast = rect_of(&text(&ui, "Couldn't change Zoom: access denied.").expect("toast"));
        assert!(toast.0 >= 76.0 && toast.1 > 600.0 && toast.1 + toast.3 <= 720.0, "page {page}: at the bottom of the window: {toast:?}");
        let ok = El::find_by_accessible_label(&ui, "OK").next().expect("the toast's OK");
        let (x, y) = centre(&ok);
        press(&ui, x, y, slint::platform::PointerEventButton::Left);
        assert_eq!(*dismissed.borrow(), page + 1, "page {page}: the toast takes the click");
    }
}

// ---- Plan 3c Task 3: smooth graphs ----

use negotium::GraphClock;

/// The testing backend's mocked time, as GraphClock's clock (crate::smooth::install).
fn mocked_clock() -> std::time::Duration {
    std::time::Duration::from_millis(i_slint_backend_testing::get_mocked_time())
}

#[test]
fn the_graph_clock_runs_from_its_lead_to_the_newest_sample_and_stops_there() {
    let ui = shown_window();
    negotium::smooth::install(&ui, mocked_clock); // the timer's rate and clock (Bridge::install does this in the app)
    let clock = ui.global::<GraphClock>();
    assert!(clock.get_live(), "smooth graphs are on by default");
    ui.invoke_scroll_from(-1_000.0, 1.0);
    assert_eq!(clock.get_offset_ms(), -1_000.0);
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO); // the timer starts
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(400));
    let o = clock.get_offset_ms();
    assert!((o + 600.0).abs() < 1.0, "400 ms later the edge is 600 ms behind: {o}");
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(2_000));
    assert_eq!(clock.get_offset_ms(), 0.0, "it stops on the newest sample");
    let now = clock.get_now();
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(1_000));
    assert_eq!(clock.get_now(), now, "and the timer stops with it");
}

#[test]
fn the_graph_clock_stands_still_with_smooth_graphs_off_hidden_or_on_pages_without_graphs() {
    let ui = shown_window();
    negotium::smooth::install(&ui, mocked_clock); // a clock that moves, so standing still means something
    let clock = ui.global::<GraphClock>();
    ui.invoke_scroll_from(-1_000.0, 1.0);
    clock.set_smooth(false);
    assert!(!clock.get_live());
    assert_eq!(clock.get_offset_ms(), 0.0, "off: the newest sample sits on the edge");
    clock.set_smooth(true);
    clock.set_shown(false);
    assert_eq!(clock.get_offset_ms(), 0.0, "hidden or minimised: no motion");
    clock.set_shown(true);
    // The control: on the Overview the same run moves.
    ui.invoke_scroll_from(-1_000.0, 1.0);
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO);
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(1_000));
    assert!(clock.get_offset_ms() > -1_000.0, "the Overview's sparklines scroll: {}", clock.get_offset_ms());
    ui.set_page(2);
    ui.invoke_scroll_from(-1_000.0, 1.0);
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO);
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(1_000));
    assert_eq!(clock.get_offset_ms(), -1_000.0, "the Startup page has no graphs: the timer doesn't run");
}

/// Setting a running Timer's interval re-arms it from now, and a run's speed changes with nearly
/// every frame: the interval must not follow it, or a slow graph moves only when frames land.
#[test]
fn the_graph_timers_interval_does_not_follow_the_runs_speed() {
    let ui = shown_window();
    negotium::smooth::install(&ui, mocked_clock);
    for page in [0, 1] {
        ui.set_page(page);
        ui.invoke_scroll_from(-1_000.0, 1.0);
        let interval = ui.get_scroll_interval();
        for speed in [0.5, 0.73, 1.2, 1.9, 2.0] {
            ui.invoke_scroll_from(-1_000.0, speed);
            assert_eq!(ui.get_scroll_interval(), interval, "page {page}, speed {speed}");
        }
    }
}

/// PF3: a run cut short (smoothing off, the window hidden or minimised) rests on the newest sample,
/// and stays there if the graphs may scroll again before the next frame lands: no jump back.
#[test]
fn a_run_cut_short_rests_on_the_newest_sample_when_scrolling_resumes() {
    let ui = shown_window();
    negotium::smooth::install(&ui, mocked_clock);
    let clock = ui.global::<GraphClock>();
    ui.invoke_scroll_from(-1_000.0, 1.0);
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO);
    // The Overview's sparklines move a pixel about every 400 ms: the timer has ticked once.
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(500));
    assert!(clock.get_offset_ms() < 0.0 && clock.get_offset_ms() > -1_000.0, "a run under way: {}", clock.get_offset_ms());
    clock.set_shown(false);
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO); // change handlers run
    clock.set_shown(true);
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(200));
    assert_eq!(clock.get_offset_ms(), 0.0, "restored: still on the newest sample");
    assert_eq!(clock.get_speed(), 0.0);
}

#[test]
fn graphs_are_redrawn_as_often_as_a_pixel_of_scrolling_needs_up_to_60_a_second() {
    let ui = shown_window(); // 1000 × 720
    negotium::smooth::install(&ui, mocked_clock);
    let overview = ui.get_scroll_interval();
    assert!((200..=800).contains(&overview), "1 min sparklines, ~150 px wide: {overview} ms");
    ui.set_page(1);
    ui.set_perf_span_ms(60_000.0);
    let minute = ui.get_scroll_interval();
    assert!((50..=160).contains(&minute), "the 1 min main graph, ~650 px wide: {minute} ms");
    ui.set_perf_span_ms(3_600_000.0);
    assert_eq!(ui.get_scroll_interval(), 1_000, "the 1 h graph moves under a pixel a second");
    ui.set_perf_span_ms(60_000.0);
    ui.window().set_size(PhysicalSize::new(5_120, 1_440));
    assert_eq!(ui.get_scroll_interval(), 16, "never more than about 60 frames a second");
}

/// The graph clock reads a clock Rust supplies, not `animation-tick()`: that call marks an
/// animation as running, and the GPU backend then draws a second, identical frame for every timer
/// tick (fix wave R9: a 22 Hz timer drew 45 frames a second).
#[test]
fn the_graph_timer_and_a_landing_frame_ask_for_no_extra_frame() {
    let ui = shown_window();
    negotium::smooth::install(&ui, mocked_clock);
    let clock = ui.global::<GraphClock>();
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    ui.invoke_scroll_from(-5_000.0, 1.0);
    assert!(!ui.window().has_active_animations(), "a landing frame starts no animation");
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO); // the timer starts
    let now = clock.get_now();
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(500)); // a tick on the Overview (~400 ms)
    assert!(clock.get_now() > now, "the timer ran");
    assert!(!ui.window().has_active_animations(), "a timer tick starts no animation");
}

/// An epoch `ms` ago, and when it was taken, so assertions can allow for the time the test itself
/// takes (collector time moves on meanwhile; ruling PF5).
fn epoch_ago(ms: u64) -> (std::time::Instant, std::time::Instant) {
    let start = std::time::Instant::now();
    (start - std::time::Duration::from_millis(ms), start)
}

/// `value` is `expected` plus at most the time measured since `start`.
fn within_elapsed(value: f32, expected: f32, start: std::time::Instant) -> bool {
    let elapsed = start.elapsed().as_secs_f32() * 1_000.0;
    value >= expected - 0.5 && value <= expected + elapsed + 0.5
}

#[test]
fn landing_frames_continue_the_scrolling_view_from_where_it_is() {
    let (ui, controls, bridge) = bridged();
    let clock = ui.global::<GraphClock>();
    clock.set_smooth(true);
    controls.set_page(Page::Performance);
    // Collector time is now about 61 s.
    let (epoch, start) = epoch_ago(61_000);
    bridge.borrow_mut().set_epoch(epoch);
    bridge.borrow_mut().apply(&ui, scrub_frame(60_000, TWO_PM));
    let first = clock.get_lead_ms();
    assert!(within_elapsed(first, -250.0, start), "the first run starts 1.25 s behind now: {first}");
    bridge.borrow_mut().apply(&ui, scrub_frame(61_000, TWO_PM));
    let lead = clock.get_lead_ms();
    assert!(within_elapsed(lead, -1_250.0, start), "the next carries on from there, 1.25 s behind its newest sample: {lead}");
    assert!((0.5..=2.0).contains(&clock.get_speed()));
    assert_eq!(ui.get_perf_span_ms(), 60_000.0);
}

#[test]
fn overview_frames_scroll_the_sparklines_too() {
    let (ui, _controls, bridge) = bridged();
    let clock = ui.global::<GraphClock>();
    clock.set_smooth(true);
    let (epoch, start) = epoch_ago(61_000);
    bridge.borrow_mut().set_epoch(epoch);
    bridge.borrow_mut().apply(&ui, Frame { graph_end_ms: 60_000, ..frame(vec![]) });
    assert!(within_elapsed(clock.get_lead_ms(), -250.0, start), "{}", clock.get_lead_ms());
}

/// PF3: frames that land while graphs don't scroll rest the view on their newest sample; when
/// scrolling resumes the view carries on from there instead of jumping back a lag (1.25 s).
#[test]
fn frames_while_graphs_stand_still_rest_on_the_newest_sample_and_resume_without_a_jump() {
    let (ui, controls, bridge) = bridged();
    let clock = ui.global::<GraphClock>();
    controls.set_page(Page::Performance);
    let (epoch, _) = epoch_ago(61_000);
    bridge.borrow_mut().set_epoch(epoch);
    // Smoothing off (bridged()): the frame rests on its newest sample.
    bridge.borrow_mut().apply(&ui, scrub_frame(60_000, TWO_PM));
    assert_eq!((clock.get_lead_ms(), clock.get_speed()), (0.0, 0.0));
    clock.set_smooth(true);
    assert_eq!(clock.get_offset_ms(), 0.0, "turning it on moves nothing");
    // The next frame: the view starts where it rests (the last newest sample), 1 s behind the new one.
    bridge.borrow_mut().apply(&ui, scrub_frame(61_000, TWO_PM));
    assert_eq!(clock.get_lead_ms(), -1_000.0, "from the sample on the edge, not a lag behind now");
    assert!(clock.get_speed() > 0.0);
    // After a long rest (that sample long gone from the paths) it starts on the newest sample.
    clock.set_smooth(false);
    bridge.borrow_mut().apply(&ui, scrub_frame(62_000, TWO_PM));
    clock.set_smooth(true);
    bridge.borrow_mut().apply(&ui, scrub_frame(3_662_000, TWO_PM));
    assert_eq!(clock.get_lead_ms(), 0.0, "an hour later: the newest sample on the edge, no jump back");
}

/// PF3: pages without graphs (Startup, Settings) rest the view on the newest sample.
#[test]
fn leaving_for_a_page_without_graphs_rests_the_view() {
    let (ui, controls, bridge) = bridged();
    let clock = ui.global::<GraphClock>();
    clock.set_smooth(true);
    controls.set_page(Page::Performance);
    let (epoch, _) = epoch_ago(61_000);
    bridge.borrow_mut().set_epoch(epoch);
    bridge.borrow_mut().apply(&ui, scrub_frame(60_000, TWO_PM));
    assert!(clock.get_lead_ms() < 0.0);
    bridge.borrow_mut().set_page(&ui, Page::Startup);
    assert_eq!((clock.get_lead_ms(), clock.get_speed(), clock.get_offset_ms()), (0.0, 0.0, 0.0));
    bridge.borrow_mut().set_page(&ui, Page::Performance);
    bridge.borrow_mut().apply(&ui, scrub_frame(61_000, TWO_PM));
    assert_eq!(clock.get_lead_ms(), -1_000.0, "back: from the sample on the edge");
}

// ---- Plan 3c Task 4: the scrub while graphs scroll ----

#[test]
fn a_scrub_maps_the_pointer_from_where_the_scrolling_graph_is_drawn() {
    use slint::Model;
    let (ui, controls, bridge) = bridged();
    ui.global::<GraphClock>().set_smooth(true);
    controls.set_page(Page::Performance);
    bridge.borrow_mut().set_history(scrub_history());
    bridge.borrow_mut().set_collector_now(Some(61_000.0)); // pinned: the edge doesn't depend on wall time
    bridge.borrow_mut().apply(&ui, scrub_frame(60_000, TWO_PM - 1_000));
    bridge.borrow_mut().apply(&ui, scrub_frame(61_000, TWO_PM));
    // The view trails its newest sample (61 s) by 1.25 s: the right edge shows the sample at 60 s.
    bridge.borrow_mut().scrub(&ui, 1.0);
    assert_eq!(ui.get_scrub_title(), "Top apps at 13:59:59");
    assert_eq!(ui.get_scrub_rows().row_data(0).unwrap().value, "60.0%");
    ui.global::<GraphClock>().set_smooth(false);
    bridge.borrow_mut().scrub(&ui, 1.0);
    assert_eq!(ui.get_scrub_title(), "Top apps at 14:00:00", "standing still, the edge is the newest sample");
}

#[test]
fn the_graph_timer_asks_a_scrub_to_follow_and_leaves_it_alone_otherwise() {
    let ui = shown_window();
    ui.set_page(1);
    let follows = Rc::new(std::cell::Cell::new(0));
    let f = follows.clone();
    ui.on_scroll_follow(move || f.set(f.get() + 1));
    ui.set_scrub_x(0.5);
    ui.invoke_scroll_from(-5_000.0, 1.0);
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO);
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(1_000));
    assert!(follows.get() >= 1, "the content moved under a still pointer: the card follows");
    ui.set_scrub_x(-1.0);
    let before = follows.get();
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(1_000));
    assert_eq!(follows.get(), before, "no scrub, no follow-up");
}

/// PF6: a still pointer on a scrolling graph is asked to follow ~22 times a second; the card is
/// rebuilt only when the sample or column under it changes.
#[test]
fn a_still_pointer_rebuilds_the_card_only_when_what_it_shows_changes() {
    let (ui, controls, bridge) = bridged();
    ui.global::<GraphClock>().set_smooth(true);
    controls.set_page(Page::Performance);
    bridge.borrow_mut().set_history(scrub_history());
    bridge.borrow_mut().set_collector_now(Some(61_000.0)); // pinned: the edge doesn't depend on wall time
    bridge.borrow_mut().apply(&ui, scrub_frame(60_000, TWO_PM - 1_000));
    bridge.borrow_mut().apply(&ui, scrub_frame(61_000, TWO_PM));
    bridge.borrow_mut().scrub(&ui, 0.5);
    let built = bridge.borrow().scrub_builds();
    for _ in 0..20 {
        bridge.borrow_mut().follow_scrub(&ui);
    }
    assert_eq!(bridge.borrow().scrub_builds(), built, "same sample under the pointer: no rebuild");
    bridge.borrow_mut().scrub(&ui, 0.6);
    assert!(bridge.borrow().scrub_builds() > built, "another moment: the card is rebuilt");
}

#[test]
fn the_card_follows_an_edge_that_advances_under_a_still_pointer() {
    use slint::Model;
    let (ui, controls, bridge) = bridged();
    ui.global::<GraphClock>().set_smooth(true);
    controls.set_page(Page::Performance);
    bridge.borrow_mut().set_history(scrub_history());
    bridge.borrow_mut().set_collector_now(Some(61_000.0));
    bridge.borrow_mut().apply(&ui, scrub_frame(60_000, TWO_PM - 1_000));
    bridge.borrow_mut().apply(&ui, scrub_frame(61_000, TWO_PM));
    bridge.borrow_mut().scrub(&ui, 1.0);
    assert_eq!(ui.get_scrub_title(), "Top apps at 13:59:59", "the edge trails the newest sample");
    let built = bridge.borrow().scrub_builds();
    // A second later, no new frame: the view has scrolled on and the timer asks the card to follow.
    bridge.borrow_mut().set_collector_now(Some(62_000.0));
    bridge.borrow_mut().follow_scrub(&ui);
    assert_eq!(ui.get_scrub_title(), "Top apps at 14:00:00", "the moment now under the pointer");
    assert_eq!(ui.get_scrub_rows().row_data(0).unwrap().value, "61.0%");
    assert_eq!(bridge.borrow().scrub_builds(), built + 1);
}

// ---- Plan 3c Task 5: the Smooth graphs switch, hidden and minimised ----

#[test]
fn the_smooth_graphs_switch_applies_at_once_and_is_saved() {
    let dir = Scratch::new("smooth");
    let path = dir.0.join("settings.json");
    let (ui, _controls, _settings) = settings_wired(&dir, Settings::default(), false);
    let clock = ui.global::<GraphClock>();
    assert_eq!(switch(&ui, "Smooth graphs").accessible_checked(), Some(true));
    switch(&ui, "Smooth graphs").invoke_accessible_default_action();
    assert!(!clock.get_smooth() && !clock.get_live(), "graphs stop scrolling at once");
    assert_eq!(switch(&ui, "Smooth graphs").accessible_checked(), Some(false));
    saved(&path, "smooth graphs off", |s| !s.smooth_graphs);
}

/// PF3: switching smoothing off mid-run puts the newest sample on the edge at once and keeps it
/// there when it comes back on before the next frame: no jump back.
#[test]
fn switching_smooth_graphs_off_rests_on_the_newest_sample_without_a_jump() {
    let dir = Scratch::new("smooth-rest");
    let (ui, _controls, _settings) = settings_wired(&dir, Settings::default(), false);
    negotium::smooth::install(&ui, mocked_clock);
    let clock = ui.global::<GraphClock>();
    ui.set_page(1); // the graph timer runs only on pages with graphs
    ui.invoke_scroll_from(-1_000.0, 1.0);
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO);
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(200));
    assert!(clock.get_offset_ms() < 0.0 && clock.get_offset_ms() > -1_000.0, "a run under way: {}", clock.get_offset_ms());
    ui.set_page(3);
    switch(&ui, "Smooth graphs").invoke_accessible_default_action();
    assert_eq!(clock.get_offset_ms(), 0.0, "off: the newest sample on the edge at once");
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO); // change handlers run
    switch(&ui, "Smooth graphs").invoke_accessible_default_action();
    assert!(clock.get_live());
    ui.set_page(1);
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(200));
    assert_eq!((clock.get_offset_ms(), clock.get_speed()), (0.0, 0.0), "on again: still on the newest sample");
}

#[test]
fn graphs_scroll_only_while_the_window_is_on_screen() {
    let ui = window();
    let clock = ui.global::<GraphClock>();
    negotium::smooth::set_shown(&ui, false, false);
    assert!(!clock.get_live(), "hidden to the tray");
    negotium::smooth::set_shown(&ui, true, true);
    assert!(!clock.get_live(), "minimised");
    negotium::smooth::set_shown(&ui, true, false);
    assert!(clock.get_live());
}
/// Every way main.rs hears of the window leaving or coming back (negotium::smooth::OnScreen):
/// graphs scroll only while it is shown and not minimised, whatever the order, and the worker's
/// pace (Controls::visible) follows hide and show from the same calls.
#[test]
fn every_way_the_window_leaves_and_comes_back_sets_whether_graphs_scroll() {
    use negotium::smooth::OnScreen;
    let ui = window();
    let clock = ui.global::<GraphClock>();
    let controls = Arc::new(Controls::default());
    let on_screen = OnScreen::new(controls.clone(), &ui);
    // Started minimised (a shortcut set to "Minimized"): read once the hooks go in.
    assert!(on_screen.minimise(true), "a change");
    on_screen.apply();
    assert!(!clock.get_live(), "started minimised");
    // Restored from the taskbar (WM_SIZE), then the WM_SIZEs of a resize: only a change counts.
    assert!(on_screen.minimise(false));
    assert!(!on_screen.minimise(false), "a resize repeats WM_SIZE: no change");
    on_screen.apply();
    assert!(clock.get_live(), "restored");
    // Minimised, then closed to the tray from the taskbar while minimised.
    on_screen.minimise(true);
    on_screen.apply();
    assert!(!clock.get_live(), "minimised");
    on_screen.hide();
    assert!(!controls.visible(), "the worker slows to the tray pace");
    assert!(!clock.get_live(), "hidden to the tray");
    // A WM_SIZE while hidden does not bring motion back.
    on_screen.minimise(false);
    on_screen.apply();
    assert!(!clock.get_live(), "still hidden");
    // Tray Open: shown (and restored by platform::bring_to_front).
    on_screen.show();
    assert!(controls.visible());
    assert!(clock.get_live(), "tray Open");
    // Minimised again, then a second launch activates it. Until the restore's WM_SIZE says so,
    // the window is not assumed restored (here there is no native window to ask).
    on_screen.minimise(true);
    on_screen.apply();
    on_screen.show();
    assert!(!clock.get_live(), "a restore that did not happen leaves graphs still");
    on_screen.minimise(false);
    on_screen.apply();
    assert!(clock.get_live(), "second-instance activation, restored");
    // Closed to the tray, then a second launch while hidden.
    on_screen.hide();
    assert!(!clock.get_live());
    on_screen.show();
    assert!(clock.get_live());
}

/// WM_SIZE, as the window hook hands it over: repeats are dropped, a change is applied from the
/// event loop (never inside the window procedure).
#[test]
fn a_size_message_applies_a_change_once_from_the_event_loop() {
    use negotium::smooth::OnScreen;
    let ui = window();
    let clock = ui.global::<GraphClock>();
    let on_screen = OnScreen::new(Arc::new(Controls::default()), &ui);
    assert!(!on_screen.on_size(false), "restored, as recorded: nothing to do");
    assert!(on_screen.on_size(true), "minimised: a change");
    assert!(clock.get_live(), "not applied inside the window procedure");
    assert!(!on_screen.on_size(true), "a repeat is dropped");
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO);
    assert!(!clock.get_live(), "applied from the event loop");
    assert!(on_screen.on_size(false));
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO);
    assert!(clock.get_live(), "restored");
}

/// Hidden or minimised, the graph timer stops: nothing is redrawn for motion.
#[test]
fn the_graph_timer_stands_still_while_the_window_is_hidden_or_minimised() {
    let ui = shown_window();
    negotium::smooth::install(&ui, mocked_clock);
    let clock = ui.global::<GraphClock>();
    for (visible, minimised) in [(false, false), (true, true)] {
        negotium::smooth::set_shown(&ui, true, false);
        ui.invoke_scroll_from(-5_000.0, 1.0);
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO);
        let (now, offset) = (clock.get_now(), clock.get_offset_ms());
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(500)); // a tick on the Overview (~400 ms)
        assert!(clock.get_now() > now && clock.get_offset_ms() > offset, "on screen, the timer runs");
        negotium::smooth::set_shown(&ui, visible, minimised);
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO);
        let now = clock.get_now();
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(1_000));
        assert_eq!(clock.get_now(), now, "visible {visible}, minimised {minimised}: the timer doesn't run");
        assert_eq!(clock.get_offset_ms(), 0.0);
    }
}

// ---- Plan 3c Task 7: Show CPU in tray icon ----

#[test]
fn the_tray_cpu_switch_saves_and_tells_the_worker_and_the_tray() {
    let dir = Scratch::new("traycpu");
    let path = dir.0.join("settings.json");
    let (ui, controls, _settings) = settings_wired(&dir, Settings::default(), false);
    controls.set_tray_cpu(true);
    assert_eq!(switch(&ui, "Show CPU in tray icon").accessible_checked(), Some(true));
    switch(&ui, "Show CPU in tray icon").invoke_accessible_default_action();
    assert!(!controls.tray_cpu(), "the worker stops sending samples");
    assert_eq!(TRAY_TOLD.with(|t| t.borrow().clone()), [false], "the tray shows the app icon again");
    assert_eq!(switch(&ui, "Show CPU in tray icon").accessible_checked(), Some(false));
    saved(&path, "tray cpu off", |s| !s.tray_cpu);
}

// ---- Top strip and meters ----

#[test]
fn every_meter_is_as_wide_as_the_others_whatever_its_text() {
    let ui = shown_window();
    let widths = || ["CPU", "Memory", "Disk", "GPU", "Network"].map(|l| rect(&ui, &format!("{l} meter")).2);
    let even = widths();
    let mut meters: Vec<MeterData> = ui.get_meters().iter().collect();
    meters[4].value = "1234.5 Mb/s".into();
    meters[1].sub = "43.6 / 95.2 GB".into();
    ui.set_meters(ModelRc::new(VecModel::from(meters)));
    let after = widths();
    assert_eq!(after, even, "a longer value does not widen its card");
    assert!(after.iter().all(|w| (w - after[0]).abs() < 1.0), "the cards split the row evenly: {after:?}");
}

fn tile(i: usize, weight: f32) -> negotium::TileData {
    negotium::TileData { id: i as i32, name: format!("App {i}").into(), value: "1%".into(), weight, hung: false, leaving: false }
}

fn tile_width(ui: &MainWindow, i: usize) -> f32 {
    rect(ui, &format!("App {i} tile")).2
}

/// Lets new tiles be born (their one-shot Timer starts when the tile is laid out, then fires),
/// then advances `ms`.
fn settle(ui: &MainWindow, ms: u64) {
    let n = ui.get_tiles().row_count();
    for i in 0..n {
        tile_width(ui, i);
    }
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO);
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(1));
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(ms));
}

/// A frame lands, as the bridge applies one: the graphs start their scroll run (crate::smooth),
/// which is the clock the tiles glide by.
fn land(ui: &MainWindow) {
    ui.invoke_scroll_from(-1_250.0, 1.0);
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO);
}

/// Advances `ms` in 10 ms steps, returning GraphClock.now and the tile's width after each.
fn steps(ui: &MainWindow, ms: u64, tile: usize) -> Vec<(i64, f32)> {
    (0..ms / 10).map(|_| {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(10));
        (ui.global::<GraphClock>().get_now(), tile_width(ui, tile))
    }).collect()
}

#[test]
fn tiles_glide_on_the_graph_clock_when_smooth_and_jump_when_not() {
    // The bridge updates tiles in place (sync_model), so their widths glide between frames.
    let ui = shown_window();
    negotium::smooth::install(&ui, mocked_clock); // the graph clock (Bridge::install does this in the app)
    let model = std::rc::Rc::new(VecModel::from(vec![tile(0, 1.0), tile(1, 1.0)]));
    ui.set_tiles(ModelRc::from(model.clone()));
    settle(&ui, 1000);
    let start = tile_width(&ui, 0);
    model.set_row_data(0, tile(0, 3.0));
    land(&ui);
    assert!((tile_width(&ui, 0) - start).abs() < 1.0, "the frame lands with the tile where it was");
    let seen = steps(&ui, 1000, 0);
    // It moves only when the graph clock does (the graphs' redraws), never on its own.
    let mut last = (ui.global::<GraphClock>().get_anchor(), start);
    for &(now, w) in &seen {
        assert!(now != last.0 || (w - last.1).abs() < 0.01, "the tile moved with the graph clock still: {seen:?}");
        last = (now, w);
    }
    let end = seen.last().unwrap().1;
    assert!(seen.iter().any(|&(_, w)| start + 1.0 < w && w < end - 1.0), "it passes part-way: {seen:?}");
    assert!(end > start * 1.3, "and grows to its new width: {start} to {end}");

    ui.set_settings_smooth_graphs(false);
    model.set_row_data(0, tile(0, 1.0));
    land(&ui);
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    assert!((tile_width(&ui, 0) - start).abs() < 1.0, "without smoothing the tile jumps straight back");
}

/// A run that ends before the glide would (the view reaches the newest sample), or never starts
/// (the view rests on it), leaves the tiles on their targets, not part-way.
#[test]
fn tiles_land_on_their_targets_when_the_graph_clock_stops_early() {
    let ui = shown_window();
    negotium::smooth::install(&ui, mocked_clock);
    let model = std::rc::Rc::new(VecModel::from(vec![tile(0, 1.0), tile(1, 1.0)]));
    ui.set_tiles(ModelRc::from(model.clone()));
    settle(&ui, 1000);
    let start = tile_width(&ui, 0);
    // Smoothing off for a moment shows where weight 3 puts it.
    ui.set_settings_smooth_graphs(false);
    model.set_row_data(0, tile(0, 3.0));
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO);
    let target = tile_width(&ui, 0);
    model.set_row_data(0, tile(0, 1.0));
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO);
    ui.set_settings_smooth_graphs(true);
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO);
    assert!((tile_width(&ui, 0) - start).abs() < 1.0);
    // A 100 ms run: the clock stops well before 450 ms.
    model.set_row_data(0, tile(0, 3.0));
    ui.invoke_scroll_from(-100.0, 1.0);
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO);
    steps(&ui, 1000, 0);
    assert_eq!(ui.global::<GraphClock>().get_offset_ms(), 0.0, "the run is over");
    assert!((tile_width(&ui, 0) - target).abs() < 1.0, "the tile is on its target: {} vs {target}", tile_width(&ui, 0));
    // A frame that rests the view (no run): the tile jumps with the frame.
    model.set_row_data(0, tile(0, 1.0));
    ui.invoke_scroll_from(0.0, 0.0);
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO);
    assert!((tile_width(&ui, 0) - start).abs() < 1.0, "with the view resting the tile jumps");
}

/// Hidden to the tray or minimised (GraphClock.shown off), nothing glides: the tiles jump, as the
/// graphs rest.
#[test]
fn tiles_jump_while_the_window_is_not_on_screen() {
    let ui = shown_window();
    negotium::smooth::install(&ui, mocked_clock);
    let model = std::rc::Rc::new(VecModel::from(vec![tile(0, 1.0), tile(1, 1.0)]));
    ui.set_tiles(ModelRc::from(model.clone()));
    settle(&ui, 1000);
    ui.global::<GraphClock>().set_shown(false);
    model.set_row_data(0, tile(0, 3.0));
    land(&ui);
    let early = tile_width(&ui, 0);
    steps(&ui, 1000, 0);
    let end = tile_width(&ui, 0);
    assert!((early - end).abs() < 1.0, "the tile is at its new width at once: {early} then {end}");
    // Shown again, the tile stays put and then glides again.
    ui.global::<GraphClock>().set_shown(true);
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    assert!((tile_width(&ui, 0) - end).abs() < 1.0, "shown again, it stays where it was");
    model.set_row_data(0, tile(0, 1.0));
    land(&ui);
    let seen = steps(&ui, 1000, 0);
    let last = seen.last().unwrap().1;
    assert!(seen.iter().any(|&(_, w)| last + 1.0 < w && w < end - 1.0), "and glides again: {seen:?}");
}

fn ids_of(model: &VecModel<negotium::TileData>) -> Vec<(i32, bool)> {
    model.iter().map(|t| (t.id, t.leaving)).collect()
}

#[test]
fn tiles_keep_their_place_a_leaver_shrinks_away_and_a_newcomer_slots_in_by_rank() {
    use negotium::bridge::sync_tiles;
    let _ui = window(); // initialises the backend
    let model = VecModel::from(vec![tile(1, 5.0), tile(2, 4.0), tile(3, 3.0)]);
    // 2 leaves, 4 arrives ranked second.
    sync_tiles(&model, vec![tile(1, 5.0), tile(4, 4.5), tile(3, 3.0)], true);
    assert_eq!(ids_of(&model), [(1, false), (4, false), (2, true), (3, false)]);
    assert_eq!(model.row_data(2).unwrap().weight, 0.0, "the leaver shrinks to nothing");
    // Next frame the leaver is gone.
    sync_tiles(&model, vec![tile(1, 5.0), tile(4, 4.5), tile(3, 3.0)], true);
    assert_eq!(ids_of(&model), [(1, false), (4, false), (3, false)]);
    // Without smoothing: replaced in order, no leavers.
    sync_tiles(&model, vec![tile(5, 9.0), tile(1, 5.0)], false);
    assert_eq!(ids_of(&model), [(5, false), (1, false)]);
}

#[test]
fn a_new_tile_grows_in_from_nothing_when_smooth() {
    let ui = shown_window();
    negotium::smooth::install(&ui, mocked_clock); // the glide's clock (Bridge::install does this in the app)
    let model = std::rc::Rc::new(VecModel::from(vec![tile(0, 1.0)]));
    ui.set_tiles(ModelRc::from(model.clone()));
    settle(&ui, 1000);
    model.push(tile(1, 1.0));
    land(&ui);
    settle(&ui, 50);
    let early = tile_width(&ui, 1);
    steps(&ui, 1000, 1);
    let settled = tile_width(&ui, 1);
    assert!(early < settled * 0.6, "it starts narrow: {early} vs {settled}");
    let first = tile_width(&ui, 0);
    assert!((settled - first).abs() <= 2.0, "and ends as wide as its equal: {settled} vs {first}");
}

// ---- App list redesign Task 6: the card list ----

#[test]
fn the_sorted_header_is_checked_and_clicking_another_sorts_by_it() {
    let dir = Scratch::new("sortheader");
    let (ui, _controls, _settings) = settings_wired(&dir, Settings::default(), false);
    ui.set_page(0);
    use i_slint_backend_testing::ElementRoot;
    // Found by its "Sort by <column>" prefix: the sorted one goes on to say which way.
    let header = |label: &str| {
        let (exact, more) = (label.to_string(), format!("{label}, "));
        ui.root_element().query_descendants()
            .match_predicate(move |e| e.accessible_label().is_some_and(|l| l == exact.as_str() || l.starts_with(&more)))
            .find_first()
            .unwrap_or_else(|| panic!("missing {label}"))
    };
    let said = |label: &str| header(label).accessible_label().unwrap().to_string();
    for label in ["Sort by Name", "Sort by CPU", "Sort by Memory", "Sort by Disk", "Sort by GPU", "Sort by Network"] {
        assert_eq!(header(label).accessible_role(), Some(i_slint_backend_testing::AccessibleRole::Button), "{label}");
        assert_eq!(header(label).accessible_checkable(), Some(true), "{label}");
    }
    assert_eq!(header("Sort by CPU").accessible_checked(), Some(true), "sorted by CPU at first");
    assert_eq!(said("Sort by CPU"), "Sort by CPU, sorted descending", "the sorted column says which way");
    assert_eq!(said("Sort by Memory"), "Sort by Memory");
    assert_eq!(header("Sort by Memory").accessible_checked(), Some(false));
    header("Sort by Memory").invoke_accessible_default_action();
    assert_eq!(ui.get_sort_column(), 1);
    assert_eq!(header("Sort by Memory").accessible_checked(), Some(true));
    assert_eq!(header("Sort by CPU").accessible_checked(), Some(false));
    // A real click on the Name header sorts by name.
    let (x, y) = centre(&header("Sort by Name"));
    press(&ui, x, y, slint::platform::PointerEventButton::Left);
    assert_eq!(ui.get_sort_column(), -1);
    assert_eq!(header("Sort by Name").accessible_checked(), Some(true));
    assert_eq!(said("Sort by Name"), "Sort by Name, sorted ascending");
    assert_eq!(said("Sort by Memory"), "Sort by Memory");
}

#[test]
fn the_end_task_icon_shows_on_hover_and_ends_that_app() {
    let ui = shown_window();
    let actions: Actions = Default::default();
    let a = actions.clone();
    ui.on_row_action(move |name, id| a.borrow_mut().push((name.to_string(), id)));
    let focus: FocusLog = Default::default();
    let f = focus.clone();
    ui.on_row_key_focus(move |id, has| f.borrow_mut().push((id, has)));
    let end = || El::find_by_accessible_label(&ui, "End task Browser").next();
    assert!(end().is_none(), "hidden until the card is hovered");
    let (rx, ry) = centre(&El::find_by_accessible_label(&ui, BROWSER_ROW).next().expect("browser card"));
    move_pointer(&ui, rx, ry);
    let button = end().expect("shown while the card is hovered");
    assert_eq!(button.accessible_role(), Some(i_slint_backend_testing::AccessibleRole::Button));
    let size = button.size();
    assert!((size.width - 22.0).abs() <= 1.0 && (size.height - 22.0).abs() <= 1.0, "a 22 px square: {size:?}");
    assert!(El::find_by_accessible_label(&ui, "End task Editor").next().is_none(), "only on the hovered card");
    let (bx, by) = centre(&button);
    move_pointer(&ui, bx, by);
    press(&ui, bx, by, slint::platform::PointerEventButton::Left);
    slint::platform::update_timers_and_animations();
    assert_eq!(*actions.borrow(), [("end-task".to_string(), 9)], "ends that app");
    assert_eq!(focus.borrow().last(), Some(&(9, true)), "its focus freezes the list like the old button: {:?}", focus.borrow());
    // The pointer leaves, the button keeps keyboard focus: it stays.
    move_pointer(&ui, 5.0, 5.0);
    assert!(end().is_some(), "shown while it holds focus");
}

/// The spec: the × shows while the card is hovered, or holds keyboard focus; the keyboard-current
/// card shows it too, so it can be reached and seen without the mouse.
#[test]
fn the_end_task_icon_shows_on_the_keyboard_current_card() {
    use slint::platform::Key;
    let (ui, controls, _bridge, _) = wired_list(vec![row_view(7, "Editor", ""), row_view(9, "Browser", "")]);
    let end = |name: &str| El::find_by_accessible_label(&ui, &format!("End task {name}")).next();
    assert!(end("Editor").is_none() && end("Browser").is_none(), "hidden with no hover and no focus");
    assert!(tab_until(&ui, || controls.frozen()), "Tab reaches the list");
    assert_eq!(ui.get_current_id(), 7);
    assert!(end("Editor").is_some(), "shown on the current card");
    assert!(end("Browser").is_none(), "only there");
    press_key(&ui, Key::DownArrow);
    assert!(end("Editor").is_none() && end("Browser").is_some(), "it follows the current card");
    press_key(&ui, Key::Escape);
    assert!(end("Browser").is_none(), "the list lost focus: no current card");
}

#[test]
fn cards_sections_and_process_rows_have_their_heights() {
    use negotium::view::{RowKind, SECTION_APPS};
    let mut browser = row_view(9, "Browser", "");
    browser.expandable = true;
    let proc = |id: i32, last: bool| RowView { id, parent: 9, depth: 1, kind: RowKind::Proc, name: format!("proc{id}"), actionable: true, last_in_card: last, ..Default::default() };
    let (ui, _controls, bridge, _) = wired_list(vec![heading(SECTION_APPS, "Apps"), browser, proc(20, false), proc(21, true), row_view(11, "Tool", "")]);
    bridge.borrow_mut().toggle(9);
    let section = rect(&ui, "Apps (2) section, expanded");
    let card = rect(&ui, "Browser, CPU 1.0%, memory ");
    let first = rect(&ui, "proc20, CPU , memory ");
    let last = rect(&ui, "proc21, CPU , memory ");
    let tool = rect(&ui, "Tool, CPU 1.0%, memory ");
    for (what, r, h) in [("section", section, 32.0), ("card", card, 50.0), ("process", first, 30.0), ("last process", last, 30.0), ("card", tool, 50.0)] {
        assert!((r.3 - h).abs() <= 1.0, "{what} is {h} px tall: {r:?}");
    }
    for (a, b) in [(section, card), (card, first), (first, last), (last, tool)] {
        assert!((a.1 + a.3 - b.1).abs() <= 1.0, "rows follow each other with no gap: {a:?} then {b:?}");
    }
}

#[test]
fn a_section_heading_click_collapses_it() {
    use negotium::view::{SECTION_APPS, SECTION_BACKGROUND};
    let (ui, _controls, bridge, _) = wired_list(vec![
        heading(SECTION_APPS, "Apps"), row_view(1, "Editor", ""),
        heading(SECTION_BACKGROUND, "Background processes"), row_view(2, "Updater", ""),
    ]);
    let b = bridge.clone();
    ui.on_toggle_row(move |id| b.borrow_mut().toggle(id));
    let heading = El::find_by_accessible_label(&ui, "Apps (1) section, expanded").next().expect("an open heading, with its count");
    let (x, y) = centre(&heading);
    move_pointer(&ui, x, y);
    press(&ui, x, y, slint::platform::PointerEventButton::Left);
    assert_eq!(shown_ids(&ui), [SECTION_APPS, SECTION_BACKGROUND, 2], "its cards go");
    assert!(El::find_by_accessible_label(&ui, "Apps (1) section, collapsed").next().is_some());
    assert!(El::find_by_accessible_label(&ui, "Background processes (1) section, expanded").next().is_some());
    // The screen reader's default action toggles it too.
    El::find_by_accessible_label(&ui, "Apps (1) section, collapsed").next().unwrap().invoke_accessible_default_action();
    assert_eq!(shown_ids(&ui), [SECTION_APPS, 1, SECTION_BACKGROUND, 2]);
}

#[test]
fn a_heading_without_a_count_is_read_without_one() {
    use negotium::view::SECTION_WINDOWS;
    let mut group = row_view(5, "Windows", "");
    (group.windows, group.actionable) = (true, false);
    let (ui, _controls, _bridge, _) = wired_list(vec![heading(SECTION_WINDOWS, "Windows processes"), group]);
    assert!(El::find_by_accessible_label(&ui, "Windows processes section, expanded").next().is_some());
}

#[test]
fn an_icon_row_draws_its_image_and_one_without_draws_the_glyph() {
    use slint::Model;
    let ui = window();
    let rows = ui.get_rows();
    let mut browser = rows.row_data(1).unwrap();
    browser.has_icon = true;
    browser.icon = slint::Image::from_rgba8(slint::SharedPixelBuffer::new(32, 32));
    rows.set_row_data(1, browser);
    let mut windows = rows.row_data(2).unwrap();
    windows.windows = true;
    rows.set_row_data(2, windows);
    ui.window().set_size(PhysicalSize::new(1000, 720));
    ui.show().unwrap();
    let row = |label: &str| El::find_by_accessible_label(&ui, label).next().unwrap_or_else(|| panic!("missing {label}"));
    let has = |label: &str, id: &str| row(label).query_descendants().match_id(id).find_first().is_some();
    assert!(has(BROWSER_ROW, "RowIcon::icon-image"), "its icon");
    assert!(!has(BROWSER_ROW, "RowIcon::generic-glyph"));
    let image = row(BROWSER_ROW).query_descendants().match_id("RowIcon::icon-image").find_first().unwrap();
    assert!((image.size().width - 24.0).abs() <= 0.5, "a 24 px icon: {:?}", image.size());
    let editor = "Editor, Not responding · 9s, CPU 1.0%, memory 1.0 MB";
    assert!(!has(editor, "RowIcon::icon-image"), "no image without an icon");
    assert!(has(editor, "RowIcon::generic-glyph"), "the generic program glyph instead");
    let group = "Windows, CPU 3.0%, memory 3.0 MB";
    assert!(has(group, "RowIcon::windows-glyph") && !has(group, "RowIcon::generic-glyph"), "the Windows group draws the Windows glyph");
}

/// Ruling (Task 6): a dead app can't be opened, but a held open card can still be closed.
#[test]
fn an_exited_row_never_expands_but_can_collapse() {
    let (ui, _controls, bridge) = bridged();
    let card = |id: i32, name: &str| RowView { expandable: true, ..row_view(id, name, "") };
    let child = |id: i32, parent: i32| RowView { id, parent, depth: 1, kind: negotium::view::RowKind::Proc, name: format!("proc{id}"), actionable: true, ..Default::default() };
    let mut b = bridge.borrow_mut();
    b.apply(&ui, frame(vec![card(1, "Editor"), card(2, "Browser"), child(20, 2), child(21, 2)]));
    b.toggle(2);
    assert_eq!(shown_ids(&ui), [1, 2, 20, 21]);
    b.set_hover(true);
    b.apply(&ui, frame(vec![]));
    assert_eq!(shown_ids(&ui), [1, 2, 20, 21], "held in place, exited");
    b.toggle(1);
    assert!(!shown_expanded(&ui, 1), "an exited app does not expand");
    assert_eq!(shown_ids(&ui), [1, 2, 20, 21]);
    b.toggle(2);
    assert!(!shown_expanded(&ui, 2), "a held open card still collapses");
    assert_eq!(shown_ids(&ui), [1, 2]);
}

/// The bars travel as a model, which compares by identity: an unchanged row must keep its bars
/// model, or every card would be rewritten every frame.
#[test]
fn a_rows_bars_are_unchanged_by_an_identical_frame() {
    use slint::Model;
    let (ui, _controls, bridge) = bridged();
    let mut b = bridge.borrow_mut();
    let rows = vec![RowView { bars: [0.5, 0.25, 0.0, 1.0, 0.1], ..row_view(1, "Editor", "") }];
    b.apply(&ui, frame(rows.clone()));
    let before = ui.get_rows().row_data(0).unwrap();
    assert_eq!(before.bars.iter().collect::<Vec<_>>(), [0.5, 0.25, 0.0, 1.0, 0.1]);
    assert_eq!(before.kind, 1, "an app card");
    b.apply(&ui, frame(rows));
    assert_eq!(ui.get_rows().row_data(0).unwrap(), before);
}

#[test]
fn a_focused_end_task_keeps_the_list_frozen_until_its_card_hangs() {
    let (ui, controls, bridge) = bridged();
    let mut b = bridge.borrow_mut();
    b.apply(&ui, frame(vec![row_view(1, "Editor", ""), row_view(2, "Browser", "")]));
    b.set_hover(true);
    b.set_key_focus(2, true); // Browser's End task, reached while the card is hovered
    b.set_hover(false);
    b.apply(&ui, frame(vec![row_view(2, "Browser", ""), row_view(1, "Editor", "")]));
    assert!(controls.frozen(), "the button stays while it holds focus, and so does the freeze");
    assert_eq!(shown_ids(&ui), [1, 2]);
    // Browser hangs: its End task gives way to Force close, which never had focus.
    b.apply(&ui, frame(vec![row_view(2, "Browser", HUNG), row_view(1, "Editor", "")]));
    assert!(!controls.frozen(), "a vanished End task must not hold the list frozen");
}

// ---- Task 6 fix round 1 ----

/// The × lives in a row built only while in view: scrolled away, it is destroyed without a
/// focus-out, and the list must not stay frozen on it once the pointer leaves.
#[test]
fn a_focused_end_task_scrolled_out_of_view_does_not_hold_the_list_frozen() {
    let rows: Vec<RowView> = (1..=40).map(|i| row_view(i, &format!("App{i}"), "")).collect();
    let (ui, controls, _bridge, actions) = wired_list(rows);
    let card = El::find_by_accessible_label(&ui, "App1, CPU 1.0%, memory ").next().expect("the first card");
    let (x, y) = centre(&card);
    move_pointer(&ui, x, y);
    let end = El::find_by_accessible_label(&ui, "End task App1").next().expect("its End task on hover");
    let (bx, by) = centre(&end);
    move_pointer(&ui, bx, by);
    press(&ui, bx, by, slint::platform::PointerEventButton::Left);
    slint::platform::update_timers_and_animations();
    slint::platform::update_timers_and_animations();
    // Ending it failed (access denied): the app stays, its × keeps focus.
    assert_eq!(*actions.borrow(), [("end-task".to_string(), 1)]);
    assert!(controls.frozen());
    ui.window().dispatch_event(WindowEvent::PointerScrolled { position: LogicalPosition::new(x, y), delta_x: 0.0, delta_y: -1500.0 });
    slint::platform::update_timers_and_animations();
    assert!(ui.get_list_scroll_y() < -500.0, "the card scrolled out of view");
    assert!(El::find_by_accessible_label(&ui, "End task App1").next().is_none(), "its × is gone with it");
    move_pointer(&ui, 5.0, 5.0);
    for _ in 0..3 {
        slint::platform::update_timers_and_animations();
    }
    assert!(!controls.frozen(), "nothing focused is left to hold the list");
}

#[test]
fn a_process_row_shows_its_pid_after_its_name() {
    use negotium::view::RowKind;
    let mut browser = row_view(9, "Browser", "");
    browser.expandable = true;
    let proc = RowView { id: 20, parent: 9, depth: 1, kind: RowKind::Proc, name: "helper.exe".into(), pid: "4242".into(), actionable: true, last_in_card: true, ..Default::default() };
    let (ui, _controls, bridge, _) = wired_list(vec![browser, proc]);
    bridge.borrow_mut().toggle(9);
    let has_text = |row: &str, text: &str| {
        let text = text.to_string();
        El::find_by_accessible_label(&ui, row).next().unwrap_or_else(|| panic!("missing {row}"))
            .query_descendants().match_predicate(move |e| e.accessible_label().is_some_and(|l| l == text.as_str())).find_first().is_some()
    };
    assert!(has_text("helper.exe, CPU , memory ", "4242"), "the PID beside the process name");
    assert!(!has_text("Browser, CPU 1.0%, memory ", "4242"));
}

/// What a fake [`negotium::update::UpdateIo`] was asked: checks waiting for an answer, downloads
/// waiting (with their release), installers started, exits, log lines.
#[derive(Clone, Default)]
struct FakeUpdates {
    checks: Rc<RefCell<Vec<negotium::update::Checked>>>,
    downloads: Rc<RefCell<Vec<(negotium::update::Release, negotium::update::Downloaded)>>>,
    installers: Rc<RefCell<Vec<std::path::PathBuf>>>,
    /// The installer start fails with this (none: it starts).
    installer_fails: Rc<RefCell<Option<negotium::update::UpdateError>>>,
    exits: Rc<std::cell::Cell<usize>>,
    log: Rc<RefCell<Vec<String>>>,
}

impl FakeUpdates {
    fn wire(ui: &MainWindow, installed: bool) -> (Rc<negotium::update::Updater>, FakeUpdates) {
        let f = FakeUpdates::default();
        let io = negotium::update::UpdateIo {
            installed,
            check: Box::new({
                let checks = f.checks.clone();
                move |done| checks.borrow_mut().push(done)
            }),
            download: Box::new({
                let downloads = f.downloads.clone();
                move |release, done| downloads.borrow_mut().push((release, done))
            }),
            // The injected launcher: answers at once (`installer_fails`).
            install: Box::new({
                let (installers, fails) = (f.installers.clone(), f.installer_fails.clone());
                move |job, done| {
                    installers.borrow_mut().push(job.msi.clone());
                    done(fails.borrow().clone().map_or(Ok(negotium::update::HeldInstaller(None)), Err));
                }
            }),
            exit: Box::new({
                let exits = f.exits.clone();
                move || exits.set(exits.get() + 1)
            }),
            log: Box::new({
                let log = f.log.clone();
                move |line| log.borrow_mut().push(line.to_string())
            }),
        };
        (negotium::update::Updater::install(ui, io), f)
    }

    /// The check asked for answers `result`.
    fn answer(&self, result: Result<Option<negotium::update::Release>, negotium::update::UpdateError>) {
        let done = self.checks.borrow_mut().pop().expect("a check is running");
        done(result);
    }
}

fn release_v(version: (u64, u64, u64)) -> negotium::update::Release {
    negotium::update::Release {
        version,
        msi_url: "https://github.com/MyndPhreak/Negotium/releases/download/v9.9.9/Negotium-9.9.9.msi".into(),
        page_url: "https://github.com/MyndPhreak/Negotium/releases/tag/v9.9.9".into(),
        digest: negotium::update::AssetDigest::Absent,
    }
}

fn button(ui: &MainWindow, label: &str) -> Option<El> {
    El::find_by_accessible_label(ui, label).find(|e| e.accessible_role() == Some(AccessibleRole::Button))
}

#[test]
fn the_update_banner_offers_the_version() {
    let ui = shown_window();
    let (updater, fake) = FakeUpdates::wire(&ui, true);
    updater.startup(true);
    assert_eq!(fake.checks.borrow().len(), 1, "the installed copy checks at startup");
    assert!(button(&ui, "Update").is_none(), "no banner before an answer");
    fake.answer(Ok(Some(release_v((9, 9, 9)))));
    assert_eq!(ui.get_update_banner(), "Update to v9.9.9");
    assert!(text(&ui, "Update to v9.9.9").is_some(), "the banner says it");
    assert_eq!(button(&ui, "Update").and_then(|b| b.accessible_enabled()), Some(true));
    // It shows over every page, the Settings page included.
    ui.set_page(3);
    assert!(text(&ui, "Update to v9.9.9").is_some());
    button(&ui, "Dismiss").expect("Dismiss").invoke_accessible_default_action();
    assert_eq!(ui.get_update_banner(), "");
    assert!(text(&ui, "Update to v9.9.9").is_none(), "dismissed");
    assert!(fake.downloads.borrow().is_empty(), "nothing downloaded");
}

#[test]
fn the_startup_check_follows_the_switch_and_skips_a_dev_build() {
    let ui = shown_window();
    let (updater, fake) = FakeUpdates::wire(&ui, false);
    updater.startup(true);
    updater.check_now();
    assert!(fake.checks.borrow().is_empty(), "a development build never checks");
    let (updater, fake) = FakeUpdates::wire(&ui, true);
    updater.startup(false);
    assert!(fake.checks.borrow().is_empty(), "the switch is off");
    // A failed startup check is only a log line.
    updater.startup(true);
    fake.answer(Err(negotium::update::UpdateError::Status(403)));
    assert_eq!(ui.get_settings_update_status(), "");
    assert_eq!(ui.get_toast(), "");
    assert!(fake.log.borrow().iter().any(|l| l.contains("update check failed") && l.contains("403")), "{:?}", fake.log.borrow());
    // Nothing newer at startup: nothing shows.
    updater.startup(true);
    fake.answer(Ok(None));
    assert_eq!(ui.get_settings_update_status(), "");
    assert_eq!(ui.get_update_banner(), "");
}

#[test]
fn check_for_updates_says_what_it_found() {
    let ui = settings_window(1000, 2600);
    negotium::settingspage::show_install(&ui, &negotium::install::Install { exe: INSTALLED_EXE.into(), installed: true });
    let (_updater, fake) = FakeUpdates::wire(&ui, true);
    let check = || button(&ui, "Check for updates").expect("the button").invoke_accessible_default_action();

    check();
    assert_eq!(ui.get_settings_update_status(), "Checking…");
    assert!(text(&ui, "Checking…").is_some(), "under the row");
    check(); // a second click while it runs asks nothing more
    assert_eq!(fake.checks.borrow().len(), 1);
    fake.answer(Ok(None));
    assert!(text(&ui, "You're up to date").is_some());

    check();
    fake.answer(Err(negotium::update::UpdateError::Network("offline".into())));
    assert!(text(&ui, "Couldn't check for updates").is_some());
    assert!(fake.log.borrow().iter().any(|l| l.contains("offline")));

    check();
    fake.answer(Ok(Some(release_v((9, 9, 9)))));
    assert_eq!(ui.get_settings_update_status(), "", "the banner says it");
    assert_eq!(ui.get_update_banner(), "Update to v9.9.9");
}

#[test]
fn update_downloads_runs_the_installer_and_exits() {
    let ui = shown_window();
    let (updater, fake) = FakeUpdates::wire(&ui, true);
    updater.startup(true);
    fake.answer(Ok(Some(release_v((9, 9, 9)))));
    button(&ui, "Update").expect("Update").invoke_accessible_default_action();
    assert_eq!(fake.downloads.borrow().len(), 1);
    assert_eq!(fake.downloads.borrow()[0].0.version, (9, 9, 9));
    assert_eq!(ui.get_update_banner(), "Downloading v9.9.9…");
    assert_eq!(button(&ui, "Update").and_then(|b| b.accessible_enabled()), Some(false), "it waits");
    updater.accept(); // a second click doesn't download again
    updater.dismiss(); // nor can the banner go meanwhile
    assert_eq!(fake.downloads.borrow().len(), 1);
    assert_eq!(ui.get_update_banner(), "Downloading v9.9.9…");
    let msi = std::path::PathBuf::from(r"C:\Temp\Negotium-update\Negotium-9.9.9.msi");
    let (_, done) = fake.downloads.borrow_mut().pop().unwrap();
    done(Ok(msi.clone()));
    assert_eq!(*fake.installers.borrow(), [msi]);
    assert_eq!(ui.get_update_banner(), "Installing v9.9.9…");
    assert_eq!(fake.exits.get(), 0, "it stays a moment, holding the installer while msiexec opens it");
    i_slint_backend_testing::mock_elapsed_time(negotium::update::EXIT_GRACE + std::time::Duration::from_millis(10));
    assert_eq!(fake.exits.get(), 1, "then exits so the installer can replace it");
}

#[test]
fn a_failed_update_says_so_and_offers_it_again() {
    let ui = shown_window();
    let (updater, fake) = FakeUpdates::wire(&ui, true);
    updater.startup(true);
    fake.answer(Ok(Some(release_v((9, 9, 9)))));
    updater.accept();
    let (_, done) = fake.downloads.borrow_mut().pop().unwrap();
    done(Err(negotium::update::UpdateError::Download("not a Windows Installer package".into())));
    assert_eq!(fake.exits.get(), 0);
    assert!(fake.installers.borrow().is_empty(), "nothing started");
    assert!(ui.get_toast().starts_with("Couldn't update to v9.9.9"), "{}", ui.get_toast());
    assert_eq!(ui.get_update_banner(), "Update to v9.9.9");
    assert!(!ui.get_update_busy());
    // The installer won't start: the same.
    *fake.installer_fails.borrow_mut() = Some(negotium::update::UpdateError::Start("not found".into()));
    updater.accept();
    let (_, done) = fake.downloads.borrow_mut().pop().unwrap();
    done(Ok("x.msi".into()));
    assert_eq!(fake.exits.get(), 0);
    assert_eq!(ui.get_update_banner(), "Update to v9.9.9");
    assert!(fake.log.borrow().iter().any(|l| l.contains("update to v9.9.9 failed")));
}

#[test]
fn a_declined_uac_prompt_keeps_negotium_running() {
    let ui = shown_window();
    let (updater, fake) = FakeUpdates::wire(&ui, true);
    updater.startup(true);
    let digest = [7u8; 32];
    fake.answer(Ok(Some(negotium::update::Release { digest: negotium::update::AssetDigest::Sha256(digest), ..release_v((9, 9, 9)) })));
    *fake.installer_fails.borrow_mut() = Some(negotium::update::UpdateError::Cancelled);
    updater.accept();
    let (release, done) = fake.downloads.borrow_mut().pop().unwrap();
    assert_eq!(release.digest, negotium::update::AssetDigest::Sha256(digest));
    done(Ok("x.msi".into()));
    assert_eq!(*fake.installers.borrow(), [std::path::PathBuf::from("x.msi")], "msiexec was asked to start");
    assert_eq!(fake.exits.get(), 0, "declined: Negotium stays");
    assert_eq!(ui.get_toast(), "Update cancelled");
    assert_eq!(ui.get_update_banner(), "Update to v9.9.9", "offered again");
    assert!(!ui.get_update_busy());
}

#[test]
fn a_check_landing_during_the_download_changes_nothing() {
    let ui = settings_window(1000, 2600);
    negotium::settingspage::show_install(&ui, &negotium::install::Install { exe: INSTALLED_EXE.into(), installed: true });
    let (updater, fake) = FakeUpdates::wire(&ui, true);
    updater.startup(true);
    fake.answer(Ok(Some(release_v((9, 9, 9)))));
    updater.check_now(); // a manual check, still out when Update is clicked
    assert_eq!(ui.get_settings_update_status(), "Checking…");
    updater.accept();
    updater.check_now(); // ignored while updating
    fake.answer(Ok(Some(release_v((10, 0, 0)))));
    assert_eq!(ui.get_update_banner(), "Downloading v9.9.9…");
    assert_eq!(ui.get_settings_update_status(), "", "its Checking… is cleared");
    assert!(fake.checks.borrow().is_empty());
}

#[test]
fn the_startup_check_switch_is_saved() {
    let dir = Scratch::new("check-updates");
    let path = dir.0.join("settings.json");
    let (integration, fake) = FakeIntegration::new(true);
    let (ui, _, settings, _) = settings_wired_integrated(&dir, Settings::default(), false, Renderer::Software, integration);
    ui.window().set_size(PhysicalSize::new(1000, 2600));
    fake.read();
    let label = "Check for updates on startup";
    assert_eq!(switch(&ui, label).accessible_checked(), Some(true), "on by default");
    switch(&ui, label).invoke_accessible_default_action();
    assert!(!settings.borrow().check_updates);
    assert_eq!(switch(&ui, label).accessible_checked(), Some(false));
    saved(&path, "check updates off", |s| !s.check_updates);
}

#[test]
fn cpu_affinity_and_priority_modals_toggle_and_report() {
    let ui = shown_window();
    assert!(!ui.get_modal_open());
    assert!(!ui.get_affinity_visible());
    assert!(!ui.get_priority_visible());

    // Show affinity modal
    ui.set_affinity_title("Set CPU Affinity - test.exe (PID 100)".into());
    ui.set_affinity_process_name("test.exe".into());
    let sel = negotium::affinity::CoreSelection::new(0b11, 0b11, &[]);
    ui.set_affinity_groups(slint::ModelRc::new(slint::VecModel::from(sel.model())));
    ui.set_affinity_visible(true);
    assert!(ui.get_modal_open());

    // Show priority modal
    ui.set_affinity_visible(false);
    ui.set_priority_title("Set CPU Priority - test.exe (PID 100)".into());
    ui.set_priority_process_name("test.exe".into());
    ui.set_priority_selected(3); // Normal
    ui.set_priority_visible(true);
    assert!(ui.get_modal_open());
    assert_eq!(ui.get_priority_selected(), 3);

    ui.set_priority_visible(false);
    assert!(!ui.get_modal_open());
}

#[test]
fn cpu_affinity_shows_groups_and_a_tile_per_cpu() {
    let ui = shown_window();
    let sel = negotium::affinity::CoreSelection::new(
        0b0111,
        0b1111,
        &[
            negotium_collector::sysinfo::CpuGroup { name: "CCD 0".into(), mask: 0b0011, l3_bytes: Some(96 << 20) },
            negotium_collector::sysinfo::CpuGroup { name: "CCD 1".into(), mask: 0b1100, l3_bytes: Some(32 << 20) },
        ],
    );
    ui.set_affinity_process_name("test.exe".into());
    ui.set_affinity_groups(slint::ModelRc::new(slint::VecModel::from(sel.model())));
    ui.set_affinity_visible(true);

    let ccd0 = ElementHandle::find_by_accessible_label(&ui, "CCD 0").next().expect("CCD 0 group header");
    assert_eq!(ccd0.accessible_checked(), Some(true), "every CPU of CCD 0 is on");
    let ccd1 = ElementHandle::find_by_accessible_label(&ui, "CCD 1").next().expect("CCD 1 group header");
    assert_eq!(ccd1.accessible_checked(), Some(false), "CPU 3 is off");
    for (cpu, on) in [(0, true), (1, true), (2, true), (3, false)] {
        let tile = ElementHandle::find_by_accessible_label(&ui, &format!("CPU {cpu}")).next().expect("a tile per CPU");
        assert_eq!(tile.accessible_checked(), Some(on), "CPU {cpu}");
    }
}

/// `NegotiumTray` is a `SystemTrayIcon`, not a window: the testing backend gives no
/// `ComponentHandle` for it, so its menu cannot be searched. The menu is checked in the source,
/// as tests/wix.rs reads main.wxs.
#[test]
fn tray_menu_offers_end_a_window() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/ui/tray.slint");
    let src = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    assert!(src.contains("callback pick-window();"), "tray declares pick-window");
    let item = "MenuItem { title: \"End a window\u{2026}\"; activated => { root.pick-window(); } }";
    let at = src.find(item).expect("an End a window\u{2026} item that calls pick-window");
    let open = src.find("MenuItem { title: \"Open\";").expect("the Open item");
    assert!(open < at, "End a window\u{2026} comes after Open");
    let between = &src[open..at];
    assert_eq!(between.matches("MenuItem").count(), 1, "End a window\u{2026} comes right after Open");
}

/// The caption button labelled `label`, if shown.
fn caption_button(ui: &MainWindow, label: &str) -> Option<El> {
    El::find_by_accessible_label(ui, label).find(|e| e.accessible_role() == Some(AccessibleRole::Button))
}

#[test]
fn the_title_bar_is_always_drawn() {
    let ui = shown_window();
    for label in ["Minimise", "Maximise", "Close"] {
        assert!(caption_button(&ui, label).is_some(), "no {label} button");
    }
}

#[test]
fn the_settings_page_offers_no_title_bar_switch() {
    let ui = settings_window(1000, 2600);
    assert_eq!(El::find_by_accessible_label(&ui, "Use the Negotium title bar").count(), 0);
}

#[test]
fn the_title_bar_buttons_ask_for_their_commands() {
    let ui = shown_window();
    let log = Rc::new(RefCell::new(Vec::<&str>::new()));
    let l = log.clone();
    ui.on_caption_minimise(move || l.borrow_mut().push("minimise"));
    let l = log.clone();
    ui.on_caption_maximise(move || l.borrow_mut().push("maximise"));
    let l = log.clone();
    ui.on_caption_close(move || l.borrow_mut().push("close"));
    for label in ["Minimise", "Maximise", "Close"] {
        caption_button(&ui, label).unwrap_or_else(|| panic!("no {label} button")).invoke_accessible_default_action();
    }
    assert_eq!(*log.borrow(), ["minimise", "maximise", "close"]);
}

#[test]
fn a_maximised_window_offers_restore() {
    let ui = shown_window();
    ui.set_maximised(true);
    assert!(caption_button(&ui, "Maximise").is_none());
    assert!(caption_button(&ui, "Restore").is_some(), "the middle button restores");
}

#[test]
fn the_title_bar_lies_over_the_top_of_the_window() {
    let ui = shown_window();
    let close = caption_button(&ui, "Close").expect("close button");
    let (p, s) = (close.absolute_position(), close.size());
    assert_eq!((p.y, s.height), (0.0, negotium::platform::CAPTION_HEIGHT));
    assert_eq!(s.width, negotium::platform::CAPTION_BUTTON_WIDTH);
    assert_eq!(p.x + s.width, 1000.0, "close is at the right edge");
}

#[test]
fn the_slint_caption_geometry_is_the_hit_tests() {
    i_slint_backend_testing::init_no_event_loop();
    let ui = MainWindow::new().unwrap();
    let caption = ui.global::<negotium::Caption>();
    assert_eq!(caption.get_height(), negotium::platform::CAPTION_HEIGHT);
    assert_eq!(caption.get_button_width(), negotium::platform::CAPTION_BUTTON_WIDTH);
}

// ---- Automatic contrast: the palette follows the effective tint ----

type Rgba = (u8, u8, u8, u8);

/// The ten colours that follow `palette-dark`, in the order of `palette_of` (today's literals).
const LIGHT_PALETTE: [Rgba; 10] = [
    (255, 255, 255, 0xb3), (0, 0, 0, 0x0f), (255, 255, 255, 0xff), (0, 0, 0, 0x08), (0x1a, 0x1a, 0x1a, 0xff),
    (0, 0, 0, 0x99), (0xc4, 0x2b, 0x1c, 0xff), (0xfd, 0xe7, 0xe9, 0xff), (0, 0, 0, 0x0a), (0, 0, 0, 0x06),
];
const DARK_PALETTE: [Rgba; 10] = [
    (255, 255, 255, 0x0f), (255, 255, 255, 0x14), (255, 255, 255, 0x17), (0, 0, 0, 0x2e), (255, 255, 255, 0xff),
    (255, 255, 255, 0xa0), (0xff, 0x99, 0xa4, 0xff), (0x44, 0x27, 0x26, 0xff), (255, 255, 255, 0x0f), (255, 255, 255, 0x0a),
];

/// card, card-border, card-hover, card-inset, text, muted, danger-text, danger-bg, caption-hover, caption-press.
fn palette_of(ui: &MainWindow) -> [Rgba; 10] {
    let t = ui.global::<negotium::Theme>();
    [
        t.get_card(), t.get_card_border(), t.get_card_hover(), t.get_card_inset(), t.get_text(),
        t.get_muted(), t.get_danger_text(), t.get_danger_bg(), t.get_caption_hover(), t.get_caption_press(),
    ]
    .map(rgba)
}

/// The fills that keep their own pair in both palettes: accent, on-accent, danger, on-danger, caption-close.
fn fixed_fills(ui: &MainWindow) -> [Rgba; 5] {
    let t = ui.global::<negotium::Theme>();
    [t.get_accent(), t.get_on_accent(), t.get_danger(), t.get_on_danger(), t.get_caption_close()].map(rgba)
}

fn palette_dark(ui: &MainWindow) -> bool {
    ui.global::<negotium::Theme>().get_palette_dark()
}

/// `palette-dark` once `s` is applied as at startup, the backdrop is on and the mode is `dark`.
fn palette_dark_with(ui: &MainWindow, s: &Settings, dark: bool) -> bool {
    window_bg_with(ui, s, true, dark);
    palette_dark(ui)
}

const BLACK: [u8; 3] = [0, 0, 0];
const WHITE: [u8; 3] = [255, 255, 255];

#[test]
fn an_unset_tint_keeps_the_modes_palette() {
    let ui = window();
    for (dark, palette) in [(false, LIGHT_PALETTE), (true, DARK_PALETTE)] {
        assert_eq!(palette_dark_with(&ui, &tinted(None, None), dark), dark, "dark = {dark}");
        assert_eq!(palette_of(&ui), palette, "dark = {dark}: the colours are today's literals");
    }
}

#[test]
fn an_unset_colour_keeps_the_modes_palette_at_any_opacity() {
    let ui = window();
    for dark in [false, true] {
        for opacity in [0, 30, 100] {
            assert_eq!(palette_dark_with(&ui, &tinted(None, Some(opacity)), dark), dark, "dark = {dark}, opacity {opacity}");
        }
    }
}

/// (colour, opacity, expected palette-dark) in light mode; `None` opacity is the mode's own (80).
/// Light Blue unset (L 124) is marginal and not pinned.
const LIGHT_CASES: [([u8; 3], Option<u8>, bool); 11] = [
    (BLACK, Some(0), false), (BLACK, Some(30), false), (BLACK, Some(100), true), (BLACK, None, true),
    (WHITE, Some(0), false), (WHITE, Some(30), false), (WHITE, Some(100), false), (WHITE, None, false),
    (BLUE, Some(0), false), (BLUE, Some(30), false), (BLUE, Some(100), true),
];

/// The same in dark mode (the mode's own opacity is 45). White unset (L 132) is marginal and not pinned.
const DARK_CASES: [([u8; 3], Option<u8>, bool); 11] = [
    (BLACK, Some(0), true), (BLACK, Some(30), true), (BLACK, Some(100), true), (BLACK, None, true),
    (WHITE, Some(0), true), (WHITE, Some(30), true), (WHITE, Some(100), false),
    (BLUE, Some(0), true), (BLUE, Some(30), true), (BLUE, Some(100), true), (BLUE, None, true),
];

#[test]
fn the_palette_follows_the_tint_in_light_mode() {
    let ui = window();
    for (colour, opacity, expected) in LIGHT_CASES {
        assert_eq!(palette_dark_with(&ui, &tinted(Some(colour), opacity), false), expected, "light, {colour:?}, {opacity:?}");
    }
}

#[test]
fn the_palette_follows_the_tint_in_dark_mode() {
    let ui = window();
    for (colour, opacity, expected) in DARK_CASES {
        assert_eq!(palette_dark_with(&ui, &tinted(Some(colour), opacity), true), expected, "dark, {colour:?}, {opacity:?}");
    }
}

#[test]
fn the_palette_flips_between_greys_either_side_of_mid_grey() {
    let ui = window();
    for dark in [false, true] {
        assert!(palette_dark_with(&ui, &tinted(Some([120; 3]), Some(100)), dark), "dark = {dark}: grey 120 is dark");
        assert!(!palette_dark_with(&ui, &tinted(Some([136; 3]), Some(100)), dark), "dark = {dark}: grey 136 is light");
    }
}

#[test]
fn white_at_full_opacity_in_dark_mode_uses_the_light_palette() {
    let ui = window();
    palette_dark_with(&ui, &tinted(None, None), true);
    let fills = fixed_fills(&ui);
    assert!(!palette_dark_with(&ui, &tinted(Some(WHITE), Some(100)), true));
    assert_eq!(palette_of(&ui), LIGHT_PALETTE, "the ten colours");
    assert_eq!(fixed_fills(&ui), fills, "the fixed fills");
    assert!(ui.global::<negotium::Theme>().get_dark(), "`dark` stays the mode");
}

#[test]
fn black_at_full_opacity_in_light_mode_uses_the_dark_palette() {
    let ui = window();
    palette_dark_with(&ui, &tinted(None, None), false);
    let fills = fixed_fills(&ui);
    assert!(palette_dark_with(&ui, &tinted(Some(BLACK), Some(100)), false));
    assert_eq!(palette_of(&ui), DARK_PALETTE, "the ten colours");
    assert_eq!(fixed_fills(&ui), fills, "the fixed fills");
    assert!(!ui.global::<negotium::Theme>().get_dark(), "`dark` stays the mode");
}

#[test]
fn mica_never_changes_the_palette() {
    use negotium::settings::BackdropChoice;
    let ui = window();
    let mica = Settings { backdrop: BackdropChoice::Mica, ..tinted(Some(WHITE), Some(100)) };
    assert!(palette_dark_with(&ui, &mica, true));
    assert_eq!(palette_of(&ui), DARK_PALETTE, "text unchanged");
}

#[test]
fn a_window_without_a_backdrop_never_changes_the_palette() {
    let ui = window();
    window_bg_with(&ui, &tinted(Some(WHITE), Some(100)), false, true);
    assert!(palette_dark(&ui));
    assert_eq!(palette_of(&ui), DARK_PALETTE, "text unchanged");
}

fn text_of(ui: &MainWindow) -> Rgba {
    rgba(ui.global::<negotium::Theme>().get_text())
}

#[test]
fn the_rows_switch_the_palette_at_once() {
    let dir = Scratch::new("contrastlive");
    let ui = tint_wired(&dir, tinted(None, None));
    ui.invoke_set_dark(true);
    assert_eq!(text_of(&ui), (255, 255, 255, 255), "dark, nothing chosen");
    radio(&ui, "White").invoke_accessible_default_action();
    slider(&ui, "Tint opacity").set_accessible_value("100");
    assert_eq!(text_of(&ui), (0x1a, 0x1a, 0x1a, 255), "dark + White@100");
    radio(&ui, "Default colour").invoke_accessible_default_action();
    assert_eq!(text_of(&ui), (255, 255, 255, 255), "Default colour");
    radio(&ui, "White").invoke_accessible_default_action();
    ui.invoke_set_dark(false);
    assert_eq!(palette_of(&ui), LIGHT_PALETTE, "light + White@100");
}

#[test]
fn a_saved_tint_and_a_clicked_tint_give_the_same_palette() {
    let dir = Scratch::new("contrastequal");
    let ui = tint_wired(&dir, tinted(Some(WHITE), Some(100)));
    ui.invoke_set_dark(true);
    let at_startup = palette_of(&ui);
    assert_eq!(at_startup, LIGHT_PALETTE, "dark + White@100 from the file");
    radio(&ui, "Default colour").invoke_accessible_default_action();
    assert_ne!(palette_of(&ui), at_startup, "moved away first");
    radio(&ui, "White").invoke_accessible_default_action();
    assert_eq!(palette_of(&ui), at_startup);
}

#[test]
fn no_slint_file_outside_the_theme_reads_the_mode_for_a_surface() {
    let ui_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui");
    let mut reads = Vec::new();
    for entry in std::fs::read_dir(&ui_dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "slint") || path.file_name().is_some_and(|n| n == "theme.slint") {
            continue;
        }
        for (i, line) in std::fs::read_to_string(&path).unwrap().lines().enumerate() {
            if line.contains("Theme.dark") && !line.trim_start().starts_with("//") {
                reads.push(format!("{}:{}", path.file_name().unwrap().to_string_lossy(), i + 1));
            }
        }
    }
    assert!(reads.is_empty(), "Theme.dark is read at {reads:?}; use Theme.palette-dark");
}

// ---- Ports page ----

use negotium::PortData;
use negotium::ports::PortRow;
use negotium::view::{PortsFrame, Target};

fn port_row(id: i32, pid: u32, local_port: u16, listening: bool, actionable: bool) -> PortRow {
    PortRow {
        id,
        app: format!("App{id}"),
        pid,
        protocol: "TCP".into(),
        local_addr: "0.0.0.0".into(),
        local_port,
        state: if listening { "Listening" } else { "Established" }.into(),
        listening,
        actionable,
        can_locate: actionable,
        ..Default::default()
    }
}

fn port_target(pid: u32) -> Target {
    Target { name: format!("App{pid}"), root: Some((pid, 1)), procs: vec![(pid, 1)], ..Default::default() }
}

fn ports_frame(rows: Vec<PortRow>, error: Option<&str>) -> Frame {
    let targets = rows.iter().filter(|r| r.actionable).map(|r| (r.id, port_target(r.pid))).collect();
    Frame {
        page: Page::Ports,
        ports: Some(PortsFrame { rows, targets, all_procs: vec![(1, 1, 0)], error: error.map(String::from) }),
        ..Default::default()
    }
}

fn shown_ports(ui: &MainWindow) -> Vec<String> {
    ui.get_port_rows().iter().map(|r| r.local_port.to_string()).collect()
}

fn ports_bridged() -> (MainWindow, Rc<RefCell<Bridge>>) {
    let (ui, controls, bridge) = bridged();
    controls.set_page(Page::Ports);
    (ui, bridge)
}

#[test]
fn a_failed_read_keeps_the_rows_and_toasts_once_per_failure_streak() {
    let (ui, bridge) = ports_bridged();
    let rows = vec![port_row(1, 10, 80, true, true), port_row(2, 11, 81, true, true)];
    let mut b = bridge.borrow_mut();
    b.apply(&ui, ports_frame(rows.clone(), None));
    assert_eq!(ui.get_port_rows().row_count(), 2);
    b.apply(&ui, ports_frame(rows.clone(), Some("boom")));
    assert_eq!(ui.get_port_rows().row_count(), 2, "the rows stay");
    assert_eq!(ui.get_toast(), "Couldn't list ports: boom.");
    ui.set_toast("".into());
    b.apply(&ui, ports_frame(rows.clone(), Some("boom")));
    assert_eq!(ui.get_toast(), "", "the same streak toasts once");
    b.apply(&ui, ports_frame(rows.clone(), None));
    b.apply(&ui, ports_frame(rows, Some("again")));
    assert_eq!(ui.get_toast(), "Couldn't list ports: again.", "a good frame ended the streak");
}

#[test]
fn searching_filters_the_pushed_rows_without_a_new_frame() {
    let (ui, bridge) = ports_bridged();
    let mut b = bridge.borrow_mut();
    b.apply(&ui, ports_frame(vec![port_row(1, 10, 5173, true, true), port_row(2, 11, 80, true, true)], None));
    assert_eq!(ui.get_ports_count_text(), "2 ports");
    b.set_ports_search("5173");
    assert_eq!(shown_ports(&ui), ["5173"]);
    assert_eq!(ui.get_ports_count_text(), "1 port matches 5173");
    b.set_ports_search("");
    assert_eq!(shown_ports(&ui), ["80", "5173"]);
}

#[test]
fn the_all_view_adds_connections_to_the_listening_ones() {
    let (ui, bridge) = ports_bridged();
    let mut b = bridge.borrow_mut();
    b.apply(&ui, ports_frame(vec![port_row(1, 10, 80, true, true), port_row(2, 11, 50000, false, true)], None));
    assert_eq!(shown_ports(&ui), ["80"], "Listening is the default view");
    b.set_ports_view(&ui, 1);
    assert_eq!(ui.get_ports_view(), 1);
    assert_eq!(shown_ports(&ui), ["80", "50000"]);
}

#[test]
fn a_header_click_sorts_and_the_same_column_again_flips_the_direction() {
    let (ui, bridge) = ports_bridged();
    let mut b = bridge.borrow_mut();
    b.apply(&ui, ports_frame(vec![port_row(1, 30, 5173, true, true), port_row(2, 10, 80, true, true), port_row(3, 20, 9, true, true)], None));
    assert_eq!((ui.get_ports_sort_column(), ui.get_ports_sort_descending()), (4, false));
    assert_eq!(shown_ports(&ui), ["9", "80", "5173"], "local port, ascending, by default");
    b.sort_ports(&ui, 4);
    assert!(ui.get_ports_sort_descending());
    assert_eq!(shown_ports(&ui), ["5173", "80", "9"]);
    b.sort_ports(&ui, 4);
    assert!(!ui.get_ports_sort_descending());
    b.sort_ports(&ui, 1);
    assert_eq!((ui.get_ports_sort_column(), ui.get_ports_sort_descending()), (1, false));
    assert_eq!(shown_ports(&ui), ["80", "9", "5173"], "by PID");
}

#[test]
fn a_ports_row_has_the_frames_target_unless_it_has_no_owner_to_act_on() {
    let (ui, bridge) = ports_bridged();
    let mut b = bridge.borrow_mut();
    b.apply(&ui, ports_frame(vec![port_row(1, 10, 80, true, true), port_row(2, 4, 81, true, false)], None));
    assert_eq!(b.port_target(1), Some(port_target(10)));
    assert_eq!(b.port_pid(1), Some(10));
    assert_eq!((b.port_target(2), b.port_pid(2)), (None, None), "the PID 4 row has no target");
    assert_eq!(b.port_procs(), [(1, 1, 0)]);
}

#[test]
fn the_rail_has_a_ports_page_button() {
    let ui = window();
    let picked = Rc::new(RefCell::new(None));
    let p = picked.clone();
    ui.on_select_page(move |i| *p.borrow_mut() = Some(i));
    let ports = El::find_by_accessible_label(&ui, "Ports page").next().expect("rail item");
    ports.invoke_accessible_default_action();
    assert_eq!(*picked.borrow(), Some(6));
    ui.set_page(6);
    assert_eq!(ports.accessible_checked(), Some(true));
}

#[test]
fn the_rail_has_a_users_page_button() {
    let ui = window();
    let picked = Rc::new(RefCell::new(None));
    let p = picked.clone();
    ui.on_select_page(move |i| *p.borrow_mut() = Some(i));
    let users = El::find_by_accessible_label(&ui, "Users page").next().expect("rail item");
    users.invoke_accessible_default_action();
    assert_eq!(*picked.borrow(), Some(7));
    ui.set_page(7);
    assert_eq!(users.accessible_checked(), Some(true));
}

#[test]
fn all_eight_rail_buttons_fit_the_smallest_window() {
    let ui = window();
    ui.window().set_size(slint::PhysicalSize::new(760, 440));
    El::find_by_accessible_label(&ui, "Users page").next().expect("rail item");
    let settings = El::find_by_accessible_label(&ui, "Settings page").next().expect("rail item");
    let bottom = settings.absolute_position().y + settings.size().height;
    assert!(bottom <= 440.0, "the Settings button ends at {bottom}");
}

fn port_data(id: i32, actionable: bool) -> PortData {
    PortData {
        id,
        app: "Node.js".into(),
        pid: "42".into(),
        protocol: "TCP".into(),
        local_addr: "0.0.0.0".into(),
        local_port: format!("{}", 5170 + id).into(),
        state: "Listening".into(),
        actionable,
        can_locate: actionable,
        ..Default::default()
    }
}

fn ports_window(rows: Vec<PortData>) -> MainWindow {
    let ui = window();
    ui.set_page(6);
    ui.set_port_rows(ModelRc::new(VecModel::from(rows)));
    ui.set_ports_count_text("2 ports".into());
    ui.window().set_size(PhysicalSize::new(1000, 720));
    ui.show().unwrap();
    ui
}

#[test]
fn the_ports_page_shows_the_view_choice_and_the_count() {
    let ui = ports_window(vec![port_data(1, true), port_data(2, true)]);
    let picked = Rc::new(RefCell::new(None));
    let p = picked.clone();
    ui.on_set_ports_view(move |v| *p.borrow_mut() = Some(v));
    let listening = El::find_by_accessible_label(&ui, "Listening").next().expect("Listening control");
    let all = El::find_by_accessible_label(&ui, "All").next().expect("All control");
    assert_eq!(listening.accessible_checked(), Some(true));
    all.invoke_accessible_default_action();
    assert_eq!(*picked.borrow(), Some(1));
    assert!(El::find_by_accessible_label(&ui, "2 ports").next().is_some(), "the count shows");
}

#[test]
fn a_row_with_no_owner_to_act_on_offers_no_end_task() {
    let ui = ports_window(vec![port_data(1, true), port_data(2, false)]);
    let actions = Rc::new(RefCell::new(Vec::new()));
    let a = actions.clone();
    ui.on_port_action(move |name, id| a.borrow_mut().push((name.to_string(), id)));
    let label = |port: u16| format!("Node.js, TCP port {port}, Listening, PID 42");
    // Row 2 cannot be acted on: its End task item is disabled, so choosing it does nothing.
    let row = El::find_by_accessible_label(&ui, &label(5172)).next().expect("second row");
    let (x, y) = centre(&row);
    press(&ui, x, y, slint::platform::PointerEventButton::Right);
    key(&ui, slint::platform::Key::DownArrow);
    key(&ui, slint::platform::Key::Return);
    assert!(actions.borrow().is_empty(), "{:?}", actions.borrow());
    key(&ui, slint::platform::Key::Escape);
    // Row 1 can: the same keys end its task.
    let row = El::find_by_accessible_label(&ui, &label(5171)).next().expect("first row");
    let (x, y) = centre(&row);
    press(&ui, x, y, slint::platform::PointerEventButton::Right);
    key(&ui, slint::platform::Key::DownArrow);
    key(&ui, slint::platform::Key::Return);
    assert_eq!(*actions.borrow(), [("end-task".to_string(), 1)]);
}

// ---- Grouped Settings (docs/superpowers/plans/2026-10-28-grouped-settings.md) ----

/// The Settings groups in page order: header and the controls of its rows, by accessible label.
const SETTING_GROUP_ROWS: [(&str, &[&str]); 7] = [
    ("Appearance", &["System", "Mica", "Smooth graphs"]),
    ("Window and tray", &["Always on top", "Keep running in the tray when closed", "Show CPU in tray icon"]),
    ("Processes", &["0.5 s", "Group apps by type", "Aequilibrium (Runaway CPU restraint)"]),
    ("Startup and shortcuts", &["Start with Windows", "Admin mode", "Open on Ctrl+Shift+Esc", "Pick a window to end on Ctrl+Alt+Shift+Esc"]),
    ("Updates", &["Check for updates on startup", "Check for updates"]),
    ("Add-ons", &["CPU temperature add-on"]),
    ("Advanced", &["Use GPU rendering", "Open Windows Task Manager"]),
];

/// The header of the Settings group named `name`.
fn group_header(ui: &MainWindow, name: &str) -> El {
    try_group_header(ui, name).unwrap_or_else(|| panic!("no group header {name:?}"))
}

fn try_group_header(ui: &MainWindow, name: &str) -> Option<El> {
    El::find_by_accessible_label(ui, name).find(|e| e.accessible_expandable() == Some(true))
}

/// The switch, choice or button labelled `label`, if the page shows it.
fn setting_control(ui: &MainWindow, label: &str) -> Option<El> {
    El::find_by_accessible_label(ui, label).find(|e| {
        matches!(e.accessible_role(), Some(AccessibleRole::Switch | AccessibleRole::RadioButton | AccessibleRole::Button))
            && e.accessible_expandable() != Some(true)
    })
}

#[test]
fn settings_groups_are_listed_in_order_and_open_by_default() {
    let ui = settings_window(1000, 2600);
    let mut last_y = f32::MIN;
    for (name, _) in SETTING_GROUP_ROWS {
        let h = group_header(&ui, name);
        assert_eq!(h.accessible_expanded(), Some(true), "{name} starts open");
        let y = rect_of(&h).1;
        assert!(y > last_y, "{name} comes after the group before it");
        last_y = y;
    }
}

#[test]
fn each_setting_sits_in_its_group() {
    let ui = settings_window(1000, 2600);
    for (i, (name, labels)) in SETTING_GROUP_ROWS.iter().enumerate() {
        let top = rect_of(&group_header(&ui, name)).1;
        let bottom = SETTING_GROUP_ROWS.get(i + 1).map_or(f32::MAX, |(next, _)| rect_of(&group_header(&ui, next)).1);
        for label in *labels {
            let c = setting_control(&ui, label).unwrap_or_else(|| panic!("no control {label:?}"));
            let y = rect_of(&c).1;
            assert!(y > top && y < bottom, "{label} is in {name}: {y} not in {top}..{bottom}");
        }
    }
}

#[test]
fn a_group_header_closes_and_reopens_its_group_and_saves_it() {
    let dir = Scratch::new("groups");
    let path = dir.0.join("settings.json");
    let (ui, _, settings) = settings_wired(&dir, Settings::default(), false);
    assert!(setting_control(&ui, "Check for updates on startup").is_some(), "the row shows while open");
    group_header(&ui, "Updates").invoke_accessible_default_action();
    assert_eq!(group_header(&ui, "Updates").accessible_expanded(), Some(false));
    assert!(setting_control(&ui, "Check for updates on startup").is_none(), "a closed group hides its rows");
    assert_eq!(settings.borrow().collapsed_groups, ["updates"]);
    saved(&path, "updates closed", |s| s.collapsed_groups == ["updates"]);
    group_header(&ui, "Updates").invoke_accessible_default_action();
    assert_eq!(group_header(&ui, "Updates").accessible_expanded(), Some(true));
    assert!(setting_control(&ui, "Check for updates on startup").is_some(), "reopened");
    saved(&path, "updates open", |s| s.collapsed_groups.is_empty());
}

#[test]
fn closed_groups_are_restored_at_launch() {
    let ui = settings_window(1000, 2600);
    negotium::settingspage::apply_settings(&ui, &Settings { collapsed_groups: vec!["advanced".into()], ..Settings::default() });
    assert_eq!(group_header(&ui, "Advanced").accessible_expanded(), Some(false));
    assert!(setting_control(&ui, "Use GPU rendering").is_none(), "the closed group's rows are hidden");
    assert!(setting_control(&ui, "System").is_some(), "other groups stay open");
}

#[test]
fn an_open_modal_disables_the_group_headers() {
    let ui = settings_window(1000, 2600);
    let toggled = Rc::new(RefCell::new(Vec::new()));
    let t = toggled.clone();
    ui.on_settings_group_toggled(move |i| t.borrow_mut().push(i));
    ui.set_details_visible(true);
    for (name, _) in SETTING_GROUP_ROWS {
        let h = group_header(&ui, name);
        assert_eq!(h.accessible_enabled(), Some(false), "{name}");
        h.invoke_accessible_default_action();
    }
    assert!(toggled.borrow().is_empty(), "nothing beneath a modal acts");
}

#[test]
fn group_headers_wrap_at_the_smallest_window() {
    use i_slint_backend_testing::ElementRoot;
    // The smallest width; tall, because the testing backend finds only what the view shows.
    let ui = settings_window(760, 2600);
    let headers = ui.root_element().query_descendants().match_id("SettingGroup::header").find_all();
    let summaries = ui.root_element().query_descendants().match_id("SettingGroup::summary").find_all();
    let chevrons = ui.root_element().query_descendants().match_id("SettingGroup::chevron").find_all();
    assert!(headers.len() == 7 && headers.len() == summaries.len() && headers.len() == chevrons.len(), "one summary and chevron per header");
    for ((h, s), c) in headers.iter().zip(&summaries).zip(&chevrons) {
        let (h, s, c) = (rect_of(h), rect_of(s), rect_of(c));
        assert!(s.1 + s.3 <= h.1 + h.3, "the summary fits in its header: {s:?} {h:?}");
        assert!(s.0 + s.2 <= c.0, "the summary stays left of the chevron: {s:?} {c:?}");
        assert!(h.0 + h.2 <= 760.0 - 14.0, "the header is clear of the scrollbar gutter: {h:?}");
    }
}

/// Types `query` into the Settings search box, replacing what was there.
fn search_settings(ui: &MainWindow, query: &str) {
    search_box(ui).set_accessible_value(query);
    slint::platform::update_timers_and_animations();
}

fn search_box(ui: &MainWindow) -> El {
    El::find_by_accessible_label(ui, "Search settings")
        .find(|e| e.accessible_role() == Some(AccessibleRole::TextInput))
        .expect("the Settings search box")
}

const NO_MATCH: &str = "No settings match your search.";

#[test]
fn search_shows_only_matching_rows_and_opens_their_groups() {
    let dir = Scratch::new("search");
    let (ui, _, settings) = settings_wired(&dir, Settings { collapsed_groups: vec!["window".into()], ..Settings::default() }, false);
    assert!(setting_control(&ui, "Show CPU in tray icon").is_none(), "closed before the search");
    search_settings(&ui, "tray");
    assert!(setting_control(&ui, "Keep running in the tray when closed").is_some());
    assert!(setting_control(&ui, "Show CPU in tray icon").is_some());
    assert!(setting_control(&ui, "Always on top").is_none(), "a row that does not match is hidden");
    assert!(setting_control(&ui, "System").is_none(), "so are other groups' rows");
    assert_eq!(group_header(&ui, "Window and tray").accessible_expanded(), Some(true), "a group with a match opens");
    assert!(try_group_header(&ui, "Updates").is_none(), "a group with no match is hidden");
    assert!(text(&ui, NO_MATCH).is_none());
    assert_eq!(settings.borrow().collapsed_groups, ["window"], "a search saves nothing");
}

#[test]
fn search_matches_descriptions() {
    let dir = Scratch::new("search-desc");
    let (ui, _, _) = settings_wired(&dir, Settings::default(), false);
    search_settings(&ui, "BLUR");
    assert!(setting_control(&ui, "Mica").is_some(), "Backdrop's description mentions blur");
    assert!(setting_control(&ui, "System").is_none());
}

#[test]
fn a_search_with_no_match_says_so() {
    let dir = Scratch::new("search-none");
    let (ui, _, _) = settings_wired(&dir, Settings::default(), false);
    assert!(text(&ui, NO_MATCH).is_none(), "not before a search");
    search_settings(&ui, "zzz");
    assert!(text(&ui, NO_MATCH).is_some());
    for (name, _) in SETTING_GROUP_ROWS {
        assert!(try_group_header(&ui, name).is_none(), "{name} is hidden");
    }
}

#[test]
fn a_search_for_a_hidden_row_says_no_match() {
    let dir = Scratch::new("search-tint");
    let (ui, _, _) = settings_wired(&dir, Settings::default(), false);
    // Mica: the Tint rows are not offered, so "tint" finds nothing.
    search_settings(&ui, "tint");
    assert!(text(&ui, NO_MATCH).is_some());
    assert!(try_group_header(&ui, "Appearance").is_none());
}

#[test]
fn clearing_the_search_restores_saved_groups_and_saves_nothing() {
    let dir = Scratch::new("search-clear");
    let path = dir.0.join("settings.json");
    let (ui, _, settings) = settings_wired(&dir, Settings::default(), false);
    group_header(&ui, "Updates").invoke_accessible_default_action();
    search_settings(&ui, "update");
    assert_eq!(group_header(&ui, "Updates").accessible_expanded(), Some(true), "opened by the search");
    search_settings(&ui, "top");
    switch(&ui, "Always on top").invoke_accessible_default_action();
    search_settings(&ui, "");
    assert_eq!(group_header(&ui, "Updates").accessible_expanded(), Some(false), "closed again");
    assert!(setting_control(&ui, "Check for updates on startup").is_none());
    assert!(setting_control(&ui, "System").is_some(), "every other row is back");
    assert_eq!(settings.borrow().collapsed_groups, ["updates"]);
    saved(&path, "on top, updates still closed", |s| s.always_on_top && s.collapsed_groups == ["updates"]);
}

#[test]
fn an_open_modal_disables_the_search() {
    let ui = settings_window(1000, 2600);
    ui.set_details_visible(true);
    assert_eq!(search_box(&ui).accessible_enabled(), Some(false));
}

#[test]
fn a_header_activated_during_a_search_saves_nothing() {
    let dir = Scratch::new("search-header");
    let (ui, _, settings) = settings_wired(&dir, Settings::default(), false);
    search_settings(&ui, "update");
    group_header(&ui, "Updates").invoke_accessible_default_action();
    assert!(settings.borrow().collapsed_groups.is_empty(), "the header shows open and changes nothing");
    search_settings(&ui, "");
    assert_eq!(group_header(&ui, "Updates").accessible_expanded(), Some(true), "still open after the search");
}

#[test]
fn the_expand_action_only_opens_a_group() {
    let dir = Scratch::new("expand-action");
    let (ui, _, settings) = settings_wired(&dir, Settings { collapsed_groups: vec!["updates".into()], ..Settings::default() }, false);
    group_header(&ui, "Appearance").invoke_accessible_expand_action();
    assert_eq!(group_header(&ui, "Appearance").accessible_expanded(), Some(true), "an open group stays open");
    group_header(&ui, "Updates").invoke_accessible_expand_action();
    assert_eq!(group_header(&ui, "Updates").accessible_expanded(), Some(true), "a closed group opens");
    assert!(settings.borrow().collapsed_groups.is_empty());
}

#[test]
fn a_search_of_only_spaces_keeps_the_saved_groups() {
    let dir = Scratch::new("search-blank");
    let (ui, _, _) = settings_wired(&dir, Settings { collapsed_groups: vec!["updates".into()], ..Settings::default() }, false);
    search_settings(&ui, "   ");
    assert_eq!(group_header(&ui, "Updates").accessible_expanded(), Some(false), "blank is not a search");
    assert!(setting_control(&ui, "System").is_some());
    assert!(text(&ui, NO_MATCH).is_none());
}

// ---- Users page (docs/superpowers/specs/2026-10-28-users-details-settings-design.md §3) ----

use negotium::users::UsersRead;
use negotium::{UserRowData, YouData};
use negotium_actions::users::{Account, DirSize, Session, SessionState};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn you_data() -> YouData {
    YouData {
        initial: "A".into(),
        name: "Alex Doe".into(),
        subtitle: "alex@example.com".into(),
        account_type: "Administrator".into(),
        signed_in: "Signed in for 10 min".into(),
        place: "This PC".into(),
        profile_path: r"C:\Users\alex".into(),
        profile_size: "1.0 GB".into(),
        sign_in: "Password, PIN, Face".into(),
        password_set: "558 days ago".into(),
        groups: "Administrators, Users".into(),
        negotium_admin: "Yes".into(),
    }
}

fn user_row(id: i32, name: &str) -> UserRowData {
    UserRowData { id, name: name.into(), account_type: "Standard".into(), state: "Disconnected".into(), idle: "—".into() }
}

fn users_window(rows: Vec<UserRowData>, need_admin: bool, note: &str) -> MainWindow {
    let ui = window();
    ui.set_page(7);
    ui.set_you(you_data());
    ui.set_user_rows(ModelRc::new(VecModel::from(rows)));
    ui.set_users_need_admin(need_admin);
    ui.set_users_read_note(note.into());
    ui.window().set_size(PhysicalSize::new(760, 1400));
    ui.show().unwrap();
    ui
}

fn users_el(ui: &MainWindow, label: &str) -> El {
    El::find_by_accessible_label(ui, label).next().unwrap_or_else(|| panic!("missing {label}"))
}

/// Records the Users page's `user-action` reports.
fn user_actions(ui: &MainWindow) -> Rc<RefCell<Vec<(String, i32)>>> {
    let log = Rc::new(RefCell::new(Vec::new()));
    let l = log.clone();
    ui.on_user_action(move |name, id| l.borrow_mut().push((name.to_string(), id)));
    log
}

#[test]
fn the_users_card_shows_the_account_and_every_detail() {
    let ui = users_window(vec![], false, "");
    for shown in [
        "Alex Doe",
        "alex@example.com",
        "Administrator",
        "Signed in for 10 min",
        "This PC",
        r"C:\Users\alex",
        "1.0 GB",
        "Password, PIN, Face",
        "558 days ago",
        "Administrators, Users",
        "Yes",
    ] {
        assert!(text(&ui, shown).is_some(), "the card shows {shown}");
    }
}

#[test]
fn with_no_other_sessions_the_page_says_so() {
    let ui = users_window(vec![], false, "");
    assert!(text(&ui, "No other users are signed in.").is_some());
}

#[test]
fn with_other_sessions_the_page_lists_each_one_and_drops_the_empty_text() {
    let ui = users_window(vec![user_row(3, "alex"), user_row(4, "bo")], false, "");
    assert!(text(&ui, "No other users are signed in.").is_none());
    for label in ["Sign out alex", "Disconnect alex", "Sign out bo", "Disconnect bo"] {
        users_el(&ui, label);
    }
}

#[test]
fn each_card_button_reports_its_action_with_no_session() {
    let ui = users_window(vec![], false, "");
    let log = user_actions(&ui);
    for label in ["Lock", "Switch user", "Sign out", "Open account settings"] {
        users_el(&ui, label).invoke_accessible_default_action();
    }
    let expected: Vec<(String, i32)> = [("lock", -1), ("switch-user", -1), ("sign-out", -1), ("account-settings", -1)]
        .iter()
        .map(|(n, i)| (n.to_string(), *i))
        .collect();
    assert_eq!(*log.borrow(), expected);
}

#[test]
fn row_buttons_report_their_session() {
    let ui = users_window(vec![user_row(3, "alex")], false, "");
    let log = user_actions(&ui);
    users_el(&ui, "Sign out alex").invoke_accessible_default_action();
    users_el(&ui, "Disconnect alex").invoke_accessible_default_action();
    assert_eq!(*log.borrow(), [("sign-out-session".to_string(), 3), ("disconnect".to_string(), 3)]);
}

#[test]
fn without_admin_rights_rows_are_disabled_and_the_banner_offers_a_restart() {
    let ui = users_window(vec![user_row(3, "alex")], true, "");
    let restarts = Rc::new(RefCell::new(0));
    let r = restarts.clone();
    ui.on_restart_admin(move || *r.borrow_mut() += 1);
    assert!(text(&ui, "To sign out or disconnect another user, restart Negotium as admin.").is_some());
    users_el(&ui, "Restart as admin").invoke_accessible_default_action();
    assert_eq!(*restarts.borrow(), 1);
    assert_eq!(users_el(&ui, "Sign out alex").accessible_enabled(), Some(false));
    assert_eq!(users_el(&ui, "Disconnect alex").accessible_enabled(), Some(false));
    assert_eq!(users_el(&ui, "Lock").accessible_enabled(), Some(true), "the card's own buttons need no admin rights");
    // Nobody to act on: nothing to explain.
    ui.set_user_rows(ModelRc::new(VecModel::from(Vec::<UserRowData>::new())));
    assert!(El::find_by_accessible_label(&ui, "Restart as admin").next().is_none());
}

#[test]
fn with_admin_rights_the_row_buttons_are_enabled_and_no_banner_shows() {
    let ui = users_window(vec![user_row(3, "alex")], false, "");
    assert_eq!(users_el(&ui, "Sign out alex").accessible_enabled(), Some(true));
    assert!(El::find_by_accessible_label(&ui, "Restart as admin").next().is_none());
}

#[test]
fn a_read_note_shows_on_the_page() {
    let ui = users_window(vec![], false, "Couldn't read the signed-in users: boom.");
    assert!(text(&ui, "Couldn't read the signed-in users: boom.").is_some());
}

#[test]
fn the_user_confirm_shows_its_text_and_reports_the_choice() {
    let ui = users_window(vec![], false, "");
    let log = Rc::new(RefCell::new(Vec::new()));
    let l = log.clone();
    ui.on_user_confirm(move |ok| l.borrow_mut().push(ok));
    assert!(!ui.get_modal_open());
    ui.invoke_show_user_confirm("Sign out?".into(), "Save your work first.".into(), "Sign out".into());
    assert!(ui.get_modal_open());
    assert!(text(&ui, "Sign out?").is_some());
    assert!(text(&ui, "Save your work first.").is_some());
    assert_eq!(users_el(&ui, "Lock").accessible_enabled(), Some(false), "the page beneath is disabled");
    users_el(&ui, "Cancel").invoke_accessible_default_action();
    // The card's own "Sign out" is disabled under the dialog: the enabled one is the dialog's.
    El::find_by_accessible_label(&ui, "Sign out")
        .find(|e| e.accessible_enabled() == Some(true))
        .expect("the dialog's Sign out")
        .invoke_accessible_default_action();
    assert_eq!(*log.borrow(), [false, true]);
}

#[test]
fn return_in_the_user_confirm_cancels() {
    let ui = users_window(vec![], false, "");
    let log = Rc::new(RefCell::new(Vec::new()));
    let l = log.clone();
    ui.on_user_confirm(move |ok| l.borrow_mut().push(ok));
    ui.invoke_show_user_confirm("Sign out?".into(), "Save your work first.".into(), "Sign out".into());
    slint::platform::update_timers_and_animations();
    key(&ui, slint::platform::Key::Return);
    assert_eq!(*log.borrow(), [false], "Cancel is the primary choice");
}

#[test]
fn the_card_and_row_buttons_do_not_overlap_and_stay_inside_the_window() {
    let ui = users_window(vec![user_row(3, "alex"), user_row(4, "bo")], false, "");
    let labels = [
        "Lock",
        "Switch user",
        "Sign out",
        "Open account settings",
        "Sign out alex",
        "Disconnect alex",
        "Sign out bo",
        "Disconnect bo",
    ];
    let rects: Vec<Rect> = labels.iter().map(|l| rect(&ui, l)).collect();
    for (label, r) in labels.iter().zip(&rects) {
        assert!(r.0 + r.2 <= 760.0 - 14.0, "{label} ends at {}", r.0 + r.2);
    }
    for (i, a) in rects.iter().enumerate() {
        for (j, b) in rects.iter().enumerate().skip(i + 1) {
            let apart = a.0 + a.2 <= b.0 || b.0 + b.2 <= a.0 || a.1 + a.3 <= b.1 || b.1 + b.3 <= a.1;
            assert!(apart, "{} overlaps {}", labels[i], labels[j]);
        }
    }
}

// The bridge side.

fn session(id: u32, user: &str) -> Session {
    Session { id, user: user.into(), domain: "PC".into(), state: SessionState::Disconnected, signed_in_secs: Some(600), idle_secs: None }
}

fn users_read(sessions: Result<Vec<Session>, String>, account: Account) -> UsersRead {
    UsersRead {
        sessions,
        current: Some(1),
        remote: false,
        account,
        admin_member: true,
        admin_group: Some("Administrators".into()),
        groups: HashMap::new(),
    }
}

fn my_account() -> Account {
    Account { name: Some(r"PC\alex".into()), full_name: Some("Alex Doe".into()), ..Default::default() }
}

fn two_sessions() -> Vec<Session> {
    vec![Session { state: SessionState::Active, ..session(1, "alex") }, session(3, "bo")]
}

fn users_bridged() -> (MainWindow, Rc<RefCell<Bridge>>) {
    let (ui, controls, bridge) = bridged();
    controls.set_page(Page::Users);
    (ui, bridge)
}

#[test]
fn set_users_pushes_the_card_and_the_other_rows() {
    let (ui, bridge) = users_bridged();
    bridge.borrow_mut().set_users(&ui, users_read(Ok(two_sessions()), my_account()), false);
    let you = ui.get_you();
    assert_eq!(
        (you.name.as_str(), you.account_type.as_str(), you.signed_in.as_str()),
        ("Alex Doe", "Administrator", "Signed in for 10 min")
    );
    let rows: Vec<(i32, String)> = ui.get_user_rows().iter().map(|r| (r.id, r.name.to_string())).collect();
    assert_eq!(rows, [(3, "bo".to_string())], "the current session is not a row");
    assert!(ui.get_users_need_admin(), "not elevated");
    assert_eq!(ui.get_users_read_note(), "");
}

#[test]
fn an_elevated_read_does_not_ask_for_admin_rights() {
    let (ui, bridge) = users_bridged();
    let mut b = bridge.borrow_mut();
    b.set_users(&ui, users_read(Ok(two_sessions()), my_account()), false);
    assert!(ui.get_users_need_admin());
    b.set_users(&ui, users_read(Ok(two_sessions()), my_account()), true);
    assert!(!ui.get_users_need_admin());
}

#[test]
fn a_failed_session_read_keeps_the_rows_and_sets_a_note_until_a_read_succeeds() {
    let (ui, bridge) = users_bridged();
    let mut b = bridge.borrow_mut();
    b.set_users(&ui, users_read(Ok(two_sessions()), my_account()), true);
    b.set_users(&ui, users_read(Err("boom".into()), my_account()), true);
    assert_eq!(ui.get_user_rows().row_count(), 1, "the rows stay");
    assert_eq!(ui.get_users_read_note(), "Couldn't read the signed-in users: boom.");
    assert_eq!(ui.get_toast(), "", "a note, not a toast");
    b.set_users(&ui, users_read(Ok(two_sessions()), my_account()), true);
    assert_eq!(ui.get_users_read_note(), "");
}

#[test]
fn a_panicked_read_job_sets_a_plain_note() {
    let (ui, bridge) = users_bridged();
    let mut b = bridge.borrow_mut();
    assert!(b.begin_users_read());
    b.users_read_failed(&ui);
    assert_eq!(ui.get_users_read_note(), "Couldn't read the users.");
    assert!(b.begin_users_read(), "the failure ends the read");
}

#[test]
fn a_picture_that_does_not_exist_leaves_the_initial() {
    let (ui, bridge) = users_bridged();
    let account = Account { picture: Some(PathBuf::from(r"C:\no\such\picture.jpg")), ..my_account() };
    bridge.borrow_mut().set_users(&ui, users_read(Ok(two_sessions()), account), true);
    assert!(!ui.get_you_has_picture());
    assert_eq!(ui.get_you().initial, "A");
}

/// A 1 x 1 PNG.
const PIXEL_PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8, 0xCF, 0xC0, 0xF0, 0x1F, 0x00, 0x05, 0x00, 0x01, 0xFF, 0x89, 0x99, 0x3D, 0x1D, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

#[test]
fn a_picture_that_exists_shows_and_is_dropped_when_the_path_goes() {
    let dir = Scratch::new("users-picture");
    let path = dir.0.join("me.png");
    std::fs::write(&path, PIXEL_PNG).unwrap();
    let (ui, bridge) = users_bridged();
    let mut b = bridge.borrow_mut();
    b.set_users(&ui, users_read(Ok(two_sessions()), Account { picture: Some(path), ..my_account() }), true);
    assert!(ui.get_you_has_picture());
    b.set_users(&ui, users_read(Ok(two_sessions()), my_account()), true);
    assert!(!ui.get_you_has_picture());
}

#[test]
fn the_profile_size_calculates_first_then_holds_the_old_value_while_a_new_walk_runs() {
    let (ui, bridge) = users_bridged();
    let mut b = bridge.borrow_mut();
    let profile = PathBuf::from(r"C:\Users\alex");
    let t0 = Instant::now();
    b.set_users(&ui, users_read(Ok(two_sessions()), Account { profile: Some(profile.clone()), ..my_account() }), true);
    assert_eq!(ui.get_you().profile_size, "Calculating\u{2026}");
    assert_eq!(b.begin_profile_size(t0), Some(profile.clone()), "the first walk is due");
    assert_eq!(b.begin_profile_size(t0), None, "one walk at a time");
    b.set_profile_size(&ui, Some(DirSize { bytes: 1 << 30, skipped: 0 }), t0);
    assert_eq!(ui.get_you().profile_size, "1.0 GB");
    assert_eq!(b.begin_profile_size(t0 + Duration::from_secs(599)), None, "not old enough");
    assert_eq!(b.begin_profile_size(t0 + Duration::from_secs(600)), Some(profile), "ten minutes old");
    assert_eq!(ui.get_you().profile_size, "1.0 GB", "the old size stays while the walk runs");
    b.set_profile_size(&ui, None, t0 + Duration::from_secs(601));
    assert_eq!(ui.get_you().profile_size, "—", "a failed walk");
}

#[test]
fn a_later_read_keeps_the_known_profile_size() {
    let (ui, bridge) = users_bridged();
    let mut b = bridge.borrow_mut();
    let account = Account { profile: Some(PathBuf::from(r"C:\Users\alex")), ..my_account() };
    b.set_users(&ui, users_read(Ok(two_sessions()), account.clone()), true);
    b.set_profile_size(&ui, Some(DirSize { bytes: 1 << 30, skipped: 2 }), Instant::now());
    b.set_users(&ui, users_read(Ok(two_sessions()), account), true);
    assert_eq!(ui.get_you().profile_size, "At least 1.0 GB");
}

#[test]
fn no_profile_folder_means_no_walk_and_a_dash() {
    let (ui, bridge) = users_bridged();
    let mut b = bridge.borrow_mut();
    b.set_users(&ui, users_read(Ok(two_sessions()), my_account()), true);
    assert_eq!(b.begin_profile_size(Instant::now()), None);
    assert_eq!(ui.get_you().profile_size, "—");
}

#[test]
fn only_one_users_read_runs_at_a_time_and_rows_answer_for_their_name() {
    let (ui, bridge) = users_bridged();
    let mut b = bridge.borrow_mut();
    assert!(b.begin_users_read());
    assert!(!b.begin_users_read(), "one read is running");
    b.set_users(&ui, users_read(Ok(two_sessions()), my_account()), true);
    assert!(b.begin_users_read(), "set_users ends the read");
    assert_eq!(b.user_name(3), Some("bo".to_string()));
    assert_eq!(b.user_name(99), None);
}

#[test]
fn the_account_picture_loads_when_there_is_one() {
    let Some(picture) = negotium_actions::users::current_account().picture else {
        eprintln!("skipped: this account has no picture");
        return;
    };
    assert!(slint::Image::load_from_path(&picture).is_ok(), "{} should decode", picture.display());
}
