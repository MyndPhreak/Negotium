//! Text formatting shared by meters, tiles and rows.

pub fn bytes(b: f64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if !b.is_finite() || b < 1024.0 {
        return format!("{} B", b.max(0.0).round() as u64);
    }
    let mut v = b;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    format!("{v:.1} {}", UNITS[u])
}

pub fn byte_rate(bps: f64) -> String {
    format!("{}/s", bytes(bps))
}

/// Network throughput in bits per second, decimal units (as Task Manager shows).
pub fn bit_rate(bytes_per_s: f64) -> String {
    let bits = (bytes_per_s.max(0.0)) * 8.0;
    if !bits.is_finite() || bits < 1000.0 {
        return format!("{} b/s", bits.round() as u64);
    }
    const UNITS: [&str; 4] = ["Kb/s", "Mb/s", "Gb/s", "Tb/s"];
    let mut v = bits / 1000.0;
    let mut u = 0;
    while v >= 1000.0 && u < UNITS.len() - 1 {
        v /= 1000.0;
        u += 1;
    }
    format!("{v:.1} {}", UNITS[u])
}

pub fn pct(p: f32) -> String {
    format!("{}%", if p.is_finite() { p.round().clamp(0.0, 100.0) } else { 0.0 })
}

pub fn pct1(p: f32) -> String {
    format!("{:.1}%", if p.is_finite() { p.clamp(0.0, 100.0) } else { 0.0 })
}

pub fn hung(ms: u64) -> String {
    let s = ms / 1000;
    if s < 60 { format!("Not responding · {s}s") } else { format!("Not responding · {}m {:02}s", s / 60, s % 60) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_use_binary_units_with_one_decimal() {
        assert_eq!(bytes(0.0), "0 B");
        assert_eq!(bytes(512.0), "512 B");
        assert_eq!(bytes(1536.0), "1.5 KB");
        assert_eq!(bytes(3.9 * 1024.0 * 1024.0 * 1024.0), "3.9 GB");
        assert_eq!(byte_rate(2.0 * 1024.0 * 1024.0), "2.0 MB/s");
    }

    #[test]
    fn network_is_bits_per_second_in_decimal_units() {
        assert_eq!(bit_rate(0.0), "0 b/s");
        assert_eq!(bit_rate(500_000.0), "4.0 Mb/s");
        assert_eq!(bit_rate(125.0), "1.0 Kb/s");
    }

    #[test]
    fn percentages_and_hung_timer() {
        assert_eq!(pct(37.6), "38%");
        assert_eq!(pct1(18.24), "18.2%");
        assert_eq!(pct1(f32::NAN), "0.0%");
        assert_eq!(hung(14_400), "Not responding · 14s");
        assert_eq!(hung(65_000), "Not responding · 1m 05s");
    }
}
