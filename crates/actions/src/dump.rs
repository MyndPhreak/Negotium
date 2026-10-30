//! Memory dump files, named and placed as Task Manager does.
use crate::{ActionError, open_verified};
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Diagnostics::Debug::{
    MiniDumpWithFullMemory, MiniDumpWithHandleData, MiniDumpWithThreadInfo, MiniDumpWithUnloadedModules,
    MiniDumpWriteDump,
};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::System::Threading::{PROCESS_QUERY_INFORMATION, PROCESS_VM_READ};

/// `<process>_<pid>_<YYYYMMDD-HHMMSS>.dmp`, without ".exe" and with no character Windows forbids
/// in a file name. `local` is (year, month, day, hour, minute, second).
pub fn dump_file_name(process: &str, pid: u32, local: (i32, u32, u32, u32, u32, u32)) -> String {
    let stem = match process.len().checked_sub(4).and_then(|i| process.split_at_checked(i)) {
        Some((stem, ext)) if ext.eq_ignore_ascii_case(".exe") => stem,
        _ => process,
    };
    let stem: String = stem.chars().map(|c| if r#"\/:*?"<>|"#.contains(c) || c.is_control() { '_' } else { c }).collect();
    let (y, mo, d, h, mi, s) = local;
    format!("{stem}_{pid}_{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}.dmp")
}

/// Writes a full memory dump of the process to `dir` and returns its path. Blocking; call on an
/// action thread. A failed dump leaves no file.
pub fn create_dump(pid: u32, create_time: i64, process: &str, dir: &Path) -> Result<PathBuf, ActionError> {
    let h = open_verified(pid, create_time, PROCESS_QUERY_INFORMATION | PROCESS_VM_READ)?;
    // SAFETY: plain Win32 call.
    let t = unsafe { GetLocalTime() };
    let local = (t.wYear as i32, t.wMonth as u32, t.wDay as u32, t.wHour as u32, t.wMinute as u32, t.wSecond as u32);
    let path = dir.join(dump_file_name(process, pid, local));
    let partial = path.with_extension("dmp.partial");
    let file = std::fs::File::create(&partial).map_err(|e| ActionError::Other(e.to_string()))?;
    let kind = MiniDumpWithFullMemory | MiniDumpWithHandleData | MiniDumpWithThreadInfo | MiniDumpWithUnloadedModules;
    // SAFETY: a verified process handle with read rights and an open, writable file handle.
    let written = unsafe { MiniDumpWriteDump(h.0, pid, HANDLE(file.as_raw_handle()), kind, None, None, None) };
    drop(file);
    let done = written.map_err(|e| ActionError::from_win(&e)).and_then(|()| {
        std::fs::rename(&partial, &path).map_err(|e| ActionError::Other(e.to_string()))
    });
    if done.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    done.map(|()| path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dump_names_follow_task_manager() {
        assert_eq!(dump_file_name("notepad.exe", 1234, (2026, 10, 29, 14, 25, 1)), "notepad_1234_20261029-142501.dmp");
    }

    #[test]
    fn dump_names_have_no_path_characters() {
        assert_eq!(dump_file_name(r"a:b\c.exe", 1, (2026, 1, 2, 3, 4, 5)), "a_b_c_1_20260102-030405.dmp");
    }

    struct Child(std::process::Child, PathBuf);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
            let _ = std::fs::remove_dir_all(&self.1);
        }
    }

    fn child(tag: &str) -> Child {
        let dir = std::env::temp_dir().join(format!("negotium-dump-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let c = std::process::Command::new("ping").args(["-n", "30", "127.0.0.1"]).stdout(std::process::Stdio::null()).spawn().unwrap();
        Child(c, dir)
    }

    #[test]
    fn a_child_process_can_be_dumped() {
        let c = child("ok");
        let ct = crate::test_create_time(c.0.id());
        let path = create_dump(c.0.id(), ct, "PING.EXE", &c.1).unwrap();
        assert!(path.starts_with(&c.1));
        assert!(path.metadata().unwrap().len() > 0);
    }

    #[test]
    fn a_failed_dump_leaves_no_file() {
        let c = child("fail");
        let ct = crate::test_create_time(c.0.id());
        assert!(create_dump(c.0.id(), ct + 1, "PING.EXE", &c.1).is_err());
        assert_eq!(std::fs::read_dir(&c.1).unwrap().count(), 0);
    }
}
