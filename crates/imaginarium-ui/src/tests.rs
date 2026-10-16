// SPDX-License-Identifier: AGPL-3.0-or-later

//! Regression tests from the M2.9 review: the tree's accessibility updates
//! (checked with AccessKit's own consumer), focus and pointer capture.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::cell::Cell;
use std::rc::Rc;

use accesskit::{Action, ActionData, ActionRequest, NodeId, TextPosition, TextSelection};
use imaginarium_render::kurbo::{Point, Rect, Size};
use imaginarium_render::peniko::Color;
use imaginarium_text::{Fonts, TextStyle};
use taffy::prelude::length;

use crate::access::AccessTree;
use crate::theme::{DEFAULT_ACCENT, Theme, Token, set_theme};
use crate::tree::{Cx, Ui, Widget, WidgetId};
use crate::widgets::{Button, ButtonColors, Flex, InputColors, Label, TextInput};
use crate::window::{Input, Key, Modifiers};

const AREA: Rect = Rect::new(0.0, 0.0, 400.0, 300.0);

fn button_colors() -> ButtonColors {
    ButtonColors {
        background: Color::WHITE.into(),
        hover: Color::WHITE.into(),
        pressed: Color::WHITE.into(),
        text: Color::BLACK.into(),
        focus: Color::BLACK.into(),
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

fn input() -> TextInput {
    TextInput::new("Name", "", TextStyle::new(14.0), input_colors())
}

fn button(clicks: &Rc<Cell<u32>>) -> Button {
    let clicks = clicks.clone();
    Button::new("Go", TextStyle::new(14.0), button_colors(), move || {
        clicks.set(clicks.get() + 1);
    })
}

fn pointer(at: Point, primary: bool, pressed: bool) -> Input {
    Input::PointerButton {
        primary,
        pressed,
        position: at,
        clicks: 1,
        modifiers: Modifiers::default(),
    }
}

/// What the shell sends AccessKit, fed to AccessKit's own tree, which
/// panics on an inconsistent update (as the platform adapters do).
fn check_access(ui: &mut Ui, fonts: &mut Fonts) -> accesskit::TreeUpdate {
    let mut tree = AccessTree::default();
    ui.access(fonts, &mut tree);
    let update = tree.into_update("Test", 1.0);
    let _ = accesskit_consumer::Tree::new(update.clone(), true);
    update
}

#[test]
fn every_access_update_is_a_whole_tree() {
    // Updates before AccessKit asked for the first tree must be complete
    // too: the adapter builds its tree from whichever update comes first.
    let update = AccessTree::default().into_update("Test", 1.0);
    assert!(update.tree.is_some());
    let _ = accesskit_consumer::Tree::new(update, true);
}

#[test]
fn a_hidden_or_detached_focused_widget_loses_the_focus() {
    let mut fonts = Fonts::bundled_only();
    let mut ui = Ui::new();
    let clicks = Rc::new(Cell::new(0));
    let field = ui.add(input());
    let other = ui.add(button(&clicks));
    let column = ui.add_with_children(Flex::column().fill(), &[field, other]);
    ui.set_root(column);
    ui.layout(&mut fonts, AREA);
    ui.set_focus(&mut fonts, Some(field));
    check_access(&mut ui, &mut fonts);
    // Detached (as a closed panel is): keys must not go to it, and the
    // screen reader's focus must name a node that exists.
    ui.set_children(column, &[other]);
    ui.layout(&mut fonts, AREA);
    assert_eq!(ui.focused(), None);
    check_access(&mut ui, &mut fonts);
    // Hidden by its style.
    ui.set_children(column, &[field, other]);
    ui.layout(&mut fonts, AREA);
    ui.set_focus(&mut fonts, Some(field));
    let hidden = ui.add_with_children(
        Flex::column().style(|style| style.display = taffy::Display::None),
        &[],
    );
    ui.set_children(hidden, &[field]);
    ui.set_children(column, &[hidden, other]);
    ui.layout(&mut fonts, AREA);
    assert_eq!(ui.focused(), None);
    check_access(&mut ui, &mut fonts);
}

#[test]
fn the_access_focus_names_an_existing_node() {
    // A focusable widget without a node of its own.
    struct Plain;
    impl Widget for Plain {
        fn style(&self) -> taffy::Style {
            taffy::Style {
                size: taffy::Size {
                    width: length(50.0),
                    height: length(20.0),
                },
                ..taffy::Style::default()
            }
        }
        fn focusable(&self) -> bool {
            true
        }
    }
    let mut fonts = Fonts::bundled_only();
    let mut ui = Ui::new();
    let plain = ui.add(Plain);
    let column = ui.add_with_children(Flex::column().fill(), &[plain]);
    ui.set_root(column);
    ui.layout(&mut fonts, AREA);
    ui.set_focus(&mut fonts, Some(plain));
    check_access(&mut ui, &mut fonts);
}

#[test]
fn a_screen_reader_cannot_focus_a_removed_widget() {
    let mut fonts = Fonts::bundled_only();
    let mut ui = Ui::new();
    let clicks = Rc::new(Cell::new(0));
    let old = ui.add(button(&clicks));
    let column = ui.add_with_children(Flex::column().fill(), &[old]);
    ui.set_root(column);
    ui.layout(&mut fonts, AREA);
    ui.remove(old);
    let request = ActionRequest {
        action: Action::Focus,
        target_tree: accesskit::TreeId::ROOT,
        target_node: NodeId(old.access_base()),
        data: None,
    };
    assert!(!ui.access_action(&mut fonts, &request));
    let new = ui.add(button(&clicks)); // reuses the slot
    ui.set_children(column, &[new]);
    ui.layout(&mut fonts, AREA);
    assert_eq!(ui.focused(), None);
}

#[test]
fn a_screen_reader_moving_the_caret_focuses_the_field_without_a_change() {
    let mut fonts = Fonts::bundled_only();
    let mut ui = Ui::new();
    let changes = Rc::new(Cell::new(0));
    let counter = changes.clone();
    let a = ui.add(input());
    let b = ui.add(input().on_change(move |_| counter.set(counter.get() + 1)));
    let column = ui.add_with_children(Flex::column().fill(), &[a, b]);
    ui.set_root(column);
    ui.layout(&mut fonts, AREA);
    ui.widget_mut::<TextInput>(b)
        .unwrap()
        .set_text(&mut fonts, "hello");
    ui.layout(&mut fonts, AREA);
    ui.set_focus(&mut fonts, Some(a));
    let run = NodeId(b.access_base() + 1);
    let at = |character_index| TextPosition {
        node: run,
        character_index,
    };
    let request = ActionRequest {
        action: Action::SetTextSelection,
        target_tree: accesskit::TreeId::ROOT,
        target_node: NodeId(b.access_base()),
        data: Some(ActionData::SetTextSelection(TextSelection {
            anchor: at(1),
            focus: at(3),
        })),
    };
    assert!(ui.access_action(&mut fonts, &request));
    assert_eq!(ui.focused(), Some(b), "the tree knows who has the focus");
    assert_eq!(changes.get(), 0, "the text didn't change");
    check_access(&mut ui, &mut fonts);
}

#[test]
fn the_pointer_stays_captured_until_every_button_is_up() {
    let mut fonts = Fonts::bundled_only();
    let mut ui = Ui::new();
    let clicks = Rc::new(Cell::new(0));
    let field = ui.add(input());
    let other = ui.add(button(&clicks));
    let column = ui.add_with_children(Flex::column().fill(), &[field, other]);
    ui.set_root(column);
    ui.layout(&mut fonts, AREA);
    ui.widget_mut::<TextInput>(field)
        .unwrap()
        .set_text(&mut fonts, "hello world");
    ui.layout(&mut fonts, AREA);
    let inside = ui.bounds(field).unwrap().center();
    let elsewhere = ui.bounds(other).unwrap().center();
    ui.input(&mut fonts, &pointer(inside, true, true));
    ui.input(&mut fonts, &pointer(inside, false, true));
    ui.input(&mut fonts, &pointer(inside, false, false));
    ui.input(&mut fonts, &pointer(elsewhere, true, false));
    let selection = |ui: &mut Ui| {
        ui.widget_mut::<TextInput>(field)
            .unwrap()
            .field()
            .editor()
            .selection()
    };
    let before = selection(&mut ui);
    let start = ui.bounds(field).unwrap().origin() + (2.0, 5.0);
    ui.input(&mut fonts, &Input::PointerMoved(start));
    assert_eq!(selection(&mut ui), before, "no button is held: no drag");
}

#[test]
fn losing_the_window_focus_ends_the_capture() {
    let mut fonts = Fonts::bundled_only();
    let mut ui = Ui::new();
    let clicks = Rc::new(Cell::new(0));
    let first = ui.add(button(&clicks));
    let second = ui.add(button(&clicks));
    let column = ui.add_with_children(Flex::column().fill(), &[first, second]);
    ui.set_root(column);
    ui.layout(&mut fonts, AREA);
    let a = ui.bounds(first).unwrap().center();
    let b = ui.bounds(second).unwrap().center();
    ui.input(&mut fonts, &pointer(a, true, true));
    // The release happened in another window.
    ui.input(&mut fonts, &Input::WindowFocus(false));
    ui.input(&mut fonts, &Input::WindowFocus(true));
    ui.input(&mut fonts, &pointer(b, true, true));
    ui.input(&mut fonts, &pointer(b, true, false));
    assert_eq!(ui.focused(), Some(second), "the press went where it was");
    assert_eq!(clicks.get(), 1);
}

/// A box whose size is a field, to change through `widget_mut`.
struct Sized(f32, bool);

impl Widget for Sized {
    fn style(&self) -> taffy::Style {
        taffy::Style {
            size: taffy::Size {
                width: length(self.0),
                height: length(self.0),
            },
            flex_shrink: 0.0,
            ..taffy::Style::default()
        }
    }
    fn clips(&self) -> bool {
        self.1
    }
}

#[test]
fn a_style_changed_through_widget_mut_is_laid_out() {
    let mut fonts = Fonts::bundled_only();
    let mut ui = Ui::new();
    let item = ui.add(Sized(20.0, false));
    let column = ui.add_with_children(Flex::column().fill(), &[item]);
    ui.set_root(column);
    ui.layout(&mut fonts, AREA);
    ui.widget_mut::<Sized>(item).unwrap().0 = 60.0;
    ui.layout(&mut fonts, AREA);
    assert_eq!(ui.bounds(item).unwrap().size(), Size::new(60.0, 60.0));
}

#[test]
fn children_overflowing_a_parent_that_does_not_clip_get_clicks() {
    struct Pressable(Rc<Cell<u32>>);
    impl Widget for Pressable {
        fn style(&self) -> taffy::Style {
            Sized(80.0, false).style()
        }
        fn event(&mut self, _cx: &mut Cx<'_>, input: &Input) -> bool {
            if let Input::PointerButton { pressed: true, .. } = input {
                self.0.set(self.0.get() + 1);
            }
            true
        }
    }
    let mut fonts = Fonts::bundled_only();
    let mut ui = Ui::new();
    let presses = Rc::new(Cell::new(0));
    let child = ui.add(Pressable(presses.clone()));
    let small = ui.add_with_children(Sized(40.0, false), &[child]);
    let clipping = ui.add_with_children(Sized(40.0, true), &[]);
    let column = ui.add_with_children(Flex::column().fill(), &[small, clipping]);
    ui.set_root(column);
    ui.layout(&mut fonts, AREA);
    let child_box = ui.bounds(child).unwrap();
    let outside_parent = Point::new(child_box.x1 - 5.0, child_box.y0 + 5.0);
    assert!(!ui.bounds(small).unwrap().contains(outside_parent));
    ui.input(&mut fonts, &pointer(outside_parent, true, true));
    ui.input(&mut fonts, &pointer(outside_parent, true, false));
    assert_eq!(presses.get(), 1);
}

#[test]
fn removing_a_wide_subtree_is_linear() {
    let mut fonts = Fonts::bundled_only();
    let mut ui = Ui::new();
    let children: Vec<WidgetId> = (0..10_000).map(|_| ui.add(Sized(1.0, false))).collect();
    let column = ui.add_with_children(Flex::column().fill(), &children);
    ui.set_root(column);
    ui.layout(&mut fonts, AREA);
    let start = std::time::Instant::now();
    ui.remove(column);
    // Linear: milliseconds in a debug build (quadratic took about 2 s).
    assert!(start.elapsed().as_millis() < 500, "{:?}", start.elapsed());
}

#[test]
fn a_held_key_activates_a_button_once() {
    let mut fonts = Fonts::bundled_only();
    let mut ui = Ui::new();
    let clicks = Rc::new(Cell::new(0));
    let go = ui.add(button(&clicks));
    let column = ui.add_with_children(Flex::column().fill(), &[go]);
    ui.set_root(column);
    ui.layout(&mut fonts, AREA);
    ui.set_focus(&mut fonts, Some(go));
    for repeat in [false, true, true] {
        ui.input(
            &mut fonts,
            &Input::Key {
                key: Key::Enter,
                text: None,
                pressed: true,
                repeat,
                modifiers: Modifiers::default(),
            },
        );
    }
    assert_eq!(clicks.get(), 1);
}

#[test]
fn a_theme_switch_repaints_every_widget_and_reshapes_labels() {
    let mut fonts = Fonts::bundled_only();
    let mut ui = Ui::new();
    let label = ui.add(Label::new("Saved", TextStyle::new(13.0)).color(Token::Text));
    let panel = ui.add_with_children(Flex::column().fill().background(Token::PanelBg), &[label]);
    ui.set_root(panel);
    let mut scene = imaginarium_render::Scene::new();
    ui.layout(&mut fonts, AREA);
    ui.paint(&mut fonts, &mut scene);
    let dark = Theme::dark(DEFAULT_ACCENT);
    let light = Theme::light(DEFAULT_ACCENT);
    assert_eq!(
        ui.widget::<Label>(label).unwrap().shaped_color(),
        Some(dark.color(Token::Text))
    );

    // Nothing changed: nothing repaints.
    ui.layout(&mut fonts, AREA);
    ui.paint(&mut fonts, &mut scene);
    assert_eq!(ui.stats().repainted, 0);

    set_theme(light.clone());
    ui.layout(&mut fonts, AREA);
    ui.paint(&mut fonts, &mut scene);
    assert_eq!(ui.stats().repainted, 2);
    assert_eq!(
        ui.widget::<Label>(label).unwrap().shaped_color(),
        Some(light.color(Token::Text))
    );
    set_theme(dark);
}

#[test]
fn inputs_and_buttons_follow_the_theme() {
    let mut fonts = Fonts::bundled_only();
    let mut ui = Ui::new();
    let style = TextStyle::new(14.0);
    let field = ui.add(TextInput::new("Name", "", style, InputColors::themed()));
    let save = ui.add(Button::new("Save", style, ButtonColors::primary(), || {}));
    let panel = ui.add_with_children(Flex::column().fill(), &[field, save]);
    ui.set_root(panel);
    let mut scene = imaginarium_render::Scene::new();
    let dark = Theme::dark(DEFAULT_ACCENT);
    let light = Theme::light(DEFAULT_ACCENT);
    let colors = |ui: &mut Ui| {
        let text = ui
            .widget_mut::<TextInput>(field)
            .unwrap()
            .field()
            .editor()
            .style()
            .color;
        let label = ui.widget::<Button>(save).unwrap().label_color();
        (text, label)
    };

    ui.layout(&mut fonts, AREA);
    ui.paint(&mut fonts, &mut scene);
    assert_eq!(
        colors(&mut ui),
        (dark.color(Token::Text), Some(dark.color(Token::OnAccent)))
    );

    set_theme(light.clone());
    ui.layout(&mut fonts, AREA);
    ui.paint(&mut fonts, &mut scene);
    assert_eq!(
        colors(&mut ui),
        (light.color(Token::Text), Some(light.color(Token::OnAccent)))
    );
    set_theme(dark);
}
