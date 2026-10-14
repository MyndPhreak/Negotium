//! UI-thread glue: applies worker Frames to Slint models in place and keeps
//! expand/search state local so they react instantly. No Windows calls here.
use crate::controls::Controls;
use crate::view::{Frame, RowView, Target, visible_rows};
use crate::{MainWindow, MeterData, RowData, TileData};
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
            expanded: HashSet::new(),
            auto_expanded: HashSet::new(),
            search: String::new(),
            last: Frame::default(),
            layout: RowLayout::default(),
            hover: false,
            menu_open: false,
            key_focus: None,
            banner_dismissed: false,
            controls,
            ui: ui.as_weak(),
        };
        ui.set_meters(ModelRc::from(b.meters.clone()));
        ui.set_tiles(ModelRc::from(b.tiles.clone()));
        ui.set_rows(ModelRc::from(b.rows.clone()));
        Rc::new(RefCell::new(b))
    }

    pub fn apply(&mut self, ui: &MainWindow, frame: Frame) {
        sync_model(&*self.meters, frame.meters.iter().map(|m| MeterData {
            label: m.label.into(), value: m.value.clone().into(), sub: m.sub.clone().into(), spark: m.spark.clone().into(), available: m.available,
        }).collect());
        sync_model(&*self.tiles, frame.tiles.iter().map(|t| TileData {
            id: t.id, name: t.name.clone().into(), value: t.value.clone().into(), weight: t.weight, hung: t.hung,
        }).collect());
        ui.set_top_caption(frame.caption.clone().into());
        ui.set_show_admin_banner(frame.admin_banner && !self.banner_dismissed);
        self.last = frame;
        if auto_expand_hung_groups(&self.last.rows, &mut self.expanded, &mut self.auto_expanded) {
            self.publish_child_rows();
        }
        let fresh = self.fresh();
        self.layout.on_frame(&self.last.rows, fresh);
        // The frame may have removed the keyboard-focused control (its row recovered or exited).
        self.update_frozen();
        self.push_rows();
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

    pub fn set_menu_open(&mut self, open: bool) {
        self.menu_open = open;
        self.update_frozen();
    }

    fn update_frozen(&mut self) {
        if !self.key_focus_holds() {
            self.key_focus = None;
        }
        let frozen = self.hover || self.menu_open || self.key_focus.is_some();
        self.controls.set_frozen(frozen);
        if frozen != self.layout.frozen() {
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
