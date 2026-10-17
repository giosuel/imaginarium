// SPDX-License-Identifier: AGPL-3.0-or-later

//! The start window (`shell.md` §4, the Start mockups): the app's name and
//! version, New board and Open… on the left, recent boards on the right.
//!
//! Open…, the recent boards and crash recovery come with files (M3.6);
//! until then Open… is disabled and the recent list says so. The "Start
//! from" templates come with M4's shape libraries, the Settings, Shortcuts
//! and What's new links with the windows they open.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use accesskit::Role;
use imaginarium_commands::{Keymap, Platform};
use imaginarium_reactive::signal;
use imaginarium_render::Scene;
use imaginarium_render::kurbo::{Rect, Size};
use imaginarium_text::{Fonts, TextStyle};
use imaginarium_ui::{
    AccessTree, ButtonBox, ButtonColors, Flex, FrameInfo, Icon, Input, Label, Logo, Placement,
    Switch, Token, Ui, View, WidgetId, WindowOptions, WindowRequest, theme,
};
use taffy::prelude::length;

use crate::app::{App, SHOW_START_WINDOW};
use crate::toolbar::shortcut;

/// What the start window's controls asked for.
#[derive(Clone, Copy, Debug, PartialEq)]
enum StartAsk {
    NewBoard,
}

type StartAsks = Rc<RefCell<Vec<StartAsk>>>;

/// The start window. Run it in the app's event loop with
/// [`StartWindow::options`]; New board opens a board window and closes it.
pub struct StartWindow {
    app: Rc<App>,
    fonts: Fonts,
    ui: Ui,
    asks: StartAsks,
    keymap: Keymap,
    new_board: WidgetId,
    requests: Vec<WindowRequest>,
    /// It asked to close (after New board).
    closing: bool,
    close_after: Option<u64>,
    frames: u64,
}

impl StartWindow {
    /// The start window of `app`.
    pub fn new(app: Rc<App>) -> Self {
        let fonts = app.fonts();
        let mut ui = Ui::new();
        let asks: StartAsks = Rc::default();
        let mut keymap = Keymap::new(Platform::current());
        // A fixed, valid binding: this can't fail.
        let _ = keymap.bind(
            "Mod+N",
            imaginarium_commands::CommandId::new("file.new_board"),
        );
        let (side, new_board) = side(&mut ui, &app, &keymap, &asks);
        let divider = ui.add(Flex::column().width(1.0).background(Token::Divider));
        let recent = recent(&mut ui);
        let root = ui.add_with_children(
            Flex::row()
                .fill()
                .background(Token::BoardBg)
                .style(|style| style.align_items = taffy::AlignItems::STRETCH),
            &[side, divider, recent],
        );
        ui.set_root(root);
        // Nothing has the keys at first, so no focus ring shows (the
        // mockup): Ctrl+N works anywhere, Tab reaches New board first.
        Self {
            app,
            fonts,
            ui,
            asks,
            keymap,
            new_board,
            requests: Vec::new(),
            closing: false,
            close_after: None,
            frames: 0,
        }
    }

    /// Closes the window after `frames` frames (for scripted runs).
    pub fn close_after(mut self, frames: u64) -> Self {
        self.close_after = Some(frames);
        self
    }

    /// The window's options: titled `Imaginarium`, 960 × 600 and not
    /// resizable, so tiling compositors float it like Blender's splash.
    pub fn options(&self) -> WindowOptions {
        WindowOptions {
            title: "Imaginarium".into(),
            size: Size::new(960.0, 600.0),
            background: theme().color(Token::BoardBg),
            app_id: Some("ch.giosuel.Imaginarium".into()),
            resizable: false,
            ..WindowOptions::default()
        }
    }

    /// The window's widgets, for tests.
    pub fn ui(&mut self) -> (&mut Ui, &mut Fonts) {
        (&mut self.ui, &mut self.fonts)
    }

    /// Opens a board window on a new board and closes this one (once: a
    /// second press before it closes does nothing).
    pub fn new_board(&mut self) {
        if self.closing {
            return;
        }
        if let Ok(window) = self.app.new_board() {
            self.requests
                .push(WindowRequest::Open(window.options(), Box::new(window)));
            self.requests.push(WindowRequest::Close);
            self.closing = true;
        }
    }

    /// The New board button, for tests.
    pub fn new_board_button(&self) -> WidgetId {
        self.new_board
    }

    fn handle_asks(&mut self) -> bool {
        let asks = std::mem::take(&mut *self.asks.borrow_mut());
        let asked = !asks.is_empty();
        for ask in asks {
            match ask {
                StartAsk::NewBoard => self.new_board(),
            }
        }
        asked
    }
}

/// Padding on each side: left, right, top, bottom.
fn padding(left: f32, right: f32, top: f32, bottom: f32) -> taffy::Rect<taffy::LengthPercentage> {
    taffy::Rect {
        left: length(left),
        right: length(right),
        top: length(top),
        bottom: length(bottom),
    }
}

/// The left side: the name, New board and Open…, Show at startup.
fn side(ui: &mut Ui, app: &App, keymap: &Keymap, asks: &StartAsks) -> (WidgetId, WidgetId) {
    let logo = ui.add(Logo::new(44.0));
    let name = ui.add(
        Label::new("Imaginarium", TextStyle::new(20.0).weight(700.0))
            .color(Token::Text)
            .single_line(),
    );
    let version = ui.add(
        Label::new(
            format!("Version {} · preview", app.version()),
            TextStyle::new(12.0),
        )
        .color(Token::TextSubtle)
        .single_line(),
    );
    let titles = ui.add_with_children(Flex::column().gap(2.0), &[name, version]);
    let brand = ui.add_with_children(Flex::row().gap(12.0).align_center(), &[logo, titles]);

    let keys = shortcut(keymap, "file.new_board");
    let asks_new = asks.clone();
    let new_board = big_button(
        ui,
        "plus",
        "New board",
        keys,
        ButtonColors::primary(),
        Some(Box::new(move || {
            asks_new.borrow_mut().push(StartAsk::NewBoard);
        })),
    );
    let open = big_button(
        ui,
        "folder-open",
        "Open…",
        Some(primary_key("O")),
        ButtonColors::secondary(),
        None,
    );
    let buttons = ui.add_with_children(Flex::column().gap(6.0), &[new_board, open]);

    let settings = app.settings().clone();
    let shown = signal(settings.borrow().get(&SHOW_START_WINDOW).unwrap_or(true));
    let switch = Switch::new(
        "Show at startup",
        move || shown.get(),
        move || {
            let on = !shown.get();
            shown.set(on);
            // A registered bool: this can't fail.
            let _ = settings.borrow_mut().set(&SHOW_START_WINDOW, on);
        },
    )
    .add_to(ui);
    let switch_label = ui.add(
        Label::new("Show at startup", TextStyle::new(13.0))
            .color(Token::TextSecondary)
            .single_line(),
    );
    let startup = ui.add_with_children(
        Flex::row().gap(8.0).align_center().style(|style| {
            style.margin.top = taffy::LengthPercentageAuto::auto();
        }),
        &[switch, switch_label],
    );

    let side = ui.add_with_children(
        Flex::column()
            .width(287.0)
            .gap(22.0)
            .background(Token::PanelBg)
            .style(|style| {
                style.padding = padding(28.0, 24.0, 28.0, 20.0);
                style.flex_shrink = 0.0;
            }),
        &[brand, buttons, startup],
    );
    (side, new_board)
}

/// `Ctrl+<key>`, or `⌘<key>` on macOS, as the menus show shortcuts.
fn primary_key(key: &str) -> String {
    match Platform::current() {
        Platform::MacOs => format!("⌘{key}"),
        _ => format!("Ctrl+{key}"),
    }
}

/// A 40 px button with an icon, a title and its shortcut. Without an
/// action it is disabled (Open… until files come).
fn big_button(
    ui: &mut Ui,
    icon: &str,
    title: &str,
    keys: Option<String>,
    colors: ButtonColors,
    action: Option<Box<dyn FnMut()>>,
) -> WidgetId {
    let primary = action.is_some() && colors.border.is_none();
    let (ink, icon_ink, keys_ink) = match (&action, primary) {
        (None, _) => (Token::TextSubtle, Token::TextSubtle, Token::TextSubtle),
        (Some(_), true) => (Token::OnAccent, Token::OnAccent, Token::OnAccent),
        (Some(_), false) => (Token::Text, Token::TextMuted, Token::TextSubtle),
    };
    let icon = ui.add(Icon::named(icon, 17.0, icon_ink));
    let weight = if primary { 600.0 } else { 500.0 };
    let label = ui.add(
        Label::new(title, TextStyle::new(14.0).weight(weight))
            .color(ink)
            .single_line()
            .grow(1.0),
    );
    let mut children = vec![icon, label];
    if let Some(keys) = &keys {
        children.push(
            ui.add(
                Label::new(keys.clone(), TextStyle::new(11.0).weight(500.0))
                    .color(keys_ink)
                    .single_line(),
            ),
        );
    }
    let enabled = action.is_some();
    let mut action = action;
    let name = if enabled {
        title.to_owned()
    } else {
        format!("{title}, unavailable: opening files comes in a later preview")
    };
    let mut button = ButtonBox::new(name, colors, move || {
        if let Some(action) = &mut action {
            action();
        }
    })
    .height(40.0)
    .padding(12.0, 12.0)
    .gap(10.0)
    .start()
    .radius(10.0)
    .disabled(!enabled);
    if !enabled {
        button = button.tooltip("Opening files comes in a later preview", Placement::Right);
    }
    button.add_to(ui, &children)
}

/// The right side: the recent boards. None yet: files come with M3.6.
fn recent(ui: &mut Ui) -> WidgetId {
    let title = ui.add(
        Label::new("Recent", TextStyle::new(15.0).weight(600.0))
            .color(Token::Text)
            .single_line(),
    );
    let icon = ui.add(Icon::named("folder-open", 20.0, Token::TextMuted));
    let empty = ui.add(
        Label::new("The boards you open show here.", TextStyle::new(13.0))
            .color(Token::TextMuted)
            .single_line(),
    );
    let later = ui.add(
        Label::new(
            "Saving and opening files come in a later preview.",
            TextStyle::new(12.0),
        )
        .color(Token::TextSubtle)
        .single_line(),
    );
    let tile = ui.add_with_children(
        Flex::column()
            .height(112.0)
            .gap(8.0)
            .align_center()
            .border(1.0, Token::ControlBorder)
            .radius(10.0)
            .style(|style| {
                style.justify_content = taffy::JustifyContent::CENTER;
            })
            .role(Role::Group, "Recent boards"),
        &[icon, empty, later],
    );
    ui.add_with_children(
        Flex::column().gap(16.0).grow(1.0).style(|style| {
            style.padding = padding(28.0, 28.0, 24.0, 20.0);
            style.min_size.width = length(0.0);
        }),
        &[title, tile],
    )
}

impl View for StartWindow {
    fn paint(&mut self, scene: &mut Scene, size: Size) {
        self.handle_asks();
        self.ui
            .layout(&mut self.fonts, Rect::from_origin_size((0.0, 0.0), size));
        self.ui.paint(&mut self.fonts, scene);
    }

    fn input(&mut self, input: &Input) -> bool {
        // Ctrl+N wherever the keys are.
        if let Input::Key {
            key,
            pressed: true,
            repeat: false,
            modifiers,
            ..
        } = input
            && let Some(press) = imaginarium_canvas::keys::key_press(key, *modifiers)
            && self
                .keymap
                .command_for_press(press)
                .is_some_and(|command| command.as_str() == "file.new_board")
        {
            self.ui.note_keyboard();
            self.new_board();
            return true;
        }
        let handled = self.ui.input(&mut self.fonts, input);
        self.handle_asks() || handled
    }

    fn presented(&mut self, frame: &FrameInfo) -> bool {
        self.frames = frame.number;
        false
    }

    fn should_close(&mut self) -> bool {
        self.close_after.is_some_and(|frames| self.frames >= frames)
    }

    fn wake_at(&mut self) -> Option<Instant> {
        self.ui.wake_at()
    }

    fn wake(&mut self, now: Instant) -> bool {
        self.ui.wake(&mut self.fonts, now);
        false
    }

    fn ime_area(&mut self) -> Option<Rect> {
        self.ui.ime_area(&mut self.fonts)
    }

    fn accessibility(&mut self, tree: &mut AccessTree) {
        self.ui.access(&mut self.fonts, tree);
    }

    fn accessibility_action(&mut self, request: &accesskit::ActionRequest) -> bool {
        let handled = self.ui.access_action(&mut self.fonts, request);
        self.handle_asks() || handled
    }

    fn take_requests(&mut self) -> Vec<WindowRequest> {
        std::mem::take(&mut self.requests)
    }
}
