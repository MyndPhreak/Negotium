//! Facts about an executable file: its version and who signed it.

/// Who signed a file, as Windows checks it (no revocation check: offline-safe and fast).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Signature {
    /// The leaf signer's display name, e.g. "Microsoft Windows".
    Signed(String),
    NotSigned,
    /// The file could not be read, or the check failed for another reason.
    Unknown,
}

/// The file version from the version resource, e.g. "10.0.26100.1".
pub fn file_version(_path: &str) -> Option<String> {
    None
}

/// Checks the embedded Authenticode signature, then the system catalogs. Blocking (it can read
/// the whole file); call off the UI thread.
pub fn signature(_path: &str) -> Signature {
    Signature::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notepad() -> String {
        format!(r"{}\System32\notepad.exe", std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()))
    }

    #[test]
    fn notepad_has_a_version() {
        assert!(file_version(&notepad()).is_some_and(|v| v.split('.').count() == 4));
    }

    #[test]
    fn a_catalog_signed_windows_file_is_signed_by_microsoft() {
        assert!(matches!(signature(&notepad()), Signature::Signed(n) if n.contains("Microsoft")));
    }

    #[test]
    fn the_test_binary_is_not_signed() {
        let exe = std::env::current_exe().unwrap();
        assert_eq!(signature(exe.to_str().unwrap()), Signature::NotSigned);
    }

    #[test]
    fn a_missing_file_is_unknown() {
        assert_eq!(signature(r"C:\nope\nope.exe"), Signature::Unknown);
        assert_eq!(file_version(r"C:\nope\nope.exe"), None);
    }
}
