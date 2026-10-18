//! `negotium.exe --integrate <action>`: the one command that changes how Windows starts Negotium
//! (the installer's custom actions and the Settings switches run it, elevated where Windows needs
//! it). It turns the Ctrl+Shift+Esc redirect (the Image File Execution Options `Debugger` value
//! for taskmgr.exe), admin mode (a per-user Task Scheduler task with highest privileges) and Start
//! with Windows (the HKCU Run value, or the admin task's logon trigger in admin mode) on and off,
//! and removes all of them on uninstall.
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
use std::time::Duration;

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
/// What [`hand_off`] passes the task for a launch with no arguments of its own: every start
/// Negotium asks of the task carries at least one argument, so a start with none is the logon
/// trigger's (Task Scheduler gives a trigger start no `$(Arg0)` value), which starts in the tray
/// ([`starts_in_tray`]). Otherwise ignored. Any start of the task with no arguments counts as
/// that, so "Run" on the task in Task Scheduler (or `schtasks /run`) also starts hidden in the
/// tray.
pub const SHOW: &str = "--show";

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
    /// Whether user `sid`'s task has the logon trigger (false when it is missing; `None` when it
    /// couldn't be read).
    fn task_logon(&self, sid: &str) -> Option<bool>;
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

/// Whether the Run value `value` starts `exe` (however the path is spelled; quoted, or unquoted
/// without spaces; with any arguments): Start with Windows is on in its Run value form.
pub fn run_value_starts(value: &str, exe: &Path) -> bool {
    let value = value.trim();
    let path = match value.strip_prefix('"') {
        Some(rest) => rest.split('"').next().unwrap_or_default(),
        None => value.split(' ').next().unwrap_or_default(),
    };
    !path.is_empty() && same_path(Path::new(path), exe)
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
///
/// Start with Windows has two forms. Without admin mode (no task) it is the Run value, in HKCU:
/// startup-on/off then touch nothing else and need no elevation. With admin mode (the user's task
/// exists, even one needing repair) it is the task's logon trigger, so sign-in starts the elevated
/// copy directly: startup-on recreates the task with the trigger, startup-off without it (both
/// need elevation), and the Run value is removed. Admin mode takes the startup with it: admin-on
/// removes the Run value (Settings and the installer pass `--logon` while Start with Windows is
/// on), admin-off sets it again when the task had the trigger.
pub fn run(action: Action, ctx: &Ctx) -> Result<(), IntegrateError> {
    let exe = installed_exe(ctx)?;
    let machine = ctx.machine;
    match action {
        Action::HotkeyOn => machine.set_ifeo(Some(&exe)),
        Action::HotkeyOff => clear_redirect(machine, &exe),
        // HKCU's Run value belongs to the user running this; a service account (SYSTEM, from an
        // installer step) has no sign-in to start at, so it is refused like admin mode.
        Action::StartupOn => {
            let sid = user_sid(ctx)?;
            if machine.task_state(&sid) == TaskState::Missing {
                machine.set_run(Some(&run_command(&exe)))
            } else {
                machine.create_task(&sid, &exe, true)?;
                machine.set_run(None)
            }
        }
        Action::StartupOff => {
            let sid = user_sid(ctx)?;
            // A trigger that couldn't be read may be there: recreated without, to be sure.
            if machine.task_state(&sid) != TaskState::Missing && machine.task_logon(&sid) != Some(false) {
                machine.create_task(&sid, &exe, false)?;
            }
            machine.set_run(None)
        }
        Action::AdminOn { logon } => {
            machine.create_task(&user_sid(ctx)?, &exe, logon)?;
            machine.set_run(None)
        }
        Action::AdminOff => {
            let sid = user_sid(ctx)?;
            let logon = machine.task_logon(&sid) == Some(true);
            machine.delete_task(&sid)?;
            if logon { machine.set_run(Some(&run_command(&exe))) } else { Ok(()) }
        }
        Action::OpenTaskManager => open_task_manager(machine, &exe, ctx.start_task_manager),
        // Every step is attempted; the first failure is reported.
        Action::Uninstall => [clear_redirect(machine, &exe), machine.set_run(None), machine.delete_task_folder()]
            .into_iter()
            .collect(),
    }
}

/// Start with Windows in its Run value form, for Settings without admin mode (run here, without
/// elevation): startup-on/off set or remove the Run value and nothing else; Task Scheduler is
/// never asked (a hung Schedule service can't hold it up). Every other action needs [`run`]
/// elevated and is refused.
pub fn run_unelevated(action: Action, ctx: &Ctx) -> Result<(), IntegrateError> {
    let exe = installed_exe(ctx)?;
    match action {
        Action::StartupOn => {
            user_sid(ctx)?;
            ctx.machine.set_run(Some(&run_command(&exe)))
        }
        Action::StartupOff => {
            user_sid(ctx)?;
            ctx.machine.set_run(None)
        }
        _ => Err(IntegrateError::NotImplemented("this action without elevation")),
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

/// Whether this launch starts hidden in the tray (Start with Windows): `--tray` (the Run value,
/// passed on by a hand-off too), or a start by the task with no arguments at all, which only its
/// logon trigger makes ([`SHOW`]). `args` is [`task_args`]' rest.
pub fn starts_in_tray(from_task: bool, args: &[OsString]) -> bool {
    args.iter().any(|a| a == TRAY) || (from_task && args.iter().all(|a| a.is_empty()))
}

/// What a launch does about admin mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Launch {
    /// Carry on in this process.
    RunHere,
    /// Start the user's admin task (an elevated copy) and exit.
    HandOff,
}

/// Whether this launch hands off to the admin task: only the installed copy, not elevated, for an
/// Administrators member whose task is ready, and never a start that came from the task itself
/// (`--from-task` without elevation means the task couldn't elevate: starting it again would
/// loop).
pub fn launch_plan(installed: bool, elevated: bool, admin_member: bool, task: TaskState, from_task: bool) -> Launch {
    if installed && !elevated && admin_member && task == TaskState::Ready && !from_task {
        Launch::HandOff
    } else {
        Launch::RunHere
    }
}

/// The command line (after [`own_args`]) split into whether the admin task started it and the
/// rest: a leading [`FROM_TASK`] is removed, and after it the literal
/// `integration::ARG_PLACEHOLDER` a logon start may leave (no arguments).
pub fn task_args(args: Vec<OsString>) -> (bool, Vec<OsString>) {
    match args.split_first() {
        Some((first, rest)) if first == FROM_TASK => {
            (true, rest.iter().filter(|a| *a != integration::ARG_PLACEHOLDER).cloned().collect())
        }
        _ => (false, args),
    }
}

/// How long a launch waits for Task Scheduler to report the admin task before it gives up and
/// runs non-elevated: a hung Schedule service must not hold up Ctrl+Shift+Esc.
pub const TASK_READ_BOUND: Duration = Duration::from_millis(750);

/// `read` on its own thread, waited for at most `bound`: `None` if it hasn't finished by then
/// (the thread is left to finish on its own; its result is dropped).
pub fn bounded<T: Send + 'static>(bound: Duration, read: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(read());
    });
    rx.recv_timeout(bound).ok()
}

/// What [`hand_off`] decides with.
pub struct LaunchCtx<'a> {
    /// This is the installed copy (`install::current`).
    pub installed: bool,
    pub elevated: bool,
    /// The user is in Administrators (`integration::is_admin_member`).
    pub admin_member: bool,
    /// [`task_args`] found [`FROM_TASK`].
    pub from_task: bool,
    /// This user's SID, read only when the task is consulted.
    pub user_sid: &'a dyn Fn() -> Option<String>,
    /// User `sid`'s task state, or `None` when Task Scheduler didn't answer in time
    /// ([`bounded`] by [`TASK_READ_BOUND`] in the app).
    pub read_task: &'a dyn Fn(&str) -> Option<TaskState>,
    /// Starts the task (`Machine::start_task`).
    pub machine: &'a dyn Machine,
    /// Shows an instance that is already running, if there is one (true: there was, and it was
    /// asked to show itself), so neither Task Scheduler is asked nor the task started.
    pub activate_running: &'a dyn Fn() -> bool,
    /// Lets the copy the task starts take the foreground (this launch has it).
    pub allow_foreground: &'a dyn Fn(),
}

/// Admin mode at launch, before the window and the single-instance lock (so the elevated copy
/// the task starts can claim it): an instance already running is shown, or, with
/// [`Launch::HandOff`], the task is started with `args` (the page and the rest of this command
/// line). Returns true when this process should exit. A task that is disabled, broken or slow to
/// read, or one that can't be started, is logged once and the launch runs here, non-elevated.
pub fn hand_off(ctx: &LaunchCtx, args: &[String], log: &mut dyn FnMut(&str)) -> bool {
    if ctx.from_task && !ctx.elevated {
        log("admin mode: the task started Negotium without admin rights; carrying on without them");
        return false;
    }
    // `launch_plan`'s conditions apart from the task, checked first: the task is read (a Task
    // Scheduler call) only when nothing else rules the hand-off out.
    if !ctx.installed || ctx.elevated || !ctx.admin_member || ctx.from_task {
        return false;
    }
    let Some(sid) = (ctx.user_sid)().filter(|sid| integration::is_account_sid(sid)) else {
        return false;
    };
    // The normal launch would show a running instance too; doing it first spares the read.
    if (ctx.activate_running)() {
        return true;
    }
    let Some(task) = (ctx.read_task)(&sid) else {
        log(&format!(
            "admin mode: Task Scheduler didn't report the task within {} ms; starting without admin rights",
            TASK_READ_BOUND.as_millis()
        ));
        return false;
    };
    match launch_plan(ctx.installed, ctx.elevated, ctx.admin_member, task.clone(), ctx.from_task) {
        Launch::RunHere => {
            match task {
                TaskState::Disabled => log("admin mode: the task is disabled; starting without admin rights"),
                TaskState::Broken(why) => {
                    log(&format!("admin mode: the task is broken ({why}); starting without admin rights"))
                }
                TaskState::Missing | TaskState::Ready => {}
            }
            false
        }
        Launch::HandOff => {
            (ctx.allow_foreground)();
            // Never an empty start: that is the logon trigger's, which starts in the tray.
            let shown = [SHOW.to_string()];
            let args = if args.is_empty() { &shown[..] } else { args };
            // Task Scheduler accepting the start is all this sees: if the elevated copy then
            // fails to run (its exe gone, a bad action), the task's last result records it and
            // the next launch reads the task as Broken and runs here, so this never loops.
            match ctx.machine.start_task(&sid, args) {
                Ok(()) => true,
                Err(e) => {
                    log(&format!("admin mode: the task couldn't be started ({e}); starting without admin rights"));
                    false
                }
            }
        }
    }
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

impl System {
    /// User `sid`'s task state and whether it has the logon trigger (`None`: not known), read in
    /// one Task Scheduler session (Settings).
    pub fn read_task(&self, sid: &str) -> (TaskState, Option<bool>) {
        let (status, logon) = integration::read_task(&self.roots, sid);
        (task_state(status), logon)
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
    fn task_logon(&self, sid: &str) -> Option<bool> {
        integration::task_logon(&self.roots, sid)
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
        /// What `task_state` reports instead of Ready/Missing from `tasks`.
        state: Option<TaskState>,
        /// Every `start_task`'s arguments.
        started: RefCell<Vec<Vec<String>>>,
        /// `task_logon` can't read the trigger.
        unreadable_trigger: bool,
        /// Task Scheduler hangs: `task_state` takes 10 s.
        hung: bool,
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
        fn task_logon(&self, sid: &str) -> Option<bool> {
            if self.unreadable_trigger {
                return None;
            }
            Some(self.tasks.borrow().get(sid).is_some_and(|(_, logon)| *logon))
        }
        fn task_state(&self, sid: &str) -> TaskState {
            if self.hung {
                std::thread::sleep(Duration::from_secs(10));
            }
            if let Some(state) = &self.state {
                return state.clone();
            }
            if self.tasks.borrow().contains_key(sid) { TaskState::Ready } else { TaskState::Missing }
        }
        fn delete_task_folder(&self) -> Result<(), IntegrateError> {
            self.call("delete_task_folder")?;
            self.tasks.borrow_mut().clear();
            Ok(())
        }
        fn start_task(&self, sid: &str, args: &[String]) -> Result<(), IntegrateError> {
            assert_eq!(sid, SID);
            self.started.borrow_mut().push(args.to_vec());
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

    /// Without admin mode Start with Windows is the Run value (HKCU: no UAC); with it, the task's
    /// logon trigger, and the Run value goes.
    #[test]
    fn startup_uses_the_run_value_without_admin_mode_and_the_task_with_it() {
        let root = Root::new();
        let fake = Fake::default();
        run(Action::StartupOn, &root.ctx(&fake)).unwrap();
        assert_eq!(fake.run_value(), Some(run_command(&root.exe())));
        assert!(fake.tasks.borrow().is_empty(), "no task without admin mode");
        assert_eq!(*fake.calls.borrow(), ["set_run"], "only HKCU is touched");
        run(Action::StartupOff, &root.ctx(&fake)).unwrap();
        assert_eq!(fake.run_value(), None);
        assert_eq!(*fake.calls.borrow(), ["set_run", "set_run"]);

        // Admin mode on: the task gets the logon trigger, and a stray Run value is removed.
        fake.tasks.borrow_mut().insert(SID.into(), (root.exe(), false));
        *fake.run.borrow_mut() = Some(run_command(&root.exe()));
        run(Action::StartupOn, &root.ctx(&fake)).unwrap();
        assert_eq!(fake.tasks.borrow().get(SID), Some(&(root.exe(), true)));
        assert_eq!(fake.task_logon(SID), Some(true));
        assert_eq!(fake.run_value(), None);
        run(Action::StartupOff, &root.ctx(&fake)).unwrap();
        assert_eq!(fake.tasks.borrow().get(SID), Some(&(root.exe(), false)), "admin mode stays, without the trigger");
        assert_eq!(fake.run_value(), None);
        // Off again with no trigger: the task is left as it is.
        fake.calls.borrow_mut().clear();
        run(Action::StartupOff, &root.ctx(&fake)).unwrap();
        assert_eq!(*fake.calls.borrow(), ["set_run"]);

        // A task that needs repair is admin mode too: recreated, with the trigger.
        let broken = Fake { state: Some(TaskState::Broken("its exe is gone".into())), ..Fake::default() };
        broken.tasks.borrow_mut().insert(SID.into(), (PathBuf::from(r"C:\gone.exe"), false));
        run(Action::StartupOn, &root.ctx(&broken)).unwrap();
        assert_eq!(broken.tasks.borrow().get(SID), Some(&(root.exe(), true)));
        assert_eq!(broken.run_value(), None);
    }

    /// A trigger that couldn't be read is never taken for "off" when it matters: startup-off
    /// still recreates the task without it; admin-off doesn't invent a Run value.
    #[test]
    fn an_unreadable_trigger_is_not_known() {
        let root = Root::new();
        let fake = Fake { unreadable_trigger: true, ..Fake::default() };
        fake.tasks.borrow_mut().insert(SID.into(), (root.exe(), true));
        run(Action::StartupOff, &root.ctx(&fake)).unwrap();
        assert_eq!(fake.tasks.borrow().get(SID), Some(&(root.exe(), false)));
        run(Action::AdminOff, &root.ctx(&fake)).unwrap();
        assert_eq!(fake.run_value(), None);
    }

    /// Settings' in-process startup-on/off (no admin mode) touch the Run value only: a hung Task
    /// Scheduler can't hold them up; anything else is refused there.
    #[test]
    fn startup_without_elevation_never_asks_task_scheduler() {
        let root = Root::new();
        let fake = Fake { hung: true, ..Fake::default() };
        let started = std::time::Instant::now();
        run_unelevated(Action::StartupOn, &root.ctx(&fake)).unwrap();
        assert_eq!(fake.run_value(), Some(run_command(&root.exe())));
        run_unelevated(Action::StartupOff, &root.ctx(&fake)).unwrap();
        assert_eq!(fake.run_value(), None);
        assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
        for action in Action::ALL.into_iter().filter(|a| !matches!(a, Action::StartupOn | Action::StartupOff)) {
            assert!(matches!(run_unelevated(action, &root.ctx(&fake)), Err(IntegrateError::NotImplemented(_))), "{action:?}");
        }
        assert_eq!(*fake.calls.borrow(), ["set_run", "set_run"]);
        // Still only the installed exe, and only a user's.
        let ctx = Ctx { user_sid: "S-1-5-18".into(), ..root.ctx(&fake) };
        assert!(matches!(run_unelevated(Action::StartupOn, &ctx), Err(IntegrateError::BadSid(_))));
        let ctx = Ctx { exe: PathBuf::from(r"C:\elsewhere
egotium.exe"), ..root.ctx(&fake) };
        assert!(matches!(run_unelevated(Action::StartupOn, &ctx), Err(IntegrateError::NotInstalled(_))));
    }

    /// Admin mode turned on or off while Start with Windows is on takes the startup with it.
    #[test]
    fn switching_admin_mode_moves_startup_between_forms() {
        let root = Root::new();
        let fake = Fake::default();
        run(Action::StartupOn, &root.ctx(&fake)).unwrap();
        // On (Settings passes --logon while Start with Windows is on): the trigger, no Run value.
        run(Action::AdminOn { logon: true }, &root.ctx(&fake)).unwrap();
        assert_eq!(fake.tasks.borrow().get(SID), Some(&(root.exe(), true)));
        assert_eq!(fake.run_value(), None);
        // Off: the Run value again.
        run(Action::AdminOff, &root.ctx(&fake)).unwrap();
        assert!(fake.tasks.borrow().is_empty());
        assert_eq!(fake.run_value(), Some(run_command(&root.exe())));
        // Start with Windows off: admin mode never adds a startup.
        run(Action::StartupOff, &root.ctx(&fake)).unwrap();
        run(Action::AdminOn { logon: false }, &root.ctx(&fake)).unwrap();
        assert_eq!(fake.tasks.borrow().get(SID), Some(&(root.exe(), false)));
        run(Action::AdminOff, &root.ctx(&fake)).unwrap();
        assert_eq!(fake.run_value(), None);
        assert!(fake.tasks.borrow().is_empty());
        // A failed delete changes nothing: no Run value next to a task that still starts.
        let stuck = Fake { failing: vec!["delete_task"], ..Fake::default() };
        stuck.tasks.borrow_mut().insert(SID.into(), (root.exe(), true));
        assert!(run(Action::AdminOff, &root.ctx(&stuck)).is_err());
        assert_eq!(stuck.run_value(), None);
    }

    /// `--tray` (the Run value) and a start by the task's logon trigger (no arguments at all:
    /// every start Negotium asks of the task carries some) start hidden in the tray.
    #[test]
    fn tray_argument_starts_hidden() {
        let os = |a: &[&str]| a.iter().map(OsString::from).collect::<Vec<_>>();
        let tray = |args: &[&str]| {
            let (from_task, rest) = task_args(own_args(os(args)));
            starts_in_tray(from_task, &rest)
        };
        assert!(tray(&["--tray"]));
        assert!(tray(&["--from-task", "--tray"]), "a Run value launch handed off");
        assert!(tray(&["--from-task", integration::ARG_PLACEHOLDER]), "the logon trigger");
        assert!(tray(&["--from-task"]), "the logon trigger, the placeholder emptied");
        assert!(tray(&["--from-task", ""]), "or an empty one");
        assert!(!tray(&[]), "a plain launch shows the window");
        assert!(!tray(&[r"C:\Windows\System32\Taskmgr.exe", "/4"]), "Ctrl+Shift+Esc");
        assert!(!tray(&["--from-task", SHOW]), "a hand-off of a plain launch");
        assert!(!tray(&["--from-task", "--restarted", "--page=startup"]));
        assert!(!tray(&["--page=startup"]));
        assert!(!tray(&["--trays"]));
        // The hand-off never sends an empty start, so it can't be taken for the logon trigger.
        let l = Launcher::new(TaskState::Ready);
        assert!(l.launch(false, false, &[]));
        assert!(l.launch(false, false, &["--tray"]));
        assert_eq!(*l.fake.started.borrow(), [vec![SHOW.to_string()], vec![TRAY.to_string()]]);
    }

    #[test]
    fn the_run_value_is_recognised_however_it_is_spelled() {
        let exe = Path::new(r"C:\Program Files\Negotium\negotium.exe");
        assert!(run_value_starts(&run_command(exe), exe));
        assert!(run_value_starts(r#""C:\PROGRAM FILES\Negotium\negotium.exe""#, exe));
        assert!(run_value_starts(r"C:\Progra~1\x.exe --tray", Path::new(r"C:\Progra~1\x.exe")));
        for other in ["", "\"\"", r#""C:\Tools\other.exe" --tray"#, r"C:\Program Files\Negotium\negotium.exe --tray"] {
            assert!(!run_value_starts(other, exe), "{other}");
        }
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
    fn launch_plan_hands_off_only_to_a_ready_task_from_a_plain_launch() {
        use Launch::{HandOff, RunHere};
        let broken = TaskState::Broken("why".into());
        for installed in [false, true] {
            for elevated in [false, true] {
                for admin in [false, true] {
                    for task in [TaskState::Missing, TaskState::Ready, TaskState::Disabled, broken.clone()] {
                        for from_task in [false, true] {
                            let want = if installed && !elevated && admin && task == TaskState::Ready && !from_task {
                                HandOff
                            } else {
                                RunHere
                            };
                            let got = launch_plan(installed, elevated, admin, task.clone(), from_task);
                            assert_eq!(got, want, "{installed} {elevated} {admin} {task:?} {from_task}");
                        }
                    }
                }
            }
        }
        assert_eq!(launch_plan(true, false, true, TaskState::Ready, false), HandOff);
    }

    #[test]
    fn the_tasks_own_arguments_are_recognised() {
        let os = |a: &[&str]| a.iter().map(OsString::from).collect::<Vec<_>>();
        assert_eq!(task_args(os(&["--from-task", "--page=startup"])), (true, os(&["--page=startup"])));
        // A logon start may leave the placeholder: no arguments.
        assert_eq!(task_args(os(&["--from-task", integration::ARG_PLACEHOLDER])), (true, vec![]));
        assert_eq!(task_args(os(&["--from-task", "--tray"])), (true, os(&["--tray"])));
        assert_eq!(task_args(os(&["--from-task"])), (true, vec![]));
        assert_eq!(task_args(os(&["--page=startup"])), (false, os(&["--page=startup"])));
        assert_eq!(task_args(os(&["--tray", "--from-task"])), (false, os(&["--tray", "--from-task"])), "only first");
        assert_eq!(task_args(vec![]), (false, vec![]));
        // The task can never reach the integration command: `--from-task --integrate …` is not
        // a valid command line.
        assert_eq!(Action::parse(["--from-task", "--integrate", "hotkey-on"]), None);
    }

    /// `hand_off` for the installed copy of an administrator, over a fake machine.
    struct Launcher {
        fake: Fake,
        running: bool,
        activated: std::cell::Cell<usize>,
        allowed: std::cell::Cell<usize>,
        /// How often the task was read, and a reader in place of the fake's.
        reads: std::cell::Cell<usize>,
        read: Option<Box<dyn Fn() -> Option<TaskState>>>,
        log: RefCell<Vec<String>>,
    }

    impl Launcher {
        fn new(state: TaskState) -> Launcher {
            Launcher::with(Fake { state: Some(state), ..Fake::default() })
        }

        fn with(fake: Fake) -> Launcher {
            Launcher {
                fake,
                running: false,
                activated: Default::default(),
                allowed: Default::default(),
                reads: Default::default(),
                read: None,
                log: Default::default(),
            }
        }

        fn launch(&self, elevated: bool, from_task: bool, args: &[&str]) -> bool {
            let sid = || Some(SID.to_string());
            let activate = || {
                self.activated.set(self.activated.get() + 1);
                self.running
            };
            let allow = || self.allowed.set(self.allowed.get() + 1);
            let read = |sid: &str| {
                self.reads.set(self.reads.get() + 1);
                match &self.read {
                    Some(read) => read(),
                    None => Some(self.fake.task_state(sid)),
                }
            };
            let ctx = LaunchCtx {
                installed: true,
                elevated,
                admin_member: true,
                from_task,
                user_sid: &sid,
                read_task: &read,
                machine: &self.fake,
                activate_running: &activate,
                allow_foreground: &allow,
            };
            let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
            hand_off(&ctx, &args, &mut |l| self.log.borrow_mut().push(l.to_string()))
        }
    }

    #[test]
    fn a_ready_task_is_started_with_the_page_and_this_launch_exits() {
        let l = Launcher::new(TaskState::Ready);
        assert!(l.launch(false, false, &["--page=startup"]));
        assert_eq!(*l.fake.started.borrow(), [vec!["--page=startup".to_string()]]);
        assert_eq!(l.allowed.get(), 1, "the elevated copy may take the foreground");
        assert!(l.log.borrow().is_empty());
        // An instance already running is shown instead; Task Scheduler isn't even asked.
        let l = Launcher { running: true, ..Launcher::new(TaskState::Ready) };
        assert!(l.launch(false, false, &[]));
        assert!(l.fake.started.borrow().is_empty());
        assert_eq!(l.activated.get(), 1);
        assert_eq!(l.reads.get(), 0);
        // Elevated already (the copy the task started, a restart as admin): run here.
        let l = Launcher::new(TaskState::Ready);
        assert!(!l.launch(true, false, &[]));
        assert!(!l.launch(true, true, &[]));
        assert!(l.fake.started.borrow().is_empty());
        assert_eq!(l.activated.get(), 0);
        assert!(l.log.borrow().is_empty());
    }

    #[test]
    fn a_failed_hand_off_runs_here_and_never_loops() {
        // The task can't be started: logged once, run here.
        let l = Launcher::with(Fake { state: Some(TaskState::Ready), failing: vec!["start_task"], ..Fake::default() });
        assert!(!l.launch(false, false, &[]));
        assert_eq!(l.fake.started.borrow().len(), 1);
        assert_eq!(l.log.borrow().len(), 1, "{:?}", l.log.borrow());
        assert!(l.log.borrow()[0].contains("start_task failed"), "{:?}", l.log.borrow());
        // The task ran but its copy isn't elevated: never start it again.
        assert_eq!(launch_plan(true, false, true, TaskState::Ready, true), Launch::RunHere);
        let l = Launcher::new(TaskState::Ready);
        assert!(!l.launch(false, true, &[]));
        assert!(l.fake.started.borrow().is_empty());
        assert_eq!(l.activated.get(), 0);
        assert_eq!(l.log.borrow().len(), 1, "{:?}", l.log.borrow());
        // Disabled or broken: logged with the reason, run here; missing (admin mode off): quiet.
        for (state, logged) in [
            (TaskState::Disabled, Some("disabled")),
            (TaskState::Broken("its exe is gone".into()), Some("its exe is gone")),
            (TaskState::Missing, None),
        ] {
            let l = Launcher::new(state.clone());
            assert!(!l.launch(false, false, &[]), "{state:?}");
            assert!(l.fake.started.borrow().is_empty());
            match logged {
                Some(why) => assert!(l.log.borrow().len() == 1 && l.log.borrow()[0].contains(why), "{:?}", l.log.borrow()),
                None => assert!(l.log.borrow().is_empty()),
            }
        }
    }

    /// A hung Schedule service: the launch runs here once the bound has passed, logged once.
    #[test]
    fn a_slow_task_read_runs_here_within_the_bound() {
        let l = Launcher {
            read: Some(Box::new(|| {
                bounded(Duration::from_millis(50), || {
                    std::thread::sleep(Duration::from_secs(5));
                    TaskState::Ready
                })
            })),
            ..Launcher::new(TaskState::Ready)
        };
        let started = std::time::Instant::now();
        assert!(!l.launch(false, false, &[]));
        assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
        assert!(l.fake.started.borrow().is_empty());
        assert_eq!(l.log.borrow().len(), 1, "{:?}", l.log.borrow());
        assert!(l.log.borrow()[0].contains("didn't report the task"), "{:?}", l.log.borrow());
        // A quick read comes back.
        assert_eq!(bounded(Duration::from_secs(5), || 7), Some(7));
        assert_eq!(TASK_READ_BOUND, Duration::from_millis(750));
    }

    #[test]
    fn only_the_installed_copy_of_an_administrator_hands_off() {
        let fake = Fake { state: Some(TaskState::Ready), ..Fake::default() };
        let sid = || Some(SID.to_string());
        let service = || Some("S-1-5-18".to_string());
        let unknown = || None;
        let running = || true;
        let nothing = || {};
        let read = |_: &str| -> Option<TaskState> { panic!("not read") };
        type Sid<'a> = &'a dyn Fn() -> Option<String>;
        let cases: [(bool, bool, Sid); 4] =
            [(false, true, &sid), (true, false, &sid), (true, true, &service), (true, true, &unknown)];
        for (installed, admin_member, user_sid) in cases {
            let ctx = LaunchCtx {
                installed,
                elevated: false,
                admin_member,
                from_task: false,
                user_sid,
                read_task: &read,
                machine: &fake,
                activate_running: &running,
                allow_foreground: &nothing,
            };
            assert!(!hand_off(&ctx, &[], &mut |_| {}), "{installed} {admin_member}");
        }
        assert!(fake.started.borrow().is_empty());
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
