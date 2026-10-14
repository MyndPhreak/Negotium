//! One busy thread should show as ~100 / logical_cpus percent, like Task Manager.
use negotium_collector::Collector;
use negotium_model::Model;
use std::process::Command;
use std::time::Duration;

#[test]
fn busy_child_reports_one_core_of_cpu() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_negotium-burn")).arg("5000").spawn().unwrap();
    let mut collector = Collector::new();
    let mut model = Model::new();
    std::thread::sleep(Duration::from_millis(500));
    model.update(&collector.tick());
    std::thread::sleep(Duration::from_millis(2000));
    let snap = collector.tick();
    let rows = model.update(&snap);
    child.kill().ok();
    child.wait().ok();

    let proc_row = rows.iter().flat_map(|r| &r.processes).find(|p| p.pid == child.id()).expect("child row");
    let expected = 100.0 / snap.totals.logical_cpus as f32;
    assert!(
        (proc_row.cpu_pct - expected).abs() <= expected * 0.35,
        "got {} %, expected about {} %", proc_row.cpu_pct, expected
    );
    let system = snap.totals.cpu_pct.expect("system cpu");
    assert!(system + 5.0 >= proc_row.cpu_pct, "system {system} below process {}", proc_row.cpu_pct);
}
