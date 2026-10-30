//! The Users page: text and decisions as pure functions, plus the read and action jobs.
use negotium_actions::ActionError;
use negotium_actions::users::{Account, DirSize, Session, SessionState, SignIn};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub const USERS_REFRESH: Duration = Duration::from_secs(5);
pub const PROFILE_SIZE_MAX_AGE: Duration = Duration::from_secs(600);

pub struct UsersRead {
    pub sessions: Result<Vec<Session>, String>,
    pub current: Option<u32>,
    pub remote: bool,
    pub account: Account,
    pub admin_member: bool,
    pub admin_group: Option<String>,
    /// Key: lower-case `DOMAIN\user`.
    pub groups: HashMap<String, Option<Vec<String>>>,
}

pub enum ProfileSize {
    Calculating,
    Done(DirSize),
    Unknown,
}

#[derive(Default)]
pub struct YouView {
    pub initial: String,
    pub name: String,
    pub subtitle: String,
    pub account_type: String,
    pub signed_in: String,
    pub place: String,
    pub profile_path: String,
    pub profile_size: String,
    pub sign_in: String,
    pub password_set: String,
    pub groups: String,
    pub negotium_admin: String,
    pub picture: Option<PathBuf>,
}

pub struct OtherRow {
    pub id: i32,
    pub name: String,
    pub account_type: String,
    pub state: String,
    pub idle: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UserAction {
    Lock,
    SwitchUser,
    SignOut,
    AccountSettings,
    SignOutSession(u32),
    Disconnect(u32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UserPlan {
    Run,
    Confirm { title: String, body: String, label: String },
    NeedsAdmin(String),
}

pub fn initial(_name: &str) -> String {
    String::new()
}

pub fn span_text(_secs: Option<u64>) -> String {
    String::new()
}

pub fn password_age_text(_secs: Option<u64>) -> String {
    String::new()
}

pub fn sign_in_text(_s: Option<SignIn>) -> String {
    String::new()
}

pub fn groups_text(_g: Option<&[String]>) -> String {
    String::new()
}

pub fn size_text(_s: &ProfileSize) -> String {
    String::new()
}

pub fn state_text(_s: SessionState) -> &'static str {
    ""
}

pub fn account_type_text(_groups: Option<&[String]>, _admin_group: Option<&str>) -> &'static str {
    ""
}

pub fn you_view(_read: &UsersRead, _sessions: &[Session], _size: &ProfileSize, _elevated: bool) -> YouView {
    YouView::default()
}

pub fn other_rows(_read: &UsersRead, _sessions: &[Session]) -> Vec<OtherRow> {
    Vec::new()
}

pub fn action_from(_name: &str, _session: i32) -> Option<UserAction> {
    None
}

pub fn plan_user_action(_a: UserAction, _user: &str, _elevated: bool) -> UserPlan {
    UserPlan::Run
}

pub fn failure_text(_a: UserAction, _user: &str, _e: &ActionError) -> String {
    String::new()
}

pub fn profile_size_due(_last: Option<Instant>, _running: bool, _now: Instant) -> bool {
    false
}

pub fn read_job() -> UsersRead {
    UsersRead {
        sessions: Err(String::new()),
        current: None,
        remote: false,
        account: Account::default(),
        admin_member: false,
        admin_group: None,
        groups: HashMap::new(),
    }
}

/// Runs the action; the failure text, or None when it worked.
pub fn run_user_action(_a: UserAction, _user: &str, _owner: isize) -> Option<String> {
    None
}
