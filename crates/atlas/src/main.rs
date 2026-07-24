//! atlas - the Editor application: Aurora-based UI shell, scene editing, inspector, code editor, embedded Play.
//!
//! Milestone 3 step 6: the editor looks like an editor - a docked, resizable
//! scene-tree, viewport, inspector, and file explorer, with selection shared
//! across panels and inspector edits committed through the undo history.

mod actions;
mod color;
mod icons;
mod project;
mod textures;
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
use photon::{Color as PhotonColor, Renderer as PhotonRenderer, SceneTarget, Sprite, Texture};
use winit::{
    application::ApplicationHandler,
    event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{Key as WinitKey, ModifiersState, NamedKey},
    window::{Window, WindowId},
};

use crate::color::Hsva;
use crate::icons::Icons;
use crate::textures::TextureCache;
use crate::ui::{
    Axis, ColorPickerView, ColorTarget, ContextMenu, EditorRows, FieldRef, MenuAction, PanelSizes,
    build_color_picker, build_editor_ui, capture_panel_sizes, field_color, field_vec2, parse_value,
    set_field_color, set_field_vec2, with_axis,
};
use crate::viewport::{Drag, EditorCamera, GizmoHit, GizmoMode};

/// The color picker's gradient image sizes (px): the SV square, the hue bar
/// width, and the alpha bar height. Must match the widget sizes in
/// `build_color_picker` for a crisp 1:1 draw.
const SV: usize = 150;
const HUE_W: usize = 18;
const ALPHA_H: usize = 16;

/// Which of the color picker's gradients a press is dragging.
#[derive(Debug, Clone, Copy)]
enum PickerDrag {
    Sv,
    Hue,
    Alpha,
}

/// The open color picker: the field it edits, the color at open time (for one
/// undo step on close), the working HSVA, where it floats, and any live drag.
#[derive(Debug, Clone, Copy)]
struct ColorPicker {
    target: ColorTarget,
    original: [f32; 4],
    hsva: Hsva,
    anchor: Vec2,
    drag: Option<PickerDrag>,
}

/// An in-progress inspector label drag-scrub: adjusting one component of a
/// Vec2 field by the horizontal cursor motion since the press.
#[derive(Debug, Clone, Copy)]
struct ScrubDrag {
    field: FieldRef,
    axis: Axis,
    original: Vec2,
    start_x: f32,
}

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
    /// Scene textures keyed by path, loaded lazily so each sprite draws with
    /// the texture it names (per-texture batching).
    textures: TextureCache,
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
    /// The open right-click context menu, if any (Aurora popup layer).
    context_menu: Option<ContextMenu>,
    /// The active transform tool (select/move/rotate/scale), chosen from the
    /// toolbar; filters which gizmo handles show and are hittable.
    gizmo_mode: GizmoMode,
    /// The OS clipboard (for text copy/cut/paste), or `None` if it could not
    /// be opened on this platform.
    clipboard: Option<arboard::Clipboard>,
    /// An in-progress inspector label drag-scrub, if any.
    scrub: Option<ScrubDrag>,
    /// The toolbar icons, rasterized and registered once at startup.
    icons: Icons,
    /// The open color picker, if any.
    picker: Option<ColorPicker>,
    /// The picker's three gradient images (registered once, updated in place as
    /// the color changes).
    picker_sv: ImageHandle,
    picker_hue: ImageHandle,
    picker_alpha: ImageHandle,
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
                    // A press outside an open menu dismisses it (before the
                    // viewport/gizmo logic, so a click on empty space just
                    // closes the menu without also deselecting).
                    if state.close_menu_if_outside() {
                        return;
                    }
                    state.ui.handle_input(InputEvent::PointerPressed);
                    // The color picker (if open) claims the press: a drag on a
                    // gradient, or a dismiss when the press lands outside it.
                    if state.handle_picker_press() {
                        return;
                    }
                    // A press on an inspector axis label starts a value scrub
                    // and consumes the press (no viewport pick behind it).
                    if state.begin_scrub() {
                        return;
                    }
                    state.viewport_press();
                    state.file_press();
                }
                (MouseButton::Left, ElementState::Released) => {
                    state.end_picker_drag();
                    state.end_scrub();
                    state.file_drop();
                    state.end_drag();
                    state.ui.handle_input(InputEvent::PointerReleased);
                }
                (MouseButton::Right, ElementState::Pressed) => state.open_context_menu(),
                (MouseButton::Middle, ElementState::Pressed) => {
                    state.panning = state.over_viewport();
                }
                (MouseButton::Middle, ElementState::Released) => state.panning = false,
                _ => {}
            },
            WindowEvent::MouseWheel { delta, .. } => state.wheel(delta),
            WindowEvent::ModifiersChanged(m) => {
                state.modifiers = m.state();
                // Aurora needs shift to know whether movement keys extend a
                // text selection.
                state.ui.set_shift(m.state().shift_key());
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                if !state.handle_shortcut(&event) && !state.handle_mode_key(&event) {
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
        let textures = TextureCache::new(&engine);
        let icons = Icons::build(&mut gui);
        // Placeholder color-picker gradients (regenerated when it opens).
        let picker_sv =
            gui.register_image_rgba(&color::sv_square(0.0, SV, 0.0, 0.0), SV as u32, SV as u32);
        let picker_hue =
            gui.register_image_rgba(&color::hue_bar(HUE_W, SV, 0.0), HUE_W as u32, SV as u32);
        let picker_alpha = gui.register_image_rgba(
            &color::alpha_bar([0.0, 0.0, 0.0], SV, ALPHA_H, 1.0),
            SV as u32,
            ALPHA_H as u32,
        );

        let project_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("demo_project");
        let project = project::open_or_create(&project_dir)?;
        // Decode the demo sprite once for its natural pixel size (the default
        // size for newly spawned sprites); its pixels load through the cache.
        let sprite_bytes =
            std::fs::read(project_dir.join("assets/sprite.png")).context("read demo sprite")?;
        let (w, h) = image::load_from_memory(&sprite_bytes)
            .context("decode demo sprite")?
            .to_rgba8()
            .dimensions();

        let (scene_target, viewport_handle) =
            build_viewport_target(&mut gui, &engine, (size.width, size.height));
        let panel_sizes = PanelSizes::default();
        let (ui, rows) = build_editor_ui(
            &project.scene,
            None,
            viewport_handle,
            &project_dir,
            panel_sizes,
            None,
            GizmoMode::default(),
            Some(&icons),
        );

        Ok(Self {
            window,
            gui,
            engine,
            textures,
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
            context_menu: None,
            gizmo_mode: GizmoMode::default(),
            clipboard: arboard::Clipboard::new()
                .inspect_err(|err| tracing::warn!("no clipboard available: {err}"))
                .ok(),
            scrub: None,
            icons,
            picker: None,
            picker_sv,
            picker_hue,
            picker_alpha,
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
        if self.scrub.is_some() {
            self.apply_scrub();
        }
        if self.picker.is_some_and(|p| p.drag.is_some()) {
            self.apply_picker_drag();
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
            && let Some(hit) = viewport::hit_gizmo(&g, world, self.gizmo_mode)
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

        let local = self.cursor - rect.pos;
        let camera = self.camera.camera(rect.size);
        // Pick against the same per-texture runs the scene renders with, so
        // alpha-discard uses each sprite's real texture; the returned index is
        // into the flattened draw order, which matches `draws`. (Re-borrow the
        // scene here so the `scene` binding above is free for `texture_runs`.)
        let draws = self.project.scene.sprite_draws();
        let runs = self.texture_runs(&draws);
        let run_refs: Vec<(&Texture, &[Sprite])> = runs
            .iter()
            .map(|(path, sprites)| (self.textures.get(path), sprites.as_slice()))
            .collect();
        let picked = self.engine.pick_runs(
            (self.scene_target.width, self.scene_target.height),
            &camera,
            &run_refs,
            (local.x.max(0.0) as u32, local.y.max(0.0) as u32),
        );
        match picked {
            Ok(Some(index)) => {
                let node = draws[index].node;
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

    /// Set the active gizmo mode (from a toolbar click or a shortcut).
    fn set_gizmo_mode(&mut self, mode: GizmoMode) {
        if self.gizmo_mode != mode {
            self.gizmo_mode = mode;
            self.dirty = true;
        }
    }

    /// The gizmo-mode shortcuts Q/W/E/R (select/move/rotate/scale, Godot's
    /// layout), consumed only when no modifier is held and no text field is
    /// focused - so typing into a field is never hijacked. Returns whether the
    /// key was handled.
    fn handle_mode_key(&mut self, event: &winit::event::KeyEvent) -> bool {
        if self.modifiers.control_key() || self.modifiers.alt_key() || self.ui.focused().is_some() {
            return false;
        }
        let WinitKey::Character(c) = &event.logical_key else {
            return false;
        };
        let mode = match c.as_str() {
            "q" | "Q" => GizmoMode::Select,
            "w" | "W" => GizmoMode::Move,
            "e" | "E" => GizmoMode::Rotate,
            "r" | "R" => GizmoMode::Scale,
            _ => return false,
        };
        self.set_gizmo_mode(mode);
        true
    }

    /// Right-click: open a context menu. Over a scene-tree row it offers node
    /// actions (and selects that node); over the viewport it offers "Add
    /// Sprite Here" at the cursor. Anywhere else it just closes any open menu.
    fn open_context_menu(&mut self) {
        let hit = self.ui.hit_test(self.cursor);
        if let Some(&(_, node)) =
            hit.and_then(|id| self.rows.tree_rows.iter().find(|(w, _)| *w == id))
        {
            self.selected = Some(node);
            let items = vec![
                ("Duplicate".to_string(), MenuAction::DuplicateNode(node)),
                ("Delete".to_string(), MenuAction::DeleteNode(node)),
            ];
            self.context_menu = Some(ContextMenu {
                anchor: self.cursor,
                items,
            });
            self.dirty = true;
        } else if hit == self.rows.viewport
            && let Some(world) = self.cursor_world()
        {
            self.context_menu = Some(ContextMenu {
                anchor: self.cursor,
                items: vec![(
                    "Add Sprite Here".to_string(),
                    MenuAction::AddSpriteAt(world),
                )],
            });
            self.dirty = true;
        } else {
            self.close_menu();
        }
    }

    /// Close an open menu if the click landed outside it. Returns whether it
    /// consumed the press (a menu was open and the click was outside it).
    fn close_menu_if_outside(&mut self) -> bool {
        let Some(popup) = self.rows.menu_popup else {
            return false;
        };
        let inside = self
            .ui
            .hit_test(self.cursor)
            .is_some_and(|id| self.ui.is_within(id, popup));
        if !inside {
            self.close_menu();
            return true;
        }
        false
    }

    fn close_menu(&mut self) {
        if self.context_menu.take().is_some() {
            self.dirty = true;
        }
    }

    /// Perform a context-menu action (a menu item was clicked), then close it.
    fn run_menu_action(&mut self, action: MenuAction) {
        match action {
            MenuAction::DeleteNode(node) => {
                if node != self.project.scene.root() {
                    self.history.remove_node(&mut self.project.scene, node);
                    if self.selected == Some(node) {
                        self.selected = None;
                    }
                }
            }
            MenuAction::DuplicateNode(node) => {
                if let Some(parent) = self.project.scene.parent(node) {
                    let mut copy = self.project.scene.node(node).clone();
                    copy.name = format!("{} copy", copy.name);
                    let new = self.history.add_node(&mut self.project.scene, parent, copy);
                    self.selected = Some(new);
                }
            }
            MenuAction::AddSpriteAt(world) => {
                let node = actions::spawn_sprite(
                    &mut self.project.scene,
                    &mut self.history,
                    world,
                    "assets/sprite.png",
                    self.texture_size,
                );
                self.selected = Some(node);
            }
        }
        self.context_menu = None;
        self.dirty = true;
    }

    /// Coalesce `draws` (already in paint order) into runs of consecutive
    /// same-texture sprites, loading each texture into the cache. Returns the
    /// owned runs; the caller borrows the cache to turn them into the
    /// `(&Texture, &[Sprite])` list photon's run methods take.
    fn texture_runs(&mut self, draws: &[helios::SpriteDraw]) -> Vec<(String, Vec<Sprite>)> {
        for draw in draws {
            self.textures
                .ensure(&self.engine, &self.project_dir, &draw.texture);
        }
        textures::coalesce_runs(draws)
    }

    /// Begin a drag-scrub if the press landed on an inspector axis label.
    /// Returns whether it did (so the caller can consume the press).
    fn begin_scrub(&mut self) -> bool {
        let hit = self.ui.hit_test(self.cursor);
        let Some(&(_, field, axis)) =
            hit.and_then(|id| self.rows.scrub_labels.iter().find(|(w, ..)| *w == id))
        else {
            return false;
        };
        self.scrub = Some(ScrubDrag {
            field,
            axis,
            original: field_vec2(&self.project.scene, field),
            start_x: self.cursor.x,
        });
        true
    }

    /// The scrub sensitivity (value change per horizontal pixel) for a field:
    /// scale is fine-grained, positions and sizes move a pixel per pixel.
    fn scrub_step(field: FieldRef) -> f32 {
        match field {
            FieldRef::Scale(_) => 0.01,
            _ => 1.0,
        }
    }

    /// Apply the live scrub value for the current cursor (called on move):
    /// the pressed axis component moves by the horizontal drag since the press.
    fn apply_scrub(&mut self) {
        let Some(scrub) = self.scrub else {
            return;
        };
        let delta = (self.cursor.x - scrub.start_x) * Self::scrub_step(scrub.field);
        let base = crate::ui::axis_of(scrub.original, scrub.axis);
        let value = with_axis(scrub.original, scrub.axis, base + delta);
        set_field_vec2(&mut self.project.scene, scrub.field, value);
        self.dirty = true;
    }

    /// Finish a scrub: rewind to the press-time value and commit the final one
    /// through the history, so the whole drag is ONE undo step.
    fn end_scrub(&mut self) {
        let Some(scrub) = self.scrub.take() else {
            return;
        };
        let current = field_vec2(&self.project.scene, scrub.field);
        if current != scrub.original {
            set_field_vec2(&mut self.project.scene, scrub.field, scrub.original);
            self.commit_field_vec2(scrub.field, current);
            self.dirty = true;
        }
    }

    /// Commit a Vec2 field value through the undo history.
    fn commit_field_vec2(&mut self, field: FieldRef, v: Vec2) {
        let scene = &mut self.project.scene;
        match field {
            FieldRef::Position(n) => {
                let t = Transform {
                    translation: v,
                    ..scene.node(n).transform
                };
                self.history.set_transform(scene, n, t);
            }
            FieldRef::Scale(n) => {
                let t = Transform {
                    scale: v,
                    ..scene.node(n).transform
                };
                self.history.set_transform(scene, n, t);
            }
            FieldRef::Component { node, index, field } => {
                self.history
                    .set_field(scene, node, index, field, Value::Vec2(v));
            }
        }
    }

    /// Open the color picker for `target`, seeded with its current color.
    fn open_color_picker(&mut self, target: ColorTarget) {
        let original = field_color(&self.project.scene, target);
        self.picker = Some(ColorPicker {
            target,
            original,
            hsva: Hsva::from_rgba(original, 0.0),
            anchor: self.cursor,
            drag: None,
        });
        self.refresh_picker_images();
        self.dirty = true;
    }

    /// Close the picker, committing its final color as one undo step (rewind to
    /// the open-time color, then set the final through the history).
    fn close_color_picker(&mut self) {
        let Some(picker) = self.picker.take() else {
            return;
        };
        let final_rgba = picker.hsva.to_rgba();
        if final_rgba != picker.original {
            set_field_color(&mut self.project.scene, picker.target, picker.original);
            self.history.set_field(
                &mut self.project.scene,
                picker.target.node,
                picker.target.index,
                picker.target.field,
                Value::Color(final_rgba),
            );
        }
        self.dirty = true;
    }

    /// Regenerate the picker's three gradient images from its current HSVA and
    /// upload them in place (their handles stay valid across shell rebuilds).
    fn refresh_picker_images(&mut self) {
        let Some(picker) = self.picker else {
            return;
        };
        let hsva = picker.hsva;
        let [r, g, b, _] = hsva.to_rgba();
        self.gui.update_image_rgba(
            self.picker_sv,
            &color::sv_square(hsva.h, SV, hsva.s, hsva.v),
            SV as u32,
            SV as u32,
        );
        self.gui.update_image_rgba(
            self.picker_hue,
            &color::hue_bar(HUE_W, SV, hsva.h),
            HUE_W as u32,
            SV as u32,
        );
        self.gui.update_image_rgba(
            self.picker_alpha,
            &color::alpha_bar([r, g, b], SV, ALPHA_H, hsva.a),
            SV as u32,
            ALPHA_H as u32,
        );
    }

    /// A left press with the picker open: dismiss it (committing) if the press
    /// is outside its popup, else start a gradient drag if on one. Returns
    /// whether it consumed the press.
    fn handle_picker_press(&mut self) -> bool {
        if self.picker.is_none() {
            return false;
        }
        let hit = self.ui.hit_test(self.cursor);
        let inside = self
            .rows
            .picker_popup
            .is_some_and(|p| hit.is_some_and(|h| self.ui.is_within(h, p)));
        if !inside {
            self.close_color_picker();
            return true;
        }
        let drag = if hit == self.rows.picker_sv {
            Some(PickerDrag::Sv)
        } else if hit == self.rows.picker_hue {
            Some(PickerDrag::Hue)
        } else if hit == self.rows.picker_alpha {
            Some(PickerDrag::Alpha)
        } else {
            None
        };
        if let Some(drag) = drag {
            if let Some(picker) = self.picker.as_mut() {
                picker.drag = Some(drag);
            }
            self.apply_picker_drag();
        }
        true
    }

    /// Apply the current cursor to the active picker gradient drag: read the
    /// value out of the region's rect, update the working HSVA, and push it to
    /// the scene live (committed as one step when the picker closes).
    fn apply_picker_drag(&mut self) {
        let Some(mut picker) = self.picker else {
            return;
        };
        let Some(drag) = picker.drag else {
            return;
        };
        let region = match drag {
            PickerDrag::Sv => self.rows.picker_sv,
            PickerDrag::Hue => self.rows.picker_hue,
            PickerDrag::Alpha => self.rows.picker_alpha,
        };
        let Some(rect) = region.and_then(|id| self.ui.rect(id)) else {
            return;
        };
        let fx = ((self.cursor.x - rect.pos.x) / rect.size.x).clamp(0.0, 1.0);
        let fy = ((self.cursor.y - rect.pos.y) / rect.size.y).clamp(0.0, 1.0);
        match drag {
            PickerDrag::Sv => {
                picker.hsva.s = fx;
                picker.hsva.v = 1.0 - fy;
            }
            PickerDrag::Hue => picker.hsva.h = fy,
            PickerDrag::Alpha => picker.hsva.a = fx,
        }
        self.picker = Some(picker);
        set_field_color(
            &mut self.project.scene,
            picker.target,
            picker.hsva.to_rgba(),
        );
        self.refresh_picker_images();
        self.dirty = true;
    }

    /// End a picker gradient drag (the picker stays open).
    fn end_picker_drag(&mut self) {
        if let Some(picker) = self.picker.as_mut() {
            picker.drag = None;
        }
    }

    /// Commit the picker's hex input into its working color.
    fn commit_picker_hex(&mut self, id: aurora::WidgetId) {
        if self.rows.picker_hex != Some(id) {
            return;
        }
        let WidgetKind::TextInput(text) = self.ui.kind(id) else {
            return;
        };
        let Some(rgba) = color::from_hex(&text.clone()) else {
            return;
        };
        if let Some(picker) = self.picker.as_mut() {
            picker.hsva = Hsva::from_rgba(rgba, picker.hsva.h);
        }
        set_field_color(&mut self.project.scene, self.picker.unwrap().target, rgba);
        self.refresh_picker_images();
        self.dirty = true;
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

    /// Keyboard shortcuts. ctrl+s saves (always). With a text field focused,
    /// ctrl+a/c/x/v are text clipboard ops (so typing is never hijacked and
    /// undo does not steal ctrl+z). With no field focused, ctrl+z /
    /// ctrl+shift+z / ctrl+y undo and redo. Returns whether handled.
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
            return self.clipboard_shortcut(c);
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

    /// ctrl+a/c/x/v on the focused text field, backed by the OS clipboard.
    /// Returns whether the key was a (handled) clipboard shortcut.
    fn clipboard_shortcut(&mut self, c: &str) -> bool {
        match c {
            "a" | "A" => self.ui.select_all(),
            "c" | "C" => {
                if let Some(text) = self.ui.selected_text() {
                    self.set_clipboard(&text);
                }
            }
            "x" | "X" => {
                if let Some(text) = self.ui.selected_text() {
                    self.set_clipboard(&text);
                    self.ui.delete_selection();
                }
            }
            "v" | "V" => {
                if let Some(text) = self.get_clipboard() {
                    self.ui.insert_str(&text);
                }
            }
            _ => return false,
        }
        true
    }

    fn set_clipboard(&mut self, text: &str) {
        if let Some(cb) = self.clipboard.as_mut()
            && let Err(err) = cb.set_text(text.to_string())
        {
            tracing::warn!("clipboard write failed: {err}");
        }
    }

    fn get_clipboard(&mut self) -> Option<String> {
        self.clipboard.as_mut()?.get_text().ok()
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
                self.context_menu.as_ref(),
                self.gizmo_mode,
                Some(&self.icons),
            );
            self.ui = ui;
            self.rows = rows;
            // The color picker floats on the popup layer, added after the shell
            // so it draws on top; its gradient images are kept up to date
            // separately (refresh_picker_images).
            if let Some(picker) = self.picker {
                let view = ColorPickerView {
                    anchor: picker.anchor,
                    rgba: picker.hsva.to_rgba(),
                    sv: self.picker_sv,
                    hue: self.picker_hue,
                    alpha: self.picker_alpha,
                };
                build_color_picker(&mut self.ui, &view, &mut self.rows);
            }
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
        // Draw the scene as per-texture runs, so each sprite shows the texture
        // it names (consecutive same-texture sprites batch, order preserved).
        let draws = self.project.scene.sprite_draws();
        let runs = self.texture_runs(&draws);
        let run_refs: Vec<(&Texture, &[Sprite])> = runs
            .iter()
            .map(|(path, sprites)| (self.textures.get(path), sprites.as_slice()))
            .collect();
        self.engine
            .render_runs_to_target(&self.scene_target, clear, &camera, &run_refs);

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
            overlay.extend(viewport::gizmo_sprites(
                &g,
                self.camera.zoom,
                self.gizmo_mode,
            ));
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
                AuroraEvent::Clicked(id)
                    if self.rows.mode_buttons.iter().any(|(w, _)| *w == id) =>
                {
                    let mode = self
                        .rows
                        .mode_buttons
                        .iter()
                        .find(|(w, _)| *w == id)
                        .map(|(_, m)| *m)
                        .expect("guarded by the match arm");
                    self.set_gizmo_mode(mode);
                }
                AuroraEvent::Clicked(id) if self.rows.menu_items.iter().any(|(w, _)| *w == id) => {
                    let action = self
                        .rows
                        .menu_items
                        .iter()
                        .find(|(w, _)| *w == id)
                        .map(|(_, a)| *a)
                        .expect("guarded by the match arm");
                    self.run_menu_action(action);
                }
                AuroraEvent::Clicked(id)
                    if self.rows.color_swatches.iter().any(|(w, _)| *w == id) =>
                {
                    let target = self
                        .rows
                        .color_swatches
                        .iter()
                        .find(|(w, _)| *w == id)
                        .map(|(_, t)| *t)
                        .expect("guarded by the match arm");
                    self.open_color_picker(target);
                }
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
                    self.commit_vec_input(id);
                    self.commit_picker_hex(id);
                }
                _ => {}
            }
        }
    }

    /// Commit a submitted Vec2 axis input: parse its text into the pressed
    /// component and commit the whole Vec2 through the history. A no-op for
    /// other widgets or unparseable text.
    fn commit_vec_input(&mut self, id: aurora::WidgetId) {
        let Some(&(_, field, axis)) = self.rows.vec_inputs.iter().find(|(w, ..)| *w == id) else {
            return;
        };
        let WidgetKind::TextInput(text) = self.ui.kind(id) else {
            return;
        };
        let Ok(parsed) = text.trim().parse::<f32>() else {
            return;
        };
        let current = field_vec2(&self.project.scene, field);
        let updated = with_axis(current, axis, parsed);
        if updated != current {
            // No shell rebuild: the field already shows the typed value and the
            // viewport re-renders from the scene each frame. Rebuilding here
            // would drop focus - which breaks tabbing between fields, since Tab
            // commits the field it leaves.
            self.commit_field_vec2(field, updated);
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
            // No rebuild - see commit_vec_input's note on keeping focus.
            self.history
                .set_transform(&mut self.project.scene, node, new);
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
        // Unparseable text is left in place (parse_value returns None); the
        // user's typing stays visible until they fix it or the shell rebuilds
        // for another reason (a selection change or undo re-reads the field).
        if let Some(new) = parse_value(&text, &old)
            && new != old
        {
            // No rebuild - see commit_vec_input's note on keeping focus.
            self.history
                .set_field(&mut self.project.scene, node, component, field, new);
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
        WinitKey::Named(NamedKey::Tab) => Some(Key::Tab),
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
