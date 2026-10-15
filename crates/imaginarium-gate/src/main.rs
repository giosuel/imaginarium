// SPDX-License-Identifier: AGPL-3.0-or-later

//! `imaginarium-gate`: the M2 gate app (ADR-0007).
//!
//! # Responsibility
//! - Show realistic content in the M2 UI toolkit (a board of cards, later
//!   text fields, a palette and the tiled canvas) and measure the gate
//!   criteria on the machine it runs on.
//! - Write the measurements to `gate-report.json`, for the decision whether
//!   the toolkit stays or GPUI replaces it.
//!
//! # Non-goals
//! - Not a product binary, and nothing depends on it. The gate is decided
//!   (we keep the toolkit); the binary measures the canvas work of M2.8,
//!   then is folded into a benchmark or deleted.
//!
//! # Measurements
//! - Cold start: process start (`/proc/self/stat`) and `main` until the
//!   first presented frame, and loading the fonts.
//! - Scripted pan: per-frame paint (toolkit), encode and submit (renderer),
//!   swapchain wait, and frame-to-frame intervals.
//! - Scripted typing into the search field: handling a keystroke
//!   (editing, searching 2,000 cards) and the frame after it.
//! - Scripted typing into the command palette (criterion 3): the search
//!   over 2,000 commands, the widget tree's layout, paint and encode.
//! - The 10,000-shape board through the tile cache (criteria 1 and 2):
//!   scripted pan, zoom and drag, each started once no tiles are owed,
//!   with tiles rendered per frame.
//! - Idle: CPU used and frames drawn while nothing happens (criterion 6).
//! - The GPU and machine it ran on.

mod app;
mod board;
mod commands;
mod content;
mod proc;
mod shapes;
mod widgets;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use clap::{Parser, ValueEnum};
use imaginarium_render::Scene;
use imaginarium_render::kurbo::{Rect, Size, Vec2};
use imaginarium_render::peniko::Color;
use imaginarium_render::{
    CpuMode, GpuFrameTime, GpuInfo, GpuOptions, GpuStart, Power, TileWorkers,
};
use imaginarium_text::Fonts;
use imaginarium_ui::{AccessTree, FrameCauses, FrameInfo, Input, View, WindowOptions};

use crate::app::App;

/// The M2 gate app: shows test content and measures the UI toolkit.
#[derive(Parser, Debug)]
#[command(version)]
struct Args {
    /// Run the scripted measurement (pan, type, idle), write the report and
    /// close. Without it the window stays open for manual testing, and the
    /// report is written when it closes.
    #[arg(long)]
    measure: bool,
    /// Frames of scripted panning to time.
    #[arg(long, default_value_t = 300)]
    pan_frames: u32,
    /// Keystrokes of scripted typing into the search field to time.
    #[arg(long, default_value_t = 60)]
    type_keys: u32,
    /// Seconds of idling to measure.
    #[arg(long, default_value_t = 10.0)]
    idle_seconds: f64,
    /// How many cards the board shows.
    #[arg(long, default_value_t = 2_000)]
    cards: usize,
    /// How many shapes the shapes board has.
    #[arg(long, default_value_t = 10_000)]
    shapes: usize,
    /// Draw every visible item every frame instead of through cached tiles
    /// (to compare).
    #[arg(long)]
    no_tiles: bool,
    /// Frames of each scripted phase on the shapes board (pan, zoom, drag).
    #[arg(long, default_value_t = 240)]
    shape_frames: u32,
    /// How many commands the palette searches.
    #[arg(long, default_value_t = 2_000)]
    commands: usize,
    /// Keystrokes of scripted typing into the command palette to time.
    #[arg(long, default_value_t = 60)]
    palette_keys: u32,
    /// Which GPU: the power-saving one (usually integrated) or the fast one.
    #[arg(long, value_enum, default_value_t = GpuChoice::Low)]
    gpu: GpuChoice,
    /// Use the adapter whose name contains this text (see the report's
    /// `gpu.name`), overriding `--gpu`.
    #[arg(long)]
    adapter: Option<String>,
    /// Don't wait for vertical sync (measures how fast frames could be).
    #[arg(long)]
    no_vsync: bool,
    /// Fill the screen, so runs on different machines (and tiling window
    /// managers) measure the same window size.
    #[arg(long)]
    fullscreen: bool,
    /// Draw every glyph's outline each frame instead of from Vello's
    /// (experimental) glyph atlas.
    #[arg(long)]
    no_glyph_atlas: bool,
    /// Don't time the GPU's work with timestamp queries (to check that
    /// timing it changes nothing).
    #[arg(long)]
    no_gpu_times: bool,
    /// Threads that render tiles (on the CPU). 0: the frame renders them
    /// on the GPU, as before M2.8. Default: half the cores, at most 8.
    #[arg(long)]
    tile_threads: Option<usize>,
    /// Where to write the report.
    #[arg(long, default_value = "gate-report.json")]
    report: PathBuf,
    /// Close after this many frames (screenshots, smoke tests).
    #[arg(long)]
    frames: Option<u64>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum GpuChoice {
    Low,
    High,
}

/// What the scripted typing types, one character per frame.
const TYPED: &str = "quarterly planning review notes";
/// What the scripted palette typing types.
const COMMAND: &str = "insert table rotate image align left";
/// Which build this is, set when a test build is made
/// (`IMAGINARIUM_BUILD="M2.7 fix 3 (abc1234)" cargo build --release`).
const BUILD: &str = match option_env!("IMAGINARIUM_BUILD") {
    Some(build) => build,
    None => "development build",
};

/// Frames to wait at most for owed tiles (and a pending level switch)
/// before the next phase; the report says when a wait ran out.
const MAX_SETTLE_FRAMES: u32 = 600;

/// Frames of a manual session kept per board, at most (an hour at 60 Hz):
/// a session left open doesn't grow without end.
const MAX_MANUAL_FRAMES: usize = 216_000;

/// In a manual session, a frame interval longer than this is a pause.
const MANUAL_GAP: Duration = Duration::from_millis(100);

/// What follows a [`Phase::Settle`].
#[derive(Clone, Copy, Debug, PartialEq)]
enum Next {
    /// Typing into the search: after the cards pan's owed tiles, so the
    /// typing's tile count is only its own.
    Type,
    Shapes(ShapePhase),
    Idle,
}

/// Where the scripted run is.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    /// Waiting for the first frame.
    Start,
    /// Panning, with this many frames to go.
    Pan(u32),
    /// Typing into the search field, with this many keystrokes to go.
    Type(u32),
    /// Typing into the command palette, with this many keystrokes to go.
    Palette(u32),
    /// The shapes board: panning, zooming, dragging a shape, with this
    /// many frames to go.
    Shapes(ShapePhase, u32),
    /// Waiting until nothing is left to draw (tiles the time budget left
    /// over, a level switch after a zoom), so the next phase measures only
    /// its own work; with the frames waited so far and the phase that
    /// follows.
    Settle(u32, Next),
    /// Idling since the instant, with the CPU time and frame count then.
    Idle {
        since: Instant,
        cpu: Option<Duration>,
        frames: u64,
    },
    /// Measured; the window closes.
    Done,
    /// Manual testing: nothing scripted.
    Manual,
}

struct Gate {
    app: App,
    measure: bool,
    pan_frames: u32,
    type_keys: u32,
    idle: Duration,
    close_after_frames: Option<u64>,
    phase: Phase,
    frames: u64,
    first_frame: Option<FrameInfo>,
    last_frame: Option<FrameInfo>,
    last_at: Option<Duration>,
    pan: Vec<FrameSample>,
    /// Layouts built during the pan (text cache misses).
    pan_layouts: usize,
    typing: Vec<KeySample>,
    palette_keys: u32,
    palette: Vec<PaletteSample>,
    shape_frames: u32,
    shape_samples: [Vec<CanvasSample>; 3],
    /// Frames and time waited for owed tiles before each shapes phase.
    settled: [Option<(u32, Duration)>; 3],
    /// The same before the typing.
    settled_typing: Option<(u32, Duration)>,
    settle_started: Option<Instant>,
    /// Tiles rendered during the cards pan.
    pan_canvas: Vec<CanvasSample>,
    dragged: Option<usize>,
    /// The handling time of the keystroke whose frame comes next.
    pending_key: Option<(Duration, Duration)>,
    typed: usize,
    idle_result: Option<IdleResult>,
    inputs: u64,
    inputs_during_idle: u64,
    /// Every frame of a manual session, by board (cards, shapes).
    manual: [Vec<CanvasSample>; 2],
    /// Draws of textures that didn't exist (skipped; a tile cache bug).
    missing_textures: u64,
    /// The GPU's time per frame, by frame number (they come back a few
    /// frames late).
    gpu_times: std::collections::HashMap<u64, GpuFrameTime>,
    /// Why the frames during the idle phase were drawn.
    idle_causes: CauseCounts,
    /// Every frame's interval and parts, for the run's longest gaps.
    gaps: Vec<Gap>,
    /// What happened to the window's surface, by phase.
    surface: Vec<(&'static str, SurfaceCounts)>,
}

/// A frame of any phase: the time since the one before, and its parts.
#[derive(Clone, Copy, Debug)]
struct Gap {
    phase: &'static str,
    number: u64,
    interval: Duration,
    paint: Duration,
    textures: Duration,
    acquire: Duration,
    encode: Duration,
    submit: Duration,
    /// The surface was set up again first (part of `acquire`).
    reconfigured: bool,
    /// Frames skipped since the one before.
    skipped_before: u32,
}

/// What happened to the window's surface in a phase: textures the driver
/// called suboptimal, set-ups again before a frame, skipped frames.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct SurfaceCounts {
    suboptimal: u64,
    reconfigured: u64,
    skipped: u64,
}

impl Phase {
    /// The phase's name in the report.
    fn name(&self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Pan(_) => "cards pan",
            Self::Type(_) => "typing",
            Self::Palette(_) => "palette",
            Self::Shapes(ShapePhase::Pan, _) => "shapes pan",
            Self::Shapes(ShapePhase::Zoom, _) => "shapes zoom",
            Self::Shapes(ShapePhase::Drag, _) => "shapes drag",
            Self::Settle(..) => "settle",
            Self::Idle { .. } => "idle",
            Self::Done => "done",
            Self::Manual => "manual",
        }
    }
}

/// Frames by cause (a frame can have several).
#[derive(Clone, Copy, Debug, Default)]
struct CauseCounts {
    system: u64,
    resize: u64,
    input: u64,
    timer: u64,
    request: u64,
    access: u64,
    /// Frames the program caused on its own (a timer or a request).
    by_itself: u64,
}

impl CauseCounts {
    fn add(&mut self, causes: &FrameCauses) {
        self.system += u64::from(causes.system);
        self.resize += u64::from(causes.resize);
        self.input += u64::from(causes.input);
        self.timer += u64::from(causes.timer);
        self.request += u64::from(causes.request);
        self.access += u64::from(causes.access);
        self.by_itself += u64::from(causes.by_itself());
    }
}

#[derive(Clone, Copy, Debug)]
struct FrameSample {
    paint: Duration,
    acquire: Duration,
    encode: Duration,
    submit: Duration,
    interval: Duration,
}

/// One scripted keystroke: handling it, then the frame showing it.
#[derive(Clone, Copy, Debug)]
struct KeySample {
    handle: Duration,
    search: Duration,
    paint: Duration,
    encode: Duration,
    /// Tiles of the board the frame rendered (the search must not make
    /// any render again).
    rendered: usize,
}

/// The scripted phases on the shapes board.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ShapePhase {
    Pan,
    Zoom,
    Drag,
}

/// One frame of a canvas phase.
#[derive(Clone, Copy, Debug, Default)]
struct CanvasSample {
    /// The work on the UI thread: paint, texture renders, encode, submit
    /// without a present that blocks.
    frame: Duration,
    paint: Duration,
    textures: Duration,
    encode: Duration,
    /// Submitting and presenting.
    submit: Duration,
    /// The wait for the window's texture (vertical sync, or the GPU).
    acquire: Duration,
    /// Presenting (of `submit`; blocks on some systems, a wait that
    /// `frame` leaves out).
    present: Duration,
    interval: Duration,
    rendered: usize,
    owed: usize,
    /// Of `owed`: tiles with nothing to stand in for them, left blank.
    blank: usize,
    items: usize,
    /// The surface size in physical pixels.
    size: (u32, u32),
    /// The frame's number, for its GPU time.
    number: u64,
    /// Of `textures`: creating textures, and submitting the renders.
    textures_create: Duration,
    textures_submit: Duration,
    /// With tile threads: tiles back from them, still with them after the
    /// frame, and their time for those back.
    arrived: usize,
    in_flight: usize,
    /// Of `in_flight`: back, waiting for a frame with time to upload them.
    parked: usize,
    worker_time: Duration,
    /// Painting the tiles' items into their scenes (of `paint`).
    build: Duration,
    /// Waiting for tiles nothing stood in for (of `paint`).
    waited: Duration,
    /// Of `textures`: uploading tiles, and tile pages made in the frame.
    textures_upload: Duration,
    pages_made: u32,
    /// Tile pages being made on their thread as the frame's uploads began.
    pages_coming: u32,
    pages: u32,
}

/// One scripted palette keystroke and the frame after it.
#[derive(Clone, Copy, Debug)]
struct PaletteSample {
    handle: Duration,
    search: Duration,
    layout: Duration,
    paint: Duration,
    encode: Duration,
    repainted: usize,
}

#[derive(Clone, Copy, Debug)]
struct IdleResult {
    wall: Duration,
    cpu: Option<Duration>,
    frames: u64,
}

impl Gate {
    fn start_idle(&mut self) {
        self.phase = Phase::Idle {
            since: Instant::now(),
            cpu: proc::cpu_time(),
            frames: self.frames,
        };
    }

    /// Types the next character of [`TYPED`] (starting over when done).
    fn type_key(&mut self) {
        let chars: Vec<char> = TYPED.chars().collect();
        if self.typed > 0 && self.typed.is_multiple_of(chars.len()) {
            self.app.clear_search();
        }
        let c = chars[self.typed % chars.len()];
        let start = Instant::now();
        self.app.type_text(&c.to_string());
        self.pending_key = Some((start.elapsed(), self.app.last_search.get()));
        self.typed += 1;
    }

    /// Types the next character of [`COMMAND`] into the palette.
    fn palette_key(&mut self) {
        let chars: Vec<char> = COMMAND.chars().collect();
        if self.typed > 0 && self.typed.is_multiple_of(chars.len()) {
            self.app.close_palette();
        }
        let c = chars[self.typed % chars.len()];
        let start = Instant::now();
        self.app.type_command(&c.to_string());
        self.pending_key = Some((start.elapsed(), Duration::ZERO));
        self.typed += 1;
    }

    fn count_surface(&mut self, frame: &FrameInfo) {
        let phase = self.phase.name();
        let at = self
            .surface
            .iter()
            .position(|(name, _)| *name == phase)
            .unwrap_or_else(|| {
                self.surface.push((phase, SurfaceCounts::default()));
                self.surface.len() - 1
            });
        let counts = &mut self.surface[at].1;
        counts.suboptimal += u64::from(frame.render.suboptimal);
        counts.reconfigured += u64::from(frame.render.reconfigured);
        counts.skipped += u64::from(frame.render.skipped_before);
    }

    fn canvas_sample(&self, frame: &FrameInfo, interval: Duration) -> CanvasSample {
        let canvas = self.app.board.borrow().last_canvas.clone();
        CanvasSample {
            // Work on the UI thread: a present that blocks (Mesa's
            // software GPU on X11) is a wait, as the acquire is.
            frame: frame.paint
                + frame.render.textures
                + frame.render.encode
                + frame.render.submit.saturating_sub(frame.render.present),
            paint: frame.paint,
            textures: frame.render.textures,
            encode: frame.render.encode,
            submit: frame.render.submit,
            acquire: frame.render.acquire,
            present: frame.render.present,
            interval,
            rendered: canvas.rendered,
            owed: canvas.owed,
            blank: canvas.blank,
            items: canvas.items_painted,
            size: frame.physical_size,
            number: frame.number,
            textures_create: frame.render.textures_create,
            textures_submit: frame.render.textures_submit,
            arrived: canvas.arrived,
            in_flight: canvas.in_flight,
            parked: canvas.parked,
            worker_time: canvas.worker_time,
            build: canvas.build,
            waited: canvas.waited,
            textures_upload: frame.render.textures_upload,
            pages_made: frame.render.tile_pages_made,
            pages_coming: frame.render.tile_pages_coming,
            pages: frame.render.tile_pages,
        }
    }

    fn start_shapes(&mut self) {
        if self.shape_frames == 0 {
            self.settle(Next::Idle);
            return;
        }
        self.app.set_mode(crate::board::Mode::Shapes);
        self.app.reset_view();
        self.settle(Next::Shapes(ShapePhase::Pan));
    }

    fn settle(&mut self, next: Next) {
        self.phase = Phase::Settle(0, next);
        self.settle_started = Some(Instant::now());
    }

    /// Moves the shapes board on for the next frame of a phase.
    fn shape_step(&mut self, which: ShapePhase, left: u32) {
        match which {
            ShapePhase::Pan => self.app.pan_board(Vec2::new(-9.0, -4.0)),
            ShapePhase::Zoom => {
                // Out for the first half, back in for the second.
                let out = left > self.shape_frames / 2;
                self.app.zoom_board(if out { 0.985 } else { 1.0 / 0.985 });
            }
            ShapePhase::Drag => {
                let mut board = self.app.board.borrow_mut();
                if self.dragged.is_none() {
                    let index = board.shape_in_view().unwrap_or(0);
                    board.begin_drag(index);
                    self.dragged = Some(index);
                }
                if left > 1 {
                    // A circle, so the shape crosses tile edges.
                    let t = f64::from(left) / 12.0;
                    board.drag_by(Vec2::new(t.cos() * 5.0, t.sin() * 5.0));
                } else {
                    board.drop_lift();
                }
                drop(board);
                self.app.ui.refresh(self.app.board_id());
            }
        }
    }

    fn after_typing(&mut self) -> bool {
        self.app.clear_search();
        if self.palette_keys > 0 {
            self.typed = 0;
            self.phase = Phase::Palette(self.palette_keys);
            self.palette_key();
        } else {
            self.start_shapes();
        }
        true
    }

    fn after_pan(&mut self) -> bool {
        if self.type_keys > 0 {
            self.settle(Next::Type);
            true
        } else {
            // On to the palette and the shapes, whatever is left.
            self.after_typing()
        }
    }
}

impl Gate {
    /// The script's state for `args`, showing `app`.
    fn new(args: &Args, app: App) -> anyhow::Result<Self> {
        let idle = Duration::try_from_secs_f64(args.idle_seconds.max(0.0))
            .context("--idle-seconds must be a number of seconds")?;
        Ok(Self {
            app,
            palette_keys: args.palette_keys,
            palette: Vec::new(),
            shape_frames: args.shape_frames,
            shape_samples: [Vec::new(), Vec::new(), Vec::new()],
            settled: [None; 3],
            settled_typing: None,
            settle_started: None,
            pan_canvas: Vec::new(),
            dragged: None,
            measure: args.measure,
            pan_frames: args.pan_frames,
            type_keys: args.type_keys,
            idle,
            close_after_frames: args.frames,
            phase: Phase::Start,
            frames: 0,
            first_frame: None,
            last_frame: None,
            last_at: None,
            pan: Vec::new(),
            pan_layouts: 0,
            typing: Vec::new(),
            pending_key: None,
            typed: 0,
            idle_result: None,
            inputs: 0,
            inputs_during_idle: 0,
            manual: [Vec::new(), Vec::new()],
            missing_textures: 0,
            gpu_times: std::collections::HashMap::new(),
            idle_causes: CauseCounts::default(),
            gaps: Vec::new(),
            surface: Vec::new(),
        })
    }
}

impl View for Gate {
    fn paint(&mut self, scene: &mut Scene, size: Size) {
        self.app.paint(scene, size);
    }

    fn opened(&mut self, gpu: &GpuInfo) {
        self.app.set_gpu(format!("{} ({})", gpu.name, gpu.backend));
    }

    fn input(&mut self, input: &Input) -> bool {
        self.inputs += 1;
        if matches!(self.phase, Phase::Idle { .. }) {
            self.inputs_during_idle += 1;
        }
        // Scripted phases ignore input, so a mouse resting on the window
        // doesn't skew them.
        match self.phase {
            Phase::Manual => self.app.input(input),
            _ => false,
        }
    }

    fn presented(&mut self, frame: &FrameInfo) -> bool {
        self.frames = frame.number;
        let interval = self.last_at.map_or(Duration::ZERO, |last| frame.at - last);
        self.last_at = Some(frame.at);
        if self.measure && !interval.is_zero() {
            self.gaps.push(Gap {
                phase: self.phase.name(),
                number: frame.number,
                interval,
                paint: frame.paint,
                textures: frame.render.textures,
                acquire: frame.render.acquire,
                encode: frame.render.encode,
                submit: frame.render.submit,
                reconfigured: frame.render.reconfigured,
                skipped_before: frame.render.skipped_before,
            });
        }
        self.count_surface(frame);
        self.app.set_frame(frame.paint, frame.render.encode);
        self.app.board.borrow_mut().note_render_time(
            frame.render.textures,
            // Work, not waits: a blocking present isn't tile time lost.
            frame.paint
                + frame.render.encode
                + frame.render.submit.saturating_sub(frame.render.present),
        );
        self.missing_textures += u64::from(frame.render.missing_textures);
        if let Some(gpu) = frame.render.gpu
            && self.gpu_times.len() < 2 * MAX_MANUAL_FRAMES
        {
            self.gpu_times.insert(gpu.frame, gpu);
        }
        if matches!(self.phase, Phase::Idle { .. }) {
            self.idle_causes.add(&frame.causes);
        }
        if self.first_frame.is_none() {
            self.first_frame = Some(*frame);
        }
        self.last_frame = Some(*frame);
        // `--frames N`: draw N frames back to back, then close.
        if let Some(limit) = self.close_after_frames {
            return frame.number < limit;
        }
        match self.phase {
            Phase::Start if !self.measure => {
                self.phase = Phase::Manual;
                false
            }
            Phase::Manual => {
                // Frames come on demand: a long gap is a pause, not a
                // slow frame.
                let interval = if interval < MANUAL_GAP {
                    interval
                } else {
                    Duration::ZERO
                };
                let sample = self.canvas_sample(frame, interval);
                let board = self.app.board.borrow().mode() as usize;
                if self.manual[board].len() < MAX_MANUAL_FRAMES {
                    self.manual[board].push(sample);
                }
                false
            }
            Phase::Start if self.pan_frames > 0 => {
                self.phase = Phase::Pan(self.pan_frames);
                self.app.pan_board(Vec2::new(-6.0, -3.0));
                true
            }
            Phase::Start => self.after_pan(),
            Phase::Pan(left) => {
                self.pan.push(FrameSample {
                    paint: frame.paint,
                    acquire: frame.render.acquire,
                    encode: frame.render.encode,
                    submit: frame.render.submit,
                    interval,
                });
                self.pan_layouts += self.app.board.borrow().last_layouts;
                let sample = self.canvas_sample(frame, interval);
                self.pan_canvas.push(sample);
                if left > 1 {
                    self.phase = Phase::Pan(left - 1);
                    self.app.pan_board(Vec2::new(-6.0, -3.0));
                    true
                } else {
                    self.after_pan()
                }
            }
            Phase::Settle(frames, next) => {
                // The board draws again by itself while tiles are owed,
                // and wakes for a level switch after a zoom.
                let board = self.app.board.borrow();
                let busy = board.owed || board.wake_at().is_some();
                drop(board);
                if busy && frames < MAX_SETTLE_FRAMES {
                    self.phase = Phase::Settle(frames + 1, next);
                    return false;
                }
                let waited = self.settle_started.take().map(|at| at.elapsed());
                match next {
                    Next::Idle => {
                        self.start_idle();
                        false
                    }
                    Next::Type => {
                        self.settled_typing = waited.map(|waited| (frames, waited));
                        self.phase = Phase::Type(self.type_keys);
                        self.type_key();
                        true
                    }
                    Next::Shapes(which) => {
                        self.settled[which as usize] = waited.map(|waited| (frames, waited));
                        self.phase = Phase::Shapes(which, self.shape_frames);
                        self.shape_step(which, self.shape_frames);
                        true
                    }
                }
            }
            Phase::Type(left) => {
                if let Some((handle, search)) = self.pending_key.take() {
                    self.typing.push(KeySample {
                        handle,
                        search,
                        paint: frame.paint,
                        encode: frame.render.encode,
                        rendered: self.app.board.borrow().last_canvas.rendered,
                    });
                }
                if left > 1 {
                    self.phase = Phase::Type(left - 1);
                    self.type_key();
                    true
                } else {
                    // Leave the field, or its caret would blink through
                    // the idle measurement.
                    self.after_typing()
                }
            }
            Phase::Palette(left) => {
                if let Some((handle, _)) = self.pending_key.take() {
                    let tree = self.app.tree_stats();
                    self.palette.push(PaletteSample {
                        handle,
                        // The memo searches when the list reads it (in
                        // layout), so it is timed there.
                        search: self.app.last_palette_search.get(),
                        layout: tree.layout.unwrap_or_default(),
                        paint: frame.paint,
                        encode: frame.render.encode,
                        repainted: tree.repainted,
                    });
                }
                if left > 1 {
                    self.phase = Phase::Palette(left - 1);
                    self.palette_key();
                } else {
                    self.app.close_palette();
                    self.start_shapes();
                }
                true
            }
            Phase::Shapes(which, left) => {
                let sample = self.canvas_sample(frame, interval);
                self.shape_samples[which as usize].push(sample);
                if left > 1 {
                    self.phase = Phase::Shapes(which, left - 1);
                    self.shape_step(which, left - 1);
                } else {
                    let next = match which {
                        ShapePhase::Pan => Some(ShapePhase::Zoom),
                        ShapePhase::Zoom => Some(ShapePhase::Drag),
                        ShapePhase::Drag => None,
                    };
                    match next {
                        Some(next) => {
                            self.app.reset_view();
                            self.settle(Next::Shapes(next));
                        }
                        None => {
                            self.app.set_mode(crate::board::Mode::Cards);
                            self.settle(Next::Idle);
                        }
                    }
                }
                true
            }
            _ => false,
        }
    }

    fn wake_at(&mut self) -> Option<Instant> {
        let deadline = match self.phase {
            Phase::Idle { since, .. } => Some(since + self.idle),
            _ => None,
        };
        match (deadline, self.app.wake_at()) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    fn wake(&mut self, now: Instant) -> bool {
        if let Phase::Idle { since, cpu, frames } = self.phase
            && now >= since + self.idle
        {
            self.idle_result = Some(IdleResult {
                wall: now - since,
                cpu: cpu
                    .zip(proc::cpu_time())
                    .map(|(start, end)| end.saturating_sub(start)),
                frames: self.frames - frames,
            });
            self.phase = Phase::Done;
            return false;
        }
        // A caret blink.
        self.app.wake(now);
        false
    }

    fn should_close(&mut self) -> bool {
        self.phase == Phase::Done
            || self
                .close_after_frames
                .is_some_and(|limit| self.frames >= limit)
    }

    fn ime_area(&mut self) -> Option<Rect> {
        self.app.ime_area()
    }

    fn accessibility(&mut self, tree: &mut AccessTree) {
        self.app.access(tree);
    }

    fn accessibility_action(&mut self, request: &accesskit::ActionRequest) -> bool {
        self.app.access_action(request)
    }
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

/// The `q` quantile of `samples`.
fn percentile(mut samples: Vec<Duration>, q: f64) -> Option<Duration> {
    if samples.is_empty() {
        return None;
    }
    samples.sort_unstable();
    Some(samples[((samples.len() - 1) as f64 * q).round() as usize])
}

/// Timings and tile counts of a canvas phase, and the frames and time
/// waited for owed tiles before it.
fn canvas_report(
    samples: &[CanvasSample],
    settled: Option<(u32, Duration)>,
    gpu: &std::collections::HashMap<u64, GpuFrameTime>,
) -> serde_json::Value {
    if samples.is_empty() {
        return serde_json::Value::Null;
    }
    let field = |f: fn(&CanvasSample) -> Duration| samples.iter().map(f).collect::<Vec<_>>();
    let counts = |f: fn(&CanvasSample) -> usize| {
        let values: Vec<usize> = samples.iter().map(f).collect();
        serde_json::json!({
            "mean": values.iter().sum::<usize>() as f64 / values.len() as f64,
            "max": values.iter().max(),
            "total": values.iter().sum::<usize>(),
        })
    };
    let arrived: usize = samples.iter().map(|s| s.arrived).sum();
    let worker_ms_per_tile = (arrived > 0)
        .then(|| millis(samples.iter().map(|s| s.worker_time).sum::<Duration>()) / arrived as f64);
    let rendered: usize = samples.iter().map(|s| s.rendered).sum();
    let build_ms_per_tile = (rendered > 0)
        .then(|| millis(samples.iter().map(|s| s.build).sum::<Duration>()) / rendered as f64);
    let upload_ms_per_tile = (arrived > 0).then(|| {
        millis(samples.iter().map(|s| s.textures_upload).sum::<Duration>()) / arrived as f64
    });
    // From the phase's first frame to the first one that `done` holds for:
    // frames, and the time from the first frame's start to that one's.
    let until = |done: fn(&CanvasSample) -> bool| {
        samples.iter().position(done).map(|index| {
            let waited: Duration = samples[1..=index].iter().map(|s| s.interval).sum();
            serde_json::json!({ "frames": index, "ms": millis(waited) })
        })
    };
    serde_json::json!({
        "frame": summary(field(|s| s.frame)),
        "frame_p99_ms": percentile(field(|s| s.frame), 0.99).map(millis),
        "paint": summary(field(|s| s.paint)),
        "textures": summary(field(|s| s.textures)),
        "encode": summary(field(|s| s.encode)),
        "acquire": summary(field(|s| s.acquire)),
        "present": summary(field(|s| s.present)),
        // The GPU's own time (timestamp queries), for the frames it came
        // back for.
        "gpu": gpu_report(samples, gpu),
        // Where the slowest tile work went: a first render per board can
        // be much slower than the rest.
        "slowest_textures": samples
            .iter()
            .enumerate()
            .max_by_key(|(_, s)| s.textures)
            .map(|(index, s)| serde_json::json!({
                "frame_in_phase": index,
                "textures_ms": millis(s.textures),
                "create_ms": millis(s.textures_create),
                "submit_ms": millis(s.textures_submit),
                "upload_ms": millis(s.textures_upload),
                "rendered": s.rendered,
                "arrived": s.arrived,
                "pages_made": s.pages_made,
                "pages_coming": s.pages_coming,
                "gpu_textures_ms": gpu.get(&s.number).map(|t| millis(t.textures)),
            })),
        // Where the slowest frames' time went, slowest first: which part
        // of the frame to look at when the p99 is over budget.
        "slowest_frames": slowest_frames(samples, 5),
        // Zero: the first frame, or one after a pause.
        "interval": summary(samples.iter().map(|s| s.interval).filter(|i| !i.is_zero()).collect()),
        // Frames that took longer than a 60 Hz refresh on the UI thread,
        // and gaps that missed one.
        "frames_over_16ms": samples.iter().filter(|s| s.frame > Duration::from_millis(16)).count(),
        "intervals_over_20ms": samples.iter().filter(|s| s.interval > Duration::from_millis(20)).count(),
        // Tiles whose items were painted (and rendered in the frame, or
        // handed to the tile threads).
        "tiles_rendered": counts(|s| s.rendered),
        "tiles_owed": counts(|s| s.owed),
        // Owed tiles nothing stood in for (blank areas on screen), the
        // frames that had any, and when the screen was first covered (no
        // blank tile) and complete (none owed). The cards' pan starts with
        // the board opening; the shapes phases start settled.
        "tiles_blank": counts(|s| s.blank),
        "frames_with_blanks": samples.iter().filter(|s| s.blank > 0).count(),
        "covered_after": until(|s| s.blank == 0),
        "complete_after": until(|s| s.owed == 0),
        // With tile threads: tiles back and uploaded, tiles still with
        // them, the threads' time per tile, the upload time per frame.
        "tiles_arrived": counts(|s| s.arrived),
        "tiles_in_flight": counts(|s| s.in_flight),
        // Of those: back, waiting for a frame with time to upload them.
        "tiles_parked": counts(|s| s.parked),
        "worker_ms_per_tile": worker_ms_per_tile,
        // Painting the tiles' items on the UI thread (part of `paint`).
        "tile_build": summary(field(|s| s.build)),
        "build_ms_per_tile": build_ms_per_tile,
        // Waiting in the frame for tiles that would show blank otherwise
        // (part of `paint`).
        "blank_wait": summary(field(|s| s.waited)),
        "upload": summary(field(|s| s.textures_upload)),
        "upload_ms_per_tile": upload_ms_per_tile,
        // Pages made in a frame because none was ready (each can stall
        // it), and pages at the phase's end.
        "tile_pages_made": samples.iter().map(|s| s.pages_made).sum::<u32>(),
        "tile_pages": samples.last().map(|s| s.pages),
        "items_painted": counts(|s| s.items),
        // At the phase's end: the window can change size meanwhile (a
        // compositor making it fullscreen after the first frame).
        "physical_size": samples.last().map(|s| [s.size.0, s.size.1]),
        "settled_before": settled_report(settled),
    })
}

/// The frames and time waited before a phase, and whether the wait ran
/// out (the phase then started with tiles owed).
fn settled_report(settled: Option<(u32, Duration)>) -> serde_json::Value {
    settled.map_or(serde_json::Value::Null, |(frames, waited)| {
        serde_json::json!({
            "frames": frames,
            "ms": millis(waited),
            "timed_out": frames >= MAX_SETTLE_FRAMES,
        })
    })
}

/// Criterion 1 for one phase: p99 frame time (work on the UI thread)
/// under 16 ms, and, with vertical sync, 60 frames per second or more:
/// at most 1 % of the intervals over 20 ms (a missed refresh at 60 Hz).
/// Without vertical sync frames come as fast as they can, so only the
/// p99 counts. `None` without samples.
fn criterion_1(samples: &[CanvasSample], vsync: bool) -> Option<bool> {
    let p99 = percentile(samples.iter().map(|sample| sample.frame).collect(), 0.99)?;
    let missed = samples
        .iter()
        .filter(|sample| sample.interval > Duration::from_millis(20))
        .count();
    Some(p99 < Duration::from_millis(16) && (!vsync || missed * 100 <= samples.len()))
}

/// Criterion 2: dragging re-renders only the tiles under the shape (its
/// old and new place), at most 4 per frame, past the frame that lifts
/// it. `None` when it can't tell: without tiles, or without a frame past
/// the lift.
fn criterion_2(samples: &[CanvasSample], tiles: bool) -> Option<bool> {
    (tiles && samples.len() >= 2).then(|| samples.iter().skip(1).all(|sample| sample.rendered <= 4))
}

/// How to open the GPU, from the arguments.
fn gpu_options(args: &Args) -> GpuOptions {
    GpuOptions {
        power: match args.gpu {
            GpuChoice::Low => Power::Low,
            GpuChoice::High => Power::High,
        },
        adapter: args.adapter.clone(),
        no_vsync: args.no_vsync,
        glyph_atlas: !args.no_glyph_atlas,
        gpu_times: !args.no_gpu_times,
    }
}

/// What happened to the window's surface, by phase.
fn surface_report(surface: &[(&'static str, SurfaceCounts)]) -> serde_json::Value {
    surface
        .iter()
        .map(|(phase, counts)| {
            (
                (*phase).to_owned(),
                serde_json::json!({
                    "suboptimal": counts.suboptimal,
                    "reconfigured": counts.reconfigured,
                    "skipped": counts.skipped,
                }),
            )
        })
        .collect::<serde_json::Map<_, _>>()
        .into()
}

/// The `count` longest intervals in `gaps`, longest first, with the phase
/// each frame belonged to and its parts.
fn longest_intervals(gaps: &[Gap], count: usize) -> serde_json::Value {
    let mut order: Vec<&Gap> = gaps.iter().collect();
    order.sort_by_key(|gap| std::cmp::Reverse(gap.interval));
    order
        .into_iter()
        .take(count)
        .map(|gap| {
            serde_json::json!({
                "phase": gap.phase,
                "frame": gap.number,
                "interval_ms": millis(gap.interval),
                "paint_ms": millis(gap.paint),
                "textures_ms": millis(gap.textures),
                "acquire_ms": millis(gap.acquire),
                "encode_ms": millis(gap.encode),
                "submit_ms": millis(gap.submit),
                "reconfigured": gap.reconfigured,
                "skipped_before": gap.skipped_before,
            })
        })
        .collect()
}

/// The `count` slowest frames of `samples` on the UI thread, slowest
/// first, with their parts.
fn slowest_frames(samples: &[CanvasSample], count: usize) -> serde_json::Value {
    let mut order: Vec<usize> = (0..samples.len()).collect();
    order.sort_by_key(|index| std::cmp::Reverse(samples[*index].frame));
    order
        .into_iter()
        .take(count)
        .map(|index| {
            let s = &samples[index];
            serde_json::json!({
                "frame_in_phase": index,
                "frame_ms": millis(s.frame),
                "paint_ms": millis(s.paint),
                "build_ms": millis(s.build),
                "waited_ms": millis(s.waited),
                "textures_ms": millis(s.textures),
                "upload_ms": millis(s.textures_upload),
                "encode_ms": millis(s.encode),
                "submit_ms": millis(s.submit),
                "interval_ms": millis(s.interval),
                "rendered": s.rendered,
                "arrived": s.arrived,
                "parked": s.parked,
                "owed": s.owed,
                "blank": s.blank,
                "items": s.items,
            })
        })
        .collect()
}

/// The GPU's own time (timestamp queries) for the frames of `samples` it
/// came back for.
fn gpu_report(
    samples: &[CanvasSample],
    gpu: &std::collections::HashMap<u64, GpuFrameTime>,
) -> serde_json::Value {
    let times: Vec<&GpuFrameTime> = samples.iter().filter_map(|s| gpu.get(&s.number)).collect();
    if times.is_empty() {
        return serde_json::Value::Null;
    }
    let field = |f: fn(&GpuFrameTime) -> Duration| times.iter().map(|t| f(t)).collect::<Vec<_>>();
    serde_json::json!({
        "frames": times.len(),
        "scene": summary(field(|t| t.scene)),
        "textures": summary(field(|t| t.textures)),
        "span": summary(field(|t| t.span)),
        "span_p99_ms": percentile(field(|t| t.span), 0.99).map(millis),
        "untimed_texture_renders": times.iter().map(|t| t.untimed).sum::<u32>(),
    })
}

/// Mean, median, 95th percentile and maximum, in milliseconds.
fn summary(mut samples: Vec<Duration>) -> serde_json::Value {
    if samples.is_empty() {
        return serde_json::Value::Null;
    }
    samples.sort_unstable();
    let at = |q: f64| millis(samples[((samples.len() - 1) as f64 * q).round() as usize]);
    let mean = millis(samples.iter().sum::<Duration>()) / samples.len() as f64;
    serde_json::json!({
        "count": samples.len(),
        "mean_ms": mean,
        "p50_ms": at(0.5),
        "p95_ms": at(0.95),
        "max_ms": at(1.0),
    })
}

fn main() -> anyhow::Result<()> {
    let main_started = Instant::now();
    let args = Args::parse();
    eprintln!("imaginarium-gate {BUILD}");
    let gpu = gpu_options(&args);
    // The drivers load while the fonts load and the view is built.
    let gpu_start = GpuStart::spawn(&gpu);
    let options = WindowOptions {
        title: "Imaginarium gate".into(),
        size: Size::new(1280.0, 800.0),
        gpu,
        background: Color::WHITE,
        fullscreen: args.fullscreen,
    };
    let fonts_started = Instant::now();
    let fonts = Fonts::new();
    let fonts_time = fonts_started.elapsed();
    let tile_threads = args
        .tile_threads
        .unwrap_or_else(TileWorkers::default_threads);
    let workers = (tile_threads > 0).then(|| TileWorkers::new(tile_threads, CpuMode::Fast));
    let app = App::new(
        fonts,
        args.cards,
        args.shapes,
        args.commands,
        !args.no_tiles,
        workers.clone(),
    );
    let mut gate = Gate::new(&args, app)?;
    // The time from exec to `main` (loading the binary and its libraries)
    // is measured separately from the time in `main`.
    let before_main = proc::since_exec().map(|since| since.saturating_sub(main_started.elapsed()));
    let before_run = main_started.elapsed();
    let stats = imaginarium_ui::run_with_gpu(gpu_start, options, &mut gate)
        .context("running the window")?;

    let first = gate.first_frame;
    let pan = |field: fn(&FrameSample) -> Duration| gate.pan.iter().map(field).collect::<Vec<_>>();
    let typing =
        |field: fn(&KeySample) -> Duration| gate.typing.iter().map(field).collect::<Vec<_>>();
    let palette =
        |field: fn(&PaletteSample) -> Duration| gate.palette.iter().map(field).collect::<Vec<_>>();
    let palette_search_p95 = {
        let mut searches = palette(|sample| sample.search);
        searches.sort_unstable();
        searches
            .get((searches.len().saturating_sub(1)) * 95 / 100)
            .copied()
    };
    let idle = gate.idle_result.map(|idle| {
        serde_json::json!({
            "seconds": idle.wall.as_secs_f64(),
            "cpu_seconds": idle.cpu.map(|cpu| cpu.as_secs_f64()),
            "cpu_percent": idle.cpu.map(|cpu| cpu.as_secs_f64() / idle.wall.as_secs_f64() * 100.0),
            "frames": idle.frames,
            "inputs": gate.inputs_during_idle,
            // Why they were drawn: the window system, a resize, an input,
            // a timer, a request, the screen reader.
            "frame_causes": {
                "system": gate.idle_causes.system,
                "resize": gate.idle_causes.resize,
                "input": gate.idle_causes.input,
                "timer": gate.idle_causes.timer,
                "request": gate.idle_causes.request,
                "access": gate.idle_causes.access,
            },
            "frames_by_itself": gate.idle_causes.by_itself,
            // Criterion 6: no frames of its own and (almost) no CPU while
            // idle. Frames the window system or the user asks for don't
            // count.
            "pass": gate.idle_causes.by_itself == 0 && idle.cpu.is_none_or(|cpu| cpu.as_secs_f64() / idle.wall.as_secs_f64() < 0.01),
        })
    });
    // Criterion 1 while panning and zooming 10,000 shapes; --no-vsync
    // shows the headroom.
    let vsync = !args.no_vsync;
    let criterion_1 = match (
        criterion_1(&gate.shape_samples[0], vsync),
        criterion_1(&gate.shape_samples[1], vsync),
    ) {
        (Some(pan), Some(zoom)) => Some(pan && zoom),
        _ => None,
    };
    let criterion_2 = criterion_2(&gate.shape_samples[2], !args.no_tiles);
    // The time to the first frame without the GPU's share of it: what the
    // start takes if the GPU were open at once.
    let without_gpu_setup = stats.first_frame.map(|first| {
        (before_main.unwrap_or_default() + before_run + first)
            .saturating_sub(stats.gpu_opened.unwrap_or_default())
    });
    let report = serde_json::json!({
        "gate_version": env!("CARGO_PKG_VERSION"),
        "build": BUILD,
        "milestone": "M2.9",
        "mode": if args.measure { "measure" } else { "manual" },
        "machine": proc::machine(),
        "gpu": stats.gpu.as_ref().map(|gpu| serde_json::json!({
            "name": gpu.name,
            "backend": gpu.backend,
            "device_type": gpu.device_type,
            "driver": gpu.driver,
            "requested": format!("{:?}", args.gpu).to_lowercase(),
            "vsync": !args.no_vsync,
            "glyph_atlas": !args.no_glyph_atlas,
            "gpu_times": !args.no_gpu_times,
            "gpu_frames_timed": gate.gpu_times.len(),
        })),
        // The last frame's size and scale: the first frame can come before
        // the compositor sets the final size (Hyprland tiles the window
        // first, then makes it fullscreen).
        "window": gate.last_frame.map(|frame| serde_json::json!({
            "scale": frame.scale,
            "physical_size": [frame.physical_size.0, frame.physical_size.1],
            "first_frame_physical_size": first.map(|first| [first.physical_size.0, first.physical_size.1]),
        })),
        "monitor": stats.monitor.as_ref().map(|monitor| serde_json::json!({
            "name": monitor.name,
            "physical_size": [monitor.physical_size.0, monitor.physical_size.1],
            "scale": monitor.scale,
            "refresh_hz": monitor.refresh_millihertz.map(|mhz| f64::from(mhz) / 1000.0),
        })),
        "cold_start": {
            "exec_to_main_ms": before_main.map(millis),
            "main_to_first_frame_ms": stats.first_frame.map(|first| millis(before_run + first)),
            "first_frame_paint_ms": first.map(|frame| millis(frame.paint)),
            "first_frame_encode_ms": first.map(|frame| millis(frame.render.encode)),
            "first_frame_textures_ms": first.map(|frame| millis(frame.render.textures)),
            "first_frame_submit_ms": first.map(|frame| millis(frame.render.submit)),
            "fonts_ms": millis(fonts_time),
            "window_ms": stats.window_created.map(millis),
            // The GPU opens on a thread of its own from the start of
            // `main`; `gpu_ms` is what the window waited for it and spent
            // on it (surface, renderer): its share of the time to the
            // first frame.
            "gpu_ms": stats.gpu_opened.map(millis),
            "gpu": stats.gpu_startup.map(|startup| serde_json::json!({
                "instance_ms": millis(startup.instance),
                "device_ms": millis(startup.device),
                "surface_ms": millis(startup.surface),
                "renderer_ms": millis(startup.renderer),
                "waited_ms": millis(startup.waited),
            })),
            "font_families": gate.app.fonts.family_count(),
            "without_gpu_setup_ms": without_gpu_setup.map(millis),
            // Criterion 6: cold start under 400 ms, not counting the GPU's
            // setup (owner, 2026-10-13: the drivers' time is the same for
            // any toolkit).
            "pass": without_gpu_setup.map(|time| time < Duration::from_millis(400)),
        },
        "content": {
            "cards": gate.app.board.borrow().cards(),
            "shapes": gate.app.board.borrow().shape_count(),
            "commands": gate.app.command_count,
            "tiles": !args.no_tiles,
            // 0: tiles render in the frame, on the GPU.
            "tile_threads": workers.as_ref().map_or(0, TileWorkers::threads),
        },
        "pan": {
            // The toolkit's share: the view building its scene.
            "paint": summary(pan(|sample| sample.paint)),
            // The renderer's share on the UI thread.
            "encode": summary(pan(|sample| sample.encode)),
            "submit": summary(pan(|sample| sample.submit)),
            // Blocked on the swapchain: vertical sync, or a GPU that is
            // behind.
            "acquire": summary(pan(|sample| sample.acquire)),
            // Frame to frame: with vertical sync, the refresh interval
            // when everything keeps up.
            "interval": summary(gate.pan.iter().skip(1).map(|sample| sample.interval).collect()),
            // Card text laid out during the pan (cache misses).
            "layouts": gate.pan_layouts,
            "canvas": canvas_report(&gate.pan_canvas, None, &gate.gpu_times),
        },
        // A manual session's frames, by board.
        "manual": {
            "cards": canvas_report(&gate.manual[0], None, &gate.gpu_times),
            "shapes": canvas_report(&gate.manual[1], None, &gate.gpu_times),
        },
        "missing_textures": gate.missing_textures,
        // Textures the driver called suboptimal, set-ups of the surface
        // before a frame (part of its `acquire`), skipped frames.
        "surface": surface_report(&gate.surface),
        "shapes": {
            "pan": canvas_report(&gate.shape_samples[0], gate.settled[0], &gate.gpu_times),
            "zoom": canvas_report(&gate.shape_samples[1], gate.settled[1], &gate.gpu_times),
            "drag": canvas_report(&gate.shape_samples[2], gate.settled[2], &gate.gpu_times),
            "criterion_1_pass": criterion_1,
            "criterion_2_pass": criterion_2,
        },
        "typing": {
            // Editing the field and searching every card's text.
            "handle": summary(typing(|sample| sample.handle)),
            "search": summary(typing(|sample| sample.search)),
            "paint": summary(typing(|sample| sample.paint)),
            "encode": summary(typing(|sample| sample.encode)),
            // Searching dims the cards it misses over the tiles, so no
            // tile renders again (the pan's owed tiles are done before).
            "tiles_rendered": gate.typing.iter().map(|sample| sample.rendered).sum::<usize>(),
            "settled_before": settled_report(gate.settled_typing),
        },
        "palette": {
            // Criterion 3: the search over all commands per keystroke.
            "search": summary(palette(|sample| sample.search)),
            "handle": summary(palette(|sample| sample.handle)),
            // The widget tree's share: Taffy layout, and painting the
            // widgets that changed.
            "layout": summary(palette(|sample| sample.layout)),
            "paint": summary(palette(|sample| sample.paint)),
            "encode": summary(palette(|sample| sample.encode)),
            "repainted_widgets_mean": if gate.palette.is_empty() { 0.0 } else {
                gate.palette.iter().map(|sample| sample.repainted as f64).sum::<f64>() / gate.palette.len() as f64
            },
            "widgets": gate.app.tree_stats().widgets,
            "pass": palette_search_p95.map(|p95| p95 < Duration::from_millis(1)),
        },
        "idle": idle,
        // The run's longest frame intervals, in any phase (including the
        // unmeasured ones between phases): where it froze, and on what.
        "longest_intervals": longest_intervals(&gate.gaps, 8),
        "frames": stats.frames,
        "inputs": gate.inputs,
        "peak_rss_bytes": proc::peak_rss_bytes(),
    });
    let text = serde_json::to_string_pretty(&report)?;
    std::fs::write(&args.report, format!("{text}\n"))
        .with_context(|| format!("writing {}", args.report.display()))?;
    println!("{text}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use imaginarium_render::FrameTiming;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn sample(frame: u64, interval: u64) -> CanvasSample {
        CanvasSample {
            frame: ms(frame),
            interval: ms(interval),
            ..CanvasSample::default()
        }
    }

    #[test]
    fn percentiles_of_one_or_two_samples() {
        assert_eq!(percentile(Vec::new(), 0.99), None);
        assert_eq!(percentile(vec![ms(5)], 0.99), Some(ms(5)));
        assert_eq!(percentile(vec![ms(9), ms(5)], 0.99), Some(ms(9)));
        assert_eq!(percentile(vec![ms(9), ms(5)], 0.5), Some(ms(9)));
        assert_eq!(summary(Vec::new()), serde_json::Value::Null);
        assert_eq!(summary(vec![ms(4)])["p95_ms"], 4.0);
        // p99 of 240 is the third largest.
        let mut many: Vec<Duration> = (1..=240).map(ms).collect();
        many.reverse();
        assert_eq!(percentile(many, 0.99), Some(ms(238)));
    }

    #[test]
    fn criterion_1_needs_short_frames_and_with_vsync_sixty_per_second() {
        let fast: Vec<CanvasSample> = (0..240).map(|_| sample(5, 16)).collect();
        assert_eq!(criterion_1(&fast, true), Some(true));
        // Short frames, but a GPU that doesn't keep up: 30 per second.
        let behind: Vec<CanvasSample> = (0..240).map(|_| sample(5, 33)).collect();
        assert_eq!(criterion_1(&behind, true), Some(false));
        assert_eq!(
            criterion_1(&behind, false),
            Some(true),
            "no vsync: headroom only"
        );
        // Two missed refreshes in 240 are within 1 %, three aren't.
        let mut some: Vec<CanvasSample> = (0..240).map(|_| sample(5, 16)).collect();
        some[10].interval = ms(25);
        some[20].interval = ms(25);
        assert_eq!(criterion_1(&some, true), Some(true));
        some[30].interval = ms(25);
        assert_eq!(criterion_1(&some, true), Some(false));
        let slow: Vec<CanvasSample> = (0..240)
            .map(|i| sample(if i < 3 { 17 } else { 5 }, 16))
            .collect();
        assert_eq!(criterion_1(&slow, true), Some(false), "p99 over 16 ms");
        assert_eq!(criterion_1(&[], true), None);
    }

    #[test]
    fn criterion_2_says_nothing_without_tiles_or_frames_past_the_lift() {
        let drag = |rendered: &[usize]| -> Vec<CanvasSample> {
            rendered
                .iter()
                .map(|&rendered| CanvasSample {
                    rendered,
                    ..CanvasSample::default()
                })
                .collect()
        };
        assert_eq!(
            criterion_2(&drag(&[12, 3, 4, 2]), true),
            Some(true),
            "the lift may render more"
        );
        assert_eq!(criterion_2(&drag(&[12, 3, 5]), true), Some(false));
        assert_eq!(criterion_2(&drag(&[12]), true), None);
        assert_eq!(criterion_2(&drag(&[0, 0]), false), None);
    }

    #[test]
    fn a_blocking_present_is_not_frame_time() {
        let gate = scripted(&["--measure"]);
        let frame = FrameInfo {
            number: 1,
            paint: ms(2),
            render: FrameTiming {
                encode: ms(3),
                submit: ms(15),
                present: ms(13),
                ..FrameTiming::default()
            },
            at: ms(0),
            scale: 1.0,
            physical_size: (1280, 800),
            causes: FrameCauses::default(),
        };
        assert_eq!(gate.canvas_sample(&frame, ms(16)).frame, ms(7));
    }

    fn scripted(args: &[&str]) -> Gate {
        let mut all = vec![
            "imaginarium-gate",
            "--cards",
            "40",
            "--shapes",
            "200",
            "--commands",
            "40",
        ];
        all.extend_from_slice(args);
        let args = Args::parse_from(all);
        let app = App::new(
            Fonts::bundled_only(),
            args.cards,
            args.shapes,
            args.commands,
            true,
            None,
        );
        Gate::new(&args, app).unwrap()
    }

    /// Presents frames until the script idles, and names the phase each
    /// frame was drawn in.
    fn phases(gate: &mut Gate) -> Vec<&'static str> {
        let mut seen = Vec::new();
        for number in 1..10_000 {
            if matches!(gate.phase, Phase::Idle { .. } | Phase::Done) {
                return seen;
            }
            seen.push(gate.phase.name());
            let frame = FrameInfo {
                number,
                paint: ms(1),
                render: FrameTiming::default(),
                at: ms(16 * number),
                scale: 1.0,
                physical_size: (1280, 800),
                causes: FrameCauses::default(),
            };
            gate.presented(&frame);
        }
        panic!("the script never idled: {:?}", seen.last());
    }

    fn runs(phases: &[&'static str]) -> Vec<(&'static str, usize)> {
        let mut runs: Vec<(&'static str, usize)> = Vec::new();
        for &phase in phases {
            match runs.last_mut() {
                Some((last, count)) if *last == phase => *count += 1,
                _ => runs.push((phase, 1)),
            }
        }
        runs
    }

    #[test]
    fn the_script_runs_every_phase_in_order_and_settles_between() {
        let mut gate = scripted(&[
            "--measure",
            "--pan-frames",
            "3",
            "--type-keys",
            "2",
            "--palette-keys",
            "2",
            "--shape-frames",
            "2",
        ]);
        let seen = phases(&mut gate);
        assert_eq!(
            runs(&seen),
            [
                ("start", 1),
                ("cards pan", 3),
                ("settle", 1),
                ("typing", 2),
                ("palette", 2),
                ("settle", 1),
                ("shapes pan", 2),
                ("settle", 1),
                ("shapes zoom", 2),
                ("settle", 1),
                ("shapes drag", 2),
                ("settle", 1),
            ]
        );
        assert_eq!(gate.typing.len(), 2);
        assert_eq!(gate.palette.len(), 2);
        assert!(gate.shape_samples.iter().all(|samples| samples.len() == 2));
        assert!(gate.settled_typing.is_some() && gate.settled.iter().all(Option::is_some));
    }

    #[test]
    fn phases_left_out_lead_on_to_the_next() {
        // dff4083: without typing, the script went straight to idle.
        let mut gate = scripted(&[
            "--measure",
            "--pan-frames",
            "2",
            "--type-keys",
            "0",
            "--palette-keys",
            "0",
            "--shape-frames",
            "1",
        ]);
        let seen = runs(&phases(&mut gate));
        assert_eq!(
            seen.iter().map(|(phase, _)| *phase).collect::<Vec<_>>(),
            [
                "start",
                "cards pan",
                "settle",
                "shapes pan",
                "settle",
                "shapes zoom",
                "settle",
                "shapes drag",
                "settle"
            ]
        );
        let mut nothing = scripted(&[
            "--measure",
            "--pan-frames",
            "0",
            "--type-keys",
            "0",
            "--palette-keys",
            "0",
            "--shape-frames",
            "0",
        ]);
        assert_eq!(runs(&phases(&mut nothing)), [("start", 1), ("settle", 1)]);
    }

    #[test]
    fn surface_set_ups_and_skips_are_counted_by_phase() {
        let mut gate = scripted(&["--measure", "--pan-frames", "3"]);
        let frame = |number: u64, render: FrameTiming| FrameInfo {
            number,
            paint: ms(1),
            render,
            at: ms(16 * number),
            scale: 1.0,
            physical_size: (1280, 800),
            causes: FrameCauses::default(),
        };
        gate.presented(&frame(1, FrameTiming::default()));
        assert_eq!(gate.phase.name(), "cards pan");
        gate.presented(&frame(
            2,
            FrameTiming {
                suboptimal: true,
                ..FrameTiming::default()
            },
        ));
        gate.presented(&frame(
            3,
            FrameTiming {
                acquire: ms(40),
                reconfigured: true,
                skipped_before: 2,
                ..FrameTiming::default()
            },
        ));
        assert_eq!(
            surface_report(&gate.surface),
            serde_json::json!({
                "start": {"suboptimal": 0, "reconfigured": 0, "skipped": 0},
                "cards pan": {"suboptimal": 1, "reconfigured": 1, "skipped": 2},
            })
        );
        // The longest gaps say which frames were set up again.
        let gaps = longest_intervals(&gate.gaps, 8);
        assert!(
            gaps.as_array().unwrap().iter().any(|gap| gap["frame"] == 3
                && gap["reconfigured"] == true
                && gap["skipped_before"] == 2),
            "{gaps}"
        );
    }

    #[test]
    fn idle_seconds_must_be_a_duration() {
        let args = Args::parse_from([
            "imaginarium-gate",
            "--idle-seconds",
            "inf",
            "--cards",
            "4",
            "--shapes",
            "4",
            "--commands",
            "4",
        ]);
        let app = App::new(Fonts::bundled_only(), 4, 4, 4, true, None);
        assert!(Gate::new(&args, app).is_err());
    }
}
