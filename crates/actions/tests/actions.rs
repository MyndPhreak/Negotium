use negotium_actions::{ActionError, end_many, end_process};
use negotium_collector::ProcessReader;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

fn spawn_ping() -> (Child, i64) {
    let child = Command::new(r"C:\Windows\System32\PING.EXE")
        .args(["-n", "30", "127.0.0.1"])
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let ct = ProcessReader::new().read().unwrap().into_iter().find(|p| p.pid == child.id()).unwrap().create_time;
    (child, ct)
}

#[test]
fn ends_a_process_after_verifying_identity() {
    let (mut child, ct) = spawn_ping();
    assert_eq!(end_process(child.id(), ct), Ok(()));
    assert!(child.wait().unwrap().code().is_some());
}

#[test]
fn wrong_create_time_is_treated_as_already_exited_and_spares_the_process() {
    let (mut child, ct) = spawn_ping();
    assert_eq!(end_process(child.id(), ct + 1), Err(ActionError::AlreadyExited));
    assert!(child.try_wait().unwrap().is_none(), "the live process must not be touched");
    child.kill().ok();
    child.wait().ok();
}

#[test]
fn exited_process_reports_already_exited() {
    let (mut child, ct) = spawn_ping();
    child.kill().unwrap();
    child.wait().unwrap();
    assert_eq!(end_process(child.id(), ct), Err(ActionError::AlreadyExited));
}

#[test]
fn end_many_treats_already_exited_as_success() {
    let (mut a, act) = spawn_ping();
    let (mut b, bct) = spawn_ping();
    b.kill().unwrap();
    b.wait().unwrap();
    let results = end_many(&[(a.id(), act), (b.id(), bct)]);
    assert!(results.iter().all(|(_, r)| r.is_ok()), "{results:?}");
    a.wait().unwrap();
}

use negotium_actions::{force_close, toggle_efficiency_mode};

#[test]
fn force_close_terminates_a_windowless_process_after_the_grace_period() {
    let (mut child, ct) = spawn_ping();
    let started = std::time::Instant::now();
    assert_eq!(force_close(child.id(), ct), Ok(()));
    assert!(child.wait().unwrap().code().is_some());
    assert!(started.elapsed() >= Duration::from_millis(2900), "should wait the 3 s grace first");
}

#[test]
fn efficiency_mode_toggles_on_then_off() {
    let (mut child, ct) = spawn_ping();
    assert_eq!(toggle_efficiency_mode(&[(child.id(), ct)]), Ok(true));
    assert_eq!(toggle_efficiency_mode(&[(child.id(), ct)]), Ok(false));
    child.kill().ok();
    child.wait().ok();
}
