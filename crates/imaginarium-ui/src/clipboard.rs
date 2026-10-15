// SPDX-License-Identifier: AGPL-3.0-or-later

//! The system clipboard (text only, for now).

/// The system clipboard, opened on first use. Failures (no clipboard
/// service, a headless session) make copying a no-op and pasting empty.
#[derive(Default)]
pub struct Clipboard {
    inner: Option<arboard::Clipboard>,
    /// Text copied while no system clipboard was available, so copy and
    /// paste still work inside the app.
    fallback: String,
}

impl std::fmt::Debug for Clipboard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Clipboard")
            .field("open", &self.inner.is_some())
            .finish()
    }
}

impl Clipboard {
    fn open(&mut self) -> Option<&mut arboard::Clipboard> {
        if self.inner.is_none() {
            self.inner = arboard::Clipboard::new().ok();
        }
        self.inner.as_mut()
    }

    /// Puts `text` on the clipboard.
    pub fn set_text(&mut self, text: &str) {
        self.fallback = text.to_owned();
        if let Some(clipboard) = self.open() {
            let _ = clipboard.set_text(text);
        }
    }

    /// The clipboard's text, if any.
    pub fn text(&mut self) -> Option<String> {
        match self.open().map(|clipboard| clipboard.get_text()) {
            Some(Ok(text)) => Some(text),
            _ => (!self.fallback.is_empty()).then(|| self.fallback.clone()),
        }
    }
}
