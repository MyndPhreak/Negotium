use negotium_collector::{PdhSources, logical_cpus, memory_totals};
use std::time::Duration;

#[test]
fn reads_cpu_and_per_core_after_priming() {
    let mut pdh = PdhSources::open().expect("open pdh");
    std::thread::sleep(Duration::from_millis(1100));
    let r = pdh.read();
    let cpu = r.cpu_pct.expect("cpu available on a real machine");
    assert!((0.0..=100.0).contains(&cpu));
    assert_eq!(r.per_core_pct.len() as u32, logical_cpus(), "one slot per logical processor");
    assert!(r.per_core_pct.iter().any(|c| c.is_some()));
    assert!(r.mem_lists.is_some(), "Memory counter set readable without admin");
    assert!(r.disks.is_some());
    assert!(r.net_bps.is_some());
}

#[test]
fn memory_totals_are_sane() {
    let m = memory_totals().expect("memory");
    assert!(m.total > 0 && m.used <= m.total);
}
