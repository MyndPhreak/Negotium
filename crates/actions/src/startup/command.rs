//! The program a Run command starts, for its name and publisher.
use windows::Win32::Storage::FileSystem::GetDriveTypeW;
use windows::core::HSTRING;

/// Windows programs that run something else (a DLL, a script, a document, a package): their own
/// name and publisher would hide the real program and lend it Microsoft's name.
const HOSTS: &[&str] = &[
    "rundll32",
    "regsvr32",
    "cmd",
    "powershell",
    "pwsh",
    "wscript",
    "cscript",
    "mshta",
    "conhost",
    "explorer",
    "msiexec",
];

/// The quoted path; else a bare host program that starts the command (`cmd /c start x.exe` runs
/// `cmd`, not `x.exe`); else the text up to and including the ".exe" that ends a word; else the
/// first word. Environment variables are expanded; a bare file name is taken from System32, as an
/// ".exe" when it has no extension. `None` for an empty command.
pub fn exe_from_command(command: &str, system_root: &str) -> Option<String> {
    let c = command.trim();
    let first = c.split_whitespace().next().unwrap_or("");
    let lower = c.to_ascii_lowercase();
    let exe_end = lower
        .match_indices(".exe")
        .map(|(i, _)| i + 4)
        .find(|&end| c[end..].chars().next().is_none_or(char::is_whitespace));
    let raw = if let Some(rest) = c.strip_prefix('"') {
        rest.split('"').next().unwrap_or(rest)
    } else if is_bare(first) && is_host_program(first) {
        first
    } else if let Some(end) = exe_end {
        &c[..end]
    } else {
        first
    };
    let path = expand_env(raw.trim(), |name| std::env::var(name).ok());
    if path.is_empty() {
        None
    } else if !is_bare(&path) {
        Some(path)
    } else {
        let ext = if path.contains('.') { "" } else { ".exe" };
        Some(format!(
            "{}\\System32\\{path}{ext}",
            system_root.trim_end_matches('\\')
        ))
    }
}

/// A file name with no folder or drive.
fn is_bare(path: &str) -> bool {
    !path.contains(['\\', '/', ':'])
}

/// Whether `exe` is one of the `HOSTS` (by file name, with or without ".exe", any case).
pub(super) fn is_host_program(exe: &str) -> bool {
    let file = exe.rsplit(['\\', '/']).next().unwrap_or(exe);
    let stem = file
        .rsplit_once('.')
        .filter(|(_, ext)| ext.eq_ignore_ascii_case("exe"))
        .map_or(file, |(s, _)| s);
    HOSTS.iter().any(|h| h.eq_ignore_ascii_case(stem))
}

/// Whether `path` is on another machine: a UNC path (`\\server\share\…`, `\\?\UNC\…`, a
/// device path) or a drive that `drive_is_remote` says is a network drive. Other paths, relative
/// ones included, are local.
pub(super) fn is_remote_path(path: &str, drive_is_remote: impl Fn(char) -> bool) -> bool {
    let path = path.replace('/', "\\");
    let path = path.strip_prefix(r"\\?\").map_or(path.as_str(), |rest| {
        if rest
            .get(..4)
            .is_some_and(|p| p.eq_ignore_ascii_case(r"UNC\"))
        {
            r"\\" // `\\?\UNC\server\…`
        } else {
            rest
        }
    });
    if path.starts_with(r"\\") {
        return true;
    }
    let mut chars = path.chars();
    match (chars.next(), chars.next()) {
        (Some(drive), Some(':')) if drive.is_ascii_alphabetic() => drive_is_remote(drive),
        _ => false,
    }
}

/// `is_remote_path`, asking Windows whether a drive letter is a network drive (that reads the
/// drive mapping; the share itself isn't contacted).
pub(super) fn is_remote(path: &str) -> bool {
    is_remote_path(path, |drive| {
        let root = HSTRING::from(format!("{drive}:\\"));
        // SAFETY: a NUL-terminated root path that outlives the call.
        unsafe { GetDriveTypeW(&root) == DRIVE_REMOTE }
    })
}

/// `GetDriveTypeW`'s network drive (`DRIVE_REMOTE` lives in a windows feature this crate doesn't
/// otherwise need).
const DRIVE_REMOTE: u32 = 4;

/// Expands `%NAME%` through `lookup`; unknown or empty names stay as written.
pub fn expand_env(s: &str, lookup: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('%') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let name = &after[..end];
        match (!name.is_empty()).then(|| lookup(name)).flatten() {
            Some(value) => out.push_str(&value),
            None => {
                out.push('%');
                out.push_str(name);
                out.push('%');
            }
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoted_unquoted_and_bare_commands() {
        assert_eq!(
            exe_from_command(
                r#""C:\Program Files\App\app.exe" --background"#,
                r"C:\Windows"
            )
            .as_deref(),
            Some(r"C:\Program Files\App\app.exe")
        );
        assert_eq!(
            exe_from_command(r"C:\Program Files\App\App.EXE /min", r"C:\Windows").as_deref(),
            Some(r"C:\Program Files\App\App.EXE"),
            "unquoted paths with spaces end at .exe"
        );
        assert_eq!(
            exe_from_command("rundll32.exe shell32.dll,Control_RunDLL", r"C:\Windows\").as_deref(),
            Some(r"C:\Windows\System32\rundll32.exe")
        );
        assert_eq!(
            exe_from_command(r#""C:\unterminated"#, r"C:\Windows").as_deref(),
            Some(r"C:\unterminated")
        );
        assert_eq!(exe_from_command("   ", r"C:\Windows"), None);
        assert_eq!(exe_from_command("", r"C:\Windows"), None);
        assert_eq!(
            exe_from_command(r#""" --flag"#, r"C:\Windows"),
            None,
            "empty quotes"
        );
        assert_eq!(
            exe_from_command(r"C:\Tools\run.bat /q", r"C:\Windows").as_deref(),
            Some(r"C:\Tools\run.bat")
        );
    }

    #[test]
    fn the_exe_is_the_one_that_ends_a_word() {
        assert_eq!(
            exe_from_command(r"C:\Apps\my.exectutables\Tool App.exe --x", r"C:\Windows").as_deref(),
            Some(r"C:\Apps\my.exectutables\Tool App.exe")
        );
        assert_eq!(
            exe_from_command(r"C:\Apps\run.EXE", r"C:\Windows").as_deref(),
            Some(r"C:\Apps\run.EXE"),
            "at the end"
        );
        assert_eq!(
            exe_from_command(r"C:\Apps\my.exes\tool", r"C:\Windows").as_deref(),
            Some(r"C:\Apps\my.exes\tool"),
            "no word ends in .exe: the first word"
        );
    }

    #[test]
    fn a_bare_host_program_first_is_the_program_not_what_it_starts() {
        for (command, exe) in [
            (
                r"cmd /c start C:\Nope\x.exe",
                r"C:\Windows\System32\cmd.exe",
            ),
            ("CMD /c start ping.exe", r"C:\Windows\System32\CMD.exe"),
            (
                r"powershell -File C:\x\y.exe",
                r"C:\Windows\System32\powershell.exe",
            ),
            (
                "rundll32.exe shell32.dll,Control_RunDLL",
                r"C:\Windows\System32\rundll32.exe",
            ),
        ] {
            assert_eq!(
                exe_from_command(command, r"C:\Windows").as_deref(),
                Some(exe),
                "{command}"
            );
        }
        assert_eq!(
            exe_from_command(r"C:\Tools\cmd thing\app.exe /x", r"C:\Windows").as_deref(),
            Some(r"C:\Tools\cmd thing\app.exe"),
            "only a bare first word counts: a folder named cmd is not the host"
        );
        assert_eq!(
            exe_from_command("tool /x", r"C:\Windows").as_deref(),
            Some(r"C:\Windows\System32\tool.exe"),
            "a bare name without an extension is looked up as an .exe"
        );
    }

    #[test]
    fn forward_slashes_and_drive_letters_mark_a_path() {
        assert_eq!(
            exe_from_command("C:/x/y.exe -a", r"C:\Windows").as_deref(),
            Some("C:/x/y.exe")
        );
        assert_eq!(
            exe_from_command("D:tool.exe", r"C:\Windows").as_deref(),
            Some("D:tool.exe")
        );
        assert_eq!(
            exe_from_command("tool.exe", r"C:\Windows").as_deref(),
            Some(r"C:\Windows\System32\tool.exe")
        );
    }

    #[test]
    fn host_programs_are_recognised_by_file_name() {
        for exe in [
            r"C:\Windows\System32\rundll32.exe",
            r"C:\WINDOWS\system32\WindowsPowerShell\v1.0\PowerShell.EXE",
            r"C:\Program Files\PowerShell\7\pwsh.exe",
            r"C:\Windows\System32\cmd",
            "C:/Windows/explorer.exe",
            r"C:\Windows\System32\msiexec.exe",
            r"C:\Windows\SysWOW64\wscript.exe",
            r"C:\Windows\System32\cscript.exe",
            r"C:\Windows\System32\mshta.exe",
            r"C:\Windows\System32\conhost.exe",
            r"C:\Windows\System32\regsvr32.exe",
        ] {
            assert!(is_host_program(exe), "{exe}");
        }
        for exe in [
            r"C:\Windows\System32\PING.EXE",
            r"C:\Apps\cmdtool.exe",
            r"C:\Apps\explorer.exe.bak",
            r"C:\cmd\app.exe",
        ] {
            assert!(!is_host_program(exe), "{exe}");
        }
    }

    #[test]
    fn environment_variables_expand_and_unknown_ones_stay() {
        let lookup =
            |name: &str| (name.eq_ignore_ascii_case("windir")).then(|| r"C:\Windows".to_string());
        assert_eq!(
            expand_env(r"%windir%\system32\x.exe", lookup),
            r"C:\Windows\system32\x.exe"
        );
        assert_eq!(expand_env(r"%NOPE%\x.exe", lookup), r"%NOPE%\x.exe");
        assert_eq!(expand_env("100%", lookup), "100%");
        assert_eq!(expand_env("%%", lookup), "%%");
        let real = exe_from_command(
            r"%SystemRoot%\system32\SecurityHealthSystray.exe",
            r"C:\Windows",
        )
        .unwrap();
        assert!(
            !real.contains('%') && real.ends_with(r"\system32\SecurityHealthSystray.exe"),
            "{real}"
        );
    }

    #[test]
    fn network_paths_are_remote_and_local_ones_are_not() {
        let mapped = |d: char| d.eq_ignore_ascii_case(&'Z');
        for path in [
            r"\\server\share\app.exe",
            "//server/share/app.exe",
            r"\\?\UNC\server\share\app.exe",
            r"\\?\unc\server\share\app.exe",
            r"\\.\pipe\x",
            r"Z:\Apps\app.exe",
            r"z:\app.exe",
            r"\\?\Z:\app.exe",
        ] {
            assert!(is_remote_path(path, mapped), "{path}");
        }
        for path in [
            r"C:\Windows\System32\PING.EXE",
            r"\\?\C:\Apps\app.exe",
            r"\Apps\app.exe",
            r"Apps\app.exe",
            r"\\?\é\app.exe",
            "",
        ] {
            assert!(!is_remote_path(path, mapped), "{path}");
        }
        assert!(
            !is_remote(r"C:\Windows\System32\PING.EXE"),
            "the system drive is local"
        );
        assert!(is_remote(r"\\server\share\app.exe"));
    }
}
