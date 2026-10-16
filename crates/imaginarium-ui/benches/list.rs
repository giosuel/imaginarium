// SPDX-License-Identifier: AGPL-3.0-or-later

//! Scrolling benchmarks: a virtual list of 100,000 text rows, as long as
//! a big palette or file list gets.
//!
//! Budget (PLAN §10, "input to pixels < 1 frame"): a wheel step, with the
//! rows it brings into view built and laid out and the tree painted, stays
//! well under 8 ms; the list's length must not matter.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use imaginarium_render::Scene;
use imaginarium_render::kurbo::{Point, Rect, Vec2};
use imaginarium_text::{Fonts, TextStyle};
use imaginarium_ui::{Flex, Input, Label, Modifiers, Scroll, Ui, VirtualList};

const AREA: Rect = Rect::new(0.0, 0.0, 800.0, 600.0);

fn list(c: &mut Criterion) {
    let mut group = c.benchmark_group("ui/list");
    let mut fonts = Fonts::bundled_only();
    let mut ui = Ui::new();
    let style = TextStyle::new(13.0);
    let scroll = ui.add_virtual_list(
        VirtualList::new(
            28.0,
            || 100_000,
            move |ui, row| {
                ui.add(Label::new(
                    format!("Row {row} · a command or a file"),
                    style,
                ))
            },
        ),
        Scroll::new().grow(1.0),
    );
    let root = ui.add_with_children(Flex::column().fill(), &[scroll]);
    ui.set_root(root);
    let mut scene = Scene::new();
    ui.layout(&mut fonts, AREA);
    ui.paint(&mut fonts, &mut scene);

    // One wheel step of three rows: some rows go, some come.
    let mut down = true;
    group.bench_function("wheel_step_100k", |b| {
        b.iter(|| {
            // Back and forth, so the list never reaches an end.
            let dy = if down { -84.0 } else { 84.0 };
            down = !down;
            ui.input(
                &mut fonts,
                &Input::Scroll {
                    delta: Vec2::new(0.0, dy),
                    position: Point::new(400.0, 300.0),
                    modifiers: Modifiers::default(),
                },
            );
            scene.clear();
            ui.layout(&mut fonts, AREA);
            ui.paint(&mut fonts, &mut scene);
            black_box(scene.len());
        });
    });

    // A jump far away: every row in view is new.
    let mut row = 0;
    group.bench_function("jump_100k", |b| {
        b.iter(|| {
            row = (row + 37_003) % 100_000;
            ui.scroll_to_row(scroll, row);
            scene.clear();
            ui.layout(&mut fonts, AREA);
            ui.paint(&mut fonts, &mut scene);
            black_box(scene.len());
        });
    });
    group.finish();
}

criterion_group!(benches, list);
criterion_main!(benches);
