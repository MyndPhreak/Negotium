//! Signed-in users: sessions, account facts of the current user, and the lock / sign-out actions.
//! Call off the UI thread.
use crate::ActionError;
use std::path::{Path, PathBuf};
use windows::Win32::System::RemoteDesktop::WTSINFOW;

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

pub fn session_state(_raw: i32) -> SessionState {
    SessionState::Other
}

/// FILETIME units (100 ns). 0 or `from > now` gives None.
pub fn elapsed_secs(_from_ft: i64, _now_ft: i64) -> Option<u64> {
    None
}

/// None when the session has no user name (session 0, listeners).
pub fn session_from_info(_info: &WTSINFOW) -> Option<Session> {
    Some(Session { id: 0, user: String::new(), domain: String::new(), state: SessionState::Other, signed_in_secs: None, idle_secs: None })
}

pub fn sign_in_from(_pin: Option<bool>, _factors: Option<u32>) -> Option<SignIn> {
    None
}

pub fn list_sessions() -> Result<Vec<Session>, ActionError> {
    Err(ActionError::Other("stub".into()))
}

pub fn current_session_id() -> Option<u32> {
    None
}

pub fn is_remote_session() -> bool {
    false
}

/// Never fails as a whole: a field Windows refuses stays None.
pub fn current_account() -> Account {
    Account::default()
}

pub fn local_groups(_account: &str) -> Option<Vec<String>> {
    None
}

/// The local name of the Administrators group (S-1-5-32-544).
pub fn admin_group_name() -> Option<String> {
    None
}

pub fn dir_size(_path: &Path) -> DirSize {
    DirSize::default()
}

pub fn lock() -> Result<(), ActionError> {
    Err(ActionError::Other("stub".into()))
}

pub fn switch_user() -> Result<(), ActionError> {
    Err(ActionError::Other("stub".into()))
}

pub fn sign_out() -> Result<(), ActionError> {
    Err(ActionError::Other("stub".into()))
}

pub fn sign_out_session(_id: u32) -> Result<(), ActionError> {
    Ok(())
}

pub fn disconnect_session(_id: u32) -> Result<(), ActionError> {
    Ok(())
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
