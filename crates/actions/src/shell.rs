//! Terminate-rights probe and the pieces to restart Explorer without admin rights.
use crate::ActionError;
use std::time::Duration;

/// A duplicate of the shell's primary token, to start Explorer again with the user's own rights.
pub struct ShellToken(());

/// Whether this process can open `pid` with terminate rights. A process that already exited
/// counts as `true`: there is nothing to warn about.
pub fn can_terminate(_pid: u32, _create_time: i64) -> bool {
    todo!()
}

/// Copies the shell's primary token; `Ok(None)` when Negotium is not elevated (a plain start
/// then already runs with the user's rights).
pub fn capture_shell_token(_shell_pid: u32) -> Result<Option<ShellToken>, ActionError> {
    todo!()
}

/// Starts `<windows dir>\explorer.exe`, with `token` when given. Never searches PATH.
pub fn start_explorer(_token: Option<&ShellToken>) -> Result<(), ActionError> {
    todo!()
}

/// Waits up to `timeout` for a `Shell_TrayWnd` window.
pub fn wait_for_taskbar(_timeout: Duration) -> bool {
    todo!()
}
