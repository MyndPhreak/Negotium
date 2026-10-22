//! The window picker: one dimmed overlay per monitor; a click on a window ends its app.
//! This file holds the session (what is hovered, the snapshot, what a click does) and runs the
//! end job; `overlay` is the Win32 side (windows, input, drawing, dialogs).
pub mod apps;
pub mod click;
pub mod label;
pub mod scene;
pub mod target;
pub mod win;

use std::time::Duration;

/// What the picker needs from the rest of the app.
pub struct PickerCtx {
    pub elevated: bool,
}

/// Opens the picker on every monitor. Does nothing when it is already open (spec goal 6).
/// UI thread only.
pub fn open(_ctx: &PickerCtx) {
    todo!()
}

/// Whether the picker is open on this thread.
pub fn is_open() -> bool {
    todo!()
}

const NO_ADMIN_RESTART: &str =
    "Negotium cannot start the taskbar again without admin rights, so it did not end File Explorer.";
const TASKBAR_DID_NOT_START: &str = "The taskbar did not start again. Press Ctrl+Shift+Esc, then run explorer.exe.";

/// The steps of ending the shell and starting the taskbar again; a seam so their order is tested.
trait ShellRestart {
    type Token;
    /// The shell's token (`capture_shell_token`); called only when Negotium is elevated.
    fn capture(&mut self) -> Option<Self::Token>;
    /// Runs the end job; returns its result text ("" on full success).
    fn run_job(&mut self) -> String;
    /// Waits up to 5 s for the old taskbar window to go.
    fn wait_old_taskbar_gone(&mut self);
    /// Whether a taskbar window exists within `timeout`.
    fn wait_taskbar(&mut self, timeout: Duration) -> bool;
    /// Starts `explorer.exe`, with the shell's token when given.
    fn start(&mut self, token: Option<&Self::Token>) -> bool;
}

/// Ends the shell-hosted app and makes sure a taskbar comes back. Returns the text to show
/// ("" when there is nothing to say).
fn end_with_restart<S: ShellRestart>(_s: &mut S, _elevated: bool) -> String {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Records each step; answers from its fields.
    #[derive(Default)]
    struct Fake {
        log: Vec<String>,
        token: Option<u8>,
        job: String,
        auto_restart: bool,
        starts: bool,
        comes_up: bool,
    }

    impl ShellRestart for Fake {
        type Token = u8;
        fn capture(&mut self) -> Option<u8> {
            self.log.push("capture".into());
            self.token
        }
        fn run_job(&mut self) -> String {
            self.log.push("run".into());
            self.job.clone()
        }
        fn wait_old_taskbar_gone(&mut self) {
            self.log.push("old gone".into());
        }
        fn wait_taskbar(&mut self, timeout: Duration) -> bool {
            self.log.push(format!("wait {}", timeout.as_secs()));
            if timeout == Duration::from_secs(3) { self.auto_restart } else { self.comes_up }
        }
        fn start(&mut self, token: Option<&u8>) -> bool {
            self.log.push(format!("start {token:?}"));
            self.starts
        }
    }

    fn fake() -> Fake {
        Fake { token: Some(7), starts: true, comes_up: true, ..Fake::default() }
    }

    #[test]
    fn elevated_without_the_shell_token_does_not_end_explorer() {
        let mut f = Fake { token: None, ..fake() };
        assert_eq!(end_with_restart(&mut f, true), NO_ADMIN_RESTART);
        assert_eq!(f.log, ["capture"]);
    }

    #[test]
    fn elevated_starts_explorer_with_the_shell_token_after_the_old_taskbar_goes() {
        let mut f = fake();
        assert_eq!(end_with_restart(&mut f, true), "");
        assert_eq!(f.log, ["capture", "run", "old gone", "wait 3", "start Some(7)", "wait 10"]);
    }

    #[test]
    fn not_elevated_takes_no_token_and_starts_a_plain_explorer() {
        let mut f = fake();
        assert_eq!(end_with_restart(&mut f, false), "");
        assert_eq!(f.log, ["run", "old gone", "wait 3", "start None", "wait 10"]);
    }

    #[test]
    fn windows_restarting_the_shell_itself_starts_no_second_explorer() {
        let mut f = Fake { auto_restart: true, ..fake() };
        assert_eq!(end_with_restart(&mut f, true), "");
        assert_eq!(f.log, ["capture", "run", "old gone", "wait 3"]);
    }

    #[test]
    fn a_taskbar_that_does_not_come_back_is_reported() {
        let mut f = Fake { comes_up: false, ..fake() };
        assert_eq!(end_with_restart(&mut f, false), TASKBAR_DID_NOT_START);
        let mut f = Fake { starts: false, ..fake() };
        assert_eq!(end_with_restart(&mut f, false), TASKBAR_DID_NOT_START);
        assert_eq!(f.log, ["run", "old gone", "wait 3", "start None"]);
    }

    #[test]
    fn a_job_failure_is_kept_beside_the_taskbar_text() {
        let mut f = Fake { job: "Access denied.".into(), comes_up: false, ..fake() };
        assert_eq!(end_with_restart(&mut f, false), format!("Access denied.\n\n{TASKBAR_DID_NOT_START}"));
        let mut f = Fake { job: "Access denied.".into(), ..fake() };
        assert_eq!(end_with_restart(&mut f, false), "Access denied.");
    }
}
