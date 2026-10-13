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
//! # Measurements so far (M2.2)
//! - Cold start: process start (`/proc/self/stat`) and `main` until the
//!   first presented frame.
//! - Scripted pan: per-frame CPU time and frame-to-frame intervals.
//! - Idle: CPU used and frames drawn while nothing happens (criterion 6).
//! - The GPU and machine it ran on.

mod board;
mod proc;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use clap::{Parser, ValueEnum};
use imaginarium_render::Scene;
use imaginarium_render::kurbo::{Size, Vec2};
use imaginarium_render::peniko::Color;
use imaginarium_render::{GpuOptions, Power};
use imaginarium_ui::{FrameInfo, Input, View, WindowOptions};

use crate::board::Board;

/// The M2 gate app: shows test content and measures the UI toolkit.
#[derive(Parser, Debug)]
#[command(version)]
struct Args {
    /// Run the scripted measurement (pan, then idle), write the report and
    /// close. Without it the window stays open for manual testing, and the
    /// report is written when it closes.
    #[arg(long)]
    measure: bool,
    /// Frames of scripted panning to time.
    #[arg(long, default_value_t = 300)]
    pan_frames: u32,
    /// Seconds of idling to measure.
    #[arg(long, default_value_t = 10.0)]
    idle_seconds: f64,
    /// How many cards the board shows.
    #[arg(long, default_value_t = 2_000)]
    cards: usize,
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

/// Where the scripted run is.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    /// Waiting for the first frame.
    Start,
    /// Panning, with this many frames to go.
    Pan(u32),
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
    board: Board,
    measure: bool,
    pan_frames: u32,
    idle: Duration,
    close_after_frames: Option<u64>,
    phase: Phase,
    frames: u64,
    first_frame: Option<FrameInfo>,
    last_at: Option<Duration>,
    pan: Vec<FrameSample>,
    idle_result: Option<IdleResult>,
    inputs: u64,
    inputs_during_idle: u64,
}

#[derive(Clone, Copy, Debug)]
struct FrameSample {
    paint: Duration,
    acquire: Duration,
    encode: Duration,
    submit: Duration,
    interval: Duration,
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
}

impl View for Gate {
    fn paint(&mut self, scene: &mut Scene, size: Size) {
        self.board.paint(scene, size);
    }

    fn input(&mut self, input: &Input) -> bool {
        self.inputs += 1;
        if matches!(self.phase, Phase::Idle { .. }) {
            self.inputs_during_idle += 1;
        }
        // Scripted phases ignore the pointer, so a mouse resting on the
        // window doesn't skew them.
        match self.phase {
            Phase::Manual => self.board.input(input),
            _ => false,
        }
    }

    fn presented(&mut self, frame: &FrameInfo) -> bool {
        self.frames = frame.number;
        let interval = self.last_at.map_or(Duration::ZERO, |last| frame.at - last);
        self.last_at = Some(frame.at);
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
            Phase::Start if self.pan_frames > 0 => {
                self.phase = Phase::Pan(self.pan_frames);
                self.board.pan(Vec2::new(-6.0, -3.0));
                true
            }
            Phase::Start => {
                self.start_idle();
                false
            }
            Phase::Pan(left) => {
                self.pan.push(FrameSample {
                    paint: frame.paint,
                    acquire: frame.render.acquire,
                    encode: frame.render.encode,
                    submit: frame.render.submit,
                    interval,
                });
                if left > 1 {
                    self.phase = Phase::Pan(left - 1);
                    self.board.pan(Vec2::new(-6.0, -3.0));
                    true
                } else {
                    self.start_idle();
                    false
                }
            }
            _ => false,
        }
    }

    fn wake_at(&mut self) -> Option<Instant> {
        match self.phase {
            Phase::Idle { since, .. } => Some(since + self.idle),
            _ => None,
        }
    }

    fn wake(&mut self, now: Instant) -> bool {
        if let Phase::Idle { since, cpu, frames } = self.phase {
            self.idle_result = Some(IdleResult {
                wall: now - since,
                cpu: cpu
                    .zip(proc::cpu_time())
                    .map(|(start, end)| end.saturating_sub(start)),
                frames: self.frames - frames,
            });
            self.phase = Phase::Done;
        }
        false
    }

    fn should_close(&mut self) -> bool {
        self.phase == Phase::Done
            || self
                .close_after_frames
                .is_some_and(|limit| self.frames >= limit)
    }
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
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
    };
    let options = WindowOptions {
        title: "Imaginarium gate".into(),
        size: Size::new(1280.0, 800.0),
        gpu,
        background: Color::WHITE,
    };
    let mut gate = Gate {
        board: Board::new(args.cards),
        measure: args.measure,
        pan_frames: args.pan_frames,
        idle: Duration::from_secs_f64(args.idle_seconds.max(0.0)),
        close_after_frames: args.frames,
        phase: Phase::Start,
        frames: 0,
        first_frame: None,
        last_at: None,
        pan: Vec::new(),
        idle_result: None,
        inputs: 0,
        inputs_during_idle: 0,
    };
    // The time from exec to `main` (loading the binary and its libraries)
    // is measured separately from the time in `main`.
    let before_main = proc::since_exec().map(|since| since.saturating_sub(main_started.elapsed()));
    let before_run = main_started.elapsed();
    let stats = imaginarium_ui::run(options, &mut gate).context("running the window")?;

    let first = gate.first_frame;
    let pan = |field: fn(&FrameSample) -> Duration| gate.pan.iter().map(field).collect::<Vec<_>>();
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
    let report = serde_json::json!({
        "gate_version": env!("CARGO_PKG_VERSION"),
        "milestone": "M2.2",
        "mode": if args.measure { "measure" } else { "manual" },
        "machine": proc::machine(),
        "gpu": stats.gpu.as_ref().map(|gpu| serde_json::json!({
            "name": gpu.name,
            "backend": gpu.backend,
            "device_type": gpu.device_type,
            "driver": gpu.driver,
            "requested": format!("{:?}", args.gpu).to_lowercase(),
            "vsync": !args.no_vsync,
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
        },
        "content": { "cards": gate.board.cards() },
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
