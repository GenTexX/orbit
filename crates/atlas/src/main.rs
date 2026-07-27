//! atlas - the Editor application: Aurora-based UI shell, scene editing, inspector, code editor, embedded Play.
//!
//! Milestone 3 step 6: the editor looks like an editor - a docked, resizable
//! scene-tree, viewport, inspector, and file explorer, with selection shared
//! across panels and inspector edits committed through the undo history.

mod actions;
mod color;
mod console;
mod dock;
mod editor_state;
mod explorer;
mod file_ops;
mod icons;
mod project;
mod settings;
mod textures;
mod thumbnails;
mod ui;
mod viewport;

use std::collections::{HashMap, HashSet};
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
use crate::dock::{DockNode, DropZone, Pane};
use crate::icons::Icons;
use crate::textures::TextureCache;
use crate::ui::{
    Axis, ColorPickerView, ColorTarget, ContextMenu, DropSpot, EditorRows, EditorTheme, FieldRef,
    InspectorSection, InspectorView, MenuAction, TreeView, axis_of, build_color_picker,
    build_editor_ui, capture_dock_scrolls, capture_dock_sizes, field_color, field_vec2, grid_cols,
    parse_value, resolve_drop, set_field_color, set_field_vec2, value_to_text, visible_nodes,
    with_axis,
};
use crate::viewport::{Drag, EditorCamera, GizmoHit, GizmoMode};

/// The color picker's gradient image sizes (px): the SV square, the hue bar
/// width, and the alpha bar height. Must match the widget sizes in
/// `build_color_picker` for a crisp 1:1 draw.
const SV: usize = 150;
const HUE_W: usize = 18;
const ALPHA_H: usize = 16;

/// Two tree-row clicks within this window count as a double-click (start a
/// rename), matching a typical desktop double-click speed.
const DOUBLE_CLICK: std::time::Duration = std::time::Duration::from_millis(400);

/// The editor's base window title; prefixed with "* " when there are unsaved
/// changes.
const WINDOW_TITLE: &str = "Orbit - Atlas";

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

/// The scene-tree selection: the nodes picked, in the order they were added,
/// plus the shift-range anchor. The last node added is the "primary" - the one
/// the inspector shows and the gizmo edits - so a plain click, which selects a
/// single node, makes that node primary.
#[derive(Debug, Clone, Default)]
struct Selection {
    nodes: Vec<NodeId>,
    /// The pivot a shift-click ranges from (the last plain/ctrl click).
    anchor: Option<NodeId>,
}

impl Selection {
    /// The node the inspector and gizmo act on: the most recently added.
    fn primary(&self) -> Option<NodeId> {
        self.nodes.last().copied()
    }

    fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    fn contains(&self, node: NodeId) -> bool {
        self.nodes.contains(&node)
    }

    fn as_set(&self) -> HashSet<NodeId> {
        self.nodes.iter().copied().collect()
    }

    /// Select exactly `node` (a plain click), making it the primary and the
    /// range anchor.
    fn select_one(&mut self, node: NodeId) {
        self.nodes.clear();
        self.nodes.push(node);
        self.anchor = Some(node);
    }

    /// Toggle `node` in the selection (a ctrl-click), moving the anchor to it.
    fn toggle(&mut self, node: NodeId) {
        if let Some(pos) = self.nodes.iter().position(|&n| n == node) {
            self.nodes.remove(pos);
        } else {
            self.nodes.push(node);
        }
        self.anchor = Some(node);
    }

    /// Replace the selection with `range` (a shift-click), keeping the anchor
    /// and making the clicked node - passed last in `range` - the primary.
    fn select_range(&mut self, range: Vec<NodeId>) {
        self.nodes = range;
    }

    fn clear(&mut self) {
        self.nodes.clear();
        self.anchor = None;
    }

    /// Drop `node` from the selection (e.g. after it is deleted).
    fn remove(&mut self, node: NodeId) {
        self.nodes.retain(|&n| n != node);
        if self.anchor == Some(node) {
            self.anchor = None;
        }
    }
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
    let logs = init_logging();
    tracing::info!("atlas starting");

    // Kept alive for the whole run; dropping it would stop the profiler server.
    let _profiler = init_profiling();

    let event_loop = EventLoop::new()?;
    // A live viewport redraws continuously, like the sandbox's game loop.
    event_loop.set_control_flow(ControlFlow::Poll);
    event_loop.run_app(&mut App { state: None, logs })?;
    Ok(())
}

fn init_logging() -> console::LogBuffer {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::{EnvFilter, fmt};
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,wgpu_core=warn,wgpu_hal=warn,naga=warn"));
    let buf = console::buffer();
    // The env filter gates both sinks; the terminal keeps its usual formatting,
    // and the Console pane mirrors the same events (see the console module).
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer())
        .with(console::ConsoleLayer::new(buf.clone()))
        .init();
    buf
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

struct App {
    state: Option<State>,
    /// The shared log ring the console pane shows (also written by the tracing
    /// layer); handed to `State` when the window is created.
    logs: console::LogBuffer,
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
    selection: Selection,
    /// The node being inline-renamed in the tree, if any.
    renaming: Option<NodeId>,
    /// Set when a rename field was just created, so the next rebuilt shell
    /// focuses it (and selects its text) once.
    focus_rename: bool,
    /// The scene-tree search text (read back from the search box each frame);
    /// filters which nodes the tree shows.
    tree_filter: String,
    /// Set when the search text changed, so the rebuilt shell re-focuses the
    /// search box at its preserved caret (it refilters, and so rebuilds, on
    /// every keystroke).
    refocus_filter: bool,
    /// The node clipboard: subtrees captured by copy/cut, instantiated by
    /// paste. Independent of the OS text clipboard.
    node_clip: Vec<actions::NodeSubtree>,
    /// The last tree-row click (widget and when), to detect a double-click as
    /// two clicks on the same row in quick succession (starts a rename).
    last_click: Option<(aurora::WidgetId, std::time::Instant)>,
    scene_target: SceneTarget,
    viewport_handle: ImageHandle,
    ui: aurora::Ui,
    rows: EditorRows,
    /// The dockable-panel layout (a tree of splits and tab groups): which panes
    /// go where, their tabs, and the split sizes - captured from the live `Ui`
    /// before each rebuild so splitter drags and rearrangements persist.
    dock: DockNode,
    /// Per-pane scroll offsets, captured before each rebuild so scroll survives
    /// like the dock sizes do (widget-tree state dies with the tree otherwise).
    pane_scrolls: HashMap<crate::dock::Pane, f32>,
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
    /// The demo texture's natural pixel size (spawned sprites default to it).
    texture_size: Vec2,
    /// Frame counting for the once-a-second fps log.
    frames: u32,
    fps_since: std::time::Instant,
    /// The fps figure currently shown in the status bar.
    last_fps: f32,
    /// Per-frame profiling (set by the ORBIT_FRAME_LOG env var): logs a timing
    /// breakdown for any frame slower than a threshold. `pointer_events` and
    /// `pointer_time` accumulate the cost of the pointer-move events handled
    /// since the last frame (they run before draw, so draw timing alone misses
    /// them - the suspect during a drag's event flood).
    frame_log: bool,
    pointer_events: u32,
    pointer_time: std::time::Duration,
    /// The cursor icon currently applied to the window (set only on change).
    applied_cursor: aurora::CursorHint,
    /// The history revision at the last save or load; the project has unsaved
    /// changes when it differs from the live `history.revision()`.
    saved_revision: u64,
    /// The window title currently applied (set only on change, like the cursor).
    applied_title: String,
    /// The shared log ring shown in the Console pane (written by the tracing
    /// layer). `last_log_gen` is the ring's generation at the last rebuild and
    /// `last_log_poll` throttles the check for new lines; `console_follow` keeps
    /// the pane pinned to the newest line while the user has not scrolled up.
    logs: console::LogBuffer,
    last_log_gen: u64,
    last_log_poll: std::time::Instant,
    console_follow: bool,
    /// Set when the per-project view state (dock, camera, tool, collapse) changed
    /// since the last editor-state save; a debounced save writes it out. Kept
    /// separate from `dirty` (the UI-rebuild flag). `editor_state_saved` is when
    /// it was last written, for the debounce.
    view_dirty: bool,
    editor_state_saved: std::time::Instant,
    /// The open right-click context menu, if any (Aurora popup layer).
    context_menu: Option<ContextMenu>,
    /// The active transform tool (select/move/rotate/scale), chosen from the
    /// toolbar; filters which gizmo handles show and are hittable.
    gizmo_mode: GizmoMode,
    /// Viewport view toggles (from the toolbar; persisted in global settings).
    show_grid: bool,
    show_axes: bool,
    /// Transform snapping (on/off + increments); persisted in global settings.
    /// Ctrl held during a drag inverts the on/off state for that drag.
    snap: settings::SnapSettings,
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
    /// Collapsed scene-tree nodes (editor state, kept across shell rebuilds).
    tree_collapsed: HashSet<NodeId>,
    /// Collapsed inspector sections per node (editor state, kept across shell
    /// rebuilds).
    inspector_collapsed: HashSet<(NodeId, InspectorSection)>,
    /// The editor's color theme, loaded from the user's settings file and
    /// hot-reloaded when it changes.
    theme: EditorTheme,
    /// The settings file's mtime when the theme was last loaded, and the last
    /// time it was polled - the theme hot-reloads when the file changes.
    settings_mtime: Option<std::time::SystemTime>,
    settings_poll: std::time::Instant,
    /// An in-progress reparent drag: the row being dragged and the current
    /// drop target under the cursor.
    reparent: Option<ReparentDrag>,
    /// The file explorer's model and view state (scanned project tree, expanded
    /// folders, selected folder/file, view mode).
    explorer: explorer::FileExplorer,
    /// Decoded image thumbnails for the explorer's previews, keyed by path.
    thumbnails: thumbnails::Thumbnails,
    /// The explorer grid's column count, recomputed from the contents pane's
    /// laid-out width each frame so the preview grid reflows with the pane.
    explorer_cols: usize,
    /// Whether the file explorer is the active pane, so the shared keyboard
    /// shortcuts (copy/cut/paste/select-all/delete/rename) act on files rather
    /// than on scene nodes. Set by clicking in the explorer, cleared by clicking
    /// the scene tree or the viewport.
    explorer_focused: bool,
    /// The explorer's file clipboard: paths copied or cut, and whether a paste
    /// should move them (a cut) rather than copy.
    file_clip: Vec<PathBuf>,
    file_clip_cut: bool,
    /// Set when a file-rename field was just created, so the next rebuild focuses
    /// it once (mirrors `focus_rename` for scene nodes).
    focus_file_rename: bool,
}

/// A tree row being dragged to reparent it; `target` is the current valid drop
/// spot under the cursor (into a node, or before/after it for reordering).
#[derive(Debug, Clone, Copy)]
struct ReparentDrag {
    source: NodeId,
    target: Option<DropSpot>,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        match State::new(event_loop, self.logs.clone()) {
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
                    // Keyboard focus follows the press: the shared shortcuts
                    // (delete/copy/rename) act on files when the press lands in
                    // the explorer pane, on the scene otherwise.
                    state.explorer_focused =
                        state.hit_in_explorer_pane(state.ui.hit_test(state.cursor));
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
                    // A press on a tree row arms a reparent drag (non-consuming:
                    // a plain click still selects via the row's Clicked event).
                    state.begin_reparent();
                    state.viewport_press();
                }
                (MouseButton::Left, ElementState::Released) => {
                    state.finish_reparent();
                    state.end_picker_drag();
                    state.end_scrub();
                    state.end_drag();
                    // Aurora's PointerReleased may emit Event::Dropped for a
                    // file-explorer drag onto the viewport (handled in react).
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
                if !state.handle_shortcut(&event)
                    && !state.handle_editor_key(&event)
                    && !state.handle_mode_key(&event)
                {
                    for ev in translate_key(&event, state.modifiers.control_key()) {
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

    /// On a clean exit, flush the per-project view state so the layout, camera,
    /// and collapse survive to the next launch (even if no debounced save fired).
    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(state) = self.state.as_mut() {
            state.save_editor_state();
        }
    }
}

impl State {
    /// Bootstrap the shared GPU context (ADR 0018), the two renderers it feeds,
    /// the demo project (opened or created), and the editor shell around it.
    fn new(event_loop: &ActiveEventLoop, logs: console::LogBuffer) -> Result<Self> {
        let window = Arc::new(
            event_loop
                .create_window(Window::default_attributes().with_title(WINDOW_TITLE))
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
        // Editor prefs come from the user's settings file (~/.config/orbit),
        // written with defaults on first run - see the `settings` module.
        let settings = settings::load();
        let theme = settings.theme;
        // Per-project view state (dock layout, camera, tool, scroll, collapsed
        // tree nodes) comes from <project>/.orbit/editor.ron if present; an
        // absent or invalid file just yields the defaults.
        let saved = editor_state::load(&project_dir);
        let dock = saved
            .as_ref()
            .map_or_else(DockNode::default_layout, |s| s.dock.clone());
        let camera = saved
            .as_ref()
            .map_or_else(EditorCamera::default, |s| s.camera.to_camera());
        let gizmo_mode = saved
            .as_ref()
            .map_or_else(GizmoMode::default, |s| s.gizmo_mode);
        let pane_scrolls: HashMap<dock::Pane, f32> = saved
            .as_ref()
            .map(|s| s.pane_scrolls.iter().copied().collect())
            .unwrap_or_default();
        let tree_collapsed = saved
            .as_ref()
            .map(|s| editor_state::decode_collapsed(&project.scene, &s.collapsed))
            .unwrap_or_default();
        // The file explorer scans the project directory; its expanded folders
        // and selected folder are restored from the same per-project state.
        let mut explorer = explorer::FileExplorer::new(&project_dir);
        if let Some(s) = &saved {
            explorer.restore(
                &s.explorer_expanded,
                s.explorer_selected.as_deref(),
                s.explorer_view,
            );
        }
        let thumbnails = thumbnails::Thumbnails::new();
        let explorer_cols = 3;
        let (ui, rows) = build_editor_ui(
            &project.scene,
            &HashSet::new(),
            None,
            viewport_handle,
            &explorer,
            &thumbnails,
            explorer_cols,
            &dock,
            &pane_scrolls,
            None,
            gizmo_mode,
            settings.show_grid,
            settings.show_axes,
            settings.snap.enabled,
            Some(&icons),
            &TreeView::default(),
            "",
            None,
            &InspectorView::default(),
            &[],
            true,
            &theme,
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
            selection: Selection::default(),
            renaming: None,
            focus_rename: false,
            tree_filter: String::new(),
            refocus_filter: false,
            node_clip: Vec::new(),
            last_click: None,
            scene_target,
            viewport_handle,
            ui,
            rows,
            dock,
            pane_scrolls,
            dirty: false,
            camera,
            drag: None,
            panning: false,
            cursor: Vec2::ZERO,
            modifiers: ModifiersState::default(),
            texture_size: Vec2::new(w as f32, h as f32),
            frames: 0,
            fps_since: std::time::Instant::now(),
            last_fps: 0.0,
            frame_log: std::env::var_os("ORBIT_FRAME_LOG").is_some(),
            pointer_events: 0,
            pointer_time: std::time::Duration::ZERO,
            applied_cursor: aurora::CursorHint::Default,
            saved_revision: 0,
            applied_title: WINDOW_TITLE.to_string(),
            logs,
            last_log_gen: 0,
            last_log_poll: std::time::Instant::now(),
            console_follow: true,
            view_dirty: false,
            editor_state_saved: std::time::Instant::now(),
            context_menu: None,
            gizmo_mode,
            show_grid: settings.show_grid,
            show_axes: settings.show_axes,
            snap: settings.snap,
            clipboard: arboard::Clipboard::new()
                .inspect_err(|err| tracing::warn!("no clipboard available: {err}"))
                .ok(),
            scrub: None,
            icons,
            picker: None,
            picker_sv,
            picker_hue,
            picker_alpha,
            tree_collapsed,
            inspector_collapsed: HashSet::new(),
            theme,
            settings_mtime: settings::modified(),
            settings_poll: std::time::Instant::now(),
            reparent: None,
            explorer,
            thumbnails,
            explorer_cols,
            explorer_focused: false,
            file_clip: Vec::new(),
            file_clip_cut: false,
            focus_file_rename: false,
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
        let timer = self.frame_log.then(std::time::Instant::now);
        let delta = p - self.cursor;
        self.cursor = p;
        self.ui.handle_input(InputEvent::PointerMoved(p));

        if self.panning {
            self.camera.pan_by_screen(delta);
            self.view_dirty = true;
        }
        if self.scrub.is_some() {
            self.apply_scrub();
        }
        if self.picker.is_some_and(|p| p.drag.is_some()) {
            self.apply_picker_drag();
        }
        if self.reparent.is_some() {
            self.update_reparent_target();
        }
        if let Some(drag) = self.drag
            && let Some(world) = self.cursor_world()
        {
            let (node, _) = drag.target();
            let mut new = drag.apply(&self.project.scene, world);
            // Snap the dragged property to its increment when snapping is on
            // (Ctrl held during the drag inverts that for this drag).
            if self.snap.enabled ^ self.modifiers.control_key() {
                new = viewport::snap_transform(
                    &drag,
                    new,
                    self.snap.move_step,
                    self.snap.rotate_step_deg.to_radians(),
                    self.snap.scale_step,
                );
            }
            self.project.scene.node_mut(node).transform = new;
            // The scene and gizmo re-render from the live transform every frame
            // anyway; update just the inspector's transform fields in place so
            // they read out live WITHOUT rebuilding the whole shell each move (a
            // full rebuild per pointer event dropped the drag to ~30 fps).
            self.sync_inspector_transform(node);
        }
        if let Some(timer) = timer {
            self.pointer_time += timer.elapsed();
            self.pointer_events += 1;
        }
    }

    /// Write `node`'s current transform into the inspector's position, rotation,
    /// and scale inputs in place (no shell rebuild) - used during a viewport drag
    /// so the readout tracks the drag without a per-move rebuild.
    fn sync_inspector_transform(&mut self, node: NodeId) {
        let scene = &self.project.scene;
        let mut updates: Vec<(aurora::WidgetId, String)> = self
            .rows
            .vec_inputs
            .iter()
            .filter(|&&(_, field, _)| {
                matches!(field, FieldRef::Position(n) | FieldRef::Scale(n) if n == node)
            })
            .map(|&(widget, field, axis)| {
                let value = axis_of(field_vec2(scene, field), axis);
                (widget, value_to_text(&Value::F32(value)))
            })
            .collect();
        let rotation = scene.node(node).transform.rotation.to_degrees();
        updates.extend(
            self.rows
                .transform_rows
                .iter()
                .filter(|&&(_, n, field)| n == node && field == "rotation")
                .map(|&(widget, ..)| (widget, value_to_text(&Value::F32(rotation)))),
        );
        for (widget, text) in updates {
            self.ui.set_text_input(widget, text);
        }
    }

    /// Update one Vec2 field component's inspector input in place (no rebuild) -
    /// used during an inspector label drag-scrub, the same way a viewport drag
    /// updates the transform inputs.
    fn sync_vec_input(&mut self, field: FieldRef, axis: Axis) {
        let value = axis_of(field_vec2(&self.project.scene, field), axis);
        let text = value_to_text(&Value::F32(value));
        let widget = self
            .rows
            .vec_inputs
            .iter()
            .find(|&&(_, f, a)| f == field && a == axis)
            .map(|&(w, ..)| w);
        if let Some(widget) = widget {
            self.ui.set_text_input(widget, text);
        }
    }

    /// A left press over the viewport: grab a gizmo handle of the current
    /// selection, else GPU-pick the sprite under the cursor (select and start
    /// moving it), else clear the selection.
    fn viewport_press(&mut self) {
        if self.ui.hit_test(self.cursor) != self.rows.viewport {
            return;
        }
        // Interacting with the viewport moves keyboard focus off the explorer.
        self.explorer_focused = false;
        let (Some(rect), Some(world)) = (self.viewport_rect(), self.cursor_world()) else {
            return;
        };
        let scene = &self.project.scene;

        // Gizmo handles first: they float outside the sprite, so they must win
        // over picking (which only sees sprite pixels).
        if let Some(sel) = self.selection.primary()
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
                let mut node = draws[index].node;
                // Alt-drag duplicates: leave the original in place and drag a
                // fresh copy (which starts exactly on top of it).
                if self.modifiers.alt_key()
                    && let Some(copy) =
                        actions::duplicate_node(&mut self.project.scene, &mut self.history, node)
                {
                    node = copy;
                }
                self.selection.select_one(node);
                self.drag = Some(Drag::Move {
                    node,
                    original: self.project.scene.node(node).transform,
                    grab_world: world,
                });
                self.dirty = true;
            }
            Ok(None) => {
                if !self.selection.is_empty() {
                    self.selection.clear();
                    self.dirty = true;
                }
            }
            Err(err) => tracing::error!("pick failed: {err}"),
        }
    }

    /// Hot-reload the theme when the user's settings file changes. Polls the
    /// file's mtime a few times a second (cheap) and rebuilds the shell if the
    /// theme changed, so hand-edits to `settings.ron` apply live.
    fn poll_settings(&mut self) {
        if self.settings_poll.elapsed() < std::time::Duration::from_millis(400) {
            return;
        }
        self.settings_poll = std::time::Instant::now();
        let mtime = settings::modified();
        if mtime == self.settings_mtime {
            return;
        }
        self.settings_mtime = mtime;
        if let Some(loaded) = settings::read()
            && loaded.theme != self.theme
        {
            self.theme = loaded.theme;
            self.dirty = true;
            tracing::info!("reloaded theme from settings");
        }
    }

    /// Set the active gizmo mode (from a toolbar click or a shortcut).
    fn set_gizmo_mode(&mut self, mode: GizmoMode) {
        if self.gizmo_mode != mode {
            self.gizmo_mode = mode;
            self.dirty = true;
            self.view_dirty = true;
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
        // A right-press also moves keyboard focus, like a left-press.
        self.explorer_focused = self.hit_in_explorer_pane(hit);
        if let Some(&(_, node)) =
            hit.and_then(|id| self.rows.tree_rows.iter().find(|(w, _)| *w == id))
        {
            // Right-clicking a node outside the selection selects just it;
            // right-clicking one already selected keeps the whole selection, so
            // Duplicate/Delete act on all of it.
            if !self.selection.contains(node) {
                self.selection.select_one(node);
            }
            let items = vec![
                ("Rename".to_string(), MenuAction::RenameNode(node)),
                ("Duplicate".to_string(), MenuAction::DuplicateSelection),
                ("Delete".to_string(), MenuAction::DeleteSelection),
            ];
            self.context_menu = Some(ContextMenu {
                anchor: self.cursor,
                items,
            });
            self.dirty = true;
        } else if let Some((path, is_dir)) = hit.and_then(|id| {
            self.rows
                .file_entries
                .iter()
                .find(|(w, ..)| *w == id)
                .map(|(_, p, d)| (p.clone(), *d))
        }) {
            self.open_file_entry_menu(&path, is_dir);
        } else if self.hit_in_explorer(hit) {
            self.open_explorer_background_menu();
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

    /// The context menu for a right-clicked contents entry: select it (if it was
    /// not part of the selection), then offer the file operations.
    fn open_file_entry_menu(&mut self, path: &std::path::Path, is_dir: bool) {
        self.explorer_focused = true;
        if !self.explorer.is_selected(path) {
            self.explorer.select_one(path);
        }
        let mut items = Vec::new();
        if is_dir {
            items.push((
                "Open".to_string(),
                MenuAction::OpenFolder(path.to_path_buf()),
            ));
        }
        items.push((
            "Rename".to_string(),
            MenuAction::RenameFile(path.to_path_buf()),
        ));
        items.push(("Duplicate".to_string(), MenuAction::DuplicateFiles));
        items.push(("Copy".to_string(), MenuAction::CopyFiles));
        items.push(("Cut".to_string(), MenuAction::CutFiles));
        if !self.file_clip.is_empty() {
            items.push(("Paste".to_string(), MenuAction::PasteFiles));
        }
        items.push(("Delete".to_string(), MenuAction::DeleteFiles));
        items.push(("New Folder".to_string(), MenuAction::NewFolder));
        self.context_menu = Some(ContextMenu {
            anchor: self.cursor,
            items,
        });
        self.dirty = true;
    }

    /// The context menu for empty explorer space: new folder, and paste when the
    /// file clipboard holds something.
    fn open_explorer_background_menu(&mut self) {
        self.explorer_focused = true;
        let mut items = vec![("New Folder".to_string(), MenuAction::NewFolder)];
        if !self.file_clip.is_empty() {
            items.push(("Paste".to_string(), MenuAction::PasteFiles));
        }
        self.context_menu = Some(ContextMenu {
            anchor: self.cursor,
            items,
        });
        self.dirty = true;
    }

    /// Whether `hit` lands inside the explorer's tree or contents area (used to
    /// offer the background menu on empty space there).
    fn hit_in_explorer(&self, hit: Option<aurora::WidgetId>) -> bool {
        let Some(hit) = hit else {
            return false;
        };
        let within = |anchor: Option<aurora::WidgetId>| {
            anchor.is_some_and(|a| a == hit || self.ui.is_within(hit, a))
        };
        within(self.rows.file_contents) || within(self.rows.file_tree_scroll)
    }

    /// Whether `hit` lands anywhere inside the whole explorer pane (bar, tree, or
    /// contents). This is the authoritative source for `explorer_focused`,
    /// recomputed on each press so the flag tracks visible focus instead of
    /// drifting - a stale flag would send Delete to the wrong place.
    fn hit_in_explorer_pane(&self, hit: Option<aurora::WidgetId>) -> bool {
        match (hit, self.rows.file_pane) {
            (Some(hit), Some(pane)) => hit == pane || self.ui.is_within(hit, pane),
            _ => false,
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
            // Delete/Duplicate act on the whole selection (open_context_menu
            // ensures the right-clicked node is part of it).
            MenuAction::DeleteSelection => self.delete_selection(),
            MenuAction::DuplicateSelection => self.duplicate_selection(),
            MenuAction::RenameNode(node) => self.start_rename(node),
            MenuAction::AddSpriteAt(world) => {
                let node = actions::spawn_sprite(
                    &mut self.project.scene,
                    &mut self.history,
                    world,
                    "assets/sprite.png",
                    self.texture_size,
                );
                self.selection.select_one(node);
            }
            MenuAction::OpenFolder(path) => self.select_explorer_dir(&path),
            MenuAction::RenameFile(path) => self.start_file_rename(&path),
            MenuAction::NewFolder => self.file_new_folder(),
            MenuAction::DuplicateFiles => self.file_duplicate(),
            MenuAction::DeleteFiles => self.file_delete(),
            MenuAction::CopyFiles => self.file_copy(false),
            MenuAction::CutFiles => self.file_copy(true),
            MenuAction::PasteFiles => self.file_paste(),
        }
        self.context_menu = None;
        self.dirty = true;
    }

    /// The selection in scene order (root first), skipping the scene root (which
    /// cannot be deleted, duplicated, or copied). The order matters so a
    /// multi-node op reads naturally and duplicates land in a sensible order.
    fn actionable_selection(&self) -> Vec<NodeId> {
        let root = self.project.scene.root();
        visible_nodes(&self.project.scene, &self.tree_view(), "")
            .into_iter()
            .filter(|&n| n != root && self.selection.contains(n))
            .collect()
    }

    /// Delete every selected node (each with its subtree), as one undo step.
    /// The scene root is never deleted. Clears the selection afterward.
    fn delete_selection(&mut self) {
        let targets = self.actionable_selection();
        if targets.is_empty() {
            return;
        }
        self.cancel_rename();
        self.history.begin_group();
        for node in targets {
            self.history.remove_node(&mut self.project.scene, node);
            self.selection.remove(node);
        }
        self.history.end_group(&mut self.project.scene);
        self.selection.clear();
        self.dirty = true;
    }

    /// Deep-duplicate every selected node beside its original, as one undo step,
    /// and select the new copies (the last becomes primary).
    fn duplicate_selection(&mut self) {
        let targets = self.actionable_selection();
        if targets.is_empty() {
            return;
        }
        self.cancel_rename();
        self.history.begin_group();
        let mut copies = Vec::new();
        for node in targets {
            if let Some(copy) =
                actions::duplicate_node(&mut self.project.scene, &mut self.history, node)
            {
                copies.push(copy);
            }
        }
        self.history.end_group(&mut self.project.scene);
        if !copies.is_empty() {
            self.selection.select_range(copies);
            self.dirty = true;
        }
    }

    /// Copy every selected subtree into the node clipboard (independent of the
    /// OS text clipboard). A no-op if nothing (other than the root) is selected.
    fn copy_selection(&mut self) {
        let targets = self.actionable_selection();
        if targets.is_empty() {
            return;
        }
        self.node_clip = targets
            .iter()
            .map(|&n| actions::capture_subtree(&self.project.scene, n))
            .collect();
    }

    /// Paste the node clipboard under the primary selection's parent (as a
    /// sibling after it), or under the scene root when nothing is selected.
    /// Selects the pasted roots. One undo step.
    fn paste_clipboard(&mut self) {
        if self.node_clip.is_empty() {
            return;
        }
        let scene = &self.project.scene;
        let (parent, mut index) = match self.selection.primary() {
            Some(node) => match scene.parent(node) {
                Some(parent) => {
                    let after = scene.children(parent).iter().position(|&c| c == node);
                    (
                        parent,
                        after.map_or(scene.children(parent).len(), |p| p + 1),
                    )
                }
                None => (node, scene.children(node).len()),
            },
            None => {
                let root = scene.root();
                (root, scene.children(root).len())
            }
        };
        self.cancel_rename();
        self.history.begin_group();
        let clip = std::mem::take(&mut self.node_clip);
        let mut pasted = Vec::new();
        for tree in &clip {
            let new = actions::paste_subtree(
                &mut self.project.scene,
                &mut self.history,
                tree,
                parent,
                index,
            );
            pasted.push(new);
            index += 1;
        }
        self.history.end_group(&mut self.project.scene);
        self.node_clip = clip;
        if !pasted.is_empty() {
            self.selection.select_range(pasted);
            self.dirty = true;
        }
    }

    /// Select every node in the scene (ctrl+a with no field focused).
    fn select_all_nodes(&mut self) {
        let all = visible_nodes(&self.project.scene, &self.tree_view(), "");
        if !all.is_empty() {
            self.selection.select_range(all);
            self.dirty = true;
        }
    }

    /// Start an inline rename of `node` (its tree row becomes an edit field,
    /// focused and selected on the next rebuilt shell). The scene root can be
    /// renamed like any node.
    fn start_rename(&mut self, node: NodeId) {
        self.selection.select_one(node);
        self.renaming = Some(node);
        self.focus_rename = true;
        self.dirty = true;
    }

    /// Commit the inline rename to `name` through the history (one undo step),
    /// then leave rename mode. Empty or whitespace-only names are rejected (the
    /// edit is cancelled, keeping the old name).
    fn commit_rename(&mut self, node: NodeId, name: String) {
        let name = name.trim().to_string();
        if !name.is_empty() {
            self.history.set_name(&mut self.project.scene, node, name);
        }
        self.renaming = None;
        self.focus_rename = false;
        self.dirty = true;
    }

    /// Leave rename mode without committing (Escape, or a delete/paste that
    /// supersedes it). A no-op when not renaming.
    fn cancel_rename(&mut self) {
        if self.renaming.take().is_some() {
            self.focus_rename = false;
            self.dirty = true;
        }
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
        // Update the scrubbed input in place rather than rebuilding every move
        // (the scene re-renders live regardless); see sync_inspector_transform.
        self.sync_vec_input(scrub.field, scrub.axis);
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

    /// The scene tree's editor state for a rebuild: the collapsed set plus the
    /// current reparent drop target (highlighted while dragging).
    fn tree_view(&self) -> TreeView {
        TreeView {
            collapsed: self.tree_collapsed.clone(),
            drop_target: self.reparent.and_then(|r| r.target),
        }
    }

    /// Collapse or expand a node's children in the tree.
    fn toggle_collapse(&mut self, node: NodeId) {
        if !self.tree_collapsed.remove(&node) {
            self.tree_collapsed.insert(node);
        }
        self.dirty = true;
        self.view_dirty = true;
    }

    /// React to a click on tree row `id` (its `node`): plain click selects just
    /// it, ctrl-click toggles it, shift-click ranges from the anchor; a second
    /// click on the same row in quick succession starts an inline rename.
    fn click_tree_row(&mut self, id: aurora::WidgetId, node: NodeId) {
        // The scene tree takes keyboard focus back from the explorer, so the
        // shared shortcuts (delete/copy/rename) act on nodes again.
        self.explorer_focused = false;
        let now = std::time::Instant::now();
        let double = self
            .last_click
            .is_some_and(|(w, t)| w == id && now.duration_since(t) < DOUBLE_CLICK);
        self.last_click = Some((id, now));
        if double {
            self.start_rename(node);
            return;
        }
        if self.modifiers.shift_key() {
            self.select_tree_range(node);
        } else if self.modifiers.control_key() {
            self.selection.toggle(node);
        } else {
            self.selection.select_one(node);
        }
        self.dirty = true;
    }

    /// Select the range of visible rows between the anchor and `node` (a
    /// shift-click), leaving the anchor fixed for a subsequent shift-click and
    /// making `node` the primary (so the inspector follows the click).
    fn select_tree_range(&mut self, node: NodeId) {
        let order = visible_nodes(&self.project.scene, &self.tree_view(), &self.tree_filter);
        let anchor = self.selection.anchor.unwrap_or(node);
        let (Some(a), Some(b)) = (
            order.iter().position(|&n| n == anchor),
            order.iter().position(|&n| n == node),
        ) else {
            self.selection.select_one(node);
            return;
        };
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        let mut range = order[lo..=hi].to_vec();
        // The clicked node goes last so it becomes primary.
        if range.last() != Some(&node) {
            range.reverse();
        }
        self.selection.select_range(range);
        self.selection.anchor = Some(anchor);
    }

    /// The inspector's editor state for a rebuild: the collapsed-sections set.
    fn inspector_view(&self) -> InspectorView {
        InspectorView {
            collapsed: self.inspector_collapsed.clone(),
        }
    }

    /// Collapse or expand an inspector section of `node`.
    fn toggle_section(&mut self, node: NodeId, section: InspectorSection) {
        if !self.inspector_collapsed.remove(&(node, section)) {
            self.inspector_collapsed.insert((node, section));
        }
        self.dirty = true;
    }

    /// The pane a dock tab (by widget) shows, if `id` is one.
    fn dock_tab_pane(&self, id: aurora::WidgetId) -> Option<Pane> {
        self.rows
            .dock_tabs
            .iter()
            .find(|(w, _)| *w == id)
            .map(|(_, p)| *p)
    }

    /// The dock group under the cursor: the pane it shows and its content rect
    /// (whose regions decide the drop zone). `None` if the cursor is over none.
    fn dock_group_at_cursor(&self) -> Option<(Pane, aurora::Rect)> {
        self.rows.dock_groups.iter().find_map(|&(content, pane)| {
            let rect = self.ui.rect(content)?;
            rect.contains(self.cursor).then_some((pane, rect))
        })
    }

    /// Which zone of a group's content `rect` the cursor is in: the central area
    /// merges as a tab, the four edge bands split off that side.
    fn dock_drop_zone(rect: aurora::Rect, cursor: Vec2) -> DropZone {
        let fx = ((cursor.x - rect.pos.x) / rect.size.x.max(1.0)).clamp(0.0, 1.0);
        let fy = ((cursor.y - rect.pos.y) / rect.size.y.max(1.0)).clamp(0.0, 1.0);
        let (left, right, top, bottom) = (fx, 1.0 - fx, fy, 1.0 - fy);
        let nearest = left.min(right).min(top).min(bottom);
        // Within ~30% of no edge -> the central tab zone.
        if nearest > 0.3 {
            DropZone::Tab
        } else if nearest == left {
            DropZone::Left
        } else if nearest == right {
            DropZone::Right
        } else if nearest == top {
            DropZone::Top
        } else {
            DropZone::Bottom
        }
    }

    /// Activate the tab showing `pane` (a tab click).
    fn activate_tab(&mut self, pane: Pane) {
        self.dock.activate(pane);
        self.dirty = true;
        self.view_dirty = true;
    }

    /// Finish a tab drag: dock `pane` into the group under the cursor at the
    /// hovered zone. A no-op if the cursor is over no group.
    fn drop_dock_tab(&mut self, pane: Pane) {
        let Some((target, rect)) = self.dock_group_at_cursor() else {
            return;
        };
        let zone = Self::dock_drop_zone(rect, self.cursor);
        self.dock.move_pane(pane, target, zone);
        self.dirty = true;
        self.view_dirty = true;
    }

    /// The scene-tree row node under the cursor, if any (a drop target). Reads
    /// the interactive widget under the cursor (the row button, not the label
    /// child a raw hit-test would return) with a fresh query, not the retained
    /// hover - so it stays correct on the frames a drag rebuilds the shell.
    fn tree_row_at_cursor(&self) -> Option<NodeId> {
        let row = self.ui.interactive_at(self.cursor)?;
        self.rows
            .tree_rows
            .iter()
            .find(|(w, _)| *w == row)
            .map(|(_, n)| *n)
    }

    /// A press on a tree row arms a reparent drag (a plain click still selects
    /// via the row's Clicked event; only a drag onto another row reparents).
    fn begin_reparent(&mut self) {
        if let Some(source) = self.tree_row_at_cursor() {
            self.reparent = Some(ReparentDrag {
                source,
                target: None,
            });
        }
    }

    /// The drop spot for the cursor: the top/bottom quarter of a row inserts
    /// before/after it (reordering siblings), the middle drops into it as a
    /// child. Snaps to the row nearest the cursor's y (not strictly the one hit),
    /// so the 1px gaps between rows - and the insertion line drawn over an edge -
    /// are never dead zones during a drag.
    fn drop_spot_at_cursor(&self) -> Option<DropSpot> {
        let cy = self.cursor.y;
        // Vertical distance from the cursor to a row's rect (0 when inside it).
        let dist = |r: &aurora::Rect| (r.pos.y - cy).max(cy - r.max().y).max(0.0);
        let (node, rect) = self
            .rows
            .tree_rows
            .iter()
            .filter_map(|&(w, n)| self.ui.rect(w).map(|r| (n, r)))
            .min_by(|(_, a), (_, b)| dist(a).total_cmp(&dist(b)))?;
        let rel = ((cy - rect.pos.y) / rect.size.y.max(1.0)).clamp(0.0, 1.0);
        Some(if rel < 0.28 {
            DropSpot::Before(node)
        } else if rel > 0.72 {
            DropSpot::After(node)
        } else {
            DropSpot::Into(node)
        })
    }

    /// Track the drop spot under the cursor, keeping only valid ones (a real
    /// reparent, no cycle). Drives the tree's highlight / insertion line.
    fn update_reparent_target(&mut self) {
        let Some(mut drag) = self.reparent else {
            return;
        };
        let target = self
            .drop_spot_at_cursor()
            .filter(|&s| resolve_drop(&self.project.scene, drag.source, s).is_some());
        if target != drag.target {
            drag.target = target;
            self.reparent = Some(drag);
            self.dirty = true;
        }
    }

    /// On release, apply the drop spot (reparent / reorder) through the history
    /// as one undo step.
    fn finish_reparent(&mut self) {
        let Some(drag) = self.reparent.take() else {
            return;
        };
        let Some(spot) = drag.target else {
            return;
        };
        if self.modifiers.alt_key() {
            // Alt-drag drops a duplicate at the target, leaving the original in
            // place. Duplicate first (the copy lands beside its original), then
            // move the copy to the drop spot - one undo step for the pair.
            self.history.begin_group();
            if let Some(copy) =
                actions::duplicate_node(&mut self.project.scene, &mut self.history, drag.source)
                && let Some((parent, index)) = resolve_drop(&self.project.scene, copy, spot)
            {
                self.history
                    .reparent(&mut self.project.scene, copy, parent, index);
                self.selection.select_one(copy);
            }
            self.history.end_group(&mut self.project.scene);
            self.dirty = true;
        } else if let Some((parent, index)) = resolve_drop(&self.project.scene, drag.source, spot) {
            self.history
                .reparent(&mut self.project.scene, drag.source, parent, index);
            self.selection.select_one(drag.source);
            self.dirty = true;
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
        self.view_dirty = true;
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
        // With a text field focused, ctrl+a/c/x/v edit its text (and only text).
        if self.ui.focused().is_some() {
            return self.clipboard_shortcut(c);
        }
        // When the file explorer is the active pane, copy/cut/paste/select-all
        // act on files rather than on scene nodes.
        if self.explorer_focused {
            if c.eq_ignore_ascii_case("c") {
                self.file_copy(false);
                return true;
            }
            if c.eq_ignore_ascii_case("x") {
                self.file_copy(true);
                return true;
            }
            if c.eq_ignore_ascii_case("v") {
                self.file_paste();
                return true;
            }
            if c.eq_ignore_ascii_case("a") {
                self.explorer_select_all();
                return true;
            }
        }
        // No field focused: ctrl combos act on the scene and the selection.
        if c.eq_ignore_ascii_case("z") {
            let changed = if self.modifiers.shift_key() {
                self.history.redo(&mut self.project.scene)
            } else {
                self.history.undo(&mut self.project.scene)
            };
            if changed {
                self.after_history_change();
            }
            return true;
        }
        if c.eq_ignore_ascii_case("y") {
            if self.history.redo(&mut self.project.scene) {
                self.after_history_change();
            }
            return true;
        }
        if c.eq_ignore_ascii_case("d") {
            self.duplicate_selection();
            return true;
        }
        if c.eq_ignore_ascii_case("c") {
            self.copy_selection();
            return true;
        }
        if c.eq_ignore_ascii_case("x") {
            self.copy_selection();
            self.delete_selection();
            return true;
        }
        if c.eq_ignore_ascii_case("v") {
            self.paste_clipboard();
            return true;
        }
        if c.eq_ignore_ascii_case("a") {
            self.select_all_nodes();
            return true;
        }
        false
    }

    /// Named keys that drive editor commands: Delete removes the selection, F2
    /// renames the primary, Escape cancels a rename or clears the selection.
    /// Returns whether the key was handled (so it is not also typed into a
    /// focused field). While a field is focused the keys keep their text-editing
    /// meaning - except Escape, which still cancels an in-progress rename.
    fn handle_editor_key(&mut self, event: &winit::event::KeyEvent) -> bool {
        let WinitKey::Named(named) = &event.logical_key else {
            return false;
        };
        match named {
            NamedKey::Escape => {
                if self.explorer.renaming().is_some() {
                    self.cancel_file_rename();
                    return true;
                }
                if self.renaming.is_some() {
                    self.cancel_rename();
                    return true;
                }
                if self.ui.focused().is_some() {
                    return false;
                }
                if self.explorer_focused && !self.explorer.selected().is_empty() {
                    self.explorer.clear_selection();
                    self.dirty = true;
                    return true;
                }
                if !self.selection.is_empty() {
                    self.selection.clear();
                    self.dirty = true;
                }
                true
            }
            NamedKey::Delete if self.ui.focused().is_none() => {
                if self.explorer_focused {
                    self.file_delete();
                } else {
                    self.delete_selection();
                }
                true
            }
            NamedKey::F2 if self.ui.focused().is_none() => {
                if self.explorer_focused {
                    if let Some(path) = self.explorer.primary().map(|p| p.to_path_buf()) {
                        self.start_file_rename(&path);
                    }
                } else if let Some(node) = self.selection.primary() {
                    self.start_rename(node);
                }
                true
            }
            _ => false,
        }
    }

    /// After an undo/redo: leave any rename, drop selected nodes the edit
    /// detached from the tree, and rebuild the shell.
    fn after_history_change(&mut self) {
        self.cancel_rename();
        let scene = &self.project.scene;
        let root = scene.root();
        // A detached node (undone add) has no parent; only the root legitimately
        // has none, so this keeps just the nodes still in the tree.
        self.selection
            .nodes
            .retain(|&n| n == root || scene.parent(n).is_some());
        if self
            .selection
            .anchor
            .is_some_and(|a| !self.selection.contains(a))
        {
            self.selection.anchor = self.selection.primary();
        }
        self.dirty = true;
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

    /// Handle an Aurora `Dropped`: a PNG row (`source`) dragged onto the
    /// viewport (`target`) spawns a sprite showing that texture at the drop
    /// point. Any other source/target pair is ignored.
    fn drop_file(&mut self, source: aurora::WidgetId, target: Option<aurora::WidgetId>) {
        if target != self.rows.viewport {
            return;
        }
        let Some(path) = self
            .rows
            .file_rows
            .iter()
            .find(|(widget, _)| *widget == source)
            .map(|(_, path)| path.clone())
        else {
            return;
        };
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
        self.selection.select_one(node);
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
        self.selection.select_one(node);
        self.dirty = true;
    }

    /// Whether the project has edits since the last save or load.
    fn is_modified(&self) -> bool {
        self.history.revision() != self.saved_revision
    }

    /// The current per-project editor state, for saving.
    fn editor_state(&self) -> editor_state::EditorState {
        let mut pane_scrolls: Vec<(dock::Pane, f32)> =
            self.pane_scrolls.iter().map(|(&p, &s)| (p, s)).collect();
        pane_scrolls.sort_by_key(|(p, _)| p.title());
        editor_state::EditorState {
            dock: self.dock.clone(),
            camera: editor_state::CameraState::from_camera(&self.camera),
            gizmo_mode: self.gizmo_mode,
            pane_scrolls,
            collapsed: editor_state::encode_collapsed(&self.project.scene, &self.tree_collapsed),
            explorer_expanded: self.explorer.expanded_rel(),
            explorer_selected: self.explorer.selected_rel(),
            explorer_view: self.explorer.view(),
        }
    }

    /// Write the per-project editor state to `<project>/.orbit/editor.ron`
    /// (best-effort; a failure is logged, not fatal).
    fn save_editor_state(&mut self) {
        let state = self.editor_state();
        if let Err(err) = editor_state::save(&self.project_dir, &state) {
            tracing::warn!("could not save editor state: {err}");
        }
        self.view_dirty = false;
        self.editor_state_saved = std::time::Instant::now();
    }

    /// A debounced editor-state save: writes ~2s after the last view change, so a
    /// continuous pan or resize does not hit the disk every frame.
    fn maybe_save_editor_state(&mut self) {
        if self.view_dirty && self.editor_state_saved.elapsed() > std::time::Duration::from_secs(2)
        {
            self.save_editor_state();
        }
    }

    /// Write the current viewport view toggles back to the global settings file
    /// so they persist across restarts (best-effort; a failure is logged).
    fn persist_view_prefs(&self) {
        let settings = settings::Settings {
            theme: self.theme,
            show_grid: self.show_grid,
            show_axes: self.show_axes,
            snap: self.snap,
        };
        if let Err(err) = settings::save(&settings) {
            tracing::warn!("could not save view settings: {err}");
        }
    }

    /// The window title for the current state ("* " prefix when unsaved).
    fn title(&self) -> String {
        let mark = if self.is_modified() { "* " } else { "" };
        format!("{mark}{WINDOW_TITLE}")
    }

    fn save_project(&mut self) {
        match self.project.save(&self.project_dir) {
            Ok(()) => {
                self.saved_revision = self.history.revision();
                tracing::info!("project saved to {}", self.project_dir.display());
            }
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
                self.saved_revision = 0;
                self.selection.clear();
                self.renaming = None;
                self.tree_filter.clear();
                self.node_clip.clear();
                // These hold NodeIds from the OLD scene arena. A reload builds a
                // fresh arena, so the old keys are invalid - indexing the slotmap
                // with them (as the next state save would, via encode_collapsed)
                // panics. Drop them; the new scene starts fully expanded.
                self.tree_collapsed.clear();
                self.inspector_collapsed.clear();
                // Re-scan the files on disk and refresh cached previews in place
                // (a reloaded project may have different files at the same paths).
                self.explorer.rescan();
                self.thumbnails.invalidate(&mut self.gui);
                self.dirty = true;
                tracing::info!("project loaded from {}", self.project_dir.display());
            }
            Err(err) => tracing::error!("load failed: {err}"),
        }
    }

    /// Decode a thumbnail for every previewable image in the folder the explorer
    /// is showing (cached, so each decodes once). When a new preview lands, mark
    /// the shell dirty so the next rebuild swaps its type icon for the image.
    fn ensure_thumbnails(&mut self) {
        // Nothing to preview when the Files pane is not built this frame (behind
        // an inactive dock tab): skip the decode work entirely.
        if self.rows.file_contents.is_none() {
            return;
        }
        let paths: Vec<std::path::PathBuf> = self
            .explorer
            .contents()
            .iter()
            .filter(|e| !e.is_dir() && explorer::can_thumbnail(&e.path))
            .map(|e| e.path.clone())
            .collect();
        for path in paths {
            let had = self.thumbnails.get(&path).is_some();
            self.thumbnails.ensure(&mut self.gui, &path);
            if !had && self.thumbnails.get(&path).is_some() {
                self.dirty = true;
            }
        }
    }

    /// Capture the explorer's live layout back into its state: the folder tree's
    /// scroll (so it survives rebuilds) and the grid column count derived from
    /// the contents pane's laid-out width (a change marks dirty so it reflows).
    fn sync_explorer_view(&mut self) {
        if let Some(id) = self.rows.file_tree_scroll {
            let offset = self.ui.scroll_offset(id);
            self.explorer.set_tree_scroll(offset);
        }
        if let Some(id) = self.rows.file_contents
            && let Some(rect) = self.ui.rect(id)
        {
            let cols = grid_cols(rect.size.x);
            if cols != self.explorer_cols {
                self.explorer_cols = cols;
                self.dirty = true;
            }
        }
    }

    /// Show a folder's contents in the explorer (from a tree row, breadcrumb, or
    /// a folder in the contents pane), rebuilding and persisting the view.
    fn select_explorer_dir(&mut self, path: &std::path::Path) {
        self.explorer.select_dir(path);
        self.view_dirty = true;
        self.dirty = true;
    }

    /// Switch the explorer's contents view (list or grid), rebuilding and
    /// persisting the choice.
    fn set_file_view(&mut self, view: explorer::FileView) {
        self.explorer.set_view(view);
        self.view_dirty = true;
        self.dirty = true;
    }

    /// A click on a contents entry: a double-click opens a folder; otherwise it
    /// updates the selection (shift = range, ctrl = toggle, plain = replace).
    fn click_file_entry(&mut self, id: aurora::WidgetId, path: &std::path::Path, is_dir: bool) {
        self.explorer_focused = true;
        let now = std::time::Instant::now();
        let double = self
            .last_click
            .is_some_and(|(w, t)| w == id && now.duration_since(t) < DOUBLE_CLICK);
        self.last_click = Some((id, now));
        if double && is_dir {
            self.select_explorer_dir(path);
            return;
        }
        if self.modifiers.shift_key() {
            self.explorer.select_range(path);
        } else if self.modifiers.control_key() {
            self.explorer.toggle_selected(path);
        } else {
            self.explorer.select_one(path);
        }
        self.dirty = true;
    }

    /// Select every entry in the current folder (explorer ctrl+a).
    fn explorer_select_all(&mut self) {
        self.explorer.select_all();
        self.dirty = true;
    }

    /// Copy (or, with `cut`, cut) the selected entries to the file clipboard.
    fn file_copy(&mut self, cut: bool) {
        let sel = self.explorer.selected();
        if sel.is_empty() {
            return;
        }
        self.file_clip = sel.to_vec();
        self.file_clip_cut = cut;
    }

    /// The folder a paste or new-folder targets: a single highlighted sub-folder
    /// if there is exactly one, else the folder whose contents are shown.
    fn explorer_dest_dir(&self) -> std::path::PathBuf {
        self.explorer
            .selected_single_dir()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| self.explorer.selected_dir().to_path_buf())
    }

    /// Paste the file clipboard into the target folder (copying, or moving for a
    /// cut), then reselect what landed.
    fn file_paste(&mut self) {
        if self.file_clip.is_empty() {
            return;
        }
        let dst = self.explorer_dest_dir();
        let clip = self.file_clip.clone();
        let cut = self.file_clip_cut;
        let mut pasted = Vec::new();
        for src in &clip {
            let result = if cut {
                file_ops::move_into(src, &dst)
            } else {
                file_ops::copy_into(src, &dst)
            };
            match result {
                Ok(path) => pasted.push(path),
                Err(err) => tracing::warn!("paste {} failed: {err}", src.display()),
            }
        }
        // Only clear a cut clipboard once something actually moved, so a fully
        // failed paste leaves it intact for a retry.
        if cut && !pasted.is_empty() {
            self.file_clip.clear();
            self.file_clip_cut = false;
        }
        self.after_file_change(&pasted);
    }

    /// Move the selected entries to the project trash (recoverable).
    fn file_delete(&mut self) {
        let sel = self.explorer.selected().to_vec();
        if sel.is_empty() {
            return;
        }
        let root = self.project_dir.clone();
        let mut moved = 0;
        for path in &sel {
            match file_ops::delete_to_trash(&root, path) {
                Ok(_) => moved += 1,
                Err(err) => tracing::error!("delete {} failed: {err}", path.display()),
            }
        }
        if moved > 0 {
            tracing::info!("moved {moved} item(s) to {}/.orbit/trash", root.display());
        }
        self.after_file_change(&[]);
    }

    /// Duplicate the selected entries beside their originals.
    fn file_duplicate(&mut self) {
        let sel = self.explorer.selected().to_vec();
        if sel.is_empty() {
            return;
        }
        let mut dups = Vec::new();
        for path in &sel {
            match file_ops::duplicate(path) {
                Ok(path) => dups.push(path),
                Err(err) => tracing::warn!("duplicate {} failed: {err}", path.display()),
            }
        }
        self.after_file_change(&dups);
    }

    /// Create a new folder in the target directory and start renaming it.
    fn file_new_folder(&mut self) {
        let dst = self.explorer_dest_dir();
        match file_ops::create_folder(&dst) {
            Ok(path) => {
                self.explorer.rescan();
                self.explorer.select_one(&path);
                self.start_file_rename(&path);
            }
            Err(err) => tracing::error!("new folder failed: {err}"),
        }
        self.dirty = true;
    }

    /// Begin an inline rename of a contents entry (focused on the next rebuild).
    fn start_file_rename(&mut self, path: &std::path::Path) {
        self.explorer.set_renaming(Some(path.to_path_buf()));
        self.focus_file_rename = true;
        self.dirty = true;
    }

    /// Commit a finished inline file rename to disk, reselecting the result.
    fn commit_file_rename(&mut self, old: &std::path::Path, name: String) {
        self.explorer.set_renaming(None);
        match file_ops::rename(old, name.trim()) {
            Ok(new_path) => {
                self.explorer.rescan();
                self.thumbnails.invalidate(&mut self.gui);
                self.explorer.select_one(&new_path);
            }
            Err(err) => tracing::warn!("rename failed: {err}"),
        }
        self.dirty = true;
    }

    /// Cancel an in-progress file rename (Escape), if any.
    fn cancel_file_rename(&mut self) {
        if self.explorer.renaming().is_some() {
            self.explorer.set_renaming(None);
            self.dirty = true;
        }
    }

    /// Shared tail of the file operations: re-scan the folder, refresh previews,
    /// and select the entries the operation produced.
    fn after_file_change(&mut self, produced: &[std::path::PathBuf]) {
        self.explorer.rescan();
        self.thumbnails.invalidate(&mut self.gui);
        self.explorer.clear_selection();
        for path in produced {
            self.explorer.toggle_selected(path);
        }
        self.dirty = true;
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
            aurora::CursorHint::Grabbing => CursorIcon::Grabbing,
        });
    }

    /// Read the search box's live text back into `tree_filter`; when it changed,
    /// mark the shell dirty (to refilter the tree) and remember to re-focus the
    /// box at its caret after the rebuild.
    fn sync_tree_filter(&mut self) {
        let Some(field) = self.rows.tree_filter else {
            return;
        };
        if let WidgetKind::TextInput(text) = self.ui.kind(field)
            && *text != self.tree_filter
        {
            self.tree_filter = text.clone();
            self.refocus_filter = true;
            self.dirty = true;
        }
    }

    /// Read the live dock splitter sizes and per-pane scroll offsets back out of
    /// the laid-out `Ui` into `self.dock`/`self.pane_scrolls`, so the next
    /// rebuild restores them. Called each frame after layout, when the Ui and
    /// dock structure are guaranteed to match.
    fn sync_dock_state(&mut self) {
        let sizes = capture_dock_sizes(&self.ui, &self.rows);
        self.dock.set_fixed_sizes(&sizes);
        self.pane_scrolls = capture_dock_scrolls(&self.ui, &self.rows);
        // The console follows new lines only while the user is at (or near) the
        // bottom; scrolling up pins it there instead.
        if let Some(console) = self.rows.console_panel {
            self.console_follow =
                self.ui.scroll_offset(console) >= self.ui.max_scroll_offset(console) - 1.0;
        }
    }

    /// Rebuild the shell when the log ring has new lines and the console pane is
    /// visible - throttled so a burst of logs does not rebuild every frame.
    fn poll_console(&mut self) {
        if self.last_log_poll.elapsed() < std::time::Duration::from_millis(150) {
            return;
        }
        self.last_log_poll = std::time::Instant::now();
        // No point rebuilding for logs the console is not currently showing.
        if self.rows.console_panel.is_none() {
            return;
        }
        let generation = self
            .logs
            .lock()
            .map_or(self.last_log_gen, |ring| ring.generation);
        if generation != self.last_log_gen {
            self.dirty = true;
        }
    }

    /// Refresh the status bar readouts in place (no shell rebuild).
    fn update_status_bar(&mut self) {
        let cursor = match (self.over_viewport(), self.cursor_world()) {
            (true, Some(w)) => format!("x {:.0}, y {:.0}", w.x, w.y),
            _ => "x -, y -".to_string(),
        };
        let selected = match (self.selection.nodes.len(), self.selection.primary()) {
            (0, _) | (_, None) => "nothing selected".to_string(),
            (1, Some(node)) => self.project.scene.node(node).name.clone(),
            (n, Some(node)) => {
                format!("{} +{} more", self.project.scene.node(node).name, n - 1)
            }
        };
        let zoom = format!("zoom {:.0}%", self.camera.zoom * 100.0);
        let fps = format!("{:.0} fps", self.last_fps);
        let modified = if self.is_modified() {
            "unsaved changes"
        } else {
            ""
        };
        for (id, text) in [
            (self.rows.status_cursor, cursor),
            (self.rows.status_zoom, zoom),
            (self.rows.status_selected, selected),
            (self.rows.status_fps, fps),
            (self.rows.status_modified, modified.to_string()),
        ] {
            if let Some(id) = id {
                self.ui.set_label(id, text);
            }
        }
        // Reflect unsaved state in the window title too (set only on change).
        let title = self.title();
        if title != self.applied_title {
            self.window.set_title(&title);
            self.applied_title = title;
        }
    }

    fn draw(&mut self) -> Result<()> {
        profiling::scope!("editor_frame");
        let fstart = std::time::Instant::now();
        self.poll_settings();
        self.poll_console();
        self.react();
        self.sync_tree_filter();
        // Decode any newly visible previews and capture the explorer's live
        // layout (tree scroll, grid columns); both can request a rebuild, so
        // they run before `dirty` is read.
        self.ensure_thumbnails();
        self.sync_explorer_view();
        let t_react = fstart.elapsed();
        let mut phase = std::time::Instant::now();
        // Whether this frame rebuilds the shell. The insertion-line overlay is
        // added post-layout onto the popup layer, so it only exists on frames
        // that (re)build - on still frames the previous frame's line persists
        // (a single copy, no accumulation). Crucially we do NOT force a rebuild
        // every frame during a reparent: that would reset Aurora's retained
        // `pressed`/`hovered` between a press and its release, killing a plain
        // click's selection and making the held-still insertion line flicker.
        let rebuilt = self.dirty;
        if self.dirty {
            // Capture the search box's caret before discarding the old Ui, so it
            // can be restored after the rebuild (the box refilters, and so
            // rebuilds, on every keystroke - see refocus_filter below). The dock
            // sizes and scroll are captured at the end of each frame instead
            // (sync_dock_state), when the Ui and dock are guaranteed to match -
            // a rearrange this frame has already restructured self.dock.
            let filter_caret = self.ui.caret_offset();
            // Snapshot the log ring for the console pane (brief lock), recording
            // the generation so the poll below only rebuilds when it changes.
            let logs: Vec<console::LogLine> = {
                let ring = self.logs.lock().expect("log ring not poisoned");
                self.last_log_gen = ring.generation;
                ring.lines.iter().cloned().collect()
            };
            let (ui, rows) = build_editor_ui(
                &self.project.scene,
                &self.selection.as_set(),
                self.selection.primary(),
                self.viewport_handle,
                &self.explorer,
                &self.thumbnails,
                self.explorer_cols,
                &self.dock,
                &self.pane_scrolls,
                self.context_menu.as_ref(),
                self.gizmo_mode,
                self.show_grid,
                self.show_axes,
                self.snap.enabled,
                Some(&self.icons),
                &self.tree_view(),
                &self.tree_filter,
                self.renaming,
                &self.inspector_view(),
                &logs,
                self.console_follow,
                &self.theme,
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
                build_color_picker(&mut self.ui, &view, &self.theme, &mut self.rows);
            }
            self.dirty = false;
            // The fresh Ui has no cursor, so its layout-time hover refresh would
            // find nothing hovered. Seed the cursor so hover (and clicking the
            // same spot again, e.g. an eye toggle) survives the rebuild without
            // the user having to nudge the mouse first.
            self.ui.set_cursor(self.cursor);
            // Focus an inline rename field the moment it appears, selecting its
            // text so the first keystroke replaces the old name.
            if self.focus_rename
                && let Some((field, _)) = self.rows.rename_field
            {
                self.ui.focus(field);
                self.focus_rename = false;
            }
            // Same for a file rename in the explorer's contents pane.
            if self.focus_file_rename
                && let Some((field, _)) = self.rows.file_rename_field.as_ref()
            {
                let field = *field;
                self.ui.focus(field);
                self.focus_file_rename = false;
            }
            // Restore focus to the search box at its preserved caret, so typing
            // a query is not interrupted by the per-keystroke rebuild.
            if self.refocus_filter
                && let Some(field) = self.rows.tree_filter
            {
                let offset = filter_caret.unwrap_or(self.tree_filter.len());
                self.ui.focus_caret(field, offset);
                self.refocus_filter = false;
            }
        }
        let t_rebuild = phase.elapsed();
        phase = std::time::Instant::now();

        self.apply_cursor();
        self.update_status_bar();

        // Lay the shell out first: the viewport widget's rect decides how big
        // the scene renders.
        let size = self.window.inner_size();
        let window = Vec2::new(size.width.max(1) as f32, size.height.max(1) as f32);
        self.ui.layout(window)?;

        // A reparent between-rows drop shows an insertion line, overlaid on the
        // popup layer (so it does not shift the rows) using their laid-out
        // rects; add it then lay out once more to position it. Only on frames we
        // just rebuilt: a fresh Ui has no line yet, while a still frame keeps the
        // one from the last rebuild (adding again would stack duplicates).
        if rebuilt && let Some(spot) = self.reparent.and_then(|r| r.target) {
            ui::add_reparent_line(&mut self.ui, &self.rows, spot, self.theme.row_drop);
            self.ui.layout(window)?;
        }

        // Now the shell is laid out and the Ui matches self.dock exactly (this
        // frame's rebuild, if any, has already restructured the dock), read the
        // live splitter sizes and pane scroll back so the next rebuild restores
        // them - the docking equivalent of the old capture_panel_sizes.
        self.sync_dock_state();
        self.maybe_save_editor_state();

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

        let t_mid = phase.elapsed();
        phase = std::time::Instant::now();

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
        // with the tintable white texture: the grid+axes first (furthest back),
        // then the axis guide line, then the selection outline and gizmo handles.
        let mut overlay = Vec::new();
        if self.show_grid || self.show_axes {
            overlay.extend(viewport::grid_sprites(
                &self.camera,
                Vec2::new(w as f32, h as f32),
                self.show_grid,
                self.show_axes,
            ));
        }
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
        if let Some(sel) = self.selection.primary()
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

        let t_scene = phase.elapsed();
        phase = std::time::Instant::now();

        let mut list = self.ui.draw_list();
        // A live drop-zone preview while a dock tab is being dragged: a
        // translucent fill over the half (or whole, for a tab merge) of the
        // group the pane would land in. Appended to the draw list each frame
        // rather than added as a popup, since a tab drag lives in Aurora's
        // retained state and must not trigger a rebuild (which would drop it).
        if let Some(src) = self.ui.drag_source()
            && self.dock_tab_pane(src).is_some()
            && let Some((_, rect)) = self.dock_group_at_cursor()
        {
            let zone = Self::dock_drop_zone(rect, self.cursor);
            list.commands.push(aurora::DrawCommand::FillRect {
                rect: dock_zone_rect(rect, zone),
                color: self.theme.row_drop.fade(0.4),
            });
        }
        self.gui.render(
            &list,
            self.ui.font_system_mut(),
            aurora::Color::rgb(0.08, 0.08, 0.10),
        )?;
        let t_gui = phase.elapsed();

        self.report_fps();
        // Frame profiling (ORBIT_FRAME_LOG): log a breakdown for any slow frame.
        // `pointer` is the time (and count) spent handling pointer-move events
        // since the last frame - they run before draw, so a drag's event flood
        // shows up here, not in the draw phases.
        if self.frame_log {
            let total = fstart.elapsed();
            let slow = total.as_secs_f32() > 0.008 || self.pointer_time.as_secs_f32() > 0.008;
            if slow {
                let ms = |d: std::time::Duration| d.as_secs_f64() * 1000.0;
                tracing::warn!(
                    "draw {:.2}ms [react {:.2} rebuild {:.2}(={}) mid {:.2} scene {:.2} gui {:.2}] | pointer {}x = {:.2}ms | measures {}",
                    ms(total),
                    ms(t_react),
                    ms(t_rebuild),
                    rebuilt,
                    ms(t_mid),
                    ms(t_scene),
                    ms(t_gui),
                    self.pointer_events,
                    ms(self.pointer_time),
                    self.ui.last_measure_count(),
                );
            }
            self.pointer_events = 0;
            self.pointer_time = std::time::Duration::ZERO;
        }
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
                AuroraEvent::Clicked(id) if Some(id) == self.rows.grid => {
                    self.show_grid = !self.show_grid;
                    self.dirty = true;
                    self.persist_view_prefs();
                }
                AuroraEvent::Clicked(id) if Some(id) == self.rows.axes => {
                    self.show_axes = !self.show_axes;
                    self.dirty = true;
                    self.persist_view_prefs();
                }
                AuroraEvent::Clicked(id) if Some(id) == self.rows.snap => {
                    self.snap.enabled = !self.snap.enabled;
                    self.dirty = true;
                    self.persist_view_prefs();
                }
                AuroraEvent::Clicked(id) if Some(id) == self.rows.file_view_list => {
                    self.explorer_focused = true;
                    self.set_file_view(explorer::FileView::List);
                }
                AuroraEvent::Clicked(id) if Some(id) == self.rows.file_view_grid => {
                    self.explorer_focused = true;
                    self.set_file_view(explorer::FileView::Grid);
                }
                AuroraEvent::Clicked(id) if Some(id) == self.rows.file_refresh => {
                    self.explorer_focused = true;
                    self.explorer.rescan();
                    self.thumbnails.invalidate(&mut self.gui);
                    self.dirty = true;
                }
                AuroraEvent::Clicked(id)
                    if self.rows.file_tree_toggles.iter().any(|(w, _)| *w == id) =>
                {
                    let path = self
                        .rows
                        .file_tree_toggles
                        .iter()
                        .find(|(w, _)| *w == id)
                        .map(|(_, p)| p.clone())
                        .expect("guarded by the match arm");
                    self.explorer_focused = true;
                    self.explorer.toggle_expand(&path);
                    self.view_dirty = true;
                    self.dirty = true;
                }
                AuroraEvent::Clicked(id)
                    if self.rows.file_tree_rows.iter().any(|(w, _)| *w == id) =>
                {
                    let path = self
                        .rows
                        .file_tree_rows
                        .iter()
                        .find(|(w, _)| *w == id)
                        .map(|(_, p)| p.clone())
                        .expect("guarded by the match arm");
                    self.explorer_focused = true;
                    self.select_explorer_dir(&path);
                }
                AuroraEvent::Clicked(id) if self.rows.file_crumbs.iter().any(|(w, _)| *w == id) => {
                    let path = self
                        .rows
                        .file_crumbs
                        .iter()
                        .find(|(w, _)| *w == id)
                        .map(|(_, p)| p.clone())
                        .expect("guarded by the match arm");
                    self.explorer_focused = true;
                    self.select_explorer_dir(&path);
                }
                AuroraEvent::Clicked(id)
                    if self.rows.file_entries.iter().any(|(w, ..)| *w == id) =>
                {
                    let (path, is_dir) = self
                        .rows
                        .file_entries
                        .iter()
                        .find(|(w, ..)| *w == id)
                        .map(|(_, p, d)| (p.clone(), *d))
                        .expect("guarded by the match arm");
                    self.click_file_entry(id, &path, is_dir);
                }
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
                        .map(|(_, a)| a.clone())
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
                AuroraEvent::Clicked(id)
                    if self.rows.tree_toggles.iter().any(|(w, _)| *w == id) =>
                {
                    let node = self
                        .rows
                        .tree_toggles
                        .iter()
                        .find(|(w, _)| *w == id)
                        .map(|(_, n)| *n)
                        .expect("guarded by the match arm");
                    self.toggle_collapse(node);
                }
                AuroraEvent::Clicked(id) if self.rows.eye_toggles.iter().any(|(w, _)| *w == id) => {
                    let node = self
                        .rows
                        .eye_toggles
                        .iter()
                        .find(|(w, _)| *w == id)
                        .map(|(_, n)| *n)
                        .expect("guarded by the match arm");
                    // Toggle visibility through the history (one undo step).
                    let now = self.project.scene.node(node).visible;
                    self.history
                        .set_visible(&mut self.project.scene, node, !now);
                    self.dirty = true;
                }
                AuroraEvent::Clicked(id)
                    if self.rows.section_toggles.iter().any(|(w, _)| *w == id) =>
                {
                    let section = self
                        .rows
                        .section_toggles
                        .iter()
                        .find(|(w, _)| *w == id)
                        .map(|(_, s)| *s)
                        .expect("guarded by the match arm");
                    // Sections belong to the primary node (the only one shown).
                    if let Some(node) = self.selection.primary() {
                        self.toggle_section(node, section);
                    }
                }
                AuroraEvent::Clicked(id) if self.dock_tab_pane(id).is_some() => {
                    let pane = self.dock_tab_pane(id).expect("guarded by the match arm");
                    self.activate_tab(pane);
                }
                AuroraEvent::Clicked(id) => {
                    if let Some(&(_, node)) =
                        self.rows.tree_rows.iter().find(|(widget, _)| *widget == id)
                    {
                        self.click_tree_row(id, node);
                    }
                }
                AuroraEvent::Submitted(id) => {
                    // A finished inline rename commits first (before the field
                    // helpers, which would not recognize the rename widget).
                    if let Some((field, node)) = self.rows.rename_field
                        && field == id
                    {
                        if let WidgetKind::TextInput(text) = self.ui.kind(id) {
                            let text = text.clone();
                            self.commit_rename(node, text);
                        }
                        continue;
                    }
                    if let Some((field, path)) = self.rows.file_rename_field.clone()
                        && field == id
                    {
                        if let WidgetKind::TextInput(text) = self.ui.kind(id) {
                            let text = text.clone();
                            self.commit_file_rename(&path, text);
                        }
                        continue;
                    }
                    self.commit_field(id);
                    self.commit_transform_field(id);
                    self.commit_vec_input(id);
                    self.commit_picker_hex(id);
                }
                AuroraEvent::Dropped { source, target } => {
                    // A dragged dock tab re-docks into the group under the
                    // cursor; anything else is a file-explorer drag.
                    if let Some(pane) = self.dock_tab_pane(source) {
                        self.drop_dock_tab(pane);
                    } else {
                        self.drop_file(source, target);
                    }
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

/// The preview rectangle for a dock drop zone within a group's `rect`: the half
/// the pane would occupy for a side drop, or the whole group for a tab merge.
fn dock_zone_rect(rect: aurora::Rect, zone: DropZone) -> aurora::Rect {
    let (w, h) = (rect.size.x, rect.size.y);
    match zone {
        DropZone::Tab => rect,
        DropZone::Left => aurora::Rect::new(rect.pos, Vec2::new(w * 0.5, h)),
        DropZone::Right => aurora::Rect::new(
            Vec2::new(rect.pos.x + w * 0.5, rect.pos.y),
            Vec2::new(w * 0.5, h),
        ),
        DropZone::Top => aurora::Rect::new(rect.pos, Vec2::new(w, h * 0.5)),
        DropZone::Bottom => aurora::Rect::new(
            Vec2::new(rect.pos.x, rect.pos.y + h * 0.5),
            Vec2::new(w, h * 0.5),
        ),
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
/// text as a run of character events. `ctrl` upgrades the arrow keys to
/// word-wise motion (ctrl+left/right).
fn translate_key(event: &winit::event::KeyEvent, ctrl: bool) -> Vec<InputEvent> {
    let named = match &event.logical_key {
        WinitKey::Named(NamedKey::Backspace) => Some(Key::Backspace),
        WinitKey::Named(NamedKey::Delete) => Some(Key::Delete),
        WinitKey::Named(NamedKey::ArrowLeft) if ctrl => Some(Key::WordLeft),
        WinitKey::Named(NamedKey::ArrowRight) if ctrl => Some(Key::WordRight),
        WinitKey::Named(NamedKey::ArrowLeft) => Some(Key::Left),
        WinitKey::Named(NamedKey::ArrowRight) => Some(Key::Right),
        WinitKey::Named(NamedKey::ArrowUp) => Some(Key::Up),
        WinitKey::Named(NamedKey::ArrowDown) => Some(Key::Down),
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
