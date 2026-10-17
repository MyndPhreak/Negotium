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

#[test]
fn a_shared_epoch_carries_collector_time_across_a_rebuilt_collector() {
    let now = std::time::Instant::now();
    // Shortly after boot an Instant may not reach 5 s back: then use an epoch the test waits past.
    let (epoch, at_least) = match now.checked_sub(Duration::from_secs(5)) {
        Some(earlier) => (earlier, 5_000),
        None => {
            std::thread::sleep(Duration::from_millis(200));
            (now, 200)
        }
    };
    let snap = negotium_collector::Collector::with_epoch(epoch).tick();
    assert!(snap.taken_at_ms >= at_least, "{} ms", snap.taken_at_ms);
}

#[test]
fn only_an_opted_in_collector_starts_the_network_feed() {
    // A plain collector never starts the feed, elevated or not, so an elevated test or bench run
    // can't replace a running Negotium's session.
    let mut plain = Collector::new();
    assert!(!plain.has_net_feed());
    assert_eq!(plain.tick().totals.net_by_pid, None);

    // Opted in: the feed runs exactly when elevated (a uniquely named session, stopped on drop).
    let name = format!("Negotium-Net-test-{}-collector", std::process::id());
    let fed = Collector::new().with_net_feed(&name);
    assert_eq!(fed.has_net_feed(), negotium_collector::netetw::is_elevated());
    drop(fed);
    negotium_collector::netetw::stop_session(&name);
}
