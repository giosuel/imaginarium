// SPDX-License-Identifier: AGPL-3.0-or-later

//! The GPU's own time per frame, from timestamp queries.
//!
//! Timestamps are written around each texture render and the frame's
//! render, resolved at the end of the frame and read back a few frames
//! later. The CPU never waits for them: a frame that finds no free slot
//! goes untimed.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

/// Frames whose timestamps can be on their way back at once.
const SLOTS: usize = 4;
/// Timestamp pairs per frame: the frame's texture renders (the canvas
/// renders at most 64 tiles a frame) and the frame itself.
const PAIRS: u32 = 72;

const FREE: u8 = 0;
const PENDING: u8 = 1;
const MAPPED: u8 = 2;
const FAILED: u8 = 3;

/// The GPU's time for one frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GpuFrameTime {
    /// The frame's number: 1 for the first frame the renderer presented.
    pub frame: u64,
    /// Rendering the textures (canvas tiles) the frame needed.
    pub textures: Duration,
    /// Rendering the frame's scene into the window.
    pub scene: Duration,
    /// From the start of the first render to the end of the last, the gaps
    /// between them included.
    pub span: Duration,
    /// Texture renders that weren't timed (more than a frame's slot holds).
    pub untimed: u32,
}

struct Slot {
    queries: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    readback: wgpu::Buffer,
    state: Arc<AtomicU8>,
    frame: u64,
    /// Queries written so far, in pairs.
    used: u32,
    /// The first query of the scene's pair.
    scene: Option<u32>,
    untimed: u32,
}

/// Times frames on the GPU (needs [`GpuTimer::FEATURES`]).
pub(crate) struct GpuTimer {
    /// Nanoseconds per timestamp tick.
    period: f64,
    slots: Vec<Slot>,
    /// The slot the frame being rendered writes to.
    current: Option<usize>,
    next: usize,
}

impl GpuTimer {
    /// The device features timing needs.
    pub(crate) const FEATURES: wgpu::Features =
        wgpu::Features::TIMESTAMP_QUERY.union(wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS);

    /// A timer, if the device has [`GpuTimer::FEATURES`].
    pub(crate) fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Option<Self> {
        if !device.features().contains(Self::FEATURES) {
            return None;
        }
        let count = PAIRS * 2;
        let size = u64::from(count) * u64::from(wgpu::QUERY_SIZE);
        let buffer = |label, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        let slots = (0..SLOTS)
            .map(|_| Slot {
                queries: device.create_query_set(&wgpu::QuerySetDescriptor {
                    label: Some("frame timestamps"),
                    ty: wgpu::QueryType::Timestamp,
                    count,
                }),
                resolve: buffer(
                    "frame timestamps resolved",
                    wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                ),
                readback: buffer(
                    "frame timestamps read back",
                    wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                ),
                state: Arc::new(AtomicU8::new(FREE)),
                frame: 0,
                used: 0,
                scene: None,
                untimed: 0,
            })
            .collect();
        Some(Self {
            period: f64::from(queue.get_timestamp_period()),
            slots,
            current: None,
            next: 0,
        })
    }

    /// Starts timing frame number `frame`.
    pub(crate) fn begin_frame(&mut self, frame: u64) {
        self.current = None;
        let slot = &mut self.slots[self.next];
        if slot.state.load(Ordering::Acquire) != FREE {
            return;
        }
        slot.frame = frame;
        slot.used = 0;
        slot.scene = None;
        slot.untimed = 0;
        self.current = Some(self.next);
        self.next = (self.next + 1) % SLOTS;
    }

    /// Writes the start of a texture render, keeping a pair for the
    /// frame's scene. `None`: this render goes untimed.
    pub(crate) fn texture_start(&mut self, encoder: &mut wgpu::CommandEncoder) -> Option<u32> {
        let slot = &mut self.slots[self.current?];
        if slot.used + 4 > PAIRS * 2 {
            slot.untimed += 1;
            return None;
        }
        Some(Self::start(slot, encoder))
    }

    /// Writes the start of the frame's scene render.
    pub(crate) fn scene_start(&mut self, encoder: &mut wgpu::CommandEncoder) -> Option<u32> {
        let slot = &mut self.slots[self.current?];
        if slot.used + 2 > PAIRS * 2 {
            return None;
        }
        let first = Self::start(slot, encoder);
        slot.scene = Some(first);
        Some(first)
    }

    fn start(slot: &mut Slot, encoder: &mut wgpu::CommandEncoder) -> u32 {
        let first = slot.used;
        encoder.write_timestamp(&slot.queries, first);
        slot.used += 2;
        first
    }

    /// Writes the end of the render that `start` began.
    pub(crate) fn end(&mut self, encoder: &mut wgpu::CommandEncoder, start: Option<u32>) {
        if let (Some(current), Some(start)) = (self.current, start) {
            encoder.write_timestamp(&self.slots[current].queries, start + 1);
        }
    }

    /// Resolves the frame's timestamps into its slot's read-back buffer,
    /// in `encoder`: the frame's last submission.
    pub(crate) fn finish(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let Some(current) = self.current else {
            return;
        };
        let slot = &self.slots[current];
        if slot.used == 0 {
            return;
        }
        encoder.resolve_query_set(&slot.queries, 0..slot.used, &slot.resolve, 0);
        let size = u64::from(slot.used) * u64::from(wgpu::QUERY_SIZE);
        encoder.copy_buffer_to_buffer(&slot.resolve, 0, &slot.readback, 0, size);
    }

    /// After the frame's last submission: starts reading its timestamps
    /// back.
    pub(crate) fn submitted(&mut self) {
        let Some(current) = self.current.take() else {
            return;
        };
        let slot = &self.slots[current];
        if slot.used == 0 {
            return;
        }
        slot.state.store(PENDING, Ordering::Release);
        let state = Arc::clone(&slot.state);
        let size = u64::from(slot.used) * u64::from(wgpu::QUERY_SIZE);
        slot.readback
            .map_async(wgpu::MapMode::Read, ..size, move |result| {
                state.store(
                    if result.is_ok() { MAPPED } else { FAILED },
                    Ordering::Release,
                );
            });
    }

    /// The oldest frame whose timestamps are back, without waiting.
    pub(crate) fn collect(&mut self, device: &wgpu::Device) -> Option<GpuFrameTime> {
        // Runs the map callbacks of finished work; never blocks.
        let _ = device.poll(wgpu::PollType::Poll);
        let mut oldest: Option<(usize, u64)> = None;
        for (index, slot) in self.slots.iter().enumerate() {
            match slot.state.load(Ordering::Acquire) {
                MAPPED if oldest.is_none_or(|(_, frame)| frame > slot.frame) => {
                    oldest = Some((index, slot.frame));
                }
                FAILED => slot.state.store(FREE, Ordering::Release),
                _ => {}
            }
        }
        let (oldest, _) = oldest?;
        let slot = &self.slots[oldest];
        let size = u64::from(slot.used) * u64::from(wgpu::QUERY_SIZE);
        let ticks: Vec<u64> = match slot.readback.get_mapped_range(..size) {
            Ok(view) => view
                .chunks_exact(8)
                .map(|bytes| {
                    let mut word = [0; 8];
                    word.copy_from_slice(bytes);
                    u64::from_le_bytes(word)
                })
                .collect(),
            Err(_) => Vec::new(),
        };
        slot.readback.unmap();
        slot.state.store(FREE, Ordering::Release);
        let time = |ticks: u64| Duration::from_nanos((ticks as f64 * self.period) as u64);
        let mut times = GpuFrameTime {
            frame: slot.frame,
            untimed: slot.untimed,
            ..GpuFrameTime::default()
        };
        let (mut first, mut last) = (u64::MAX, 0);
        for (start, pair) in (0u32..).step_by(2).zip(ticks.chunks_exact(2)) {
            let took = time(pair[1].saturating_sub(pair[0]));
            if slot.scene == Some(start) {
                times.scene = took;
            } else {
                times.textures += took;
            }
            first = first.min(pair[0]);
            last = last.max(pair[1]);
        }
        times.span = time(last.saturating_sub(first));
        Some(times)
    }
}
