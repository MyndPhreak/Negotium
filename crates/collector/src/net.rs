//! Network interfaces: friendly name, hardware or virtual, link speed and addresses from the
//! interface table, joined to PDH "Network Interface" instances by their mangled description.
//! The table is read every REFRESH, or sooner (at most every MIN_REFRESH) when PDH reports an
//! instance it doesn't know — never every tick.
use negotium_snapshot::AdapterSample;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::NetworkManagement::IpHelper::{
    FreeMibTable, GetIfTable2, GetUnicastIpAddressTable, MIB_IF_TABLE2, MIB_UNICASTIPADDRESS_TABLE,
};
use windows::Win32::NetworkManagement::Ndis::MediaConnectStateConnected;
use windows::Win32::Networking::WinSock::{AF_INET, AF_INET6, AF_UNSPEC, SOCKADDR_INET};

pub const REFRESH: Duration = Duration::from_secs(10);
pub const MIN_REFRESH: Duration = Duration::from_secs(2);

/// `MIB_IF_ROW2.InterfaceAndOperStatusFlags` bits.
const HARDWARE_INTERFACE: u8 = 0x01;
const FILTER_INTERFACE: u8 = 0x02;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct IfInfo {
    pub alias: String,
    pub description: String,
    pub hardware: bool,
    pub connected: bool,
    pub link_bps: Option<u64>,
    /// IPv4 first, then IPv6; loopback and IPv6 link-local omitted.
    pub addresses: Vec<String>,
}

/// PDH names a "Network Interface" instance after the adapter description with `(`→`[`,
/// `)`→`]`, and `#`, `/`, `\` → `_`.
pub fn pdh_instance_name(description: &str) -> String {
    description
        .chars()
        .map(|c| match c {
            '(' => '[',
            ')' => ']',
            '#' | '/' | '\\' => '_',
            c => c,
        })
        .collect()
}

#[derive(Default)]
pub struct NetTable {
    by_instance: HashMap<String, IfInfo>,
    read_at: Option<Instant>,
}

impl NetTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_interfaces(list: Vec<IfInfo>, read_at: Instant) -> Self {
        Self {
            by_instance: list.into_iter().map(|i| (pdh_instance_name(&i.description), i)).collect(),
            read_at: Some(read_at),
        }
    }

    pub fn get(&self, instance: &str) -> Option<&IfInfo> {
        self.by_instance.get(instance)
    }

    pub fn needs_refresh<'a>(&self, instances: impl IntoIterator<Item = &'a str>, now: Instant) -> bool {
        let Some(at) = self.read_at else { return true };
        let age = now.saturating_duration_since(at);
        age >= REFRESH || (age >= MIN_REFRESH && instances.into_iter().any(|i| !self.by_instance.contains_key(i)))
    }

    /// Re-reads the interface table. The time is recorded even on failure, so a failing table
    /// is retried on the refresh schedule, not every tick.
    pub fn refresh(&mut self, now: Instant) -> Result<(), String> {
        self.read_at = Some(now);
        let list = read_interfaces()?;
        *self = Self::from_interfaces(list, now);
        Ok(())
    }
}

pub fn join_adapters(raw: &[(String, f64, f64)], table: &NetTable) -> Vec<AdapterSample> {
    raw.iter()
        .map(|(name, sent, recv)| {
            let info = table.get(name);
            AdapterSample {
                name: name.clone(),
                alias: info.map(|i| i.alias.clone()),
                description: info.map(|i| i.description.clone()),
                sent_bps: *sent,
                recv_bps: *recv,
                hardware: info.is_some_and(|i| i.hardware),
                connected: info.is_none_or(|i| i.connected),
                link_bps: info.and_then(|i| i.link_bps),
                addresses: info.map(|i| i.addresses.clone()).unwrap_or_default(),
            }
        })
        .collect()
}

/// Physical traffic counted once (ruling P3-2): hardware interfaces only; every interface when
/// none is hardware (inside a VM, or without the interface table).
pub fn hardware_total(adapters: &[AdapterSample]) -> f64 {
    let any_hardware = adapters.iter().any(|a| a.hardware);
    adapters
        .iter()
        .filter(|a| a.hardware || !any_hardware)
        .map(|a| a.sent_bps.max(0.0) + a.recv_bps.max(0.0))
        .sum()
}

fn wide(units: &[u16]) -> String {
    let end = units.iter().position(|&c| c == 0).unwrap_or(units.len());
    String::from_utf16_lossy(&units[..end])
}

fn show_address(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => !v4.is_loopback(),
        IpAddr::V6(v6) => !v6.is_loopback() && (v6.segments()[0] & 0xffc0) != 0xfe80,
    }
}

/// # Safety
/// `a.si_family` must describe the active member, as the IP helper API guarantees.
unsafe fn sockaddr_ip(a: &SOCKADDR_INET) -> Option<IpAddr> {
    // SAFETY: the caller guarantees si_family selects the initialised member.
    unsafe {
        if a.si_family == AF_INET {
            Some(IpAddr::V4(Ipv4Addr::from(a.Ipv4.sin_addr.S_un.S_addr.to_ne_bytes())))
        } else if a.si_family == AF_INET6 {
            Some(IpAddr::V6(Ipv6Addr::from(a.Ipv6.sin6_addr.u.Byte)))
        } else {
            None
        }
    }
}

fn read_interfaces() -> Result<Vec<IfInfo>, String> {
    let mut by_luid: HashMap<u64, IfInfo> = HashMap::new();
    let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
    // SAFETY: GetIfTable2 allocates the table; it is freed with FreeMibTable below.
    let status = unsafe { GetIfTable2(&mut table) };
    if status != ERROR_SUCCESS {
        return Err(format!("network: GetIfTable2 failed {:#010x}", status.0));
    }
    // SAFETY: on success `table` points to NumEntries consecutive rows.
    let rows = unsafe { std::slice::from_raw_parts((*table).Table.as_ptr(), (*table).NumEntries as usize) };
    for row in rows {
        let flags = row.InterfaceAndOperStatusFlags._bitfield;
        if flags & FILTER_INTERFACE != 0 {
            continue;
        }
        let link = row.ReceiveLinkSpeed;
        // SAFETY: NET_LUID_LH is a union whose `Value` covers the whole 64 bits.
        let luid = unsafe { row.InterfaceLuid.Value };
        by_luid.insert(luid, IfInfo {
            alias: wide(&row.Alias),
            description: wide(&row.Description),
            hardware: flags & HARDWARE_INTERFACE != 0,
            connected: row.MediaConnectState == MediaConnectStateConnected,
            link_bps: (link != 0 && link != u64::MAX).then_some(link),
            addresses: Vec::new(),
        });
    }
    // SAFETY: allocated by GetIfTable2, freed exactly once.
    unsafe { FreeMibTable(table as *const _) };

    let mut addresses: *mut MIB_UNICASTIPADDRESS_TABLE = std::ptr::null_mut();
    // SAFETY: as above, freed with FreeMibTable.
    if unsafe { GetUnicastIpAddressTable(AF_UNSPEC, &mut addresses) } == ERROR_SUCCESS {
        // SAFETY: on success `addresses` points to NumEntries consecutive rows.
        let rows = unsafe { std::slice::from_raw_parts((*addresses).Table.as_ptr(), (*addresses).NumEntries as usize) };
        for row in rows {
            // SAFETY: as for the interface LUID above.
            let luid = unsafe { row.InterfaceLuid.Value };
            // SAFETY: rows from GetUnicastIpAddressTable carry a valid si_family.
            let ip = unsafe { sockaddr_ip(&row.Address) };
            if let (Some(info), Some(ip)) = (by_luid.get_mut(&luid), ip)
                && show_address(&ip)
            {
                info.addresses.push(ip.to_string());
            }
        }
        // SAFETY: allocated by GetUnicastIpAddressTable, freed exactly once.
        unsafe { FreeMibTable(addresses as *const _) };
    }
    let mut list: Vec<IfInfo> = by_luid.into_values().collect();
    for i in &mut list {
        i.addresses.sort_by_key(|a| a.contains(':'));
    }
    Ok(list)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn info(alias: &str, description: &str, hardware: bool) -> IfInfo {
        IfInfo {
            alias: alias.into(), description: description.into(), hardware, connected: true,
            link_bps: Some(1_000_000_000), addresses: vec!["192.168.1.20".into()],
        }
    }
    fn sample(name: &str, hardware: bool, sent: f64, recv: f64) -> AdapterSample {
        AdapterSample { name: name.into(), hardware, sent_bps: sent, recv_bps: recv, connected: true, ..Default::default() }
    }

    #[test]
    fn pdh_instance_names_mangle_the_description_like_perflib() {
        assert_eq!(pdh_instance_name("Intel(R) Ethernet Connection (7) I219-V"), "Intel[R] Ethernet Connection [7] I219-V");
        assert_eq!(pdh_instance_name("Realtek USB GbE #2"), "Realtek USB GbE _2");
        assert_eq!(pdh_instance_name("Wi-Fi a/b\\c"), "Wi-Fi a_b_c");
    }

    #[test]
    fn hyper_v_and_vpn_traffic_counts_once() {
        // The physical NIC carries everything; the vEthernet and the VPN tunnel see it again.
        let adapters = [
            sample("Intel[R] Ethernet", true, 400.0, 600.0),
            sample("Hyper-V Virtual Ethernet Adapter", false, 400.0, 600.0),
            sample("WireGuard Tunnel", false, 100.0, 200.0),
        ];
        assert_eq!(hardware_total(&adapters), 1000.0);
    }

    #[test]
    fn without_any_hardware_interface_every_interface_counts() {
        let vm = [sample("Microsoft Hyper-V Network Adapter", false, 10.0, 20.0), sample("Other", false, 1.0, 2.0)];
        assert_eq!(hardware_total(&vm), 33.0);
        assert_eq!(hardware_total(&[]), 0.0);
    }

    #[test]
    fn join_fills_interface_details_and_tolerates_unknown_instances() {
        let table = NetTable::from_interfaces(vec![info("Ethernet", "Intel(R) Ethernet", true)], Instant::now());
        let joined = join_adapters(
            &[("Intel[R] Ethernet".into(), 1.0, 2.0), ("Mystery Adapter".into(), 3.0, 4.0)],
            &table,
        );
        assert_eq!(joined[0].alias.as_deref(), Some("Ethernet"));
        assert_eq!(joined[0].description.as_deref(), Some("Intel(R) Ethernet"));
        assert!(joined[0].hardware && joined[0].connected);
        assert_eq!(joined[0].link_bps, Some(1_000_000_000));
        assert_eq!(joined[0].addresses, ["192.168.1.20"]);
        assert_eq!(joined[1].alias, None);
        assert!(!joined[1].hardware && joined[1].connected, "unknown interfaces are shown, not counted as hardware");
    }

    #[test]
    fn the_interface_table_is_reread_on_age_or_on_an_unknown_instance_but_not_every_tick() {
        let t0 = Instant::now();
        let table = NetTable::from_interfaces(vec![info("Ethernet", "Intel(R) Ethernet", true)], t0);
        assert!(NetTable::new().needs_refresh(["x"], t0), "never read");
        assert!(!table.needs_refresh(["Intel[R] Ethernet"], t0 + Duration::from_secs(1)));
        assert!(!table.needs_refresh(["New Adapter"], t0 + Duration::from_secs(1)), "unknown, but read just now");
        assert!(table.needs_refresh(["New Adapter"], t0 + MIN_REFRESH));
        assert!(table.needs_refresh(["Intel[R] Ethernet"], t0 + REFRESH));
    }
}
