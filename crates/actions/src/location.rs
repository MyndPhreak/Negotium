use crate::ActionError;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Raw (unescaped) argument Explorer expects. Windows paths cannot contain quotes.
fn select_arg(path: &str) -> String {
    format!("/select,\"{path}\"")
}

/// `<Windows directory>\explorer.exe`.
fn explorer_in(windows: &Path) -> PathBuf {
    windows.join("explorer.exe")
}

/// The Explorer this starts: under the Windows directory Windows reports
/// ([`negotium_collector::windows_dir`]), never `%SystemRoot%`, which a same-user process can
/// point at a folder holding its own `explorer.exe` for an elevated copy to start.
fn explorer() -> PathBuf {
    explorer_in(Path::new(&negotium_collector::windows_dir()))
}

/// Opens File Explorer with the file selected.
pub fn open_file_location(path: &str) -> Result<(), ActionError> {
    Command::new(explorer())
        .raw_arg(select_arg(path))
        .spawn()
        .map(|_| ())
        .map_err(|e| ActionError::Other(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_arg_quotes_the_path_verbatim() {
        assert_eq!(select_arg(r"C:\Program Files\A, B\app.exe"), r#"/select,"C:\Program Files\A, B\app.exe""#);
    }

    #[test]
    fn explorer_is_in_the_windows_directory() {
        assert_eq!(explorer_in(Path::new(r"C:\Windows")), PathBuf::from(r"C:\Windows\explorer.exe"));
    }

    /// Set in the child process `explorer_ignores_system_root` starts.
    const CHILD: &str = "NEGOTIUM_TEST_EXPLORER_CHILD";

    /// A planted `%SystemRoot%` must not move Explorer: the child runs with it pointing at a temp
    /// folder and still reports `<real Windows dir>\explorer.exe`.
    #[test]
    fn explorer_ignores_system_root() {
        if std::env::var_os(CHILD).is_some() {
            println!("EXPLORER={}", explorer().display());
            return;
        }
        let real = explorer();
        assert!(real.is_file(), "{}", real.display());
        let fake = std::env::temp_dir().join("negotium-not-system-root");
        let out = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "location::tests::explorer_ignores_system_root", "--nocapture", "--test-threads=1"])
            .env(CHILD, "1")
            .env("SystemRoot", &fake)
            .env("windir", &fake)
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        let line = text
            .lines()
            .find_map(|l| l.split_once("EXPLORER=").map(|(_, p)| p))
            .unwrap_or_else(|| panic!("no EXPLORER line in {text}"));
        assert_eq!(Path::new(line), real);
        assert!(!Path::new(line).starts_with(&fake), "{line}");
    }
}
