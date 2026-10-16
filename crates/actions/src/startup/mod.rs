//! Startup apps (spec §5.3): Run-key entries, the Startup folders and packaged startup tasks,
//! with the enabled state Task Manager uses. Listing and toggling call Windows: run them on an
//! action thread. Only StartupApproved values (and packaged task states) are ever written; Run
//! values and shortcuts are never edited or deleted.
mod approved;
mod command;
mod folders;
pub mod packaged;
pub mod registry;

pub use approved::{APPROVED_LEN, encode_approved, filetime_now, is_enabled};
pub use command::{exe_from_command, expand_env};
pub use folders::{ComApartment, resolve_shortcut, write_shortcut};

use crate::ActionError;
use command::{is_host_program, is_remote};
use negotium_collector::VersionInfo;
use registry::{Hive, RegKey, RegValue};
use std::path::PathBuf;

const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN32: &str = r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Run";
const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StartupSource {
    UserRun,
    MachineRun,
    /// HKLM\SOFTWARE\WOW6432Node\…\Run (32-bit programs).
    MachineRun32,
    /// The per-user Startup folder (state under HKCU …\StartupApproved\StartupFolder).
    UserFolder,
    /// The all-users Startup folder (state under HKLM …\StartupApproved\StartupFolder).
    MachineFolder,
    /// A packaged app's startup task (state under HKCU …\AppModel\SystemAppData).
    Packaged,
}

impl StartupSource {
    /// Changing these entries writes under HKLM: admin only.
    pub fn needs_admin(self) -> bool {
        matches!(
            self,
            StartupSource::MachineRun | StartupSource::MachineRun32 | StartupSource::MachineFolder
        )
    }

    pub fn label(self) -> &'static str {
        match self {
            StartupSource::UserRun => "Registry (you)",
            StartupSource::MachineRun => "Registry (all users)",
            StartupSource::MachineRun32 => "Registry (all users, 32-bit)",
            StartupSource::UserFolder => "Startup folder (you)",
            StartupSource::MachineFolder => "Startup folder (all users)",
            StartupSource::Packaged => "Packaged app",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct StartupEntry {
    pub source: StartupSource,
    /// What identifies it for a toggle: the Run value name, the shortcut's file name, or
    /// "PackageFamilyName\TaskId".
    pub key: String,
    /// The program's description, else `key` (or the shortcut's name).
    pub name: String,
    pub publisher: Option<String>,
    pub enabled: bool,
    /// False when Windows policy fixes the state.
    pub changeable: bool,
    pub command: String,
    /// Where the entry lives: a registry key or a folder.
    pub location: String,
}

#[derive(Clone, Debug)]
pub struct StartupRoots {
    pub user_run: RegKey,
    pub machine_run: RegKey,
    pub machine_run32: RegKey,
    /// The StartupApproved parents; `Run`, `Run32` and `StartupFolder` are their children.
    pub user_approved: RegKey,
    pub machine_approved: RegKey,
    pub system_root: String,
    /// The per-user and all-users Startup folders; `None` skips a folder.
    pub user_folder: Option<PathBuf>,
    pub machine_folder: Option<PathBuf>,
    /// The packaged apps' task states: `<PackageFamilyName>\<TaskId>` keys with a `State`.
    pub packaged: RegKey,
}

fn system_root() -> String {
    std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string())
}

impl StartupRoots {
    /// The real lists.
    pub fn system() -> Self {
        let (user_folder, machine_folder) = folders::startup_folders();
        Self {
            user_run: RegKey::new(Hive::CurrentUser, RUN),
            machine_run: RegKey::new(Hive::LocalMachine, RUN),
            machine_run32: RegKey::new(Hive::LocalMachine, RUN32),
            user_approved: RegKey::new(Hive::CurrentUser, APPROVED),
            machine_approved: RegKey::new(Hive::LocalMachine, APPROVED),
            system_root: system_root(),
            user_folder,
            machine_folder,
            packaged: RegKey::new(Hive::CurrentUser, packaged::SYSTEM_APP_DATA),
        }
    }

    /// Every list below HKCU\`base`, for tests: the user's real entries are never touched. No
    /// Startup folders until `with_folders`.
    pub fn under(base: &str) -> Self {
        let key = |name: &str| RegKey::new(Hive::CurrentUser, &format!("{base}\\{name}"));
        Self {
            user_run: key("UserRun"),
            machine_run: key("MachineRun"),
            machine_run32: key("MachineRun32"),
            user_approved: key("UserApproved"),
            machine_approved: key("MachineApproved"),
            system_root: system_root(),
            user_folder: None,
            machine_folder: None,
            packaged: key("Packaged"),
        }
    }

    /// These folders as the per-user and all-users Startup folders (tests use temp folders).
    pub fn with_folders(mut self, user: PathBuf, machine: PathBuf) -> Self {
        self.user_folder = Some(user);
        self.machine_folder = Some(machine);
        self
    }

    /// The StartupApproved key holding `source`'s states; `None` for packaged tasks, whose state
    /// is their own `State` value.
    fn approved_key(&self, source: StartupSource) -> Option<RegKey> {
        Some(match source {
            StartupSource::UserRun => self.user_approved.child("Run"),
            StartupSource::MachineRun => self.machine_approved.child("Run"),
            StartupSource::MachineRun32 => self.machine_approved.child("Run32"),
            StartupSource::UserFolder => self.user_approved.child("StartupFolder"),
            StartupSource::MachineFolder => self.machine_approved.child("StartupFolder"),
            StartupSource::Packaged => return None,
        })
    }
}

/// The name and publisher shown for a startup program: its version resource's FileDescription and
/// CompanyName (one read of the resource), else `fallback` and no publisher. Shared by every
/// source that starts a program file (the Run keys and the Startup folders).
/// A host program (`rundll32`, `cmd`, `powershell`, …) counts as unknown: its own name and
/// publisher would hide what it runs. So does a program on a network share or mapped drive: its
/// resource isn't read, so a share that doesn't answer can't hold up the list.
fn describe_program(exe: Option<&str>, fallback: &str) -> (String, Option<String>) {
    let info = exe
        .filter(|e| !is_host_program(e) && !is_remote(e))
        .and_then(VersionInfo::read);
    let info = info.as_ref();
    let name = info
        .and_then(|i| i.string("FileDescription"))
        .unwrap_or_else(|| fallback.to_string());
    (name, info.and_then(|i| i.string("CompanyName")))
}

/// Lines in `list_startup`'s errors that report one value left out (it isn't a command, or it is
/// over 1 MB) start with this; every other line is a list, folder or key that couldn't be read.
pub const SKIPPED: &str = "Skipped ";

/// Whether an error line from `list_startup` is a read error rather than a skipped value.
pub fn is_read_error(line: &str) -> bool {
    !line.starts_with(SKIPPED)
}

fn skipped(name: &str, place: &str, value: &RegValue) -> String {
    let why = if *value == RegValue::TooLarge {
        "it is larger than 1 MB"
    } else {
        "it isn't a command"
    };
    format!("{SKIPPED}{name} in {place}: {why}")
}

/// Enabled unless a StartupApproved value (names compare case-insensitively) says disabled.
fn enabled_in(states: &[(String, RegValue)], name: &str) -> bool {
    states
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .and_then(|(_, v)| v.binary())
        .is_none_or(is_enabled)
}

/// The StartupApproved values of `source`'s list (none for packaged tasks). A key that can't be
/// read adds a line to `errors` and reads as empty (every entry enabled); a value over 1 MB adds a
/// "Skipped" line.
fn approved_states(
    roots: &StartupRoots,
    source: StartupSource,
    errors: &mut Vec<String>,
) -> Vec<(String, RegValue)> {
    let Some(key) = roots.approved_key(source) else {
        return Vec::new();
    };
    match registry::values(&key) {
        Ok(states) => {
            for (name, value) in &states {
                if *value == RegValue::TooLarge {
                    errors.push(skipped(name, &key.display(), value));
                }
            }
            states
        }
        Err(e) => {
            errors.push(format!("Couldn't read {}: {e}", key.display()));
            Vec::new()
        }
    }
}

/// Something that starts a program file (a Run value, a file in a Startup folder).
struct ProgramEntry<'a> {
    source: StartupSource,
    /// The value or file name: what a toggle writes.
    key: String,
    /// The name shown when `exe` has none.
    fallback: &'a str,
    exe: Option<&'a str>,
    command: String,
    location: String,
}

impl ProgramEntry<'_> {
    /// The entry, named and published after `exe` (see `describe_program`), its state from
    /// `states` by `key`.
    fn into_entry(self, states: &[(String, RegValue)]) -> StartupEntry {
        let (name, publisher) = describe_program(self.exe, self.fallback);
        StartupEntry {
            source: self.source,
            name,
            publisher,
            enabled: enabled_in(states, &self.key),
            changeable: true,
            command: self.command,
            location: self.location,
            key: self.key,
        }
    }
}

/// One Run list's entries. A list that can't be read, an unreadable StartupApproved key (its
/// entries are then listed as enabled) and a value that isn't a command (or is over 1 MB) each
/// add a line to `errors`.
fn run_entries(
    roots: &StartupRoots,
    source: StartupSource,
    run: &RegKey,
    errors: &mut Vec<String>,
) -> Vec<StartupEntry> {
    let values = match registry::values(run) {
        Ok(values) => values,
        Err(e) => {
            errors.push(format!("Couldn't read {}: {e}", run.display()));
            return Vec::new();
        }
    };
    let states = approved_states(roots, source, errors);
    let mut entries = Vec::new();
    for (name, value) in values {
        if name.is_empty() {
            continue; // the key's default value: Windows doesn't start it
        }
        let Some(command) = value.as_str().map(str::to_string) else {
            errors.push(skipped(&name, &run.display(), &value));
            continue;
        };
        let exe = exe_from_command(&command, &roots.system_root);
        let entry = ProgramEntry {
            source,
            key: name.clone(),
            fallback: &name,
            exe: exe.as_deref(),
            command,
            location: run.display(),
        };
        entries.push(entry.into_entry(&states));
    }
    entries
}

/// Every startup entry, and a readable line for each list, folder or value that couldn't be read
/// (`is_read_error` tells a read error from a skipped value). Holds a COM apartment on this thread
/// for the shortcuts and the package names (a thread already in the MTA keeps it).
pub fn list_startup(roots: &StartupRoots) -> (Vec<StartupEntry>, Vec<String>) {
    let _com = ComApartment::enter();
    let mut entries = Vec::new();
    let mut errors = Vec::new();
    let runs = [
        (StartupSource::UserRun, &roots.user_run),
        (StartupSource::MachineRun, &roots.machine_run),
        (StartupSource::MachineRun32, &roots.machine_run32),
    ];
    for (source, run) in runs {
        entries.extend(run_entries(roots, source, run, &mut errors));
    }
    let folders = [
        (StartupSource::UserFolder, roots.user_folder.as_deref()),
        (
            StartupSource::MachineFolder,
            roots.machine_folder.as_deref(),
        ),
    ];
    for (source, dir) in folders {
        if let Some(dir) = dir {
            entries.extend(folders::folder_entries(roots, source, dir, &mut errors));
        }
    }
    entries.extend(packaged::packaged_entries(&roots.packaged, &mut errors));
    (entries, errors)
}

/// Enables or disables an entry the way Task Manager does. Writing an HKLM value without admin
/// fails with `ActionError::AccessDenied` (shown inline with the restart-as-admin hint); a packaged
/// task whose state a policy sets is refused ("its state is set by a policy"); any other write
/// failure is an `ActionError::Other` with Windows' message.
pub fn set_startup_enabled(
    roots: &StartupRoots,
    entry: &StartupEntry,
    enabled: bool,
) -> Result<(), ActionError> {
    let Some(key) = roots.approved_key(entry.source) else {
        return packaged::set_enabled(&roots.packaged, entry, enabled);
    };
    let previous = registry::value(&key, &entry.key);
    let bytes = encode_approved(
        previous.as_ref().and_then(RegValue::binary),
        enabled,
        filetime_now(),
    );
    registry::set_binary(&key, &entry.key, &bytes)
}
