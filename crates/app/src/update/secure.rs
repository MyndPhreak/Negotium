//! Where the downloaded installer lives, and how it is opened, so nothing else can swap it before
//! msiexec reads it.
//!
//! A non-elevated Negotium downloads to `%TEMP%\Negotium-update`: anything that could change the
//! file there runs as the same user, and msiexec still asks for elevation (a UAC prompt). An
//! elevated Negotium (admin mode makes every launch elevated) starts msiexec without a prompt, so
//! it downloads to `%ProgramData%\Negotium-update` instead: a folder it creates owned by
//! Administrators with a protected DACL granting only Administrators and SYSTEM, checked on every
//! use (owner, DACL, not a link). If the folder can't be made so, the update is refused.
//!
//! In both cases the installer is then opened without following links, shared for reading only
//! (no writing, renaming or deleting while it is open), hashed through that handle, and the
//! handle is kept until Negotium exits ([`super::verify_and_launch`]).
use super::UpdateError;
use std::fs::File;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};

const FILE_SHARE_READ: u32 = 0x1;
const FILE_SHARE_ALL: u32 = 0x7;
const DELETE: u32 = 0x0001_0000;
const READ_CONTROL: u32 = 0x0002_0000;
const FILE_READ_ATTRIBUTES: u32 = 0x80;
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
/// The admin-only folder's security: owner Administrators, a protected DACL (nothing inherited
/// from ProgramData, whose DACL lets users create files) with full control for Administrators and
/// SYSTEM, inherited by what is created inside.
pub const ADMIN_ONLY_SDDL: &str = "O:BAG:SYD:P(A;OICI;FA;;;BA)(A;OICI;FA;;;SY)";

/// The folder downloads go to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateDir {
    /// `%TEMP%\Negotium-update` (not elevated).
    Temp(PathBuf),
    /// `%ProgramData%\Negotium-update`, admin-only (elevated).
    Admin(PathBuf),
}

impl UpdateDir {
    pub fn path(&self) -> &Path {
        match self {
            UpdateDir::Temp(p) | UpdateDir::Admin(p) => p,
        }
    }

    /// The folder, ready to write to: `%TEMP%`'s as it is (created), the admin-only one made and
    /// checked through `acl` ([`prepare_admin_dir`]).
    pub fn prepare(&self, acl: &dyn DirAcl) -> Result<&Path, UpdateError> {
        match self {
            UpdateDir::Temp(p) => {
                std::fs::create_dir_all(p).map_err(|e| UpdateError::Io(e.to_string()))?;
            }
            UpdateDir::Admin(p) => prepare_admin_dir(p, acl)?,
        }
        Ok(self.path())
    }
}

/// Which folder an update of a process that is (or isn't) `elevated` uses: `temp\Negotium-update`,
/// or, elevated, `program_data\Negotium-update` (no ProgramData: refused).
pub fn update_dir_for(elevated: bool, temp: &Path, program_data: Option<&Path>) -> Result<UpdateDir, UpdateError> {
    if !elevated {
        return Ok(UpdateDir::Temp(temp.join("Negotium-update")));
    }
    program_data
        .map(|pd| UpdateDir::Admin(pd.join("Negotium-update")))
        .ok_or_else(|| UpdateError::Folder("ProgramData could not be found".into()))
}

/// [`update_dir_for`] for this machine: `%TEMP%`, and ProgramData from its known folder.
pub fn update_dir(elevated: bool) -> Result<UpdateDir, UpdateError> {
    use windows::Win32::UI::Shell::FOLDERID_ProgramData;
    let program_data = elevated.then(|| crate::install::known_folder(&FOLDERID_ProgramData)).flatten();
    update_dir_for(elevated, &std::env::temp_dir(), program_data.as_deref())
}

/// An account a folder's owner or DACL entry names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Principal {
    Administrators,
    System,
    /// Anyone else (named for the message).
    Other(String),
}

/// One DACL entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ace {
    Allow(Principal),
    Deny(Principal),
    /// An entry of another kind (object, callback, …): not judged, so not accepted.
    Unknown(u8),
}

/// What [`check_dir`] judges about a folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirSecurity {
    /// The path is a link (junction, symlink, mount point), not the folder itself.
    pub reparse_point: bool,
    pub owner: Principal,
    /// The DACL is protected (nothing inherited from the parent).
    pub protected: bool,
    /// `None`: a NULL DACL (everyone has full control).
    pub dacl: Option<Vec<Ace>>,
}

/// Why the folder isn't admin-only, if it isn't: it must be the folder itself (not a link), owned
/// by Administrators or SYSTEM, with a protected DACL that allows nobody else (deny entries are
/// harmless; entries of other kinds are refused).
pub fn check_dir(s: &DirSecurity) -> Result<(), String> {
    let admin = |p: &Principal| matches!(p, Principal::Administrators | Principal::System);
    if s.reparse_point {
        return Err("is a link".into());
    }
    if !admin(&s.owner) {
        return Err(format!("is owned by {}", name(&s.owner)));
    }
    let Some(dacl) = &s.dacl else { return Err("lets everyone in (no DACL)".into()) };
    if !s.protected {
        return Err("inherits its permissions".into());
    }
    for ace in dacl {
        match ace {
            Ace::Allow(p) if !admin(p) => return Err(format!("lets {} in", name(p))),
            Ace::Unknown(kind) => return Err(format!("has a permission entry of type {kind}")),
            Ace::Allow(_) | Ace::Deny(_) => {}
        }
    }
    Ok(())
}

fn name(p: &Principal) -> &str {
    match p {
        Principal::Administrators => "Administrators",
        Principal::System => "SYSTEM",
        Principal::Other(sid) => sid,
    }
}

/// Makes and reads the admin-only folder. The real one is [`SystemAcl`]; tests fake it.
pub trait DirAcl {
    /// Creates `dir` with [`ADMIN_ONLY_SDDL`]; an existing one is left as it is (`Ok`).
    fn create(&self, dir: &Path) -> std::io::Result<()>;
    /// `dir`'s owner and DACL, read through a handle that doesn't follow links.
    fn read(&self, dir: &Path) -> Result<DirSecurity, String>;
}

/// Creates the admin-only folder if needed and checks it; any doubt refuses the update.
pub fn prepare_admin_dir(dir: &Path, acl: &dyn DirAcl) -> Result<(), UpdateError> {
    let folder = |why: String| UpdateError::Folder(format!("{} {why}", dir.display()));
    acl.create(dir).map_err(|e| folder(format!("couldn't be created: {e}")))?;
    let security = acl.read(dir).map_err(|e| folder(format!("couldn't be checked: {e}")))?;
    check_dir(&security).map_err(|why| folder(format!("{why}; remove it and try again")))
}

/// The real [`DirAcl`]: CreateDirectory with a security descriptor, GetSecurityInfo.
pub struct SystemAcl;

impl DirAcl for SystemAcl {
    fn create(&self, dir: &Path) -> std::io::Result<()> {
        use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, HLOCAL, LocalFree};
        use windows::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
        use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
        use windows::Win32::Storage::FileSystem::CreateDirectoryW;
        use windows::core::HSTRING;
        let mut sd = PSECURITY_DESCRIPTOR::default();
        // SAFETY: a valid SDDL string; `sd` receives a LocalAlloc'd descriptor, freed below.
        unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(&HSTRING::from(ADMIN_ONLY_SDDL), 1, &mut sd, None) }
            .map_err(std::io::Error::other)?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd.0,
            bInheritHandle: false.into(),
        };
        // SAFETY: `attributes` and the descriptor it points at are alive for the call; the path
        // is a NUL-terminated HSTRING.
        let created = unsafe { CreateDirectoryW(&HSTRING::from(dir.as_os_str()), Some(&attributes)) };
        // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW; freed once.
        unsafe { LocalFree(Some(HLOCAL(sd.0))) };
        match created {
            Ok(()) => Ok(()),
            Err(e) if e.code() == ERROR_ALREADY_EXISTS.to_hresult() => Ok(()),
            Err(e) => Err(std::io::Error::other(e)),
        }
    }

    fn read(&self, dir: &Path) -> Result<DirSecurity, String> {
        use windows::Win32::Foundation::{ERROR_SUCCESS, HANDLE, HLOCAL, LocalFree};
        use windows::Win32::Security::Authorization::{GetSecurityInfo, SE_FILE_OBJECT};
        use windows::Win32::Security::{
            ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, DACL_SECURITY_INFORMATION, GetAce, GetSecurityDescriptorControl,
            OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SE_DACL_PROTECTED,
        };
        let handle = std::fs::OpenOptions::new()
            .access_mode(READ_CONTROL | FILE_READ_ATTRIBUTES)
            .share_mode(FILE_SHARE_ALL)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(dir)
            .map_err(|e| e.to_string())?;
        let meta = handle.metadata().map_err(|e| e.to_string())?;
        let reparse_point = meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0;
        if !reparse_point && !meta.is_dir() {
            return Err("is not a folder".into());
        }
        let (mut owner, mut dacl, mut sd) = (PSID::default(), std::ptr::null_mut::<ACL>(), PSECURITY_DESCRIPTOR::default());
        // SAFETY: `handle` is a live handle with READ_CONTROL; the out-parameters are writable
        // locals; `sd` (which `owner` and `dacl` point into) is freed below, after their last use.
        let status = unsafe {
            GetSecurityInfo(
                HANDLE(handle.as_raw_handle()),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                Some(&mut owner),
                None,
                Some(&mut dacl),
                None,
                Some(&mut sd),
            )
        };
        if status != ERROR_SUCCESS {
            return Err(windows::core::Error::from(status.to_hresult()).message());
        }
        let principal = |sid: PSID| sid_principal(sid);
        let result = (|| {
            let (mut control, mut revision) = (0u16, 0u32);
            // SAFETY: `sd` is the descriptor GetSecurityInfo returned; writable locals.
            unsafe { GetSecurityDescriptorControl(sd, &mut control, &mut revision) }.map_err(|e| e.message())?;
            let aces = if dacl.is_null() {
                None
            } else {
                // SAFETY: `dacl` points into `sd`, a valid ACL.
                let count = unsafe { (*dacl).AceCount };
                let mut aces = Vec::with_capacity(count as usize);
                for i in 0..count as u32 {
                    let mut ace = std::ptr::null_mut();
                    // SAFETY: `i` is below the ACL's count; `ace` receives a pointer into the ACL.
                    unsafe { GetAce(dacl, i, &mut ace) }.map_err(|e| e.message())?;
                    // SAFETY: every ACE starts with an ACE_HEADER; allow and deny ACEs share
                    // ACCESS_ALLOWED_ACE's layout, the SID starting at `SidStart`.
                    let kind = unsafe { (*(ace as *const ACE_HEADER)).AceType };
                    let sid = || PSID(unsafe { &raw mut (*(ace as *mut ACCESS_ALLOWED_ACE)).SidStart } as *mut _);
                    aces.push(match kind {
                        0 => Ace::Allow(principal(sid())),
                        1 => Ace::Deny(principal(sid())),
                        other => Ace::Unknown(other),
                    });
                }
                Some(aces)
            };
            Ok(DirSecurity { reparse_point, owner: principal(owner), protected: control & SE_DACL_PROTECTED.0 != 0, dacl: aces })
        })();
        // SAFETY: allocated by GetSecurityInfo; freed once, after the last use of `owner`/`dacl`.
        unsafe { LocalFree(Some(HLOCAL(sd.0))) };
        result
    }
}

/// Administrators, SYSTEM, or the SID's string.
fn sid_principal(sid: windows::Win32::Security::PSID) -> Principal {
    use windows::Win32::Security::{IsWellKnownSid, WinBuiltinAdministratorsSid, WinLocalSystemSid};
    // SAFETY: `sid` is a valid SID inside a live security descriptor.
    unsafe {
        if IsWellKnownSid(sid, WinBuiltinAdministratorsSid).as_bool() {
            return Principal::Administrators;
        }
        if IsWellKnownSid(sid, WinLocalSystemSid).as_bool() {
            return Principal::System;
        }
    }
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
    let mut text = windows::core::PWSTR::null();
    // SAFETY: `sid` is valid; `text` receives a LocalAlloc'd string, read and freed below.
    let name = unsafe {
        ConvertSidToStringSidW(sid, &mut text).ok().and_then(|()| {
            let s = text.to_string().ok();
            LocalFree(Some(HLOCAL(text.0.cast())));
            s
        })
    };
    Principal::Other(name.unwrap_or_else(|| "another account".into()))
}

/// Opens the installer for hashing and holding: read access, shared for reading only (nothing
/// can write, rename or delete it while this is open), the path's last part not followed if it is
/// a link (a link is refused).
pub fn open_held(msi: &Path) -> Result<File, UpdateError> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(msi)
        .map_err(|e| UpdateError::Io(format!("{}: {e}", msi.display())))?;
    let meta = file.metadata().map_err(|e| UpdateError::Io(e.to_string()))?;
    if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 || !meta.is_file() {
        return Err(UpdateError::Download(format!("{} is not a plain file", msi.display())));
    }
    Ok(file)
}

/// Deletes `path` itself, through a handle that doesn't follow a link (a link is left alone and
/// refused): an elevated Negotium never deletes where a planted junction points.
pub fn delete_no_follow(path: &Path) -> Result<(), UpdateError> {
    let file = std::fs::OpenOptions::new()
        .access_mode(DELETE | FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_ALL)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map_err(|e| UpdateError::Io(e.to_string()))?;
    let meta = file.metadata().map_err(|e| UpdateError::Io(e.to_string()))?;
    if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 || !meta.is_file() {
        return Err(UpdateError::Io(format!("{} is not a plain file; not deleted", path.display())));
    }
    delete_handle(&file).map_err(|e| UpdateError::Io(e.message()))
}

/// Marks the file `file` is open on (with DELETE access) for deletion: it goes when the last
/// handle closes, wherever it is, whatever its path now names.
pub fn delete_handle(file: &File) -> windows::core::Result<()> {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{FILE_DISPOSITION_INFO, FileDispositionInfo, SetFileInformationByHandle};
    let info = FILE_DISPOSITION_INFO { DeleteFile: true };
    // SAFETY: `file` is a live handle; `info` is a valid FILE_DISPOSITION_INFO of the size passed.
    unsafe {
        SetFileInformationByHandle(
            HANDLE(file.as_raw_handle()),
            FileDispositionInfo,
            &info as *const FILE_DISPOSITION_INFO as *const _,
            std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn an_elevated_update_uses_the_admin_only_folder() {
        let (temp, pd) = (Path::new(r"C:\Users\me\AppData\Local\Temp"), Path::new(r"C:\ProgramData"));
        assert_eq!(update_dir_for(false, temp, Some(pd)), Ok(UpdateDir::Temp(temp.join("Negotium-update"))));
        assert_eq!(update_dir_for(true, temp, Some(pd)), Ok(UpdateDir::Admin(pd.join("Negotium-update"))));
        assert!(matches!(update_dir_for(true, temp, None), Err(UpdateError::Folder(_))), "elevated never falls back to %TEMP%");
    }

    fn safe() -> DirSecurity {
        DirSecurity {
            reparse_point: false,
            owner: Principal::Administrators,
            protected: true,
            dacl: Some(vec![Ace::Allow(Principal::Administrators), Ace::Allow(Principal::System)]),
        }
    }

    #[test]
    fn only_an_admin_only_folder_passes() {
        assert_eq!(check_dir(&safe()), Ok(()));
        assert_eq!(check_dir(&DirSecurity { owner: Principal::System, ..safe() }), Ok(()), "SYSTEM may own it");
        let denied = DirSecurity { dacl: Some(vec![Ace::Deny(Principal::Other("S-1-5-32-545".into())), Ace::Allow(Principal::System)]), ..safe() };
        assert_eq!(check_dir(&denied), Ok(()), "a deny entry is harmless");
        let user = Principal::Other("S-1-5-21-1-2-3-1001".into());
        for (unsafe_dir, why) in [
            (DirSecurity { reparse_point: true, ..safe() }, "link"),
            (DirSecurity { owner: user.clone(), ..safe() }, "owned by S-1-5-21-1-2-3-1001"),
            (DirSecurity { protected: false, ..safe() }, "inherits"),
            (DirSecurity { dacl: None, ..safe() }, "everyone"),
            (DirSecurity { dacl: Some(vec![Ace::Allow(Principal::Administrators), Ace::Allow(user.clone())]), ..safe() }, "lets S-1-5-21"),
            (DirSecurity { dacl: Some(vec![Ace::Unknown(5)]), ..safe() }, "type 5"),
        ] {
            let got = check_dir(&unsafe_dir);
            assert!(got.as_ref().is_err_and(|e| e.contains(why)), "{unsafe_dir:?}: {got:?}");
        }
    }

    /// Answers `create` and `read` as set, recording what was asked.
    struct FakeAcl {
        create: Result<(), std::io::ErrorKind>,
        read: Result<DirSecurity, String>,
        asked: RefCell<Vec<&'static str>>,
    }

    impl DirAcl for FakeAcl {
        fn create(&self, _: &Path) -> std::io::Result<()> {
            self.asked.borrow_mut().push("create");
            self.create.map_err(std::io::Error::from)
        }
        fn read(&self, _: &Path) -> Result<DirSecurity, String> {
            self.asked.borrow_mut().push("read");
            self.read.clone()
        }
    }

    fn fake(create: Result<(), std::io::ErrorKind>, read: Result<DirSecurity, String>) -> FakeAcl {
        FakeAcl { create, read, asked: RefCell::new(Vec::new()) }
    }

    #[test]
    fn the_admin_folder_is_made_and_checked_on_every_use() {
        let dir = UpdateDir::Admin(PathBuf::from(r"C:\ProgramData\Negotium-update"));
        let acl = fake(Ok(()), Ok(safe()));
        assert_eq!(dir.prepare(&acl), Ok(Path::new(r"C:\ProgramData\Negotium-update")));
        assert_eq!(dir.prepare(&acl).map(Path::to_path_buf), Ok(r"C:\ProgramData\Negotium-update".into()));
        assert_eq!(*acl.asked.borrow(), ["create", "read", "create", "read"]);
        for acl in [
            fake(Err(std::io::ErrorKind::PermissionDenied), Ok(safe())),
            fake(Ok(()), Err("access denied".into())),
            fake(Ok(()), Ok(DirSecurity { owner: Principal::Other("S-1-5-21-9".into()), ..safe() })),
            fake(Ok(()), Ok(DirSecurity { reparse_point: true, ..safe() })),
        ] {
            let got = dir.prepare(&acl);
            assert!(matches!(&got, Err(UpdateError::Folder(m)) if m.starts_with(r"C:\ProgramData\Negotium-update ")), "{got:?}");
        }
    }

    #[test]
    fn the_real_reader_sees_a_users_folder_as_unsafe() {
        let dir = std::env::temp_dir().join(format!("negotium-update-acl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let got = SystemAcl.read(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        let security = got.unwrap();
        assert!(!security.reparse_point);
        assert!(check_dir(&security).is_err(), "a folder under %TEMP% is not admin-only: {security:?}");
    }

    /// Creates a uniquely named admin-only folder under ProgramData, reads it back as safe, then
    /// removes it.
    #[test]
    #[ignore = "needs an elevated terminal"]
    fn the_real_admin_folder_is_admin_only() {
        use windows::Win32::UI::Shell::FOLDERID_ProgramData;
        let pd = crate::install::known_folder(&FOLDERID_ProgramData).unwrap();
        let dir = pd.join(format!("Negotium-update-test-{}", std::process::id()));
        let result = prepare_admin_dir(&dir, &SystemAcl);
        let security = SystemAcl.read(&dir);
        let _ = std::fs::remove_dir(&dir);
        assert_eq!(result, Ok(()));
        let security = security.unwrap();
        assert_eq!(security.owner, Principal::Administrators);
        assert!(security.protected);
        assert_eq!(security.dacl, Some(vec![Ace::Allow(Principal::Administrators), Ace::Allow(Principal::System)]));
    }

    #[test]
    fn a_held_installer_cant_be_changed_or_deleted() {
        let dir = std::env::temp_dir().join(format!("negotium-update-held-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let msi = dir.join("Negotium-0.2.0.msi");
        std::fs::write(&msi, b"abc").unwrap();
        let held = open_held(&msi).unwrap();
        assert!(std::fs::OpenOptions::new().write(true).open(&msi).is_err(), "no writing while held");
        assert!(std::fs::remove_file(&msi).is_err(), "no deleting while held");
        assert!(std::fs::rename(&msi, dir.join("other.msi")).is_err(), "no renaming while held");
        assert!(std::fs::File::open(&msi).is_ok(), "reading is fine (msiexec)");
        drop(held);
        delete_no_follow(&msi).unwrap();
        assert!(!msi.exists(), "deleted through its handle");
        assert!(open_held(&dir).is_err(), "a folder is not an installer");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
