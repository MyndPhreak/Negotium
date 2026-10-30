//! Memory dump files, named and placed as Task Manager does.
use crate::ActionError;
use std::path::{Path, PathBuf};

/// `<process>_<pid>_<YYYYMMDD-HHMMSS>.dmp`, without ".exe" and with no character Windows forbids
/// in a file name. `local` is (year, month, day, hour, minute, second).
pub fn dump_file_name(_process: &str, _pid: u32, _local: (i32, u32, u32, u32, u32, u32)) -> String {
    String::new()
}

/// Writes a full memory dump of the process to `dir` and returns its path. Blocking; call on an
/// action thread. A failed dump leaves no file.
pub fn create_dump(_pid: u32, _create_time: i64, _process: &str, _dir: &Path) -> Result<PathBuf, ActionError> {
    Err(ActionError::Other("not built".into()))
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
