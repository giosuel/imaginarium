// SPDX-License-Identifier: AGPL-3.0-or-later

//! Threads that render canvas tiles with `vello_cpu`, off the UI thread.
//!
//! The UI thread paints a tile's items into a [`Scene`] and hands it over;
//! a worker renders it into a pixmap and sends it back, and the canvas
//! uploads it in a later frame ([`Scene::upload_texture`]). Rendering is
//! most of a tile's cost, and tiles are independent of each other.

use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Condvar, Mutex, PoisonError, mpsc};
use std::time::{Duration, Instant};

use vello_common::peniko::Color;
use vello_cpu::Pixmap;

use crate::canvas::TileKey;
use crate::cpu::{CpuMode, draw};
use crate::scene::{Scene, Textures};

/// Threads that render canvas tiles into pixmaps
/// ([`crate::canvas::Canvas::with_workers`]).
///
/// Clones share the threads, so the canvases of a window can share them.
/// The threads end when the last clone is dropped, after the tile each is
/// on.
///
/// ```
/// use imaginarium_render::{CpuMode, TileWorkers, canvas::Canvas};
///
/// let workers = TileWorkers::new(2, CpuMode::Fast);
/// let cards = Canvas::new().with_workers(workers.clone());
/// let shapes = Canvas::new().with_workers(workers);
/// # let _ = (cards, shapes);
/// ```
#[derive(Clone, Debug)]
pub struct TileWorkers {
    handle: Arc<Handle>,
}

#[derive(Debug)]
struct Handle {
    shared: Arc<Shared>,
    threads: usize,
}

#[derive(Debug, Default)]
struct Shared {
    queue: Mutex<Queue>,
    ready: Condvar,
}

#[derive(Debug, Default)]
struct Queue {
    jobs: VecDeque<Job>,
    closed: bool,
}

/// A tile to render.
#[derive(Debug)]
pub(crate) struct Job {
    pub(crate) key: TileKey,
    /// Tells this job's result from an earlier one's for the same tile.
    pub(crate) tag: u64,
    pub(crate) scene: Scene,
    pub(crate) size: u16,
    pub(crate) reply: mpsc::Sender<Done>,
}

/// A rendered tile.
#[derive(Debug)]
pub(crate) struct Done {
    pub(crate) key: TileKey,
    pub(crate) tag: u64,
    /// `None`: rendering it failed (`vello_cpu` panicked).
    pub(crate) pixmap: Option<Arc<Pixmap>>,
    /// The worker's time for it.
    pub(crate) took: Duration,
}

impl TileWorkers {
    /// Starts `threads` threads (at least one) rendering in `mode`.
    pub fn new(threads: usize, mode: CpuMode) -> Self {
        let shared = Arc::new(Shared::default());
        let mut started = 0;
        for index in 0..threads.max(1) {
            let shared = Arc::clone(&shared);
            let spawned = std::thread::Builder::new()
                .name(format!("tiles-{index}"))
                .spawn(move || work(&shared, mode));
            if spawned.is_ok() {
                started += 1;
            }
        }
        Self {
            handle: Arc::new(Handle {
                shared,
                threads: started,
            }),
        }
    }

    /// The threads worth starting on this machine: half its cores (the
    /// other half for the UI thread, the GPU driver and the rest of the
    /// system), between 1 and 8.
    pub fn default_threads() -> usize {
        std::thread::available_parallelism()
            .map_or(2, |cores| cores.get() / 2)
            .clamp(1, 8)
    }

    /// The threads running.
    pub fn threads(&self) -> usize {
        self.handle.threads
    }

    /// Queues `job`. Without threads (none could start), the job is
    /// dropped and its tile never comes back.
    pub(crate) fn submit(&self, job: Job) {
        let mut queue = self
            .handle
            .shared
            .queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        queue.jobs.push_back(job);
        drop(queue);
        self.handle.shared.ready.notify_one();
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.shared
            .queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .closed = true;
        self.shared.ready.notify_all();
    }
}

fn work(shared: &Shared, mode: CpuMode) {
    let mut resources = vello_cpu::Resources::new();
    let mut context = None;
    loop {
        let job = {
            let mut queue = shared.queue.lock().unwrap_or_else(PoisonError::into_inner);
            loop {
                if let Some(job) = queue.jobs.pop_front() {
                    break job;
                }
                if queue.closed {
                    return;
                }
                queue = shared
                    .ready
                    .wait(queue)
                    .unwrap_or_else(PoisonError::into_inner);
            }
        };
        let started = Instant::now();
        let rendered = catch_unwind(AssertUnwindSafe(|| {
            let context = context.get_or_insert_with(|| mode.context(job.size, job.size));
            if (context.width(), context.height()) != (job.size, job.size) {
                *context = mode.context(job.size, job.size);
            }
            draw(
                context,
                &mut resources,
                mode,
                &job.scene,
                Textures::None,
                Color::TRANSPARENT,
            )
        }));
        if rendered.is_err() {
            // Whatever it left half done goes with it.
            resources = vello_cpu::Resources::new();
            context = None;
        }
        // The canvas may be gone; then nobody waits for it.
        let _ = job.reply.send(Done {
            key: job.key,
            tag: job.tag,
            pixmap: rendered.ok().map(Arc::new),
            took: started.elapsed(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vello_common::kurbo::Rect;

    #[test]
    fn workers_render_tiles_and_send_them_back() {
        let workers = TileWorkers::new(2, CpuMode::Deterministic);
        assert_eq!(workers.threads(), 2);
        let (reply, done) = mpsc::channel();
        for tag in 0..8 {
            let mut scene = Scene::new();
            scene.fill(&Rect::new(0.0, 0.0, 8.0, 8.0), Color::BLACK);
            workers.submit(Job {
                key: TileKey {
                    level: 0,
                    x: tag as i32,
                    y: 0,
                },
                tag,
                scene,
                size: 16,
                reply: reply.clone(),
            });
        }
        let mut tags: Vec<u64> = (0..8)
            .map(|_| {
                let done = done
                    .recv_timeout(Duration::from_secs(30))
                    .expect("a tile comes back");
                let pixmap = done.pixmap.expect("rendered");
                assert_eq!((pixmap.width(), pixmap.height()), (16, 16));
                // Black inside the square, transparent outside.
                assert_eq!(pixmap.data()[0].a, 255);
                assert_eq!(pixmap.data()[15 * 16 + 15].a, 0);
                assert_eq!(done.key.x, done.tag as i32);
                done.tag
            })
            .collect();
        tags.sort_unstable();
        assert_eq!(tags, (0..8).collect::<Vec<_>>());
    }

    #[test]
    fn scenes_and_pixmaps_cross_threads() {
        fn send<T: Send>() {}
        send::<Scene>();
        send::<Job>();
        send::<Done>();
    }
}
