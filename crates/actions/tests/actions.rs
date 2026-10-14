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

#[test]
fn efficiency_toggle_skips_an_exited_first_target_and_still_applies_to_the_rest() {
    let (mut a, act) = spawn_ping();
    let (mut b, bct) = spawn_ping();
    a.kill().unwrap();
    a.wait().unwrap();
    assert_eq!(toggle_efficiency_mode(&[(a.id(), act), (b.id(), bct)]), Ok(true));
    assert_eq!(toggle_efficiency_mode(&[(a.id(), act)]), Err(ActionError::AlreadyExited));
    b.kill().ok();
    b.wait().ok();
}

use negotium_actions::{is_elevated, process_details};

#[test]
fn details_include_command_line_user_and_priority() {
    let (mut child, ct) = spawn_ping();
    let d = process_details(child.id(), ct).unwrap();
    child.kill().ok();
    child.wait().ok();
    assert_eq!(d.pid, child.id());
    assert!(d.command_line.as_deref().unwrap_or("").contains("127.0.0.1"), "{d:?}");
    assert!(d.user.as_deref().unwrap_or("").contains('\\'), "DOMAIN\\user expected: {d:?}");
    assert_eq!(d.priority.as_deref(), Some("Normal"));
    assert!(d.image_path.as_deref().unwrap_or("").to_lowercase().ends_with("ping.exe"));
}

#[test]
fn elevation_query_does_not_panic() {
    let _ = is_elevated();
}

use negotium_actions::breaks_on_termination;

#[test]
fn an_ordinary_process_is_not_marked_break_on_termination() {
    let (mut child, ct) = spawn_ping();
    let flag = breaks_on_termination(child.id(), ct);
    child.kill().ok();
    child.wait().ok();
    assert_eq!(flag, Some(false));
}

#[test]
fn wininit_reads_as_critical_when_elevated_and_the_query_is_safe_otherwise() {
    // Read-only: never terminates anything. Unelevated, Windows denies the query; it must still
    // return cleanly. Elevated (SeDebugPrivilege), wininit must read as BreakOnTermination.
    let procs = ProcessReader::new().read().unwrap();
    let wininit = procs.iter().find(|p| p.name.eq_ignore_ascii_case("wininit.exe")).expect("wininit is always running");
    if is_elevated() {
        let _ = negotium_actions::enable_debug_privilege();
        assert_eq!(breaks_on_termination(wininit.pid, wininit.create_time), Some(true));
    } else {
        let _ = breaks_on_termination(wininit.pid, wininit.create_time);
    }
}

#[test]
fn debug_privilege_is_enabled_exactly_when_elevated() {
    // An unelevated token never holds SeDebugPrivilege, so this only succeeds elevated.
    assert_eq!(negotium_actions::enable_debug_privilege().is_ok(), is_elevated());
}
