//! Packaged-app startup tasks (MSIX `windows.startupTask`, ruling P3b-1). Windows keeps each task's
//! state in SYSTEM_APP_DATA\<PackageFamilyName>\<TaskId>, DWORD `State`, with the
//! Windows.ApplicationModel.StartupTaskState values: 0 disabled, 1 disabled by user, 2 enabled,
//! 3 disabled by policy, 4 enabled by policy. Display names and publishers come from
//! PackageManager for the current user (no admin), which needs a COM apartment on this thread.
use super::registry::{self, RegKey};
use super::{StartupEntry, StartupSource};
use crate::ActionError;
use std::collections::HashMap;
use windows::Management::Deployment::PackageManager;
use windows::core::HSTRING;

pub const SYSTEM_APP_DATA: &str = r"Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\SystemAppData";

/// Where packaged entries say they live.
const LOCATION: &str = "Packaged app";

/// Per-package AppModel data kept beside the task keys. Only task keys are known to carry a
/// `State` (checked on one machine, not documented), so these never count as tasks.
const NOT_TASKS: &[&str] = &[
    "Schemas",
    "PSR",
    "SplashScreen",
    "HAM",
    "PersistedStorageItemTable",
    "ApplicationFrame",
    "PersistedTitleBarData",
    "AppUriHandlers",
    "PersistedPickerData",
];

pub fn state_enabled(state: u32) -> bool {
    matches!(state, 2 | 4)
}

/// Policy states (3, 4) are fixed.
pub fn state_changeable(state: u32) -> bool {
    state <= 2
}

/// What Task Manager writes: 2 (enabled) or 1 (disabled by user).
pub fn toggled_state(enabled: bool) -> u32 {
    if enabled { 2 } else { 1 }
}

/// The name shown for a family that isn't installed for this user: its name part.
pub fn family_fallback_name(pfn: &str) -> String {
    pfn.split('_').next().unwrap_or(pfn).to_string()
}

/// A task's `State`, when the key is a startup task: a DWORD StartupTaskState (0–4) on a key
/// that isn't other AppModel data.
fn task_state(family: &RegKey, task: &str) -> Option<u32> {
    if NOT_TASKS.iter().any(|n| n.eq_ignore_ascii_case(task)) {
        return None;
    }
    registry::value(&family.child(task), "State")
        .and_then(|v| v.dword())
        .filter(|&s| s <= 4)
}

/// (display name, publisher) of an installed package; `Ok(None)` when the family isn't installed
/// for this user; `Err(line)` with a readable line for `list_startup`'s errors.
type Names = Result<Option<(String, Option<String>)>, String>;

/// PackageManager, made on the first lookup and kept for the listing.
struct PackageNames {
    manager: Option<Result<PackageManager, String>>,
}

impl PackageNames {
    fn new() -> Self {
        Self { manager: None }
    }

    fn lookup(&mut self, pfn: &str) -> Names {
        let manager = self
            .manager
            .get_or_insert_with(|| PackageManager::new().map_err(|e| plain(&e)))
            .as_ref()
            .map_err(|e| format!("Couldn't read the names of packaged apps: {e}"))?;
        let read = || -> windows::core::Result<Option<(String, Option<String>)>> {
            let found = manager.FindPackagesByUserSecurityIdPackageFamilyName(
                &HSTRING::new(),
                &HSTRING::from(pfn),
            )?;
            let Some(package) = found.into_iter().next() else {
                return Ok(None);
            };
            let publisher = package.PublisherDisplayName().map(|p| p.to_string()).ok();
            Ok(Some((package.DisplayName()?.to_string(), publisher)))
        };
        read().map_err(|e| format!("Couldn't read the package of {pfn}: {}", plain(&e)))
    }
}

fn plain(e: &windows::core::Error) -> String {
    let text = e.message();
    let text = text.trim().trim_end_matches('.');
    if text.is_empty() {
        format!("{:#010x}", e.code().0)
    } else {
        text.to_string()
    }
}

/// Every startup task under `packaged`, named through `lookup` (once per family). A family that
/// can't be read, and a failed lookup, add a line to `errors`; the tasks are then listed under
/// their family's name.
fn entries_with(
    packaged: &RegKey,
    mut lookup: impl FnMut(&str) -> Names,
    errors: &mut Vec<String>,
) -> Vec<StartupEntry> {
    let families = match registry::subkeys(packaged) {
        Ok(families) => families,
        Err(e) => {
            errors.push(format!("Couldn't read {}: {e}", packaged.display()));
            return Vec::new();
        }
    };
    let mut named: HashMap<String, (String, Option<String>)> = HashMap::new();
    let mut out = Vec::new();
    for pfn in families {
        let family = packaged.child(&pfn);
        let tasks = match registry::subkeys(&family) {
            Ok(tasks) => tasks,
            Err(e) => {
                errors.push(format!("Couldn't read {}: {e}", family.display()));
                continue;
            }
        };
        for task in tasks {
            let Some(state) = task_state(&family, &task) else {
                continue;
            };
            let (name, publisher) = named
                .entry(pfn.clone())
                .or_insert_with(|| {
                    let found = lookup(&pfn).unwrap_or_else(|line| {
                        if !errors.contains(&line) {
                            errors.push(line);
                        }
                        None
                    });
                    let (name, publisher) = found.unwrap_or_default();
                    let name = Some(name)
                        .filter(|n| !n.is_empty())
                        .unwrap_or_else(|| family_fallback_name(&pfn));
                    (name, publisher.filter(|p| !p.is_empty()))
                })
                .clone();
            out.push(StartupEntry {
                source: StartupSource::Packaged,
                key: format!(r"{pfn}\{task}"),
                name,
                publisher,
                enabled: state_enabled(state),
                changeable: state_changeable(state),
                command: task,
                location: LOCATION.to_string(),
            });
        }
    }
    out
}

/// The real packaged tasks (see `entries_with`), named through PackageManager.
pub(super) fn packaged_entries(packaged: &RegKey, errors: &mut Vec<String>) -> Vec<StartupEntry> {
    let mut names = PackageNames::new();
    entries_with(packaged, |pfn| names.lookup(pfn), errors)
}

/// Writes 2 or 1 to the task's `State`. A task whose state a policy sets (as listed, or as it reads
/// now) is refused and left as it is; so is one whose `State` is gone (its app uninstalled since
/// the listing), which is never written back into being.
pub(super) fn set_enabled(
    packaged: &RegKey,
    entry: &StartupEntry,
    enabled: bool,
) -> Result<(), ActionError> {
    let (pfn, task) = entry
        .key
        .split_once('\\')
        .ok_or_else(|| ActionError::Other("unknown packaged task".into()))?;
    let key = packaged.child(pfn).child(task);
    let Some(now) = registry::value(&key, "State").and_then(|v| v.dword()) else {
        return Err(ActionError::Other("it is no longer installed".into()));
    };
    if !entry.changeable || !state_changeable(now) {
        return Err(ActionError::Other("its state is set by a policy".into()));
    }
    registry::set_dword(&key, "State", toggled_state(enabled))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::startup::registry::{Hive, TEST_ROOT};

    #[test]
    fn task_states_follow_the_startup_task_api() {
        assert_eq!(
            [0, 1, 2, 3, 4].map(state_enabled),
            [false, false, true, false, true]
        );
        assert_eq!(
            [0, 1, 2, 3, 4].map(state_changeable),
            [true, true, true, false, false]
        );
        assert_eq!(
            (toggled_state(true), toggled_state(false)),
            (2, 1),
            "Task Manager: enabled / disabled by user"
        );
    }

    #[test]
    fn a_family_that_isnt_installed_is_named_after_itself() {
        assert_eq!(
            family_fallback_name("Contoso.App_8wekyb3d8bbwe"),
            "Contoso.App"
        );
        assert_eq!(family_fallback_name("odd"), "odd");
    }

    /// A sandbox key under the test root, removed on drop.
    struct Sandbox(RegKey);

    impl Sandbox {
        fn new(name: &str) -> Self {
            let key = RegKey::new(
                Hive::CurrentUser,
                &format!(r"{TEST_ROOT}\packaged-unit-{name}-{}", std::process::id()),
            );
            let _ = registry::delete_test_tree(&key);
            Self(key)
        }
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            let _ = registry::delete_test_tree(&self.0);
        }
    }

    #[test]
    fn only_task_keys_with_a_startup_state_are_listed() {
        let sb = Sandbox::new("filter");
        let family = sb.0.child("Contoso.App_abc");
        registry::set_dword(&family.child("Task"), "State", 0).unwrap();
        // AppModel keeps other per-package data beside the tasks; none of it is a startup task.
        registry::set_dword(&family.child("Schemas"), "State", 2).unwrap();
        registry::set_dword(&family.child("SplashScreen"), "State", 2).unwrap();
        registry::set_dword(&family.child("Odd"), "State", 9).unwrap();
        registry::set_string(&family.child("Text"), "State", "2").unwrap();
        let mut errors = Vec::new();
        let entries = entries_with(&sb.0, |_| Ok(None), &mut errors);
        assert!(errors.is_empty(), "{errors:?}");
        let keys: Vec<&str> = entries.iter().map(|e| e.key.as_str()).collect();
        assert_eq!(keys, [r"Contoso.App_abc\Task"]);
        assert!(!entries[0].enabled && entries[0].changeable);
    }

    #[test]
    fn names_come_from_the_package_and_a_failed_lookup_keeps_the_family_name() {
        let sb = Sandbox::new("names");
        registry::set_dword(&sb.0.child(r"Contoso.Named_abc\A"), "State", 2).unwrap();
        registry::set_dword(&sb.0.child(r"Contoso.Named_abc\B"), "State", 1).unwrap();
        registry::set_dword(&sb.0.child(r"Contoso.Broken_abc\T"), "State", 2).unwrap();
        registry::set_dword(&sb.0.child(r"Contoso.Gone_abc\T"), "State", 2).unwrap();
        let mut asked = Vec::new();
        let mut errors = Vec::new();
        let entries = entries_with(
            &sb.0,
            |pfn| {
                asked.push(pfn.to_string());
                match pfn {
                    "Contoso.Named_abc" => {
                        Ok(Some(("Contoso Named".into(), Some("Contoso Ltd".into()))))
                    }
                    "Contoso.Broken_abc" => {
                        Err("Couldn't read the package of Contoso.Broken_abc: boom".into())
                    }
                    _ => Ok(None),
                }
            },
            &mut errors,
        );
        asked.sort();
        assert_eq!(
            asked,
            [
                "Contoso.Broken_abc",
                "Contoso.Gone_abc",
                "Contoso.Named_abc"
            ],
            "one lookup per family"
        );
        assert_eq!(entries.len(), 4, "{entries:?}");
        let by_key = |k: &str| entries.iter().find(|e| e.key == k).unwrap();
        for k in [r"Contoso.Named_abc\A", r"Contoso.Named_abc\B"] {
            let e = by_key(k);
            assert_eq!(
                (e.name.as_str(), e.publisher.as_deref()),
                ("Contoso Named", Some("Contoso Ltd"))
            );
        }
        let broken = by_key(r"Contoso.Broken_abc\T");
        assert_eq!(
            (broken.name.as_str(), broken.publisher.as_deref()),
            ("Contoso.Broken", None)
        );
        let gone = by_key(r"Contoso.Gone_abc\T");
        assert_eq!(
            (gone.name.as_str(), gone.publisher.as_deref()),
            ("Contoso.Gone", None)
        );
        assert_eq!(
            errors,
            ["Couldn't read the package of Contoso.Broken_abc: boom"]
        );
    }

    #[test]
    fn a_manager_that_cant_start_is_reported_once() {
        let mut names = PackageNames {
            manager: Some(Err("no apartment".into())),
        };
        let first = names.lookup("A_x").unwrap_err();
        let second = names.lookup("B_x").unwrap_err();
        assert_eq!(first, second, "the same line: list_startup reports it once");
        assert_eq!(
            first,
            "Couldn't read the names of packaged apps: no apartment"
        );
    }
}
