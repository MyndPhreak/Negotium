//! Signed-in users: sessions, account facts of the current user, and the lock / sign-out actions.
//! Call off the UI thread.
use crate::details::sid_account;
use crate::integration::{ADMINISTRATORS, current_user_sid};
use crate::startup::folders::known_folder;
use crate::startup::registry::{Hive, RegKey, RegValue, subkeys, values};
use crate::ActionError;
use std::ffi::c_void;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::{HLOCAL, LocalFree};
use windows::Win32::NetworkManagement::NetManagement::{
    LG_INCLUDE_INDIRECT, LOCALGROUP_USERS_INFO_0, MAX_PREFERRED_LENGTH, NetApiBufferFree, NetUserGetInfo, NetUserGetLocalGroups,
    USER_INFO_2, USER_INFO_24,
};
use windows::Win32::Security::Authorization::ConvertStringSidToSidW;
use windows::Win32::Security::PSID;
use windows::Win32::System::RemoteDesktop::{
    ProcessIdToSessionId, WTS_SESSION_INFOW, WTSDisconnectSession, WTSEnumerateSessionsW, WTSFreeMemory, WTSINFOW,
    WTSLogoffSession, WTSQuerySessionInformationW, WTSSessionInfo,
};
use windows::Win32::System::Shutdown::{EWX_LOGOFF, ExitWindowsEx, LockWorkStation, SHUTDOWN_REASON};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Shell::FOLDERID_Profile;
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_REMOTESESSION};
use windows::core::{HSTRING, PWSTR};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionState {
    Active,
    Disconnected,
    Other,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Session {
    pub id: u32,
    pub user: String,
    pub domain: String,
    pub state: SessionState,
    pub signed_in_secs: Option<u64>,
    pub idle_secs: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SignIn {
    pub pin: bool,
    pub face: bool,
    pub fingerprint: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Account {
    /// `DOMAIN\user`.
    pub name: Option<String>,
    pub sid: Option<String>,
    pub full_name: Option<String>,
    pub email: Option<String>,
    pub password_age_secs: Option<u64>,
    pub groups: Option<Vec<String>>,
    pub picture: Option<PathBuf>,
    pub profile: Option<PathBuf>,
    pub sign_in: Option<SignIn>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DirSize {
    pub bytes: u64,
    pub skipped: u32,
}

/// `WTS_CONNECTSTATE_CLASS`: 0 is WTSActive, 4 is WTSDisconnected.
pub fn session_state(raw: i32) -> SessionState {
    match raw {
        0 => SessionState::Active,
        4 => SessionState::Disconnected,
        _ => SessionState::Other,
    }
}

/// FILETIME units (100 ns). 0 or `from > now` gives None.
pub fn elapsed_secs(from_ft: i64, now_ft: i64) -> Option<u64> {
    (from_ft > 0 && from_ft <= now_ft).then(|| ((now_ft - from_ft) / 10_000_000) as u64)
}

fn utf16_until_nul(units: &[u16]) -> String {
    String::from_utf16_lossy(units.split(|u| *u == 0).next().unwrap_or_default())
}

/// None when the session has no user name (session 0, listeners).
pub fn session_from_info(info: &WTSINFOW) -> Option<Session> {
    let user = utf16_until_nul(&info.UserName);
    if user.is_empty() {
        return None;
    }
    Some(Session {
        id: info.SessionId,
        user,
        domain: utf16_until_nul(&info.Domain),
        state: session_state(info.State.0),
        signed_in_secs: elapsed_secs(info.LogonTime, info.CurrentTime),
        idle_secs: elapsed_secs(info.LastInputTime, info.CurrentTime),
    })
}

/// WINBIO_TYPE_FACIAL_FEATURES and WINBIO_TYPE_FINGERPRINT in `EnrolledFactors`.
const FACTOR_FACE: u32 = 0x2;
const FACTOR_FINGERPRINT: u32 = 0x8;

/// None when either read failed (shown as a dash).
pub fn sign_in_from(pin: Option<bool>, factors: Option<u32>) -> Option<SignIn> {
    let (pin, factors) = (pin?, factors?);
    Some(SignIn { pin, face: factors & FACTOR_FACE != 0, fingerprint: factors & FACTOR_FINGERPRINT != 0 })
}

/// Sessions with a user. A session whose query fails is skipped.
pub fn list_sessions() -> Result<Vec<Session>, ActionError> {
    let mut list: *mut WTS_SESSION_INFOW = std::ptr::null_mut();
    let mut count = 0u32;
    // SAFETY: the out pointers are valid locals; the list is freed below.
    unsafe { WTSEnumerateSessionsW(None, 0, 1, &mut list, &mut count) }.map_err(|e| ActionError::from_win(&e))?;
    // SAFETY: Windows returned `count` entries at `list`; they stay valid until the free below.
    let ids: Vec<u32> = unsafe { std::slice::from_raw_parts(list, count as usize) }.iter().map(|s| s.SessionId).collect();
    // SAFETY: allocated by WTSEnumerateSessionsW; freed exactly once (the ids were copied out).
    unsafe { WTSFreeMemory(list.cast()) };
    Ok(ids.into_iter().filter_map(query_session).collect())
}

fn query_session(id: u32) -> Option<Session> {
    let mut buf = PWSTR::null();
    let mut bytes = 0u32;
    // SAFETY: the out pointers are valid locals; the buffer is freed below.
    unsafe { WTSQuerySessionInformationW(None, id, WTSSessionInfo, &mut buf, &mut bytes) }.ok()?;
    let session = if bytes as usize >= size_of::<WTSINFOW>() && !buf.is_null() {
        // SAFETY: Windows wrote a WTSINFOW of at least `bytes` bytes at `buf`; it may not be
        // aligned for the struct, hence the unaligned read of a copy.
        let info = unsafe { std::ptr::read_unaligned(buf.0.cast::<WTSINFOW>()) };
        session_from_info(&info)
    } else {
        None
    };
    // SAFETY: allocated by WTSQuerySessionInformationW; freed exactly once.
    unsafe { WTSFreeMemory(buf.0.cast::<c_void>()) };
    session
}

pub fn current_session_id() -> Option<u32> {
    let mut id = 0u32;
    // SAFETY: `id` is a valid out pointer.
    unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut id) }.ok()?;
    Some(id)
}

pub fn is_remote_session() -> bool {
    // SAFETY: a plain metrics query.
    unsafe { GetSystemMetrics(SM_REMOTESESSION) != 0 }
}

/// (domain, name) of the account the string SID `sid` names.
fn account_of_sid(sid: &str) -> Option<(String, String)> {
    let mut psid = PSID::default();
    // SAFETY: `psid` receives a LocalAlloc'd SID that is freed below.
    unsafe { ConvertStringSidToSidW(&HSTRING::from(sid), &mut psid) }.ok()?;
    // SAFETY: `psid` is the valid SID just built.
    let found = unsafe { sid_account(psid) };
    // SAFETY: allocated by ConvertStringSidToSidW with LocalAlloc; freed exactly once.
    unsafe { LocalFree(Some(HLOCAL(psid.0))) };
    found
}

/// Frees a NetApi buffer on drop.
struct NetBuf(*mut u8);

impl Drop for NetBuf {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: allocated by a NetApi call; freed exactly once.
            unsafe { NetApiBufferFree(Some(self.0.cast_const().cast())) };
        }
    }
}

fn text(p: PWSTR) -> Option<String> {
    // SAFETY: a PWSTR inside a NetApi struct is null or NUL-terminated and alive while its buffer is.
    let s = if p.is_null() { String::new() } else { unsafe { p.to_string() }.ok()? };
    (!s.is_empty()).then_some(s)
}

/// (full name, password age in seconds) from `NetUserGetInfo` level 2.
fn user_info_2(user: &str) -> Option<(Option<String>, u64)> {
    let mut buf = std::ptr::null_mut();
    // SAFETY: the out pointer is a valid local; the buffer is freed by `NetBuf`.
    let status = unsafe { NetUserGetInfo(None, &HSTRING::from(user), 2, &mut buf) };
    let buf = NetBuf(buf);
    if status != 0 || buf.0.is_null() {
        return None;
    }
    // SAFETY: level 2 returns a USER_INFO_2 that lives until `buf` drops.
    let info = unsafe { &*(buf.0 as *const USER_INFO_2) };
    Some((text(info.usri2_full_name), u64::from(info.usri2_password_age)))
}

/// The Microsoft account email from `NetUserGetInfo` level 24 (fails for other accounts).
fn user_email(user: &str) -> Option<String> {
    let mut buf = std::ptr::null_mut();
    // SAFETY: the out pointer is a valid local; the buffer is freed by `NetBuf`.
    let status = unsafe { NetUserGetInfo(None, &HSTRING::from(user), 24, &mut buf) };
    let buf = NetBuf(buf);
    if status != 0 || buf.0.is_null() {
        return None;
    }
    // SAFETY: level 24 returns a USER_INFO_24 that lives until `buf` drops.
    let info = unsafe { &*(buf.0 as *const USER_INFO_24) };
    if info.usri24_internet_identity.as_bool() { text(info.usri24_internet_principal_name) } else { None }
}

/// The local groups `account` (`DOMAIN\user`) belongs to, directly or through nesting.
pub fn local_groups(account: &str) -> Option<Vec<String>> {
    let (mut buf, mut read, mut total) = (std::ptr::null_mut(), 0u32, 0u32);
    // SAFETY: the out pointers are valid locals; the buffer is freed by `NetBuf`.
    let status = unsafe {
        NetUserGetLocalGroups(None, &HSTRING::from(account), 0, LG_INCLUDE_INDIRECT, &mut buf, MAX_PREFERRED_LENGTH, &mut read, &mut total)
    };
    let buf = NetBuf(buf);
    if status != 0 {
        return None;
    }
    if buf.0.is_null() {
        return Some(Vec::new());
    }
    // SAFETY: level 0 returns `read` LOCALGROUP_USERS_INFO_0 entries that live until `buf` drops.
    let entries = unsafe { std::slice::from_raw_parts(buf.0 as *const LOCALGROUP_USERS_INFO_0, read as usize) };
    Some(entries.iter().filter_map(|e| text(e.lgrui0_name)).collect())
}

/// The local name of the Administrators group (S-1-5-32-544).
pub fn admin_group_name() -> Option<String> {
    account_of_sid(ADMINISTRATORS).map(|(_, name)| name)
}

const CREDENTIAL_PROVIDERS: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Authentication\Credential Providers";
const PIN_PROVIDER: &str = "{D6886603-9D2F-4EB2-B667-1971041FA96B}";
const WINBIO_ACCOUNTS: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\WinBio\AccountInfo";
const ACCOUNT_PICTURES: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\AccountPicture\Users";

/// Whether a PIN is set: the provider has a subkey named after the SID. A missing key is "not set".
fn has_pin(sid: &str) -> Option<bool> {
    let key = RegKey::new(Hive::LocalMachine, &format!(r"{CREDENTIAL_PROVIDERS}\{PIN_PROVIDER}"));
    Some(subkeys(&key).ok()?.iter().any(|k| k.eq_ignore_ascii_case(sid)))
}

/// The enrolled biometric factor bits. A missing key is 0 (nothing enrolled).
fn enrolled_factors(sid: &str) -> Option<u32> {
    let key = RegKey::new(Hive::LocalMachine, &format!(r"{WINBIO_ACCOUNTS}\{sid}"));
    let all = values(&key).ok()?;
    Some(all.into_iter().find_map(|(n, v)| match v {
        RegValue::Dword(f) if n.eq_ignore_ascii_case("EnrolledFactors") => Some(f),
        _ => None,
    })
    .unwrap_or(0))
}

/// The account picture: the 192 px image, else the 96 px one.
fn picture_path(sid: &str) -> Option<PathBuf> {
    let all = values(&RegKey::new(Hive::LocalMachine, &format!(r"{ACCOUNT_PICTURES}\{sid}"))).ok()?;
    let find = |wanted: &str| {
        all.iter().find_map(|(n, v)| match v {
            RegValue::Str(p) if n.eq_ignore_ascii_case(wanted) && !p.is_empty() => Some(PathBuf::from(p)),
            _ => None,
        })
    };
    find("Image192").or_else(|| find("Image96"))
}

/// Never fails as a whole: a field Windows refuses stays None.
pub fn current_account() -> Account {
    let sid = current_user_sid().ok();
    let name = sid.as_deref().and_then(account_of_sid).map(|(domain, name)| format!("{domain}\\{name}"));
    let user = name.as_deref().and_then(|n| n.rsplit('\\').next());
    let info = user.and_then(user_info_2);
    Account {
        full_name: info.as_ref().and_then(|i| i.0.clone()),
        password_age_secs: info.map(|i| i.1),
        email: user.and_then(user_email),
        groups: name.as_deref().and_then(local_groups),
        picture: sid.as_deref().and_then(picture_path),
        profile: known_folder(&FOLDERID_Profile),
        sign_in: sid.as_deref().and_then(|s| sign_in_from(has_pin(s), enrolled_factors(s))),
        name,
        sid,
    }
}

/// Logical size of everything under `path`. Reparse points (junctions, symlinks) are not followed,
/// so `Application Data`-style loops end; a folder or entry that cannot be read counts as skipped.
pub fn dir_size(path: &Path) -> DirSize {
    const REPARSE_POINT: u32 = 0x400;
    let mut out = DirSize::default();
    let mut pending = vec![path.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            out.skipped += 1;
            continue;
        };
        for entry in entries {
            let Ok(meta) = entry.and_then(|e| e.metadata().map(|m| (e.path(), m))) else {
                out.skipped += 1;
                continue;
            };
            let (p, m) = meta;
            if m.file_attributes() & REPARSE_POINT != 0 {
                continue;
            }
            if m.is_dir() {
                pending.push(p);
            } else {
                out.bytes += m.len();
            }
        }
    }
    out
}

pub fn lock() -> Result<(), ActionError> {
    // SAFETY: no arguments; locks this session.
    unsafe { LockWorkStation() }.map_err(|e| ActionError::from_win(&e))
}

/// Disconnects the current session to the sign-in screen (what `tsdiscon` does).
pub fn switch_user() -> Result<(), ActionError> {
    // SAFETY: the current session on the local server.
    unsafe { WTSDisconnectSession(None, u32::MAX, false) }.map_err(|e| ActionError::from_win(&e))
}

/// Not forced: apps can ask to save.
pub fn sign_out() -> Result<(), ActionError> {
    // SAFETY: a plain logoff request.
    unsafe { ExitWindowsEx(EWX_LOGOFF, SHUTDOWN_REASON(0)) }.map_err(|e| ActionError::from_win(&e))
}

pub fn sign_out_session(id: u32) -> Result<(), ActionError> {
    // SAFETY: the local server; `id` is a session number.
    unsafe { WTSLogoffSession(None, id, false) }.map_err(|e| ActionError::from_win(&e))
}

pub fn disconnect_session(id: u32) -> Result<(), ActionError> {
    // SAFETY: the local server; `id` is a session number.
    unsafe { WTSDisconnectSession(None, id, false) }.map_err(|e| ActionError::from_win(&e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integration::{current_user_sid, is_admin_member};
    use std::fs;
    use std::process::Command;

    fn wide<const N: usize>(s: &str) -> [u16; N] {
        let mut out = [0u16; N];
        for (slot, unit) in out.iter_mut().zip(s.encode_utf16()) {
            *slot = unit;
        }
        out
    }

    /// A fresh directory under the temp dir, removed (with any ACL denial lifted) on drop.
    struct Sandbox(PathBuf);

    impl Sandbox {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("negotium-users-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Sandbox(dir)
        }
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            let _ = Command::new("icacls").arg(&self.0).args(["/reset", "/t", "/q"]).output();
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write(path: &Path, bytes: usize) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, vec![b'x'; bytes]).unwrap();
    }

    #[test]
    fn session_state_maps_active_and_disconnected_and_the_rest_to_other() {
        for (raw, want) in [
            (0, SessionState::Active),
            (4, SessionState::Disconnected),
            (1, SessionState::Other),
            (2, SessionState::Other),
            (3, SessionState::Other),
            (5, SessionState::Other),
            (6, SessionState::Other),
            (7, SessionState::Other),
            (8, SessionState::Other),
            (9, SessionState::Other),
            (-1, SessionState::Other),
        ] {
            assert_eq!(session_state(raw), want, "raw {raw}");
        }
    }

    #[test]
    fn elapsed_secs_counts_filetime_units_and_refuses_zero_and_backwards() {
        let t = 133_000_000_000_000_000i64;
        assert_eq!(elapsed_secs(t, t + 1_200_000_000), Some(120));
        assert_eq!(elapsed_secs(0, t), None);
        assert_eq!(elapsed_secs(t, t - 1), None);
        assert_eq!(elapsed_secs(t, t), Some(0));
    }

    #[test]
    fn session_from_info_reads_user_domain_state_and_times() {
        let now = 133_000_000_000_000_000i64;
        let info = WTSINFOW {
            State: windows::Win32::System::RemoteDesktop::WTS_CONNECTSTATE_CLASS(4),
            SessionId: 3,
            Domain: wide("PC"),
            UserName: wide("alex"),
            LogonTime: now - 6_000_000_000,
            LastInputTime: 0,
            CurrentTime: now,
            ..Default::default()
        };
        let want = Session {
            id: 3,
            user: "alex".into(),
            domain: "PC".into(),
            state: SessionState::Disconnected,
            signed_in_secs: Some(600),
            idle_secs: None,
        };
        assert_eq!(session_from_info(&info), Some(want));
    }

    #[test]
    fn session_from_info_skips_a_session_with_no_user() {
        let info = WTSINFOW { SessionId: 0, Domain: wide("WORKGROUP"), ..Default::default() };
        assert_eq!(session_from_info(&info), None);
    }

    #[test]
    fn sign_in_from_reads_pin_and_the_enrolled_factor_bits() {
        let s = |pin, face, fingerprint| Some(SignIn { pin, face, fingerprint });
        assert_eq!(sign_in_from(Some(true), Some(0x2)), s(true, true, false));
        assert_eq!(sign_in_from(Some(false), Some(0x8)), s(false, false, true));
        assert_eq!(sign_in_from(Some(false), Some(0x0A)), s(false, true, true));
        assert_eq!(sign_in_from(Some(false), Some(0)), s(false, false, false));
        assert_eq!(sign_in_from(None, Some(0)), None);
        assert_eq!(sign_in_from(Some(true), None), None);
    }

    #[test]
    fn dir_size_adds_files_in_two_levels() {
        let sb = Sandbox::new("sum");
        write(&sb.0.join("a.bin"), 10);
        write(&sb.0.join("sub").join("b.bin"), 20);
        write(&sb.0.join("sub").join("c.bin"), 30);
        assert_eq!(dir_size(&sb.0), DirSize { bytes: 60, skipped: 0 });
    }

    #[test]
    fn dir_size_does_not_follow_a_junction() {
        let sb = Sandbox::new("junction");
        let target = Sandbox::new("junction-target");
        write(&target.0.join("big.bin"), 1000);
        write(&sb.0.join("a.bin"), 10);
        let link = sb.0.join("link");
        let made = Command::new("cmd").arg("/C").arg("mklink").arg("/J").arg(&link).arg(&target.0).output().unwrap();
        assert!(made.status.success(), "mklink: {}", String::from_utf8_lossy(&made.stdout));
        assert_eq!(dir_size(&sb.0), DirSize { bytes: 10, skipped: 0 });
    }

    #[test]
    fn dir_size_counts_a_denied_folder_as_skipped_and_the_rest() {
        let sb = Sandbox::new("denied");
        write(&sb.0.join("a.bin"), 10);
        write(&sb.0.join("locked").join("b.bin"), 500);
        let user = std::env::var("USERNAME").unwrap();
        let denied = Command::new("icacls").arg(sb.0.join("locked")).arg("/deny").arg(format!("{user}:(RX)")).output().unwrap();
        assert!(denied.status.success(), "icacls: {}", String::from_utf8_lossy(&denied.stdout));
        assert_eq!(dir_size(&sb.0), DirSize { bytes: 10, skipped: 1 });
    }

    #[test]
    fn dir_size_of_a_missing_path_is_zero_with_one_skipped() {
        let missing = std::env::temp_dir().join(format!("negotium-users-missing-{}", std::process::id()));
        assert_eq!(dir_size(&missing), DirSize { bytes: 0, skipped: 1 });
    }

    #[test]
    fn list_sessions_contains_the_current_session_as_the_current_user() {
        let id = current_session_id().expect("current session id");
        let sessions = list_sessions().expect("list sessions");
        let mine = sessions.iter().find(|s| s.id == id).unwrap_or_else(|| panic!("session {id} in {sessions:?}"));
        let account = current_account().name.expect("account name");
        assert!(account.eq_ignore_ascii_case(&format!("{}\\{}", mine.domain, mine.user)), "{account} vs {mine:?}");
        assert_eq!(mine.state, SessionState::Active);
    }

    #[test]
    fn current_account_names_the_user_sid_and_an_existing_profile() {
        let account = current_account();
        let name = account.name.expect("name");
        let user = std::env::var("USERNAME").unwrap();
        assert!(name.to_lowercase().ends_with(&format!("\\{}", user.to_lowercase())), "{name}");
        assert_eq!(account.sid, current_user_sid().ok());
        assert!(account.profile.expect("profile").is_dir());
    }

    #[test]
    fn admin_group_name_is_some_and_not_empty() {
        assert!(admin_group_name().is_some_and(|n| !n.is_empty()));
    }

    #[test]
    fn the_token_and_the_group_lookup_agree_on_the_current_user_being_an_admin() {
        let name = current_account().name.expect("name");
        let admin = admin_group_name().expect("admin group");
        let groups = local_groups(&name).expect("groups");
        let listed = groups.iter().any(|g| g.eq_ignore_ascii_case(&admin));
        assert_eq!(is_admin_member(), listed, "groups: {groups:?}");
    }

    #[test]
    fn logging_off_or_disconnecting_a_session_that_does_not_exist_fails() {
        assert!(sign_out_session(65_000).is_err());
        assert!(disconnect_session(65_000).is_err());
    }
}
