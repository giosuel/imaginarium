// SPDX-License-Identifier: AGPL-3.0-or-later

//! The widget tree without a window: layout, input routing, focus, paint
//! caching, signals and the accessibility tree.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::cell::Cell;
use std::rc::Rc;

use accesskit::Role;
use imaginarium_reactive::signal;
use imaginarium_render::Scene;
use imaginarium_render::kurbo::{Point, Rect};
use imaginarium_render::peniko::Color;
use imaginarium_text::{Fonts, TextStyle};
use imaginarium_ui::{
    AccessTree, Button, ButtonColors, Flex, Input, InputColors, Key, Label, Modifiers, TextInput,
    Ui, label,
};

const AREA: Rect = Rect::new(0.0, 0.0, 400.0, 300.0);

fn colors() -> ButtonColors {
    ButtonColors {
        background: Color::WHITE.into(),
        hover: Color::WHITE.into(),
        pressed: Color::WHITE.into(),
        text: Color::BLACK.into(),
        focus: Color::BLACK.into(),
        border: None,
    }
}

fn input_colors() -> InputColors {
    InputColors {
        background: Color::WHITE.into(),
        border: Color::BLACK.into(),
        accent: Color::BLACK.into(),
        placeholder: Color::BLACK.into(),
        text: Color::BLACK.into(),
    }
}

fn press(at: Point, pressed: bool) -> Input {
    Input::PointerButton {
        primary: true,
        pressed,
        position: at,
        clicks: 1,
        modifiers: Modifiers::default(),
    }
}

fn key(key: Key, text: Option<&str>) -> Input {
    Input::Key {
        key,
        text: text.map(str::to_owned),
        pressed: true,
        repeat: false,
        modifiers: Modifiers::default(),
    }
}

/// A column: a text input, a button that counts clicks, and a label.
fn sample(fonts: &mut Fonts) -> (Ui, Rc<Cell<u32>>, [imaginarium_ui::WidgetId; 4]) {
    let mut ui = Ui::new();
    let clicks = Rc::new(Cell::new(0));
    let counter = clicks.clone();
    let input = ui.add(TextInput::new(
        "Name",
        "Your name",
        TextStyle::new(14.0),
        input_colors(),
    ));
    let button = ui.add(Button::new(
        "Count",
        TextStyle::new(14.0),
        colors(),
        move || {
            counter.set(counter.get() + 1);
        },
    ));
    let text = ui.add(Label::new("Hello", TextStyle::new(14.0)));
    let column = ui.add_with_children(
        Flex::column().gap(8.0).padding(10.0, 10.0).fill(),
        &[input, button, text],
    );
    ui.set_root(column);
    ui.layout(fonts, AREA);
    (ui, clicks, [column, input, button, text])
}

#[test]
fn flex_lays_children_out_in_a_column() {
    let mut fonts = Fonts::bundled_only();
    let (ui, _, [column, input, button, text]) = sample(&mut fonts);
    assert_eq!(ui.bounds(column), Some(AREA));
    let input = ui.bounds(input).unwrap();
    let button = ui.bounds(button).unwrap();
    let text = ui.bounds(text).unwrap();
    assert_eq!(input.x0, 10.0);
    assert_eq!(input.width(), 380.0, "the input fills the width");
    assert!(button.y0 >= input.y1 + 8.0 - 0.5, "gap after the input");
    assert!(text.y0 >= button.y1 + 8.0 - 0.5);
    assert!(
        text.width() > 20.0 && text.height() > 10.0,
        "measured: {text:?}"
    );
}

#[test]
fn clicks_focus_tab_moves_and_enter_activates() {
    let mut fonts = Fonts::bundled_only();
    let (mut ui, clicks, [_, input, button, _]) = sample(&mut fonts);
    let at = ui.bounds(input).unwrap().center();
    ui.input(&mut fonts, &press(at, true));
    ui.input(&mut fonts, &press(at, false));
    assert_eq!(ui.focused(), Some(input));
    for c in ["h", "i"] {
        ui.input(&mut fonts, &key(Key::Character(c.into()), Some(c)));
    }
    assert_eq!(ui.widget::<TextInput>(input).unwrap().text(), "hi");
    ui.input(&mut fonts, &key(Key::Tab, None));
    assert_eq!(ui.focused(), Some(button), "Tab to the next focusable");
    ui.input(&mut fonts, &key(Key::Enter, None));
    assert_eq!(clicks.get(), 1);
    // A click on the button: press and release inside.
    let at = ui.bounds(button).unwrap().center();
    ui.input(&mut fonts, &press(at, true));
    ui.input(&mut fonts, &press(at, false));
    assert_eq!(clicks.get(), 2);
    // Pressing inside and releasing outside cancels.
    ui.input(&mut fonts, &press(at, true));
    ui.input(&mut fonts, &press(Point::new(390.0, 290.0), false));
    assert_eq!(clicks.get(), 2);
    // Tab wraps around, Shift+Tab goes back.
    ui.input(&mut fonts, &key(Key::Tab, None));
    assert_eq!(ui.focused(), Some(input));
}

#[test]
fn unchanged_widgets_reuse_their_scenes() {
    let mut fonts = Fonts::bundled_only();
    let (mut ui, _, [_, _, button, _]) = sample(&mut fonts);
    let mut scene = Scene::new();
    ui.paint(&mut fonts, &mut scene);
    assert_eq!(ui.stats().repainted, 4);
    let commands = scene.len();
    let mut again = Scene::new();
    ui.layout(&mut fonts, AREA);
    ui.paint(&mut fonts, &mut again);
    assert_eq!(ui.stats().repainted, 0, "nothing changed");
    assert_eq!(again.len(), commands, "the same picture from the caches");
    // Hovering the button repaints only the button.
    let at = ui.bounds(button).unwrap().center();
    ui.input(&mut fonts, &Input::PointerMoved(at));
    ui.layout(&mut fonts, AREA);
    ui.paint(&mut fonts, &mut Scene::new());
    assert_eq!(ui.stats().repainted, 1);
}

#[test]
fn a_label_follows_its_signal() {
    let mut fonts = Fonts::bundled_only();
    let mut ui = Ui::new();
    let count = signal(1);
    let text = label(&mut ui, TextStyle::new(14.0), Color::BLACK, move || {
        format!("{} items", count.get())
    });
    let row = ui.add_with_children(Flex::row().fill(), &[text]);
    ui.set_root(row);
    ui.layout(&mut fonts, AREA);
    let narrow = ui.bounds(text).unwrap().width();
    ui.paint(&mut fonts, &mut Scene::new());
    count.set(1_000_000);
    ui.layout(&mut fonts, AREA);
    assert!(ui.bounds(text).unwrap().width() > narrow, "re-laid out");
    ui.paint(&mut fonts, &mut Scene::new());
    assert_eq!(
        ui.stats().repainted,
        1,
        "only the label (the row keeps its box)"
    );
    // Removing the label disposes its effect.
    let before = imaginarium_reactive::live_nodes_for_tests();
    ui.remove(text);
    assert!(imaginarium_reactive::live_nodes_for_tests() < before);
    count.set(2); // no effect left to invalidate a removed widget
    ui.layout(&mut fonts, AREA);
}

#[test]
fn the_access_tree_has_roles_names_and_focus() {
    let mut fonts = Fonts::bundled_only();
    let (mut ui, _, [_, input, _, _]) = sample(&mut fonts);
    ui.set_focus(&mut fonts, Some(input));
    let mut tree = AccessTree::default();
    ui.access(&mut fonts, &mut tree);
    let roles: Vec<Role> = tree.nodes().iter().map(|(_, node)| node.role()).collect();
    assert!(roles.contains(&Role::TextInput));
    assert!(roles.contains(&Role::Button));
    assert!(roles.contains(&Role::Label));
    let button = tree
        .nodes()
        .iter()
        .find(|(_, node)| node.role() == Role::Button)
        .unwrap();
    assert_eq!(button.1.label(), Some("Count"));
    assert_eq!(tree.focus().map(|id| id.0), Some(input.access_base()));
}

/// A widget that renders a texture once and draws it (what a canvas does).
struct Textured(imaginarium_render::TextureKey);

impl imaginarium_ui::Widget for Textured {
    fn style(&self) -> taffy::Style {
        taffy::Style {
            size: taffy::Size {
                width: taffy::style_helpers::length(64.0),
                height: taffy::style_helpers::length(64.0),
            },
            ..taffy::Style::default()
        }
    }

    fn paint(&mut self, cx: &mut imaginarium_ui::Cx<'_>, scene: &mut Scene) {
        let mut content = Scene::new();
        content.fill(&Rect::new(0.0, 0.0, 8.0, 8.0), Color::BLACK);
        scene.render_texture(self.0, 8, 8, content);
        scene.draw_texture(self.0, 8, 8, cx.bounds());
    }
}

#[test]
fn a_cached_scene_does_not_render_its_textures_again() {
    let mut fonts = Fonts::bundled_only();
    let mut ui = Ui::new();
    let textured = ui.add(Textured(imaginarium_render::TextureKey::unique()));
    let column = ui.add_with_children(Flex::column().fill(), &[textured]);
    ui.set_root(column);
    ui.layout(&mut fonts, AREA);
    let mut first = Scene::new();
    ui.paint(&mut fonts, &mut first);
    assert_eq!(first.texture_renders(), 1);
    let mut again = Scene::new();
    ui.layout(&mut fonts, AREA);
    ui.paint(&mut fonts, &mut again);
    assert_eq!(ui.stats().repainted, 0);
    assert_eq!(again.len(), first.len(), "still drawn, from the cache");
    assert_eq!(again.texture_renders(), 0, "but not rendered again");
}
