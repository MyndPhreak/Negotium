# Benchmarks

Machine: AMD Ryzen 9 7950X3D 16-Core Processor (16 cores, 32 logical processors), 95.2 GB RAM, Windows 11 Pro (NT 10.0.26300).

## Collector tick (negotium-probe --release --ticks 60 --timing)

```
collector tick over 60 ticks: first 36.59 ms, median 18.73 ms, p95 24.75 ms, max 36.59 ms; history 41783 bytes
```

A second run on the same machine: first 48.20 ms, median 19.09 ms, p95 26.51 ms, max 48.20 ms (history 40981 bytes).
Measured with about 680 processes running and a normal desktop workload (system CPU 20-30 %).

Target (spec §8): median ≤ 5 ms. **Not met: median is about 19 ms.**

### Breakdown by source

Command (release build, defaults: 60 iterations, 1000 ms apart, same cadence as the headline run; overridable with `--ticks N --interval-ms MS`):

```
cargo run -p negotium-collector --example tick_breakdown --release
```

Source: `crates/collector/examples/tick_breakdown.rs`. Each iteration times, in this order, one raw `NtQuerySystemInformation(5, ...)` call into a reused buffer (`nt_call`), `ProcessReader::read()` (`process_read`, call plus parse), `IdentityCache::get` for every process from that read on a persistent cache (`identity`), `PdhSources::read()` (`pdh`), `scan_windows()` (`windows`), `memory_totals()` (`memory`), and a whole `Collector::tick()` on a separate collector (`tick`). One run, 674 processes, times in ms:

| Column | First | Median | p95 | Max |
| --- | --- | --- | --- | --- |
| nt_call | 15.075 | 14.958 | 22.242 | 31.640 |
| process_read | 25.901 | 14.488 | 19.799 | 28.830 |
| identity | 11.337 | 0.127 | 0.630 | 11.337 |
| pdh | 4.744 | 6.109 | 7.590 | 26.880 |
| windows | 1.347 | 1.233 | 2.127 | 3.213 |
| memory | 0.003 | 0.003 | 0.004 | 0.005 |
| tick | 41.285 | 22.346 | 30.550 | 41.285 |

What the numbers support:

- The raw `NtQuerySystemInformation` call alone costs about 15 ms median, which is essentially all of `process_read` (14.5 ms median). Parse time derived as `process_read - nt_call` is -0.47 ms at the median, i.e. indistinguishable from zero within run-to-run noise. The two columns are separate calls, so this is a derived estimate, not a direct measurement; it says parsing is small relative to the call, not that it is exactly zero.
- PDH is the second-largest source at about 6 ms median; the window scan is about 1.2 ms; steady-state identity lookups are about 0.13 ms; memory totals are negligible.
- The first-iteration `process_read` (25.9 ms) and `identity` (11.3 ms) are slower: the first `process_read` is the reader's first call, and the first `identity` fills the cache with an `OpenProcess` query per process. The first-tick `Collector::tick()` (41.3 ms) includes both.
- The sum of the medians of `process_read`, `identity`, `pdh` and `windows` (about 22 ms) matches the `tick` median (22.3 ms) in this run. This run's `tick` median (22.3 ms) is higher than the headline probe runs (18.7 and 19.1 ms); absolute values vary between runs (the two probe runs also differ from each other by 0.4 ms). The ordering (process list, then PDH, then everything else) is the same.

The target stays unchanged; the process-list kernel call is the dominant cost on this machine and is the first place to look for savings.

## CPU accuracy

`cargo test -p negotium-collector --test accuracy`: one busy thread reports within ±35 % of 100 / logical_cpus percent. Tolerance unchanged.

## Task Manager baseline (Taskmgr.exe 10.0.26100.9549)

Raw data (not yet recorded): `2026-10-14-taskmgr-baseline.json`

The measurement needs an elevated terminal (Task Manager auto-elevates and UIPI blocks probing higher-integrity windows), so it has not been run yet. Close Task Manager, then run from the repository root in an elevated terminal and fill the table from the JSON:

```
cargo run -p negotium-bench --release -- taskmgr --runs 5 --sample-secs 60 --out docs/benchmarks/2026-10-14-taskmgr-baseline.json
```

The first 5 s after the window appears (`--warmup-secs`, default 5) are excluded from the CPU, working-set and responsiveness samples so the launch burst does not skew them; the `--sample-secs` window starts after the warm-up.

If interrupted with Ctrl+C during sampling, close Task Manager before re-running.

| Metric | Task Manager | Negotium target (spec §8) |
|---|---|---|
| Launch → first window (run 1 is cold, runs 2–5 warm; median of 5) | pending (run elevated) | ≤ 250 ms |
| Launch → first window, cold (`startup_ms[0]`) | pending (run elevated) | ≤ 250 ms |
| CPU while visible, mean / max | pending (run elevated) | ≤ 0.5 % of one core (= 0.0156 % of total on 32 logical processors) |
| Private working set, max | pending (run elevated) | ≤ 60 MB |
| UI responsiveness (WM_NULL), median / p95 / max | pending (run elevated) | no frame > 16 ms (Plan 2 measures frames) |
| Responsiveness timeouts (> 1 s) | pending (run elevated) | 0 |

## Negotium vs Task Manager

Run both from an elevated terminal (close both apps first):

    cargo build --release -p negotium -p negotium-bench
    targetelease
egotium-bench.exe taskmgr  --runs 5 --sample-secs 60 --out docs/benchmarks/taskmgr.json
    targetelease
egotium-bench.exe negotium --runs 5 --sample-secs 60 --out docs/benchmarks/negotium.json

| Metric | Task Manager | Negotium | Target (spec §8) |
|---|---|---|---|
| Launch → first window, run 1 (cold) | pending | pending | ≤ 250 ms |
| Launch → first window, median of 5 | pending | pending | ≤ 250 ms |
| CPU while visible, mean / max (% of total) | pending | pending | ≤ 0.5 % of one core |
| Private working set, max | pending | pending | ≤ 60 MB |
| UI responsiveness (WM_NULL) median / p95 / max | pending | pending | — |
| Render time per frame, max / frames > 16 ms | n/a | pending | none > 16 ms |

## Manual check: frozen windows

1. `cargo run -p negotium-collector --bin negotium-hang --release`
2. In another terminal: `cargo run -p negotium-probe --release -- --ticks 30`
3. After ~8 s the window title shows "Not Responding"; click inside it once.
4. Expected: the probe lists `negotium-hang` with `NOT RESPONDING` on every tick after that, and no `dwm` app row appears.
