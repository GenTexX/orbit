//! aurora-wgpu renderer: turns an Aurora draw list into batched, clipped colored quads on a surface.

use aurora::{Color, DrawCommand, DrawList, Rect};
use glam::Vec2;
use wgpu::util::DeviceExt;

use crate::error::RenderError;

/// One filled rectangle as uploaded to the GPU; `#[repr(C)]` to match the
/// vertex attributes and the `Instance` struct in rect.wgsl.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct RectInstance {
    pos: [f32; 2],
    size: [f32; 2],
    color: [f32; 4],
}

impl RectInstance {
    fn layout() -> wgpu::VertexBufferLayout<'static> {
        const ATTRS: [wgpu::VertexAttribute; 3] =
            wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4];
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<RectInstance>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &ATTRS,
        }
    }
}

/// Renders Aurora draw lists to a window surface. Windowing-agnostic: it takes a
/// `raw-window-handle` target plus a size, exactly like photon's renderer.
pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    screen_buffer: wgpu::Buffer,
    screen_bind_group: wgpu::BindGroup,
}

impl Renderer {
    /// Create a renderer that presents to `target` (e.g. an `Arc<Window>`) at the
    /// given pixel `size`. Blocks on GPU acquisition; wgpu's async setup never
    /// leaks to callers.
    pub fn new(
        target: impl Into<wgpu::SurfaceTarget<'static>>,
        size: (u32, u32),
    ) -> Result<Self, RenderError> {
        // Validation layers are a debugging aid, off by default; opt in with
        // WGPU_VALIDATION=1 (see photon's gpu.rs for the same rationale).
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle_from_env();
        if std::env::var_os("WGPU_VALIDATION").is_none() {
            desc.flags.remove(wgpu::InstanceFlags::VALIDATION);
        }
        let instance = wgpu::Instance::new(desc);
        let surface = instance.create_surface(target)?;

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            ..Default::default()
        }))?;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("aurora-wgpu device"),
                ..Default::default()
            }))?;

        let (width, height) = (size.0.max(1), size.1.max(1));
        let config = surface
            .get_default_config(&adapter, width, height)
            .ok_or(RenderError::SurfaceUnsupported)?;
        surface.configure(&device, &config);

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

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("rect shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("rect.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("rect pipeline layout"),
            bind_group_layouts: &[Some(&screen_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("rect pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[Some(RectInstance::layout())],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
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

        Ok(Self {
            device,
            queue,
            surface,
            config,
            pipeline,
            screen_buffer,
            screen_bind_group,
        })
    }

    /// Reconfigure the surface for a new window size.
    pub fn resize(&mut self, size: (u32, u32)) {
        self.config.width = size.0.max(1);
        self.config.height = size.1.max(1);
        self.surface.configure(&self.device, &self.config);
    }

    /// Draw `list` over a `clear` background and present. Runs of filled rects
    /// under the same clip are batched into one instanced draw; a `PushClip` or
    /// `PopClip` flushes the current batch and updates the scissor.
    pub fn render(&mut self, list: &DrawList, clear: Color) -> Result<(), RenderError> {
        let (sw, sh) = (self.config.width as f32, self.config.height as f32);
        self.queue.write_buffer(
            &self.screen_buffer,
            0,
            bytemuck::cast_slice(&screen_projection(sw, sh)),
        );

        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f)
            | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
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

        // Translate the command list into an instance buffer plus a set of
        // (scissor, instance range) draws.
        let mut instances: Vec<RectInstance> = Vec::new();
        let mut draws: Vec<(Scissor, std::ops::Range<u32>)> = Vec::new();
        let mut clips: Vec<Rect> = Vec::new();
        let mut batch_start: u32 = 0;
        let mut scissor = Scissor::of(&clips, sw, sh);

        let flush = |instances: &[RectInstance],
                     draws: &mut Vec<(Scissor, std::ops::Range<u32>)>,
                     batch_start: &mut u32,
                     scissor: Scissor| {
            let end = instances.len() as u32;
            if end > *batch_start {
                if let Scissor::Rect(r) = scissor {
                    draws.push((Scissor::Rect(r), *batch_start..end));
                }
                *batch_start = end;
            }
        };

        for cmd in &list.commands {
            match *cmd {
                DrawCommand::FillRect { rect, color } => instances.push(RectInstance {
                    pos: rect.pos.to_array(),
                    size: rect.size.to_array(),
                    color: [color.r, color.g, color.b, color.a],
                }),
                DrawCommand::PushClip { rect } => {
                    flush(&instances, &mut draws, &mut batch_start, scissor);
                    clips.push(rect);
                    scissor = Scissor::of(&clips, sw, sh);
                }
                DrawCommand::PopClip => {
                    flush(&instances, &mut draws, &mut batch_start, scissor);
                    clips.pop();
                    scissor = Scissor::of(&clips, sw, sh);
                }
            }
        }
        flush(&instances, &mut draws, &mut batch_start, scissor);

        let instance_buffer = (!instances.is_empty()).then(|| {
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("rect instances"),
                    contents: bytemuck::cast_slice(&instances),
                    usage: wgpu::BufferUsages::VERTEX,
                })
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
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

            if let Some(buffer) = &instance_buffer {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &self.screen_bind_group, &[]);
                pass.set_vertex_buffer(0, buffer.slice(..));
                for (scissor, range) in &draws {
                    let Scissor::Rect([x, y, w, h]) = scissor else {
                        continue;
                    };
                    pass.set_scissor_rect(*x, *y, *w, *h);
                    pass.draw(0..6, range.clone());
                }
            }
        }
        self.queue.submit(Some(encoder.finish()));
        self.queue.present(frame);
        Ok(())
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
