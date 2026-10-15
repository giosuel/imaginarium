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
    FrameOutcome, FrameTiming, GpuInfo, GpuOptions, GpuStart, GpuStartup, RenderError, Scene,
    WindowRenderer,
};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalPosition, LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::NamedKey;
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
}

impl Default for WindowOptions {
    fn default() -> Self {
        Self {
            title: "Imaginarium".into(),
            size: Size::new(1280.0, 800.0),
            gpu: GpuOptions::default(),
            background: Color::WHITE,
            fullscreen: false,
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
}

/// Events from other threads into the event loop.
#[derive(Debug)]
enum UserEvent {
    Access(accesskit_winit::Event),
}

impl From<accesskit_winit::Event> for UserEvent {
    fn from(event: accesskit_winit::Event) -> Self {
        Self::Access(event)
    }
}

/// What a run of the window did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RunStats {
    /// Frames presented.
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
}

/// The window's scale factor (device pixels per logical pixel), for
/// content that snaps to device pixels (canvas tiles).
pub fn scale_factor() -> f64 {
    SCALE_FACTOR.with(Cell::get)
}

/// Asks for a frame of the current thread's window. Effects call this when
/// what they show changed; the request is collected before the event loop
/// goes back to sleep.
pub fn request_frame() {
    FRAME_REQUESTED.with(|requested| requested.set(true));
}

/// Opens a window showing `view` and runs until it is closed (by the user
/// or [`View::should_close`]). Blocks the calling thread, which must be the
/// main thread on some platforms. Pass `&mut view` to keep the view.
pub fn run(options: WindowOptions, view: impl View) -> Result<RunStats, UiError> {
    let gpu = GpuStart::spawn(&options.gpu);
    run_with_gpu(gpu, options, view)
}

/// [`run`], on a GPU that started opening earlier: call
/// [`GpuStart::spawn`] first thing, so the drivers load while the program
/// loads fonts and builds its view. Its options replace `options.gpu`.
pub fn run_with_gpu(
    gpu: GpuStart,
    options: WindowOptions,
    view: impl View,
) -> Result<RunStats, UiError> {
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .map_err(|err| UiError::EventLoop(err.to_string()))?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();
    let mut shell = Shell {
        options,
        view,
        window: None,
        renderer: None,
        gpu: Some(gpu),
        causes: FrameCauses::default(),
        scene: Scene::new(),
        scale: 1.0,
        physical: PhysicalSize::new(0, 0),
        pointer: Point::ZERO,
        modifiers: Modifiers::default(),
        clicks: ClickCounter::default(),
        ime: None,
        proxy,
        access: None,
        access_initial: false,
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

struct Shell<V> {
    options: WindowOptions,
    view: V,
    window: Option<Arc<Window>>,
    renderer: Option<WindowRenderer>,
    /// The GPU opening meanwhile, until the first frame takes it.
    gpu: Option<GpuStart>,
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
    proxy: EventLoopProxy<UserEvent>,
    /// The screen reader bridge; idle until a screen reader asks.
    access: Option<accesskit_winit::Adapter>,
    /// The next tree update must be a full one.
    access_initial: bool,
    started: Instant,
    stats: RunStats,
    error: Option<UiError>,
}

impl<V: View> Shell<V> {
    fn fail(&mut self, event_loop: &ActiveEventLoop, err: UiError) {
        self.error = Some(err);
        event_loop.exit();
    }

    fn request_redraw(&mut self, cause: Cause) {
        match cause {
            Cause::Resize => self.causes.resize = true,
            Cause::Input => self.causes.input = true,
            Cause::Timer => self.causes.timer = true,
            Cause::Request => self.causes.request = true,
            Cause::Access => self.causes.access = true,
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// Notes the monitor the window is on, for the statistics. Wayland
    /// knows it only once the window is shown, so this follows resizes.
    fn note_monitor(&mut self) {
        if let Some(monitor) = self
            .window
            .as_ref()
            .and_then(|window| window.current_monitor())
        {
            self.stats.monitor = Some(MonitorInfo::of(&monitor));
        }
    }

    fn logical(&self, position: PhysicalPosition<f64>) -> Point {
        Point::new(position.x / self.scale, position.y / self.scale)
    }

    /// Tells the window whether a text field has the focus and where its
    /// caret is, when that changed.
    fn update_ime(&mut self) {
        let Some(window) = &self.window else {
            return;
        };
        let area = self.view.ime_area();
        if area == self.ime {
            return;
        }
        if area.is_some() != self.ime.is_some() {
            window.set_ime_allowed(area.is_some());
        }
        if let Some(area) = area {
            window.set_ime_cursor_area(
                LogicalPosition::new(area.x0, area.y0),
                LogicalSize::new(area.width(), area.height()),
            );
        }
        self.ime = area;
    }

    /// Sends the view's accessibility tree, if a screen reader listens.
    fn update_access(&mut self) {
        let Some(adapter) = &mut self.access else {
            return;
        };
        let (view, title, scale) = (&mut self.view, &self.options.title, self.scale);
        let initial = self.access_initial;
        let mut sent = false;
        adapter.update_if_active(|| {
            sent = true;
            let mut tree = AccessTree::default();
            view.accessibility(&mut tree);
            tree.into_update(title, scale, initial)
        });
        if sent {
            self.access_initial = false;
        }
    }

    fn deliver(&mut self, input: Input) {
        if self.view.input(&input) {
            self.request_redraw(Cause::Input);
        }
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        let Some(window) = self.window.clone() else {
            return;
        };
        let start = Instant::now();
        let physical = self.physical;
        let size = Size::new(
            f64::from(physical.width) / self.scale,
            f64::from(physical.height) / self.scale,
        );
        self.scene.clear();
        SCALE_FACTOR.with(|scale| scale.set(self.scale));
        self.view.paint(&mut self.scene, size);
        let paint = start.elapsed();
        if self.renderer.is_none() {
            let gpu = self
                .gpu
                .take()
                .unwrap_or_else(|| GpuStart::spawn(&self.options.gpu));
            let opening = Instant::now();
            match WindowRenderer::with_start(gpu, window.clone(), physical.width, physical.height) {
                Ok(renderer) => {
                    self.stats.gpu_opened = Some(opening.elapsed());
                    self.stats.gpu_startup = Some(*renderer.startup());
                    self.stats.gpu = Some(renderer.info().clone());
                    self.view.opened(renderer.info());
                    self.renderer = Some(renderer);
                    // The view learned about the GPU after this frame's
                    // layout: one more frame shows what it does with it.
                    self.request_redraw(Cause::Request);
                }
                Err(err) => return self.fail(event_loop, err.into()),
            }
        }
        let Some(renderer) = &mut self.renderer else {
            return;
        };
        let outcome = renderer.render(
            &self.scene,
            self.options.background,
            Affine::scale(self.scale),
        );
        let render = match outcome {
            Ok(FrameOutcome::Presented(timing)) => timing,
            Ok(FrameOutcome::Skipped) => {
                window.request_redraw(); // try again when the surface is ready
                return;
            }
            Err(err) => {
                self.fail(event_loop, err.into());
                return;
            }
        };
        self.stats.frames += 1;
        let mut causes = std::mem::take(&mut self.causes);
        if causes == FrameCauses::default() {
            causes.system = true;
        }
        let now = Instant::now();
        if self.stats.first_frame.is_none() {
            self.stats.first_frame = Some(now - self.started);
            self.note_monitor();
        }
        let frame = FrameInfo {
            number: self.stats.frames,
            paint,
            render,
            at: now - self.started,
            scale: self.scale,
            physical_size: (physical.width, physical.height),
            causes,
        };
        if self.view.presented(&frame) {
            self.request_redraw(Cause::Request);
        }
        // What is on screen changed: tell the screen reader.
        self.update_access();
    }
}

impl<V: View> ApplicationHandler<UserEvent> for Shell<V> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        // Invisible until the accessibility adapter is attached, as
        // AccessKit requires (on Windows and macOS).
        let attributes = Window::default_attributes()
            .with_visible(false)
            .with_title(self.options.title.clone())
            .with_inner_size(LogicalSize::new(
                self.options.size.width,
                self.options.size.height,
            ))
            .with_fullscreen(
                self.options
                    .fullscreen
                    .then_some(winit::window::Fullscreen::Borderless(None)),
            );
        let creating = Instant::now();
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(err) => return self.fail(event_loop, UiError::Window(err.to_string())),
        };
        self.stats.window_created = Some(creating.elapsed());
        self.scale = window.scale_factor();
        self.physical = window.inner_size();
        // The renderer comes with the first frame, after its layout: the
        // GPU keeps opening meanwhile.
        self.access = Some(accesskit_winit::Adapter::with_event_loop_proxy(
            event_loop,
            &window,
            self.proxy.clone(),
        ));
        window.set_visible(true);
        window.request_redraw();
        self.window = Some(window);
    }

    fn user_event(&mut self, _: &ActiveEventLoop, event: UserEvent) {
        let UserEvent::Access(event) = event;
        match event.window_event {
            accesskit_winit::WindowEvent::InitialTreeRequested => {
                self.access_initial = true;
                self.update_access();
            }
            accesskit_winit::WindowEvent::ActionRequested(request) => {
                if self.view.accessibility_action(&request) {
                    self.request_redraw(Cause::Access);
                }
                self.update_access();
            }
            accesskit_winit::WindowEvent::AccessibilityDeactivated => {}
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        if let (Some(access), Some(window)) = (&mut self.access, &self.window) {
            access.process_event(window, &event);
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                // Compositors repeat the size (Hyprland on focus changes):
                // nothing to draw then.
                if size == self.physical {
                    return;
                }
                self.physical = size;
                if let Some(renderer) = &mut self.renderer {
                    renderer.resize(size.width, size.height);
                }
                self.note_monitor();
                self.request_redraw(Cause::Resize);
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                // A move to a monitor with another scale (criterion 5). The
                // new physical size follows as a `Resized` event.
                self.scale = scale_factor;
                self.note_monitor();
                self.request_redraw(Cause::Resize);
            }
            WindowEvent::RedrawRequested => self.redraw(event_loop),
            WindowEvent::CursorMoved { position, .. } => {
                self.pointer = self.logical(position);
                self.deliver(Input::PointerMoved(self.pointer));
            }
            WindowEvent::CursorLeft { .. } => self.deliver(Input::PointerLeft),
            WindowEvent::MouseInput { state, button, .. } => {
                let pressed = state == ElementState::Pressed;
                let clicks = self.clicks.count(button, pressed, self.pointer);
                let input = Input::PointerButton {
                    primary: button == MouseButton::Left,
                    pressed,
                    position: self.pointer,
                    clicks,
                    modifiers: self.modifiers,
                };
                self.deliver(input);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let delta = match delta {
                    // One wheel notch scrolls about three lines.
                    MouseScrollDelta::LineDelta(x, y) => {
                        Vec2::new(f64::from(x), f64::from(y)) * 48.0
                    }
                    MouseScrollDelta::PixelDelta(p) => Vec2::new(p.x, p.y) / self.scale,
                };
                let input = Input::Scroll {
                    delta,
                    position: self.pointer,
                    modifiers: self.modifiers,
                };
                self.deliver(input);
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                let state = modifiers.state();
                self.modifiers = Modifiers {
                    shift: state.shift_key(),
                    ctrl: state.control_key(),
                    alt: state.alt_key(),
                    meta: state.super_key(),
                };
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let input = key_input(&event, self.modifiers);
                self.deliver(input);
            }
            WindowEvent::Ime(ime) => {
                let ime = match ime {
                    winit::event::Ime::Enabled => Ime::Enabled,
                    winit::event::Ime::Preedit(text, cursor) => Ime::Preedit { text, cursor },
                    winit::event::Ime::Commit(text) => Ime::Commit(text),
                    winit::event::Ime::Disabled => Ime::Disabled,
                };
                self.deliver(Input::Ime(ime));
            }
            WindowEvent::Focused(focused) => {
                if !focused {
                    // Keys released elsewhere never reach us.
                    self.modifiers = Modifiers::default();
                }
                self.deliver(Input::WindowFocus(focused));
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        let mut wake_at = self.view.wake_at();
        if wake_at.is_some_and(|at| at <= now) {
            if self.view.wake(now) {
                self.request_redraw(Cause::Timer);
            }
            wake_at = self.view.wake_at().filter(|at| *at > now);
        }
        // After the timer, which may have asked for a frame too.
        if FRAME_REQUESTED.with(|requested| requested.replace(false)) {
            self.request_redraw(Cause::Request);
        }
        // After the timer: it may have ended a scripted run.
        if self.view.should_close() {
            event_loop.exit();
            return;
        }
        self.update_ime();
        // Sleep until input arrives or the view's timer runs out: nothing
        // is drawn unless something changed (gate criterion 6).
        event_loop.set_control_flow(match wake_at {
            Some(at) => ControlFlow::WaitUntil(at),
            None => ControlFlow::Wait,
        });
    }
}

/// Translates a winit key event.
fn key_input(event: &KeyEvent, modifiers: Modifiers) -> Input {
    let key = match &event.logical_key {
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
    };
    // Shortcuts don't type. Ctrl + Alt is AltGr on Windows, which does.
    let shortcut = (modifiers.ctrl && !modifiers.alt) || modifiers.meta;
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
