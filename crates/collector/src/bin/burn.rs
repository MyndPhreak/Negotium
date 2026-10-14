//! Test helper: keeps one core busy for the given milliseconds (default 10000).
use std::time::{Duration, Instant};

fn main() {
    let ms: u64 = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(10_000);
    let end = Instant::now() + Duration::from_millis(ms);
    let mut x: u64 = 1;
    while Instant::now() < end {
        x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        std::hint::black_box(x);
    }
}
