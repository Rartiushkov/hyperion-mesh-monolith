//! RAMAN SORP-lite anomaly detector for the Hyperion hot path.
//!
//! A tiny sliding-window detector. It keeps the last `WINDOW` latencies, computes
//! a robust median and median-absolute-deviation (MAD), and returns a z-like
//! score. Inspired by RAMAN Theorem XV (Silent Outlier Resonance Protocol) but
//! simplified for sub-microsecond update cost inside the Rust network layer.

const WINDOW: usize = 128;
const ANOMALY_Z_THRESHOLD: f64 = 3.0;

/// Silent outlier detector for a single scalar feature (e.g. server latency).
#[derive(Clone, Debug)]
pub struct RamanAnomalyDetector {
    ring: [u64; WINDOW],
    len: usize,
    pos: usize,
}

impl Default for RamanAnomalyDetector {
    fn default() -> Self {
        Self {
            ring: [0u64; WINDOW],
            len: 0,
            pos: 0,
        }
    }
}

impl RamanAnomalyDetector {
    pub fn new() -> Self {
        Self::default()
    }

    /// Push a new sample and return the RAMAN anomaly z-score plus flag.
    pub fn update(&mut self, sample: u64) -> (f64, bool) {
        self.ring[self.pos] = sample;
        self.pos = (self.pos + 1) % WINDOW;
        if self.len < WINDOW {
            self.len += 1;
        }

        if self.len < 8 {
            // Not enough history for a robust MAD; declare normal.
            return (0.0, false);
        }

        let mut sorted: [u64; WINDOW] = [0u64; WINDOW];
        sorted[..self.len].copy_from_slice(&self.ring[..self.len]);
        sorted[..self.len].sort_unstable();

        let median = if self.len % 2 == 1 {
            sorted[self.len / 2] as f64
        } else {
            ((sorted[self.len / 2 - 1] + sorted[self.len / 2]) as f64) / 2.0
        };

        // MAD = median(|x - median|) * 1.4826, a robust estimate of stddev.
        let mut abs_devs: [f64; WINDOW] = [0.0; WINDOW];
        for i in 0..self.len {
            abs_devs[i] = ((sorted[i] as f64) - median).abs();
        }
        abs_devs[..self.len]
            .sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mad = if self.len % 2 == 1 {
            abs_devs[self.len / 2]
        } else {
            (abs_devs[self.len / 2 - 1] + abs_devs[self.len / 2]) / 2.0
        };
        let mad_std = mad * 1.4826;

        let z = if mad_std < 1.0 {
            // Degenerate case: more than half the window is identical.
            // Any sample different from the median is treated as an extreme outlier.
            if (sample as f64) == median {
                0.0
            } else {
                f64::INFINITY
            }
        } else {
            ((sample as f64) - median) / mad_std
        };
        let is_anomaly = z.is_infinite() || z.abs() > ANOMALY_Z_THRESHOLD;
        (z, is_anomaly)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_obvious_outlier() {
        let mut d = RamanAnomalyDetector::new();
        for _ in 0..64 {
            let _ = d.update(1000);
        }
        let (z, flag) = d.update(50_000);
        assert!(flag, "expected anomaly: z={z}");
        assert!(z.is_infinite() || z.abs() > ANOMALY_Z_THRESHOLD);
    }

    #[test]
    fn normal_samples_not_flagged() {
        let mut d = RamanAnomalyDetector::new();
        for i in 0..128u64 {
            let (z, flag) = d.update(1000 + i % 10);
            if i >= 8 {
                assert!(!flag, "expected no anomaly at sample {i}, z={z}");
            }
        }
    }
}
