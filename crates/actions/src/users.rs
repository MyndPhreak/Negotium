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
    None
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
    Err(ActionError::Other("stub".into()))
}

pub fn disconnect_session(_id: u32) -> Result<(), ActionError> {
    Err(ActionError::Other("stub".into()))
}
