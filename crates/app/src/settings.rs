//! User settings (spec 5.4) in %LOCALAPPDATA%\Negotium\settings.json. A missing file means
//! defaults. An unreadable or invalid file is moved aside to settings.json.bad and defaults are
//! used: a bad file never stops Negotium from starting. Unknown fields are ignored and missing
//! ones take their defaults, so files from older or newer versions load.
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

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
}

impl Default for Settings {
    fn default() -> Self {
        Self { refresh_ms: 1000, theme: ThemeChoice::System, always_on_top: false, keep_in_tray: true, gpu_rendering: true }
    }
}

impl Settings {
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

pub fn default_path() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("Negotium").join("settings.json")
}

#[derive(Debug, PartialEq)]
pub enum Loaded {
    Defaults,
    File,
    /// The file was unusable (the reason) and was renamed to settings.json.bad.
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
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Settings::default(), Loaded::Defaults),
        Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
            return (Settings::default(), set_aside(path, &e.to_string()));
        }
        // A sharing violation or missing permission says nothing about the file's content.
        Err(e) => return (Settings::default(), Loaded::Unreadable(e.to_string())),
    };
    let fields = match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(serde_json::Value::Object(fields)) => fields,
        Ok(_) => return (Settings::default(), set_aside(path, "the file is not a JSON object")),
        Err(e) => return (Settings::default(), set_aside(path, &e.to_string())),
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
    let s = s.normalized();
    if bad.is_empty() {
        (s, Loaded::File)
    } else {
        match std::fs::copy(path, path.with_extension("json.bad")) {
            Ok(_) => (s, Loaded::Partial(bad)),
            Err(e) => (s, Loaded::PartialNotKept(bad, e.to_string())),
        }
    }
}

/// Moves an unusable file to settings.json.bad (replacing an older one). If that fails, the
/// result says so instead of claiming the file was set aside.
fn set_aside(path: &Path, reason: &str) -> Loaded {
    match std::fs::rename(path, path.with_extension("json.bad")) {
        Ok(()) => Loaded::Recovered(reason.to_string()),
        Err(e) => Loaded::Unreadable(format!("{reason}; it could not be set aside: {e}")),
    }
}

/// Writes atomically: a temporary file next to the target, flushed, then a rename over it. The
/// fixed `.tmp` name relies on Negotium being a single-instance app.
pub fn save(path: &Path, s: &Settings) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    let mut f = std::fs::File::create(&tmp)?;
    f.write_all(&serde_json::to_vec_pretty(s).map_err(std::io::Error::other)?)?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, path)
}

/// Saves off the UI thread. Overlapping saves write in order and the newest settings win.
#[derive(Clone)]
pub struct Saver {
    path: PathBuf,
    pending: Arc<Mutex<Option<Settings>>>,
    write: Arc<Mutex<()>>,
}

impl Saver {
    pub fn new(path: PathBuf) -> Self {
        Self { path, pending: Default::default(), write: Default::default() }
    }

    pub fn save(&self, s: Settings) -> std::thread::JoinHandle<()> {
        *self.pending.lock().unwrap_or_else(|e| e.into_inner()) = Some(s);
        let (path, pending, write) = (self.path.clone(), self.pending.clone(), self.write.clone());
        std::thread::spawn(move || {
            let _writing = write.lock().unwrap_or_else(|e| e.into_inner());
            let next = pending.lock().unwrap_or_else(|e| e.into_inner()).take();
            if let Some(s) = next
                && let Err(e) = save(&path, &s)
            {
                crate::log::Log::new(crate::log::Log::default_path()).warn(&format!("settings not saved: {e}"));
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let (s, how) = load(&dir.file());
        assert_eq!(how, Loaded::Defaults);
        assert_eq!(s, Settings::default());
        assert!(s.keep_in_tray, "spec 5.4: keep running in tray defaults on");
        assert!(s.gpu_rendering, "the GPU renderer is the default");
        assert_eq!((s.refresh_ms, s.theme, s.always_on_top), (1000, ThemeChoice::System, false));
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
        };
        save(&p, &s).unwrap();
        assert!(std::fs::read_to_string(&p).unwrap().contains("\"theme\": \"dark\""));
        assert_eq!(load(&p), (s, Loaded::File));
        assert!(!p.with_extension("json.tmp").exists(), "the temporary file is renamed into place");
    }

    #[test]
    fn a_corrupt_file_is_set_aside_and_defaults_are_used() {
        let dir = TempDir::new("corrupt");
        let p = dir.file();
        std::fs::write(&p, "{\"refresh_ms\": 20").unwrap();
        let (s, how) = load(&p);
        assert_eq!(s, Settings::default());
        assert!(matches!(how, Loaded::Recovered(_)), "{how:?}");
        assert_eq!(std::fs::read_to_string(p.with_extension("json.bad")).unwrap(), "{\"refresh_ms\": 20");
        assert!(!p.exists());
        assert_eq!(load(&p).1, Loaded::Defaults, "the next start is clean");
    }

    #[test]
    fn a_second_broken_file_replaces_the_first_bad_one() {
        let dir = TempDir::new("twice");
        let p = dir.file();
        std::fs::write(&p, "first").unwrap();
        load(&p);
        std::fs::write(&p, "second").unwrap();
        assert!(matches!(load(&p).1, Loaded::Recovered(_)));
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
            let (s, how) = load(&p);
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
        let (s, how) = load(&p);
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
        let (s, how) = load(&p);
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
        let (s, how) = load(&p);
        assert_eq!(how, Loaded::Partial(vec!["theme".to_string()]));
        assert_eq!(s, Settings { refresh_ms: 2000, keep_in_tray: false, ..Settings::default() });
    }

    #[test]
    fn gpu_rendering_off_survives_a_bad_sibling() {
        let dir = TempDir::new("gpusibling");
        let p = dir.file();
        std::fs::write(&p, r#"{"gpu_rendering": false, "theme": 7, "always_on_top": "yes"}"#).unwrap();
        let (s, how) = load(&p);
        assert!(!s.gpu_rendering);
        assert_eq!(how, Loaded::Partial(vec!["theme".to_string(), "always_on_top".to_string()]));
    }

    #[test]
    fn a_file_that_cannot_be_read_is_left_alone() {
        let dir = TempDir::new("unreadable");
        let p = dir.file();
        std::fs::create_dir(&p).unwrap(); // reading a directory is an I/O error, not bad content
        let (s, how) = load(&p);
        assert_eq!(s, Settings::default());
        assert!(matches!(how, Loaded::Unreadable(_)), "{how:?}");
        assert!(p.is_dir() && !p.with_extension("json.bad").exists());
    }

    #[test]
    fn unknown_fields_are_ignored_and_missing_ones_default() {
        let dir = TempDir::new("partial");
        let p = dir.file();
        std::fs::write(&p, r#"{"theme": "light", "admin_mode": "no-prompt"}"#).unwrap();
        let (s, how) = load(&p);
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
        let (s, how) = load(&p);
        assert_eq!(how, Loaded::File);
        assert_eq!(s, Settings { refresh_ms: 2000, gpu_rendering: false, ..Settings::default() });
    }

    #[test]
    fn gpu_rendering_defaults_on_for_a_file_that_predates_it() {
        let dir = TempDir::new("predates");
        let p = dir.file();
        std::fs::write(&p, r#"{"refresh_ms": 500, "keep_in_tray": false}"#).unwrap();
        let (s, how) = load(&p);
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
        let (s, _) = load(&p);
        assert_eq!(s.refresh_ms, 500);
        assert_eq!(s.refresh_index(), 0);
        assert_eq!(refresh_from_index(3), 5000);
        assert_eq!(refresh_from_index(-1), 500);
    }

    #[test]
    fn the_newest_save_wins_when_saves_overlap() {
        let dir = TempDir::new("overlap");
        let p = dir.file();
        let saver = Saver::new(p.clone());
        let a = saver.save(Settings { refresh_ms: 500, ..Settings::default() });
        let b = saver.save(Settings { refresh_ms: 5000, ..Settings::default() });
        a.join().unwrap();
        b.join().unwrap();
        assert_eq!(load(&p).0.refresh_ms, 5000);
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
