//! aurora-wgpu renderer: turns an Aurora draw list into batched, clipped quads
//! (atlas-textured for rects/glyphs, image-textured for registered pictures)
//! on a surface.

use aether::Gpu;
use aurora::cosmic_text::FontSystem;
use aurora::{Color, DrawCommand, DrawList, ImageHandle, Rect};
use glam::Vec2;

use crate::atlas::{Atlas, GlyphEntry};
use crate::error::RenderError;
use crate::image::{ImageRegistry, Mips};

/// One quad as uploaded to the GPU: a pixel rectangle plus the UV span it
/// samples. Shared by both pipelines - the atlas pipeline (fills sample a
/// white texel, glyphs sample coverage) and the image pipeline (a registered
/// texture, sampled directly). `#[repr(C)]` to match the vertex attributes and
/// the `Instance` struct in quad.wgsl / image.wgsl.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct QuadInstance {
    pos: [f32; 2],
    size: [f32; 2],
    uv_min: [f32; 2],
    uv_max: [f32; 2],
    color: [f32; 4],
    /// Border color, used only when `border` > 0 (quad pipeline only).
    border_color: [f32; 4],
    /// Per-corner radius in px, `[top_left, top_right, bottom_right, bottom_left]`
    /// (0 = square). The image pipeline ignores it; the quad shader evaluates a
    /// rounded-rect SDF when any corner or the border is set.
    radius: [f32; 4],
    /// Which edges the border draws on, `[top, right, bottom, left]` (1 = draw,
    /// 0 = open). All ones is a normal all-around border.
    border_sides: [f32; 4],
    /// Border width in px (0 = none).
    border: f32,
}

impl QuadInstance {
    fn layout() -> wgpu::VertexBufferLayout<'static> {
        // The quad shader reads all nine; the image shader reads only 0..=4 and
        // ignores the border/shape attributes, which is allowed.
        const ATTRS: [wgpu::VertexAttribute; 9] = wgpu::vertex_attr_array![
            0 => Float32x2, 1 => Float32x2, 2 => Float32x2, 3 => Float32x2, 4 => Float32x4,
            5 => Float32x4, 6 => Float32x4, 7 => Float32x4, 8 => Float32];
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<QuadInstance>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &ATTRS,
        }
    }

    /// A solid fill: the whole quad samples the atlas's white texel.
    fn fill(rect: Rect, color: Color, white_uv: [f32; 2]) -> Self {
        Self {
            pos: rect.pos.to_array(),
            size: rect.size.to_array(),
            uv_min: white_uv,
            uv_max: white_uv,
            color: [color.r, color.g, color.b, color.a],
            border_color: [0.0; 4],
            radius: [0.0; 4],
            border_sides: [1.0; 4],
            border: 0.0,
        }
    }

    /// A rounded and/or bordered fill: like [`fill`](Self::fill) but the shader
    /// rounds each corner to its `radius` and paints the outer `border` px (if
    /// any) in `border_color`, only on the edges `sides` selects.
    fn rounded(
        rect: Rect,
        color: Color,
        radius: [f32; 4],
        border: f32,
        border_color: Color,
        sides: [bool; 4],
        white_uv: [f32; 2],
    ) -> Self {
        Self {
            pos: rect.pos.to_array(),
            size: rect.size.to_array(),
            uv_min: white_uv,
            uv_max: white_uv,
            color: [color.r, color.g, color.b, color.a],
            border_color: [
                border_color.r,
                border_color.g,
                border_color.b,
                border_color.a,
            ],
            radius,
            border_sides: sides.map(|on| if on { 1.0 } else { 0.0 }),
            border,
        }
    }

    /// A glyph quad: placed at the pen position offset by the bitmap's bearing,
    /// sampling the glyph's coverage from the atlas.
    fn glyph(pen_x: f32, pen_y: f32, entry: &GlyphEntry, color: Color) -> Self {
        Self {
            pos: [pen_x + entry.left as f32, pen_y - entry.top as f32],
            size: [entry.width, entry.height],
            uv_min: entry.uv_min,
            uv_max: entry.uv_max,
            color: [color.r, color.g, color.b, color.a],
            border_color: [0.0; 4],
            radius: [0.0; 4],
            border_sides: [1.0; 4],
            border: 0.0,
        }
    }

    /// A registered image filling `rect`, multiplied by `tint` (opaque white
    /// leaves it untouched; a color recolors a white icon).
    fn image(rect: Rect, tint: Color) -> Self {
        Self {
            pos: rect.pos.to_array(),
            size: rect.size.to_array(),
            uv_min: [0.0, 0.0],
            uv_max: [1.0, 1.0],
            color: [tint.r, tint.g, tint.b, tint.a],
            border_color: [0.0; 4],
            radius: [0.0; 4],
            border_sides: [1.0; 4],
            border: 0.0,
        }
    }
}

/// One draw in the command list's order: either a run of atlas-pipeline quads
/// (fills and glyphs, which can batch together) or a single image-pipeline
/// draw (each references its own texture, so it cannot batch with others).
/// Recorded in list order so overlapping draws composite correctly regardless
/// of which pipeline they use.
enum Op {
    Quads {
        rect: [u32; 4],
        range: std::ops::Range<u32>,
    },
    Image {
        rect: [u32; 4],
        handle: ImageHandle,
        instance: u32,
    },
}

/// Renders Aurora draw lists to a window surface. Windowing-agnostic: it takes a
/// `raw-window-handle` target plus a size, exactly like photon's renderer.
/// Created self-contained with [`new`](Self::new), or sharing a GPU context
/// with another renderer via [`from_gpu`](Self::from_gpu) (ADR 0018).
pub struct Renderer {
    gpu: Gpu,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    image_pipeline: wgpu::RenderPipeline,
    screen_buffer: wgpu::Buffer,
    screen_bind_group: wgpu::BindGroup,
    /// The texture+sampler bind group layout shared by the atlas bind group,
    /// every registered image's bind group, and both pipelines' group 1.
    texture_layout: wgpu::BindGroupLayout,
    atlas: Atlas,
    atlas_bind_group: wgpu::BindGroup,
    images: ImageRegistry,
    /// Persistent, growable instance buffers reused every frame (grown by
    /// power-of-two when a frame needs more) so a normal frame does no GPU
    /// buffer allocation - just a `write_buffer` upload.
    quad_instances: wgpu::Buffer,
    quad_cap: u64,
    image_instances: wgpu::Buffer,
    image_cap: u64,
}

/// Initial instance-buffer capacity, in bytes (grown on demand).
const INSTANCE_INIT_BYTES: u64 = 4096;

impl Renderer {
    /// Create a self-contained renderer that presents to `target` (e.g. an
    /// `Arc<Window>`) at the given pixel `size`: acquires its own GPU context.
    /// Blocks on GPU acquisition; wgpu's async setup never leaks to callers.
    pub fn new(
        target: impl Into<wgpu::SurfaceTarget<'static>>,
        size: (u32, u32),
    ) -> Result<Self, RenderError> {
        let instance = Gpu::default_instance();
        let surface = instance.create_surface(target)?;
        let gpu = pollster::block_on(Gpu::request(instance, Some(&surface)))?;
        Self::build(gpu, surface, size)
    }

    /// Create a renderer on an externally supplied GPU context and an
    /// already-created surface (ADR 0018: the editor's GUI backend renders on
    /// the same device as the scene). `gpu`'s adapter must be compatible with
    /// `surface` (e.g. acquired via `Gpu::request(instance, Some(&surface))`).
    pub fn from_gpu(
        gpu: Gpu,
        surface: wgpu::Surface<'static>,
        size: (u32, u32),
    ) -> Result<Self, RenderError> {
        Self::build(gpu, surface, size)
    }

    /// Build the renderer on an already-acquired `gpu` and `surface`: surface
    /// configuration, the atlas, the image registry, and both pipelines.
    fn build(
        gpu: Gpu,
        surface: wgpu::Surface<'static>,
        size: (u32, u32),
    ) -> Result<Self, RenderError> {
        let device = &gpu.device;

        let (width, height) = (size.0.max(1), size.1.max(1));
        let config = surface
            .get_default_config(&gpu.adapter, width, height)
            .ok_or(RenderError::SurfaceUnsupported)?;
        surface.configure(device, &config);

        let screen_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("screen bind group layout"),
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
        let screen_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("screen projection"),
            size: std::mem::size_of::<[f32; 16]>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let screen_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("screen bind group"),
            layout: &screen_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: screen_buffer.as_entire_binding(),
            }],
        });

        // One texture+sampler layout, shared by the glyph atlas, every
        // registered image, and both pipelines' group 1 - the shape (a
        // filterable float texture plus a filtering sampler) is identical
        // whether the texture holds glyph coverage or an RGBA picture.
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("texture bind group layout"),
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

        let atlas = Atlas::new(device, &gpu.queue);
        let atlas_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("atlas sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let atlas_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("atlas bind group"),
            layout: &texture_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&atlas.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&atlas_sampler),
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("aurora pipeline layout"),
            bind_group_layouts: &[Some(&screen_layout), Some(&texture_layout)],
            immediate_size: 0,
        });

        let quad_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("quad shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("quad.wgsl").into()),
        });
        let pipeline = Self::build_pipeline(
            device,
            &pipeline_layout,
            &quad_shader,
            "quad pipeline",
            config.format,
        );

        let image_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("image shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("image.wgsl").into()),
        });
        let image_pipeline = Self::build_pipeline(
            device,
            &pipeline_layout,
            &image_shader,
            "image pipeline",
            config.format,
        );

        let images = ImageRegistry::new(device);

        let new_instance_buffer = |label| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: INSTANCE_INIT_BYTES,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        // Bind these before moving `gpu` into the struct (they borrow `device`).
        let quad_instances = new_instance_buffer("quad instances");
        let image_instances = new_instance_buffer("image instances");

        Ok(Self {
            gpu,
            surface,
            config,
            pipeline,
            image_pipeline,
            screen_buffer,
            screen_bind_group,
            texture_layout,
            atlas,
            atlas_bind_group,
            images,
            quad_instances,
            quad_cap: INSTANCE_INIT_BYTES,
            image_instances,
            image_cap: INSTANCE_INIT_BYTES,
        })
    }

    /// Build a quad-instanced render pipeline for `shader`, targeting `format`.
    /// The atlas and image pipelines differ only in shader and label.
    fn build_pipeline(
        device: &wgpu::Device,
        layout: &wgpu::PipelineLayout,
        shader: &wgpu::ShaderModule,
        label: &str,
        format: wgpu::TextureFormat,
    ) -> wgpu::RenderPipeline {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some("vs_main"),
                buffers: &[Some(QuadInstance::layout())],
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

    /// Register a texture (e.g. photon's scene target view) for sampling by
    /// `DrawCommand::Image`, returning the handle to give a `Ui::image` widget
    /// (ADR 0018). The registered bind group is built against this renderer's
    /// device, so `view` must come from a texture created on the same device -
    /// true by construction when both were built from the same shared `Gpu`.
    pub fn register_image(&mut self, view: &wgpu::TextureView) -> ImageHandle {
        self.images
            .register(&self.gpu.device, &self.texture_layout, view)
    }

    /// Register an image from raw RGBA8 bytes (`width * height * 4`, row-major),
    /// keeping the texture alive internally - for CPU-generated images like
    /// rasterized icons. Returns the handle Aurora's draw list references.
    ///
    /// The image gets a full mipmap chain, so it stays clean when drawn smaller
    /// than its native size (a 64px icon in a 16px slot). For an image drawn at
    /// its native size - especially one refilled often - use
    /// [`register_image_rgba_unmipped`](Self::register_image_rgba_unmipped),
    /// which skips the per-level CPU downsample on every upload.
    pub fn register_image_rgba(&mut self, rgba: &[u8], width: u32, height: u32) -> ImageHandle {
        self.register_rgba_with(rgba, width, height, Mips::Chain)
    }

    /// Register an RGBA8 image with no mipmap chain - for an image drawn at its
    /// native size, such as a color picker's gradient that is regenerated as the
    /// user drags. Building mips would cost a CPU downsample per level on every
    /// refill for levels that are never sampled.
    pub fn register_image_rgba_unmipped(
        &mut self,
        rgba: &[u8],
        width: u32,
        height: u32,
    ) -> ImageHandle {
        self.register_rgba_with(rgba, width, height, Mips::None)
    }

    fn register_rgba_with(
        &mut self,
        rgba: &[u8],
        width: u32,
        height: u32,
        mips: Mips,
    ) -> ImageHandle {
        self.images.register_rgba(
            &self.gpu.device,
            &self.gpu.queue,
            &self.texture_layout,
            rgba,
            width,
            height,
            mips,
        )
    }

    /// Replace a registry-owned image's pixels in place (keeps its handle) - for
    /// CPU-generated images that change, like the color picker's gradients.
    /// Returns `false` on an unknown handle.
    pub fn update_image_rgba(
        &mut self,
        handle: ImageHandle,
        rgba: &[u8],
        width: u32,
        height: u32,
    ) -> bool {
        self.images.update_rgba(
            &self.gpu.device,
            &self.gpu.queue,
            &self.texture_layout,
            handle,
            rgba,
            width,
            height,
        )
    }

    /// Point an already-registered image at a new texture view (e.g. the
    /// viewport's render target recreated at a new size). The handle - and
    /// every widget referencing it - stays valid, so no UI rebuild is needed.
    /// Returns `false` on an unknown handle.
    pub fn update_image(&mut self, handle: ImageHandle, view: &wgpu::TextureView) -> bool {
        self.images
            .update(&self.gpu.device, &self.texture_layout, handle, view)
    }

    /// Reconfigure the surface for a new window size.
    pub fn resize(&mut self, size: (u32, u32)) {
        self.config.width = size.0.max(1);
        self.config.height = size.1.max(1);
        self.surface.configure(&self.gpu.device, &self.config);
    }

    /// Draw `list` over a `clear` background and present. Runs of atlas-pipeline
    /// quads (fills and glyphs) under the same clip are batched into one
    /// instanced draw; each `Image` command is its own draw against its
    /// registered texture. A `PushClip`/`PopClip` flushes the current quad batch
    /// and updates the scissor; ops are recorded in draw-list order, so mixed
    /// quads and images still composite correctly. `font_system` supplies the
    /// glyphs the draw list references, rasterized into the atlas on first use.
    pub fn render(
        &mut self,
        list: &DrawList,
        font_system: &mut FontSystem,
        clear: Color,
    ) -> Result<(), RenderError> {
        profiling::scope!("render");
        let (sw, sh) = (self.config.width as f32, self.config.height as f32);
        self.queue_write_screen(sw, sh);

        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f)
            | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.gpu.device, &self.config);
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Validation => return Err(RenderError::SurfaceAcquire),
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let (instances, image_instances, ops) = {
            profiling::scope!("build_instances");
            self.build_ops(list, font_system, sw, sh)
        };

        // Upload into the persistent, growable instance buffers (no per-frame
        // GPU allocation unless a frame needs more room than before).
        let have_quads = !instances.is_empty();
        let have_images = !image_instances.is_empty();
        if have_quads {
            Self::ensure_instance_capacity(
                &self.gpu.device,
                &mut self.quad_instances,
                &mut self.quad_cap,
                std::mem::size_of_val(&instances[..]) as u64,
                "quad instances",
            );
            self.gpu
                .queue
                .write_buffer(&self.quad_instances, 0, bytemuck::cast_slice(&instances));
        }
        if have_images {
            Self::ensure_instance_capacity(
                &self.gpu.device,
                &mut self.image_instances,
                &mut self.image_cap,
                std::mem::size_of_val(&image_instances[..]) as u64,
                "image instances",
            );
            self.gpu.queue.write_buffer(
                &self.image_instances,
                0,
                bytemuck::cast_slice(&image_instances),
            );
        }

        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            profiling::scope!("record_pass");
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("aurora pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: clear.r as f64,
                            g: clear.g as f64,
                            b: clear.b as f64,
                            a: clear.a as f64,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            for op in &ops {
                match op {
                    Op::Quads { rect, range } => {
                        if !have_quads {
                            continue;
                        }
                        pass.set_pipeline(&self.pipeline);
                        pass.set_bind_group(0, &self.screen_bind_group, &[]);
                        pass.set_bind_group(1, &self.atlas_bind_group, &[]);
                        pass.set_vertex_buffer(0, self.quad_instances.slice(..));
                        pass.set_scissor_rect(rect[0], rect[1], rect[2], rect[3]);
                        pass.draw(0..6, range.clone());
                    }
                    Op::Image {
                        rect,
                        handle,
                        instance,
                    } => {
                        let (true, Some(bind_group)) =
                            (have_images, self.images.bind_group(*handle))
                        else {
                            continue;
                        };
                        pass.set_pipeline(&self.image_pipeline);
                        pass.set_bind_group(0, &self.screen_bind_group, &[]);
                        pass.set_bind_group(1, bind_group, &[]);
                        pass.set_vertex_buffer(0, self.image_instances.slice(..));
                        pass.set_scissor_rect(rect[0], rect[1], rect[2], rect[3]);
                        pass.draw(0..6, *instance..*instance + 1);
                    }
                }
            }
        }
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

    fn queue_write_screen(&self, sw: f32, sh: f32) {
        self.gpu.queue.write_buffer(
            &self.screen_buffer,
            0,
            bytemuck::cast_slice(&screen_projection(sw, sh)),
        );
    }

    /// Grow `buffer` to hold at least `needed` bytes if it is too small,
    /// rounding up to a power of two so a growing frame reallocates rarely. The
    /// buffer keeps `VERTEX | COPY_DST` usage so it can be written every frame.
    fn ensure_instance_capacity(
        device: &wgpu::Device,
        buffer: &mut wgpu::Buffer,
        cap: &mut u64,
        needed: u64,
        label: &str,
    ) {
        if needed <= *cap {
            return;
        }
        let new_cap = needed.next_power_of_two().max(INSTANCE_INIT_BYTES);
        *buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: new_cap,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        *cap = new_cap;
    }

    /// Translate the command list into the atlas-pipeline instances, the
    /// image-pipeline instances, and the ordered list of draws over both.
    /// Glyphs are rasterized into the atlas as they are first encountered here
    /// (queued before the pass runs).
    fn build_ops(
        &mut self,
        list: &DrawList,
        font_system: &mut FontSystem,
        sw: f32,
        sh: f32,
    ) -> (Vec<QuadInstance>, Vec<QuadInstance>, Vec<Op>) {
        let white_uv = self.atlas.white_uv;
        let mut instances: Vec<QuadInstance> = Vec::new();
        let mut image_instances: Vec<QuadInstance> = Vec::new();
        let mut ops: Vec<Op> = Vec::new();
        let mut clips: Vec<Rect> = Vec::new();
        let mut batch_start: u32 = 0;
        let mut scissor = Scissor::of(&clips, sw, sh);

        let flush = |instances: &[QuadInstance],
                     ops: &mut Vec<Op>,
                     batch_start: &mut u32,
                     scissor: Scissor| {
            let end = instances.len() as u32;
            if end > *batch_start {
                if let Scissor::Rect(rect) = scissor {
                    ops.push(Op::Quads {
                        rect,
                        range: *batch_start..end,
                    });
                }
                *batch_start = end;
            }
        };

        for cmd in &list.commands {
            match cmd {
                DrawCommand::FillRect { rect, color } => {
                    instances.push(QuadInstance::fill(*rect, *color, white_uv));
                }
                DrawCommand::RoundedRect {
                    rect,
                    color,
                    radius,
                    border_width,
                    border_color,
                    border_sides,
                } => {
                    instances.push(QuadInstance::rounded(
                        *rect,
                        *color,
                        *radius,
                        *border_width,
                        *border_color,
                        *border_sides,
                        white_uv,
                    ));
                }
                DrawCommand::Text { glyphs, color } => {
                    for g in glyphs {
                        if let Some(entry) =
                            self.atlas.ensure(&self.gpu.queue, font_system, g.cache_key)
                        {
                            instances.push(QuadInstance::glyph(g.x, g.y, &entry, *color));
                        }
                    }
                }
                DrawCommand::Image { rect, handle, tint } => {
                    // Flush pending atlas quads first so this image draws in
                    // its correct position in the paint order, not after them.
                    flush(&instances, &mut ops, &mut batch_start, scissor);
                    if let Scissor::Rect(r) = scissor {
                        image_instances.push(QuadInstance::image(*rect, *tint));
                        ops.push(Op::Image {
                            rect: r,
                            handle: *handle,
                            instance: (image_instances.len() - 1) as u32,
                        });
                    }
                }
                DrawCommand::PushClip { rect } => {
                    flush(&instances, &mut ops, &mut batch_start, scissor);
                    clips.push(*rect);
                    scissor = Scissor::of(&clips, sw, sh);
                }
                DrawCommand::PopClip => {
                    flush(&instances, &mut ops, &mut batch_start, scissor);
                    clips.pop();
                    scissor = Scissor::of(&clips, sw, sh);
                }
            }
        }
        flush(&instances, &mut ops, &mut batch_start, scissor);

        (instances, image_instances, ops)
    }
}

/// A resolved scissor rectangle in pixels, or `Empty` when the clip stack
/// intersects to nothing (so the batch is skipped entirely).
#[derive(Clone, Copy)]
enum Scissor {
    Rect([u32; 4]),
    Empty,
}

impl Scissor {
    /// Intersect the clip stack with the surface bounds.
    fn of(clips: &[Rect], sw: f32, sh: f32) -> Self {
        let mut min = Vec2::ZERO;
        let mut max = Vec2::new(sw, sh);
        for clip in clips {
            min = min.max(clip.pos);
            max = max.min(clip.pos + clip.size);
        }
        min = min.max(Vec2::ZERO);
        max = max.min(Vec2::new(sw, sh));
        if max.x <= min.x || max.y <= min.y {
            Scissor::Empty
        } else {
            Scissor::Rect([
                min.x as u32,
                min.y as u32,
                (max.x - min.x) as u32,
                (max.y - min.y) as u32,
            ])
        }
    }
}

/// A pixel -> clip-space projection: (0,0) top-left maps to NDC (-1, +1) and
/// (w,h) bottom-right to (+1, -1). Y-down, matching photon (ADR 0012).
/// Returned column-major to match wgpu's `mat4x4`.
fn screen_projection(w: f32, h: f32) -> [f32; 16] {
    let sx = 2.0 / w;
    let sy = -2.0 / h;
    [
        sx, 0.0, 0.0, 0.0, // col 0
        0.0, sy, 0.0, 0.0, // col 1
        0.0, 0.0, 1.0, 0.0, // col 2
        -1.0, 1.0, 0.0, 1.0, // col 3 (translation)
    ]
}
