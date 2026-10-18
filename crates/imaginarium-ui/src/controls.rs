// SPDX-License-Identifier: AGPL-3.0-or-later

//! Controls for choosing among a few values (the Settings mockups): a
//! segmented control, a dropdown's button, and the "Changed" badge.
//!
//! The dropdown's list is a layer the owner opens ([`Ui::open_layer`]),
//! as menus are: [`dropdown`] only says when it was asked for.

use std::cell::RefCell;
use std::rc::Rc;

use accesskit::Role;
use imaginarium_text::TextStyle;

use crate::theme::Token;
use crate::tree::{Ui, WidgetId};
use crate::widgets::{ButtonBox, ButtonColors, Flex, Icon, Label, label};

/// A row of options, one of them chosen (Dots, Grid, Plain). Each is a
/// button; the chosen one is filled with the panel's color.
///
/// ```
/// # use imaginarium_ui::{Segmented, Ui};
/// # use imaginarium_reactive::signal;
/// let mut ui = Ui::new();
/// let chosen = signal(0usize);
/// let control = Segmented::new(
///     "Board background",
///     &["Dots", "Grid", "Plain"],
///     move || chosen.get(),
///     move |at| chosen.set(at),
/// )
/// .add_to(&mut ui);
/// # let _ = control;
/// ```
pub struct Segmented {
    label: String,
    options: Vec<String>,
    chosen: Rc<dyn Fn() -> usize>,
    choose: Rc<RefCell<dyn FnMut(usize)>>,
}

impl Segmented {
    /// A control named `label` for screen readers, with `options`, showing
    /// `chosen` (it may read signals) and calling `choose` with an
    /// option's index when it is picked.
    pub fn new(
        label: impl Into<String>,
        options: &[&str],
        chosen: impl Fn() -> usize + 'static,
        choose: impl FnMut(usize) + 'static,
    ) -> Self {
        Self {
            label: label.into(),
            options: options.iter().map(|&option| option.to_owned()).collect(),
            chosen: Rc::new(chosen),
            choose: Rc::new(RefCell::new(choose)),
        }
    }

    /// Adds it to `ui`.
    pub fn add_to(self, ui: &mut Ui) -> WidgetId {
        let colors = ButtonColors {
            background: Token::ControlHover.into(),
            hover: Token::ControlActive.into(),
            pressed: Token::ControlActive.into(),
            text: Token::Text.into(),
            focus: Token::AccentInk.into(),
            border: None,
        };
        let buttons: Vec<WidgetId> = self
            .options
            .iter()
            .enumerate()
            .map(|(at, option)| {
                let text = ui.add(
                    Label::new(option.clone(), TextStyle::new(13.0).weight(500.0))
                        .color(Token::Text)
                        .single_line(),
                );
                let choose = self.choose.clone();
                let chosen = self.chosen.clone();
                ButtonBox::new(option.clone(), colors, move || {
                    (choose.borrow_mut())(at);
                })
                .height(28.0)
                .padding(12.0, 12.0)
                .radius(6.0)
                .keep_focus()
                .role(Role::RadioButton)
                .selected(move || chosen() == at)
                .selected_fill(Token::PanelBg)
                .add_to(ui, &[text])
            })
            .collect();
        ui.add_with_children(
            Flex::row()
                .gap(2.0)
                .padding(3.0, 3.0)
                .radius(8.0)
                .background(Token::ControlHover)
                .role(Role::RadioGroup, self.label),
            &buttons,
        )
    }
}

/// A dropdown's button: the chosen value (`text`, which may read signals)
/// and a chevron, `width` wide. A click (or Enter) calls `open`, which
/// opens the list of values as a layer below it.
pub fn dropdown(
    ui: &mut Ui,
    name: impl Into<String>,
    width: f64,
    text: impl Fn() -> String + 'static,
    open: impl FnMut() + 'static,
) -> WidgetId {
    let value = label(ui, TextStyle::new(13.0), Token::Text, text);
    if let Some(label) = ui.widget_mut::<Label>(value) {
        label.set_single_line(true);
    }
    let grow = ui.add_with_children(Flex::row().grow(1.0), &[value]);
    let chevron = ui.add(Icon::named("chevron-down", 16.0, Token::TextMuted));
    ButtonBox::new(name, ButtonColors::secondary(), open)
        .size(width, 32.0)
        .padding(12.0, 10.0)
        .start()
        .radius(8.0)
        .role(Role::ComboBox)
        .add_to(ui, &[grow, chevron])
}

/// A small badge: `text` on the accent's soft color ("Changed").
pub fn badge(ui: &mut Ui, text: &str) -> WidgetId {
    let label = ui.add(
        Label::new(text, TextStyle::new(11.0).weight(500.0))
            .color(Token::AccentInk)
            .single_line(),
    );
    ui.add_with_children(
        Flex::row()
            .padding(6.0, 1.0)
            .radius(4.0)
            .background(Token::AccentSoft),
        &[label],
    )
}
