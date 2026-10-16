//! The program a Run command starts, for its name and publisher.

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

/// The quoted path, else the text up to and including the ".exe" that ends a word, else the first
/// word; environment variables expanded; a bare file name taken from System32. `None` for an
/// empty command.
pub fn exe_from_command(command: &str, system_root: &str) -> Option<String> {
    let c = command.trim();
    let lower = c.to_ascii_lowercase();
    let exe_end = lower
        .match_indices(".exe")
        .map(|(i, _)| i + 4)
        .find(|&end| c[end..].chars().next().is_none_or(char::is_whitespace));
    let raw = if let Some(rest) = c.strip_prefix('"') {
        rest.split('"').next().unwrap_or(rest)
    } else if let Some(end) = exe_end {
        &c[..end]
    } else {
        c.split_whitespace().next().unwrap_or("")
    };
    let path = expand_env(raw.trim(), |name| std::env::var(name).ok());
    if path.is_empty() {
        None
    } else if path.contains(['\\', '/', ':']) {
        Some(path)
    } else {
        Some(format!(
            "{}\\System32\\{path}",
            system_root.trim_end_matches('\\')
        ))
    }
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
}
