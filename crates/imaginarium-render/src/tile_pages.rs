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
//!
//! Uploads take the lowest page with room, so the later pages empty as
//! tiles go; a page left empty is given back (its 16 MB freed) while
//! another page's worth of free slots stays in reserve. The window's tile
//! memory follows what it shows, not the most it ever showed.

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
    /// The pages by index; `None` where one was given back (its index is
    /// reused by the next).
    pages: Vec<Option<Page>>,
    /// Each page's free slots, the last taken first.
    free: Vec<Vec<u16>>,
    /// Free slots in all pages.
    free_count: usize,
    used: HashMap<TextureKey, Slot>,
    made: mpsc::Receiver<Page>,
    make: mpsc::Sender<Page>,
    /// Pages asked for that haven't come yet.
    coming: usize,
    /// Pages added so far (tests).
    #[cfg(test)]
    added: usize,
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
            free_count: 0,
            used: HashMap::new(),
            made,
            make,
            coming: 0,
            #[cfg(test)]
            added: 0,
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
        #[cfg(test)]
        {
            self.added += 1;
        }
        bindings.insert(
            page.id,
            page.texture
                .create_view(&wgpu::TextureViewDescriptor::default()),
        );
        // Reversed, so slots are taken in order: tiles uploaded together
        // share a page, and a frame draws them with one binding.
        let slots: Vec<u16> = (0..SLOTS).rev().collect();
        self.free_count += slots.len();
        match self.pages.iter().position(Option::is_none) {
            Some(index) => {
                self.pages[index] = Some(page);
                self.free[index] = slots;
            }
            None => {
                self.pages.push(Some(page));
                self.free.push(slots);
            }
        }
    }

    /// A free slot of the lowest page with room (so the later ones empty).
    fn take_slot(&mut self) -> Option<Slot> {
        let page = self.free.iter().position(|slots| !slots.is_empty())?;
        let index = self.free[page].pop()?;
        self.free_count -= 1;
        Some(Slot { page, index })
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
        self.make_ahead(pages_to_make(self.free_count, self.coming, count));
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
                if self.free_count == 0 {
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
                let Some(slot) = self.take_slot() else {
                    unreachable!("a page was just added");
                };
                // A page in reserve, so the next one is made before it is
                // needed.
                if self.free_count + self.coming * usize::from(SLOTS) < usize::from(SLOTS) {
                    self.make_ahead(1);
                }
                self.used.insert(key, slot);
                slot
            }
        };
        let region = slot.region();
        let Some(page) = &self.pages[slot.page] else {
            unreachable!("a slot in use is on a page that is there");
        };
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &page.texture,
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
            id: page.id,
            region,
        }
    }

    /// Frees `key`'s slot, if it has one; a page left empty is given back
    /// while two pages' worth of free slots stay (with those on their
    /// way). A page's worth would be one slot from making a page again
    /// ([`Self::upload`] keeps one in reserve): on a long pan, the last
    /// page draining while new tiles fill the first would make and give
    /// back a page every few frames. So two empty pages stay when every
    /// tile is gone.
    pub(crate) fn free(&mut self, key: TextureKey, bindings: &mut vello_gpu::TextureBindings) {
        let Some(slot) = self.used.remove(&key) else {
            return;
        };
        self.free[slot.page].push(slot.index);
        self.free_count += 1;
        let slots = usize::from(SLOTS);
        let others = self.free_count - slots + self.coming * slots;
        if self.free[slot.page].len() == slots && others >= 2 * slots {
            if let Some(page) = self.pages[slot.page].take() {
                bindings.remove(page.id);
                // The texture goes with it.
            }
            self.free[slot.page].clear();
            self.free_count -= slots;
        }
    }

    /// Pages there are now.
    pub(crate) fn len(&self) -> usize {
        self.pages.iter().flatten().count()
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
    fn empty_pages_are_given_back_and_uploads_fill_the_first_pages() {
        let Some((device, queue)) = crate::gpu::device_for_tests() else {
            eprintln!("no GPU: skipped");
            return;
        };
        let slots = usize::from(SLOTS);
        let mut pages = TilePages::new(&device, &queue);
        let mut bindings = vello_gpu::TextureBindings::new();
        let pixmap = Pixmap::new(SLOT, SLOT);
        let mut stats = PageStats::default();
        // The pages on their way, all here, so the counts are exact.
        let arrive = |pages: &mut TilePages, bindings: &mut vello_gpu::TextureBindings| {
            let started = std::time::Instant::now();
            while pages.coming() > 0 && started.elapsed() < std::time::Duration::from_secs(30) {
                pages.receive(bindings);
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            assert_eq!(pages.coming(), 0, "pages still on their way");
        };
        let keys: Vec<TextureKey> = (0..3 * slots).map(|_| TextureKey::unique()).collect();
        for key in &keys {
            pages.upload(&mut bindings, *key, &pixmap, &mut stats);
        }
        arrive(&mut pages, &mut bindings);
        // Three pages full, in order, and one in reserve.
        assert_eq!(pages.len(), 4);
        assert_eq!(pages.free_count, slots);
        for (at, key) in keys.iter().enumerate() {
            assert_eq!(pages.used[key].page, at / slots);
        }
        // The second and third pages' tiles go. The second stays (the
        // reserve's and its own free slots are only two pages' worth with
        // it); the third is given back, with its binding.
        let third = pages.pages[2].as_ref().map(|page| page.id);
        for key in &keys[slots..] {
            pages.free(*key, &mut bindings);
        }
        assert_eq!(pages.len(), 3);
        assert!(pages.pages[2].is_none());
        assert!(third.is_some_and(|id| bindings.remove(id).is_none()));
        // New tiles fill the lowest page with room (the second), not the
        // reserve.
        let more: Vec<TextureKey> = (0..10).map(|_| TextureKey::unique()).collect();
        for key in &more {
            pages.upload(&mut bindings, *key, &pixmap, &mut stats);
            assert_eq!(pages.used[key].page, 1);
        }
        arrive(&mut pages, &mut bindings);
        assert_eq!(pages.len(), 3);
        // All gone: two pages' worth of free slots stay.
        for key in keys[..slots].iter().chain(&more) {
            pages.free(*key, &mut bindings);
        }
        assert_eq!(pages.len(), 2);
        assert_eq!(pages.free_count, 2 * slots);
    }

    #[test]
    fn a_page_given_back_is_not_made_again_at_once() {
        let Some((device, queue)) = crate::gpu::device_for_tests() else {
            eprintln!("no GPU: skipped");
            return;
        };
        let slots = usize::from(SLOTS);
        let mut pages = TilePages::new(&device, &queue);
        let mut bindings = vello_gpu::TextureBindings::new();
        let pixmap = Pixmap::new(SLOT, SLOT);
        let mut stats = PageStats::default();
        // A long pan: tiles come in at the front and go at the back, a
        // few a frame, with about three pages' worth on screen.
        let mut shown: std::collections::VecDeque<TextureKey> = std::collections::VecDeque::new();
        for _ in 0..3 * slots {
            let key = TextureKey::unique();
            pages.upload(&mut bindings, key, &pixmap, &mut stats);
            shown.push_back(key);
        }
        let started = pages.added + pages.coming();
        let mut made_since = 0;
        for _ in 0..40 * slots {
            pages.receive(&mut bindings);
            let key = TextureKey::unique();
            pages.upload(&mut bindings, key, &pixmap, &mut stats);
            shown.push_back(key);
            if let Some(gone) = shown.pop_front() {
                pages.free(gone, &mut bindings);
            }
            made_since = pages.added + pages.coming() - started;
        }
        // Without room to spare, each page given back is made again a
        // slot later: about one page per page of uploads.
        assert!(made_since <= 2, "{made_since} pages made on the pan");
        assert!(pages.len() <= 6, "{} pages", pages.len());
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
