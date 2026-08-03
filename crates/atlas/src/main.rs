//! atlas - the Editor application: Aurora-based UI shell, scene editing, inspector, code editor, embedded Play.
//!
//! Milestone 3 step 6: the editor looks like an editor - a docked, resizable
//! scene-tree, viewport, inspector, and file explorer, with selection shared
//! across panels and inspector edits committed through the undo history.

mod actions;
mod console;
mod dock;
mod editor_state;
mod explorer;
mod file_ops;
mod icons;
mod modal;
mod project;
mod settings;
mod textures;
mod theme;
mod thumbnails;
mod ui;
mod viewport;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
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

use crate::dock::{DockNode, DropZone, Pane};
use crate::icons::Icons;
use crate::modal::ModalAction;
use crate::textures::TextureCache;
use crate::ui::{
    Axis, ColorTarget, ContextMenu, DropSpot, EditorRows, EditorTheme, FieldRef, InspectorSection,
    InspectorView, MenuAction, TreeView, axis_of, build_editor_ui, build_modal,
    capture_dock_scrolls, capture_dock_sizes, field_color, field_vec2, grid_cols, parse_value,
    resolve_drop, set_field_color, set_field_vec2, value_to_text, visible_nodes, with_axis,
};
use crate::viewport::{Drag, EditorCamera, GizmoHit, GizmoMode};
use aurora::picker::{ColorPicker as AuroraPicker, Press};

/// Two tree-row clicks within this window count as a double-click (start a
/// rename), matching a typical desktop double-click speed.
const DOUBLE_CLICK: std::time::Duration = std::time::Duration::from_millis(400);
/// `a` blended toward `b` by `t` (0 = all `a`, 1 = all `b`).
fn mix(a: aurora::Color, b: aurora::Color, t: f32) -> aurora::Color {
    aurora::Color::rgba(
        a.r + (b.r - a.r) * t,
        a.g + (b.g - a.g) * t,
        a.b + (b.b - a.b) * t,
        a.a + (b.a - a.a) * t,
    )
}

/// How far above and below a tab strip still counts as being "on" it while
/// carrying a tab. The strip is thin, so aiming at it exactly is fussy; this
/// makes the snap forgiving without reaching into the pane's body.
const STRIP_GRAB_Y: f32 = 22.0;

/// A tab strip's grab band: the bar, grown vertically so it is easy to hit.
fn strip_band(rect: aurora::Rect) -> aurora::Rect {
    aurora::Rect::new(
        Vec2::new(rect.pos.x, rect.pos.y - STRIP_GRAB_Y),
        Vec2::new(rect.size.x, rect.size.y + STRIP_GRAB_Y * 2.0),
    )
}

/// The width of the gap a tab strip holds open for a tab being dragged into it.
/// A rough tab width is enough: the gap only has to read as "it lands here".
const CARRIED_TAB_W: f32 = 90.0;

/// How long the cursor must rest on a file entry before its tooltip appears.
const TOOLTIP_DELAY: std::time::Duration = std::time::Duration::from_millis(450);

/// The editor's base window title; prefixed with "* " when there are unsaved
/// changes.
const WINDOW_TITLE: &str = "Orbit - Atlas";

/// The open color picker: which field it edits, the color it had when opened
/// (so the whole drag commits as one undo step), and aurora's picker - which
/// owns the gradients, the drag routing, and the widgets.
struct ColorPicker {
    target: ColorTarget,
    original: [f32; 4],
    picker: AuroraPicker,
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

/// An in-progress slider drag: where it writes, and what was there when it
/// started, so the whole drag becomes one undo step.
#[derive(Debug, Clone)]
struct SliderDrag {
    node: NodeId,
    component: usize,
    field: String,
    original: Option<Value>,
}

/// An in-progress inspector label drag-scrub: adjusting one component of a
/// Vec2 field by the horizontal cursor motion since the press.
#[derive(Debug, Clone)]
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
    /// When the previous frame ran, for animation timing.
    last_frame: std::time::Instant,
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
    /// The script the Code pane shows: its project-relative path, its current
    /// buffer, and whether that buffer differs from the file on disk. One script
    /// at a time is milestone 4's cut, so this is single-valued editor state
    /// rather than anything the dock knows about.
    open_script: Option<String>,
    script_text: String,
    script_modified: bool,
    /// The open script's diagnostics from the last service run, for the status
    /// bar. The squiggles themselves are already on the Ui.
    script_diagnostics: Vec<comet::Diagnostic>,
    /// The open script's syntax colors from the last service run.
    script_spans: Vec<aurora::TextSpan>,
    /// Everything the language service knows about the current buffer, computed
    /// once per edit rather than once per question.
    analysis: comet::service::Analysis,
    /// How the open script was encoded on disk, so a save writes it back the
    /// same way.
    script_encoding: TextEncoding,
    /// Set when the window should close - after the unsaved-changes question, if
    /// there was one. The event loop reads it once per frame.
    quit_requested: bool,
    /// The open script's file was deleted or moved out from under the buffer.
    /// The text stays; the header says so, and Save is unavailable until it is
    /// opened somewhere real again.
    script_orphaned: bool,
    /// Focus the find bar's query field after the next rebuild, and select what
    /// is in it. Consumed there, like `refocus_filter`.
    focus_find: bool,
    /// The diagnostic message currently shown as a tooltip, so the shell only
    /// rebuilds when what is under the pointer changes.
    diagnostic_tooltip: Option<String>,
    /// Where the pointer was when it last moved, and when it stopped - a
    /// tooltip belongs to a resting pointer, not a passing one.
    tooltip_cursor: Vec2,
    tooltip_rested: std::time::Instant,
    /// The go-to-line box's contents while it is open.
    go_to_line: Option<String>,
    /// The go-to-symbol palette: what has been typed into it, and the list of
    /// matching declarations. Open only over the Code pane.
    symbol_palette: Option<(String, aurora::list::ListPopup)>,
    /// Where the caret and the view were, per script, so reopening a file this
    /// session puts you back where you left rather than at the top.
    script_positions: std::collections::HashMap<String, (usize, f32)>,
    /// The call the caret is inside, and where to draw the hint. Refreshed on
    /// every edit and caret move, like the completion list.
    signature: Option<(comet::service::SignatureHelp, Vec2)>,
    /// A transient line for the status bar - what a jump found, or why it did
    /// not go anywhere - and when it was set, so it fades rather than lingering.
    status_note: Option<(String, std::time::Instant)>,
    /// An in-progress symbol rename: where in the buffer, the name being typed,
    /// and why the last attempt was refused.
    symbol_rename: Option<SymbolRename>,
    /// Set when a script is opened: focus the editor after the next rebuild.
    /// A file that appears with no caret swallows the first keystroke.
    focus_editor: bool,
    /// Set by ctrl+space: offer the list wherever the caret is, rather than only
    /// where a name is being typed. Cleared as soon as it has been used.
    force_completions: bool,
    /// When the caret's blink phase last flipped, and what the caret was doing
    /// then - so it holds solid while you type instead of blinking under you.
    caret_phase: std::time::Instant,
    caret_visible: bool,
    caret_was: Option<usize>,
    /// Text undo for the Code pane: snapshots of the buffer and where the caret
    /// was, newest last.
    ///
    /// Here rather than in aurora because a rebuild swaps the whole Ui, and an
    /// undo history that vanished whenever a node got selected would be worse
    /// than none. atlas owns the document; aurora owns the widget showing it.
    script_undo: Vec<(String, usize)>,
    script_redo: Vec<(String, usize)>,
    /// Where the caret was last time the buffer was read, so an undo step
    /// records the position the edit started from rather than where it ended.
    script_caret: usize,
    /// Whether the last edit joined the open undo step. A run only continues a
    /// run: without this, the first letter after a space would fold into the
    /// space's step and one undo would swallow both.
    script_coalescing: bool,
    /// The autocomplete popup over the Code pane, while one is open, and the
    /// byte range of the word it is completing.
    ///
    /// The range is held rather than re-read on accept, because pressing a row
    /// moves focus to that row - a button is not a text input - and the caret is
    /// a property of the focused widget, so by the time the click arrives there
    /// is no caret to ask about.
    completions: Option<(aurora::list::ListPopup, std::ops::Range<usize>)>,
    /// The find bar over the Code pane, while one is open.
    find: Option<aurora::find::FindBar>,
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
    /// Strip trailing whitespace and fix the final newline when saving a script.
    tidy_on_save: bool,
    /// Transform snapping (on/off + increments); persisted in global settings.
    /// Ctrl held during a drag inverts the on/off state for that drag.
    snap: settings::SnapSettings,
    /// The OS clipboard (for text copy/cut/paste), or `None` if it could not
    /// be opened on this platform.
    clipboard: Option<arboard::Clipboard>,
    /// An in-progress inspector label drag-scrub, if any.
    scrub: Option<ScrubDrag>,
    slider_drag: Option<SliderDrag>,
    /// The toolbar icons, rasterized and registered once at startup.
    icons: Icons,
    /// The open color picker, if any.
    picker: Option<ColorPicker>,
    /// The open modal dialog (error message or the settings overlay), if any.
    /// While it is `Some`, a backdrop popup blocks all interaction with the
    /// shell and the editor's keyboard shortcuts are suppressed.
    modal: Option<modal::Modal>,
    /// The picker's three gradient images (registered once, updated in place as
    /// the color changes).
    picker_images: [ImageHandle; 3],
    /// Collapsed scene-tree nodes (editor state, kept across shell rebuilds).
    tree_collapsed: HashSet<NodeId>,
    /// Collapsed inspector sections per node (editor state, kept across shell
    /// rebuilds).
    inspector_collapsed: HashSet<(NodeId, InspectorSection)>,
    /// The editor's resolved color theme (what the UI draws with), and the
    /// authored document it came from. Both are loaded from the user's settings
    /// file and hot-reloaded when it changes; the doc is kept so persisting other
    /// settings does not clobber the user's authored theme.
    theme: EditorTheme,
    theme_doc: theme::ThemeDoc,
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
    /// The explorer entry the cursor is resting on and when it arrived, used to
    /// show a details tooltip after a short hover.
    hover_file: Option<(PathBuf, std::time::Instant)>,
    /// The entry whose hover tooltip is currently shown (a rebuild adds it).
    tooltip_target: Option<PathBuf>,
    /// Whether the left mouse button is currently held. While it is, the tooltip
    /// logic is frozen: a tooltip appears via a rebuild, and rebuilding mid-press
    /// would invalidate Aurora's retained drag state and kill a file drag.
    pointer_down: bool,
    /// The newest pointer position reported since the last frame, applied by
    /// `flush_pointer`. Motion events arrive at the mouse's report rate (125-1000
    /// Hz); running the drag handlers per event repeated the same work many times
    /// between two presented frames.
    pending_cursor: Option<Vec2>,
    /// One aurora tab bar per dock group, in dock-walk order: it owns dragging a
    /// tab along its strip to reorder, and the slide that follows.
    tab_bars: Vec<aurora::TabBar>,
    /// The pane being dragged out of its tab bar. While set, the shell is built
    /// without it - so the dock reflows as though it were already gone - and a
    /// small preview of its tab follows the pointer instead.
    carrying: Option<Pane>,
    /// Where inside the tab the pointer grabbed it, so the carried copy sits
    /// under the cursor the same way it did before it was lifted out.
    carry_grab: Vec2,
    /// A pane just dropped into a tab strip and the x it was dropped at, so its
    /// tab can slide into place once the shell has been rebuilt with it.
    arriving: Option<(Pane, f32)>,
}

/// A tree row being dragged to reparent it; `target` is the current valid drop
/// spot under the cursor (into a node, or before/after it for reordering).
#[derive(Debug, Clone, Copy)]
struct ReparentDrag {
    source: NodeId,
    target: Option<DropSpot>,
}

/// How long the pointer must rest before a hover tooltip appears.
const HOVER_DELAY: std::time::Duration = std::time::Duration::from_millis(400);

/// How long a status-bar note stays up before the ordinary readout returns.
const STATUS_NOTE_LIFETIME: std::time::Duration = std::time::Duration::from_secs(4);

/// An in-progress rename of the symbol at `offset`.
#[derive(Debug, Clone)]
struct SymbolRename {
    /// Where in the buffer the symbol was, so the rename resolves against the
    /// same declaration no matter where the caret goes while typing.
    offset: usize,
    draft: String,
    /// Why the last confirm was refused, shown beside the field.
    error: Option<String>,
}

/// How long each half of the caret's blink lasts.
const CARET_BLINK: std::time::Duration = std::time::Duration::from_millis(530);

/// How many text-undo steps the Code pane keeps. Deep enough that a session
/// never notices, bounded so a long one cannot grow without limit.
const MAX_TEXT_UNDO: usize = 200;

/// Whether an edit from `before` to `after` should join the previous undo step
/// rather than start a new one: a run of ordinary typing, so undo goes back a
/// word at a time instead of a letter at a time. Whitespace ends a run, which is
/// what makes "a word" the unit.
fn coalesces(before: &str, after: &str) -> bool {
    if after.len() != before.len() + 1 || !after.starts_with(before) {
        return false;
    }
    after
        .as_bytes()
        .last()
        .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
}

/// Comment or uncomment a block of lines.
///
/// Uncomments only when every non-blank line is already commented, so a block
/// where half is commented gets the rest commented rather than each line
/// flipped and the same mess left inverted. The `//` goes at the shallowest
/// indentation in the block, so the code keeps its shape, and uncommenting
/// takes back the single space it added - a round trip is exact.
fn toggle_comment_block(block: &str) -> String {
    let lines: Vec<&str> = block.split('\n').collect();
    let content: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|l| !l.trim().is_empty())
        .collect();
    let all_commented =
        !content.is_empty() && content.iter().all(|l| l.trim_start().starts_with("//"));
    let indent = content
        .iter()
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);

    lines
        .iter()
        .map(|line| {
            if line.trim().is_empty() {
                return (*line).to_string();
            }
            if all_commented {
                let at = line.find("//").expect("every content line has one");
                let rest = &line[at + 2..];
                let rest = rest.strip_prefix(' ').unwrap_or(rest);
                format!("{}{rest}", &line[..at])
            } else {
                format!("{}// {}", &line[..indent], &line[indent..])
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The diagnostic covering `offset`, narrowest first.
///
/// Narrowest wins because a wide diagnostic - "this function must return f32",
/// spanning a whole body - would otherwise hide the precise one inside it, and
/// the precise one is the one that says what to change. A zero-width span (an
/// unclosed brace, reported at the end of the file) is found at its own offset.
fn narrowest_at(all: &[comet::Diagnostic], offset: usize) -> Option<&comet::Diagnostic> {
    all.iter()
        .filter(|d| {
            let (lo, hi) = (d.span.start as usize, d.span.end as usize);
            offset >= lo && (offset < hi || (lo == hi && offset == lo))
        })
        .min_by_key(|d| d.span.end - d.span.start)
}

/// How the open script's bytes were encoded on disk, so a save writes it back
/// the way it came rather than quietly rewriting the whole file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct TextEncoding {
    /// The file began with a UTF-8 byte-order mark.
    bom: bool,
    /// Its dominant line ending was CRLF.
    crlf: bool,
}

impl TextEncoding {
    /// Decode `raw`, returning the text with LF endings and no BOM, plus what
    /// was stripped. `None` when the bytes are not UTF-8.
    ///
    /// The buffer always holds LF: a CRLF file otherwise puts a carriage return
    /// inside every line, which shifts every column by one and makes comet
    /// report diagnostics one character off.
    fn decode(raw: &[u8]) -> Option<(String, Self)> {
        let bom = raw.starts_with(&[0xEF, 0xBB, 0xBF]);
        let body = if bom { &raw[3..] } else { raw };
        let text = std::str::from_utf8(body).ok()?;
        let crlf = text.matches("\r\n").count() * 2 > text.matches('\n').count();
        Some((
            text.replace("\r\n", "\n").replace('\r', "\n"),
            Self { bom, crlf },
        ))
    }

    /// Put back what decoding took off.
    fn encode(self, text: &str) -> String {
        let body = if self.crlf {
            text.replace('\n', "\r\n")
        } else {
            text.to_string()
        };
        if self.bom {
            format!("\u{feff}{body}")
        } else {
            body
        }
    }
}

/// Write `bytes` to `path` without ever leaving it half-written: a temp sibling
/// is written and flushed first, then renamed over the target.
///
/// `std::fs::write` truncates the file before it writes, so a crash or a full
/// disk mid-save leaves an empty or partial script and the original is gone. The
/// temp name starts with a dot so the file explorer, which hides dotfiles, does
/// not flicker it into the list.
fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let temp = path.with_file_name(format!(".{name}.tmp"));
    {
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    match std::fs::rename(&temp, path) {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = std::fs::remove_file(&temp);
            Err(err)
        }
    }
}

/// What a find-bar button does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FindAction {
    Next,
    Previous,
    Replace,
    ReplaceAll,
    Close,
}

/// What to draw a completion's name and its note in.
///
/// The same colors the code itself uses, so the list looks like what it is
/// about to insert: a keyword arrives in the keyword color, a type in the type
/// color. The note is colored by what it *is* rather than by the item's kind -
/// a variable's note is its type, a function's note is its signature, and a
/// keyword's note is a sentence of English, which is not code and is drawn as
/// the quiet prose it is.
fn completion_colors(
    kind: comet::service::CompletionKind,
    theme: &ui::EditorTheme,
) -> (Option<aurora::Color>, Option<aurora::Color>) {
    use comet::service::CompletionKind as K;
    match kind {
        K::Keyword => (Some(theme.code_keyword), None),
        K::Type | K::GenericType => (Some(theme.code_type), None),
        K::Function => (Some(theme.code_function), Some(theme.code_function)),
        // An identifier is plain in the editor, and its note is its type.
        K::Variable | K::Field => (None, Some(theme.code_type)),
    }
}

/// The value an exported variable of this type starts at, as the inspector's
/// `Value`. `None` for a type that cannot be stored on a component - the
/// checker refuses those, so this is the same rule seen from the other side.
fn default_for(ty: comet::Type) -> Option<helios::Value> {
    Some(match ty {
        comet::Type::F32 => helios::Value::F32(0.0),
        // The inspector has no int field yet, so an exported int is shown and
        // stored as an f32. Narrowing back happens at the wasm boundary.
        comet::Type::Int => helios::Value::F32(0.0),
        comet::Type::Bool => helios::Value::Bool(false),
        comet::Type::Vec2 => helios::Value::Vec2(Vec2::ZERO),
        // A payload-free enum is stored as its tag. The inspector has no
        // dropdown yet, so it shows as a number - provisional, and the same
        // wart as an exported int.
        comet::Type::Enum(_) => helios::Value::F32(0.0),
        _ => return None,
    })
}

/// Which way [`State::change_case`] is going.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Case {
    Upper,
    Lower,
    Title,
}

impl Case {
    fn apply(self, text: &str) -> String {
        match self {
            Case::Upper => text.to_uppercase(),
            Case::Lower => text.to_lowercase(),
            // Title case runs over words, and `_` separates them as much as a
            // space does in code.
            Case::Title => {
                let mut out = String::with_capacity(text.len());
                let mut fresh = true;
                for ch in text.chars() {
                    if fresh {
                        out.extend(ch.to_uppercase());
                    } else {
                        out.extend(ch.to_lowercase());
                    }
                    fresh = !ch.is_alphanumeric();
                }
                out
            }
        }
    }
}

/// Collapse `block` onto one line: trailing whitespace goes, the next line's
/// indent goes, and the two are joined by a single space - or by nothing when
/// one side is empty, so a blank line does not become a stray space.
fn join_block(block: &str) -> String {
    let mut out = String::with_capacity(block.len());
    for line in block.split('\n') {
        let piece = line.trim();
        if piece.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(piece);
    }
    // The first line keeps its indentation: joining is about the seam, not
    // about moving the block to the margin.
    let indent: String = block
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();
    format!("{indent}{out}")
}

/// The swap ctrl+t should make at `caret`: the range to replace and what to put
/// there, or `None` when there is nothing to swap.
///
/// Characters, not bytes, so a multi-byte character is never split in half.
fn transposition(text: &str, caret: usize) -> Option<(std::ops::Range<usize>, String)> {
    let caret = caret.min(text.len());
    if !text.is_char_boundary(caret) {
        return None;
    }
    let line_start = text[..caret].rfind('\n').map_or(0, |at| at + 1);
    let line_end = text[caret..].find('\n').map_or(text.len(), |at| caret + at);
    let before = text[line_start..caret].chars().next_back()?;
    // At the end of a line there is nothing after the caret to swap with, so
    // take the two behind it - which is where the typo you just spotted is.
    let after = text[caret..line_end].chars().next();
    match after {
        Some(after) => {
            let start = caret - before.len_utf8();
            let end = caret + after.len_utf8();
            Some((start..end, format!("{after}{before}")))
        }
        None => {
            let earlier = text[line_start..caret - before.len_utf8()]
                .chars()
                .next_back()?;
            let start = caret - before.len_utf8() - earlier.len_utf8();
            Some((start..caret, format!("{before}{earlier}")))
        }
    }
}

/// The word the caret is in or next to: where it starts, and its text.
///
/// Unlike [`word_before`], this reaches forward as well, so the caret sitting at
/// the start of a name still finds that name.
fn word_before_or_around(text: &str, caret: usize) -> (usize, &str) {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let caret = caret.min(text.len());
    if !text.is_char_boundary(caret) {
        return (caret, "");
    }
    let start = text[..caret]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_word(*c))
        .last()
        .map_or(caret, |(i, _)| i);
    let end = text[caret..]
        .char_indices()
        .take_while(|(_, c)| is_word(*c))
        .last()
        .map_or(caret, |(i, c)| caret + i + c.len_utf8());
    (start, &text[start..end])
}

/// Strip trailing spaces and tabs from every line, and end the text with
/// exactly one newline. Returns the result and where `caret` ended up.
///
/// The caret's own line is spared when it holds nothing but whitespace: that is
/// a fresh auto-indent being typed into, and yanking it away on save would move
/// the caret to the margin under someone's hands.
///
/// The caret is kept on the same character. It has to be: the tidy runs through
/// the buffer, so the widget is rewritten under a caret that is still live.
fn tidy_for_save(text: &str, caret: usize) -> (String, usize) {
    let caret = caret.min(text.len());
    let mut out = String::with_capacity(text.len());
    let mut removed_before_caret = 0usize;
    let mut at = 0usize;
    for line in text.split_inclusive('\n') {
        let body = line.strip_suffix('\n').unwrap_or(line);
        let blank = !body.is_empty() && body.bytes().all(|b| b == b' ' || b == b'\t');
        let caret_here = caret >= at && caret <= at + body.len();
        let trimmed = if caret_here && blank {
            body
        } else {
            body.trim_end_matches([' ', '\t'])
        };
        let cut_from = at + trimmed.len();
        if caret >= at + body.len() {
            removed_before_caret += body.len() - trimmed.len();
        } else if caret > cut_from {
            removed_before_caret += caret - cut_from;
        }
        out.push_str(trimmed);
        if line.len() != body.len() {
            out.push('\n');
        }
        at += line.len();
    }
    // One trailing newline, not none and not four. Files this editor writes get
    // diffed and concatenated, and a missing final newline is a spurious
    // one-line diff forever.
    while out.ends_with("\n\n") {
        out.pop();
    }
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    let caret = caret.saturating_sub(removed_before_caret).min(out.len());
    (out, caret)
}

/// Where ctrl+up / ctrl+down should put the caret, or `None` when there is no
/// declaration that way.
///
/// Strictly past the caret in the direction of travel, so holding the key walks
/// the file rather than sticking on the declaration it is already on. At the
/// last one, going further does nothing: wrapping to the top would look
/// identical to not having moved.
fn step_symbol_target(
    symbols: &[comet::service::Symbol],
    caret: usize,
    forward: bool,
) -> Option<usize> {
    let caret = caret as u32;
    let target = if forward {
        symbols.iter().find(|s| s.span.start > caret)
    } else {
        symbols.iter().rev().find(|s| s.span.start < caret)
    };
    target.map(|s| s.span.start as usize)
}

/// Where a completion popup should be anchored for a caret at `caret`, or
/// `None` if none should be open there.
///
/// One predicate, so "should there be a popup" has exactly one answer rather
/// than being decided in pieces at each place that might close one. A popup is
/// wanted while a name is being typed, and right after a `.` where the name has
/// not been started yet - and nowhere else, which is what takes it down when
/// the caret is clicked into whitespace eleven lines away.
fn completion_anchor(text: &str, caret: usize) -> Option<usize> {
    let (start, prefix) = word_before(text, caret);
    if !prefix.is_empty() {
        return Some(start);
    }
    // Right after a dot: field completions are the whole point of asking there.
    let after_dot = start > 0 && text.as_bytes()[start - 1] == b'.';
    after_dot.then_some(start)
}

/// The identifier the caret sits in or just after: where it starts, and its
/// text. An empty name means the caret is not in one.
///
/// comet's, not a second copy: this decides what a completion replaces, so the
/// language it is completing should be the one that says where a name begins.
/// The copy that used to live here stepped one *byte* past the character it
/// found, so a single non-ASCII character before the caret - an em dash pasted
/// into a comment - sliced mid-character and took the editor down. This runs on
/// every keystroke in the Code pane.
fn word_before(text: &str, caret: usize) -> (usize, &str) {
    comet::service::word_before(text, caret)
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        match State::new(event_loop, self.logs.clone()) {
            Ok(mut state) => {
                // A scene opened at startup can already have scripts attached,
                // and their exported values are read from the source files -
                // which nothing has looked at yet. Without this the inspector
                // showed a script's path and none of its fields until the
                // script was next saved.
                state.reconcile_script_exports();
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
            WindowEvent::CloseRequested => {
                // Ask before dropping unsaved script edits. Scene edits are
                // tracked separately (history.revision vs saved_revision) and
                // get the same guard.
                if state.close_would_lose_work() {
                    state.ask_before_quitting();
                } else {
                    event_loop.exit();
                }
            }
            WindowEvent::Resized(size) => state.resize((size.width, size.height)),
            WindowEvent::CursorMoved { position, .. } => {
                // Record only; draw() applies the newest position once per frame
                // (see flush_pointer). A mouse reports far faster than the editor
                // presents, and the drag handlers below are not cheap.
                state.pending_cursor = Some(Vec2::new(position.x as f32, position.y as f32));
            }
            WindowEvent::CursorLeft { .. } => state.ui.handle_input(InputEvent::PointerLeft),
            WindowEvent::MouseInput {
                state: btn_state,
                button,
                ..
            } => {
                // Any button acts on the position it was pressed/released at, so
                // apply a pending move first (see flush_pointer).
                state.flush_pointer();
                match (button, btn_state) {
                    (MouseButton::Left, ElementState::Pressed) => {
                        // A modal takes over: the backdrop already blocks the shell,
                        // and this routes the press to the dialog (button/input) or
                        // dismisses it on a backdrop click.
                        if state.modal.is_some() {
                            state.handle_modal_press();
                            return;
                        }
                        // While the button is held the tooltip logic is frozen (see
                        // update_file_tooltip): a rebuild here - even to hide a shown
                        // tooltip - would re-id the widgets and break a file drag.
                        state.pointer_down = true;
                        let hit = state.ui.hit_test(state.cursor);
                        // Keyboard focus follows the press: the shared shortcuts
                        // (delete/copy/rename) act on files when the press lands in
                        // the explorer pane, on the scene otherwise.
                        state.explorer_focused = state.hit_in_explorer_pane(hit);
                        // A press on empty space in the contents area clears the file
                        // selection (click "nothing" to deselect).
                        state.clear_file_selection_on_empty_press(hit);
                        // A press outside an open menu dismisses it (before the
                        // viewport/gizmo logic, so a click on empty space just
                        // closes the menu without also deselecting).
                        if state.close_menu_if_outside() {
                            return;
                        }
                        state.ui.handle_input(InputEvent::PointerPressed);
                        // ctrl+click in the code editor jumps to what declares
                        // the name under the pointer. After the press, so the
                        // caret has already moved to where the click landed and
                        // go_to_definition reads the name that was clicked.
                        if state.modifiers.control_key()
                            && hit.is_some()
                            && hit == state.rows.code_editor
                        {
                            state.go_to_definition();
                            return;
                        }
                        // A press on a dock tab arms a reorder drag along its own
                        // bar. Non-consuming: a plain click still activates the
                        // tab, and dragging off the bar still becomes a dock move.
                        state.press_tab_bar();
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
                        state.dismiss_code_popups();
                        state.begin_reparent();
                        state.viewport_press();
                    }
                    (MouseButton::Left, ElementState::Released) => {
                        state.pointer_down = false;
                        state.release_tab_bars();
                        state.finish_reparent();
                        state.end_picker_drag();
                        state.end_scrub();
                        state.end_slider_drag();
                        state.end_drag();
                        // Aurora's PointerReleased may emit Event::Dropped for a
                        // file-explorer drag onto the viewport (handled in react).
                        state.ui.handle_input(InputEvent::PointerReleased);
                    }
                    (MouseButton::Right, ElementState::Pressed) => state.open_context_menu(),
                    (MouseButton::Middle, ElementState::Pressed) => {
                        // A modal takes over: no viewport pan behind it.
                        state.panning = state.modal.is_none() && state.over_viewport();
                    }
                    (MouseButton::Middle, ElementState::Released) => state.panning = false,
                    _ => {}
                }
            }
            WindowEvent::MouseWheel { delta, .. } => state.wheel(delta),
            WindowEvent::ModifiersChanged(m) => {
                state.modifiers = m.state();
                // Aurora needs shift to know whether movement keys extend a
                // text selection.
                state.ui.set_shift(m.state().shift_key());
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                // A modal takes over the keyboard: Escape closes it, Enter
                // confirms its default action (when no field is focused), and
                // every other key reaches only a focused modal input - the
                // editor's own shortcuts are suppressed.
                if state.modal.is_some() {
                    if let WinitKey::Named(named) = &event.logical_key {
                        if *named == NamedKey::Escape && state.modal_closable() {
                            state.close_modal();
                            return;
                        }
                        if *named == NamedKey::Enter && state.ui.focused().is_none() {
                            state.confirm_modal_default();
                            return;
                        }
                    }
                    for ev in translate_key(
                        &event,
                        state.modifiers.control_key(),
                        state.modifiers.shift_key(),
                    ) {
                        state.ui.handle_input(ev);
                    }
                    return;
                }
                // The code pane's popups go first: an open completion list owns
                // the arrows and Enter, and Escape closes whichever is open -
                // otherwise Enter would insert a newline behind the popup.
                if state.handle_code_key(&event) {
                    return;
                }
                if !state.handle_shortcut(&event)
                    && !state.handle_editor_key(&event)
                    && !state.handle_mode_key(&event)
                {
                    for ev in translate_key(
                        &event,
                        state.modifiers.control_key(),
                        state.modifiers.shift_key(),
                    ) {
                        state.ui.handle_input(ev);
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                if let Err(err) = state.draw() {
                    tracing::error!("render error: {err:#}");
                }
                if state.quit_requested {
                    event_loop.exit();
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
        // The picker's three gradients, registered once; aurora refills them in
        // place as the color changes.
        let picker_images = AuroraPicker::initial_bitmaps()
            .map(|b| gui.register_image_rgba_unmipped(&b.rgba, b.width, b.height));

        let project_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("demo_project");
        let project = project::open_or_create(&project_dir)?;
        // Decode the demo sprite once for its natural pixel size (the default
        // size for newly spawned sprites); its pixels load through the cache.
        // Missing or unreadable (e.g. deleted through the file explorer) falls
        // back to a fixed size rather than refusing to open the project.
        let (w, h) = std::fs::read(project_dir.join("assets/sprite.png"))
            .ok()
            .and_then(|bytes| image::load_from_memory(&bytes).ok())
            .map(|img| img.to_rgba8().dimensions())
            .unwrap_or_else(|| {
                tracing::warn!(
                    "demo sprite assets/sprite.png missing or unreadable; \
                     using a default sprite size"
                );
                (100, 100)
            });

        let (scene_target, viewport_handle) =
            build_viewport_target(&mut gui, &engine, (size.width, size.height));
        // Editor prefs come from the user's settings file (~/.config/orbit),
        // written with defaults on first run - see the `settings` module.
        let settings = settings::load();
        let theme_doc = settings.theme;
        let theme = theme::resolve(&theme_doc);
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
            None,
            "",
            false,
            false,
            &[],
            &[],
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
            last_frame: std::time::Instant::now(),
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
            open_script: None,
            script_text: String::new(),
            script_modified: false,
            script_diagnostics: Vec::new(),
            script_spans: Vec::new(),
            analysis: comet::service::Analysis::default(),
            script_encoding: TextEncoding::default(),
            quit_requested: false,
            script_orphaned: false,
            focus_find: false,
            diagnostic_tooltip: None,
            tooltip_cursor: Vec2::ZERO,
            tooltip_rested: std::time::Instant::now(),
            go_to_line: None,
            symbol_palette: None,
            script_positions: std::collections::HashMap::new(),
            focus_editor: false,
            signature: None,
            status_note: None,
            symbol_rename: None,
            force_completions: false,
            caret_phase: std::time::Instant::now(),
            caret_visible: true,
            caret_was: None,
            script_undo: Vec::new(),
            script_redo: Vec::new(),
            script_caret: 0,
            script_coalescing: false,
            completions: None,
            find: None,
            view_dirty: false,
            editor_state_saved: std::time::Instant::now(),
            context_menu: None,
            gizmo_mode,
            show_grid: settings.show_grid,
            show_axes: settings.show_axes,
            tidy_on_save: settings.tidy_on_save,
            snap: settings.snap,
            clipboard: arboard::Clipboard::new()
                .inspect_err(|err| tracing::warn!("no clipboard available: {err}"))
                .ok(),
            scrub: None,
            slider_drag: None,
            icons,
            picker: None,
            modal: None,
            picker_images,
            tree_collapsed,
            inspector_collapsed: HashSet::new(),
            theme,
            theme_doc,
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
            hover_file: None,
            tooltip_target: None,
            pointer_down: false,
            pending_cursor: None,
            tab_bars: Vec::new(),
            carrying: None,
            carry_grab: Vec2::ZERO,
            arriving: None,
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

    /// Apply the newest pointer position reported since the last call, if any.
    /// Called once per frame, and before a button event so a click acts on the
    /// position the user made it at.
    fn flush_pointer(&mut self) {
        if let Some(p) = self.pending_cursor.take() {
            self.pointer_moved(p);
        }
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
        if self.picker.as_ref().is_some_and(|p| p.picker.is_dragging()) {
            self.apply_picker_drag();
        }
        if self.tab_drag_active() {
            self.drag_tab_bar();
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
            .filter(|(_, field, _)| {
                matches!(field, FieldRef::Position(n) | FieldRef::Scale(n) if *n == node)
            })
            .map(|(widget, field, axis)| {
                let value = axis_of(field_vec2(scene, field), *axis);
                (*widget, value_to_text(&Value::F32(value)))
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
    fn sync_vec_input(&mut self, field: &FieldRef, axis: Axis) {
        let value = axis_of(field_vec2(&self.project.scene, field), axis);
        let text = value_to_text(&Value::F32(value));
        let widget = self
            .rows
            .vec_inputs
            .iter()
            .find(|(_, f, a)| f == field && *a == axis)
            .map(|(w, ..)| *w);
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
        let Some(world) = self.cursor_world() else {
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

        match self.pick_node_at_cursor() {
            Some(mut node) => {
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
            None => {
                if !self.selection.is_empty() {
                    self.selection.clear();
                    self.dirty = true;
                }
            }
        }
    }

    /// GPU-pick the sprite under the cursor, or `None` for empty space (or a
    /// cursor outside the viewport). Picks against the same per-texture runs the
    /// scene renders with, so alpha-discard uses each sprite's real texture; the
    /// returned index is into the flattened draw order, which matches `draws`.
    fn pick_node_at_cursor(&mut self) -> Option<NodeId> {
        let rect = self.viewport_rect()?;
        let local = self.cursor - rect.pos;
        let camera = self.camera.camera(rect.size);
        let draws = self.project.scene.sprite_draws();
        let runs = self.texture_runs(&draws);
        let run_refs: Vec<(&Texture, &[Sprite])> = runs
            .iter()
            .map(|(path, sprites)| (self.textures.get(path), sprites.as_slice()))
            .collect();
        match self.engine.pick_runs(
            (self.scene_target.width, self.scene_target.height),
            &camera,
            &run_refs,
            (local.x.max(0.0) as u32, local.y.max(0.0) as u32),
        ) {
            Ok(picked) => picked.map(|index| draws[index].node),
            Err(err) => {
                tracing::error!("pick failed: {err}");
                None
            }
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
            && loaded.theme != self.theme_doc
        {
            self.theme_doc = loaded.theme;
            self.theme = theme::resolve(&self.theme_doc);
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
        // A modal owns all input; no context menu behind it.
        if self.modal.is_some() {
            return;
        }
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

    /// Clear the file selection when a press lands on empty space in the contents
    /// area - inside the contents pane but not on any entry (or its children).
    fn clear_file_selection_on_empty_press(&mut self, hit: Option<aurora::WidgetId>) {
        let (Some(hit), Some(contents)) = (hit, self.rows.file_contents) else {
            return;
        };
        let in_contents = hit == contents || self.ui.is_within(hit, contents);
        let on_entry = self
            .rows
            .file_entries
            .iter()
            .any(|(w, ..)| *w == hit || self.ui.is_within(hit, *w));
        if in_contents && !on_entry && !self.explorer.selected().is_empty() {
            self.explorer.clear_selection();
            self.dirty = true;
        }
    }

    /// Track the hovered file entry and, after a short rest, show its details
    /// tooltip (a rebuild adds the popup post-layout). Hovering off hides it; an
    /// open menu suppresses it.
    fn update_file_tooltip(&mut self) {
        // Frozen while a button is held (a tooltip change forces a rebuild, and
        // rebuilding during a press/drag breaks the file drag) or while a modal
        // is up (it owns the screen).
        if self.pointer_down || self.modal.is_some() {
            return;
        }
        let hovered = if self.context_menu.is_some() {
            None
        } else {
            self.ui.hovered().and_then(|h| {
                self.rows
                    .file_entries
                    .iter()
                    .find(|(w, ..)| *w == h)
                    .map(|(_, p, _)| p.clone())
            })
        };
        let Some(path) = hovered else {
            self.hover_file = None;
            self.hide_tooltip();
            return;
        };
        // Whether the cursor is still on the same entry, and if so whether it has
        // rested long enough (computed in a scoped borrow so `hover_file` is free
        // to reassign below).
        let rested = match &self.hover_file {
            Some((p, since)) if *p == path => Some(since.elapsed() >= TOOLTIP_DELAY),
            _ => None,
        };
        match rested {
            Some(ready) => {
                if ready && self.tooltip_target.as_deref() != Some(path.as_path()) {
                    self.tooltip_target = Some(path);
                    self.dirty = true;
                }
            }
            None => {
                self.hover_file = Some((path, std::time::Instant::now()));
                self.hide_tooltip();
            }
        }
    }

    /// Hide the file tooltip if one is showing (marking the shell for rebuild).
    fn hide_tooltip(&mut self) {
        if self.tooltip_target.take().is_some() {
            self.dirty = true;
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
                self.select_new_node(node);
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

    // ---- modal dialogs ----

    /// Open a modal. A modal is exclusive: any menu, picker, or tooltip is
    /// dismissed so nothing lingers under the backdrop.
    fn open_modal(&mut self, modal: modal::Modal) {
        self.context_menu = None;
        self.picker = None;
        self.tooltip_target = None;
        self.modal = Some(modal);
        self.dirty = true;
    }

    /// Close the open modal (discarding any draft).
    fn close_modal(&mut self) {
        if self.modal.take().is_some() {
            self.dirty = true;
        }
    }

    /// Whether the open modal, if any, can be dismissed without a button.
    fn modal_closable(&self) -> bool {
        self.modal.as_ref().is_some_and(|m| m.closable)
    }

    /// Open the settings overlay, seeded from the current view toggles and snap.
    fn open_settings_modal(&mut self) {
        let draft =
            modal::SettingsDraft::new(self.show_grid, self.show_axes, self.tidy_on_save, self.snap);
        self.open_modal(modal::Modal::settings(draft));
    }

    /// A press while a modal is open: focus a clicked input / arm a button, and
    /// (for a closable modal) dismiss when the press lands on the backdrop rather
    /// than the card. Replaces the shell's own press handling entirely.
    fn handle_modal_press(&mut self) {
        self.ui.handle_input(InputEvent::PointerPressed);
        if !self.modal_closable() {
            return;
        }
        let hit = self.ui.hit_test(self.cursor);
        let in_card = self.rows.modal_card.is_some_and(|card| {
            hit == Some(card) || hit.is_some_and(|h| self.ui.is_within(h, card))
        });
        if !in_card {
            self.close_modal();
        }
    }

    /// Run the default footer action (what Enter confirms).
    fn confirm_modal_default(&mut self) {
        if let Some(action) = self.modal.as_ref().map(|m| m.default_action()) {
            self.run_modal_action(action);
        }
    }

    /// Dispatch a clicked modal footer button.
    fn run_modal_action(&mut self, action: ModalAction) {
        match action {
            ModalAction::Close => self.close_modal(),
            ModalAction::SaveSettings => self.save_settings_from_modal(),
            ModalAction::SaveThenProceed => {
                let quitting = matches!(
                    self.modal.as_ref().map(|m| &m.body),
                    Some(modal::ModalBody::Confirm(c)) if c.pending == modal::Pending::Quit
                );
                if quitting && self.is_modified() {
                    self.save_project();
                }
                if self.script_dirty() {
                    self.save_script();
                }
                // A failed save opens its own modal saying why; going ahead
                // then would throw away the work the save was meant to keep.
                if !self.script_dirty() {
                    self.proceed_with_pending();
                }
            }
            ModalAction::DiscardAndProceed => {
                self.script_modified = false;
                self.proceed_with_pending();
            }
        }
    }

    /// Do the thing the unsaved-changes question was guarding.
    fn proceed_with_pending(&mut self) {
        let pending = match self.modal.as_ref().map(|m| &m.body) {
            Some(modal::ModalBody::Confirm(c)) => c.pending.clone(),
            _ => return,
        };
        self.close_modal();
        match pending {
            modal::Pending::Quit => self.quit_requested = true,
            modal::Pending::OpenScript(path) => self.open_script_file(&path),
        }
    }

    /// Keep the open script's path pointing at the file it is actually in.
    ///
    /// `to` is where the file went, or `None` if it is gone. A deleted file
    /// leaves the buffer open and orphaned rather than closing it: the text is
    /// still the user's, and losing it because a file was deleted in another
    /// pane would be worse than a header that says the file is missing.
    fn follow_open_script(&mut self, from: &std::path::Path, to: Option<&std::path::Path>) {
        let Some(open) = self.open_script.clone() else {
            return;
        };
        let current = self.project_dir.join(&open);
        if current != from {
            return;
        }
        match to.and_then(|p| self.explorer.project_relative(p)) {
            Some(relative) => self.open_script = Some(relative),
            None => {
                self.open_script = None;
                self.script_orphaned = true;
                // The text is still unsaved work; keep it and say so.
                self.script_modified = true;
            }
        }
        self.dirty = true;
    }

    /// Whether dropping the open buffer now would lose work.
    fn script_dirty(&self) -> bool {
        self.open_script.is_some() && self.script_modified
    }

    /// Whether quitting right now would throw away unsaved work of any kind.
    fn close_would_lose_work(&self) -> bool {
        self.script_dirty() || self.is_modified()
    }

    /// Ask before closing the window over unsaved work.
    fn ask_before_quitting(&mut self) {
        let what = match (self.script_dirty(), self.is_modified()) {
            (true, true) => "The scene and the open script have unsaved changes.",
            (true, false) => "The open script has unsaved changes.",
            _ => "The scene has unsaved changes.",
        };
        self.open_modal(modal::Modal::confirm(
            format!("{what} Save before closing?"),
            modal::Pending::Quit,
        ));
    }

    /// Apply and persist the settings draft, folding in each numeric input's live
    /// text first (so an un-submitted edit is not lost), then close.
    fn save_settings_from_modal(&mut self) {
        let Some(mut draft) = self.modal.as_ref().and_then(modal::Modal::settings_draft) else {
            self.close_modal();
            return;
        };
        for (id, field) in self.rows.modal_inputs.clone() {
            if let WidgetKind::TextInput(text) = self.ui.kind(id)
                && let Ok(value) = text.trim().parse::<f32>()
            {
                draft.set_number(field, value);
            }
        }
        self.show_grid = draft.show_grid;
        self.show_axes = draft.show_axes;
        self.tidy_on_save = draft.tidy_on_save;
        self.snap = draft.snap();
        self.persist_view_prefs();
        self.close_modal();
    }

    // ---- asset fields ----

    /// Set an inspector asset (`Value::Asset`) field to a project-relative path,
    /// through history (one undo step). Marks dirty so the field's preview and
    /// the sprite both refresh. A no-op if the value is unchanged or the field is
    /// not an asset.
    fn commit_asset_field(&mut self, node: NodeId, component: usize, field: &str, path: String) {
        let old = self
            .project
            .scene
            .node(node)
            .components
            .get(component)
            .and_then(|c| c.as_reflect().get(field));
        if let Some(Value::Asset(current)) = old
            && current != path
        {
            self.history.set_field(
                &mut self.project.scene,
                node,
                component,
                field,
                Value::Asset(path),
            );
            // A different script exports different variables, so the fields
            // the inspector shows change with it.
            self.reconcile_script_exports();
        }
        self.dirty = true;
    }

    /// Open the chooser for an asset field: every asset of the kind this field
    /// can actually hold, in a grid. Which kind comes from the component that
    /// owns the field, not from the field's name - a Script's source takes a
    /// `.cmt`, and offering it the project's textures would be a dead end.
    fn open_asset_chooser(&mut self, node: NodeId, component: usize, field: &str) {
        let kind = self.asset_kind_of(node, component);
        let assets = match kind {
            modal::AssetKind::Image => self.explorer.all_images(),
            modal::AssetKind::Script => self.explorer.all_scripts(),
        };
        let target = modal::AssetTarget {
            node,
            component,
            field: field.to_string(),
        };
        self.open_modal(modal::Modal::asset_chooser(target, assets, kind));
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
        let Some((_, field, axis)) =
            hit.and_then(|id| self.rows.scrub_labels.iter().find(|(w, ..)| *w == id))
        else {
            return false;
        };
        let (field, axis) = (field.clone(), *axis);
        let original = field_vec2(&self.project.scene, &field);
        self.scrub = Some(ScrubDrag {
            field,
            axis,
            original,
            start_x: self.cursor.x,
        });
        true
    }

    /// The scrub sensitivity (value change per horizontal pixel) for a field:
    /// scale is fine-grained, positions and sizes move a pixel per pixel.
    fn scrub_step(field: &FieldRef) -> f32 {
        match field {
            FieldRef::Scale(_) => 0.01,
            _ => 1.0,
        }
    }

    /// Apply the live scrub value for the current cursor (called on move):
    /// the pressed axis component moves by the horizontal drag since the press.
    fn apply_scrub(&mut self) {
        let Some(scrub) = self.scrub.clone() else {
            return;
        };
        let delta = (self.cursor.x - scrub.start_x) * Self::scrub_step(&scrub.field);
        let base = crate::ui::axis_of(scrub.original, scrub.axis);
        let value = with_axis(scrub.original, scrub.axis, base + delta);
        set_field_vec2(&mut self.project.scene, &scrub.field, value);
        // Update the scrubbed input in place rather than rebuilding every move
        // (the scene re-renders live regardless); see sync_inspector_transform.
        self.sync_vec_input(&scrub.field, scrub.axis);
    }

    /// Finish a slider drag: rewind to the value it had when the drag began and
    /// commit the final one through the history, so the whole drag is one undo
    /// step rather than one per pixel.
    fn end_slider_drag(&mut self) {
        let Some(drag) = self.slider_drag.take() else {
            return;
        };
        let reflect = self.project.scene.node(drag.node).components[drag.component].as_reflect();
        let current = reflect.get(&drag.field);
        if current == drag.original {
            return;
        }
        if let (Some(original), Some(current)) = (drag.original, current) {
            self.project.scene.node_mut(drag.node).components[drag.component]
                .as_reflect_mut()
                .set(&drag.field, original);
            self.history.set_field(
                &mut self.project.scene,
                drag.node,
                drag.component,
                &drag.field,
                current,
            );
        }
        self.dirty = true;
    }

    /// Finish a scrub: rewind to the press-time value and commit the final one
    /// through the history, so the whole drag is ONE undo step.
    fn end_scrub(&mut self) {
        let Some(scrub) = self.scrub.take() else {
            return;
        };
        let current = field_vec2(&self.project.scene, &scrub.field);
        if current != scrub.original {
            set_field_vec2(&mut self.project.scene, &scrub.field, scrub.original);
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
                    .set_field(scene, node, index, &field, Value::Vec2(v));
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

    /// Start a tab reorder drag if the press landed on a tab. Returns whether
    /// it did, so the caller can let the press fall through otherwise.
    fn press_tab_bar(&mut self) -> bool {
        let cursor = self.cursor;
        self.tab_bars
            .resize_with(self.rows.dock_tab_bars.len(), Default::default);
        for (group, rows) in self.rows.dock_tab_bars.iter().enumerate() {
            let Some(index) = self.tab_bars[group].press(&self.ui, rows, cursor) else {
                continue;
            };
            // Where in the tab it was grabbed. Captured now, while the pointer is
            // genuinely on the tab: by the time it is dragged out of the bar the
            // cursor has already left, and measuring then put the carried copy
            // that far off the pointer for the rest of the drag.
            if let Some(rect) = rows.tabs.get(index).and_then(|&t| self.ui.rect(t)) {
                self.carry_grab = cursor - rect.pos;
            }
            return true;
        }
        false
    }

    /// Advance a live tab drag. This only rearranges how the tabs are *shown* -
    /// nothing is committed and the shell is not rebuilt, because a rebuild
    /// would drop Aurora's retained drag and the tab could no longer be pulled
    /// out to another group afterwards. The move is applied on release.
    fn drag_tab_bar(&mut self) {
        let cursor = self.cursor;
        for group in 0..self.tab_bars.len() {
            if !self.tab_bars[group].is_dragging() {
                continue;
            }
            let Some(rows) = self.rows.dock_tab_bars.get(group) else {
                continue;
            };
            let outcome = self.tab_bars[group].drag_to(&self.ui, rows, cursor);
            // Dragging clear of the strip lifts the pane out of the dock right
            // away, so the remaining panes reflow and you can see where it would
            // land. Nothing is committed: the shell is simply built without it
            // until the drop (or until the gesture is abandoned).
            if outcome == Some(aurora::TabDrag::TornOut)
                && self.carrying.is_none()
                && self.dock.panes().len() > 1
                && let Some(&tab) = rows.tabs.first()
                && let Some(pane) = self.dock_tab_pane(tab)
            {
                let held = self.tab_bars[group]
                    .held_tab(rows)
                    .and_then(|t| self.dock_tab_pane(t));
                self.carrying = held.or(Some(pane));
                // The pane is about to leave this bar, so the bar's own drag is
                // over: its held index would otherwise point at whichever tab
                // shifts into that slot.
                self.tab_bars[group].cancel();
                self.dirty = true;
            }
        }
    }

    /// End any tab drag, committing a rearrangement the bar previewed. Records
    /// that the drop was consumed, so the dock does not also treat the release
    /// as a re-dock onto the group the tab already belongs to.
    fn release_tab_bars(&mut self) {
        // A carried pane lands wherever the pointer let go; if that is nowhere
        // valid, dropping the carry alone restores it to where it came from,
        // since the real dock was never changed.
        if let Some(pane) = self.carrying.take() {
            for bar in &mut self.tab_bars {
                bar.set_insertion(None);
            }
            for rows in &self.rows.dock_tab_bars {
                if let Some(bar) = rows.bar {
                    self.ui.set_background(bar, self.theme.aurora.bar_bg);
                }
            }
            if let Some((_, target, slot)) = self.tab_drop_target() {
                // Merging into the group it already lives in is a no-op in
                // move_pane, which is right: dropping a tab back on its own bar
                // should only change its position, which the reorder below does.
                self.dock.move_pane(pane, target, DropZone::Tab);
                if let Some(from) = self.dock.index_in_group(pane) {
                    // A slot past the end simply leaves it last.
                    self.dock.reorder_in_group(pane, from, slot);
                }
                // It should settle from where it was dropped, not appear there.
                self.arriving = Some((pane, self.cursor.x - self.carry_grab.x));
                self.dirty = true;
                self.view_dirty = true;
            } else {
                self.drop_dock_tab(pane);
            }
            for bar in &mut self.tab_bars {
                bar.release();
            }
            self.dirty = true;
            return;
        }
        for group in 0..self.tab_bars.len() {
            let Some(reorder) = self.tab_bars[group].release() else {
                continue;
            };
            let Some(&tab) = self
                .rows
                .dock_tab_bars
                .get(group)
                .and_then(|rows| rows.tabs.first())
            else {
                continue;
            };
            if let Some(pane) = self.dock_tab_pane(tab)
                && self.dock.reorder_in_group(pane, reorder.from, reorder.to)
            {
                self.dirty = true;
            }
        }
    }

    /// Whether a tab is being dragged along its bar right now.
    fn tab_drag_active(&self) -> bool {
        self.tab_bars.iter().any(|b| b.is_dragging())
    }

    /// Advance each tab bar's animation. Called after layout, when the tabs'
    /// settled positions are known.
    fn tick_tab_bars(&mut self, dt: f32) {
        let cursor = self.cursor;
        // A tab that has just arrived from another bar slides in from where it
        // was dropped. Its group is only known now, after the rebuild.
        if let Some((pane, from_x)) = self.arriving {
            let found = self
                .rows
                .dock_tab_bars
                .iter()
                .enumerate()
                .find_map(|(g, rows)| {
                    let slot = rows
                        .tabs
                        .iter()
                        .position(|&t| self.dock_tab_pane(t) == Some(pane))?;
                    Some((g, slot))
                });
            if let Some((group, slot)) = found {
                self.tab_bars
                    .resize_with(self.rows.dock_tab_bars.len(), Default::default);
                self.tab_bars[group].slide_in(slot, from_x);
                self.arriving = None;
            }
        }
        self.tab_bars
            .resize_with(self.rows.dock_tab_bars.len(), Default::default);
        for (group, rows) in self.rows.dock_tab_bars.iter().enumerate() {
            self.tab_bars[group].tick(dt, &mut self.ui, rows, cursor);
        }
    }

    /// The pane a dock tab (by widget) shows, if `id` is one.
    fn dock_tab_pane(&self, id: aurora::WidgetId) -> Option<Pane> {
        self.rows
            .dock_tabs
            .iter()
            .find(|(w, _)| *w == id)
            .map(|(_, p)| *p)
    }

    /// The tab strip under the cursor, as (group index, a pane of that group,
    /// the slot the carried tab would land in). Dropping here merges the pane
    /// into that group rather than splitting it.
    fn tab_drop_target(&self) -> Option<(usize, Pane, usize)> {
        for (group, rows) in self.rows.dock_tab_bars.iter().enumerate() {
            let Some(rect) = rows.bar.and_then(|b| self.ui.rect(b)) else {
                continue;
            };
            if !strip_band(rect).contains(self.cursor) {
                continue;
            }
            let pane = rows.tabs.first().and_then(|&t| self.dock_tab_pane(t))?;
            let slot = self.tab_bars[group].drop_slot(&self.ui, rows, self.cursor);
            return Some((group, pane, slot));
        }
        None
    }

    /// While carrying a pane, hold a gap open in whichever strip it is over and
    /// lift that strip, so it is clear which bar would receive the tab.
    fn update_carry_preview(&mut self) {
        let target = self.tab_drop_target();
        for group in 0..self.tab_bars.len() {
            let insertion = match target {
                Some((g, _, slot)) if g == group => Some((slot, CARRIED_TAB_W)),
                _ => None,
            };
            self.tab_bars[group].set_insertion(insertion);
            // Lift the target strip so it is obvious which bar would receive the
            // tab. set_background edits the tree in place, so this costs no
            // rebuild - which a live drag could not survive anyway.
            if let Some(rows) = self.rows.dock_tab_bars.get(group)
                && let Some(bar) = rows.bar
            {
                let base = self.theme.aurora.bar_bg;
                let accent = self.theme.aurora.focus;
                let fill = if insertion.is_some() {
                    mix(base, accent, 0.22)
                } else {
                    base
                };
                self.ui.set_background(bar, fill);
            }
        }
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
        // Merging as a tab is aimed at the tab strip now (see tab_drop_target),
        // so anywhere in a pane's body splits it - the nearest edge wins.
        if nearest == left {
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
        if self.modal.is_some() {
            return;
        }
        let original = field_color(&self.project.scene, &target);
        self.picker = Some(ColorPicker {
            target,
            original,
            picker: AuroraPicker::new(self.picker_images, self.cursor, original),
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
        let final_rgba = picker.picker.rgba();
        if final_rgba != picker.original {
            set_field_color(&mut self.project.scene, &picker.target, picker.original);
            self.history.set_field(
                &mut self.project.scene,
                picker.target.node,
                picker.target.index,
                &picker.target.field,
                Value::Color(final_rgba),
            );
        }
        self.dirty = true;
    }

    /// Upload whatever gradients aurora reports as changed since the last call.
    fn refresh_picker_images(&mut self) {
        let Some(open) = self.picker.as_mut() else {
            return;
        };
        for (handle, bitmap) in open.picker.dirty_bitmaps() {
            self.gui
                .update_image_rgba(handle, &bitmap.rgba, bitmap.width, bitmap.height);
        }
    }

    /// A left press with the picker open: dismiss it (committing) if the press
    /// is outside its popup, else start a gradient drag if on one. Returns
    /// whether it consumed the press.
    fn handle_picker_press(&mut self) -> bool {
        let (ui, rows, cursor) = (&self.ui, self.rows.picker, self.cursor);
        let Some(open) = self.picker.as_mut() else {
            return false;
        };
        match open.picker.press(ui, &rows, cursor) {
            Press::Grabbed(_) => self.apply_picker_drag(),
            Press::Inside => {}
            Press::Outside => self.close_color_picker(),
        }
        true
    }

    /// Apply the current cursor to the active gradient drag and push the new
    /// color to the scene live (committed as one undo step when the picker
    /// closes).
    fn apply_picker_drag(&mut self) {
        let (ui, rows, cursor) = (&self.ui, self.rows.picker, self.cursor);
        let Some(open) = self.picker.as_mut() else {
            return;
        };
        if !open.picker.drag_to(ui, &rows, cursor) {
            return;
        }
        let (target, rgba) = (open.target.clone(), open.picker.rgba());
        set_field_color(&mut self.project.scene, &target, rgba);
        self.refresh_picker_images();
        self.dirty = true;
    }

    /// End a picker gradient drag (the picker stays open).
    fn end_picker_drag(&mut self) {
        if let Some(picker) = self.picker.as_mut() {
            picker.picker.end_drag();
        }
    }

    /// Commit the picker's hex input into its working color.
    fn commit_picker_hex(&mut self, id: aurora::WidgetId) {
        if self.rows.picker.hex != Some(id) {
            return;
        }
        let WidgetKind::TextInput(text) = self.ui.kind(id) else {
            return;
        };
        let Some(picker) = self.picker.as_mut() else {
            return;
        };
        if !picker.picker.commit_hex(text.as_str()) {
            return;
        }
        let (target, rgba) = (picker.target.clone(), picker.picker.rgba());
        set_field_color(&mut self.project.scene, &target, rgba);
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
        // With the code editor focused, ctrl+s saves the script rather than the
        // scene: it is the file you are looking at. Only without shift, which
        // sorts the selected lines below.
        if c.eq_ignore_ascii_case("s") && !self.modifiers.shift_key() {
            if self.ui.focused() == self.rows.code_editor && self.open_script.is_some() {
                self.save_script();
            } else {
                self.save_project();
            }
            return true;
        }
        // The three actions that used to be toolbar buttons. The toolbar is now
        // what the pointer is doing and what the viewport shows, so these live
        // on the keyboard - and none of them was reachable any other way, which
        // is why they are here rather than simply deleted.
        if c.eq_ignore_ascii_case("o") && !self.code_context_focused() {
            self.load_project();
            return true;
        }
        if self.modifiers.shift_key() && !self.code_context_focused() {
            if c.eq_ignore_ascii_case("n") {
                self.add_sprite_action();
                return true;
            }
            if c.eq_ignore_ascii_case("m") {
                self.add_script_action();
                return true;
            }
        }
        // Only when the code editor or the find bar itself has focus. It used
        // to fire whenever a Code pane existed at all, so ctrl+f while renaming
        // a node or filtering the scene tree opened a find bar over the editor.
        if c.eq_ignore_ascii_case("f") && self.code_context_focused() {
            self.toggle_find();
            return true;
        }
        // ctrl+m jumps to the matching bracket - the other half of highlighting
        // it, and how you get from the top of a long block to its end.
        if c.eq_ignore_ascii_case("m")
            && let Some(editor) = self.rows.code_editor
            && self.ui.focused() == Some(editor)
        {
            if let Some(at) = self.ui.caret_offset()
                && let Some(partner) = self.analysis.bracket_at(at).and_then(|b| b.partner)
            {
                self.ui.focus_caret(editor, partner.end as usize);
                self.dirty = true;
            }
            return true;
        }
        // With the code editor focused, ctrl+z/y walk the script's own text
        // history rather than the scene's - undoing a typo should not also undo
        // moving a sprite. The line commands live here for the same reason:
        // ctrl+d keeps meaning "duplicate node" everywhere else in the editor.
        if self.ui.focused() == self.rows.code_editor && self.rows.code_editor.is_some() {
            if c == "/" {
                self.toggle_comment();
                return true;
            }
            if c.eq_ignore_ascii_case("d") {
                self.duplicate_lines();
                return true;
            }
            if c.eq_ignore_ascii_case("k") {
                self.delete_lines();
                return true;
            }
            // ctrl+shift+j joins lines, the inverse of Enter. ctrl+j alone is
            // left free: it is a newline on some terminals and the surprise
            // would be expensive.
            if c.eq_ignore_ascii_case("j") && self.modifiers.shift_key() {
                self.join_lines();
                return true;
            }
            if c.eq_ignore_ascii_case("t") {
                self.transpose_chars();
                return true;
            }
            // ctrl+shift+u upper, ctrl+shift+l lower, ctrl+shift+y title. The
            // result stays selected so the three can be tried in sequence,
            // which is how this actually gets used. Plain ctrl+y is still redo:
            // this arm requires shift.
            if self.modifiers.shift_key()
                && let Some(case) = match c.to_ascii_lowercase().as_str() {
                    "u" => Some(Case::Upper),
                    "l" => Some(Case::Lower),
                    "y" => Some(Case::Title),
                    _ => None,
                }
            {
                self.change_case(case);
                return true;
            }
            // ctrl+shift+s sorts the selected lines; with alt, dropping
            // duplicates. Plain ctrl+s is save and is handled far above.
            if c.eq_ignore_ascii_case("s") && self.modifiers.shift_key() {
                self.sort_lines(self.modifiers.alt_key());
                return true;
            }
            if c.eq_ignore_ascii_case("g") {
                self.open_go_to_line();
                return true;
            }
            // ctrl+shift+o: jump to a declaration by name. ctrl+g is by line
            // number, which is what you have when a diagnostic told you one.
            if c.eq_ignore_ascii_case("o") {
                self.open_symbol_palette();
                return true;
            }
            if c.eq_ignore_ascii_case("z") {
                self.script_history(self.modifiers.shift_key());
                return true;
            }
            if c.eq_ignore_ascii_case("y") {
                self.script_history(true);
                return true;
            }
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
            // With nothing selected, copy and cut take the whole line. Reaching
            // for ctrl+X to move a line and getting nothing is the single most
            // common wasted keystroke in an editor without this.
            "c" | "C" => {
                if let Some(text) = self.ui.selected_text() {
                    self.set_clipboard(&text);
                } else if let Some((_, line)) = self.ui.current_line() {
                    self.set_clipboard(&line);
                }
            }
            "x" | "X" => {
                if let Some(text) = self.ui.selected_text() {
                    self.set_clipboard(&text);
                    self.ui.delete_selection();
                } else if let Some((range, line)) = self.ui.current_line() {
                    self.set_clipboard(&line);
                    if let Some(id) = self.ui.focused() {
                        let start = range.start;
                        self.ui.replace_range(id, range, "");
                        self.ui.focus_caret(id, start);
                    }
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

    /// Handle an Aurora `Dropped`: a file dragged out of the explorer, released
    /// somewhere in the editor. What it does depends on what was dragged - an
    /// image makes or retextures a sprite, a script gives a node something to
    /// run - so the file's kind picks the route, and a drop that would write a
    /// path a field can never use is refused rather than committed.
    fn drop_file(&mut self, source: aurora::WidgetId, target: Option<aurora::WidgetId>) {
        // Resolve the dragged explorer entry to its project-relative path.
        let Some(path) = self
            .rows
            .file_rows
            .iter()
            .find(|(widget, _)| *widget == source)
            .map(|(_, path)| path.clone())
        else {
            return;
        };
        match explorer::classify(Path::new(&path)) {
            explorer::FileKind::Script => self.drop_script(path, target),
            _ => self.drop_image(path, target),
        }
    }

    /// An image dropped on the viewport spawns a sprite there; dropped on an
    /// image asset field, it sets that field.
    fn drop_image(&mut self, path: String, target: Option<aurora::WidgetId>) {
        if target == self.rows.viewport {
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
            self.select_new_node(node);
            return;
        }
        if let Some((node, component, field)) = self.asset_field_at(target, modal::AssetKind::Image)
        {
            self.commit_asset_field(node, component, &field, path);
        }
    }

    /// A script dropped on a node - its row in the scene tree, its sprite in the
    /// viewport, or its script field in the inspector - points that node at the
    /// script. Dropped anywhere else it does nothing: a script with no node to
    /// move has nothing to be.
    fn drop_script(&mut self, path: String, target: Option<aurora::WidgetId>) {
        if let Some((node, component, field)) =
            self.asset_field_at(target, modal::AssetKind::Script)
        {
            self.commit_asset_field(node, component, &field, path);
            return;
        }
        // The scene tree runs its own reparent drag rather than aurora drop
        // targets, so a row is found under the cursor rather than in `target`.
        if let Some(node) = self.tree_row_at_cursor() {
            self.set_node_script(node, path);
            return;
        }
        // The viewport turns script drags down (a script lands on a node, not on
        // the pane), so the release names no target - the outlined node is what
        // it landed on. Deliberately the same test that drew the outline, not
        // the finer GPU pick: dropping somewhere other than where the cue said
        // is worse than ignoring per-pixel transparency.
        if let Some(node) = self.script_drop_node() {
            self.set_node_script(node, path);
        }
    }

    /// The node a script drag hovering the viewport would land on, or `None`
    /// when no script is being dragged or the cursor is over empty space. Uses
    /// the cheap CPU quad test rather than the GPU pick, because this runs every
    /// frame the drag is live.
    fn script_drop_node(&self) -> Option<NodeId> {
        let source = self.ui.drag_source()?;
        let (_, path) = self.rows.file_rows.iter().find(|(w, _)| *w == source)?;
        if explorer::classify(Path::new(path)) != explorer::FileKind::Script {
            return None;
        }
        if self.ui.hit_test(self.cursor) != self.rows.viewport {
            return None;
        }
        let world = self.cursor_world()?;
        let order: Vec<NodeId> = self
            .project
            .scene
            .sprite_draws()
            .iter()
            .map(|d| d.node)
            .collect();
        viewport::node_at(&self.project.scene, &order, world, self.camera.zoom)
    }

    /// The asset field under `target`, if there is one and it holds `want`.
    /// Refusing the mismatch is the point: a Script's source cannot use a `.png`
    /// and a Sprite's texture cannot use a `.cmt`, and silently writing either
    /// leaves a field that looks set but can never load.
    fn asset_field_at(
        &self,
        target: Option<aurora::WidgetId>,
        want: modal::AssetKind,
    ) -> Option<(NodeId, usize, String)> {
        let target = target?;
        let (_, node, component, field) =
            self.rows.asset_fields.iter().find(|(w, ..)| *w == target)?;
        let (node, component) = (*node, *component);
        (self.asset_kind_of(node, component) == want).then_some((node, component, field.clone()))
    }

    /// Which kind of asset a component's fields hold. Asked of the component
    /// rather than derived from a field name, so the answer stays right as
    /// components grow fields.
    fn asset_kind_of(&self, node: NodeId, component: usize) -> modal::AssetKind {
        match self
            .project
            .scene
            .node(node)
            .components
            .get(component)
            .map(|c| c.as_reflect().type_name())
        {
            Some("Script") => modal::AssetKind::Script,
            _ => modal::AssetKind::Image,
        }
    }

    /// Point `node`'s script at `path`, attaching a Script component first if it
    /// has none. One undo step either way, so dropping a script on a bare node
    /// undoes in one press rather than leaving an empty component behind.
    fn set_node_script(&mut self, node: NodeId, path: String) {
        self.history.begin_group();
        // An empty script component is a slot waiting for a file; anything else
        // gets its own, because a node may run several.
        let index = actions::empty_script_index(&self.project.scene, node).unwrap_or_else(|| {
            actions::attach_script(&mut self.project.scene, &mut self.history, node)
        });
        self.history.set_field(
            &mut self.project.scene,
            node,
            index,
            "source",
            Value::Asset(path),
        );
        self.history.end_group(&mut self.project.scene);
        // The script's exported variables are what the inspector shows for it,
        // and they come from the file that was just pointed at. Without this
        // the fields appeared only after the script was next saved.
        self.reconcile_script_exports();
        self.selection.select_one(node);
        self.explorer_focused = false;
        self.dirty = true;
    }

    /// Select a just-created scene node and move keyboard focus to the scene, so
    /// a following Delete/Duplicate acts on the node - not on whatever file was
    /// last focused in the explorer (e.g. the PNG that was dragged in to place
    /// the sprite, which otherwise kept the delete key pointed at the file).
    fn select_new_node(&mut self, node: NodeId) {
        self.selection.select_one(node);
        self.explorer_focused = false;
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
        self.select_new_node(node);
    }

    /// The toolbar's Add Script: give the selected node a Script component. The
    /// `.cmt` file is chosen afterwards in the inspector, the same way a
    /// sprite's texture is - so this needs a selection, not a viewport point.
    fn add_script_action(&mut self) {
        let Some(node) = self.selection.primary() else {
            return;
        };
        actions::attach_script(&mut self.project.scene, &mut self.history, node);
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
            // Write the authored theme back unchanged - resolving and re-authoring
            // would drop the user's variables and references.
            theme: self.theme_doc.clone(),
            show_grid: self.show_grid,
            show_axes: self.show_axes,
            tidy_on_save: self.tidy_on_save,
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
            Err(err) => {
                tracing::error!("save failed: {err}");
                self.open_modal(modal::Modal::report("Save failed", &err));
            }
        }
    }

    /// Reload the project from disk, discarding unsaved edits. The history is
    /// cleared too: its edits hold NodeIds from the old scene's arena, which
    /// mean nothing (or worse, the wrong node) in the freshly loaded one.
    fn load_project(&mut self) {
        match Project::load(&self.project_dir) {
            Ok(project) => {
                self.project = project;
                // A scene saved before a script's exports changed still holds
                // the old set; bring them into line before anything is drawn.
                self.reconcile_script_exports();
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
            Err(err) => {
                tracing::error!("load failed: {err}");
                self.open_modal(modal::Modal::report("Load failed", &err));
            }
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

    /// Ensure thumbnails for the images shown outside the explorer: the asset
    /// chooser's grid, and the selected node's asset fields (so the inspector
    /// shows a preview). A newly decoded preview marks dirty to reveal it.
    fn ensure_asset_thumbnails(&mut self) {
        let mut paths: Vec<std::path::PathBuf> = Vec::new();
        if let Some(images) = self.modal.as_ref().and_then(|m| m.asset_images()) {
            paths.extend(images.iter().map(|a| a.abs.clone()));
        }
        if let Some(node) = self.selection.primary() {
            for comp in &self.project.scene.node(node).components {
                let reflect = comp.as_reflect();
                for field in reflect.field_names() {
                    if let Some(Value::Asset(rel)) = reflect.get(&field)
                        && !rel.is_empty()
                    {
                        paths.push(self.explorer.resolve_relative(&rel));
                    }
                }
            }
        }
        // Decode a bounded number per frame so opening the chooser on a project
        // with many images streams them in over a few frames instead of one long
        // stall; `dirty` keeps the pump going until the queue drains.
        const BUDGET: usize = 8;
        let mut decoded = 0;
        for path in paths {
            if !explorer::can_thumbnail(&path) || self.thumbnails.contains(&path) {
                continue;
            }
            if decoded >= BUDGET {
                self.dirty = true;
                break;
            }
            if self.thumbnails.ensure(&mut self.gui, &path).is_some() {
                self.dirty = true;
            }
            decoded += 1;
        }
    }

    /// Capture the explorer's live layout back into its state: the folder tree's
    /// scroll (so it survives rebuilds) and the grid column count derived from
    /// the contents pane's laid-out width (a change marks dirty so it reflows).
    fn sync_explorer_view(&mut self) {
        if let Some(id) = self.rows.file_tree_scroll {
            let offset = self.ui.scroll_offset(id);
            self.explorer.set_tree_scroll(offset);
            // Capture the tree's width back from its splitter so a resize sticks
            // across rebuilds (mirrors how the dock captures its splitter sizes).
            if let Some(rect) = self.ui.rect(id) {
                self.explorer.set_tree_width(rect.size.x);
            }
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
        // Double-clicking a script opens it in the Code pane, and brings that
        // pane to the front - opening a file into a tab you cannot see is the
        // same as not opening it.
        if double && explorer::classify(path) == explorer::FileKind::Script {
            self.open_script_file(path);
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

    /// Open `path` in the Code pane, reading it from disk. A read failure is
    /// reported rather than leaving a stale buffer claiming to be the new file.
    fn open_script_file(&mut self, path: &std::path::Path) {
        // Replacing a buffer with unsaved edits would drop them silently.
        if self.script_dirty() {
            let name = self.open_script.clone().unwrap_or_default();
            self.open_modal(modal::Modal::confirm(
                format!("{name} has unsaved changes. Save them before opening another script?"),
                modal::Pending::OpenScript(path.to_path_buf()),
            ));
            return;
        }
        let raw = match std::fs::read(path) {
            Ok(raw) => raw,
            Err(err) => {
                self.open_modal(modal::Modal::error("Cannot open script", &err));
                return;
            }
        };
        // Remember where the outgoing file was being read, before anything about
        // it is replaced.
        self.remember_script_position();
        // A file that is not UTF-8 is refused by name rather than opened as
        // replacement characters and then saved back over the original.
        let (text, encoding) = match TextEncoding::decode(&raw) {
            Some(decoded) => decoded,
            None => {
                self.open_modal(modal::Modal::report(
                    "Cannot open script",
                    format!("{} is not valid UTF-8 text", path.display()),
                ));
                return;
            }
        };
        self.script_encoding = encoding;
        let relative = self
            .explorer
            .project_relative(path)
            .unwrap_or_else(|| path.display().to_string());
        self.open_script = Some(relative);
        self.script_orphaned = false;
        self.script_text = text;
        self.script_modified = false;
        self.adopt_open_script();
        self.dock.activate(dock::Pane::Code);
        self.dirty = true;
    }

    /// Make the Code pane show `script_text` after the app has replaced it
    /// wholesale, and forget everything that belonged to the previous file.
    ///
    /// `sync_script_buffer` treats the widget as the source of truth and the
    /// mirror as following it. That is right while someone is typing, and
    /// exactly wrong here: the app has just swapped the buffer and the widget
    /// is the stale one. Without pushing the text across, the sync that runs
    /// just before the rebuild copied the previous file's text back over the
    /// one being opened - so the title bar changed and the content did not.
    ///
    /// The history goes with it. Undo steps hold whole-buffer snapshots, so a
    /// step left over from the last file would not just look wrong, it would
    /// put that file's text into this one - and the next save would write it.
    fn adopt_open_script(&mut self) {
        if let Some(editor) = self.rows.code_editor {
            self.ui.set_text_input(editor, self.script_text.clone());
        }
        self.script_undo.clear();
        self.script_redo.clear();
        // Back where this file was left, if it has been open this session.
        let (caret, _) = self
            .open_script
            .as_ref()
            .and_then(|name| self.script_positions.get(name).copied())
            .unwrap_or((0, 0.0));
        self.script_caret = caret.min(self.script_text.len());
        self.focus_editor = true;
        self.script_coalescing = false;
        // A completion list and a find bar are both about the buffer that just
        // went away.
        self.completions = None;
        if let Some(find) = &mut self.find {
            find.refresh(&self.script_text);
        }
        self.analyze_script();
    }

    /// Bring every Script component's stored values into line with what its
    /// source now exports: keep what is still declared at the same type, drop
    /// what is gone, add what is new at its default.
    ///
    /// Called when a script's path changes and when a script is saved, rather
    /// than watched continuously - a live file watcher is M5's, along with the
    /// reload it would drive. Returns whether anything changed, so a caller can
    /// decide whether the shell needs rebuilding.
    fn reconcile_script_exports(&mut self) -> bool {
        fn collect(
            scene: &helios::Scene,
            node: helios::NodeId,
            out: &mut Vec<(helios::NodeId, usize, String)>,
        ) {
            for (index, component) in scene.node(node).components.iter().enumerate() {
                if let helios::Component::Script(script) = component
                    && !script.source.is_empty()
                {
                    out.push((node, index, script.source.clone()));
                }
            }
            for child in scene.children(node) {
                collect(scene, *child, out);
            }
        }
        let mut sources = Vec::new();
        collect(&self.project.scene, self.project.scene.root(), &mut sources);

        let mut changed = false;
        for (node, index, source) in sources {
            let Ok(text) = std::fs::read_to_string(self.project_dir.join(&source)) else {
                continue;
            };
            let exported = comet::exports(&text, helios::script_schema());
            let hints: Vec<(String, Vec<comet::Hint>)> = exported
                .iter()
                .filter(|e| !e.hints.is_empty())
                .map(|e| (e.name.clone(), e.hints.clone()))
                .collect();
            let declared: Vec<(String, helios::Value)> = exported
                .into_iter()
                .filter_map(|e| Some((e.name, default_for(e.ty)?)))
                .collect();
            let helios::Component::Script(script) =
                &mut self.project.scene.node_mut(node).components[index]
            else {
                continue;
            };
            let before = (script.exports.clone(), script.hints.clone());
            script.reconcile(&declared);
            script.hints = hints;
            changed |= (script.exports.clone(), script.hints.clone()) != before;
        }
        changed
    }

    /// Write the Code pane's buffer back to its file, in the encoding it came
    /// in: the line endings it had, and its byte-order mark if it had one.
    fn save_script(&mut self) {
        let Some(relative) = self.open_script.clone() else {
            return;
        };
        if self.tidy_on_save {
            self.tidy_script();
        }
        let path = self.project_dir.join(&relative);
        let body = self.script_encoding.encode(&self.script_text);
        match write_atomic(&path, body.as_bytes()) {
            Ok(()) => {
                self.script_modified = false;
                // The file just changed, so what it exports may have too.
                self.reconcile_script_exports();
                self.dirty = true;
            }
            Err(err) => self.open_modal(modal::Modal::error("Cannot save script", &err)),
        }
    }

    /// Strip trailing whitespace and fix the file's ending, in the buffer.
    ///
    /// Through the widget as well as the mirror: writing only to disk would
    /// leave the two disagreeing, and the modified mark would come straight
    /// back on the next sync.
    fn tidy_script(&mut self) {
        let (tidied, caret) = tidy_for_save(&self.script_text, self.editor_caret());
        if tidied == self.script_text {
            return;
        }
        self.script_text = tidied;
        if let Some(editor) = self.rows.code_editor {
            self.ui.set_text_input(editor, self.script_text.clone());
            if self.ui.focused() == Some(editor) {
                self.ui.focus_caret(editor, caret);
            }
        }
        self.script_caret = caret;
        self.analyze_script();
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
        let mut first_err = None;
        for src in &clip {
            let result = if cut {
                file_ops::move_into(src, &dst)
            } else {
                file_ops::copy_into(src, &dst)
            };
            match result {
                Ok(path) => pasted.push(path),
                Err(err) => {
                    tracing::warn!("paste {} failed: {err}", src.display());
                    first_err.get_or_insert(err);
                }
            }
        }
        // Only clear a cut clipboard once something actually moved, so a fully
        // failed paste leaves it intact for a retry.
        if cut && !pasted.is_empty() {
            self.file_clip.clear();
            self.file_clip_cut = false;
        }
        self.after_file_change(&pasted);
        if let Some(err) = first_err {
            self.open_modal(modal::Modal::error("Paste failed", &err));
        }
    }

    /// Move the selected entries to the project trash (recoverable).
    fn file_delete(&mut self) {
        let sel = self.explorer.selected().to_vec();
        if sel.is_empty() {
            return;
        }
        let root = self.project_dir.clone();
        let mut moved = 0;
        let mut first_err = None;
        for path in &sel {
            match file_ops::delete_to_trash(&root, path) {
                Ok(_) => moved += 1,
                Err(err) => {
                    tracing::error!("delete {} failed: {err}", path.display());
                    first_err.get_or_insert(err);
                }
            }
        }
        if moved > 0 {
            tracing::info!("moved {moved} item(s) to {}/.orbit/trash", root.display());
            for path in &sel {
                self.follow_open_script(path, None);
            }
        }
        self.after_file_change(&[]);
        if let Some(err) = first_err {
            self.open_modal(modal::Modal::error("Delete failed", &err));
        }
    }

    /// Duplicate the selected entries beside their originals.
    fn file_duplicate(&mut self) {
        let sel = self.explorer.selected().to_vec();
        if sel.is_empty() {
            return;
        }
        let mut dups = Vec::new();
        let mut first_err = None;
        for path in &sel {
            match file_ops::duplicate(path) {
                Ok(path) => dups.push(path),
                Err(err) => {
                    tracing::warn!("duplicate {} failed: {err}", path.display());
                    first_err.get_or_insert(err);
                }
            }
        }
        self.after_file_change(&dups);
        if let Some(err) = first_err {
            self.open_modal(modal::Modal::error("Duplicate failed", &err));
        }
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
            Err(err) => {
                tracing::error!("new folder failed: {err}");
                self.open_modal(modal::Modal::error("New folder failed", &err));
            }
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
        // Only the first commit of a rename session touches the disk. A stray
        // second Submitted for the same field would otherwise try to rename `old`
        // (now gone) onto the name the first commit just created and report a
        // spurious "already exists". Clearing the renaming state first makes the
        // filesystem rename idempotent, independent of how many events arrive.
        if self.explorer.renaming().is_none() {
            return;
        }
        self.explorer.set_renaming(None);
        match file_ops::rename(old, name.trim()) {
            Ok(new_path) => {
                // If that was the open script, follow it - otherwise the next
                // save writes to a path that no longer exists and quietly
                // recreates the old file.
                self.follow_open_script(old, Some(&new_path));
                self.explorer.rescan();
                self.thumbnails.invalidate(&mut self.gui);
                self.explorer.select_one(&new_path);
            }
            Err(err) => {
                tracing::warn!("rename failed: {err}");
                self.open_modal(modal::Modal::error("Rename failed", &err));
            }
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

    /// Fold the find bar's live field text back into the bar, so a query typed
    /// but not submitted is what a step or a replace acts on.
    fn commit_find_fields(&mut self) {
        let query = self
            .rows
            .find
            .query
            .and_then(|id| self.ui.text_of(id))
            .map(str::to_string);
        let replacement = self
            .rows
            .find
            .replacement
            .and_then(|id| self.ui.text_of(id))
            .map(str::to_string);
        let text = self.script_text.clone();
        let caret = self.editor_caret();
        if let Some(find) = &mut self.find {
            if let Some(query) = query {
                find.set_query(query, &text, caret);
            }
            if let Some(replacement) = replacement {
                find.set_replacement(replacement);
            }
        }
    }

    /// The diagnostic whose span covers `offset`, if any. The narrowest one
    /// wins, so a wide "this function must return" does not hide a precise
    /// type error inside it.
    fn diagnostic_at(&self, offset: usize) -> Option<&comet::Diagnostic> {
        narrowest_at(&self.script_diagnostics, offset)
    }

    /// What to say about whatever the pointer is over in the Code pane: the
    /// diagnostic there, or failing that what the name under it is.
    ///
    /// A diagnostic wins - an error is the more urgent news about a name, and
    /// showing the type of something the compiler has already rejected reads as
    /// the editor missing the point.
    fn hover_under_pointer(&self) -> Option<String> {
        let editor = self.rows.code_editor?;
        let offset = self.ui.offset_at_point(editor, self.cursor)?;
        if let Some(d) = self.diagnostic_at(offset) {
            return Some(d.message.clone());
        }
        comet::service::hover_at(&self.script_text, helios::script_schema(), offset)
    }

    /// Move the caret to the next problem after it, or the previous one before
    /// it, wrapping. A no-op when the file is clean.
    fn step_diagnostic(&mut self, forward: bool) {
        let Some(editor) = self.rows.code_editor else {
            return;
        };
        if self.script_diagnostics.is_empty() {
            return;
        }
        let caret = self.ui.caret_offset().unwrap_or(0);
        let mut spans: Vec<(usize, usize)> = self
            .script_diagnostics
            .iter()
            .map(|d| (d.span.start as usize, d.span.end as usize))
            .collect();
        spans.sort_unstable();
        let next = if forward {
            spans
                .iter()
                .find(|(start, _)| *start > caret)
                .or_else(|| spans.first())
        } else {
            spans
                .iter()
                .rev()
                .find(|(start, _)| *start < caret)
                .or_else(|| spans.last())
        };
        if let Some(&(start, end)) = next {
            // Select the span, so the problem is visible as well as reached.
            self.ui.select_range(editor, start, end.max(start));
            self.dirty = true;
        }
    }

    /// Blink the caret, holding it solid while the caret is moving.
    ///
    /// A caret that blinks through your own typing is worse than one that does
    /// not blink at all: the phase restarts on every keystroke, so what you see
    /// while writing is a steady bar.
    fn tick_caret(&mut self) {
        let caret = self.ui.caret_offset();
        if caret != self.caret_was {
            // The caret moving is also what makes an open completion popup
            // stale: it was offering names for a word the caret has left.
            // Refreshing closes it when there is no longer a word there.
            if self.completions.is_some() {
                self.refresh_completions();
            }
            // The signature hint follows the caret whether or not anything was
            // typed: clicking or arrowing into a call is how you come back to
            // an argument you left, and that is exactly when you want it.
            self.update_signature();
            self.caret_was = caret;
            self.caret_phase = std::time::Instant::now();
            self.caret_visible = true;
        } else if self.caret_phase.elapsed() >= CARET_BLINK {
            self.caret_phase = std::time::Instant::now();
            self.caret_visible = !self.caret_visible;
        }
        self.ui.set_caret_on(self.caret_visible);
    }

    /// Keep the hovered-diagnostic tooltip in step with the pointer.
    ///
    /// Gated on the pointer being up for the reason every poller here is: a
    /// rebuild swaps the whole Ui and drops aurora's drag state, so anything
    /// that sets `dirty` mid-gesture kills the gesture.
    fn poll_diagnostic_tooltip(&mut self) {
        if self.pointer_down {
            return;
        }
        // A tooltip belongs to a resting pointer. It appears only after the
        // pointer has stopped, and anything the user does next - a key, a click,
        // a move - takes it away again. Without that it had no dismissal path at
        // all except moving off the word, and sat over the code indefinitely.
        if self.cursor != self.tooltip_cursor {
            self.tooltip_cursor = self.cursor;
            self.tooltip_rested = std::time::Instant::now();
            self.hide_hover();
            return;
        }
        if self.tooltip_rested.elapsed() < HOVER_DELAY {
            return;
        }
        let message = self.hover_under_pointer();
        if message != self.diagnostic_tooltip {
            self.diagnostic_tooltip = message;
            self.dirty = true;
        }
    }

    /// A press outside the completion popup dismisses it. Clicking elsewhere
    /// means you are done with what it was offering, and a list that survives
    /// the click sits over whatever you clicked on.
    fn dismiss_code_popups(&mut self) {
        self.hide_hover();
        if self.completions.is_none() {
            return;
        }
        let on_popup = self
            .rows
            .completions
            .rows
            .iter()
            .any(|(w, _)| self.ui.rect(*w).is_some_and(|r| r.contains(self.cursor)));
        if !on_popup {
            self.completions = None;
            self.dirty = true;
        }
    }

    /// Take the hover tooltip down, if one is up.
    fn hide_hover(&mut self) {
        if self.diagnostic_tooltip.take().is_some() {
            self.dirty = true;
        }
    }

    /// Jump to a line by number.
    ///
    /// Reuses the find bar rather than adding a dialog: a number typed into a
    /// box that already exists, with `:` as the marker, is less to build and
    /// less to learn than a second floating box that looks the same.
    fn open_go_to_line(&mut self) {
        let Some(editor) = self.rows.code_editor else {
            return;
        };
        let line = self.ui.caret_line_column().map_or(1, |(line, _)| line);
        self.go_to_line = Some(line.to_string());
        let _ = editor;
        self.dirty = true;
    }

    /// Select `span` in the code editor and bring it into view.
    ///
    /// Selecting rather than only placing the caret: arriving from a list you
    /// want to see what you arrived at, and select_range scrolls it in.
    fn reveal_span(&mut self, span: comet::Span) {
        let Some(editor) = self.rows.code_editor else {
            return;
        };
        let len = self.script_text.len();
        self.ui.select_range(
            editor,
            (span.start as usize).min(len),
            (span.end as usize).min(len),
        );
        self.dock.activate(dock::Pane::Code);
        self.dirty = true;
    }

    /// Say something in the status bar for a few seconds.
    fn note(&mut self, text: impl Into<String>) {
        self.status_note = Some((text.into(), std::time::Instant::now()));
    }

    /// Jump to what declares the name at the caret, or say why there is nowhere
    /// to go.
    ///
    /// A builtin and an unknown name both report rather than doing nothing: a
    /// key that silently does nothing is indistinguishable from one that is not
    /// bound.
    fn go_to_definition(&mut self) {
        let Some(editor) = self.rows.code_editor else {
            return;
        };
        let at = self.editor_caret();
        match comet::service::definition_at(&self.script_text, helios::script_schema(), at) {
            Some(found) if found.kind.is_in_source() => {
                self.ui
                    .select_range(editor, found.span.start as usize, found.span.end as usize);
                let line = self.script_text[..found.span.start as usize]
                    .matches('\n')
                    .count()
                    + 1;
                self.note(format!(
                    "{} is {} (line {line})",
                    found.name,
                    found.kind.describe()
                ));
            }
            Some(found) => {
                self.note(format!(
                    "{} is {} - the engine provides it",
                    found.name,
                    found.kind.describe()
                ));
            }
            None => self.note("nothing here declares that name"),
        }
        self.dirty = true;
    }

    /// Start renaming the symbol at the caret, if it is one this file declares.
    fn start_symbol_rename(&mut self) {
        let at = self.editor_caret();
        let Some(found) =
            comet::service::definition_at(&self.script_text, helios::script_schema(), at)
        else {
            self.note("put the caret on a name to rename it");
            return;
        };
        if !found.kind.is_in_source() {
            self.note(format!(
                "{} is provided by the engine and cannot be renamed",
                found.name
            ));
            return;
        }
        self.symbol_rename = Some(SymbolRename {
            offset: at,
            draft: found.name,
            error: None,
        });
        self.dirty = true;
    }

    /// Apply the in-progress rename, or refuse it with a reason.
    ///
    /// Every site is rewritten back to front, so the earlier spans stay valid
    /// while the later ones are replaced. script_undo snapshots the whole
    /// buffer, so the whole rename is naturally one step - but the coalescing
    /// run has to be broken, or the next character typed folds into it.
    fn commit_symbol_rename(&mut self) {
        let Some(rename) = self.symbol_rename.clone() else {
            return;
        };
        let Some(editor) = self.rows.code_editor else {
            return;
        };
        let name = rename.draft.trim().to_string();
        if !comet::service::is_identifier(&name) {
            self.set_rename_error("that is not a name");
            return;
        }
        if !comet::service::rename_is_safe(
            &self.script_text,
            helios::script_schema(),
            rename.offset,
            &name,
        ) {
            self.set_rename_error(format!("`{name}` already means something here"));
            return;
        }
        let Some(mut spans) =
            comet::service::rename_spans(&self.script_text, helios::script_schema(), rename.offset)
        else {
            self.set_rename_error("nothing to rename");
            return;
        };
        spans.sort_by_key(|span| std::cmp::Reverse(span.start));
        let sites = spans.len();
        for span in spans {
            self.ui
                .replace_range(editor, span.start as usize..span.end as usize, &name);
        }
        self.script_coalescing = false;
        self.symbol_rename = None;
        self.sync_script_buffer();
        self.note(format!("renamed {sites} occurrences"));
        self.dirty = true;
    }

    fn set_rename_error(&mut self, message: impl Into<String>) {
        if let Some(rename) = &mut self.symbol_rename {
            rename.error = Some(message.into());
        }
        self.dirty = true;
    }

    /// Open the go-to-symbol palette over the editor, listing every function
    /// and every piece of script state.
    ///
    /// The list is built once on open rather than on every keystroke: the file
    /// cannot change while the palette owns the keyboard, and rebuilding it
    /// would throw away the highlighted row on every letter typed.
    fn open_symbol_palette(&mut self) {
        let Some(editor) = self.rows.code_editor else {
            return;
        };
        let anchor = self
            .ui
            .rect(editor)
            .map(|r| Vec2::new(r.pos.x + 40.0, r.pos.y + 6.0))
            .unwrap_or_default();
        let items: Vec<aurora::list::ListItem> = comet::service::symbols(&self.script_text)
            .into_iter()
            .map(|symbol| {
                let (label_color, detail_color) = match symbol.kind {
                    comet::service::SymbolKind::Function => (
                        Some(self.theme.code_function),
                        Some(self.theme.code_function),
                    ),
                    comet::service::SymbolKind::State => (None, Some(self.theme.code_type)),
                };
                aurora::list::ListItem {
                    label: symbol.name,
                    detail: format!("line {}", symbol.line),
                    label_color,
                    detail_color,
                }
            })
            .collect();
        if items.is_empty() {
            return;
        }
        self.symbol_palette = Some((
            String::new(),
            aurora::list::ListPopup::new(items, Vec2::new(anchor.x, anchor.y + 34.0)),
        ));
        self.dirty = true;
    }

    /// Put the caret on the declaration named `name`, if the script has one.
    fn go_to_symbol(&mut self, name: &str) {
        let Some(editor) = self.rows.code_editor else {
            return;
        };
        let Some(symbol) = comet::service::symbols(&self.script_text)
            .into_iter()
            .find(|symbol| symbol.name == name)
        else {
            return;
        };
        self.symbol_palette = None;
        self.ui.focus_caret(editor, symbol.span.start as usize);
        self.dirty = true;
    }

    /// Move the caret to the next declaration after it, or the previous one.
    ///
    /// A script is a list of functions and you navigate it by function; without
    /// this the only way down a long file is PageDown and reading.
    fn step_symbol(&mut self, forward: bool) {
        let Some(editor) = self.rows.code_editor else {
            return;
        };
        let caret = self.editor_caret();
        let symbols = comet::service::symbols(&self.script_text);
        if let Some(at) = step_symbol_target(&symbols, caret, forward) {
            self.ui.focus_caret(editor, at);
            self.dirty = true;
        }
    }

    /// Move the caret to the start of 1-based `line`, clamped to the file.
    fn go_to_line_number(&mut self, line: usize) {
        let Some(editor) = self.rows.code_editor else {
            return;
        };
        let line = line.max(1);
        let offset = self
            .script_text
            .split_inclusive('\n')
            .take(line - 1)
            .map(str::len)
            .sum::<usize>()
            .min(self.script_text.len());
        self.ui.focus_caret(editor, offset);
        self.go_to_line = None;
        self.dirty = true;
    }

    /// Toggle `//` on every line the selection touches.
    ///
    /// Uncomments only when every non-blank line is already commented - so a
    /// block where you commented half of it comments the rest, rather than
    /// flipping each line and leaving the same mess inverted. The `//` goes at
    /// the shallowest indentation in the block, so the code keeps its shape.
    fn toggle_comment(&mut self) {
        let Some(editor) = self.rows.code_editor else {
            return;
        };
        let (range, block) = self.ui.selected_lines(editor);
        if block.is_empty() && range.start == range.end {
            return;
        }
        let toggled = toggle_comment_block(&block);
        self.ui.replace_lines(editor, range, &toggled);
        self.sync_script_buffer();
        self.dirty = true;
    }

    /// Duplicate the lines the selection touches, below themselves.
    fn duplicate_lines(&mut self) {
        let Some(editor) = self.rows.code_editor else {
            return;
        };
        let (range, block) = self.ui.selected_lines(editor);
        self.ui
            .replace_lines(editor, range, &format!("{block}\n{block}"));
        self.sync_script_buffer();
        self.dirty = true;
    }

    /// Join the lines the selection touches into one, the inverse of Enter.
    ///
    /// The trailing whitespace of each line goes and the next line's indent goes
    /// with it, so joining an indented block does not leave a trail of double
    /// spaces. A line that would join onto nothing - the next one is blank -
    /// just loses its newline rather than gaining a space before nothing.
    fn join_lines(&mut self) {
        let Some(editor) = self.rows.code_editor else {
            return;
        };
        let (range, block) = self.ui.selected_lines(editor);
        // One line selected means "join the one below onto this one", so the
        // block has to reach a line further than the selection does.
        let end = self.script_text[range.end..]
            .find('\n')
            .map(|at| range.end + 1 + at)
            .unwrap_or(self.script_text.len());
        let block = if block.contains('\n') {
            block
        } else {
            self.script_text[range.start..end].to_string()
        };
        let joined = join_block(&block);
        let range = range.start..range.start + block.len();
        self.ui.replace_lines(editor, range.clone(), &joined);
        // Selecting the result would be wrong here: after a join you want to
        // carry on typing at the seam, not replace what you just joined.
        let at = range.start + joined.len();
        let len = self.ui.text_of(editor).map_or(at, str::len);
        self.ui.focus_caret(editor, at.min(len));
        self.sync_script_buffer();
        self.dirty = true;
    }

    /// Swap the characters either side of the caret, the standard fix for the
    /// commonest typo. At the end of a line it swaps the two before the caret,
    /// which is where the typo you just noticed actually is.
    fn transpose_chars(&mut self) {
        let Some(editor) = self.rows.code_editor else {
            return;
        };
        let caret = self.editor_caret();
        let Some((range, swapped)) = transposition(&self.script_text, caret) else {
            return;
        };
        let end = range.start + swapped.len();
        self.ui.replace_range(editor, range, &swapped);
        self.ui.focus_caret(editor, end);
        self.sync_script_buffer();
        self.dirty = true;
    }

    /// Recase the selection, or the word under the caret when there is none.
    ///
    /// The result stays selected so the three cases can be tried in sequence -
    /// which is how you actually use this, rather than getting it right first
    /// time.
    fn change_case(&mut self, case: Case) {
        let Some(editor) = self.rows.code_editor else {
            return;
        };
        let range = match self.ui.selection_range() {
            Some(range) => range,
            None => {
                let caret = self.editor_caret();
                let (start, word) = word_before_or_around(&self.script_text, caret);
                if word.is_empty() {
                    return;
                }
                start..start + word.len()
            }
        };
        let recased = case.apply(&self.script_text[range.clone()]);
        // to_uppercase can change the byte length, so the new end comes from the
        // replacement rather than from the range that was replaced.
        let start = range.start;
        self.ui.replace_range(editor, range, &recased);
        self.ui
            .focus_selection(editor, start, start + recased.len());
        self.sync_script_buffer();
        self.dirty = true;
    }

    /// Sort the lines the selection touches, optionally dropping duplicates.
    fn sort_lines(&mut self, dedup: bool) {
        let Some(editor) = self.rows.code_editor else {
            return;
        };
        let (range, block) = self.ui.selected_lines(editor);
        let mut lines: Vec<&str> = block.split('\n').collect();
        if lines.len() < 2 {
            return;
        }
        lines.sort();
        if dedup {
            lines.dedup();
        }
        self.ui.replace_lines(editor, range, &lines.join("\n"));
        self.sync_script_buffer();
        self.dirty = true;
    }

    /// Delete the lines the selection touches.
    fn delete_lines(&mut self) {
        let Some(editor) = self.rows.code_editor else {
            return;
        };
        let (range, _) = self.ui.selected_lines(editor);
        // Take the newline that followed too, or deleting a line leaves a blank
        // one where it was.
        let end = if self.script_text[range.end..].starts_with('\n') {
            range.end + 1
        } else {
            range.end
        };
        let start = if end == range.end && range.start > 0 {
            range.start - 1
        } else {
            range.start
        };
        self.ui.replace_range(editor, start..end, "");
        self.sync_script_buffer();
        self.dirty = true;
    }

    /// Move the lines the selection touches up or down by one.
    fn move_lines(&mut self, down: bool) {
        let Some(editor) = self.rows.code_editor else {
            return;
        };
        let (range, block) = self.ui.selected_lines(editor);
        let text = self.script_text.clone();
        if down {
            let Some(rest) = text.get(range.end..).filter(|r| r.starts_with('\n')) else {
                return; // already the last line
            };
            let next_end = rest[1..]
                .find('\n')
                .map_or(text.len(), |at| range.end + 1 + at);
            let next = &text[range.end + 1..next_end];
            self.ui
                .replace_lines(editor, range.start..next_end, &format!("{next}\n{block}"));
            // Keep the moved lines selected, so alt+down repeats.
            let start = range.start + next.len() + 1;
            self.ui.focus_selection(editor, start, start + block.len());
        } else {
            if range.start == 0 {
                return;
            }
            let prev_start = text[..range.start - 1].rfind('\n').map_or(0, |at| at + 1);
            let prev = &text[prev_start..range.start - 1];
            self.ui
                .replace_lines(editor, prev_start..range.end, &format!("{block}\n{prev}"));
            self.ui
                .focus_selection(editor, prev_start, prev_start + block.len());
        }
        self.sync_script_buffer();
        self.dirty = true;
    }

    /// Which find-bar button `id` is, if any.
    fn find_button(&self, id: aurora::WidgetId) -> Option<FindAction> {
        let rows = &self.rows.find;
        [
            (rows.previous, FindAction::Previous),
            (rows.next, FindAction::Next),
            (rows.replace, FindAction::Replace),
            (rows.replace_all, FindAction::ReplaceAll),
            (rows.close, FindAction::Close),
        ]
        .into_iter()
        .find(|(widget, _)| *widget == Some(id))
        .map(|(_, action)| action)
    }

    fn find_action(&mut self, id: aurora::WidgetId) {
        let (Some(action), Some(editor)) = (self.find_button(id), self.rows.code_editor) else {
            return;
        };
        self.commit_find_fields();
        let Some(find) = &mut self.find else {
            return;
        };
        match action {
            FindAction::Next => {
                find.find_next();
            }
            FindAction::Previous => {
                find.find_previous();
            }
            FindAction::Replace => {
                find.replace_current(&mut self.ui, editor);
            }
            FindAction::ReplaceAll => {
                find.replace_all(&mut self.ui, editor);
            }
            FindAction::Close => {
                self.find = None;
                self.dirty = true;
                return;
            }
        }
        self.reveal_find_match();
        self.sync_script_buffer();
        self.dirty = true;
    }

    /// Select the current match, so stepping through them moves the caret and
    /// scrolls it into view. The highlights themselves are drawn by
    /// [`apply_script_marks`](Self::apply_script_marks), which
    /// owns the editor's decorations.
    fn reveal_find_match(&mut self) {
        let (Some(find), Some(editor)) = (&self.find, self.rows.code_editor) else {
            return;
        };
        find.reveal(&mut self.ui, editor);
    }

    /// The completion label a click on `id` would insert.
    /// The symbol the palette row `id` stands for, if it is one.
    fn symbol_row(&self, id: aurora::WidgetId) -> Option<String> {
        let (_, list) = self.symbol_palette.as_ref()?;
        let index = self
            .rows
            .symbol_rows
            .rows
            .iter()
            .find(|(widget, _)| *widget == id)
            .map(|(_, index)| *index)?;
        list.item(index).map(str::to_string)
    }

    fn completion_row(&self, id: aurora::WidgetId) -> Option<String> {
        let (list, _) = self.completions.as_ref()?;
        let index = self
            .rows
            .completions
            .rows
            .iter()
            .find(|(widget, _)| *widget == id)
            .map(|(_, index)| *index)?;
        list.item(index).map(str::to_string)
    }

    /// Step the Code pane's text history. Returns whether anything moved.
    fn script_history(&mut self, redo: bool) -> bool {
        let Some(editor) = self.rows.code_editor else {
            return false;
        };
        let (from, to) = if redo {
            (&mut self.script_redo, &mut self.script_undo)
        } else {
            (&mut self.script_undo, &mut self.script_redo)
        };
        let Some((text, caret)) = from.pop() else {
            return false;
        };
        to.push((self.script_text.clone(), self.script_caret));
        self.script_text = text;
        // Straight onto the widget: going through the normal edit path would
        // record this as a fresh edit and undo would never make progress.
        self.ui.set_text_input(editor, self.script_text.clone());
        let caret = caret.min(self.script_text.len());
        self.ui.focus_caret(editor, caret);
        self.script_caret = caret;
        self.script_modified = true;
        self.script_coalescing = false;
        self.analyze_script();
        self.dirty = true;
        true
    }

    /// Note where the open script is being read, so reopening it this session
    /// comes back here rather than to the top of the file.
    ///
    /// Both the caret and the scroll: a fresh Ui has no scroll for the field, so
    /// restoring only the caret leaves the view to be re-derived from it, which
    /// parks the caret's line at the bottom of the pane.
    fn remember_script_position(&mut self) {
        let (Some(name), Some(editor)) = (self.open_script.clone(), self.rows.code_editor) else {
            return;
        };
        let caret = self.editor_caret();
        let scroll = self.ui.text_scroll(editor).map_or(0.0, |(y, _)| y);
        self.script_positions.insert(name, (caret, scroll));
    }

    /// Where the caret is in the script, or 0 when the editor is not focused.
    ///
    /// What a search starts from. Once the find bar has focus the editor no
    /// longer reports a caret, so this is read before opening the bar and while
    /// the query is being typed, and it keeps returning the last place the
    /// editor's own caret was.
    fn editor_caret(&self) -> usize {
        self.rows
            .code_editor
            .filter(|&editor| self.ui.focused() == Some(editor))
            .and_then(|_| self.ui.caret_offset())
            .unwrap_or(self.script_caret)
    }

    /// Whether focus is in the code editor or in the find bar over it - the
    /// only places a code-pane shortcut should fire from.
    fn code_context_focused(&self) -> bool {
        let focused = self.ui.focused();
        focused.is_some()
            && (focused == self.rows.code_editor
                || focused == self.rows.find.query
                || focused == self.rows.find.replacement)
    }

    /// Open the find bar over the Code pane, or close it if it is already there.
    fn toggle_find(&mut self) {
        if self.find.is_some() {
            // ctrl+f with the bar already open re-focuses and re-selects the
            // query, which is what every editor does - closing it would fight
            // the reflex of pressing it again to search for something else.
            self.focus_find = true;
            self.dirty = true;
            return;
        }
        // Top-right of the editor, the corner a find bar traditionally hides in
        // and the one least likely to cover what is being searched.
        let anchor = self
            .rows
            .code_editor
            .and_then(|editor| self.ui.rect(editor))
            .map(|r| Vec2::new(r.pos.x + r.size.x - 320.0, r.pos.y + 6.0))
            .unwrap_or_default();
        let mut find = aurora::find::FindBar::new(anchor);
        // Pre-fill from the selection, the way every editor does, and stand on
        // the match nearest the caret rather than the one at the top of the file.
        let caret = self.editor_caret();
        if let Some(selected) = self.ui.selected_text() {
            find.set_query(selected, &self.script_text, caret);
        }
        self.find = Some(find);
        // The point of ctrl+f is to type a query. Without this the caret stays
        // in the source file and the query is typed into the code.
        self.focus_find = true;
        self.dirty = true;
    }

    /// Keys the Code pane's popups claim before anything else sees them.
    /// Returns whether one was consumed.
    fn handle_code_key(&mut self, event: &winit::event::KeyEvent) -> bool {
        // Any key takes a hover tooltip down: it describes what the pointer is
        // resting on, and once you are typing you are not looking there.
        self.hide_hover();
        // ctrl+space asks for the list explicitly - the way to see what is
        // available without typing a letter first and deleting it.
        if self.modifiers.control_key()
            && event.logical_key == WinitKey::Named(NamedKey::Space)
            && self.ui.focused() == self.rows.code_editor
        {
            self.force_completions = true;
            self.refresh_completions();
            self.force_completions = false;
            return true;
        }
        let WinitKey::Named(named) = &event.logical_key else {
            return false;
        };
        // Enter in the find bar steps matches. Left to aurora it would step
        // focus out of the single-line query field, and the submit that follows
        // would put the caret in the source - so the next keystroke was typed
        // into the code.
        if *named == NamedKey::Enter
            && (self.ui.focused() == self.rows.find.query
                || self.ui.focused() == self.rows.find.replacement)
        {
            let shift = self.modifiers.shift_key();
            // Commit what is in the field first, so Enter searches for what you
            // just typed rather than for the last committed query.
            self.commit_find_fields();
            if let Some(find) = &mut self.find
                && find.key(aurora::Key::Enter, shift) == aurora::find::FindKey::Stepped
            {
                self.reveal_find_match();
                self.focus_find = true;
                self.dirty = true;
            }
            return true;
        }
        // alt+up / alt+down move the selected lines. Not a ctrl chord, because
        // ctrl+arrow is word motion in the field underneath.
        if self.modifiers.alt_key()
            && self.ui.focused() == self.rows.code_editor
            && matches!(named, NamedKey::ArrowUp | NamedKey::ArrowDown)
        {
            self.move_lines(*named == NamedKey::ArrowDown);
            return true;
        }
        // ctrl+up / ctrl+down jump to the previous or next declaration. ctrl on
        // the horizontal arrows is word motion; on the vertical ones aurora has
        // no meaning for it, so the pane can have them.
        if self.modifiers.control_key()
            && self.ui.focused() == self.rows.code_editor
            && matches!(named, NamedKey::ArrowUp | NamedKey::ArrowDown)
        {
            self.step_symbol(*named == NamedKey::ArrowDown);
            return true;
        }
        // F3 steps find matches without the bar being focused; shift+F3 back.
        if *named == NamedKey::F3 && self.find.is_some() {
            let back = self.modifiers.shift_key();
            if let Some(find) = &mut self.find {
                if back {
                    find.find_previous();
                } else {
                    find.find_next();
                }
            }
            self.reveal_find_match();
            self.dirty = true;
            return true;
        }
        // F12 jumps to what declares the name at the caret; F2 renames it.
        // Both only with the editor focused - F2 elsewhere renames a node or a
        // file, and that binding is older.
        if self.ui.focused() == self.rows.code_editor && self.rows.code_editor.is_some() {
            if *named == NamedKey::F12 {
                self.go_to_definition();
                return true;
            }
            if *named == NamedKey::F2 {
                self.start_symbol_rename();
                return true;
            }
        }
        if *named == NamedKey::Enter
            && self.symbol_rename.is_some()
            && self.ui.focused() == self.rows.symbol_rename
        {
            self.sync_symbol_rename();
            self.commit_symbol_rename();
            return true;
        }
        // F8 steps to the next problem, shift+F8 to the previous. With the
        // message in the status bar this is how a broken file gets walked.
        if *named == NamedKey::F8 && self.rows.code_editor.is_some() {
            self.step_diagnostic(!self.modifiers.shift_key());
            return true;
        }
        if *named == NamedKey::Enter
            && self.go_to_line.is_some()
            && self.ui.focused() == self.rows.go_to_line
        {
            let typed = self
                .rows
                .go_to_line
                .and_then(|id| self.ui.text_of(id))
                .and_then(|t| t.trim().parse::<usize>().ok());
            match typed {
                Some(line) => self.go_to_line_number(line),
                None => self.go_to_line = None,
            }
            self.dirty = true;
            return true;
        }
        // Escape closes what is open, innermost first.
        if *named == NamedKey::Escape {
            // A rename in progress leaves the buffer untouched.
            if self.symbol_rename.take().is_some() {
                self.dirty = true;
                return true;
            }
            if self.symbol_palette.take().is_some() {
                self.dirty = true;
                return true;
            }
            if self.go_to_line.take().is_some() {
                self.dirty = true;
                return true;
            }
            if self.completions.take().is_some() || self.find.take().is_some() {
                self.dirty = true;
                return true;
            }
            // Nothing open: Escape drops a stray selection in the editor. It
            // could otherwise only be cleared by clicking, and clicking puts the
            // caret somewhere you did not ask for.
            if self.ui.focused() == self.rows.code_editor && self.ui.collapse_selection() {
                return true;
            }
            return false;
        }
        // The symbol palette owns the arrows and Enter while it is open.
        if let Some((_, list)) = &mut self.symbol_palette {
            let key = match named {
                NamedKey::ArrowDown => aurora::Key::Down,
                NamedKey::ArrowUp => aurora::Key::Up,
                NamedKey::Enter => aurora::Key::Enter,
                _ => return false,
            };
            return match list.key(key) {
                aurora::list::ListKey::Moved => {
                    self.dirty = true;
                    true
                }
                aurora::list::ListKey::Accepted(index) => {
                    let name = list.item(index).map(str::to_string);
                    if let Some(name) = name {
                        self.go_to_symbol(&name);
                    }
                    true
                }
                aurora::list::ListKey::Ignored => false,
            };
        }
        // ctrl+Enter opens a line; the popup must not take it as "accept". A
        // chord the list has no meaning for belongs to the editor underneath.
        if self.modifiers.control_key() && *named == NamedKey::Enter {
            return false;
        }
        let Some((list, _)) = &mut self.completions else {
            return false;
        };
        let key = match named {
            NamedKey::ArrowDown => aurora::Key::Down,
            NamedKey::ArrowUp => aurora::Key::Up,
            NamedKey::Enter => aurora::Key::Enter,
            NamedKey::Tab => aurora::Key::Tab,
            _ => return false,
        };
        match list.key(key) {
            aurora::list::ListKey::Moved => {
                self.dirty = true;
                true
            }
            aurora::list::ListKey::Accepted(index) => {
                let Some(label) = list.item(index).map(str::to_string) else {
                    return false;
                };
                self.accept_completion(&label);
                true
            }
            aurora::list::ListKey::Ignored => false,
        }
    }

    /// Open, refresh, or close the completion popup for the word the caret is
    /// in. Called after every edit, so the offered set follows what is typed.
    fn refresh_completions(&mut self) {
        let was_open = self.completions.is_some();
        self.refresh_completions_inner();
        self.update_signature();
        // Every exit from here has to mark the shell dirty when the popup's
        // presence changed. Five of them used to just clear the state and
        // return, so the popup vanished from `self.completions` but the old Ui
        // kept drawing it - which is the whole of "sometimes it does not
        // disappear". It only ever went away when something else happened to
        // ask for a rebuild that frame.
        if self.completions.is_some() != was_open {
            self.dirty = true;
        }
    }

    /// Decide what the popup should be, without worrying about redraws.
    fn refresh_completions_inner(&mut self) {
        let (Some(editor), true) = (self.rows.code_editor, self.open_script.is_some()) else {
            self.completions = None;
            return;
        };
        if self.ui.focused() != Some(editor) {
            self.completions = None;
            return;
        }
        let Some(caret) = self.ui.caret_offset() else {
            self.completions = None;
            return;
        };
        // ctrl+space asks for the list wherever the caret is, even on an empty
        // prefix; otherwise it appears only where a name is being typed.
        let anchor = match completion_anchor(&self.script_text, caret) {
            Some(start) => start,
            None if self.force_completions => caret,
            None => {
                self.completions = None;
                return;
            }
        };
        let (_, prefix) = word_before(&self.script_text, caret);
        let theme = &self.theme;
        let items: Vec<aurora::list::ListItem> =
            comet::service::completions_at(&self.script_text, helios::script_schema(), caret)
                .into_iter()
                .map(|item| {
                    let (label, detail) = completion_colors(item.kind, theme);
                    aurora::list::ListItem::with_detail(item.label, item.detail)
                        .colored(label, detail)
                })
                .collect();
        // No anchor here on purpose. The buffer has just been edited and not
        // re-shaped, so the caret has no on-screen position yet - asking for one
        // returns None, and defaulting to zero put the popup in the corner of
        // the window. update_completion_anchor sets it after layout instead.
        match &mut self.completions {
            Some((list, at)) => {
                *at = anchor..caret;
                list.set_items(items);
                list.set_filter(prefix);
            }
            None => {
                let mut list = aurora::list::ListPopup::new(items, self.completion_anchor_now());
                list.set_filter(prefix);
                self.completions = Some((list, anchor..caret));
            }
        }
        // Nothing left to offer is the same as nothing to show.
        if self.completions.as_ref().is_some_and(|(l, _)| l.is_empty()) {
            self.completions = None;
        }
        self.dirty = true;
    }

    /// Where the completion popup should sit right now: just under the caret,
    /// so it does not cover the word being typed.
    ///
    /// Falls back to the last anchor rather than to the origin: the caret has no
    /// position between an edit and the next layout, and a popup that jumps to
    /// the corner of the window whenever that happens is worse than one that
    /// waits a frame.
    fn completion_anchor_now(&self) -> Vec2 {
        let previous = self
            .completions
            .as_ref()
            .map(|(list, _)| list.anchor())
            .unwrap_or_default();
        self.rows
            .code_editor
            .and_then(|editor| self.ui.caret_rect(editor))
            .map_or(previous, |r| r.pos + Vec2::new(0.0, r.size.y + 2.0))
    }

    /// Refresh the signature hint for the call the caret is inside.
    ///
    /// Sits below the caret like the completion list, and gives way to it: two
    /// popups in the same place is worse than either. The completion list is
    /// what you are acting on; the signature is reference.
    fn update_signature(&mut self) {
        let was = self.signature.is_some();
        let editor = self.rows.code_editor;
        let focused = editor.is_some() && self.ui.focused() == editor;
        let help = (focused && self.completions.is_none())
            .then(|| comet::service::signature_at(&self.script_text, self.editor_caret()))
            .flatten();
        self.signature = help.map(|help| {
            let anchor = editor
                .and_then(|editor| self.ui.caret_rect(editor))
                .map(|r| r.pos + Vec2::new(0.0, r.size.y + 2.0))
                .or_else(|| self.signature.as_ref().map(|(_, at)| *at))
                .unwrap_or_default();
            (help, anchor)
        });
        // Same rule as the completion popup: a change in whether it is there at
        // all has to reach the shell, or the old Ui keeps drawing it.
        if self.signature.is_some() != was {
            self.dirty = true;
        }
    }

    /// Move the popup under the caret, after layout has given the caret a
    /// position. Also the backstop that takes it down when the editor is no
    /// longer the focused widget - clicking away should not leave it floating.
    fn update_completion_anchor(&mut self) {
        if self.completions.is_none() {
            return;
        }
        // Not while the pointer is down: a press on a row moves focus to that
        // row, and closing here would destroy the row before its release could
        // become a click.
        if !self.pointer_down
            && (self.ui.focused() != self.rows.code_editor || self.rows.code_editor.is_none())
        {
            self.completions = None;
            self.dirty = true;
            return;
        }
        let anchor = self.completion_anchor_now();
        if let Some((list, _)) = &mut self.completions
            && list.anchor() != anchor
        {
            list.set_anchor(anchor);
            self.dirty = true;
        }
    }

    /// Keep the signature hint under the caret, the way the completion popup is
    /// kept under it.
    fn update_signature_anchor(&mut self) {
        let Some(editor) = self.rows.code_editor else {
            return;
        };
        let Some(rect) = self.ui.caret_rect(editor) else {
            return;
        };
        let anchor = rect.pos + Vec2::new(0.0, rect.size.y + 2.0);
        if let Some((_, at)) = &mut self.signature
            && *at != anchor
        {
            *at = anchor;
            self.dirty = true;
        }
    }

    /// Replace the word the popup was completing with `label`.
    fn accept_completion(&mut self, label: &str) {
        let (Some(editor), Some((_, range))) = (self.rows.code_editor, self.completions.as_ref())
        else {
            return;
        };
        let range = range.clone();
        // A function completion brings its call parens, with the caret between
        // them: every one of them is followed within a keystroke by `(`. The
        // kind comes from asking the service rather than from state kept beside
        // the popup, which is one fewer thing that can drift - and it costs one
        // pipeline run per acceptance, not per keystroke.
        let kind =
            comet::service::completions_at(&self.script_text, helios::script_schema(), range.start)
                .into_iter()
                .find(|item| item.label == label)
                .map(|item| item.kind);
        let is_call = kind == Some(comet::service::CompletionKind::Function);
        // A generic type is never written without its argument, so accepting one
        // brings the brackets and leaves the caret between them - the same
        // reasoning as a function's call parens.
        let is_generic = kind == Some(comet::service::CompletionKind::GenericType);
        // Unless there is already one there, in which case adding another is
        // how you end up with `print(())`.
        let has_parens = self.script_text[range.end..].trim_start().starts_with('(');
        let has_args = self.script_text[range.end..].trim_start().starts_with('<');
        let insertion = if is_call && !has_parens {
            format!("{label}()")
        } else if is_generic && !has_args {
            format!("{label}<>")
        } else {
            label.to_string()
        };
        // select_range re-focuses the editor, which a click on a row had taken
        // focus away from.
        self.ui.select_range(editor, range.start, range.end);
        self.ui.insert_str(&insertion);
        if is_call && !has_parens {
            self.ui
                .focus_caret(editor, range.start + insertion.len() - 1);
        }
        self.completions = None;
        self.sync_script_buffer();
        self.dirty = true;
    }

    /// Re-run the language service over the open script, caching what it found.
    ///
    /// Called when the buffer changes, not every frame: the pipeline is fast on
    /// a script-sized file, but re-lexing it sixty times a second to learn
    /// nothing new is still work nobody asked for.
    fn analyze_script(&mut self) {
        if self.open_script.is_none() && !self.script_orphaned {
            self.analysis = comet::service::Analysis::default();
            self.script_spans.clear();
            self.script_diagnostics.clear();
            return;
        }
        // One pass for everything: diagnostics, syntax classes and brackets all
        // come out of the same lex and parse rather than three of each.
        self.analysis = comet::service::Analysis::new(&self.script_text, helios::script_schema());
        let theme = &self.theme;
        self.script_spans = self
            .analysis
            .tokens
            .iter()
            .copied()
            .filter_map(|token| {
                let color = match token.class {
                    comet::service::TokenClass::Keyword => theme.code_keyword,
                    comet::service::TokenClass::Number => theme.code_number,
                    comet::service::TokenClass::Str => theme.code_string,
                    comet::service::TokenClass::Comment => theme.code_comment,
                    comet::service::TokenClass::Function => theme.code_function,
                    comet::service::TokenClass::Type => theme.code_type,
                    comet::service::TokenClass::Annotation => theme.code_annotation,
                    // Identifiers, operators and punctuation are the widget's
                    // own color - a span for each would be noise and cost a
                    // draw command apiece.
                    _ => return None,
                };
                Some(aurora::TextSpan {
                    range: token.span.start as usize..token.span.end as usize,
                    color,
                })
            })
            .collect();
        self.script_diagnostics = self.analysis.diagnostics.clone();
    }

    /// Push the cached syntax colors and diagnostics onto the live Ui, along
    /// with the find bar's matches.
    ///
    /// Every frame, and deliberately so: a rebuild hands back a fresh Ui with
    /// none of this on it, and both are read when the draw list is built rather
    /// than when the text is shaped - so re-applying costs a regroup of glyphs
    /// already laid out, and no rebuild of our own.
    fn apply_script_marks(&mut self) {
        let Some(editor) = self.rows.code_editor else {
            return;
        };
        self.ui.set_text_spans(editor, self.script_spans.clone());
        let theme = &self.theme;
        let diagnostics = &self.script_diagnostics;
        // One owner for the editor's decorations, set fresh each frame: find
        // matches behind (they fill), squiggles over them. Two callers each
        // setting their own would mean whichever ran last erased the other.
        let mut decorations: Vec<aurora::Decoration> = self
            .find
            .as_ref()
            .map(|find| find.decorations(&theme.aurora))
            .unwrap_or_default();
        decorations.extend(diagnostics.iter().map(|d| aurora::Decoration {
            range: d.span.start as usize..d.span.end as usize,
            color: match d.severity {
                comet::Severity::Error => theme.code_error,
                comet::Severity::Warning => theme.code_warning,
            },
            style: aurora::DecorationStyle::Squiggle,
        }));
        // Every occurrence of the word the caret is in, faintly. Reading code
        // is mostly "where else does this name appear", and this answers it
        // without a search, a shortcut, or moving the caret.
        if self.ui.focused() == Some(editor)
            && let Some(at) = self.ui.caret_offset()
        {
            decorations.extend(
                comet::service::occurrences_at(&self.script_text, at)
                    .iter()
                    .map(|span| aurora::Decoration {
                        range: span.start as usize..span.end as usize,
                        color: theme.code_occurrence,
                        style: aurora::DecorationStyle::Highlight,
                    }),
            );
        }
        // The bracket the caret is touching, and its partner - or the bracket
        // itself marked in the error color when it has none. comet is entirely
        // braces and the pane has no folding or indent guides, so this is the
        // only way to pair a `}` with its `{`.
        if self.ui.focused() == Some(editor)
            && let Some(at) = self.ui.caret_offset()
            && let Some(bracket) = self.analysis.bracket_at(at)
        {
            let (color, style) = match bracket.partner {
                Some(_) => (theme.code_function, aurora::DecorationStyle::Highlight),
                None => (theme.code_error, aurora::DecorationStyle::Underline),
            };
            for span in [Some(bracket.span), bracket.partner].into_iter().flatten() {
                decorations.push(aurora::Decoration {
                    range: span.start as usize..span.end as usize,
                    color: match style {
                        aurora::DecorationStyle::Highlight => color.fade(0.45),
                        _ => color,
                    },
                    style,
                });
            }
        }
        self.ui.set_decorations(editor, decorations);

        // Where the problems are in a file you cannot see all of. Positions are
        // by line rather than by byte, so a long line does not read as a big
        // stretch of trouble.
        let lines = self.script_text.split('\n').count().max(1) as f32;
        let marks = self
            .script_diagnostics
            .iter()
            .map(|d| {
                let at = (d.span.start as usize).min(self.script_text.len());
                let line = self.script_text[..at].matches('\n').count() as f32;
                let color = match d.severity {
                    comet::Severity::Error => theme.code_error,
                    comet::Severity::Warning => theme.code_warning,
                };
                (line / lines, color)
            })
            .collect();
        self.ui.set_scroll_marks(editor, marks);

        // The same problems in the gutter, by line rather than by fraction. An
        // error on a line wins over a warning: the line cannot compile either
        // way, and the more serious colour is the useful one.
        let mut gutter: Vec<(usize, aurora::Color)> = Vec::new();
        for d in &self.script_diagnostics {
            let at = (d.span.start as usize).min(self.script_text.len());
            let line = self.script_text[..at].matches('\n').count();
            let color = match d.severity {
                comet::Severity::Error => theme.code_error,
                comet::Severity::Warning => theme.code_warning,
            };
            match gutter.iter_mut().find(|(existing, _)| *existing == line) {
                Some(entry) if d.severity == comet::Severity::Error => entry.1 = color,
                Some(_) => {}
                None => gutter.push((line, color)),
            }
        }
        self.ui.set_gutter_marks(editor, gutter);
    }

    /// Assert the editor shows what we think it does.
    ///
    /// The invariant the frame order exists to keep: after a rebuild, the
    /// widget's text is `script_text`. If a sync ever moves back after the
    /// rebuild, this fires in a debug build instead of silently eating edits.
    fn check_editor_in_sync(&self, when: &str) {
        let Some(shown) = self.rows.code_editor.and_then(|id| self.ui.text_of(id)) else {
            return;
        };
        if shown != self.script_text {
            // Logged rather than asserted: this fires in a session someone is
            // using, and a crash is a worse way to learn about it than a line
            // in the Console pane. If it ever appears, an edit was about to be
            // thrown away by a rebuild reading a stale mirror.
            tracing::error!(
                "code editor and script_text disagree {when} ({} vs {} bytes)",
                shown.len(),
                self.script_text.len()
            );
        }
    }

    /// Read the Code pane's live text back out of the Ui.
    ///
    /// Deliberately does not mark the shell dirty per keystroke: the only thing
    /// a rebuild would change is the header's modified mark, and rebuilding
    /// while someone types is how a caret gets lost. Everything else the editor
    /// shows - squiggles, syntax colors - is set on the live Ui and needs no
    /// rebuild at all.
    fn sync_script_buffer(&mut self) {
        let Some(editor) = self.rows.code_editor else {
            return;
        };
        let Some(text) = self.ui.text_of(editor) else {
            return;
        };
        if text == self.script_text {
            return;
        }
        let text = text.to_string();
        let caret_before = self.script_caret.min(self.script_text.len());
        let text = text.as_str();
        // Snapshot what it was before adopting the change, so undo has somewhere
        // to go back to. A run of ordinary typing collapses into one step -
        // undoing a word at a time, not a letter at a time.
        let previous = std::mem::replace(&mut self.script_text, text.to_string());
        let joins = coalesces(&previous, &self.script_text) && self.script_coalescing;
        self.script_coalescing = coalesces(&previous, &self.script_text);
        if !joins {
            self.script_undo.push((previous, caret_before));
            if self.script_undo.len() > MAX_TEXT_UNDO {
                self.script_undo.remove(0);
            }
        }
        self.script_redo.clear();
        self.analyze_script();
        self.script_caret = self.ui.caret_offset().unwrap_or(self.script_text.len());
        if !self.script_modified {
            self.script_modified = true;
            self.dirty = true;
        }
        self.refresh_completions();
        // A find bar over a buffer that just changed is showing stale matches.
        if let Some(find) = &mut self.find {
            find.refresh(&self.script_text);
            self.dirty = true;
        }
    }

    /// Re-search as the query is typed, rather than only on Enter.
    /// Read what has been typed into the symbol palette's query field back into
    /// the palette, narrowing the list. Same shape as the find query, and for
    /// the same reason: the field lives in the Ui, the state lives here, and a
    /// rebuild reconstructs the field from the state.
    fn sync_symbol_query(&mut self) {
        let Some(field) = self.rows.symbol_query else {
            return;
        };
        let Some(text) = self.ui.text_of(field).map(str::to_string) else {
            return;
        };
        let Some((query, list)) = &mut self.symbol_palette else {
            return;
        };
        if *query == text {
            return;
        }
        query.clone_from(&text);
        list.set_filter(&text);
        self.dirty = true;
    }

    /// Read the rename field's text back into the pending rename.
    fn sync_symbol_rename(&mut self) {
        let Some(field) = self.rows.symbol_rename else {
            return;
        };
        let Some(text) = self.ui.text_of(field).map(str::to_string) else {
            return;
        };
        let Some(rename) = &mut self.symbol_rename else {
            return;
        };
        if rename.draft == text {
            return;
        }
        rename.draft = text;
        // Typing is an answer to whatever the last refusal said.
        rename.error = None;
    }

    fn sync_find_query(&mut self) {
        let Some(query_id) = self.rows.find.query else {
            return;
        };
        let Some(text) = self.ui.text_of(query_id).map(str::to_string) else {
            return;
        };
        let script = self.script_text.clone();
        let caret = self.editor_caret();
        let Some(find) = &mut self.find else {
            return;
        };
        if find.query() == text {
            return;
        }
        find.set_query(text, &script, caret);
        // Show where the matches are while typing, but do not drag the caret
        // into the file on every keystroke - that would steal focus from the
        // query box being typed into.
        self.dirty = true;
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
        // The Code pane's readouts, blank when it is not what you are using.
        let (caret, diagnostic) = match self.rows.code_editor {
            Some(editor) if self.ui.focused() == Some(editor) => {
                let position = self
                    .ui
                    .caret_line_column()
                    .map(|(line, column)| format!("Ln {line}, Col {column}"))
                    .unwrap_or_default();
                // A note about what just happened wins over the standing
                // diagnostic readout, for as long as it is fresh.
                let note = self.status_note.as_ref().and_then(|(text, at)| {
                    (at.elapsed() < STATUS_NOTE_LIFETIME).then(|| text.clone())
                });
                let message = note
                    .or_else(|| {
                        self.ui
                            .caret_offset()
                            .and_then(|at| self.diagnostic_at(at))
                            .map(|d| d.message.clone())
                    })
                    .unwrap_or_else(|| match self.script_diagnostics.len() {
                        0 => String::new(),
                        // Not on one, but the file has some: say how many, so a
                        // broken file never looks clean.
                        1 => "1 problem".to_string(),
                        n => format!("{n} problems"),
                    });
                (position, message)
            }
            _ => (String::new(), String::new()),
        };
        let zoom = format!("zoom {:.0}%", self.camera.zoom * 100.0);
        let fps = format!("{:.0} fps", self.last_fps);
        let modified = if self.is_modified() {
            "unsaved changes"
        } else {
            ""
        };
        for (id, text) in [
            (self.rows.status_caret, caret),
            (self.rows.status_diagnostic, diagnostic),
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
        self.flush_pointer();
        // BEFORE anything that can set `dirty`. A rebuild reconstructs the code
        // editor from `self.script_text`, so that has to already hold what the
        // user typed into the old Ui - otherwise the rebuild puts the stale text
        // back and the edit is gone. That was the bug behind "backspace
        // sometimes just moves the cursor back": the character was deleted in
        // the Ui, a rebuild restored the pre-edit text, and the caret came back
        // one place to the left.
        self.sync_script_buffer();
        self.sync_find_query();
        self.sync_symbol_query();
        self.sync_symbol_rename();
        self.poll_settings();
        self.poll_diagnostic_tooltip();
        self.tick_caret();
        self.react();
        self.sync_tree_filter();
        // Incidental background maintenance that can flip `dirty` and so force a
        // shell rebuild: new console lines, streamed-in thumbnails, and the
        // explorer's grid reflowing as its pane resizes. A rebuild swaps the whole
        // Ui, dropping Aurora's retained press state - which would abort an
        // in-progress splitter/slider/file drag the instant one of these fires
        // (e.g. dragging the Files splitter across a column boundary). So defer
        // them while a button is held; none is time-critical, and they resume the
        // moment it is released. (update_file_tooltip is frozen for the same
        // reason - see its guard.)
        if !self.pointer_down {
            self.poll_console();
            self.ensure_thumbnails();
            self.ensure_asset_thumbnails();
            self.sync_explorer_view();
        }
        self.update_file_tooltip();
        let t_react = fstart.elapsed();
        let mut phase = std::time::Instant::now();
        // Whether this frame rebuilds the shell. The insertion-line overlay is
        // added post-layout onto the popup layer, so it only exists on frames
        // that (re)build - on still frames the previous frame's line persists
        // (a single copy, no accumulation). Crucially we do NOT force a rebuild
        // every frame during a reparent: that would reset Aurora's retained
        // `pressed`/`hovered` between a press and its release, killing a plain
        // click's selection and making the held-still insertion line flicker.
        // Once more before the rebuild: react() above can change the editor's
        // text - accepting a completion, a replace, a line command - and the
        // rebuild reconstructs the field from `script_text`. Cheap when nothing
        // changed, and it closes the window in which the two could disagree.
        self.sync_script_buffer();
        let rebuilt = self.dirty;
        if self.dirty {
            // Capture the search box's caret before discarding the old Ui, so it
            // can be restored after the rebuild (the box refilters, and so
            // rebuilds, on every keystroke - see refocus_filter below). The dock
            // sizes and scroll are captured at the end of each frame instead
            // (sync_dock_state), when the Ui and dock are guaranteed to match -
            // a rearrange this frame has already restructured self.dock.
            let filter_caret = self.ui.caret_offset();
            // Same for the code editor: the one rebuild typing causes is the
            // first edit (the header gains its modified mark), and losing the
            // caret on that keystroke would be maddening.
            let in_code = self.ui.focused() == self.rows.code_editor;
            let code_caret = in_code.then(|| self.ui.caret_offset()).flatten();
            let code_anchor = in_code.then(|| self.ui.selection_anchor()).flatten();
            // Snapshot the log ring for the console pane (brief lock), recording
            // the generation so the poll below only rebuilds when it changes.
            // While a tab is being carried out of its bar, the shell is built
            // as if that pane were already removed - the real dock is untouched,
            // so abandoning the drag simply restores it.
            let dock_view = match self.carrying {
                Some(pane) => self
                    .dock
                    .clone()
                    .remove_pane(pane)
                    .unwrap_or_else(|| self.dock.clone()),
                None => self.dock.clone(),
            };
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
                &dock_view,
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
                self.open_script.as_deref(),
                &self.script_text,
                self.script_modified,
                self.script_orphaned,
                &self.script_diagnostics,
                &self.tab_bars,
                &self.theme,
            );
            self.ui = ui;
            self.rows = rows;
            // The color picker floats on the popup layer, added after the shell
            // so it draws on top; its gradient images are kept up to date
            // separately (refresh_picker_images).
            if let Some(open) = &self.picker {
                self.rows.picker = open.picker.build(&mut self.ui, &self.theme.aurora);
            }
            if let Some((list, _)) = &self.completions {
                self.rows.completions = list.build(&mut self.ui, &self.theme.aurora);
            }
            if let Some((help, anchor)) = &self.signature {
                ui::add_signature_hint(&mut self.ui, *anchor, help, &self.theme);
            }
            if let Some(find) = &self.find {
                self.rows.find = find.build(&mut self.ui, &self.theme.aurora);
            }
            if let Some(rename) = &self.symbol_rename
                && let Some(editor) = self.rows.code_editor
                && let Some(rect) = self.ui.rect(editor)
            {
                let anchor = Vec2::new(rect.pos.x + 40.0, rect.pos.y + 6.0);
                let draft = rename.draft.clone();
                let error = rename.error.clone();
                self.rows.symbol_rename = ui::add_symbol_rename(
                    &mut self.ui,
                    anchor,
                    &draft,
                    error.as_deref(),
                    &self.theme,
                );
                if let Some(field) = self.rows.symbol_rename {
                    let len = draft.len();
                    self.ui.focus_selection(field, 0, len);
                }
            }
            if let Some((query, list)) = &self.symbol_palette
                && let Some(editor) = self.rows.code_editor
                && let Some(rect) = self.ui.rect(editor)
            {
                let anchor = Vec2::new(rect.pos.x + 40.0, rect.pos.y + 6.0);
                let query = query.clone();
                self.rows.symbol_rows = list.build(&mut self.ui, &self.theme.aurora);
                self.rows.symbol_query =
                    ui::add_symbol_query(&mut self.ui, anchor, &query, &self.theme);
                // The query field takes the keyboard: the palette exists to be
                // typed into, and a caret left in the source would put the next
                // letter in the file.
                if let Some(field) = self.rows.symbol_query {
                    let len = query.len();
                    self.ui.focus_selection(field, len, len);
                }
            }
            if let Some(value) = self.go_to_line.clone()
                && let Some(editor) = self.rows.code_editor
                && let Some(rect) = self.ui.rect(editor)
            {
                let anchor = Vec2::new(rect.pos.x + rect.size.x - 200.0, rect.pos.y + 6.0);
                self.rows.go_to_line =
                    ui::add_go_to_line(&mut self.ui, anchor, &value, &self.theme);
                if let Some(field) = self.rows.go_to_line {
                    let len = value.len();
                    self.ui.focus_selection(field, 0, len);
                }
            }
            // A carried tab: a small floating copy that follows the pointer, so
            // there is something in hand while its pane is out of the layout.
            if let Some(pane) = self.carrying {
                self.rows.carried_tab = Some(ui::build_carried_tab(
                    &mut self.ui,
                    pane.title(),
                    &self.theme,
                ));
            }
            // The modal is the topmost popup: added last so its backdrop covers
            // (and blocks) the shell and any picker beneath it.
            if let Some(m) = &self.modal {
                build_modal(
                    &mut self.ui,
                    m,
                    Some(&self.icons),
                    &self.thumbnails,
                    &self.theme,
                    &mut self.rows,
                );
            }
            self.dirty = false;
            self.check_editor_in_sync("after a rebuild");
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
            // A freshly opened script takes the keyboard, at the position it
            // was left. Ahead of the ordinary caret restore below, which only
            // fires when the editor already had focus - which, on the frame a
            // file is opened from the explorer, it did not.
            if self.focus_editor
                && let Some(editor) = self.rows.code_editor
            {
                let (caret, scroll) = self
                    .open_script
                    .as_ref()
                    .and_then(|name| self.script_positions.get(name).copied())
                    .unwrap_or((0, 0.0));
                self.ui
                    .focus_caret(editor, caret.min(self.script_text.len()));
                self.ui.set_text_scroll(editor, scroll);
                self.focus_editor = false;
            } else if self.focus_find
                && let Some(query) = self.rows.find.query
            {
                let len = self.ui.text_of(query).map_or(0, str::len);
                self.ui.focus_selection(query, 0, len);
                self.focus_find = false;
            } else if let (Some(offset), Some(editor)) = (code_caret, self.rows.code_editor) {
                // Restore the selection, not only the caret: a rebuild used to
                // silently empty it, so ctrl+c after one copied nothing.
                match code_anchor {
                    Some(anchor) => self.ui.focus_selection(editor, anchor, offset),
                    None => self.ui.focus_caret(editor, offset),
                }
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

        // A squiggle's message, near the pointer. The whole reason diagnostics
        // are worth drawing: a red line with no text anywhere tells you that
        // something is wrong and nothing about what.
        if rebuilt && let Some(message) = self.diagnostic_tooltip.clone() {
            ui::add_diagnostic_tooltip(&mut self.ui, self.cursor, window, &message, &self.theme);
            self.ui.layout(window)?;
        }

        // The file-details tooltip is likewise overlaid post-layout near the
        // cursor (only on rebuilt frames; a still frame keeps the last one).
        if rebuilt
            && let Some(path) = self.tooltip_target.clone()
            && let Some(entry) = self.explorer.contents().iter().find(|e| e.path == path)
        {
            ui::add_file_tooltip(&mut self.ui, self.cursor, window, entry, &self.theme);
            self.ui.layout(window)?;
        }

        // With the tabs at their settled positions, advance the slide that plays
        // after a reorder (and keep a dragged tab pinned under the pointer).
        let dt = self.last_frame.elapsed().as_secs_f32().min(0.1);
        self.last_frame = std::time::Instant::now();
        self.tick_tab_bars(dt);
        if let Some(id) = self.rows.carried_tab {
            // The carried tab sits under the pointer at the same point it was
            // grabbed by, tracking it exactly - no easing, which reads as lag.
            // Over a tab strip it snaps into line with the tabs instead: moving
            // along the bar stays 1:1 with the pointer, but the tab is either in
            // the bar or it is not, and snapping is what makes that legible.
            self.update_carry_preview();
            let want = self.cursor - self.carry_grab;
            let strip_y = self.rows.dock_tab_bars.iter().find_map(|r| {
                r.bar
                    .and_then(|b| self.ui.rect(b))
                    .filter(|q| strip_band(*q).contains(self.cursor))
                    .map(|q| q.pos.y + 5.0)
            });
            // translate is an offset from where the widget laid out, not an
            // absolute position.
            let base = self.ui.rect(id).map(|r| r.pos).unwrap_or(Vec2::ZERO);
            let at = Vec2::new(want.x, strip_y.unwrap_or(want.y));
            self.ui.set_translate(id, at - base);
        }

        // Now the shell is laid out and the Ui matches self.dock exactly (this
        // frame's rebuild, if any, has already restructured the dock), read the
        // live splitter sizes and pane scroll back so the next rebuild restores
        // them - the docking equivalent of the old capture_panel_sizes.
        self.apply_script_marks();
        self.update_completion_anchor();
        self.update_signature_anchor();
        self.check_editor_in_sync("at the end of a frame");
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
        // Viewport colors come from the theme (clear + grid + gizmo palette),
        // converted from the aurora color space into photon's.
        let to_photon = |c: aurora::Color| PhotonColor::new(c.r, c.g, c.b, c.a);
        let clear = to_photon(self.theme.viewport_bg);
        let pal = viewport::Palette {
            outline: to_photon(self.theme.selection_outline),
            axis_x: to_photon(self.theme.axis_x),
            axis_y: to_photon(self.theme.axis_y),
            rotate_handle: to_photon(self.theme.gizmo_rotate),
            scale_handle: to_photon(self.theme.gizmo_scale),
            grid_minor: to_photon(self.theme.grid_line),
            grid_major: to_photon(self.theme.grid_line_strong),
        };
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
                &pal,
            ));
        }
        if let Some(drag) = &self.drag
            && let Some(guide) = viewport::axis_guide_sprite(
                drag,
                &self.project.scene,
                &self.camera,
                Vec2::new(w as f32, h as f32),
                &pal,
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
                &pal,
            ));
        }
        // A script being dragged over the viewport lands on one node, not on the
        // viewport at large, so it outlines that node rather than letting the
        // whole pane light up (which is what aurora's drop highlight would do,
        // and why the viewport turns script drags down).
        if let Some(node) = self.script_drop_node()
            && let Some(g) = viewport::gizmo(&self.project.scene, node, self.camera.zoom)
        {
            overlay.extend(viewport::outline_sprites(
                &g,
                self.camera.zoom,
                pal.rotate_handle,
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
        if self.carrying.is_some()
            && self.tab_drop_target().is_none()
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
                // A slider is an exported value the script gave a `@range`.
                // It commits through the history like a typed field does, so
                // one drag is one undo step.
                // A slider drag writes straight into the scene and does NOT
                // mark the shell dirty: a rebuild swaps the whole Ui and drops
                // aurora's retained pressed state, so the drag would end after
                // one step. The history entry is made on release, which is also
                // what makes the whole drag one undo step - the same shape the
                // inspector's label scrub uses.
                AuroraEvent::SliderChanged { id, value }
                    if self.rows.field_sliders.iter().any(|(w, ..)| *w == id) =>
                {
                    let (node, component, field) = self
                        .rows
                        .field_sliders
                        .iter()
                        .find(|(w, ..)| *w == id)
                        .map(|(_, n, c, f)| (*n, *c, f.clone()))
                        .expect("guarded by the match arm");
                    let reflect = self.project.scene.node(node).components[component].as_reflect();
                    let original = reflect.get(&field);
                    self.slider_drag.get_or_insert_with(|| SliderDrag {
                        node,
                        component,
                        field: field.clone(),
                        original: original.clone(),
                    });
                    self.project.scene.node_mut(node).components[component]
                        .as_reflect_mut()
                        .set(&field, Value::F32(value));
                }
                AuroraEvent::Clicked(id) if Some(id) == self.rows.code_save => {
                    self.save_script();
                }
                AuroraEvent::Clicked(id) if self.find_button(id).is_some() => {
                    self.find_action(id);
                }
                AuroraEvent::Clicked(id)
                    if self.rows.problem_rows.iter().any(|(row, _)| *row == id) =>
                {
                    if let Some(&(_, span)) =
                        self.rows.problem_rows.iter().find(|(row, _)| *row == id)
                    {
                        self.reveal_span(span);
                    }
                }
                AuroraEvent::Clicked(id) if self.symbol_row(id).is_some() => {
                    if let Some(name) = self.symbol_row(id) {
                        self.go_to_symbol(&name);
                    }
                }
                AuroraEvent::Clicked(id) if self.completion_row(id).is_some() => {
                    if let Some(label) = self.completion_row(id) {
                        self.accept_completion(&label);
                    }
                }
                AuroraEvent::Submitted(id) if Some(id) == self.rows.find.query => {
                    let text = self.ui.text_of(id).unwrap_or_default().to_string();
                    let caret = self.editor_caret();
                    if let Some(find) = &mut self.find {
                        find.set_query(text, &self.script_text, caret);
                        self.reveal_find_match();
                    }
                }
                AuroraEvent::Submitted(id) if Some(id) == self.rows.find.replacement => {
                    let text = self.ui.text_of(id).unwrap_or_default().to_string();
                    if let Some(find) = &mut self.find {
                        find.set_replacement(text);
                    }
                }
                AuroraEvent::Toggled { id, checked } if Some(id) == self.rows.find.match_case => {
                    if let Some(find) = &mut self.find {
                        find.set_match_case(checked, &self.script_text);
                        self.reveal_find_match();
                    }
                }
                AuroraEvent::Toggled { id, checked } if Some(id) == self.rows.find.whole_word => {
                    if let Some(find) = &mut self.find {
                        find.set_whole_word(checked, &self.script_text);
                        self.reveal_find_match();
                    }
                }
                AuroraEvent::Clicked(id)
                    if self.rows.component_removes.iter().any(|(w, ..)| *w == id) =>
                {
                    let (node, index) = self
                        .rows
                        .component_removes
                        .iter()
                        .find(|(w, ..)| *w == id)
                        .map(|(_, n, i)| (*n, *i))
                        .expect("guarded by the match arm");
                    self.history
                        .remove_component(&mut self.project.scene, node, index);
                    self.dirty = true;
                }
                AuroraEvent::Clicked(id) if Some(id) == self.rows.settings_button => {
                    self.open_settings_modal();
                }
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
                AuroraEvent::Clicked(id) if Some(id) == self.rows.modal_close => self.close_modal(),
                AuroraEvent::Clicked(id)
                    if self.rows.modal_buttons.iter().any(|(w, _)| *w == id) =>
                {
                    let action = self
                        .rows
                        .modal_buttons
                        .iter()
                        .find(|(w, _)| *w == id)
                        .map(|(_, a)| *a)
                        .expect("guarded by the match arm");
                    self.run_modal_action(action);
                }
                AuroraEvent::Toggled { id, checked }
                    if self.rows.modal_checks.iter().any(|(w, _)| *w == id) =>
                {
                    let field = self
                        .rows
                        .modal_checks
                        .iter()
                        .find(|(w, _)| *w == id)
                        .map(|(_, f)| *f)
                        .expect("guarded by the match arm");
                    if let Some(draft) = self.modal.as_mut().and_then(|m| m.settings_draft_mut()) {
                        draft.set_checked(field, checked);
                    }
                    // No rebuild: Aurora flips the checkbox's own visual state,
                    // and a rebuild here would re-render the numeric inputs from
                    // the draft, discarding any number the user was mid-typing.
                }
                AuroraEvent::Clicked(id)
                    if self.rows.color_swatches.iter().any(|(w, _)| *w == id) =>
                {
                    let target = self
                        .rows
                        .color_swatches
                        .iter()
                        .find(|(w, _)| *w == id)
                        .map(|(_, t)| t.clone())
                        .expect("guarded by the match arm");
                    self.open_color_picker(target);
                }
                AuroraEvent::Clicked(id)
                    if self.rows.asset_browse.iter().any(|(w, ..)| *w == id) =>
                {
                    let (node, component, field) = self
                        .rows
                        .asset_browse
                        .iter()
                        .find(|(w, ..)| *w == id)
                        .map(|(_, n, c, f)| (*n, *c, f.clone()))
                        .expect("guarded by the match arm");
                    self.open_asset_chooser(node, component, &field);
                }
                AuroraEvent::Clicked(id)
                    if self.rows.asset_clear.iter().any(|(w, ..)| *w == id) =>
                {
                    let (node, component, field) = self
                        .rows
                        .asset_clear
                        .iter()
                        .find(|(w, ..)| *w == id)
                        .map(|(_, n, c, f)| (*n, *c, f.clone()))
                        .expect("guarded by the match arm");
                    self.commit_asset_field(node, component, &field, String::new());
                }
                AuroraEvent::Clicked(id)
                    if self.rows.asset_choices.iter().any(|(w, _)| *w == id) =>
                {
                    let rel = self
                        .rows
                        .asset_choices
                        .iter()
                        .find(|(w, _)| *w == id)
                        .map(|(_, p)| p.clone())
                        .expect("guarded by the match arm");
                    if let Some(t) = self.modal.as_ref().and_then(|m| m.asset_target()) {
                        self.commit_asset_field(t.node, t.component, &t.field, rel);
                    }
                    self.close_modal();
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
                    if let Some(&(_, field)) = self.rows.modal_inputs.iter().find(|(w, _)| *w == id)
                    {
                        if let WidgetKind::TextInput(text) = self.ui.kind(id)
                            && let Ok(value) = text.trim().parse::<f32>()
                            && let Some(draft) =
                                self.modal.as_mut().and_then(|m| m.settings_draft_mut())
                        {
                            draft.set_number(field, value);
                        }
                        continue;
                    }
                    self.commit_field(id);
                    self.commit_transform_field(id);
                    self.commit_vec_input(id);
                    self.commit_picker_hex(id);
                }
                AuroraEvent::Dropped { source, target } => {
                    // Tab drags are routed by the tab bar, not by Aurora's
                    // generic drag, so anything dropped here is a file.
                    self.drop_file(source, target);
                }
                _ => {}
            }
        }
    }

    /// Commit a submitted Vec2 axis input: parse its text into the pressed
    /// component and commit the whole Vec2 through the history. A no-op for
    /// other widgets or unparseable text.
    fn commit_vec_input(&mut self, id: aurora::WidgetId) {
        let Some((_, field, axis)) = self.rows.vec_inputs.iter().find(|(w, ..)| *w == id) else {
            return;
        };
        let (field, axis) = (field.clone(), *axis);
        let WidgetKind::TextInput(text) = self.ui.kind(id) else {
            return;
        };
        let Ok(parsed) = text.trim().parse::<f32>() else {
            return;
        };
        let current = field_vec2(&self.project.scene, &field);
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
        let Some((_, node, component, field)) = self
            .rows
            .field_rows
            .iter()
            .find(|(widget, ..)| *widget == id)
        else {
            return;
        };
        let (node, component, field) = (*node, *component, field.clone());
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
            .and_then(|c| c.as_reflect().get(&field))
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
                .set_field(&mut self.project.scene, node, component, &field, new);
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
/// text as a run of character events.
///
/// The modifiers widen what a key means rather than changing it: ctrl takes an
/// arrow from a character to a word and a delete from a character to a word,
/// and ctrl+shift takes that delete out to the rest of the line. One shape to
/// learn, three scopes.
fn translate_key(event: &winit::event::KeyEvent, ctrl: bool, shift: bool) -> Vec<InputEvent> {
    let named = match &event.logical_key {
        WinitKey::Named(NamedKey::Backspace) if ctrl && shift => Some(Key::DeleteToLineStart),
        WinitKey::Named(NamedKey::Delete) if ctrl && shift => Some(Key::DeleteToLineEnd),
        WinitKey::Named(NamedKey::Backspace) if ctrl => Some(Key::DeleteWordLeft),
        WinitKey::Named(NamedKey::Delete) if ctrl => Some(Key::DeleteWordRight),
        WinitKey::Named(NamedKey::Backspace) => Some(Key::Backspace),
        WinitKey::Named(NamedKey::Delete) => Some(Key::Delete),
        // ctrl+Enter opens a line below without splitting the one you are on;
        // ctrl+shift+Enter opens one above.
        WinitKey::Named(NamedKey::Enter) if ctrl && shift => Some(Key::LineAbove),
        WinitKey::Named(NamedKey::Enter) if ctrl => Some(Key::LineBelow),
        WinitKey::Named(NamedKey::ArrowLeft) if ctrl => Some(Key::WordLeft),
        WinitKey::Named(NamedKey::ArrowRight) if ctrl => Some(Key::WordRight),
        WinitKey::Named(NamedKey::ArrowLeft) => Some(Key::Left),
        WinitKey::Named(NamedKey::ArrowRight) => Some(Key::Right),
        WinitKey::Named(NamedKey::ArrowUp) => Some(Key::Up),
        WinitKey::Named(NamedKey::ArrowDown) => Some(Key::Down),
        // ctrl+Home / ctrl+End reach the ends of the document; plain Home is
        // smart-Home inside a text area.
        WinitKey::Named(NamedKey::Home) if ctrl => Some(Key::DocumentStart),
        WinitKey::Named(NamedKey::End) if ctrl => Some(Key::DocumentEnd),
        WinitKey::Named(NamedKey::Home) => Some(Key::Home),
        WinitKey::Named(NamedKey::End) => Some(Key::End),
        WinitKey::Named(NamedKey::PageUp) => Some(Key::PageUp),
        WinitKey::Named(NamedKey::PageDown) => Some(Key::PageDown),
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

#[cfg(test)]
mod tests {
    use super::{
        Case, TextEncoding, coalesces, completion_anchor, join_block, narrowest_at,
        step_symbol_target, tidy_for_save, toggle_comment_block, transposition, word_before,
        word_before_or_around, write_atomic,
    };
    use comet::{Diagnostic, Span};

    #[test]
    fn typing_a_word_joins_one_undo_step_and_anything_else_starts_a_new_one() {
        // Undo should go back a word at a time, not a letter at a time.
        assert!(coalesces("let x", "let xy"));
        assert!(coalesces("let x", "let x_"));
        // Whitespace and punctuation end a run - they are where words end.
        assert!(!coalesces("let x", "let x "));
        assert!(!coalesces("let x", "let x."));
        // A deletion, a paste, or an edit anywhere but the end is its own step.
        assert!(!coalesces("let xy", "let x"));
        assert!(!coalesces("let x", "let xyz"));
        assert!(!coalesces("let x", "Xlet x"));
    }

    #[test]
    fn commenting_a_block_keeps_its_shape_and_round_trips() {
        let block = "    let a = 1;\n        let b = 2;";
        let commented = toggle_comment_block(block);
        assert_eq!(commented, "    // let a = 1;\n    //     let b = 2;");
        assert_eq!(
            toggle_comment_block(&commented),
            block,
            "uncommenting takes back exactly what commenting added"
        );
    }

    #[test]
    fn a_half_commented_block_gets_commented_rather_than_flipped() {
        // Flipping each line would leave the same mess inverted.
        let block = "// a\nb";
        assert_eq!(toggle_comment_block(block), "// // a\n// b");
    }

    #[test]
    fn a_blank_line_in_a_block_gains_no_trailing_comment() {
        assert_eq!(toggle_comment_block("a\n\nb"), "// a\n\n// b");
        assert_eq!(toggle_comment_block("// a\n\n// b"), "a\n\nb");
    }

    #[test]
    fn the_narrowest_diagnostic_wins_where_they_overlap() {
        // A wide "this function must return f32" spanning a whole body must not
        // hide the precise type error inside it - the narrow one is the one that
        // says what to change.
        let wide = Diagnostic::error(Span::new(0, 100), "must return `f32`");
        let narrow = Diagnostic::error(Span::new(40, 45), "expected `f32`, found `bool`");
        let all = vec![wide, narrow];
        assert_eq!(
            narrowest_at(&all, 42).map(|d| d.message.as_str()),
            Some("expected `f32`, found `bool`")
        );
        assert_eq!(
            narrowest_at(&all, 10).map(|d| d.message.as_str()),
            Some("must return `f32`"),
            "outside the narrow one, the wide one still applies"
        );
        assert!(narrowest_at(&all, 200).is_none());
    }

    #[test]
    fn a_zero_width_diagnostic_is_still_found_at_its_offset() {
        // An unclosed brace is reported at the end of the file with an empty
        // span; hovering it has to still find it.
        let all = vec![Diagnostic::error(Span::new(10, 10), "expected `}`")];
        assert!(narrowest_at(&all, 10).is_some());
        assert!(narrowest_at(&all, 11).is_none());
    }

    #[test]
    fn a_files_line_endings_and_bom_survive_a_round_trip() {
        // The buffer always holds LF - a carriage return inside every line
        // shifts every column by one and makes comet report diagnostics one
        // character off - but the file gets back what it came with.
        let crlf = b"let a = 1;\r\nlet b = 2;\r\n";
        let (text, enc) = TextEncoding::decode(crlf).expect("valid utf-8");
        assert_eq!(text, "let a = 1;\nlet b = 2;\n", "LF in the buffer");
        assert!(enc.crlf);
        assert_eq!(enc.encode(&text).as_bytes(), crlf);

        // A BOM is stripped so it is not a stray character the lexer squiggles,
        // and written back so the file is not silently rewritten.
        let bom = "\u{feff}let a = 1;\n".as_bytes();
        let (text, enc) = TextEncoding::decode(bom).unwrap();
        assert_eq!(text, "let a = 1;\n", "no invisible character in the buffer");
        assert!(enc.bom);
        assert_eq!(enc.encode(&text).as_bytes(), bom);

        // A plain LF file stays exactly as it was.
        let plain = b"let a = 1;\n";
        let (text, enc) = TextEncoding::decode(plain).unwrap();
        assert_eq!(enc, TextEncoding::default());
        assert_eq!(enc.encode(&text).as_bytes(), plain);

        // Mixed endings normalise to whichever dominates.
        let (_, enc) = TextEncoding::decode(b"a\r\nb\r\nc\n").unwrap();
        assert!(enc.crlf, "two of three lines are CRLF");
    }

    #[test]
    fn bytes_that_are_not_utf8_are_refused_rather_than_mangled() {
        // Opening them as replacement characters would then save that back over
        // the original, which is how a file gets destroyed by being looked at.
        assert!(TextEncoding::decode(&[0xff, 0xfe, 0x00]).is_none());
    }

    #[test]
    fn a_save_never_leaves_the_file_half_written() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("script.cmt");
        write_atomic(&path, b"first").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"first");
        write_atomic(&path, b"second, longer").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"second, longer");

        // And the temp sibling is gone, and was a dotfile so the explorer -
        // which hides those - never flickered it into the list.
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n != "script.cmt")
            .collect();
        assert!(leftovers.is_empty(), "left behind: {leftovers:?}");
    }

    #[test]
    fn a_completion_popup_is_wanted_only_where_a_name_is_being_typed() {
        // The stale-popup bug: a popup anchored eleven lines up stayed open
        // after the caret was clicked into whitespace. There is now one
        // predicate for "should there be a popup", so every place that might
        // close one gets the same answer.
        assert_eq!(completion_anchor("pos.x", 5), Some(4), "mid-name");
        assert_eq!(completion_anchor("pos.", 4), Some(4), "just after a dot");
        assert_eq!(completion_anchor("let ", 4), None, "after a space");
        assert_eq!(completion_anchor("}\n\n", 3), None, "in empty space");
        assert_eq!(completion_anchor("", 0), None);
        assert_eq!(completion_anchor("a = 1;", 6), None, "after punctuation");
    }

    #[test]
    fn the_word_before_the_caret_is_what_a_completion_replaces() {
        assert_eq!(word_before("pos.x", 5), (4, "x"));
        assert_eq!(word_before("let speed", 9), (4, "speed"));
        // Right after a separator there is no word yet.
        assert_eq!(word_before("pos.", 4), (4, ""));
        assert_eq!(word_before("", 0), (0, ""));
        // Past the end is clamped rather than panicking.
        assert_eq!(word_before("abc", 99), (0, "abc"));
    }

    #[test]
    fn stepping_by_declaration_walks_the_file_and_stops_at_each_end() {
        let source = "let speed: f32 = 1.0;\nfunc update(dt: f32) { }\nfunc reset() { }";
        let symbols = comet::service::symbols(source);
        let at = |name: &str| source.find(name).expect("in the fixture");

        // From the top, forward, one declaration at a time.
        assert_eq!(
            step_symbol_target(&symbols, 0, true),
            Some(at("speed")),
            "the first declaration"
        );
        assert_eq!(
            step_symbol_target(&symbols, at("speed"), true),
            Some(at("update")),
            "strictly past, so holding the key does not stick"
        );
        assert_eq!(
            step_symbol_target(&symbols, at("reset"), true),
            None,
            "past the last one, nothing - wrapping would look like not moving"
        );

        // And back up.
        assert_eq!(
            step_symbol_target(&symbols, at("reset"), false),
            Some(at("update"))
        );
        assert_eq!(step_symbol_target(&symbols, 0, false), None);
    }

    #[test]
    fn saving_strips_trailing_whitespace_and_fixes_the_file_ending() {
        // Four trailing newlines collapse to the one that terminates the last
        // line - no blank line after it.
        let (out, _) = tidy_for_save("let a = 1.0;   \nlet b = 2.0;\t\n\n\n\n", 0);
        assert_eq!(out, "let a = 1.0;\nlet b = 2.0;\n");

        // A file with no final newline gets exactly one.
        let (out, _) = tidy_for_save("func f() { }", 0);
        assert_eq!(out, "func f() { }\n");

        // An empty buffer stays empty rather than becoming a blank line.
        let (out, _) = tidy_for_save("", 0);
        assert_eq!(out, "");
    }

    #[test]
    fn tidying_leaves_the_caret_on_the_same_character() {
        // Caret on the `2` of the second line, after a line that loses three
        // trailing spaces.
        let text = "let a = 1.0;   \nlet b = 2.0;\n";
        let caret = text.find("2.0").expect("in the fixture");
        let (out, moved) = tidy_for_save(text, caret);
        assert_eq!(&out[moved..moved + 3], "2.0");

        // A caret standing inside a run that gets trimmed lands at the trim.
        let (out, moved) = tidy_for_save("let a = 1.0;   \n", 14);
        assert_eq!(moved, out.find('\n').expect("a newline"));
    }

    #[test]
    fn a_fresh_auto_indent_under_the_caret_is_not_yanked_away() {
        // Pressing Enter inside a block leaves the caret on a line of nothing
        // but spaces. Saving there must not pull it back to the margin.
        let text = "func f() {\n    \n}\n";
        let caret = text.find("\n}").expect("in the fixture");
        let (out, moved) = tidy_for_save(text, caret);
        assert_eq!(out, text, "the caret's own blank line is left alone");
        assert_eq!(moved, caret);

        // With the caret elsewhere, the same line is stripped.
        let (out, _) = tidy_for_save(text, 0);
        assert_eq!(out, "func f() {\n\n}\n");
    }

    #[test]
    fn joining_lines_collapses_the_indent_rather_than_leaving_double_spaces() {
        assert_eq!(
            join_block("    let x =\n        compute(a);"),
            "    let x = compute(a);"
        );
        // The first line keeps its own indentation: a join is about the seam,
        // not about moving the block to the margin.
        assert_eq!(join_block("        a\n        b"), "        a b");
        // A blank line contributes nothing, rather than a stray space.
        assert_eq!(join_block("a\n\nb"), "a b");
        assert_eq!(join_block("a\n"), "a");
    }

    #[test]
    fn transpose_swaps_across_the_caret_and_behind_it_at_a_line_end() {
        // "teh|" -> the two before the caret, because there is nothing after.
        let (range, text) = transposition("teh", 3).expect("something to swap");
        assert_eq!((range, text.as_str()), (1..3, "he"));

        // Mid-line, it swaps across the caret.
        let (range, text) = transposition("abcd", 2).expect("something to swap");
        assert_eq!((range, text.as_str()), (1..3, "cb"));

        // At the very start of a line there is nothing behind it.
        assert!(transposition("abc", 0).is_none());
        assert!(transposition("ab\ncd", 3).is_none());
    }

    #[test]
    fn transpose_moves_whole_characters_not_bytes() {
        // Splitting a multi-byte character in half would panic on the slice.
        // Written as an escape so this file stays ASCII: \u{fc} is a two-byte
        // `u` with an umlaut, the character that crashed the lexer once.
        let (range, text) = transposition("a\u{fc}", 3).expect("something to swap");
        assert_eq!(text, "\u{fc}a");
        assert_eq!(range, 0..3);
    }

    #[test]
    fn changing_case_covers_the_three_and_title_case_treats_underscore_as_a_break() {
        assert_eq!(Case::Upper.apply("max_speed"), "MAX_SPEED");
        assert_eq!(Case::Lower.apply("MAX_SPEED"), "max_speed");
        assert_eq!(Case::Title.apply("max_speed limit"), "Max_Speed Limit");
    }

    #[test]
    fn the_word_for_a_recase_reaches_forward_as_well_as_back() {
        let text = "let speed = 1.0;";
        let at = text.find("speed").expect("in the fixture");
        // At the start of the word, which word_before alone would miss.
        assert_eq!(word_before_or_around(text, at), (at, "speed"));
        // And in the middle of it.
        assert_eq!(word_before_or_around(text, at + 2), (at, "speed"));
        // Just after a word still finds it - that is where the caret is when
        // you finish typing a name and reach for a recase.
        assert_eq!(word_before_or_around(text, 3), (0, "let"));
        // Surrounded by whitespace, there is no word.
        assert_eq!(word_before_or_around("a  b", 2).1, "");
    }

    #[test]
    fn every_demo_script_still_compiles() {
        // The scripts shipped with the demo project are the first Comet anyone
        // reads, and nothing else compiles them - so a language change could
        // break them silently. squiggles.cmt is wrong on purpose and is the one
        // that must NOT compile.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("demo_project/scripts");
        let mut checked = 0;
        for entry in std::fs::read_dir(&dir).expect("the demo project has a scripts folder") {
            let path = entry.expect("a readable entry").path();
            if path.extension().is_none_or(|e| e != "cmt") {
                continue;
            }
            let source = std::fs::read_to_string(&path).expect("readable");
            let name = path
                .file_name()
                .expect("a name")
                .to_string_lossy()
                .to_string();
            let result = comet::compile(&source, helios::script_schema());
            if name == "squiggles.cmt" {
                assert!(result.is_err(), "squiggles.cmt is wrong on purpose");
            } else {
                assert!(result.is_ok(), "{name} should compile: {:?}", result.err());
            }
            checked += 1;
        }
        assert!(
            checked >= 2,
            "expected to find the demo scripts, found {checked}"
        );
    }
}
