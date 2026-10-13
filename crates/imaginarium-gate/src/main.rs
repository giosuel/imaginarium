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
//! - Not a product binary, and nothing depends on it. It is deleted or
//!   folded into a benchmark once the gate is decided.
//!
//! # Measurements so far (M2.3)
//! - Cold start: process start (`/proc/self/stat`) and `main` until the
//!   first presented frame, and loading the fonts.
//! - Scripted pan: per-frame paint (toolkit), encode and submit (renderer),
//!   swapchain wait, and frame-to-frame intervals.
//! - Scripted typing into the search field: handling a keystroke
//!   (editing, searching 2,000 cards) and the frame after it.
//! - Scripted typing into the command palette (criterion 3): the search
//!   over 2,000 commands, the widget tree's layout, paint and encode.
//! - The 10,000-shape board through the tile cache (criteria 1 and 2):
//!   scripted pan, zoom and drag, with tiles rendered per frame.
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
use imaginarium_render::{GpuInfo, GpuOptions, Power};
use imaginarium_text::Fonts;
use imaginarium_ui::{AccessTree, FrameInfo, Input, View, WindowOptions};

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
    /// Draw every glyph's outline each frame instead of from Vello's
    /// (experimental) glyph atlas.
    #[arg(long)]
    no_glyph_atlas: bool,
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
/// In a manual session, a frame interval longer than this is a pause.
const MANUAL_GAP: Duration = Duration::from_millis(100);

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
    /// Waiting for the frame that shows the cleared search, so the idle
    /// measurement starts with nothing left to draw.
    Settle,
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
    last_at: Option<Duration>,
    pan: Vec<FrameSample>,
    /// Layouts built during the pan (text cache misses).
    pan_layouts: usize,
    typing: Vec<KeySample>,
    palette_keys: u32,
    palette: Vec<PaletteSample>,
    shape_frames: u32,
    shape_samples: [Vec<CanvasSample>; 3],
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
    /// Everything on the UI thread: paint, texture renders, encode, submit.
    frame: Duration,
    paint: Duration,
    textures: Duration,
    encode: Duration,
    interval: Duration,
    rendered: usize,
    owed: usize,
    items: usize,
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

    fn canvas_sample(&self, frame: &FrameInfo, interval: Duration) -> CanvasSample {
        let canvas = self.app.board.borrow().last_canvas.clone();
        CanvasSample {
            frame: frame.paint + frame.render.textures + frame.render.encode + frame.render.submit,
            paint: frame.paint,
            textures: frame.render.textures,
            encode: frame.render.encode,
            interval,
            rendered: canvas.rendered,
            owed: canvas.owed,
            items: canvas.items_painted,
        }
    }

    fn start_shapes(&mut self) {
        if self.shape_frames == 0 {
            self.phase = Phase::Settle;
            return;
        }
        self.app.set_mode(crate::board::Mode::Shapes);
        self.app.reset_view();
        self.phase = Phase::Shapes(ShapePhase::Pan, self.shape_frames);
        self.shape_step(ShapePhase::Pan, self.shape_frames);
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
            self.phase = Phase::Type(self.type_keys);
            self.type_key();
            true
        } else {
            // On to the palette and the shapes, whatever is left.
            self.after_typing()
        }
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
        self.app.set_frame(frame.paint, frame.render.encode);
        self.app
            .board
            .borrow_mut()
            .note_render_time(frame.render.textures);
        self.missing_textures += u64::from(frame.render.missing_textures);
        if self.first_frame.is_none() {
            self.first_frame = Some(*frame);
        }
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
                self.manual[board].push(sample);
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
            Phase::Settle => {
                self.start_idle();
                false
            }
            Phase::Type(left) => {
                if let Some((handle, search)) = self.pending_key.take() {
                    self.typing.push(KeySample {
                        handle,
                        search,
                        paint: frame.paint,
                        encode: frame.render.encode,
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
                            self.phase = Phase::Shapes(next, self.shape_frames);
                            self.shape_step(next, self.shape_frames);
                        }
                        None => {
                            self.app.set_mode(crate::board::Mode::Cards);
                            self.phase = Phase::Settle;
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

/// Timings and tile counts of a canvas phase.
fn canvas_report(samples: &[CanvasSample]) -> serde_json::Value {
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
    serde_json::json!({
        "frame": summary(field(|s| s.frame)),
        "frame_p99_ms": percentile(field(|s| s.frame), 0.99).map(millis),
        "paint": summary(field(|s| s.paint)),
        "textures": summary(field(|s| s.textures)),
        "encode": summary(field(|s| s.encode)),
        // Zero: the first frame, or one after a pause.
        "interval": summary(samples.iter().map(|s| s.interval).filter(|i| !i.is_zero()).collect()),
        // Frames that took longer than a 60 Hz refresh on the UI thread,
        // and gaps that missed one.
        "frames_over_16ms": samples.iter().filter(|s| s.frame > Duration::from_millis(16)).count(),
        "intervals_over_20ms": samples.iter().filter(|s| s.interval > Duration::from_millis(20)).count(),
        "tiles_rendered": counts(|s| s.rendered),
        "tiles_owed": counts(|s| s.owed),
        "items_painted": counts(|s| s.items),
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
    let gpu = GpuOptions {
        power: match args.gpu {
            GpuChoice::Low => Power::Low,
            GpuChoice::High => Power::High,
        },
        adapter: args.adapter.clone(),
        no_vsync: args.no_vsync,
        glyph_atlas: !args.no_glyph_atlas,
    };
    let options = WindowOptions {
        title: "Imaginarium gate".into(),
        size: Size::new(1280.0, 800.0),
        gpu,
        background: Color::WHITE,
    };
    let fonts_started = Instant::now();
    let fonts = Fonts::new();
    let fonts_time = fonts_started.elapsed();
    let mut gate = Gate {
        app: App::new(
            fonts,
            args.cards,
            args.shapes,
            args.commands,
            !args.no_tiles,
        ),
        palette_keys: args.palette_keys,
        palette: Vec::new(),
        shape_frames: args.shape_frames,
        shape_samples: [Vec::new(), Vec::new(), Vec::new()],
        pan_canvas: Vec::new(),
        dragged: None,
        measure: args.measure,
        pan_frames: args.pan_frames,
        type_keys: args.type_keys,
        idle: Duration::from_secs_f64(args.idle_seconds.max(0.0)),
        close_after_frames: args.frames,
        phase: Phase::Start,
        frames: 0,
        first_frame: None,
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
    };
    // The time from exec to `main` (loading the binary and its libraries)
    // is measured separately from the time in `main`.
    let before_main = proc::since_exec().map(|since| since.saturating_sub(main_started.elapsed()));
    let before_run = main_started.elapsed();
    let stats = imaginarium_ui::run(options, &mut gate).context("running the window")?;

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
            // Criterion 6: no frames and (almost) no CPU while idle.
            "pass": idle.frames == 0 && idle.cpu.is_none_or(|cpu| cpu.as_secs_f64() / idle.wall.as_secs_f64() < 0.01),
        })
    });
    // Criterion 1: p99 frame time under 16 ms while panning and zooming
    // 10,000 shapes. (Frames per second come from the intervals with
    // vertical sync; --no-vsync shows the headroom.)
    let criterion_1 = [&gate.shape_samples[0], &gate.shape_samples[1]]
        .iter()
        .all(|samples| {
            percentile(samples.iter().map(|sample| sample.frame).collect(), 0.99)
                .is_some_and(|p99| p99 < Duration::from_millis(16))
        });
    // Criterion 2: dragging re-renders only the tiles under the shape (its
    // old and new place): at most 4 per frame.
    let criterion_2 = !gate.shape_samples[2].is_empty()
        && gate.shape_samples[2]
            .iter()
            .skip(1)
            .all(|sample| sample.rendered <= 4);
    let report = serde_json::json!({
        "gate_version": env!("CARGO_PKG_VERSION"),
        "milestone": "M2.7",
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
        })),
        "window": first.map(|frame| serde_json::json!({
            "scale": frame.scale,
            "physical_size": [frame.physical_size.0, frame.physical_size.1],
        })),
        "cold_start": {
            "exec_to_main_ms": before_main.map(millis),
            "main_to_first_frame_ms": stats.first_frame.map(|first| millis(before_run + first)),
            "first_frame_paint_ms": first.map(|frame| millis(frame.paint)),
            "first_frame_encode_ms": first.map(|frame| millis(frame.render.encode)),
            "fonts_ms": millis(fonts_time),
            "window_ms": stats.window_created.map(millis),
            "gpu_ms": stats.gpu_opened.map(millis),
            "font_families": gate.app.fonts.family_count(),
            // Criterion 6: cold start under 400 ms.
            "pass": stats.first_frame.map(|first| before_main.unwrap_or_default() + before_run + first < Duration::from_millis(400)),
        },
        "content": {
            "cards": gate.app.board.borrow().cards(),
            "shapes": gate.app.board.borrow().shape_count(),
            "commands": args.commands,
            "tiles": !args.no_tiles,
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
            "canvas": canvas_report(&gate.pan_canvas),
        },
        // A manual session's frames, by board.
        "manual": {
            "cards": canvas_report(&gate.manual[0]),
            "shapes": canvas_report(&gate.manual[1]),
        },
        "missing_textures": gate.missing_textures,
        "shapes": {
            "pan": canvas_report(&gate.shape_samples[0]),
            "zoom": canvas_report(&gate.shape_samples[1]),
            "drag": canvas_report(&gate.shape_samples[2]),
            "criterion_1_pass": criterion_1,
            "criterion_2_pass": criterion_2,
        },
        "typing": {
            // Editing the field and searching every card's text.
            "handle": summary(typing(|sample| sample.handle)),
            "search": summary(typing(|sample| sample.search)),
            "paint": summary(typing(|sample| sample.paint)),
            "encode": summary(typing(|sample| sample.encode)),
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
