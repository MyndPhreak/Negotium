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
    UserPlan::NeedsAdmin(String::new())
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

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: u64 = 60;

    fn names(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn session(id: u32, user: &str, state: SessionState, signed_in: Option<u64>, idle: Option<u64>) -> Session {
        Session { id, user: user.into(), domain: "PC".into(), state, signed_in_secs: signed_in, idle_secs: idle }
    }

    fn account() -> Account {
        Account { name: Some(r"PC\alex".into()), ..Default::default() }
    }

    fn read(account: Account, sessions: Vec<Session>) -> UsersRead {
        UsersRead {
            sessions: Ok(sessions),
            current: Some(1),
            remote: false,
            account,
            admin_member: false,
            admin_group: Some("Administrators".into()),
            groups: HashMap::new(),
        }
    }

    #[test]
    fn span_text_is_a_dash_without_a_value() {
        assert_eq!(span_text(None), "—");
        assert_eq!(span_text(Some(600)), "10 min");
    }

    #[test]
    fn password_age_text_counts_whole_days() {
        for (secs, want) in [
            (None, "—"),
            (Some(0), "Today"),
            (Some(86_399), "Today"),
            (Some(86_400), "1 day ago"),
            (Some(48_211_200), "558 days ago"),
        ] {
            assert_eq!(password_age_text(secs), want, "{secs:?}");
        }
    }

    #[test]
    fn sign_in_text_always_lists_password_and_then_the_set_methods() {
        assert_eq!(sign_in_text(None), "—");
        assert_eq!(sign_in_text(Some(SignIn::default())), "Password");
        assert_eq!(sign_in_text(Some(SignIn { pin: true, face: true, fingerprint: true })), "Password, PIN, Face, Fingerprint");
        assert_eq!(sign_in_text(Some(SignIn { pin: false, face: true, fingerprint: false })), "Password, Face");
    }

    #[test]
    fn groups_text_joins_names_and_says_none_for_an_empty_list() {
        assert_eq!(groups_text(None), "—");
        assert_eq!(groups_text(Some(&[])), "None");
        assert_eq!(groups_text(Some(&names(&["Administrators", "Users"]))), "Administrators, Users");
    }

    #[test]
    fn size_text_says_at_least_when_a_folder_was_skipped() {
        let gib = 1u64 << 30;
        assert_eq!(size_text(&ProfileSize::Calculating), "Calculating…");
        assert_eq!(size_text(&ProfileSize::Done(DirSize { bytes: gib, skipped: 0 })), "1.0 GB");
        assert_eq!(size_text(&ProfileSize::Done(DirSize { bytes: gib, skipped: 2 })), "At least 1.0 GB");
        assert_eq!(size_text(&ProfileSize::Unknown), "—");
    }

    #[test]
    fn state_text_names_active_and_disconnected_only() {
        assert_eq!(state_text(SessionState::Active), "Active");
        assert_eq!(state_text(SessionState::Disconnected), "Disconnected");
        assert_eq!(state_text(SessionState::Other), "—");
    }

    #[test]
    fn account_type_text_compares_the_local_group_name_ignoring_case() {
        let groups = names(&["administrators", "Users"]);
        assert_eq!(account_type_text(None, Some("Administrators")), "—");
        assert_eq!(account_type_text(Some(&groups), None), "—");
        assert_eq!(account_type_text(Some(&groups), Some("Administrators")), "Administrator");
        assert_eq!(account_type_text(Some(&names(&["Users"])), Some("Administrators")), "Standard");
    }

    #[test]
    fn initial_is_the_first_letter_or_digit_in_capitals() {
        for (name, want) in [("alex", "A"), ("  éva", "É"), ("", "?"), ("_x", "X")] {
            assert_eq!(initial(name), want, "{name:?}");
        }
    }

    #[test]
    fn you_view_prefers_the_full_name_and_the_email() {
        let mut acc = account();
        acc.full_name = Some("Alex Doe".into());
        acc.email = Some("alex@example.com".into());
        let v = you_view(&read(acc, vec![]), &[], &ProfileSize::Unknown, false);
        assert_eq!((v.name.as_str(), v.subtitle.as_str(), v.initial.as_str()), ("Alex Doe", "alex@example.com", "A"));
    }

    #[test]
    fn you_view_falls_back_to_the_account_name() {
        let v = you_view(&read(account(), vec![]), &[], &ProfileSize::Unknown, false);
        assert_eq!((v.name.as_str(), v.subtitle.as_str()), ("alex", r"PC\alex"));
    }

    #[test]
    fn you_view_says_administrator_only_for_an_admin_member() {
        let mut r = read(account(), vec![]);
        assert_eq!(you_view(&r, &[], &ProfileSize::Unknown, false).account_type, "Standard");
        r.admin_member = true;
        assert_eq!(you_view(&r, &[], &ProfileSize::Unknown, false).account_type, "Administrator");
    }

    #[test]
    fn you_view_gives_the_sign_in_time_of_the_current_session() {
        let sessions = [session(1, "alex", SessionState::Active, Some(10 * MIN), None)];
        let v = you_view(&read(account(), sessions.to_vec()), &sessions, &ProfileSize::Unknown, false);
        assert_eq!(v.signed_in, "Signed in for 10 min");
    }

    #[test]
    fn you_view_shows_a_dash_when_the_current_session_is_not_listed() {
        let sessions = [session(5, "other", SessionState::Active, Some(MIN), None)];
        let v = you_view(&read(account(), sessions.to_vec()), &sessions, &ProfileSize::Unknown, false);
        assert_eq!(v.signed_in, "—");
    }

    #[test]
    fn you_view_names_the_place_and_whether_negotium_is_admin() {
        let mut r = read(account(), vec![]);
        let local = you_view(&r, &[], &ProfileSize::Unknown, false);
        assert_eq!((local.place.as_str(), local.negotium_admin.as_str()), ("This PC", "No"));
        r.remote = true;
        let remote = you_view(&r, &[], &ProfileSize::Unknown, true);
        assert_eq!((remote.place.as_str(), remote.negotium_admin.as_str()), ("Remote", "Yes"));
    }

    #[test]
    fn you_view_shows_dashes_for_what_windows_refused() {
        let v = you_view(&read(account(), vec![]), &[], &ProfileSize::Unknown, false);
        assert_eq!(
            (v.profile_path.as_str(), v.profile_size.as_str(), v.sign_in.as_str(), v.password_set.as_str(), v.groups.as_str()),
            ("—", "—", "—", "—", "—")
        );
        assert_eq!(v.picture, None);
    }

    #[test]
    fn you_view_formats_the_account_facts() {
        let mut acc = account();
        acc.profile = Some(PathBuf::from(r"C:\Users\alex"));
        acc.password_age_secs = Some(0);
        acc.groups = Some(names(&["Users"]));
        acc.sign_in = Some(SignIn { pin: true, face: false, fingerprint: false });
        acc.picture = Some(PathBuf::from(r"C:\pic.jpg"));
        let v = you_view(&read(acc, vec![]), &[], &ProfileSize::Calculating, false);
        assert_eq!(v.profile_path, r"C:\Users\alex");
        assert_eq!((v.profile_size.as_str(), v.password_set.as_str()), ("Calculating…", "Today"));
        assert_eq!((v.groups.as_str(), v.sign_in.as_str()), ("Users", "Password, PIN"));
        assert_eq!(v.picture, Some(PathBuf::from(r"C:\pic.jpg")));
    }

    #[test]
    fn other_rows_skip_the_current_session_and_sort_by_name_ignoring_case() {
        let sessions = [
            session(7, "bob", SessionState::Disconnected, None, Some(125)),
            session(1, "alex", SessionState::Active, None, None),
            session(3, "Adam", SessionState::Active, None, None),
        ];
        let mut r = read(account(), sessions.to_vec());
        r.groups.insert(r"pc\bob".into(), Some(names(&["Administrators"])));
        r.groups.insert(r"pc\adam".into(), Some(names(&["Users"])));
        let rows = other_rows(&r, &sessions);
        let got: Vec<_> = rows.iter().map(|o| (o.id, o.name.as_str(), o.account_type.as_str(), o.state.as_str(), o.idle.as_str())).collect();
        assert_eq!(got, [(3, "Adam", "Standard", "Active", "—"), (7, "bob", "Administrator", "Disconnected", "2 min")]);
    }

    #[test]
    fn other_rows_show_a_dash_for_the_type_when_the_groups_are_unknown() {
        let sessions = [session(7, "bob", SessionState::Active, None, None)];
        let mut r = read(account(), sessions.to_vec());
        assert_eq!(other_rows(&r, &sessions)[0].account_type, "—");
        r.groups.insert(r"pc\bob".into(), None);
        assert_eq!(other_rows(&r, &sessions)[0].account_type, "—");
    }

    #[test]
    fn action_from_maps_the_six_names_and_refuses_the_rest() {
        assert_eq!(action_from("lock", -1), Some(UserAction::Lock));
        assert_eq!(action_from("switch-user", -1), Some(UserAction::SwitchUser));
        assert_eq!(action_from("sign-out", -1), Some(UserAction::SignOut));
        assert_eq!(action_from("account-settings", -1), Some(UserAction::AccountSettings));
        assert_eq!(action_from("sign-out-session", 3), Some(UserAction::SignOutSession(3)));
        assert_eq!(action_from("disconnect", 3), Some(UserAction::Disconnect(3)));
        assert_eq!(action_from("sign-out-session", -1), None);
        assert_eq!(action_from("disconnect", -1), None);
        assert_eq!(action_from("x", 3), None);
    }

    #[test]
    fn plan_runs_lock_switch_user_and_account_settings_at_once() {
        for elevated in [false, true] {
            for a in [UserAction::Lock, UserAction::SwitchUser, UserAction::AccountSettings] {
                assert_eq!(plan_user_action(a, "alex", elevated), UserPlan::Run, "{a:?} elevated {elevated}");
            }
        }
    }

    #[test]
    fn plan_asks_before_signing_out_whoever_runs_it() {
        let want = UserPlan::Confirm {
            title: "Sign out?".into(),
            body: "Windows closes all your apps. Save your work before you sign out.".into(),
            label: "Sign out".into(),
        };
        for elevated in [false, true] {
            assert_eq!(plan_user_action(UserAction::SignOut, "alex", elevated), want, "elevated {elevated}");
        }
    }

    #[test]
    fn plan_refuses_other_sessions_without_admin() {
        let want = UserPlan::NeedsAdmin("To sign out or disconnect another user, restart Negotium as admin.".into());
        assert_eq!(plan_user_action(UserAction::SignOutSession(3), "alex", false), want);
        assert_eq!(plan_user_action(UserAction::Disconnect(3), "alex", false), want);
    }

    #[test]
    fn plan_confirms_signing_out_another_user_with_admin() {
        let want = UserPlan::Confirm {
            title: "Sign out alex?".into(),
            body: "Windows closes all apps of alex. Work that alex did not save is lost.".into(),
            label: "Sign out".into(),
        };
        assert_eq!(plan_user_action(UserAction::SignOutSession(3), "alex", true), want);
    }

    #[test]
    fn plan_confirms_disconnecting_another_user_with_admin() {
        let want = UserPlan::Confirm {
            title: "Disconnect alex?".into(),
            body: "The apps of alex continue to run. alex can sign in again to continue.".into(),
            label: "Disconnect".into(),
        };
        assert_eq!(plan_user_action(UserAction::Disconnect(3), "alex", true), want);
    }

    #[test]
    fn failure_text_names_the_action_and_the_reason() {
        let e = ActionError::AccessDenied;
        for (a, want) in [
            (UserAction::Lock, "Couldn't lock the PC: access denied."),
            (UserAction::SwitchUser, "Couldn't switch user: access denied."),
            (UserAction::SignOut, "Couldn't sign out: access denied."),
            (UserAction::AccountSettings, "Couldn't open account settings: access denied."),
            (UserAction::SignOutSession(3), "Couldn't sign out alex: access denied."),
            (UserAction::Disconnect(3), "Couldn't disconnect alex: access denied."),
        ] {
            assert_eq!(failure_text(a, "alex", &e), want);
        }
    }

    #[test]
    fn profile_size_is_due_when_unknown_or_older_than_ten_minutes_and_not_running() {
        let now = Instant::now() + Duration::from_secs(3_600);
        let ago = |m: u64| Some(now - Duration::from_secs(m * 60));
        assert!(profile_size_due(None, false, now));
        assert!(!profile_size_due(None, true, now));
        assert!(!profile_size_due(ago(9), false, now));
        assert!(profile_size_due(ago(10), false, now));
        assert!(!profile_size_due(ago(11), true, now));
    }

    #[test]
    fn read_job_lists_the_current_session_and_the_account() {
        let read = read_job();
        let sessions = read.sessions.expect("sessions");
        let id = read.current.expect("current session");
        assert!(sessions.iter().any(|s| s.id == id), "session {id} in {sessions:?}");
        assert!(read.account.name.is_some() && read.admin_group.is_some());
    }
}
