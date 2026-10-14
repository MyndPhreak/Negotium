//! Optional render-time statistics for the benchmark (NEGOTIUM_FRAME_STATS=<path>).
use std::time::Instant;

#[derive(Default)]
pub struct FrameStats {
    started: Option<Instant>,
    ms: Vec<f64>,
}

impl FrameStats {
    pub fn begin(&mut self) {
        self.started = Some(Instant::now());
    }
    pub fn end(&mut self) {
        if let Some(t) = self.started.take() {
            self.push_ms(t.elapsed().as_secs_f64() * 1000.0);
        }
    }
    pub fn push_ms(&mut self, ms: f64) {
        self.ms.push(ms);
    }
    pub fn summary_json(&self) -> String {
        let mut v = self.ms.clone();
        v.sort_by(f64::total_cmp);
        let at = |p: f64| if v.is_empty() { 0.0 } else { v[((v.len() - 1) as f64 * p).round() as usize] };
        let mean = if v.is_empty() { 0.0 } else { v.iter().sum::<f64>() / v.len() as f64 };
        serde_json::json!({
            "frames": v.len(),
            "render_ms": { "median": at(0.5), "p95": at(0.95), "max": at(1.0), "mean": mean },
            "over_16ms": v.iter().filter(|&&m| m > 16.0).count(),
        })
        .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarises_and_counts_slow_frames() {
        let mut s = FrameStats::default();
        for ms in [2.0, 4.0, 20.0] {
            s.push_ms(ms);
        }
        let json: serde_json::Value = serde_json::from_str(&s.summary_json()).unwrap();
        assert_eq!(json["frames"], 3);
        assert_eq!(json["over_16ms"], 1);
        assert_eq!(json["render_ms"]["max"], 20.0);
    }
}
