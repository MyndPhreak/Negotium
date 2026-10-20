//! Custom "Run new task" power launcher: supports Normal, Elevated (Administrator), Force Standard
//! User (`RunAsInvoker`), and Windows Sandbox (isolated disposable VM) launches, compatibility layers,
//! optional working directory, and starting in Efficiency Mode (EcoQoS + idle priority).
use crate::{ActionError, OwnedHandle};
use std::ffi::c_void;
use std::mem::size_of;
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::{HANDLE, HWND};
use windows::Win32::Storage::FileSystem::SearchPathW;
use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Threading::{
    IDLE_PRIORITY_CLASS, PROCESS_POWER_THROTTLING_CURRENT_VERSION, PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
    PROCESS_POWER_THROTTLING_STATE, ProcessPowerThrottling, SetPriorityClass, SetProcessInformation,
};
use windows::Win32::UI::Shell::{SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{HSTRING, PCWSTR, w};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PrivilegeMode {
    #[default]
    Normal,
    Elevated,
    ForceInvoker,
    WindowsSandbox,
}

impl PrivilegeMode {
    pub fn from_index(i: i32) -> Self {
        match i {
            1 => Self::Elevated,
            2 => Self::ForceInvoker,
            3 => Self::WindowsSandbox,
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
    let (prog, args) = match split_command(&opts.command) {
        Some((p, a)) => (p, a),
        None => {
            if opts.privilege == PrivilegeMode::WindowsSandbox {
                ("", "")
            } else {
                return Err(ActionError::Other("enter a program, path, or command to run".into()));
            }
        }
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
        PrivilegeMode::WindowsSandbox => launch_windows_sandbox(prog, args, dir_opt, opts.efficiency_mode),
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

/// Checks whether a path resides inside the Windows directory (e.g. C:\Windows\System32).
fn is_system_path(path: &Path) -> bool {
    std::env::var("SystemRoot").is_ok_and(|sysroot| path.starts_with(Path::new(&sysroot)))
}

/// Resolves an executable name or path to a host file if possible.
fn resolve_executable_path(prog: &str, working_dir: Option<&Path>) -> Option<PathBuf> {
    let p = Path::new(prog);
    if p.is_file() {
        return Some(p.to_path_buf());
    }
    if let Some(dir) = working_dir {
        let candidate = dir.join(p);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    let name = HSTRING::from(prog);
    let mut buf = vec![0u16; 1024];
    // SAFETY: searches PATH for `name` (adding `.exe` if needed) into `buf`.
    let len = unsafe { SearchPathW(PCWSTR::null(), &name, w!(".exe"), Some(&mut buf), None) };
    if len > 0 && (len as usize) < buf.len() {
        let found = PathBuf::from(HSTRING::from_wide(&buf[..len as usize]).to_string_lossy());
        if found.is_file() {
            return Some(found);
        }
    }
    None
}

/// Escapes XML special characters for inclusion in a .wsb configuration.
pub fn escape_xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

/// Builds the XML configuration content for a Windows Sandbox (.wsb) session.
///
/// If `prog` refers to an existing host file or directory outside the Windows system directory,
/// its parent directory (or `working_dir`) is mapped read-only into the sandbox at
/// `C:\Users\WDAGUtilityAccount\Desktop\Shared`, and `<LogonCommand>` invokes the application
/// inside the sandbox. If `prog` is a system command or built-in tool, it is invoked directly.
pub fn build_wsb_config(prog: &str, args: &str, working_dir: Option<&Path>) -> String {
    let mut xml = String::from("<Configuration>\n  <Networking>Default</Networking>\n");

    let host_file = if prog.is_empty() {
        None
    } else {
        resolve_executable_path(prog, working_dir)
    };

    let (mapped_host_dir, logon_command) = if let Some(ref file) = host_file {
        if is_system_path(file) {
            // Built-in system utility (e.g. C:\Windows\System32\notepad.exe)
            let cmd = if args.is_empty() {
                format!(r#"cmd.exe /c start "" "{prog}""#)
            } else {
                format!(r#"cmd.exe /c start "" "{prog}" {args}"#)
            };
            (None, cmd)
        } else {
            // Non-system executable on host: map parent folder read-only to Desktop\Shared
            let folder = working_dir.map(Path::to_path_buf).or_else(|| file.parent().map(Path::to_path_buf));
            let filename = file.file_name().and_then(|n| n.to_str()).unwrap_or(prog);
            let sandbox_path = format!(r"C:\Users\WDAGUtilityAccount\Desktop\Shared\{filename}");
            let cmd = if args.is_empty() {
                format!(r#"cmd.exe /c start "" "{sandbox_path}""#)
            } else {
                format!(r#"cmd.exe /c start "" "{sandbox_path}" {args}"#)
            };
            (folder, cmd)
        }
    } else if !prog.is_empty() {
        let folder = working_dir.map(Path::to_path_buf);
        let cmd = if args.is_empty() {
            format!(r#"cmd.exe /c start "" "{prog}""#)
        } else {
            format!(r#"cmd.exe /c start "" "{prog}" {args}"#)
        };
        (folder, cmd)
    } else {
        (working_dir.map(Path::to_path_buf), String::new())
    };

    if let Some(ref dir) = mapped_host_dir {
        xml.push_str("  <MappedFolders>\n    <MappedFolder>\n");
        xml.push_str(&format!("      <HostFolder>{}</HostFolder>\n", escape_xml(&dir.to_string_lossy())));
        xml.push_str("      <SandboxFolder>C:\\Users\\WDAGUtilityAccount\\Desktop\\Shared</SandboxFolder>\n");
        xml.push_str("      <ReadOnly>true</ReadOnly>\n    </MappedFolder>\n  </MappedFolders>\n");
    }

    if !logon_command.is_empty() {
        xml.push_str("  <LogonCommand>\n");
        xml.push_str(&format!("    <Command>{}</Command>\n", escape_xml(&logon_command)));
        xml.push_str("  </LogonCommand>\n");
    }

    xml.push_str("</Configuration>\n");
    xml
}

/// Locates the `WindowsSandbox.exe` binary on the host system.
fn find_windows_sandbox_exe() -> Option<PathBuf> {
    if let Ok(sysroot) = std::env::var("SystemRoot") {
        let p = Path::new(&sysroot).join("System32").join("WindowsSandbox.exe");
        if p.is_file() {
            return Some(p);
        }
    }
    let name = HSTRING::from("WindowsSandbox.exe");
    let mut buf = vec![0u16; 1024];
    // SAFETY: searches PATH for `WindowsSandbox.exe` into `buf`.
    let len = unsafe { SearchPathW(PCWSTR::null(), &name, None, Some(&mut buf), None) };
    if len > 0 && (len as usize) < buf.len() {
        let found = PathBuf::from(HSTRING::from_wide(&buf[..len as usize]).to_string_lossy());
        if found.is_file() {
            return Some(found);
        }
    }
    None
}

fn launch_windows_sandbox(
    prog: &str,
    args: &str,
    dir: Option<&Path>,
    efficiency_mode: bool,
) -> Result<(), ActionError> {
    let sandbox_exe = find_windows_sandbox_exe().ok_or_else(|| {
        ActionError::Other(
            "Windows Sandbox is not installed or enabled. To enable it, open 'Turn Windows features on or off' and check 'Windows Sandbox' (or run 'Enable-WindowsOptionalFeature -Online -FeatureName Containers-DisposableClientVM -All' in an Administrator PowerShell) and restart.".into(),
        )
    })?;

    let wsb_content = build_wsb_config(prog, args, dir);
    let temp_dir = std::env::temp_dir();
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let wsb_path = temp_dir.join(format!("negotium-task-{timestamp}.wsb"));

    std::fs::write(&wsb_path, wsb_content)
        .map_err(|e| ActionError::Other(format!("Failed to write sandbox configuration: {e}")))?;

    // Spawn Windows Sandbox with the generated .wsb configuration
    let mut cmd = std::process::Command::new(sandbox_exe);
    cmd.arg(&wsb_path);
    if efficiency_mode {
        cmd.creation_flags(IDLE_PRIORITY_CLASS.0);
    }
    let child = cmd
        .spawn()
        .map_err(|e| ActionError::Other(format!("Failed to start Windows Sandbox: {e}")))?;

    if efficiency_mode {
        let raw = HANDLE(child.as_raw_handle());
        apply_efficiency_to_handle(raw);
    }

    // Clean up temporary .wsb file after WindowsSandbox has loaded it
    let cleanup_path = wsb_path;
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(30));
        let _ = std::fs::remove_file(cleanup_path);
    });

    Ok(())
}

/// The official PowerShell command to enable the Windows Sandbox feature on Windows 10/11 Pro & Enterprise.
pub const ENABLE_SANDBOX_CMD: &str =
    "Enable-WindowsOptionalFeature -Online -FeatureName \"Containers-DisposableClientVM\" -All";

/// Returns whether `WindowsSandbox.exe` is installed on this machine.
pub fn is_windows_sandbox_available() -> bool {
    find_windows_sandbox_exe().is_some()
}

/// Copies the given UTF-8 text into the Windows clipboard as CF_UNICODETEXT.
pub fn copy_to_clipboard(text: &str) -> Result<(), ActionError> {
    let utf16: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = utf16.len() * std::mem::size_of::<u16>();

    // SAFETY: Allocates movable global memory for the clipboard data.
    let hmem = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes) }
        .map_err(|e| ActionError::Other(format!("Failed to allocate clipboard memory: {e}")))?;

    // SAFETY: Locks the global memory handle to write the UTF-16 string into it.
    let ptr = unsafe { GlobalLock(hmem) };
    if ptr.is_null() {
        return Err(ActionError::Other("Failed to lock clipboard memory".into()));
    }
    unsafe {
        std::ptr::copy_nonoverlapping(utf16.as_ptr() as *const u8, ptr as *mut u8, bytes);
        let _ = GlobalUnlock(hmem);
    }

    // CF_UNICODETEXT = 13
    const CF_UNICODETEXT: u32 = 13;

    // SAFETY: Opens the clipboard, empties it, and assigns the global memory.
    unsafe {
        OpenClipboard(None)
            .map_err(|e| ActionError::Other(format!("Failed to open clipboard: {e}")))?;
        let _ = EmptyClipboard();
        let res = SetClipboardData(CF_UNICODETEXT, Some(HANDLE(hmem.0)));
        let _ = CloseClipboard();
        if res.is_err() {
            return Err(ActionError::Other("Failed to set clipboard data".into()));
        }
    }
    Ok(())
}

/// Launches an elevated PowerShell window to run the command enabling Windows Sandbox.
pub fn enable_windows_sandbox_in_terminal(owner_hwnd: isize) -> Result<(), ActionError> {
    let _com = crate::startup::ComApartment::enter();
    let file = HSTRING::from("powershell.exe");
    let cmd = format!(
        "-NoExit -Command \"Write-Host 'Enabling Windows Sandbox feature...' -ForegroundColor Cyan; Write-Host '{ENABLE_SANDBOX_CMD}' -ForegroundColor Yellow; {ENABLE_SANDBOX_CMD}\""
    );
    let params = HSTRING::from(cmd);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        hwnd: HWND(owner_hwnd as *mut c_void),
        fMask: SEE_MASK_NOASYNC,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    // SAFETY: `info` is initialized and all HSTRING buffers outlive the call.
    unsafe { ShellExecuteExW(&mut info) }.map_err(|e| ActionError::from_win(&e))?;
    Ok(())
}

/// Opens a standard Windows File Open Dialog modal to `owner` to browse for an executable.
/// Returns the chosen path, or `None` if cancelled or on failure.
pub fn pick_executable(owner: isize) -> Option<std::path::PathBuf> {
    use windows::Win32::System::Com::{
        CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize,
    };
    use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
    use windows::Win32::UI::Shell::{
        FileOpenDialog, FOS_FILEMUSTEXIST, FOS_FORCEFILESYSTEM, IFileOpenDialog, SIGDN_FILESYSPATH,
    };

    // SAFETY: Initializes COM STA on this thread; balanced by CoUninitialize.
    let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let dialog: IFileOpenDialog = unsafe { CoCreateInstance(&FileOpenDialog, None, CLSCTX_ALL) }.ok()?;

    let filters = [
        COMDLG_FILTERSPEC {
            pszName: w!("Programs (*.exe;*.pif;*.com;*.bat;*.cmd)"),
            pszSpec: w!("*.exe;*.pif;*.com;*.bat;*.cmd"),
        },
        COMDLG_FILTERSPEC {
            pszName: w!("All Files (*.*)"),
            pszSpec: w!("*.*"),
        },
    ];

    // SAFETY: Configures and displays the modal File Open Dialog.
    unsafe {
        let _ = dialog.SetFileTypes(&filters);
        let _ = dialog.SetOptions(FOS_FORCEFILESYSTEM | FOS_FILEMUSTEXIST);
        let _ = dialog.SetTitle(w!("Browse"));
        let hwnd = HWND(owner as *mut c_void);
        if dialog.Show(Some(hwnd)).is_err() {
            CoUninitialize();
            return None;
        }
        let item = dialog.GetResult().ok()?;
        let display_name = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let path = std::path::PathBuf::from(display_name.to_string().ok()?);
        CoTaskMemFree(Some(display_name.as_ptr() as *const c_void));
        CoUninitialize();
        Some(path)
    }
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

    #[test]
    fn escape_xml_handles_special_characters() {
        assert_eq!(escape_xml("normal_text"), "normal_text");
        assert_eq!(escape_xml(r#"<a b="1" & '2'>"#), "&lt;a b=&quot;1&quot; &amp; &apos;2&apos;&gt;");
    }

    #[test]
    fn build_wsb_config_blank() {
        let xml = build_wsb_config("", "", None);
        assert!(xml.contains("<Configuration>"));
        assert!(xml.contains("<Networking>Default</Networking>"));
        assert!(!xml.contains("<MappedFolders>"));
        assert!(!xml.contains("<LogonCommand>"));
    }

    #[test]
    fn build_wsb_config_system_command() {
        let xml = build_wsb_config("cmd.exe", "/k echo hello", None);
        assert!(xml.contains("<Configuration>"));
        assert!(xml.contains(r#"cmd.exe /c start &quot;&quot; &quot;cmd.exe&quot; /k echo hello"#));
        assert!(!xml.contains("<MappedFolders>"));
    }

    #[test]
    fn build_wsb_config_with_mapped_folder() {
        let temp_dir = std::env::temp_dir();
        let test_file = temp_dir.join("negotium_test_app.exe");
        std::fs::write(&test_file, b"test").unwrap();

        let xml = build_wsb_config(test_file.to_str().unwrap(), "--run", None);
        let _ = std::fs::remove_file(&test_file);

        assert!(xml.contains("<MappedFolders>"));
        assert!(xml.contains("<ReadOnly>true</ReadOnly>"));
        assert!(xml.contains(r#"C:\Users\WDAGUtilityAccount\Desktop\Shared\negotium_test_app.exe"#));
        assert!(xml.contains("--run"));
    }

    #[test]
    fn enable_sandbox_command_is_valid_powershell() {
        assert!(ENABLE_SANDBOX_CMD.starts_with("Enable-WindowsOptionalFeature"));
        assert!(ENABLE_SANDBOX_CMD.contains("Containers-DisposableClientVM"));
    }

    #[test]
    fn clipboard_copy_does_not_panic() {
        let _ = copy_to_clipboard("negotium_test_clipboard");
    }
}
