//! Open TCP and UDP sockets with their owner PID (`GetExtendedTcpTable` / `GetExtendedUdpTable`,
//! IPv4 and IPv6). No admin rights needed. Ports in the MIB rows are in network byte order in the
//! low 16 bits of a `u32`; `port_from_mib` is the one place that reads them.
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV6};
use windows::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;
use windows::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, GetExtendedUdpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCPROW_OWNER_PID, MIB_UDP6ROW_OWNER_PID,
    MIB_UDPROW_OWNER_PID, TCP_TABLE_OWNER_PID_ALL, UDP_TABLE_OWNER_PID,
};
use windows::Win32::Networking::WinSock::{AF_INET, AF_INET6};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Protocol {
    Tcp,
    Udp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TcpState {
    Closed,
    Listen,
    SynSent,
    SynReceived,
    Established,
    FinWait1,
    FinWait2,
    CloseWait,
    Closing,
    LastAck,
    TimeWait,
    DeleteTcb,
    Unknown(u32),
}

/// What identifies a socket across reads (the UI keeps its selection by it).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SocketKey {
    pub protocol: Protocol,
    pub local: SocketAddr,
    pub remote: Option<SocketAddr>,
    pub pid: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Socket {
    pub protocol: Protocol,
    pub local: SocketAddr,
    /// TCP only, and not for a listening socket.
    pub remote: Option<SocketAddr>,
    /// TCP only.
    pub state: Option<TcpState>,
    pub pid: u32,
}

impl Socket {
    pub fn key(&self) -> SocketKey {
        SocketKey { protocol: self.protocol, local: self.local, remote: self.remote, pid: self.pid }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SocketError {
    Api { table: &'static str, code: u32 },
    KeptGrowing { table: &'static str },
}

impl fmt::Display for SocketError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SocketError::Api { table, code } => write!(f, "{table} failed: 0x{code:X}"),
            SocketError::KeptGrowing { table } => write!(f, "{table} kept growing"),
        }
    }
}

/// Most tries `read_table` makes while a table keeps growing under it.
pub const MAX_TABLE_TRIES: usize = 4;

/// The first buffer: room for a few thousand sockets, so a normal read is one call.
const FIRST_BUFFER: usize = 64 * 1024;

/// A MIB port: network byte order in the low 16 bits (the high 16 are undefined).
pub fn port_from_mib(dw: u32) -> u16 {
    u16::from_be(dw as u16)
}

/// A `MIB_TCP_STATE` value.
pub fn tcp_state(dw: u32) -> TcpState {
    match dw {
        1 => TcpState::Closed,
        2 => TcpState::Listen,
        3 => TcpState::SynSent,
        4 => TcpState::SynReceived,
        5 => TcpState::Established,
        6 => TcpState::FinWait1,
        7 => TcpState::FinWait2,
        8 => TcpState::CloseWait,
        9 => TcpState::Closing,
        10 => TcpState::LastAck,
        11 => TcpState::TimeWait,
        12 => TcpState::DeleteTcb,
        other => TcpState::Unknown(other),
    }
}

/// An IPv4 address as a MIB row holds it: the four bytes in memory order.
fn v4(dw: u32) -> Ipv4Addr {
    Ipv4Addr::from(dw.to_le_bytes())
}

fn v6(octets: [u8; 16], scope: u32, port: u32) -> SocketAddr {
    SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::from(octets), port_from_mib(port), 0, scope))
}

/// A listening TCP socket reports the remote `0.0.0.0:0` (or `[::]:0`): no remote.
fn remote_of(addr: SocketAddr) -> Option<SocketAddr> {
    (!(addr.ip().is_unspecified() && addr.port() == 0)).then_some(addr)
}

pub fn tcp4_row(row: &MIB_TCPROW_OWNER_PID) -> Socket {
    Socket {
        protocol: Protocol::Tcp,
        local: SocketAddr::new(IpAddr::V4(v4(row.dwLocalAddr)), port_from_mib(row.dwLocalPort)),
        remote: remote_of(SocketAddr::new(IpAddr::V4(v4(row.dwRemoteAddr)), port_from_mib(row.dwRemotePort))),
        state: Some(tcp_state(row.dwState)),
        pid: row.dwOwningPid,
    }
}

pub fn tcp6_row(row: &MIB_TCP6ROW_OWNER_PID) -> Socket {
    Socket {
        protocol: Protocol::Tcp,
        local: v6(row.ucLocalAddr, row.dwLocalScopeId, row.dwLocalPort),
        remote: remote_of(v6(row.ucRemoteAddr, row.dwRemoteScopeId, row.dwRemotePort)),
        state: Some(tcp_state(row.dwState)),
        pid: row.dwOwningPid,
    }
}

pub fn udp4_row(row: &MIB_UDPROW_OWNER_PID) -> Socket {
    Socket {
        protocol: Protocol::Udp,
        local: SocketAddr::new(IpAddr::V4(v4(row.dwLocalAddr)), port_from_mib(row.dwLocalPort)),
        remote: None,
        state: None,
        pid: row.dwOwningPid,
    }
}

pub fn udp6_row(row: &MIB_UDP6ROW_OWNER_PID) -> Socket {
    Socket {
        protocol: Protocol::Udp,
        local: v6(row.ucLocalAddr, row.dwLocalScopeId, row.dwLocalPort),
        remote: None,
        state: None,
        pid: row.dwOwningPid,
    }
}

/// Calls `read(buf, &mut size)` until it returns NO_ERROR, growing `buf` to the size asked for on
/// ERROR_INSUFFICIENT_BUFFER, at most MAX_TABLE_TRIES times.
pub fn read_table(
    table: &'static str,
    mut read: impl FnMut(*mut core::ffi::c_void, &mut u32) -> u32,
) -> Result<Vec<u8>, SocketError> {
    let mut buf = vec![0u8; FIRST_BUFFER];
    let mut size = buf.len() as u32;
    for _ in 0..MAX_TABLE_TRIES {
        match read(buf.as_mut_ptr().cast(), &mut size) {
            0 => {
                buf.truncate(size as usize);
                return Ok(buf);
            }
            code if code == ERROR_INSUFFICIENT_BUFFER.0 => buf.resize(size as usize, 0),
            code => return Err(SocketError::Api { table, code }),
        }
    }
    Err(SocketError::KeptGrowing { table })
}

/// The rows of a `MIB_*TABLE_OWNER_PID` buffer: a `u32` count, then that many rows. A count that
/// does not fit the buffer reads as the rows that do.
fn rows<T: Copy>(buf: &[u8]) -> Vec<T> {
    let Some(count) = buf.first_chunk::<4>().map(|c| u32::from_le_bytes(*c) as usize) else { return Vec::new() };
    let size = size_of::<T>();
    let fit = (buf.len() - 4) / size;
    (0..count.min(fit))
        // SAFETY: the row lies inside `buf` (`fit`), and the buffer's alignment is not assumed.
        .map(|i| unsafe { std::ptr::read_unaligned(buf.as_ptr().add(4 + i * size).cast::<T>()) })
        .collect()
}

/// TCP v4 + v6 (TCP_TABLE_OWNER_PID_ALL) and UDP v4 + v6 (UDP_TABLE_OWNER_PID). All or nothing.
pub fn list_sockets() -> Result<Vec<Socket>, SocketError> {
    let tcp = |table, family: u32| {
        read_table(table, |buf, size| {
            // SAFETY: `buf` holds `*size` bytes, as `read_table` promises.
            unsafe { GetExtendedTcpTable(Some(buf), size, false, family, TCP_TABLE_OWNER_PID_ALL, 0) }
        })
    };
    let udp = |table, family: u32| {
        read_table(table, |buf, size| {
            // SAFETY: as above.
            unsafe { GetExtendedUdpTable(Some(buf), size, false, family, UDP_TABLE_OWNER_PID, 0) }
        })
    };
    let mut out = Vec::new();
    out.extend(rows::<MIB_TCPROW_OWNER_PID>(&tcp("GetExtendedTcpTable (IPv4)", AF_INET.0 as u32)?).iter().map(tcp4_row));
    out.extend(rows::<MIB_TCP6ROW_OWNER_PID>(&tcp("GetExtendedTcpTable (IPv6)", AF_INET6.0 as u32)?).iter().map(tcp6_row));
    out.extend(rows::<MIB_UDPROW_OWNER_PID>(&udp("GetExtendedUdpTable (IPv4)", AF_INET.0 as u32)?).iter().map(udp4_row));
    out.extend(rows::<MIB_UDP6ROW_OWNER_PID>(&udp("GetExtendedUdpTable (IPv6)", AF_INET6.0 as u32)?).iter().map(udp6_row));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr, SocketAddrV4, SocketAddrV6, TcpListener, TcpStream, UdpSocket};

    const INSUFFICIENT_BUFFER: u32 = 122;
    const INVALID_PARAMETER: u32 = 87;

    /// A port as a MIB row holds it: network byte order in the low 16 bits.
    fn mib(port: u16) -> u32 {
        port.to_be() as u32
    }

    fn v4(a: [u8; 4]) -> u32 {
        u32::from_le_bytes(a)
    }

    fn tcp4(state: u32, local: ([u8; 4], u16), remote: ([u8; 4], u16), pid: u32) -> MIB_TCPROW_OWNER_PID {
        MIB_TCPROW_OWNER_PID {
            dwState: state,
            dwLocalAddr: v4(local.0),
            dwLocalPort: mib(local.1),
            dwRemoteAddr: v4(remote.0),
            dwRemotePort: mib(remote.1),
            dwOwningPid: pid,
        }
    }

    fn sa4(a: [u8; 4], port: u16) -> SocketAddr {
        SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::from(a), port))
    }

    #[test]
    fn port_from_mib_reads_network_order_from_the_low_16_bits() {
        assert_eq!(port_from_mib(0x0000_3514), 5173);
        assert_eq!(port_from_mib(0x0000_5000), 80);
        assert_eq!(port_from_mib(0xFFFF_3514), 5173, "high bits are ignored");
    }

    #[test]
    fn tcp_state_maps_every_mib_state() {
        let expected = [
            (1, TcpState::Closed),
            (2, TcpState::Listen),
            (3, TcpState::SynSent),
            (4, TcpState::SynReceived),
            (5, TcpState::Established),
            (6, TcpState::FinWait1),
            (7, TcpState::FinWait2),
            (8, TcpState::CloseWait),
            (9, TcpState::Closing),
            (10, TcpState::LastAck),
            (11, TcpState::TimeWait),
            (12, TcpState::DeleteTcb),
            (99, TcpState::Unknown(99)),
            (0, TcpState::Unknown(0)),
        ];
        for (dw, state) in expected {
            assert_eq!(tcp_state(dw), state, "state {dw}");
        }
    }

    #[test]
    fn tcp4_row_listening_has_no_remote() {
        let row = tcp4(2, ([0, 0, 0, 0], 5173), ([0, 0, 0, 0], 0), 42);
        assert_eq!(
            tcp4_row(&row),
            Socket { protocol: Protocol::Tcp, local: sa4([0, 0, 0, 0], 5173), remote: None, state: Some(TcpState::Listen), pid: 42 }
        );
    }

    #[test]
    fn tcp4_row_established_keeps_the_remote() {
        let row = tcp4(5, ([192, 168, 1, 5], 50000), ([93, 184, 216, 34], 443), 7);
        let s = tcp4_row(&row);
        assert_eq!(s.remote, Some(sa4([93, 184, 216, 34], 443)));
        assert_eq!(s.local, sa4([192, 168, 1, 5], 50000));
        assert_eq!(s.state, Some(TcpState::Established));
    }

    #[test]
    fn tcp4_row_time_wait_without_an_owner_keeps_pid_zero_and_the_remote() {
        let row = tcp4(11, ([127, 0, 0, 1], 50001), ([127, 0, 0, 1], 5173), 0);
        let s = tcp4_row(&row);
        assert_eq!((s.state, s.pid), (Some(TcpState::TimeWait), 0));
        assert_eq!(s.remote, Some(sa4([127, 0, 0, 1], 5173)));
    }

    fn tcp6(state: u32, local: (Ipv6Addr, u32, u16), pid: u32) -> MIB_TCP6ROW_OWNER_PID {
        MIB_TCP6ROW_OWNER_PID {
            ucLocalAddr: local.0.octets(),
            dwLocalScopeId: local.1,
            dwLocalPort: mib(local.2),
            ucRemoteAddr: [0; 16],
            dwRemoteScopeId: 0,
            dwRemotePort: 0,
            dwState: state,
            dwOwningPid: pid,
        }
    }

    #[test]
    fn tcp6_row_listening_on_loopback() {
        let s = tcp6_row(&tcp6(2, (Ipv6Addr::LOCALHOST, 0, 8080), 9));
        assert_eq!(s.local, SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::LOCALHOST, 8080, 0, 0)));
        assert_eq!((s.remote, s.state, s.pid, s.protocol), (None, Some(TcpState::Listen), 9, Protocol::Tcp));
    }

    #[test]
    fn tcp6_row_keeps_the_scope_id_of_a_link_local_address() {
        let link_local = Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1);
        let s = tcp6_row(&tcp6(2, (link_local, 12, 445), 4));
        assert_eq!(s.local, SocketAddr::V6(SocketAddrV6::new(link_local, 445, 0, 12)));
    }

    #[test]
    fn udp_rows_have_no_state_and_no_remote() {
        let s = udp4_row(&MIB_UDPROW_OWNER_PID { dwLocalAddr: v4([0, 0, 0, 0]), dwLocalPort: mib(5353), dwOwningPid: 3 });
        assert_eq!(s, Socket { protocol: Protocol::Udp, local: sa4([0, 0, 0, 0], 5353), remote: None, state: None, pid: 3 });
        let s = udp6_row(&MIB_UDP6ROW_OWNER_PID { ucLocalAddr: [0; 16], dwLocalScopeId: 0, dwLocalPort: mib(5353), dwOwningPid: 3 });
        assert_eq!(
            s,
            Socket {
                protocol: Protocol::Udp,
                local: SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, 5353, 0, 0)),
                remote: None,
                state: None,
                pid: 3
            }
        );
    }

    #[test]
    fn read_table_returns_the_buffer_of_a_first_call_that_succeeds() {
        let mut calls = 0;
        let buf = read_table("t", |ptr, size| {
            calls += 1;
            assert!(*size >= 4, "the first buffer has room to start with");
            // SAFETY: the buffer is at least `size` bytes.
            unsafe { *(ptr as *mut u8) = 7 };
            *size = 4;
            0
        })
        .unwrap();
        assert_eq!((calls, buf.len(), buf[0]), (1, 4, 7));
    }

    #[test]
    fn read_table_grows_the_buffer_to_the_size_asked_for() {
        let mut sizes = Vec::new();
        let buf = read_table("t", |_, size| {
            sizes.push(*size);
            if sizes.len() == 1 {
                *size = 64;
                return INSUFFICIENT_BUFFER;
            }
            0
        })
        .unwrap();
        assert_eq!(sizes.len(), 2);
        assert_eq!(sizes[1], 64, "the second call offers the size the first asked for");
        assert_eq!(buf.len(), 64);
    }

    #[test]
    fn read_table_gives_up_after_four_tries_of_a_table_that_keeps_growing() {
        let mut calls = 0usize;
        let r = read_table("GetExtendedTcpTable (IPv4)", |_, size| {
            calls += 1;
            *size += 64;
            INSUFFICIENT_BUFFER
        });
        assert_eq!(r, Err(SocketError::KeptGrowing { table: "GetExtendedTcpTable (IPv4)" }));
        assert_eq!(calls, MAX_TABLE_TRIES);
        assert_eq!(MAX_TABLE_TRIES, 4);
    }

    #[test]
    fn read_table_does_not_retry_another_error() {
        let mut calls = 0;
        let r = read_table("t", |_, _| {
            calls += 1;
            INVALID_PARAMETER
        });
        assert_eq!(r, Err(SocketError::Api { table: "t", code: 87 }));
        assert_eq!(calls, 1);
    }

    #[test]
    fn socket_errors_name_the_table() {
        let api = SocketError::Api { table: "GetExtendedTcpTable (IPv4)", code: 87 };
        let grew = SocketError::KeptGrowing { table: "GetExtendedUdpTable (IPv6)" };
        assert_eq!(api.to_string(), "GetExtendedTcpTable (IPv4) failed: 0x57");
        assert_eq!(grew.to_string(), "GetExtendedUdpTable (IPv6) kept growing");
    }

    #[test]
    fn socket_key_is_the_socket_tuple() {
        let s = Socket {
            protocol: Protocol::Tcp,
            local: sa4([127, 0, 0, 1], 80),
            remote: Some(sa4([10, 0, 0, 1], 5000)),
            state: Some(TcpState::Established),
            pid: 5,
        };
        let key = SocketKey { protocol: Protocol::Tcp, local: s.local, remote: s.remote, pid: 5 };
        assert_eq!(s.key(), key);
        let other = Socket { pid: 6, ..s };
        assert_ne!(s.key(), other.key(), "only the PID differs");
    }

    fn find(all: &[Socket], f: impl Fn(&Socket) -> bool) -> Option<Socket> {
        all.iter().copied().find(f)
    }

    #[test]
    fn a_tcp_listener_is_listed_with_our_pid() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let all = list_sockets().expect("the tables read without elevation");
        let s = find(&all, |s| s.protocol == Protocol::Tcp && s.local.port() == port && s.pid == std::process::id())
            .expect("the listener is in the table");
        assert_eq!((s.state, s.remote, s.local), (Some(TcpState::Listen), None, sa4([127, 0, 0, 1], port)));
    }

    #[test]
    fn a_udp_socket_is_listed_with_our_pid() {
        let u = UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = u.local_addr().unwrap().port();
        let all = list_sockets().expect("the tables read without elevation");
        let s = find(&all, |s| s.protocol == Protocol::Udp && s.local.port() == port && s.pid == std::process::id())
            .expect("the socket is in the table");
        assert_eq!((s.state, s.remote), (None, None));
    }

    #[test]
    fn an_ipv6_listener_is_listed_with_an_ipv6_address() {
        let Ok(l) = TcpListener::bind("[::1]:0") else {
            eprintln!("skipped: IPv6 loopback can't be bound on this machine");
            return;
        };
        let port = l.local_addr().unwrap().port();
        let all = list_sockets().expect("the tables read without elevation");
        let s = find(&all, |s| s.protocol == Protocol::Tcp && s.local.port() == port && s.pid == std::process::id())
            .expect("the listener is in the table");
        assert!(s.local.is_ipv6(), "{s:?}");
        assert_eq!(s.state, Some(TcpState::Listen));
    }

    #[test]
    fn an_accepted_connection_is_listed_as_established_with_its_peer() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let server = l.local_addr().unwrap();
        let client = TcpStream::connect(server).unwrap();
        let (_accepted, _) = l.accept().unwrap();
        let client_addr = client.local_addr().unwrap();
        let all = list_sockets().expect("the tables read without elevation");
        let s = find(&all, |s| s.protocol == Protocol::Tcp && s.local == client_addr && s.pid == std::process::id())
            .expect("the client end is in the table");
        assert_eq!((s.state, s.remote), (Some(TcpState::Established), Some(server)));
    }
}
