//! Percentile helpers for benchmark summaries.
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Summary {
    pub median: f64,
    pub p95: f64,
    pub max: f64,
    pub mean: f64,
}

/// `sorted` must be ascending and non-empty; `p` in [0, 1].
pub fn percentile(sorted: &[f64], p: f64) -> f64 {
    sorted[((sorted.len() - 1) as f64 * p.clamp(0.0, 1.0)).round() as usize]
}

pub fn summary(values: &[f64]) -> Summary {
    if values.is_empty() {
        return Summary { median: 0.0, p95: 0.0, max: 0.0, mean: 0.0 };
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    Summary {
        median: percentile(&sorted, 0.5),
        p95: percentile(&sorted, 0.95),
        max: *sorted.last().unwrap(),
        mean: sorted.iter().sum::<f64>() / sorted.len() as f64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentile_uses_nearest_rank_on_sorted_input() {
        let v = [1.0, 2.0, 3.0, 4.0, 5.0];
        assert_eq!(percentile(&v, 0.5), 3.0);
        assert_eq!(percentile(&v, 1.0), 5.0);
        assert_eq!(percentile(&v, 0.0), 1.0);
    }

    #[test]
    fn summary_of_unsorted_values() {
        let s = summary(&[5.0, 1.0, 3.0]);
        assert_eq!(s, Summary { median: 3.0, p95: 5.0, max: 5.0, mean: 3.0 });
    }

    #[test]
    fn summary_of_empty_is_zero() {
        assert_eq!(summary(&[]), Summary { median: 0.0, p95: 0.0, max: 0.0, mean: 0.0 });
    }
}
