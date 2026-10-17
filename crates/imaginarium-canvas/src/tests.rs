// SPDX-License-Identifier: AGPL-3.0-or-later

//! Tests of the canvas: outlines, items and what a board paints.

use std::path::PathBuf;
use std::time::Instant;

use imaginarium_model::{Actor, Document, DocumentKind, Geometry, NodeId, NodeKind, Parent, Value};
use imaginarium_render::kurbo::{Point, Rect, Shape, Size};
use imaginarium_render::{CpuMode, CpuRenderer, Pixmap, Scene};
use imaginarium_text::Fonts;
use imaginarium_ui::{DEFAULT_ACCENT, Theme, set_theme, theme};

use crate::BoardCanvas;
use crate::items::{BoardColors, Items, Look, Z_GAP};
use crate::shapes::shape_path;

/// A board with one node of `kind`, set up by `edit`.
fn one_node(
    kind: NodeKind,
    edit: impl FnOnce(&mut imaginarium_model::Transaction<'_>, NodeId),
) -> (Document, NodeId) {
    let mut document = Document::new(DocumentKind::Board).unwrap();
    let (id, _) = document
        .transact(&Actor::User, |txn| {
            let id = txn.create_node(Parent::Root, None, &kind)?;
            txn.set_geometry(id, Geometry::new(10.0, 20.0, 100.0, 50.0))?;
            edit(txn, id);
            Ok(id)
        })
        .unwrap();
    (document, id)
}

fn preset(name: &str) -> (Document, NodeId) {
    one_node(NodeKind::Shape, |txn, id| {
        txn.set_prop(id, "shape", Value::String(name.into()))
            .unwrap();
    })
}

fn build(document: &Document) -> Items {
    let colors = BoardColors::of(&theme());
    Items::build(document.view(), &mut Fonts::bundled_only(), &colors)
}

#[test]
fn every_preset_has_its_outline_in_the_box() {
    let size = Size::new(100.0, 50.0);
    for (name, closed) in [
        ("rect", true),
        ("ellipse", true),
        ("triangle", true),
        ("diamond", true),
        ("line", false),
        ("path", true),
        ("no-such-preset", true),
    ] {
        let (document, id) = preset(name);
        let node = document.view().node(id).unwrap();
        let outline = shape_path(node, size, 0.0);
        assert_eq!(outline.closed, closed, "{name}");
        let bounds = outline.path.bounding_box();
        assert!(
            (bounds.width() - 100.0).abs() < 0.01 && (bounds.height() - 50.0).abs() < 0.01,
            "{name}: {bounds:?}"
        );
    }
}

#[test]
fn a_line_joins_the_corners_its_flip_names() {
    for (flip, start, end) in [
        (None, (0.0, 0.0), (100.0, 50.0)),
        (Some("h"), (100.0, 0.0), (0.0, 50.0)),
        (Some("v"), (0.0, 50.0), (100.0, 0.0)),
        (Some("hv"), (100.0, 50.0), (0.0, 0.0)),
    ] {
        let (document, id) = one_node(NodeKind::Shape, |txn, id| {
            txn.set_prop(id, "shape", Value::String("line".into()))
                .unwrap();
            if let Some(flip) = flip {
                txn.set_prop(id, "flip", Value::String(flip.into()))
                    .unwrap();
            }
        });
        let outline = shape_path(
            document.view().node(id).unwrap(),
            Size::new(100.0, 50.0),
            0.0,
        );
        let points: Vec<Point> = outline
            .path
            .elements()
            .iter()
            .filter_map(|element| element.end_point())
            .collect();
        assert_eq!(
            points,
            vec![Point::from(start), Point::from(end)],
            "{flip:?}"
        );
    }
}

#[test]
fn corner_radii_come_from_the_node_or_the_style_and_are_clamped() {
    let (document, id) = one_node(NodeKind::Shape, |txn, id| {
        let radii = [4.0, 8.0, 100.0, 0.0].map(Value::Number);
        txn.set_prop(id, "radii", Value::List(radii.into()))
            .unwrap();
    });
    let node = document.view().node(id).unwrap();
    let rounded = shape_path(node, Size::new(100.0, 50.0), 0.0).path;
    // The top left corner is cut, the bottom left one isn't.
    assert!(!rounded.contains(Point::new(0.5, 0.5)));
    assert!(rounded.contains(Point::new(0.5, 49.5)));
    // 100 is clamped to half the height: the bottom right corner is round.
    assert!(!rounded.contains(Point::new(99.0, 49.0)));
    // The style's radius, when the node has none.
    let (document, id) = preset("rect");
    let node = document.view().node(id).unwrap();
    assert!(
        !shape_path(node, Size::new(100.0, 50.0), 10.0)
            .path
            .contains(Point::new(1.0, 1.0))
    );
    assert!(
        shape_path(node, Size::new(100.0, 50.0), 0.0)
            .path
            .contains(Point::new(1.0, 1.0))
    );
}

#[test]
fn the_sample_board_has_an_item_per_painting_node_in_z_order() {
    let document = crate::demo::launch_plan().unwrap();
    let items = build(&document);
    let view = document.view();
    // Every node paints something: 15 nodes, 15 items.
    assert_eq!(view.node_count(), 15);
    assert_eq!(items.by_node.len(), 15);
    // Keys follow the walk, back to front, with gaps between them.
    let keys: Vec<u64> = view.walk().iter().map(|id| items.by_node[id].z).collect();
    assert!(
        keys.windows(2).all(|pair| pair[1] - pair[0] == Z_GAP),
        "{keys:?}"
    );
    // The frame's children are clipped to it and placed in its space.
    let frame = view
        .walk()
        .into_iter()
        .find(|id| matches!(view.node(*id).unwrap().kind(), NodeKind::Frame))
        .unwrap();
    let frame_box = Rect::new(240.0, 200.0, 780.0, 650.0);
    for child in view.children(Parent::Node(frame)) {
        let item = &items.by_node[&child];
        assert_eq!(item.clips.len(), 1);
        assert!(
            frame_box.inflate(1.0, 1.0).contains_rect(item.bounds),
            "{:?}",
            item.bounds
        );
    }
    assert_eq!(items.by_node[&frame].name.as_deref(), Some("Research"));
}

#[test]
fn hidden_nodes_and_their_subtrees_have_no_items_and_groups_paint_nothing() {
    let mut document = Document::new(DocumentKind::Board).unwrap();
    let ((group, inner, hidden, under_hidden), _) = document
        .transact(&Actor::User, |txn| {
            let group = txn.create_node(Parent::Root, None, &NodeKind::Group)?;
            txn.set_geometry(group, Geometry::new(100.0, 100.0, 50.0, 50.0))?;
            let inner = txn.create_node(Parent::Node(group), None, &NodeKind::Shape)?;
            txn.set_geometry(inner, Geometry::new(10.0, 10.0, 20.0, 20.0))?;
            let hidden = txn.create_node(Parent::Root, None, &NodeKind::Frame)?;
            txn.set_prop(hidden, "hidden", Value::Bool(true))?;
            let under = txn.create_node(Parent::Node(hidden), None, &NodeKind::Shape)?;
            Ok((group, inner, hidden, under))
        })
        .unwrap();
    let items = build(&document);
    assert!(!items.by_node.contains_key(&group));
    assert!(!items.by_node.contains_key(&hidden));
    assert!(!items.by_node.contains_key(&under_hidden));
    // The child is placed in the group's space.
    let bounds = items.by_node[&inner].bounds;
    assert!(
        bounds.contains(Point::new(120.0, 120.0)) && !bounds.contains(Point::new(108.0, 108.0)),
        "{bounds:?}"
    );
}

#[test]
fn rotation_turns_the_box_around_its_center() {
    let (document, id) = one_node(NodeKind::Shape, |txn, id| {
        txn.set_geometry(
            id,
            Geometry {
                x: 0.0,
                y: 0.0,
                w: 100.0,
                h: 20.0,
                r: 90.0,
            },
        )
        .unwrap();
    });
    let items = build(&document);
    let bounds = items.by_node[&id].bounds;
    // Upright now: 20 wide and 100 tall around the same center.
    assert!((bounds.center().x - 50.0).abs() < 0.01 && (bounds.center().y - 10.0).abs() < 0.01);
    assert!(
        bounds.height() > 100.0 && bounds.width() < 30.0,
        "{bounds:?}"
    );
}

#[test]
fn bounds_hold_the_stroke_and_unknown_kinds_are_placeholders() {
    let (document, id) = one_node(NodeKind::Shape, |txn, id| {
        txn.set_style_override(id, "stroke_w", Value::Number(10.0))
            .unwrap();
    });
    let bounds = build(&document).by_node[&id].bounds;
    assert!(
        bounds.contains_rect(Rect::new(5.0, 15.0, 115.0, 75.0)),
        "{bounds:?}"
    );
    let (document, id) = one_node(NodeKind::Unknown("table".into()), |_, _| {});
    assert!(matches!(
        build(&document).by_node[&id].look,
        Look::Placeholder
    ));
}

fn golden(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name)
}

/// The sample board at 50 %, drawn on the CPU.
fn render_board(board: &mut BoardCanvas, fonts: &mut Fonts) -> Pixmap {
    let (width, height) = (720u16, 433u16);
    let mut scene = Scene::new();
    let bounds = Rect::new(0.0, 0.0, f64::from(width), f64::from(height));
    board.paint_into(fonts, &mut scene, bounds, Instant::now());
    let background = theme().color(imaginarium_ui::Token::BoardBg);
    CpuRenderer::new(CpuMode::Deterministic).render(&scene, width, height, background)
}

fn check_golden(pixmap: Pixmap, name: &str) {
    let path = golden(name);
    if std::env::var_os("IMAGINARIUM_UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, pixmap.into_png().unwrap()).unwrap();
        return;
    }
    let expected = Pixmap::from_png(std::io::Cursor::new(std::fs::read(&path).unwrap())).unwrap();
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
        differing, 0,
        "{name} differs (IMAGINARIUM_UPDATE_GOLDEN=1 rewrites it)"
    );
}

#[test]
fn the_sample_board_matches_its_golden_images_in_both_themes() {
    let mut fonts = Fonts::bundled_only();
    let mut board = BoardCanvas::new(crate::demo::launch_plan().unwrap());
    board.set_view(Point::ZERO, 0.5);
    set_theme(Theme::dark(DEFAULT_ACCENT));
    check_golden(render_board(&mut board, &mut fonts), "launch-plan-dark.png");
    // A theme switch rebuilds the items in the palette's other versions.
    set_theme(Theme::light(DEFAULT_ACCENT));
    check_golden(
        render_board(&mut board, &mut fonts),
        "launch-plan-light.png",
    );
    set_theme(Theme::dark(DEFAULT_ACCENT));
}

#[test]
fn tiles_paint_what_drawing_directly_paints() {
    let mut fonts = Fonts::bundled_only();
    let mut board = BoardCanvas::new(crate::demo::launch_plan().unwrap());
    board.set_view(Point::ZERO, 0.5);
    let direct = render_board(&mut board, &mut fonts);
    // The next frames draw tiles (rendered in the frame, no workers),
    // within the frame's time budget: on a busy machine over several.
    let mut tiled = render_board(&mut board, &mut fonts);
    for _ in 0..20 {
        if board.stats().owed == 0 {
            break;
        }
        tiled = render_board(&mut board, &mut fonts);
    }
    assert!(board.stats().owed == 0, "every tile rendered");
    let differing = direct
        .data()
        .iter()
        .zip(tiled.data())
        .filter(|(a, b)| a.r.abs_diff(b.r) > 2 || a.g.abs_diff(b.g) > 2 || a.b.abs_diff(b.b) > 2)
        .count();
    assert!(differing < 50, "{differing} pixels differ");
}

#[test]
fn a_theme_switch_drops_the_old_tiles_instead_of_showing_them() {
    let mut fonts = Fonts::bundled_only();
    let mut board = BoardCanvas::new(crate::demo::grid(2_000).unwrap());
    let bounds = Rect::new(0.0, 0.0, 2000.0, 1422.0);
    let frame = |board: &mut BoardCanvas, fonts: &mut Fonts| {
        let mut scene = Scene::new();
        board.paint_into(fonts, &mut scene, bounds, Instant::now());
        board.stats().clone()
    };
    for _ in 0..40 {
        if frame(&mut board, &mut fonts).owed == 0 {
            break;
        }
    }
    set_theme(Theme::light(DEFAULT_ACCENT));
    // No tile of the dark theme is drawn again, not even as a stand-in.
    // The first frame after the switch is drawn directly; the next ones
    // render tiles of the new theme and show nothing old meanwhile.
    frame(&mut board, &mut fonts);
    let stats = frame(&mut board, &mut fonts);
    assert_eq!(stats.reused, 0, "{stats:?}");
    for _ in 0..3 {
        let stats = frame(&mut board, &mut fonts);
        assert_eq!(stats.blank, stats.owed, "nothing stands in: {stats:?}");
    }
    set_theme(Theme::dark(DEFAULT_ACCENT));
}

/// The pixels outside `bounds` (document space) that painting `node`
/// at `scale` touches.
fn pixels_outside(document: &Document, node: NodeId, scale: f64) -> usize {
    let items = build(document);
    let item = &items.by_node[&node];
    let (width, height) = (400u16, 400u16);
    // The item's bounds in the middle of the picture.
    let center = item.bounds.center();
    let camera = imaginarium_render::kurbo::Affine::translate((200.0, 200.0))
        * imaginarium_render::kurbo::Affine::scale(scale)
        * imaginarium_render::kurbo::Affine::translate(-center.to_vec2());
    let mut scene = Scene::new();
    scene.paint_with(camera, |scene| crate::paint::paint(item, scene, scale));
    let pixmap = CpuRenderer::new(CpuMode::Deterministic).render(
        &scene,
        width,
        height,
        imaginarium_render::peniko::Color::TRANSPARENT,
    );
    let inside = camera.transform_rect_bbox(item.bounds).inflate(1.0, 1.0);
    let mut outside = 0;
    for (index, pixel) in pixmap.data().iter().enumerate() {
        let (x, y) = (
            (index % usize::from(width)) as f64 + 0.5,
            (index / usize::from(width)) as f64 + 0.5,
        );
        if pixel.a > 0 && !inside.contains(Point::new(x, y)) {
            outside += 1;
        }
    }
    outside
}

#[test]
fn turned_shapes_paint_inside_their_bounds_at_every_scale() {
    for preset in ["diamond", "ellipse", "triangle", "rect", "line"] {
        let (document, id) = one_node(NodeKind::Shape, |txn, id| {
            txn.set_prop(id, "shape", Value::String(preset.into()))
                .unwrap();
            txn.set_geometry(
                id,
                Geometry {
                    x: 0.0,
                    y: 0.0,
                    w: 400.0,
                    h: 120.0,
                    r: 45.0,
                },
            )
            .unwrap();
            txn.set_style_override(id, "fill", Value::String("#E0E7FF".into()))
                .unwrap();
            txn.insert_text(id, 0, "Label").unwrap();
        });
        for scale in [0.1, 0.5, 1.0] {
            assert_eq!(
                pixels_outside(&document, id, scale),
                0,
                "{preset} at {scale}"
            );
        }
    }
    let (document, id) = one_node(NodeKind::Text, |txn, id| {
        txn.insert_text(id, 0, "Ïfjgy ÅÉ").unwrap();
        txn.set_style_override(id, "size", Value::Number(200.0))
            .unwrap();
    });
    assert_eq!(pixels_outside(&document, id, 0.5), 0, "big text");
}

#[test]
fn a_frames_opacity_fades_it() {
    let (document, id) = one_node(NodeKind::Frame, |txn, id| {
        txn.set_style_override(id, "fill", Value::String("#FFFFFF".into()))
            .unwrap();
        txn.set_style_override(id, "opacity", Value::Number(0.5))
            .unwrap();
    });
    let items = build(&document);
    let mut scene = Scene::new();
    crate::paint::paint(&items.by_node[&id], &mut scene, 1.0);
    let pixmap = CpuRenderer::new(CpuMode::Deterministic).render(
        &scene,
        200,
        100,
        imaginarium_render::peniko::Color::TRANSPARENT,
    );
    // Inside the frame (10, 20, 100 × 50): white at half opacity.
    let pixel = pixmap.data()[40 * 200 + 50];
    assert!((i32::from(pixel.a) - 128).abs() <= 1, "{pixel:?}");
}

/// An empty board, drawn on the CPU: its background alone.
fn render_background(background: crate::Background) -> Pixmap {
    let mut board = BoardCanvas::new(Document::new(DocumentKind::Board).unwrap());
    board.set_background(background);
    let mut fonts = Fonts::bundled_only();
    let (width, height) = (96u16, 96u16);
    let mut scene = Scene::new();
    let bounds = Rect::new(0.0, 0.0, f64::from(width), f64::from(height));
    board.paint_into(&mut fonts, &mut scene, bounds, Instant::now());
    let background = theme().color(imaginarium_ui::Token::BoardBg);
    CpuRenderer::new(CpuMode::Deterministic).render(&scene, width, height, background)
}

#[test]
fn grid_lines_are_one_device_pixel_and_plain_draws_nothing() {
    let pixmap = render_background(crate::Background::Grid);
    let width = usize::from(pixmap.width());
    // The background is the most common pixel (a corner may be on a line).
    let mut counts: std::collections::HashMap<[u8; 4], usize> = std::collections::HashMap::new();
    for pixel in pixmap.data() {
        *counts
            .entry([pixel.r, pixel.g, pixel.b, pixel.a])
            .or_default() += 1;
    }
    let back = counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(pixel, _)| pixel)
        .unwrap();
    let height = usize::from(pixmap.height());
    let lit = |x: usize| -> Vec<bool> {
        (0..height)
            .map(|y| {
                let pixel = pixmap.data()[y * width + x];
                [pixel.r, pixel.g, pixel.b, pixel.a] != back
            })
            .collect()
    };
    // Down a column between the vertical lines (the one with the fewest
    // lit pixels): every horizontal line is one pixel tall.
    let column = (0..width)
        .map(lit)
        .min_by_key(|column| column.iter().filter(|on| **on).count())
        .unwrap();
    let lines = column.iter().filter(|line| **line).count();
    assert!(lines >= 3, "{column:?}");
    assert!(
        column.windows(2).all(|pair| !(pair[0] && pair[1])),
        "a line two pixels tall: {column:?}"
    );
    let plain = render_background(crate::Background::Plain);
    let first = plain.data()[0];
    assert!(plain.data().iter().all(|pixel| *pixel == first));
}

#[test]
fn the_interface_size_leaves_the_board_as_big_as_it_was() {
    let mut board = BoardCanvas::new(crate::demo::launch_plan().unwrap());
    board.set_view(Point::ZERO, 1.0);
    let bounds = imaginarium_render::kurbo::Rect::new(0.0, 0.0, 800.0, 600.0);
    // Logical pixels 25 % bigger: a point covers 0.8 of them, so it is
    // still one system pixel.
    imaginarium_ui::set_interface_scale(1.25);
    let at = board.to_document(bounds, Point::new(80.0, 40.0));
    assert!(
        (at.x - 100.0).abs() < 1e-9 && (at.y - 50.0).abs() < 1e-9,
        "{at:?}"
    );
    // The zoom shown is the board's: still 100 %.
    assert_eq!(board.zoom(), 1.0);
    imaginarium_ui::set_interface_scale(1.0);
    let at = board.to_document(bounds, Point::new(80.0, 40.0));
    assert!((at.x - 80.0).abs() < 1e-9, "{at:?}");
}
