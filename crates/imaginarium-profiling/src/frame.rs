// SPDX-License-Identifier: AGPL-3.0-or-later

//! Frame-time statistics: a fixed-size ring buffer plus percentiles.
//!
//! The UI records one sample per presented frame. The frame-time HUD
//! (`F12`), benchmarks and the M2 prototype gate ("p99 < 16 ms") read
//! [`FrameStats::summary`].

use std::time::Duration;

/// Default window: 10 s of frames at 60 Hz.
pub const DEFAULT_CAPACITY: usize = 600;

/// Rolling window of the most recent frame times.
///
/// Recording is O(1) and never allocates after construction, so it is safe
/// to call on the UI thread every frame. Summaries sort a copy of the window
/// (O(n log n), ~10 µs for 600 samples). Request one at HUD refresh rate,
/// not per frame.
///
/// ```
/// use std::time::Duration;
/// use imaginarium_profiling::frame::FrameStats;
///
/// let mut stats = FrameStats::with_capacity(4);
/// for ms in [8, 9, 10, 30] {
///     stats.record(Duration::from_millis(ms));
/// }
/// let summary = stats.summary().unwrap();
/// assert_eq!(summary.p50, Duration::from_millis(9));
/// assert_eq!(summary.max, Duration::from_millis(30));
/// assert_eq!(stats.count_over(Duration::from_millis(16)), 1);
/// ```
#[derive(Clone, Debug)]
pub struct FrameStats {
    /// Ring buffer storage. Its length is the number of valid samples.
    samples: Vec<Duration>,
    /// Maximum number of samples kept.
    capacity: usize,
    /// Index of the oldest sample, which is overwritten next once full.
    next: usize,
    /// Total samples ever recorded (including overwritten ones).
    total: u64,
}

/// Statistics over the current window of a [`FrameStats`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameSummary {
    /// Number of samples in the window.
    pub count: usize,
    /// Shortest frame.
    pub min: Duration,
    /// Arithmetic mean.
    pub mean: Duration,
    /// Median (nearest-rank).
    pub p50: Duration,
    /// 95th percentile (nearest-rank).
    pub p95: Duration,
    /// 99th percentile (nearest-rank).
    pub p99: Duration,
    /// Longest frame.
    pub max: Duration,
}

impl Default for FrameStats {
    fn default() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }
}

impl FrameStats {
    /// Creates an empty window holding up to `capacity` samples (at least 1).
    pub fn with_capacity(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            samples: Vec::with_capacity(capacity),
            capacity,
            next: 0,
            total: 0,
        }
    }

    /// Records one frame time, replacing the oldest sample once full.
    pub fn record(&mut self, frame_time: Duration) {
        if self.samples.len() < self.capacity {
            self.samples.push(frame_time);
        } else {
            self.samples[self.next] = frame_time;
            self.next = (self.next + 1) % self.capacity;
        }
        self.total += 1;
    }

    /// Number of samples currently in the window.
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// Whether no samples have been recorded (since the last [`clear`](Self::clear)).
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Maximum number of samples kept.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Total samples ever recorded, including ones that left the window.
    pub fn total_recorded(&self) -> u64 {
        self.total
    }

    /// Removes all samples. Capacity and the total counter are kept.
    pub fn clear(&mut self) {
        self.samples.clear();
        self.next = 0;
    }

    /// Samples in the window that took longer than `budget` ("janky"
    /// frames), e.g. `count_over(Duration::from_micros(16_667))` at 60 Hz.
    pub fn count_over(&self, budget: Duration) -> usize {
        self.samples.iter().filter(|&&t| t > budget).count()
    }

    /// Computes statistics over the window, or `None` if it is empty.
    pub fn summary(&self) -> Option<FrameSummary> {
        let mut sorted = self.samples.clone();
        sorted.sort_unstable();
        let count = sorted.len();
        let total: u128 = sorted.iter().map(Duration::as_nanos).sum();
        let mean_nanos = total / count.max(1) as u128;
        Some(FrameSummary {
            count,
            min: *sorted.first()?,
            mean: Duration::from_nanos(u64::try_from(mean_nanos).unwrap_or(u64::MAX)),
            p50: percentile(&sorted, 50.0)?,
            p95: percentile(&sorted, 95.0)?,
            p99: percentile(&sorted, 99.0)?,
            max: *sorted.last()?,
        })
    }
}

/// Nearest-rank percentile of an **ascending sorted** slice.
///
/// Returns the smallest sample such that at least `p` % of the samples are
/// less than or equal to it: rank `⌈p/100 · n⌉` (1-based), clamped to
/// `1..=n`. The result is always an actual sample (no interpolation), which
/// is what frame-time budgets mean ("99 % of frames took ≤ X"). Returns
/// `None` for an empty slice. `p` is clamped to `0.0..=100.0`, and NaN is
/// treated as 0.
pub fn percentile(sorted: &[Duration], p: f64) -> Option<Duration> {
    if sorted.is_empty() {
        return None;
    }
    let p = if p.is_nan() { 0.0 } else { p.clamp(0.0, 100.0) };
    let n = sorted.len();
    // Multiply before dividing: for integer `p` and `n`, `p * n` is exact,
    // so ranks at exact boundaries (e.g. p = 10, n = 10) don't drift.
    let rank = (p * n as f64 / 100.0).ceil() as usize;
    sorted.get(rank.clamp(1, n) - 1).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(values: impl IntoIterator<Item = u64>) -> Vec<Duration> {
        values.into_iter().map(Duration::from_millis).collect()
    }

    #[test]
    fn percentile_nearest_rank_on_1_to_10() {
        let sorted = ms(1..=10);
        let at = |p| percentile(&sorted, p).unwrap().as_millis();
        assert_eq!(at(0.0), 1);
        assert_eq!(at(10.0), 1);
        assert_eq!(at(11.0), 2);
        assert_eq!(at(50.0), 5);
        assert_eq!(at(95.0), 10);
        assert_eq!(at(99.0), 10);
        assert_eq!(at(100.0), 10);
    }

    #[test]
    fn percentile_on_1_to_100_matches_the_rank() {
        let sorted = ms(1..=100);
        for p in [1u32, 25, 50, 95, 99, 100] {
            assert_eq!(
                percentile(&sorted, f64::from(p)).unwrap(),
                Duration::from_millis(u64::from(p))
            );
        }
    }

    #[test]
    fn percentile_edge_cases() {
        assert_eq!(percentile(&[], 50.0), None);
        let one = ms([7]);
        assert_eq!(percentile(&one, 0.0), Some(Duration::from_millis(7)));
        assert_eq!(percentile(&one, 99.9), Some(Duration::from_millis(7)));
        let sorted = ms(1..=10);
        assert_eq!(percentile(&sorted, -5.0), Some(Duration::from_millis(1)));
        assert_eq!(percentile(&sorted, 250.0), Some(Duration::from_millis(10)));
        assert_eq!(
            percentile(&sorted, f64::NAN),
            Some(Duration::from_millis(1))
        );
    }

    #[test]
    fn summary_of_unsorted_samples() {
        let mut stats = FrameStats::with_capacity(16);
        for t in ms([5, 1, 4, 2, 3]) {
            stats.record(t);
        }
        let s = stats.summary().unwrap();
        assert_eq!(s.count, 5);
        assert_eq!(s.min, Duration::from_millis(1));
        assert_eq!(s.max, Duration::from_millis(5));
        assert_eq!(s.mean, Duration::from_millis(3));
        assert_eq!(s.p50, Duration::from_millis(3));
        assert_eq!(s.p95, Duration::from_millis(5));
    }

    #[test]
    fn empty_window_has_no_summary() {
        assert_eq!(FrameStats::default().summary(), None);
    }

    #[test]
    fn ring_buffer_keeps_only_the_newest_samples() {
        let mut stats = FrameStats::with_capacity(3);
        for t in ms(1..=5) {
            stats.record(t);
        }
        assert_eq!(stats.len(), 3);
        assert_eq!(stats.total_recorded(), 5);
        let s = stats.summary().unwrap();
        assert_eq!(
            (s.min, s.max),
            (Duration::from_millis(3), Duration::from_millis(5))
        );
    }

    #[test]
    fn count_over_budget_and_clear() {
        let mut stats = FrameStats::with_capacity(10);
        for t in ms([8, 16, 17, 33, 9]) {
            stats.record(t);
        }
        assert_eq!(stats.count_over(Duration::from_millis(16)), 2);
        stats.clear();
        assert!(stats.is_empty());
        assert_eq!(stats.total_recorded(), 5);
        stats.record(Duration::from_millis(1));
        assert_eq!(stats.len(), 1);
    }

    #[test]
    fn zero_capacity_is_raised_to_one() {
        let mut stats = FrameStats::with_capacity(0);
        stats.record(Duration::from_millis(1));
        stats.record(Duration::from_millis(2));
        assert_eq!(stats.capacity(), 1);
        assert_eq!(stats.summary().unwrap().max, Duration::from_millis(2));
    }
}
