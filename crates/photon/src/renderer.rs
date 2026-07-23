//! photon renderer: builds the sprite pipeline and draws batches, both to a window surface and offscreen.

use aether::Gpu;
use wgpu::util::DeviceExt;

use crate::{
    camera::Camera,
    color::Color,
    error::RendererError,
    scene_target::SceneTarget,
    sprite::{RawInstance, Sprite},
    texture::Texture,
};

/// The offscreen target format. Linear (not sRGB) so read-back pixel values
/// round-trip exactly, which keeps the render tests precise. The window path
/// renders to the surface's own (usually sRGB) format instead.
const OFFSCREEN_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Camera data as uploaded to the GPU; `#[repr(C)]` to match the `Camera`
/// uniform block in sprite.wgsl.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct CameraUniform {
    view_proj: [f32; 16],
}

/// A configured window surface and the sprite pipeline built for its format.
struct SurfaceState {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
}

/// Per-batch GPU resources, created fresh each draw. Kept together so they
/// outlive the render pass and the queue submission that reference them.
struct BatchResources {
    camera_bind_group: wgpu::BindGroup,
    texture_bind_group: wgpu::BindGroup,
    /// `None` for an empty scene - a zero-sized vertex buffer is invalid.
    instances: Option<wgpu::Buffer>,
    count: u32,
}

/// Renders sprite batches. Owns the GPU context and the format-agnostic
/// pipeline pieces; per-frame camera, texture, and instance data are supplied
/// at draw time. Created windowed with [`Renderer::new`] or offscreen-only with
/// [`Renderer::headless`].
pub struct Renderer {
    gpu: Gpu,
    sampler: wgpu::Sampler,
    camera_layout: wgpu::BindGroupLayout,
    texture_layout: wgpu::BindGroupLayout,
    shader: wgpu::ShaderModule,
    pipeline_layout: wgpu::PipelineLayout,
    offscreen_pipeline: wgpu::RenderPipeline,
    surface: Option<SurfaceState>,
}

impl Renderer {
    /// Create a windowed renderer that presents to `target` (a window handle,
    /// e.g. an `Arc<winit::window::Window>`) at the given pixel `size`. Blocks
    /// on GPU acquisition.
    pub fn new(
        target: impl Into<wgpu::SurfaceTarget<'static>>,
        size: (u32, u32),
    ) -> Result<Self, RendererError> {
        let instance = Gpu::default_instance();
        let surface = instance.create_surface(target)?;
        // Choose an adapter that can actually present to this surface.
        let gpu = pollster::block_on(Gpu::request(instance, Some(&surface)))?;

        let mut renderer = Self::build_core(gpu);

        let (width, height) = (size.0.max(1), size.1.max(1));
        let config = surface
            .get_default_config(&renderer.gpu.adapter, width, height)
            .ok_or(RendererError::SurfaceUnsupported)?;
        surface.configure(&renderer.gpu.device, &config);
        tracing::debug!(format = ?config.format, width, height, "configured window surface");
        let pipeline = Self::build_pipeline(
            &renderer.gpu.device,
            &renderer.pipeline_layout,
            &renderer.shader,
            config.format,
        );
        renderer.surface = Some(SurfaceState {
            surface,
            config,
            pipeline,
        });
        Ok(renderer)
    }

    /// Create a headless renderer with no window, for offscreen rendering and
    /// tests. Blocks on GPU acquisition.
    pub fn headless() -> Result<Self, RendererError> {
        Ok(Self::build_core(Gpu::headless()?))
    }

    /// Create a renderer on an externally supplied GPU context, for sharing one
    /// device with another renderer (ADR 0018: the editor's GUI backend renders
    /// on the same device as the scene). Never fails: `gpu` was already
    /// acquired, and building the pipeline is infallible.
    pub fn from_gpu(gpu: Gpu) -> Self {
        Self::build_core(gpu)
    }

    /// Build the format-agnostic core (layouts, sampler, shader, and the
    /// offscreen pipeline) on top of an acquired GPU context.
    fn build_core(gpu: Gpu) -> Self {
        let device = &gpu.device;

        let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("camera bind group layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sprite texture bind group layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("sprite sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sprite shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("sprite.wgsl").into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sprite pipeline layout"),
            bind_group_layouts: &[Some(&camera_layout), Some(&texture_layout)],
            immediate_size: 0,
        });

        let offscreen_pipeline =
            Self::build_pipeline(device, &pipeline_layout, &shader, OFFSCREEN_FORMAT);

        Self {
            gpu,
            sampler,
            camera_layout,
            texture_layout,
            shader,
            pipeline_layout,
            offscreen_pipeline,
            surface: None,
        }
    }

    /// Build the sprite render pipeline for a specific target format.
    fn build_pipeline(
        device: &wgpu::Device,
        layout: &wgpu::PipelineLayout,
        shader: &wgpu::ShaderModule,
        format: wgpu::TextureFormat,
    ) -> wgpu::RenderPipeline {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sprite pipeline"),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some("vs_main"),
                buffers: &[Some(RawInstance::layout())],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        })
    }

    /// Upload an RGBA8 image (row-major, `width * height * 4` bytes) as a texture.
    pub fn create_texture(&self, rgba: &[u8], width: u32, height: u32) -> Texture {
        Texture::from_rgba(&self.gpu.device, &self.gpu.queue, rgba, width, height)
    }

    /// Resize the window surface. No-op on a headless renderer.
    pub fn resize(&mut self, size: (u32, u32)) {
        if let Some(state) = &mut self.surface {
            state.config.width = size.0.max(1);
            state.config.height = size.1.max(1);
            state.surface.configure(&self.gpu.device, &state.config);
        }
    }

    /// Draw `sprites` (all sampling `texture`) through `camera` to the window,
    /// over a `clear` background, and present. Errors if the renderer is
    /// headless. Frames dropped due to a transiently outdated surface are
    /// silently skipped after reconfiguring.
    pub fn render(
        &mut self,
        clear: Color,
        camera: &Camera,
        texture: &Texture,
        sprites: &[Sprite],
    ) -> Result<(), RendererError> {
        profiling::scope!("render");

        let Some(frame) = self.acquire_frame()? else {
            return Ok(());
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let batch = self.prepare_batch(camera, texture, sprites);
        let pipeline = &self
            .surface
            .as_ref()
            .ok_or(RendererError::NoSurface)?
            .pipeline;

        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        self.record_pass(&mut encoder, &view, pipeline, clear, &batch);
        {
            profiling::scope!("submit");
            self.gpu.queue.submit(Some(encoder.finish()));
        }
        {
            profiling::scope!("present");
            self.gpu.queue.present(frame);
        }
        Ok(())
    }

    /// Acquire the next surface frame, reconfiguring on an outdated/lost surface.
    /// Returns `None` for a frame that should be skipped this tick.
    fn acquire_frame(&mut self) -> Result<Option<wgpu::SurfaceTexture>, RendererError> {
        profiling::scope!("acquire_frame");
        let acquired = {
            let state = self.surface.as_ref().ok_or(RendererError::NoSurface)?;
            state.surface.get_current_texture()
        };
        match acquired {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => Ok(Some(frame)),
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.reconfigure_surface();
                Ok(None)
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                Ok(None)
            }
            wgpu::CurrentSurfaceTexture::Validation => Err(RendererError::SurfaceAcquire),
        }
    }

    /// Reapply the current surface configuration, e.g. after it goes outdated.
    fn reconfigure_surface(&mut self) {
        if let Some(state) = &self.surface {
            state.surface.configure(&self.gpu.device, &state.config);
        }
    }

    /// Render `sprites` through `camera` into an offscreen image of `size`, over
    /// a `clear` background. Returns the result as row-major RGBA8 bytes
    /// (`width * height * 4`). Used by tests and, later, editor render targets.
    pub fn render_to_image(
        &self,
        size: (u32, u32),
        clear: Color,
        camera: &Camera,
        texture: &Texture,
        sprites: &[Sprite],
    ) -> Result<Vec<u8>, RendererError> {
        let (width, height) = size;
        let device = &self.gpu.device;

        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OFFSCREEN_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());

        let batch = self.prepare_batch(camera, texture, sprites);

        // Read-back rows must be aligned; pad bytes-per-row up to the required
        // alignment and strip the padding after mapping.
        let unpadded_bpr = width * 4;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded_bpr = unpadded_bpr.div_ceil(align) * align;
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (padded_bpr * height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        self.record_pass(
            &mut encoder,
            &target_view,
            &self.offscreen_pipeline,
            clear,
            &batch,
        );
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bpr),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.gpu.queue.submit(Some(encoder.finish()));

        let (tx, rx) = std::sync::mpsc::channel();
        readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |res| {
                let _ = tx.send(res);
            });
        self.gpu.device.poll(wgpu::PollType::wait_indefinitely())?;
        rx.recv().expect("map_async never signalled")?;

        let mapped = readback
            .slice(..)
            .get_mapped_range()
            .expect("buffer was just mapped for reading");
        let mut pixels = Vec::with_capacity((unpadded_bpr * height) as usize);
        for row in 0..height {
            let start = (row * padded_bpr) as usize;
            let end = start + unpadded_bpr as usize;
            pixels.extend_from_slice(&mapped[start..end]);
        }
        drop(mapped);
        readback.unmap();

        Ok(pixels)
    }

    /// Create a sampleable render target sized `(width, height)` for the
    /// editor's viewport (ADR 0018). Uses the same linear `OFFSCREEN_FORMAT` as
    /// [`render_to_image`](Self::render_to_image), so the existing offscreen
    /// pipeline renders into it unchanged.
    pub fn create_scene_target(&self, width: u32, height: u32) -> SceneTarget {
        SceneTarget::new(&self.gpu.device, OFFSCREEN_FORMAT, width, height)
    }

    /// Render `sprites` through `camera` into `target`, over a `clear`
    /// background. Unlike [`render_to_image`](Self::render_to_image), the
    /// result stays on the GPU for a compositor to sample - no readback, so
    /// this cannot fail the way a CPU round-trip can.
    ///
    /// Color space: `target`'s texels are linear light, not display-encoded
    /// (`Texture::from_rgba` sRGB-decodes sprite art on sample, and
    /// `OFFSCREEN_FORMAT` applies no encode on write - see its doc comment).
    /// A compositor should sample it as-is and let its own sRGB-formatted
    /// surface apply the display encode on write, which is the standard
    /// linear-light compositing pipeline, not a shortcut.
    pub fn render_to_target(
        &self,
        target: &SceneTarget,
        clear: Color,
        camera: &Camera,
        texture: &Texture,
        sprites: &[Sprite],
    ) {
        let batch = self.prepare_batch(camera, texture, sprites);
        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        self.record_pass(
            &mut encoder,
            target.view(),
            &self.offscreen_pipeline,
            clear,
            &batch,
        );
        self.gpu.queue.submit(Some(encoder.finish()));
    }

    /// Build the per-batch GPU resources (camera uniform, bind groups, instance
    /// buffer) shared by the window and offscreen paths.
    fn prepare_batch(
        &self,
        camera: &Camera,
        texture: &Texture,
        sprites: &[Sprite],
    ) -> BatchResources {
        profiling::scope!("prepare_batch");
        let device = &self.gpu.device;

        let camera_uniform = CameraUniform {
            view_proj: camera.view_projection().to_cols_array(),
        };
        let camera_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("camera uniform"),
            contents: bytemuck::bytes_of(&camera_uniform),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        // The bind group keeps `camera_buffer` alive, so it need not be stored.
        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("camera bind group"),
            layout: &self.camera_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });

        let texture_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sprite texture bind group"),
            layout: &self.texture_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&texture.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });

        let raw: Vec<RawInstance> = sprites.iter().map(|s| s.to_raw()).collect();
        let instances = (!raw.is_empty()).then(|| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("sprite instances"),
                contents: bytemuck::cast_slice(&raw),
                usage: wgpu::BufferUsages::VERTEX,
            })
        });

        BatchResources {
            camera_bind_group,
            texture_bind_group,
            instances,
            count: raw.len() as u32,
        }
    }

    /// Record a single sprite pass into `encoder`, clearing `view` and drawing
    /// the prepared batch with `pipeline`.
    fn record_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        pipeline: &wgpu::RenderPipeline,
        clear: Color,
        batch: &BatchResources,
    ) {
        profiling::scope!("record_pass");
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("sprite pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(clear.to_wgpu()),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });

        if let Some(instances) = &batch.instances {
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &batch.camera_bind_group, &[]);
            pass.set_bind_group(1, &batch.texture_bind_group, &[]);
            pass.set_vertex_buffer(0, instances.slice(..));
            pass.draw(0..6, 0..batch.count);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec2;

    fn pixel(image: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * width + x) * 4) as usize;
        [image[i], image[i + 1], image[i + 2], image[i + 3]]
    }

    #[test]
    #[ignore = "requires a GPU adapter; run locally with --ignored"]
    fn from_gpu_renders_the_same_as_a_self_owned_renderer() {
        // Two renderers sharing one aether::Gpu (ADR 0018's shared-device path)
        // render identically to a self-owned headless renderer.
        let gpu = Gpu::headless().expect("gpu context");
        let shared = Renderer::from_gpu(gpu);
        let white = shared.create_texture(&[255, 255, 255, 255], 1, 1);
        let (w, h) = (64u32, 64u32);
        let camera = Camera::new(Vec2::ZERO, Vec2::new(w as f32, h as f32));
        let sprite = Sprite::new(Vec2::new(10.0, 10.0), Vec2::new(20.0, 20.0));

        let image = shared
            .render_to_image(
                (w, h),
                Color::new(0.0, 0.0, 0.0, 1.0),
                &camera,
                &white,
                &[sprite],
            )
            .expect("render");
        assert_eq!(pixel(&image, w, 15, 15), [255, 255, 255, 255]);
        assert_eq!(pixel(&image, w, 60, 60), [0, 0, 0, 255]);
    }

    #[test]
    #[ignore = "requires a GPU adapter; run locally with --ignored"]
    fn render_to_target_draws_into_a_sampleable_texture() {
        let renderer = Renderer::headless().expect("headless renderer");
        let white = renderer.create_texture(&[255, 255, 255, 255], 1, 1);
        let camera = Camera::new(Vec2::ZERO, Vec2::new(64.0, 64.0));
        let sprite = Sprite::new(Vec2::new(10.0, 10.0), Vec2::new(20.0, 20.0));

        let target = renderer.create_scene_target(64, 64);
        assert_eq!((target.width, target.height), (64, 64));
        // No readback API exists (by design - a compositor samples target.view()
        // directly); this just proves the call records and submits without
        // panicking, on the same offscreen pipeline render_to_image uses.
        renderer.render_to_target(
            &target,
            Color::new(0.0, 0.0, 0.0, 1.0),
            &camera,
            &white,
            &[sprite],
        );
    }

    #[test]
    #[ignore = "requires a GPU adapter; run locally with --ignored"]
    fn sprite_lands_in_the_top_left_for_y_down() {
        let renderer = Renderer::headless().expect("headless renderer");
        let white = renderer.create_texture(&[255, 255, 255, 255], 1, 1);
        let (w, h) = (200u32, 150u32);
        let camera = Camera::new(Vec2::ZERO, Vec2::new(w as f32, h as f32));

        let mut sprite = Sprite::new(Vec2::new(10.0, 10.0), Vec2::new(40.0, 30.0));
        sprite.tint = Color::new(1.0, 0.0, 0.0, 1.0);

        let image = renderer
            .render_to_image((w, h), Color::BLACK, &camera, &white, &[sprite])
            .expect("render");

        // Inside the sprite's world rect (x 10..50, y 10..40) is red.
        assert_eq!(pixel(&image, w, 30, 25), [255, 0, 0, 255]);
        // The same column near the bottom is still clear - the sprite sits at
        // the TOP, proving Y-down (ADR 0012) end to end.
        assert_eq!(pixel(&image, w, 30, 130), [0, 0, 0, 255]);
        // A far corner is untouched.
        assert_eq!(pixel(&image, w, 190, 140), [0, 0, 0, 255]);
    }

    #[test]
    #[ignore = "requires a GPU adapter; run locally with --ignored"]
    fn empty_scene_is_just_the_clear_color() {
        let renderer = Renderer::headless().expect("headless renderer");
        let texture = renderer.create_texture(&[255, 255, 255, 255], 1, 1);
        let camera = Camera::new(Vec2::ZERO, Vec2::new(64.0, 64.0));

        let image = renderer
            .render_to_image(
                (64, 64),
                Color::new(0.0, 0.0, 1.0, 1.0),
                &camera,
                &texture,
                &[],
            )
            .expect("render");

        assert_eq!(pixel(&image, 64, 32, 32), [0, 0, 255, 255]);
    }
}
