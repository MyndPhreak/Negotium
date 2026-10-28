# Negotium: Ports page (plan)

Spec: `docs/superpowers/specs/2026-10-28-negotium-ports-design.md`.

## Summary

The user wants to find out which app holds a port without running `netstat -ano`. A new rail page,
**Ports**, lists every TCP and UDP socket (IPv4 and IPv6) with its owner app, PID, addresses, ports
and state. It shows listening sockets by default and has an All view, a search where a number
matches a port or PID exactly, sortable columns, and the existing process actions on each row. The
collector reads the sockets with `GetExtendedTcpTable` / `GetExtendedUdpTable`, which needs no admin
rights. The worker reads them only while the page is on screen, at most every 2 s or when the user
clicks Refresh, and joins them with the tick's snapshot, so names, icons and action targets come
from the same data the Overview uses.

## Architectural decisions

1. **The worker builds the page, like Overview and Performance.** The spec left open whether the
   2 s refresh comes from a Slint `Timer` or the worker tick. The worker already has the snapshot
   (names, image paths, create times) and already builds only the visible page (spec §8 of the
   main design), so `Page::Ports` gets a `PortsFrame`. The Services job pattern was the other
   option, but it would need a second copy of the process map on the UI thread.
2. **The tables are read at most every 2 s; the join runs every tick.** A `PortReader` in the
   worker keeps the last socket list and reads again when 2 s have passed, when the page has just
   been shown, or when Refresh was clicked (`Controls::request_ports_refresh`, the same
   one-shot pattern as `request_reset_history`). Joining with each tick's snapshot keeps an owner
   that just ended from staying on screen for 2 s.
3. **Service names come from `list_services` on each socket read** (only while the page is
   visible), mapped PID → names. If that fails, rows show no service line and the failure is
   logged once per failure streak. There is no toast: the ports themselves are still right.
4. **Windows calls and MIB parsing live in the collector** (`sockets.rs`). Parsing one MIB row
   into a `Socket` is a pure function per row type. `read_table` takes the API call as a closure,
   so the grow-and-retry loop is tested without Windows.
5. **Rows, labels, filter, sort and the count text are pure functions in `app/src/ports.rs`**,
   the same split as `services.rs`. The bridge keeps the view, search and sort state on the UI
   thread and filters and sorts the last frame's rows, so typing in the search box does not wait
   for a tick.
6. **Stable row ids.** `PortIds` maps `SocketKey` (protocol, local address and port, remote
   address and port, PID) to an `i32` that stays the same across frames. Ids only grow and are
   never reused. With `sync_model` this keeps the selection, focus and scroll position across a
   refresh.
7. **Actions reuse `dispatch::plan`.** `PortsFrame.targets` holds one `view::Target` per
   actionable row: a single process with root `(pid, create_time)`, its image path, and
   `critical` from `negotium_actions::is_critical`. `on_port_action` runs `goto` through
   `select_pid` (as Services does) and the rest through `dispatch::plan`. That gives the same
   confirmations, critical-process guard and failure toasts as the Overview.
8. **Owners without actions:** PID 4 ("System"), PID 0 ("No owner") and a PID missing from the
   snapshot ("Ended") get no target, and every menu item on their rows is disabled
   (`actionable: false`, `can-locate: false`).
9. **Address text:** plain `IpAddr` display with no brackets, since the port has its own column
   (`0.0.0.0`, `::`, `::1`). An IPv6 scope id shows as `fe80::1%12`. This corrects the spec's
   `[::1]` example (recorded under Deviations). A listening TCP socket reports a remote of `0.0.0.0:0`;
   the parser maps it to `None`.
10. **Read failures keep the last list.** `PortsFrame.error` carries the message. The bridge shows
    one toast per failure streak ("Couldn't list ports: …") and keeps the rows it has.
11. **No flag.** This is a new page that changes no existing behaviour. Per the project rule
    (memory: no feature flags), it ships on.

## Files

| File | Change |
| --- | --- |
| `crates/collector/src/sockets.rs` | new: `Protocol`, `TcpState`, `Socket`, `SocketKey`, `SocketError`, `port_from_mib`, `tcp_state`, `tcp4_row`, `tcp6_row`, `udp4_row`, `udp6_row`, `read_table`, `list_sockets`; unit and live tests |
| `crates/collector/src/lib.rs` | `pub mod sockets;` and re-exports |
| `crates/collector/Cargo.toml` | only if `MIB_TCP6ROW_OWNER_PID` / `TCP_TABLE_OWNER_PID_ALL` need a feature beyond `Win32_NetworkManagement_IpHelper` + `Win32_Networking_WinSock` (verify at build) |
| `crates/app/src/ports.rs` | new: `PortOwner`, `PortView`, `PortRow`, `PortIds`, `PortReader`, `PORTS_REFRESH`, `owners_from`, `build_port_rows`, `state_label`, `address_text`, `is_listening`, `filter_port_rows`, `sort_port_rows`, `count_text`, `port_target`; tests |
| `crates/app/src/lib.rs` | `pub mod ports;` |
| `crates/app/src/controls.rs` | `Page::Ports = 6` in `ALL`, `from_index`, `name`/`from_name` (`"ports"`); `request_ports_refresh` / `take_ports_refresh`; tests |
| `crates/app/src/view.rs` | `Frame.ports: Option<PortsFrame>`; `PortsFrame { rows, targets, error }` |
| `crates/app/src/worker.rs` | `LiveSource.ports: PortReader` (socket and service sources swappable for tests); `Page::Ports` arm builds `PortsFrame`; logs a read longer than 20 ms; tests |
| `crates/app/src/bridge.rs` | ports state (view, search, sort, last rows, error streak); `set_page` treats Ports like the other list pages; `apply` handles `Page::Ports`; `set_ports_view`, `set_ports_search`, `sort_ports`, `port_target`, `port_pid`; `push_port_rows` (icons through `IconCache`); `flush_icons` also pushes port rows |
| `crates/app/src/main.rs` | wires `search-ports`, `sort-ports`, `set-ports-view`, `refresh-ports`, `port-action`; `show_page` requests a ports refresh and pokes the worker for `Page::Ports` |
| `crates/app/ui/portspage.slint` | new: `PortData`, `PortsPage` (Listening/All control, search, Refresh, sortable header, two-line App cell with icon, context menu, count line), modelled on `servicespage.slint` |
| `crates/app/ui/rail.slint` | Ports button (`page == 6`) between Services and Settings |
| `crates/app/ui/main.slint` | ports properties and callbacks; `PortsPage` at `page == 6` |
| `crates/app/tests/ui.rs` | rail and page tests |

## Contracts (landed as compiling stubs in the red commit)

No HTTP surface and no persisted document change: the settings file is untouched, and the page
index only appears in `--page=ports` on a restart. The in-process contracts:

```rust
// crates/collector/src/sockets.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Protocol { Tcp, Udp }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TcpState {
    Closed, Listen, SynSent, SynReceived, Established, FinWait1, FinWait2,
    CloseWait, Closing, LastAck, TimeWait, DeleteTcb, Unknown(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SocketKey {
    pub protocol: Protocol,
    pub local: std::net::SocketAddr,
    pub remote: Option<std::net::SocketAddr>,
    pub pid: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Socket {
    pub protocol: Protocol,
    pub local: std::net::SocketAddr,
    /// TCP only, and not for a listening socket.
    pub remote: Option<std::net::SocketAddr>,
    /// TCP only.
    pub state: Option<TcpState>,
    pub pid: u32,
}
impl Socket { pub fn key(&self) -> SocketKey; }

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SocketError {
    Api { table: &'static str, code: u32 },
    KeptGrowing { table: &'static str },
}
impl std::fmt::Display for SocketError { /* "GetExtendedTcpTable (IPv4) failed: 0x…" / "… kept growing" */ }

pub const MAX_TABLE_TRIES: usize = 4;
pub fn port_from_mib(dw: u32) -> u16;
pub fn tcp_state(dw: u32) -> TcpState;
pub fn tcp4_row(row: &MIB_TCPROW_OWNER_PID) -> Socket;
pub fn tcp6_row(row: &MIB_TCP6ROW_OWNER_PID) -> Socket;
pub fn udp4_row(row: &MIB_UDPROW_OWNER_PID) -> Socket;
pub fn udp6_row(row: &MIB_UDP6ROW_OWNER_PID) -> Socket;
/// Calls `read(buf, &mut size)` until it returns NO_ERROR, growing `buf` to the size asked for on
/// ERROR_INSUFFICIENT_BUFFER, at most MAX_TABLE_TRIES times.
pub fn read_table(table: &'static str, read: impl FnMut(*mut core::ffi::c_void, &mut u32) -> u32) -> Result<Vec<u8>, SocketError>;
/// TCP v4 + v6 (TCP_TABLE_OWNER_PID_ALL) and UDP v4 + v6 (UDP_TABLE_OWNER_PID). All or nothing.
pub fn list_sockets() -> Result<Vec<Socket>, SocketError>;
```

```rust
// crates/app/src/ports.rs
pub const PORTS_REFRESH: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, PartialEq)]
pub struct PortOwner {
    pub pid: u32,
    pub create_time: i64,
    /// Description, or the image name when there is none (as the Overview shows it).
    pub name: String,
    pub process_name: String,
    pub image_path: Option<String>,
    pub services: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PortView { #[default] Listening, All }

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PortRow {
    pub id: i32,
    pub app: String,          // "Google Chrome", "System", "No owner", "Ended"
    pub sub: String,          // service names joined ", ", or "Kernel driver (for example HTTP.sys)"
    pub icon: String,         // image path, empty for none
    pub pid: u32,
    pub protocol: String,     // "TCP" | "UDP"
    pub local_addr: String,
    pub local_port: u16,
    pub remote_addr: String,  // empty when none
    pub remote_port: Option<u16>,
    pub state: String,        // state_label
    pub listening: bool,
    pub actionable: bool,     // End task / End process tree / Go to process
    pub can_locate: bool,     // Open file location
    pub search_text: String,  // lowercased app, process name, services, addresses, state
}

pub struct PortIds { /* HashMap<SocketKey, i32>, next: i32 */ }
impl PortIds { pub fn id(&mut self, key: SocketKey) -> i32; pub fn retain(&mut self, live: &HashSet<SocketKey>); }

pub struct PortReader { /* sources, last read, last sockets, last services */ }
impl PortReader {
    pub fn new(sockets: Box<dyn FnMut() -> Result<Vec<Socket>, SocketError> + Send>,
               services: Box<dyn FnMut() -> Result<HashMap<u32, Vec<String>>, String> + Send>) -> Self;
    pub fn live() -> Self;
    /// Reads again when due (first call, PORTS_REFRESH passed, or `force`); otherwise the last list.
    pub fn sockets(&mut self, now: Instant, force: bool) -> (&[Socket], &HashMap<u32, Vec<String>>, Option<String>);
}

pub fn owners_from(processes: &[ProcessSample], services: &HashMap<u32, Vec<String>>) -> HashMap<u32, PortOwner>;
pub fn build_port_rows(sockets: &[Socket], owners: &HashMap<u32, PortOwner>, ids: &mut PortIds) -> Vec<PortRow>;
pub fn state_label(s: &Socket) -> &'static str;
pub fn address_text(a: &SocketAddr) -> String;
pub fn is_listening(s: &Socket) -> bool;
pub fn filter_port_rows(rows: &[PortRow], view: PortView, query: &str) -> Vec<PortRow>;
/// Columns: 0 App, 1 PID, 2 Protocol, 3 Local address, 4 Local port, 5 Remote address,
/// 6 Remote port, 7 State. Ties: local port, protocol, PID, id.
pub fn sort_port_rows(rows: &mut [PortRow], col: i32, desc: bool);
pub fn count_text(n: usize, query: &str) -> String;
pub fn port_target(row: &PortRow, owners: &HashMap<u32, PortOwner>) -> Option<crate::view::Target>;
```

```rust
// crates/app/src/view.rs
#[derive(Clone, Debug, Default)]
pub struct PortsFrame {
    pub rows: Vec<crate::ports::PortRow>,
    pub targets: HashMap<i32, Target>,
    pub error: Option<String>,
}
// Frame gains: pub ports: Option<PortsFrame>,

// crates/app/src/controls.rs
// Page::Ports = 6; Controls::request_ports_refresh(&self); Controls::take_ports_refresh(&self) -> bool
```

```slint
// crates/app/ui/portspage.slint
export struct PortData {
    id: int, app: string, sub: string, has-icon: bool, icon: image, pid: string,
    protocol: string, local-addr: string, local-port: string, remote-addr: string,
    remote-port: string, state: string, actionable: bool, can-locate: bool,
}
export component PortsPage inherits VerticalLayout {
    in property <[PortData]> rows;
    in property <int> view;               // 0 Listening, 1 All
    in-out property <string> search-text;
    in property <int> sort-column;
    in property <bool> sort-descending;
    in property <string> count-text;
    in property <length> bottom-inset;
    in property <bool> interactive;
    callback set-view(int);
    callback search(string);
    callback sort-clicked(int);
    callback refresh();
    callback port-action(string, int);   // "goto" | "end-task" | "end-tree" | "open-location"
}
// main.slint: port-rows, ports-view, ports-search-text, ports-sort-column (default 4),
// ports-sort-descending, ports-count-text; callbacks search-ports(string), sort-ports(int),
// set-ports-view(int), refresh-ports(), port-action(string, int).
```

The action strings are the ones `dispatch::plan` already accepts for app rows. The builder copies
them from `dispatch.rs`; the names above are placeholders if they differ.

## Test plan

`crates/collector/src/sockets.rs` (unit)

- `port_from_mib(0x0000_3514)` → 5173; `port_from_mib(0x0000_5000)` → 80;
  `port_from_mib(0xFFFF_3514)` → 5173 (high bits ignored)
- `tcp_state` maps each of 1..=12 to `Closed`, `Listen`, `SynSent`, `SynReceived`, `Established`,
  `FinWait1`, `FinWait2`, `CloseWait`, `Closing`, `LastAck`, `TimeWait`, `DeleteTcb` (12 cases);
  `99` → `Unknown(99)`; `0` → `Unknown(0)`
- `tcp4_row` with state Listen, local `0.0.0.0:5173`, remote `0.0.0.0:0`, PID 42 →
  `Socket { Tcp, 0.0.0.0:5173, remote None, Some(Listen), 42 }`
- `tcp4_row` Established, remote `93.184.216.34:443` → remote `Some(93.184.216.34:443)`
- `tcp4_row` TimeWait, PID 0 → state `TimeWait`, pid 0, remote kept
- `tcp6_row` Listen on `::1`, scope 0 → local `[::1]:port`, remote None
- `tcp6_row` with local `fe80::1` scope 12 → `SocketAddrV6` with scope id 12
- `udp4_row` → protocol Udp, state None, remote None; `udp6_row` the same for `::`
- `read_table`: first call returns NO_ERROR → that buffer
- `read_table`: INSUFFICIENT_BUFFER asking 64, then NO_ERROR → called twice, second buffer is 64 bytes
- `read_table`: INSUFFICIENT_BUFFER on all 4 tries → `KeptGrowing { table }`, exactly 4 calls
- `read_table`: `ERROR_INVALID_PARAMETER` → `Api { code: 87 }`, no retry
- `SocketError` display text for both variants names the table
- `Socket::key` equals for the same tuple and differs when only the PID differs

`crates/collector/src/sockets.rs` (live, not ignored, no elevation)

- a `TcpListener` on `127.0.0.1:0` appears in `list_sockets()` as Tcp, Listen, that port, our PID
- a `UdpSocket` on `127.0.0.1:0` appears as Udp, state None, that port, our PID
- a `TcpListener` on `[::1]:0` appears with an IPv6 local address (skipped with a printed reason
  when IPv6 loopback can't be bound on the machine)
- an accepted TCP connection between two local sockets appears as Established with the peer as remote

`crates/app/src/ports.rs` (unit)

- owner known: row `app` = owner name, `sub` = services joined ", ", icon = image path,
  `actionable` and `can_locate` true
- owner known, no image path: `can_locate` false, `actionable` true
- svchost owner with services `RpcSs`, `EventLog` → `sub` "RpcSs, EventLog"
- PID 4 → "System" / "Kernel driver (for example HTTP.sys)", not actionable, cannot locate
- PID 0 → "No owner", not actionable, cannot locate
- PID missing from owners → "Ended", not actionable, cannot locate
- `state_label`: all 12 TCP states → "Closed", "Listening", "SYN sent", "SYN received",
  "Established", "FIN wait 1", "FIN wait 2", "Close wait", "Closing", "Last ACK", "Time wait",
  "Delete TCB"; `Unknown(_)` → "Unknown"; UDP → "Open"
- `address_text`: `0.0.0.0`, `127.0.0.1`, `::`, `::1`, `fe80::1%12`
- `is_listening`: TCP Listen yes; TCP Established, TimeWait, SynSent no; UDP yes
- `filter_port_rows` Listening with no query hides Established and TimeWait rows and keeps
  TCP Listen and UDP; All keeps every row
- query "80" matches local port 80, remote port 80 and PID 80; does not match 8080, 180 or PID 800
- query " 5173 " (spaces) acts as "5173"; query "" and "   " filter nothing
- query "CHROME" matches app "Google Chrome"; "rpcss" matches a service; "chrome.exe" matches
  the process name; "127.0.0" matches an address; "time" matches "Time wait" in All
- query "5173" in Listening does not show an Established row on 5173 (the view still applies)
- `sort_port_rows` default (col 4 asc): ports 9 before 10 before 5173 (numeric, not text)
- each column 0..=7 ascending and descending orders a 3-row fixture as expected (16 cases); the
  tie on equal values falls to local port, protocol, PID
- remote port `None` sorts before any number ascending
- `count_text(1, "")` → "1 port"; `(3, "")` → "3 ports"; `(0, "")` → "0 ports";
  `(3, "5173")` → "3 ports match 5173"; `(1, "5173")` → "1 port matches 5173"
- `PortIds`: same key twice → same id; two keys → different ids; after `retain` drops a key and
  it comes back, it gets a new id (ids never reused)
- `port_target` for a known owner → root and procs `[(pid, create_time)]`, image path, name;
  `critical` true for `csrss.exe`; `None` for PID 4, PID 0 and an ended PID
- `PortReader`: first call reads; a call 1 s later does not read; a call at 2 s reads; `force`
  reads at 0.5 s (counted through the fake source)
- `PortReader`: a socket read error returns the last list and the error text; the next success
  clears the error
- `PortReader`: a services read error returns the sockets with empty services and no error text
- `owners_from` uses `description`, then `name`; keeps `create_time` and `image_path`

`crates/app/src/controls.rs` (unit)

- `Page::from_index(6)` → Ports; `Page::Ports.index()` → 6; `name()` → "ports";
  `from_name("ports")` → Ports; `Page::ALL` has 7 pages
- `restart_args(Page::Ports)` round-trips through `page_from_args`
- `take_ports_refresh` is false by default, true once after `request_ports_refresh`, then false

`crates/app/src/worker.rs` (unit, `LiveSource` with fake port sources)

- on `Page::Ports` the frame has `ports` with rows and targets, and no Overview rows or perf
- on `Page::Overview` the fake socket source is never called
- window hidden on Ports: no frame and no socket read
- two ticks within 2 s read the sockets once; `request_ports_refresh` between them makes it two
- a socket read error gives `PortsFrame.error` and the previous rows

`crates/app/src/bridge.rs` (unit, existing bridge test setup)

- a Ports frame with an error after a good one keeps the rows and sets one toast; a second error
  frame sets no second toast; a good frame ends the streak, so the next error toasts again
- `set_ports_search("5173")` filters the pushed rows without a new frame
- `sort_ports` on the same column twice flips the direction
- `port_target(id)` for an actionable row returns the frame's target; `None` for a PID 4 row

`crates/app/tests/ui.rs`

- the rail has a "Ports page" button; clicking it sets `page` to 6
- with `port-rows` set and `page` 6: the "Listening" and "All" controls exist; the count text shows
- a row with `actionable: false` has its End task menu item disabled

### Exemptions from test-first (manual checks replace them)

- **Visual layout of `portspage.slint`** (column widths, two-line App cell, icon): no snapshot
  test exists for pages in this repo. Covered by manual steps 1–3.
- **Real owners on the user's machine** (svchost services, HTTP.sys on PID 4, a protected process
  with no image path): the machine's state can't be fixed in a test. Covered by manual steps 4–5.
- **The 20 ms read budget**: not a test target (spec anti-goal says measured once). The worker
  logs any read over 20 ms. Covered by manual step 6.

### Manual verification

1. `./scripts/try.ps1`, run `try\negotium.exe`, open **Ports**: the list shows listening TCP and
   UDP sockets, sorted by local port.
2. Run `npx vite` (or any server on 5173), type `5173` in the search: one or two rows (IPv4/IPv6)
   with node's icon and PID; `80` does not show 8080.
3. Switch to **All**: Established and Time wait rows appear; sort by each column.
4. A row on port 135 shows `svchost.exe` with `RpcSs` under it. If something listens on 80 or
   5357 through HTTP.sys, the row shows "System" / "Kernel driver (for example HTTP.sys)" with
   End task disabled.
5. Right-click the node row → **End task**: the confirmation and result match the Overview; the
   row disappears within 2 s. **Go to process** opens the Overview filtered to that PID.
6. Leave the page open for a minute; `negotium.log` has no "ports read took" line (no read over
   20 ms). Switch away; a debugger or the log shows no further reads.
7. Non-elevated and elevated runs both show owners.

## Deviations

- Spec goal 2 shows `[::1]` for an IPv6 address. Rows show `::1` (no brackets), because the port
  is in its own column (decision 9).
- Spec goal 8 says "every 2 s". The tables are read every 2 s, but the rows are rebuilt on each
  worker tick, so with the refresh setting at 0.5 s an owner that ends disappears sooner
  (decision 2).

## Out of scope

- Traffic per connection, closing one TCP connection, reverse DNS, GeoIP.
- A tray action or shortcut "What is on port…".
- History of sockets, notifications when a port opens.
- `netsh http` URL reservations behind PID 4.
- Merging the `0.0.0.0` and `::` rows of one listener into one row (the spec leaves it open; the
  default, two rows, is kept).
- Follow-up named here: per-connection bytes from the ETW feed when elevated.

## Definition of done

- [ ] `cargo test --workspace` passes, including every test named above
- [ ] `list_sockets` returns a test's own TCP and UDP listeners with its PID, unelevated (live tests)
- [ ] A MIB port in network byte order is parsed in one function and tested (`port_from_mib`)
- [ ] A table that keeps growing fails after 4 tries with `KeptGrowing`, never a partial list
- [ ] Rail shows **Ports** between Services and Settings; `--page=ports` reopens it (controls + ui tests)
- [ ] Listening is the default view; All adds the other TCP states (filter tests, manual 3)
- [ ] A digits-only query matches a port or PID exactly (filter tests, manual 2)
- [ ] PID 4, PID 0 and ended owners show their labels and have no actions (ports tests, ui test, manual 4)
- [ ] End task, End process tree, Open file location and Go to process work from a row (bridge test, manual 5)
- [ ] Sockets are read only while the page is on screen and the window is visible, at most every 2 s unless Refresh (worker tests, manual 6)
- [ ] A read failure keeps the last rows and shows one toast per failure streak (worker + bridge tests)
- [ ] No read over 20 ms in a minute on the user's machine (manual 6)
- [ ] `cargo clippy --workspace -- -D warnings` is clean

## Acceptance criteria

1. When the user opens Ports, the app lists every listening TCP socket and every UDP socket with
   its owner app, PID, addresses, port and state. → DoD 2, 5, 6
2. When the user types a port number, only sockets on that exact port (or that PID) stay. → DoD 7
3. When a socket's owner is the kernel, has no owner, or has ended, the row says so and offers no
   action. → DoD 8
4. When the user picks an action on a row, it behaves as the same action on the Overview. → DoD 9
5. When the page is not on screen, the app reads no socket tables. → DoD 10
6. When Windows fails to give the tables, the page keeps the last list and tells the user once.
   → DoD 4, 11
7. The page refresh costs less than 20 ms. → DoD 12

## Phases

One phase, two commits on `feat/ports-page`:

1. `Add failing tests for the Ports page`: the stubs above, returning typed empty or default
   values (`Ok(vec![])`, `0`, `TcpState::Unknown(0)`, `String::new()`, `None`, `false`; no
   `todo!()`, so every test fails on an assertion, not a panic), the Slint page with its
   properties and no layout, and every test above. The live socket tests fail because
   `list_sockets` returns an empty list.
2. `Implement the Ports page`.
