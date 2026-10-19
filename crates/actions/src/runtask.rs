//! Custom "Run new task" power launcher: supports Normal, Elevated (Administrator), Force Standard
//! User (`RunAsInvoker`), and Sandboxed (Low Integrity token) launches, compatibility layers,
//! optional working directory, and starting in Efficiency Mode (EcoQoS + idle priority).
use crate::{ActionError, OwnedHandle};
use std::ffi::c_void;
use std::mem::size_of;
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::Path;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LocalFree};
use windows::Win32::Security::Authorization::ConvertStringSidToSidW;
use windows::Win32::Security::{
    CreateRestrictedToken, DISABLE_MAX_PRIVILEGE, PSID, SID_AND_ATTRIBUTES, SetTokenInformation,
    TOKEN_ADJUST_DEFAULT, TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_MANDATORY_LABEL, TOKEN_QUERY,
    TokenIntegrityLevel,
};

const SE_GROUP_INTEGRITY: u32 = 0x0000_0020;
use windows::Win32::Storage::FileSystem::SearchPathW;
use windows::Win32::System::Threading::{
    CreateProcessAsUserW, GetCurrentProcess, IDLE_PRIORITY_CLASS, OpenProcessToken, PROCESS_INFORMATION,
    PROCESS_POWER_THROTTLING_CURRENT_VERSION, PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_STATE,
    ProcessPowerThrottling, STARTUPINFOW, SetPriorityClass, SetProcessInformation,
};
use windows::Win32::UI::Shell::{SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{HSTRING, PCWSTR, PWSTR, w};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PrivilegeMode {
    #[default]
    Normal,
    Elevated,
    ForceInvoker,
    LowIntegrity,
}

impl PrivilegeMode {
    pub fn from_index(i: i32) -> Self {
        match i {
            1 => Self::Elevated,
            2 => Self::ForceInvoker,
            3 => Self::LowIntegrity,
            _ => Self::Normal,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CompatLayer {
    #[default]
    None,
    Win8,
    Win7,
    HighDpiAware,
}

impl CompatLayer {
    pub fn from_index(i: i32) -> Self {
        match i {
            1 => Self::Win8,
            2 => Self::Win7,
            3 => Self::HighDpiAware,
            _ => Self::None,
        }
    }

    pub fn token(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Win8 => Some("WIN8RTM"),
            Self::Win7 => Some("WIN7RTM"),
            Self::HighDpiAware => Some("HIGHDPIAWARE"),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunTaskOptions {
    pub command: String,
    pub working_dir: String,
    pub privilege: PrivilegeMode,
    pub compat: CompatLayer,
    pub efficiency_mode: bool,
}

/// Splits a command line into `(program, rest_args)`. Handles a leading `"quoted path"` or an
/// unquoted first token.
pub fn split_command(line: &str) -> Option<(&str, &str)> {
    let s = line.trim();
    if s.is_empty() {
        return None;
    }
    if let Some(after_quote) = s.strip_prefix('"') {
        if let Some(end) = after_quote.find('"') {
            let prog = &after_quote[..end];
            let rest = after_quote[end + 1..].trim_start();
            return (!prog.is_empty()).then_some((prog, rest));
        }
        return (!after_quote.is_empty()).then_some((after_quote, ""));
    }
    match s.find(char::is_whitespace) {
        Some(pos) => Some((&s[..pos], s[pos..].trim_start())),
        None => Some((s, "")),
    }
}

/// Builds the `__COMPAT_LAYER` environment variable value when either `RunAsInvoker` or a
/// compatibility layer is selected.
pub fn compat_env(privilege: PrivilegeMode, compat: CompatLayer) -> Option<String> {
    let mut parts = Vec::new();
    if privilege == PrivilegeMode::ForceInvoker {
        parts.push("RunAsInvoker");
    }
    if let Some(tok) = compat.token() {
        parts.push(tok);
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

/// Launches the requested command off the UI thread.
pub fn run_new_task(opts: &RunTaskOptions, owner_hwnd: isize) -> Result<(), ActionError> {
    let Some((prog, args)) = split_command(&opts.command) else {
        return Err(ActionError::Other("enter a program, path, or command to run".into()));
    };
    let working_dir = opts.working_dir.trim();
    let dir_opt = (!working_dir.is_empty()).then(|| Path::new(working_dir));
    if let Some(d) = dir_opt
        && !d.is_dir()
    {
        return Err(ActionError::Other(format!("working directory '{}' does not exist", working_dir)));
    }
    match opts.privilege {
        PrivilegeMode::Elevated => launch_shell(w!("runas"), prog, args, dir_opt, owner_hwnd, opts.efficiency_mode),
        PrivilegeMode::LowIntegrity => launch_low_integrity(prog, args, dir_opt, opts),
        PrivilegeMode::Normal | PrivilegeMode::ForceInvoker => launch_standard(prog, args, dir_opt, owner_hwnd, opts),
    }
}

fn apply_efficiency_to_handle(handle: HANDLE) {
    let state = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        StateMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
    };
    // SAFETY: `state` is a valid PROCESS_POWER_THROTTLING_STATE; best-effort on newly spawned process.
    unsafe {
        let _ = SetProcessInformation(
            handle,
            ProcessPowerThrottling,
            &state as *const _ as *const c_void,
            size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        );
        let _ = SetPriorityClass(handle, IDLE_PRIORITY_CLASS);
    }
}

fn launch_standard(
    prog: &str,
    args: &str,
    dir: Option<&Path>,
    owner_hwnd: isize,
    opts: &RunTaskOptions,
) -> Result<(), ActionError> {
    let mut cmd = std::process::Command::new(prog);
    if !args.is_empty() {
        cmd.raw_arg(args);
    }
    if let Some(d) = dir {
        cmd.current_dir(d);
    }
    if let Some(compat) = compat_env(opts.privilege, opts.compat) {
        cmd.env("__COMPAT_LAYER", compat);
    }
    if opts.efficiency_mode {
        cmd.creation_flags(IDLE_PRIORITY_CLASS.0);
    }
    match cmd.spawn() {
        Ok(child) => {
            if opts.efficiency_mode {
                let raw = HANDLE(child.as_raw_handle());
                apply_efficiency_to_handle(raw);
            }
            Ok(())
        }
        Err(_) if opts.privilege == PrivilegeMode::Normal => {
            // Fall back to ShellExecuteEx ("open") so folders, documents, URLs, and App Execution Aliases work.
            launch_shell(w!("open"), prog, args, dir, owner_hwnd, opts.efficiency_mode)
        }
        Err(e) => Err(ActionError::Other(e.to_string())),
    }
}

fn launch_shell(
    verb: PCWSTR,
    prog: &str,
    args: &str,
    dir: Option<&Path>,
    owner_hwnd: isize,
    efficiency_mode: bool,
) -> Result<(), ActionError> {
    let _com = crate::startup::ComApartment::enter();
    let file = HSTRING::from(prog);
    let params = HSTRING::from(args);
    let dir_h = dir.map(|d| HSTRING::from(d.as_os_str())).unwrap_or_default();
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        hwnd: HWND(owner_hwnd as *mut c_void),
        fMask: if efficiency_mode { SEE_MASK_NOASYNC | SEE_MASK_NOCLOSEPROCESS } else { SEE_MASK_NOASYNC },
        lpVerb: verb,
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        lpDirectory: if dir.is_some() { PCWSTR(dir_h.as_ptr()) } else { PCWSTR::null() },
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    // SAFETY: `info` is initialized and all HSTRING buffers outlive the call.
    unsafe { ShellExecuteExW(&mut info) }.map_err(|e| ActionError::from_win(&e))?;
    if efficiency_mode && !info.hProcess.is_invalid() {
        let proc = OwnedHandle(info.hProcess);
        apply_efficiency_to_handle(proc.0);
    }
    Ok(())
}

/// Resolves an executable name (e.g. `notepad` or `cmd.exe`) via `SearchPathW` when needed so
/// `CreateProcessAsUserW` can launch it.
fn resolve_executable(prog: &str) -> HSTRING {
    let name = HSTRING::from(prog);
    let mut buf = vec![0u16; 1024];
    // SAFETY: searches PATH for `name` (adding `.exe` if needed) into `buf`.
    let len = unsafe { SearchPathW(PCWSTR::null(), &name, w!(".exe"), Some(&mut buf), None) };
    if len > 0 && (len as usize) < buf.len() {
        HSTRING::from_wide(&buf[..len as usize])
    } else {
        name
    }
}

fn launch_low_integrity(
    prog: &str,
    args: &str,
    dir: Option<&Path>,
    opts: &RunTaskOptions,
) -> Result<(), ActionError> {
    let mut token = HANDLE::default();
    // SAFETY: opens our own process token and wraps it in OwnedHandle.
    unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_DUPLICATE | TOKEN_ADJUST_DEFAULT | TOKEN_QUERY | TOKEN_ASSIGN_PRIMARY,
            &mut token,
        )
    }
    .map_err(|e| ActionError::from_win(&e))?;
    let token = OwnedHandle(token);

    let mut restricted = HANDLE::default();
    // SAFETY: creates a restricted primary token with maximum privileges stripped.
    unsafe { CreateRestrictedToken(token.0, DISABLE_MAX_PRIVILEGE, None, None, None, &mut restricted) }
        .map_err(|e| ActionError::from_win(&e))?;
    let restricted = OwnedHandle(restricted);

    // S-1-16-4096 = Low Mandatory Level
    let mut low_sid = PSID::default();
    // SAFETY: converts static SID string; freed with LocalFree below.
    unsafe { ConvertStringSidToSidW(w!("S-1-16-4096"), &mut low_sid) }.map_err(|e| ActionError::from_win(&e))?;
    struct FreeSid(PSID);
    impl Drop for FreeSid {
        fn drop(&mut self) {
            // SAFETY: allocated by ConvertStringSidToSidW.
            unsafe {
                let _ = LocalFree(Some( windows::Win32::Foundation::HLOCAL(self.0.0) ));
            }
        }
    }
    let _free_sid = FreeSid(low_sid);

    let tml = TOKEN_MANDATORY_LABEL { Label: SID_AND_ATTRIBUTES { Sid: low_sid, Attributes: SE_GROUP_INTEGRITY as u32 } };
    // SAFETY: sets the restricted token's integrity level to Low.
    unsafe {
        SetTokenInformation(
            restricted.0,
            TokenIntegrityLevel,
            &tml as *const _ as *const c_void,
            size_of::<TOKEN_MANDATORY_LABEL>() as u32,
        )
    }
    .map_err(|e| ActionError::from_win(&e))?;

    let resolved = resolve_executable(prog);
    let full_cmd = if args.is_empty() {
        format!("\"{}\"", resolved.to_string_lossy())
    } else {
        format!("\"{}\" {args}", resolved.to_string_lossy())
    };
    let mut cmd_wide: Vec<u16> = full_cmd.encode_utf16().chain(std::iter::once(0)).collect();
    let dir_h = dir.map(|d| HSTRING::from(d.as_os_str())).unwrap_or_default();
    let si = STARTUPINFOW { cb: size_of::<STARTUPINFOW>() as u32, ..Default::default() };
    let mut pi = PROCESS_INFORMATION::default();
    let flags = if opts.efficiency_mode { IDLE_PRIORITY_CLASS } else { Default::default() };
    // SAFETY: `restricted` is a primary token, `cmd_wide` is NUL-terminated mutable UTF-16.
    unsafe {
        CreateProcessAsUserW(
            Some(restricted.0),
            &resolved,
            Some(PWSTR(cmd_wide.as_mut_ptr())),
            None,
            None,
            false,
            flags,
            None,
            if dir.is_some() { PCWSTR(dir_h.as_ptr()) } else { PCWSTR::null() },
            &si,
            &mut pi,
        )
    }
    .map_err(|e| ActionError::from_win(&e))?;
    let proc = OwnedHandle(pi.hProcess);
    // SAFETY: thread handle from CreateProcessAsUserW closed immediately.
    unsafe {
        let _ = CloseHandle(pi.hThread);
    }
    if opts.efficiency_mode {
        apply_efficiency_to_handle(proc.0);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_command_handles_quotes_and_arguments() {
        assert_eq!(split_command(""), None);
        assert_eq!(split_command("   "), None);
        assert_eq!(split_command("notepad"), Some(("notepad", "")));
        assert_eq!(split_command("cmd /k dir /w"), Some(("cmd", "/k dir /w")));
        assert_eq!(
            split_command(r#""C:\Program Files\App\app.exe" --flag 1"#),
            Some((r"C:\Program Files\App\app.exe", "--flag 1"))
        );
        assert_eq!(split_command(r#""C:\Tools\only.exe""#), Some((r"C:\Tools\only.exe", "")));
    }

    #[test]
    fn compat_env_combines_invoker_and_layer() {
        assert_eq!(compat_env(PrivilegeMode::Normal, CompatLayer::None), None);
        assert_eq!(compat_env(PrivilegeMode::ForceInvoker, CompatLayer::None).as_deref(), Some("RunAsInvoker"));
        assert_eq!(compat_env(PrivilegeMode::Normal, CompatLayer::Win7).as_deref(), Some("WIN7RTM"));
        assert_eq!(
            compat_env(PrivilegeMode::ForceInvoker, CompatLayer::HighDpiAware).as_deref(),
            Some("RunAsInvoker HIGHDPIAWARE")
        );
    }
}
