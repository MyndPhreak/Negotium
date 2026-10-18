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
//!   was asked for (nothing swapped in between). Anything else refuses the write;
//! - nothing there is renamed, rotated or deleted (each of those would act on whatever a planted
//!   link names): files are appended to or overwritten in place.
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
    /// refused); nothing is renamed or deleted.
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

    /// Writes `bytes` as the whole of `path` ([`Open::Overwrite`]), flushed.
    pub fn write_all(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        use std::io::Write;
        let mut f = self.open(path, Open::Overwrite)?;
        f.write_all(bytes)?;
        f.sync_all()
    }
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
    let parts = parts_below(base, path).map_err(refused)?;
    let (file_name, folders) = parts.split_last().expect("parts_below returns at least one");
    let mut dir = base.to_path_buf();
    for name in folders {
        dir.push(name);
        let found = match inspect(&dir) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                std::fs::create_dir(&dir).or_else(|e| if e.kind() == io::ErrorKind::AlreadyExists { Ok(()) } else { Err(e) })?;
                inspect(&dir)?
            }
            other => other?,
        };
        folder_ok(&found).map_err(refused)?;
    }
    let mut options = std::fs::OpenOptions::new();
    // Never truncated on open: emptied below, once the file is known to be the right one.
    options.create(true).share_mode(FILE_SHARE_READ_WRITE).custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    match how {
        Open::Append => options.append(true),
        Open::Overwrite => options.write(true),
    };
    let file = options.open(dir.join(file_name))?;
    file_ok(&entry(&file)?).map_err(refused)?;
    let base_handle = std::fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_ALL)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(base)?;
    let expected = parts.iter().fold(final_path(&base_handle)?, |p, name| p.join(name));
    let got = final_path(&file)?;
    if !same_final_path(&expected, &got) {
        return Err(refused(format!("{} opened as {}", expected.display(), got.display())));
    }
    if how == Open::Overwrite {
        file.set_len(0)?;
    }
    Ok(file)
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
    }
}
