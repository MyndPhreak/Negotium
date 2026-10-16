//! Registry helpers for the startup lists: read a key's values and subkeys, write one value.
//! Paths are relative to a hive; a missing key reads as empty. Every write goes through `set`,
//! which refuses anything outside the test sandbox until the app calls `allow_system_writes`, and
//! even then anything but the startup state keys a toggle writes (`may_write`).
use crate::ActionError;
use std::sync::atomic::{AtomicBool, Ordering};
use windows::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS, ERROR_SUCCESS,
    WIN32_ERROR,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_SET_VALUE, REG_BINARY, REG_DWORD,
    REG_EXPAND_SZ, REG_OPTION_NON_VOLATILE, REG_SAM_FLAGS, REG_SZ, REG_VALUE_TYPE, RRF_NOEXPAND,
    RRF_RT_ANY, RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegEnumKeyExW, RegEnumValueW,
    RegGetValueW, RegOpenKeyExW, RegSetValueExW,
};
use windows::core::{HSTRING, PCWSTR, PWSTR};

/// The only place tests may write to, and `delete_test_tree` may delete from: tests' sandboxes.
pub const TEST_ROOT: &str = r"Software\Negotium-Test";

/// Off in every build until the app's `main` turns it on: a process that never unlocks (every
/// test, in any crate) can only write inside the test sandbox.
static SYSTEM_WRITES: AtomicBool = AtomicBool::new(false);

/// Lets this process write outside HKCU\Software\Negotium-Test\…: the real StartupApproved
/// values and packaged task states only (`may_write`). Called once, by the app's `main`; tests never call it.
pub fn allow_system_writes() {
    SYSTEM_WRITES.store(true, Ordering::SeqCst);
}

/// Whether a write to `key` would be attempted: inside the test sandbox always; after
/// `allow_system_writes`, also the startup state keys the app writes (`is_startup_state_key`),
/// and nothing else. Pure: it never touches the registry.
pub fn may_write(key: &RegKey) -> bool {
    may_write_with(key, SYSTEM_WRITES.load(Ordering::SeqCst))
}

/// `may_write` with the unlock given, so both modes can be tested.
fn may_write_with(key: &RegKey, unlocked: bool) -> bool {
    in_test_root(key) || (unlocked && is_startup_state_key(key))
}

/// The keys a startup toggle writes outside the sandbox (defence in depth for the unlock):
/// `…\Explorer\StartupApproved\{Run,Run32,StartupFolder}` under HKCU or HKLM, and a packaged
/// task's key `HKCU\…\AppModel\SystemAppData\<family>\<task>`. Names compare case-insensitively;
/// an empty component (a doubled or trailing backslash) never matches.
fn is_startup_state_key(key: &RegKey) -> bool {
    let parts: Vec<&str> = key.path.split('\\').collect();
    if parts.iter().any(|p| p.is_empty()) {
        return false;
    }
    let under = |prefix: &str, extra: usize| {
        let prefix: Vec<&str> = prefix.split('\\').collect();
        parts.len() == prefix.len() + extra
            && parts.iter().zip(&prefix).all(|(a, b)| a.eq_ignore_ascii_case(b))
    };
    let approved_list = under(super::APPROVED, 1)
        && ["Run", "Run32", "StartupFolder"]
            .iter()
            .any(|l| parts[parts.len() - 1].eq_ignore_ascii_case(l));
    approved_list || (is_packaged_task_key(key) && under(super::packaged::SYSTEM_APP_DATA, 2))
}

/// A packaged task's key (HKCU\…\SystemAppData\<family>\<task>) by its place, not its contents.
fn is_packaged_task_key(key: &RegKey) -> bool {
    let prefix = format!("{}\\", super::packaged::SYSTEM_APP_DATA);
    key.hive == Hive::CurrentUser
        && key
            .path
            .get(..prefix.len())
            .is_some_and(|p| p.eq_ignore_ascii_case(&prefix))
}

/// Value `name` may be written to `key` (once the key itself may be): outside the sandbox, a
/// packaged task's key takes only its `State`.
fn value_allowed(key: &RegKey, name: &str) -> bool {
    in_test_root(key) || !is_packaged_task_key(key) || name.eq_ignore_ascii_case("State")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Hive {
    CurrentUser,
    LocalMachine,
}

impl Hive {
    fn hkey(self) -> HKEY {
        match self {
            Hive::CurrentUser => HKEY_CURRENT_USER,
            Hive::LocalMachine => HKEY_LOCAL_MACHINE,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Hive::CurrentUser => "HKCU",
            Hive::LocalMachine => "HKLM",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegKey {
    pub hive: Hive,
    pub path: String,
}

impl RegKey {
    pub fn new(hive: Hive, path: &str) -> Self {
        Self {
            hive,
            path: path.to_string(),
        }
    }

    pub fn child(&self, name: &str) -> RegKey {
        RegKey {
            hive: self.hive,
            path: format!("{}\\{name}", self.path),
        }
    }

    pub fn display(&self) -> String {
        format!("{}\\{}", self.hive.name(), self.path)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum RegValue {
    Str(String),
    Binary(Vec<u8>),
    Dword(u32),
    Other,
    /// Larger than `MAX_VALUE_BYTES`: skipped by `values`, not read.
    TooLarge,
}

impl RegValue {
    pub fn as_str(&self) -> Option<&str> {
        if let RegValue::Str(s) = self {
            Some(s)
        } else {
            None
        }
    }

    pub fn binary(&self) -> Option<&[u8]> {
        if let RegValue::Binary(b) = self {
            Some(b)
        } else {
            None
        }
    }

    pub fn dword(&self) -> Option<u32> {
        if let RegValue::Dword(d) = self {
            Some(*d)
        } else {
            None
        }
    }

    fn decode(kind: REG_VALUE_TYPE, data: &[u8]) -> RegValue {
        match kind {
            REG_SZ | REG_EXPAND_SZ => {
                let units: Vec<u16> = data
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|&c| u16::from_le_bytes(c))
                    .collect();
                let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
                RegValue::Str(String::from_utf16_lossy(&units[..end]))
            }
            REG_BINARY => RegValue::Binary(data.to_vec()),
            REG_DWORD if data.len() >= 4 => {
                RegValue::Dword(u32::from_le_bytes([data[0], data[1], data[2], data[3]]))
            }
            _ => RegValue::Other,
        }
    }
}

struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: opened by RegOpenKeyExW / RegCreateKeyExW and closed exactly once here.
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

fn error(code: WIN32_ERROR) -> ActionError {
    if code == ERROR_ACCESS_DENIED {
        ActionError::AccessDenied
    } else {
        let text = windows::core::Error::from(code.to_hresult()).message();
        ActionError::Other(text.trim().trim_end_matches('.').to_string())
    }
}

/// A strict subkey of HKCU\Software\Negotium-Test (names compare case-insensitively).
fn in_test_root(key: &RegKey) -> bool {
    let prefix = format!("{TEST_ROOT}\\");
    key.hive == Hive::CurrentUser
        && key
            .path
            .get(..prefix.len())
            .is_some_and(|p| p.eq_ignore_ascii_case(&prefix))
        && !key.path[prefix.len()..].trim_matches('\\').is_empty()
}

/// Refuses a write `may_write` (or, for value `name`, `value_allowed`) rejects.
fn guard_write(key: &RegKey, name: &str) -> Result<(), ActionError> {
    if !may_write(key) || !value_allowed(key, name) {
        return Err(ActionError::Other(format!(
            "writing {} is not allowed in this process",
            key.display()
        )));
    }
    Ok(())
}

fn open(key: &RegKey, access: REG_SAM_FLAGS) -> Result<Option<Key>, ActionError> {
    let mut h = HKEY::default();
    let path = HSTRING::from(key.path.as_str());
    // SAFETY: opens a key into a writable local; the handle is owned (and closed) by `Key`.
    let status = unsafe { RegOpenKeyExW(key.hive.hkey(), &path, None, access, &mut h) };
    match status {
        ERROR_SUCCESS => Ok(Some(Key(h))),
        ERROR_FILE_NOT_FOUND => Ok(None),
        e => Err(error(e)),
    }
}

/// The largest value `values` reads; a larger one is listed as `RegValue::TooLarge`.
pub const MAX_VALUE_BYTES: usize = 1 << 20;

/// Every value of `key`, in Windows' order; one over `MAX_VALUE_BYTES` comes back as
/// `RegValue::TooLarge` without its data.
pub fn values(key: &RegKey) -> Result<Vec<(String, RegValue)>, ActionError> {
    let Some(k) = open(key, KEY_READ)? else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    let mut name = vec![0u16; 16_384]; // the registry's longest value name, plus the terminator
    let mut data = vec![0u8; 64 * 1024];
    let mut index = 0u32;
    loop {
        let mut name_len = name.len() as u32;
        let mut data_len = data.len() as u32;
        let mut kind = 0u32;
        // SAFETY: both buffers are writable for the lengths passed; the lengths are updated.
        let status = unsafe {
            RegEnumValueW(
                k.0,
                index,
                Some(PWSTR(name.as_mut_ptr())),
                &mut name_len,
                None,
                Some(&mut kind),
                Some(data.as_mut_ptr()),
                Some(&mut data_len),
            )
        };
        match status {
            ERROR_SUCCESS => {
                out.push((
                    String::from_utf16_lossy(&name[..name_len as usize]),
                    RegValue::decode(REG_VALUE_TYPE(kind), &data[..data_len as usize]),
                ));
                index += 1;
            }
            // Over the cap: listed by name as `TooLarge` (the name is read again without the
            // data, as the failed call's name buffer isn't guaranteed).
            ERROR_MORE_DATA if data_len as usize > MAX_VALUE_BYTES => {
                let mut name_len = name.len() as u32;
                // SAFETY: `name` is writable for `name_len` units; no data is requested.
                let status = unsafe {
                    RegEnumValueW(
                        k.0,
                        index,
                        Some(PWSTR(name.as_mut_ptr())),
                        &mut name_len,
                        None,
                        None,
                        None,
                        None,
                    )
                };
                if status != ERROR_SUCCESS {
                    return Err(error(status));
                }
                out.push((
                    String::from_utf16_lossy(&name[..name_len as usize]),
                    RegValue::TooLarge,
                ));
                index += 1;
            }
            // A value larger than the buffer: grow it to the size Windows reported (or double it,
            // should the value have grown meanwhile), up to the cap, and read the same index again.
            ERROR_MORE_DATA => {
                let wanted = (data_len as usize).max(data.len() * 2).min(MAX_VALUE_BYTES);
                data.resize(wanted, 0);
            }
            ERROR_NO_MORE_ITEMS => break,
            e => return Err(error(e)),
        }
    }
    Ok(out)
}

/// One value, `None` when the key or the value is missing or unreadable. Names compare
/// case-insensitively, as everywhere in the registry.
pub fn value(key: &RegKey, name: &str) -> Option<RegValue> {
    let (path, name) = (HSTRING::from(key.path.as_str()), HSTRING::from(name));
    let flags = RRF_RT_ANY | RRF_NOEXPAND;
    let mut kind = REG_VALUE_TYPE::default();
    let mut size = 0u32;
    // SAFETY: size query with no buffer; the outputs are writable locals.
    let status = unsafe {
        RegGetValueW(
            key.hive.hkey(),
            &path,
            &name,
            flags,
            Some(&mut kind),
            None,
            Some(&mut size),
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    // Twice at most: a value that grew since the size query reports ERROR_MORE_DATA and its new
    // size, and dropping it would lose its flag bits on the next toggle.
    for _ in 0..2 {
        let mut data = vec![0u8; size as usize];
        // SAFETY: `data` has `size` writable bytes, and `size` says so.
        let status = unsafe {
            RegGetValueW(
                key.hive.hkey(),
                &path,
                &name,
                flags,
                Some(&mut kind),
                Some(data.as_mut_ptr().cast()),
                Some(&mut size),
            )
        };
        match status {
            ERROR_SUCCESS => {
                return Some(RegValue::decode(
                    kind,
                    &data[..(size as usize).min(data.len())],
                ));
            }
            ERROR_MORE_DATA => continue,
            _ => return None,
        }
    }
    None
}

pub fn subkeys(key: &RegKey) -> Result<Vec<String>, ActionError> {
    let Some(k) = open(key, KEY_READ)? else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    let mut name = vec![0u16; 256]; // key names are at most 255 characters
    for index in 0u32.. {
        let mut len = name.len() as u32;
        // SAFETY: `name` is writable for `len` units; the other outputs are not requested.
        let status = unsafe {
            RegEnumKeyExW(
                k.0,
                index,
                Some(PWSTR(name.as_mut_ptr())),
                &mut len,
                None,
                None,
                None,
                None,
            )
        };
        match status {
            ERROR_SUCCESS => out.push(String::from_utf16_lossy(&name[..len as usize])),
            ERROR_NO_MORE_ITEMS => break,
            e => return Err(error(e)),
        }
    }
    Ok(out)
}

/// Writes value `name` into the open key `h`.
fn set_value(h: &Key, name: &str, kind: REG_VALUE_TYPE, data: &[u8]) -> Result<(), ActionError> {
    let name = HSTRING::from(name);
    // SAFETY: valid key handle; the data is passed as a slice.
    let status = unsafe { RegSetValueExW(h.0, &name, None, kind, Some(data)) };
    if status == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(error(status))
    }
}

/// Writes value `name` of `key` (its hive and subkey), creating the key. One of the two writers;
/// both check `guard_write` first.
fn set(key: &RegKey, name: &str, kind: REG_VALUE_TYPE, data: &[u8]) -> Result<(), ActionError> {
    guard_write(key, name)?;
    let mut h = HKEY::default();
    let path = HSTRING::from(key.path.as_str());
    // SAFETY: creates or opens the key for writing into a writable local; the handle is owned by `Key`.
    let status = unsafe {
        RegCreateKeyExW(
            key.hive.hkey(),
            &path,
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut h,
            None,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(error(status));
    }
    set_value(&Key(h), name, kind, data)
}

/// Writes DWORD `name` of `key` only if the key exists: `Ok(false)` when it doesn't, and nothing
/// is created. The other writer; `guard_write` comes first.
pub fn set_existing_dword(key: &RegKey, name: &str, value: u32) -> Result<bool, ActionError> {
    guard_write(key, name)?;
    let Some(h) = open(key, KEY_SET_VALUE)? else {
        return Ok(false);
    };
    set_value(&h, name, REG_DWORD, &value.to_le_bytes())?;
    Ok(true)
}

pub fn set_binary(key: &RegKey, name: &str, data: &[u8]) -> Result<(), ActionError> {
    set(key, name, REG_BINARY, data)
}

pub fn set_dword(key: &RegKey, name: &str, value: u32) -> Result<(), ActionError> {
    set(key, name, REG_DWORD, &value.to_le_bytes())
}

pub fn set_string(key: &RegKey, name: &str, value: &str) -> Result<(), ActionError> {
    let bytes: Vec<u8> = value
        .encode_utf16()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect();
    set(key, name, REG_SZ, &bytes)
}

/// Deletes a test sandbox. Refuses anything but a subkey of HKCU\Software\Negotium-Test.
pub fn delete_test_tree(key: &RegKey) -> Result<(), ActionError> {
    if !in_test_root(key) {
        return Err(ActionError::Other(format!(
            "refusing to delete {}",
            key.display()
        )));
    }
    let path = HSTRING::from(key.path.as_str());
    // SAFETY: deletes a key tree under HKCU that was checked to be a test sandbox.
    match unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, &path) } {
        ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => Ok(()),
        e => Err(error(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Foundation::ERROR_INVALID_PARAMETER;

    #[test]
    fn only_subkeys_of_the_hkcu_test_root_are_writable_in_tests() {
        let key = |hive, path: &str| RegKey::new(hive, path);
        assert!(in_test_root(&key(
            Hive::CurrentUser,
            r"Software\Negotium-Test\a"
        )));
        assert!(in_test_root(&key(
            Hive::CurrentUser,
            r"SOFTWARE\negotium-test\a\b"
        )));
        assert!(!in_test_root(&key(
            Hive::CurrentUser,
            r"Software\Negotium-Test"
        )));
        assert!(!in_test_root(&key(
            Hive::CurrentUser,
            r"Software\Negotium-Test\"
        )));
        assert!(!in_test_root(&key(
            Hive::CurrentUser,
            r"Software\Negotium-TestX\a"
        )));
        assert!(!in_test_root(&key(
            Hive::LocalMachine,
            r"Software\Negotium-Test\a"
        )));
        assert!(!in_test_root(&key(
            Hive::CurrentUser,
            r"Software\Microsoft\Windows\CurrentVersion\Run"
        )));
    }

    const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved";
    const APP_DATA: &str = r"Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\SystemAppData";

    /// The keys the app writes once unlocked: the StartupApproved lists (both hives) and a
    /// packaged task's key (HKCU).
    fn startup_state_keys() -> Vec<RegKey> {
        let mut keys = Vec::new();
        for hive in [Hive::CurrentUser, Hive::LocalMachine] {
            for list in ["Run", "Run32", "StartupFolder"] {
                keys.push(RegKey::new(hive, &format!(r"{APPROVED}\{list}")));
            }
        }
        keys.push(RegKey::new(
            Hive::CurrentUser,
            &format!(r"{APP_DATA}\Contoso.App_8wekyb3d8bbwe\Boot"),
        ));
        // Names compare case-insensitively, as Windows does.
        keys.push(RegKey::new(
            Hive::LocalMachine,
            &format!(r"{}\run", APPROVED.to_uppercase()),
        ));
        keys
    }

    /// Keys never written, unlocked or not.
    fn other_keys() -> Vec<RegKey> {
        let k = RegKey::new;
        vec![
            k(Hive::CurrentUser, r"Software\Microsoft\Windows\CurrentVersion\Run"),
            k(Hive::LocalMachine, r"Software\Microsoft\Windows\CurrentVersion\Run"),
            k(Hive::CurrentUser, APPROVED),
            k(Hive::CurrentUser, &format!(r"{APPROVED}\Run\sub")),
            k(Hive::CurrentUser, &format!(r"{APPROVED}\RunOnce")),
            k(Hive::CurrentUser, &format!(r"{APPROVED}\\Run")),
            k(Hive::CurrentUser, &format!(r"{APPROVED}\Run\")),
            k(Hive::CurrentUser, APP_DATA),
            k(Hive::CurrentUser, &format!(r"{APP_DATA}\Contoso.App_8wekyb3d8bbwe")),
            k(Hive::CurrentUser, &format!(r"{APP_DATA}\Contoso.App_8wekyb3d8bbwe\Boot\Extra")),
            k(Hive::CurrentUser, &format!(r"{APP_DATA}\\Boot")),
            k(Hive::LocalMachine, &format!(r"{APP_DATA}\Contoso.App_8wekyb3d8bbwe\Boot")),
            k(Hive::CurrentUser, r"Software\Policies\Microsoft\Windows"),
            k(Hive::LocalMachine, &format!(r"{TEST_ROOT}\a")),
            k(Hive::CurrentUser, TEST_ROOT),
        ]
    }

    #[test]
    fn locked_only_the_test_root_is_writable() {
        for key in startup_state_keys().iter().chain(&other_keys()) {
            assert!(!may_write_with(key, false), "locked: {}", key.display());
        }
        let sandbox = RegKey::new(Hive::CurrentUser, &format!(r"{TEST_ROOT}\a"));
        assert!(may_write_with(&sandbox, false));
    }

    #[test]
    fn unlocked_only_the_startup_state_keys_and_the_test_root_are_writable() {
        for key in startup_state_keys() {
            assert!(may_write_with(&key, true), "unlocked: {}", key.display());
        }
        for key in other_keys() {
            assert!(!may_write_with(&key, true), "unlocked, still refused: {}", key.display());
        }
        let sandbox = RegKey::new(Hive::CurrentUser, &format!(r"{TEST_ROOT}\a\b"));
        assert!(may_write_with(&sandbox, true));
    }

    #[test]
    fn every_key_a_real_toggle_writes_is_allowed_once_unlocked() {
        use crate::startup::{StartupRoots, StartupSource};
        let roots = StartupRoots::system();
        for source in [
            StartupSource::UserRun,
            StartupSource::MachineRun,
            StartupSource::MachineRun32,
            StartupSource::UserFolder,
            StartupSource::MachineFolder,
        ] {
            let key = roots.approved_key(source).unwrap();
            assert!(may_write_with(&key, true), "{source:?}: {}", key.display());
        }
        let task = roots.packaged.child(r"Contoso.App_8wekyb3d8bbwe\Boot");
        assert!(may_write_with(&task, true) && value_allowed(&task, "State"));
    }

    #[test]
    fn a_packaged_task_key_takes_only_its_state() {
        let task = RegKey::new(
            Hive::CurrentUser,
            &format!(r"{APP_DATA}\Contoso.App_8wekyb3d8bbwe\Boot"),
        );
        assert!(value_allowed(&task, "State") && value_allowed(&task, "state"));
        assert!(!value_allowed(&task, "UserEnabledStartupOnce"));
        let approved = RegKey::new(Hive::CurrentUser, &format!(r"{APPROVED}\Run"));
        assert!(
            value_allowed(&approved, "Any entry name"),
            "StartupApproved values are named after entries"
        );
        let sandbox = RegKey::new(Hive::CurrentUser, &format!(r"{TEST_ROOT}\a\Pkg\Task"));
        assert!(
            value_allowed(&sandbox, "UserEnabledStartupOnce"),
            "tests build task keys in the sandbox"
        );
    }

    #[test]
    fn writes_outside_the_sandbox_are_refused_by_default() {
        let real = RegKey::new(
            Hive::CurrentUser,
            r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run",
        );
        assert!(matches!(guard_write(&real, "x"), Err(ActionError::Other(_))));
        assert_eq!(
            guard_write(
                &RegKey::new(Hive::CurrentUser, r"Software\Negotium-Test\unit"),
                "x"
            ),
            Ok(())
        );
    }

    #[test]
    fn write_failures_read_as_plain_errors() {
        assert_eq!(error(ERROR_ACCESS_DENIED), ActionError::AccessDenied);
        match error(ERROR_INVALID_PARAMETER) {
            ActionError::Other(text) => {
                assert!(!text.is_empty() && !text.ends_with('.'), "{text:?}")
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn existing_key_writes_never_create_the_key_and_check_the_guard_first() {
        let base = RegKey::new(
            Hive::CurrentUser,
            &format!(r"{TEST_ROOT}\existing-{}", std::process::id()),
        );
        let _ = delete_test_tree(&base);
        let key = base.child("Task");
        assert_eq!(
            set_existing_dword(&key, "State", 1),
            Ok(false),
            "missing: not written"
        );
        assert!(subkeys(&base).unwrap().is_empty(), "and not created");
        set_dword(&key, "State", 2).unwrap();
        assert_eq!(set_existing_dword(&key, "State", 1), Ok(true));
        assert_eq!(value(&key, "State").and_then(|v| v.dword()), Some(1));
        let _ = delete_test_tree(&base);
        // Outside the sandbox the guard answers before Windows is asked, even for a missing key.
        let outside = RegKey::new(Hive::CurrentUser, r"Software\Negotium-NoSuchKey\x");
        match set_existing_dword(&outside, "State", 1) {
            Err(ActionError::Other(text)) => assert!(text.contains("not allowed"), "{text}"),
            other => panic!("expected the guard, got {other:?}"),
        }
    }
}
