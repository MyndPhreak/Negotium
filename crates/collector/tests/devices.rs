use negotium_collector::PdhSources;
use negotium_collector::net::{NetTable, join_adapters};
use std::time::{Duration, Instant};

#[test]
fn live_network_instances_join_the_interface_table() {
    let mut pdh = PdhSources::open().expect("open pdh");
    std::thread::sleep(Duration::from_millis(1100));
    let raw = pdh.read().adapters.expect("Network Interface counters on a real machine");
    let mut table = NetTable::new();
    table.refresh(Instant::now()).expect("GetIfTable2");
    let joined = join_adapters(&raw, &table);
    assert_eq!(joined.len(), raw.len());
    if !raw.is_empty() {
        assert!(joined.iter().any(|a| a.alias.is_some()), "at least one PDH instance matches an interface: {joined:?}");
    }
}
