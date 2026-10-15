// SPDX-License-Identifier: AGPL-3.0-or-later

//! What the gate window shows, as a widget tree: a toolbar (logo, title,
//! match count, the command button, the search input), the board, a
//! status line, and the command palette above everything (Ctrl+K).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use imaginarium_reactive::{Signal, memo, signal};
use imaginarium_render::Scene;
use imaginarium_render::kurbo::{Rect, Size};
use imaginarium_render::peniko::Color;
use imaginarium_text::{Fonts, TextStyle};
use imaginarium_ui::{
    AccessTree, Button, ButtonColors, Flex, Input, InputColors, Key, Label, TextInput, TreeStats,
    Ui, WidgetId, label,
};

use crate::board::{ACCENT, Board, INK, PAPER};
use crate::commands::Commands;
use crate::widgets::{BoardWidget, Logo, Palette, ResultList, Results};

const CHROME: Color = Color::from_rgb8(0xff, 0xff, 0xff);
const CHROME_EDGE: Color = Color::from_rgb8(0xd9, 0xd6, 0xcc);

/// The window's content.
pub(crate) struct App {
    pub(crate) fonts: Fonts,
    pub(crate) ui: Ui,
    pub(crate) board: Rc<RefCell<Board>>,
    board_id: WidgetId,
    search_id: WidgetId,
    palette_input: WidgetId,
    status_id: WidgetId,
    palette_open: Signal<bool>,
    query: Signal<String>,
    /// The last frame's paint and encode time (ms), shown in the status
    /// line. Not a signal: showing it must not ask for another frame.
    frame: Rc<Cell<(f64, f64)>>,
    /// Time spent in the last board search and palette search.
    pub(crate) last_search: Rc<Cell<Duration>>,
    pub(crate) last_palette_search: Rc<Cell<Duration>>,
    gpu: Signal<String>,
    size: Size,
}

fn input_colors() -> InputColors {
    InputColors {
        background: PAPER,
        border: CHROME_EDGE,
        accent: ACCENT,
        placeholder: INK.with_alpha(0.45),
    }
}

impl App {
    /// The content, with a board of `cards` and a palette of `commands`.
    pub(crate) fn new(mut fonts: Fonts, cards: usize, commands: usize) -> Self {
        let _ = &mut fonts;
        let mut ui = Ui::new();
        let board = Rc::new(RefCell::new(Board::new(cards)));
        let commands = Rc::new(RefCell::new(Commands::new(commands)));
        let command_count = commands.borrow().len();

        let matches: Signal<Option<usize>> = signal(None);
        let palette_open = signal(false);
        let query = signal(String::new());
        let selected = signal(0usize);
        let last_command: Signal<Option<String>> = signal(None);
        let frame = Rc::new(Cell::new((0.0, 0.0)));
        let gpu = signal(String::new());
        let last_search = Rc::new(Cell::new(Duration::ZERO));
        let last_palette_search = Rc::new(Cell::new(Duration::ZERO));

        // The palette's results: a memo over the query, so a keystroke
        // searches once, whoever reads the results.
        let results = {
            let commands = commands.clone();
            let timing = last_palette_search.clone();
            memo(move |_| {
                let query = query.get();
                let start = Instant::now();
                let rows = commands.borrow().search(&query, 50);
                let elapsed = start.elapsed();
                timing.set(elapsed);
                Results {
                    rows,
                    search_ms: elapsed.as_secs_f64() * 1000.0,
                }
            })
        };

        // The board.
        let board_id = ui.add(BoardWidget {
            board: board.clone(),
        });

        // The toolbar.
        let logo = ui.add(Logo);
        let title = ui.add(Label::new(
            "Imaginarium",
            TextStyle::new(15.0).weight(700.0).color(INK),
        ));
        let spacer = ui.add(Flex::row().grow(1.0));
        let count = label(
            &mut ui,
            TextStyle::new(13.0).color(INK.with_alpha(0.6)),
            move || match matches.get() {
                Some(count) => format!("{count} matching"),
                None => String::new(),
            },
        );
        let button_colors = ButtonColors {
            background: PAPER,
            hover: INK.with_alpha(0.08),
            pressed: INK.with_alpha(0.14),
            text: INK,
            focus: ACCENT,
        };
        let commands_button = ui.add(Button::new(
            "Commands  Ctrl+K",
            TextStyle::new(13.0),
            button_colors,
            move || palette_open.set(true),
        ));
        let search = {
            let board = board.clone();
            let timing = last_search.clone();
            let input = TextInput::new(
                "Search cards",
                "Search cards…  (Ctrl+F)",
                TextStyle::new(14.0).color(INK),
                input_colors(),
            )
            .on_change(move |text| {
                let start = Instant::now();
                let count = board.borrow_mut().search(text);
                timing.set(start.elapsed());
                matches.set(count);
                imaginarium_ui::invalidate(board_id, false);
            });
            ui.add(input)
        };
        let search_box = ui.add_with_children(Flex::row().width(320.0), &[search]);
        let toolbar = ui.add_with_children(
            Flex::row()
                .height(48.0)
                .padding(12.0, 0.0)
                .gap(10.0)
                .background(CHROME)
                .role(accesskit::Role::Toolbar, "Toolbar"),
            &[logo, title, spacer, count, commands_button, search_box],
        );
        let toolbar_edge = ui.add(Flex::row().height(1.0).background(CHROME_EDGE));

        // The status line.
        let status = {
            let frame = frame.clone();
            let board = board.clone();
            label(
                &mut ui,
                TextStyle::new(12.0).color(INK.with_alpha(0.65)),
                move || {
                    let (paint, encode) = frame.get();
                    let zoom = board.try_borrow().map_or(1.0, |board| board.zoom());
                    let mut text = format!(
                        "{} cards · {command_count} commands · zoom {:.0} % · last frame: paint {paint:.2} ms, encode {encode:.2} ms",
                        cards,
                        zoom * 100.0
                    );
                    let gpu = gpu.get();
                    if !gpu.is_empty() {
                        text.push_str(" · ");
                        text.push_str(&gpu);
                    }
                    if let Some(command) = last_command.get() {
                        text.push_str(" · ran: ");
                        text.push_str(&command);
                    }
                    text
                },
            )
        };
        // The hints never change: a label of their own, laid out once.
        let hints = ui.add(Label::new(
            "wheel/drag: pan · Ctrl+wheel: zoom · double-click: edit · Ctrl+K: commands · Tab: focus",
            TextStyle::new(12.0).color(INK.with_alpha(0.45)),
        ));
        let spring = ui.add(Flex::row().grow(1.0));
        let status_bar = ui.add_with_children(
            Flex::row()
                .height(26.0)
                .padding(12.0, 0.0)
                .gap(12.0)
                .background(CHROME)
                .border(1.0, CHROME_EDGE)
                .clip(),
            &[status, spring, hints],
        );

        // The palette.
        let run = {
            let commands = commands.clone();
            move |command: usize| {
                let mut commands = commands.borrow_mut();
                commands.ran(command, 1);
                last_command.set(Some(commands.commands[command].title.clone()));
                palette_open.set(false);
            }
        };
        let palette_input = ui.add(
            TextInput::new(
                "Command",
                "Type a command…",
                TextStyle::new(16.0).color(INK),
                input_colors(),
            )
            .passing([Key::ArrowUp, Key::ArrowDown, Key::Enter, Key::Escape])
            .on_change(move |text| {
                query.set(text.to_owned());
                selected.set(0);
            }),
        );
        let list = ui.add(ResultList::new(
            commands.clone(),
            results,
            selected,
            run.clone(),
        ));
        let footer = label(
            &mut ui,
            TextStyle::new(12.0).color(INK.with_alpha(0.5)),
            move || {
                let results = results.get();
                format!(
                    "{} of {command_count} commands · search {:.3} ms · ↑↓ choose · Enter run · Esc close",
                    results.rows.len(),
                    results.search_ms
                )
            },
        );
        let footer_row = ui.add_with_children(Flex::row().padding(4.0, 2.0), &[footer]);
        let palette = ui.add_with_children(
            Palette {
                open: palette_open,
                selected,
                results,
                run: Box::new(run),
            },
            &[palette_input, list, footer_row],
        );
        ui.watch(palette, true, move || {
            palette_open.get();
        });
        ui.watch(list, true, move || {
            results.get();
            selected.get();
        });

        let root = ui.add_with_children(
            Flex::column().fill().background(PAPER),
            &[toolbar, toolbar_edge, board_id, status_bar, palette],
        );
        ui.set_root(root);

        Self {
            fonts,
            ui,
            board,
            board_id,
            search_id: search,
            status_id: status,
            palette_input,
            palette_open,
            query,
            frame,
            last_search,
            last_palette_search,
            gpu,
            size: Size::ZERO,
        }
    }

    /// The GPU's name, for the status line.
    pub(crate) fn set_gpu(&mut self, gpu: String) {
        self.gpu.set(gpu);
    }

    /// The last frame's timings, shown in the next frame's status line.
    /// Doesn't ask for a frame by itself.
    pub(crate) fn set_frame(&mut self, paint: Duration, encode: Duration) {
        self.frame
            .set((paint.as_secs_f64() * 1000.0, encode.as_secs_f64() * 1000.0));
    }

    /// Pans the board (scripted measurement) and marks it for painting.
    pub(crate) fn pan_board(&mut self, delta: imaginarium_render::kurbo::Vec2) {
        self.board.borrow_mut().pan(delta);
        self.ui.refresh(self.board_id);
    }

    /// What the tree did in the last frame.
    pub(crate) fn tree_stats(&self) -> TreeStats {
        self.ui.stats()
    }

    fn area(&self) -> Rect {
        Rect::from_origin_size((0.0, 0.0), self.size)
    }

    /// Types `text` into the search field (scripted measurement).
    pub(crate) fn type_text(&mut self, text: &str) {
        self.focus_search();
        for c in text.chars() {
            let s = c.to_string();
            self.ui.input(
                &mut self.fonts,
                &Input::Key {
                    key: Key::Character(s.clone()),
                    text: Some(s),
                    pressed: true,
                    repeat: false,
                    modifiers: Default::default(),
                },
            );
        }
    }

    /// Types `text` into the open palette (scripted measurement).
    pub(crate) fn type_command(&mut self, text: &str) {
        if !self.palette_open.get_untracked() {
            self.open_palette();
        }
        for c in text.chars() {
            let s = c.to_string();
            self.ui.input(
                &mut self.fonts,
                &Input::Key {
                    key: Key::Character(s.clone()),
                    text: Some(s),
                    pressed: true,
                    repeat: false,
                    modifiers: Default::default(),
                },
            );
        }
    }

    /// Empties and leaves the search field.
    pub(crate) fn clear_search(&mut self) {
        if let Some(input) = self.ui.widget_mut::<TextInput>(self.search_id) {
            input.set_text("");
        }
        self.board.borrow_mut().search("");
        self.ui.set_focus(&mut self.fonts, None);
        imaginarium_ui::invalidate(self.board_id, false);
    }

    /// Closes the palette and forgets its query.
    pub(crate) fn close_palette(&mut self) {
        self.palette_open.set(false);
        if let Some(input) = self.ui.widget_mut::<TextInput>(self.palette_input) {
            input.set_text("");
        }
        self.query.set(String::new());
        self.ui.set_focus(&mut self.fonts, None);
    }

    fn focus_search(&mut self) {
        if self.ui.focused() != Some(self.search_id) {
            self.ui.set_focus(&mut self.fonts, Some(self.search_id));
        }
    }

    fn open_palette(&mut self) {
        if let Some(input) = self.ui.widget_mut::<TextInput>(self.palette_input) {
            input.set_text("");
        }
        self.query.set(String::new());
        self.palette_open.set(true);
        self.ui.set_focus(&mut self.fonts, Some(self.palette_input));
    }

    pub(crate) fn paint(&mut self, scene: &mut Scene, size: Size) {
        self.size = size;
        let area = self.area();
        // The status line shows this frame's predecessor's timings and the
        // zoom: refreshed with every frame, never a reason for one.
        self.ui.refresh(self.status_id);
        self.ui.layout(&mut self.fonts, area);
        self.ui.paint(&mut self.fonts, scene);
    }

    pub(crate) fn input(&mut self, input: &Input) -> bool {
        let was_open = self.palette_open.get_untracked();
        // App-wide shortcuts.
        if let Input::Key {
            key: Key::Character(c),
            pressed: true,
            modifiers,
            ..
        } = input
            && modifiers.command()
        {
            if c.eq_ignore_ascii_case("k") {
                if was_open {
                    self.close_palette();
                } else {
                    self.open_palette();
                }
                return true;
            }
            if c.eq_ignore_ascii_case("f") && !was_open {
                self.focus_search();
                if let Some(input) = self.ui.widget_mut::<TextInput>(self.search_id) {
                    input
                        .field()
                        .editor()
                        .edit(&mut self.fonts, imaginarium_text::Edit::SelectAll);
                }
                return true;
            }
        }
        // While the palette is open, a press outside it closes it.
        let handled = self.ui.input(&mut self.fonts, input);
        let open = self.palette_open.get_untracked();
        if !was_open && open {
            // Opened by the button.
            self.open_palette();
        } else if was_open && !open {
            self.close_palette();
        }
        handled || was_open != open
    }

    pub(crate) fn ime_area(&mut self) -> Option<Rect> {
        self.ui.ime_area(&mut self.fonts)
    }

    pub(crate) fn wake_at(&self) -> Option<Instant> {
        self.ui.wake_at()
    }

    pub(crate) fn wake(&mut self, now: Instant) {
        self.ui.wake(&mut self.fonts, now);
    }

    pub(crate) fn access(&mut self, tree: &mut AccessTree) {
        self.ui.access(&mut self.fonts, tree);
    }

    pub(crate) fn access_action(&mut self, request: &accesskit::ActionRequest) -> bool {
        self.ui.access_action(&mut self.fonts, request)
    }
}
