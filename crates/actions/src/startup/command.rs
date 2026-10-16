//! The program a Run command starts, for its name and publisher.

/// The quoted path, else the text up to and including ".exe", else the first word; environment
/// variables expanded; a bare file name taken from System32. `None` for an empty command.
pub fn exe_from_command(command: &str, system_root: &str) -> Option<String> {
    let c = command.trim();
    let raw = if let Some(rest) = c.strip_prefix('"') {
        rest.split('"').next().unwrap_or(rest)
    } else if let Some(end) = c.to_ascii_lowercase().find(".exe") {
        &c[..end + 4]
    } else {
        c.split_whitespace().next().unwrap_or("")
    };
    let path = expand_env(raw.trim(), |name| std::env::var(name).ok());
    if path.is_empty() {
        None
    } else if path.contains('\\') {
        Some(path)
    } else {
        Some(format!("{}\\System32\\{path}", system_root.trim_end_matches('\\')))
    }
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
            exe_from_command(r#""C:\Program Files\App\app.exe" --background"#, r"C:\Windows").as_deref(),
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
        assert_eq!(exe_from_command(r#""C:\unterminated"#, r"C:\Windows").as_deref(), Some(r"C:\unterminated"));
        assert_eq!(exe_from_command("   ", r"C:\Windows"), None);
        assert_eq!(exe_from_command("", r"C:\Windows"), None);
        assert_eq!(exe_from_command(r#""" --flag"#, r"C:\Windows"), None, "empty quotes");
        assert_eq!(exe_from_command(r"C:\Tools\run.bat /q", r"C:\Windows").as_deref(), Some(r"C:\Tools\run.bat"));
    }

    #[test]
    fn environment_variables_expand_and_unknown_ones_stay() {
        let lookup = |name: &str| (name.eq_ignore_ascii_case("windir")).then(|| r"C:\Windows".to_string());
        assert_eq!(expand_env(r"%windir%\system32\x.exe", lookup), r"C:\Windows\system32\x.exe");
        assert_eq!(expand_env(r"%NOPE%\x.exe", lookup), r"%NOPE%\x.exe");
        assert_eq!(expand_env("100%", lookup), "100%");
        assert_eq!(expand_env("%%", lookup), "%%");
        let real = exe_from_command(r"%SystemRoot%\system32\SecurityHealthSystray.exe", r"C:\Windows").unwrap();
        assert!(!real.contains('%') && real.ends_with(r"\system32\SecurityHealthSystray.exe"), "{real}");
    }
}
