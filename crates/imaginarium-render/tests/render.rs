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
use imaginarium_render::{
    CpuMode, CpuRenderer, GpuOptions, OffscreenRenderer, Pixmap, Scene, TextureKey,
};

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
    // PNG stores straight alpha: the few pixels that aren't quite opaque
    // come back off by one after the round trip through premultiplying.
    let differing = pixmap
        .data()
        .iter()
        .zip(expected.data())
        .filter(|(a, b)| {
            a.r.abs_diff(b.r) > 1
                || a.g.abs_diff(b.g) > 1
                || a.b.abs_diff(b.b) > 1
                || a.a.abs_diff(b.a) > 1
        })
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

/// What tiles do: content rendered into a texture once, then drawn.
fn through_texture(content: &Scene, offset: (f64, f64)) -> (Scene, TextureKey) {
    let key = TextureKey::unique();
    let mut scene = Scene::new();
    scene.render_texture(key, WIDTH, HEIGHT, content.clone());
    scene.draw_texture(
        key,
        WIDTH,
        HEIGHT,
        Rect::new(
            offset.0,
            offset.1,
            offset.0 + f64::from(WIDTH),
            offset.1 + f64::from(HEIGHT),
        ),
    );
    (scene, key)
}

#[test]
fn a_texture_drawn_one_to_one_matches_drawing_directly() {
    let direct =
        CpuRenderer::new(CpuMode::Deterministic).render(&sample(), WIDTH, HEIGHT, background());
    let mut renderer = CpuRenderer::new(CpuMode::Deterministic);
    let (scene, key) = through_texture(&sample(), (0.0, 0.0));
    let textured = renderer.render(&scene, WIDTH, HEIGHT, background());
    assert_eq!(renderer.texture_count(), 1, "the texture is kept");
    let (max, share) = compare(&direct, &textured, 2);
    assert!(share == 0.0, "{share} of pixels differ (max {max})");
    // Drawing it again needs no new render.
    let mut again = Scene::new();
    again.draw_texture(
        key,
        WIDTH,
        HEIGHT,
        Rect::new(0.0, 0.0, f64::from(WIDTH), f64::from(HEIGHT)),
    );
    assert_eq!(again.texture_renders(), 0);
    let reused = renderer.render(&again, WIDTH, HEIGHT, background());
    assert_eq!(compare(&textured, &reused, 0).1, 0.0);
}

#[test]
fn a_texture_follows_the_transform_and_its_destination() {
    // A 2×2 texture: one red texel at (1, 1), drawn into (10, 10)–(14, 14)
    // under a 2× transform: the red texel covers device (24, 24)–(28, 28).
    let mut content = Scene::new();
    content.fill(&Rect::new(1.0, 1.0, 2.0, 2.0), css::RED);
    let key = TextureKey::unique();
    let mut scene = Scene::new();
    scene.render_texture(key, 2, 2, content);
    scene.set_transform(Affine::scale(2.0));
    scene.draw_texture(key, 2, 2, Rect::new(10.0, 10.0, 14.0, 14.0));
    let pixmap = CpuRenderer::new(CpuMode::Deterministic).render(&scene, 40, 40, background());
    let at = |x: usize, y: usize| pixmap.data()[y * 40 + x];
    assert!(
        at(26, 26).r > 200 && at(26, 26).g < 60,
        "red inside: {:?}",
        at(26, 26)
    );
    assert!(at(21, 21).g > 200, "white outside: {:?}", at(21, 21));
    assert!(at(30, 30).g > 200, "white outside: {:?}", at(30, 30));
}

#[test]
fn the_gpu_draws_textures_like_the_cpu() {
    let mut gpu = match OffscreenRenderer::new(&GpuOptions::default()) {
        Ok(gpu) => gpu,
        Err(err) => {
            eprintln!("skipped: {err}");
            return;
        }
    };
    let (scene, _) = through_texture(&sample(), (0.0, 0.0));
    let on_gpu = gpu.render(&scene, WIDTH, HEIGHT, background()).unwrap();
    let on_cpu =
        CpuRenderer::new(CpuMode::Deterministic).render(&scene, WIDTH, HEIGHT, background());
    let (max, share) = compare(&on_gpu, &on_cpu, 24);
    assert!(share < 0.01, "{:.2} % differ (max {max})", share * 100.0);
}

/// A texture that doesn't exist (never rendered, or freed) shows nothing on
/// both backends, instead of failing the frame.
#[test]
fn drawing_a_missing_texture_draws_nothing() {
    let mut scene = Scene::new();
    scene.draw_texture(
        TextureKey::unique(),
        WIDTH,
        HEIGHT,
        Rect::new(0.0, 0.0, f64::from(WIDTH), f64::from(HEIGHT)),
    );
    let empty =
        CpuRenderer::new(CpuMode::Deterministic).render(&Scene::new(), WIDTH, HEIGHT, background());
    let on_cpu =
        CpuRenderer::new(CpuMode::Deterministic).render(&scene, WIDTH, HEIGHT, background());
    assert_eq!(compare(&empty, &on_cpu, 0).1, 0.0);
    let mut gpu = match OffscreenRenderer::new(&GpuOptions::default()) {
        Ok(gpu) => gpu,
        Err(err) => {
            eprintln!("skipped: {err}");
            return;
        }
    };
    let on_gpu = gpu
        .render(&scene, WIDTH, HEIGHT, background())
        .expect("a missing texture doesn't fail the frame");
    assert_eq!(compare(&empty, &on_gpu, 2).1, 0.0);
}

/// Color emoji are bitmap glyphs, which vello_gpu draws only from its glyph
/// atlas and up to 128 pixels per em; any other bitmap glyph made it panic
/// (the gate's first frame, drawn without the atlas, with an emoji font
/// installed). They draw on both backends, without the atlas option and
/// bigger than the atlas takes.
#[test]
fn bitmap_glyphs_draw_at_any_size() {
    use imaginarium_render::peniko::{Blob, FontData};
    use imaginarium_render::{Glyph, GlyphRun};
    use skrifa::MetadataProvider;

    const FONT: &[u8] = include_bytes!("fonts/EmojiRedSquare.ttf");
    let font = FontData::new(Blob::new(std::sync::Arc::new(FONT)), 0);
    let face = skrifa::FontRef::new(FONT).unwrap();
    let id = face.charmap().map('\u{1F7E5}').unwrap().to_u32();
    let mut gpu = OffscreenRenderer::new(&GpuOptions::default())
        .map_err(|err| eprintln!("GPU skipped: {err}"))
        .ok();
    // 20 pixels per em, and 13 × 12 = 156 (over the atlas limit).
    for (size, scale) in [(20.0_f32, 1.0), (13.0, 12.0)] {
        let mut scene = Scene::new();
        scene.set_transform(Affine::scale(scale));
        scene.glyphs(
            GlyphRun {
                font: &font,
                size,
                coords: &[],
                hint: false,
                color: Color::BLACK,
            },
            [Glyph {
                id,
                x: 1.0,
                y: size,
            }],
        );
        // The square's middle, in device pixels.
        let middle = ((1.0 + size / 2.0) * scale as f32) as usize;
        let mut pixmaps = vec![(
            "cpu",
            CpuRenderer::new(CpuMode::Deterministic).render(&scene, 200, 200, Color::WHITE),
        )];
        if let Some(gpu) = &mut gpu {
            pixmaps.push(("gpu", gpu.render(&scene, 200, 200, Color::WHITE).unwrap()));
        }
        for (backend, pixmap) in pixmaps {
            let at = pixmap.data()[middle * 200 + middle];
            assert!(
                at.r > 180 && at.g < 120 && at.b < 120,
                "{backend} at {size} × {scale}: {at:?}"
            );
        }
    }
}
