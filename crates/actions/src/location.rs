use crate::ActionError;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

/// Raw (unescaped) argument Explorer expects. Windows paths cannot contain quotes.
fn select_arg(path: &str) -> String {
    format!("/select,\"{path}\"")
}

/// Opens File Explorer with the file selected.
pub fn open_file_location(path: &str) -> Result<(), ActionError> {
    let root = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    Command::new(root.join("explorer.exe"))
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
}
