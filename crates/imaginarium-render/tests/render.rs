// SPDX-License-Identifier: AGPL-3.0-or-later

//! Golden images on the CPU, and the GPU compared with the CPU.
//!
//! Regenerate the golden files after a deliberate change:
//! `IMAGINARIUM_UPDATE_GOLDEN=1 cargo test -p imaginarium-render --test render`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use imaginarium_render::kurbo::{Affine, Circle, Rect, RoundedRect};
use imaginarium_render::peniko::Color;
use imaginarium_render::peniko::color::palette::css;
use imaginarium_render::{CpuMode, CpuRenderer, GpuOptions, OffscreenRenderer, Pixmap, Scene};

const WIDTH: u16 = 160;
const HEIGHT: u16 = 120;

/// A scene with every command kind.
fn sample() -> Scene {
    let mut scene = Scene::new();
    scene.fill(&Rect::new(10.0, 10.0, 70.0, 50.0), css::TEAL);
    scene.fill(&RoundedRect::new(80.0, 10.0, 150.0, 60.0, 12.0), css::CORAL);
    scene.stroke(&Circle::new((40.0, 85.0), 22.0), 4.0, css::MIDNIGHT_BLUE);
    scene.push_clip(&Rect::new(90.0, 70.0, 140.0, 110.0));
    scene.fill(&Circle::new((115.0, 90.0), 30.0), css::GOLD);
    scene.pop();
    scene.push_opacity(0.5);
    scene.set_transform(Affine::rotate_about(
        0.3,
        imaginarium_render::kurbo::Point::new(60.0, 60.0),
    ));
    scene.fill(&Rect::new(45.0, 45.0, 75.0, 75.0), css::REBECCA_PURPLE);
    scene.pop();
    scene
}

fn background() -> Color {
    css::WHITE
}

fn golden(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name)
}

fn check_golden(name: &str, pixmap: Pixmap) {
    let path = golden(name);
    if std::env::var_os("IMAGINARIUM_UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, pixmap.into_png().unwrap()).unwrap();
        return;
    }
    let expected = Pixmap::from_png(std::io::Cursor::new(std::fs::read(&path).unwrap())).unwrap();
    assert_eq!(
        (pixmap.width(), pixmap.height()),
        (expected.width(), expected.height())
    );
    let differing = pixmap
        .data()
        .iter()
        .zip(expected.data())
        .filter(|(a, b)| a != b)
        .count();
    assert_eq!(
        differing,
        0,
        "{differing} pixels differ from {}",
        path.display()
    );
}

#[test]
fn the_cpu_backend_matches_its_golden_image() {
    let pixmap =
        CpuRenderer::new(CpuMode::Deterministic).render(&sample(), WIDTH, HEIGHT, background());
    check_golden("sample.png", pixmap);
}

#[test]
fn deterministic_cpu_rendering_is_repeatable() {
    let mut renderer = CpuRenderer::new(CpuMode::Deterministic);
    let first = renderer.render(&sample(), WIDTH, HEIGHT, background());
    let second = renderer.render(&sample(), WIDTH, HEIGHT, background());
    assert_eq!(first.data(), second.data());
}

/// The largest difference of any channel, and the share of pixels that
/// differ by more than `tolerance`.
fn compare(a: &Pixmap, b: &Pixmap, tolerance: u8) -> (u8, f64) {
    let mut max = 0;
    let mut off = 0usize;
    for (p, q) in a.data().iter().zip(b.data()) {
        let d = [
            p.r.abs_diff(q.r),
            p.g.abs_diff(q.g),
            p.b.abs_diff(q.b),
            p.a.abs_diff(q.a),
        ]
        .into_iter()
        .max()
        .unwrap();
        max = max.max(d);
        if d > tolerance {
            off += 1;
        }
    }
    (max, off as f64 / a.data().len() as f64)
}

#[test]
fn the_gpu_backend_draws_what_the_cpu_backend_draws() {
    // CI runners have no GPU; on machines without one this is skipped.
    let mut gpu = match OffscreenRenderer::new(&GpuOptions::default()) {
        Ok(gpu) => gpu,
        Err(err) => {
            eprintln!("skipped: {err}");
            return;
        }
    };
    let on_gpu = gpu.render(&sample(), WIDTH, HEIGHT, background()).unwrap();
    let on_cpu =
        CpuRenderer::new(CpuMode::Deterministic).render(&sample(), WIDTH, HEIGHT, background());
    // The two rasterize independently: antialiased edges may differ a
    // little, the shapes and colors must not.
    let (max, share) = compare(&on_gpu, &on_cpu, 24);
    assert!(
        share < 0.01,
        "{:.2} % of pixels differ by more than 24 (max {max}) on {:?}",
        share * 100.0,
        gpu.info()
    );
}

#[test]
fn scenes_balance_their_layers() {
    let mut scene = Scene::new();
    scene.push_opacity(0.5);
    scene.push_clip(&Rect::new(0.0, 0.0, 10.0, 10.0));
    scene.pop();
    scene.pop();
    scene.pop(); // extra pops are ignored
    assert_eq!(scene.len(), 4);
    // Unbalanced scenes still render (open layers close at the end).
    let mut open = Scene::new();
    open.push_opacity(0.5);
    open.fill(&Rect::new(0.0, 0.0, 8.0, 8.0), css::BLACK);
    let mut outer = Scene::new();
    outer.append(&open, Affine::translate((4.0, 4.0)));
    let pixmap = CpuRenderer::new(CpuMode::Deterministic).render(&outer, 16, 16, background());
    let inside = pixmap.data()[8 * 16 + 8];
    assert!(
        inside.r > 100 && inside.r < 160,
        "half-transparent black: {inside:?}"
    );
    let outside = pixmap.data()[0];
    assert_eq!(outside.r, 255, "the translation applied");
}
