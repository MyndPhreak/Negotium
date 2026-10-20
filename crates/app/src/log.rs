//! Warnings-and-errors log (spec section 7): %LOCALAPPDATA%\Negotium\logs\negotium.log,
//! rotated at MAX_LOG_BYTES, each distinct message written once per run.
//!
//! Elevated (crate::appdata), the log is LocalAppData's known folder's, appended to through the
//! guarded write only, and never rotated: once it is full, an elevated Negotium logs nothing more
//! (the next non-elevated run rotates it). Every line is one line: control characters in a
//! message are replaced, so no message can forge another.
use crate::appdata::{Open, Writes};
use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;

pub const MAX_LOG_BYTES: u64 = 1024 * 1024;

pub struct Log {
    path: PathBuf,
    seen: HashSet<String>,
    writes: Writes,
}

impl Log {
    /// The log at `path`, written as this process writes there (`appdata::writes`).
    pub fn new(path: PathBuf) -> Self {
        Self::with_writes(path, crate::appdata::writes())
    }

    /// The log at `path`, written through `writes` (tests choose).
    pub fn with_writes(path: PathBuf, writes: Writes) -> Self {
        Self { path, seen: HashSet::new(), writes }
    }

    pub fn default_path() -> PathBuf {
        crate::appdata::negotium_dir().join("logs").join("negotium.log")
    }

    pub fn info(&mut self, msg: &str) {
        self.write("INFO", msg);
    }

    pub fn warn(&mut self, msg: &str) {
        if !self.seen.insert(msg.to_string()) {
            return;
        }
        self.write("WARN", msg);
    }

    /// Appends one `level` line, rotating first when the log is past MAX_LOG_BYTES.
    fn write(&mut self, level: &str, msg: &str) {
        if std::fs::metadata(&self.path).map(|m| m.len() > MAX_LOG_BYTES).unwrap_or(false) {
            if !self.writes.may_move() {
                return; // elevated: never rotated, and not grown past the limit
            }
            let _ = std::fs::rename(&self.path, self.path.with_extension("log.1"));
        }
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let line: String = msg.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
        if let Ok(mut f) = self.writes.open(&self.path, Open::Append) {
            let _ = writeln!(f, "{secs} {level} {line}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("negotium-log-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn writes_each_distinct_message_once() {
        let dir = temp_dir("dedup");
        let mut log = Log::with_writes(dir.join("negotium.log"), Writes::Plain);
        log.warn("pdh gpu: counter unavailable");
        log.warn("pdh gpu: counter unavailable");
        log.warn("worker restarted after panic: boom");
        let text = std::fs::read_to_string(dir.join("negotium.log")).unwrap();
        assert_eq!(text.lines().count(), 2);
        assert!(text.lines().all(|l| l.contains(" WARN ")));
    }

    #[test]
    fn rotates_when_too_large() {
        let dir = temp_dir("rotate");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("negotium.log"), vec![b'x'; MAX_LOG_BYTES as usize + 1]).unwrap();
        let mut log = Log::with_writes(dir.join("negotium.log"), Writes::Plain);
        log.warn("after rotation");
        assert!(dir.join("negotium.log.1").exists());
        assert!(std::fs::read_to_string(dir.join("negotium.log")).unwrap().contains("after rotation"));
    }

    #[test]
    fn info_rotates_when_too_large() {
        let dir = temp_dir("rotate-info");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("negotium.log"), vec![b'x'; MAX_LOG_BYTES as usize + 1]).unwrap();
        let mut log = Log::with_writes(dir.join("negotium.log"), Writes::Plain);
        log.info("after rotation");
        assert!(dir.join("negotium.log.1").exists());
        assert!(std::fs::read_to_string(dir.join("negotium.log")).unwrap().contains(" INFO after rotation"));
    }

    /// Elevated: appended through the guarded write, never rotated (renaming would act on
    /// whatever a planted link names); a full log takes nothing more.
    #[test]
    fn elevated_it_appends_and_never_rotates() {
        let base = temp_dir("elevated");
        std::fs::create_dir_all(&base).unwrap();
        let path = base.join("Negotium").join("logs").join("negotium.log");
        let guarded = || Writes::Guarded(Some(base.clone()));
        let mut log = Log::with_writes(path.clone(), guarded());
        log.warn("first");
        log.warn("second");
        assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 2);
        std::fs::write(&path, vec![b'x'; MAX_LOG_BYTES as usize + 1]).unwrap();
        let mut log = Log::with_writes(path.clone(), guarded());
        log.warn("after the limit");
        assert!(!path.with_extension("log.1").exists(), "not rotated");
        assert_eq!(std::fs::metadata(&path).unwrap().len(), MAX_LOG_BYTES + 1, "nothing appended");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A message can't add a line of its own.
    #[test]
    fn a_message_is_one_line() {
        let dir = temp_dir("oneline");
        let mut log = Log::with_writes(dir.join("negotium.log"), Writes::Plain);
        log.warn("integrate: bad\r\n0 WARN forged\u{1b}[2J");
        let text = std::fs::read_to_string(dir.join("negotium.log")).unwrap();
        assert_eq!(text.lines().count(), 1, "{text:?}");
        assert!(text.contains("bad  0 WARN forged [2J"), "{text:?}");
    }
}
