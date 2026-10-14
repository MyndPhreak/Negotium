use negotium_collector::ProcessReader;
use std::process::Command;
use std::time::Duration;

#[test]
fn includes_current_process() {
    let me = std::process::id();
    let list = ProcessReader::new().read().expect("read");
    let p = list.iter().find(|p| p.pid == me).expect("own pid listed");
    assert!(p.name.to_lowercase().ends_with(".exe"), "name was {}", p.name);
    assert!(p.create_time > 0);
    assert!(p.threads >= 1);
    assert!(p.private_working_set > 0);
}

#[test]
fn sees_child_with_parent_and_growing_cpu_time() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_negotium-burn")).arg("3000").spawn().unwrap();
    let mut reader = ProcessReader::new();
    std::thread::sleep(Duration::from_millis(300));
    let first = reader.read().unwrap().into_iter().find(|p| p.pid == child.id()).expect("child listed");
    assert_eq!(first.parent_pid, std::process::id());
    std::thread::sleep(Duration::from_millis(700));
    let second = reader.read().unwrap().into_iter().find(|p| p.pid == child.id()).expect("child listed");
    assert_eq!(second.create_time, first.create_time);
    assert!(second.cpu_time_100ns > first.cpu_time_100ns + 3_000_000, "cpu time should grow by >300 ms");
    child.kill().ok();
    child.wait().ok();
}

#[test]
fn includes_idle_and_system() {
    let list = ProcessReader::new().read().unwrap();
    let idle = list.iter().find(|p| p.pid == 0).expect("pid 0");
    assert_eq!(idle.name, "System Idle Process");
    assert!(list.iter().any(|p| p.pid == 4 && p.name == "System"));
}
