// SPDX-License-Identifier: AGPL-3.0-or-later

//! Benchmarks for [`FrameStats`]. This is also the reference example of our
//! benchmark conventions (see the `imaginarium-profiling` crate docs).
//!
//! Budgets guarded here:
//! - `record` runs on the UI thread every frame and must stay far below 1 µs.
//! - `summary` over the default 600-sample window runs at HUD refresh rate
//!   and must stay below 100 µs.

use std::hint::black_box;
use std::time::Duration;

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use imaginarium_profiling::frame::{DEFAULT_CAPACITY, FrameStats};
use imaginarium_test_utils::rng::Rng;

/// Seed for the synthetic frame-time distribution. Never change it.
const SEED: u64 = 0x00F2_A3E5_0000_0001;

/// Realistic frame times: mostly ~8 ms (120 Hz), with occasional 20–40 ms
/// spikes.
fn frame_times(count: usize) -> Vec<Duration> {
    let mut rng = Rng::new(SEED);
    (0..count)
        .map(|_| {
            let ms = if rng.chance(0.02) {
                rng.range_f64(20.0, 40.0)
            } else {
                rng.range_f64(6.0, 10.0)
            };
            Duration::from_secs_f64(ms / 1_000.0)
        })
        .collect()
}

fn bench_frame_stats(c: &mut Criterion) {
    let samples = frame_times(DEFAULT_CAPACITY);
    let mut group = c.benchmark_group("profiling/frame_stats");

    group.bench_function("record_full_window", |b| {
        let mut stats = FrameStats::default();
        samples.iter().for_each(|&t| stats.record(t));
        let mut i = 0;
        b.iter(|| {
            stats.record(black_box(samples[i % samples.len()]));
            i += 1;
        });
    });

    group.bench_function("summary_600", |b| {
        b.iter_batched_ref(
            || {
                let mut stats = FrameStats::default();
                samples.iter().for_each(|&t| stats.record(t));
                stats
            },
            |stats| black_box(stats.summary()),
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

criterion_group!(benches, bench_frame_stats);
criterion_main!(benches);
