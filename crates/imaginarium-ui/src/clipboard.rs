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

/// Where read texts go: the window's queue, or a test's.
type Sink = Box<dyn Fn(String) + Send>;

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
    /// Whether there is a system clipboard (a headless session has none).
    fn available(&self) -> bool {
        true
    }
    /// The clipboard's text, if it has any. May block.
    fn get(&mut self) -> Option<String>;
    /// Puts `text` on the clipboard.
    fn set(&mut self, text: &str);
}

struct System(Option<arboard::Clipboard>);

impl Backend for System {
    fn available(&self) -> bool {
        self.0.is_some()
    }

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

/// The system clipboard, opened on its thread on first use. Without one
/// (no clipboard service, a headless session) copy and paste still work
/// inside the app.
pub struct Clipboard {
    requests: Option<Sender<Request>>,
    open: Box<dyn Fn() -> Box<dyn Backend> + Send + Sync>,
    sink: Option<Sink>,
}

impl Default for Clipboard {
    fn default() -> Self {
        Self::with_backend(|| Box::new(System(arboard::Clipboard::new().ok())), deliver)
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
    /// A clipboard on `open`'s backend (made on the clipboard's thread)
    /// that hands read texts to `sink`.
    pub(crate) fn with_backend(
        open: impl Fn() -> Box<dyn Backend> + Send + Sync + 'static,
        sink: impl Fn(String) + Send + 'static,
    ) -> Self {
        Self {
            requests: None,
            open: Box::new(open),
            sink: Some(Box::new(sink)),
        }
    }

    fn send(&mut self, request: Request) {
        if self.requests.is_none()
            && let Some(sink) = self.sink.take()
        {
            let (sender, receiver) = mpsc::channel();
            let open = std::mem::replace(&mut self.open, Box::new(|| Box::new(System(None))));
            let started = std::thread::Builder::new()
                .name("clipboard".into())
                .spawn(move || serve(open(), receiver, sink));
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
fn serve(mut backend: Box<dyn Backend>, requests: Receiver<Request>, sink: Sink) {
    // Without a system clipboard, the text copied in the app, so copy and
    // paste still work inside it. (With one, a read that fails pastes
    // nothing, not an older copy.)
    let mut fallback = String::new();
    for request in requests {
        match request {
            Request::Set(text) => {
                backend.set(&text);
                fallback = text;
            }
            Request::Get => {
                let text = if backend.available() {
                    backend.get()
                } else {
                    Some(fallback.clone())
                };
                if let Some(text) = text.filter(|text| !text.is_empty()) {
                    sink(text);
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

    /// A clipboard whose owner never answers in time, or no clipboard.
    struct Failing(bool);

    impl Backend for Failing {
        fn available(&self) -> bool {
            self.0
        }
        fn get(&mut self) -> Option<String> {
            None
        }
        fn set(&mut self, _text: &str) {}
    }

    fn collector() -> (impl Fn(String) + Send + 'static, mpsc::Receiver<String>) {
        let (sender, receiver) = mpsc::channel();
        (move |text| drop(sender.send(text)), receiver)
    }

    #[test]
    fn a_slow_clipboard_does_not_block_the_caller() {
        let (sink, pasted) = collector();
        let mut clipboard = Clipboard::with_backend(|| Box::new(Slow), sink);
        let start = Instant::now();
        clipboard.request_text();
        assert!(start.elapsed() < Duration::from_millis(100));
        let text = pasted.recv_timeout(Duration::from_secs(5));
        assert_eq!(text.as_deref(), Ok("slow"));
    }

    #[test]
    fn a_failed_read_pastes_nothing_and_without_a_clipboard_the_app_still_can() {
        let (sink, pasted) = collector();
        let mut clipboard = Clipboard::with_backend(|| Box::new(Failing(true)), sink);
        clipboard.set_text("copied earlier");
        clipboard.request_text();
        assert!(pasted.recv_timeout(Duration::from_millis(300)).is_err());
        let (sink, pasted) = collector();
        let mut headless = Clipboard::with_backend(|| Box::new(Failing(false)), sink);
        headless.set_text("in the app");
        headless.request_text();
        let text = pasted.recv_timeout(Duration::from_secs(5));
        assert_eq!(text.as_deref(), Ok("in the app"));
    }
}
