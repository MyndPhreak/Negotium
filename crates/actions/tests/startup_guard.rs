//! Its own test binary, so this is a fresh process that has called nothing: the registry helpers
//! must refuse writes outside the test sandbox by default. Checked through the pure guard only;
//! nothing here writes to the registry.
use negotium_actions::startup::registry::{self, Hive, RegKey};

#[test]
fn a_process_that_never_unlocks_may_only_write_inside_the_test_sandbox() {
    let real = [
        RegKey::new(
            Hive::CurrentUser,
            r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run",
        ),
        RegKey::new(
            Hive::LocalMachine,
            r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run32",
        ),
        RegKey::new(Hive::CurrentUser, registry::TEST_ROOT),
        RegKey::new(Hive::CurrentUser, r"Software\Negotium-TestX\a"),
        RegKey::new(Hive::LocalMachine, &format!(r"{}\a", registry::TEST_ROOT)),
    ];
    for key in &real {
        assert!(!registry::may_write(key), "{}", key.display());
    }
    assert!(registry::may_write(&RegKey::new(
        Hive::CurrentUser,
        &format!(r"{}\sandbox", registry::TEST_ROOT)
    )));
}

#[test]
fn a_process_that_never_unlocks_may_not_write_the_integration_values() {
    let ifeo = RegKey::new(Hive::LocalMachine, registry::IFEO_TASKMGR);
    let run = RegKey::new(Hive::CurrentUser, registry::USER_RUN);
    assert!(!registry::may_write_value(&ifeo, "Debugger"));
    assert!(!registry::may_write_value(&run, "Negotium"));
}
