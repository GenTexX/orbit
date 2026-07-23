//! atlas - the Editor application: Aurora-based UI shell, scene editing, inspector, code editor, embedded Play.
//!
//! Milestone 3 step 6: the editor looks like an editor - a docked, resizable
//! scene-tree, viewport, inspector, and file explorer, with selection shared
//! across panels and inspector edits committed through the undo history.

mod actions;
mod project;
mod ui;
mod viewport;

use std::path::PathBuf;
use std::sync::Arc;

use aether::Gpu;
use anyhow::{Context, Result};
use aurora::{Event as AuroraEvent, ImageHandle, InputEvent, Key, WidgetKind};
use aurora_wgpu::Renderer as AuroraRenderer;
use glam::Vec2;
use helios::{History, NodeId, Project, Transform, Value};
use photon::{Color as PhotonColor, Renderer as PhotonRenderer, SceneTarget, Texture};
use winit::{
    application::ApplicationHandler,
    event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{Key as WinitKey, ModifiersState, NamedKey},
    window::{Window, WindowId},
};

use crate::ui::{EditorRows, PanelSizes, build_editor_ui, capture_panel_sizes, parse_value};
use crate::viewport::{Drag, EditorCamera, GizmoHit};

fn main() -> Result<()> {
    init_logging();
    tracing::info!("atlas starting");

    // Kept alive for the whole run; dropping it would stop the profiler server.
    let _profiler = init_profiling();

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

/// Start the puffin profiler server when ORBIT_PROFILE is set (the same
/// opt-in as the sandbox and the aurora-wgpu examples). Off otherwise, so
/// every `profiling::scope!` across the editor, aurora, and photon stays a
/// cheap no-op. Connect a viewer with `puffin_viewer --url 127.0.0.1:8585`.
fn init_profiling() -> Option<puffin_http::Server> {
    std::env::var_os("ORBIT_PROFILE")?;
    puffin::set_scopes_on(true);
    match puffin_http::Server::new("0.0.0.0:8585") {
        Ok(server) => {
            tracing::info!("puffin profiler on; connect with: puffin_viewer --url 127.0.0.1:8585");
            Some(server)
        }
        Err(err) => {
            tracing::error!("failed to start puffin server: {err}");
            None
        }
    }
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
    /// A 1x1 white texture the gizmo overlay tints per sprite.
    white: Texture,
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
    /// The editor's pan/zoom view of the scene.
    camera: EditorCamera,
    /// An in-progress viewport drag (move, rotate, or scale).
    drag: Option<Drag>,
    /// Whether a middle-button pan is in progress.
    panning: bool,
    /// The last cursor position, in window pixels.
    cursor: Vec2,
    /// The current keyboard modifiers (for the undo/redo shortcuts).
    modifiers: ModifiersState,
    /// A PNG being dragged from the file explorer; dropped over the viewport
    /// it spawns a sprite there, dropped anywhere else it just cancels.
    file_drag: Option<String>,
    /// The demo texture's natural pixel size (spawned sprites default to it).
    texture_size: Vec2,
    /// Frame counting for the once-a-second fps log.
    frames: u32,
    fps_since: std::time::Instant,
    /// The fps figure currently shown in the status bar.
    last_fps: f32,
    /// The cursor icon currently applied to the window (set only on change).
    applied_cursor: aurora::CursorHint,
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
                state.pointer_moved(p);
            }
            WindowEvent::CursorLeft { .. } => state.ui.handle_input(InputEvent::PointerLeft),
            WindowEvent::MouseInput {
                state: btn_state,
                button,
                ..
            } => match (button, btn_state) {
                (MouseButton::Left, ElementState::Pressed) => {
                    state.ui.handle_input(InputEvent::PointerPressed);
                    state.viewport_press();
                    state.file_press();
                }
                (MouseButton::Left, ElementState::Released) => {
                    state.file_drop();
                    state.end_drag();
                    state.ui.handle_input(InputEvent::PointerReleased);
                }
                (MouseButton::Middle, ElementState::Pressed) => {
                    state.panning = state.over_viewport();
                }
                (MouseButton::Middle, ElementState::Released) => state.panning = false,
                _ => {}
            },
            WindowEvent::MouseWheel { delta, .. } => state.wheel(delta),
            WindowEvent::ModifiersChanged(m) => state.modifiers = m.state(),
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                if !state.handle_shortcut(&event) {
                    for ev in translate_key(&event) {
                        state.ui.handle_input(ev);
                    }
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
        let white = engine.create_texture(&[255, 255, 255, 255], 1, 1);

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
            white,
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
            camera: EditorCamera::default(),
            drag: None,
            panning: false,
            cursor: Vec2::ZERO,
            modifiers: ModifiersState::default(),
            file_drag: None,
            texture_size: Vec2::new(w as f32, h as f32),
            frames: 0,
            fps_since: std::time::Instant::now(),
            last_fps: 0.0,
            applied_cursor: aurora::CursorHint::Default,
        })
    }

    fn resize(&mut self, size: (u32, u32)) {
        // Only the surface needs reconfiguring here: draw() re-sizes the scene
        // target to the viewport widget's laid-out rect every frame, which a
        // window resize changes like any panel drag does.
        self.gui.resize(size);
    }

    /// The viewport widget's on-screen rectangle, from the last layout.
    fn viewport_rect(&self) -> Option<aurora::Rect> {
        self.rows.viewport.and_then(|id| self.ui.rect(id))
    }

    /// Whether the cursor is over the viewport itself (and not a panel).
    fn over_viewport(&self) -> bool {
        self.ui.hit_test(self.cursor).is_some()
            && self.ui.hit_test(self.cursor) == self.rows.viewport
    }

    /// The cursor's world-space position, when the viewport has a layout.
    fn cursor_world(&self) -> Option<Vec2> {
        let rect = self.viewport_rect()?;
        Some(self.camera.screen_to_world(self.cursor - rect.pos))
    }

    /// Route a pointer move: Aurora always sees it; a live pan or drag applies
    /// its delta too.
    fn pointer_moved(&mut self, p: Vec2) {
        let delta = p - self.cursor;
        self.cursor = p;
        self.ui.handle_input(InputEvent::PointerMoved(p));

        if self.panning {
            self.camera.pan_by_screen(delta);
        }
        if let Some(drag) = self.drag
            && let Some(world) = self.cursor_world()
        {
            let (node, _) = drag.target();
            let new = drag.apply(&self.project.scene, world);
            self.project.scene.node_mut(node).transform = new;
            // Rebuild so the inspector's transform rows read out live.
            self.dirty = true;
        }
    }

    /// A left press over the viewport: grab a gizmo handle of the current
    /// selection, else GPU-pick the sprite under the cursor (select and start
    /// moving it), else clear the selection.
    fn viewport_press(&mut self) {
        if self.ui.hit_test(self.cursor) != self.rows.viewport {
            return;
        }
        let (Some(rect), Some(world)) = (self.viewport_rect(), self.cursor_world()) else {
            return;
        };
        let scene = &self.project.scene;

        // Gizmo handles first: they float outside the sprite, so they must win
        // over picking (which only sees sprite pixels).
        if let Some(sel) = self.selected
            && let Some(g) = viewport::gizmo(scene, sel, self.camera.zoom)
            && let Some(hit) = viewport::hit_gizmo(&g, world)
        {
            let original = scene.node(sel).transform;
            self.drag = Some(match hit {
                GizmoHit::MoveX => Drag::MoveAxis {
                    node: sel,
                    original,
                    grab_world: world,
                    axis_world: g.axis_x,
                    vertical: false,
                },
                GizmoHit::MoveY => Drag::MoveAxis {
                    node: sel,
                    original,
                    grab_world: world,
                    axis_world: g.axis_y,
                    vertical: true,
                },
                GizmoHit::Rotate => Drag::Rotate {
                    node: sel,
                    original,
                    pivot: g.center,
                    grab_angle: (world - g.center).to_angle(),
                },
                // The corner scales freely; Shift preserves the ratio
                // (uniform), per the usual editor convention.
                GizmoHit::ScaleCorner if self.modifiers.shift_key() => Drag::ScaleUniform {
                    node: sel,
                    original,
                    pivot: g.center,
                    grab_dist: world.distance(g.center),
                },
                GizmoHit::ScaleCorner => Drag::ScaleFree {
                    node: sel,
                    original,
                    pivot: g.center,
                    axis_x: g.axis_x,
                    axis_y: g.axis_y,
                    grab_proj_x: (world - g.center).dot(g.axis_x),
                    grab_proj_y: (world - g.center).dot(g.axis_y),
                },
                GizmoHit::ScaleX => Drag::ScaleAxis {
                    node: sel,
                    original,
                    pivot: g.center,
                    axis_world: g.axis_x,
                    grab_proj: (world - g.center).dot(g.axis_x),
                    vertical: false,
                },
                GizmoHit::ScaleY => Drag::ScaleAxis {
                    node: sel,
                    original,
                    pivot: g.center,
                    axis_world: g.axis_y,
                    grab_proj: (world - g.center).dot(g.axis_y),
                    vertical: true,
                },
            });
            return;
        }

        let entries = scene.sprite_entries();
        let sprites: Vec<photon::Sprite> = entries.iter().map(|&(_, s)| s).collect();
        let local = self.cursor - rect.pos;
        let camera = self.camera.camera(rect.size);
        let picked = self.engine.pick(
            (self.scene_target.width, self.scene_target.height),
            &camera,
            &self.texture,
            &sprites,
            (local.x.max(0.0) as u32, local.y.max(0.0) as u32),
        );
        match picked {
            Ok(Some(index)) => {
                let node = entries[index].0;
                self.selected = Some(node);
                self.drag = Some(Drag::Move {
                    node,
                    original: self.project.scene.node(node).transform,
                    grab_world: world,
                });
                self.dirty = true;
            }
            Ok(None) => {
                if self.selected.take().is_some() {
                    self.dirty = true;
                }
            }
            Err(err) => tracing::error!("pick failed: {err}"),
        }
    }

    /// Finish a viewport drag: rewind to the press-time transform and commit
    /// the final one through the history, so the whole drag is ONE undo step.
    fn end_drag(&mut self) {
        let Some(drag) = self.drag.take() else {
            return;
        };
        let (node, original) = drag.target();
        let dragged = self.project.scene.node(node).transform;
        if dragged != original {
            self.project.scene.node_mut(node).transform = original;
            self.history
                .set_transform(&mut self.project.scene, node, dragged);
            self.dirty = true;
        }
    }

    /// The wheel zooms the viewport under the cursor; anywhere else it
    /// scrolls the Aurora scroll container under the cursor (panels).
    fn wheel(&mut self, delta: MouseScrollDelta) {
        if !self.over_viewport() {
            // Wheel-up (positive line delta) reveals earlier content, i.e. a
            // negative scroll offset delta; a line tick is ~48px.
            let px = match delta {
                MouseScrollDelta::LineDelta(_, y) => -y * 48.0,
                MouseScrollDelta::PixelDelta(p) => -p.y as f32,
            };
            self.ui.handle_input(InputEvent::Scroll(px));
            return;
        }
        let Some(rect) = self.viewport_rect() else {
            return;
        };
        let factor = match delta {
            MouseScrollDelta::LineDelta(_, y) => 1.1f32.powf(y),
            MouseScrollDelta::PixelDelta(p) => 1.001f32.powf(p.y as f32),
        };
        self.camera.zoom_about(self.cursor - rect.pos, factor);
    }

    /// Keyboard shortcuts: ctrl+s saves; ctrl+z / ctrl+shift+z / ctrl+y undo
    /// and redo (only when no text field is focused, so typing is never
    /// hijacked - saving has no such conflict). Returns whether handled.
    fn handle_shortcut(&mut self, event: &winit::event::KeyEvent) -> bool {
        if !self.modifiers.control_key() {
            return false;
        }
        let WinitKey::Character(c) = &event.logical_key else {
            return false;
        };
        if c.eq_ignore_ascii_case("s") {
            self.save_project();
            return true;
        }
        if self.ui.focused().is_some() {
            return false;
        }
        let changed = if c.eq_ignore_ascii_case("z") {
            if self.modifiers.shift_key() {
                self.history.redo(&mut self.project.scene)
            } else {
                self.history.undo(&mut self.project.scene)
            }
        } else if c.eq_ignore_ascii_case("y") {
            self.history.redo(&mut self.project.scene)
        } else {
            return false;
        };
        if changed {
            self.dirty = true;
        }
        true
    }

    /// A press over a PNG row in the file explorer arms a file drag; releasing
    /// over the viewport drops it as a new sprite ([`file_drop`](Self::file_drop)).
    fn file_press(&mut self) {
        let hit = self.ui.hit_test(self.cursor);
        self.file_drag = self
            .rows
            .file_rows
            .iter()
            .find(|(widget, _)| Some(*widget) == hit)
            .map(|(_, path)| path.clone());
    }

    /// Complete (or cancel) a file drag: dropped over the viewport, spawn a
    /// sprite showing that texture at the drop point; anywhere else, cancel.
    fn file_drop(&mut self) {
        let Some(path) = self.file_drag.take() else {
            return;
        };
        if self.ui.hit_test(self.cursor) != self.rows.viewport {
            return;
        }
        let Some(world) = self.cursor_world() else {
            return;
        };
        let node = actions::spawn_sprite(
            &mut self.project.scene,
            &mut self.history,
            world,
            &path,
            self.texture_size,
        );
        self.selected = Some(node);
        self.dirty = true;
    }

    /// The toolbar's Add Sprite: spawn at the center of the current view.
    fn add_sprite_action(&mut self) {
        let Some(rect) = self.viewport_rect() else {
            return;
        };
        let world = self.camera.screen_to_world(rect.size * 0.5);
        let node = actions::spawn_sprite(
            &mut self.project.scene,
            &mut self.history,
            world,
            "assets/sprite.png",
            self.texture_size,
        );
        self.selected = Some(node);
        self.dirty = true;
    }

    fn save_project(&mut self) {
        match self.project.save(&self.project_dir) {
            Ok(()) => tracing::info!("project saved to {}", self.project_dir.display()),
            Err(err) => tracing::error!("save failed: {err}"),
        }
    }

    /// Reload the project from disk, discarding unsaved edits. The history is
    /// cleared too: its edits hold NodeIds from the old scene's arena, which
    /// mean nothing (or worse, the wrong node) in the freshly loaded one.
    fn load_project(&mut self) {
        match Project::load(&self.project_dir) {
            Ok(project) => {
                self.project = project;
                self.history = History::new();
                self.selected = None;
                self.dirty = true;
                tracing::info!("project loaded from {}", self.project_dir.display());
            }
            Err(err) => tracing::error!("load failed: {err}"),
        }
    }

    /// Log the frame rate once a second (the M3 "usable editor" health check).
    fn report_fps(&mut self) {
        self.frames += 1;
        let elapsed = self.fps_since.elapsed().as_secs_f32();
        if elapsed >= 1.0 {
            let fps = self.frames as f32 / elapsed;
            self.last_fps = fps;
            tracing::debug!("{} fps ({:.2} ms/frame)", fps as u32, 1000.0 / fps);
            self.frames = 0;
            self.fps_since = std::time::Instant::now();
        }
    }

    /// Map Aurora's cursor hint onto the window cursor (resize arrows over
    /// splitters, a text beam over inputs), setting it only when it changes.
    fn apply_cursor(&mut self) {
        use winit::window::CursorIcon;
        let hint = self.ui.cursor_hint();
        if hint == self.applied_cursor {
            return;
        }
        self.applied_cursor = hint;
        self.window.set_cursor(match hint {
            aurora::CursorHint::Default => CursorIcon::Default,
            aurora::CursorHint::Text => CursorIcon::Text,
            aurora::CursorHint::ResizeHorizontal => CursorIcon::ColResize,
            aurora::CursorHint::ResizeVertical => CursorIcon::RowResize,
        });
    }

    /// Refresh the status bar readouts in place (no shell rebuild).
    fn update_status_bar(&mut self) {
        let cursor = match (self.over_viewport(), self.cursor_world()) {
            (true, Some(w)) => format!("x {:.0}, y {:.0}", w.x, w.y),
            _ => "x -, y -".to_string(),
        };
        let selected = match self.selected {
            Some(node) => self.project.scene.node(node).name.clone(),
            None => "nothing selected".to_string(),
        };
        let zoom = format!("zoom {:.0}%", self.camera.zoom * 100.0);
        let fps = format!("{:.0} fps", self.last_fps);
        for (id, text) in [
            (self.rows.status_cursor, cursor),
            (self.rows.status_zoom, zoom),
            (self.rows.status_selected, selected),
            (self.rows.status_fps, fps),
        ] {
            if let Some(id) = id {
                self.ui.set_label(id, text);
            }
        }
    }

    fn draw(&mut self) -> Result<()> {
        profiling::scope!("editor_frame");
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

        self.apply_cursor();
        self.update_status_bar();

        // Lay the shell out first: the viewport widget's rect decides how big
        // the scene renders.
        let size = self.window.inner_size();
        let window = Vec2::new(size.width.max(1) as f32, size.height.max(1) as f32);
        self.ui.layout(window)?;

        // Match the scene target to the viewport widget's on-screen size, so
        // sprites draw 1:1 (a panel resize reflows the viewport, it does not
        // stretch it). The handle stays valid across the swap (update_image),
        // so no shell rebuild is needed - a splitter drag re-targets live.
        let viewport = self
            .rows
            .viewport
            .and_then(|id| self.ui.rect(id))
            .map_or(window, |r| r.size);
        let (w, h) = (
            viewport.x.round().max(1.0) as u32,
            viewport.y.round().max(1.0) as u32,
        );
        if (w, h) != (self.scene_target.width, self.scene_target.height) {
            self.scene_target = self.engine.create_scene_target(w, h);
            self.gui
                .update_image(self.viewport_handle, self.scene_target.view());
        }

        profiling::scope!("scene_render");
        let clear = PhotonColor::new(0.02, 0.02, 0.05, 1.0);
        let camera = self.camera.camera(Vec2::new(w as f32, h as f32));
        self.engine.render_to_target(
            &self.scene_target,
            clear,
            &camera,
            &self.texture,
            &self.project.scene.sprites(),
        );

        // Editor overlays, drawn over the scene in a second pass (LoadOp::Load)
        // with the tintable white texture: the axis guide line first (under),
        // then the selection's outline and gizmo handles.
        let mut overlay = Vec::new();
        if let Some(drag) = &self.drag
            && let Some(guide) = viewport::axis_guide_sprite(
                drag,
                &self.project.scene,
                &self.camera,
                Vec2::new(w as f32, h as f32),
            )
        {
            overlay.push(guide);
        }
        if let Some(sel) = self.selected
            && let Some(g) = viewport::gizmo(&self.project.scene, sel, self.camera.zoom)
        {
            overlay.extend(viewport::gizmo_sprites(&g, self.camera.zoom));
        }
        if !overlay.is_empty() {
            self.engine
                .overlay_to_target(&self.scene_target, &camera, &self.white, &overlay);
        }

        let list = self.ui.draw_list();
        self.gui.render(
            &list,
            self.ui.font_system_mut(),
            aurora::Color::rgb(0.08, 0.08, 0.10),
        )?;

        self.report_fps();
        // Mark the frame boundary for the profiler (a no-op when profiling off).
        profiling::finish_frame!();
        Ok(())
    }

    /// Drain this frame's Aurora events: a tree-row click selects its node, a
    /// toolbar click runs its action, a submitted field commits its text
    /// through the undo history.
    fn react(&mut self) {
        for event in self.ui.drain_events() {
            match event {
                AuroraEvent::Clicked(id) if Some(id) == self.rows.add_sprite => {
                    self.add_sprite_action();
                }
                AuroraEvent::Clicked(id) if Some(id) == self.rows.save => self.save_project(),
                AuroraEvent::Clicked(id) if Some(id) == self.rows.load => self.load_project(),
                AuroraEvent::Clicked(id) => {
                    if let Some(&(_, node)) =
                        self.rows.tree_rows.iter().find(|(widget, _)| *widget == id)
                    {
                        self.selected = Some(node);
                        self.dirty = true;
                    }
                }
                AuroraEvent::Submitted(id) => {
                    self.commit_field(id);
                    self.commit_transform_field(id);
                }
                _ => {}
            }
        }
    }

    /// Commit a submitted transform row (position/rotation/scale) through the
    /// undo history. A no-op for other widgets or unparseable text.
    fn commit_transform_field(&mut self, id: aurora::WidgetId) {
        let Some(&(_, node, field)) = self
            .rows
            .transform_rows
            .iter()
            .find(|(widget, ..)| *widget == id)
        else {
            return;
        };
        let WidgetKind::TextInput(text) = self.ui.kind(id) else {
            return;
        };
        let text = text.clone();
        let t = self.project.scene.node(node).transform;
        let new = match field {
            "position" => parse_value(&text, &Value::Vec2(t.translation)).map(|v| match v {
                Value::Vec2(p) => Transform {
                    translation: p,
                    ..t
                },
                _ => t,
            }),
            // Displayed and edited in degrees, stored in radians.
            "rotation" => parse_value(&text, &Value::F32(t.rotation)).map(|v| match v {
                Value::F32(deg) => Transform {
                    rotation: deg.to_radians(),
                    ..t
                },
                _ => t,
            }),
            "scale" => parse_value(&text, &Value::Vec2(t.scale)).map(|v| match v {
                Value::Vec2(s) => Transform { scale: s, ..t },
                _ => t,
            }),
            _ => None,
        };
        if let Some(new) = new
            && new != t
        {
            self.history
                .set_transform(&mut self.project.scene, node, new);
            self.dirty = true;
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
        if let Some(new) = parse_value(&text, &old)
            && new != old
        {
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
