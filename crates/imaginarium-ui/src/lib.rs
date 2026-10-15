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
//! - Accessibility: views describe themselves in an [`AccessTree`]; the
//!   widget tree does it for its widgets ([`Widget::accessibility`]).
//!
//! # Not here yet
//! Scrolling containers, menus, drag and drop, and animation.

mod access;
mod clipboard;
mod text_field;
mod tree;
mod widgets;
mod window;

pub use clipboard::Clipboard;
pub use tree::{Cx, TreeStats, Ui, Widget, WidgetId, invalidate};
pub use widgets::{Button, ButtonColors, Flex, InputColors, Label, TextInput, label};

pub use access::{AccessTree, ROOT, rect as access_rect};
pub use text_field::{FieldResponse, TextField};

pub use window::{
    FrameInfo, Ime, Input, Key, Modifiers, RunStats, UiError, View, WindowOptions, request_frame,
    run,
};
