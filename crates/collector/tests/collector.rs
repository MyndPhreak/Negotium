use negotium_collector::Collector;
use std::time::Duration;

#[test]
fn ticks_produce_complete_snapshots() {
    let mut c = Collector::new();
    let first = c.tick();
    std::thread::sleep(Duration::from_millis(1000));
    let second = c.tick();

    assert!(second.taken_at_ms >= first.taken_at_ms + 900);
    assert!(second.processes.len() > 20);
    assert!(!second.source_errors.iter().any(|e| e.starts_with("processes")), "{:?}", second.source_errors);

    let me = second.processes.iter().find(|p| p.pid == std::process::id()).expect("self");
    assert!(me.image_path.is_some());
    assert!(second.totals.logical_cpus >= 1);
    assert!(second.totals.memory.is_some());
    assert!(second.totals.cpu_pct.is_some());
    let t = &second.totals;
    assert_eq!(t.per_core_pct.len() as u32, t.logical_cpus);
    assert!(t.uptime_s > 0 && t.local_time_ms < 86_400_000);
    let m = t.memory.as_ref().unwrap();
    assert!(m.commit_limit > 0);
    let lists = m.lists.expect("memory composition");
    assert!(lists.modified + lists.standby + lists.free <= m.total);
    assert!(c.system_info().cpu_name.as_deref().is_some_and(|n| !n.is_empty()));
}
