use crate::ActionError;
use std::process::Command;

/// Opens File Explorer with the file selected.
pub fn open_file_location(path: &str) -> Result<(), ActionError> {
    Command::new("explorer.exe")
        .arg(format!("/select,\"{path}\""))
        .spawn()
        .map(|_| ())
        .map_err(|e| ActionError::Other(e.to_string()))
}
