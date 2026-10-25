/// Nearest-rank percentiles over milliseconds.
pub struct Summary {
    pub n: usize,
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
    pub max: f64,
}

impl Summary {
    pub fn of(values: &[f64]) -> Option<Self> {
        if values.is_empty() {
            return None;
        }
        let mut v = values.to_vec();
        v.sort_by(f64::total_cmp);
        let at = |p: f64| v[((p * v.len() as f64).ceil() as usize).clamp(1, v.len()) - 1];
        Some(Self {
            n: v.len(),
            p50: at(0.50),
            p95: at(0.95),
            p99: at(0.99),
            max: v[v.len() - 1],
        })
    }
}

impl std::fmt::Display for Summary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "n={} p50={:.2} p95={:.2} p99={:.2} max={:.2} ms",
            self.n, self.p50, self.p95, self.p99, self.max
        )
    }
}

pub fn over(values: &[f64], limit_ms: f64) -> usize {
    values.iter().filter(|&&v| v > limit_ms).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_rank() {
        let v: Vec<f64> = (1..=100).map(f64::from).collect();
        let s = Summary::of(&v).unwrap();
        assert_eq!((s.p50, s.p95, s.p99, s.max), (50.0, 95.0, 99.0, 100.0));
        assert_eq!(Summary::of(&[3.0]).unwrap().p99, 3.0);
        assert!(Summary::of(&[]).is_none());
        assert_eq!(over(&v, 98.0), 2);
    }
}
