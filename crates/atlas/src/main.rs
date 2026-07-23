//! atlas - the Editor application: Aurora-based UI shell, scene editing, inspector, code editor, embedded Play.
//!
//! Milestone 3 step 6: the editor looks like an editor - a docked, resizable
//! scene-tree, viewport, inspector, and file explorer, with selection shared
//! across panels and inspector edits committed through the undo history.

mod project;
mod ui;

use std::path::PathBuf;
use std::sync::Arc;

use aether::Gpu;
use anyhow::{Context, Result};
use aurora::{Event as AuroraEvent, ImageHandle, InputEvent, Key, WidgetKind};
use aurora_wgpu::Renderer as AuroraRenderer;
use glam::Vec2;
use helios::{History, NodeId, Project};
use photon::{Camera, Color as PhotonColor, Renderer as PhotonRenderer, SceneTarget, Texture};
use winit::{
    application::ApplicationHandler,
    event::{ElementState, MouseButton, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{Key as WinitKey, NamedKey},
    window::{Window, WindowId},
};

use crate::ui::{EditorRows, PanelSizes, build_editor_ui, capture_panel_sizes, parse_value};

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
    texture: Texture,
    project: Project,
    project_dir: PathBuf,
    history: History,
    selected: Option<NodeId>,
    scene_target: SceneTarget,
    viewport_handle: ImageHandle,
    ui: aurora::Ui,
    rows: EditorRows,
    /// The dock's panel sizes, captured from the live `Ui` before each rebuild
    /// so splitter drags survive rebuilds (and, later, restarts).
    panel_sizes: PanelSizes,
    /// Set when the scene, selection, or viewport handle changed since the
    /// `Ui` was last built; rebuilding only then (not every frame) keeps
    /// in-progress typing and hover state intact across ordinary frames.
    dirty: bool,
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
            WindowEvent::CursorMoved { position, .. } => {
                let p = Vec2::new(position.x as f32, position.y as f32);
                state.ui.handle_input(InputEvent::PointerMoved(p));
            }
            WindowEvent::CursorLeft { .. } => state.ui.handle_input(InputEvent::PointerLeft),
            WindowEvent::MouseInput {
                state: btn_state,
                button: MouseButton::Left,
                ..
            } => {
                let ev = match btn_state {
                    ElementState::Pressed => InputEvent::PointerPressed,
                    ElementState::Released => InputEvent::PointerReleased,
                };
                state.ui.handle_input(ev);
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                for ev in translate_key(&event) {
                    state.ui.handle_input(ev);
                }
            }
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
    /// the demo project (opened or created), and the editor shell around it.
    fn new(event_loop: &ActiveEventLoop) -> Result<Self> {
        let window = Arc::new(
            event_loop
                .create_window(Window::default_attributes().with_title("Orbit - Atlas"))
                .context("create window")?,
        );
        let size = window.inner_size();

        // One instance, one adapter, one device: photon (the scene) and
        // aurora-wgpu (the chrome) both render on it (ADR 0018).
        let instance = Gpu::default_instance();
        let surface = instance
            .create_surface(window.clone())
            .context("create window surface")?;
        let gpu = pollster::block_on(Gpu::request(instance, Some(&surface)))
            .context("acquire shared GPU context")?;

        let mut gui = AuroraRenderer::from_gpu(gpu.clone(), surface, (size.width, size.height))
            .context("create GUI renderer")?;
        let engine = PhotonRenderer::from_gpu(gpu);

        let project_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("demo_project");
        let project = project::open_or_create(&project_dir)?;
        let sprite_bytes =
            std::fs::read(project_dir.join("assets/sprite.png")).context("read demo sprite")?;
        let decoded = image::load_from_memory(&sprite_bytes)
            .context("decode demo sprite")?
            .to_rgba8();
        let (w, h) = decoded.dimensions();
        let texture = engine.create_texture(&decoded.into_raw(), w, h);

        let (scene_target, viewport_handle) =
            build_viewport_target(&mut gui, &engine, (size.width, size.height));
        let panel_sizes = PanelSizes::default();
        let (ui, rows) = build_editor_ui(
            &project.scene,
            None,
            viewport_handle,
            &project_dir,
            panel_sizes,
        );

        Ok(Self {
            window,
            gui,
            engine,
            texture,
            project,
            project_dir,
            history: History::new(),
            selected: None,
            scene_target,
            viewport_handle,
            ui,
            rows,
            panel_sizes,
            dirty: false,
        })
    }

    fn resize(&mut self, size: (u32, u32)) {
        self.gui.resize(size);
        // The scene target and its registered handle are sized to the window,
        // so both are rebuilt; the shell is rebuilt next draw to reference the
        // fresh handle (ImageRegistry has no update-in-place - see
        // aurora-wgpu's image.rs).
        let (scene_target, viewport_handle) =
            build_viewport_target(&mut self.gui, &self.engine, size);
        self.scene_target = scene_target;
        self.viewport_handle = viewport_handle;
        self.dirty = true;
    }

    fn draw(&mut self) -> Result<()> {
        self.react();
        if self.dirty {
            // Splitter drags live only in the widget tree; read the panels'
            // current sizes back before discarding it, or a rebuild would snap
            // every panel to its default (a real bug the first time).
            self.panel_sizes = capture_panel_sizes(&self.ui, &self.rows, self.panel_sizes);
            let (ui, rows) = build_editor_ui(
                &self.project.scene,
                self.selected,
                self.viewport_handle,
                &self.project_dir,
                self.panel_sizes,
            );
            self.ui = ui;
            self.rows = rows;
            self.dirty = false;
        }

        let size = self.window.inner_size();
        let viewport = Vec2::new(size.width.max(1) as f32, size.height.max(1) as f32);
        let clear = PhotonColor::new(0.02, 0.02, 0.05, 1.0);

        let camera = Camera::new(Vec2::ZERO, viewport);
        self.engine.render_to_target(
            &self.scene_target,
            clear,
            &camera,
            &self.texture,
            &self.project.scene.sprites(),
        );

        self.ui.layout(viewport)?;
        let list = self.ui.draw_list();
        self.gui.render(
            &list,
            self.ui.font_system_mut(),
            aurora::Color::rgb(0.08, 0.08, 0.10),
        )?;
        Ok(())
    }

    /// Drain this frame's Aurora events: a tree-row click selects its node, a
    /// submitted field commits its text through the undo history.
    fn react(&mut self) {
        for event in self.ui.drain_events() {
            match event {
                AuroraEvent::Clicked(id) => {
                    if let Some(&(_, node)) =
                        self.rows.tree_rows.iter().find(|(widget, _)| *widget == id)
                    {
                        self.selected = Some(node);
                        self.dirty = true;
                    }
                }
                AuroraEvent::Submitted(id) => self.commit_field(id),
                _ => {}
            }
        }
    }

    fn commit_field(&mut self, id: aurora::WidgetId) {
        let Some(&(_, node, component, field)) = self
            .rows
            .field_rows
            .iter()
            .find(|(widget, ..)| *widget == id)
        else {
            return;
        };
        let WidgetKind::TextInput(text) = self.ui.kind(id) else {
            return;
        };
        let text = text.clone();
        let Some(old) = self
            .project
            .scene
            .node(node)
            .components
            .get(component)
            .and_then(|c| c.as_reflect().get(field))
        else {
            return;
        };
        // A value that fails to parse leaves the field's prior value in place
        // (parse_value returns None; the text the user typed stays visible
        // until they fix it or select elsewhere, since we only rebuild the
        // shell - and so re-render the field from the scene - on success).
        if let Some(new) = parse_value(&text, &old) {
            self.history
                .set_field(&mut self.project.scene, node, component, field, new);
            self.dirty = true;
        }
    }
}

/// Create the scene render target sized to `size` and register it as an
/// Aurora image, returning the handle the viewport widget will reference.
fn build_viewport_target(
    gui: &mut AuroraRenderer,
    engine: &PhotonRenderer,
    size: (u32, u32),
) -> (SceneTarget, ImageHandle) {
    let scene_target = engine.create_scene_target(size.0.max(1), size.1.max(1));
    let handle = gui.register_image(scene_target.view());
    (scene_target, handle)
}

/// Map a winit key press to Aurora input: a named editing key, or the typed
/// text as a run of character events.
fn translate_key(event: &winit::event::KeyEvent) -> Vec<InputEvent> {
    let named = match &event.logical_key {
        WinitKey::Named(NamedKey::Backspace) => Some(Key::Backspace),
        WinitKey::Named(NamedKey::Delete) => Some(Key::Delete),
        WinitKey::Named(NamedKey::ArrowLeft) => Some(Key::Left),
        WinitKey::Named(NamedKey::ArrowRight) => Some(Key::Right),
        WinitKey::Named(NamedKey::Home) => Some(Key::Home),
        WinitKey::Named(NamedKey::End) => Some(Key::End),
        WinitKey::Named(NamedKey::Enter) => Some(Key::Enter),
        _ => None,
    };
    if let Some(key) = named {
        return vec![InputEvent::Key(key)];
    }
    event
        .text
        .iter()
        .flat_map(|t| t.chars())
        .filter(|c| !c.is_control())
        .map(InputEvent::Text)
        .collect()
}
