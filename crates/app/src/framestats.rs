//! Optional render-time statistics for the benchmark (NEGOTIUM_FRAME_STATS=<path>).
//! The first completed frame (GL initialisation and first paint) is reported separately as
//! `first_frame_ms` and excluded from `render_ms` and `over_16ms`; `frames` counts every frame.
use std::time::Instant;

#[derive(Default)]
pub struct FrameStats {
    started: Option<Instant>,
    first_frame_ms: Option<f64>,
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
        if self.first_frame_ms.is_none() {
            self.first_frame_ms = Some(ms);
        } else {
            self.ms.push(ms);
        }
    }
    pub fn summary_json(&self) -> String {
        let mut v = self.ms.clone();
        v.sort_by(f64::total_cmp);
        let at = |p: f64| if v.is_empty() { 0.0 } else { v[((v.len() - 1) as f64 * p).round() as usize] };
        let mean = if v.is_empty() { 0.0 } else { v.iter().sum::<f64>() / v.len() as f64 };
        serde_json::json!({
            "frames": v.len() + usize::from(self.first_frame_ms.is_some()),
            "first_frame_ms": self.first_frame_ms.unwrap_or(0.0),
            "render_ms": { "median": at(0.5), "p95": at(0.95), "max": at(1.0), "mean": mean },
            "over_16ms": v.iter().filter(|&&m| m > 16.0).count(),
        })
        .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(values: &[f64]) -> serde_json::Value {
        let mut s = FrameStats::default();
        for &ms in values {
            s.push_ms(ms);
        }
        serde_json::from_str(&s.summary_json()).unwrap()
    }

    #[test]
    fn summarises_and_counts_slow_frames() {
        // The first push is the first-paint frame; the rest are summarised.
        let json = summary(&[3.0, 2.0, 4.0, 20.0]);
        assert_eq!(json["frames"], 4);
        assert_eq!(json["first_frame_ms"], 3.0);
        assert_eq!(json["over_16ms"], 1);
        assert_eq!(json["render_ms"]["max"], 20.0);
    }

    #[test]
    fn first_frame_does_not_count_toward_over_16ms_or_max() {
        let json = summary(&[183.0, 2.0, 4.0]);
        assert_eq!(json["frames"], 3);
        assert_eq!(json["first_frame_ms"], 183.0);
        assert_eq!(json["over_16ms"], 0);
        assert_eq!(json["render_ms"]["max"], 4.0);
    }

    #[test]
    fn empty_and_single_frame_are_zeroed() {
        let json = summary(&[]);
        assert_eq!(json["frames"], 0);
        assert_eq!(json["first_frame_ms"], 0.0);
        let json = summary(&[50.0]);
        assert_eq!(json["frames"], 1);
        assert_eq!(json["first_frame_ms"], 50.0);
        assert_eq!(json["over_16ms"], 0);
        assert_eq!(json["render_ms"]["max"], 0.0);
    }
}
