// SPDX-License-Identifier: AGPL-3.0-or-later

//! Layout, editing and drawing text, with only the bundled font so the
//! results are the same on every machine.
//!
//! Regenerate the golden files after a deliberate change:
//! `IMAGINARIUM_UPDATE_GOLDEN=1 cargo test -p imaginarium-text --test text`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use imaginarium_render::kurbo::Point;
use imaginarium_render::peniko::color::palette::css;
use imaginarium_render::{CpuMode, CpuRenderer, GpuOptions, OffscreenRenderer, Pixmap, Scene};
use imaginarium_text::{Edit, Editor, Fonts, Motion, TextBlock, TextStyle};

#[test]
fn text_wraps_at_the_width() {
    let mut fonts = Fonts::bundled_only();
    let style = TextStyle::new(16.0);
    let text = "The quick brown fox jumps over the lazy dog";
    let one_line = TextBlock::new(&mut fonts, text, &style, None);
    assert_eq!(one_line.line_count(), 1);
    let wrapped = TextBlock::new(&mut fonts, text, &style, Some(120.0));
    assert!(wrapped.line_count() >= 3, "{} lines", wrapped.line_count());
    assert!(wrapped.size().width <= 120.0);
    assert!(wrapped.size().height > one_line.size().height * 2.5);
}

#[test]
fn heavier_text_is_wider() {
    // Inter is variable: the weight axis must reach the shaper.
    let mut fonts = Fonts::bundled_only();
    let regular = TextBlock::new(&mut fonts, "Weight", &TextStyle::new(20.0), None);
    let bold = TextBlock::new(
        &mut fonts,
        "Weight",
        &TextStyle::new(20.0).weight(800.0),
        None,
    );
    assert!(bold.size().width > regular.size().width);
}

fn editor(text: &str) -> (Fonts, Editor) {
    let mut fonts = Fonts::bundled_only();
    let mut editor = Editor::new(text, TextStyle::new(16.0), false);
    editor.edit(&mut fonts, Edit::Move(Motion::TextEnd));
    (fonts, editor)
}

#[test]
fn typing_deleting_and_moving_by_words() {
    let (mut fonts, mut editor) = editor("hello world");
    editor.insert(&mut fonts, "!");
    assert_eq!(editor.text(), "hello world!");
    editor.edit(&mut fonts, Edit::Backspace);
    editor.edit(&mut fonts, Edit::BackspaceWord);
    assert_eq!(editor.text(), "hello ");
    editor.edit(&mut fonts, Edit::Move(Motion::TextStart));
    editor.edit(&mut fonts, Edit::Extend(Motion::WordRight));
    assert_eq!(editor.selected_text(), Some("hello"));
    editor.insert(&mut fonts, "goodbye");
    assert_eq!(editor.text(), "goodbye ");
    editor.edit(&mut fonts, Edit::SelectAll);
    editor.edit(&mut fonts, Edit::Delete);
    assert_eq!(editor.text(), "");
}

#[test]
fn a_single_line_editor_turns_line_breaks_into_spaces() {
    let (mut fonts, mut editor) = editor("a");
    editor.insert(&mut fonts, "b\nc\r\nd\u{2029}e\u{85}f");
    editor.edit(&mut fonts, Edit::Newline);
    assert_eq!(editor.text(), "ab c d e f");
    assert_eq!(editor.access_lines(&mut fonts).len(), 1);
    let mut multi = Editor::new("a", TextStyle::new(16.0), true);
    multi.edit(&mut fonts, Edit::Move(Motion::TextEnd));
    multi.edit(&mut fonts, Edit::Newline);
    multi.insert(&mut fonts, "b\r\nc");
    assert_eq!(multi.text(), "a\nb\nc");
}

#[test]
fn an_input_method_composes_then_commits() {
    // The sequence winit delivers for typing 日本 with a Japanese IME:
    // growing preedits, an empty preedit, then the commit.
    let (mut fonts, mut editor) = editor("x");
    editor.compose(&mut fonts, "に", Some((3, 3)));
    editor.compose(&mut fonts, "にほん", Some((9, 9)));
    assert!(editor.is_composing());
    assert_eq!(editor.text(), "x", "the composition isn't text yet");
    editor.compose(&mut fonts, "日本", None);
    editor.compose(&mut fonts, "", None);
    assert!(!editor.is_composing());
    editor.insert(&mut fonts, "日本");
    assert_eq!(editor.text(), "x日本");
    // A cancelled composition leaves nothing behind.
    editor.compose(&mut fonts, "ご", Some((3, 3)));
    editor.cancel_compose(&mut fonts);
    assert_eq!(editor.text(), "x日本");
    // Out-of-range cursors from a misbehaving IME are ignored.
    editor.compose(&mut fonts, "a", Some((0, 9)));
    editor.cancel_compose(&mut fonts);
}

#[test]
fn clicks_place_the_caret_and_select_words() {
    let (mut fonts, mut editor) = editor("alpha beta gamma");
    let size = editor.size(&mut fonts);
    let middle = Point::new(size.width / 2.0, size.height / 2.0);
    editor.click(&mut fonts, middle, 2, false);
    assert_eq!(editor.selected_text(), Some("beta"));
    editor.click(&mut fonts, Point::new(0.0, middle.y), 1, false);
    assert_eq!(editor.selection(), (0, 0));
    editor.click(&mut fonts, Point::new(size.width + 10.0, middle.y), 1, true);
    assert_eq!(editor.selected_text(), Some("alpha beta gamma"));
}

#[test]
fn access_lines_cover_the_text() {
    let mut fonts = Fonts::bundled_only();
    let mut editor = Editor::new(
        "one two three four five six seven eight",
        TextStyle::new(16.0),
        true,
    );
    editor.set_width(Some(90.0));
    let lines = editor.access_lines(&mut fonts);
    assert!(lines.len() >= 3);
    let joined: String = lines.iter().map(|line| line.text.as_str()).collect();
    assert_eq!(joined, editor.text());
    for line in &lines {
        let total: usize = line.character_lengths.iter().map(|&n| usize::from(n)).sum();
        assert_eq!(total, line.text.len());
        assert!(line.bounds.height() > 0.0);
    }
}

fn sample(fonts: &mut Fonts) -> Scene {
    let mut scene = Scene::new();
    let title = TextBlock::new(
        fonts,
        "Imaginarium",
        &TextStyle::new(28.0).weight(700.0).color(css::MIDNIGHT_BLUE),
        None,
    );
    title.paint(&mut scene, Point::new(12.0, 8.0), true);
    let body = TextBlock::new(
        fonts,
        "Glyphs from Parley, drawn by Vello. Naïve café, Ωμέγα, Привет.",
        &TextStyle::new(14.0).color(css::DIM_GRAY),
        Some(296.0),
    );
    body.paint(&mut scene, Point::new(12.0, 50.0), true);
    let mut editor = Editor::new("Edit me", TextStyle::new(16.0), false);
    editor.edit(fonts, Edit::SelectAll);
    editor.paint(
        fonts,
        &mut scene,
        Point::new(12.0, 100.0),
        true,
        true,
        css::ROYAL_BLUE,
    );
    scene
}

fn golden(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name)
}

#[test]
fn text_matches_its_golden_image() {
    let mut fonts = Fonts::bundled_only();
    let pixmap =
        CpuRenderer::new(CpuMode::Deterministic).render(&sample(&mut fonts), 320, 132, css::WHITE);
    let path = golden("text.png");
    if std::env::var_os("IMAGINARIUM_UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, pixmap.into_png().unwrap()).unwrap();
        return;
    }
    let expected = Pixmap::from_png(std::io::Cursor::new(std::fs::read(&path).unwrap())).unwrap();
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
fn the_gpu_draws_text_like_the_cpu() {
    let mut gpu = match OffscreenRenderer::new(&GpuOptions::default()) {
        Ok(gpu) => gpu,
        Err(err) => {
            eprintln!("skipped: {err}");
            return;
        }
    };
    let mut fonts = Fonts::bundled_only();
    let scene = sample(&mut fonts);
    let on_gpu = gpu.render(&scene, 320, 132, css::WHITE).unwrap();
    let on_cpu = CpuRenderer::new(CpuMode::Deterministic).render(&scene, 320, 132, css::WHITE);
    let off = on_gpu
        .data()
        .iter()
        .zip(on_cpu.data())
        .filter(|(p, q)| {
            [p.r.abs_diff(q.r), p.g.abs_diff(q.g), p.b.abs_diff(q.b)]
                .into_iter()
                .max()
                .unwrap_or(0)
                > 48
        })
        .count();
    // Glyph edges are all antialiasing: allow more than for shapes.
    let share = off as f64 / on_cpu.data().len() as f64;
    assert!(share < 0.02, "{:.2} % of pixels differ", share * 100.0);
}

#[test]
fn setting_the_text_while_composing_shows_the_caret_again() {
    let (mut fonts, mut editor) = editor("ab");
    editor.compose(&mut fonts, "日本", None);
    assert!(
        editor.caret(&mut fonts).is_none(),
        "the input method hid it"
    );
    editor.set_text(&mut fonts, "x");
    assert!(!editor.is_composing());
    assert_eq!(editor.text(), "x");
    assert!(editor.caret(&mut fonts).is_some());
}

#[test]
fn access_offsets_are_in_the_shown_text() {
    let (mut fonts, mut editor) = editor("ab");
    editor.compose(&mut fonts, "にほん", Some((9, 9)));
    assert_eq!(editor.text(), "ab");
    assert_eq!(editor.shown_text(), "abにほん");
    let lines = editor.access_lines(&mut fonts);
    assert_eq!(lines[0].text, editor.shown_text());
    assert_eq!(editor.selection(), (11, 11));
}

#[test]
fn access_characters_are_what_the_caret_steps_over() {
    // "é" as e plus a combining accent, then a family emoji (ZWJ sequence).
    let text = "e\u{301}x\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
    let (mut fonts, mut editor) = editor(text);
    let lines = editor.access_lines(&mut fonts);
    assert_eq!(lines[0].character_lengths, vec![3, 1, 18]);
    // The caret agrees: one step left from the end skips the whole emoji.
    editor.edit(&mut fonts, Edit::Move(Motion::Left));
    assert_eq!(editor.selection(), (4, 4));
}
