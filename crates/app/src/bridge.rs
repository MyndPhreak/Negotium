//! UI-thread glue: applies worker Frames to Slint models in place and keeps
//! expand/search state local so they react instantly. No Windows calls here.
use crate::controls::Controls;
use crate::view::{Frame, Target, visible_rows};
use crate::{MainWindow, MeterData, RowData, TileData};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::cell::RefCell;
use std::collections::HashSet;
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

pub struct Bridge {
    meters: Rc<VecModel<MeterData>>,
    tiles: Rc<VecModel<TileData>>,
    rows: Rc<VecModel<RowData>>,
    pub expanded: HashSet<i32>,
    pub search: String,
    last: Frame,
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
            search: String::new(),
            last: Frame::default(),
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
        self.refresh_rows();
    }

    pub fn refresh_rows(&self) {
        let visible = visible_rows(&self.last.rows, &self.expanded, &self.search);
        sync_model(&*self.rows, visible.into_iter().map(|r| RowData {
            id: r.id, depth: r.depth, name: r.name.into(), count: r.count.into(), cpu: r.cpu.into(), mem: r.mem.into(),
            disk: r.disk.into(), gpu: r.gpu.into(), net: r.net.into(), bar: r.bar, hung: r.hung.into(),
            expandable: r.expandable, expanded: self.expanded.contains(&r.id),
        }).collect());
    }

    pub fn dismiss_banner(&mut self) {
        self.banner_dismissed = true;
        if let Some(ui) = self.ui.upgrade() {
            ui.set_show_admin_banner(false);
        }
    }

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

    #[test]
    fn vec_model_store_round_trips() {
        let model = VecModel::from(vec![1, 2]);
        sync_model(&model, vec![5, 2, 7]);
        assert_eq!((0..RowStore::len(&model)).map(|i| RowStore::get(&model, i).unwrap()).collect::<Vec<_>>(), [5, 2, 7]);
    }
}
