//! The installed app's Windows integration, as plain functions the app's `--integrate` command
//! (its `Machine`) calls: the Ctrl+Shift+Esc redirect (taskmgr.exe's Image File Execution Options
//! `Debugger` value), Start with Windows (the HKCU Run value `Negotium`) and admin mode (a
//! per-user Task Scheduler task with highest privileges), plus the user's SID that names the task.
//!
//! Every registry write and delete goes through the startup registry guard, which allows exactly
//! the two values (once the app's `main` unlocks it) and test-only names. Every root is in
//! [`Roots`], so tests point at a sandbox key, a made-up image name and a throwaway task folder,
//! never the real ones.
mod ifeo;
mod run;
mod sid;
mod tasks;

pub use ifeo::{ifeo_value, parse_ifeo, read_ifeo, set_ifeo};
pub use run::{RUN_VALUE, read_run, set_run};
pub use sid::{ADMINISTRATORS, current_user_sid, is_account_sid, is_admin_member, is_string_sid};
pub use tasks::{
    ARG_PLACEHOLDER, TaskStatus, TaskView, create_task, delete_task, delete_task_folder, join_args,
    read_task, start_task, task_logon, task_state, task_status, task_xml,
};

use crate::startup::registry::{Hive, IFEO_TASKMGR, RegKey, USER_RUN};

/// Where the integration lives. `system()` is the real machine; tests build their own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Roots {
    /// The image key whose `Debugger` value is the redirect (taskmgr.exe's, really).
    pub ifeo: RegKey,
    /// The key holding the Run value `Negotium` (HKCU's Run key, really).
    pub run: RegKey,
    /// The Task Scheduler folder under the root: one name, no backslashes (`Negotium`).
    pub task_folder: String,
}

/// The real task folder's name: `\Negotium\`.
pub const TASK_FOLDER: &str = "Negotium";

impl Roots {
    pub fn system() -> Self {
        Self {
            ifeo: RegKey::new(Hive::LocalMachine, IFEO_TASKMGR),
            run: RegKey::new(Hive::CurrentUser, USER_RUN),
            task_folder: TASK_FOLDER.to_string(),
        }
    }
}
