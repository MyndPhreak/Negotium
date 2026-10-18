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

/// [`detect`] for the running exe and this machine's Program Files ([`program_files`]); without
/// a known folder nothing counts as installed.
pub fn current() -> Install {
    let exe = std::env::current_exe().unwrap_or_default();
    match program_files() {
        Some(program_files) => detect(&exe, &program_files),
        None => Install { exe, installed: false },
    }
}

/// The machine's Program Files folder (the 64-bit one for this 64-bit exe), from the shell's
/// known folder rather than `%ProgramFiles%`: an environment variable the user can change would
/// make a build tree look like the installed copy.
pub fn program_files() -> Option<PathBuf> {
    known_folder(&windows::Win32::UI::Shell::FOLDERID_ProgramFiles)
}

/// A shell known folder (`FOLDERID_*`), or `None` if Windows doesn't say.
pub fn known_folder(id: &windows::core::GUID) -> Option<PathBuf> {
    use std::ffi::c_void;
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{KNOWN_FOLDER_FLAG, SHGetKnownFolderPath};
    // SAFETY: returns a CoTaskMem string that is freed below.
    let p = unsafe { SHGetKnownFolderPath(id, KNOWN_FOLDER_FLAG(0), None) }.ok()?;
    // SAFETY: `p` is a valid NUL-terminated string until freed.
    let s = unsafe { p.to_string() }.ok();
    // SAFETY: allocated by SHGetKnownFolderPath; freed exactly once.
    unsafe { CoTaskMemFree(Some(p.0 as *const c_void)) };
    s.filter(|s| !s.is_empty()).map(PathBuf::from)
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

    /// Set in the child process `current_uses_the_known_folder` starts.
    const CHILD: &str = "NEGOTIUM_TEST_PROGRAM_FILES_CHILD";

    /// A changed `%ProgramFiles%` must not move "installed": the child runs with it pointing at a
    /// build tree and still reports the machine's real Program Files (the known folder).
    #[test]
    fn current_uses_the_known_folder() {
        if std::env::var_os(CHILD).is_some() {
            println!("PF={}", program_files().unwrap().display());
            return;
        }
        let real = program_files().expect("the Program Files known folder");
        assert!(real.is_absolute() && real.is_dir(), "{}", real.display());
        let fake = std::env::temp_dir().join("negotium-not-program-files");
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "install::tests::current_uses_the_known_folder", "--nocapture", "--test-threads=1"])
            .env(CHILD, "1")
            .env("ProgramFiles", &fake)
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        let line = text.lines().find_map(|l| l.split_once("PF=").map(|(_, pf)| pf)).unwrap_or_else(|| panic!("no PF line in {text}"));
        assert_eq!(Path::new(line), real);
        assert_ne!(Path::new(line), fake);
    }

    #[test]
    fn the_version_line_names_a_development_build() {
        assert_eq!(version_line(true), format!("Version {VERSION}"));
        assert_eq!(version_line(false), format!("Version {VERSION} (development build)"));
    }
}
