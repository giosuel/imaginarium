// SPDX-License-Identifier: AGPL-3.0-or-later

//! The UI toolkit (ADR-0007): windows, input, the frame pipeline, the
//! widget tree, text input and accessibility, on winit, Taffy, AccessKit
//! and `imaginarium-render`.
//!
//! # Pieces
//! - [`run`] opens a window and shows a [`View`] in it. Each frame is one
//!   scene, built by the view in logical pixels and drawn at the window's
//!   scale factor.
//! - **No frame without a reason:** a frame is drawn when the window
//!   appears, is resized or moves to a monitor with another scale, when
//!   [`View::input`], [`View::presented`] or the view's timer
//!   ([`View::wake_at`]) asks for one, or when [`request_frame`] was called
//!   (effects of `imaginarium-reactive` do that). Otherwise the event loop
//!   sleeps (gate criterion 6).
//! - [`Input`]: pointer, wheel, keys ([`Key`], [`Modifiers`]) and the input
//!   method ([`Ime`]) in logical pixels. [`FrameInfo`] and [`RunStats`]:
//!   what the gate report measures.
//! - [`Ui`]: the retained widget tree. [`Widget`]s live in an arena, are
//!   laid out by Taffy (flexbox), and keep their drawing in a scene of
//!   their own until they are invalidated, so a frame repaints only what
//!   changed ([`TreeStats`]). The tree routes pointer input by hit testing
//!   (with hover and pointer capture), keys to the focused widget, and
//!   bubbles unhandled input to the parents; Tab moves the focus.
//! - **Driven by signals:** [`Ui::watch`] ties a widget to what a closure
//!   reads; when those signals change, an effect [`invalidate`]s only that
//!   widget. [`label`] is a label computed from signals.
//! - Widgets: [`Flex`] (rows and columns with background, border, radius,
//!   shadow), [`Label`], [`Button`], [`TextInput`]; [`TextField`] is the
//!   editing core they and canvas items use. [`Clipboard`]: the system
//!   clipboard.
//! - **Layers** above the root: popovers and menus (closed by `Esc` or a
//!   click outside), modal dialogs (all input, Tab kept inside) and
//!   tooltips (no input), placed next to an anchor and kept in the
//!   window ([`Ui::open_layer`], [`LayerOptions`]); the focus returns to
//!   where it was when one closes.
//! - **Scrolling:** [`Scroll`] scrolls its content (wheel, touchpad, keys,
//!   a scroll bar, the focus kept in view); [`VirtualList`] has widgets
//!   only for the rows in view ([`Ui::add_virtual_list`]).
//! - **Animation** ([`animation`]): short transitions with easing for
//!   hover and press colors, layers fading in and keyboard scrolling, all
//!   instant with reduced motion; nothing redraws once they arrive.
//! - Accessibility: views describe themselves in an [`AccessTree`]; the
//!   widget tree does it for its widgets ([`Widget::accessibility`]).
//! - **Themes:** the interface's colors are [`Token`]s of a [`Theme`]
//!   (dark first, `docs/design/shell.md` §7). Widgets take a [`Paint`], a
//!   token or a fixed color, and resolve it when they paint;
//!   [`set_theme`] repaints the whole tree, and [`theme`] is a dependency
//!   inside effects and memos.
//!
//! # Not here yet
//! Drag and drop.

mod access;
pub mod animation;
mod clipboard;
pub mod icons;
mod layers;
mod scroll;
#[cfg(test)]
mod tests;
mod text_field;
mod theme;
mod tree;
mod widgets;
mod window;

pub use clipboard::Clipboard;
pub use layers::{Align, Anchor, LayerId, LayerKind, LayerOptions, Placement};
pub use scroll::{RowBuilder, Scroll, VirtualList};
pub use tree::{ACCESS_LOCAL_IDS, Cx, TOOLTIP_DELAY, TreeStats, Ui, Widget, WidgetId, invalidate};
pub use widgets::{
    Button, ButtonBox, ButtonColors, Flex, Icon, IconButton, InputColors, Label, Slider, Swatch,
    SwatchShape, Switch, TextInput, label,
};

pub use access::{AccessTree, ROOT, rect as access_rect};
pub use text_field::{FieldResponse, TextField};
pub use theme::{DEFAULT_ACCENT, Paint, Theme, ThemeKind, Token, contrast, mix, set_theme, theme};

pub use window::{
    FrameCauses, FrameInfo, Ime, Input, Key, Modifiers, MonitorInfo, RunStats, UiError, View,
    WindowOptions, request_frame, run, run_with_gpu, scale_factor,
};
