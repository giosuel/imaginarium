// SPDX-License-Identifier: AGPL-3.0-or-later

//! Errors of the render backends.

use thiserror::Error;

/// What can go wrong setting up or using a backend.
#[derive(Debug, Error)]
pub enum RenderError {
    /// No GPU fits the request (or there is none).
    #[error("no suitable GPU: {0}")]
    NoAdapter(String),
    /// The GPU was found but couldn't be opened.
    #[error("couldn't open the GPU: {0}")]
    Device(String),
    /// The window's surface couldn't be created or used.
    #[error("window surface: {0}")]
    Surface(String),
    /// Vello failed to render a frame.
    #[error("rendering failed: {0}")]
    Render(String),
    /// Reading a rendered frame back from the GPU failed.
    #[error("reading the frame back failed: {0}")]
    Readback(String),
}
