//! Admin mode's Task Scheduler task, through the Task Scheduler COM API (ITaskService): one task
//! per user in the `Roots::task_folder` folder, named by the user's SID, running the installed exe
//! as that user (InteractiveToken: only while they are signed in, no password stored) with highest
//! privileges, optionally also at that user's sign-in.
//!
//! The action's arguments are `<prefix> $(Arg0)`: `start_task` passes its arguments as that one
//! value (Task Scheduler substitutes it), so a start can carry e.g. a page to open.
use super::{Roots, is_account_sid};
use crate::ActionError;
use crate::startup::ComApartment;
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::System::TaskScheduler::{
    IExecAction, IRegisteredTask, ITaskFolder, ITaskService, TASK_ACTION_EXEC, TASK_ACTION_TYPE,
    TASK_CREATE_OR_UPDATE, TASK_ENUM_HIDDEN, TASK_LOGON_INTERACTIVE_TOKEN, TASK_STATE_DISABLED,
    TASK_TRIGGER_LOGON, TASK_TRIGGER_TYPE2, TaskScheduler,
};
use windows::Win32::System::Variant::VARIANT;
use windows::core::{BSTR, Interface};

/// The action's placeholder for `start_task`'s arguments. A start with no value given (the logon
/// trigger) may leave it in the command line as written; the app must read it as "no arguments".
pub const ARG_PLACEHOLDER: &str = "$(Arg0)";

/// A task as Task Scheduler reports it (`task_status` turns it into a state).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskView {
    pub enabled: bool,
    /// `State` is TASK_STATE_DISABLED.
    pub state_disabled: bool,
    /// The exec actions' paths, as stored (maybe quoted, maybe with `%VARIABLES%`).
    pub exec_paths: Vec<String>,
    /// Actions of any other kind (email, message, COM handler).
    pub other_actions: usize,
    /// `LastTaskResult`: the last run's exit code, or Task Scheduler's HRESULT when it couldn't start.
    pub last_result: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TaskStatus {
    Missing,
    Ready,
    Disabled,
    /// Registered but unusable, or unreadable, with the reason (for the log and "needs repair"):
    /// not exactly one exec action, its exe is gone, the last start failed, or the task, its
    /// folder or Task Scheduler couldn't be read.
    Broken(String),
}

/// Whether `LastTaskResult` says Task Scheduler couldn't start the task (an HRESULT of the Win32
/// or the scheduler's own facility, e.g. 0x80070002 "file not found"). Exit codes of a run that
/// started, including a crash's NTSTATUS, don't count: the next start may well work.
fn failed_to_start(last_result: i32) -> bool {
    let hr = last_result as u32;
    let facility = (hr >> 16) & 0x1fff;
    hr & 0x8000_0000 != 0 && (hr & 0x2000_0000) == 0 && (facility == 7 || facility == 4)
}

/// The state of a task read as `view` (`None`: not registered); `exists` checks the action's exe.
pub fn task_status(view: Option<&TaskView>, exists: impl Fn(&Path) -> bool) -> TaskStatus {
    let Some(view) = view else {
        return TaskStatus::Missing;
    };
    let broken = |why: String| TaskStatus::Broken(why);
    match view.exec_paths.as_slice() {
        [path] if view.other_actions == 0 => {
            let exe = action_path(path);
            if !exists(&exe) {
                return broken(format!("its program is missing: {}", exe.display()));
            }
        }
        _ => {
            return broken(format!(
                "it has {} program and {} other actions, not one program",
                view.exec_paths.len(),
                view.other_actions
            ));
        }
    }
    if failed_to_start(view.last_result) {
        broken(format!("its last start failed (0x{:08X})", view.last_result as u32))
    } else if !view.enabled || view.state_disabled {
        TaskStatus::Disabled
    } else {
        TaskStatus::Ready
    }
}

/// An exec action's stored path as a file path: unquoted, `%VARIABLES%` expanded.
fn action_path(stored: &str) -> PathBuf {
    let path = stored.trim().trim_matches('"');
    PathBuf::from(crate::startup::expand_env(path, |name| std::env::var(name).ok()))
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

/// One argument quoted the way `CommandLineToArgvW` (and Rust's `std::env::args`) reads it back.
fn quote_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '\n', '\u{b}', '"']) {
        return arg.to_string();
    }
    let mut out = String::from('"');
    let mut backslashes = 0;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            c => {
                out.extend(std::iter::repeat_n('\\', backslashes));
                out.push(c);
                backslashes = 0;
            }
        }
    }
    out.extend(std::iter::repeat_n('\\', backslashes * 2));
    out.push('"');
    out
}

/// `args` as one command-line fragment, each quoted as needed.
pub fn join_args(args: &[String]) -> String {
    args.iter().map(|a| quote_arg(a)).collect::<Vec<_>>().join(" ")
}

/// The task's definition: principal `sid` (InteractiveToken, highest privileges), one action
/// `"<exe>" <prefix> $(Arg0)`, with `logon` a trigger at `sid`'s sign-in. Settings keep it
/// running for as long as the app runs (no time limit, not stopped on battery or idle), at normal
/// priority, and let a start run alongside a running copy (the app's own single-instance rule
/// decides what happens then).
pub fn task_xml(sid: &str, exe: &Path, prefix: &str, logon: bool) -> String {
    let sid = xml_escape(sid);
    let command = xml_escape(&format!("\"{}\"", exe.display()));
    let arguments = xml_escape(format!("{prefix} {ARG_PLACEHOLDER}").trim_start());
    let triggers = if logon {
        format!(
            "  <Triggers>\n    <LogonTrigger>\n      <Enabled>true</Enabled>\n      <UserId>{sid}</UserId>\n    </LogonTrigger>\n  </Triggers>\n"
        )
    } else {
        String::new()
    };
    format!(
        r#"<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Author>Negotium</Author>
    <Description>Starts Negotium as administrator without a prompt (Negotium's admin mode).</Description>
  </RegistrationInfo>
{triggers}  <Principals>
    <Principal id="Author">
      <UserId>{sid}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>HighestAvailable</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>Parallel</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <StartWhenAvailable>false</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <IdleSettings>
      <StopOnIdleEnd>false</StopOnIdleEnd>
      <RestartOnIdle>false</RestartOnIdle>
    </IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>4</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{command}</Command>
      <Arguments>{arguments}</Arguments>
    </Exec>
  </Actions>
</Task>
"#
    )
}

fn com(e: windows::core::Error) -> ActionError {
    ActionError::from_win(&e)
}

fn is_not_found(e: &windows::core::Error) -> bool {
    let code = e.code();
    code == ERROR_FILE_NOT_FOUND.to_hresult() || code == ERROR_PATH_NOT_FOUND.to_hresult()
}

/// A name Task Scheduler takes as one plain component: `[A-Za-z0-9_-]+` (so never `.`, `..`,
/// a separator, a wildcard or whitespace).
fn is_plain_name(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// `\<name>`, for a plain folder name (`is_plain_name`).
fn folder_path(roots: &Roots) -> Result<BSTR, ActionError> {
    let name = roots.task_folder.as_str();
    if !is_plain_name(name) {
        return Err(ActionError::Other(format!("not a task folder name: {name:?}")));
    }
    Ok(BSTR::from(format!("\\{name}")))
}

/// The task's name: an account SID (`is_account_sid`), which is also a plain name.
fn task_name(sid: &str) -> Result<BSTR, ActionError> {
    if !is_account_sid(sid) || !is_plain_name(sid) {
        return Err(ActionError::Other(format!("not a user account's SID: {sid:?}")));
    }
    Ok(BSTR::from(sid))
}

fn service() -> Result<ITaskService, ActionError> {
    // SAFETY: creates the in-process Task Scheduler object; the caller holds a COM apartment.
    let service: ITaskService = unsafe { CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER) }.map_err(com)?;
    let none = VARIANT::default();
    // SAFETY: connects to the local machine as the current user (all four arguments empty).
    unsafe { service.Connect(&none, &none, &none, &none) }.map_err(com)?;
    Ok(service)
}

/// The folder, `None` when it doesn't exist.
fn folder(service: &ITaskService, roots: &Roots) -> Result<Option<ITaskFolder>, ActionError> {
    let path = folder_path(roots)?;
    // SAFETY: plain COM call on a connected service with a valid BSTR.
    match unsafe { service.GetFolder(&path) } {
        Ok(f) => Ok(Some(f)),
        Err(e) if is_not_found(&e) => Ok(None),
        Err(e) => Err(com(e)),
    }
}

fn root(service: &ITaskService) -> Result<ITaskFolder, ActionError> {
    // SAFETY: plain COM call on a connected service with a valid BSTR.
    unsafe { service.GetFolder(&BSTR::from("\\")) }.map_err(com)
}

/// The registered task, `None` when it (or its folder) doesn't exist.
fn registered(service: &ITaskService, roots: &Roots, sid: &str) -> Result<Option<IRegisteredTask>, ActionError> {
    let name = task_name(sid)?;
    let Some(folder) = folder(service, roots)? else {
        return Ok(None);
    };
    // SAFETY: plain COM call on a valid folder with a valid BSTR.
    match unsafe { folder.GetTask(&name) } {
        Ok(t) => Ok(Some(t)),
        Err(e) if is_not_found(&e) => Ok(None),
        Err(e) => Err(com(e)),
    }
}

/// Creates (or replaces) user `sid`'s task in `roots.task_folder` (created if needed):
/// `task_xml(sid, exe, prefix, logon)`.
pub fn create_task(roots: &Roots, sid: &str, exe: &Path, prefix: &str, logon: bool) -> Result<(), ActionError> {
    let name = task_name(sid)?;
    let path = folder_path(roots)?;
    let _com = ComApartment::enter();
    let service = service()?;
    let folder = match folder(&service, roots)? {
        Some(f) => f,
        None => {
            let none = VARIANT::default();
            // SAFETY: plain COM call on the root folder; the default security descriptor is
            // inherited from the root (administrators and SYSTEM may change tasks in it).
            match unsafe { root(&service)?.CreateFolder(&path, &none) } {
                Ok(f) => f,
                // Created meanwhile by another run.
                Err(e) if e.code() == ERROR_ALREADY_EXISTS.to_hresult() => {
                    folder(&service, roots)?.ok_or_else(|| com(e))?
                }
                Err(e) => return Err(com(e)),
            }
        }
    };
    let xml = BSTR::from(task_xml(sid, exe, prefix, logon));
    let none = VARIANT::default();
    // SAFETY: plain COM call on a valid folder; the principal (the user, InteractiveToken) comes
    // from the XML, so no user or password is passed.
    unsafe {
        folder.RegisterTask(&name, &xml, TASK_CREATE_OR_UPDATE.0, &none, &none, TASK_LOGON_INTERACTIVE_TOKEN, &none)
    }
    .map_err(com)?;
    Ok(())
}

fn view(task: &IRegisteredTask) -> windows::core::Result<TaskView> {
    // SAFETY (this block): plain COM property reads on a valid registered task and the objects it
    // returns; out-parameters are writable locals; collections are 1-based.
    unsafe {
        let actions = task.Definition()?.Actions()?;
        let mut count = 0i32;
        actions.Count(&mut count)?;
        let (mut exec_paths, mut other_actions) = (Vec::new(), 0);
        for i in 1..=count {
            let action = actions.get_Item(i)?;
            let mut kind = TASK_ACTION_TYPE::default();
            action.Type(&mut kind)?;
            if kind == TASK_ACTION_EXEC {
                let mut path = BSTR::new();
                action.cast::<IExecAction>()?.Path(&mut path)?;
                exec_paths.push(path.to_string());
            } else {
                other_actions += 1;
            }
        }
        Ok(TaskView {
            enabled: task.Enabled()?.as_bool(),
            state_disabled: task.State()? == TASK_STATE_DISABLED,
            exec_paths,
            other_actions,
            last_result: task.LastTaskResult()?,
        })
    }
}

/// User `sid`'s task state. Anything that stops it being read (a bad SID or folder name, COM,
/// Task Scheduler not running, access) is `Broken` with that reason.
pub fn task_state(roots: &Roots, sid: &str) -> TaskStatus {
    let _com = ComApartment::enter();
    let read = || -> Result<Option<TaskView>, ActionError> {
        // Names first: a bad one never reaches Task Scheduler.
        task_name(sid)?;
        folder_path(roots)?;
        let service = service()?;
        registered(&service, roots, sid)?.map(|t| view(&t).map_err(com)).transpose()
    };
    match read() {
        Ok(v) => task_status(v.as_ref(), Path::is_file),
        Err(e) => TaskStatus::Broken(format!("it couldn't be read: {e}")),
    }
}

fn has_logon_trigger(task: &IRegisteredTask) -> windows::core::Result<bool> {
    // SAFETY (this block): plain COM property reads on a valid registered task and the objects it
    // returns; out-parameters are writable locals; the collection is 1-based.
    unsafe {
        let triggers = task.Definition()?.Triggers()?;
        let mut count = 0i32;
        triggers.Count(&mut count)?;
        for i in 1..=count {
            let mut kind = TASK_TRIGGER_TYPE2::default();
            triggers.get_Item(i)?.Type(&mut kind)?;
            if kind == TASK_TRIGGER_LOGON {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Whether user `sid`'s task also starts at sign-in (has a logon trigger): Start with Windows in
/// admin mode. A missing or unreadable task has none.
pub fn task_logon(roots: &Roots, sid: &str) -> bool {
    if task_name(sid).is_err() || folder_path(roots).is_err() {
        return false;
    }
    let _com = ComApartment::enter();
    let read = || -> Result<bool, ActionError> {
        let service = service()?;
        match registered(&service, roots, sid)? {
            Some(task) => has_logon_trigger(&task).map_err(com),
            None => Ok(false),
        }
    };
    read().unwrap_or(false)
}

/// Starts user `sid`'s task now, with `args` as its `$(Arg0)` (`join_args`).
pub fn start_task(roots: &Roots, sid: &str, args: &[String]) -> Result<(), ActionError> {
    task_name(sid)?;
    folder_path(roots)?;
    let _com = ComApartment::enter();
    let service = service()?;
    let task = registered(&service, roots, sid)?
        .ok_or_else(|| ActionError::Other("the admin mode task is missing".into()))?;
    let params = VARIANT::from(BSTR::from(join_args(args)));
    // SAFETY: plain COM call on a valid registered task; `params` is a BSTR VARIANT.
    unsafe { task.Run(&params) }.map_err(com)?;
    Ok(())
}

/// Deletes user `sid`'s task (a missing task or folder is fine); the folder stays.
pub fn delete_task(roots: &Roots, sid: &str) -> Result<(), ActionError> {
    let name = task_name(sid)?;
    folder_path(roots)?;
    let _com = ComApartment::enter();
    let service = service()?;
    let Some(folder) = folder(&service, roots)? else {
        return Ok(());
    };
    // SAFETY: plain COM call on a valid folder with a valid BSTR.
    match unsafe { folder.DeleteTask(&name, 0) } {
        Ok(()) => Ok(()),
        Err(e) if is_not_found(&e) => Ok(()),
        Err(e) => Err(com(e)),
    }
}

/// Deletes every task and subfolder in `folder` (Task Scheduler deletes only empty folders).
fn empty(folder: &ITaskFolder) -> windows::core::Result<()> {
    // SAFETY (this block): plain COM calls on a valid folder and the collections it returns;
    // names are collected before anything is deleted; collections are 1-based.
    unsafe {
        let tasks = folder.GetTasks(TASK_ENUM_HIDDEN.0)?;
        let names: Vec<BSTR> = (1..=tasks.Count()?)
            .map(|i| tasks.get_Item(&VARIANT::from(i))?.Name())
            .collect::<windows::core::Result<_>>()?;
        for name in &names {
            folder.DeleteTask(name, 0)?;
        }
        let subfolders = folder.GetFolders(0)?;
        let subs: Vec<(ITaskFolder, BSTR)> = (1..=subfolders.Count()?)
            .map(|i| {
                let sub = subfolders.get_Item(&VARIANT::from(i))?;
                let name = sub.Name()?;
                Ok((sub, name))
            })
            .collect::<windows::core::Result<_>>()?;
        for (sub, name) in &subs {
            empty(sub)?;
            folder.DeleteFolder(name, 0)?;
        }
    }
    Ok(())
}

/// Deletes `roots.task_folder` with every task in it (a missing folder is fine).
pub fn delete_task_folder(roots: &Roots) -> Result<(), ActionError> {
    let path = folder_path(roots)?;
    let _com = ComApartment::enter();
    let service = service()?;
    let Some(folder) = folder(&service, roots)? else {
        return Ok(());
    };
    empty(&folder).map_err(com)?;
    // SAFETY: plain COM call on the root folder with a valid BSTR naming its (now empty) subfolder.
    match unsafe { root(&service)?.DeleteFolder(&path, 0) } {
        Ok(()) => Ok(()),
        Err(e) if is_not_found(&e) => Ok(()),
        Err(e) => Err(com(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integration::{TASK_FOLDER, current_user_sid};

    const SID: &str = "S-1-5-21-1004336348-1177238915-682003330-1001";
    const EXE: &str = r"C:\Program Files\Negotium\negotium.exe";

    #[test]
    fn the_definition_runs_the_exe_as_the_user_with_highest_privileges() {
        let xml = task_xml(SID, Path::new(EXE), "--from-task", false);
        for part in [
            r#"<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">"#,
            "<Principal id=\"Author\">\n      <UserId>S-1-5-21-1004336348-1177238915-682003330-1001</UserId>\n      <LogonType>InteractiveToken</LogonType>\n      <RunLevel>HighestAvailable</RunLevel>",
            "<Actions Context=\"Author\">\n    <Exec>\n      <Command>&quot;C:\\Program Files\\Negotium\\negotium.exe&quot;</Command>\n      <Arguments>--from-task $(Arg0)</Arguments>\n    </Exec>\n  </Actions>",
            "<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>",
            "<DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>",
            "<StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>",
            "<StopOnIdleEnd>false</StopOnIdleEnd>",
            "<MultipleInstancesPolicy>Parallel</MultipleInstancesPolicy>",
            "<Priority>4</Priority>",
            "<Enabled>true</Enabled>",
        ] {
            assert!(xml.contains(part), "missing {part}\n{xml}");
        }
        assert!(!xml.contains("<Triggers>"), "no trigger without logon\n{xml}");
        assert_eq!(xml.matches("<Exec>").count(), 1);
        assert!(xml.ends_with("</Task>\n"));
    }

    #[test]
    fn with_logon_the_task_also_starts_at_that_users_sign_in() {
        let xml = task_xml(SID, Path::new(EXE), "--from-task", true);
        let trigger = format!(
            "  <Triggers>\n    <LogonTrigger>\n      <Enabled>true</Enabled>\n      <UserId>{SID}</UserId>\n    </LogonTrigger>\n  </Triggers>\n"
        );
        assert!(xml.contains(&format!("{trigger}  <Principals>")), "{xml}");
        assert_eq!(xml.matches("Trigger>").count(), 2, "one logon trigger, nothing else");
        // The same task otherwise.
        assert_eq!(xml.replace(&trigger, ""), task_xml(SID, Path::new(EXE), "--from-task", false));
    }

    #[test]
    fn the_definition_escapes_what_it_is_given() {
        let xml = task_xml(SID, Path::new(r"C:\A&B <x>\it's.exe"), "/c", false);
        assert!(xml.contains(r"<Command>&quot;C:\A&amp;B &lt;x&gt;\it&apos;s.exe&quot;</Command>"), "{xml}");
        assert!(xml.contains("<Arguments>/c $(Arg0)</Arguments>"));
        assert!(task_xml(SID, Path::new(EXE), "", false).contains("<Arguments>$(Arg0)</Arguments>"));
    }

    #[test]
    fn arguments_join_into_one_command_line_fragment() {
        let s = |v: &[&str]| join_args(&v.iter().map(|a| a.to_string()).collect::<Vec<_>>());
        assert_eq!(s(&[]), "");
        assert_eq!(s(&["--page", "network"]), "--page network");
        assert_eq!(s(&["exit", "0"]), "exit 0");
        assert_eq!(s(&["a b", ""]), "\"a b\" \"\"");
        assert_eq!(s(&[r#"say "hi""#]), r#""say \"hi\"""#);
        assert_eq!(s(&[r"C:\dir with space\"]), r#""C:\dir with space\\""#);
        assert_eq!(s(&[r"C:\plain\"]), r"C:\plain\");
        assert_eq!(s(&[r#"a\"b"#]), r#""a\\\"b""#);
    }

    fn view(paths: &[&str], other: usize, enabled: bool, disabled: bool, last: i32) -> TaskView {
        TaskView {
            enabled,
            state_disabled: disabled,
            exec_paths: paths.iter().map(|p| p.to_string()).collect(),
            other_actions: other,
            last_result: last,
        }
    }

    #[test]
    fn a_task_is_ready_disabled_or_broken() {
        let exists = |p: &Path| p == Path::new(EXE);
        let quoted = format!("\"{EXE}\"");
        let st = |v: TaskView| task_status(Some(&v), exists);
        let hr = |h: u32| h as i32;
        assert_eq!(task_status(None, exists), TaskStatus::Missing);
        assert_eq!(st(view(&[&quoted], 0, true, false, 0)), TaskStatus::Ready);
        assert_eq!(st(view(&[EXE], 0, true, false, 0x41303)), TaskStatus::Ready, "never run yet");
        assert_eq!(st(view(&[EXE], 0, true, false, 0x41301)), TaskStatus::Ready, "running");
        assert_eq!(st(view(&[EXE], 0, true, false, 1)), TaskStatus::Ready, "the app's own exit code");
        assert_eq!(st(view(&[EXE], 0, true, false, hr(0xC000_0005))), TaskStatus::Ready, "a crash isn't a broken task");
        assert_eq!(st(view(&[EXE], 0, false, false, 0)), TaskStatus::Disabled);
        assert_eq!(st(view(&[EXE], 0, true, true, 0)), TaskStatus::Disabled);
        // Broken, with the reason: the exe is gone, the action isn't one exec action, the last
        // start failed.
        let reason = |v: TaskView| match st(v) {
            TaskStatus::Broken(why) => why,
            other => panic!("not broken: {other:?}"),
        };
        let gone = r"C:\gone\negotium.exe";
        assert_eq!(reason(view(&[gone], 0, true, false, 0)), format!("its program is missing: {gone}"));
        assert_eq!(reason(view(&[], 0, true, false, 0)), "it has 0 program and 0 other actions, not one program");
        assert_eq!(reason(view(&[EXE, EXE], 0, true, false, 0)), "it has 2 program and 0 other actions, not one program");
        assert_eq!(reason(view(&[EXE], 1, true, false, 0)), "it has 1 program and 1 other actions, not one program");
        assert_eq!(reason(view(&[EXE], 0, true, false, hr(0x8007_0002))), "its last start failed (0x80070002)");
        assert_eq!(reason(view(&[EXE], 0, true, false, hr(0x8004_1315))), "its last start failed (0x80041315)");
        assert!(matches!(st(view(&[gone], 0, false, false, 0)), TaskStatus::Broken(_)), "broken wins");
    }

    #[test]
    fn action_paths_are_unquoted_and_expanded() {
        let root = std::env::var("SystemRoot").unwrap();
        assert_eq!(
            action_path(r#""%SystemRoot%\System32\cmd.exe""#),
            PathBuf::from(format!(r"{root}\System32\cmd.exe"))
        );
        assert_eq!(action_path(EXE), PathBuf::from(EXE));
    }

    #[test]
    fn names_are_checked_before_task_scheduler_is_asked() {
        for bad in ["", ".", "..", r"a\b", "a/b", " x", "x ", "a.b", "a*", "Négotium", r"Negotium\..\Microsoft"] {
            let roots = Roots { task_folder: bad.into(), ..Roots::system() };
            assert!(folder_path(&roots).is_err(), "{bad:?}");
            assert!(matches!(delete_task_folder(&roots), Err(ActionError::Other(_))), "{bad:?}");
            // Refused before COM, so the reason names the folder.
            match task_state(&roots, SID) {
                TaskStatus::Broken(why) => assert!(why.contains("not a task folder name"), "{why}"),
                other => panic!("{bad:?}: {other:?}"),
            }
        }
        for good in ["Negotium", "Negotium-test-1234", "a_b"] {
            let roots = Roots { task_folder: good.into(), ..Roots::system() };
            assert_eq!(folder_path(&roots).unwrap(), format!("\\{good}"));
        }
        for bad in [r"..\x", "*", ".", "S-1-5-18", "S-1-5-21-1-2-3"] {
            assert!(matches!(delete_task(&Roots::system(), bad), Err(ActionError::Other(_))), "{bad}");
            assert!(matches!(create_task(&Roots::system(), bad, Path::new(EXE), "", false), Err(ActionError::Other(_))), "{bad}");
            assert!(matches!(start_task(&Roots::system(), bad, &[]), Err(ActionError::Other(_))), "{bad}");
            assert!(!task_logon(&Roots::system(), bad), "{bad}");
        }
        assert_eq!(folder_path(&Roots::system()).unwrap(), "\\Negotium");
    }

    /// Deletes the test folder however the test ends.
    struct TestFolder(Roots);

    impl Drop for TestFolder {
        fn drop(&mut self) {
            let _ = delete_task_folder(&self.0);
        }
    }

    /// A throwaway folder `\Negotium-test-<pid>\` (never `\Negotium\`) holding a task that runs
    /// cmd.exe (`/c exit 0`): created, read, started, replaced, deleted, then the folder deleted.
    #[test]
    #[ignore = "needs an elevated terminal"]
    fn create_read_start_and_delete_a_task_in_a_test_folder() {
        let roots = Roots { task_folder: format!("Negotium-test-{}", std::process::id()), ..Roots::system() };
        assert_ne!(roots.task_folder, TASK_FOLDER);
        let _cleanup = TestFolder(roots.clone());
        let sid = current_user_sid().unwrap();
        let cmd = PathBuf::from(std::env::var("SystemRoot").unwrap()).join(r"System32\cmd.exe");
        assert_eq!(task_state(&roots, &sid), TaskStatus::Missing);
        create_task(&roots, &sid, &cmd, "/c", false).unwrap();
        assert_eq!(task_state(&roots, &sid), TaskStatus::Ready);
        assert!(!task_logon(&roots, &sid), "no logon trigger");
        start_task(&roots, &sid, &["exit".to_string(), "0".to_string()]).unwrap();
        // Replaced in place, now with the logon trigger.
        create_task(&roots, &sid, &cmd, "/c", true).unwrap();
        assert_ne!(task_state(&roots, &sid), TaskStatus::Missing);
        assert!(task_logon(&roots, &sid), "the logon trigger reads back");
        delete_task(&roots, &sid).unwrap();
        assert!(!task_logon(&roots, &sid), "a missing task has none");
        assert_eq!(task_state(&roots, &sid), TaskStatus::Missing);
        delete_task(&roots, &sid).unwrap();
        // A task whose exe is gone reads as broken.
        let gone = std::env::temp_dir().join(format!("negotium-test-gone-{}.exe", std::process::id()));
        create_task(&roots, &sid, &gone, "/c", false).unwrap();
        assert!(matches!(task_state(&roots, &sid), TaskStatus::Broken(why) if why.contains("missing")));
        // The folder goes with every task in it.
        create_task(&roots, &sid, &cmd, "/c", false).unwrap();
        delete_task_folder(&roots).unwrap();
        assert_eq!(task_state(&roots, &sid), TaskStatus::Missing);
        let _com = ComApartment::enter();
        assert!(folder(&service().unwrap(), &roots).unwrap().is_none(), "the folder is gone");
        delete_task_folder(&roots).unwrap();
    }
}
