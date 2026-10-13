// SPDX-License-Identifier: AGPL-3.0-or-later

//! The GPU backend: `vello_gpu` on `wgpu`.

use std::time::{Duration, Instant};

use vello_common::kurbo::Affine;
use vello_common::peniko::Color;
use vello_cpu::{PixelMetadata, Pixmap};

use crate::RenderError;
use crate::scene::{Scene, Target, replay};

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

struct Gpu {
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
}

fn instance() -> wgpu::Instance {
    // `WGPU_BACKEND` and friends still work, which helps on odd machines.
    wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env())
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
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("imaginarium"),
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

/// Vello works with `u16` sizes; a zero-sized surface is invalid.
fn clamp_size(width: u32, height: u32) -> (u16, u16) {
    let clamp = |v: u32| u16::try_from(v.max(1)).unwrap_or(u16::MAX);
    (clamp(width), clamp(height))
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
}

/// What happened to a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameOutcome {
    /// It is on its way to the screen.
    Presented(FrameTiming),
    /// The window wasn't ready (minimized, resized meanwhile, …): draw
    /// again on the next redraw.
    Skipped,
}

/// Draws scenes into a window with `vello_gpu`.
pub struct WindowRenderer {
    gpu: Gpu,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
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
        let instance = instance();
        let surface = instance
            .create_surface(window)
            .map_err(|err| RenderError::Surface(err.to_string()))?;
        let gpu = pollster::block_on(open(&instance, options, Some(&surface)))?;
        let capabilities = surface.get_capabilities(&gpu.adapter);
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(|f| {
                matches!(
                    f,
                    wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Bgra8Unorm
                )
            })
            .or_else(|| capabilities.formats.first().copied())
            .ok_or_else(|| RenderError::Surface("the surface supports no format".into()))?;
        let view_format = format.remove_srgb_suffix();
        let (w, h) = clamp_size(width, height);
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
            alpha_mode: capabilities
                .alpha_modes
                .first()
                .copied()
                .unwrap_or(wgpu::CompositeAlphaMode::Auto),
            view_formats: if view_format == format {
                Vec::new()
            } else {
                vec![view_format]
            },
            // Plain sRGB, as before wgpu 30 (no HDR or wide gamut yet).
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        surface.configure(&gpu.device, &config);
        let (renderer, resources) = vello_gpu::Renderer::new(
            &gpu.device,
            &vello_gpu::RenderTargetConfig {
                format: view_format,
                width: w,
                height: h,
            },
        );
        let depth = vello_gpu::Renderer::create_depth_texture_view(
            &gpu.device,
            &vello_gpu::RenderSize {
                width: w,
                height: h,
            },
        );
        let info = GpuInfo::of(&gpu.adapter);
        Ok(Self {
            gpu,
            surface,
            config,
            view_format,
            renderer,
            resources,
            depth,
            scene: vello_gpu::Scene::new(w, h),
            info,
            glyph_atlas: options.glyph_atlas,
            frames: 0,
        })
    }

    /// The GPU in use.
    pub fn info(&self) -> &GpuInfo {
        &self.info
    }

    /// The surface size in physical pixels.
    pub fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    /// Follows a window resize (physical pixels).
    pub fn resize(&mut self, width: u32, height: u32) {
        let (w, h) = clamp_size(width, height);
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
        let start = Instant::now();
        let texture = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture)
            | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => texture,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.gpu.device, &self.config);
                return Ok(FrameOutcome::Skipped);
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err(RenderError::Surface(
                    "the surface texture failed validation".into(),
                ));
            }
            _ => return Ok(FrameOutcome::Skipped), // timeout, occluded
        };
        let acquired = Instant::now();
        let view = texture.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(self.view_format),
            ..Default::default()
        });
        let (w, h) = clamp_size(self.config.width, self.config.height);
        self.scene.reset_and_resize(w, h);
        replay(
            scene,
            &mut Target {
                context: &mut self.scene,
                resources: &mut self.resources,
                glyph_atlas: self.glyph_atlas && self.frames > 0,
            },
            transform,
        );
        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
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
                &vello_gpu::TextureBindings::new(),
                vello_gpu::TargetInit::Clear(vello_gpu::ClearSettings::Viewport {
                    color: background,
                }),
            )
            .map_err(|err| RenderError::Render(err.to_string()))?;
        let encoded = Instant::now();
        self.gpu.queue.submit([encoder.finish()]);
        self.gpu.queue.present(texture);
        self.frames += 1;
        Ok(FrameOutcome::Presented(FrameTiming {
            acquire: acquired - start,
            encode: encoded - acquired,
            submit: encoded.elapsed(),
        }))
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
        let (mut renderer, mut resources) = vello_gpu::Renderer::new(
            device,
            &vello_gpu::RenderTargetConfig {
                format,
                width: w,
                height: h,
            },
        );
        let size = vello_gpu::RenderSize {
            width: w,
            height: h,
        };
        let depth = vello_gpu::Renderer::create_depth_texture_view(device, &size);
        let mut vello_scene = vello_gpu::Scene::new(w, h);
        replay(
            scene,
            &mut Target {
                context: &mut vello_scene,
                resources: &mut resources,
                glyph_atlas: false,
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
                &vello_gpu::TextureBindings::new(),
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
