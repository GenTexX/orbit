//! orbit-renderer renderer: builds the sprite pipeline and draws batches; offscreen now, on-screen at Step 3.

use wgpu::util::DeviceExt;

use crate::{
    camera::Camera,
    color::Color,
    error::RendererError,
    gpu::Gpu,
    sprite::{RawInstance, Sprite},
    texture::Texture,
};

/// The offscreen target format. Linear (not sRGB) so read-back pixel values
/// round-trip exactly, which keeps the render tests precise. The windowed path
/// (Step 3) will render to the surface's own (sRGB) format instead.
const OFFSCREEN_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Camera data as uploaded to the GPU; `#[repr(C)]` to match the `Camera`
/// uniform block in sprite.wgsl.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct CameraUniform {
    view_proj: [f32; 16],
}

/// Renders sprite batches. Owns the GPU context and the sprite pipeline; the
/// per-frame camera, texture, and instance data are supplied at draw time.
pub struct Renderer {
    gpu: Gpu,
    pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    camera_layout: wgpu::BindGroupLayout,
    texture_layout: wgpu::BindGroupLayout,
}

impl Renderer {
    /// Create a headless renderer with no window, targeting an offscreen image.
    /// Blocks on GPU acquisition; see `Gpu::headless`.
    pub fn headless() -> Result<Self, RendererError> {
        let gpu = Gpu::headless()?;
        Ok(Self::from_gpu(gpu))
    }

    /// Build the pipeline and bind-group layouts on top of an acquired GPU
    /// context. Shared by every construction path.
    fn from_gpu(gpu: Gpu) -> Self {
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

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sprite pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[Some(RawInstance::layout())],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: OFFSCREEN_FORMAT,
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
        });

        Self {
            gpu,
            pipeline,
            sampler,
            camera_layout,
            texture_layout,
        }
    }

    /// Upload an RGBA8 image (row-major, `width * height * 4` bytes) as a texture.
    pub fn create_texture(&self, rgba: &[u8], width: u32, height: u32) -> Texture {
        Texture::from_rgba(&self.gpu.device, &self.gpu.queue, rgba, width, height)
    }

    /// Render `sprites` (all sampling `texture`) through `camera` into an
    /// offscreen image of `size`, over a `clear` background. Returns the result
    /// as row-major RGBA8 bytes (`width * height * 4`).
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

        let camera_uniform = CameraUniform {
            view_proj: camera.view_projection().to_cols_array(),
        };
        let camera_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("camera uniform"),
            contents: bytemuck::bytes_of(&camera_uniform),
            usage: wgpu::BufferUsages::UNIFORM,
        });
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

        // Skip the instance buffer entirely for an empty scene - a zero-sized
        // buffer is invalid, and there is nothing to draw anyway.
        let instances: Vec<RawInstance> = sprites.iter().map(|s| s.to_raw()).collect();
        let instance_buffer = (!instances.is_empty()).then(|| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("sprite instances"),
                contents: bytemuck::cast_slice(&instances),
                usage: wgpu::BufferUsages::VERTEX,
            })
        });

        // Read-back rows must be aligned; pad the buffer's bytes-per-row up to
        // the required alignment and strip the padding after mapping.
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
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("sprite pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target_view,
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

            if let Some(instance_buffer) = &instance_buffer {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &camera_bind_group, &[]);
                pass.set_bind_group(1, &texture_bind_group, &[]);
                pass.set_vertex_buffer(0, instance_buffer.slice(..));
                pass.draw(0..6, 0..instances.len() as u32);
            }
        }

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

        // Block until the copy completes and the buffer is mapped.
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
