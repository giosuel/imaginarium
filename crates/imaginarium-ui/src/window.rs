// SPDX-License-Identifier: AGPL-3.0-or-later

//! A window and its frame pipeline: winit events in, one Vello scene per
//! frame out, and no frame at all when nothing changed.

use std::cell::Cell;

use crate::access::AccessTree;
use std::sync::Arc;
use std::time::{Duration, Instant};

use imaginarium_render::kurbo::{Affine, Point, Rect, Size, Vec2};
use imaginarium_render::peniko::Color;
use imaginarium_render::{
    FrameOutcome, FrameTiming, GpuInfo, GpuOptions, GpuStart, GpuStartup, RenderError, Scene, Skip,
    WindowRenderer,
};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{KeyCode, NamedKey, PhysicalKey};
use winit::window::{Window, WindowId};

/// What can go wrong running a window.
#[derive(Debug, thiserror::Error)]
pub enum UiError {
    /// The platform's event loop couldn't start.
    #[error("event loop: {0}")]
    EventLoop(String),
    /// The window couldn't be created.
    #[error("window: {0}")]
    Window(String),
    /// The GPU couldn't be set up or failed while drawing.
    #[error(transparent)]
    Render(#[from] RenderError),
}

/// How to open the window.
#[derive(Clone, Debug)]
pub struct WindowOptions {
    /// The title bar text.
    pub title: String,
    /// The initial inner size, in logical pixels.
    pub size: Size,
    /// Which GPU, and whether to wait for vertical sync.
    pub gpu: GpuOptions,
    /// The color behind everything the view draws.
    pub background: Color,
    /// Fill the screen (borderless) instead of opening at `size`.
    pub fullscreen: bool,
    /// The app ID on Wayland and the window class on X11
    /// (`ch.giosuel.Imaginarium`), which window rules match. Ignored on
    /// Windows and macOS.
    pub app_id: Option<String>,
    /// Whether the user can resize it. A window of fixed size (the start
    /// window) floats on tiling compositors (Hyprland, Sway), as dialogs do.
    pub resizable: bool,
    /// The program opens more windows from this one (New board): once it
    /// has drawn, a renderer for the next window is built on a thread, so
    /// that one opens without compiling pipelines first. Off for programs
    /// of one window, which would only pay for it.
    pub more_windows: bool,
}

impl Default for WindowOptions {
    fn default() -> Self {
        Self {
            title: "Imaginarium".into(),
            size: Size::new(1280.0, 800.0),
            gpu: GpuOptions::default(),
            background: Color::WHITE,
            fullscreen: false,
            app_id: None,
            resizable: true,
            more_windows: false,
        }
    }
}

/// Modifier keys held during an input.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    /// Shift.
    pub shift: bool,
    /// Control.
    pub ctrl: bool,
    /// Alt (Option on macOS).
    pub alt: bool,
    /// The logo key: Super, Windows, ⌘.
    pub meta: bool,
}

impl Modifiers {
    /// The platform's shortcut modifier: ⌘ on macOS, Ctrl elsewhere.
    pub fn command(&self) -> bool {
        if cfg!(target_os = "macos") {
            self.meta
        } else {
            self.ctrl
        }
    }

    /// The modifier that moves by words: Option on macOS, Ctrl elsewhere.
    pub fn word(&self) -> bool {
        if cfg!(target_os = "macos") {
            self.alt
        } else {
            self.ctrl
        }
    }
}

/// A key, by what it means in the current layout (not where it is).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Key {
    /// A key that types: "a", "A", "ä", "1", "+", " ".
    Character(String),
    /// Enter or Return.
    Enter,
    /// Tab.
    Tab,
    /// Escape.
    Escape,
    /// Backspace (deletes backwards).
    Backspace,
    /// Delete (deletes forwards).
    Delete,
    /// ←.
    ArrowLeft,
    /// →.
    ArrowRight,
    /// ↑.
    ArrowUp,
    /// ↓.
    ArrowDown,
    /// Home.
    Home,
    /// End.
    End,
    /// Page Up.
    PageUp,
    /// Page Down.
    PageDown,
    /// Any other key, by its winit name.
    Other(String),
}

/// What an input method (IME) is doing: composing CJK text, dead keys,
/// the emoji picker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ime {
    /// The input method became active for the focused field.
    Enabled,
    /// Text being composed, not part of the document yet. `cursor` is a
    /// byte range in `text` (the IME's own caret or highlight); `None`
    /// hides the caret.
    Preedit {
        /// The composition.
        text: String,
        /// The highlighted range, in bytes of `text`.
        cursor: Option<(usize, usize)>,
    },
    /// Finished text to insert (replaces any composition).
    Commit(String),
    /// The input method was turned off.
    Disabled,
}

/// Input for the view, in logical pixels.
#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    /// The pointer moved to `position`.
    PointerMoved(Point),
    /// The pointer left the window.
    PointerLeft,
    /// A pointer button went down or up.
    PointerButton {
        /// The primary (usually left) button.
        primary: bool,
        /// Down or up.
        pressed: bool,
        /// Where the pointer is.
        position: Point,
        /// 1 for a single click, 2 for a double click, 3 for a triple
        /// click (on presses; releases repeat the press's count).
        clicks: u8,
        /// Modifier keys held.
        modifiers: Modifiers,
    },
    /// The wheel or touchpad scrolled by `delta` logical pixels.
    Scroll {
        /// How far (positive: content moves right and down).
        delta: Vec2,
        /// Where the pointer is.
        position: Point,
        /// Modifier keys held (zoom with [`Modifiers::command`]).
        modifiers: Modifiers,
    },
    /// A key went down or up.
    Key {
        /// The key.
        key: Key,
        /// The text it types, if any (`None` for shortcuts and named
        /// keys). Text arrives here only while no input method composes.
        text: Option<String>,
        /// Down or up.
        pressed: bool,
        /// Whether this is a key repeat.
        repeat: bool,
        /// Modifier keys held.
        modifiers: Modifiers,
    },
    /// Input method events, while [`View::ime_area`] says a text field
    /// has the focus.
    Ime(Ime),
    /// Text read from the clipboard after a
    /// [`Clipboard::request_text`](crate::Clipboard::request_text), for
    /// the focused widget.
    Paste(String),
    /// The window gained (`true`) or lost the keyboard focus.
    WindowFocus(bool),
    /// A file was dropped on the window (see [`file_drops_supported`]).
    FileDropped(std::path::PathBuf),
}

/// How much bigger than its size a window opens: by the interface size
/// for one of a fixed size, else not at all.
fn fixed_factor(options: &WindowOptions) -> f64 {
    if options.resizable {
        1.0
    } else {
        interface_scale()
    }
}

/// What a window says the system prefers: on Windows and macOS (on Linux
/// winit doesn't know; the desktop portal says, `crate::appearance`).
fn system_theme(theme: Option<winit::window::Theme>) {
    if cfg!(any(target_os = "windows", target_os = "macos"))
        && let Some(theme) = theme
    {
        crate::appearance::set_system_prefers_dark(Some(theme == winit::window::Theme::Dark));
    }
}

/// Whether dropped files reach the windows ([`Input::FileDropped`]): on
/// Windows, macOS and X11, but not on Wayland, where winit 0.30 doesn't
/// report drops yet (it is used when `WAYLAND_DISPLAY` or `WAYLAND_SOCKET`
/// is set).
pub fn file_drops_supported() -> bool {
    if cfg!(any(target_os = "windows", target_os = "macos")) {
        return true;
    }
    // As winit picks Wayland: either variable, set and not empty.
    ["WAYLAND_DISPLAY", "WAYLAND_SOCKET"]
        .iter()
        .all(|name| std::env::var_os(name).is_none_or(|value| value.is_empty()))
}

/// One presented frame, for statistics.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameInfo {
    /// Frames presented so far, this one included.
    pub number: u64,
    /// The view building its scene (the toolkit's share of the frame).
    pub paint: Duration,
    /// The renderer's share: waiting for the window's texture, encoding
    /// the scene and submitting it.
    pub render: FrameTiming,
    /// When it was presented, since [`run`] was called.
    pub at: Duration,
    /// The window's scale factor (physical pixels per logical pixel).
    pub scale: f64,
    /// The surface size in physical pixels.
    pub physical_size: (u32, u32),
    /// Why it was drawn.
    pub causes: FrameCauses,
}

/// Why a frame was drawn; several reasons can come together.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameCauses {
    /// The window system asked: the window was shown or exposed.
    pub system: bool,
    /// The window's size or scale factor changed.
    pub resize: bool,
    /// The view handled an input.
    pub input: bool,
    /// A view timer fired (a caret blink, the end of a zoom pause).
    pub timer: bool,
    /// The view or an effect asked ([`request_frame`]), or the last frame
    /// asked for another ([`View::presented`]).
    pub request: bool,
    /// The screen reader asked for an action.
    pub access: bool,
}

impl FrameCauses {
    /// Whether the program caused it on its own (not the window system,
    /// the user or the screen reader): what an idle window must not do.
    pub fn by_itself(&self) -> bool {
        self.timer || self.request
    }
}

/// What made the shell ask for a frame.
#[derive(Clone, Copy, Debug)]
enum Cause {
    Resize,
    Input,
    Timer,
    Request,
    Access,
}

/// What a window shows. Until the widget tree (M2.4), views implement this
/// directly.
pub trait View {
    /// Draws the content into `scene`, in logical pixels, for a window of
    /// `size` logical pixels.
    fn paint(&mut self, scene: &mut Scene, size: Size);

    /// The window is open and drawing on `gpu`.
    fn opened(&mut self, gpu: &GpuInfo) {
        let _ = gpu;
    }

    /// Handles input. Return `true` if the content changed and needs a
    /// frame.
    fn input(&mut self, input: &Input) -> bool {
        let _ = input;
        false
    }

    /// Called after every presented frame. Return `true` to ask for
    /// another one right away (animations, scripted measurements).
    fn presented(&mut self, frame: &FrameInfo) -> bool {
        let _ = frame;
        false
    }

    /// When the view next wants to be woken without input (a caret blink,
    /// the end of a measurement). `None`: only input wakes it.
    fn wake_at(&mut self) -> Option<Instant> {
        None
    }

    /// The time from [`View::wake_at`] arrived. Return `true` if the
    /// content changed and needs a frame.
    fn wake(&mut self, now: Instant) -> bool {
        let _ = now;
        false
    }

    /// Whether the window should close now (scripted runs).
    fn should_close(&mut self) -> bool {
        false
    }

    /// Where the focused text field's caret or composition is, in logical
    /// pixels, so the input method places its candidate window next to it.
    /// `None` when no text field has the focus: the input method is off
    /// and keys arrive as plain [`Input::Key`]s.
    fn ime_area(&mut self) -> Option<Rect> {
        None
    }

    /// Describes the content for screen readers: adds nodes to `tree`
    /// (ids from 1; bounds in logical pixels). Called after frames while a
    /// screen reader listens.
    fn accessibility(&mut self, tree: &mut AccessTree) {
        let _ = tree;
    }

    /// A screen reader asks for something (focus, click, select text, …).
    /// Return `true` if the content changed and needs a frame.
    fn accessibility_action(&mut self, request: &accesskit::ActionRequest) -> bool {
        let _ = request;
        false
    }

    /// Windows to open, or this one to close: asked after input and
    /// timers.
    fn take_requests(&mut self) -> Vec<WindowRequest> {
        Vec::new()
    }

    /// The user asked to close the window (its close button, the window
    /// manager). Return `false` to keep it open; requests made now
    /// ([`View::take_requests`]) are still handled.
    fn close_requested(&mut self) -> bool {
        true
    }
}

impl<V: View + ?Sized> View for &mut V {
    fn paint(&mut self, scene: &mut Scene, size: Size) {
        (**self).paint(scene, size);
    }
    fn opened(&mut self, gpu: &GpuInfo) {
        (**self).opened(gpu);
    }
    fn input(&mut self, input: &Input) -> bool {
        (**self).input(input)
    }
    fn presented(&mut self, frame: &FrameInfo) -> bool {
        (**self).presented(frame)
    }
    fn wake_at(&mut self) -> Option<Instant> {
        (**self).wake_at()
    }
    fn wake(&mut self, now: Instant) -> bool {
        (**self).wake(now)
    }
    fn should_close(&mut self) -> bool {
        (**self).should_close()
    }
    fn ime_area(&mut self) -> Option<Rect> {
        (**self).ime_area()
    }
    fn accessibility(&mut self, tree: &mut AccessTree) {
        (**self).accessibility(tree);
    }
    fn accessibility_action(&mut self, request: &accesskit::ActionRequest) -> bool {
        (**self).accessibility_action(request)
    }
    fn take_requests(&mut self) -> Vec<WindowRequest> {
        (**self).take_requests()
    }
    fn close_requested(&mut self) -> bool {
        (**self).close_requested()
    }
}

impl<V: View + ?Sized> View for Box<V> {
    fn paint(&mut self, scene: &mut Scene, size: Size) {
        (**self).paint(scene, size);
    }
    fn opened(&mut self, gpu: &GpuInfo) {
        (**self).opened(gpu);
    }
    fn input(&mut self, input: &Input) -> bool {
        (**self).input(input)
    }
    fn presented(&mut self, frame: &FrameInfo) -> bool {
        (**self).presented(frame)
    }
    fn wake_at(&mut self) -> Option<Instant> {
        (**self).wake_at()
    }
    fn wake(&mut self, now: Instant) -> bool {
        (**self).wake(now)
    }
    fn should_close(&mut self) -> bool {
        (**self).should_close()
    }
    fn ime_area(&mut self) -> Option<Rect> {
        (**self).ime_area()
    }
    fn accessibility(&mut self, tree: &mut AccessTree) {
        (**self).accessibility(tree);
    }
    fn accessibility_action(&mut self, request: &accesskit::ActionRequest) -> bool {
        (**self).accessibility_action(request)
    }
    fn take_requests(&mut self) -> Vec<WindowRequest> {
        (**self).take_requests()
    }
    fn close_requested(&mut self) -> bool {
        (**self).close_requested()
    }
}

/// What a view asks of the windows (one window per document, a start
/// window that opens a board and closes itself).
pub enum WindowRequest {
    /// Open another window showing this view.
    Open(WindowOptions, Box<dyn View>),
    /// Close this view's window. The run ends when the last one closes.
    Close,
    /// Change this view's window title (a board renamed or saved).
    SetTitle(String),
}

impl std::fmt::Debug for WindowRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Open(options, _) => f.debug_tuple("Open").field(&options.title).finish(),
            Self::Close => f.write_str("Close"),
            Self::SetTitle(title) => f.debug_tuple("SetTitle").field(title).finish(),
        }
    }
}

/// Events from other threads into the event loop.
#[derive(Debug)]
enum UserEvent {
    Access(accesskit_winit::Event),
    /// The clipboard's thread read a text.
    Pasted,
    /// Another thread asked for frames ([`frame_waker`]): of one window,
    /// or of every one.
    Wake(Option<WindowId>),
}

/// Wakes the event loop from another thread.
type Waker = Box<dyn Fn(Option<WindowId>) + Send>;
static FRAME_WAKER: std::sync::Mutex<Option<Waker>> = std::sync::Mutex::new(None);

fn wake_from_another_thread(window: Option<WindowId>) {
    if let Ok(waker) = FRAME_WAKER.lock()
        && let Some(wake) = waker.as_ref()
    {
        wake(window);
    }
}

/// Something another thread can call to have every window draw again.
/// Safe to call before the event loop runs (it does nothing then) or after
/// it ended.
pub fn frame_waker() -> impl Fn() + Send + Sync + Clone + 'static {
    || wake_from_another_thread(None)
}

/// [`frame_waker`] for the window whose view is running now (a file saved
/// on its thread, so that window's pill shows it): only it draws. `None`
/// outside a window's view.
pub fn window_waker() -> Option<impl Fn() + Send + Sync + Clone + 'static> {
    let window = CURRENT.with(Cell::get)?;
    Some(move || wake_from_another_thread(Some(window)))
}

impl From<accesskit_winit::Event> for UserEvent {
    fn from(event: accesskit_winit::Event) -> Self {
        Self::Access(event)
    }
}

/// What a run of the window did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RunStats {
    /// Frames presented (by the first window).
    pub frames: u64,
    /// From the call to [`run`] until the first frame was presented.
    pub first_frame: Option<Duration>,
    /// Creating the window (part of the time to the first frame).
    pub window_created: Option<Duration>,
    /// The window's wait for the GPU and its work on it (surface,
    /// renderer): the GPU's share of the time to the first frame. The GPU
    /// opens on a thread of its own meanwhile ([`GpuStart`]).
    pub gpu_opened: Option<Duration>,
    /// Opening the GPU, step by step.
    pub gpu_startup: Option<GpuStartup>,
    /// The GPU that drew them.
    pub gpu: Option<GpuInfo>,
    /// The monitor the window was last on, if the window system says.
    pub monitor: Option<MonitorInfo>,
    /// Windows that closed because they failed (no surface, no GPU); the
    /// others stayed open.
    pub window_errors: Vec<String>,
}

/// A monitor, as the window system describes it.
#[derive(Clone, Debug, PartialEq)]
pub struct MonitorInfo {
    /// Its name (a connector such as `eDP-1` on Wayland), if it has one.
    pub name: Option<String>,
    /// Its size in physical pixels.
    pub physical_size: (u32, u32),
    /// Its scale factor (physical pixels per logical pixel).
    pub scale: f64,
    /// Its refresh rate in millihertz, if known.
    pub refresh_millihertz: Option<u32>,
}

impl MonitorInfo {
    fn of(monitor: &winit::monitor::MonitorHandle) -> Self {
        let size = monitor.size();
        Self {
            name: monitor.name(),
            physical_size: (size.width, size.height),
            scale: monitor.scale_factor(),
            refresh_millihertz: monitor.refresh_rate_millihertz(),
        }
    }
}

thread_local! {
    /// A frame for every window (asked outside any window's view).
    static FRAME_REQUESTED: Cell<bool> = const { Cell::new(false) };
    /// Frames for these windows.
    static WINDOW_FRAMES: std::cell::RefCell<Vec<WindowId>> = const { std::cell::RefCell::new(Vec::new()) };
    static SCALE_FACTOR: Cell<f64> = const { Cell::new(1.0) };
    /// The interface size (`ui.scale`): logical pixels are this much
    /// bigger than the system makes them.
    static INTERFACE_SCALE: Cell<f64> = const { Cell::new(1.0) };
    /// The input method starts over in every window.
    static IME_RESET: Cell<bool> = const { Cell::new(false) };
    /// The input method starts over in these windows.
    static IME_RESETS: std::cell::RefCell<Vec<WindowId>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The keyboard focus moved: the input method starts over, so a
/// composition begun in one field can't be committed into the next.
pub(crate) fn reset_ime() {
    match CURRENT.with(Cell::get) {
        Some(window) => IME_RESETS.with(|resets| resets.borrow_mut().push(window)),
        None => IME_RESET.with(|reset| reset.set(true)),
    }
}

/// A call into the view of window `id`: the frames it asks for, the scale
/// it reads and the input method it resets are that window's. Undone when
/// dropped.
struct Entered {
    previous: Option<WindowId>,
    scale: f64,
}

fn enter(id: WindowId, scale: f64) -> Entered {
    Entered {
        previous: CURRENT.with(|current| current.replace(Some(id))),
        scale: SCALE_FACTOR.with(|factor| factor.replace(scale)),
    }
}

impl Drop for Entered {
    fn drop(&mut self) {
        CURRENT.with(|current| current.set(self.previous));
        SCALE_FACTOR.with(|factor| factor.set(self.scale));
    }
}

/// How long to wait before drawing again after the GPU gave no texture in
/// time.
const RETRY_AFTER_TIMEOUT: Duration = Duration::from_millis(100);

/// The window's scale factor (device pixels per logical pixel), for
/// content that snaps to device pixels (canvas tiles). It includes the
/// [`interface_scale`].
pub fn scale_factor() -> f64 {
    SCALE_FACTOR.with(Cell::get)
}

/// The interface size: how much bigger than the system's every logical
/// pixel is (1 is the system's). Content that keeps its own size, the
/// board, divides by it.
pub fn interface_scale() -> f64 {
    INTERFACE_SCALE.with(Cell::get)
}

/// Sets the [`interface_scale`] (0.5 to 3): every window lays out again
/// at it, and windows of a fixed size grow or shrink with it (others keep
/// their size on screen: their content is larger in it).
pub fn set_interface_scale(scale: f64) {
    let scale = if scale.is_finite() {
        scale.clamp(0.5, 3.0)
    } else {
        1.0
    };
    if INTERFACE_SCALE.with(|current| current.replace(scale)) != scale {
        request_frame_everywhere();
    }
}

/// Asks for a frame: of the window whose view is running (its input, its
/// timers, its painting), else of every window on this thread. Effects
/// call this when what they show changed; the request is collected before
/// the event loop goes back to sleep.
pub fn request_frame() {
    match CURRENT.with(Cell::get) {
        Some(window) => WINDOW_FRAMES.with(|frames| frames.borrow_mut().push(window)),
        None => FRAME_REQUESTED.with(|requested| requested.set(true)),
    }
}

thread_local! {
    /// The window whose view is being called, if any.
    static CURRENT: Cell<Option<WindowId>> = const { Cell::new(None) };
    /// Widget trees and the windows they are in.
    static TREES: std::cell::RefCell<std::collections::HashMap<u64, WindowId>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// The key of the window whose view is being called (a clipboard read
/// remembers who asked).
pub(crate) fn current_window() -> Option<u64> {
    CURRENT.with(Cell::get).map(u64::from)
}

/// Tree `tree` is used in the window being called.
pub(crate) fn bind_tree(tree: u64) {
    if let Some(window) = CURRENT.with(Cell::get) {
        TREES.with(|trees| {
            trees.borrow_mut().insert(tree, window);
        });
    }
}

/// Tree `tree` is gone.
pub(crate) fn unbind_tree(tree: u64) {
    let _ = TREES.try_with(|trees| {
        if let Ok(mut trees) = trees.try_borrow_mut() {
            trees.remove(&tree);
        }
    });
}

/// Asks every window for a frame, wherever it is called from: for changes
/// every window shows (the theme, a setting changed in one window that
/// another follows).
pub fn request_frame_everywhere() {
    FRAME_REQUESTED.with(|requested| requested.set(true));
}

/// A frame for tree `tree`'s window (a signal changed by another window's
/// input can show in this one), or as [`request_frame`] for a tree not
/// laid out in a window yet.
pub(crate) fn request_frame_for_tree(tree: u64) {
    match TREES.with(|trees| trees.borrow().get(&tree).copied()) {
        Some(window) => WINDOW_FRAMES.with(|frames| frames.borrow_mut().push(window)),
        None => request_frame(),
    }
}

/// Opens a window showing `view` and runs until it is closed (by the user
/// or [`View::should_close`]) and every window it opened is closed too.
/// Blocks the calling thread, which must be the main thread on some
/// platforms. Pass `&mut view` to keep the view.
pub fn run<'v>(options: WindowOptions, view: impl View + 'v) -> Result<RunStats, UiError> {
    let gpu = GpuStart::spawn(&options.gpu);
    run_with_gpu(gpu, options, view)
}

/// [`run`], on a GPU that started opening earlier: call
/// [`GpuStart::spawn`] first thing, so the drivers load while the program
/// loads fonts and builds its view. Its options replace `options.gpu`.
pub fn run_with_gpu<'v>(
    gpu: GpuStart,
    options: WindowOptions,
    view: impl View + 'v,
) -> Result<RunStats, UiError> {
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .map_err(|err| UiError::EventLoop(err.to_string()))?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();
    let waker = proxy.clone();
    crate::clipboard::set_waker(move || {
        let _ = waker.send_event(UserEvent::Pasted);
    });
    let frames = proxy.clone();
    if let Ok(mut slot) = FRAME_WAKER.lock() {
        *slot = Some(Box::new(move |window| {
            let _ = frames.send_event(UserEvent::Wake(window));
        }));
    }
    // What the system prefers (the desktop portal, on Linux), from now on.
    crate::appearance::listen_to_the_system();
    let mut shell = Shell {
        windows: std::collections::HashMap::new(),
        pending: vec![(options, Box::new(view) as Box<dyn View + 'v>)],
        first: None,
        focused: None,
        gpu: Some(gpu),
        shared: None,
        proxy,
        started: Instant::now(),
        stats: RunStats::default(),
        error: None,
    };
    event_loop
        .run_app(&mut shell)
        .map_err(|err| UiError::EventLoop(err.to_string()))?;
    match shell.error {
        Some(err) => Err(err),
        None => Ok(shell.stats),
    }
}

/// One window: its view and what drawing and input need of it.
struct WindowState<'v> {
    options: WindowOptions,
    view: Box<dyn View + 'v>,
    window: Arc<Window>,
    renderer: Option<WindowRenderer>,
    /// Why the next frame is drawn.
    causes: FrameCauses,
    scene: Scene,
    /// Device pixels per logical pixel: the system's, times the interface
    /// scale.
    scale: f64,
    /// The system's part of it (the monitor's).
    os_scale: f64,
    /// The interface scale it was worked out with.
    interface: f64,
    /// The surface size in physical pixels, from `Resized` (asking the
    /// window costs a round trip to the X server).
    physical: PhysicalSize<u32>,
    pointer: Point,
    modifiers: Modifiers,
    clicks: ClickCounter,
    /// The input method's state as last told to the window: `None` off,
    /// otherwise on with this caret area.
    ime: Option<Rect>,
    /// The screen reader bridge; idle until a screen reader asks.
    access: accesskit_winit::Adapter,
    /// The last frame was skipped because the window is hidden: draw again
    /// when it shows (or something else asks), not in a loop.
    occluded: bool,
    /// After a frame skipped for a timeout: when to try again.
    retry_at: Option<Instant>,
    /// Send the input method's caret area even if it looks unchanged (the
    /// scale factor changed, and with it the physical position).
    ime_resend: bool,
    frames: u64,
    /// It asked to close (or the user did).
    closing: bool,
    /// It closes because it failed (no surface, no GPU).
    failed: Option<UiError>,
}

/// The event loop's state: every window, and the GPU they share.
struct Shell<'v> {
    windows: std::collections::HashMap<WindowId, WindowState<'v>>,
    /// Windows asked for, opened at the next chance.
    pending: Vec<(WindowOptions, Box<dyn View + 'v>)>,
    /// The window opened first, whose start the statistics measure.
    first: Option<WindowId>,
    /// The window with the keyboard (pasted text goes there).
    focused: Option<WindowId>,
    /// The GPU opening meanwhile, until the first frame takes it.
    gpu: Option<GpuStart>,
    /// The GPU the windows share once it is open, with a renderer ready
    /// for the next window.
    shared: Option<imaginarium_render::SharedGpu>,
    proxy: EventLoopProxy<UserEvent>,
    started: Instant,
    stats: RunStats,
    error: Option<UiError>,
}

impl WindowState<'_> {
    fn request_redraw(&mut self, cause: Cause) {
        match cause {
            Cause::Resize => self.causes.resize = true,
            Cause::Input => self.causes.input = true,
            Cause::Timer => self.causes.timer = true,
            Cause::Request => self.causes.request = true,
            Cause::Access => self.causes.access = true,
        }
        self.window.request_redraw();
    }

    fn monitor(&self) -> Option<MonitorInfo> {
        self.window
            .current_monitor()
            .map(|monitor| MonitorInfo::of(&monitor))
    }

    fn logical(&self, position: PhysicalPosition<f64>) -> Point {
        Point::new(position.x / self.scale, position.y / self.scale)
    }

    /// Tells the window whether a text field has the focus and where its
    /// caret is, when that changed.
    fn update_ime(&mut self, reset: bool) {
        let area = self.view.ime_area();
        if reset && self.ime.is_some() {
            self.window.set_ime_allowed(false);
            self.ime = None;
        }
        if area == self.ime && !std::mem::take(&mut self.ime_resend) {
            return;
        }
        if area.is_some() != self.ime.is_some() {
            self.window.set_ime_allowed(area.is_some());
        }
        if let Some(area) = area {
            // In device pixels: winit's logical ones are the system's,
            // without the interface size.
            let scale = self.scale;
            self.window.set_ime_cursor_area(
                PhysicalPosition::new(area.x0 * scale, area.y0 * scale),
                PhysicalSize::new(area.width() * scale, area.height() * scale),
            );
        }
        self.ime = area;
    }

    /// Turns the input method off (its window lost the keys while a field
    /// had them).
    fn ime_off(&mut self) {
        if self.ime.take().is_some() {
            self.window.set_ime_allowed(false);
        }
    }

    /// Sends the view's accessibility tree, if a screen reader listens.
    fn update_access(&mut self) {
        let (view, title, scale) = (&mut self.view, &self.options.title, self.scale);
        self.access.update_if_active(|| {
            let mut tree = AccessTree::default();
            view.accessibility(&mut tree);
            tree.into_update(title, scale)
        });
    }

    fn deliver(&mut self, input: Input) {
        if self.view.input(&input) {
            self.request_redraw(Cause::Input);
        }
    }
}

impl<'v> Shell<'v> {
    fn fail(&mut self, event_loop: &ActiveEventLoop, err: UiError) {
        self.error = Some(err);
        event_loop.exit();
    }

    /// Closes window `id` because it failed. The others stay open; the
    /// run fails if it was the last.
    fn fail_window(&mut self, id: WindowId, err: UiError) {
        if let Some(state) = self.windows.get_mut(&id)
            && state.failed.is_none()
        {
            self.stats.window_errors.push(err.to_string());
            state.closing = true;
            state.failed = Some(err);
        }
    }

    /// Opens the windows asked for. One that can't be opened is left out
    /// (the run fails if no window is left).
    fn open_pending(&mut self, event_loop: &ActiveEventLoop) {
        for (options, view) in std::mem::take(&mut self.pending) {
            if let Err(err) = self.open(event_loop, options, view) {
                self.stats.window_errors.push(err.to_string());
                if self.windows.is_empty() {
                    return self.fail(event_loop, err);
                }
            }
        }
    }

    fn open(
        &mut self,
        event_loop: &ActiveEventLoop,
        options: WindowOptions,
        view: Box<dyn View + 'v>,
    ) -> Result<(), UiError> {
        // Invisible until the accessibility adapter is attached, as
        // AccessKit requires (on Windows and macOS).
        let attributes = Window::default_attributes()
            .with_visible(false)
            .with_title(options.title.clone())
            // A window of a fixed size grows with the interface size (it
            // must hold its content); others open at their size, which the
            // user then changes.
            .with_inner_size(LogicalSize::new(
                options.size.width * fixed_factor(&options),
                options.size.height * fixed_factor(&options),
            ))
            .with_resizable(options.resizable)
            .with_fullscreen(
                options
                    .fullscreen
                    .then_some(winit::window::Fullscreen::Borderless(None)),
            );
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "android")))]
        let attributes = match &options.app_id {
            Some(app_id) => {
                use winit::platform::{wayland, x11};
                let attributes =
                    wayland::WindowAttributesExtWayland::with_name(attributes, app_id, app_id);
                x11::WindowAttributesExtX11::with_name(attributes, app_id, app_id)
            }
            None => attributes,
        };
        let creating = Instant::now();
        let window = event_loop
            .create_window(attributes)
            .map(Arc::new)
            .map_err(|err| UiError::Window(err.to_string()))?;
        let id = window.id();
        if self.first.is_none() {
            self.first = Some(id);
            self.stats.window_created = Some(creating.elapsed());
        }
        system_theme(window.theme());
        // The renderer comes with the first frame, after its layout: the
        // GPU keeps opening meanwhile.
        let access = accesskit_winit::Adapter::with_event_loop_proxy(
            event_loop,
            &window,
            self.proxy.clone(),
        );
        window.set_visible(true);
        window.request_redraw();
        let state = WindowState {
            options,
            view,
            scale: window.scale_factor() * interface_scale(),
            os_scale: window.scale_factor(),
            interface: interface_scale(),
            physical: window.inner_size(),
            window,
            renderer: None,
            causes: FrameCauses::default(),
            scene: Scene::new(),
            pointer: Point::ZERO,
            modifiers: Modifiers::default(),
            clicks: ClickCounter::default(),
            ime: None,
            access,
            occluded: false,
            retry_at: None,
            ime_resend: false,
            frames: 0,
            closing: false,
            failed: None,
        };
        self.windows.insert(id, state);
        self.focused.get_or_insert(id);
        Ok(())
    }

    /// The windows' requests: windows to open, and windows to close.
    fn handle_requests(&mut self) {
        for (id, state) in &mut self.windows {
            let _entered = enter(*id, state.scale);
            for request in state.view.take_requests() {
                match request {
                    WindowRequest::Open(options, view) => self.pending.push((options, view)),
                    WindowRequest::Close => state.closing = true,
                    WindowRequest::SetTitle(title) => state.window.set_title(&title),
                }
            }
            if state.view.should_close() {
                state.closing = true;
            }
        }
    }

    /// A renderer for `id`: the GPU opening since the start for the first
    /// window, a sibling of another window's for the next ones.
    fn renderer_for(&mut self, id: WindowId) -> Result<WindowRenderer, UiError> {
        let Some(state) = self.windows.get(&id) else {
            return Err(UiError::Window("no such window".into()));
        };
        let (window, physical) = (state.window.clone(), state.physical);
        let renderer = match &mut self.shared {
            Some(shared) => shared.window(window, physical.width, physical.height)?,
            None => {
                let gpu = self
                    .gpu
                    .take()
                    .unwrap_or_else(|| GpuStart::spawn(&state.options.gpu));
                let renderer =
                    WindowRenderer::with_start(gpu, window, physical.width, physical.height)?;
                self.shared = Some(renderer.shared_gpu());
                renderer
            }
        };
        Ok(renderer)
    }

    fn redraw(&mut self, id: WindowId) {
        let start = Instant::now();
        // A window closing (it failed, or asked to) draws no more.
        let Some(state) = self.windows.get_mut(&id).filter(|state| !state.closing) else {
            return;
        };
        // The interface size changed: the window lays out at it, and one of
        // a fixed size (the start window) grows or shrinks with it.
        let interface = interface_scale();
        if state.interface != interface {
            let old = state.scale;
            state.interface = interface;
            state.scale = state.os_scale * interface;
            state.ime_resend = true;
            // The pointer, in the new logical pixels (until it moves).
            state.pointer = Point::new(
                state.pointer.x * old / state.scale,
                state.pointer.y * old / state.scale,
            );
            if !state.options.resizable {
                let size = LogicalSize::new(
                    state.options.size.width * interface,
                    state.options.size.height * interface,
                );
                // A window that can't be resized has its size as both bounds
                // (Wayland): they move first.
                state.window.set_min_inner_size(Some(size));
                state.window.set_max_inner_size(Some(size));
                // Wayland applies it at once and says so here, with no
                // `Resized` to follow.
                if let Some(physical) = state.window.request_inner_size(size)
                    && physical != state.physical
                {
                    state.physical = physical;
                    if let Some(renderer) = &mut state.renderer {
                        renderer.resize(physical.width, physical.height);
                    }
                }
            }
        }
        let physical = state.physical;
        let size = Size::new(
            f64::from(physical.width) / state.scale,
            f64::from(physical.height) / state.scale,
        );
        state.scene.clear();
        let _entered = enter(id, state.scale);
        state.view.paint(&mut state.scene, size);
        let paint = start.elapsed();
        if state.renderer.is_none() {
            let opening = Instant::now();
            let renderer = match self.renderer_for(id) {
                Ok(renderer) => renderer,
                Err(err) => return self.fail_window(id, err),
            };
            let first = self.first == Some(id);
            if first {
                self.stats.gpu_opened = Some(opening.elapsed());
                self.stats.gpu_startup = Some(*renderer.startup());
                self.stats.gpu = Some(renderer.info().clone());
            }
            let Some(state) = self.windows.get_mut(&id) else {
                return;
            };
            state.view.opened(renderer.info());
            state.renderer = Some(renderer);
            // The view learned about the GPU after this frame's layout:
            // one more frame shows what it does with it.
            state.request_redraw(Cause::Request);
        }
        let Some(state) = self.windows.get_mut(&id) else {
            return;
        };
        let Some(renderer) = &mut state.renderer else {
            return;
        };
        let outcome = renderer.render(
            &state.scene,
            state.options.background,
            Affine::scale(state.scale),
        );
        let render = match outcome {
            Ok(FrameOutcome::Presented(timing)) => timing,
            // The causes stay: the frame that is drawn counts them.
            Ok(FrameOutcome::Skipped(skip)) => {
                match skip {
                    Skip::Reconfigured => state.window.request_redraw(),
                    Skip::Lost => match renderer.recreate_surface(state.window.clone()) {
                        Ok(()) => state.window.request_redraw(),
                        Err(err) => self.fail_window(id, err.into()),
                    },
                    // Asking again at once would spin while it's minimized
                    // (winit delivers redraws to hidden windows too).
                    Skip::Occluded => state.occluded = true,
                    Skip::Timeout => state.retry_at = Some(Instant::now() + RETRY_AFTER_TIMEOUT),
                }
                return;
            }
            Err(err) => {
                self.fail_window(id, err.into());
                return;
            }
        };
        state.occluded = false;
        state.frames += 1;
        // Drawn and about to idle: the next window's renderer builds now,
        // not while this one starts up.
        if state.frames == 2
            && state.options.more_windows
            && let Some(shared) = &mut self.shared
        {
            shared.prepare();
        }
        let mut causes = std::mem::take(&mut state.causes);
        if causes == FrameCauses::default() {
            causes.system = true;
        }
        let now = Instant::now();
        if self.first == Some(id) {
            self.stats.frames = state.frames;
            if self.stats.first_frame.is_none() {
                self.stats.first_frame = Some(now - self.started);
                self.stats.monitor = state.monitor().or(self.stats.monitor.take());
            }
        }
        let frame = FrameInfo {
            number: state.frames,
            paint,
            render,
            at: now - self.started,
            scale: state.scale,
            physical_size: (physical.width, physical.height),
            causes,
        };
        if state.view.presented(&frame) {
            state.request_redraw(Cause::Request);
        }
        // What is on screen changed: tell the screen reader.
        state.update_access();
    }
}

impl ApplicationHandler<UserEvent> for Shell<'_> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.open_pending(event_loop);
    }

    fn user_event(&mut self, _: &ActiveEventLoop, event: UserEvent) {
        let event = match event {
            UserEvent::Access(event) => event,
            UserEvent::Wake(window) => {
                for (id, state) in &mut self.windows {
                    if window.is_none_or(|window| window == *id) {
                        state.request_redraw(Cause::Request);
                    }
                }
                return;
            }
            UserEvent::Pasted => {
                // Pasted text goes to the window that asked, else where
                // the keys are.
                for (asked, text) in crate::clipboard::take_pasted() {
                    // Not to another window if the one that asked closed.
                    let target = match asked {
                        Some(asked) => self
                            .windows
                            .keys()
                            .copied()
                            .find(|id| u64::from(*id) == asked),
                        None => self.focused,
                    };
                    if let Some(id) = target
                        && let Some(state) = self.windows.get_mut(&id)
                    {
                        let _entered = enter(id, state.scale);
                        state.deliver(Input::Paste(text));
                    }
                }
                return;
            }
        };
        let Some(state) = self.windows.get_mut(&event.window_id) else {
            return;
        };
        let _entered = enter(event.window_id, state.scale);
        match event.window_event {
            accesskit_winit::WindowEvent::InitialTreeRequested => state.update_access(),
            accesskit_winit::WindowEvent::ActionRequested(request) => {
                if state.view.accessibility_action(&request) {
                    state.request_redraw(Cause::Access);
                }
                state.update_access();
            }
            accesskit_winit::WindowEvent::AccessibilityDeactivated => {}
        }
    }

    fn window_event(&mut self, _: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if matches!(event, WindowEvent::RedrawRequested) {
            // A change of the system's look shows in this frame.
            crate::appearance::refresh();
            return self.redraw(id);
        }
        if let WindowEvent::ThemeChanged(theme) = &event {
            system_theme(Some(*theme));
        }
        let Some(state) = self.windows.get_mut(&id) else {
            return;
        };
        let _entered = enter(id, state.scale);
        state.access.process_event(&state.window, &event);
        match event {
            WindowEvent::CloseRequested => {
                if state.view.close_requested() {
                    state.closing = true;
                }
            }
            WindowEvent::Resized(size) => {
                // Compositors repeat the size (Hyprland on focus changes):
                // nothing to draw then.
                if size == state.physical {
                    return;
                }
                state.physical = size;
                if let Some(renderer) = &mut state.renderer {
                    renderer.resize(size.width, size.height);
                }
                if self.first == Some(id) {
                    self.stats.monitor = state.monitor().or(self.stats.monitor.take());
                }
                state.request_redraw(Cause::Resize);
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                // A move to a monitor with another scale (criterion 5). The
                // new physical size follows as a `Resized` event.
                state.os_scale = scale_factor;
                state.scale = scale_factor * state.interface;
                state.ime_resend = true;
                if self.first == Some(id) {
                    self.stats.monitor = state.monitor().or(self.stats.monitor.take());
                }
                state.request_redraw(Cause::Resize);
            }
            WindowEvent::Occluded(false) if state.occluded => {
                state.occluded = false;
                state.request_redraw(Cause::Resize);
            }
            WindowEvent::CursorMoved { position, .. } => {
                state.pointer = state.logical(position);
                let at = state.pointer;
                state.deliver(Input::PointerMoved(at));
            }
            WindowEvent::CursorLeft { .. } => state.deliver(Input::PointerLeft),
            WindowEvent::DroppedFile(path) => state.deliver(Input::FileDropped(path)),
            // The left button is `primary`; `primary: false` is the right
            // one. The others (middle, back, forward) aren't delivered yet,
            // so they don't open context menus.
            WindowEvent::MouseInput { button, .. }
                if !matches!(button, MouseButton::Left | MouseButton::Right) => {}
            WindowEvent::MouseInput {
                state: pressed,
                button,
                ..
            } => {
                let pressed = pressed == ElementState::Pressed;
                let clicks = state.clicks.count(button, pressed, state.pointer);
                let input = Input::PointerButton {
                    primary: button == MouseButton::Left,
                    pressed,
                    position: state.pointer,
                    clicks,
                    modifiers: state.modifiers,
                };
                state.deliver(input);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let delta = match delta {
                    // One wheel notch scrolls about three lines.
                    MouseScrollDelta::LineDelta(x, y) => {
                        Vec2::new(f64::from(x), f64::from(y)) * 48.0
                    }
                    MouseScrollDelta::PixelDelta(p) => Vec2::new(p.x, p.y) / state.scale,
                };
                let input = Input::Scroll {
                    delta,
                    position: state.pointer,
                    modifiers: state.modifiers,
                };
                state.deliver(input);
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                let keys = modifiers.state();
                state.modifiers = Modifiers {
                    shift: keys.shift_key(),
                    ctrl: keys.control_key(),
                    alt: keys.alt_key(),
                    meta: keys.super_key(),
                };
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let input = key_input(&event, state.modifiers);
                state.deliver(input);
            }
            WindowEvent::Ime(ime) => {
                let ime = match ime {
                    winit::event::Ime::Enabled => Ime::Enabled,
                    winit::event::Ime::Preedit(text, cursor) => Ime::Preedit { text, cursor },
                    winit::event::Ime::Commit(text) => Ime::Commit(text),
                    winit::event::Ime::Disabled => Ime::Disabled,
                };
                state.deliver(Input::Ime(ime));
            }
            WindowEvent::Focused(focused) => {
                if focused {
                    self.focused = Some(id);
                } else {
                    // Keys released elsewhere never reach us.
                    state.modifiers = Modifiers::default();
                }
                state.deliver(Input::WindowFocus(focused));
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        let mut wake_at: Option<Instant> = None;
        for (id, state) in &mut self.windows {
            if state.retry_at.is_some_and(|at| at <= now) {
                state.retry_at = None;
                state.window.request_redraw();
            }
            let _entered = enter(*id, state.scale);
            let mut wake = state.view.wake_at();
            if wake.is_some_and(|at| at <= now) {
                if state.view.wake(now) {
                    state.request_redraw(Cause::Timer);
                }
                wake = state.view.wake_at().filter(|at| *at > now);
            }
            for at in [wake, state.retry_at].into_iter().flatten() {
                wake_at = Some(wake_at.map_or(at, |soonest| soonest.min(at)));
            }
        }
        // After the timers, which may have asked for frames too: each
        // window draws when its own view asked, all of them when the
        // request came from outside any window (a theme switch).
        let everyone = FRAME_REQUESTED.with(|requested| requested.replace(false));
        let asked = WINDOW_FRAMES.with(|frames| std::mem::take(&mut *frames.borrow_mut()));
        for (id, state) in &mut self.windows {
            if everyone || asked.contains(id) {
                state.request_redraw(Cause::Request);
            }
        }
        // Windows to open or close, after the timers: one may have ended
        // a scripted run.
        self.handle_requests();
        let closing: Vec<WindowId> = self
            .windows
            .iter()
            .filter(|(_, state)| state.closing)
            .map(|(id, _)| *id)
            .collect();
        let mut failed = None;
        for id in closing {
            if let Some(mut state) = self.windows.remove(&id) {
                failed = state.failed.take().or(failed);
            }
        }
        // The keys go somewhere else only when the window system says so.
        if self
            .focused
            .is_some_and(|id| !self.windows.contains_key(&id))
        {
            self.focused = None;
        }
        self.open_pending(event_loop);
        if self.windows.is_empty() {
            // Ended by a failure, the run fails.
            if let Some(err) = failed {
                self.error.get_or_insert(err);
            }
            event_loop.exit();
            return;
        }
        let everyone = IME_RESET.with(|reset| reset.replace(false));
        let resets = IME_RESETS.with(|resets| std::mem::take(&mut *resets.borrow_mut()));
        let focused = self.focused;
        for (id, state) in &mut self.windows {
            let reset = everyone || resets.contains(id);
            let _entered = enter(*id, state.scale);
            if focused == Some(*id) {
                state.update_ime(reset);
            } else if reset {
                // Off where the keys aren't; on again when they come back.
                state.ime_off();
            }
        }
        // Sleep until input arrives or a view's timer runs out: nothing is
        // drawn unless something changed (gate criterion 6).
        event_loop.set_control_flow(match wake_at {
            Some(at) => ControlFlow::WaitUntil(at),
            None => ControlFlow::Wait,
        });
    }
}

/// Translates a winit key event.
fn key_input(event: &KeyEvent, modifiers: Modifiers) -> Input {
    // Shortcuts don't type. Ctrl + Alt is AltGr on Windows, which does.
    let shortcut = (modifiers.ctrl && !modifiers.alt) || modifiers.meta;
    let key = shortcut_key(logical_key(event), event.physical_key, shortcut);
    let text = event
        .text
        .as_ref()
        .map(|text| text.chars().filter(|c| !c.is_control()).collect::<String>())
        .filter(|text| !text.is_empty() && !shortcut);
    Input::Key {
        key,
        text,
        pressed: event.state == ElementState::Pressed,
        repeat: event.repeat,
        modifiers,
    }
}

/// With a shortcut modifier held, a letter of a non-Latin layout (Ctrl+С
/// on a Russian keyboard) stands for the Latin letter on the same key, as
/// in other apps: shortcuts are matched by their Latin names.
fn shortcut_key(key: Key, physical: PhysicalKey, shortcut: bool) -> Key {
    let Key::Character(typed) = &key else {
        return key;
    };
    if !shortcut || typed.is_ascii() {
        return key;
    }
    match latin_letter(physical) {
        Some(letter) => Key::Character(letter.to_string()),
        None => key,
    }
}

/// The letter or digit a key has on a US layout.
fn latin_letter(physical: PhysicalKey) -> Option<char> {
    let PhysicalKey::Code(code) = physical else {
        return None;
    };
    let letters = [
        (KeyCode::KeyA, 'a'),
        (KeyCode::KeyB, 'b'),
        (KeyCode::KeyC, 'c'),
        (KeyCode::KeyD, 'd'),
        (KeyCode::KeyE, 'e'),
        (KeyCode::KeyF, 'f'),
        (KeyCode::KeyG, 'g'),
        (KeyCode::KeyH, 'h'),
        (KeyCode::KeyI, 'i'),
        (KeyCode::KeyJ, 'j'),
        (KeyCode::KeyK, 'k'),
        (KeyCode::KeyL, 'l'),
        (KeyCode::KeyM, 'm'),
        (KeyCode::KeyN, 'n'),
        (KeyCode::KeyO, 'o'),
        (KeyCode::KeyP, 'p'),
        (KeyCode::KeyQ, 'q'),
        (KeyCode::KeyR, 'r'),
        (KeyCode::KeyS, 's'),
        (KeyCode::KeyT, 't'),
        (KeyCode::KeyU, 'u'),
        (KeyCode::KeyV, 'v'),
        (KeyCode::KeyW, 'w'),
        (KeyCode::KeyX, 'x'),
        (KeyCode::KeyY, 'y'),
        (KeyCode::KeyZ, 'z'),
        (KeyCode::Digit0, '0'),
        (KeyCode::Digit1, '1'),
        (KeyCode::Digit2, '2'),
        (KeyCode::Digit3, '3'),
        (KeyCode::Digit4, '4'),
        (KeyCode::Digit5, '5'),
        (KeyCode::Digit6, '6'),
        (KeyCode::Digit7, '7'),
        (KeyCode::Digit8, '8'),
        (KeyCode::Digit9, '9'),
    ];
    letters
        .iter()
        .find(|(key, _)| *key == code)
        .map(|&(_, letter)| letter)
}

/// Our name for the key's meaning on the current layout.
fn logical_key(event: &KeyEvent) -> Key {
    match &event.logical_key {
        winit::keyboard::Key::Character(text) => Key::Character(text.to_string()),
        winit::keyboard::Key::Named(named) => match named {
            NamedKey::Enter => Key::Enter,
            NamedKey::Tab => Key::Tab,
            NamedKey::Escape => Key::Escape,
            NamedKey::Backspace => Key::Backspace,
            NamedKey::Delete => Key::Delete,
            NamedKey::ArrowLeft => Key::ArrowLeft,
            NamedKey::ArrowRight => Key::ArrowRight,
            NamedKey::ArrowUp => Key::ArrowUp,
            NamedKey::ArrowDown => Key::ArrowDown,
            NamedKey::Home => Key::Home,
            NamedKey::End => Key::End,
            NamedKey::PageUp => Key::PageUp,
            NamedKey::PageDown => Key::PageDown,
            NamedKey::Space => Key::Character(" ".into()),
            other => Key::Other(format!("{other:?}")),
        },
        other => Key::Other(format!("{other:?}")),
    }
}

/// Counts presses of the same button at the same spot in quick succession
/// (double and triple clicks).
#[derive(Debug, Default)]
struct ClickCounter {
    last: Option<(MouseButton, Instant, Point)>,
    count: u8,
}

impl ClickCounter {
    /// The usual default of desktop platforms.
    const INTERVAL: Duration = Duration::from_millis(500);
    /// How far the pointer may move between clicks, in logical pixels.
    const SLOP: f64 = 4.0;

    fn count(&mut self, button: MouseButton, pressed: bool, position: Point) -> u8 {
        if !pressed {
            return self.count.max(1);
        }
        let now = Instant::now();
        let again = self.last.is_some_and(|(last, at, spot)| {
            last == button && now - at <= Self::INTERVAL && (position - spot).hypot() <= Self::SLOP
        });
        // A fourth click starts over.
        self.count = if again && self.count < 3 {
            self.count + 1
        } else {
            1
        };
        self.last = Some((button, now, position));
        self.count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(key: u64) -> WindowId {
        WindowId::from(key)
    }

    fn take_frames() -> (bool, Vec<WindowId>) {
        (
            FRAME_REQUESTED.with(|requested| requested.replace(false)),
            WINDOW_FRAMES.with(|frames| std::mem::take(&mut *frames.borrow_mut())),
        )
    }

    #[test]
    fn a_frame_is_asked_of_the_window_whose_view_runs() {
        take_frames();
        {
            let _entered = enter(window(1), 2.0);
            request_frame();
            assert_eq!(scale_factor(), 2.0);
        }
        assert_eq!(take_frames(), (false, vec![window(1)]));
        // Outside any view (a theme switch): every window.
        request_frame();
        assert_eq!(take_frames(), (true, vec![]));
    }

    #[test]
    fn a_theme_switch_in_one_window_draws_them_all() {
        take_frames();
        {
            let _entered = enter(window(5), 1.0);
            crate::theme::set_theme(crate::theme::Theme::light(crate::theme::DEFAULT_ACCENT));
        }
        let (everyone, _) = take_frames();
        assert!(everyone);
        crate::theme::set_theme(crate::theme::Theme::dark(crate::theme::DEFAULT_ACCENT));
        take_frames();
    }

    #[test]
    fn leaving_a_window_restores_the_scale_and_window() {
        let _outer = enter(window(1), 1.0);
        {
            let _inner = enter(window(2), 2.0);
            assert_eq!(scale_factor(), 2.0);
            assert_eq!(current_window(), Some(2));
        }
        assert_eq!(scale_factor(), 1.0);
        assert_eq!(current_window(), Some(1));
    }

    #[test]
    fn a_tree_asks_its_own_window_for_frames() {
        take_frames();
        {
            let _entered = enter(window(7), 1.0);
            bind_tree(42);
        }
        // A signal changed by window 8's input shows in window 7's tree.
        {
            let _entered = enter(window(8), 1.0);
            request_frame_for_tree(42);
        }
        assert_eq!(take_frames(), (false, vec![window(7)]));
        unbind_tree(42);
        {
            let _entered = enter(window(8), 1.0);
            request_frame_for_tree(42);
        }
        assert_eq!(take_frames(), (false, vec![window(8)]));
    }

    #[test]
    fn the_input_method_resets_in_the_window_that_moved_the_focus() {
        IME_RESETS.with(|resets| resets.borrow_mut().clear());
        {
            let _entered = enter(window(3), 1.0);
            reset_ime();
        }
        assert_eq!(
            IME_RESETS.with(|resets| std::mem::take(&mut *resets.borrow_mut())),
            vec![window(3)]
        );
        assert!(!IME_RESET.with(Cell::get));
    }

    #[test]
    fn shortcuts_on_non_latin_layouts_use_the_latin_letter() {
        let c = PhysicalKey::Code(KeyCode::KeyC);
        let cyrillic = || Key::Character("с".into());
        assert_eq!(
            shortcut_key(cyrillic(), c, true),
            Key::Character("c".into())
        );
        assert_eq!(shortcut_key(cyrillic(), c, false), cyrillic(), "typing");
        // Latin layouts keep theirs (Ctrl+Z on a German keyboard's Y key).
        let z = Key::Character("z".into());
        let y = PhysicalKey::Code(KeyCode::KeyY);
        assert_eq!(shortcut_key(z.clone(), y, true), z);
    }

    #[test]
    fn clicks_in_quick_succession_count_up_to_three() {
        let mut clicks = ClickCounter::default();
        let at = Point::new(10.0, 10.0);
        assert_eq!(clicks.count(MouseButton::Left, true, at), 1);
        assert_eq!(clicks.count(MouseButton::Left, false, at), 1);
        assert_eq!(clicks.count(MouseButton::Left, true, at), 2);
        assert_eq!(clicks.count(MouseButton::Left, true, at), 3);
        assert_eq!(clicks.count(MouseButton::Left, true, at), 1);
        // Elsewhere, or another button: a new single click.
        assert_eq!(
            clicks.count(MouseButton::Left, true, Point::new(40.0, 10.0)),
            1
        );
        assert_eq!(
            clicks.count(MouseButton::Right, true, Point::new(40.0, 10.0)),
            1
        );
    }
}
