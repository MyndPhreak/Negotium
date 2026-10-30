plan-version: 1

# Users page Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:subagent-driven-development (or
> superpowers:executing-plans) to carry out this plan task by task. Steps use `- [ ]` checkboxes.

**Goal:** A new **Users** rail page: a card about your own account (picture, name, type, sign-in
time, place, profile folder and size, sign-in methods, password age, groups, whether Negotium runs
as admin) with Lock / Switch user / Sign out / Open account settings, and a list of the other
signed-in sessions with Sign out / Disconnect.

**Architecture:** The Services pattern, not the worker: a background job (`users::read_job`) reads
sessions and account facts on page show and every 5 s while the page is on screen; a second job
walks the profile folder. Windows calls live in `negotium_actions::users`; all text and decisions
are pure functions in `crates/app/src/users.rs`; the bridge keeps the last good state and pushes
it to `userspage.slint`.

**Tech Stack:** Rust 2024, `windows` 0.62.2 (adds `Win32_System_RemoteDesktop`,
`Win32_NetworkManagement_NetManagement`, `Win32_System_Shutdown` to `crates/actions/Cargo.toml`),
Slint 1.18.1, `i_slint_backend_testing`.

**Spec:** `docs/superpowers/specs/2026-10-28-users-details-settings-design.md` §3 and
"Constraints (all pieces)".

## Global constraints

- No feature flag (project rule): the page ships on.
- Red/green per task: `Add failing tests for <area>` (stubs compile, tests fail on an assertion or
  a named `expect`), then `Implement <area>`. Stubs return typed defaults, never `todo!()`.
- UI text is ASD-STE100. Any field Windows refuses shows `—`; no section hides because one field failed.
- Must fit the 760 × 440 minimum window (`main.slint:24-25`).
- Run everything from this worktree (its own `target/`); never rename a locked `negotium.exe`.
- Commands: `cargo test -p negotium-actions users`, `cargo test -p negotium`,
  `cargo clippy -p negotium -p negotium-actions --all-targets -- -D warnings`.

## Review focus

1. The rail now holds 8 buttons: `RailButton` height 56 → 48 px (`rail.slint:9`, comment at :51).
2. Admin-only actions on other sessions: disabled + banner when not elevated, and a defensive
   `NeedsAdmin` refusal in `plan_user_action`. Windows is the real boundary (WTS calls fail with
   access denied).
3. The profile walk never follows reparse points (`Application Data` junction loops).
4. No toast every 5 s: a failed session read is an inline note.

## Summary

Task Manager's Users tab tells you nothing about your own account. Negotium adds a Users page whose
top card describes the signed-in account and whose list shows every other signed-in session.
Readers use WTS for sessions, NetUserGetInfo / NetUserGetLocalGroups for account facts, the
registry for picture and sign-in methods, and `std::fs` for the profile size. Actions use
LockWorkStation, WTSDisconnectSession, ExitWindowsEx and WTSLogoffSession. Sign out of any session
and Disconnect of another session ask first in a dedicated confirm dialog.

## Architectural decisions (rulings; cost if wrong in brackets)

1. **Background job, no worker frame.** Nothing here needs the per-tick snapshot once per-user
   CPU/memory is dropped (ruling 2); `services.rs:83-130` is the pattern. [Low: a worker arm can be
   added later.]
2. **Per-user CPU and memory: dropped.** It needs a worker arm joining `ProcessSample.session_id`
   (`snapshot/src/lib.rs:12`) with model rows each tick, a third data path into the page. Most PCs
   have one user. Follow-up named. [Users who compare with Task Manager miss two columns.]
3. **Refresh:** read on page show and every `USERS_REFRESH` = 5 s while the page is on screen and
   the window is visible (one `slint::Timer`, one job at a time). Profile size: on page show when
   none is known or the last is older than `PROFILE_SIZE_MAX_AGE` = 10 min; the old value stays
   on screen while a new walk runs. [Disk churn if 10 min is too short; stale size if too long.]
4. **Sessions:** `WTSEnumerateSessionsW` + `WTSQuerySessionInformationW(WTSSessionInfo)`;
   `WTSINFOW` (UserName 21, Domain 17 units) fits SAM and NetBIOS limits. Sessions with no user
   (session 0, listeners) are skipped; a session whose query fails is skipped. Idle = `CurrentTime
   − LastInputTime`; Windows gives 0 for the console (captured: `quser` prints `none`), shown `—`.
   [Idle shows `—` for local fast-user-switch sessions.]
5. **Current session and place:** `ProcessIdToSessionId(GetCurrentProcessId())`;
   `GetSystemMetrics(SM_REMOTESESSION)` → "Remote" / "This PC".
6. **Account type:** you → `integration::is_admin_member()` (token, language-neutral, exists);
   others → `NetUserGetLocalGroups(LG_INCLUDE_INDIRECT)` contains the local name of
   `ADMINISTRATORS` (S-1-5-32-544) from `LookupAccountSidW`. Equivalence test ties both for the
   current user. [Misreport on a token read failure: `is_admin_member` says Standard.]
7. **Name, email, password:** `NetUserGetInfo` level 2 (`usri2_full_name`, `usri2_password_age`)
   and level 24 (`usri24_internet_principal_name` when `usri24_internet_identity`). Domain accounts
   fail both → name falls back to the account name, email and password `—`. Password age shows as
   "Today" / "1 day ago" / "N days ago" (no date formatting code). [Domain users see dashes.]
8. **Sign-in methods:** "Password" always; PIN = subkey `<SID>` under
   `HKLM\…\Authentication\Credential Providers\{D6886603-9D2F-4EB2-B667-1971041FA96B}`; face /
   fingerprint = `HKLM\…\WinBio\AccountInfo\<SID>` `EnrolledFactors` bits 0x2 / 0x8
   (WINBIO_TYPE_FACIAL_FEATURES / FINGERPRINT). Captured on the dev PC, unelevated: PIN subkey
   present, `EnrolledFactors` = 0x2. Missing key = not set; read error = `—`. [Passwordless MSA
   accounts still list Password.]
9. **Picture:** `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\AccountPicture\Users\<SID>` value
   `Image192`, else `Image96` (captured: JPG paths, readable unelevated). Loaded on the UI thread
   with `slint::Image::load_from_path`; failure → the name's initial. Live test proves JPEG decode.
10. **Profile:** `SHGetKnownFolderPath(FOLDERID_Profile)`; size = logical bytes from a `std::fs`
    walk that skips `FILE_ATTRIBUTE_REPARSE_POINT` entries; unreadable folders are counted and the
    text becomes "At least 12.3 GB".
11. **Actions:** Lock `LockWorkStation`; Switch user `WTSDisconnectSession(None,
    WTS_CURRENT_SESSION, false)` (what `tsdiscon` does); Sign out `ExitWindowsEx(EWX_LOGOFF)`, not
    forced, so apps can ask to save; other session `WTSLogoffSession` / `WTSDisconnectSession`
    (`bWait` false); Open account settings `shell_open("ms-settings:yourinfo")`
    (`elevation.rs:81`). [Switch user fails where fast user switching is off: toast says why.]
12. **Own confirm dialog.** The existing `Modal` instance is hard-wired to "End process"
    (`main.slint:660-666`) and its pending slot holds a `dispatch::Job` (`main.rs:1092`). A second
    `Modal` instance with title/body/label properties and `cancel-primary: true` leaves the
    process path untouched.
13. **Admin path:** not elevated → row buttons disabled and a non-dismissable `Banner`
    (`overlays.slint:9`) above the list wired to the existing `restart-admin`; `restart_args`
    (`controls.rs:69`) returns to `--page=users`.
14. **Rail:** Users = index 7, after Ports. Eight 56 px buttons need 463 px; the window gives 408
    (`rail.slint:51`), so `RailButton` becomes 48 px (399 px). [Smaller hit target.]
15. **Session read failure:** the list keeps the last good sessions and the page shows "Couldn't
    read the signed-in users: <reason>." until a read succeeds. A job panic gives "Couldn't read
    the users.".
16. **Token-name reuse:** the `LookupAccountSidW` part of `details.rs:85-119` moves into
    `pub(crate) fn sid_account(sid: PSID) -> Option<(String, String)>` (domain, name); `user()`
    and `users.rs` both call it. No new copy.

## Failure behaviour

| Dependency | Not configured | Failed to read | Direction |
|---|---|---|---|
| WTS enumerate | — | keep last list + note | degrade |
| WTS query (one session) | no user → skip | skip that session | degrade |
| NetUserGetInfo 2 / 24 | not MSA → no email | name = account name; email, password `—` | degrade |
| NetUserGetLocalGroups | — | Groups `—`; other row type `—` | degrade |
| Registry PIN / WinBio | key missing → not set | Sign-in methods `—` | degrade |
| Registry picture / image decode | missing → initial | initial | degrade |
| Profile path / walk | — | path and size `—`; per folder → "At least" | degrade |
| Lock / switch / sign out / logoff / disconnect | — | toast `failure_text` | fail loudly |

No writes, so nothing can be lost. Concurrency: one read job and one walk at a time (bridge
flags); a timer tick during a job does nothing.

## Invocation paths for the admin guard and confirms

Other-session Sign out / Disconnect: only `user-action(name, id)` from a row button (pointer and
keyboard both raise it) → `plan_user_action`. Own Sign out: the same callback with id −1. The
confirm OK runs the action held in `pending_user`; elevation cannot change inside a process. One
path, so no equivalence test.

## Files

| File | Change |
|---|---|
| `crates/actions/Cargo.toml` | 3 features (above) |
| `crates/actions/src/users.rs` | new: types, pure mappers, readers, actions, `dir_size`; tests |
| `crates/actions/src/lib.rs` | `pub mod users;` |
| `crates/actions/src/details.rs` | extract `sid_account` (ruling 16) |
| `crates/app/src/users.rs` | new: `UsersRead`, views, text fns, `UserAction`, plan, jobs; tests |
| `crates/app/src/format.rs` | `span(secs)`; test |
| `crates/app/src/lib.rs` | `pub mod users;` |
| `crates/app/src/controls.rs` | `Page::Users = 7` in enum, `ALL`, `from_index`, `name` ("users"); test |
| `crates/app/src/worker.rs` | `Page::Users` joins the `None` arm (:338); test |
| `crates/app/src/bridge.rs` | rest list (:517), apply arm (:538), users state and methods |
| `crates/app/src/main.rs` | `load_users`, timer, `show_page` arm, `user-action`, `user-confirm` |
| `crates/app/ui/userspage.slint` | new: `YouData`, `UserRowData`, `UsersPage` |
| `crates/app/ui/rail.slint` | Users button (page 7); `RailButton` 48 px |
| `crates/app/ui/main.slint` | users properties, page at `page == 7`, user confirm `Modal` |
| `crates/app/tests/ui.rs` | rail, page, bridge tests; :626 expects 48.0 |

## Contracts (compiling stubs land in each phase's first red commit)

No HTTP surface, no settings change; `--page=users` is the only new outside string.

```rust
// crates/actions/src/users.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum SessionState { Active, Disconnected, Other }
#[derive(Clone, Debug, PartialEq)] pub struct Session {
    pub id: u32, pub user: String, pub domain: String, pub state: SessionState,
    pub signed_in_secs: Option<u64>, pub idle_secs: Option<u64> }
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)] pub struct SignIn { pub pin: bool, pub face: bool, pub fingerprint: bool }
#[derive(Clone, Debug, Default, PartialEq)] pub struct Account {
    pub name: Option<String>, /* DOMAIN\user */ pub sid: Option<String>, pub full_name: Option<String>,
    pub email: Option<String>, pub password_age_secs: Option<u64>, pub groups: Option<Vec<String>>,
    pub picture: Option<PathBuf>, pub profile: Option<PathBuf>, pub sign_in: Option<SignIn> }
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)] pub struct DirSize { pub bytes: u64, pub skipped: u32 }
pub fn session_state(raw: i32) -> SessionState;
pub fn elapsed_secs(from_ft: i64, now_ft: i64) -> Option<u64>;      // FILETIME units; 0 or from > now → None
pub fn session_from_info(info: &WTSINFOW) -> Option<Session>;        // None when UserName is empty
pub fn sign_in_from(pin: Option<bool>, factors: Option<u32>) -> Option<SignIn>;
pub fn list_sessions() -> Result<Vec<Session>, ActionError>;
pub fn current_session_id() -> Option<u32>;
pub fn is_remote_session() -> bool;
pub fn current_account() -> Account;                                 // never fails as a whole
pub fn local_groups(account: &str) -> Option<Vec<String>>;
pub fn admin_group_name() -> Option<String>;
pub fn dir_size(path: &Path) -> DirSize;
pub fn lock() -> Result<(), ActionError>;            pub fn switch_user() -> Result<(), ActionError>;
pub fn sign_out() -> Result<(), ActionError>;
pub fn sign_out_session(id: u32) -> Result<(), ActionError>;
pub fn disconnect_session(id: u32) -> Result<(), ActionError>;
```

```rust
// crates/app/src/users.rs
pub const USERS_REFRESH: Duration = Duration::from_secs(5);
pub const PROFILE_SIZE_MAX_AGE: Duration = Duration::from_secs(600);
pub struct UsersRead { pub sessions: Result<Vec<Session>, String>, pub current: Option<u32>,
    pub remote: bool, pub account: Account, pub admin_member: bool, pub admin_group: Option<String>,
    pub groups: HashMap<String, Option<Vec<String>>> /* key: lower-case DOMAIN\user */ }
pub enum ProfileSize { Calculating, Done(DirSize), Unknown }
pub struct YouView { pub initial, name, subtitle, account_type, signed_in, place, profile_path,
    profile_size, sign_in, password_set, groups, negotium_admin: String, pub picture: Option<PathBuf> }
pub struct OtherRow { pub id: i32, pub name: String, pub account_type: String, pub state: String, pub idle: String }
pub enum UserAction { Lock, SwitchUser, SignOut, AccountSettings, SignOutSession(u32), Disconnect(u32) }
pub enum UserPlan { Run, Confirm { title: String, body: String, label: String }, NeedsAdmin(String) }
pub fn initial(name: &str) -> String;                 pub fn span_text(secs: Option<u64>) -> String;
pub fn password_age_text(secs: Option<u64>) -> String; pub fn sign_in_text(s: Option<SignIn>) -> String;
pub fn groups_text(g: Option<&[String]>) -> String;    pub fn size_text(s: &ProfileSize) -> String;
pub fn state_text(s: SessionState) -> &'static str;
pub fn account_type_text(groups: Option<&[String]>, admin_group: Option<&str>) -> &'static str;
pub fn you_view(read: &UsersRead, sessions: &[Session], size: &ProfileSize, elevated: bool) -> YouView;
pub fn other_rows(read: &UsersRead, sessions: &[Session]) -> Vec<OtherRow>;
pub fn action_from(name: &str, session: i32) -> Option<UserAction>;
pub fn plan_user_action(a: UserAction, user: &str, elevated: bool) -> UserPlan;
pub fn failure_text(a: UserAction, user: &str, e: &ActionError) -> String;
pub fn profile_size_due(last: Option<Instant>, running: bool, now: Instant) -> bool;
pub fn read_job() -> UsersRead;
pub fn run_user_action(a: UserAction, user: &str, owner: isize) -> Option<String>;
// crates/app/src/format.rs
pub fn span(secs: u64) -> String;
```

```slint
// crates/app/ui/userspage.slint
export struct YouData { initial: string, name: string, subtitle: string, account-type: string,
    signed-in: string, place: string, profile-path: string, profile-size: string, sign-in: string,
    password-set: string, groups: string, negotium-admin: string }
export struct UserRowData { id: int, name: string, account-type: string, state: string, idle: string }
export component UsersPage inherits VerticalLayout {
    in property <YouData> you; in property <image> picture; in property <bool> has-picture;
    in property <[UserRowData]> rows; in property <bool> need-admin; in property <string> read-note;
    in property <length> bottom-inset; in property <bool> interactive;
    callback act(string, int);   // "lock" | "switch-user" | "sign-out" | "account-settings" (−1) | "sign-out-session" | "disconnect" (id)
    callback restart-admin();
}
// main.slint: you, you-picture, you-has-picture, user-rows, users-need-admin, users-read-note,
// user-confirm-visible/-title/-text/-label, public function show-user-confirm(string, string, string),
// callbacks user-action(string, int), user-confirm(bool); modal-open includes user-confirm-visible.
```

Bridge (stubs in P2.T4 red): `set_users(&mut self, ui, read: UsersRead, elevated: bool)`,
`begin_users_read(&mut self) -> bool`, `begin_profile_size(&mut self, now: Instant) -> Option<PathBuf>`,
`set_profile_size(&mut self, ui, size: Option<DirSize>, now: Instant)`, `user_name(&self, id: i32) -> Option<String>`.

## Test Plan

`crates/actions/src/users.rs` (unit, pure)
- `session_state`: 0 → Active, 4 → Disconnected, 1, 2, 3, 5, 6, 7, 8, 9, −1 → Other (11 cases)
- `elapsed_secs(t, t + 1_200_000_000)` → Some(120); `(0, n)` → None; `(n, n − 1)` → None; `(n, n)` → Some(0)
- `session_from_info`: user "alex", domain "PC", state 4, LogonTime 10 min before CurrentTime,
  LastInputTime 0 → Session { user "alex", domain "PC", Disconnected, Some(600), idle None }
- `session_from_info` with empty UserName → None
- `sign_in_from`: (Some(true), Some(0x2)) → pin+face; (Some(false), Some(0x8)) → fingerprint;
  (Some(false), Some(0x0A)) → face+fingerprint; (Some(false), Some(0)) → none set;
  (None, Some(0)) → None; (Some(true), None) → None
- `dir_size`: temp dir with 3 files (10, 20, 30 bytes) in two levels → 60 bytes, 0 skipped
- `dir_size`: a junction (`cmd /C mklink /J`) to a dir holding 1000 bytes is not followed → total excludes it
- `dir_size`: a subfolder denied with `icacls /deny <user>:(RX)` → skipped 1, other bytes counted (ACL restored after)
- `dir_size` of a missing path → `DirSize { 0, 1 }`

`crates/actions/src/users.rs` (live, unelevated, not ignored)
- `list_sessions` contains `current_session_id()` with the user of `current_account().name`, state Active
- `current_account()`: `name` ends with `\` + `%USERNAME%`; `sid` = `current_user_sid()`; `profile` exists as a dir
- `admin_group_name()` is Some and non-empty
- equivalence: `is_admin_member()` == `local_groups(name).contains(admin_group_name())`
- `sign_out_session(65_000)` and `disconnect_session(65_000)` return Err (no such session), no panic

`crates/app/src/format.rs`: `span` 0 → "Less than 1 min", 59 → same, 60 → "1 min", 3_599 → "59 min",
3_600 → "1 h 0 min", 7_500 → "2 h 5 min", 86_400 → "1 d 0 h", 280_800 → "3 d 6 h"

`crates/app/src/users.rs` (unit)
- `span_text(None)` → "—"; `password_age_text`: None "—", 0 "Today", 86_399 "Today", 86_400 "1 day ago", 48_211_200 "558 days ago"
- `sign_in_text`: None "—"; none set "Password"; pin+face+fingerprint "Password, PIN, Face, Fingerprint"
- `groups_text`: None "—"; `[]` "None"; `["Administrators","Users"]` "Administrators, Users"
- `size_text`: Calculating "Calculating…"; Done(1 GiB, 0) "1.0 GB"; Done(1 GiB, 2) "At least 1.0 GB"; Unknown "—"
- `state_text`: Active "Active", Disconnected "Disconnected", Other "—"
- `account_type_text`: (None, _) "—"; (_, None) "—"; groups hold "administrators" vs admin "Administrators" → "Administrator"; no match → "Standard"
- `initial`: "alex" "A"; "  éva" "É"; "" "?"; "_x" "X"
- `you_view`: full name "Alex Doe" + email → name "Alex Doe", subtitle the email; no full name, no email → name "alex", subtitle "PC\alex"; admin_member → "Administrator" else "Standard"; current session 600 s → "Signed in for 10 min"; current id missing from sessions → signed_in "—"; remote → "Remote" else "This PC"; elevated → negotium_admin "Yes" else "No"; profile None → path "—"
- `other_rows`: excludes the current session; sorted by name case-insensitively; id = session id; idle None → "—"; type from `groups` map (missing key → "—")
- `action_from`: the 6 names with valid ids map; "sign-out-session" / "disconnect" with −1 → None; "x" → None
- `plan_user_action` — all 12 (6 actions × elevated):
  Lock, SwitchUser, AccountSettings → Run (6); SignOut → Confirm{"Sign out?", "Windows closes all your apps. Save your work before you sign out.", "Sign out"} (2);
  SignOutSession not elevated / Disconnect not elevated → NeedsAdmin("To sign out or disconnect another user, restart Negotium as admin.") (2);
  SignOutSession elevated → Confirm{"Sign out alex?", "Windows closes all apps of alex. Work that alex did not save is lost.", "Sign out"};
  Disconnect elevated → Confirm{"Disconnect alex?", "The apps of alex continue to run. alex can sign in again to continue.", "Disconnect"}
- `failure_text` with AccessDenied: Lock "Couldn't lock the PC: access denied."; SwitchUser "Couldn't switch user: …"; SignOut "Couldn't sign out: …"; AccountSettings "Couldn't open account settings: …"; SignOutSession "Couldn't sign out alex: …"; Disconnect "Couldn't disconnect alex: …"
- `profile_size_due`: (None, false) true; (None, true) false; (now − 9 min, false) false; (now − 10 min, false) true; (now − 11 min, true) false

`crates/app/src/controls.rs`: `from_index(7)` Users; `Users.index()` 7; `name()` "users"; `from_name("Users")` Some(Users); `ALL.len()` 8; `restart_args(Users)` round-trips through `page_from_args`

`crates/app/src/worker.rs`: on `Page::Users` `next_frame` returns None (`no_frame_is_built_for_the_users_page`)

`crates/app/tests/ui.rs`
- rail: "Users page" activates `select-page(7)` and reads checked at page 7; at 760 × 440 the "Settings page" button's bottom ≤ 440; :626 `rail.size().height` == 48.0
- page (page 7, 760 × 1400): card texts shown (name, subtitle, "Administrator", "Signed in for 10 min", "This PC", each detail value)
- empty `user-rows` → text "No other users are signed in."; with rows → no such text, one row per entry
- "Lock", "Switch user", "Sign out", "Open account settings" report `act` ("lock", −1) … ("account-settings", −1)
- row buttons "Sign out alex" / "Disconnect alex" report ("sign-out-session", 3) / ("disconnect", 3)
- `need-admin` with rows: banner text and "Restart as admin" shown, button reports `restart-admin`; row buttons disabled; `need-admin` with no rows → no banner
- `read-note` non-empty → its text shows
- `show-user-confirm("Sign out?", body, "Sign out")`: title, body and label show; "Cancel" reports `user-confirm(false)`, "Sign out" reports `user-confirm(true)`; Return reports false (cancel is primary); `modal-open` true and page buttons disabled
- layout at 760 × 1400: the four card buttons and row buttons do not overlap each other and end ≤ 760 − 14
- bridge `set_users`: card and rows pushed; a second read with `sessions: Err("boom")` keeps the rows and sets note "Couldn't read the signed-in users: boom."; the next good read clears it
- bridge: picture path that does not exist → `you-has-picture` false; initial shows
- bridge profile size: first → "Calculating…"; `set_profile_size(Some(1 GiB, 0))` → "1.0 GB"; `begin_profile_size` within 10 min → None; at 10 min → Some(path) and the text stays "1.0 GB"; `set_profile_size(None)` → "—"
- bridge: `begin_users_read` true, then false until `set_users`; `user_name(3)` → Some("alex"); unknown id → None
- live: when `current_account().picture` is Some, `slint::Image::load_from_path` is Ok (prints a skip reason otherwise)

### Exemptions from test-first (what covers them)
- Lock, Switch user, own Sign out, real logoff/disconnect of another session: running them ends
  the test run's session. Covered by the pure `plan_user_action` / `failure_text` tests, the
  "no such session" live tests, and manual steps 3–6.
- `main.rs` wiring (timer, `show_page`, callbacks): wiring, as for Services. Manual steps 1, 6, 8.
- Visual look of the card: manual steps 1 and 7.
- A second signed-in user on a CI-less dev PC: manual step 6 (skipped and stated if the PC has one account).

### Manual verification
1. `./scripts/try.ps1`, run `try\negotium.exe`, open **Users**: picture, name, email or account,
   three chips, every detail; size shows "Calculating…", then a size.
2. Windows Settings → Accounts → Sign-in options lists the same methods.
3. **Lock** → lock screen. **Switch user** → sign-in screen; sign in again, Negotium still runs.
4. **Sign out** → dialog, Cancel is the default; Cancel does nothing. (Sign out for real once, work saved.)
5. **Open account settings** → Settings opens on Your info.
6. Sign in a second account via Switch user, come back: its row shows Active/Disconnected. Not
   elevated: banner and disabled buttons; **Restart as admin** reopens on Users. Elevated:
   Disconnect → confirm → state Disconnected within 5 s; Sign out → row gone within 5 s.
7. At 760 × 440: all 8 rail buttons show; card and list scroll, nothing clipped.
8. Leave and return within 10 min: the size shows at once, no "Calculating…".

## Out of scope
- Managing accounts (spec). Send message / Connect to a session (Task Manager items).
- Follow-ups: per-user CPU and memory (ruling 2); domain display name via `GetUserNameExW(NameDisplay)`
  (ruling 7); passwordless detection (ruling 8); size on disk vs logical size.

## Deviations from spec
- Idle time shows `—` for console sessions: Windows does not report it (ruling 4).
- "When the password was last set" shows as an age in days (ruling 7).

## Definition of done
- [ ] Rail shows **Users** after Ports; 8 buttons fit 760 × 440; `--page=users` reopens it — controls + rail ui tests, manual 7
- [ ] Card shows every field; a refused field shows `—` and the rest stay — `you_view`, text fn tests, manual 1
- [ ] Sign-in methods, picture and account type match Windows — sign-in/live/equivalence tests, manual 1–2
- [ ] Profile size computes off the UI thread, "Calculating…" first, junctions not followed — `dir_size`, bridge size tests, manual 8
- [ ] Lock, Switch user, Open account settings act at once; Sign out asks first — `plan_user_action` (12), ui confirm test, manual 3–5
- [ ] Other sessions listed with type, state, idle; "No other users are signed in." when alone — `other_rows`, ui empty test, manual 6
- [ ] Without admin, other-session buttons are disabled and the banner offers Restart as admin — ui need-admin test, `NeedsAdmin` cases, manual 6
- [ ] A failed session read keeps the list and shows a note, no toast — bridge note test
- [ ] Failed actions toast a reason — `failure_text` tests, live "no such session" tests
- [ ] `cargo test -p negotium-actions -p negotium` passes; clippy `-D warnings` clean

## Acceptance criteria
1. Opening Users shows the signed-in account's facts within one read; slow facts follow. → DoD 2, 3, 4
2. Each card action does what its label says; Sign out never runs without a confirm. → DoD 5
3. Other sessions appear, with a plain empty state. → DoD 6
4. Without admin rights the app says how to get them instead of failing. → DoD 7, 9
5. A Windows read failure never hides a section or floods toasts. → DoD 2, 8
6. The page fits the smallest window. → DoD 1

## Phase 1 — data (branch `feat/users-page`)

### P1.T1 | Users page index and rail | controls.rs, worker.rs, bridge.rs, rail.slint, main.slint, ui.rs:626 | controls, worker, rail tests | risk: rail height change
**Files:** above, plus every P1 stub: `crates/actions/src/users.rs`, `crates/app/src/users.rs`, `format::span`, Cargo features.
**Interfaces:** `Page::Users = 7`; contracts above (stubs return `Other`, `None`, `Default`, `Err(ActionError::Other("stub"))`, `String::new()`, `UserPlan::Run`, `false`).
- [ ] Red: add the stubs; add `Page::Users` with `name()` arm `""` and worker arm `Page::Users => Some(Frame { page: Page::Users, ..Default::default() })`; add the controls, worker and three rail tests; set :626 to 48.0.
- [ ] Run `cargo test -p negotium users rail` → Expected: `from_index(7)` assert fails (Overview), worker test fails (Some), rail tests fail on `expect("rail item")` and 56 ≠ 48.
- [ ] Commit `Add failing tests for the Users page index and rail`.
- [ ] Green: `ALL`, `from_index`, `name` "users"; worker None arm; bridge rest list and apply arm; `RailButton` 48 px, comment "Eight buttons…"; Users button (person icon path) after Ports; empty `UsersPage` at `page == 7`.
- [ ] Run `cargo test -p negotium` → Expected: all pass. Commit `Implement the Users page index and rail`.

### P1.T2 | Windows readers and actions | actions users.rs, details.rs, Cargo.toml | actions unit + live lines | risk: `icacls` and `mklink /J` in tests
- [ ] Red: add the actions unit and live tests. Run `cargo test -p negotium-actions users` → Expected: failures on `session_state`, `elapsed_secs`, `sign_in_from`, `dir_size` (0 ≠ 60), `list_sessions` (stub Err), `current_account().name` None. Commit `Add failing tests for the Users readers`.
- [ ] Green: implement per rulings 4–11 and 16; free WTS / Net buffers on every path; `// SAFETY:` on each unsafe block. Run the same → all pass; then `cargo test -p negotium-actions` → no regression. Commit `Implement the Users readers`.

### P1.T3 | Users text and decisions | app users.rs, format.rs | format + app users unit lines | none
- [ ] Red: add the `format::span` and `users.rs` unit tests. Run `cargo test -p negotium users::` → Expected: assertion failures (stubs return "" / Run / false). Commit `Add failing tests for the Users text and decisions`.
- [ ] Green: implement; `read_job` calls the readers and fills `groups` for each other session's `DOMAIN\user`; `run_user_action` maps the action to its reader call and `failure_text`. Run → pass. Commit `Implement the Users text and decisions`.

## Phase 2 — page and wiring

### P2.T4 | Users page UI | userspage.slint, main.slint | ui page lines | risk: 760 px layout
**Interfaces:** Slint contract above; bridge method stubs (no-ops returning `false` / `None`).
- [ ] Red: `UsersPage` with properties and callbacks only; main.slint properties, callbacks, `show-user-confirm` and the second `Modal`; bridge stubs; all ui page and bridge tests. Run `cargo test -p negotium --test ui users` → Expected: page tests fail on missing texts and buttons; bridge tests fail on empty properties. Commit `Add failing tests for the Users page`.
- [ ] Green (layout): `RevealScrollView` with the card (avatar 64 px image or initial circle; name, subtitle; chip row; two-column details with `wrap: word-wrap` values; button row), the `Banner` when `need-admin && rows.length > 0`, `read-note`, the rows (two-line name/type cell, state, idle, two buttons with `accessible-label: "Sign out " + name` / `"Disconnect " + name`, `enabled: !need-admin && interactive`) or the empty text. Run the page tests → pass.

### P2.T5 | Bridge state, jobs and wiring | bridge.rs, users.rs, main.rs | ui bridge lines + live picture | none
- [ ] Green (bridge): `set_users` keeps last good sessions, sets the note, loads the picture only when its path changes, pushes `YouData` and `user-rows` via `sync_model`; size and read flags per the tests. Run `cargo test -p negotium` → all pass.
- [ ] Wiring: `users::spawn_read(weak, elevated)` and `users::spawn_profile_size(weak, path)` (catch_unwind, `invoke_from_event_loop`, mirroring `services::spawn_job`); `load_users` in `show_page` for `Page::Users` plus `begin_profile_size`; a `Rc<slint::Timer>` repeated `USERS_REFRESH` calling `load_users` when `controls.page() == Users && controls.visible()`; `on_user_action` → `action_from` → `user_name` → `plan_user_action` → run on a thread / `pending_user` + `show_user_confirm` / toast; `on_user_confirm`; a successful session action calls `load_users`.
- [ ] Run tests + clippy (Global constraints) → Expected: 0 failures, 0 warnings. Commit `Implement the Users page`. Then manual steps 1–8.

## Self-review
- Every task survives the ladder: `is_admin_member`, `current_user_sid`, `registry::{subkeys, values}`, `shell_open`, `format::bytes`, `Banner`, `Modal`, `RevealScrollView`, `sync_model` are reused (grepped); only `sid_account` is extracted.
- P2 has one red commit for T4+T5 (one Slint surface); T5's green follows T4's green. Deliberate.
- Left open: the Users rail icon path (builder draws a head-and-shoulders outline in the 20 × 20 box).
- Left open: whether the 56 → 48 px rail change needs the user's eye; manual step 7 checks it.
- The `EnrolledFactors` bit meaning rests on one capture (0x2 on a PC with Face); manual step 2 confirms.
