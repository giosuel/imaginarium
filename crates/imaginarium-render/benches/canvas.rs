// SPDX-License-Identifier: AGPL-3.0-or-later

//! Canvas benchmarks: the UI thread's cost of edits.
//!
//! PLAN §10: input to pixels within a frame (8 ms). Moving a selection of
//! every shape on a 10,000-shape board is one edit of 10,000 items, each
//! of which invalidates its old and new area in the tile cache. The board
//! is generated (a grid of boxes), so no fixture data is needed.

use std::hint::black_box;
use std::time::Instant;

use criterion::{Criterion, criterion_group, criterion_main};
use imaginarium_render::Scene;
use imaginarium_render::canvas::{Canvas, CanvasContent, ItemId, Viewport};
use imaginarium_render::kurbo::{Point, Rect, Vec2};
use imaginarium_render::peniko::Color;

const ITEMS: u64 = 10_000;

struct Boxes;

impl CanvasContent for Boxes {
    fn paint_item(&mut self, id: ItemId, scene: &mut Scene, _level_scale: f64) {
        scene.fill(&bounds(id.0), Color::BLACK);
    }
}

fn bounds(i: u64) -> Rect {
    let x = (i % 100) as f64 * 120.0;
    let y = (i / 100) as f64 * 90.0;
    Rect::new(x, y, x + 100.0, y + 70.0)
}

fn edits(c: &mut Criterion) {
    let mut group = c.benchmark_group("canvas/edit");
    group.sample_size(20);
    let mut canvas = Canvas::new().with_limits(10_000, 1_000);
    for i in 0..ITEMS {
        canvas.upsert(ItemId(i), i, bounds(i));
    }
    // Fill the cache: tiles at a few zoom levels across the board.
    let now = Instant::now();
    for (zoom, step) in [(0.1, 4_000.0), (0.35, 2_000.0), (1.0, 1_000.0)] {
        let mut y = 0.0;
        while y < 9_000.0 {
            let mut x = 0.0;
            while x < 12_000.0 {
                let view = Viewport {
                    origin: Point::new(x, y),
                    zoom,
                    screen: Rect::new(0.0, 0.0, 1280.0, 800.0),
                    scale_factor: 1.0,
                };
                canvas.frame(&mut Scene::new(), &view, now, &mut Boxes);
                x += step;
            }
            y += step;
        }
    }
    let mut offset = 0.0;
    group.bench_function("move_10k_items_1000_cached_tiles", |b| {
        b.iter(|| {
            offset = if offset == 0.0 { 1.0 } else { 0.0 };
            for i in 0..ITEMS {
                canvas.upsert(ItemId(i), i, bounds(i) + Vec2::new(offset, 0.0));
            }
        });
    });
    black_box(canvas.len());
    group.finish();
}

criterion_group!(benches, edits);
criterion_main!(benches);
