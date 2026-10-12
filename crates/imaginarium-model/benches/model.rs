// SPDX-License-Identifier: AGPL-3.0-or-later

//! Model benchmarks against the PLAN §10 budgets, on the canonical
//! fixtures (`BoardFixture::LARGE`, `DocumentFixture::LARGE`):
//! - open a 300-page document (CRDT import plus view build); budget for
//!   the first page: < 500 ms;
//! - apply a remote update including the view update; budget < 1 ms with
//!   decrypt and verify (~74 µs in M1.4);
//! - a local keystroke or drag step including the view update; it must fit
//!   well inside a frame (8 ms).
//!
//! Memory is printed once (allocator counts), before and after
//! `Document::compact_memory`.

// Benchmark setup panics on failure by design.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::hint::black_box;
use std::time::{Duration, Instant};

use criterion::{Criterion, criterion_group, criterion_main};
use imaginarium_model::{
    Actor, Document, DocumentKind, Geometry, NodeId, NodeKind, OpenOptions, Parent, Value,
};
use imaginarium_test_utils::fixtures::{BoardFixture, DocumentFixture, ParagraphRole, ShapeKind};
use peak_alloc::PeakAlloc;

#[global_allocator]
static ALLOCATOR: PeakAlloc = PeakAlloc;

fn mib(bytes: usize) -> String {
    format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
}

/// The 10,000-shape board, built in one transaction.
fn board() -> (Document, Vec<NodeId>) {
    let mut doc = Document::new(DocumentKind::Board).unwrap();
    let ids = doc
        .transact(&Actor::User, |t| {
            let mut ids = Vec::new();
            for shape in BoardFixture::LARGE.shapes() {
                let kind = if shape.kind == ShapeKind::Text {
                    NodeKind::Text
                } else {
                    NodeKind::Shape
                };
                let id = t.create_node(Parent::Root, None, &kind)?;
                t.set_geometry(
                    id,
                    Geometry {
                        x: shape.x,
                        y: shape.y,
                        w: shape.width,
                        h: shape.height,
                        r: shape.rotation_deg,
                    },
                )?;
                t.set_style_override(id, "fill", Value::from(format!("#{:06x}", shape.fill_rgb)))?;
                match shape.kind {
                    ShapeKind::Text => t.insert_text(id, 0, &shape.label)?,
                    ShapeKind::Rectangle => t.set_prop(id, "shape", "rect".into())?,
                    ShapeKind::Ellipse => t.set_prop(id, "shape", "ellipse".into())?,
                    ShapeKind::Line => t.set_prop(id, "shape", "line".into())?,
                }
                ids.push(id);
            }
            Ok(ids)
        })
        .unwrap()
        .0;
    (doc, ids)
}

/// The 300-page document: one text flow with heading and list marks.
fn document() -> (Document, NodeId) {
    let paragraphs: Vec<_> = DocumentFixture::LARGE.paragraphs().collect();
    let mut text = String::new();
    let mut marks = Vec::new();
    for paragraph in &paragraphs {
        text.push_str(&paragraph.text);
        let newline = text.chars().count();
        text.push('\n');
        match paragraph.role {
            ParagraphRole::Heading1 => marks.push((newline, "h", Value::Int(1))),
            ParagraphRole::Heading2 => marks.push((newline, "h", Value::Int(2))),
            ParagraphRole::ListItem => marks.push((newline, "li", Value::from("bullet"))),
            ParagraphRole::Body => {}
        }
    }
    let mut doc = Document::new(DocumentKind::Document).unwrap();
    let id = doc
        .transact(&Actor::User, |t| {
            let id = t.create_node(Parent::Root, None, &NodeKind::Text)?;
            t.insert_text(id, 0, &text)?;
            for (pos, key, value) in marks {
                t.set_paragraph_mark(id, pos, key, value)?;
            }
            Ok(id)
        })
        .unwrap()
        .0;
    (doc, id)
}

fn memory_report(name: &str, build: impl FnOnce() -> Document) {
    let base = ALLOCATOR.current_usage();
    let doc = build();
    let built = ALLOCATOR.current_usage().saturating_sub(base);
    doc.compact_memory();
    let compacted = ALLOCATOR.current_usage().saturating_sub(base);
    let snapshot = doc.export_snapshot().unwrap();
    let shallow = doc.export_shallow().unwrap();
    drop(doc);
    let before_open = ALLOCATOR.current_usage();
    ALLOCATOR.reset_peak_usage();
    let opened = Document::open(&[&snapshot]).unwrap();
    let open = ALLOCATOR.current_usage().saturating_sub(before_open);
    let peak = ALLOCATOR.peak_usage().saturating_sub(before_open);
    eprintln!(
        "memory {name}: built {} (after compact_memory {}); opened {} (peak {}); snapshot {} (shallow {}); {} nodes",
        mib(built),
        mib(compacted),
        mib(open),
        mib(peak),
        mib(snapshot.len()),
        mib(shallow.len()),
        opened.view().node_count(),
    );
}

fn bench_open(c: &mut Criterion) {
    let (board, _) = board();
    let board_snapshot = board.export_snapshot().unwrap();
    drop(board);
    let (document, _) = document();
    let document_snapshot = document.export_snapshot().unwrap();
    drop(document);
    memory_report("board 10k shapes", || self::board().0);
    memory_report("document 300 pages", || self::document().0);

    let mut group = c.benchmark_group("model/open");
    group.sample_size(10);
    group.bench_function("board_10k_shapes", |b| {
        b.iter(|| black_box(Document::open(&[&board_snapshot]).unwrap()));
    });
    group.bench_function("document_300_pages", |b| {
        b.iter(|| black_box(Document::open(&[&document_snapshot]).unwrap()));
    });
    group.finish();
}

fn bench_local_edits(c: &mut Criterion) {
    let (mut doc, id) = document();
    let middle = doc.view().node(id).unwrap().text().unwrap().len() / 2;
    let mut group = c.benchmark_group("model/local");
    group.bench_function("keystroke_in_300_pages", |b| {
        b.iter(|| {
            black_box(
                doc.transact(&Actor::User, |t| t.insert_text(id, middle, "x"))
                    .unwrap(),
            )
        });
    });
    let (mut board, ids) = board();
    let mut step = 0usize;
    group.bench_function("drag_step_on_10k_board", |b| {
        b.iter(|| {
            step += 1;
            let node = ids[step % ids.len()];
            let x = step as f64;
            black_box(
                board
                    .transact(&Actor::User, |t| {
                        t.set_geometry(node, Geometry::new(x, x, 50.0, 50.0))
                    })
                    .unwrap(),
            )
        });
    });
    group.finish();
}

/// A value no earlier edit used, so that every edit changes something.
fn next_value() -> f64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed) as f64
}

/// Applies `iters` distinct updates made by `peer`, timing only the apply.
fn remote(
    target: &mut Document,
    peer: &mut Document,
    iters: u64,
    mut edit: impl FnMut(&mut Document, u64),
) -> Duration {
    let mut updates = Vec::with_capacity(iters as usize);
    for i in 0..iters {
        let version = peer.version();
        edit(peer, i);
        updates.push(peer.export_updates_since(&version).unwrap());
    }
    let start = Instant::now();
    for update in &updates {
        black_box(target.apply_remote(&[update]).unwrap());
    }
    start.elapsed()
}

fn bench_remote(c: &mut Criterion) {
    let options = OpenOptions::default();
    let (doc, id) = document();
    let snapshot = doc.export_snapshot().unwrap();
    let mut target = Document::open_with(&[&snapshot], &options).unwrap();
    let mut peer = Document::open_with(&[&snapshot], &options).unwrap();
    let middle = doc.view().node(id).unwrap().text().unwrap().len() / 2;
    let mut group = c.benchmark_group("model/remote");
    group.bench_function("keystroke_in_300_pages", |b| {
        b.iter_custom(|iters| {
            remote(&mut target, &mut peer, iters, |peer, i| {
                let pos = middle + (i as usize % 100);
                peer.transact(&Actor::User, |t| t.insert_text(id, pos, "y"))
                    .unwrap();
            })
        });
    });
    let (board, ids) = board();
    let snapshot = board.export_snapshot().unwrap();
    let mut target = Document::open_with(&[&snapshot], &options).unwrap();
    let mut peer = Document::open_with(&[&snapshot], &options).unwrap();
    group.bench_function("drag_step_on_10k_board", |b| {
        b.iter_custom(|iters| {
            remote(&mut target, &mut peer, iters, |peer, i| {
                let node = ids[i as usize % ids.len()];
                let x = next_value();
                peer.transact(&Actor::User, |t| {
                    t.set_geometry(node, Geometry::new(x, x, 50.0, 50.0))
                })
                .unwrap();
            })
        });
    });
    group.finish();
}

criterion_group!(benches, bench_open, bench_local_edits, bench_remote);
criterion_main!(benches);
