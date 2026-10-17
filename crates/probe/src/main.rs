//! Console live view of the data core. Prints system totals and the top 10 apps
//! each tick; optionally records snapshots (JSON lines) and tick timings.
use anyhow::{Context, Result, bail};
use negotium_collector::Collector;
use negotium_history::History;
use negotium_model::{Metric, Model, Ranker, Sort, SortColumn};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::time::{Duration, Instant};

struct Args {
    metric: Metric,
    ticks: Option<u64>,
    interval: Duration,
    timing: bool,
    record: Option<String>,
}

fn parse_args() -> Result<Args> {
    let mut args = Args { metric: Metric::Cpu, ticks: None, interval: Duration::from_millis(1000), timing: false, record: None };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--metric" => {
                args.metric = match it.next().as_deref() {
                    Some("cpu") => Metric::Cpu,
                    Some("memory") => Metric::Memory,
                    Some("disk") => Metric::Disk,
                    Some("gpu") => Metric::Gpu,
                    other => bail!("unknown metric {other:?} (use cpu|memory|disk|gpu)"),
                }
            }
            "--ticks" => args.ticks = Some(it.next().context("--ticks needs a number")?.parse()?),
            "--interval-ms" => args.interval = Duration::from_millis(it.next().context("--interval-ms needs a number")?.parse()?),
            "--timing" => args.timing = true,
            "--record" => args.record = Some(it.next().context("--record needs a path")?),
            other => bail!("unknown argument {other}"),
        }
    }
    Ok(args)
}

fn fmt_bytes(b: f64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    format!("{v:.1} {}", UNITS[u])
}

fn main() -> Result<()> {
    let args = parse_args()?;
    let mut collector = Collector::new();
    let mut model = Model::new();
    let mut ranker = Ranker::new();
    let mut history = History::new();
    let mut recorder = match &args.record {
        Some(path) => Some(BufWriter::new(File::create(path).with_context(|| format!("create {path}"))?)),
        None => None,
    };
    let mut tick_ms: Vec<f64> = Vec::new();
    let mut n = 0u64;

    loop {
        let started = Instant::now();
        let snap = collector.tick();
        tick_ms.push(started.elapsed().as_secs_f64() * 1000.0);
        let rows = model.update(&snap);
        history.push(snap.taken_at_ms, &snap.totals, &rows);
        let order = ranker.order(&rows, Sort::by(SortColumn::Metric(args.metric)), snap.taken_at_ms, false);

        let t = &snap.totals;
        println!(
            "\n[{:>7} ms] CPU {:>5.1}%  Mem {}/{}  Disk {:>5.1}%  GPU {:>5.1}%  Net {}/s  ({} procs, {} apps)",
            snap.taken_at_ms,
            t.cpu_pct.unwrap_or(f32::NAN),
            fmt_bytes(t.memory.as_ref().map_or(0, |m| m.in_use()) as f64),
            fmt_bytes(t.memory.as_ref().map_or(0, |m| m.total) as f64),
            t.busiest_disk_pct().unwrap_or(f32::NAN),
            t.gpu_pct.unwrap_or(f32::NAN),
            fmt_bytes(t.net_bps.unwrap_or(0.0)),
            snap.processes.len(),
            rows.len(),
        );
        for &i in order.iter().take(10) {
            let r = &rows[i];
            let hung = r.hung_for_ms.map(|ms| format!("  NOT RESPONDING {}s", ms / 1000)).unwrap_or_default();
            println!(
                "  {:<28} {:>3} procs  CPU {:>5.1}%  Mem {:>10}  Disk {:>10}/s  GPU {:>5.1}%{}",
                r.name, r.processes.len(), r.cpu_pct, fmt_bytes(r.private_bytes as f64), fmt_bytes(r.disk_bps),
                r.gpu_pct.unwrap_or(0.0), hung
            );
        }
        if !snap.source_errors.is_empty() {
            println!("  source errors: {:?}", snap.source_errors);
        }
        if let Some(w) = recorder.as_mut() {
            serde_json::to_writer(&mut *w, &snap)?;
            w.write_all(b"\n")?;
        }

        n += 1;
        if args.ticks.is_some_and(|max| n >= max) {
            break;
        }
        std::thread::sleep(args.interval.saturating_sub(started.elapsed()));
    }

    if args.timing {
        let mut sorted = tick_ms.clone();
        sorted.sort_by(f64::total_cmp);
        let pct = |p: f64| sorted[((sorted.len() - 1) as f64 * p).round() as usize];
        println!(
            "\ncollector tick over {} ticks: first {:.2} ms, median {:.2} ms, p95 {:.2} ms, max {:.2} ms; history {} bytes",
            sorted.len(), tick_ms[0], pct(0.5), pct(0.95), pct(1.0), history.approx_bytes()
        );
    }
    Ok(())
}
