//! Startup page (spec §5.3): the view model (name, publisher, status and location, sorted by
//! name), and the jobs behind it. Listing and toggling call the registry, the file system, COM
//! and WinRT, so they run on action threads; results come back through the bridge.
use crate::MainWindow;
use crate::bridge::Bridge;
use negotium_actions::startup::{StartupEntry, StartupRoots, is_read_error, list_startup, set_startup_enabled};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct StartupRow {
    /// Index of the entry in the list the row came from.
    pub id: i32,
    pub name: String,
    pub publisher: String,
    pub status: String,
    pub location: String,
    pub command: String,
    pub enabled: bool,
    /// The switch works: not fixed by policy, and elevated when the entry is for all users.
    pub can_change: bool,
    /// Why it can't change ("Set by policy", "Needs admin"), or empty.
    pub note: String,
}

/// The Status column's text.
pub fn status_text(enabled: bool) -> &'static str {
    if enabled { "Enabled" } else { "Disabled" }
}

pub fn startup_rows(entries: &[StartupEntry], elevated: bool) -> Vec<StartupRow> {
    let mut rows: Vec<StartupRow> = entries
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let needs_admin = e.source.needs_admin() && !elevated;
            StartupRow {
                id: i as i32,
                name: e.name.clone(),
                publisher: e.publisher.clone().unwrap_or_default(),
                status: status_text(e.enabled).to_string(),
                location: e.source.label().to_string(),
                command: e.command.clone(),
                enabled: e.enabled,
                can_change: e.changeable && !needs_admin,
                // Policy first: admin rights would not help.
                note: if !e.changeable {
                    "Set by policy"
                } else if needs_admin {
                    "Needs admin"
                } else {
                    ""
                }
                .to_string(),
            }
        })
        .collect();
    rows.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.id.cmp(&b.id)));
    rows
}

/// The inline "Restart as admin" banner: some entry could change, but only with admin rights.
pub fn needs_admin_banner(entries: &[StartupEntry], elevated: bool) -> bool {
    !elevated && entries.iter().any(|e| e.source.needs_admin() && e.changeable)
}

/// What a startup job hands back: the entries, the lines `list_startup` couldn't read or skipped,
/// and the inline failure message of a toggle.
pub type JobResult = (Vec<StartupEntry>, Vec<String>, Option<String>);

/// Runs on an action thread: re-reads the list so the page shows the real state.
pub fn list_job(roots: &StartupRoots) -> JobResult {
    let (entries, errors) = list_startup(roots);
    (entries, errors, None)
}

/// Runs on an action thread: toggles, then re-reads the list so the page shows the real state
/// either way.
pub fn toggle_job(roots: &StartupRoots, entry: &StartupEntry, enabled: bool, elevated: bool) -> JobResult {
    let failure = set_startup_enabled(roots, entry, enabled).err().map(|e| e.message(&entry.name, elevated));
    let (entries, errors, _) = list_job(roots);
    (entries, errors, failure)
}

// One startup job at a time: `startup-loading` is set from the start of a list or toggle until
// its result lands (`Bridge::set_startup` clears it). A second job could otherwise finish first
// and an older list land last, showing a state that is no longer true.

/// UI thread: starts a list unless a job is running. True if the caller should run it.
pub fn begin_list(ui: &MainWindow) -> bool {
    if ui.get_startup_loading() {
        return false;
    }
    ui.set_startup_loading(true);
    true
}

/// UI thread: starts toggling row `id` unless a job is running (the switch, which only shows the
/// listed state, then stays as it was). Moves the switch at once and returns the entry to toggle.
pub fn begin_toggle(bridge: &Bridge, ui: &MainWindow, id: i32, enabled: bool) -> Option<StartupEntry> {
    if ui.get_startup_loading() {
        return None;
    }
    let entry = bridge.startup_entry(id)?;
    bridge.show_startup_pending(id, enabled);
    ui.set_startup_loading(true);
    Some(entry)
}

/// What lands on the UI thread after a job: the entries (`None`: the job failed outright, the
/// last listed state stays), whether some list, folder or key couldn't be read, and the inline
/// failure message.
#[derive(Debug, PartialEq)]
pub struct JobOutcome {
    pub entries: Option<Vec<StartupEntry>>,
    pub read_error: bool,
    pub failure: Option<String>,
}

/// Shown when a job panicked: nothing changed on screen, and the log has the details.
pub const JOB_FAILED: &str = "Couldn't read the startup apps; details are in the log.";

/// Turns a job's result (or its panic) into what the page shows, logging what it must: each
/// read error (a "Skipped" line is informational and not logged) and a panic's message.
pub fn finish(result: std::thread::Result<JobResult>) -> JobOutcome {
    match result {
        Ok((entries, errors, failure)) => {
            let read_errors: Vec<&String> = errors.iter().filter(|e| is_read_error(e)).collect();
            for e in &read_errors {
                warn(&format!("startup list: {e}"));
            }
            JobOutcome { entries: Some(entries), read_error: !read_errors.is_empty(), failure }
        }
        Err(panic) => {
            let why = panic
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".to_string());
            warn(&format!("startup job panicked: {why}"));
            JobOutcome { entries: None, read_error: false, failure: Some(JOB_FAILED.to_string()) }
        }
    }
}

/// One log for every startup job, so a line already written isn't written again on each open.
fn warn(msg: &str) {
    static LOG: std::sync::OnceLock<std::sync::Mutex<crate::log::Log>> = std::sync::OnceLock::new();
    let log = LOG.get_or_init(|| std::sync::Mutex::new(crate::log::Log::new(crate::log::Log::default_path())));
    log.lock().unwrap_or_else(std::sync::PoisonError::into_inner).warn(msg);
}

/// Runs `job` on an action thread and hands its outcome to `deliver` on the UI thread. The one
/// path every list and toggle takes; a panicking job still delivers, so the page never stays
/// stuck in `startup-loading`.
pub fn spawn_job(weak: slint::Weak<MainWindow>, elevated: bool, job: impl FnOnce() -> JobResult + Send + 'static) {
    std::thread::spawn(move || {
        let outcome = finish(std::panic::catch_unwind(std::panic::AssertUnwindSafe(job)));
        // Fails only once the event loop is gone (the app is quitting): nothing left to show.
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = weak.upgrade() {
                deliver(&ui, outcome, elevated);
            }
        });
    });
}

/// UI thread: a job's outcome lands and the next job may start. The list shows the real state
/// (after an outright failure, the last listed one again, undoing a pending switch); what couldn't
/// be read is noted on the page; a failure says why inline.
pub fn deliver(ui: &MainWindow, outcome: JobOutcome, elevated: bool) {
    crate::bridge::with_bridge(|b| {
        let entries = outcome.entries.unwrap_or_else(|| b.startup_entries().to_vec());
        b.set_startup(ui, entries, elevated);
    });
    ui.set_startup_loading(false);
    ui.set_startup_read_note(outcome.read_error);
    if let Some(message) = outcome.failure {
        ui.set_toast(message.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use negotium_actions::startup::registry::{self, Hive, RegKey};
    use negotium_actions::startup::StartupSource;

    fn entry(source: StartupSource, key: &str, name: &str, enabled: bool, changeable: bool) -> StartupEntry {
        StartupEntry {
            source, key: key.into(), name: name.into(), publisher: None, enabled, changeable,
            command: format!(r"C:\Apps\{key}.exe"), location: "x".into(),
        }
    }

    #[test]
    fn rows_are_sorted_by_name_and_explain_what_cannot_change() {
        let entries = vec![
            entry(StartupSource::UserRun, "zoom", "Zoom", true, true),
            entry(StartupSource::MachineRun, "old", "old tool", false, true),
            entry(StartupSource::Packaged, "pfn\\task", "Contoso", true, false),
        ];
        let rows = startup_rows(&entries, false);
        assert_eq!(rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["Contoso", "old tool", "Zoom"]);
        assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), [2, 1, 0], "ids point back into the entry list");
        assert_eq!((rows[0].can_change, rows[0].note.as_str()), (false, "Set by policy"));
        assert_eq!((rows[1].can_change, rows[1].note.as_str(), rows[1].status.as_str()), (false, "Needs admin", "Disabled"));
        assert_eq!((rows[2].can_change, rows[2].note.as_str(), rows[2].status.as_str()), (true, "", "Enabled"));
        assert_eq!(rows[1].location, "Registry (all users)");
        assert!(needs_admin_banner(&entries, false));
        let elevated = startup_rows(&entries, true);
        assert!(elevated[1].can_change && elevated[1].note.is_empty());
        assert!(!needs_admin_banner(&entries, true));
    }

    #[test]
    fn policy_and_admin_are_told_apart() {
        // A policy-fixed entry that would also need admin: policy wins, elevated or not, and it
        // alone never asks for the admin banner (admin would not help).
        let entries = vec![entry(StartupSource::MachineRun, "fixed", "Fixed", true, false)];
        for elevated in [false, true] {
            let rows = startup_rows(&entries, elevated);
            assert_eq!((rows[0].can_change, rows[0].note.as_str()), (false, "Set by policy"));
        }
        assert!(!needs_admin_banner(&entries, false));
        assert_eq!((status_text(true), status_text(false)), ("Enabled", "Disabled"));
    }

    struct Sandbox(RegKey);

    impl Drop for Sandbox {
        fn drop(&mut self) {
            let _ = registry::delete_test_tree(&self.0);
        }
    }

    #[test]
    fn a_failed_toggle_reports_inline_and_the_list_shows_the_real_state() {
        let base = RegKey::new(Hive::CurrentUser, &format!(r"{}\app-toggle-{}", registry::TEST_ROOT, std::process::id()));
        let _sandbox = Sandbox(base.clone());
        let roots = StartupRoots::under(&base.path);
        // A startup task key as Windows writes it (Task 6): `State` and the task marker.
        let task = roots.packaged.child(r"Contoso.Locked_abc\Boot");
        registry::set_dword(&task, "State", 3).unwrap();
        registry::set_dword(&task, "UserEnabledStartupOnce", 0).unwrap();
        registry::set_string(&roots.user_run, "Tool", r"C:\Nope\tool.exe").unwrap();
        let (entries, _) = list_startup(&roots);
        let locked = entries.iter().find(|e| e.key.starts_with("Contoso.Locked")).unwrap().clone();
        let (after, _, failure) = toggle_job(&roots, &locked, true, false);
        assert_eq!(failure.as_deref(), Some("Couldn't change Contoso.Locked: its state is set by a policy."));
        assert!(!after.iter().find(|e| e.key == locked.key).unwrap().enabled, "still disabled");
        let tool = entries.iter().find(|e| e.key == "Tool").unwrap().clone();
        let (after, _, failure) = toggle_job(&roots, &tool, false, false);
        assert_eq!(failure, None);
        assert!(!after.iter().find(|e| e.key == "Tool").unwrap().enabled);
    }

    #[test]
    fn only_read_errors_are_noted_and_a_panic_still_delivers() {
        let skipped = format!("{}Bad in HKCU Run: it isn't a command", negotium_actions::startup::SKIPPED);
        let ok = finish(Ok((vec![], vec![skipped.clone()], None)));
        assert_eq!(ok, JobOutcome { entries: Some(vec![]), read_error: false, failure: None }, "a skipped value is no read error");
        let unread = finish(Ok((vec![], vec![skipped, "HKLM Run: access denied".into()], Some("x".into()))));
        assert!(unread.read_error);
        assert_eq!(unread.failure.as_deref(), Some("x"));
        let panicked = finish(std::panic::catch_unwind(|| -> JobResult { std::panic::panic_any("boom".to_string()) }));
        assert_eq!(panicked, JobOutcome { entries: None, read_error: false, failure: Some(JOB_FAILED.into()) });
    }
}
