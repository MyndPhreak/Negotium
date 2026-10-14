//! Warnings-and-errors log (spec section 7): %LOCALAPPDATA%\Negotium\logs\negotium.log,
//! rotated at MAX_LOG_BYTES, each distinct message written once per run.
use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;

pub const MAX_LOG_BYTES: u64 = 1024 * 1024;

pub struct Log {
    path: PathBuf,
    seen: HashSet<String>,
}

impl Log {
    pub fn new(path: PathBuf) -> Self {
        Self { path, seen: HashSet::new() }
    }

    pub fn default_path() -> PathBuf {
        let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        base.join("Negotium").join("logs").join("negotium.log")
    }

    pub fn warn(&mut self, msg: &str) {
        if !self.seen.insert(msg.to_string()) {
            return;
        }
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if std::fs::metadata(&self.path).map(|m| m.len() > MAX_LOG_BYTES).unwrap_or(false) {
            let _ = std::fs::rename(&self.path, self.path.with_extension("log.1"));
        }
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&self.path) {
            let _ = writeln!(f, "{secs} WARN {msg}");
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
        let mut log = Log::new(dir.join("negotium.log"));
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
        let mut log = Log::new(dir.join("negotium.log"));
        log.warn("after rotation");
        assert!(dir.join("negotium.log.1").exists());
        assert!(std::fs::read_to_string(dir.join("negotium.log")).unwrap().contains("after rotation"));
    }
}
