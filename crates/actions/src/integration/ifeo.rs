//! The Ctrl+Shift+Esc redirect: an image's IFEO `Debugger` value. Windows starts
//! `<Debugger value> <original command line>` with no application name, so the exe must be quoted
//! (Program Files has a space) or Windows would try `C:\Program` first.
use super::Roots;
use crate::ActionError;
use crate::startup::registry;
use std::path::{Path, PathBuf};

const DEBUGGER: &str = "Debugger";

/// The value naming `exe`: the path in double quotes, nothing else.
pub fn ifeo_value(exe: &Path) -> String {
    format!("\"{}\"", exe.display())
}

/// The exe a `Debugger` value names, quoted or not, without any arguments after it (another
/// tool's value may carry some). Unquoted, the path runs to the first `.exe` that ends a word (so
/// an unquoted path with spaces is still read whole), else to the first whitespace. `None` for an
/// empty value or an unterminated quote.
pub fn parse_ifeo(value: &str) -> Option<PathBuf> {
    let value = value.trim();
    let path = if let Some(rest) = value.strip_prefix('"') {
        &rest[..rest.find('"')?]
    } else {
        let lower = value.to_ascii_lowercase();
        let exe_end = lower
            .match_indices(".exe")
            .map(|(i, _)| i + 4)
            .find(|&end| value[end..].chars().next().is_none_or(char::is_whitespace));
        match exe_end {
            Some(end) => &value[..end],
            None => value.split_whitespace().next().unwrap_or(""),
        }
    };
    let path = path.trim();
    (!path.is_empty()).then(|| PathBuf::from(path))
}

/// Sets (`Some`: `ifeo_value(exe)`) or removes (`None`) the `Debugger` value of `roots.ifeo`.
pub fn set_ifeo(roots: &Roots, exe: Option<&Path>) -> Result<(), ActionError> {
    match exe {
        Some(exe) => registry::set_string(&roots.ifeo, DEBUGGER, &ifeo_value(exe)),
        None => registry::delete_value(&roots.ifeo, DEBUGGER),
    }
}

/// The exe the `Debugger` value of `roots.ifeo` names (`parse_ifeo`), unquoted.
pub fn read_ifeo(roots: &Roots) -> Option<PathBuf> {
    registry::value(&roots.ifeo, DEBUGGER)?.as_str().and_then(parse_ifeo)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::startup::registry::{Hive, RegKey, TEST_ROOT};

    const EXE: &str = r"C:\Program Files\Negotium\negotium.exe";

    #[test]
    fn the_value_is_the_quoted_exe_and_reads_back_unquoted() {
        assert_eq!(ifeo_value(Path::new(EXE)), format!("\"{EXE}\""));
        assert_eq!(parse_ifeo(&ifeo_value(Path::new(EXE))), Some(PathBuf::from(EXE)));
    }

    #[test]
    fn values_parse_quoted_or_not_ignoring_arguments() {
        let upper = EXE.to_uppercase();
        let procexp = r"C:\Tools\procexp64.exe";
        let cases: Vec<(String, Option<&str>)> = vec![
            (format!("\"{EXE}\""), Some(EXE)),
            (format!("  \"{EXE}\" --flag \"x y\" "), Some(EXE)),
            (EXE.to_string(), Some(EXE)),
            (format!("{EXE} --flag"), Some(EXE)),
            (format!("{upper} /e"), Some(&upper)),
            (procexp.to_string(), Some(procexp)),
            (format!("\"{procexp}\""), Some(procexp)),
            // Not ending in .exe: the first word.
            (r"C:\x\vsjitdebugger -p".to_string(), Some(r"C:\x\vsjitdebugger")),
            // ".exe" inside a word doesn't end the path.
            (r"C:\a.exe.d\b.exe arg".to_string(), Some(r"C:\a.exe.d\b.exe")),
            (String::new(), None),
            ("   ".to_string(), None),
            ("\"\"".to_string(), None),
            (format!("\"{EXE}"), None),
        ];
        for (value, expected) in cases {
            let parsed = parse_ifeo(&value).map(|p| p.display().to_string());
            assert_eq!(parsed.as_deref(), expected, "{value:?}");
        }
    }

    #[test]
    fn set_read_and_remove_in_the_sandbox() {
        // The real code path against a sandbox key: no elevation, no real image.
        let base = RegKey::new(Hive::CurrentUser, &format!(r"{TEST_ROOT}\ifeo-{}", std::process::id()));
        let _ = registry::delete_test_tree(&base);
        let roots = Roots { ifeo: base.child("image.exe"), ..Roots::system() };
        assert_eq!(read_ifeo(&roots), None);
        set_ifeo(&roots, Some(Path::new(EXE))).unwrap();
        assert_eq!(
            registry::value(&roots.ifeo, DEBUGGER).and_then(|v| v.as_str().map(str::to_string)),
            Some(format!("\"{EXE}\""))
        );
        assert_eq!(read_ifeo(&roots), Some(PathBuf::from(EXE)));
        set_ifeo(&roots, None).unwrap();
        assert_eq!(read_ifeo(&roots), None);
        set_ifeo(&roots, None).unwrap();
        registry::delete_test_tree(&base).unwrap();
    }

    #[test]
    fn the_real_redirect_is_guarded() {
        // Checked through the pure guard only: no test ever attempts the real value.
        let roots = Roots::system();
        assert_eq!(roots.ifeo, RegKey::new(Hive::LocalMachine, registry::IFEO_TASKMGR));
        assert!(!registry::may_write_value(&roots.ifeo, DEBUGGER), "locked in tests");
    }

    /// The real HKLM IFEO key with a made-up image name (never taskmgr.exe); the key it creates
    /// is deleted again.
    #[test]
    #[ignore = "needs an elevated terminal"]
    fn set_read_and_remove_a_test_images_redirect() {
        let image = registry::test_ifeo_image();
        let key = RegKey::new(Hive::LocalMachine, &format!(r"{}\{image}", registry::IFEO_ROOT));
        assert!(!key.path.to_ascii_lowercase().contains("taskmgr"));
        let roots = Roots { ifeo: key.clone(), ..Roots::system() };
        let exe = Path::new(r"C:\Windows\System32\cmd.exe");
        let result = (|| {
            set_ifeo(&roots, Some(exe))?;
            assert_eq!(read_ifeo(&roots).as_deref(), Some(exe));
            set_ifeo(&roots, None)?;
            assert_eq!(read_ifeo(&roots), None);
            Ok::<(), ActionError>(())
        })();
        let cleanup = registry::delete_test_ifeo_key(&key);
        result.unwrap();
        cleanup.unwrap();
        let left = registry::subkeys(&RegKey::new(Hive::LocalMachine, registry::IFEO_ROOT)).unwrap();
        assert!(left.iter().all(|k| !k.eq_ignore_ascii_case(&image)), "the test key is gone");
    }
}
