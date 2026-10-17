//! UI-thread glue: applies worker Frames to Slint models in place and keeps
//! expand/search state local so they react instantly. No Windows calls here.
use crate::controls::{Controls, Page};
use crate::icons::{IconCache, IconPixels, RequestIcon};
use crate::nav::{NavKey, NavOutcome, navigate};
use crate::perf::{GraphView, PerfFrame};
use crate::smooth::{self, Scroll};
use crate::view::{Frame, RowKind, RowView, SECTION_APPS, Target, close_cards, row_height, visible_rows};
use crate::worker::SharedHistory;
use crate::{DeviceData, GraphClock, GraphData, MainWindow, MeterData, RowData, ScrubRow, SegmentData, StartupData, StatData, TileData};
use negotium_actions::startup::StartupEntry;
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

/// Minimal row storage the sync can use. Deliberately has no "replace all".
#[allow(clippy::len_without_is_empty)] // internal row-store abstraction; emptiness is never queried
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

/// RowData's `kind` (applist.slint): 0 a section heading, 1 an app's card, 2 a process row.
fn kind_index(kind: RowKind) -> i32 {
    match kind {
        RowKind::Section => 0,
        RowKind::App => 1,
        RowKind::Proc => 2,
    }
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

/// Applies a frame's top tiles (in rank order) so each app keeps its tile across frames: the
/// strip animates only what changed. Smoothly, an app leaving the top keeps its place for one
/// frame marked `leaving` (shrinking away) and a newcomer is inserted next to the tile ranked
/// just above it (growing in). Apps that stay but swap ranks trade tiles in place; by then their
/// averaged widths are about equal, so only the labels change. Without smoothing tiles are simply
/// replaced in order.
pub fn sync_tiles(model: &VecModel<TileData>, tiles: Vec<TileData>, smooth: bool) {
    for i in (0..model.row_count()).rev() {
        if model.row_data(i).is_some_and(|t| t.leaving) {
            model.remove(i);
        }
    }
    if !smooth {
        sync_model(model, tiles);
        return;
    }
    let ids: HashSet<i32> = tiles.iter().map(|t| t.id).collect();
    let mut kept = HashSet::new();
    for i in 0..model.row_count() {
        let Some(mut t) = model.row_data(i) else { continue };
        if ids.contains(&t.id) {
            kept.insert(t.id);
        } else {
            t.leaving = true;
            t.weight = 0.0;
            model.set_row_data(i, t);
        }
    }
    // The staying apps, in rank order, into the staying tiles in strip order.
    let mut staying = tiles.iter().filter(|t| kept.contains(&t.id));
    for i in 0..model.row_count() {
        if model.row_data(i).is_some_and(|t| !t.leaving) {
            // One each, unless ids repeat; never panic on the UI thread over it.
            let Some(next) = staying.next().cloned() else { break };
            if model.row_data(i).as_ref() != Some(&next) {
                model.set_row_data(i, next);
            }
        }
    }
    // Newcomers after the tile ranked just above them (already placed: rank order).
    for (r, t) in tiles.iter().enumerate() {
        if kept.contains(&t.id) {
            continue;
        }
        let at = match r.checked_sub(1) {
            None => 0,
            Some(prev) => {
                let above = tiles[prev].id;
                (0..model.row_count()).find(|&i| model.row_data(i).is_some_and(|m| !m.leaving && m.id == above)).map_or(model.row_count(), |i| i + 1)
            }
        };
        model.insert(at, t.clone());
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

    /// The user closed section `id`: just its block (cards and their processes) goes. The list is
    /// frozen whenever it has keyboard focus, so this is how a heading closes from the keyboard.
    pub fn close_section(&mut self, id: i32, all: &[RowView], fresh: Vec<RowView>) {
        if !self.frozen {
            self.shown = fresh;
            return;
        }
        if let Some(at) = self.shown.iter().position(|r| r.id == id) {
            let end = section_end(&self.shown, at);
            let gone: HashSet<i32> = self.shown.drain(at + 1..end).map(|r| r.id).collect();
            self.admit.retain(|a| !gone.contains(a));
        }
        self.on_frame(all, fresh);
    }

    /// The user opened section `id`: its block as it would show unfrozen goes in right below the
    /// heading; nothing else moves, and new apps elsewhere keep waiting.
    pub fn open_section(&mut self, id: i32, all: &[RowView], fresh: Vec<RowView>) {
        if !self.frozen {
            self.shown = fresh;
            return;
        }
        if let (Some(at), Some(from)) = (self.shown.iter().position(|r| r.id == id), fresh.iter().position(|r| r.id == id)) {
            let end = section_end(&self.shown, at);
            let have: HashSet<i32> = self.shown.iter().map(|r| r.id).collect();
            let block: Vec<RowView> = fresh[from + 1..section_end(&fresh, from)].iter().filter(|r| !have.contains(&r.id)).cloned().collect();
            self.shown.splice(end..end, block);
        }
        self.on_frame(all, fresh);
    }
}

/// Where the block of the heading at `at` ends: the next heading, or the end of the list.
fn section_end(rows: &[RowView], at: usize) -> usize {
    rows[at + 1..].iter().position(|r| r.kind == RowKind::Section).map_or(rows.len(), |p| at + 1 + p)
}

/// Section heading ids sit at the top of the id range (crate::view::SECTION_APPS).
fn is_section(id: i32) -> bool {
    id >= SECTION_APPS
}

/// The y of row `i` within the list and its height, by its kind: rows differ in height (headings,
/// cards, process rows), so neither is a multiple of one row height.
pub fn row_top(rows: &[RowView], i: usize) -> (f32, f32) {
    let y = rows.iter().take(i).map(|r| row_height(r.kind)).sum();
    (y, rows.get(i).map_or(0.0, |r| row_height(r.kind)))
}

/// Keeps `prev`'s ids at their indices, refreshing each from `fresh` (a heading's count is worked
/// out there), else from `all`; ids missing from both become exited placeholders. New children of
/// admitted apps go at the end of their app's block.
/// Which process closes each card is then worked out again from this order (`close_cards`): the
/// frame's flag belongs to the frame's order.
fn hold(prev: &[RowView], all: &[RowView], fresh: &[RowView], admit: &HashSet<i32>) -> Vec<RowView> {
    let live: HashMap<i32, &RowView> = all.iter().chain(fresh).map(|r| (r.id, r)).collect();
    let prev_ids: HashSet<i32> = prev.iter().map(|r| r.id).collect();
    let mut out = Vec::with_capacity(prev.len());
    for (i, p) in prev.iter().enumerate() {
        out.push(match live.get(&p.id) {
            Some(r) => (*r).clone(),
            None => RowView {
                id: p.id,
                parent: p.parent,
                depth: p.depth,
                kind: p.kind,
                name: p.name.clone(),
                search_text: p.search_text.clone(),
                icon: p.icon.clone(),
                // Its card still draws as it did.
                expandable: p.expandable,
                windows: p.windows,
                pid: p.pid.clone(),
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
    close_cards(&mut out);
    out
}

/// Expands a never-actionable group row (the Windows group) the first time it reports a hung
/// member, so that member's own row, with its Force close, is in view. A collapse by the user
/// stands until that hang clears. Returns true if `expanded` changed.
fn auto_expand_hung_groups(rows: &[RowView], expanded: &mut HashSet<i32>, auto: &mut HashSet<i32>) -> bool {
    let mut changed = false;
    for r in rows.iter().filter(|r| r.kind == RowKind::App && r.depth == 0 && !r.actionable) {
        if r.hung.is_empty() {
            auto.remove(&r.id);
        } else if auto.insert(r.id) {
            changed |= expanded.insert(r.id);
        }
    }
    auto.retain(|id| rows.iter().any(|r| r.id == *id));
    changed
}

/// Opens a closed section the first time one of its apps hangs, so that app's Force close is in
/// view; the same rule as `auto_expand_hung_groups`: closed again by the user, it stays closed
/// until the section has no hung app. Returns true if `collapsed` changed.
pub fn reopen_hung_sections(rows: &[RowView], collapsed: &mut HashSet<i32>, reopened: &mut HashSet<i32>) -> bool {
    let mut hung: HashMap<i32, bool> = HashMap::new();
    let mut section = None;
    for r in rows {
        if r.kind == RowKind::Section {
            section = Some(r.id);
            hung.insert(r.id, false);
        } else if let Some(s) = section.filter(|_| r.kind == RowKind::App && r.depth == 0 && !r.hung.is_empty()) {
            hung.insert(s, true);
        }
    }
    let mut changed = false;
    for (&id, &h) in &hung {
        if !h {
            reopened.remove(&id);
        } else if reopened.insert(id) {
            changed |= collapsed.remove(&id);
        }
    }
    reopened.retain(|id| hung.contains_key(id));
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
    /// The card as last built, with the fraction it was built for: a still pointer on a scrolling
    /// graph asks again every tick, and the card is rebuilt only when what it says changed (PF6).
    scrub_shown: Option<(f32, crate::scrub::ScrubView)>,
    scrub_builds: u32,
    scrub_rows: Rc<VecModel<ScrubRow>>,
    /// Collector time counts from here (the worker's epoch), so the scrolling view can be placed
    /// against real time. Shared with `GraphClock.clock` (crate::smooth::install), so Slint moves
    /// the view by the same clock.
    epoch: Rc<Cell<Instant>>,
    /// Collector time fixed here instead of read from `epoch` (tests: edges that don't depend on
    /// wall time).
    fixed_now_ms: Option<f64>,
    /// The scrolling graphs' current run (crate::smooth): resting on its newest sample (speed 0)
    /// while graphs don't scroll; None before the first graph frame.
    scroll: Option<Scroll>,
    expanded: HashSet<i32>,
    /// Groups expanded automatically for their current hang (see `auto_expand_hung_groups`).
    auto_expanded: HashSet<i32>,
    /// Sections the user closed. Never published to the worker: a closed section's apps are built
    /// as usual, only not shown. Not saved: every section starts open.
    collapsed: HashSet<i32>,
    /// Sections opened automatically for their current hang (see `reopen_hung_sections`).
    reopened: HashSet<i32>,
    search: String,
    last: Frame,
    layout: RowLayout,
    hover: bool,
    menu_open: bool,
    /// The row whose control (Force close, End task) has keyboard focus.
    key_focus: Option<i32>,
    /// Whether that row was hung when its control took focus: the control is its Force close,
    /// not its End task.
    key_focus_hung: bool,
    /// Bumped by every focus change; a deferred focus-out applies only if nothing came after it.
    focus_generation: u64,
    /// The keyboard-current row (by id). Kept while focus is elsewhere, so Tab back resumes there.
    current: Option<i32>,
    /// The list itself has keyboard focus: it stays frozen, like on hover (ruling P3-7).
    list_focused: bool,
    /// Bumped by every list focus change; a deferred list focus-out applies only if none came after.
    list_generation: u64,
    banner_dismissed: bool,
    /// The startup entries as last listed; a Startup row's id indexes them.
    startup: Vec<StartupEntry>,
    startup_rows: Rc<VecModel<StartupData>>,
    /// Program icons by image path, and where the ones not yet seen are requested (none in
    /// tests that don't set one: rows then show no icons).
    icons: IconCache,
    icon_loader: Option<Box<dyn RequestIcon>>,
    /// What a row without an icon carries: one image shared by every such row, because Slint's
    /// empty image never equals itself and would make `sync_model` rewrite those rows every frame.
    no_icon: slint::Image,
    /// The bars of every row without any (see `bars_model`).
    no_bars: ModelRc<f32>,
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
            scrub_shown: None,
            scrub_builds: 0,
            scrub_rows: Rc::new(VecModel::default()),
            epoch: Rc::new(Cell::new(Instant::now())),
            fixed_now_ms: None,
            scroll: None,
            expanded: HashSet::new(),
            auto_expanded: HashSet::new(),
            collapsed: HashSet::new(),
            reopened: HashSet::new(),
            search: String::new(),
            last: Frame::default(),
            layout: RowLayout::default(),
            hover: false,
            menu_open: false,
            key_focus: None,
            key_focus_hung: false,
            focus_generation: 0,
            current: None,
            list_focused: false,
            list_generation: 0,
            banner_dismissed: false,
            startup: Vec::new(),
            startup_rows: Rc::new(VecModel::default()),
            icons: IconCache::default(),
            icon_loader: None,
            no_icon: slint::Image::from_rgba8(slint::SharedPixelBuffer::new(1, 1)),
            no_bars: ModelRc::new(VecModel::from(vec![0.0; 5])),
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
        ui.set_startup_rows(ModelRc::from(b.startup_rows.clone()));
        let epoch = b.epoch.clone();
        smooth::install(ui, move || epoch.get().elapsed());
        Rc::new(RefCell::new(b))
    }

    /// The user picked `page` on the rail.
    pub fn set_page(&mut self, ui: &MainWindow, page: Page) {
        self.controls.set_page(page);
        if page != Page::Overview {
            // The hidden list can't report pointer-leave: release the hover freeze here. Nor may it
            // keep keyboard focus (a Narrator rail activation doesn't move focus): park it, so keys
            // can't reach the hidden list, and drop its freeze now rather than on a later focus-out.
            if self.list_focused || self.key_focus.is_some() {
                ui.invoke_park_focus();
            }
            self.clear_key_focus();
            self.set_hover(false);
        }
        if page != Page::Performance {
            // Likewise the hidden graphs: end any scrub.
            self.scrub(ui, -1.0);
        }
        if matches!(page, Page::Startup | Page::Settings) {
            // No graphs there, and no frames: the view rests on the newest sample (PF3).
            self.rest(ui);
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
            // The worker builds no frame for these (nothing on them changes per tick).
            Page::Startup | Page::Settings => {}
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
        ui.set_perf_span_ms(p.span_ms as f32);
        self.scroll_to(ui, p.window_end_ms);
        self.last_perf = Some(p);
        // The window moved under a still pointer: the card follows it to the new moment.
        self.show_scrub(ui);
    }

    /// The worker's collector epoch (`worker::spawn`).
    pub fn set_epoch(&mut self, epoch: Instant) {
        self.epoch.set(epoch);
    }

    /// Fixes collector time at `now_ms` instead of reading the clock, so a test places the
    /// scrolling view without waiting; None reads the clock again.
    pub fn set_collector_now(&mut self, now_ms: Option<f64>) {
        self.fixed_now_ms = now_ms;
    }

    fn collector_now_ms(&self) -> f64 {
        self.fixed_now_ms.unwrap_or_else(|| self.epoch.get().elapsed().as_secs_f64() * 1_000.0)
    }

    /// A frame's graphs landed with their newest sample at `newest_ms`. While graphs scroll, the
    /// view carries on from where it is (crate::smooth::Scroll) and Slint moves it until the next
    /// frame; while they don't, it rests on the newest sample.
    fn scroll_to(&mut self, ui: &MainWindow, newest_ms: u64) {
        let clock = ui.global::<GraphClock>();
        let (now, newest) = (self.collector_now_ms(), newest_ms as f64);
        if !clock.get_live() {
            self.scroll = Some(Scroll { r0: newest, at: now, speed: 0.0, newest });
            ui.invoke_scroll_from(0.0, 0.0);
            return;
        }
        // Resting (GraphClock speed 0: rested here, or by GraphClock itself when scrolling stopped
        // between frames), the view shows the run's newest sample on its edge: it carries on from
        // there while the frame still draws that moment, and otherwise starts on the frame's
        // newest sample. Never from a lag behind now: that would jump back (ruling PF3).
        let prev = self.scroll.map(|p| {
            if clock.get_speed() != 0.0 {
                return p;
            }
            let floor = newest - smooth::SCROLL_MARGIN_MS as f64;
            let edge = if (floor..=newest).contains(&p.newest) { p.newest } else { newest };
            Scroll { r0: edge, at: now, speed: 0.0, newest: edge }
        });
        let lag = smooth::lag_ms(self.controls.interval().as_millis() as u64);
        let run = Scroll::next(prev.as_ref(), now, newest, lag);
        ui.invoke_scroll_from(run.lead_ms() as f32, run.speed as f32);
        self.scroll = Some(run);
    }

    /// Graphs stop scrolling (a page without graphs): the view rests on the newest sample.
    fn rest(&mut self, ui: &MainWindow) {
        let now = self.collector_now_ms();
        self.scroll = self.scroll.map(|p| Scroll { r0: p.newest, at: now, speed: 0.0, newest: p.newest });
        ui.invoke_scroll_from(0.0, 0.0);
    }

    /// A startup list (or a toggle's re-read) landed: the Startup page shows it, and the next job
    /// may start.
    pub fn set_startup(&mut self, ui: &MainWindow, entries: Vec<StartupEntry>, elevated: bool) {
        let rows = crate::startup::startup_rows(&entries, elevated);
        sync_model(&*self.startup_rows, rows.iter().map(|r| StartupData {
            id: r.id, name: r.name.as_str().into(), publisher: r.publisher.as_str().into(), status: r.status.as_str().into(),
            location: r.location.as_str().into(), command: r.command.as_str().into(), enabled: r.enabled,
            can_change: r.can_change, note: r.note.as_str().into(),
        }).collect());
        ui.set_startup_admin_banner(crate::startup::needs_admin_banner(&entries, elevated));
        ui.set_startup_loading(false);
        self.startup = entries;
    }

    /// The startup entries as last listed.
    pub fn startup_entries(&self) -> &[StartupEntry] {
        &self.startup
    }

    /// The entry behind a Startup row id, as last listed.
    pub fn startup_entry(&self, id: i32) -> Option<StartupEntry> {
        usize::try_from(id).ok().and_then(|i| self.startup.get(i)).cloned()
    }

    /// Moves a switch at once; the re-read after the toggle then shows the real state.
    pub fn show_startup_pending(&self, id: i32, enabled: bool) {
        for i in 0..self.startup_rows.row_count() {
            if let Some(mut row) = self.startup_rows.row_data(i)
                && row.id == id
            {
                row.enabled = enabled;
                row.status = crate::startup::status_text(enabled).into();
                self.startup_rows.set_row_data(i, row);
            }
        }
    }

    pub fn set_history(&mut self, history: SharedHistory) {
        self.history = Some(history);
    }

    /// The pointer is `fraction` of the way across a Performance graph (-1: it left).
    pub fn scrub(&mut self, ui: &MainWindow, fraction: f32) {
        self.scrub_at = (fraction.is_finite() && fraction >= 0.0).then_some(fraction.min(1.0));
        self.show_scrub(ui);
    }

    /// Collector time at the right edge of the Performance graphs as drawn: where the scrolling
    /// view is now (crate::smooth), else the newest sample.
    fn view_end_ms(&self, ui: &MainWindow, perf: &PerfFrame) -> u64 {
        match &self.scroll {
            Some(run) if ui.global::<GraphClock>().get_live() => run.right_edge(self.collector_now_ms()).max(0.0).round() as u64,
            _ => perf.window_end_ms,
        }
    }

    /// The graph timer moved the view under a still pointer: the card follows to the moment now
    /// under it.
    pub fn follow_scrub(&mut self, ui: &MainWindow) {
        self.show_scrub(ui);
    }

    /// How many times the card was rebuilt (for tests).
    pub fn scrub_builds(&self) -> u32 {
        self.scrub_builds
    }

    /// Runs on the UI thread, which never waits for the worker: while the worker holds the history
    /// (or a panic left it poisoned, until the restarted worker resets it) the current card stays.
    fn show_scrub(&mut self, ui: &MainWindow) {
        let (Some(f), Some(perf), Some(history)) = (self.scrub_at, self.last_perf.as_ref(), self.history.as_ref()) else {
            self.scrub_shown = None;
            ui.set_scrub_x(-1.0);
            return;
        };
        let view_end = self.view_end_ms(ui, perf);
        let Ok(h) = history.try_lock() else {
            return;
        };
        let view = crate::scrub::scrub(&h, perf, view_end, f);
        drop(h);
        // The same sample under the same pointer: nothing to rebuild.
        if self.scrub_shown.as_ref().is_some_and(|(shown_f, shown)| *shown_f == f && *shown == view) {
            return;
        }
        self.scrub_builds += 1;
        self.scrub_shown = Some((f, view.clone()));
        ui.set_scrub_title(view.title.into());
        ui.set_scrub_empty(view.empty.into());
        sync_model(&*self.scrub_rows, view.rows.into_iter().map(|(name, value)| ScrubRow { name: name.into(), value: value.into() }).collect());
        ui.set_scrub_x(f);
    }

    fn apply_overview(&mut self, ui: &MainWindow, frame: Frame) {
        sync_model(&*self.meters, frame.meters.iter().map(|m| MeterData {
            label: m.label.into(), value: m.value.clone().into(), sub: m.sub.clone().into(), spark: m.spark.clone().into(), available: m.available,
        }).collect());
        sync_tiles(&self.tiles, frame.tiles.iter().map(|t| TileData {
            id: t.id, name: t.name.clone().into(), value: t.value.clone().into(), weight: t.weight, hung: t.hung, leaving: false,
        }).collect(), ui.get_settings_smooth_graphs());
        ui.set_top_caption(frame.caption.clone().into());
        ui.set_show_admin_banner(frame.admin_banner && !self.banner_dismissed);
        self.scroll_to(ui, frame.graph_end_ms);
        self.last = frame;
        // While frozen, newly shown children (or a reopened section's cards) would be deferred
        // anyway: expand on unfreeze instead.
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
        reopen_hung_sections(&self.last.rows, &mut self.collapsed, &mut self.reopened);
        if auto_expand_hung_groups(&self.last.rows, &mut self.expanded, &mut self.auto_expanded) {
            self.publish_child_rows();
        }
    }

    /// Tells the worker which apps need child rows (expanded ones; all of them while searching).
    fn publish_child_rows(&self) {
        self.controls.set_child_rows(&self.expanded, !self.search.trim().is_empty());
    }

    fn fresh(&self) -> Vec<RowView> {
        visible_rows(&self.last.rows, &self.expanded, &self.collapsed, &self.search)
    }

    /// Where rows' icons come from (crate::icons::IconLoader in the app).
    pub fn set_icon_loader(&mut self, loader: impl RequestIcon + 'static) {
        self.icon_loader = Some(Box::new(loader));
    }

    /// An icon came back from the loader: rows showing that image get it (only they change).
    pub fn put_icon(&mut self, path: String, px: Option<IconPixels>) {
        let shown = self.layout.rows().iter().any(|r| r.icon == path);
        self.icons.put(path, px);
        if shown {
            self.push_rows();
        }
    }

    /// The icon a row shows, if it has arrived; asks for it the first time its image is seen. The
    /// Windows group draws its own glyph and asks for none.
    fn icon_of(icons: &mut IconCache, loader: Option<&dyn RequestIcon>, r: &RowView) -> Option<slint::Image> {
        match loader {
            Some(loader) if !r.windows && !r.icon.is_empty() => icons.get(&r.icon, loader),
            _ => None,
        }
    }

    fn push_rows(&mut self) {
        let loader = self.icon_loader.as_deref();
        let icons: Vec<Option<slint::Image>> = self.layout.rows().iter().map(|r| Self::icon_of(&mut self.icons, loader, r)).collect();
        let rows = self.layout.rows().iter().zip(icons).enumerate().map(|(i, (r, icon))| RowData {
            id: r.id, kind: kind_index(r.kind), depth: r.depth, name: r.name.clone().into(), count: r.count.clone().into(),
            sub: r.sub.clone().into(), cpu: r.cpu.clone().into(), mem: r.mem.clone().into(), disk: r.disk.clone().into(),
            gpu: r.gpu.clone().into(), net: r.net.clone().into(), bars: self.bars_model(i, &r.bars),
            hung: r.hung.clone().into(), expandable: r.expandable, expanded: self.is_open(r),
            actionable: r.actionable, exited: r.exited, has_icon: icon.is_some(),
            icon: icon.unwrap_or_else(|| self.no_icon.clone()), windows: r.windows, last_in_card: r.last_in_card, pid: r.pid.clone().into(),
        }).collect();
        sync_model(&*self.rows, rows);
        self.sync_current();
    }

    /// A row's bars as the model Slint reads. A model compares by identity, so the row shown at
    /// `i` keeps its own while the values are the same, and every all-zero row (headings, process
    /// rows) shares one: otherwise `sync_model` would see every card changed on every frame.
    fn bars_model(&self, i: usize, bars: &[f32; 5]) -> ModelRc<f32> {
        if bars.iter().all(|b| *b == 0.0) {
            return self.no_bars.clone();
        }
        match self.rows.row_data(i) {
            Some(shown) if shown.bars.iter().eq(bars.iter().copied()) => shown.bars,
            _ => ModelRc::new(VecModel::from(bars.to_vec())),
        }
    }

    pub fn set_search(&mut self, text: &str) {
        self.search = text.to_string();
        self.publish_child_rows();
        let fresh = self.fresh();
        self.layout.rebase(fresh);
        self.push_rows();
    }

    /// Whether a row shows open: a section unless closed, an app when expanded.
    fn is_open(&self, r: &RowView) -> bool {
        if r.kind == RowKind::Section { !self.collapsed.contains(&r.id) } else { self.expanded.contains(&r.id) }
    }

    /// Opens or closes a section heading, or expands or collapses an app.
    pub fn toggle(&mut self, id: i32) {
        if is_section(id) {
            let opening = self.collapsed.remove(&id);
            if !opening {
                self.collapsed.insert(id);
            }
            // Closed with the mouse while the keyboard-current row is inside it: the current row
            // moves to the heading instead of going with its card.
            let rows = self.layout.rows();
            if !opening
                && let (Some(at), Some(cur)) = (rows.iter().position(|r| r.id == id), self.current_index())
                && (at + 1..section_end(rows, at)).contains(&cur)
            {
                self.current = Some(id);
            }
            let fresh = self.fresh();
            if opening {
                self.layout.open_section(id, &self.last.rows, fresh);
            } else {
                self.layout.close_section(id, &self.last.rows, fresh);
            }
            self.push_rows();
            return;
        }
        // An exited app has no processes left to show: it never opens (a held open one still closes).
        if !self.expanded.contains(&id) && self.layout.rows().iter().any(|r| r.id == id && r.exited) {
            return;
        }
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
        // Its section opens too, or there would be nothing to show.
        let rows = &self.last.rows;
        if let Some(h) = rows.iter().position(|r| r.id == id).and_then(|at| rows[..at].iter().rfind(|r| r.kind == RowKind::Section)) {
            self.collapsed.remove(&h.id);
        }
        self.publish_child_rows();
        let fresh = self.fresh();
        self.layout.rebase(fresh);
        self.push_rows();
        if let (Some(i), Some(ui)) = (self.layout.rows().iter().position(|r| r.id == id), self.ui.upgrade()) {
            let (y, h) = row_top(self.layout.rows(), i);
            ui.invoke_ensure_row_visible(y, h);
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
            // A row has one such control at a time: Force close while hung, End task otherwise.
            self.key_focus_hung = self.layout.rows().iter().any(|r| r.id == id && !r.hung.is_empty());
        } else if self.key_focus == Some(id) {
            self.key_focus = None;
        }
        self.update_frozen();
    }

    /// Whether the focused control still exists. Force close shows on every hung actionable row,
    /// so it goes when the row recovers; a card's End task shows while the card is hovered or the
    /// button itself holds focus, so a focused one goes only when the card turns hung. Either
    /// goes when its row exits or can no longer be acted on. A control destroyed while focused
    /// sends no focus-out, so this check is what releases the freeze then.
    fn key_focus_holds(&self) -> bool {
        let Some(id) = self.key_focus else { return false };
        self.layout.rows().iter().any(|r| {
            r.id == id && r.actionable && !r.exited
                && if self.key_focus_hung { !r.hung.is_empty() } else { r.hung.is_empty() && r.kind == RowKind::App && r.depth == 0 }
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
        let mut outcome = navigate(self.layout.rows(), &self.expanded, &self.collapsed, self.current, key);
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
        // Where the keyboard menu anchors (and what it brings back into view first).
        let (y, h) = index.map_or((0.0, 0.0), |i| row_top(rows, i));
        ui.set_current_row_y(y);
        ui.set_current_row_h(h);
        ui.set_current_id(self.current.unwrap_or(-1));
        ui.set_current_actionable(index.is_some_and(|i| rows[i].actionable && !rows[i].exited));
    }

    /// Scrolls the current row into view: only for the user's own keyboard moves (ruling PF1).
    fn reveal_current(&self) {
        if let (Some(i), Some(ui)) = (self.current_index(), self.ui.upgrade()) {
            let (y, h) = row_top(self.layout.rows(), i);
            ui.invoke_ensure_row_visible(y, h);
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

    fn section(id: i32) -> RowView {
        RowView { id, parent: -1, depth: 0, kind: RowKind::Section, expandable: true, ..Default::default() }
    }

    #[test]
    fn a_hung_app_reopens_its_section_once() {
        use crate::view::{SECTION_APPS, SECTION_BACKGROUND};
        let rows = |hung: &str| vec![section(SECTION_APPS), app(1, "1%"), section(SECTION_BACKGROUND), RowView { hung: hung.into(), ..app(2, "1%") }];
        let (mut collapsed, mut reopened) = (HashSet::from([SECTION_BACKGROUND]), HashSet::new());
        assert!(!reopen_hung_sections(&rows(""), &mut collapsed, &mut reopened));
        assert!(collapsed.contains(&SECTION_BACKGROUND), "no hang: the section stays closed");
        assert!(reopen_hung_sections(&rows("Not responding · 9s"), &mut collapsed, &mut reopened));
        assert!(!collapsed.contains(&SECTION_BACKGROUND), "the hang opens it");
        collapsed.insert(SECTION_BACKGROUND); // the user closes it again
        assert!(!reopen_hung_sections(&rows("Not responding · 10s"), &mut collapsed, &mut reopened));
        assert!(collapsed.contains(&SECTION_BACKGROUND), "still the same hang: the user's close stands");
        assert!(!reopen_hung_sections(&rows(""), &mut collapsed, &mut reopened));
        assert!(reopen_hung_sections(&rows("Not responding · 1s"), &mut collapsed, &mut reopened), "a new hang opens it again");
        assert!(collapsed.is_empty());
    }

    #[test]
    fn ensure_visible_uses_real_row_tops() {
        let proc = |id: i32| RowView { kind: RowKind::Proc, ..child(id, 3) };
        let rows = [section(crate::view::SECTION_APPS), app(2, "1%"), app(3, "1%"), proc(4), proc(5), section(crate::view::SECTION_BACKGROUND)];
        assert_eq!(row_top(&rows, 0), (0.0, 32.0));
        assert_eq!(row_top(&rows, 2), (32.0 + 50.0, 50.0));
        assert_eq!(row_top(&rows, 4), (32.0 + 50.0 + 50.0 + 30.0, 30.0));
        assert_eq!(row_top(&rows, 5), (32.0 + 50.0 + 50.0 + 30.0 + 30.0, 32.0));
    }

    /// The list is frozen whenever it has keyboard focus, so a heading toggled from the keyboard
    /// must still close and open its own block in place; nothing else moves.
    #[test]
    fn a_section_closes_and_opens_in_place_while_frozen() {
        use crate::view::{SECTION_APPS, SECTION_BACKGROUND};
        let all = vec![section(SECTION_APPS), app(1, "1%"), section(SECTION_BACKGROUND), app(2, "1%"), app(3, "1%")];
        let mut layout = RowLayout::default();
        layout.on_frame(&all, all.clone());
        layout.set_frozen(true, &all, all.clone());
        // A new app 9 arrives meanwhile in Apps: it must keep waiting.
        let next = vec![section(SECTION_APPS), app(1, "1%"), app(9, "1%"), section(SECTION_BACKGROUND), app(2, "1%"), app(3, "1%")];
        let closed = vec![section(SECTION_APPS), app(1, "1%"), app(9, "1%"), section(SECTION_BACKGROUND)];
        layout.close_section(SECTION_BACKGROUND, &next, closed);
        assert_eq!(ids(layout.rows()), [SECTION_APPS, 1, SECTION_BACKGROUND]);
        layout.open_section(SECTION_BACKGROUND, &next, next.clone());
        assert_eq!(ids(layout.rows()), [SECTION_APPS, 1, SECTION_BACKGROUND, 2, 3]);
        layout.set_frozen(false, &next, next.clone());
        assert_eq!(ids(layout.rows()), [SECTION_APPS, 1, 9, SECTION_BACKGROUND, 2, 3]);
    }

    #[test]
    fn an_exited_placeholder_keeps_its_cards_shape() {
        let mut last = child(3, 2);
        last.kind = RowKind::Proc;
        last.last_in_card = true;
        let mut group = app(2, "1%");
        (group.windows, group.expandable) = (true, true);
        let prev = vec![group, last];
        let held = hold(&prev, &[], &[], &HashSet::new());
        assert!(held.iter().all(|r| r.exited));
        assert!(held[0].windows && held[0].expandable, "the held group still draws as the Windows card");
        assert!(held[1].last_in_card, "the held last process still closes its card");
    }

    /// While frozen, rows keep their places but take each frame's values: the card must still close
    /// below its last shown process, never mid-card or twice.
    #[test]
    fn a_frozen_card_closes_once_below_its_last_shown_process() {
        let proc = |id: i32, last: bool| RowView { kind: RowKind::Proc, last_in_card: last, ..child(id, 2) };
        let closing = |layout: &RowLayout| layout.rows().iter().filter(|r| r.last_in_card).map(|r| r.id).collect::<Vec<_>>();
        let first = vec![app(2, "1%"), proc(3, false), proc(4, true), app(5, "1%")];
        let frozen = || {
            let mut layout = RowLayout::default();
            layout.on_frame(&first, first.clone());
            layout.set_frozen(true, &first, first.clone());
            layout
        };
        // The processes swap places in the next frame: the held order stands, and so must its end.
        let mut layout = frozen();
        let reordered = vec![app(2, "1%"), proc(4, false), proc(3, true), app(5, "1%")];
        layout.on_frame(&reordered, reordered.clone());
        assert_eq!(ids(layout.rows()), [2, 3, 4, 5]);
        assert_eq!(closing(&layout), [4], "a frozen reorder");
        // The last process exits: it stays in place, inert, and still closes the card alone.
        let mut layout = frozen();
        let exited = vec![app(2, "1%"), proc(3, true), app(5, "1%")];
        layout.on_frame(&exited, exited.clone());
        assert_eq!(ids(layout.rows()), [2, 3, 4, 5]);
        assert!(layout.rows()[2].exited);
        assert_eq!(closing(&layout), [4], "a frozen exit of the last process");
    }

    #[test]
    fn vec_model_store_round_trips() {
        let model = VecModel::from(vec![1, 2]);
        sync_model(&model, vec![5, 2, 7]);
        assert_eq!((0..RowStore::len(&model)).map(|i| RowStore::get(&model, i).unwrap()).collect::<Vec<_>>(), [5, 2, 7]);
    }
}
