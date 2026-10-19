//! User settings (spec 5.4) in %LOCALAPPDATA%\Negotium\settings.json. A missing file means
//! defaults. An unreadable or invalid file is moved aside to settings.json.bad and defaults are
//! used: a bad file never stops Negotium from starting. Unknown fields are ignored and missing
//! ones take their defaults, so files from older or newer versions load.
use crate::appdata::Writes;
use crate::controls::sort_column_index;
use negotium_model::{Metric, Sort, SortColumn};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

pub const REFRESH_CHOICES_MS: [u32; 4] = [500, 1000, 2000, 5000];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeChoice {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemeChoice {
    pub const ALL: [ThemeChoice; 3] = [ThemeChoice::System, ThemeChoice::Light, ThemeChoice::Dark];

    pub fn index(self) -> i32 {
        self as i32
    }

    pub fn from_index(i: i32) -> ThemeChoice {
        Self::ALL[(i.max(0) as usize).min(Self::ALL.len() - 1)]
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Refresh interval while the window is visible, one of REFRESH_CHOICES_MS.
    pub refresh_ms: u32,
    pub theme: ThemeChoice,
    pub always_on_top: bool,
    /// Closing the window hides it to the tray (spec 5.5) instead of exiting.
    pub keep_in_tray: bool,
    /// Use the GPU renderer. The effective renderer is the GPU only when this is on and no
    /// automatic-fallback marker exists (`crate::renderer`). Read at launch.
    pub gpu_rendering: bool,
    /// Graphs scroll continuously between samples (crate::smooth).
    pub smooth_graphs: bool,
    /// The tray icon draws the CPU graph (crate::tray).
    pub tray_cpu: bool,
    /// The Overview's sort column: "name", "cpu", "memory", "disk", "gpu" or "network" (an unknown
    /// one reads as the default, CPU). A string so the file stays readable and a newer column can't
    /// break an older version.
    pub sort_column: String,
    pub sort_descending: bool,
    /// The Overview groups apps, background processes and Windows processes under headings.
    pub group_by_type: bool,
    /// Look for a newer release when Negotium starts (the installed copy only).
    pub check_updates: bool,
    /// Negotium starts when the user signs in. Whether the Run value or logon task exists is read
    /// from the machine; this is the choice the user made.
    pub start_with_windows: bool,
    /// Optional table columns on the Overview page.
    pub col_pid: bool,
    pub col_proc_name: bool,
    pub col_status: bool,
    pub col_threads: bool,
    pub col_handles: bool,
    pub col_publisher: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            refresh_ms: 1000,
            theme: ThemeChoice::System,
            always_on_top: false,
            keep_in_tray: true,
            gpu_rendering: true,
            smooth_graphs: true,
            tray_cpu: true,
            sort_column: "cpu".to_string(),
            sort_descending: true,
            group_by_type: true,
            check_updates: true,
            start_with_windows: false,
            col_pid: false,
            col_proc_name: false,
            col_status: false,
            col_threads: false,
            col_handles: false,
            col_publisher: false,
        }
    }
}

/// The names `Settings::sort_column` holds, in the window's column order (name first).
const SORT_NAMES: [&str; 6] = ["name", "cpu", "memory", "disk", "gpu", "network"];

impl Settings {
    /// The saved sort; an unknown column name is the default column.
    pub fn sort(&self) -> Sort {
        let column = match SORT_NAMES.iter().position(|n| *n == self.sort_column) {
            Some(0) => SortColumn::Name,
            Some(i) => SortColumn::Metric(Metric::ALL[i - 1]),
            None => SortColumn::Metric(Metric::Cpu),
        };
        Sort { column, descending: self.sort_descending }
    }

    pub fn set_sort(&mut self, s: Sort) {
        self.sort_column = SORT_NAMES[(sort_column_index(s.column) + 1) as usize].to_string();
        self.sort_descending = s.descending;
    }

    pub fn normalized(mut self) -> Settings {
        self.refresh_ms = nearest_refresh(self.refresh_ms);
        self
    }

    pub fn refresh_index(&self) -> i32 {
        REFRESH_CHOICES_MS.iter().position(|&c| c == nearest_refresh(self.refresh_ms)).unwrap_or(1) as i32
    }
}

pub fn nearest_refresh(ms: u32) -> u32 {
    REFRESH_CHOICES_MS.into_iter().min_by_key(|c| c.abs_diff(ms)).unwrap_or(1000)
}

pub fn refresh_from_index(i: i32) -> u32 {
    REFRESH_CHOICES_MS[(i.max(0) as usize).min(REFRESH_CHOICES_MS.len() - 1)]
}

pub fn effective_dark(theme: ThemeChoice, system_dark: bool) -> bool {
    match theme {
        ThemeChoice::System => system_dark,
        ThemeChoice::Light => false,
        ThemeChoice::Dark => true,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseAction {
    HideToTray,
    Exit,
}

/// What closing the window does. `--exit-on-close` (benchmark use) always exits.
pub fn close_action(keep_in_tray: bool, exit_on_close: bool) -> CloseAction {
    if keep_in_tray && !exit_on_close { CloseAction::HideToTray } else { CloseAction::Exit }
}

/// The settings file: `%LOCALAPPDATA%\Negotium\settings.json` (the temp directory if
/// `LOCALAPPDATA` is unset). The benchmark gives Negotium a scratch LOCALAPPDATA with settings of
/// its own there, so it never reads or changes the user's. Elevated, LocalAppData is its known
/// folder, whatever the environment says (crate::appdata).
pub fn default_path() -> PathBuf {
    crate::appdata::negotium_dir().join("settings.json")
}

/// [`default_path`] from LOCALAPPDATA's value.
pub fn path_from(local_app_data: Option<std::ffi::OsString>) -> PathBuf {
    local_app_data.map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("Negotium").join("settings.json")
}

#[derive(Debug, PartialEq)]
pub enum Loaded {
    Defaults,
    File,
    /// The file was unusable (the reason) and was renamed to settings.json.bad (elevated: copied
    /// there, the original left for the next save to replace).
    Recovered(String),
    /// Some fields were invalid (their names) and took their defaults; the valid ones were kept.
    /// The original text was copied to settings.json.bad, so it survives the next save.
    Partial(Vec<String>),
    /// As `Partial`, but the original could not be copied to settings.json.bad (the reason): the
    /// next save replaces it.
    PartialNotKept(Vec<String>, String),
    /// The file could not be read or set aside (the reason): defaults are used and the file is
    /// left untouched.
    Unreadable(String),
}

pub fn load(path: &Path) -> (Settings, Loaded) {
    load_with(&crate::appdata::writes(), path)
}

/// [`load`], setting a bad file aside through `writes` (elevated: copied, never renamed).
pub fn load_with(writes: &Writes, path: &Path) -> (Settings, Loaded) {
    // Elevated, read through the guarded open (no link followed), like the save.
    let read = writes.read(path).and_then(|bytes| {
        String::from_utf8(bytes).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    });
    let text = match read {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Settings::default(), Loaded::Defaults),
        Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
            return (Settings::default(), set_aside(writes, path, &e.to_string()));
        }
        // A sharing violation or missing permission says nothing about the file's content.
        Err(e) => return (Settings::default(), Loaded::Unreadable(e.to_string())),
    };
    let fields = match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(serde_json::Value::Object(fields)) => fields,
        Ok(_) => return (Settings::default(), set_aside(writes, path, "the file is not a JSON object")),
        Err(e) => return (Settings::default(), set_aside(writes, path, &e.to_string())),
    };
    let mut bad: Vec<String> = Vec::new();
    let mut s = Settings::default();
    // Each known field on its own: an invalid one takes its default and the rest are kept.
    macro_rules! field {
        ($name:ident) => {
            if let Some(v) = fields.get(stringify!($name)) {
                match serde_json::from_value(v.clone()) {
                    Ok(x) => s.$name = x,
                    Err(_) => bad.push(stringify!($name).to_string()),
                }
            }
        };
    }
    field!(refresh_ms);
    field!(theme);
    field!(always_on_top);
    field!(keep_in_tray);
    field!(gpu_rendering);
    field!(smooth_graphs);
    field!(tray_cpu);
    field!(sort_column);
    // A well-formed string can still name a column this version doesn't have.
    if !SORT_NAMES.contains(&s.sort_column.as_str()) {
        s.sort_column = Settings::default().sort_column;
        bad.push("sort_column".to_string());
    }
    field!(sort_descending);
    field!(group_by_type);
    field!(check_updates);
    field!(start_with_windows);
    field!(col_pid);
    field!(col_proc_name);
    field!(col_status);
    field!(col_threads);
    field!(col_handles);
    field!(col_publisher);
    let s = s.normalized();
    if bad.is_empty() {
        (s, Loaded::File)
    } else {
        match copy_aside(writes, path) {
            Ok(()) => (s, Loaded::Partial(bad)),
            Err(e) => (s, Loaded::PartialNotKept(bad, e.to_string())),
        }
    }
}

/// Moves an unusable file to settings.json.bad (replacing an older one); elevated, copies it
/// there instead (nothing is renamed: [`copy_aside`]). If that fails, the result says so instead
/// of claiming the file was set aside.
fn set_aside(writes: &Writes, path: &Path, reason: &str) -> Loaded {
    let moved = if writes.may_move() { std::fs::rename(path, path.with_extension("json.bad")) } else { copy_aside(writes, path) };
    match moved {
        Ok(()) => Loaded::Recovered(reason.to_string()),
        Err(e) => Loaded::Unreadable(format!("{reason}; it could not be set aside: {e}")),
    }
}

/// Copies the file's bytes to settings.json.bad (replacing an older one) through `writes`, read
/// through it too (elevated: a linked file is never read, so never copied into the user's folder).
fn copy_aside(writes: &Writes, path: &Path) -> std::io::Result<()> {
    let bytes = writes.read(path)?;
    writes.write_all(&path.with_extension("json.bad"), &bytes)
}

/// Writes atomically: a temporary file next to the target, flushed, then a rename over it. The
/// fixed `.tmp` name relies on Negotium being a single-instance app. Elevated, written in place
/// through the guarded write instead (no rename: crate::appdata).
pub fn save(path: &Path, s: &Settings) -> std::io::Result<()> {
    save_with(&crate::appdata::writes(), path, s)
}

/// [`save`] through `writes`.
pub fn save_with(writes: &Writes, path: &Path, s: &Settings) -> std::io::Result<()> {
    use std::io::Write;
    let json = serde_json::to_vec_pretty(s).map_err(std::io::Error::other)?;
    if !writes.may_move() {
        return writes.write_all(path, &json);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    let mut f = std::fs::File::create(&tmp)?;
    f.write_all(&json)?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, path)
}

/// Saves off the UI thread. Overlapping saves write in order and the newest settings win. Every
/// write it starts (and any other started with `run`) is counted, so `flush` can wait for them
/// before Negotium exits or restarts: the next instance then reads the new state.
#[derive(Clone)]
pub struct Saver {
    path: PathBuf,
    writes: Writes,
    pending: Arc<Mutex<Option<Settings>>>,
    write: Arc<Mutex<()>>,
    in_flight: Arc<(Mutex<usize>, Condvar)>,
}

/// Counts a background write until it ends, panicking or not.
struct InFlight(Arc<(Mutex<usize>, Condvar)>);

impl InFlight {
    fn start(count: &Arc<(Mutex<usize>, Condvar)>) -> Self {
        *count.0.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        Self(count.clone())
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        let mut n = self.0.0.lock().unwrap_or_else(|e| e.into_inner());
        *n = n.saturating_sub(1);
        self.0.1.notify_all();
    }
}

impl Saver {
    /// Saves to `path` as this process writes there (`appdata::writes`).
    pub fn new(path: PathBuf) -> Self {
        Self::with_writes(path, crate::appdata::writes())
    }

    /// Saves to `path` through `writes` (tests choose, so they behave the same elevated or not).
    pub fn with_writes(path: PathBuf, writes: Writes) -> Self {
        Self { path, writes, pending: Default::default(), write: Default::default(), in_flight: Default::default() }
    }

    /// How this saver writes (other writes that go with the settings use it too).
    pub fn writes(&self) -> &Writes {
        &self.writes
    }

    pub fn save(&self, s: Settings) -> std::thread::JoinHandle<()> {
        *self.pending.lock().unwrap_or_else(|e| e.into_inner()) = Some(s);
        let (path, writes, pending, write) = (self.path.clone(), self.writes.clone(), self.pending.clone(), self.write.clone());
        self.run(move || {
            let _writing = write.lock().unwrap_or_else(|e| e.into_inner());
            let next = pending.lock().unwrap_or_else(|e| e.into_inner()).take();
            if let Some(s) = next
                && let Err(e) = save_with(&writes, &path, &s)
            {
                crate::log::Log::new(crate::log::Log::default_path()).warn(&format!("settings not saved: {e}"));
            }
        })
    }

    /// Runs another write that must land before an exit (the GPU fallback marker) on its own
    /// thread, counted for `flush`.
    pub fn run(&self, f: impl FnOnce() + Send + 'static) -> std::thread::JoinHandle<()> {
        // Counted before the thread starts, so a flush right after this call waits for it.
        let counted = InFlight::start(&self.in_flight);
        std::thread::spawn(move || {
            let _counted = counted;
            f();
        })
    }

    /// Waits up to `timeout` for every write started so far; true if they all ended.
    pub fn flush(&self, timeout: Duration) -> bool {
        let (count, cv) = &*self.in_flight;
        let n = count.lock().unwrap_or_else(|e| e.into_inner());
        let (n, _) = cv.wait_timeout_while(n, timeout, |n| *n > 0).unwrap_or_else(|e| e.into_inner());
        *n == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The benchmark writes these keys (crates/bench `APP_FIELDS` / `settings_json`); the tolerant
    /// loader silently ignores a renamed one, so this pins them.
    #[test]
    fn the_bench_settings_keys_are_fields_the_app_loads() {
        let dir = std::env::temp_dir().join(format!("negotium-settings-keys-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(&path, r#"{"smooth_graphs":false,"tray_cpu":false,"keep_in_tray":false}"#).unwrap();
        let (s, loaded) = load_with(&Writes::Plain, &path);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(loaded, Loaded::File);
        assert!(!s.smooth_graphs && !s.tray_cpu && !s.keep_in_tray, "{s:?}");
    }

    /// The only settings file is the one under LOCALAPPDATA: the benchmark gives Negotium a
    /// scratch LOCALAPPDATA and writes its settings there (crates/bench `childenv::settings_path`),
    /// so a relaunch keeps them too. Nothing else redirects the shipping app's settings.
    #[test]
    fn the_settings_file_is_under_local_app_data() {
        use std::ffi::OsString;
        assert_eq!(path_from(Some(OsString::from(r"C:\L"))), PathBuf::from(r"C:\L\Negotium\settings.json"));
        assert_eq!(path_from(None), std::env::temp_dir().join("Negotium").join("settings.json"));
    }

    /// Elevated: a bad file is copied aside (never renamed), and saves overwrite the file in place
    /// (no temporary file renamed over it), through the guarded write.
    #[test]
    fn elevated_nothing_is_renamed() {
        let dir = TempDir::new("elevated");
        let guarded = Writes::Guarded(Some(dir.0.clone()));
        std::fs::write(dir.file(), b"not json").unwrap();
        let (s, loaded) = load_with(&guarded, &dir.file());
        assert!(matches!(loaded, Loaded::Recovered(_)), "{loaded:?}");
        assert_eq!(s, Settings::default());
        assert_eq!(std::fs::read(dir.file().with_extension("json.bad")).unwrap(), b"not json");
        assert!(dir.file().exists(), "the original stays (the next save replaces it)");
        // Partly invalid: copied aside too.
        std::fs::write(dir.file(), br#"{"refresh_ms":"fast"}"#).unwrap();
        let (_, loaded) = load_with(&guarded, &dir.file());
        assert_eq!(loaded, Loaded::Partial(vec!["refresh_ms".into()]));
        assert_eq!(std::fs::read(dir.file().with_extension("json.bad")).unwrap(), br#"{"refresh_ms":"fast"}"#);
        // Saved in place, read back.
        let changed = Settings { always_on_top: true, ..Settings::default() };
        save_with(&guarded, &dir.file(), &changed).unwrap();
        assert!(!dir.file().with_extension("json.tmp").exists(), "no temporary file");
        assert_eq!(load_with(&guarded, &dir.file()), (changed.normalized(), Loaded::File));
    }

    /// Elevated: a settings.json that is another name of some other file (here a hard link to a
    /// "secret" that isn't JSON) is neither read nor copied to settings.json.bad; defaults are used
    /// and both files stay as they were.
    #[test]
    fn elevated_a_linked_file_is_never_read_or_copied_aside() {
        let dir = TempDir::new("elevated-link");
        let elsewhere = TempDir::new("elevated-link-target");
        let secret = elsewhere.0.join("secret.txt");
        std::fs::write(&secret, b"admin-only secret").unwrap();
        std::fs::hard_link(&secret, dir.file()).unwrap();
        let guarded = Writes::Guarded(Some(dir.0.clone()));
        let (s, loaded) = load_with(&guarded, &dir.file());
        assert!(matches!(loaded, Loaded::Unreadable(_)), "{loaded:?}");
        assert_eq!(s, Settings::default());
        assert!(!dir.file().with_extension("json.bad").exists(), "nothing copied aside");
        assert_eq!(std::fs::read(&secret).unwrap(), b"admin-only secret");
        // Not elevated, the same user's own link is read (and set aside) as before.
        let (_, loaded) = load_with(&Writes::Plain, &dir.file());
        assert!(matches!(loaded, Loaded::Recovered(_)), "{loaded:?}");
    }

    /// A fresh directory under the temp dir, removed when dropped (also when a test fails).
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let d = std::env::temp_dir().join(format!("negotium-settings-test-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            TempDir(d)
        }

        fn file(&self) -> PathBuf {
            self.0.join("settings.json")
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_missing_file_means_defaults_and_the_tray_is_on_by_default() {
        let dir = TempDir::new("missing");
        let (s, how) = load_with(&Writes::Plain, &dir.file());
        assert_eq!(how, Loaded::Defaults);
        assert_eq!(s, Settings::default());
        assert!(s.keep_in_tray, "spec 5.4: keep running in tray defaults on");
        assert!(s.gpu_rendering, "the GPU renderer is the default");
        assert_eq!((s.refresh_ms, s.theme, s.always_on_top), (1000, ThemeChoice::System, false));
    }

    #[test]
    fn smooth_graphs_default_on_and_a_bad_value_loses_only_itself() {
        let dir = TempDir::new("smooth");
        let p = dir.file();
        std::fs::write(&p, r#"{"refresh_ms": 2000}"#).unwrap();
        assert!(load_with(&Writes::Plain, &p).0.smooth_graphs, "a file that predates the switch: on");
        std::fs::write(&p, r#"{"smooth_graphs": "yes", "refresh_ms": 2000}"#).unwrap();
        let (s, how) = load_with(&Writes::Plain, &p);
        assert_eq!(how, Loaded::Partial(vec!["smooth_graphs".to_string()]));
        assert!(s.smooth_graphs && s.refresh_ms == 2000);
        std::fs::write(&p, r#"{"smooth_graphs": false}"#).unwrap();
        assert!(!load_with(&Writes::Plain, &p).0.smooth_graphs);
    }

    #[test]
    fn new_fields_default_and_load_tolerantly() {
        let dir = TempDir::new("installer-fields");
        let p = dir.file();
        std::fs::write(&p, r#"{"refresh_ms": 2000}"#).unwrap();
        let (s, how) = load_with(&Writes::Plain, &p);
        assert_eq!(how, Loaded::File);
        assert!(s.check_updates && !s.start_with_windows, "a file that predates them: updates on, start off");
        std::fs::write(&p, r#"{"check_updates": "yes", "start_with_windows": 1, "refresh_ms": 2000}"#).unwrap();
        let (s, how) = load_with(&Writes::Plain, &p);
        assert_eq!(how, Loaded::Partial(vec!["check_updates".to_string(), "start_with_windows".to_string()]));
        assert!(s.check_updates && !s.start_with_windows && s.refresh_ms == 2000);
        std::fs::write(&p, r#"{"check_updates": false, "start_with_windows": true}"#).unwrap();
        let s = load_with(&Writes::Plain, &p).0;
        assert!(!s.check_updates && s.start_with_windows);
    }

    #[test]
    fn tray_cpu_defaults_on_and_a_bad_value_loses_only_itself() {
        let dir = TempDir::new("traycpu");
        let p = dir.file();
        std::fs::write(&p, r#"{"keep_in_tray": false}"#).unwrap();
        assert!(load_with(&Writes::Plain, &p).0.tray_cpu, "a file that predates the switch: on");
        std::fs::write(&p, r#"{"tray_cpu": 1, "keep_in_tray": false}"#).unwrap();
        let (s, how) = load_with(&Writes::Plain, &p);
        assert_eq!(how, Loaded::Partial(vec!["tray_cpu".to_string()]));
        assert!(s.tray_cpu && !s.keep_in_tray);
    }

    #[test]
    fn sort_and_grouping_default_and_load_tolerantly() {
        use negotium_model::{Metric, Sort, SortColumn};
        let dir = TempDir::new("sort");
        let p = dir.file();
        std::fs::write(&p, r#"{"refresh_ms": 2000}"#).unwrap();
        let (s, how) = load_with(&Writes::Plain, &p);
        assert_eq!(how, Loaded::File);
        assert_eq!(s.sort(), Sort { column: SortColumn::Metric(Metric::Cpu), descending: true });
        assert!(s.group_by_type, "a file that predates the switch: grouped");
        std::fs::write(&p, r#"{"sort_column": "banana", "sort_descending": false, "group_by_type": false}"#).unwrap();
        let (s, how) = load_with(&Writes::Plain, &p);
        assert_eq!(how, Loaded::Partial(vec!["sort_column".to_string()]));
        assert_eq!(s.sort(), Sort { column: SortColumn::Metric(Metric::Cpu), descending: false }, "only the column is lost");
        assert!(!s.group_by_type);
        let mut s = Settings::default();
        s.set_sort(Sort { column: SortColumn::Name, descending: false });
        s.group_by_type = false;
        assert_eq!((s.sort_column.as_str(), s.sort_descending), ("name", false));
        save_with(&Writes::Plain, &p, &s).unwrap();
        let (back, how) = load_with(&Writes::Plain, &p);
        assert_eq!((how, back.sort(), back.group_by_type), (Loaded::File, Sort { column: SortColumn::Name, descending: false }, false));
        for m in Metric::ALL {
            s.set_sort(Sort::by(SortColumn::Metric(m)));
            assert_eq!(s.sort(), Sort::by(SortColumn::Metric(m)));
        }
    }

    #[test]
    fn settings_round_trip_as_readable_json() {
        let dir = TempDir::new("roundtrip");
        let p = dir.file();
        let s = Settings {
            refresh_ms: 2000,
            theme: ThemeChoice::Dark,
            always_on_top: true,
            keep_in_tray: false,
            gpu_rendering: false,
            smooth_graphs: false,
            tray_cpu: false,
            sort_column: "memory".to_string(),
            sort_descending: false,
            group_by_type: false,
            check_updates: false,
            start_with_windows: true,
            ..Settings::default()
        };
        save_with(&Writes::Plain, &p, &s).unwrap();
        assert!(std::fs::read_to_string(&p).unwrap().contains("\"theme\": \"dark\""));
        assert_eq!(load_with(&Writes::Plain, &p), (s, Loaded::File));
        assert!(!p.with_extension("json.tmp").exists(), "the temporary file is renamed into place");
    }

    #[test]
    fn a_corrupt_file_is_set_aside_and_defaults_are_used() {
        let dir = TempDir::new("corrupt");
        let p = dir.file();
        std::fs::write(&p, "{\"refresh_ms\": 20").unwrap();
        let (s, how) = load_with(&Writes::Plain, &p);
        assert_eq!(s, Settings::default());
        assert!(matches!(how, Loaded::Recovered(_)), "{how:?}");
        assert_eq!(std::fs::read_to_string(p.with_extension("json.bad")).unwrap(), "{\"refresh_ms\": 20");
        assert!(!p.exists());
        assert_eq!(load_with(&Writes::Plain, &p).1, Loaded::Defaults, "the next start is clean");
    }

    #[test]
    fn a_second_broken_file_replaces_the_first_bad_one() {
        let dir = TempDir::new("twice");
        let p = dir.file();
        std::fs::write(&p, "first").unwrap();
        load_with(&Writes::Plain, &p);
        std::fs::write(&p, "second").unwrap();
        assert!(matches!(load_with(&Writes::Plain, &p).1, Loaded::Recovered(_)));
        assert_eq!(std::fs::read_to_string(p.with_extension("json.bad")).unwrap(), "second");
    }

    #[test]
    fn foreign_and_binary_files_are_set_aside_too() {
        for (name, bytes) in [
            ("empty", &b""[..]),
            ("array", &b"[1, 2, 3]"[..]),
            ("text", &b"not json at all"[..]),
            ("binary", &[0xff, 0xfe, 0x00, 0x80][..]),
        ] {
            let dir = TempDir::new(&format!("foreign-{name}"));
            let p = dir.file();
            std::fs::write(&p, bytes).unwrap();
            let (s, how) = load_with(&Writes::Plain, &p);
            assert_eq!(s, Settings::default(), "{name}");
            assert!(matches!(how, Loaded::Recovered(_)), "{name}: {how:?}");
            assert!(p.with_extension("json.bad").exists(), "{name}");
        }
    }

    #[test]
    fn a_bad_field_loses_only_itself() {
        let dir = TempDir::new("wrongtype");
        let p = dir.file();
        let text = r#"{"refresh_ms": "fast", "theme": "dark", "always_on_top": true, "gpu_rendering": false}"#;
        std::fs::write(&p, text).unwrap();
        let (s, how) = load_with(&Writes::Plain, &p);
        assert_eq!(how, Loaded::Partial(vec!["refresh_ms".to_string()]));
        assert_eq!(
            s,
            Settings { theme: ThemeChoice::Dark, always_on_top: true, gpu_rendering: false, ..Settings::default() }
        );
        assert_eq!(std::fs::read_to_string(p.with_extension("json.bad")).unwrap(), text, "the original survives");
        assert!(p.exists(), "the file itself is not removed");
    }

    #[test]
    fn a_bad_field_whose_original_cannot_be_kept_says_so() {
        let dir = TempDir::new("nocopy");
        let p = dir.file();
        std::fs::write(&p, r#"{"theme": 7, "refresh_ms": 2000}"#).unwrap();
        std::fs::create_dir(p.with_extension("json.bad")).unwrap(); // the copy cannot land
        let (s, how) = load_with(&Writes::Plain, &p);
        assert_eq!(s.refresh_ms, 2000);
        match how {
            Loaded::PartialNotKept(fields, why) => {
                assert_eq!(fields, vec!["theme".to_string()]);
                assert!(!why.is_empty());
            }
            other => panic!("expected PartialNotKept, got {other:?}"),
        }
    }

    #[test]
    fn a_theme_from_a_newer_version_keeps_the_other_settings() {
        let dir = TempDir::new("sepia");
        let p = dir.file();
        std::fs::write(&p, r#"{"theme": "sepia", "refresh_ms": 2000, "keep_in_tray": false}"#).unwrap();
        let (s, how) = load_with(&Writes::Plain, &p);
        assert_eq!(how, Loaded::Partial(vec!["theme".to_string()]));
        assert_eq!(s, Settings { refresh_ms: 2000, keep_in_tray: false, ..Settings::default() });
    }

    #[test]
    fn gpu_rendering_off_survives_a_bad_sibling() {
        let dir = TempDir::new("gpusibling");
        let p = dir.file();
        std::fs::write(&p, r#"{"gpu_rendering": false, "theme": 7, "always_on_top": "yes"}"#).unwrap();
        let (s, how) = load_with(&Writes::Plain, &p);
        assert!(!s.gpu_rendering);
        assert_eq!(how, Loaded::Partial(vec!["theme".to_string(), "always_on_top".to_string()]));
    }

    #[test]
    fn a_file_that_cannot_be_read_is_left_alone() {
        let dir = TempDir::new("unreadable");
        let p = dir.file();
        std::fs::create_dir(&p).unwrap(); // reading a directory is an I/O error, not bad content
        let (s, how) = load_with(&Writes::Plain, &p);
        assert_eq!(s, Settings::default());
        assert!(matches!(how, Loaded::Unreadable(_)), "{how:?}");
        assert!(p.is_dir() && !p.with_extension("json.bad").exists());
    }

    #[test]
    fn unknown_fields_are_ignored_and_missing_ones_default() {
        let dir = TempDir::new("partial");
        let p = dir.file();
        std::fs::write(&p, r#"{"theme": "light", "admin_mode": "no-prompt"}"#).unwrap();
        let (s, how) = load_with(&Writes::Plain, &p);
        assert_eq!(how, Loaded::File);
        assert_eq!(s, Settings { theme: ThemeChoice::Light, ..Settings::default() });
        assert!(!p.with_extension("json.bad").exists());
    }

    #[test]
    fn a_file_from_a_newer_version_keeps_the_fields_this_version_knows() {
        let dir = TempDir::new("newer");
        let p = dir.file();
        std::fs::write(
            &p,
            r#"{"version": 7, "refresh_ms": 2000, "gpu_rendering": false,
                "columns": {"cpu": true}, "startup": {"sort": ["name", "asc"]}}"#,
        )
        .unwrap();
        let (s, how) = load_with(&Writes::Plain, &p);
        assert_eq!(how, Loaded::File);
        assert_eq!(s, Settings { refresh_ms: 2000, gpu_rendering: false, ..Settings::default() });
    }

    #[test]
    fn gpu_rendering_defaults_on_for_a_file_that_predates_it() {
        let dir = TempDir::new("predates");
        let p = dir.file();
        std::fs::write(&p, r#"{"refresh_ms": 500, "keep_in_tray": false}"#).unwrap();
        let (s, how) = load_with(&Writes::Plain, &p);
        assert_eq!(how, Loaded::File);
        assert!(s.gpu_rendering);
        assert!(!s.keep_in_tray);
    }

    #[test]
    fn odd_refresh_values_snap_to_the_nearest_choice() {
        assert_eq!(nearest_refresh(3), 500);
        assert_eq!(nearest_refresh(700), 500);
        assert_eq!(nearest_refresh(1600), 2000);
        assert_eq!(nearest_refresh(60_000), 5000);
        let dir = TempDir::new("snap");
        let p = dir.file();
        std::fs::write(&p, r#"{"refresh_ms": 3}"#).unwrap();
        let (s, _) = load_with(&Writes::Plain, &p);
        assert_eq!(s.refresh_ms, 500);
        assert_eq!(s.refresh_index(), 0);
        assert_eq!(refresh_from_index(3), 5000);
        assert_eq!(refresh_from_index(-1), 500);
    }

    #[test]
    fn the_newest_save_wins_when_saves_overlap() {
        let dir = TempDir::new("overlap");
        let p = dir.file();
        let saver = Saver::with_writes(p.clone(), Writes::Plain);
        let a = saver.save(Settings { refresh_ms: 500, ..Settings::default() });
        let b = saver.save(Settings { refresh_ms: 5000, ..Settings::default() });
        a.join().unwrap();
        b.join().unwrap();
        assert_eq!(load_with(&Writes::Plain, &p).0.refresh_ms, 5000);
    }

    #[test]
    fn a_flush_lands_pending_writes_before_exit() {
        let dir = TempDir::new("flush");
        let p = dir.file();
        let saver = Saver::with_writes(p.clone(), Writes::Plain);
        // Fire and forget, as the Settings page does.
        drop(saver.save(Settings { refresh_ms: 2000, ..Settings::default() }));
        let marker = dir.0.join("marker");
        {
            let marker = marker.clone();
            saver.run(move || {
                std::thread::sleep(std::time::Duration::from_millis(150));
                std::fs::write(marker, b"x").unwrap();
            });
        }
        assert!(saver.clone().flush(std::time::Duration::from_secs(5)), "everything landed in time");
        assert_eq!(load_with(&Writes::Plain, &p).0.refresh_ms, 2000, "the pending save is on disk");
        assert!(marker.is_file(), "the other background write landed too");
        assert!(saver.flush(std::time::Duration::ZERO), "nothing left: a flush returns at once");
    }

    #[test]
    fn a_panicking_run_closure_still_lets_flush_return() {
        let saver = Saver::with_writes(PathBuf::from("unused.json"), Writes::Plain);
        crate::worker::quiet_expected_panics();
        let h = saver.run(|| panic!("test panic: settings write"));
        assert!(h.join().is_err(), "the closure did panic");
        assert!(saver.flush(std::time::Duration::from_secs(5)), "the panicked write no longer counts");
    }

    #[test]
    fn a_flush_is_bounded() {
        let saver = Saver::with_writes(std::env::temp_dir().join("negotium-unused-settings.json"), Writes::Plain);
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        saver.run(move || {
            let _ = rx.recv(); // until released below
        });
        let start = std::time::Instant::now();
        assert!(!saver.flush(std::time::Duration::from_millis(100)), "a stuck write doesn't hold the exit");
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
        drop(tx);
        assert!(saver.flush(std::time::Duration::from_secs(5)));
    }

    #[test]
    fn theme_and_close_rules() {
        assert!(effective_dark(ThemeChoice::System, true));
        assert!(!effective_dark(ThemeChoice::System, false));
        assert!(!effective_dark(ThemeChoice::Light, true));
        assert!(effective_dark(ThemeChoice::Dark, false));
        assert_eq!(ThemeChoice::from_index(2), ThemeChoice::Dark);
        assert_eq!(ThemeChoice::from_index(9), ThemeChoice::Dark);
        assert_eq!(close_action(true, false), CloseAction::HideToTray);
        assert_eq!(close_action(false, false), CloseAction::Exit);
        assert_eq!(close_action(true, true), CloseAction::Exit, "--exit-on-close (benchmark) always exits");
    }
}
