# Benchmarks

Machine: AMD Ryzen 9 7950X3D 16-Core Processor (16 cores, 32 logical processors), 95.2 GB RAM, Windows 11 Pro (NT 10.0.26300).

## Collector tick (negotium-probe --release --ticks 60 --timing)

```
collector tick over 60 ticks: first 36.59 ms, median 18.73 ms, p95 24.75 ms, max 36.59 ms; history 41783 bytes
```

A second run on the same machine: first 48.20 ms, median 19.09 ms, p95 26.51 ms, max 48.20 ms (history 40981 bytes).
Measured with about 680 processes running and a normal desktop workload (system CPU 20-30 %).

Target (spec §8): median ≤ 5 ms. **Not met: median is about 19 ms.**

Per-source median cost, measured by timing each source on its own (release build, 40 ticks, 300 ms apart):

| Source | Median |
| --- | --- |
| Process list (single `NtQuerySystemInformation` call plus parse) | about 15-17 ms |
| PDH counters (CPU, disks, network, GPU) | about 5.2-5.6 ms |
| Window scan | about 0.8-1.4 ms |
| Identity cache (steady state) | about 0.05-0.2 ms |
| Memory totals | about 0.003 ms |

The process list dominates. It is one kernel call whose cost scales with the process and thread count of this machine, not Rust-side work, so the target is a machine-dependent follow-up and is left unchanged. The first tick is slower (identity cache fill, PDH warm-up).

## CPU accuracy

`cargo test -p negotium-collector --test accuracy`: one busy thread reports within ±35 % of 100 / logical_cpus percent. Tolerance unchanged.

## Task Manager baseline

See Task 12.
