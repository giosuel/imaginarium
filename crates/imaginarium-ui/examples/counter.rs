// SPDX-License-Identifier: AGPL-3.0-or-later

//! The smallest app on the toolkit: a text input, a button and a label,
//! tied together with signals, in the theme's colors (a second button
//! switches between dark and light). The developer guide walks through it.
//!
//! ```sh
//! cargo run --release -p imaginarium-ui --example counter
//! cargo run --release -p imaginarium-ui --example counter -- --frames 30   # closes after 30 frames
//! ```

use std::time::Instant;

use imaginarium_reactive::signal;
use imaginarium_render::Scene;
use imaginarium_render::kurbo::{Rect, Size};
use imaginarium_text::{Fonts, TextStyle};
use imaginarium_ui::{
    AccessTree, Button, ButtonColors, DEFAULT_ACCENT, Flex, FrameInfo, Input, InputColors, Label,
    TextInput, Theme, ThemeKind, Token, Ui, UiError, View, WindowOptions, label, set_theme, theme,
};

/// The window's view: the widget tree does the work, the view passes the
/// window's events on to it.
struct Counter {
    fonts: Fonts,
    ui: Ui,
    /// `--frames N`: close after N frames.
    close_after: Option<u64>,
    frames: u64,
}

impl View for Counter {
    fn paint(&mut self, scene: &mut Scene, size: Size) {
        // Lays out only if something changed, then repaints only the
        // widgets that changed.
        self.ui
            .layout(&mut self.fonts, Rect::from_origin_size((0.0, 0.0), size));
        self.ui.paint(&mut self.fonts, scene);
    }

    fn input(&mut self, input: &Input) -> bool {
        self.ui.input(&mut self.fonts, input)
    }

    fn presented(&mut self, frame: &FrameInfo) -> bool {
        self.frames = frame.number;
        false // no frame without a reason
    }

    fn should_close(&mut self) -> bool {
        self.close_after.is_some_and(|frames| self.frames >= frames)
    }

    // The caret blinks on a timer.
    fn wake_at(&mut self) -> Option<Instant> {
        self.ui.wake_at()
    }

    fn wake(&mut self, now: Instant) -> bool {
        self.ui.wake(&mut self.fonts, now);
        false // the woken widgets ask for their frame themselves
    }

    // The input method's candidate window goes next to the caret.
    fn ime_area(&mut self) -> Option<Rect> {
        self.ui.ime_area(&mut self.fonts)
    }

    // Screen readers.
    fn accessibility(&mut self, tree: &mut AccessTree) {
        self.ui.access(&mut self.fonts, tree);
    }

    fn accessibility_action(&mut self, request: &accesskit::ActionRequest) -> bool {
        self.ui.access_action(&mut self.fonts, request)
    }
}

fn main() -> Result<(), UiError> {
    // State lives in signals, outside the widgets.
    let name = signal(String::new());
    let clicks = signal(0u32);

    let style = TextStyle::new(16.0);
    let mut ui = Ui::new();
    let input = ui.add(
        TextInput::new("Name", "Your name", style, InputColors::themed())
            .on_change(move |text| name.set(text.to_owned())),
    );
    let button = ui.add(Button::new(
        "Click me",
        style,
        ButtonColors::primary(),
        move || {
            clicks.update(|clicks| *clicks += 1);
        },
    ));
    // Every widget repaints in the other theme's colors.
    let switch = ui.add(Button::new(
        "Switch theme",
        style,
        ButtonColors::quiet(),
        || {
            set_theme(match theme().kind() {
                ThemeKind::Dark => Theme::light(DEFAULT_ACCENT),
                ThemeKind::Light => Theme::dark(DEFAULT_ACCENT),
            });
        },
    ));
    // A label computed from signals: it lays out again when they change,
    // and nothing else does.
    let greeting = label(&mut ui, style, move || {
        let name = name.get();
        let who = if name.is_empty() { "there" } else { &name };
        format!("Hello, {who}! You clicked {} times.", clicks.get())
    });
    if let Some(greeting) = ui.widget_mut::<Label>(greeting) {
        greeting.set_color(Token::Text);
    }
    let buttons = ui.add_with_children(Flex::row().gap(8.0), &[button, switch]);
    let column = ui.add_with_children(
        Flex::column()
            .gap(12.0)
            .padding(24.0, 24.0)
            .fill()
            .background(Token::PanelBg),
        &[input, buttons, greeting],
    );
    ui.set_root(column);

    let close_after = std::env::args()
        .skip_while(|arg| arg != "--frames")
        .nth(1)
        .and_then(|frames| frames.parse().ok());
    let options = WindowOptions {
        title: "Counter".into(),
        size: Size::new(480.0, 200.0),
        ..WindowOptions::default()
    };
    let view = Counter {
        fonts: Fonts::new(),
        ui,
        close_after,
        frames: 0,
    };
    imaginarium_ui::run(options, view)?;
    Ok(())
}
