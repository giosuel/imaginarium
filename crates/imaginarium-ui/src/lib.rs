// SPDX-License-Identifier: AGPL-3.0-or-later

//! The UI toolkit (ADR-0007): windows, input and the frame pipeline, on
//! winit and `imaginarium-render`.
//!
//! # Pieces (M2.2)
//! - [`run`] opens a window and shows a [`View`] in it. Each frame is one
//!   scene, built by the view in logical pixels and drawn at the window's
//!   scale factor.
//! - **No frame without a reason:** a frame is drawn when the window
//!   appears, is resized or moves to a monitor with another scale, when
//!   [`View::input`], [`View::presented`] or the view's timer
//!   ([`View::wake_at`]) asks for one, or when [`request_frame`] was called
//!   (effects of `imaginarium-reactive` do that). Otherwise the event loop
//!   sleeps (gate criterion 6).
//! - [`Input`]: pointer, wheel and keys in logical pixels. [`FrameInfo`]
//!   and [`RunStats`]: what the gate report measures.
//!
//! # Not here yet
//! The widget tree, layout, focus, text input with IME and accessibility
//! come in M2.3 and M2.4.

mod window;

pub use window::{FrameInfo, Input, RunStats, UiError, View, WindowOptions, request_frame, run};
