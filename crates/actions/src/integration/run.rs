//! Start with Windows without admin mode: the Run value `Negotium` (a whole command line).
use super::Roots;
use crate::ActionError;
use crate::startup::registry;

/// The Run value's name, the only one written under the Run key.
pub const RUN_VALUE: &str = "Negotium";

/// Sets (`Some`) or removes (`None`) the Run value of `roots.run`.
pub fn set_run(roots: &Roots, command: Option<&str>) -> Result<(), ActionError> {
    match command {
        Some(command) => registry::set_string(&roots.run, RUN_VALUE, command),
        None => registry::delete_value(&roots.run, RUN_VALUE),
    }
}

/// The Run value of `roots.run`, if it is a string.
pub fn read_run(roots: &Roots) -> Option<String> {
    registry::value(&roots.run, RUN_VALUE)?.as_str().map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::startup::registry::{Hive, RegKey, TEST_ROOT};

    /// The real code path against a sandbox Run key (the value keeps its real name, the key is a
    /// test one), so no guard rule is loosened and nothing starts at sign-in.
    #[test]
    fn set_read_and_remove_in_the_sandbox() {
        let base = RegKey::new(Hive::CurrentUser, &format!(r"{TEST_ROOT}\run-{}", std::process::id()));
        let _ = registry::delete_test_tree(&base);
        let roots = Roots { run: base.child("Run"), ..Roots::system() };
        registry::set_string(&roots.run, "Other", "keep").unwrap();
        assert_eq!(read_run(&roots), None);
        let command = r#""C:\Program Files\Negotium\negotium.exe" --tray"#;
        set_run(&roots, Some(command)).unwrap();
        assert_eq!(read_run(&roots).as_deref(), Some(command));
        set_run(&roots, None).unwrap();
        assert_eq!(read_run(&roots), None);
        set_run(&roots, None).unwrap();
        assert!(registry::value(&roots.run, "Other").is_some(), "other values are left alone");
        registry::delete_test_tree(&base).unwrap();
    }

    #[test]
    fn the_real_run_value_is_guarded() {
        // Checked through the pure guard only: no test ever attempts the real value.
        let roots = Roots::system();
        assert_eq!(roots.run, RegKey::new(Hive::CurrentUser, registry::USER_RUN));
        assert!(!registry::may_write_value(&roots.run, RUN_VALUE), "locked in tests");
    }
}
