use crate::ActionError;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use windows::Win32::UI::Shell::{SHOP_FILEPATH, SHObjectProperties, ShellExecuteW};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{HSTRING, PCWSTR, w};

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

/// A web search for `file_name` (Bing, as Task Manager's Search online).
pub fn search_url(file_name: &str) -> String {
    let query: String = file_name
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect();
    format!("https://www.bing.com/search?q={query}")
}

/// Opens `url` in the default browser.
pub fn open_url(url: &str) -> Result<(), ActionError> {
    // SAFETY: plain Win32 call with valid strings.
    let r = unsafe { ShellExecuteW(None, w!("open"), &HSTRING::from(url), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL) };
    // ShellExecute reports success as a value above 32.
    if r.0 as usize > 32 { Ok(()) } else { Err(ActionError::Other(format!("Windows could not open the link (code {})", r.0 as usize))) }
}

/// Opens the Windows file Properties dialog for `path`. The dialog runs on its own shell thread.
pub fn show_properties(path: &str) -> Result<(), ActionError> {
    // SAFETY: plain Win32 call with a valid string.
    if unsafe { SHObjectProperties(None, SHOP_FILEPATH, &HSTRING::from(path), PCWSTR::null()) }.as_bool() {
        Ok(())
    } else {
        Err(ActionError::Other("Windows could not open the file properties".into()))
    }
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

    #[test]
    fn search_urls_are_encoded() {
        assert_eq!(search_url("my app.exe"), "https://www.bing.com/search?q=my%20app.exe");
        assert_eq!(search_url("a&b#.exe"), "https://www.bing.com/search?q=a%26b%23.exe");
    }
}
