# Per-app network — rulings and follow-ups

Plan: `2026-10-17-negotium-app-network.md`. Spec: `../specs/2026-10-17-negotium-app-network-design.md`.

## Rulings made during execution
- `NetRate` is bytes per second (matches `net::hardware_total` and `format::bit_rate`); the plan's "bits per second" wording was wrong.
- The feed is opt-in: `Collector::new`/`with_epoch` never start it; only the app's worker does, so the bench, probe and collector tests never touch the app's session. The app's exit path stops the session by name.
- The session name is per logon session (`Negotium-Net-<session id>`), not the plan's fixed `Negotium-Net`, so another user's elevated Negotium never stops ours.
- A stopped feed retries every 60 s.
- Budget: not claimed as met. The smooth-off comparison shows no clear cost, but the evidence is confounded (fewer frames, run-to-run spread ~1.5 points); see `docs/benchmarks/README.md`.
- Admin mode moved to the installer plan (the user's decision), with the behaviour agreed in brainstorming recorded in the spec.

## Follow-ups
- Measure the feed's cost properly: the same elevated build with the feed on and off, plus the collector tick median.
- The exit stop can race a retry's start (a late session, replaced by the next elevated start).
- The UDP live test needs an IPv4 default route; assert a send succeeded so a missing route reads clearly.
- The hover's upload/download split comes from the current tick while the total comes from the newest kept sample (they may not add up at a 0.5 s refresh); the split is matched by app name; it is absent at the 1 h range when the right column peaks on an older sample; very narrow hover cards can overflow left.
- A leftover session survives an unelevated relaunch until the next elevated run or a reboot.
- Small: `row.net_bps` recomputed per process; an exit within ~1 s of launch can leave a late session.
