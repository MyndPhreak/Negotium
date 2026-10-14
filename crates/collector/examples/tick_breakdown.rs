//! Times each collector source on its own, plus a whole `Collector::tick()`, so the
//! tick cost can be attributed. Usage:
//!   cargo run -p negotium-collector --example tick_breakdown --release -- [--ticks N] [--interval-ms MS]
//! Defaults match the probe's timing run: 60 iterations, 1000 ms apart. Times are in ms.
use negotium_collector::{Collector, IdentityCache, PdhSources, ProcessReader, memory_totals, scan_windows};
use std::collections::HashSet;
use std::ffi::c_void;
use std::time::{Duration, Instant};

const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC000_0004_u32 as i32;

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQuerySystemInformation(class: u32, info: *mut c_void, len: u32, return_len: *mut u32) -> i32;
}

/// One raw SystemProcessInformation (class 5) call into `buf` (8-byte-aligned words).
fn nt_call(buf: &mut [u64]) -> (i32, u32) {
    let mut needed = 0u32;
    // SAFETY: `buf` is a writable, 8-byte-aligned buffer of `buf.len() * 8` bytes.
    let status = unsafe { NtQuerySystemInformation(5, buf.as_mut_ptr().cast(), (buf.len() * 8) as u32, &mut needed) };
    (status, needed)
}

fn ms(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}

fn main() {
    let mut ticks = 60usize;
    let mut interval = Duration::from_millis(1000);
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--ticks" => ticks = it.next().and_then(|v| v.parse().ok()).expect("--ticks needs a number"),
            "--interval-ms" => {
                interval = Duration::from_millis(it.next().and_then(|v| v.parse().ok()).expect("--interval-ms needs a number"))
            }
            other => panic!("unknown argument {other}"),
        }
    }

    // Raw-call buffer: grown until it fits, before any timing.
    let mut buf: Vec<u64> = vec![0; 512 * 1024 / 8];
    loop {
        let (status, needed) = nt_call(&mut buf);
        if status == STATUS_INFO_LENGTH_MISMATCH {
            let want = (needed as usize + 64 * 1024).div_ceil(8);
            buf.resize(want.max(buf.len() * 2), 0);
        } else {
            assert!(status >= 0, "NtQuerySystemInformation failed: {status:#010x}");
            break;
        }
    }

    let mut reader = ProcessReader::new();
    let mut identity = IdentityCache::new();
    let mut pdh = PdhSources::open().expect("PDH open");
    let mut collector = Collector::new();

    const COLS: [&str; 7] = ["nt_call", "process_read", "identity", "pdh", "windows", "memory", "tick"];
    let mut samples: [Vec<f64>; 7] = Default::default();
    let mut process_count = 0usize;

    for i in 0..ticks {
        let started = Instant::now();

        let t = Instant::now();
        let (status, _) = nt_call(&mut buf);
        samples[0].push(ms(t));
        assert!(status >= 0, "NtQuerySystemInformation failed: {status:#010x}");

        let t = Instant::now();
        let raw = reader.read().expect("process read");
        samples[1].push(ms(t));
        process_count = raw.len();

        let alive: HashSet<(u32, i64)> = raw.iter().map(|p| (p.pid, p.create_time)).collect();
        identity.retain_alive(&alive);
        let t = Instant::now();
        for p in &raw {
            identity.get(p.pid, p.create_time);
        }
        samples[2].push(ms(t));

        let t = Instant::now();
        let _ = pdh.read();
        samples[3].push(ms(t));

        let t = Instant::now();
        let _ = scan_windows();
        samples[4].push(ms(t));

        let t = Instant::now();
        let _ = memory_totals();
        samples[5].push(ms(t));

        let t = Instant::now();
        let _ = collector.tick();
        samples[6].push(ms(t));

        if i + 1 < ticks {
            std::thread::sleep(interval.saturating_sub(started.elapsed()));
        }
    }

    println!("{ticks} iterations, {} ms apart, {process_count} processes (ms)", interval.as_millis());
    println!("{:<14} {:>9} {:>9} {:>9} {:>9}", "column", "first", "median", "p95", "max");
    for (name, v) in COLS.iter().zip(&samples) {
        let first = v[0];
        let mut sorted = v.clone();
        sorted.sort_by(f64::total_cmp);
        let pct = |p: f64| sorted[((sorted.len() - 1) as f64 * p).round() as usize];
        println!("{name:<14} {first:>9.3} {:>9.3} {:>9.3} {:>9.3}", pct(0.5), pct(0.95), pct(1.0));
    }
}
