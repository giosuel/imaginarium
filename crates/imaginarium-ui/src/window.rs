// SPDX-License-Identifier: AGPL-3.0-or-later

//! A window and its frame pipeline: winit events in, one Vello scene per
//! frame out, and no frame at all when nothing changed.

use std::cell::Cell;
use std::sync::Arc;
use std::time::{Duration, Instant};

use imaginarium_render::kurbo::{Affine, Point, Size, Vec2};
use imaginarium_render::peniko::Color;
use imaginarium_render::{
    FrameOutcome, FrameTiming, GpuInfo, GpuOptions, RenderError, Scene, WindowRenderer,
};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
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
}

impl Default for WindowOptions {
    fn default() -> Self {
        Self {
            title: "Imaginarium".into(),
            size: Size::new(1280.0, 800.0),
            gpu: GpuOptions::default(),
            background: Color::WHITE,
        }
    }
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
    },
    /// The wheel or touchpad scrolled by `delta` logical pixels.
    Scroll {
        /// How far (positive: content moves right and down).
        delta: Vec2,
        /// Where the pointer is.
        position: Point,
        /// Whether the platform's zoom modifier (Ctrl, ⌘ on macOS) is held.
        zoom: bool,
    },
    /// A key went down or up. `key` is its logical name ("a", "Enter",
    /// "ArrowLeft", …).
    Key {
        /// The key.
        key: String,
        /// Down or up.
        pressed: bool,
    },
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
}

/// What a window shows. Until the widget tree (M2.4), views implement this
/// directly.
pub trait View {
    /// Draws the content into `scene`, in logical pixels, for a window of
    /// `size` logical pixels.
    fn paint(&mut self, scene: &mut Scene, size: Size);

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
}

impl<V: View + ?Sized> View for &mut V {
    fn paint(&mut self, scene: &mut Scene, size: Size) {
        (**self).paint(scene, size);
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
}

/// What a run of the window did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RunStats {
    /// Frames presented.
    pub frames: u64,
    /// From the call to [`run`] until the first frame was presented.
    pub first_frame: Option<Duration>,
    /// The GPU that drew them.
    pub gpu: Option<GpuInfo>,
}

thread_local! {
    static FRAME_REQUESTED: Cell<bool> = const { Cell::new(false) };
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
    let event_loop = EventLoop::new().map_err(|err| UiError::EventLoop(err.to_string()))?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut shell = Shell {
        options,
        view,
        window: None,
        renderer: None,
        scene: Scene::new(),
        scale: 1.0,
        physical: PhysicalSize::new(0, 0),
        pointer: Point::ZERO,
        zoom_modifier: false,
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
    scene: Scene,
    scale: f64,
    /// The surface size in physical pixels, from `Resized` (asking the
    /// window costs a round trip to the X server).
    physical: PhysicalSize<u32>,
    pointer: Point,
    zoom_modifier: bool,
    started: Instant,
    stats: RunStats,
    error: Option<UiError>,
}

impl<V: View> Shell<V> {
    fn fail(&mut self, event_loop: &ActiveEventLoop, err: UiError) {
        self.error = Some(err);
        event_loop.exit();
    }

    fn request_redraw(&self) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn logical(&self, position: PhysicalPosition<f64>) -> Point {
        Point::new(position.x / self.scale, position.y / self.scale)
    }

    fn deliver(&mut self, input: Input) {
        if self.view.input(&input) {
            self.request_redraw();
        }
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        let (Some(window), Some(renderer)) = (&self.window, &mut self.renderer) else {
            return;
        };
        let start = Instant::now();
        let physical = self.physical;
        let size = Size::new(
            f64::from(physical.width) / self.scale,
            f64::from(physical.height) / self.scale,
        );
        self.scene.clear();
        self.view.paint(&mut self.scene, size);
        let paint = start.elapsed();
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
        let now = Instant::now();
        if self.stats.first_frame.is_none() {
            self.stats.first_frame = Some(now - self.started);
        }
        let frame = FrameInfo {
            number: self.stats.frames,
            paint,
            render,
            at: now - self.started,
            scale: self.scale,
            physical_size: (physical.width, physical.height),
        };
        if self.view.presented(&frame) {
            self.request_redraw();
        }
    }
}

impl<V: View> ApplicationHandler for Shell<V> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title(self.options.title.clone())
            .with_inner_size(LogicalSize::new(
                self.options.size.width,
                self.options.size.height,
            ));
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(err) => return self.fail(event_loop, UiError::Window(err.to_string())),
        };
        self.scale = window.scale_factor();
        let size = window.inner_size();
        self.physical = size;
        match WindowRenderer::new(window.clone(), size.width, size.height, &self.options.gpu) {
            Ok(renderer) => {
                self.stats.gpu = Some(renderer.info().clone());
                self.renderer = Some(renderer);
            }
            Err(err) => return self.fail(event_loop, err.into()),
        }
        window.request_redraw();
        self.window = Some(window);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                self.physical = size;
                if let Some(renderer) = &mut self.renderer {
                    renderer.resize(size.width, size.height);
                }
                self.request_redraw();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                // A move to a monitor with another scale (criterion 5). The
                // new physical size follows as a `Resized` event.
                self.scale = scale_factor;
                self.request_redraw();
            }
            WindowEvent::RedrawRequested => self.redraw(event_loop),
            WindowEvent::CursorMoved { position, .. } => {
                self.pointer = self.logical(position);
                self.deliver(Input::PointerMoved(self.pointer));
            }
            WindowEvent::CursorLeft { .. } => self.deliver(Input::PointerLeft),
            WindowEvent::MouseInput { state, button, .. } => {
                let input = Input::PointerButton {
                    primary: button == MouseButton::Left,
                    pressed: state == ElementState::Pressed,
                    position: self.pointer,
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
                    zoom: self.zoom_modifier,
                };
                self.deliver(input);
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                let state = modifiers.state();
                self.zoom_modifier = if cfg!(target_os = "macos") {
                    state.super_key()
                } else {
                    state.control_key()
                };
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let key = match &event.logical_key {
                    winit::keyboard::Key::Character(text) => text.to_string(),
                    winit::keyboard::Key::Named(named) => format!("{named:?}"),
                    other => format!("{other:?}"),
                };
                self.deliver(Input::Key {
                    key,
                    pressed: event.state == ElementState::Pressed,
                });
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if FRAME_REQUESTED.with(|requested| requested.replace(false)) {
            self.request_redraw();
        }
        let now = Instant::now();
        let mut wake_at = self.view.wake_at();
        if wake_at.is_some_and(|at| at <= now) {
            if self.view.wake(now) {
                self.request_redraw();
            }
            wake_at = self.view.wake_at().filter(|at| *at > now);
        }
        // After the timer: it may have ended a scripted run.
        if self.view.should_close() {
            event_loop.exit();
            return;
        }
        // Sleep until input arrives or the view's timer runs out: nothing
        // is drawn unless something changed (gate criterion 6).
        event_loop.set_control_flow(match wake_at {
            Some(at) => ControlFlow::WaitUntil(at),
            None => ControlFlow::Wait,
        });
    }
}
