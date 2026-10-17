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
use winit::dpi::{LogicalPosition, LogicalSize, PhysicalPosition, PhysicalSize};
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
}

/// What a view asks of the windows (one window per document, a start
/// window that opens a board and closes itself).
pub enum WindowRequest {
    /// Open another window showing this view.
    Open(WindowOptions, Box<dyn View>),
    /// Close this view's window. The run ends when the last one closes.
    Close,
}

impl std::fmt::Debug for WindowRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Open(options, _) => f.debug_tuple("Open").field(&options.title).finish(),
            Self::Close => f.write_str("Close"),
        }
    }
}

/// Events from other threads into the event loop.
#[derive(Debug)]
enum UserEvent {
    Access(accesskit_winit::Event),
    /// The clipboard's thread read a text.
    Pasted,
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
    static FRAME_REQUESTED: Cell<bool> = const { Cell::new(false) };
    static SCALE_FACTOR: Cell<f64> = const { Cell::new(1.0) };
    static IME_RESET: Cell<bool> = const { Cell::new(false) };
}

/// The keyboard focus moved: the input method starts over, so a
/// composition begun in one field can't be committed into the next.
pub(crate) fn reset_ime() {
    IME_RESET.with(|reset| reset.set(true));
}

/// How long to wait before drawing again after the GPU gave no texture in
/// time.
const RETRY_AFTER_TIMEOUT: Duration = Duration::from_millis(100);

/// The window's scale factor (device pixels per logical pixel), for
/// content that snaps to device pixels (canvas tiles).
pub fn scale_factor() -> f64 {
    SCALE_FACTOR.with(Cell::get)
}

/// Asks for a frame of the current thread's windows. Effects call this
/// when what they show changed; the request is collected before the event
/// loop goes back to sleep.
pub fn request_frame() {
    FRAME_REQUESTED.with(|requested| requested.set(true));
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
    let mut shell = Shell {
        windows: std::collections::HashMap::new(),
        pending: vec![(options, Box::new(view) as Box<dyn View + 'v>)],
        first: None,
        focused: None,
        gpu: Some(gpu),
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
    scale: f64,
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
            self.window.set_ime_cursor_area(
                LogicalPosition::new(area.x0, area.y0),
                LogicalSize::new(area.width(), area.height()),
            );
        }
        self.ime = area;
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

    /// Opens the windows asked for.
    fn open_pending(&mut self, event_loop: &ActiveEventLoop) {
        for (options, view) in std::mem::take(&mut self.pending) {
            if let Err(err) = self.open(event_loop, options, view) {
                return self.fail(event_loop, err);
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
            .with_inner_size(LogicalSize::new(options.size.width, options.size.height))
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
            scale: window.scale_factor(),
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
        };
        self.windows.insert(id, state);
        self.focused.get_or_insert(id);
        Ok(())
    }

    /// The windows' requests: windows to open, and windows to close.
    fn handle_requests(&mut self) {
        for state in self.windows.values_mut() {
            for request in state.view.take_requests() {
                match request {
                    WindowRequest::Open(options, view) => self.pending.push((options, view)),
                    WindowRequest::Close => state.closing = true,
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
        if let Some(other) = self
            .windows
            .values()
            .find_map(|other| other.renderer.as_ref())
        {
            return Ok(other.sibling(window, physical.width, physical.height)?);
        }
        let gpu = self
            .gpu
            .take()
            .unwrap_or_else(|| GpuStart::spawn(&state.options.gpu));
        Ok(WindowRenderer::with_start(
            gpu,
            window,
            physical.width,
            physical.height,
        )?)
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop, id: WindowId) {
        let start = Instant::now();
        let Some(state) = self.windows.get_mut(&id) else {
            return;
        };
        let physical = state.physical;
        let size = Size::new(
            f64::from(physical.width) / state.scale,
            f64::from(physical.height) / state.scale,
        );
        state.scene.clear();
        SCALE_FACTOR.with(|scale| scale.set(state.scale));
        state.view.paint(&mut state.scene, size);
        let paint = start.elapsed();
        if state.renderer.is_none() {
            let opening = Instant::now();
            let renderer = match self.renderer_for(id) {
                Ok(renderer) => renderer,
                Err(err) => return self.fail(event_loop, err),
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
                        Err(err) => self.fail(event_loop, err.into()),
                    },
                    // Asking again at once would spin while it's minimized
                    // (winit delivers redraws to hidden windows too).
                    Skip::Occluded => state.occluded = true,
                    Skip::Timeout => state.retry_at = Some(Instant::now() + RETRY_AFTER_TIMEOUT),
                }
                return;
            }
            Err(err) => {
                self.fail(event_loop, err.into());
                return;
            }
        };
        state.occluded = false;
        state.frames += 1;
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
            UserEvent::Pasted => {
                // Pasted text goes where the keys are.
                let target = self.focused.and_then(|id| self.windows.get_mut(&id));
                if let Some(state) = target {
                    for text in crate::clipboard::take_pasted() {
                        state.deliver(Input::Paste(text));
                    }
                }
                return;
            }
        };
        let Some(state) = self.windows.get_mut(&event.window_id) else {
            return;
        };
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

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if matches!(event, WindowEvent::RedrawRequested) {
            return self.redraw(event_loop, id);
        }
        let Some(state) = self.windows.get_mut(&id) else {
            return;
        };
        state.access.process_event(&state.window, &event);
        match event {
            WindowEvent::CloseRequested => state.closing = true,
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
                state.scale = scale_factor;
                state.ime_resend = true;
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
        let requested = FRAME_REQUESTED.with(|requested| requested.replace(false));
        let mut wake_at: Option<Instant> = None;
        for state in self.windows.values_mut() {
            if state.retry_at.is_some_and(|at| at <= now) {
                state.retry_at = None;
                state.window.request_redraw();
            }
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
        // After the timers, which may have asked for a frame too. Which
        // window's content asked isn't known: they all draw (one that
        // didn't change draws the same).
        if requested || FRAME_REQUESTED.with(|requested| requested.replace(false)) {
            for state in self.windows.values_mut() {
                state.request_redraw(Cause::Request);
            }
        }
        // Windows to open or close, after the timers: one may have ended
        // a scripted run.
        self.handle_requests();
        self.windows.retain(|_, state| !state.closing);
        if self
            .focused
            .is_some_and(|id| !self.windows.contains_key(&id))
        {
            self.focused = self.windows.keys().next().copied();
        }
        self.open_pending(event_loop);
        if self.windows.is_empty() {
            event_loop.exit();
            return;
        }
        let reset = IME_RESET.with(|reset| reset.replace(false));
        if let Some(state) = self.focused.and_then(|id| self.windows.get_mut(&id)) {
            state.update_ime(reset);
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
