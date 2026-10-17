// SPDX-License-Identifier: AGPL-3.0-or-later

//! Board benchmarks (`docs/design/canvas.md` §10), on 10,000-node boards.
//!
//! Budgets (PLAN §10, ADR-0020): a drag step (the command, the change
//! set, the items and the canvas index) under 1 ms; hit testing and a
//! style change well inside a frame; opening as fast as the model allows.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::hint::black_box;
use std::time::Instant;

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use imaginarium_canvas::{BoardCanvas, demo};
use imaginarium_commands::{ArgValue, Args};
use imaginarium_model::{Actor, Document, Geometry, NodeId, NodeKind, Parent, Value};
use imaginarium_render::Scene;
use imaginarium_render::kurbo::{Point, Rect};
use imaginarium_text::Fonts;

const SCREEN: Rect = Rect::new(0.0, 0.0, 1440.0, 866.0);

fn snapshot(labels: bool) -> Vec<u8> {
    demo::grid_with(10_000, labels)
        .unwrap()
        .export_snapshot()
        .unwrap()
}

fn frame(board: &mut BoardCanvas, fonts: &mut Fonts) {
    let mut scene = Scene::new();
    board.paint_into(fonts, &mut scene, SCREEN, Instant::now());
    black_box(scene.len());
}

fn open(c: &mut Criterion) {
    let mut group = c.benchmark_group("canvas/open");
    group.sample_size(10);
    let mut fonts = Fonts::bundled_only();
    for (name, labels) in [("10k", false), ("10k_labels", true)] {
        let bytes = snapshot(labels);
        group.bench_function(format!("document_{name}"), |b| {
            b.iter(|| black_box(Document::open(&[&bytes]).unwrap()))
        });
        // The items and the first frame (drawn directly), from an open
        // document.
        group.bench_function(format!("first_frame_{name}"), |b| {
            b.iter_batched(
                || BoardCanvas::new(Document::open(&[&bytes]).unwrap()),
                |mut board| {
                    frame(&mut board, &mut fonts);
                    board
                },
                BatchSize::LargeInput,
            )
        });
    }
    group.finish();
}

fn ready(fonts: &mut Fonts) -> (BoardCanvas, NodeId) {
    let mut board = BoardCanvas::new(demo::grid(10_000).unwrap());
    frame(&mut board, fonts);
    let first = board.document().view().walk()[0];
    board.select(&[first]);
    (board, first)
}

fn edit(c: &mut Criterion) {
    let mut group = c.benchmark_group("canvas/edit");
    let mut fonts = Fonts::bundled_only();

    // One pointer move of a drag: board.move, then the board up to date.
    let (mut board, _) = ready(&mut fonts);
    let mut right = true;
    group.bench_function("drag_step_10k", |b| {
        b.iter(|| {
            let dx = if right { 1.0 } else { -1.0 };
            right = !right;
            let args = Args::new()
                .with("dx", ArgValue::Float(dx))
                .with("dy", ArgValue::Float(0.0));
            board.execute("board.move", &args).unwrap();
            board.update(&mut fonts);
        })
    });

    // A style used by 1,000 of the nodes changes its fill.
    let (mut board, _) = ready(&mut fonts);
    let style = board
        .transact(&Actor::User, |txn| {
            let style = txn.create_style("Note", None)?;
            let nodes: Vec<NodeId> = txn.children(Parent::Root);
            for node in nodes.iter().step_by(10) {
                txn.set_style(*node, Some(&style))?;
            }
            Ok(style)
        })
        .unwrap();
    board.update(&mut fonts);
    let fills = ["#E0E7FF", "#FEF3C7"];
    let mut turn = 0;
    group.bench_function("style_change_1k_of_10k", |b| {
        b.iter(|| {
            turn = 1 - turn;
            board
                .transact(&Actor::User, |txn| {
                    txn.set_style_prop(&style, "fill", Value::String(fills[turn].into()))
                })
                .unwrap();
            board.update(&mut fonts);
        })
    });

    // A click's hit test, over shapes and between them.
    let (board, _) = ready(&mut fonts);
    let mut at = 0.0;
    // A frame with all 10,000 nodes selected (a select-all on a big
    // board): only the outlines on screen are drawn, and the selection's
    // box is measured once.
    {
        let (mut all, _) = ready(&mut fonts);
        let nodes = all.document().view().walk();
        all.select(&nodes);
        frame(&mut all, &mut fonts);
        group.bench_function("frame_10k_selected", |b| {
            b.iter(|| frame(&mut all, &mut fonts))
        });
    }
    group.bench_function("hit_10k", |b| {
        b.iter(|| {
            at = (at + 7.0) % 800.0;
            black_box(board.node_at(Point::new(at, at * 0.5)))
        })
    });

    // A paste of 1,000 shapes: one transaction, then the board up to date.
    let bytes = snapshot(false);
    group.sample_size(20);
    group.bench_function("paste_1k_into_10k", |b| {
        b.iter_batched(
            || {
                let mut board = BoardCanvas::new(Document::open(&[&bytes]).unwrap());
                let mut fonts = Fonts::bundled_only();
                frame(&mut board, &mut fonts);
                (board, fonts)
            },
            |(mut board, mut fonts)| {
                board
                    .transact(&Actor::User, |txn| {
                        for index in 0..1_000 {
                            let id = txn.create_node(Parent::Root, None, &NodeKind::Shape)?;
                            let x = f64::from(index % 40) * 20.0;
                            let y = 7_000.0 + f64::from(index / 40) * 20.0;
                            txn.set_geometry(id, Geometry::new(x, y, 16.0, 16.0))?;
                        }
                        Ok(())
                    })
                    .unwrap();
                board.update(&mut fonts);
                board
            },
            BatchSize::LargeInput,
        )
    });
    group.finish();
}

criterion_group!(benches, open, edit);
criterion_main!(benches);
