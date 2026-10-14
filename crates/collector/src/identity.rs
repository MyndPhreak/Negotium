//! Expensive per-process facts, fetched once per (pid, create_time) and cached.
use std::collections::{HashMap, HashSet};
use windows::Win32::Foundation::{CloseHandle, ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS, HANDLE};
use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
use windows::Win32::Storage::Packaging::Appx::GetPackageFamilyName;
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::core::{HSTRING, PCWSTR, PWSTR};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Identity {
    pub image_path: Option<String>,
    pub package_family: Option<String>,
    pub description: Option<String>,
}

#[derive(Default)]
pub struct IdentityCache {
    entries: HashMap<(u32, i64), Identity>,
    opens: u64,
}

impl IdentityCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&mut self, pid: u32, create_time: i64) -> &Identity {
        let opens = &mut self.opens;
        self.entries.entry((pid, create_time)).or_insert_with(|| {
            *opens += 1;
            query(pid)
        })
    }

    /// Drops entries for processes that no longer exist.
    pub fn retain_alive(&mut self, alive: &HashSet<(u32, i64)>) {
        self.entries.retain(|k, _| alive.contains(k));
    }

    pub fn opens(&self) -> u64 {
        self.opens
    }
}

fn query(pid: u32) -> Identity {
    if pid == 0 || pid == 4 {
        return Identity::default();
    }
    // SAFETY: plain Win32 call; the handle is closed below.
    let Ok(handle) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }) else {
        return Identity::default();
    };
    let image_path = image_path(handle);
    let identity = Identity {
        description: image_path.as_deref().and_then(file_description),
        package_family: package_family(handle),
        image_path,
    };
    // SAFETY: `handle` came from OpenProcess and is closed exactly once.
    unsafe {
        let _ = CloseHandle(handle);
    }
    identity
}

fn image_path(handle: HANDLE) -> Option<String> {
    let mut buf = vec![0u16; 1024];
    let mut len = buf.len() as u32;
    // SAFETY: `buf` has `len` writable UTF-16 units; `len` is updated to the written length.
    unsafe { QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len) }.ok()?;
    Some(String::from_utf16_lossy(&buf[..len as usize]))
}

fn package_family(handle: HANDLE) -> Option<String> {
    let mut len = 0u32;
    // SAFETY: size query with no buffer; unpackaged processes return APPMODEL_ERROR_NO_PACKAGE.
    let first = unsafe { GetPackageFamilyName(handle, &mut len, None) };
    if first != ERROR_INSUFFICIENT_BUFFER || len == 0 {
        return None;
    }
    let mut buf = vec![0u16; len as usize];
    // SAFETY: `buf` holds `len` UTF-16 units including the terminator.
    let second = unsafe { GetPackageFamilyName(handle, &mut len, Some(PWSTR(buf.as_mut_ptr()))) };
    if second != ERROR_SUCCESS {
        return None;
    }
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..end]))
}

/// FileDescription for the first translation in the version resource, trimmed; `None` if absent.
fn file_description(path: &str) -> Option<String> {
    let path = HSTRING::from(path);
    // SAFETY: size query on a NUL-terminated path.
    let size = unsafe { GetFileVersionInfoSizeW(&path, None) };
    if size == 0 {
        return None;
    }
    let mut data = vec![0u8; size as usize];
    // SAFETY: `data` has `size` writable bytes.
    unsafe { GetFileVersionInfoW(&path, None, size, data.as_mut_ptr().cast()) }.ok()?;
    let mut ptr = std::ptr::null_mut();
    let mut len = 0u32;
    // SAFETY: queries the translation table inside `data`; ptr points into `data`.
    if !unsafe { VerQueryValueW(data.as_ptr().cast(), windows::core::w!("\\VarFileInfo\\Translation"), &mut ptr, &mut len) }.as_bool() || len < 4 {
        return None;
    }
    // SAFETY: at least one (language, codepage) pair of u16 is present (len >= 4).
    let (lang, cp) = unsafe { (*(ptr as *const u16), *(ptr as *const u16).add(1)) };
    let key = HSTRING::from(format!("\\StringFileInfo\\{lang:04x}{cp:04x}\\FileDescription"));
    // SAFETY: queries a string value inside `data`; len is in UTF-16 units including NUL.
    if !unsafe { VerQueryValueW(data.as_ptr().cast(), PCWSTR(key.as_ptr()), &mut ptr, &mut len) }.as_bool() || len == 0 {
        return None;
    }
    // SAFETY: `ptr` points to `len` UTF-16 units inside `data`.
    let units = unsafe { std::slice::from_raw_parts(ptr as *const u16, len as usize) };
    let end = units.iter().position(|&c| c == 0).unwrap_or(units.len());
    let text = String::from_utf16_lossy(&units[..end]).trim().to_string();
    (!text.is_empty()).then_some(text)
}
