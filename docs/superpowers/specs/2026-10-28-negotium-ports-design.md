# Negotium — Ports page (design)

Date: 2026-10-28. A new rail page that answers "what is on port N?".

## Why

The user often finds a port in use (a dev server won't start on 5173, something already holds
8080) and has to drop to `netstat -ano` plus a PID lookup to find the owner. Negotium already
knows every process, its icon and its services; it should answer this in one search and let the
user end the owner from the same row.

## Goals

1. **A "Ports" rail page** (`Page::Ports = 6`), placed after Services and before Settings. Rail
   label "Ports" (my call: the word the user searches for; "Network" is already a Performance
   page). The page is remembered across restarts like the others (`Page::ALL`).
2. **One row per socket**, TCP and UDP, IPv4 and IPv6, from `GetExtendedTcpTable`
   (`TCP_TABLE_OWNER_PID_ALL`) and `GetExtendedUdpTable` (`UDP_TABLE_OWNER_PID`) for `AF_INET`
   and `AF_INET6`. Columns:

   | Column         | Content                                                              |
   |----------------|----------------------------------------------------------------------|
   | App            | Icon and display name, as the Overview list shows the process        |
   | PID            | Owner PID                                                            |
   | Protocol       | `TCP` or `UDP`                                                       |
   | Local address  | As Windows gives it: `0.0.0.0`, `127.0.0.1`, `::`, `[::1]`, …        |
   | Local port     | Number                                                               |
   | Remote address | TCP only; empty for UDP and for listening TCP sockets                |
   | Remote port    | Same rule                                                            |
   | State          | TCP state (`Listening`, `Established`, `Time wait`, …); UDP: `Open`  |

   Every column sorts. The default sort is Local port, ascending, then Protocol, then PID.
3. **Two views**, a segmented control at the top: **Listening** (default) and **All**.
   Listening shows TCP sockets in `LISTEN` and every UDP endpoint (UDP has no state; a bound UDP
   socket is what holds a port). All adds every other TCP state.
4. **Search box** (same look as the Services search). A query of digits only matches a row
   whose local port, remote port or PID is **exactly** that number, so `80` does not also match
   `8080` (my call). Any other query is a case-insensitive substring match over app name, process
   name, service names, addresses and state.
5. **Services on svchost rows.** When the owner hosts services, the App cell shows the process
   name and, under it, the service names (e.g. `svchost.exe` / `RpcSs`). The search matches them.
6. **System rows.** PID 4 shows as "System" with the second line "Kernel driver (for example
   HTTP.sys)". PID 0 (TCP sockets in `Time wait` that no longer have an owner) shows as
   "No owner". End task and End process tree are off for both.
7. **Row actions** (context menu and keyboard, as on the Services page): **Go to process**
   (`select_pid`, as Services does), **End task**, **End process tree**, **Open file location**.
   They use the existing actions and their existing confirmations and failure toasts.
8. **Refresh.** The list loads when the page opens, then every 2 s while the page is on screen
   and the window is visible (my call), plus a Refresh button. Nothing is read while the page is
   hidden. A refresh keeps the selection and scroll position when the selected socket
   (protocol, local address, local port, remote address, remote port, PID) still exists.
9. **Not elevated works.** The owner PID comes from the tables without admin rights. When the
   image path can't be read (protected process), the row shows the process name from the
   snapshot and a generic icon.
10. **Status line** under the search: "N ports" for the current view and filter, e.g.
    "3 ports match 5173".

## Already there (not new work)

- `Win32_NetworkManagement_IpHelper` and `Win32_Networking_WinSock` in the collector's
  `windows` features.
- The Services page pattern: background list job (`services::list_job` / `spawn_job` /
  `begin_job`), loading flag, sort and filter helpers, context menu, and the `goto` action
  through `select_pid`.
- `negotium_actions::services::list_services` (gives PID → service names).
- Process names, image paths and icons from the latest snapshot; End task, End process tree and
  Open file location actions.

## Non-goals

- Per-connection traffic (bytes per socket). The ETW session in `netetw.rs` counts per process
  only; a later spec can add it for elevated runs.
- Closing a single TCP connection (`SetTcpEntry` with `MIB_TCP_STATE_DELETE_TCB`).
- Reverse DNS, GeoIP or any lookup of remote addresses.
- A tray action or global shortcut "What is on port…".
- History of past sockets, or a notification when a port opens.
- URL reservations (`netsh http show urlacl`): the System row says HTTP.sys and stops there.

## Anti-goals

- Cost while hidden: no socket tables are read unless the Ports page is on screen and the
  window is visible.
- A slow page: one refresh (both tables, both families, plus the row build) stays under 20 ms
  median on the user's machine with a normal number of sockets (my call; measured once with a
  log timing, not a bench target).
- A blank list after a failure: a failed refresh keeps the last list and shows a toast
  ("Couldn't list ports: …"), as Services does.
- Any change to the system: the page only reads.

## Constraints

- Rust + `windows` 0.62.2. The Windows calls live in the collector crate (new `sockets.rs`); the
  table-to-row parsing, the row build (join with snapshot processes and services), filter and
  sort are pure functions with unit tests on data.
- Ports in the MIB rows are in network byte order in the low 16 bits of a `u32`; IPv6 rows carry
  a scope id. Both are parsed in one tested function, not at each call site.
- The tables can grow between the size call and the read: the reader retries on
  `ERROR_INSUFFICIENT_BUFFER` with the new size, a bounded number of times, then fails with an
  error (never a partial list).
- A PID in the table that is not in the snapshot (the process ended, or started after the last
  tick) shows the PID with the name "Ended" and no actions.
- PID reuse: the tables give no create time, so a row joins on PID only. Accepted; the 2 s
  refresh keeps the window small.
- An integration test binds a `TcpListener` and a `UdpSocket` on `127.0.0.1:0`, reads the
  tables, and finds both with the test's own PID and port. It needs no elevation.
- UI text follows ASD-STE100.

## Left to the builder

- Whether the list job reads the snapshot through the bridge or receives the process map with
  the job; how the 2 s refresh timer is driven (Slint `Timer` on the page, or the worker tick).
- Column widths and the exact second-line styling for service names.
- Whether rows with the same PID, protocol and port on `0.0.0.0` and `::` stay as two rows (the
  default, it is what Windows reports) or show as one with both addresses.
