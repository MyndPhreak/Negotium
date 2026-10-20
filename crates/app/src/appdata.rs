//! Negotium's per-user folder, `<LocalAppData>\Negotium` (settings.json, the renderer marker, the
//! log), and how files there are written.
//!
//! That folder is the user's to change, and an elevated Negotium (admin mode makes every launch
//! elevated) must not be steered into writing elsewhere through it. So, elevated:
//! - LocalAppData is the shell's known folder, never `%LOCALAPPDATA%` (an environment variable the
//!   user can set in HKCU\Environment for the admin task's process);
//! - every write is [`Writes::Guarded`]: each part of the path below LocalAppData (`Negotium`,
//!   `logs`, the file) must be a plain folder or file, not a reparse point (a junction, a
//!   symlink, a mount point), each checked through a handle that doesn't follow links; the file
//!   must have no other hard link; and once opened, the file's final path must still be where it
//!   was asked for (nothing swapped in between). Anything else refuses the write, and a file the
//!   refused open created (where a folder swapped in after its check pointed) is deleted through
//!   its handle;
//! - a file read there (settings.json, read and copied aside) is opened through the same checks
//!   ([`Writes::read`]), so a link can't make the elevated copy read, and copy into the user's
//!   folder, a file only an administrator may read;
//! - nothing there is renamed, rotated or deleted by path (each of those would act on whatever a
//!   planted link names): files are appended to or overwritten in place, or replaced
//!   ([`Writes::replace`]) by a temporary file written through those checks and renamed through
//!   its handle.
//!
//! Not elevated, writes are plain: anything that could redirect them runs as the same user.
use std::fs::File;
use std::io;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

const FILE_SHARE_ALL: u32 = 0x7;
/// Not FILE_SHARE_DELETE: the open file can't be renamed or deleted (swapped) while it is written.
const FILE_SHARE_READ_WRITE: u32 = 0x3;
const FILE_READ_ATTRIBUTES: u32 = 0x80;
const DELETE: u32 = 0x0001_0000;
/// What std's `write(true)` asks for (as FILE_GENERIC_WRITE); without FILE_WRITE_DATA, what its
/// `append(true)` asks for. Spelled out so DELETE can be added.
const FILE_GENERIC_WRITE: u32 = 0x0012_0116;
const FILE_WRITE_DATA: u32 = 0x2;
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

/// Whether this process is elevated (read once).
pub fn elevated() -> bool {
    static ELEVATED: OnceLock<bool> = OnceLock::new();
    *ELEVATED.get_or_init(negotium_actions::is_elevated)
}

/// LocalAppData from the shell's known folder.
pub fn known_local_app_data() -> Option<PathBuf> {
    crate::install::known_folder(&windows::Win32::UI::Shell::FOLDERID_LocalAppData)
}

/// `<LocalAppData>\Negotium`. Not elevated, LocalAppData is `%LOCALAPPDATA%` (the benchmark gives
/// Negotium a scratch one) or the temp directory without it; elevated, only the known folder
/// ([`folder_for`]).
pub fn negotium_dir() -> PathBuf {
    folder_for(elevated(), std::env::var_os("LOCALAPPDATA"), known_local_app_data).join("Negotium")
}

/// LocalAppData for a process that is (or isn't) `elevated`: the environment's (`env`, else the
/// temp directory), or, elevated, `known()` whatever the environment says (the temp directory,
/// where no write is allowed, without it).
pub fn folder_for(elevated: bool, env: Option<std::ffi::OsString>, known: impl FnOnce() -> Option<PathBuf>) -> PathBuf {
    if elevated {
        known().unwrap_or_else(std::env::temp_dir)
    } else {
        env.map(PathBuf::from).unwrap_or_else(std::env::temp_dir)
    }
}

/// How a file is opened for writing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Open {
    /// Created if missing, written at its end.
    Append,
    /// Created if missing, emptied (after the checks), written from the start.
    Overwrite,
}

/// How this process writes under `<LocalAppData>\Negotium`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Writes {
    /// Not elevated: plain writes, and files may be renamed or deleted.
    Plain,
    /// Elevated: [`open_guarded`] under this LocalAppData (`None`: no known folder, every write
    /// refused); nothing is renamed or deleted by path.
    Guarded(Option<PathBuf>),
}

/// This process's [`Writes`].
pub fn writes() -> Writes {
    if elevated() { Writes::Guarded(known_local_app_data()) } else { Writes::Plain }
}

impl Writes {
    /// Whether files may be renamed, rotated or deleted (only when not elevated).
    pub fn may_move(&self) -> bool {
        matches!(self, Writes::Plain)
    }

    /// Opens `path` for writing, creating its folders as needed.
    pub fn open(&self, path: &Path, how: Open) -> io::Result<File> {
        match self {
            Writes::Plain => {
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                let mut options = std::fs::OpenOptions::new();
                options.create(true);
                match how {
                    Open::Append => options.append(true),
                    Open::Overwrite => options.write(true).truncate(true),
                };
                options.open(path)
            }
            Writes::Guarded(None) => Err(refused("LocalAppData's known folder couldn't be read")),
            Writes::Guarded(Some(base)) => open_guarded(base, path, how),
        }
    }

    /// Reads the whole of `path`: plainly, or elevated through the same checks as a write
    /// ([`read_guarded`]: no link on the way, a single-name file whose final path is `path`).
    pub fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        match self {
            Writes::Plain => std::fs::read(path),
            Writes::Guarded(None) => Err(refused("LocalAppData's known folder couldn't be read")),
            Writes::Guarded(Some(base)) => read_guarded(base, path),
        }
    }

    /// Writes `bytes` as the whole of `path` ([`Open::Overwrite`]), flushed.
    pub fn write_all(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        use std::io::Write;
        let mut f = self.open(path, Open::Overwrite)?;
        f.write_all(bytes)?;
        f.sync_all()
    }

    /// Replaces `path` with `bytes` atomically: written to a temporary file beside it
    /// ([`temp_beside`]), flushed, then renamed over it, so a crash leaves the old file or the new
    /// one, never part of one. Elevated, the temporary file is opened through [`open_guarded`]'s
    /// checks and renamed through its handle ([`replace_guarded`]). A failed replace deletes the
    /// temporary file and leaves `path` as it was.
    pub fn replace(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        use std::io::Write;
        let tmp = temp_beside(path)?;
        match self {
            Writes::Plain => {
                let replaced = (|| {
                    let mut f = self.open(&tmp, Open::Overwrite)?;
                    f.write_all(bytes)?;
                    f.sync_all()?;
                    drop(f);
                    std::fs::rename(&tmp, path)
                })();
                if replaced.is_err() {
                    let _ = std::fs::remove_file(&tmp);
                }
                replaced
            }
            Writes::Guarded(None) => Err(refused("LocalAppData's known folder couldn't be read")),
            Writes::Guarded(Some(base)) => replace_guarded(base, &tmp, path, bytes),
        }
    }
}

/// `<path>.<pid>-<n>.tmp`, beside `path`: unique to this process (a second Negotium may run beside
/// a hung one) and to this call.
fn temp_beside(path: &Path) -> io::Result<PathBuf> {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let mut name = path.file_name().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?.to_os_string();
    name.push(format!(".{}-{}.tmp", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    Ok(path.with_file_name(name))
}

/// The elevated replace: `tmp` (beside `path`) opened through [`open_guarded`]'s checks with DELETE
/// access, written, flushed, then renamed over `path` through its handle, to its own final folder
/// (checked at open) plus `path`'s name. While that handle is open the folders on its way can't be
/// renamed or removed (a folder with an open file inside can't), so the target can't be swapped
/// either; a link named like the target is replaced itself, never followed.
fn replace_guarded(base: &Path, tmp: &Path, path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;
    if tmp.parent() != path.parent() {
        return Err(refused("the temporary file isn't beside the file"));
    }
    let name = path.file_name().ok_or_else(|| refused("no file name"))?;
    let mut file = open_guarded_with(base, tmp, Open::Overwrite, DELETE)?;
    let replaced = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        rename_handle(&file, &final_path(&file)?.with_file_name(name))
    })();
    if replaced.is_err() {
        let _ = crate::update::secure::delete_handle(&file);
    }
    replaced
}

/// Renames the file `handle` is open on (with DELETE access) to `to`, a full path, replacing
/// what is there.
fn rename_handle(handle: &File, to: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{FILE_RENAME_INFO, FileRenameInfo, SetFileInformationByHandle};
    let name: Vec<u16> = to.as_os_str().encode_wide().collect();
    let offset = std::mem::offset_of!(FILE_RENAME_INFO, FileName);
    let size = (offset + (name.len() + 1) * 2).max(std::mem::size_of::<FILE_RENAME_INFO>());
    // u64s: aligned for FILE_RENAME_INFO; zeroed, so the name is NUL-terminated.
    let mut buf = vec![0u64; size.div_ceil(8)];
    let info = buf.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    // SAFETY: `buf` is zeroed, aligned and `size` bytes long, room for the header and the name
    // (copied after `offset`); `handle` is a live handle with DELETE access.
    unsafe {
        (*info).Anonymous.ReplaceIfExists = true;
        (*info).FileNameLength = (name.len() * 2) as u32;
        std::ptr::copy_nonoverlapping(name.as_ptr(), (&raw mut (*info).FileName).cast::<u16>(), name.len());
        SetFileInformationByHandle(HANDLE(handle.as_raw_handle()), FileRenameInfo, info.cast_const().cast(), size as u32)
    }
    .map_err(io::Error::other)
}

fn refused(why: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, format!("refused (elevated): {}", why.into()))
}

/// One part of a path, as a handle that doesn't follow links reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub reparse_point: bool,
    pub directory: bool,
    /// Hard links to the file (1 for a plain file).
    pub links: u32,
}

/// Whether a folder on the way to the file may be written through: the folder itself.
pub fn folder_ok(e: &Entry) -> Result<(), &'static str> {
    if e.reparse_point {
        Err("a folder on the way is a link")
    } else if !e.directory {
        Err("a folder on the way is a file")
    } else {
        Ok(())
    }
}

/// Whether the opened file may be written: a plain file with no other name.
pub fn file_ok(e: &Entry) -> Result<(), &'static str> {
    if e.reparse_point {
        Err("the file is a link")
    } else if e.directory {
        Err("the file is a folder")
    } else if e.links > 1 {
        Err("the file has another hard link")
    } else {
        Ok(())
    }
}

/// Whether the opened file's final path is the one asked for (`expected`: the base's final path
/// and the parts below it), as Windows compares paths (case-insensitively).
pub fn same_final_path(expected: &Path, got: &Path) -> bool {
    let norm = |p: &Path| p.to_string_lossy().replace('/', "\\").trim_end_matches('\\').to_lowercase();
    norm(expected) == norm(got)
}

/// `path`'s parts below `base`: plain names only (no `..`, no root), at least the file's.
pub fn parts_below<'a>(base: &Path, path: &'a Path) -> Result<Vec<&'a std::ffi::OsStr>, &'static str> {
    let rel = path.strip_prefix(base).map_err(|_| "the path is outside LocalAppData")?;
    let parts: Vec<_> = rel
        .components()
        .map(|c| match c {
            Component::Normal(name) => Ok(name),
            _ => Err("the path has more than plain names below LocalAppData"),
        })
        .collect::<Result<_, _>>()?;
    if parts.is_empty() { Err("the path is LocalAppData itself") } else { Ok(parts) }
}

/// What a handle reports about the file or folder it is open on.
fn entry(handle: &File) -> io::Result<Entry> {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle};
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: `handle` is a live file handle; `info` is a writable BY_HANDLE_FILE_INFORMATION.
    unsafe { GetFileInformationByHandle(HANDLE(handle.as_raw_handle()), &mut info) }.map_err(io::Error::other)?;
    Ok(Entry {
        reparse_point: info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0,
        directory: info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0,
        links: info.nNumberOfLinks,
    })
}

/// Opens `path` itself (a link is opened, not followed) just to read its attributes.
fn inspect(path: &Path) -> io::Result<Entry> {
    let handle = std::fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_ALL)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    entry(&handle)
}

/// The final path of what `handle` is open on (`\\?\C:\…`).
fn final_path(handle: &File) -> io::Result<PathBuf> {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{FILE_NAME_NORMALIZED, GetFinalPathNameByHandleW};
    let mut buf = vec![0u16; 32 * 1024];
    // SAFETY: `handle` is live; `buf` is a writable buffer of the length passed.
    let len = unsafe { GetFinalPathNameByHandleW(HANDLE(handle.as_raw_handle()), &mut buf, FILE_NAME_NORMALIZED) };
    if len == 0 || len as usize >= buf.len() {
        return Err(io::Error::last_os_error());
    }
    Ok(PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])))
}

/// The elevated write: `path` under `base` (LocalAppData), opened only when every part below
/// `base` is a plain folder (created one level at a time when missing) and the file is a plain
/// file with one name whose final path, once opened, is `base`'s final path plus those parts.
/// `base` itself may be anything Windows makes it (a redirected profile). An existing file is
/// emptied only after the checks ([`Open::Overwrite`]).
pub fn open_guarded(base: &Path, path: &Path, how: Open) -> io::Result<File> {
    open_guarded_with(base, path, how, 0)
}

/// [`open_guarded`], the handle also given `access` (DELETE, to rename it).
fn open_guarded_with(base: &Path, path: &Path, how: Open, access: u32) -> io::Result<File> {
    let parts = parts_below(base, path).map_err(refused)?;
    // Never truncated on open: emptied below, once the file is known to be the right one.
    let options = |access: u32| {
        let mut options = std::fs::OpenOptions::new();
        options.share_mode(FILE_SHARE_READ_WRITE).custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        match how {
            Open::Append => options.append(true).access_mode((FILE_GENERIC_WRITE & !FILE_WRITE_DATA) | access),
            Open::Overwrite => options.write(true).access_mode(FILE_GENERIC_WRITE | access),
        };
        options
    };
    // A new file is opened with DELETE, to be deleted again if the checks refuse it.
    let mut new = options(access | DELETE);
    new.create_new(true);
    let file = open_checked(base, &parts, Some(&new), &options(access))?;
    if how == Open::Overwrite {
        file.set_len(0)?;
    }
    Ok(file)
}

/// The elevated read: `path` under `base` opened through [`open_guarded`]'s checks (every part
/// below `base` a plain folder, none created; a plain single-name file whose final path is the one
/// asked for), then read whole. A missing file or folder is `NotFound`.
pub fn read_guarded(base: &Path, path: &Path) -> io::Result<Vec<u8>> {
    use std::io::Read;
    let parts = parts_below(base, path).map_err(refused)?;
    let mut options = std::fs::OpenOptions::new();
    options.read(true).share_mode(FILE_SHARE_READ_WRITE).custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    let mut file = open_checked(base, &parts, None, &options)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// Opens `base` + `parts` with `options` (which must not follow a link: OPEN_REPARSE_POINT) once
/// every folder on the way is a plain folder (`create`: made one level at a time when missing, and
/// the file created with it, CREATE_NEW and DELETE access, when missing), and returns the file
/// only if it is a plain single-name file whose final path is `base`'s final path plus `parts`. A
/// file this call created and then refused (a folder swapped for a link after its check) is
/// deleted through its handle; an existing one is left as it was.
fn open_checked(base: &Path, parts: &[&std::ffi::OsStr], create: Option<&std::fs::OpenOptions>, options: &std::fs::OpenOptions) -> io::Result<File> {
    let (file_name, folders) = parts.split_last().expect("parts_below returns at least one");
    let mut dir = base.to_path_buf();
    for name in folders {
        dir.push(name);
        let found = match inspect(&dir) {
            Err(e) if create.is_some() && e.kind() == io::ErrorKind::NotFound => {
                std::fs::create_dir(&dir).or_else(|e| if e.kind() == io::ErrorKind::AlreadyExists { Ok(()) } else { Err(e) })?;
                inspect(&dir)?
            }
            other => other?,
        };
        folder_ok(&found).map_err(refused)?;
    }
    let path = dir.join(file_name);
    let (file, created) = match create.map(|new| new.open(&path)) {
        Some(Ok(file)) => (file, true),
        Some(Err(e)) if e.kind() != io::ErrorKind::AlreadyExists => return Err(e),
        _ => (options.open(&path)?, false),
    };
    let checked = check_opened(base, parts, &file);
    if checked.is_err() && created {
        let _ = crate::update::secure::delete_handle(&file);
    }
    checked.map(|()| file)
}

/// [`open_checked`]'s checks on the opened `file`: a plain single-name file whose final path is
/// `base`'s final path plus `parts`.
fn check_opened(base: &Path, parts: &[&std::ffi::OsStr], file: &File) -> io::Result<()> {
    file_ok(&entry(file)?).map_err(refused)?;
    let base_handle = std::fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_ALL)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(base)?;
    let expected = parts.iter().fold(final_path(&base_handle)?, |p, name| p.join(name));
    let got = final_path(file)?;
    if !same_final_path(&expected, &got) {
        return Err(refused(format!("{} opened as {}", expected.display(), got.display())));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A scratch "LocalAppData" under the temp dir, removed on drop.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            static N: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "negotium-appdata-{name}-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::SeqCst)
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            // A junction is removed as a folder, never followed.
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// `mklink /J` (no elevation needed).
    fn junction(link: &Path, target: &Path) {
        let out = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    }

    #[test]
    fn elevated_local_app_data_is_the_known_folder_whatever_the_environment_says() {
        let env = Some(std::ffi::OsString::from(r"C:\Users\Public\planted"));
        let known = || Some(PathBuf::from(r"C:\Users\me\AppData\Local"));
        assert_eq!(folder_for(true, env.clone(), known), PathBuf::from(r"C:\Users\me\AppData\Local"));
        assert_eq!(folder_for(false, env, known), PathBuf::from(r"C:\Users\Public\planted"), "the bench's scratch one");
        assert_eq!(folder_for(false, None, known), std::env::temp_dir());
        assert_eq!(folder_for(true, None, || None), std::env::temp_dir(), "no known folder: nothing is written there");
        assert!(Writes::Guarded(None).open(&std::env::temp_dir().join("x"), Open::Append).is_err());
    }

    #[test]
    fn only_plain_folders_and_single_name_files_are_written() {
        let plain = Entry { reparse_point: false, directory: true, links: 1 };
        assert_eq!(folder_ok(&plain), Ok(()));
        assert!(folder_ok(&Entry { reparse_point: true, ..plain }).is_err());
        assert!(folder_ok(&Entry { directory: false, ..plain }).is_err());
        let file = Entry { reparse_point: false, directory: false, links: 1 };
        assert_eq!(file_ok(&file), Ok(()));
        assert!(file_ok(&Entry { reparse_point: true, ..file }).is_err());
        assert!(file_ok(&Entry { directory: true, ..file }).is_err());
        assert!(file_ok(&Entry { links: 2, ..file }).is_err());
    }

    #[test]
    fn only_paths_below_local_app_data_are_written() {
        let base = Path::new(r"C:\Users\me\AppData\Local");
        let names = |p: &str| parts_below(base, Path::new(p)).map(|v| v.iter().map(|n| n.to_string_lossy().into_owned()).collect::<Vec<_>>());
        assert_eq!(names(r"C:\Users\me\AppData\Local\Negotium\logs\negotium.log").unwrap(), ["Negotium", "logs", "negotium.log"]);
        assert!(names(r"C:\Windows\System32\x.dll").is_err());
        assert!(names(r"C:\Users\me\AppData\Local").is_err());
        assert!(names(r"C:\Users\me\AppData\Local\Negotium\..\..\Roaming\x").is_err());
        assert!(same_final_path(Path::new(r"\\?\C:\Users\Me\x"), Path::new(r"\\?\c:\users\me\X")));
        assert!(!same_final_path(Path::new(r"\\?\C:\Users\me\x"), Path::new(r"\\?\C:\Windows\x")));
    }

    #[test]
    fn a_guarded_write_creates_appends_and_overwrites_in_place() {
        let base = Scratch::new("plain");
        let w = Writes::Guarded(Some(base.0.clone()));
        let log = base.0.join("Negotium").join("logs").join("negotium.log");
        use std::io::Write;
        writeln!(w.open(&log, Open::Append).unwrap(), "one").unwrap();
        writeln!(w.open(&log, Open::Append).unwrap(), "two").unwrap();
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "one\ntwo\n");
        let settings = base.0.join("Negotium").join("settings.json");
        w.write_all(&settings, b"a much longer first version").unwrap();
        w.write_all(&settings, b"short").unwrap();
        assert_eq!(std::fs::read_to_string(&settings).unwrap(), "short");
        assert!(!w.may_move() && Writes::Plain.may_move());
        // Outside LocalAppData: refused.
        assert!(w.open(&std::env::temp_dir().join("negotium-outside.txt"), Open::Append).is_err());
    }

    /// A replace swaps the whole file in (no temporary file left), elevated or not; elevated, a
    /// junction on the way refuses it and nothing lands where it points.
    #[test]
    fn a_replace_renames_a_temporary_file_over_the_target() {
        let base = Scratch::new("replace");
        let only = |dir: &Path| std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect::<Vec<_>>();
        for w in [Writes::Plain, Writes::Guarded(Some(base.0.clone()))] {
            let settings = base.0.join("Negotium").join("settings.json");
            w.replace(&settings, b"a much longer first version").unwrap();
            w.replace(&settings, b"short").unwrap();
            assert_eq!(std::fs::read_to_string(&settings).unwrap(), "short", "{w:?}");
            assert_eq!(only(&base.0.join("Negotium")), ["settings.json"], "{w:?}");
            std::fs::remove_file(&settings).unwrap();
        }
        let base = Scratch::new("replace-junction");
        let elsewhere = Scratch::new("replace-target");
        junction(&base.0.join("Negotium"), &elsewhere.0);
        let w = Writes::Guarded(Some(base.0.clone()));
        let err = w.replace(&base.0.join("Negotium").join("settings.json"), b"{}").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied, "{err}");
        assert!(only(&elsewhere.0).is_empty(), "nothing written through the junction");
    }

    /// A junction anywhere below LocalAppData (the Negotium folder, its logs folder) refuses the
    /// write, and nothing lands where it points.
    #[test]
    fn a_junction_on_the_way_refuses_the_write() {
        let base = Scratch::new("junction");
        let elsewhere = Scratch::new("target");
        let w = Writes::Guarded(Some(base.0.clone()));
        junction(&base.0.join("Negotium"), &elsewhere.0);
        let err = w.write_all(&base.0.join("Negotium").join("settings.json"), b"{}").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied, "{err}");
        assert!(std::fs::read_dir(&elsewhere.0).unwrap().next().is_none(), "nothing written through the junction");

        let base = Scratch::new("junction-logs");
        std::fs::create_dir_all(base.0.join("Negotium")).unwrap();
        junction(&base.0.join("Negotium").join("logs"), &elsewhere.0);
        let w = Writes::Guarded(Some(base.0.clone()));
        assert!(w.open(&base.0.join("Negotium").join("logs").join("negotium.log"), Open::Append).is_err());
        assert!(std::fs::read_dir(&elsewhere.0).unwrap().next().is_none());
        // Plain (not elevated) writes follow it, as the same user could anyway.
        assert!(Writes::Plain.open(&base.0.join("Negotium").join("logs").join("x.log"), Open::Append).is_ok());
    }

    /// A file with another hard link (to something the user wants overwritten) is refused, and
    /// left as it was.
    #[test]
    fn a_hard_linked_file_is_refused() {
        let base = Scratch::new("hardlink");
        let elsewhere = Scratch::new("hardlink-target");
        let victim = elsewhere.0.join("victim.txt");
        std::fs::write(&victim, b"keep me").unwrap();
        std::fs::create_dir_all(base.0.join("Negotium")).unwrap();
        let marker = base.0.join("Negotium").join("renderer-fallback");
        std::fs::hard_link(&victim, &marker).unwrap();
        let w = Writes::Guarded(Some(base.0.clone()));
        assert!(w.write_all(&marker, b"x").is_err());
        assert_eq!(std::fs::read(&victim).unwrap(), b"keep me", "not emptied either");
        assert!(w.read(&marker).is_err(), "nor read through the other name");
    }

    /// The elevated read takes a plain file, reports a missing one (or a missing folder, which it
    /// doesn't create) as NotFound, and refuses one reached through a junction or outside
    /// LocalAppData.
    #[test]
    fn a_guarded_read_reads_only_a_plain_file() {
        let base = Scratch::new("read");
        let w = Writes::Guarded(Some(base.0.clone()));
        let settings = base.0.join("Negotium").join("settings.json");
        assert_eq!(w.read(&settings).unwrap_err().kind(), io::ErrorKind::NotFound);
        assert!(!base.0.join("Negotium").exists(), "a read creates no folder");
        w.write_all(&settings, b"{}").unwrap();
        assert_eq!(w.read(&settings).unwrap(), b"{}");
        assert!(w.read(&std::env::temp_dir().join("negotium-outside.txt")).is_err());

        let base = Scratch::new("read-junction");
        let elsewhere = Scratch::new("read-target");
        std::fs::write(elsewhere.0.join("settings.json"), b"secret").unwrap();
        junction(&base.0.join("Negotium"), &elsewhere.0);
        let w = Writes::Guarded(Some(base.0.clone()));
        let err = w.read(&base.0.join("Negotium").join("settings.json")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied, "{err}");
        assert_eq!(Writes::Plain.read(&base.0.join("Negotium").join("settings.json")).unwrap(), b"secret");
    }
}
