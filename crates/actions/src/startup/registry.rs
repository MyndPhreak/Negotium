//! Registry helpers for the startup lists: read a key's values and subkeys, write one value.
//! Paths are relative to a hive; a missing key reads as empty. Every write goes through `set`,
//! which honours the test confinement (`confine_writes_to_test_root`).
use crate::ActionError;
use std::sync::atomic::{AtomicBool, Ordering};
use windows::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS, ERROR_SUCCESS, WIN32_ERROR,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_SET_VALUE, REG_BINARY, REG_DWORD, REG_EXPAND_SZ,
    REG_OPTION_NON_VOLATILE, REG_SAM_FLAGS, REG_SZ, REG_VALUE_TYPE, RRF_NOEXPAND, RRF_RT_ANY, RegCloseKey,
    RegCreateKeyExW, RegDeleteTreeW, RegEnumKeyExW, RegEnumValueW, RegGetValueW, RegOpenKeyExW, RegSetValueExW,
};
use windows::core::{HSTRING, PCWSTR, PWSTR};

/// The only place tests may write to, and `delete_test_tree` may delete from: tests' sandboxes.
pub const TEST_ROOT: &str = r"Software\Negotium-Test";

/// Set once a test confines this process; unit tests start confined.
static CONFINED: AtomicBool = AtomicBool::new(cfg!(test));

/// From now on, refuse every registry write outside HKCU\Software\Negotium-Test\… in this
/// process. Tests call it first, so no bug can reach the user's real startup entries.
pub fn confine_writes_to_test_root() {
    CONFINED.store(true, Ordering::SeqCst);
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
        Self { hive, path: path.to_string() }
    }

    pub fn child(&self, name: &str) -> RegKey {
        RegKey { hive: self.hive, path: format!("{}\\{name}", self.path) }
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
}

impl RegValue {
    pub fn as_str(&self) -> Option<&str> {
        if let RegValue::Str(s) = self { Some(s) } else { None }
    }

    pub fn binary(&self) -> Option<&[u8]> {
        if let RegValue::Binary(b) = self { Some(b) } else { None }
    }

    pub fn dword(&self) -> Option<u32> {
        if let RegValue::Dword(d) = self { Some(*d) } else { None }
    }

    fn decode(kind: REG_VALUE_TYPE, data: &[u8]) -> RegValue {
        match kind {
            REG_SZ | REG_EXPAND_SZ => {
                let units: Vec<u16> = data.as_chunks::<2>().0.iter().map(|&c| u16::from_le_bytes(c)).collect();
                let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
                RegValue::Str(String::from_utf16_lossy(&units[..end]))
            }
            REG_BINARY => RegValue::Binary(data.to_vec()),
            REG_DWORD if data.len() >= 4 => RegValue::Dword(u32::from_le_bytes([data[0], data[1], data[2], data[3]])),
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
        && key.path.get(..prefix.len()).is_some_and(|p| p.eq_ignore_ascii_case(&prefix))
        && !key.path[prefix.len()..].trim_matches('\\').is_empty()
}

/// Refuses a write outside the test root while confined.
fn guard_write(key: &RegKey) -> Result<(), ActionError> {
    if CONFINED.load(Ordering::SeqCst) && !in_test_root(key) {
        return Err(ActionError::Other(format!("refusing to write {} from a test", key.display())));
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

pub fn values(key: &RegKey) -> Result<Vec<(String, RegValue)>, ActionError> {
    let Some(k) = open(key, KEY_READ)? else { return Ok(Vec::new()) };
    let mut out = Vec::new();
    let mut name = vec![0u16; 16_384]; // the registry's longest value name, plus the terminator
    let mut data = vec![0u8; 64 * 1024];
    for index in 0u32.. {
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
            ERROR_SUCCESS => out.push((
                String::from_utf16_lossy(&name[..name_len as usize]),
                RegValue::decode(REG_VALUE_TYPE(kind), &data[..data_len as usize]),
            )),
            ERROR_MORE_DATA => continue, // over 64 KB: not a startup entry
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
    let status = unsafe { RegGetValueW(key.hive.hkey(), &path, &name, flags, Some(&mut kind), None, Some(&mut size)) };
    if status != ERROR_SUCCESS {
        return None;
    }
    let mut data = vec![0u8; size as usize];
    // SAFETY: `data` has `size` writable bytes, and `size` says so.
    let status = unsafe {
        RegGetValueW(key.hive.hkey(), &path, &name, flags, Some(&mut kind), Some(data.as_mut_ptr().cast()), Some(&mut size))
    };
    (status == ERROR_SUCCESS).then(|| RegValue::decode(kind, &data[..(size as usize).min(data.len())]))
}

pub fn subkeys(key: &RegKey) -> Result<Vec<String>, ActionError> {
    let Some(k) = open(key, KEY_READ)? else { return Ok(Vec::new()) };
    let mut out = Vec::new();
    let mut name = vec![0u16; 256]; // key names are at most 255 characters
    for index in 0u32.. {
        let mut len = name.len() as u32;
        // SAFETY: `name` is writable for `len` units; the other outputs are not requested.
        let status = unsafe { RegEnumKeyExW(k.0, index, Some(PWSTR(name.as_mut_ptr())), &mut len, None, None, None, None) };
        match status {
            ERROR_SUCCESS => out.push(String::from_utf16_lossy(&name[..len as usize])),
            ERROR_NO_MORE_ITEMS => break,
            e => return Err(error(e)),
        }
    }
    Ok(out)
}

/// The one function that writes: value `name` of `key` (its hive and subkey), creating the key.
fn set(key: &RegKey, name: &str, kind: REG_VALUE_TYPE, data: &[u8]) -> Result<(), ActionError> {
    guard_write(key)?;
    let mut h = HKEY::default();
    let path = HSTRING::from(key.path.as_str());
    // SAFETY: creates or opens the key for writing into a writable local; the handle is owned by `Key`.
    let status = unsafe {
        RegCreateKeyExW(key.hive.hkey(), &path, None, PCWSTR::null(), REG_OPTION_NON_VOLATILE, KEY_SET_VALUE, None, &mut h, None)
    };
    if status != ERROR_SUCCESS {
        return Err(error(status));
    }
    let h = Key(h);
    let name = HSTRING::from(name);
    // SAFETY: valid key handle; the data is passed as a slice.
    let status = unsafe { RegSetValueExW(h.0, &name, None, kind, Some(data)) };
    if status == ERROR_SUCCESS { Ok(()) } else { Err(error(status)) }
}

pub fn set_binary(key: &RegKey, name: &str, data: &[u8]) -> Result<(), ActionError> {
    set(key, name, REG_BINARY, data)
}

pub fn set_dword(key: &RegKey, name: &str, value: u32) -> Result<(), ActionError> {
    set(key, name, REG_DWORD, &value.to_le_bytes())
}

pub fn set_string(key: &RegKey, name: &str, value: &str) -> Result<(), ActionError> {
    let bytes: Vec<u8> = value.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect();
    set(key, name, REG_SZ, &bytes)
}

/// Deletes a test sandbox. Refuses anything but a subkey of HKCU\Software\Negotium-Test.
pub fn delete_test_tree(key: &RegKey) -> Result<(), ActionError> {
    if !in_test_root(key) {
        return Err(ActionError::Other(format!("refusing to delete {}", key.display())));
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
        assert!(in_test_root(&key(Hive::CurrentUser, r"Software\Negotium-Test\a")));
        assert!(in_test_root(&key(Hive::CurrentUser, r"SOFTWARE\negotium-test\a\b")));
        assert!(!in_test_root(&key(Hive::CurrentUser, r"Software\Negotium-Test")));
        assert!(!in_test_root(&key(Hive::CurrentUser, r"Software\Negotium-Test\")));
        assert!(!in_test_root(&key(Hive::CurrentUser, r"Software\Negotium-TestX\a")));
        assert!(!in_test_root(&key(Hive::LocalMachine, r"Software\Negotium-Test\a")));
        assert!(!in_test_root(&key(Hive::CurrentUser, r"Software\Microsoft\Windows\CurrentVersion\Run")));
    }

    #[test]
    fn unit_tests_are_confined_from_the_start() {
        let real = RegKey::new(Hive::CurrentUser, r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run");
        assert!(matches!(guard_write(&real), Err(ActionError::Other(_))));
        assert_eq!(guard_write(&RegKey::new(Hive::CurrentUser, r"Software\Negotium-Test\unit")), Ok(()));
    }

    #[test]
    fn write_failures_read_as_plain_errors() {
        assert_eq!(error(ERROR_ACCESS_DENIED), ActionError::AccessDenied);
        match error(ERROR_INVALID_PARAMETER) {
            ActionError::Other(text) => assert!(!text.is_empty() && !text.ends_with('.'), "{text:?}"),
            other => panic!("{other:?}"),
        }
    }
}
