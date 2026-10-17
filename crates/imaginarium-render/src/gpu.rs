// SPDX-License-Identifier: AGPL-3.0-or-later

//! The GPU backend: `vello_gpu` on `wgpu`.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use vello_common::geometry::RectU16;
use vello_common::kurbo::Affine;
use vello_common::peniko::Color;
use vello_cpu::{PixelMetadata, Pixmap};

use crate::RenderError;
use crate::gpu_timer::{GpuFrameTime, GpuTimer};
use crate::scene::{
    Placed, Scene, Target, TextureKey, TextureWork, Textures as SceneTextures, replay,
};
use crate::tile_pages::{PageStats, SLOT, TilePages, UPLOAD_FORMAT};

/// Which GPU to prefer on machines with more than one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Power {
    /// The integrated GPU: what the performance budgets are set for
    /// (ADR-0007).
    #[default]
    Low,
    /// The discrete GPU.
    High,
}

/// How to choose and drive the GPU.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GpuOptions {
    /// Integrated or discrete, when no `adapter` is named.
    pub power: Power,
    /// Use the adapter whose name contains this (case-insensitive), e.g.
    /// "intel" or "3050".
    pub adapter: Option<String>,
    /// Present without waiting for vertical sync (for measuring).
    pub no_vsync: bool,
    /// Rasterize glyphs once into an atlas and draw them from there,
    /// instead of building every glyph's strips each frame. About 3× less
    /// encode time for text-heavy frames (measured in the M2 gate app);
    /// Vello calls it experimental, so it can be turned off. Filling the
    /// atlas makes the frame that first uses it slower, so the first frame
    /// (cold start) is drawn without it.
    pub glyph_atlas: bool,
    /// Time the GPU's work for each frame with timestamp queries, if the
    /// GPU can ([`FrameTiming::gpu`]). For measuring; it costs a few
    /// timestamps and a small read-back per frame.
    pub gpu_times: bool,
}

/// What the GPU is, for reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GpuInfo {
    /// The adapter's name.
    pub name: String,
    /// The graphics API in use (Vulkan, Metal, Dx12, …).
    pub backend: String,
    /// Integrated, discrete, virtual or CPU (software).
    pub device_type: String,
    /// The driver and its version.
    pub driver: String,
}

impl GpuInfo {
    fn of(adapter: &wgpu::Adapter) -> Self {
        let info = adapter.get_info();
        Self {
            name: info.name,
            backend: format!("{:?}", info.backend),
            device_type: format!("{:?}", info.device_type),
            driver: format!("{} {}", info.driver, info.driver_info)
                .trim()
                .to_owned(),
        }
    }
}

#[derive(Clone)]
struct Gpu {
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
}

fn instance() -> wgpu::Instance {
    // The primary APIs only (Vulkan, Metal, DirectX 12): enabling OpenGL
    // too loads its drivers at startup (EGL), which costs time on every
    // start for a backend that is never chosen while one of those works.
    // `WGPU_BACKEND` and friends still work, which helps on odd machines.
    wgpu::Instance::new(
        wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        }
        .with_env(),
    )
}

/// How long opening the GPU took, step by step (the gate's cold start).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GpuStartup {
    /// Loading the graphics drivers (the API's instance).
    pub instance: Duration,
    /// Choosing the GPU and opening it.
    pub device: Duration,
    /// Creating and configuring the window's surface.
    pub surface: Duration,
    /// Creating Vello's pipelines (the driver compiles its shaders).
    pub renderer: Duration,
    /// How long the window waited for the GPU thread
    /// ([`WindowRenderer::with_start`]), after its own startup work.
    pub waited: Duration,
}

/// A GPU being opened on a thread of its own, so the drivers load while the
/// program does the rest of its startup (fonts, laying out the first
/// frame). [`WindowRenderer::with_start`] takes it over; the window's
/// surface and the renderer follow there, as they need the window.
#[derive(Debug)]
pub struct GpuStart {
    options: GpuOptions,
    thread: Option<std::thread::JoinHandle<Result<OpenedGpu, RenderError>>>,
}

struct OpenedGpu {
    instance: wgpu::Instance,
    gpu: Gpu,
    /// The renderer, built for [`LIKELY_FORMAT`] before the window's
    /// surface says which format it takes.
    renderer: Option<(vello_gpu::Renderer, vello_gpu::Resources)>,
    startup: GpuStartup,
}

/// The format window surfaces take almost everywhere (Vulkan on Linux and
/// Windows, Metal, DirectX), so the renderer can be built before the
/// window exists; built again if the surface doesn't take it.
const LIKELY_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8Unorm;

impl GpuStart {
    /// Starts opening the GPU that `options` ask for.
    pub fn spawn(options: &GpuOptions) -> Self {
        let wanted = options.clone();
        let thread = std::thread::Builder::new()
            .name("gpu-open".into())
            .spawn(move || {
                let started = Instant::now();
                let instance = instance();
                let loaded = started.elapsed();
                let gpu = pollster::block_on(open(&instance, &wanted, None))?;
                let opened = started.elapsed();
                let renderer = new_renderer(&gpu.device, LIKELY_FORMAT, 1, 1);
                Ok(OpenedGpu {
                    instance,
                    gpu,
                    renderer: Some(renderer),
                    startup: GpuStartup {
                        instance: loaded,
                        device: opened - loaded,
                        renderer: started.elapsed() - opened,
                        ..GpuStartup::default()
                    },
                })
            })
            // Without a thread, the window opens the GPU itself.
            .ok();
        Self {
            options: options.clone(),
            thread,
        }
    }

    /// The options it was started with.
    pub fn options(&self) -> &GpuOptions {
        &self.options
    }
}

async fn open(
    instance: &wgpu::Instance,
    options: &GpuOptions,
    surface: Option<&wgpu::Surface<'_>>,
) -> Result<Gpu, RenderError> {
    let adapter = match &options.adapter {
        Some(wanted) => {
            let wanted = wanted.to_lowercase();
            let mut found = None;
            for adapter in instance.enumerate_adapters(wgpu::Backends::all()).await {
                let name = adapter.get_info().name.to_lowercase();
                if name.contains(&wanted) && surface.is_none_or(|s| adapter.is_surface_supported(s))
                {
                    found = Some(adapter);
                    break;
                }
            }
            found
                .ok_or_else(|| RenderError::NoAdapter(format!("no GPU named like \"{wanted}\"")))?
        }
        None => instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: match options.power {
                    Power::Low => wgpu::PowerPreference::LowPower,
                    Power::High => wgpu::PowerPreference::HighPerformance,
                },
                force_fallback_adapter: false,
                compatible_surface: surface,
                // Bucketing hides exact limits from untrusted content; we
                // run no untrusted content on the GPU.
                apply_limit_buckets: false,
            })
            .await
            .map_err(|err| RenderError::NoAdapter(err.to_string()))?,
    };
    let required_features = if options.gpu_times {
        adapter.features() & GpuTimer::FEATURES
    } else {
        wgpu::Features::empty()
    };
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("imaginarium"),
            required_features,
            // Smaller memory blocks (8 to 64 MB rather than 128 to 256 MB).
            // A block's memory may be backed only when a submission first
            // uses it, and an integrated GPU's driver then allocates and
            // clears it in that submission: the bigger the block, the
            // longer that frame (the gate laptop's Intel GPU stalled in
            // single submissions for 58-117 ms).
            memory_hints: wgpu::MemoryHints::MemoryUsage,
            ..Default::default()
        })
        .await
        .map_err(|err| RenderError::Device(err.to_string()))?;
    Ok(Gpu {
        adapter,
        device,
        queue,
    })
}

/// A Vello renderer for targets of `format`.
///
/// Its glyph atlas (which takes the image atlas's settings) has pages of
/// 2048² rather than Vello's 4096²: a page is created when a glyph first
/// needs it, and wgpu clears the whole texture on its first use, which made
/// the first frame with text stall for 70 ms on the VM (64 MB). A quarter
/// of that holds the UI's and a board's glyphs; more pages come as needed,
/// up to 32 (Vello's 8 of 4096² hold as much). A full atlas draws glyphs
/// directly, which vello_gpu can't do for bitmap glyphs (color emoji).
fn new_renderer(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    width: u16,
    height: u16,
) -> (vello_gpu::Renderer, vello_gpu::Resources) {
    let mut settings = vello_gpu::RenderSettings::default();
    settings.memory_settings.image_atlas_config.atlas_size = (ATLAS_PAGE, ATLAS_PAGE);
    settings.memory_settings.image_atlas_config.max_atlases = ATLAS_PAGES;
    vello_gpu::Renderer::new_with(
        device,
        &vello_gpu::RenderTargetConfig {
            format,
            width,
            height,
        },
        settings,
    )
}

/// The side of a glyph (and image) atlas page, in pixels.
const ATLAS_PAGE: u16 = 2048;
/// Glyph atlas pages at most: 512 MB, as many glyphs as Vello's default.
const ATLAS_PAGES: usize = 32;

/// Vello works with `u16` sizes; a zero-sized surface is invalid.
fn clamp_size(width: u32, height: u32) -> (u16, u16) {
    let clamp = |v: u32| u16::try_from(v.max(1)).unwrap_or(u16::MAX);
    (clamp(width), clamp(height))
}

/// A surface size the device can make textures of: a window bigger than
/// the GPU's limit (a huge virtual desktop) would fail validation.
fn surface_size(device: &wgpu::Device, width: u32, height: u32) -> (u16, u16) {
    let max = device.limits().max_texture_dimension_2d.max(1);
    clamp_size(width.min(max), height.min(max))
}

/// Where a presented frame's time went on the calling thread. The GPU's
/// own work isn't included: it runs after `submit`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameTiming {
    /// Waiting for the window's next texture (blocks while the swapchain
    /// is full, i.e. on vertical sync).
    pub acquire: Duration,
    /// Turning the scene into GPU work: Vello flattens paths and builds
    /// its strips on the CPU here.
    pub encode: Duration,
    /// Submitting the work and presenting.
    pub submit: Duration,
    /// Of `submit`: presenting. Some systems block here until the GPU or
    /// the display catches up (Mesa's software GPU on X11): a wait, not
    /// work.
    pub present: Duration,
    /// Rendering textures (canvas tiles) before the frame: encoding and
    /// submitting each.
    pub textures: Duration,
    /// How many textures were rendered.
    pub texture_renders: u32,
    /// Of `textures`: creating textures (a tile's first render).
    pub textures_create: Duration,
    /// Of `textures`: submitting the renders, one submission each.
    pub textures_submit: Duration,
    /// How many textures were uploaded (canvas tiles rendered elsewhere).
    pub texture_uploads: u32,
    /// Of `textures`: handing uploads to the GPU (copying them for the
    /// frame's submission).
    pub textures_upload: Duration,
    /// Tile pages (shared textures for uploaded tiles) in use.
    pub tile_pages: u32,
    /// Tile pages the frame made, or waited for, because none was ready:
    /// each can make the frame slow.
    pub tile_pages_made: u32,
    /// Tile pages being made on their thread when the frame's texture work
    /// began (whether a slow upload may have waited behind one).
    pub tile_pages_coming: u32,
    /// Draws of textures that don't exist, skipped (a bug in their owner).
    pub missing_textures: u32,
    /// The GPU's time for an earlier frame, read back by now (with
    /// [`GpuOptions::gpu_times`]); its number says which.
    pub gpu: Option<GpuFrameTime>,
    /// The driver called this frame's texture suboptimal: it still works,
    /// but the surface no longer matches what the window system wants.
    pub suboptimal: bool,
    /// The surface was set up again before this frame, because the last
    /// texture was suboptimal. That is part of `acquire`.
    pub reconfigured: bool,
    /// Frames skipped since the last presented one ([`Skip`]).
    pub skipped_before: u32,
}

/// What happened to a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[expect(
    clippy::large_enum_variant,
    reason = "returned once per frame and taken apart at once, never stored"
)]
pub enum FrameOutcome {
    /// It is on its way to the screen.
    Presented(FrameTiming),
    /// The window wasn't ready: nothing was drawn.
    Skipped(Skip),
}

/// Why a frame was skipped, which says when to draw again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Skip {
    /// The surface changed (a resize under way) and was set up again:
    /// draw again now.
    Reconfigured,
    /// The surface was lost: call [`WindowRenderer::recreate_surface`],
    /// then draw again.
    Lost,
    /// The window is hidden (minimized, on another workspace): draw again
    /// once it shows.
    Occluded,
    /// No texture came in time (the GPU is busy): draw again a little
    /// later, not at once.
    Timeout,
}

/// Draws scenes into a window with `vello_gpu`.
pub struct WindowRenderer {
    instance: wgpu::Instance,
    gpu: Gpu,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    /// The last texture was suboptimal: set the surface up again before
    /// the next one.
    reconfigure: bool,
    /// Frames skipped since the last presented one.
    skipped: u32,
    /// The format Vello renders in: never sRGB, because Vello's colors are
    /// sRGB-encoded already and must not be encoded twice.
    view_format: wgpu::TextureFormat,
    renderer: vello_gpu::Renderer,
    resources: vello_gpu::Resources,
    depth: wgpu::TextureView,
    scene: vello_gpu::Scene,
    info: GpuInfo,
    glyph_atlas: bool,
    frames: u64,
    textures: Textures,
    bitmap_fonts: BitmapFonts,
    startup: GpuStartup,
    timer: Option<GpuTimer>,
    /// What it was opened with, for windows that share its GPU.
    options: GpuOptions,
}

/// Which fonts have bitmap glyphs (color emoji in CBDT or sbix, EBDT), by
/// font file and face: vello_gpu draws those only from its glyph atlas.
#[derive(Debug, Default)]
pub(crate) struct BitmapFonts {
    known: HashMap<(u64, u32), bool>,
}

impl BitmapFonts {
    pub(crate) fn has_bitmaps(&mut self, font: &vello_common::peniko::FontData) -> bool {
        *self
            .known
            .entry((font.data.id(), font.index))
            .or_insert_with(|| {
                skrifa::FontRef::from_index(font.data.data(), font.index).is_ok_and(|face| {
                    [b"CBDT", b"sbix", b"EBDT"]
                        .iter()
                        .any(|tag| face.table_data(skrifa::Tag::new(tag)).is_some())
                })
            })
    }
}

/// What [`Textures::process`] did, and where its time went.
#[derive(Clone, Copy, Debug, Default)]
struct TextureStats {
    renders: u32,
    create: Duration,
    submit: Duration,
    uploads: u32,
    upload: Duration,
    pages: PageStats,
}

/// Textures kept for their owners across frames (canvas tiles).
struct Textures {
    /// The pixel format of rendered textures (the window's).
    format: wgpu::TextureFormat,
    /// Textures of their own: rendered ones, and uploads that aren't a
    /// tile's size.
    own: HashMap<TextureKey, wgpu::Texture>,
    /// Uploaded tiles, in shared pages.
    pages: TilePages,
    /// Where each texture's texels are, for drawing.
    placed: HashMap<TextureKey, Placed>,
    bindings: vello_gpu::TextureBindings,
    /// Depth views by size, shared by textures of that size.
    depths: HashMap<(u16, u16), wgpu::TextureView>,
    /// Scratch scene the textures' content is encoded in.
    scene: vello_gpu::Scene,
}

impl Textures {
    fn new(format: wgpu::TextureFormat, gpu: &Gpu) -> Self {
        Self {
            format,
            own: HashMap::new(),
            pages: TilePages::new(&gpu.device, &gpu.queue),
            placed: HashMap::new(),
            bindings: vello_gpu::TextureBindings::new(),
            depths: HashMap::new(),
            scene: vello_gpu::Scene::new(1, 1),
        }
    }

    fn free(&mut self, key: TextureKey) {
        self.drop_own(key);
        self.pages.free(key);
        self.placed.remove(&key);
    }

    fn drop_own(&mut self, key: TextureKey) {
        if self.own.remove(&key).is_some() {
            self.bindings.remove(vello_common::TextureId(key.id()));
        }
    }

    /// `key`'s own texture of `width × height` in `format`, made (and
    /// bound) if it has none of that kind.
    fn own_texture(
        &mut self,
        device: &wgpu::Device,
        key: TextureKey,
        (width, height): (u16, u16),
        format: wgpu::TextureFormat,
        stats: &mut TextureStats,
    ) -> wgpu::Texture {
        let fits = self.own.get(&key).is_some_and(|texture| {
            (texture.width(), texture.height(), texture.format())
                == (u32::from(width), u32::from(height), format)
        });
        if !fits {
            let creating = Instant::now();
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("canvas texture"),
                size: wgpu::Extent3d {
                    width: u32::from(width),
                    height: u32::from(height),
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let id = vello_common::TextureId(key.id());
            self.bindings.insert(
                id,
                texture.create_view(&wgpu::TextureViewDescriptor::default()),
            );
            self.own.insert(key, texture);
            self.placed.insert(
                key,
                Placed {
                    id,
                    region: RectU16 {
                        x0: 0,
                        y0: 0,
                        x1: width,
                        y1: height,
                    },
                },
            );
            stats.create += creating.elapsed();
        }
        match self.own.get(&key) {
            Some(texture) => texture.clone(),
            None => unreachable!("made above"),
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the window renderer's parts, borrowed apart from each other"
    )]
    /// Renders, uploads and frees textures as `work` says. Each render is a
    /// submission of its own: Vello rewrites its buffers between renders,
    /// so renders sharing one submission would see the last one's data.
    /// Uploads go with the next submission.
    fn process(
        &mut self,
        gpu: &Gpu,
        renderer: &mut vello_gpu::Renderer,
        resources: &mut vello_gpu::Resources,
        bitmap_fonts: &mut BitmapFonts,
        mut timer: Option<&mut GpuTimer>,
        glyph_atlas: bool,
        work: &[TextureWork],
    ) -> Result<TextureStats, RenderError> {
        let mut stats = TextureStats::default();
        // All of the work is done even when a render fails: the owners
        // count on their uploads and frees.
        let mut failed = None;
        self.pages.receive(&mut self.bindings);
        stats.pages.coming = u32::try_from(self.pages.coming()).unwrap_or(u32::MAX);
        for item in work {
            match item {
                TextureWork::Free(key) => self.free(*key),
                TextureWork::Expect(count) => self.pages.expect(*count),
                TextureWork::Upload { key, pixmap } => {
                    let uploading = Instant::now();
                    let pixmap = &pixmap.0;
                    let size = (pixmap.width(), pixmap.height());
                    if size.0 == 0 || size.1 == 0 {
                        self.free(*key);
                    } else if size == (SLOT, SLOT) {
                        self.drop_own(*key);
                        let placed =
                            self.pages
                                .upload(&mut self.bindings, *key, pixmap, &mut stats.pages);
                        self.placed.insert(*key, placed);
                    } else {
                        self.pages.free(*key);
                        let texture =
                            self.own_texture(&gpu.device, *key, size, UPLOAD_FORMAT, &mut stats);
                        gpu.queue.write_texture(
                            texture.as_image_copy(),
                            pixmap.data_as_u8_slice(),
                            wgpu::TexelCopyBufferLayout {
                                offset: 0,
                                bytes_per_row: Some(u32::from(size.0) * 4),
                                rows_per_image: None,
                            },
                            texture.size(),
                        );
                    }
                    stats.uploads += 1;
                    stats.upload += uploading.elapsed();
                }
                TextureWork::Render {
                    key,
                    width,
                    height,
                    scene,
                } => {
                    let (w, h) = ((*width).max(1), (*height).max(1));
                    self.pages.free(*key);
                    let texture =
                        self.own_texture(&gpu.device, *key, (w, h), self.format, &mut stats);
                    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                    let size = vello_gpu::RenderSize {
                        width: w,
                        height: h,
                    };
                    let depth = self.depths.entry((w, h)).or_insert_with(|| {
                        vello_gpu::Renderer::create_depth_texture_view(&gpu.device, &size)
                    });
                    self.scene.reset_and_resize(w, h);
                    replay(
                        scene,
                        &mut Target {
                            context: &mut self.scene,
                            resources: &mut *resources,
                            glyph_atlas,
                            bitmap_fonts: Some(&mut *bitmap_fonts),
                            transform: Affine::IDENTITY,
                            textures: SceneTextures::None,
                            missing: 0,
                        },
                        Affine::IDENTITY,
                    );
                    let mut encoder =
                        gpu.device
                            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                                label: Some("canvas texture"),
                            });
                    let timed = timer
                        .as_deref_mut()
                        .and_then(|timer| timer.texture_start(&mut encoder));
                    let rendered = renderer
                        .render(
                            &self.scene,
                            resources,
                            &gpu.device,
                            &gpu.queue,
                            &mut encoder,
                            &size,
                            &view,
                            Some(depth),
                            &vello_gpu::TextureBindings::new(),
                            vello_gpu::TargetInit::Clear(vello_gpu::ClearSettings::Viewport {
                                color: Color::TRANSPARENT,
                            }),
                        )
                        .map_err(|err| RenderError::Render(err.to_string()));
                    // Submitted either way: its timestamps are counted on.
                    if let Some(timer) = timer.as_deref_mut() {
                        timer.end(&mut encoder, timed);
                    }
                    let submitting = Instant::now();
                    gpu.queue.submit([encoder.finish()]);
                    stats.submit += submitting.elapsed();
                    match rendered {
                        Ok(()) => stats.renders += 1,
                        Err(err) => {
                            // Missing rather than showing whatever the
                            // texture held before.
                            self.free(*key);
                            failed.get_or_insert(err);
                        }
                    }
                }
            }
        }
        match failed {
            Some(err) => Err(err),
            None => Ok(stats),
        }
    }
}

impl WindowRenderer {
    /// Opens the GPU for `window` (anything with a window and display
    /// handle, such as an `Arc<winit::window::Window>`), with its inner size
    /// in physical pixels.
    pub fn new(
        window: impl Into<wgpu::SurfaceTarget<'static>>,
        width: u32,
        height: u32,
        options: &GpuOptions,
    ) -> Result<Self, RenderError> {
        let started = Instant::now();
        let instance = instance();
        let loaded = started.elapsed();
        let surface = instance
            .create_surface(window)
            .map_err(|err| RenderError::Surface(err.to_string()))?;
        let opening = Instant::now();
        let gpu = pollster::block_on(open(&instance, options, Some(&surface)))?;
        let startup = GpuStartup {
            instance: loaded,
            device: opening.elapsed(),
            ..GpuStartup::default()
        };
        Self::finish(
            instance, surface, gpu, None, width, height, options, startup,
        )
    }

    /// Opens the window's renderer on the GPU that `start` opened in the
    /// meantime, waiting for it if it isn't open yet. If that GPU can't
    /// draw to this window, or the thread failed, it opens one as
    /// [`WindowRenderer::new`] does.
    pub fn with_start(
        start: GpuStart,
        window: impl Into<wgpu::SurfaceTarget<'static>>,
        width: u32,
        height: u32,
    ) -> Result<Self, RenderError> {
        let waiting = Instant::now();
        let opened = start.thread.and_then(|thread| thread.join().ok());
        let waited = waiting.elapsed();
        let Some(opened) = opened else {
            return Self::new(window, width, height, &start.options);
        };
        let OpenedGpu {
            instance,
            gpu,
            renderer,
            mut startup,
        } = opened?;
        startup.waited = waited;
        let creating = Instant::now();
        let surface = instance
            .create_surface(window)
            .map_err(|err| RenderError::Surface(err.to_string()))?;
        let (gpu, renderer) = if gpu.adapter.is_surface_supported(&surface) {
            (gpu, renderer)
        } else {
            let other = pollster::block_on(open(&instance, &start.options, Some(&surface)))?;
            (other, None)
        };
        startup.surface = creating.elapsed();
        Self::finish(
            instance,
            surface,
            gpu,
            renderer,
            width,
            height,
            &start.options,
            startup,
        )
    }

    /// The surface's configuration and the renderer, on an open GPU.
    #[expect(clippy::too_many_arguments, reason = "the parts of a renderer, once")]
    fn finish(
        instance: wgpu::Instance,
        surface: wgpu::Surface<'static>,
        gpu: Gpu,
        built: Option<(vello_gpu::Renderer, vello_gpu::Resources)>,
        width: u32,
        height: u32,
        options: &GpuOptions,
        mut startup: GpuStartup,
    ) -> Result<Self, RenderError> {
        let configuring = Instant::now();
        let capabilities = surface.get_capabilities(&gpu.adapter);
        // The format a renderer was built for already, else plain 8-bit
        // RGBA or BGRA, else whatever comes first.
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(|f| built.is_some() && *f == LIKELY_FORMAT)
            .or_else(|| {
                capabilities.formats.iter().copied().find(|f| {
                    matches!(
                        f,
                        wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Bgra8Unorm
                    )
                })
            })
            .or_else(|| capabilities.formats.first().copied())
            .ok_or_else(|| RenderError::Surface("the surface supports no format".into()))?;
        let view_format = format.remove_srgb_suffix();
        let (w, h) = surface_size(&gpu.device, width, height);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: u32::from(w),
            height: u32::from(h),
            present_mode: if options.no_vsync {
                wgpu::PresentMode::AutoNoVsync
            } else {
                wgpu::PresentMode::AutoVsync
            },
            desired_maximum_frame_latency: 2,
            // Opaque where offered: the frame covers the window, and a
            // compositor treats a window with alpha as see-through (it
            // blends, maybe blurs, what is behind it every frame).
            alpha_mode: if capabilities
                .alpha_modes
                .contains(&wgpu::CompositeAlphaMode::Opaque)
            {
                wgpu::CompositeAlphaMode::Opaque
            } else {
                capabilities
                    .alpha_modes
                    .first()
                    .copied()
                    .unwrap_or(wgpu::CompositeAlphaMode::Auto)
            },
            view_formats: if view_format == format {
                Vec::new()
            } else {
                vec![view_format]
            },
            // Plain sRGB, as before wgpu 30 (no HDR or wide gamut yet).
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        surface.configure(&gpu.device, &config);
        startup.surface += configuring.elapsed();
        let creating = Instant::now();
        let (renderer, resources) = match built {
            Some(built) if view_format == LIKELY_FORMAT => built,
            _ => new_renderer(&gpu.device, view_format, w, h),
        };
        let depth = vello_gpu::Renderer::create_depth_texture_view(
            &gpu.device,
            &vello_gpu::RenderSize {
                width: w,
                height: h,
            },
        );
        startup.renderer += creating.elapsed();
        let info = GpuInfo::of(&gpu.adapter);
        let timer = options
            .gpu_times
            .then(|| GpuTimer::new(&gpu.device, &gpu.queue))
            .flatten();
        let textures = Textures::new(view_format, &gpu);
        Ok(Self {
            timer,
            instance,
            gpu,
            surface,
            config,
            reconfigure: false,
            skipped: 0,
            view_format,
            renderer,
            resources,
            depth,
            scene: vello_gpu::Scene::new(w, h),
            info,
            glyph_atlas: options.glyph_atlas,
            frames: 0,
            textures,
            bitmap_fonts: BitmapFonts::default(),
            startup,
            options: options.clone(),
        })
    }

    /// The GPU it draws with, for more windows on it (one window per
    /// document): they share the device instead of opening the GPU again.
    pub fn shared_gpu(&self) -> SharedGpu {
        SharedGpu {
            instance: self.instance.clone(),
            gpu: self.gpu.clone(),
            options: self.options.clone(),
            spare: Spare::None,
        }
    }

    /// Closes the renderer, handing its Vello renderer to `shared` for the
    /// next window if it has none ready (its pipelines took long to
    /// build).
    pub fn recycle(self, shared: &mut SharedGpu) {
        if self.view_format == LIKELY_FORMAT && matches!(shared.spare, Spare::None) {
            shared.spare = Spare::Ready(Box::new((self.renderer, self.resources)));
        }
    }

    /// The GPU in use.
    pub fn info(&self) -> &GpuInfo {
        &self.info
    }

    /// How long opening it took.
    pub fn startup(&self) -> &GpuStartup {
        &self.startup
    }

    /// The surface size in physical pixels.
    pub fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    /// Makes a new surface for `window` after [`Skip::Lost`], with the
    /// same setup.
    ///
    /// # Errors
    /// If the window has no surface any more.
    pub fn recreate_surface(
        &mut self,
        window: impl Into<wgpu::SurfaceTarget<'static>>,
    ) -> Result<(), RenderError> {
        let surface = self
            .instance
            .create_surface(window)
            .map_err(|err| RenderError::Surface(err.to_string()))?;
        surface.configure(&self.gpu.device, &self.config);
        self.surface = surface;
        self.reconfigure = false;
        Ok(())
    }

    /// Follows a window resize (physical pixels).
    pub fn resize(&mut self, width: u32, height: u32) {
        let (w, h) = surface_size(&self.gpu.device, width, height);
        if (u32::from(w), u32::from(h)) == self.size() {
            return;
        }
        self.config.width = u32::from(w);
        self.config.height = u32::from(h);
        self.surface.configure(&self.gpu.device, &self.config);
        self.depth = vello_gpu::Renderer::create_depth_texture_view(
            &self.gpu.device,
            &vello_gpu::RenderSize {
                width: w,
                height: h,
            },
        );
    }

    /// Draws `scene` over `background` and presents it. `transform` maps
    /// the scene's coordinates to physical pixels (the window's scale
    /// factor, for a scene in logical pixels).
    pub fn render(
        &mut self,
        scene: &Scene,
        background: Color,
        transform: Affine,
    ) -> Result<FrameOutcome, RenderError> {
        // Texture work first, even if this frame is then skipped: the
        // owners count on it being done.
        let start = Instant::now();
        let gpu = self
            .timer
            .as_mut()
            .and_then(|timer| timer.collect(&self.gpu.device));
        if let Some(timer) = &mut self.timer {
            timer.begin_frame(self.frames + 1);
        }
        let glyph_atlas = self.glyph_atlas && self.frames > 0;
        let texture_stats = self.textures.process(
            &self.gpu,
            &mut self.renderer,
            &mut self.resources,
            &mut self.bitmap_fonts,
            self.timer.as_mut(),
            glyph_atlas,
            scene.texture_work(),
        )?;
        let textured = Instant::now();
        let reconfigured = std::mem::take(&mut self.reconfigure);
        if reconfigured {
            self.surface.configure(&self.gpu.device, &self.config);
        }
        let mut suboptimal = false;
        let skip = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture) => Ok(texture),
            wgpu::CurrentSurfaceTexture::Suboptimal(texture) => {
                // Still drawable: draw it, and set up the surface again
                // before the next frame.
                suboptimal = true;
                self.reconfigure = true;
                Ok(texture)
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.gpu.device, &self.config);
                Err(Skip::Reconfigured)
            }
            wgpu::CurrentSurfaceTexture::Lost => Err(Skip::Lost),
            wgpu::CurrentSurfaceTexture::Occluded => Err(Skip::Occluded),
            wgpu::CurrentSurfaceTexture::Timeout => Err(Skip::Timeout),
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err(RenderError::Surface(
                    "the surface texture failed validation".into(),
                ));
            }
        };
        let texture = match skip {
            Ok(texture) => texture,
            Err(skip) => {
                self.skipped = self.skipped.saturating_add(1);
                self.finish_timing();
                return Ok(FrameOutcome::Skipped(skip));
            }
        };
        let acquired = Instant::now();
        let view = texture.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(self.view_format),
            ..Default::default()
        });
        let (w, h) = clamp_size(self.config.width, self.config.height);
        self.scene.reset_and_resize(w, h);
        let mut target = Target {
            context: &mut self.scene,
            resources: &mut self.resources,
            glyph_atlas,
            bitmap_fonts: Some(&mut self.bitmap_fonts),
            transform: Affine::IDENTITY,
            textures: SceneTextures::Bound(&self.textures.placed),
            missing: 0,
        };
        replay(scene, &mut target, transform);
        let missing_textures = target.missing;
        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        let timed = self
            .timer
            .as_mut()
            .and_then(|timer| timer.scene_start(&mut encoder));
        self.renderer
            .render(
                &self.scene,
                &mut self.resources,
                &self.gpu.device,
                &self.gpu.queue,
                &mut encoder,
                &vello_gpu::RenderSize {
                    width: w,
                    height: h,
                },
                &view,
                Some(&self.depth),
                &self.textures.bindings,
                vello_gpu::TargetInit::Clear(vello_gpu::ClearSettings::Viewport {
                    color: background,
                }),
            )
            .map_err(|err| RenderError::Render(err.to_string()))?;
        if let Some(timer) = &mut self.timer {
            timer.end(&mut encoder, timed);
            timer.finish(&mut encoder);
        }
        let encoded = Instant::now();
        self.gpu.queue.submit([encoder.finish()]);
        if let Some(timer) = &mut self.timer {
            timer.submitted();
        }
        let presenting = Instant::now();
        self.gpu.queue.present(texture);
        let present = presenting.elapsed();
        self.frames += 1;
        Ok(FrameOutcome::Presented(FrameTiming {
            acquire: acquired - textured,
            encode: encoded - acquired,
            submit: encoded.elapsed(),
            present,
            textures: textured - start,
            texture_renders: texture_stats.renders,
            textures_create: texture_stats.create,
            textures_submit: texture_stats.submit,
            texture_uploads: texture_stats.uploads,
            textures_upload: texture_stats.upload,
            tile_pages: u32::try_from(self.textures.pages.len()).unwrap_or(u32::MAX),
            tile_pages_made: texture_stats.pages.made,
            tile_pages_coming: texture_stats.pages.coming,
            missing_textures,
            gpu,
            suboptimal,
            reconfigured,
            skipped_before: std::mem::take(&mut self.skipped),
        }))
    }

    /// Sends a skipped frame's timestamps (its texture renders) on their
    /// way back.
    fn finish_timing(&mut self) {
        if let Some(timer) = &mut self.timer {
            let mut encoder =
                self.gpu
                    .device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some("frame timestamps"),
                    });
            timer.finish(&mut encoder);
            self.gpu.queue.submit([encoder.finish()]);
            timer.submitted();
        }
    }
}

/// A GPU that windows share (from [`WindowRenderer::shared_gpu`]), with
/// a Vello renderer ready for the next window: building one compiles its
/// pipelines, which takes a while (130 ms on llvmpipe) and so happens off
/// the UI thread ([`SharedGpu::prepare`]).
pub struct SharedGpu {
    instance: wgpu::Instance,
    gpu: Gpu,
    options: GpuOptions,
    spare: Spare,
}

/// The next window's Vello renderer.
enum Spare {
    None,
    Building(std::thread::JoinHandle<(vello_gpu::Renderer, vello_gpu::Resources)>),
    Ready(Box<(vello_gpu::Renderer, vello_gpu::Resources)>),
}

impl std::fmt::Debug for SharedGpu {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let spare = match self.spare {
            Spare::None => "none",
            Spare::Building(_) => "building",
            Spare::Ready(_) => "ready",
        };
        f.debug_struct("SharedGpu")
            .field("adapter", &self.gpu.adapter.get_info().name)
            .field("spare", &spare)
            .finish()
    }
}

impl SharedGpu {
    /// Starts building the next window's Vello renderer on a thread of its
    /// own, unless one is ready or on its way.
    pub fn prepare(&mut self) {
        if !matches!(self.spare, Spare::None) {
            return;
        }
        let device = self.gpu.device.clone();
        if let Ok(thread) = std::thread::Builder::new()
            .name("gpu-renderer".into())
            .spawn(move || new_renderer(&device, LIKELY_FORMAT, 1, 1))
        {
            self.spare = Spare::Building(thread);
        }
    }

    /// A renderer for `window`: its own surface, the shared device, and
    /// the spare Vello renderer if there is one (waiting for it if it is
    /// still being built). A window the GPU can't draw to (another
    /// display's GPU) gets one of its own.
    ///
    /// # Errors
    /// If the window's surface can't be made, or its own GPU opened.
    pub fn window(
        &mut self,
        window: impl Into<wgpu::SurfaceTarget<'static>>,
        width: u32,
        height: u32,
    ) -> Result<WindowRenderer, RenderError> {
        let creating = Instant::now();
        let surface = self
            .instance
            .create_surface(window)
            .map_err(|err| RenderError::Surface(err.to_string()))?;
        if !self.gpu.adapter.is_surface_supported(&surface) {
            let instance = self.instance.clone();
            let gpu = pollster::block_on(open(&instance, &self.options, Some(&surface)))?;
            return WindowRenderer::finish(
                instance,
                surface,
                gpu,
                None,
                width,
                height,
                &self.options,
                GpuStartup::default(),
            );
        }
        let surface_time = creating.elapsed();
        let waiting = Instant::now();
        let built = match std::mem::replace(&mut self.spare, Spare::None) {
            Spare::None => None,
            Spare::Building(thread) => thread.join().ok(),
            Spare::Ready(built) => Some(*built),
        };
        let startup = GpuStartup {
            surface: surface_time,
            waited: waiting.elapsed(),
            ..GpuStartup::default()
        };
        WindowRenderer::finish(
            self.instance.clone(),
            surface,
            self.gpu.clone(),
            built,
            width,
            height,
            &self.options,
            startup,
        )
    }
}

/// Renders scenes into pixmaps on the GPU, without a window: screenshots,
/// and comparisons with the CPU backend.
pub struct OffscreenRenderer {
    gpu: Gpu,
    info: GpuInfo,
}

impl OffscreenRenderer {
    /// Opens a GPU.
    pub fn new(options: &GpuOptions) -> Result<Self, RenderError> {
        let gpu = pollster::block_on(open(&instance(), options, None))?;
        let info = GpuInfo::of(&gpu.adapter);
        Ok(Self { gpu, info })
    }

    /// The GPU in use.
    pub fn info(&self) -> &GpuInfo {
        &self.info
    }

    /// Renders `scene` into a `width × height` pixmap over `background`.
    pub fn render(
        &mut self,
        scene: &Scene,
        width: u16,
        height: u16,
        background: Color,
    ) -> Result<Pixmap, RenderError> {
        let (w, h) = clamp_size(u32::from(width), u32::from(height));
        let device = &self.gpu.device;
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen"),
            size: wgpu::Extent3d {
                width: u32::from(w),
                height: u32::from(h),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let (mut renderer, mut resources) = new_renderer(device, format, w, h);
        let size = vello_gpu::RenderSize {
            width: w,
            height: h,
        };
        let depth = vello_gpu::Renderer::create_depth_texture_view(device, &size);
        let mut vello_scene = vello_gpu::Scene::new(w, h);
        let mut textures = Textures::new(format, &self.gpu);
        let mut bitmap_fonts = BitmapFonts::default();
        textures.process(
            &self.gpu,
            &mut renderer,
            &mut resources,
            &mut bitmap_fonts,
            None,
            false,
            scene.texture_work(),
        )?;
        replay(
            scene,
            &mut Target {
                context: &mut vello_scene,
                resources: &mut resources,
                glyph_atlas: false,
                bitmap_fonts: Some(&mut bitmap_fonts),
                transform: Affine::IDENTITY,
                textures: SceneTextures::Bound(&textures.placed),
                missing: 0,
            },
            Affine::IDENTITY,
        );
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("offscreen"),
        });
        renderer
            .render(
                &vello_scene,
                &mut resources,
                device,
                &self.gpu.queue,
                &mut encoder,
                &size,
                &view,
                Some(&depth),
                &textures.bindings,
                vello_gpu::TargetInit::Clear(vello_gpu::ClearSettings::Viewport {
                    color: background,
                }),
            )
            .map_err(|err| RenderError::Render(err.to_string()))?;
        let row = (u32::from(w) * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: u64::from(row) * u64::from(h),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d {
                width: u32::from(w),
                height: u32::from(h),
                depth_or_array_layers: 1,
            },
        );
        self.gpu.queue.submit([encoder.finish()]);
        let (sender, receiver) = std::sync::mpsc::channel();
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
            });
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|err| RenderError::Readback(err.to_string()))?;
        receiver
            .recv()
            .map_err(|err| RenderError::Readback(err.to_string()))?
            .map_err(|err| RenderError::Readback(err.to_string()))?;
        let mapped = buffer
            .slice(..)
            .get_mapped_range()
            .map_err(|err| RenderError::Readback(err.to_string()))?;
        let mut pixels = Vec::with_capacity(usize::from(w) * usize::from(h) * 4);
        for line in mapped.chunks_exact(row as usize) {
            pixels.extend_from_slice(&line[..usize::from(w) * 4]);
        }
        drop(mapped);
        buffer.unmap();
        Ok(Pixmap::from_parts(pixels, w, h, PixelMetadata::default()))
    }
}
