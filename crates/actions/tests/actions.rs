use negotium_actions::{ActionError, can_terminate, capture_shell_token, end_many, end_process};
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
    // A child inherits its parent's priority class (CI runners run below normal), so compare with ours.
    let me = std::process::id();
    let mine = ProcessReader::new().read().unwrap().into_iter().find(|p| p.pid == me).unwrap().create_time;
    let ours = process_details(me, mine).unwrap();
    assert!(d.priority.is_some(), "{d:?}");
    assert_eq!(d.priority, ours.priority);
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


use negotium_actions::{
    PriorityClass, get_priority_class, get_process_affinity, is_efficiency_mode, set_efficiency_mode,
    set_power_throttling, set_priority_class, set_process_affinity,
};

#[test]
fn priority_class_get_and_set() {
    let (mut child, ct) = spawn_ping();
    let initial = get_priority_class(child.id(), ct).unwrap();
    set_priority_class(child.id(), ct, PriorityClass::BelowNormal).unwrap();
    assert_eq!(get_priority_class(child.id(), ct).unwrap(), PriorityClass::BelowNormal);
    set_priority_class(child.id(), ct, PriorityClass::Idle).unwrap();
    assert_eq!(get_priority_class(child.id(), ct).unwrap(), PriorityClass::Idle);
    set_priority_class(child.id(), ct, initial).unwrap();
    assert_eq!(get_priority_class(child.id(), ct).unwrap(), initial);
    child.kill().ok();
    child.wait().ok();
}

#[test]
fn affinity_get_and_set() {
    let (mut child, ct) = spawn_ping();
    let (proc_mask, sys_mask) = get_process_affinity(child.id(), ct).unwrap();
    assert!(proc_mask > 0);
    assert!(sys_mask > 0);
    set_process_affinity(child.id(), ct, 1).unwrap();
    let (new_proc_mask, _) = get_process_affinity(child.id(), ct).unwrap();
    assert_eq!(new_proc_mask, 1);
    set_process_affinity(child.id(), ct, proc_mask).unwrap();
    let (restored_mask, _) = get_process_affinity(child.id(), ct).unwrap();
    assert_eq!(restored_mask, proc_mask);
    child.kill().ok();
    child.wait().ok();
}

#[test]
fn direct_efficiency_and_power_throttling() {
    let (mut child, ct) = spawn_ping();
    let initial_priority = get_priority_class(child.id(), ct).unwrap();
    assert!(!is_efficiency_mode(child.id(), ct).unwrap());
    
    // Test pure power throttling (leaves priority intact)
    set_power_throttling(child.id(), ct, true).unwrap();
    assert!(is_efficiency_mode(child.id(), ct).unwrap());
    assert_eq!(get_priority_class(child.id(), ct).unwrap(), initial_priority);
    set_power_throttling(child.id(), ct, false).unwrap();
    assert!(!is_efficiency_mode(child.id(), ct).unwrap());

    // Test full efficiency mode (changes priority to Idle)
    set_efficiency_mode(child.id(), ct, true).unwrap();
    assert!(is_efficiency_mode(child.id(), ct).unwrap());
    set_efficiency_mode(child.id(), ct, false).unwrap();
    assert!(!is_efficiency_mode(child.id(), ct).unwrap());
    
    child.kill().ok();
    child.wait().ok();
}

#[test]
fn can_terminate_own_child() {
    let (mut child, ct) = spawn_ping();
    assert!(can_terminate(child.id(), ct));
    child.kill().ok();
    child.wait().ok();
}

#[test]
fn can_terminate_an_exited_process_is_true() {
    let (mut child, ct) = spawn_ping();
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(can_terminate(child.id(), ct), "nothing to warn about for a process that is gone");
}

#[test]
fn cannot_terminate_a_protected_process_when_not_elevated() {
    if is_elevated() {
        eprintln!("skipped: elevated Negotium can open System");
        return;
    }
    // System's real create time: a made-up one would read as "exited" (and so `true`).
    let procs = ProcessReader::new().read().unwrap();
    let system = procs.iter().find(|p| p.pid == 4).expect("System (pid 4) is always listed");
    assert!(!can_terminate(system.pid, system.create_time));
}

#[test]
fn capture_shell_token_is_none_when_not_elevated() {
    if is_elevated() {
        eprintln!("skipped: elevated Negotium captures a token");
        return;
    }
    assert!(matches!(capture_shell_token(std::process::id(), 0), Ok(None)));
}
