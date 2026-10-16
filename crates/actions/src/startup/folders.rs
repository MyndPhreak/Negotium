//! The per-user and all-users Startup folders: one entry per file (desktop.ini excepted). A
//! shortcut is resolved through IShellLink to its target, which names it and gives its publisher.
use super::{StartupEntry, StartupRoots, StartupSource, approved_states, program_entry};
use crate::ActionError;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::E_ACCESSDENIED;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoTaskMemFree, CoUninitialize, IPersistFile, STGM_READ,
};
use windows::Win32::UI::Shell::{
    FOLDERID_CommonStartup, FOLDERID_Startup, IShellLinkW, KNOWN_FOLDER_FLAG, SHGetKnownFolderPath,
    ShellLink,
};
use windows::core::{GUID, HSTRING, Interface};

/// A COM apartment for the calling thread (shell links and package names need one), released on
/// drop if this guard initialised it.
pub struct ComApartment(bool);

impl ComApartment {
    pub fn enter() -> Self {
        // SAFETY: plain per-thread initialisation; balanced in Drop when it succeeded (S_FALSE,
        // "already initialised", included). A thread already in another apartment gets an error
        // and keeps the apartment it has, which COM calls can use as well.
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        Self(hr.is_ok())
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: balances the successful CoInitializeEx in `enter` on the same thread.
            unsafe { CoUninitialize() };
        }
    }
}

fn known_folder(id: &GUID) -> Option<PathBuf> {
    // SAFETY: returns a CoTaskMem string that is freed below.
    let p = unsafe { SHGetKnownFolderPath(id, KNOWN_FOLDER_FLAG(0), None) }.ok()?;
    // SAFETY: `p` is a valid NUL-terminated string until freed.
    let s = unsafe { p.to_string() }.ok();
    // SAFETY: allocated by SHGetKnownFolderPath; freed exactly once.
    unsafe { CoTaskMemFree(Some(p.0 as *const c_void)) };
    s.map(PathBuf::from)
}

/// (per-user, all-users) Startup folders.
pub(super) fn startup_folders() -> (Option<PathBuf>, Option<PathBuf>) {
    (
        known_folder(&FOLDERID_Startup),
        known_folder(&FOLDERID_CommonStartup),
    )
}

fn wide(units: &[u16]) -> String {
    let end = units.iter().position(|&c| c == 0).unwrap_or(units.len());
    String::from_utf16_lossy(&units[..end])
}

/// A shortcut's (target, arguments); `None` when it can't be loaded or has no file target (an
/// advertised installer shortcut, say). Needs a COM apartment on this thread. Only reads the
/// link: it is never resolved, so no search or dialog runs.
pub fn resolve_shortcut(path: &Path) -> Option<(String, String)> {
    // SAFETY: a COM object created and used on this thread only; buffers sized as passed.
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        link.cast::<IPersistFile>()
            .ok()?
            .Load(&HSTRING::from(path.as_os_str()), STGM_READ)
            .ok()?;
        let mut target = [0u16; 1024];
        link.GetPath(&mut target, std::ptr::null_mut(), 0).ok()?;
        let mut args = [0u16; 1024];
        let _ = link.GetArguments(&mut args);
        let target = wide(&target);
        (!target.is_empty()).then(|| (target, wide(&args)))
    }
}

fn com_error(e: windows::core::Error) -> ActionError {
    if e.code() == E_ACCESSDENIED {
        ActionError::AccessDenied
    } else {
        ActionError::Other(e.message().trim().trim_end_matches('.').to_string())
    }
}

/// Writes a shortcut (tests build their Startup folders with it).
pub fn write_shortcut(path: &Path, target: &str, args: &str) -> Result<(), ActionError> {
    let _com = ComApartment::enter();
    // SAFETY: a COM object created and used on this thread only.
    unsafe {
        let link: IShellLinkW =
            CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).map_err(com_error)?;
        link.SetPath(&HSTRING::from(target)).map_err(com_error)?;
        link.SetArguments(&HSTRING::from(args)).map_err(com_error)?;
        link.cast::<IPersistFile>()
            .map_err(com_error)?
            .Save(&HSTRING::from(path.as_os_str()), true)
            .map_err(com_error)
    }
}

/// One Startup folder's entries, sorted by file name. A missing folder is empty; one that can't
/// be read, an unreadable file in it, and an unreadable StartupApproved key (its entries are then
/// listed as enabled) each add a line to `errors`. Needs a COM apartment on this thread.
pub(super) fn folder_entries(
    roots: &StartupRoots,
    source: StartupSource,
    dir: &Path,
    errors: &mut Vec<String>,
) -> Vec<StartupEntry> {
    let read = match std::fs::read_dir(dir) {
        Ok(read) => read,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            errors.push(format!("Couldn't read {}: {e}", dir.display()));
            return Vec::new();
        }
    };
    let states = approved_states(roots, source, errors);
    let location = dir.display().to_string();
    let mut out = Vec::new();
    for item in read {
        let item = match item {
            Ok(item) => item,
            Err(e) => {
                errors.push(format!("Couldn't read a file in {location}: {e}"));
                continue;
            }
        };
        let path = item.path();
        let file = item.file_name().to_string_lossy().into_owned();
        if !path.is_file() || file.eq_ignore_ascii_case("desktop.ini") {
            continue;
        }
        let shortcut = path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("lnk"));
        let (target, args) = if shortcut {
            resolve_shortcut(&path).unwrap_or_default()
        } else {
            (path.to_string_lossy().into_owned(), String::new())
        };
        let stem = path
            .file_stem()
            .map_or_else(|| file.clone(), |s| s.to_string_lossy().into_owned());
        let command = if args.is_empty() {
            target.clone()
        } else {
            format!("{target} {args}")
        };
        let exe = (!target.is_empty()).then_some(target.as_str());
        out.push(program_entry(
            source,
            file,
            &stem,
            exe,
            command,
            location.clone(),
            &states,
        ));
    }
    out.sort_by(|a, b| a.key.cmp(&b.key));
    out
}
