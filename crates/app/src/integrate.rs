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
use std::ffi::OsStr;
use std::fmt;
use std::path::{Path, PathBuf};

/// The flag that selects the integration command instead of the window.
pub const FLAG: &str = "--integrate";
/// `admin-on` only: the task also starts at the user's sign-in (Start with Windows in admin mode).
pub const LOGON: &str = "--logon";
/// The argument the Run value (and a logon start) passes: start hidden in the tray.
pub const TRAY: &str = "--tray";

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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskState {
    Missing,
    Ready,
    Disabled,
    /// Registered but unusable (wrong action, failed last run, unreadable definition).
    Broken,
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
    /// Sets (`Some`) or removes (`None`) taskmgr.exe's IFEO `Debugger` value.
    fn set_ifeo(&self, exe: Option<&Path>) -> Result<(), IntegrateError>;
    /// The exe taskmgr.exe's IFEO `Debugger` value names, if any.
    fn ifeo(&self) -> Option<PathBuf>;
    /// Sets (`Some`, a whole command line) or removes (`None`) the HKCU Run value `Negotium`.
    fn set_run(&self, command: Option<&str>) -> Result<(), IntegrateError>;
    /// The HKCU Run value `Negotium`, if any.
    fn run_value(&self) -> Option<String>;
    /// Creates (or replaces) user `sid`'s admin task in `\Negotium\`: highest privileges, action
    /// `exe`, and with `logon` a trigger at that user's sign-in.
    fn create_task(&self, sid: &str, exe: &Path, logon: bool) -> Result<(), IntegrateError>;
    /// Deletes user `sid`'s admin task (absent is fine).
    fn delete_task(&self, sid: &str) -> Result<(), IntegrateError>;
    fn task_state(&self, sid: &str) -> TaskState;
    /// Deletes the `\Negotium\` folder with every user's task in it (absent is fine).
    fn delete_task_folder(&self) -> Result<(), IntegrateError>;
    /// Starts user `sid`'s admin task with `args`.
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
    let ok = sid.strip_prefix("S-1-").is_some_and(|rest| {
        rest.split('-').count() >= 2
            && rest.split('-').all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
    });
    if ok { Ok(sid.to_string()) } else { Err(IntegrateError::BadSid(sid.to_string())) }
}

/// The Run value: the installed exe, quoted (Program Files has a space), started in the tray.
pub fn run_command(exe: &Path) -> String {
    format!("\"{}\" {TRAY}", exe.display())
}

fn same_path(a: &Path, b: &Path) -> bool {
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

/// Runs `action` as the installed exe (refusing otherwise, before any change).
pub fn run(action: Action, ctx: &Ctx) -> Result<(), IntegrateError> {
    let exe = installed_exe(ctx)?;
    let machine = ctx.machine;
    match action {
        Action::HotkeyOn => machine.set_ifeo(Some(&exe)),
        Action::HotkeyOff => clear_redirect(machine, &exe),
        Action::StartupOn => machine.set_run(Some(&run_command(&exe))),
        Action::StartupOff => machine.set_run(None),
        Action::AdminOn { logon } => machine.create_task(&user_sid(ctx)?, &exe, logon),
        Action::AdminOff => machine.delete_task(&user_sid(ctx)?),
        Action::OpenTaskManager => Err(IntegrateError::NotImplemented("open-task-manager")),
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
    };
    match run(action, &ctx) {
        Ok(()) => 0,
        Err(e) => {
            log(&format!("integrate {}: {e}", action.name()));
            1
        }
    }
}

/// The machine `main` uses until the registry and Task Scheduler implementation lands: every
/// change and every read is refused as not implemented.
pub struct Unavailable;

impl Unavailable {
    const WHAT: &'static str = "Windows integration";
}

impl Machine for Unavailable {
    fn set_ifeo(&self, _: Option<&Path>) -> Result<(), IntegrateError> {
        Err(IntegrateError::NotImplemented(Self::WHAT))
    }
    fn ifeo(&self) -> Option<PathBuf> {
        None
    }
    fn set_run(&self, _: Option<&str>) -> Result<(), IntegrateError> {
        Err(IntegrateError::NotImplemented(Self::WHAT))
    }
    fn run_value(&self) -> Option<String> {
        None
    }
    fn create_task(&self, _: &str, _: &Path, _: bool) -> Result<(), IntegrateError> {
        Err(IntegrateError::NotImplemented(Self::WHAT))
    }
    fn delete_task(&self, _: &str) -> Result<(), IntegrateError> {
        Err(IntegrateError::NotImplemented(Self::WHAT))
    }
    fn task_state(&self, _: &str) -> TaskState {
        TaskState::Missing
    }
    fn delete_task_folder(&self) -> Result<(), IntegrateError> {
        Err(IntegrateError::NotImplemented(Self::WHAT))
    }
    fn start_task(&self, _: &str, _: &[String]) -> Result<(), IntegrateError> {
        Err(IntegrateError::NotImplemented(Self::WHAT))
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
            Ctx { exe: self.exe(), program_files: Some(self.0.clone()), user_sid: SID.to_string(), machine }
        }
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

    #[test]
    fn open_task_manager_is_not_implemented_yet() {
        let root = Root::new();
        let fake = Fake::default();
        assert!(matches!(run(Action::OpenTaskManager, &root.ctx(&fake)), Err(IntegrateError::NotImplemented(_))));
        assert!(fake.calls.borrow().is_empty());
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

    #[test]
    fn the_unavailable_machine_refuses_every_change() {
        let root = Root::new();
        for action in Action::ALL {
            let r = run(action, &root.ctx(&Unavailable));
            assert!(matches!(r, Err(IntegrateError::NotImplemented(_))), "{action:?}: {r:?}");
        }
    }
}
