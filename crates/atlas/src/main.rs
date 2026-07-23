//! atlas - the Editor application: Aurora-based UI shell, scene editing, inspector, code editor, embedded Play.
//!
//! Milestone 3 step 5: a bare window proving the GPU integration (ADR 0018) -
//! one shared wgpu device, the engine's scene rendered into a texture, and
//! Aurora compositing that texture into a viewport widget. No panels, no
//! interaction yet; that is step 6.

use std::sync::Arc;

use aether::Gpu;
use anyhow::{Context, Result};
use aurora::{Style, Ui};
use aurora_wgpu::Renderer as AuroraRenderer;
use glam::Vec2;
use helios::{Component, Node, Scene, SpriteComponent, Transform};
use photon::{Camera, Color as PhotonColor, Renderer as PhotonRenderer, SceneTarget, Texture};
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    window::{Window, WindowId},
};

fn main() -> Result<()> {
    init_logging();
    tracing::info!("atlas starting");

    let event_loop = EventLoop::new()?;
    // A live viewport redraws continuously, like the sandbox's game loop.
    event_loop.set_control_flow(ControlFlow::Poll);
    event_loop.run_app(&mut App::default())?;
    Ok(())
}

fn init_logging() {
    use tracing_subscriber::{EnvFilter, fmt};
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,wgpu_core=warn,wgpu_hal=warn,naga=warn"));
    fmt().with_env_filter(filter).init();
}

#[derive(Default)]
struct App {
    state: Option<State>,
}

struct State {
    window: Arc<Window>,
    gui: AuroraRenderer,
    engine: PhotonRenderer,
    white: Texture,
    scene: Scene,
    scene_target: SceneTarget,
    ui: Ui,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        match State::new(event_loop) {
            Ok(state) => {
                state.window.request_redraw();
                self.state = Some(state);
            }
            Err(err) => {
                tracing::error!("failed to start atlas: {err:#}");
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(state) = self.state.as_mut() else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => state.resize((size.width, size.height)),
            WindowEvent::RedrawRequested => {
                if let Err(err) = state.draw() {
                    tracing::error!("render error: {err:#}");
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(state) = &self.state {
            state.window.request_redraw();
        }
    }
}

impl State {
    /// Bootstrap the shared GPU context (ADR 0018), the two renderers it feeds,
    /// a small demo scene, and the viewport widget that shows it.
    fn new(event_loop: &ActiveEventLoop) -> Result<Self> {
        let window = Arc::new(
            event_loop
                .create_window(Window::default_attributes().with_title("Orbit - Atlas"))
                .context("create window")?,
        );
        let size = window.inner_size();

        // One instance, one adapter, one device: photon (the scene) and
        // aurora-wgpu (the chrome) both render on it (ADR 0018). The adapter is
        // chosen compatible with the window surface aurora-wgpu will present to.
        let instance = Gpu::default_instance();
        let surface = instance
            .create_surface(window.clone())
            .context("create window surface")?;
        let gpu = pollster::block_on(Gpu::request(instance, Some(&surface)))
            .context("acquire shared GPU context")?;

        let mut gui = AuroraRenderer::from_gpu(gpu.clone(), surface, (size.width, size.height))
            .context("create GUI renderer")?;
        let engine = PhotonRenderer::from_gpu(gpu);
        let white = engine.create_texture(&[255, 255, 255, 255], 1, 1);

        let scene = demo_scene();
        let (scene_target, ui) = build_viewport(&mut gui, &engine, (size.width, size.height));

        Ok(Self {
            window,
            gui,
            engine,
            white,
            scene,
            scene_target,
            ui,
        })
    }

    fn resize(&mut self, size: (u32, u32)) {
        self.gui.resize(size);
        // ImageRegistry has no update-in-place (see aurora-wgpu's image.rs), so
        // a resized scene target is re-registered under a fresh handle, and the
        // (currently trivial) widget tree is rebuilt to reference it.
        let (scene_target, ui) = build_viewport(&mut self.gui, &self.engine, size);
        self.scene_target = scene_target;
        self.ui = ui;
    }

    fn draw(&mut self) -> Result<()> {
        let size = self.window.inner_size();
        let viewport = Vec2::new(size.width.max(1) as f32, size.height.max(1) as f32);
        let clear = PhotonColor::new(0.02, 0.02, 0.05, 1.0);

        // Render the scene into its offscreen target, on the same device the
        // GUI will composite it from (no readback - ADR 0018).
        let camera = Camera::new(Vec2::ZERO, viewport);
        self.engine.render_to_target(
            &self.scene_target,
            clear,
            &camera,
            &self.white,
            &self.scene.sprites(),
        );

        // Lay out and draw the (trivial, for now) Aurora tree: one widget
        // sampling the scene texture just rendered.
        self.ui.layout(viewport)?;
        let list = self.ui.draw_list();
        self.gui.render(
            &list,
            self.ui.font_system_mut(),
            aurora::Color::rgb(0.08, 0.08, 0.10),
        )?;
        Ok(())
    }
}

/// Create the scene render target sized to `size`, register it as an Aurora
/// image, and build the (currently trivial) widget tree around it.
fn build_viewport(
    gui: &mut AuroraRenderer,
    engine: &PhotonRenderer,
    size: (u32, u32),
) -> (SceneTarget, Ui) {
    let scene_target = engine.create_scene_target(size.0.max(1), size.1.max(1));
    let handle = gui.register_image(scene_target.view());

    let mut ui = Ui::new();
    let root = ui.root_panel(Style::new().fill());
    ui.image(root, handle, Style::new().fill());

    (scene_target, ui)
}

/// A small scene to prove the pipeline: one red sprite, off-center.
fn demo_scene() -> Scene {
    let mut scene = Scene::new("root");
    let root = scene.root();
    let mut sprite = Node::new("sprite");
    sprite.transform = Transform::from_translation(Vec2::new(80.0, 60.0));
    sprite.components.push(Component::Sprite(SpriteComponent {
        texture: String::new(),
        tint: [0.85, 0.30, 0.30, 1.0],
        size: Vec2::new(120.0, 90.0),
    }));
    scene.add_child(root, sprite);
    scene
}
