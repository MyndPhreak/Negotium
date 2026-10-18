//! `negotium.exe --integrate <action>`: the one command that changes how Windows starts Negotium
//! (the installer's custom actions and the Settings switches run it, elevated where Windows needs
//! it). It turns the Ctrl+Shift+Esc redirect (the Image File Execution Options `Debugger` value
//! for taskmgr.exe), admin mode (a per-user Task Scheduler task with highest privileges) and Start
//! with Windows (the HKCU Run value) on and off, and removes all of them on uninstall.
//!
//! Every effect goes through a [`Machine`] (the registry and Task Scheduler in the app, an
//! in-memory fake in tests), and [`run`] refuses unless it runs as the installed exe: nothing it
//! writes ever names anything but `<Program Files>\Negotium\negotium.exe`.
use crate::install;
use negotium_actions::ActionError;
use negotium_actions::integration::{self, Roots, TaskStatus};
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Path, PathBuf};

/// The flag that selects the integration command instead of the window.
pub const FLAG: &str = "--integrate";
/// `admin-on` only: the task also starts at the user's sign-in (Start with Windows in admin mode).
pub const LOGON: &str = "--logon";
/// The argument the Run value (and a logon start) passes: start hidden in the tray.
pub const TRAY: &str = "--tray";
/// The admin task's first argument: this start came from the task (already elevated). A start
/// with arguments passes them after it; a logon start may pass the literal
/// `integration::ARG_PLACEHOLDER` instead, which means none.
pub const FROM_TASK: &str = "--from-task";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    HotkeyOn,
    HotkeyOff,
    AdminOn { logon: bool },
    AdminOff,
    StartupOn,
    StartupOff,
    OpenTaskManager,
    Uninstall,
}

impl Action {
    /// Every action, for tests and help text.
    pub const ALL: [Action; 9] = [
        Action::HotkeyOn,
        Action::HotkeyOff,
        Action::AdminOn { logon: false },
        Action::AdminOn { logon: true },
        Action::AdminOff,
        Action::StartupOn,
        Action::StartupOff,
        Action::OpenTaskManager,
        Action::Uninstall,
    ];

    fn name(self) -> &'static str {
        match self {
            Action::HotkeyOn => "hotkey-on",
            Action::HotkeyOff => "hotkey-off",
            Action::AdminOn { .. } => "admin-on",
            Action::AdminOff => "admin-off",
            Action::StartupOn => "startup-on",
            Action::StartupOff => "startup-off",
            Action::OpenTaskManager => "open-task-manager",
            Action::Uninstall => "uninstall",
        }
    }

    /// The action in `args` (the command line after the program name): exactly
    /// `--integrate <action>`, or `--integrate admin-on --logon`. Anything else (an unknown
    /// action, a stray argument, `--logon` on another action) is `None`: an elevated command
    /// does only what it was asked, spelled exactly.
    pub fn parse<I, S>(args: I) -> Option<Action>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let args: Vec<S> = args.into_iter().collect();
        let args: Vec<&str> = args.iter().map(|a| a.as_ref().to_str()).collect::<Option<_>>()?;
        match args.as_slice() {
            [FLAG, "admin-on", LOGON] => Some(Action::AdminOn { logon: true }),
            [FLAG, name] => Action::ALL
                .into_iter()
                .find(|a| a.name() == *name && *a != Action::AdminOn { logon: true }),
            _ => None,
        }
    }

    /// The arguments that run this action (`parse` reads them back).
    pub fn to_args(&self) -> Vec<String> {
        let mut args = vec![FLAG.to_string(), self.name().to_string()];
        if let Action::AdminOn { logon: true } = self {
            args.push(LOGON.to_string());
        }
        args
    }
}

/// The command line (after the program name) without what Windows appends when the redirect
/// starts Negotium: the original Task Manager command line (`"C:\Windows\System32\Taskmgr.exe"
/// /4`) follows Negotium's own arguments (none, from the redirect). Everything from an argument
/// naming taskmgr.exe (any case, any path, with or without `.exe`) on is dropped, so such a start
/// is a normal launch.
pub fn own_args<I, S>(args: I) -> Vec<OsString>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    args.into_iter().map(|a| a.as_ref().to_os_string()).take_while(|a| !names_task_manager(a)).collect()
}

/// Whether `arg` is a path (or bare name) of taskmgr.exe.
fn names_task_manager(arg: &OsStr) -> bool {
    let arg = arg.to_string_lossy();
    let arg = arg.trim().trim_matches('"');
    let name = arg.rsplit(['\\', '/']).next().unwrap_or_default();
    name.eq_ignore_ascii_case("taskmgr.exe") || name.eq_ignore_ascii_case("taskmgr")
}

/// Whether the command line (after the program name) asks for the integration command at all;
/// `main` then never shows a window, even when the action doesn't parse.
pub fn requested<I, S>(args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    args.into_iter().any(|a| a.as_ref() == OsStr::new(FLAG))
}

/// The admin task's state, as Task Scheduler reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TaskState {
    Missing,
    Ready,
    Disabled,
    /// Registered but unusable (wrong action, failed last start, unreadable), with the reason for
    /// the log and Settings' "needs repair".
    Broken(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IntegrateError {
    /// Not running as the installed exe (or it isn't there): nothing is changed.
    NotInstalled(PathBuf),
    /// The user's SID is missing or malformed, so no task name can be formed.
    BadSid(String),
    /// Not available yet (in this build, or this part of the command).
    NotImplemented(&'static str),
    /// The registry or Task Scheduler refused.
    Failed(String),
}

impl fmt::Display for IntegrateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IntegrateError::NotInstalled(exe) => write!(
                f,
                "only the installed Negotium can change Windows integration (this is {})",
                exe.display()
            ),
            IntegrateError::BadSid(sid) => write!(f, "not a user SID: {sid:?}"),
            IntegrateError::NotImplemented(what) => write!(f, "{what} is not available in this build"),
            IntegrateError::Failed(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for IntegrateError {}

/// The system state the command changes. Reads report absent as `None` / `Missing`.
pub trait Machine {
    /// Sets (`Some`) or removes (`None`) taskmgr.exe's IFEO `Debugger` value. The value written
    /// is the exe in double quotes (`integration::ifeo_value`): Windows runs `<value> <original
    /// command line>` with no application name, so an unquoted Program Files path would not run.
    fn set_ifeo(&self, exe: Option<&Path>) -> Result<(), IntegrateError>;
    /// The exe taskmgr.exe's IFEO `Debugger` value names, if any, unquoted and without arguments
    /// (`integration::parse_ifeo`), so hotkey-off and uninstall recognise the value `set_ifeo`
    /// wrote as the installed exe.
    fn ifeo(&self) -> Option<PathBuf>;
    /// Sets (`Some`, a whole command line) or removes (`None`) the HKCU Run value `Negotium`.
    fn set_run(&self, command: Option<&str>) -> Result<(), IntegrateError>;
    /// The HKCU Run value `Negotium`, if any.
    fn run_value(&self) -> Option<String>;
    /// Creates (or replaces) user `sid`'s admin task in `\Negotium\`: highest privileges, action
    /// `exe` with [`FROM_TASK`], and with `logon` a trigger at that user's sign-in.
    fn create_task(&self, sid: &str, exe: &Path, logon: bool) -> Result<(), IntegrateError>;
    /// Deletes user `sid`'s admin task (absent is fine).
    fn delete_task(&self, sid: &str) -> Result<(), IntegrateError>;
    fn task_state(&self, sid: &str) -> TaskState;
    /// Deletes the `\Negotium\` folder with every user's task in it (absent is fine).
    fn delete_task_folder(&self) -> Result<(), IntegrateError>;
    /// Starts user `sid`'s admin task with `args` (after [`FROM_TASK`]).
    fn start_task(&self, sid: &str, args: &[String]) -> Result<(), IntegrateError>;
}

/// What the command runs as and against.
pub struct Ctx<'a> {
    /// The running exe; must be the installed one.
    pub exe: PathBuf,
    /// The machine's Program Files (`install::program_files`); `None` refuses everything.
    pub program_files: Option<PathBuf>,
    /// The user the admin task is for (its name), e.g. `S-1-5-21-…-1001`.
    pub user_sid: String,
    pub machine: &'a dyn Machine,
    /// Starts Windows' own Task Manager and returns once its process exists (open-task-manager);
    /// [`start_task_manager`] in the app.
    pub start_task_manager: &'a dyn Fn() -> Result<(), IntegrateError>,
}

/// The installed exe, when `ctx.exe` is it and it exists; this path (not `ctx.exe`'s spelling)
/// is the only one ever written.
fn installed_exe(ctx: &Ctx) -> Result<PathBuf, IntegrateError> {
    let refuse = || IntegrateError::NotInstalled(ctx.exe.clone());
    let program_files = ctx.program_files.as_deref().ok_or_else(refuse)?;
    let exe = program_files.join("Negotium").join("negotium.exe");
    if install::detect(&ctx.exe, program_files).installed && exe.is_file() { Ok(exe) } else { Err(refuse()) }
}

/// A string SID (`S-1-<authority>-<sub>…`): digits between dashes only, so it is safe as a task
/// name (no path separators or wildcards).
fn user_sid(ctx: &Ctx) -> Result<String, IntegrateError> {
    let sid = ctx.user_sid.as_str();
    if integration::is_account_sid(sid) { Ok(sid.to_string()) } else { Err(IntegrateError::BadSid(sid.to_string())) }
}

/// The Run value: the installed exe, quoted (Program Files has a space), started in the tray.
pub fn run_command(exe: &Path) -> String {
    format!("\"{}\" {TRAY}", exe.display())
}

pub(crate) fn same_path(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| p.to_string_lossy().replace('/', "\\").trim_end_matches('\\').to_lowercase();
    norm(a) == norm(b)
}

/// Removes the redirect unless another tool's (Process Explorer's) is set, which is left alone.
/// Removing an absent value is still asked of the machine, so a failure to reach it is reported.
fn clear_redirect(machine: &dyn Machine, exe: &Path) -> Result<(), IntegrateError> {
    match machine.ifeo() {
        Some(current) if !same_path(&current, exe) => Ok(()),
        _ => machine.set_ifeo(None),
    }
}

/// Whether taskmgr.exe's redirect is set to `exe` (the Settings switch and the tray's "Open
/// Windows Task Manager" show this).
pub fn redirect_is(machine: &dyn Machine, exe: &Path) -> bool {
    machine.ifeo().is_some_and(|current| same_path(&current, exe))
}

/// The redirect lifted for a moment: it is put back when this is dropped, on every path out of
/// [`open_task_manager`] (an error, a panic in the starter); [`Lifted::restore`] reports how that
/// went.
struct Lifted<'a> {
    machine: &'a dyn Machine,
    exe: &'a Path,
    restored: bool,
}

impl Lifted<'_> {
    fn restore(mut self) -> Result<(), IntegrateError> {
        self.restored = true;
        self.machine.set_ifeo(Some(self.exe))
    }
}

impl Drop for Lifted<'_> {
    fn drop(&mut self) {
        if !self.restored {
            let _ = self.machine.set_ifeo(Some(self.exe));
        }
    }
}

/// Starts Windows' own Task Manager. With Negotium's redirect set, the redirect is lifted first
/// (Task Manager would otherwise open Negotium) and put back once Task Manager's process exists,
/// whether or not it started. Another tool's redirect, or none, is left alone.
fn open_task_manager(
    machine: &dyn Machine,
    exe: &Path,
    start: &dyn Fn() -> Result<(), IntegrateError>,
) -> Result<(), IntegrateError> {
    if !redirect_is(machine, exe) {
        return start();
    }
    // Armed before the value is touched: even a failed removal puts our value back.
    let lifted = Lifted { machine, exe, restored: false };
    machine.set_ifeo(None)?;
    let started = start();
    let restored = lifted.restore();
    match (started, restored) {
        (Ok(()), restored) => restored,
        (Err(e), Ok(())) => Err(e),
        (Err(e), Err(r)) => Err(IntegrateError::Failed(format!("{e}; and the redirect could not be restored: {r}"))),
    }
}

/// Runs `action` as the installed exe (refusing otherwise, before any change).
pub fn run(action: Action, ctx: &Ctx) -> Result<(), IntegrateError> {
    let exe = installed_exe(ctx)?;
    let machine = ctx.machine;
    match action {
        Action::HotkeyOn => machine.set_ifeo(Some(&exe)),
        Action::HotkeyOff => clear_redirect(machine, &exe),
        // HKCU's Run value belongs to the user running this; a service account (SYSTEM, from an
        // installer step) has no sign-in to start at, so it is refused like admin mode.
        Action::StartupOn => {
            user_sid(ctx)?;
            machine.set_run(Some(&run_command(&exe)))
        }
        Action::StartupOff => {
            user_sid(ctx)?;
            machine.set_run(None)
        }
        Action::AdminOn { logon } => machine.create_task(&user_sid(ctx)?, &exe, logon),
        Action::AdminOff => machine.delete_task(&user_sid(ctx)?),
        Action::OpenTaskManager => open_task_manager(machine, &exe, ctx.start_task_manager),
        // Every step is attempted; the first failure is reported.
        Action::Uninstall => [clear_redirect(machine, &exe), machine.set_run(None), machine.delete_task_folder()]
            .into_iter()
            .collect(),
    }
}

/// The whole command for `main`: parse `args` (after the program name), run against `machine` as
/// the running exe, log a failure. Returns the process exit code: 0 done, 1 failed, 2 not a
/// valid `--integrate` command line.
pub fn command<I, S>(args: I, machine: &dyn Machine, user_sid: String, log: &mut dyn FnMut(&str)) -> i32
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let args: Vec<S> = args.into_iter().collect();
    let Some(action) = Action::parse(&args) else {
        let shown: Vec<String> = args.iter().map(|a| a.as_ref().to_string_lossy().into_owned()).collect();
        log(&format!("integrate: not a valid command line: {}", shown.join(" ")));
        return 2;
    };
    let ctx = Ctx {
        exe: std::env::current_exe().unwrap_or_default(),
        program_files: install::program_files(),
        user_sid,
        machine,
        start_task_manager: &start_task_manager,
    };
    match run(action, &ctx) {
        Ok(()) => 0,
        Err(e) => {
            log(&format!("integrate {}: {e}", action.name()));
            1
        }
    }
}

/// Windows' own Task Manager, `<System32>\Taskmgr.exe`, from the shell's known folder (not
/// `%SystemRoot%`, which the user can point anywhere: an elevated open-task-manager must never
/// start a file the user picked).
pub fn task_manager_path() -> Option<PathBuf> {
    use std::ffi::c_void;
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{FOLDERID_System, KNOWN_FOLDER_FLAG, SHGetKnownFolderPath};
    // SAFETY: returns a CoTaskMem string that is freed below.
    let p = unsafe { SHGetKnownFolderPath(&FOLDERID_System, KNOWN_FOLDER_FLAG(0), None) }.ok()?;
    // SAFETY: `p` is a valid NUL-terminated string until freed.
    let s = unsafe { p.to_string() }.ok();
    // SAFETY: allocated by SHGetKnownFolderPath; freed exactly once.
    unsafe { CoTaskMemFree(Some(p.0 as *const c_void)) };
    s.filter(|s| !s.is_empty()).map(|s| PathBuf::from(s).join("Taskmgr.exe"))
}

/// The real starter for open-task-manager (run elevated, so Task Manager's "highest available"
/// manifest needs no prompt): CreateProcess returns once the process exists, having consulted the
/// (lifted) redirect, so the redirect can be restored as soon as this returns. The process is
/// left running. Never called by tests.
pub fn start_task_manager() -> Result<(), IntegrateError> {
    let path = task_manager_path().ok_or_else(|| IntegrateError::Failed("System32 could not be found".into()))?;
    std::process::Command::new(&path)
        .spawn()
        .map(drop)
        .map_err(|e| IntegrateError::Failed(format!("{} could not be started: {e}", path.display())))
}

/// The real machine: the registry (through its write guard) and Task Scheduler, over
/// `negotium_actions::integration`. `main` unlocks the guard first; nothing but the two values
/// and the task folder is ever written.
pub struct System {
    roots: Roots,
}

impl System {
    /// taskmgr.exe's redirect, HKCU's Run key and the `\Negotium\` task folder.
    pub fn new() -> Self {
        Self::with_roots(Roots::system())
    }

    /// The same machine over other roots (tests: a sandbox key, a test image, a test folder).
    pub fn with_roots(roots: Roots) -> Self {
        Self { roots }
    }
}

impl Default for System {
    fn default() -> Self {
        Self::new()
    }
}

fn failed(e: ActionError) -> IntegrateError {
    IntegrateError::Failed(e.to_string())
}

impl Machine for System {
    fn set_ifeo(&self, exe: Option<&Path>) -> Result<(), IntegrateError> {
        integration::set_ifeo(&self.roots, exe).map_err(failed)
    }
    fn ifeo(&self) -> Option<PathBuf> {
        integration::read_ifeo(&self.roots)
    }
    fn set_run(&self, command: Option<&str>) -> Result<(), IntegrateError> {
        integration::set_run(&self.roots, command).map_err(failed)
    }
    fn run_value(&self) -> Option<String> {
        integration::read_run(&self.roots)
    }
    fn create_task(&self, sid: &str, exe: &Path, logon: bool) -> Result<(), IntegrateError> {
        integration::create_task(&self.roots, sid, exe, FROM_TASK, logon).map_err(failed)
    }
    fn delete_task(&self, sid: &str) -> Result<(), IntegrateError> {
        integration::delete_task(&self.roots, sid).map_err(failed)
    }
    fn task_state(&self, sid: &str) -> TaskState {
        task_state(integration::task_state(&self.roots, sid))
    }
    fn delete_task_folder(&self) -> Result<(), IntegrateError> {
        integration::delete_task_folder(&self.roots).map_err(failed)
    }
    fn start_task(&self, sid: &str, args: &[String]) -> Result<(), IntegrateError> {
        integration::start_task(&self.roots, sid, args).map_err(failed)
    }
}

fn task_state(status: TaskStatus) -> TaskState {
    match status {
        TaskStatus::Missing => TaskState::Missing,
        TaskStatus::Ready => TaskState::Ready,
        TaskStatus::Disabled => TaskState::Disabled,
        TaskStatus::Broken(why) => TaskState::Broken(why),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// An in-memory machine: the IFEO value, the Run value and the `\Negotium\` task folder.
    #[derive(Default)]
    struct Fake {
        ifeo: RefCell<Option<PathBuf>>,
        run: RefCell<Option<String>>,
        /// sid -> (exe, logon trigger)
        tasks: RefCell<BTreeMap<String, (PathBuf, bool)>>,
        /// Every mutating call, in order.
        calls: RefCell<Vec<String>>,
        /// Mutating calls whose name is listed fail.
        failing: Vec<&'static str>,
    }

    impl Fake {
        fn call(&self, name: &'static str) -> Result<(), IntegrateError> {
            self.calls.borrow_mut().push(name.to_string());
            if self.failing.contains(&name) { Err(IntegrateError::Failed(format!("{name} failed"))) } else { Ok(()) }
        }
    }

    impl Machine for Fake {
        fn set_ifeo(&self, exe: Option<&Path>) -> Result<(), IntegrateError> {
            self.call("set_ifeo")?;
            *self.ifeo.borrow_mut() = exe.map(Path::to_path_buf);
            Ok(())
        }
        fn ifeo(&self) -> Option<PathBuf> {
            self.ifeo.borrow().clone()
        }
        fn set_run(&self, command: Option<&str>) -> Result<(), IntegrateError> {
            self.call("set_run")?;
            *self.run.borrow_mut() = command.map(str::to_string);
            Ok(())
        }
        fn run_value(&self) -> Option<String> {
            self.run.borrow().clone()
        }
        fn create_task(&self, sid: &str, exe: &Path, logon: bool) -> Result<(), IntegrateError> {
            self.call("create_task")?;
            self.tasks.borrow_mut().insert(sid.to_string(), (exe.to_path_buf(), logon));
            Ok(())
        }
        fn delete_task(&self, sid: &str) -> Result<(), IntegrateError> {
            self.call("delete_task")?;
            self.tasks.borrow_mut().remove(sid);
            Ok(())
        }
        fn task_state(&self, sid: &str) -> TaskState {
            if self.tasks.borrow().contains_key(sid) { TaskState::Ready } else { TaskState::Missing }
        }
        fn delete_task_folder(&self) -> Result<(), IntegrateError> {
            self.call("delete_task_folder")?;
            self.tasks.borrow_mut().clear();
            Ok(())
        }
        fn start_task(&self, _: &str, _: &[String]) -> Result<(), IntegrateError> {
            self.call("start_task")
        }
    }

    const SID: &str = "S-1-5-21-1004336348-1177238915-682003330-1001";
    const OTHER_SID: &str = "S-1-5-21-1004336348-1177238915-682003330-1002";

    /// A throwaway "Program Files" under the temp dir with `Negotium\negotium.exe` in it (an
    /// empty file); removed on drop.
    struct Root(PathBuf);

    impl Root {
        fn new() -> Root {
            static N: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "negotium-integrate-test-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::SeqCst)
            ));
            std::fs::create_dir_all(dir.join("Negotium")).unwrap();
            std::fs::write(dir.join("Negotium").join("negotium.exe"), b"").unwrap();
            Root(dir)
        }
        fn exe(&self) -> PathBuf {
            self.0.join("Negotium").join("negotium.exe")
        }
        fn ctx<'a>(&self, machine: &'a dyn Machine) -> Ctx<'a> {
            Ctx {
                exe: self.exe(),
                program_files: Some(self.0.clone()),
                user_sid: SID.to_string(),
                machine,
                start_task_manager: &never_started,
            }
        }
    }

    fn never_started() -> Result<(), IntegrateError> {
        panic!("this test never starts Task Manager")
    }

    impl Drop for Root {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn args_round_trip() {
        for action in Action::ALL {
            let args = action.to_args();
            assert_eq!(args[0], FLAG);
            assert!(requested(&args), "{args:?}");
            assert_eq!(Action::parse(&args), Some(action), "{args:?}");
        }
        assert_eq!(Action::AdminOn { logon: true }.to_args(), ["--integrate", "admin-on", "--logon"]);
        assert_eq!(Action::HotkeyOn.to_args(), ["--integrate", "hotkey-on"]);
        for bad in [
            &[][..],
            &["--integrate"],
            &["--integrate", "bogus"],
            &["--integrate", "HOTKEY-ON"],
            &["--integrate", "hotkey-on", "--logon"],
            &["--integrate", "admin-on", "--logon", "--logon"],
            &["--integrate", "uninstall", "extra"],
            &["hotkey-on", "--integrate"],
            &["--tray", "--integrate", "hotkey-on"],
        ] {
            assert_eq!(Action::parse(bad), None, "{bad:?}");
        }
        assert!(!requested(["--tray"]));
        assert!(requested(["--integrate", "bogus"]));
    }

    #[test]
    fn hotkey_on_and_off_set_and_clear_the_redirect() {
        let root = Root::new();
        let fake = Fake::default();
        run(Action::HotkeyOn, &root.ctx(&fake)).unwrap();
        assert_eq!(fake.ifeo(), Some(root.exe()));
        run(Action::HotkeyOff, &root.ctx(&fake)).unwrap();
        assert_eq!(fake.ifeo(), None);
        // Off with nothing set is still fine.
        run(Action::HotkeyOff, &root.ctx(&fake)).unwrap();
        assert_eq!(fake.ifeo(), None);
    }

    #[test]
    fn hotkey_off_leaves_another_tools_redirect() {
        let root = Root::new();
        let fake = Fake::default();
        let procexp = PathBuf::from(r"C:\Tools\procexp64.exe");
        *fake.ifeo.borrow_mut() = Some(procexp.clone());
        run(Action::HotkeyOff, &root.ctx(&fake)).unwrap();
        run(Action::Uninstall, &root.ctx(&fake)).unwrap();
        assert_eq!(fake.ifeo(), Some(procexp));
        assert!(!fake.calls.borrow().contains(&"set_ifeo".to_string()));
        // Ours is recognised however it is spelled.
        *fake.ifeo.borrow_mut() = Some(PathBuf::from(root.exe().to_string_lossy().to_uppercase()));
        run(Action::HotkeyOff, &root.ctx(&fake)).unwrap();
        assert_eq!(fake.ifeo(), None);
    }

    #[test]
    fn startup_on_and_off_set_and_clear_the_run_value() {
        let root = Root::new();
        let fake = Fake::default();
        run(Action::StartupOn, &root.ctx(&fake)).unwrap();
        assert_eq!(fake.run_value(), Some(format!("\"{}\" --tray", root.exe().display())));
        run(Action::StartupOff, &root.ctx(&fake)).unwrap();
        assert_eq!(fake.run_value(), None);
    }

    #[test]
    fn admin_on_creates_the_users_task_with_or_without_logon_and_off_deletes_it() {
        let root = Root::new();
        let fake = Fake::default();
        run(Action::AdminOn { logon: false }, &root.ctx(&fake)).unwrap();
        assert_eq!(fake.tasks.borrow().get(SID), Some(&(root.exe(), false)));
        assert_eq!(fake.task_state(SID), TaskState::Ready);
        run(Action::AdminOn { logon: true }, &root.ctx(&fake)).unwrap();
        assert_eq!(fake.tasks.borrow().get(SID), Some(&(root.exe(), true)));
        // Another user's task is untouched by this user's off.
        fake.tasks.borrow_mut().insert(OTHER_SID.into(), (root.exe(), false));
        run(Action::AdminOff, &root.ctx(&fake)).unwrap();
        assert_eq!(fake.task_state(SID), TaskState::Missing);
        assert_eq!(fake.task_state(OTHER_SID), TaskState::Ready);
    }

    #[test]
    fn admin_mode_refuses_a_malformed_sid() {
        let root = Root::new();
        let fake = Fake::default();
        for sid in ["", "S-1-", "S-1-5", r"S-1-5-21\..\x", "S-1-5-*", "s-1-5-21-1", "S-1-5--21", "S-1-5-21-"] {
            let ctx = Ctx { user_sid: sid.to_string(), ..root.ctx(&fake) };
            for action in [Action::AdminOn { logon: true }, Action::AdminOff] {
                assert_eq!(run(action, &ctx), Err(IntegrateError::BadSid(sid.to_string())), "{sid:?}");
            }
        }
        assert!(fake.calls.borrow().is_empty());
    }

    /// An installer step running as SYSTEM (or another service account) fails loudly: no task
    /// `\Negotium\S-1-5-18` and no Run value in SYSTEM's HKCU. Uninstall still cleans up.
    #[test]
    fn admin_mode_and_startup_refuse_service_and_well_known_sids() {
        let root = Root::new();
        let fake = Fake::default();
        let refused = [
            Action::AdminOn { logon: false },
            Action::AdminOn { logon: true },
            Action::AdminOff,
            Action::StartupOn,
            Action::StartupOff,
        ];
        for sid in ["S-1-5-18", "S-1-5-19", "S-1-5-20", "S-1-5-32-544", "S-1-1-0", "S-1-5-21-1-2-3"] {
            let ctx = Ctx { user_sid: sid.to_string(), ..root.ctx(&fake) };
            for action in refused {
                assert_eq!(run(action, &ctx), Err(IntegrateError::BadSid(sid.to_string())), "{action:?} {sid}");
            }
        }
        assert!(fake.calls.borrow().is_empty(), "{:?}", fake.calls.borrow());
        let ctx = Ctx { user_sid: "S-1-5-18".into(), ..root.ctx(&fake) };
        run(Action::HotkeyOn, &ctx).unwrap();
        run(Action::Uninstall, &ctx).unwrap();
        // An Entra ID account is a user.
        let ctx = Ctx { user_sid: "S-1-12-1-1885673120-1143434361-2093374368-2546219385".into(), ..root.ctx(&fake) };
        run(Action::AdminOn { logon: false }, &ctx).unwrap();
        run(Action::StartupOn, &ctx).unwrap();
    }

    /// Started by the redirect, Negotium sees `"C:\Windows\System32\Taskmgr.exe" /4` after its own
    /// (empty) arguments: dropped, however the path is spelled.
    #[test]
    fn the_taskmgr_argument_is_ignored() {
        for args in [
            &[r"C:\Windows\System32\Taskmgr.exe", "/4"][..],
            &[r"C:\WINDOWS\system32\taskmgr.exe"],
            &[r#""C:\Windows\System32\Taskmgr.exe""#, "/7", "/startup"],
            &["taskmgr.exe", "/0"],
            &["TASKMGR"],
            &[r"\\?\C:\Windows\System32\Taskmgr.exe"],
            &["C:/Windows/System32/taskmgr.EXE", "--integrate", "hotkey-off"],
            &[r"C:\Windows\SysWOW64\Taskmgr.exe"],
        ] {
            assert!(own_args(args).is_empty(), "{args:?}");
            assert!(!requested(own_args(args)), "{args:?}");
        }
        // Negotium's own arguments before it are kept; anything else is untouched.
        assert_eq!(own_args(["--tray", r"C:\Windows\System32\Taskmgr.exe", "/4"]), ["--tray"]);
        assert_eq!(own_args(["--page=startup", "--restarted"]), ["--page=startup", "--restarted"]);
        assert_eq!(own_args(["--integrate", "hotkey-on"]), ["--integrate", "hotkey-on"]);
        for kept in ["taskmgr.exe2", "mytaskmgr.exe", r"C:\taskmgr.exe\other.exe", "--page=taskmgr.exe"] {
            assert_eq!(own_args([kept]), [kept], "{kept}");
        }
        assert!(own_args(Vec::<OsString>::new()).is_empty());
    }

    #[test]
    fn opening_task_manager_lifts_the_redirect_while_it_starts() {
        let root = Root::new();
        let fake = Fake::default();
        run(Action::HotkeyOn, &root.ctx(&fake)).unwrap();
        let seen = RefCell::new(Vec::new());
        let start = || {
            seen.borrow_mut().push(fake.ifeo());
            Ok(())
        };
        run(Action::OpenTaskManager, &Ctx { start_task_manager: &start, ..root.ctx(&fake) }).unwrap();
        assert_eq!(*seen.borrow(), [None], "Task Manager starts with no redirect");
        assert_eq!(fake.ifeo(), Some(root.exe()), "and it is back");
        assert_eq!(*fake.calls.borrow(), ["set_ifeo", "set_ifeo", "set_ifeo"]);
    }

    #[test]
    fn opening_task_manager_restores_the_redirect_even_on_error() {
        let root = Root::new();
        let fake = Fake::default();
        *fake.ifeo.borrow_mut() = Some(root.exe());
        let start = || {
            assert_eq!(fake.ifeo(), None, "lifted while it starts");
            Err(IntegrateError::Failed("Taskmgr.exe could not be started".into()))
        };
        let ctx = Ctx { start_task_manager: &start, ..root.ctx(&fake) };
        assert_eq!(run(Action::OpenTaskManager, &ctx), Err(IntegrateError::Failed("Taskmgr.exe could not be started".into())));
        assert_eq!(fake.ifeo(), Some(root.exe()));
        // A starter that panics still leaves the redirect in place.
        let panics = || -> Result<(), IntegrateError> { panic!("starter panicked") };
        let ctx = Ctx { start_task_manager: &panics, ..root.ctx(&fake) };
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(Action::OpenTaskManager, &ctx))).is_err());
        assert_eq!(fake.ifeo(), Some(root.exe()));
    }

    #[test]
    fn opening_task_manager_does_not_start_it_while_the_redirect_stays() {
        let root = Root::new();
        // Every write fails: the value can't be lifted, so Task Manager isn't started (it would
        // open Negotium); the restore is still tried and the failure is reported.
        let fake = Fake { failing: vec!["set_ifeo"], ..Fake::default() };
        *fake.ifeo.borrow_mut() = Some(root.exe());
        let started = std::cell::Cell::new(false);
        let start = || {
            started.set(true);
            Ok(())
        };
        let ctx = Ctx { start_task_manager: &start, ..root.ctx(&fake) };
        assert_eq!(run(Action::OpenTaskManager, &ctx), Err(IntegrateError::Failed("set_ifeo failed".into())));
        assert!(!started.get());
        assert_eq!(*fake.calls.borrow(), ["set_ifeo", "set_ifeo"], "the removal, then the restore");
    }

    #[test]
    fn opening_task_manager_leaves_another_redirect_or_none_alone() {
        let root = Root::new();
        for current in [None, Some(PathBuf::from(r"C:\Tools\procexp64.exe"))] {
            let fake = Fake::default();
            *fake.ifeo.borrow_mut() = current.clone();
            let started = std::cell::Cell::new(0);
            let start = || {
                started.set(started.get() + 1);
                Ok(())
            };
            run(Action::OpenTaskManager, &Ctx { start_task_manager: &start, ..root.ctx(&fake) }).unwrap();
            assert_eq!(started.get(), 1);
            assert_eq!(fake.ifeo(), current);
            assert!(fake.calls.borrow().is_empty(), "{:?}", fake.calls.borrow());
        }
    }

    #[test]
    fn the_redirect_is_recognised_however_it_is_spelled() {
        let root = Root::new();
        let fake = Fake::default();
        assert!(!redirect_is(&fake, &root.exe()));
        *fake.ifeo.borrow_mut() = Some(PathBuf::from(root.exe().to_string_lossy().to_uppercase()));
        assert!(redirect_is(&fake, &root.exe()));
        *fake.ifeo.borrow_mut() = Some(PathBuf::from(r"C:\Tools\procexp64.exe"));
        assert!(!redirect_is(&fake, &root.exe()));
    }

    #[test]
    fn task_manager_is_in_system32() {
        let path = task_manager_path().unwrap();
        assert!(path.is_absolute(), "{}", path.display());
        assert!(path.to_string_lossy().to_lowercase().ends_with(r"\system32\taskmgr.exe"), "{}", path.display());
    }

    #[test]
    fn the_redirect_only_ever_names_the_installed_exe() {
        let root = Root::new();
        let fake = Fake::default();
        let build = PathBuf::from(r"C:\Users\me\DEV\Negotium\target\release\negotium.exe");
        let missing = Root::new();
        std::fs::remove_file(missing.exe()).unwrap();
        let refused = [
            // A build tree, a lookalike folder, another file, no known folder, a missing exe.
            Ctx { exe: build.clone(), ..root.ctx(&fake) },
            Ctx { exe: root.0.join("Negotium2").join("negotium.exe"), ..root.ctx(&fake) },
            Ctx { exe: root.0.join("Negotium").join("other.exe"), ..root.ctx(&fake) },
            Ctx { program_files: None, ..root.ctx(&fake) },
            missing.ctx(&fake),
            // Program Files pointed at the build tree's parent still needs the exe to be there.
            Ctx { exe: build.clone(), program_files: Some(PathBuf::from(r"C:\Users\me\DEV")), ..root.ctx(&fake) },
        ];
        for ctx in &refused {
            for action in Action::ALL {
                assert_eq!(run(action, ctx), Err(IntegrateError::NotInstalled(ctx.exe.clone())), "{action:?} {}", ctx.exe.display());
            }
        }
        assert!(fake.calls.borrow().is_empty(), "{:?}", fake.calls.borrow());
        // Run as the installed exe under any spelling, it writes the canonical installed path.
        let shouted = PathBuf::from(root.exe().to_string_lossy().to_uppercase());
        run(Action::HotkeyOn, &Ctx { exe: shouted, ..root.ctx(&fake) }).unwrap();
        assert_eq!(fake.ifeo(), Some(root.exe()));
    }

    #[test]
    fn uninstall_removes_redirect_run_value_and_every_task() {
        let root = Root::new();
        let fake = Fake::default();
        for action in [Action::HotkeyOn, Action::StartupOn, Action::AdminOn { logon: true }] {
            run(action, &root.ctx(&fake)).unwrap();
        }
        fake.tasks.borrow_mut().insert(OTHER_SID.into(), (root.exe(), false));
        run(Action::Uninstall, &root.ctx(&fake)).unwrap();
        assert_eq!(fake.ifeo(), None);
        assert_eq!(fake.run_value(), None);
        assert!(fake.tasks.borrow().is_empty());
        assert_eq!(fake.task_state(SID), TaskState::Missing);
        assert_eq!(fake.task_state(OTHER_SID), TaskState::Missing);
    }

    #[test]
    fn uninstall_attempts_every_step_and_reports_the_first_failure() {
        let root = Root::new();
        let fake = Fake { failing: vec!["set_ifeo"], ..Fake::default() };
        *fake.ifeo.borrow_mut() = Some(root.exe());
        *fake.run.borrow_mut() = Some("x".into());
        fake.tasks.borrow_mut().insert(SID.into(), (root.exe(), false));
        assert_eq!(run(Action::Uninstall, &root.ctx(&fake)), Err(IntegrateError::Failed("set_ifeo failed".into())));
        assert_eq!(*fake.calls.borrow(), ["set_ifeo", "set_run", "delete_task_folder"]);
        assert_eq!(fake.run_value(), None);
        assert!(fake.tasks.borrow().is_empty());
    }

    #[test]
    fn machine_failures_are_reported() {
        let root = Root::new();
        let fake = Fake { failing: vec!["set_run", "create_task"], ..Fake::default() };
        assert!(matches!(run(Action::StartupOn, &root.ctx(&fake)), Err(IntegrateError::Failed(_))));
        assert!(matches!(run(Action::AdminOn { logon: false }, &root.ctx(&fake)), Err(IntegrateError::Failed(_))));
    }

    #[test]
    fn the_command_exits_non_zero_with_a_log_line() {
        let mut lines = Vec::new();
        // An invalid command line: 2, and nothing runs.
        assert_eq!(command(["--integrate", "bogus"], &Fake::default(), SID.into(), &mut |l| lines.push(l.to_string())), 2);
        // The test binary is not the installed exe: refused, 1.
        let fake = Fake::default();
        assert_eq!(command(["--integrate", "hotkey-on"], &fake, SID.into(), &mut |l| lines.push(l.to_string())), 1);
        assert!(fake.calls.borrow().is_empty());
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].contains("bogus"), "{lines:?}");
        assert!(lines[1].starts_with("integrate hotkey-on: only the installed Negotium"), "{lines:?}");
    }

    /// The real machine over a sandbox: its IFEO and Run keys under HKCU\Software\Negotium-Test
    /// (the guard's sandbox), so `run` goes through the registry for real without touching
    /// taskmgr.exe's redirect or the real Run value. Task Scheduler isn't reached.
    #[test]
    fn the_real_machine_writes_the_quoted_redirect_and_the_run_value() {
        use negotium_actions::startup::registry::{self, Hive, RegKey, TEST_ROOT};
        let base = RegKey::new(Hive::CurrentUser, &format!(r"{TEST_ROOT}\integrate-{}", std::process::id()));
        let _ = registry::delete_test_tree(&base);
        let roots = Roots {
            ifeo: base.child("taskmgr.exe"),
            run: base.child("Run"),
            task_folder: format!("Negotium-test-{}", std::process::id()),
        };
        let machine = System::with_roots(roots.clone());
        let root = Root::new();
        let exe = root.exe();
        run(Action::HotkeyOn, &root.ctx(&machine)).unwrap();
        let raw = registry::value(&roots.ifeo, "Debugger").and_then(|v| v.as_str().map(str::to_string));
        assert_eq!(raw, Some(format!("\"{}\"", exe.display())), "quoted");
        assert_eq!(machine.ifeo(), Some(exe.clone()), "read back unquoted");
        run(Action::HotkeyOff, &root.ctx(&machine)).unwrap();
        assert_eq!(machine.ifeo(), None, "hotkey-off recognised its own value");
        run(Action::StartupOn, &root.ctx(&machine)).unwrap();
        assert_eq!(machine.run_value(), Some(run_command(&exe)));
        run(Action::StartupOff, &root.ctx(&machine)).unwrap();
        assert_eq!(machine.run_value(), None);
        registry::delete_test_tree(&base).unwrap();
    }

    #[test]
    fn the_real_machine_is_the_systems_roots() {
        assert_eq!(System::new().roots, Roots::system());
        assert_eq!(Roots::system().task_folder, "Negotium");
    }

    #[test]
    fn task_states_map_one_to_one() {
        for (status, state) in [
            (TaskStatus::Missing, TaskState::Missing),
            (TaskStatus::Ready, TaskState::Ready),
            (TaskStatus::Disabled, TaskState::Disabled),
            (TaskStatus::Broken("why".into()), TaskState::Broken("why".into())),
        ] {
            assert_eq!(task_state(status), state);
        }
    }

    #[test]
    fn this_users_sid_names_a_task() {
        let sid = integration::current_user_sid().unwrap();
        let root = Root::new();
        let fake = Fake::default();
        let ctx = Ctx { user_sid: sid.clone(), ..root.ctx(&fake) };
        run(Action::AdminOn { logon: false }, &ctx).unwrap();
        assert!(fake.tasks.borrow().contains_key(&sid));
    }
}
