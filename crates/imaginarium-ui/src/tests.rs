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
/// Runs every animation to its end and lays out the result.
fn settle(ui: &mut Ui, fonts: &mut Fonts) {
    let later = std::time::Instant::now() + std::time::Duration::from_secs(1);
    crate::animation::set_clock_for_tests(Some(later));
    ui.wake(fonts, later);
    frame(ui, fonts);
    crate::animation::set_clock_for_tests(None);
}

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

fn key(key: Key) -> Input {
    Input::Key {
        key,
        text: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::default(),
    }
}

/// A frame's worth of tree work.
fn frame(ui: &mut Ui, fonts: &mut Fonts) {
    let mut scene = imaginarium_render::Scene::new();
    ui.layout(fonts, AREA);
    ui.paint(fonts, &mut scene);
}

/// A window with an opener button at the top left and a second button
/// further down, for layers to open over.
fn window_with_buttons(ui: &mut Ui, clicks: &Rc<Cell<u32>>) -> (WidgetId, WidgetId) {
    let opener = ui.add(button(clicks));
    let below = ui.add(button(clicks));
    let root = ui.add_with_children(
        Flex::column().fill().gap(100.0).padding(10.0, 10.0),
        &[opener, below],
    );
    ui.set_root(root);
    (opener, below)
}

fn menu(ui: &mut Ui, clicks: &Rc<Cell<u32>>) -> (WidgetId, WidgetId, WidgetId) {
    let first = ui.add(button(clicks));
    let second = ui.add(button(clicks));
    let content = ui.add_with_children(Flex::column().padding(4.0, 4.0), &[first, second]);
    (content, first, second)
}

#[test]
fn layers_and_hovered_buttons_animate_then_come_to_rest() {
    use crate::animation::{durations, set_clock_for_tests};
    use crate::layers::{Anchor, LayerOptions};
    let mut fonts = Fonts::bundled_only();
    let mut ui = Ui::new();
    let clicks = Rc::new(Cell::new(0));
    let (opener, below) = window_with_buttons(&mut ui, &clicks);
    let start = std::time::Instant::now();
    set_clock_for_tests(Some(start));
    frame(&mut ui, &mut fonts);
    assert_eq!(ui.wake_at(), None, "nothing moves at first");
    // A hover fades the button's color in.
    let at = ui.bounds(below).unwrap().center();
    ui.input(&mut fonts, &Input::PointerMoved(at));
    assert!(
        ui.wake_at()
            .is_some_and(|at| at <= start + durations::HOVER)
    );
    // A menu fades in, and wakes once more when it arrives.
    let (content, _, _) = menu(&mut ui, &clicks);
    ui.open_layer(content, LayerOptions::popover(Anchor::Widget(opener)));
    frame(&mut ui, &mut fonts);
    let end = start + durations::POPOVER;
    set_clock_for_tests(Some(end));
    assert!(ui.wake_at().is_some_and(|at| at <= end));
    ui.wake(&mut fonts, end);
    frame(&mut ui, &mut fonts);
    assert_eq!(ui.wake_at(), None, "at rest, nothing redraws");
    set_clock_for_tests(None);
}

#[test]
fn a_popover_takes_the_focus_and_esc_gives_it_back() {
    use crate::layers::{Anchor, LayerOptions};
    let mut fonts = Fonts::bundled_only();
    let mut ui = Ui::new();
    let clicks = Rc::new(Cell::new(0));
    let (opener, _) = window_with_buttons(&mut ui, &clicks);
    frame(&mut ui, &mut fonts);
    ui.set_focus(&mut fonts, Some(opener));
    let closed = Rc::new(Cell::new(0));
    let (content, first, _) = menu(&mut ui, &clicks);
    let layer = ui.open_layer(
        content,
        LayerOptions::popover(Anchor::Widget(opener)).on_close({
            let closed = closed.clone();
            move || closed.set(closed.get() + 1)
        }),
    );
    frame(&mut ui, &mut fonts);
    assert_eq!(
        ui.focused(),
        Some(first),
        "the menu's first item has the focus"
    );
    let (anchor, menu_box) = (ui.bounds(opener).unwrap(), ui.bounds(content).unwrap());
    assert_eq!(menu_box.y0, anchor.y1 + 8.0, "opens below its anchor");
    assert_eq!(menu_box.x0, anchor.x0);

    // Tab stays in the menu.
    ui.input(&mut fonts, &key(Key::Tab));
    ui.input(&mut fonts, &key(Key::Tab));
    assert_eq!(ui.focused(), Some(first));

    assert!(ui.input(&mut fonts, &key(Key::Escape)));
    assert!(!ui.is_layer_open(layer));
    assert_eq!(
        ui.focused(),
        Some(opener),
        "the focus is back on the opener"
    );
    assert_eq!(closed.get(), 1);
    assert!(ui.bounds(first).is_none(), "its widgets are gone");
}

#[test]
fn a_click_outside_a_popover_closes_it_without_clicking_below() {
    use crate::layers::{Anchor, LayerOptions};
    let mut fonts = Fonts::bundled_only();
    let mut ui = Ui::new();
    let clicks = Rc::new(Cell::new(0));
    let (opener, below) = window_with_buttons(&mut ui, &clicks);
    frame(&mut ui, &mut fonts);
    let (content, first, _) = menu(&mut ui, &clicks);
    let layer = ui.open_layer(content, LayerOptions::popover(Anchor::Widget(opener)));
    frame(&mut ui, &mut fonts);

    // A click inside runs the item and keeps the menu.
    let item = ui.bounds(first).unwrap().center();
    ui.input(&mut fonts, &pointer(item, true, true));
    ui.input(&mut fonts, &pointer(item, true, false));
    assert_eq!(clicks.get(), 1);
    assert!(ui.is_layer_open(layer));

    // A click on the button below only closes the menu (unless the menu
    // happens to cover it).
    let target = ui.bounds(below).unwrap().center();
    assert!(!ui.bounds(content).unwrap().contains(target));
    ui.input(&mut fonts, &pointer(target, true, true));
    ui.input(&mut fonts, &pointer(target, true, false));
    assert!(!ui.is_layer_open(layer));
    assert_eq!(clicks.get(), 1, "the press was used up by closing");

    // The next click reaches it.
    frame(&mut ui, &mut fonts);
    ui.input(&mut fonts, &pointer(target, true, true));
    ui.input(&mut fonts, &pointer(target, true, false));
    assert_eq!(clicks.get(), 2);
}

#[test]
fn a_modal_takes_all_input() {
    use crate::layers::LayerOptions;
    let mut fonts = Fonts::bundled_only();
    let mut ui = Ui::new();
    let clicks = Rc::new(Cell::new(0));
    let (opener, _) = window_with_buttons(&mut ui, &clicks);
    frame(&mut ui, &mut fonts);
    let dialog_clicks = Rc::new(Cell::new(0));
    let (content, first, second) = menu(&mut ui, &dialog_clicks);
    let layer = ui.open_layer(content, LayerOptions::modal());
    frame(&mut ui, &mut fonts);
    let dialog = ui.bounds(content).unwrap();
    assert!((dialog.center() - AREA.center()).hypot() < 1.0, "centered");

    // Clicks outside it reach nothing and don't close it.
    let target = ui.bounds(opener).unwrap().center();
    assert!(!dialog.contains(target));
    ui.input(&mut fonts, &pointer(target, true, true));
    ui.input(&mut fonts, &pointer(target, true, false));
    assert_eq!(clicks.get(), 0);
    assert!(ui.is_layer_open(layer));

    // Tab cycles inside it; Enter reaches its focused button.
    assert_eq!(ui.focused(), Some(first));
    ui.input(&mut fonts, &key(Key::Tab));
    assert_eq!(ui.focused(), Some(second));
    ui.input(&mut fonts, &key(Key::Tab));
    assert_eq!(ui.focused(), Some(first));
    ui.input(&mut fonts, &key(Key::Enter));
    assert_eq!(dialog_clicks.get(), 1);

    ui.input(&mut fonts, &key(Key::Escape));
    assert!(!ui.is_layer_open(layer));
}

#[test]
fn a_layer_closes_with_its_anchor_and_a_widget_can_close_its_layer() {
    use crate::layers::{Anchor, LayerOptions};
    let mut fonts = Fonts::bundled_only();
    let mut ui = Ui::new();
    let clicks = Rc::new(Cell::new(0));
    let (opener, _) = window_with_buttons(&mut ui, &clicks);
    frame(&mut ui, &mut fonts);
    let (content, _, _) = menu(&mut ui, &clicks);
    let layer = ui.open_layer(content, LayerOptions::popover(Anchor::Widget(opener)));
    frame(&mut ui, &mut fonts);
    ui.remove(opener);
    frame(&mut ui, &mut fonts);
    assert!(!ui.is_layer_open(layer), "its anchor went away");

    /// Closes its layer when pressed, like a menu item that ran.
    struct Closer;
    impl Widget for Closer {
        fn style(&self) -> taffy::Style {
            taffy::Style {
                size: taffy::Size {
                    width: length(80.0),
                    height: length(30.0),
                },
                ..taffy::Style::default()
            }
        }
        fn event(&mut self, cx: &mut Cx<'_>, input: &Input) -> bool {
            if let Input::PointerButton { pressed: false, .. } = input {
                cx.close_layer();
                return true;
            }
            false
        }
    }
    let closer = ui.add(Closer);
    let content = ui.add_with_children(Flex::column(), &[closer]);
    let layer = ui.open_layer(
        content,
        LayerOptions::popover(Anchor::Point(Point::new(50.0, 50.0))),
    );
    frame(&mut ui, &mut fonts);
    let at = ui.bounds(closer).unwrap().center();
    ui.input(&mut fonts, &pointer(at, true, true));
    ui.input(&mut fonts, &pointer(at, true, false));
    assert!(!ui.is_layer_open(layer));
}

#[test]
fn tooltips_take_no_input_and_layers_are_painted_and_read_aloud() {
    use crate::layers::{Anchor, LayerOptions};
    let mut fonts = Fonts::bundled_only();
    let mut ui = Ui::new();
    let clicks = Rc::new(Cell::new(0));
    let (opener, _) = window_with_buttons(&mut ui, &clicks);
    frame(&mut ui, &mut fonts);
    let tip = ui.add(
        Flex::row()
            .width(120.0)
            .height(40.0)
            .background(Token::TooltipBg)
            .role(accesskit::Role::Tooltip, "Go · Enter"),
    );
    ui.open_layer(tip, LayerOptions::tooltip(Anchor::Widget(opener)));
    frame(&mut ui, &mut fonts);
    // A click on the tooltip goes through to the button under it, if any.
    let opener_box = ui.bounds(opener).unwrap();
    ui.input(&mut fonts, &pointer(opener_box.center(), true, true));
    ui.input(&mut fonts, &pointer(opener_box.center(), true, false));
    assert_eq!(clicks.get(), 1);
    assert_eq!(ui.stats().widgets, 4);
    let update = check_access(&mut ui, &mut fonts);
    let tooltip_node = NodeId(tip.access_base());
    assert!(
        update.nodes.iter().any(|(id, _)| *id == tooltip_node),
        "the tooltip is in the tree"
    );
}

fn typed(text: &str) -> Input {
    Input::Key {
        key: Key::Character(text.into()),
        text: Some(text.into()),
        pressed: true,
        repeat: false,
        modifiers: Modifiers::default(),
    }
}

#[test]
fn keys_never_reach_a_field_behind_a_dialog() {
    use crate::layers::LayerOptions;
    let mut fonts = Fonts::bundled_only();
    let mut ui = Ui::new();
    let field = ui.add(input());
    let root = ui.add_with_children(Flex::column().fill(), &[field]);
    ui.set_root(root);
    frame(&mut ui, &mut fonts);
    ui.set_focus(&mut fonts, Some(field));
    ui.input(&mut fonts, &typed("a"));

    // A dialog with nothing to focus; keys arrive before and after its
    // first layout.
    let note = ui.add(Flex::row().width(200.0).height(80.0));
    let dialog = ui.add_with_children(Flex::column(), &[note]);
    ui.open_layer(dialog, LayerOptions::modal());
    ui.input(&mut fonts, &typed("b"));
    frame(&mut ui, &mut fonts);
    ui.input(&mut fonts, &typed("c"));
    assert_eq!(ui.widget_mut::<TextInput>(field).unwrap().text(), "a");
    assert_ne!(
        ui.focused(),
        Some(field),
        "the focus left the field behind the dialog"
    );
}

#[test]
fn a_screen_reader_cannot_click_behind_a_dialog() {
    use crate::layers::LayerOptions;
    let mut fonts = Fonts::bundled_only();
    let mut ui = Ui::new();
    let clicks = Rc::new(Cell::new(0));
    let (opener, _) = window_with_buttons(&mut ui, &clicks);
    frame(&mut ui, &mut fonts);
    let (content, _, _) = menu(&mut ui, &Rc::new(Cell::new(0)));
    ui.open_layer(content, LayerOptions::modal());
    frame(&mut ui, &mut fonts);
    let click = ActionRequest {
        action: Action::Click,
        target_node: NodeId(opener.access_base()),
        target_tree: accesskit::TreeId::ROOT,
        data: None,
    };
    assert!(!ui.access_action(&mut fonts, &click));
    assert_eq!(clicks.get(), 0);
    let update = check_access(&mut ui, &mut fonts);
    assert!(
        !update
            .nodes
            .iter()
            .any(|(id, _)| *id == NodeId(opener.access_base())),
        "nothing behind the dialog is in the tree"
    );
}

#[test]
fn a_widget_closing_its_layer_as_it_loses_the_focus_leaves_one_focus() {
    use crate::layers::{Anchor, LayerOptions};
    /// Closes its popover when it loses the focus (as a search popover may).
    struct ClosesOnBlur;
    impl Widget for ClosesOnBlur {
        fn style(&self) -> taffy::Style {
            taffy::Style {
                size: taffy::Size {
                    width: length(80.0),
                    height: length(30.0),
                },
                ..taffy::Style::default()
            }
        }
        fn focusable(&self) -> bool {
            true
        }
        fn focus_changed(&mut self, cx: &mut Cx<'_>, focused: bool) {
            if !focused {
                cx.close_layer();
            }
        }
    }
    /// Remembers what its focus callbacks told it.
    struct Tracker(Rc<Cell<bool>>);
    impl Widget for Tracker {
        fn style(&self) -> taffy::Style {
            taffy::Style {
                size: taffy::Size {
                    width: length(80.0),
                    height: length(30.0),
                },
                ..taffy::Style::default()
            }
        }
        fn focusable(&self) -> bool {
            true
        }
        fn focus_changed(&mut self, _cx: &mut Cx<'_>, focused: bool) {
            self.0.set(focused);
        }
    }
    let mut fonts = Fonts::bundled_only();
    let mut ui = Ui::new();
    let thinks_focused = Rc::new(Cell::new(false));
    let opener = ui.add(Tracker(thinks_focused.clone()));
    let clicks = Rc::new(Cell::new(0));
    let other = ui.add(button(&clicks));
    let root = ui.add_with_children(Flex::column().fill().gap(40.0), &[opener, other]);
    ui.set_root(root);
    frame(&mut ui, &mut fonts);
    ui.set_focus(&mut fonts, Some(opener));
    let closer = ui.add(ClosesOnBlur);
    let content = ui.add_with_children(Flex::column(), &[closer]);
    let layer = ui.open_layer(content, LayerOptions::popover(Anchor::Widget(opener)));
    frame(&mut ui, &mut fonts);
    assert_eq!(ui.focused(), Some(closer));
    assert!(!thinks_focused.get());

    // Focusing another widget blurs the closer, which closes its popover;
    // the focus must not detour to the opener on the way.
    ui.set_focus(&mut fonts, Some(other));
    assert!(!ui.is_layer_open(layer));
    frame(&mut ui, &mut fonts);
    assert_eq!(ui.focused(), Some(other));
    assert!(
        !thinks_focused.get(),
        "the opener wasn't left thinking it has the focus"
    );
}

#[test]
fn a_submenu_opened_with_its_menu_returns_the_focus_to_the_menu() {
    use crate::layers::{Anchor, LayerOptions, Placement};
    let mut fonts = Fonts::bundled_only();
    let mut ui = Ui::new();
    let clicks = Rc::new(Cell::new(0));
    let (opener, _) = window_with_buttons(&mut ui, &clicks);
    frame(&mut ui, &mut fonts);
    ui.set_focus(&mut fonts, Some(opener));
    let (menu_box, menu_first, _) = menu(&mut ui, &clicks);
    ui.open_layer(menu_box, LayerOptions::popover(Anchor::Widget(opener)));
    let (sub_box, sub_first, _) = menu(&mut ui, &clicks);
    let sub = ui.open_layer(
        sub_box,
        LayerOptions::popover(Anchor::Widget(menu_first)).placement(Placement::Right),
    );
    frame(&mut ui, &mut fonts);
    assert_eq!(ui.focused(), Some(sub_first));
    ui.close_layer(&mut fonts, sub);
    assert_eq!(ui.focused(), Some(menu_first));
}

#[test]
fn layers_stay_inside_the_window() {
    use crate::layers::{EDGE, LayerOptions};
    use crate::widgets::Label;
    let mut fonts = Fonts::bundled_only();
    let mut ui = Ui::new();
    let root = ui.add(Flex::column().fill());
    ui.set_root(root);
    let long =
        "A dialog whose text is much longer than the window is wide, so it has to wrap ".repeat(3);
    let text = ui.add(Label::new(long, TextStyle::new(14.0)));
    let dialog = ui.add_with_children(Flex::column().padding(16.0, 16.0), &[text]);
    ui.open_layer(dialog, LayerOptions::modal());
    frame(&mut ui, &mut fonts);
    let bounds = ui.bounds(dialog).unwrap();
    assert!(
        bounds.x0 >= EDGE && bounds.x1 <= AREA.x1 - EDGE + 0.5,
        "{bounds:?}"
    );
    assert!(ui.bounds(text).unwrap().height() > 20.0, "its text wrapped");
}

#[test]
fn a_tooltip_neither_shields_nor_closes_a_popover() {
    use crate::layers::{Anchor, LayerOptions};
    let mut fonts = Fonts::bundled_only();
    let mut ui = Ui::new();
    let clicks = Rc::new(Cell::new(0));
    let (opener, below) = window_with_buttons(&mut ui, &clicks);
    frame(&mut ui, &mut fonts);
    let (content, _, _) = menu(&mut ui, &clicks);
    let popover = ui.open_layer(content, LayerOptions::popover(Anchor::Widget(opener)));
    let tip = ui.add(Flex::row().width(60.0).height(30.0));
    ui.open_layer(tip, LayerOptions::tooltip(Anchor::Widget(below)));
    frame(&mut ui, &mut fonts);
    let on_tip = ui.bounds(tip).unwrap().center();
    assert!(!ui.bounds(content).unwrap().contains(on_tip));
    ui.input(&mut fonts, &pointer(on_tip, true, true));
    ui.input(&mut fonts, &pointer(on_tip, true, false));
    assert!(!ui.is_layer_open(popover));
    assert_eq!(clicks.get(), 0, "used up by closing");
}

#[test]
fn a_dialog_ends_a_drag_below_it() {
    use crate::layers::LayerOptions;
    let mut fonts = Fonts::bundled_only();
    let mut ui = Ui::new();
    let clicks = Rc::new(Cell::new(0));
    let (opener, _) = window_with_buttons(&mut ui, &clicks);
    frame(&mut ui, &mut fonts);
    let at = ui.bounds(opener).unwrap().center();
    ui.input(&mut fonts, &pointer(at, true, true));
    let (content, _, _) = menu(&mut ui, &Rc::new(Cell::new(0)));
    ui.open_layer(content, LayerOptions::modal());
    frame(&mut ui, &mut fonts);
    ui.input(&mut fonts, &pointer(at, true, false));
    assert_eq!(clicks.get(), 0, "the release went nowhere");
}

#[test]
fn a_layer_removed_from_outside_gives_the_focus_back() {
    use crate::layers::{Anchor, LayerOptions};
    let mut fonts = Fonts::bundled_only();
    let mut ui = Ui::new();
    let clicks = Rc::new(Cell::new(0));
    let (opener, _) = window_with_buttons(&mut ui, &clicks);
    frame(&mut ui, &mut fonts);
    ui.set_focus(&mut fonts, Some(opener));
    let (content, _, _) = menu(&mut ui, &clicks);
    let layer = ui.open_layer(content, LayerOptions::popover(Anchor::Widget(opener)));
    frame(&mut ui, &mut fonts);
    ui.remove(content);
    frame(&mut ui, &mut fonts);
    assert!(!ui.is_layer_open(layer));
    assert_eq!(ui.focused(), Some(opener));
}

#[test]
fn a_click_focuses_without_a_ring_and_a_key_brings_it() {
    /// Records `is_focus_visible` each time it paints.
    struct Ring(Rc<Cell<Option<bool>>>);
    impl Widget for Ring {
        fn style(&self) -> taffy::Style {
            taffy::Style {
                size: taffy::Size {
                    width: length(80.0),
                    height: length(30.0),
                },
                ..taffy::Style::default()
            }
        }
        fn focusable(&self) -> bool {
            true
        }
        fn paint(&mut self, cx: &mut Cx<'_>, _scene: &mut imaginarium_render::Scene) {
            self.0.set(Some(cx.is_focus_visible()));
        }
    }
    let mut fonts = Fonts::bundled_only();
    let mut ui = Ui::new();
    let ring = Rc::new(Cell::new(None));
    let widget = ui.add(Ring(ring.clone()));
    let root = ui.add_with_children(Flex::column().fill(), &[widget]);
    ui.set_root(root);
    frame(&mut ui, &mut fonts);
    let at = ui.bounds(widget).unwrap().center();
    ui.input(&mut fonts, &pointer(at, true, true));
    ui.input(&mut fonts, &pointer(at, true, false));
    frame(&mut ui, &mut fonts);
    assert_eq!(ui.focused(), Some(widget));
    assert_eq!(ring.get(), Some(false), "a click shows no ring");
    // Ctrl alone, or a Ctrl shortcut, isn't navigation (Ctrl+wheel zooms).
    ui.input(&mut fonts, &key(Key::Other("Control".into())));
    let ctrl = Modifiers {
        ctrl: true,
        ..Modifiers::default()
    };
    ui.input(
        &mut fonts,
        &Input::Key {
            key: Key::Character("z".into()),
            text: None,
            pressed: true,
            repeat: false,
            modifiers: ctrl,
        },
    );
    frame(&mut ui, &mut fonts);
    assert_eq!(
        ring.get(),
        Some(false),
        "modifiers and shortcuts show no ring"
    );
    ui.input(&mut fonts, &key(Key::ArrowDown));
    frame(&mut ui, &mut fonts);
    assert_eq!(ring.get(), Some(true), "the keyboard does");
}

#[test]
fn a_dialog_starts_on_the_widget_it_names() {
    use crate::layers::LayerOptions;
    let mut fonts = Fonts::bundled_only();
    let mut ui = Ui::new();
    let clicks = Rc::new(Cell::new(0));
    window_with_buttons(&mut ui, &clicks);
    frame(&mut ui, &mut fonts);
    let (content, _, second) = menu(&mut ui, &clicks);
    ui.open_layer(content, LayerOptions::modal().focus(second));
    frame(&mut ui, &mut fonts);
    assert_eq!(ui.focused(), Some(second));
}

#[test]
fn a_label_wraps_down_to_its_longest_word() {
    use crate::widgets::Label;
    let mut fonts = Fonts::bundled_only();
    let mut ui = Ui::new();
    let text = ui.add(Label::new(
        "several short words in a row",
        TextStyle::new(14.0),
    ));
    // A row too narrow for the line: the label shrinks and wraps.
    let row = ui.add_with_children(Flex::row().width(90.0), &[text]);
    let root = ui.add_with_children(Flex::column().fill(), &[row]);
    ui.set_root(root);
    frame(&mut ui, &mut fonts);
    let bounds = ui.bounds(text).unwrap();
    assert!(bounds.width() <= 90.5, "{bounds:?}");
    assert!(bounds.height() > 30.0, "it wrapped: {bounds:?}");
}

#[test]
fn icons_parse_their_paths_and_take_their_size() {
    use crate::widgets::Icon;
    let mut fonts = Fonts::bundled_only();
    let mut ui = Ui::new();
    let icon =
        ui.add(Icon::new(&["M5 12h14", "not a path", "M12 5v14"], 20.0, Token::Text).label("Add"));
    let root = ui.add_with_children(Flex::row().fill(), &[icon]);
    ui.set_root(root);
    frame(&mut ui, &mut fonts);
    assert_eq!(ui.bounds(icon).unwrap().size(), Size::new(20.0, 20.0));
    let update = check_access(&mut ui, &mut fonts);
    assert!(
        update
            .nodes
            .iter()
            .any(|(id, _)| *id == NodeId(icon.access_base()))
    );
}

#[test]
fn a_late_screen_reader_request_cannot_reach_the_widget_in_a_reused_slot() {
    let mut fonts = Fonts::bundled_only();
    let mut ui = Ui::new();
    let clicks = Rc::new(Cell::new(0));
    let old = ui.add(button(&clicks));
    let root = ui.add_with_children(Flex::column().fill(), &[old]);
    ui.set_root(root);
    frame(&mut ui, &mut fonts);
    let stale = NodeId(old.access_base());
    ui.remove(old);
    // The new button takes the freed slot.
    let new = ui.add(button(&clicks));
    ui.set_children(root, &[new]);
    frame(&mut ui, &mut fonts);
    assert_ne!(
        stale,
        NodeId(new.access_base()),
        "same slot, other generation"
    );
    for action in [Action::Click, Action::Focus] {
        let request = ActionRequest {
            action,
            target_node: stale,
            target_tree: accesskit::TreeId::ROOT,
            data: None,
        };
        assert!(!ui.access_action(&mut fonts, &request));
    }
    assert_eq!(clicks.get(), 0);
    assert_ne!(ui.focused(), Some(new));
    let fresh = ActionRequest {
        action: Action::Click,
        target_node: NodeId(new.access_base()),
        target_tree: accesskit::TreeId::ROOT,
        data: None,
    };
    assert!(ui.access_action(&mut fonts, &fresh));
    assert_eq!(clicks.get(), 1);
}

fn wheel(at: Point, dy: f64) -> Input {
    Input::Scroll {
        delta: imaginarium_render::kurbo::Vec2::new(0.0, dy),
        position: at,
        modifiers: Modifiers::default(),
    }
}

/// A scroll container 100 px tall with ten 40 px buttons in it.
fn scroller(ui: &mut Ui, clicks: &Rc<Cell<u32>>) -> (WidgetId, Vec<WidgetId>) {
    use crate::scroll::Scroll;
    let rows: Vec<WidgetId> = (0..10)
        .map(|_| {
            let b = ui.add(button(clicks));
            ui.add_with_children(Flex::row().height(40.0), &[b])
        })
        .collect();
    let content = ui.add_with_children(Flex::column(), &rows);
    let scroll = ui.add_with_children(Scroll::new().height(100.0), &[content]);
    let root = ui.add_with_children(Flex::column().fill(), &[scroll]);
    ui.set_root(root);
    (scroll, rows)
}

#[test]
fn a_scroll_container_scrolls_and_hands_the_rest_on() {
    use crate::scroll::Scroll;
    let mut fonts = Fonts::bundled_only();
    let mut ui = Ui::new();
    let clicks = Rc::new(Cell::new(0));
    let (scroll, rows) = scroller(&mut ui, &clicks);
    frame(&mut ui, &mut fonts);
    let inside = ui.bounds(scroll).unwrap().center();
    let first = ui.bounds(rows[0]).unwrap();
    assert!(ui.input(&mut fonts, &wheel(inside, -30.0)));
    frame(&mut ui, &mut fonts);
    assert_eq!(ui.widget::<Scroll>(scroll).unwrap().offset(), 30.0);
    assert_eq!(
        ui.bounds(rows[0]).unwrap().y0,
        first.y0 - 30.0,
        "the content moved up"
    );
    // Far past the end: it stops at 400 − 100 and says so once more.
    assert!(ui.input(&mut fonts, &wheel(inside, -10_000.0)));
    frame(&mut ui, &mut fonts);
    assert_eq!(ui.widget::<Scroll>(scroll).unwrap().offset(), 300.0);
    assert!(
        !ui.input(&mut fonts, &wheel(inside, -10.0)),
        "at the end the scroll goes on"
    );
    // Rows scrolled out can't be clicked through the clip.
    let above = Point::new(inside.x, ui.bounds(scroll).unwrap().y0 - 5.0);
    assert!(ui.bounds(rows[0]).unwrap().contains(Point::new(
        ui.bounds(rows[0]).unwrap().center().x,
        ui.bounds(rows[0]).unwrap().center().y
    )));
    ui.input(&mut fonts, &pointer(above, true, true));
    ui.input(&mut fonts, &pointer(above, true, false));
    assert_eq!(clicks.get(), 0);
}

#[test]
fn the_focus_scrolls_into_view() {
    use crate::scroll::Scroll;
    let mut fonts = Fonts::bundled_only();
    let mut ui = Ui::new();
    let clicks = Rc::new(Cell::new(0));
    let (scroll, rows) = scroller(&mut ui, &clicks);
    frame(&mut ui, &mut fonts);
    let view = ui.bounds(scroll).unwrap();
    let last_button = ui.widget_children_for_tests(rows[9])[0];
    ui.set_focus(&mut fonts, Some(last_button));
    frame(&mut ui, &mut fonts);
    let shown = ui.bounds(last_button).unwrap();
    assert!(
        shown.y1 <= view.y1 + 0.5 && shown.y0 >= view.y0 - 0.5,
        "{shown:?} in {view:?}"
    );
    assert!(ui.widget::<Scroll>(scroll).unwrap().offset() > 0.0);
}

#[test]
fn the_scroll_bar_drags_and_leaves_the_rest_of_the_edge_to_the_rows() {
    use crate::scroll::Scroll;
    /// A full-width row that counts presses.
    struct Row(Rc<Cell<u32>>);
    impl Widget for Row {
        fn style(&self) -> taffy::Style {
            taffy::Style {
                size: taffy::Size {
                    width: taffy::prelude::percent(1.0),
                    height: length(40.0),
                },
                ..taffy::Style::default()
            }
        }
        fn event(&mut self, _cx: &mut Cx<'_>, input: &Input) -> bool {
            if let Input::PointerButton { pressed: true, .. } = input {
                self.0.set(self.0.get() + 1);
                return true;
            }
            false
        }
    }
    let mut fonts = Fonts::bundled_only();
    let mut ui = Ui::new();
    let clicks = Rc::new(Cell::new(0));
    let rows: Vec<WidgetId> = (0..10).map(|_| ui.add(Row(clicks.clone()))).collect();
    let content = ui.add_with_children(Flex::column(), &rows);
    let scroll = ui.add_with_children(Scroll::new().height(100.0), &[content]);
    let root = ui.add_with_children(Flex::column().fill(), &[scroll]);
    ui.set_root(root);
    frame(&mut ui, &mut fonts);
    let view = ui.bounds(scroll).unwrap();
    // Below the thumb (it is at the top), the edge belongs to the row.
    let edge = Point::new(view.x1 - 5.0, ui.bounds(rows[1]).unwrap().center().y);
    ui.input(&mut fonts, &pointer(edge, true, true));
    ui.input(&mut fonts, &pointer(edge, true, false));
    assert_eq!(clicks.get(), 1);
    // Drag the thumb to the bottom.
    let grab = Point::new(view.x1 - 5.0, view.y0 + 4.0);
    ui.input(&mut fonts, &pointer(grab, true, true));
    ui.input(
        &mut fonts,
        &Input::PointerMoved(Point::new(grab.x, view.y1 + 200.0)),
    );
    // A right-button release doesn't end a left drag.
    ui.input(
        &mut fonts,
        &pointer(Point::new(grab.x, view.y1 + 200.0), false, false),
    );
    ui.input(
        &mut fonts,
        &Input::PointerMoved(Point::new(grab.x, view.y1 + 210.0)),
    );
    ui.input(
        &mut fonts,
        &pointer(Point::new(grab.x, view.y1 + 200.0), true, false),
    );
    frame(&mut ui, &mut fonts);
    assert_eq!(ui.widget::<Scroll>(scroll).unwrap().offset(), 300.0);
    assert_eq!(clicks.get(), 1, "the drag clicked nothing");
}

#[test]
fn a_drag_ends_when_the_window_loses_the_focus() {
    use crate::scroll::Scroll;
    let mut fonts = Fonts::bundled_only();
    let mut ui = Ui::new();
    let clicks = Rc::new(Cell::new(0));
    let (scroll, _) = scroller(&mut ui, &clicks);
    frame(&mut ui, &mut fonts);
    let view = ui.bounds(scroll).unwrap();
    let grab = Point::new(view.x1 - 5.0, view.y0 + 4.0);
    ui.input(&mut fonts, &pointer(grab, true, true));
    ui.input(&mut fonts, &Input::WindowFocus(false));
    // The button came up in another window; moving over the list scrolls nothing.
    ui.input(
        &mut fonts,
        &Input::PointerMoved(Point::new(grab.x - 40.0, view.y1 - 5.0)),
    );
    frame(&mut ui, &mut fonts);
    assert_eq!(ui.widget::<Scroll>(scroll).unwrap().offset(), 0.0);
}

#[test]
fn page_keys_reach_a_scroll_after_a_click_on_plain_content() {
    use crate::scroll::Scroll;
    let mut fonts = Fonts::bundled_only();
    let mut ui = Ui::new();
    let rows: Vec<WidgetId> = (0..10).map(|_| ui.add(Flex::row().height(40.0))).collect();
    let content = ui.add_with_children(Flex::column(), &rows);
    let scroll = ui.add_with_children(Scroll::new().height(100.0), &[content]);
    let root = ui.add_with_children(Flex::column().fill(), &[scroll]);
    ui.set_root(root);
    frame(&mut ui, &mut fonts);
    let at = ui.bounds(rows[0]).unwrap().center();
    ui.input(&mut fonts, &pointer(at, true, true));
    ui.input(&mut fonts, &pointer(at, true, false));
    assert_eq!(ui.focused(), Some(scroll));
    // The page glides there over the scroll duration.
    let start = std::time::Instant::now();
    crate::animation::set_clock_for_tests(Some(start));
    ui.input(&mut fonts, &key(Key::PageDown));
    frame(&mut ui, &mut fonts);
    let offset = |ui: &Ui| ui.widget::<Scroll>(scroll).unwrap().offset();
    assert_eq!(offset(&ui), 0.0);
    let halfway = start + crate::animation::durations::SCROLL / 2;
    crate::animation::set_clock_for_tests(Some(halfway));
    // The window wakes when the frame it asked for is due.
    ui.wake(&mut fonts, halfway + crate::animation::FRAME);
    frame(&mut ui, &mut fonts);
    assert!(offset(&ui) > 45.0 && offset(&ui) < 90.0, "{}", offset(&ui));
    // A late wake still lands exactly on the target.
    let late = start + crate::animation::durations::SCROLL * 2;
    crate::animation::set_clock_for_tests(Some(late));
    assert!(ui.wake_at().is_some(), "the arrival still wants its frame");
    ui.wake(&mut fonts, late);
    frame(&mut ui, &mut fonts);
    assert_eq!(offset(&ui), 90.0);
    assert_eq!(ui.wake_at(), None, "at rest, nothing redraws");
    crate::animation::set_clock_for_tests(None);
}

#[test]
fn page_keys_jump_with_reduced_motion() {
    use crate::scroll::Scroll;
    let mut fonts = Fonts::bundled_only();
    let mut ui = Ui::new();
    let rows: Vec<WidgetId> = (0..10).map(|_| ui.add(Flex::row().height(40.0))).collect();
    let content = ui.add_with_children(Flex::column(), &rows);
    let scroll = ui.add_with_children(Scroll::new().height(100.0), &[content]);
    let root = ui.add_with_children(Flex::column().fill(), &[scroll]);
    ui.set_root(root);
    frame(&mut ui, &mut fonts);
    ui.set_focus(&mut fonts, Some(scroll));
    crate::animation::set_reduce_motion(true);
    ui.input(&mut fonts, &key(Key::PageDown));
    ui.input(&mut fonts, &key(Key::PageDown));
    crate::animation::set_reduce_motion(false);
    frame(&mut ui, &mut fonts);
    assert_eq!(ui.widget::<Scroll>(scroll).unwrap().offset(), 180.0);
    assert_eq!(ui.wake_at(), None);
}

#[test]
fn padding_at_the_end_can_be_scrolled_to() {
    use crate::scroll::Scroll;
    let mut fonts = Fonts::bundled_only();
    let mut ui = Ui::new();
    let block = ui.add(Flex::row().height(200.0));
    let content = ui.add_with_children(Flex::column(), &[block]);
    let scroll = ui.add_with_children(
        Scroll::new().height(100.0).style(|style| {
            style.padding.top = length(30.0);
            style.padding.bottom = length(30.0);
        }),
        &[content],
    );
    let root = ui.add_with_children(Flex::column().fill(), &[scroll]);
    ui.set_root(root);
    frame(&mut ui, &mut fonts);
    ui.input(&mut fonts, &key(Key::End));
    ui.set_focus(&mut fonts, Some(scroll));
    ui.input(&mut fonts, &key(Key::End));
    settle(&mut ui, &mut fonts);
    assert_eq!(ui.widget::<Scroll>(scroll).unwrap().offset(), 260.0 - 100.0);
}

#[test]
fn a_virtual_list_builds_only_the_rows_in_view() {
    use crate::scroll::{Scroll, VirtualList};
    use imaginarium_reactive::signal;
    let mut fonts = Fonts::bundled_only();
    let mut ui = Ui::new();
    let count = signal(10_000usize);
    let built = Rc::new(Cell::new(0));
    let list = ui.add_virtual_list(
        VirtualList::new(30.0, move || count.get(), {
            let built = built.clone();
            move |ui, _row| {
                built.set(built.get() + 1);
                ui.add(Flex::row().fill())
            }
        })
        .label("Results"),
        Scroll::new().height(300.0),
    );
    let root = ui.add_with_children(Flex::column().fill(), &[list]);
    ui.set_root(root);
    frame(&mut ui, &mut fonts);
    // 10 rows in view and 4 below; none above at the top.
    assert_eq!(
        ui.stats().widgets,
        4 + 14 * 2,
        "root, scroll, body, spacer, and slot + row each"
    );
    assert_eq!(built.get(), 14);
    let update = check_access(&mut ui, &mut fonts);
    let item = update
        .nodes
        .iter()
        .find(|(_, node)| node.role() == accesskit::Role::ListItem)
        .map(|(_, node)| node.size_of_set());
    assert_eq!(
        item,
        Some(Some(10_000)),
        "a screen reader hears the full length"
    );

    // Scrolled to the middle: rows around row 5,000.
    ui.scroll_to_row(list, 5_000);
    frame(&mut ui, &mut fonts);
    let offset = ui.widget::<Scroll>(list).unwrap().offset();
    assert!((offset - (5_001.0 * 30.0 - 300.0)).abs() < 0.5, "{offset}");
    assert!(ui.stats().widgets <= 4 + 18 * 2);

    // Fewer rows: the list follows the signal and the offset stays in range.
    count.set(20);
    frame(&mut ui, &mut fonts);
    frame(&mut ui, &mut fonts);
    assert!(ui.widget::<Scroll>(list).unwrap().offset() <= 20.0 * 30.0 - 300.0 + 0.5);
    assert!(ui.stats().widgets <= 4 + 20 * 2);
}

/// A list of `count` rows whose labels show `data`.
fn data_list(ui: &mut Ui, data: imaginarium_reactive::Signal<Vec<String>>) -> WidgetId {
    use crate::scroll::{Scroll, VirtualList};
    use crate::widgets::Label;
    ui.add_virtual_list(
        VirtualList::new(
            30.0,
            move || data.with(Vec::len),
            move |ui, row| {
                let text = data.with_untracked(|rows| rows[row].clone());
                ui.add(Label::new(text, TextStyle::new(13.0)))
            },
        ),
        Scroll::new().height(300.0),
    )
}

fn label_texts(ui: &mut Ui, fonts: &mut Fonts) -> Vec<String> {
    let update = check_access(ui, fonts);
    update
        .nodes
        .iter()
        .filter(|(_, node)| node.role() == accesskit::Role::Label)
        .filter_map(|(_, node)| node.value().map(str::to_owned))
        .collect()
}

#[test]
fn rows_show_new_data_even_at_the_same_count() {
    use imaginarium_reactive::signal;
    let mut fonts = Fonts::bundled_only();
    let mut ui = Ui::new();
    let data = signal((0..100).map(|i| format!("old {i}")).collect::<Vec<_>>());
    let list = data_list(&mut ui, data);
    let root = ui.add_with_children(Flex::column().fill(), &[list]);
    ui.set_root(root);
    frame(&mut ui, &mut fonts);
    assert!(label_texts(&mut ui, &mut fonts).contains(&"old 0".to_owned()));
    data.set((0..100).map(|i| format!("new {i}")).collect());
    frame(&mut ui, &mut fonts);
    let texts = label_texts(&mut ui, &mut fonts);
    assert!(texts.contains(&"new 0".to_owned()), "{texts:?}");
    assert!(!texts.iter().any(|text| text.starts_with("old")));
}

#[test]
fn a_list_in_a_layer_fills_on_first_open_and_scrolls_to_its_row() {
    use crate::layers::LayerOptions;
    use crate::scroll::Scroll;
    use imaginarium_reactive::signal;
    let mut fonts = Fonts::bundled_only();
    let mut ui = Ui::new();
    let root = ui.add(Flex::column().fill());
    ui.set_root(root);
    frame(&mut ui, &mut fonts);
    let data = signal((0..1000).map(|i| format!("row {i}")).collect::<Vec<_>>());
    let list = data_list(&mut ui, data);
    let panel = ui.add_with_children(Flex::column().width(300.0), &[list]);
    // Before its first layout.
    ui.scroll_to_row(list, 0);
    ui.open_layer(panel, LayerOptions::modal());
    frame(&mut ui, &mut fonts);
    let texts = label_texts(&mut ui, &mut fonts);
    assert!(
        texts.len() >= 10,
        "a screenful, not just the overscan: {}",
        texts.len()
    );
    assert!(texts.contains(&"row 0".to_owned()), "row 0 is in view");
    assert_eq!(ui.widget::<Scroll>(list).unwrap().offset(), 0.0);
}

#[test]
fn a_list_that_shrinks_shows_rows_in_the_same_frame() {
    use imaginarium_reactive::signal;
    let mut fonts = Fonts::bundled_only();
    let mut ui = Ui::new();
    let data = signal((0..1000).map(|i| format!("row {i}")).collect::<Vec<_>>());
    let list = data_list(&mut ui, data);
    let root = ui.add_with_children(Flex::column().fill(), &[list]);
    ui.set_root(root);
    frame(&mut ui, &mut fonts);
    ui.scroll_to_row(list, 900);
    frame(&mut ui, &mut fonts);
    data.set((0..20).map(|i| format!("row {i}")).collect());
    frame(&mut ui, &mut fonts);
    let texts = label_texts(&mut ui, &mut fonts);
    assert!(texts.len() >= 10, "{texts:?}");
}

#[test]
fn a_focused_row_scrolled_away_leaves_the_focus_with_its_list() {
    use crate::scroll::{Scroll, VirtualList};
    let mut fonts = Fonts::bundled_only();
    let mut ui = Ui::new();
    let clicks = Rc::new(Cell::new(0));
    let list = ui.add_virtual_list(
        VirtualList::new(30.0, || 1000, {
            let clicks = clicks.clone();
            move |ui, _row| ui.add(button(&clicks))
        }),
        Scroll::new().height(300.0),
    );
    let root = ui.add_with_children(Flex::column().fill(), &[list]);
    ui.set_root(root);
    frame(&mut ui, &mut fonts);
    ui.focus_next(&mut fonts, true); // the list itself
    ui.focus_next(&mut fonts, true); // its first row's button
    let row_button = ui.focused().unwrap();
    assert_ne!(row_button, list);
    ui.scroll_to_row(list, 900);
    frame(&mut ui, &mut fonts);
    assert!(ui.bounds(row_button).is_none(), "the row is gone");
    assert_eq!(ui.focused(), Some(list), "keys still reach the list");
}
