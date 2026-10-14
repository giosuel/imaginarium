// SPDX-License-Identifier: AGPL-3.0-or-later

//! Pages of uploaded tiles on the GPU.
//!
//! Each canvas tile used to be a texture of its own: a draw and a bind
//! group of its own in every frame, and a new tile's texture was memory the
//! driver backed only when a frame first used it. Uploaded tiles share
//! slots of a few large textures instead. Pages are made ahead of need on a
//! thread of their own and cleared there, so that work is done before a
//! frame needs the page: when the canvas hands tiles to its workers, it
//! says how many will come ([`crate::Scene::expect_uploads`]).

use std::collections::HashMap;
use std::sync::mpsc;
use std::time::Duration;

use vello_common::TextureId;
use vello_common::geometry::RectU16;
use vello_cpu::Pixmap;

use crate::scene::{Placed, TextureKey};

/// The side of a page, in pixels: 64 slots, 16 MB.
const PAGE: u16 = 2048;
/// The side of a slot: a canvas tile ([`crate::canvas::TILE`]).
pub(crate) const SLOT: u16 = 256;
const SLOTS_PER_ROW: u16 = PAGE / SLOT;
const SLOTS: u16 = SLOTS_PER_ROW * SLOTS_PER_ROW;

/// How long an upload waits for a page on its way before making one
/// itself (the page thread is gone).
const PAGE_WAIT: Duration = Duration::from_millis(500);

/// The pixel format of uploads: `vello_cpu`'s premultiplied RGBA.
pub(crate) const UPLOAD_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

struct Page {
    /// Its binding for Vello.
    id: TextureId,
    texture: wgpu::Texture,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Slot {
    page: usize,
    index: u16,
}

impl Slot {
    fn region(self) -> RectU16 {
        let x0 = (self.index % SLOTS_PER_ROW) * SLOT;
        let y0 = (self.index / SLOTS_PER_ROW) * SLOT;
        RectU16 {
            x0,
            y0,
            x1: x0 + SLOT,
            y1: y0 + SLOT,
        }
    }
}

/// Uploaded textures of [`SLOT`]² pixels in shared pages.
pub(crate) struct TilePages {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pages: Vec<Page>,
    /// Free slots, the last taken first.
    free: Vec<Slot>,
    used: HashMap<TextureKey, Slot>,
    made: mpsc::Receiver<Page>,
    make: mpsc::Sender<Page>,
    /// Pages asked for that haven't come yet.
    coming: usize,
}

/// What a frame's uploads cost in pages.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct PageStats {
    /// Pages the frame made, or waited for, because none was ready.
    pub(crate) made: u32,
    /// Pages being made on their thread when the frame's uploads began.
    pub(crate) coming: u32,
}

impl TilePages {
    pub(crate) fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let (make, made) = mpsc::channel();
        Self {
            device: device.clone(),
            queue: queue.clone(),
            pages: Vec::new(),
            free: Vec::new(),
            used: HashMap::new(),
            made,
            make,
            coming: 0,
        }
    }

    /// Takes the pages made since the last call.
    pub(crate) fn receive(&mut self, bindings: &mut vello_gpu::TextureBindings) {
        while let Ok(page) = self.made.try_recv() {
            self.coming = self.coming.saturating_sub(1);
            self.add(page, bindings);
        }
    }

    fn add(&mut self, page: Page, bindings: &mut vello_gpu::TextureBindings) {
        bindings.insert(
            page.id,
            page.texture
                .create_view(&wgpu::TextureViewDescriptor::default()),
        );
        let index = self.pages.len();
        // Reversed, so slots are taken in order: tiles uploaded together
        // share a page, and a frame draws them with one binding.
        self.free.extend((0..SLOTS).rev().map(|slot| Slot {
            page: index,
            index: slot,
        }));
        self.pages.push(page);
    }

    /// Starts making `count` pages on a thread of their own.
    fn make_ahead(&mut self, count: usize) {
        if count == 0 {
            return;
        }
        let (device, queue, sender) = (self.device.clone(), self.queue.clone(), self.make.clone());
        let spawned = std::thread::Builder::new()
            .name("tile-pages".into())
            .spawn(move || {
                for _ in 0..count {
                    if sender.send(make_page(&device, &queue)).is_err() {
                        break;
                    }
                }
            });
        // Without the thread, pages are made when needed.
        if spawned.is_ok() {
            self.coming += count;
        }
    }

    /// Makes room ahead for `count` uploads of tiles without a slot, and a
    /// page more in reserve ([`crate::Scene::expect_uploads`]).
    pub(crate) fn expect(&mut self, count: usize) {
        self.make_ahead(pages_to_make(self.free.len(), self.coming, count));
    }

    /// Pages being made on their thread.
    pub(crate) fn coming(&self) -> usize {
        self.coming
    }

    /// Writes `pixmap` ([`SLOT`]² pixels) into `key`'s slot, giving it one
    /// first if it has none.
    pub(crate) fn upload(
        &mut self,
        bindings: &mut vello_gpu::TextureBindings,
        key: TextureKey,
        pixmap: &Pixmap,
        stats: &mut PageStats,
    ) -> Placed {
        let slot = match self.used.get(&key) {
            Some(slot) => *slot,
            None => {
                if self.free.is_empty() {
                    stats.made += 1;
                    // Waiting for a page on its way costs at most what
                    // making another would, and doesn't make one too many.
                    let coming = (self.coming > 0)
                        .then(|| self.made.recv_timeout(PAGE_WAIT).ok())
                        .flatten();
                    let page = match coming {
                        Some(page) => {
                            self.coming -= 1;
                            page
                        }
                        None => {
                            self.coming = 0;
                            make_page(&self.device, &self.queue)
                        }
                    };
                    self.add(page, bindings);
                }
                let Some(slot) = self.free.pop() else {
                    unreachable!("a page was just added");
                };
                // A page in reserve, so the next one is made before it is
                // needed.
                if self.free.len() + self.coming * usize::from(SLOTS) < usize::from(SLOTS) {
                    self.make_ahead(1);
                }
                self.used.insert(key, slot);
                slot
            }
        };
        let region = slot.region();
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.pages[slot.page].texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: u32::from(region.x0),
                    y: u32::from(region.y0),
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            pixmap.data_as_u8_slice(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(u32::from(SLOT) * 4),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: u32::from(SLOT),
                height: u32::from(SLOT),
                depth_or_array_layers: 1,
            },
        );
        Placed {
            id: self.pages[slot.page].id,
            region,
        }
    }

    /// Frees `key`'s slot, if it has one.
    pub(crate) fn free(&mut self, key: TextureKey) {
        if let Some(slot) = self.used.remove(&key) {
            self.free.push(slot);
        }
    }

    /// Pages made so far.
    pub(crate) fn len(&self) -> usize {
        self.pages.len()
    }
}

/// The pages to start making for `count` more uploads, with `free` slots
/// and `coming` pages on their way: room for them all and a page more.
fn pages_to_make(free: usize, coming: usize, count: usize) -> usize {
    let slots = usize::from(SLOTS);
    let room = free + coming * slots;
    (count + slots).saturating_sub(room).div_ceil(slots)
}

/// A cleared page. Clearing it here makes the driver back it with memory
/// now, rather than in the frame that first uses it, and tells wgpu it
/// needs no clearing later.
fn make_page(device: &wgpu::Device, queue: &wgpu::Queue) -> Page {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("tile page"),
        size: wgpu::Extent3d {
            width: u32::from(PAGE),
            height: u32::from(PAGE),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: UPLOAD_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("tile page clear"),
    });
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("tile page clear"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    queue.submit([encoder.finish()]);
    Page {
        id: TextureId(TextureKey::unique().id()),
        texture,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_tile_the_page() {
        let regions: Vec<RectU16> = (0..SLOTS)
            .map(|index| Slot { page: 0, index }.region())
            .collect();
        assert_eq!(
            regions[0],
            RectU16 {
                x0: 0,
                y0: 0,
                x1: 256,
                y1: 256
            }
        );
        assert_eq!(
            regions[usize::from(SLOTS) - 1],
            RectU16 {
                x0: 1792,
                y0: 1792,
                x1: 2048,
                y1: 2048
            }
        );
        for (i, a) in regions.iter().enumerate() {
            for b in &regions[i + 1..] {
                assert!(a.x1 <= b.x0 || b.x1 <= a.x0 || a.y1 <= b.y0 || b.y1 <= a.y0);
            }
        }
    }

    #[test]
    fn expected_uploads_get_room_and_a_page_in_reserve() {
        // Nothing yet: a screenful of the gate laptop's panel (21 × 13
        // tiles at most) takes five pages, the last one in reserve.
        assert_eq!(pages_to_make(0, 0, 273), 6);
        assert_eq!(pages_to_make(0, 0, 228), 5);
        assert_eq!(pages_to_make(0, 0, 1), 2);
        // Said again the next frame, nothing more is made.
        assert_eq!(pages_to_make(0, 5, 228), 0);
        // A page came and 30 tiles went into it.
        assert_eq!(pages_to_make(34, 4, 198), 0);
        // Nothing coming: still a page in reserve.
        assert_eq!(pages_to_make(64, 0, 0), 0);
        assert_eq!(pages_to_make(63, 0, 0), 1);
    }
}
