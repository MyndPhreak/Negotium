//! State shared between the UI thread (writes) and the worker (reads).
use crate::perf::Range;
use negotium_model::{Metric, Sort, SortColumn};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

/// Which page the window shows; the worker builds only that page's view.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Page {
    #[default]
    Overview = 0,
    Performance = 1,
    Startup = 2,
    Settings = 3,
    AppHistory = 4,
    Services = 5,
    Ports = 6,
    Users = 7,
}

impl Page {
    pub const ALL: [Page; 8] = [
        Page::Overview,
        Page::Performance,
        Page::Startup,
        Page::Settings,
        Page::AppHistory,
        Page::Services,
        Page::Ports,
        Page::Users,
    ];

    /// Unknown indexes fall back to the Overview.
    pub fn from_index(i: i32) -> Page {
        match i {
            1 => Page::Performance,
            2 => Page::Startup,
            3 => Page::Settings,
            4 => Page::AppHistory,
            5 => Page::Services,
            6 => Page::Ports,
            _ => Page::Overview,
        }
    }

    pub fn index(self) -> i32 {
        self as i32
    }

    /// The page's name on the command line (`--page=<name>`).
    pub fn name(self) -> &'static str {
        match self {
            Page::Overview => "overview",
            Page::Performance => "performance",
            Page::Startup => "startup",
            Page::Settings => "settings",
            Page::AppHistory => "history",
            Page::Services => "services",
            Page::Ports => "ports",
            Page::Users => "",
        }
    }

    pub fn from_name(name: &str) -> Option<Page> {
        Self::ALL.into_iter().find(|p| p.name().eq_ignore_ascii_case(name))
    }
}

/// Arguments for the elevated instance (spec §6): wait for this one to exit, open on `page`. Only
/// the page is carried; the Performance device and range and the search start fresh.
pub fn restart_args(page: Page) -> String {
    format!("--restarted --page={}", page.name())
}

/// The toast after a failed restart as admin: declining the UAC prompt (ERROR_CANCELLED) is a
/// cancel; anything else says what went wrong.
pub fn restart_failure_text(e: &negotium_actions::ActionError) -> String {
    match e {
        negotium_actions::ActionError::Cancelled => "Restart as admin was cancelled.".to_string(),
        other => format!("Couldn't restart as admin: {}.", other.to_string().trim_end_matches('.')),
    }
}

/// The page asked for with `--page=<name>`, if any. An unknown name asks for none (the Overview).
pub fn page_from_args<I: IntoIterator<Item = String>>(args: I) -> Option<Page> {
    args.into_iter().find_map(|a| a.strip_prefix("--page=").and_then(Page::from_name))
}

/// [`page_from_args`] for the process's own arguments: an argument that isn't valid Unicode can't
/// be a `--page=` request, so it is skipped instead of panicking like `std::env::args` would.
pub fn page_from_os_args<I: IntoIterator<Item = std::ffi::OsString>>(args: I) -> Option<Page> {
    page_from_args(args.into_iter().filter_map(|a| a.into_string().ok()))
}

/// A header click: the same column flips its direction, another starts at its own (`Sort::by`).
pub fn header_click(current: Sort, column: SortColumn) -> Sort {
    if current.column == column { current.flipped() } else { Sort::by(column) }
}

/// A sort column as the window carries it: -1 is the name, 0..=4 a metric's index.
pub fn sort_column_index(c: SortColumn) -> i32 {
    match c {
        SortColumn::Name => -1,
        SortColumn::Metric(m) => m as i32,
    }
}

/// [`sort_column_index`] reversed; anything above the last metric is the last metric.
pub fn sort_column_from_index(i: i32) -> SortColumn {
    if i < 0 { SortColumn::Name } else { SortColumn::Metric(Metric::ALL[(i as usize).min(Metric::ALL.len() - 1)]) }
}

pub struct Controls {
    metric: AtomicU8,
    /// The list's sort packed as `column + 1` in the low bits (0 = name) and the direction in bit 7,
    /// so the worker reads both in one load.
    sort: AtomicU8,
    /// App History sort column (low bits, 0 = Name, 1 = CPU, 2 = Net, 3 = Disk, 4 = Peak Mem) and
    /// direction in bit 7.
    history_sort: AtomicU8,
    /// Reset cumulative app history on the next worker tick.
    reset_history: AtomicBool,
    /// Read the socket tables on the next worker tick (the Ports page's Refresh, or the page just shown).
    ports_refresh: AtomicBool,
    /// Overview list grouped under Apps / Background processes / Windows processes headings.
    grouped: AtomicBool,
    page: AtomicU8,
    /// Selected Performance device id and graph range.
    perf: Mutex<(String, Range)>,
    frozen: AtomicBool,
    visible: AtomicBool,
    /// Refresh interval while visible (Settings), milliseconds.
    refresh_ms: AtomicU32,
    /// Settings > Show CPU in tray icon: the worker sends each kept sample's CPU minute to the tray.
    tray_cpu: AtomicBool,
    /// Explorer's taskbar window (`Shell_TrayWnd`) was hung at the worker's latest tick: the tray waits
    /// (crate::tray, PF2; `platform::shell_tray_hung`).
    shell_hung: AtomicBool,
    stop: AtomicBool,
    /// Expanded app row ids and whether a search is active: the worker builds child rows only
    /// for those (spec section 8: no per-process formatting for rows nobody can see).
    child_rows: Mutex<(HashSet<i32>, bool)>,
    /// Latched wake flag: a poke that lands while the worker is busy (not waiting) is
    /// remembered, so the next `wait` returns immediately instead of losing the wakeup.
    woken: Mutex<bool>,
    moderator: Mutex<negotium_moderator::ModeratorConfig>,
    cv: Condvar,
}

const DESCENDING_BIT: u8 = 0x80;

fn pack_sort(s: Sort) -> u8 {
    ((sort_column_index(s.column) + 1) as u8) | if s.descending { DESCENDING_BIT } else { 0 }
}

impl Default for Controls {
    fn default() -> Self {
        Self {
            metric: AtomicU8::new(0),
            sort: AtomicU8::new(pack_sort(Sort::default())),
            history_sort: AtomicU8::new(1 | DESCENDING_BIT),
            reset_history: AtomicBool::new(false),
            ports_refresh: AtomicBool::new(false),
            moderator: Mutex::new(negotium_moderator::ModeratorConfig::default()),
            grouped: AtomicBool::new(true),
            page: AtomicU8::new(Page::Overview as u8),
            perf: Mutex::new(("cpu".to_string(), Range::Minute)),
            frozen: AtomicBool::new(false),
            visible: AtomicBool::new(true),
            refresh_ms: AtomicU32::new(1000),
            tray_cpu: AtomicBool::new(false),
            shell_hung: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            child_rows: Mutex::new((HashSet::new(), false)),
            woken: Mutex::new(false),
            cv: Condvar::new(),
        }
    }
}

impl Controls {
    pub fn set_metric(&self, m: Metric) {
        self.metric.store(m as u8, Ordering::Relaxed);
        self.poke();
    }
    pub fn metric(&self) -> Metric {
        Metric::ALL[(self.metric.load(Ordering::Relaxed) as usize).min(Metric::ALL.len() - 1)]
    }
    /// Sets the list's sort and wakes the worker: an explicit sort change shows at once.
    pub fn set_sort(&self, s: Sort) {
        self.sort.store(pack_sort(s), Ordering::Relaxed);
        self.poke();
    }
    pub fn sort(&self) -> Sort {
        let v = self.sort.load(Ordering::Relaxed);
        Sort { column: sort_column_from_index((v & !DESCENDING_BIT) as i32 - 1), descending: v & DESCENDING_BIT != 0 }
    }
    pub fn set_history_sort(&self, col: i32, descending: bool) {
        let packed = (col.clamp(0, 4) as u8) | if descending { DESCENDING_BIT } else { 0 };
        self.history_sort.store(packed, Ordering::Relaxed);
        self.poke();
    }
    pub fn history_sort(&self) -> (i32, bool) {
        let v = self.history_sort.load(Ordering::Relaxed);
        ((v & !DESCENDING_BIT) as i32, v & DESCENDING_BIT != 0)
    }
    pub fn request_reset_history(&self) {
        self.reset_history.store(true, Ordering::Relaxed);
        self.poke();
    }
    pub fn take_reset_history(&self) -> bool {
        self.reset_history.swap(false, Ordering::Relaxed)
    }
    pub fn request_ports_refresh(&self) {
        self.ports_refresh.store(true, Ordering::Relaxed);
        self.poke();
    }
    pub fn take_ports_refresh(&self) -> bool {
        self.ports_refresh.swap(false, Ordering::Relaxed)
    }
    pub fn set_grouped(&self, on: bool) {
        self.grouped.store(on, Ordering::Relaxed);
        self.poke();
    }
    pub fn grouped(&self) -> bool {
        self.grouped.load(Ordering::Relaxed)
    }
    pub fn set_page(&self, p: Page) {
        self.page.store(p as u8, Ordering::Relaxed);
        self.poke();
    }
    pub fn page(&self) -> Page {
        Page::from_index(self.page.load(Ordering::Relaxed) as i32)
    }
    pub fn select_device(&self, id: &str) {
        self.perf.lock().unwrap_or_else(|e| e.into_inner()).0 = id.to_string();
        self.poke();
    }
    /// Records the device a frame fell back to; the frame is already on screen, so no wakeup.
    pub fn adopt_device(&self, id: &str) {
        self.perf.lock().unwrap_or_else(|e| e.into_inner()).0 = id.to_string();
    }
    pub fn set_range(&self, r: Range) {
        self.perf.lock().unwrap_or_else(|e| e.into_inner()).1 = r;
        self.poke();
    }
    pub fn perf(&self) -> (String, Range) {
        self.perf.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    pub fn set_frozen(&self, f: bool) {
        self.frozen.store(f, Ordering::Relaxed);
    }
    pub fn frozen(&self) -> bool {
        self.frozen.load(Ordering::Relaxed)
    }
    pub fn set_visible(&self, v: bool) {
        self.visible.store(v, Ordering::Relaxed);
        self.poke();
    }
    pub fn visible(&self) -> bool {
        self.visible.load(Ordering::Relaxed)
    }
    /// Sets the refresh rate (Settings) and wakes the worker, so it takes effect at once. The
    /// history still keeps at most one sample per second (`negotium_history::MIN_SPACING_MS`).
    pub fn set_refresh_ms(&self, ms: u32) {
        self.refresh_ms.store(ms.max(100), Ordering::Relaxed);
        self.poke();
    }
    /// The chosen rate while visible; hidden in the tray, 1 s or the chosen rate if slower.
    pub fn interval(&self) -> Duration {
        let refresh = self.refresh_ms.load(Ordering::Relaxed).max(100) as u64;
        Duration::from_millis(if self.visible() { refresh } else { refresh.max(1000) })
    }
    pub fn moderator_config(&self) -> negotium_moderator::ModeratorConfig {
        self.moderator.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    pub fn set_moderator_config(&self, config: negotium_moderator::ModeratorConfig) {
        *self.moderator.lock().unwrap_or_else(|e| e.into_inner()) = config;
    }
    pub fn set_tray_cpu(&self, on: bool) {
        self.tray_cpu.store(on, Ordering::Relaxed);
    }
    pub fn tray_cpu(&self) -> bool {
        self.tray_cpu.load(Ordering::Relaxed)
    }
    pub fn set_shell_hung(&self, hung: bool) {
        self.shell_hung.store(hung, Ordering::Relaxed);
    }
    pub fn shell_hung(&self) -> bool {
        self.shell_hung.load(Ordering::Relaxed)
    }
    /// Shares which apps need child rows. Wakes the worker when more are needed (an expand, or a
    /// search starting), so they show without waiting for the next tick.
    pub fn set_child_rows(&self, expanded: &HashSet<i32>, searching: bool) {
        let more = {
            let mut cur = self.child_rows.lock().unwrap_or_else(|e| e.into_inner());
            let more = (searching && !cur.1) || expanded.iter().any(|id| !cur.0.contains(id));
            *cur = (expanded.clone(), searching);
            more
        };
        if more {
            self.poke();
        }
    }
    pub fn child_rows(&self) -> (HashSet<i32>, bool) {
        self.child_rows.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    pub fn request_stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        self.poke();
    }
    pub fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }
    pub fn poke(&self) {
        *self.woken.lock().unwrap_or_else(|e| e.into_inner()) = true;
        self.cv.notify_all();
    }
    /// Sleeps up to `d`; returns true if poked earlier (or a poke was already pending).
    pub fn wait(&self, d: Duration) -> bool {
        let guard = self.woken.lock().unwrap_or_else(|e| e.into_inner());
        let (mut guard, result) = self
            .cv
            .wait_timeout_while(guard, d, |woken| !*woken)
            .unwrap_or_else(|e| e.into_inner());
        let woke = !result.timed_out();
        *guard = false;
        woke
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    #[test]
    fn a_failed_restart_as_admin_says_why() {
        use negotium_actions::ActionError;
        assert_eq!(restart_failure_text(&ActionError::Cancelled), "Restart as admin was cancelled.");
        assert_eq!(
            restart_failure_text(&ActionError::Other("The system cannot find the file specified.".into())),
            "Couldn't restart as admin: The system cannot find the file specified."
        );
        assert_eq!(restart_failure_text(&ActionError::AccessDenied), "Couldn't restart as admin: access denied.");
    }

    #[test]
    fn interval_depends_on_visibility() {
        let c = Controls::default();
        c.set_visible(true);
        assert_eq!(c.interval(), Duration::from_secs(1));
        c.set_visible(false);
        assert_eq!(c.interval(), Duration::from_secs(1));
    }

    #[test]
    fn the_refresh_rate_applies_while_visible_and_never_speeds_up_the_tray() {
        let c = Controls::default();
        c.set_refresh_ms(500);
        assert!(c.wait(Duration::from_millis(20)), "a new rate takes effect at once");
        c.set_visible(true);
        assert_eq!(c.interval(), Duration::from_millis(500));
        c.set_visible(false);
        assert_eq!(c.interval(), Duration::from_secs(1), "hidden: 1 s");
        c.set_refresh_ms(5000);
        assert_eq!(c.interval(), Duration::from_secs(5), "hidden never ticks faster than chosen");
        c.set_visible(true);
        assert_eq!(c.interval(), Duration::from_secs(5));
    }

    #[test]
    fn poke_wakes_a_waiter_early() {
        let c = Arc::new(Controls::default());
        let c2 = c.clone();
        let t = std::thread::spawn(move || {
            let start = Instant::now();
            let woke = c2.wait(Duration::from_secs(5));
            (woke, start.elapsed())
        });
        std::thread::sleep(Duration::from_millis(100));
        c.set_metric(Metric::Memory);
        let (woke, elapsed) = t.join().unwrap();
        assert!(woke && elapsed < Duration::from_secs(2));
        assert_eq!(c.metric(), Metric::Memory);
    }

    #[test]
    fn child_rows_are_shared_and_newly_needed_children_wake_the_worker() {
        let c = Controls::default();
        c.set_child_rows(&HashSet::from([7]), false);
        assert!(c.wait(Duration::from_millis(20)), "an expand asks for a frame with its children now");
        assert_eq!(c.child_rows(), (HashSet::from([7]), false));
        c.set_child_rows(&HashSet::new(), false);
        assert!(!c.wait(Duration::from_millis(20)), "collapsing needs nothing new");
        c.set_child_rows(&HashSet::new(), true);
        assert!(c.wait(Duration::from_millis(20)), "starting a search needs every app's children");
        assert_eq!(c.child_rows(), (HashSet::new(), true));
    }

    #[test]
    fn page_and_performance_selection_are_shared_and_wake_the_worker() {
        use crate::perf::Range;
        let c = Controls::default();
        assert_eq!(c.page(), Page::Overview);
        assert_eq!(c.perf(), ("cpu".to_string(), Range::Minute));
        c.set_page(Page::Performance);
        assert!(c.wait(Duration::from_millis(20)));
        assert_eq!(c.page(), Page::Performance);
        c.select_device("memory");
        assert!(c.wait(Duration::from_millis(20)));
        c.set_range(Range::Hour);
        assert!(c.wait(Duration::from_millis(20)));
        assert_eq!(c.perf(), ("memory".to_string(), Range::Hour));
        c.adopt_device("cpu");
        assert!(!c.wait(Duration::from_millis(20)), "a fallback adopted from a frame needs no new frame");
        assert_eq!(c.perf().0, "cpu");
        assert_eq!(Page::ALL.map(Page::index), [0, 1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(Page::from_index(6), Page::Ports);
        assert_eq!(Page::from_index(7), Page::Users);
        assert_eq!(Page::Users.index(), 7);
        assert_eq!(Page::Ports.index(), 6);
        assert_eq!(Page::from_index(3), Page::Settings);
        assert_eq!(Page::from_index(4), Page::AppHistory);
        assert_eq!(Page::from_index(5), Page::Services);
        assert_eq!(Page::from_index(9), Page::Overview, "unknown pages fall back to the Overview");
        assert_eq!(Page::from_index(-2), Page::Overview);
    }

    #[test]
    fn restart_arguments_carry_the_page() {
        for page in Page::ALL {
            assert_eq!(page_from_args(restart_args(page).split(' ').map(String::from)), Some(page));
        }
        assert_eq!(restart_args(Page::Startup), "--restarted --page=startup");
        assert_eq!(page_from_args(["negotium.exe".to_string(), "--page=SETTINGS".to_string()]), Some(Page::Settings));
        assert_eq!(page_from_args(["--page=nope".to_string()]), None);
        assert_eq!(page_from_args(Vec::<String>::new()), None);
        // A non-Unicode argument is skipped, not a panic, and doesn't hide a later --page=.
        use std::os::windows::ffi::OsStringExt;
        let bad = std::ffi::OsString::from_wide(&[0x61, 0xD800, 0x62]);
        let args = [std::ffi::OsString::from("negotium.exe"), bad, std::ffi::OsString::from("--page=startup")];
        assert_eq!(page_from_os_args(args), Some(Page::Startup));
        // Garbage falls back to the Overview (no page asked for); the device, range and search
        // are never carried.
        for junk in ["--page=", "--page", "page=startup", "--page=startup2", "--PAGE=startup", "--page= startup"] {
            assert_eq!(page_from_args(["negotium.exe".to_string(), junk.to_string()]), None, "{junk}");
        }
        assert_eq!(Page::from_name("Performance"), Some(Page::Performance));
        assert_eq!(Page::ALL.map(Page::name), ["overview", "performance", "startup", "settings", "history", "services", "ports", "users"]);
        assert_eq!(Page::from_name("Users"), Some(Page::Users));
        assert_eq!(Page::ALL.len(), 8);
        assert_eq!(restart_args(Page::Users), "--restarted --page=users");
        assert_eq!(page_from_args(restart_args(Page::Users).split(' ').map(String::from)), Some(Page::Users));
        assert_eq!(Page::from_name("ports"), Some(Page::Ports));
        assert_eq!(restart_args(Page::Ports), "--restarted --page=ports");
        assert_eq!(page_from_args(restart_args(Page::Ports).split(' ').map(String::from)), Some(Page::Ports));
    }

    #[test]
    fn a_ports_refresh_is_taken_once_and_wakes_the_worker() {
        let c = Controls::default();
        assert!(!c.take_ports_refresh(), "false by default");
        c.request_ports_refresh();
        assert!(c.wait(Duration::from_millis(20)), "a Refresh is shown without waiting for the next tick");
        assert!(c.take_ports_refresh());
        assert!(!c.take_ports_refresh(), "taken once");
    }

    #[test]
    fn header_click_flips_the_same_column_and_starts_others_fresh() {
        let cpu = SortColumn::Metric(Metric::Cpu);
        let desc = |column| Sort { column, descending: true };
        let asc = |column| Sort { column, descending: false };
        assert_eq!(header_click(desc(cpu), cpu), asc(cpu));
        assert_eq!(header_click(asc(cpu), SortColumn::Name), asc(SortColumn::Name));
        assert_eq!(header_click(asc(SortColumn::Name), SortColumn::Name), desc(SortColumn::Name));
        let mem = SortColumn::Metric(Metric::Memory);
        assert_eq!(header_click(desc(SortColumn::Name), mem), desc(mem));
    }

    #[test]
    fn the_sort_and_grouping_are_shared_and_a_sort_wakes_the_worker() {
        let c = Controls::default();
        assert_eq!((c.sort(), c.grouped()), (Sort::default(), true));
        c.set_sort(Sort { column: SortColumn::Name, descending: true });
        assert!(c.wait(Duration::from_millis(20)), "a new sort is shown without waiting for the next tick");
        assert_eq!(c.sort(), Sort { column: SortColumn::Name, descending: true });
        c.set_sort(Sort { column: SortColumn::Metric(Metric::Network), descending: false });
        assert_eq!(c.sort(), Sort { column: SortColumn::Metric(Metric::Network), descending: false });
        assert!(c.wait(Duration::from_millis(20)));
        c.set_grouped(false);
        assert!(c.wait(Duration::from_millis(20)));
        assert!(!c.grouped());
    }

    #[test]
    fn sort_columns_use_the_slint_encoding() {
        assert_eq!(sort_column_index(SortColumn::Name), -1);
        assert_eq!(sort_column_index(SortColumn::Metric(Metric::Network)), 4);
        for i in -1..5 {
            assert_eq!(sort_column_index(sort_column_from_index(i)), i);
        }
        assert_eq!(sort_column_from_index(9), SortColumn::Metric(Metric::Network));
        assert_eq!(sort_column_from_index(-7), SortColumn::Name);
    }

    #[test]
    fn poke_before_wait_is_not_lost() {
        let c = Controls::default();
        c.poke();
        let start = Instant::now();
        assert!(c.wait(Duration::from_secs(5)));
        assert!(start.elapsed() < Duration::from_secs(1));
        // The latch is consumed: a second wait times out.
        assert!(!c.wait(Duration::from_millis(20)));
    }
}
