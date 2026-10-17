# Negotium — Per-app network (design)

Date: 2026-10-17. First of the Plan 4 projects (later: installer + admin mode + Ctrl+Shift+Esc;
CPU temperature add-on). Amends spec §11 (the SRUM risk) of `2026-10-14-negotium-design.md`.

## Why

The app list's Network column shows "—" for every app, and the Network meter's "Top network
users" tiles and the Performance network pages' hover list can't name anyone. The user wants
per-app network use. Windows only gives reliable live per-process network numbers to an
elevated reader, which the user accepted ("that's fine"): the column fills in when Negotium runs
as admin ("Restart as admin" today).

**Admin mode moves to the installer plan** (the user's decision): a "Run with highest privileges"
task must start an exe only admins can change, and Negotium runs from a user-writable folder
until it is installed to Program Files. The admin-mode behaviour agreed in brainstorming (one UAC
prompt to turn it on; every non-elevated launch hands over to the task; a broken task falls back
to non-elevated with the banner, no dialogs, no launch loop; only for administrator accounts) is
recorded here for that plan.

## Goals

### Per-app network

1. While Negotium runs elevated, it reads a live per-process network feed and fills the
   **Network** value of every app card and every process row, in the same bit-rate format as
   the Network meter (`format::bit_rate`). Upload and download are summed in the list (as Task
   Manager does). Loopback traffic (an app talking to itself / to local processes) does not
   count, so no app exceeds the machine total on the Network meter (my call).
2. The Network column then sorts, draws its bar (relative to the busiest app this frame, as the
   card list already does for Network), and the Network meter's top tiles ("Top network users")
   list real apps.
3. The Performance network pages' hover list (the scrub) lists the busiest apps by network, with
   upload and download shown separately for the live sample (past samples: total only). Per-app traffic is across all
   adapters (Windows does not attribute it to an adapter), the same way per-app disk is across
   all disks; the hover list says so.
4. History keeps per-app network like the other per-app series (one summed value per app, no
   change to the history crate's shape), so the scrub works for past samples too; past samples
   show the total only, not the split (my call).
5. Not elevated: the Network column stays "—" and the existing "Restart as admin" banner covers it
   (its text mentions network use). No dialogs. If the trace session can't start while elevated
   (e.g. the system's session limit), the column stays "—", the reason is logged once, and the
   rest of the app works.

## Already there (not new work)

- `negotium_actions::restart_as_admin` (ShellExecuteEx `runas`) and `controls::restart_args`
  (keep the page across a restart).
- `AppRow.net_bps: Option<f64>` (always `None` today), the Network column, `Metric::Network`
  sorting and bars, the scrub for network pages (says per-app is unavailable today).

## Non-goals

- No per-app network when not elevated (no undocumented SRUM/Task Manager hooks; the old spec's
  SRUM plan is dropped).
- No per-adapter per-app breakdown; no per-connection view (remote addresses, ports).
- Admin mode, the installer, Ctrl+Shift+Esc and CPU temperature are later plans.

## Anti-goals

- Visibly higher CPU: the feed sends an event per packet batch; Negotium only adds bytes per
  process on the event thread and reads totals once per update. Budget (my call): an elevated
  Overview run costs at most 0.5 % of a core more than the last app-list measurement under
  ordinary network use, and the collector tick median stays ≤ 5 ms (spec §8). Measured with
  `negotium-bench` (elevated run, which the user runs).
- Leaking a kernel trace session: a crash or kill must not leave Negotium's session running
  forever, and a new start must recover a leftover one (stop it and start fresh, or reuse it).
- Any change to the user's machine beyond Negotium's own trace session while it runs.

## Constraints

- Rust + `windows` 0.62.2; the collector and model crates keep their pure / Windows split
  (Windows calls live in the collector and actions crates; the model stays testable on data).
- Tests never touch the user's real settings, log or registry, never open the real window, and
  never start a trace session that outlives the test (sessions in tests use unique names and are
  always stopped; tests needing elevation are `#[ignore]`d with a reason and handed to the user).
- The worker's tick budget (spec §8: collector tick median ≤ 5 ms) holds.
- Elevated runs are handed to the user (agents can't run elevated).

## Left to the builder

- The feed: an ETW real-time session on the Microsoft-Windows-Kernel-Network provider (TCP and UDP,
  IPv4 and IPv6 send/receive events carrying the PID and byte count) is the expected source; the
  session name, buffer sizes, and how a leftover session is detected and recovered.
- How the event thread hands per-process byte counts to the collector (atomics, a locked map
  swapped per tick), and how process exit / PID reuse is handled (create time).
