//! Whether this process is the installed copy (`<Program Files>\Negotium\negotium.exe`) or a
//! development build (`target\release`, a portable copy). Admin mode, the Ctrl+Shift+Esc
//! redirect, Start with Windows and updates are offered for the installed copy only: their
//! registry values and tasks point at the installed exe, never at a build tree.
use std::path::{Path, PathBuf};

/// The running version, shown on the Settings page and compared with the latest release.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Install {
    /// The running executable.
    pub exe: PathBuf,
    /// `exe` is `<Program Files>\Negotium\negotium.exe`.
    pub installed: bool,
}

/// Whether `exe` is the installed copy under `program_files`. Windows paths are case-insensitive;
/// a lookalike folder (`Negotium2`) or another file name is not the installed copy.
pub fn detect(exe: &Path, program_files: &Path) -> Install {
    let expected = program_files.join("Negotium").join("negotium.exe");
    let lower = |p: &Path| p.to_string_lossy().replace('/', "\\").trim_end_matches('\\').to_lowercase();
    Install { exe: exe.to_path_buf(), installed: lower(exe) == lower(&expected) }
}

/// [`detect`] for the running exe and this machine's Program Files (`%ProgramFiles%`, which is
/// the 64-bit folder for this 64-bit exe).
pub fn current() -> Install {
    let exe = std::env::current_exe().unwrap_or_default();
    let program_files = std::env::var_os("ProgramFiles").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Program Files"));
    detect(&exe, &program_files)
}

/// The Settings page's version line.
pub fn version_line(installed: bool) -> String {
    if installed { format!("Version {VERSION}") } else { format!("Version {VERSION} (development build)") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_recognises_only_the_program_files_exe() {
        let pf = Path::new(r"C:\Program Files");
        for (exe, want) in [
            (r"C:\Program Files\Negotium\negotium.exe", true),
            (r"c:\program files\negotium\NEGOTIUM.EXE", true),
            (r"C:\Users\me\DEV\Negotium\target\release\negotium.exe", false),
            (r"C:\Program Files\Negotium2\negotium.exe", false),
            (r"C:\Program Files\Negotium\other.exe", false),
            (r"C:\Program Files\Negotium\sub\negotium.exe", false),
        ] {
            assert_eq!(detect(Path::new(exe), pf).installed, want, "{exe}");
        }
    }

    #[test]
    fn the_version_line_names_a_development_build() {
        assert_eq!(version_line(true), format!("Version {VERSION}"));
        assert_eq!(version_line(false), format!("Version {VERSION} (development build)"));
    }
}
