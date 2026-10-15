// SPDX-License-Identifier: AGPL-3.0-or-later

//! The system clipboard (text only, for now), on a thread of its own.
//!
//! Reading the clipboard can wait for another app: on X11 the app that
//! copied is asked for the text and may not answer for seconds, and on
//! Wayland the text comes through a pipe from that app. The UI thread
//! never waits for that. A paste is a request; the text arrives later as
//! [`Input::Paste`](crate::Input::Paste) for the focused widget.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, Sender};

/// Texts read from the clipboard, waiting for the window to deliver them.
static PASTED: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());

/// Wakes the window's event loop when a text was read.
type Waker = Box<dyn Fn() + Send>;
static WAKER: Mutex<Option<Waker>> = Mutex::new(None);

/// Sets what wakes the event loop when a paste is ready.
pub(crate) fn set_waker(waker: impl Fn() + Send + 'static) {
    if let Ok(mut slot) = WAKER.lock() {
        *slot = Some(Box::new(waker));
    }
}

/// The texts read since the last call, oldest first.
pub(crate) fn take_pasted() -> Vec<String> {
    PASTED
        .lock()
        .map(|mut pasted| pasted.drain(..).collect())
        .unwrap_or_default()
}

fn deliver(text: String) {
    if let Ok(mut pasted) = PASTED.lock() {
        pasted.push_back(text);
    }
    if let Ok(waker) = WAKER.lock()
        && let Some(wake) = waker.as_ref()
    {
        wake();
    }
}

/// Where text is kept: the system clipboard, or a stand-in in tests.
pub(crate) trait Backend {
    /// The clipboard's text, if it has any. May block.
    fn get(&mut self) -> Option<String>;
    /// Puts `text` on the clipboard.
    fn set(&mut self, text: &str);
}

struct System(Option<arboard::Clipboard>);

impl Backend for System {
    fn get(&mut self) -> Option<String> {
        self.0.as_mut()?.get_text().ok()
    }

    fn set(&mut self, text: &str) {
        if let Some(clipboard) = &mut self.0 {
            let _ = clipboard.set_text(text);
        }
    }
}

enum Request {
    Set(String),
    Get,
}

/// The system clipboard, opened on its thread on first use. Failures (no
/// clipboard service, a headless session) keep copy and paste working
/// inside the app.
pub struct Clipboard {
    requests: Option<Sender<Request>>,
    open: Box<dyn Fn() -> Box<dyn Backend> + Send + Sync>,
}

impl Default for Clipboard {
    fn default() -> Self {
        Self::with_backend(|| Box::new(System(arboard::Clipboard::new().ok())))
    }
}

impl std::fmt::Debug for Clipboard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Clipboard")
            .field("started", &self.requests.is_some())
            .finish()
    }
}

impl Clipboard {
    /// A clipboard on `open`'s backend (made on the clipboard's thread).
    pub(crate) fn with_backend(
        open: impl Fn() -> Box<dyn Backend> + Send + Sync + 'static,
    ) -> Self {
        Self {
            requests: None,
            open: Box::new(open),
        }
    }

    fn send(&mut self, request: Request) {
        if self.requests.is_none() {
            let (sender, receiver) = mpsc::channel();
            let open = std::mem::replace(&mut self.open, Box::new(|| Box::new(System(None))));
            let started = std::thread::Builder::new()
                .name("clipboard".into())
                .spawn(move || serve(open(), receiver));
            if started.is_ok() {
                self.requests = Some(sender);
            }
        }
        if let Some(requests) = &self.requests {
            let _ = requests.send(request);
        }
    }

    /// Puts `text` on the clipboard.
    pub fn set_text(&mut self, text: &str) {
        self.send(Request::Set(text.to_owned()));
    }

    /// Asks for the clipboard's text, which arrives as
    /// [`Input::Paste`](crate::Input::Paste) if there is any.
    pub fn request_text(&mut self) {
        self.send(Request::Get);
    }
}

/// The clipboard thread: requests in order, so a paste right after a copy
/// gets the copied text.
fn serve(mut backend: Box<dyn Backend>, requests: Receiver<Request>) {
    // Text copied while no system clipboard was available, so copy and
    // paste still work inside the app.
    let mut fallback = String::new();
    for request in requests {
        match request {
            Request::Set(text) => {
                backend.set(&text);
                fallback = text;
            }
            Request::Get => {
                let text = backend
                    .get()
                    .filter(|text| !text.is_empty())
                    .or_else(|| (!fallback.is_empty()).then(|| fallback.clone()));
                if let Some(text) = text {
                    deliver(text);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    /// A clipboard owner that takes its time to answer.
    struct Slow;

    impl Backend for Slow {
        fn get(&mut self) -> Option<String> {
            std::thread::sleep(Duration::from_millis(300));
            Some("slow".into())
        }
        fn set(&mut self, _text: &str) {}
    }

    #[test]
    fn a_slow_clipboard_does_not_block_the_caller() {
        let mut clipboard = Clipboard::with_backend(|| Box::new(Slow));
        let start = Instant::now();
        clipboard.request_text();
        assert!(start.elapsed() < Duration::from_millis(100));
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut pasted = Vec::new();
        while pasted.is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
            pasted = take_pasted();
        }
        assert_eq!(pasted, ["slow"]);
    }
}
