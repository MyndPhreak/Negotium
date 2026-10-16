//! UI-thread glue: applies worker Frames to Slint models in place and keeps
//! expand/search state local so they react instantly. No Windows calls here.
use crate::controls::{Controls, Page};
use crate::nav::{NavKey, NavOutcome, navigate};
use crate::perf::{GraphView, PerfFrame};
use crate::view::{Frame, RowView, Target, visible_rows};
use crate::worker::SharedHistory;
use crate::{DeviceData, GraphData, MainWindow, MeterData, RowData, ScrubRow, SegmentData, StatData, TileData};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

/// Minimal row storage the sync can use. Deliberately has no "replace all".
pub trait RowStore<T> {
    fn len(&self) -> usize;
    fn get(&self, i: usize) -> Option<T>;
    fn set(&self, i: usize, v: T);
    fn push(&self, v: T);
    fn pop(&self);
}

impl<T: Clone + 'static> RowStore<T> for VecModel<T> {
    fn len(&self) -> usize { self.row_count() }
    fn get(&self, i: usize) -> Option<T> { self.row_data(i) }
    fn set(&self, i: usize, v: T) { self.set_row_data(i, v) }
    fn push(&self, v: T) { VecModel::push(self, v) }
    fn pop(&self) { let n = self.row_count(); if n > 0 { self.remove(n - 1); } }
}

pub fn sync_model<T: Clone + PartialEq, S: RowStore<T>>(store: &S, items: Vec<T>) {
    let common = store.len().min(items.len());
    for (i, item) in items.iter().enumerate().take(common) {
        if store.get(i).as_ref() != Some(item) {
            store.set(i, item.clone());
        }
    }
    while store.len() > items.len() {
        store.pop();
    }
    for item in items.into_iter().skip(common) {
        store.push(item);
    }
}

/// Which rows the list shows, in which order. Unfrozen it simply follows the latest frame. While
/// frozen (pointer over the list, or a row's context menu open) every index keeps its row: rows
/// that went away stay in place, marked exited and inert, and new rows wait until the list
/// unfreezes, so a click can never land on a different app than the one under the pointer.
/// Values of the rows it keeps go on updating.
#[derive(Default)]
pub struct RowLayout {
    shown: Vec<RowView>,
    /// Apps the user expanded while frozen: their children are let in once, with the first frame
    /// that has them (they are only built after the expand reaches the worker).
    admit: HashSet<i32>,
    frozen: bool,
}

impl RowLayout {
    pub fn rows(&self) -> &[RowView] {
        &self.shown
    }

    pub fn frozen(&self) -> bool {
        self.frozen
    }

    /// A new frame: `all` is every row it built, `fresh` the rows the list would show unfrozen.
    pub fn on_frame(&mut self, all: &[RowView], fresh: Vec<RowView>) {
        if !self.frozen {
            self.shown = fresh;
            return;
        }
        self.shown = hold(&self.shown, all, &fresh, &self.admit);
        self.admit.retain(|p| !fresh.iter().any(|r| r.parent == *p));
    }

    pub fn set_frozen(&mut self, frozen: bool, all: &[RowView], fresh: Vec<RowView>) {
        if frozen == self.frozen {
            return;
        }
        self.frozen = frozen;
        if frozen {
            self.on_frame(all, fresh);
        } else {
            self.admit.clear();
            self.shown = fresh;
        }
    }

    /// The user changed the search: rows move as the direct result of their typing.
    pub fn rebase(&mut self, fresh: Vec<RowView>) {
        self.admit.clear();
        self.shown = fresh;
    }

    /// The user expanded app `id`: its children appear right below it (now, or with the first
    /// frame that has them); nothing else moves.
    pub fn expand(&mut self, id: i32, all: &[RowView], fresh: Vec<RowView>) {
        if !self.frozen {
            self.shown = fresh;
            return;
        }
        self.admit.insert(id);
        self.on_frame(all, fresh);
    }

    /// The user collapsed app `id`: just its children go.
    pub fn collapse(&mut self, id: i32, all: &[RowView], fresh: Vec<RowView>) {
        if !self.frozen {
            self.shown = fresh;
            return;
        }
        self.admit.remove(&id);
        self.shown.retain(|r| r.parent != id);
        self.on_frame(all, fresh);
    }
}

/// Keeps `prev`'s ids at their indices, refreshing each from `all`; ids missing from `all` become
/// exited placeholders. New children of admitted apps go at the end of their app's block.
fn hold(prev: &[RowView], all: &[RowView], fresh: &[RowView], admit: &HashSet<i32>) -> Vec<RowView> {
    let live: HashMap<i32, &RowView> = all.iter().map(|r| (r.id, r)).collect();
    let prev_ids: HashSet<i32> = prev.iter().map(|r| r.id).collect();
    let mut out = Vec::with_capacity(prev.len());
    for (i, p) in prev.iter().enumerate() {
        out.push(match live.get(&p.id) {
            Some(r) => (*r).clone(),
            None => RowView {
                id: p.id,
                parent: p.parent,
                depth: p.depth,
                name: p.name.clone(),
                search_text: p.search_text.clone(),
                exited: true,
                actionable: false,
                ..Default::default()
            },
        });
        let owner = if p.depth == 0 { p.id } else { p.parent };
        let block_ends = prev.get(i + 1).is_none_or(|n| n.depth == 0);
        if block_ends && admit.contains(&owner) {
            out.extend(fresh.iter().filter(|r| r.parent == owner && !prev_ids.contains(&r.id)).cloned());
        }
    }
    out
}

/// Expands a never-actionable group row (the Windows group) the first time it reports a hung
/// member, so that member's own row, with its Force close, is in view. A collapse by the user
/// stands until that hang clears. Returns true if `expanded` changed.
fn auto_expand_hung_groups(rows: &[RowView], expanded: &mut HashSet<i32>, auto: &mut HashSet<i32>) -> bool {
    let mut changed = false;
    for r in rows.iter().filter(|r| r.depth == 0 && !r.actionable) {
        if r.hung.is_empty() {
            auto.remove(&r.id);
        } else if auto.insert(r.id) {
            changed |= expanded.insert(r.id);
        }
    }
    auto.retain(|id| rows.iter().any(|r| r.id == *id));
    changed
}

pub struct Bridge {
    meters: Rc<VecModel<MeterData>>,
    tiles: Rc<VecModel<TileData>>,
    rows: Rc<VecModel<RowData>>,
    perf_devices: Rc<VecModel<DeviceData>>,
    perf_graphs: Rc<VecModel<GraphData>>,
    perf_cells: Rc<VecModel<GraphData>>,
    perf_stats: Rc<VecModel<StatData>>,
    perf_segments: Rc<VecModel<SegmentData>>,
    /// The Performance frame on screen (the scrub reads its graph window).
    pub last_perf: Option<PerfFrame>,
    /// The worker's history, read (never waited for) to answer the scrub.
    history: Option<SharedHistory>,
    /// Where the pointer is across a Performance graph, while it hovers one.
    scrub_at: Option<f32>,
    scrub_rows: Rc<VecModel<ScrubRow>>,
    expanded: HashSet<i32>,
    /// Groups expanded automatically for their current hang (see `auto_expand_hung_groups`).
    auto_expanded: HashSet<i32>,
    search: String,
    last: Frame,
    layout: RowLayout,
    hover: bool,
    menu_open: bool,
    /// The row whose control (Force close, End task) has keyboard focus.
    key_focus: Option<i32>,
    /// Bumped by every focus change; a deferred focus-out applies only if nothing came after it.
    focus_generation: u64,
    /// The keyboard-current row (by id). Kept while focus is elsewhere, so Tab back resumes there.
    current: Option<i32>,
    /// The list itself has keyboard focus: it stays frozen, like on hover (ruling P3-7).
    list_focused: bool,
    /// Bumped by every list focus change; a deferred list focus-out applies only if none came after.
    list_generation: u64,
    banner_dismissed: bool,
    pub controls: Arc<Controls>,
    ui: slint::Weak<MainWindow>,
}

impl Bridge {
    pub fn install(ui: &MainWindow, controls: Arc<Controls>) -> Rc<RefCell<Bridge>> {
        let b = Bridge {
            meters: Rc::new(VecModel::default()),
            tiles: Rc::new(VecModel::default()),
            rows: Rc::new(VecModel::default()),
            perf_devices: Rc::new(VecModel::default()),
            perf_graphs: Rc::new(VecModel::default()),
            perf_cells: Rc::new(VecModel::default()),
            perf_stats: Rc::new(VecModel::default()),
            perf_segments: Rc::new(VecModel::default()),
            last_perf: None,
            history: None,
            scrub_at: None,
            scrub_rows: Rc::new(VecModel::default()),
            expanded: HashSet::new(),
            auto_expanded: HashSet::new(),
            search: String::new(),
            last: Frame::default(),
            layout: RowLayout::default(),
            hover: false,
            menu_open: false,
            key_focus: None,
            focus_generation: 0,
            current: None,
            list_focused: false,
            list_generation: 0,
            banner_dismissed: false,
            controls,
            ui: ui.as_weak(),
        };
        ui.set_meters(ModelRc::from(b.meters.clone()));
        ui.set_tiles(ModelRc::from(b.tiles.clone()));
        ui.set_rows(ModelRc::from(b.rows.clone()));
        ui.set_perf_devices(ModelRc::from(b.perf_devices.clone()));
        ui.set_perf_graphs(ModelRc::from(b.perf_graphs.clone()));
        ui.set_perf_cells(ModelRc::from(b.perf_cells.clone()));
        ui.set_perf_stats(ModelRc::from(b.perf_stats.clone()));
        ui.set_perf_segments(ModelRc::from(b.perf_segments.clone()));
        ui.set_scrub_rows(ModelRc::from(b.scrub_rows.clone()));
        Rc::new(RefCell::new(b))
    }

    /// The user picked `page` on the rail.
    pub fn set_page(&mut self, ui: &MainWindow, page: Page) {
        self.controls.set_page(page);
        if page != Page::Overview {
            // The hidden list can't report pointer-leave: release the hover freeze here.
            self.set_hover(false);
        }
        if page != Page::Performance {
            // Likewise the hidden graphs: end any scrub.
            self.scrub(ui, -1.0);
        }
        ui.set_page(page.index());
    }

    pub fn apply(&mut self, ui: &MainWindow, frame: Frame) {
        // Built for a page the user has since left: the next frame is already on its way.
        if frame.page != self.controls.page() {
            return;
        }
        match frame.page {
            Page::Overview => self.apply_overview(ui, frame),
            Page::Performance => {
                if let Some(perf) = frame.perf {
                    self.apply_perf(ui, perf);
                }
            }
        }
    }

    fn apply_perf(&mut self, ui: &MainWindow, p: PerfFrame) {
        let (device, range) = self.controls.perf();
        if p.requested != device || p.range != range {
            return; // asked for before the user picked another device or range
        }
        if p.selected != device {
            self.controls.adopt_device(&p.selected); // that device is gone: CPU stands in, and stays
        }
        sync_model(&*self.perf_devices, p.devices.iter().map(|d| DeviceData {
            id: d.id.as_str().into(), title: d.title.as_str().into(), value: d.value.as_str().into(), spark: d.spark.as_str().into(),
        }).collect());
        ui.set_perf_selected(p.selected.as_str().into());
        ui.set_perf_title(p.title.as_str().into());
        ui.set_perf_subtitle(p.subtitle.as_str().into());
        let graph = |g: &GraphView| GraphData {
            label: g.label.as_str().into(), value: g.value.as_str().into(), scale: g.scale.as_str().into(),
            line: g.line.as_str().into(), area: g.area.as_str().into(), line2: g.line2.as_str().into(),
        };
        sync_model(&*self.perf_graphs, p.graphs.iter().map(graph).collect());
        sync_model(&*self.perf_cells, p.cells.iter().map(graph).collect());
        sync_model(&*self.perf_stats, p.stats.iter().map(|s| StatData { label: s.label.as_str().into(), value: s.value.as_str().into() }).collect());
        sync_model(&*self.perf_segments, p.segments.iter().map(|s| SegmentData {
            label: s.label.as_str().into(), value: s.value.as_str().into(), fraction: s.fraction,
        }).collect());
        ui.set_perf_note(p.note.as_str().into());
        ui.set_perf_range(p.range.index());
        self.last_perf = Some(p);
        // The window moved under a still pointer: the card follows it to the new moment.
        self.show_scrub(ui);
    }

    pub fn set_history(&mut self, history: SharedHistory) {
        self.history = Some(history);
    }

    /// The pointer is `fraction` of the way across a Performance graph (-1: it left).
    pub fn scrub(&mut self, ui: &MainWindow, fraction: f32) {
        self.scrub_at = (fraction.is_finite() && fraction >= 0.0).then_some(fraction.min(1.0));
        self.show_scrub(ui);
    }

    /// Runs on the UI thread, which never waits for the worker: while the worker holds the history
    /// (or a panic left it poisoned, until the restarted worker resets it) the current card stays.
    fn show_scrub(&self, ui: &MainWindow) {
        let (Some(f), Some(perf), Some(history)) = (self.scrub_at, self.last_perf.as_ref(), self.history.as_ref()) else {
            ui.set_scrub_x(-1.0);
            return;
        };
        let Ok(h) = history.try_lock() else {
            return;
        };
        let view = crate::scrub::scrub(&h, perf, f);
        drop(h);
        ui.set_scrub_title(view.title.into());
        ui.set_scrub_empty(view.empty.into());
        sync_model(&*self.scrub_rows, view.rows.into_iter().map(|(name, value)| ScrubRow { name: name.into(), value: value.into() }).collect());
        ui.set_scrub_x(f);
    }

    fn apply_overview(&mut self, ui: &MainWindow, frame: Frame) {
        sync_model(&*self.meters, frame.meters.iter().map(|m| MeterData {
            label: m.label.into(), value: m.value.clone().into(), sub: m.sub.clone().into(), spark: m.spark.clone().into(), available: m.available,
        }).collect());
        sync_model(&*self.tiles, frame.tiles.iter().map(|t| TileData {
            id: t.id, name: t.name.clone().into(), value: t.value.clone().into(), weight: t.weight, hung: t.hung,
        }).collect());
        ui.set_top_caption(frame.caption.clone().into());
        ui.set_show_admin_banner(frame.admin_banner && !self.banner_dismissed);
        self.last = frame;
        // While frozen, newly shown children would be deferred anyway: expand on unfreeze instead.
        if !self.layout.frozen() {
            self.auto_expand();
        }
        let fresh = self.fresh();
        self.layout.on_frame(&self.last.rows, fresh);
        // The frame may have removed the keyboard-focused control (its row recovered or exited).
        self.update_frozen();
        self.push_rows();
    }

    fn auto_expand(&mut self) {
        if auto_expand_hung_groups(&self.last.rows, &mut self.expanded, &mut self.auto_expanded) {
            self.publish_child_rows();
        }
    }

    /// Tells the worker which apps need child rows (expanded ones; all of them while searching).
    fn publish_child_rows(&self) {
        self.controls.set_child_rows(&self.expanded, !self.search.trim().is_empty());
    }

    fn fresh(&self) -> Vec<RowView> {
        visible_rows(&self.last.rows, &self.expanded, &self.search)
    }

    fn push_rows(&self) {
        sync_model(&*self.rows, self.layout.rows().iter().map(|r| RowData {
            id: r.id, depth: r.depth, name: r.name.clone().into(), count: r.count.clone().into(), cpu: r.cpu.clone().into(),
            mem: r.mem.clone().into(), disk: r.disk.clone().into(), gpu: r.gpu.clone().into(), net: r.net.clone().into(),
            bar: r.bar, hung: r.hung.clone().into(), expandable: r.expandable, expanded: self.expanded.contains(&r.id),
            actionable: r.actionable, exited: r.exited,
        }).collect());
        self.sync_current();
    }

    pub fn set_search(&mut self, text: &str) {
        self.search = text.to_string();
        self.publish_child_rows();
        let fresh = self.fresh();
        self.layout.rebase(fresh);
        self.push_rows();
    }

    pub fn toggle(&mut self, id: i32) {
        let expanding = !self.expanded.remove(&id);
        if expanding {
            self.expanded.insert(id);
        }
        self.publish_child_rows();
        let fresh = self.fresh();
        if expanding {
            self.layout.expand(id, &self.last.rows, fresh);
        } else {
            self.layout.collapse(id, &self.last.rows, fresh);
        }
        self.push_rows();
    }

    /// A top-strip tile: clears the search and expands that app.
    pub fn open_tile(&mut self, id: i32) {
        self.search.clear(); // the box itself is emptied by main.slint in the same click
        self.expanded.insert(id);
        self.publish_child_rows();
        let fresh = self.fresh();
        self.layout.rebase(fresh);
        self.push_rows();
        if let (Some(i), Some(ui)) = (self.layout.rows().iter().position(|r| r.id == id), self.ui.upgrade()) {
            ui.invoke_ensure_row_visible(i as i32);
        }
    }

    pub fn set_hover(&mut self, hover: bool) {
        self.hover = hover;
        self.update_frozen();
    }

    /// Keyboard focus entered or left a control in row `id`. Mirrors the hover freeze, so Space on
    /// a focused Force close can't land on another app that moved into that index.
    pub fn set_key_focus(&mut self, id: i32, focused: bool) {
        if focused {
            self.key_focus = Some(id);
        } else if self.key_focus == Some(id) {
            self.key_focus = None;
        }
        self.update_frozen();
    }

    /// Whether the focused control still exists. Force close shows on every hung actionable row;
    /// End task only on the hovered one. A control destroyed while focused sends no focus-out,
    /// so this check is what releases the freeze then.
    fn key_focus_holds(&self) -> bool {
        let Some(id) = self.key_focus else { return false };
        self.layout.rows().iter().any(|r| {
            r.id == id && r.actionable && !r.exited && (!r.hung.is_empty() || (self.hover && r.depth == 0))
        })
    }

    /// A modal opened: its controls are disabled, and a disabled Button or FocusScope never sees
    /// the focus-out, so forget the keyboard focus (a row control's and the list's) here, and drop
    /// any pending release.
    pub fn clear_key_focus(&mut self) {
        self.focus_generation += 1;
        self.key_focus = None;
        self.list_generation += 1;
        self.list_focused = false;
        self.update_frozen();
    }

    /// The list itself gained or lost keyboard focus. Gaining it starts at the first row unless a
    /// current row is still shown, and brings that row into view.
    pub fn set_list_focus(&mut self, focused: bool) {
        self.list_focused = focused;
        if focused && self.current_index().is_none() {
            self.current = self.layout.rows().first().map(|r| r.id);
        }
        self.update_frozen();
        self.sync_current();
        if focused {
            self.reveal_current();
        }
    }

    /// A navigation key while the list has focus. Moves and expand/collapse happen here; an action
    /// is returned for the caller to run through the window's row-action handler. Every outcome is
    /// decided by the current row's id, never by an index.
    pub fn list_key(&mut self, key: NavKey) -> NavOutcome {
        // The key came from the focused list: a focus edge that never arrived (a modal opened
        // without parking focus first) must not leave it acting on an unfrozen list.
        let reasserted = !self.list_focused;
        if reasserted {
            self.set_list_focus(true);
        }
        let mut outcome = navigate(self.layout.rows(), &self.expanded, self.current, key);
        // The ring only appears with this key (and may have moved to another app if the current
        // one left meanwhile): never act on a row the user has not yet seen highlighted.
        if reasserted && matches!(outcome, NavOutcome::Action(..)) {
            outcome = NavOutcome::Nothing;
        }
        match outcome {
            NavOutcome::Move(id) => {
                self.current = Some(id);
                self.sync_current();
                self.reveal_current();
            }
            NavOutcome::Toggle(id) => {
                self.current = Some(id);
                self.toggle(id);
                self.reveal_current();
            }
            NavOutcome::Action(..) | NavOutcome::Nothing => {}
        }
        outcome
    }

    fn current_index(&self) -> Option<usize> {
        self.current.and_then(|id| self.layout.rows().iter().position(|r| r.id == id))
    }

    /// Tells the list which row is current (its index moves when rows expand or collapse). Never
    /// scrolls (ruling PF1): a frame must not pull a wheel-scrolled list back to the current row.
    fn sync_current(&self) {
        let Some(ui) = self.ui.upgrade() else { return };
        let rows = self.layout.rows();
        let index = self.current_index();
        ui.set_current_row(index.map_or(-1, |i| i as i32));
        ui.set_current_id(self.current.unwrap_or(-1));
        ui.set_current_actionable(index.is_some_and(|i| rows[i].actionable && !rows[i].exited));
    }

    /// Scrolls the current row into view: only for the user's own keyboard moves (ruling PF1).
    fn reveal_current(&self) {
        if let (Some(i), Some(ui)) = (self.current_index(), self.ui.upgrade()) {
            ui.invoke_ensure_row_visible(i as i32);
        }
    }

    pub fn set_menu_open(&mut self, open: bool) {
        self.menu_open = open;
        self.update_frozen();
    }

    fn update_frozen(&mut self) {
        if !self.key_focus_holds() {
            self.key_focus = None;
        }
        let frozen = self.hover || self.menu_open || self.key_focus.is_some() || self.list_focused;
        self.controls.set_frozen(frozen);
        if frozen != self.layout.frozen() {
            if !frozen {
                self.auto_expand();
            }
            let fresh = self.fresh();
            self.layout.set_frozen(frozen, &self.last.rows, fresh);
            self.push_rows();
        }
    }

    pub fn dismiss_banner(&mut self) {
        self.banner_dismissed = true;
        if let Some(ui) = self.ui.upgrade() {
            ui.set_show_admin_banner(false);
        }
    }

    /// The action target for a row id; `None` once that row's app or process has exited.
    pub fn target(&self, id: i32) -> Option<Target> {
        self.last.targets.get(&id).cloned()
    }

    pub fn all_procs(&self) -> &[(u32, i64, u32)] {
        &self.last.all_procs
    }
}

thread_local! {
    static CURRENT: RefCell<Option<Rc<RefCell<Bridge>>>> = const { RefCell::new(None) };
}

/// Applies a keyboard focus change (ruling R11): a focus-in at once; a focus-out on the next turn
/// of the event loop, dropped if another focus change on the same counter came meanwhile. Tabbing
/// from one control to the next delivers the old one's focus-out first, and releasing the freeze
/// in between would let a pending re-sort move rows under the newly focused control.
fn deferred_focus(
    bridge: &Rc<RefCell<Bridge>>,
    focused: bool,
    counter: fn(&mut Bridge) -> &mut u64,
    apply: impl FnOnce(&mut Bridge, bool) + 'static,
) {
    let generation = {
        let mut b = bridge.borrow_mut();
        let count = counter(&mut b);
        *count += 1;
        let generation = *count;
        if focused {
            apply(&mut b, true);
            return;
        }
        generation
    };
    let b = bridge.clone();
    slint::Timer::single_shot(std::time::Duration::ZERO, move || {
        let mut b = b.borrow_mut();
        if *counter(&mut b) == generation {
            apply(&mut b, false);
        }
    });
}

/// Keyboard focus entered or left a control in row `id` (the UI's `row-key-focus`); see
/// `deferred_focus`.
pub fn row_focus(bridge: &Rc<RefCell<Bridge>>, id: i32, focused: bool) {
    deferred_focus(bridge, focused, |b| &mut b.focus_generation, move |b, f| b.set_key_focus(id, f));
}

/// The list gained or lost keyboard focus (the UI's `list-focus`); see `deferred_focus`. A focus
/// moving from the list to a row control meanwhile keeps the list frozen through `key_focus`.
pub fn list_focus(bridge: &Rc<RefCell<Bridge>>, focused: bool) {
    deferred_focus(bridge, focused, |b| &mut b.list_generation, Bridge::set_list_focus);
}

/// A key pressed while the list itself has focus (the UI's `list-key`). An action (Delete: End
/// task) runs through the window's row-action handler, like a click, after the bridge borrow is
/// released (that handler borrows it again). Returns true when it was a navigation key.
pub fn handle_list_key(bridge: &Rc<RefCell<Bridge>>, ui: &MainWindow, text: &str) -> bool {
    let Some(key) = NavKey::from_text(text) else { return false };
    let outcome = bridge.borrow_mut().list_key(key);
    if let NavOutcome::Action(name, id) = outcome {
        ui.invoke_row_action(name.into(), id);
    }
    true
}

pub fn set_current(b: Rc<RefCell<Bridge>>) {
    CURRENT.with(|c| *c.borrow_mut() = Some(b));
}

pub fn with_bridge(f: impl FnOnce(&mut Bridge)) {
    CURRENT.with(|c| {
        if let Some(b) = c.borrow().as_ref() {
            f(&mut b.borrow_mut());
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// Records every operation so the test can prove updates are minimal and in place.
    #[derive(Default)]
    struct Recording {
        items: RefCell<Vec<i32>>,
        ops: RefCell<Vec<String>>,
    }

    impl RowStore<i32> for Recording {
        fn len(&self) -> usize { self.items.borrow().len() }
        fn get(&self, i: usize) -> Option<i32> { self.items.borrow().get(i).copied() }
        fn set(&self, i: usize, v: i32) { self.ops.borrow_mut().push(format!("set {i}")); self.items.borrow_mut()[i] = v; }
        fn push(&self, v: i32) { self.ops.borrow_mut().push("push".into()); self.items.borrow_mut().push(v); }
        fn pop(&self) { self.ops.borrow_mut().push("pop".into()); self.items.borrow_mut().pop(); }
    }

    #[test]
    fn sync_updates_only_changed_rows_in_place() {
        let store = Recording::default();
        *store.items.borrow_mut() = vec![1, 2, 3, 4];
        sync_model(&store, vec![1, 9, 3]);
        assert_eq!(*store.items.borrow(), [1, 9, 3]);
        assert_eq!(*store.ops.borrow(), ["set 1", "pop"], "unchanged rows are untouched");
        store.ops.borrow_mut().clear();
        sync_model(&store, vec![1, 9, 3, 5, 6]);
        assert_eq!(*store.ops.borrow(), ["push", "push"]);
    }

    fn app(id: i32, cpu: &str) -> RowView {
        RowView { id, parent: -1, depth: 0, name: format!("app{id}"), cpu: cpu.into(), actionable: true, ..Default::default() }
    }
    fn child(id: i32, parent: i32) -> RowView {
        RowView { id, parent, depth: 1, name: format!("proc{id}"), cpu: "0.0%".into(), actionable: true, ..Default::default() }
    }
    fn ids(rows: &[RowView]) -> Vec<i32> {
        rows.iter().map(|r| r.id).collect()
    }

    #[test]
    fn frozen_rows_keep_their_index_while_apps_exit_and_appear() {
        let mut layout = RowLayout::default();
        // X, Y (expanded: Y1, Y2), Z.
        let first = vec![app(1, "5.0%"), app(2, "4.0%"), child(3, 2), child(4, 2), app(5, "1.0%")];
        layout.on_frame(&first, first.clone());
        layout.set_frozen(true, &first, first.clone());
        // Next frame: X exited, Y gained a child (Y3), a new app W appeared, Z's values changed.
        let second = vec![app(2, "4.0%"), child(3, 2), child(4, 2), child(6, 2), app(5, "9.0%"), app(7, "8.0%")];
        layout.on_frame(&second, second.clone());
        let shown = layout.rows();
        assert_eq!(ids(shown), [1, 2, 3, 4, 5], "every index keeps its id; new rows wait");
        assert!(shown[0].exited && !shown[0].actionable, "the exited app stays in place, inert");
        assert!(shown[1..].iter().all(|r| r.actionable && !r.exited));
        assert_eq!(shown[4].cpu, "9.0%", "surviving rows keep updating");
        // Pointer leaves: the list catches up at once.
        layout.set_frozen(false, &second, second.clone());
        assert_eq!(ids(layout.rows()), [2, 3, 4, 6, 5, 7]);
    }

    #[test]
    fn expanding_while_frozen_admits_only_that_apps_children() {
        let mut layout = RowLayout::default();
        let rows = vec![app(1, "1%"), app(2, "1%")];
        layout.on_frame(&rows, rows.clone());
        layout.set_frozen(true, &rows, rows.clone());
        // The user expands app 2; its children arrive with the next frame, as does a new app 9.
        layout.expand(2, &rows, rows.clone());
        let next = vec![app(1, "1%"), app(2, "1%"), child(3, 2), child(4, 2), app(9, "1%")];
        let visible = next.clone();
        layout.on_frame(&next, visible);
        assert_eq!(ids(layout.rows()), [1, 2, 3, 4], "the expanded app's children appear; the new app waits");
        // A later child of the same app is a new row like any other: deferred.
        let later = vec![app(1, "1%"), app(2, "1%"), child(3, 2), child(4, 2), child(5, 2), app(9, "1%")];
        layout.on_frame(&later, later.clone());
        assert_eq!(ids(layout.rows()), [1, 2, 3, 4]);
        // Collapsing removes just those children, in place.
        let collapsed = vec![app(1, "1%"), app(2, "1%"), app(9, "1%")];
        layout.collapse(2, &later, collapsed);
        assert_eq!(ids(layout.rows()), [1, 2]);
    }

    #[test]
    fn a_hung_windows_group_expands_once_and_a_collapse_sticks_until_the_hang_clears() {
        let group = |hung: &str| RowView { id: 1, parent: -1, depth: 0, name: "Windows".into(), hung: hung.into(), ..Default::default() };
        let (mut expanded, mut auto) = (HashSet::new(), HashSet::new());
        assert!(!auto_expand_hung_groups(&[group(""), app(2, "1%")], &mut expanded, &mut auto));
        assert!(auto_expand_hung_groups(&[group("Not responding · 9s")], &mut expanded, &mut auto));
        assert!(expanded.contains(&1));
        expanded.remove(&1); // the user collapses it
        assert!(!auto_expand_hung_groups(&[group("Not responding · 10s")], &mut expanded, &mut auto));
        assert!(!expanded.contains(&1), "still the same hang: the user's collapse stands");
        auto_expand_hung_groups(&[group("")], &mut expanded, &mut auto);
        assert!(auto_expand_hung_groups(&[group("Not responding · 1s")], &mut expanded, &mut auto), "a new hang expands again");
        // Ordinary (actionable) apps are never expanded automatically: their row has Force close.
        let mut hung_app = app(5, "1%");
        hung_app.hung = "Not responding · 3s".into();
        assert!(!auto_expand_hung_groups(&[hung_app], &mut expanded, &mut auto));
    }

    #[test]
    fn vec_model_store_round_trips() {
        let model = VecModel::from(vec![1, 2]);
        sync_model(&model, vec![5, 2, 7]);
        assert_eq!((0..RowStore::len(&model)).map(|i| RowStore::get(&model, i).unwrap()).collect::<Vec<_>>(), [5, 2, 7]);
    }
}
