//! atlas editor UI: builds the docked shell (scene-tree, viewport, inspector,
//! file explorer) from the current scene, selection, and project directory
//! (M3 step 6 - "the editor looks like an editor").

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use aurora::TabBar;
use aurora::{Color, ImageHandle, Orientation, Style, Theme, Ui, WidgetId};
use glam::Vec2;
use helios::{NodeId, Scene, Value};

use crate::console::LogLine;
use crate::dock::{DockNode, Fixed, Pane, SplitDir};
use crate::explorer::{Entry, FileExplorer, FileKind, FileView, TreeRow, can_thumbnail, classify};
use crate::icons::{Icon, Icons};
use crate::modal::{
    AssetChooser, AssetKind, Modal, ModalAction, ModalBody, SettingField, SettingsDraft,
};
use crate::thumbnails::Thumbnails;
use crate::viewport::GizmoMode;

/// The newest console lines the pane builds a widget for (the ring stores more
/// for scrollback; older lines are summarized with a count).
const CONSOLE_TAIL: usize = 500;

/// The rendered size of a toolbar button's icon. Kept generous so the icon
/// detail (arrowheads, gear teeth) stays legible rather than washing out.
const TOOLBAR_ICON: f32 = 24.0;

/// The editor's resolved runtime theme: aurora's [`Theme`] (which carries the
/// whole general-purpose palette - controls, chrome surfaces, text, borders, and
/// shape radii) plus the few colors that only mean something in a scene editor.
/// The UI reads these concrete values; they are produced by resolving the
/// authored [`ThemeDoc`](crate::theme::ThemeDoc) loaded from the user's settings
/// file (see the `theme` and `settings` modules), and swapped as a unit so the
/// whole editor recolors at once.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EditorTheme {
    /// The general UI palette: every color and radius that is not specific to
    /// editing a scene. Widgets that live in aurora are themed entirely by this.
    pub aurora: Theme,
    /// The reparent drop-line / drop-into highlight.
    pub row_drop: Color,
    /// The active gizmo-mode toolbar button's background.
    pub mode_active: Color,
    /// Engine axis palette (X red, Y green) for the inspector's Vec2 labels.
    pub axis_x: Color,
    pub axis_y: Color,
    /// The console pane's WARN-level line color (ERROR reuses `axis_x`, INFO
    /// the theme's heading, lower levels its subhead).
    pub console_warn: Color,
    /// Syntax colors for the Code pane. Identifiers and punctuation use the
    /// theme's own heading color, so only what should stand out is named here.
    pub code_keyword: Color,
    pub code_number: Color,
    pub code_string: Color,
    pub code_comment: Color,
    pub code_function: Color,
    pub code_type: Color,
    /// `@export` and friends: metadata about a declaration rather than part of
    /// what runs, so it gets a color of its own.
    pub code_annotation: Color,
    /// The squiggle under an error and under a warning.
    pub code_error: Color,
    pub code_warning: Color,
    /// The faint backing behind every occurrence of the word under the caret.
    pub code_occurrence: Color,
    /// Viewport (scene-view) colors. Stored as [`Color`] like the rest of the
    /// theme, converted to photon colors where the scene is drawn.
    /// The clear color behind the scene.
    pub viewport_bg: Color,
    /// Minor and major editor grid lines.
    pub grid_line: Color,
    pub grid_line_strong: Color,
    /// The selected sprite's outline, and the rotate/scale gizmo handles.
    pub selection_outline: Color,
    pub gizmo_rotate: Color,
    pub gizmo_scale: Color,
}

fn default_console_warn() -> Color {
    Color::rgb(0.85, 0.72, 0.30)
}

impl EditorTheme {
    /// The default dark editor theme.
    pub fn dark() -> Self {
        Self {
            aurora: Theme::dark(),
            row_drop: Color::rgb(0.18, 0.40, 0.30),
            mode_active: Color::rgb(0.22, 0.38, 0.60),
            axis_x: Color::rgb(0.90, 0.32, 0.32),
            axis_y: Color::rgb(0.35, 0.70, 0.38),
            console_warn: default_console_warn(),
            code_keyword: Color::rgb(0.78, 0.55, 0.90),
            code_number: Color::rgb(0.72, 0.85, 0.62),
            code_string: Color::rgb(0.82, 0.68, 0.45),
            code_comment: Color::rgb(0.45, 0.50, 0.48),
            code_function: Color::rgb(0.45, 0.72, 0.95),
            code_type: Color::rgb(0.40, 0.80, 0.78),
            code_annotation: Color::rgb(0.86, 0.58, 0.72),
            code_error: Color::rgb(0.92, 0.36, 0.36),
            code_warning: Color::rgb(0.90, 0.72, 0.32),
            code_occurrence: Color::rgba(0.55, 0.60, 0.70, 0.28),
            viewport_bg: Color::rgb(0.052, 0.052, 0.055),
            grid_line: Color::rgba(0.0, 0.0, 0.0, 0.10),
            grid_line_strong: Color::rgba(0.0, 0.0, 0.0, 0.22),
            selection_outline: Color::rgb(0.30, 0.55, 0.90),
            gizmo_rotate: Color::rgb(0.35, 0.55, 1.0),
            gizmo_scale: Color::rgb(1.0, 0.75, 0.25),
        }
    }

    /// A bundled light editor theme - a preset a user can copy into their
    /// settings file (the editor loads whatever theme that file holds).
    #[allow(dead_code)]
    pub fn light() -> Self {
        Self {
            aurora: Theme::light(),
            row_drop: Color::rgb(0.55, 0.82, 0.66),
            mode_active: Color::rgb(0.62, 0.76, 0.96),
            axis_x: Color::rgb(0.85, 0.30, 0.30),
            axis_y: Color::rgb(0.28, 0.62, 0.34),
            console_warn: Color::rgb(0.72, 0.55, 0.10),
            code_keyword: Color::rgb(0.50, 0.20, 0.62),
            code_number: Color::rgb(0.20, 0.45, 0.18),
            code_string: Color::rgb(0.60, 0.35, 0.10),
            code_comment: Color::rgb(0.48, 0.53, 0.50),
            code_function: Color::rgb(0.12, 0.38, 0.70),
            code_type: Color::rgb(0.05, 0.48, 0.46),
            code_annotation: Color::rgb(0.62, 0.20, 0.42),
            code_error: Color::rgb(0.78, 0.16, 0.16),
            code_warning: Color::rgb(0.70, 0.50, 0.10),
            code_occurrence: Color::rgba(0.45, 0.50, 0.60, 0.22),
            viewport_bg: Color::rgb(0.86, 0.87, 0.89),
            grid_line: Color::rgba(0.0, 0.0, 0.0, 0.08),
            grid_line_strong: Color::rgba(0.0, 0.0, 0.0, 0.16),
            selection_outline: Color::rgb(0.20, 0.50, 0.90),
            gizmo_rotate: Color::rgb(0.25, 0.45, 0.95),
            gizmo_scale: Color::rgb(0.90, 0.62, 0.10),
        }
    }
}

impl Default for EditorTheme {
    fn default() -> Self {
        Self::dark()
    }
}

/// One axis of a 2D value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    X,
    Y,
}

/// A reference to a `Vec2`-valued field the inspector edits, resolved against
/// the scene to read or write it (for both typed commits and label drag-scrub).
/// Not `Copy`: a component's field names are owned now, because a `Script`'s
/// are whatever its source exports rather than a fixed list.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldRef {
    /// The node's transform translation.
    Position(NodeId),
    /// The node's transform scale.
    Scale(NodeId),
    /// A `Vec2` field of one of the node's components.
    Component {
        node: NodeId,
        index: usize,
        field: String,
    },
}

/// This value's `axis` component.
pub fn axis_of(v: Vec2, axis: Axis) -> f32 {
    match axis {
        Axis::X => v.x,
        Axis::Y => v.y,
    }
}

/// `v` with its `axis` component replaced by `val`.
pub fn with_axis(v: Vec2, axis: Axis, val: f32) -> Vec2 {
    match axis {
        Axis::X => Vec2::new(val, v.y),
        Axis::Y => Vec2::new(v.x, val),
    }
}

/// The current value of a `Vec2` field.
pub fn field_vec2(scene: &Scene, r: &FieldRef) -> Vec2 {
    match r {
        FieldRef::Position(n) => scene.node(*n).transform.translation,
        FieldRef::Scale(n) => scene.node(*n).transform.scale,
        FieldRef::Component { node, index, field } => {
            match scene.node(*node).components[*index].as_reflect().get(field) {
                Some(Value::Vec2(v)) => v,
                _ => Vec2::ZERO,
            }
        }
    }
}

/// Write a `Vec2` field directly (no undo step) - used for live drag-scrub;
/// the caller commits the final value through the history on release.
pub fn set_field_vec2(scene: &mut Scene, r: &FieldRef, v: Vec2) {
    match r {
        FieldRef::Position(n) => scene.node_mut(*n).transform.translation = v,
        FieldRef::Scale(n) => scene.node_mut(*n).transform.scale = v,
        FieldRef::Component { node, index, field } => {
            scene.node_mut(*node).components[*index]
                .as_reflect_mut()
                .set(field, Value::Vec2(v));
        }
    }
}

/// A `Color`-valued component field the color picker edits.
#[derive(Debug, Clone, PartialEq)]
pub struct ColorTarget {
    pub node: NodeId,
    pub index: usize,
    pub field: String,
}

/// The current value of a color field (opaque black if it is not a `Color`).
pub fn field_color(scene: &Scene, t: &ColorTarget) -> [f32; 4] {
    match scene.node(t.node).components[t.index]
        .as_reflect()
        .get(&t.field)
    {
        Some(Value::Color(c)) => c,
        _ => [0.0, 0.0, 0.0, 1.0],
    }
}

/// Write a color field directly (no undo step) - the picker commits the final
/// value through the history when it closes.
pub fn set_field_color(scene: &mut Scene, t: &ColorTarget, rgba: [f32; 4]) {
    scene.node_mut(t.node).components[t.index]
        .as_reflect_mut()
        .set(&t.field, Value::Color(rgba));
}

/// Where a dragged tree row will drop, relative to the row under the cursor:
/// as a child of it, or as a sibling just before or after it (for reordering).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DropSpot {
    Into(NodeId),
    Before(NodeId),
    After(NodeId),
}

/// Resolve a drop into `(new_parent, index)` for `History::reparent`, or `None`
/// if it is invalid (onto itself, into its own descendant, or relative to
/// itself). The index is into the parent's child list AFTER `source` is
/// unlinked - exactly what `reparent` (unlink-then-link) needs, so a same-level
/// reorder lands correctly.
pub fn resolve_drop(scene: &Scene, source: NodeId, spot: DropSpot) -> Option<(NodeId, usize)> {
    match spot {
        DropSpot::Into(n) => {
            if n == source || scene.is_ancestor(source, n) {
                return None;
            }
            Some((n, scene.children(n).len()))
        }
        DropSpot::Before(n) | DropSpot::After(n) => {
            if n == source {
                return None;
            }
            let parent = scene.parent(n)?;
            if parent == source || scene.is_ancestor(source, parent) {
                return None;
            }
            let siblings: Vec<NodeId> = scene
                .children(parent)
                .iter()
                .copied()
                .filter(|&c| c != source)
                .collect();
            let pos = siblings.iter().position(|&c| c == n)?;
            Some((
                parent,
                pos + usize::from(matches!(spot, DropSpot::After(_))),
            ))
        }
    }
}

/// The scene tree's editor state, kept across shell rebuilds (the same lesson
/// as panel sizes and scroll): which nodes are collapsed, and the current
/// reparent drop spot (a row highlight for Into, an insertion line for
/// Before/After). Cheap to clone (a handful of node ids) at each rebuild.
#[derive(Debug, Clone, Default)]
pub struct TreeView {
    pub collapsed: HashSet<NodeId>,
    pub drop_target: Option<DropSpot>,
}

/// A collapsible section of the inspector: the selected node's transform, or one
/// of its components (by index).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InspectorSection {
    Transform,
    Component(usize),
}

/// The inspector's editor state kept across shell rebuilds: which sections are
/// collapsed. Keyed by `(node, section)` so collapse is per node - collapsing a
/// component on one entity does not fold it on another (which may not even have
/// it). Cheap to clone at each rebuild (the same pattern as [`TreeView`]).
#[derive(Debug, Clone, Default)]
pub struct InspectorView {
    pub collapsed: HashSet<(NodeId, InspectorSection)>,
}

/// Maps widget handles back to the domain data they represent, so the app can
/// react to Aurora events without Aurora knowing about `Scene`/`NodeId`.
#[derive(Default)]
pub struct EditorRows {
    /// A clicked tree row selects this node (and a press begins a reparent
    /// drag); the row rect also serves as the drop target during a drag.
    pub tree_rows: Vec<(WidgetId, NodeId)>,
    /// A clicked disclosure triangle collapses/expands this node.
    pub tree_toggles: Vec<(WidgetId, NodeId)>,
    /// The scene-tree search box; its live text filters the tree by name.
    pub tree_filter: Option<WidgetId>,
    /// The inline rename field (a text input over a tree row) and the node it
    /// renames, present only while a rename is in progress.
    pub rename_field: Option<(WidgetId, NodeId)>,
    /// A clicked eye icon toggles this node's visibility.
    pub eye_toggles: Vec<(WidgetId, NodeId)>,
    /// A clicked inspector section header collapses/expands that section.
    pub section_toggles: Vec<(WidgetId, InspectorSection)>,
    /// A submitted field row commits its current text to
    /// `(node, component index, field name)`.
    pub field_rows: Vec<(WidgetId, NodeId, usize, String)>,
    /// Exported values shown as a slider, because the script asked with
    /// `@range`. Separate from `field_rows` because a slider reports a value
    /// rather than text.
    pub field_sliders: Vec<(WidgetId, NodeId, usize, String)>,
    /// Inspector asset (`Value::Asset`) fields: each is a drop target that,
    /// when an explorer image is dropped on it, sets `(node, component, field)`.
    pub asset_fields: Vec<(WidgetId, NodeId, usize, String)>,
    /// The "browse" (open chooser) and "clear" buttons of each asset field.
    pub asset_browse: Vec<(WidgetId, NodeId, usize, String)>,
    pub asset_clear: Vec<(WidgetId, NodeId, usize, String)>,
    /// The asset chooser's grid cells, each mapped to the project-relative image
    /// path it selects.
    pub asset_choices: Vec<(WidgetId, String)>,
    /// A submitted transform row commits to the node's Transform; the field is
    /// `"position"`, `"rotation"`, or `"scale"`.
    pub transform_rows: Vec<(WidgetId, NodeId, &'static str)>,
    /// Dock tabs mapped to the pane each shows: a click activates it, a drag
    /// re-docks it.
    pub dock_tabs: Vec<(WidgetId, Pane)>,
    /// Each dock group's tab-strip widgets, in the order the dock is walked -
    /// the handle aurora's TabBar routes drags and animation through.
    pub dock_tab_bars: Vec<aurora::TabBarRows>,
    /// The floating copy of a tab being dragged out of its bar.
    pub carried_tab: Option<WidgetId>,
    /// Each tab group's content area mapped to the pane it currently shows: the
    /// drop region (and its rect gives the zones) a tab drag lands in.
    pub dock_groups: Vec<(WidgetId, Pane)>,
    /// The fixed-size child of each split and the split's direction, in the same
    /// depth-first order [`DockNode::set_fixed_sizes`] walks - so their laid-out
    /// sizes can be read back into the dock after a splitter drag.
    pub dock_splits: Vec<(WidgetId, SplitDir)>,
    /// Each scrollable pane and its scroll panel, so scroll survives a rebuild.
    pub pane_scrolls: Vec<(Pane, WidgetId)>,
    /// The console pane's scroll panel, if the console is visible (for the app's
    /// stick-to-bottom follow logic).
    pub console_panel: Option<WidgetId>,
    /// The viewport image widget; the app sizes the scene render target to
    /// this widget's laid-out rect each frame, so the scene draws 1:1 instead
    /// of stretching a window-sized texture into a smaller panel.
    pub viewport: Option<WidgetId>,
    /// Draggable image entries in the file explorer, dragged into the viewport
    /// to spawn a sprite: `(entry widget, project-relative path)`. A subset of
    /// `file_entries` (the image files); the same widget is in both.
    pub file_rows: Vec<(WidgetId, String)>,
    /// Folder rows in the explorer's tree: a click shows that folder's contents.
    pub file_tree_rows: Vec<(WidgetId, PathBuf)>,
    /// Folder disclosure arrows in the tree: a click expands/collapses.
    pub file_tree_toggles: Vec<(WidgetId, PathBuf)>,
    /// Contents-pane entries (folders and files): a click navigates into a
    /// folder or selects a file. `bool` is whether the entry is a directory.
    pub file_entries: Vec<(WidgetId, PathBuf, bool)>,
    /// Breadcrumb segments: a click jumps to that ancestor folder.
    pub file_crumbs: Vec<(WidgetId, PathBuf)>,
    /// The explorer's view toggles and refresh button.
    pub file_view_list: Option<WidgetId>,
    pub file_view_grid: Option<WidgetId>,
    pub file_refresh: Option<WidgetId>,
    /// The contents scroll panel, whose laid-out width sets the grid column
    /// count (read back each frame, like the dock split sizes).
    pub file_contents: Option<WidgetId>,
    /// The folder-tree scroll panel, whose offset is captured each frame so the
    /// tree does not jump to the top on every shell rebuild.
    pub file_tree_scroll: Option<WidgetId>,
    /// The inline rename field over a contents entry and the path it renames,
    /// present only while a file rename is in progress.
    pub file_rename_field: Option<(WidgetId, PathBuf)>,
    /// The explorer pane's root widget: a press inside it gives the explorer
    /// keyboard focus (so its shortcuts act on files, not scene nodes).
    pub file_pane: Option<WidgetId>,
    /// The Code pane's editor and its Save button.
    pub code_editor: Option<WidgetId>,
    pub code_save: Option<WidgetId>,
    /// The toolbar actions. Adding a sprite or a script, saving and loading all
    /// live in the menus and on shortcuts; the toolbar is what the pointer is
    /// doing and what the viewport shows.
    /// The "x" in each component header, and what it takes off.
    pub component_removes: Vec<(WidgetId, NodeId, usize)>,
    /// The toolbar's view toggles (grid, world axes, snapping): highlighted when
    /// active, flipped on click.
    pub grid: Option<WidgetId>,
    pub axes: Option<WidgetId>,
    pub snap: Option<WidgetId>,
    /// The toolbar's settings gear (opens the settings modal).
    pub settings_button: Option<WidgetId>,
    /// The toolbar's gizmo-mode buttons, each mapped to the mode it selects.
    pub mode_buttons: Vec<(WidgetId, GizmoMode)>,
    /// Numeric inputs for the x/y components of a Vec2 field: submitting one
    /// commits that component of `(FieldRef, Axis)`.
    pub vec_inputs: Vec<(WidgetId, FieldRef, Axis)>,
    /// The colored x/y axis labels: pressing and dragging one scrubs that
    /// component of `(FieldRef, Axis)`.
    pub scrub_labels: Vec<(WidgetId, FieldRef, Axis)>,
    /// Status bar readouts, updated in place every frame via `set_label`.
    pub status_cursor: Option<WidgetId>,
    /// The Code pane's caret position, and the diagnostic under it.
    pub status_caret: Option<WidgetId>,
    pub status_diagnostic: Option<WidgetId>,
    pub status_zoom: Option<WidgetId>,
    pub status_selected: Option<WidgetId>,
    pub status_fps: Option<WidgetId>,
    /// Shows "unsaved changes" (brighter) when the project has edits since the
    /// last save, empty otherwise.
    pub status_modified: Option<WidgetId>,
    /// The open context-menu popup (for dismiss-on-click-outside), and its
    /// clickable item rows mapped to the action each performs.
    pub menu_popup: Option<WidgetId>,
    pub menu_items: Vec<(WidgetId, MenuAction)>,
    /// Inspector color swatches; clicking one opens the picker for that field.
    pub color_swatches: Vec<(WidgetId, ColorTarget)>,
    /// The open color picker: its popup (for dismiss), the three draggable
    /// gradient regions, and the hex input.
    /// The open color picker's widgets (aurora fills these when it builds it).
    pub picker: aurora::picker::PickerRows,
    /// The Code pane's autocomplete popup and find bar, while open.
    pub completions: aurora::list::ListRows,
    pub find: aurora::find::FindRows,
    /// The go-to-line box's field, while it is open.
    pub go_to_line: Option<WidgetId>,
    /// The go-to-symbol palette's query field, and the rows of its list.
    pub symbol_query: Option<WidgetId>,
    /// The symbol-rename field over the editor.
    pub symbol_rename: Option<WidgetId>,
    /// Each Problems row and the span it points at.
    pub problem_rows: Vec<(WidgetId, comet::Span)>,
    pub symbol_rows: aurora::list::ListRows,
    /// The open modal dialog: its card (for backdrop-click dismiss detection),
    /// close button, footer action buttons, and settings-form controls.
    pub modal_card: Option<WidgetId>,
    pub modal_close: Option<WidgetId>,
    pub modal_buttons: Vec<(WidgetId, ModalAction)>,
    pub modal_checks: Vec<(WidgetId, SettingField)>,
    pub modal_inputs: Vec<(WidgetId, SettingField)>,
}

/// An action offered by a right-click context menu (built on Aurora popups).
#[derive(Debug, Clone)]
pub enum MenuAction {
    /// Delete the current selection (all selected nodes, with their subtrees).
    DeleteSelection,
    /// Deep-copy the current selection, each new subtree beside its original.
    DuplicateSelection,
    /// Start an inline rename of this node.
    RenameNode(NodeId),
    /// Spawn a new sprite at this world position (viewport menu).
    AddSpriteAt(Vec2),
    // --- file explorer ---
    /// Open (navigate into) this folder.
    OpenFolder(PathBuf),
    /// Start an inline rename of this file or folder.
    RenameFile(PathBuf),
    /// Create a new folder in the current directory.
    NewFolder,
    /// Duplicate the selected entries.
    DuplicateFiles,
    /// Move the selected entries to the project trash.
    DeleteFiles,
    /// Copy / cut the selected entries to the file clipboard.
    CopyFiles,
    CutFiles,
    /// Paste the file clipboard into the current directory.
    PasteFiles,
}

/// An open right-click context menu: where it is anchored and what it offers.
#[derive(Debug, Clone)]
pub struct ContextMenu {
    pub anchor: Vec2,
    pub items: Vec<(String, MenuAction)>,
}

/// Read each split's fixed-child size out of a laid-out `Ui`, in the depth-first
/// order [`DockNode::set_fixed_sizes`] expects, so a splitter drag (which
/// mutates the live widget tree) can be written back into the dock and survive
/// the next rebuild - otherwise a rebuild would snap every panel to its default.
pub fn capture_dock_sizes(ui: &Ui, rows: &EditorRows) -> Vec<f32> {
    rows.dock_splits
        .iter()
        .filter_map(|&(id, dir)| {
            ui.rect(id).map(|r| match dir {
                SplitDir::Row => r.size.x,
                SplitDir::Column => r.size.y,
            })
        })
        .collect()
}

/// Read each scrollable pane's current scroll offset out of a laid-out `Ui`, so
/// scroll survives a rebuild the same way (widget-tree state dies with the tree
/// unless captured and fed back).
pub fn capture_dock_scrolls(ui: &Ui, rows: &EditorRows) -> HashMap<Pane, f32> {
    rows.pane_scrolls
        .iter()
        .map(|&(pane, id)| (pane, ui.scroll_offset(id)))
        .collect()
}

/// Build the whole editor shell for one frame from the dock layout: a tree of
/// resizable, tabbed panes. Rebuild whenever the scene or selection changes; the
/// tree is cheap enough that a fresh rebuild beats diffing (M2's inspector
/// proved this pattern's frame cost is negligible). `dock` carries the layout
/// and split sizes across rebuilds, `scrolls` the per-pane scroll offsets.
// The editor view is described by several small pieces of state; a params
// struct would not read more clearly than these named arguments.
#[allow(clippy::too_many_arguments)]
pub fn build_editor_ui(
    scene: &Scene,
    selected: &HashSet<NodeId>,
    primary: Option<NodeId>,
    viewport_handle: ImageHandle,
    explorer: &FileExplorer,
    thumbnails: &Thumbnails,
    explorer_cols: usize,
    dock: &DockNode,
    scrolls: &HashMap<Pane, f32>,
    menu: Option<&ContextMenu>,
    gizmo_mode: GizmoMode,
    show_grid: bool,
    show_axes: bool,
    snap_enabled: bool,
    icons: Option<&Icons>,
    tree: &TreeView,
    filter: &str,
    renaming: Option<NodeId>,
    inspector: &InspectorView,
    logs: &[LogLine],
    console_follow: bool,
    open_script: Option<&str>,
    script_text: &str,
    script_modified: bool,
    script_orphaned: bool,
    diagnostics: &[comet::Diagnostic],
    tab_bars: &[TabBar],
    theme: &EditorTheme,
) -> (Ui, EditorRows) {
    let mut ui = Ui::new();
    ui.set_theme(theme.aurora);
    let mut rows = EditorRows::default();

    let root = ui.root_panel(
        Style::new()
            .fill()
            .column()
            .background(theme.aurora.root_bg),
    );
    // The dock area fills the window above the status bar. It clips
    // (Overflow::Hidden) so the scroll panes inside stay window-sized instead of
    // ballooning the row to their tall content (see aurora's nested-row test).
    let dock_area = ui.panel(root, Style::new().row().grow(1.0).clip());

    let ctx = PaneCtx {
        scene,
        selected,
        primary,
        tree,
        filter,
        renaming,
        inspector,
        gizmo_mode,
        show_grid,
        show_axes,
        snap_enabled,
        icons,
        viewport_handle,
        explorer,
        thumbnails,
        explorer_cols,
        scrolls,
        logs,
        console_follow,
        open_script,
        script_text,
        script_modified,
        script_orphaned,
        diagnostics,
        tab_bars,
        theme,
    };
    build_dock_node(&mut ui, dock_area, dock, Sizing::Grow, &ctx, &mut rows);

    build_status_bar(&mut ui, root, theme, &mut rows);

    // A right-click context menu floats above everything on Aurora's popup
    // layer, so it is built last and dismissed by the app on a click outside.
    if let Some(menu) = menu {
        build_context_menu(&mut ui, menu, theme, &mut rows);
    }

    (ui, rows)
}

/// The read-only editor state a pane needs to render itself, bundled so the
/// dock recursion threads one reference instead of a dozen arguments.
struct PaneCtx<'a> {
    scene: &'a Scene,
    selected: &'a HashSet<NodeId>,
    primary: Option<NodeId>,
    tree: &'a TreeView,
    filter: &'a str,
    renaming: Option<NodeId>,
    inspector: &'a InspectorView,
    gizmo_mode: GizmoMode,
    show_grid: bool,
    show_axes: bool,
    snap_enabled: bool,
    icons: Option<&'a Icons>,
    viewport_handle: ImageHandle,
    explorer: &'a FileExplorer,
    thumbnails: &'a Thumbnails,
    /// The grid view's column count (from the contents pane's width last frame).
    explorer_cols: usize,
    scrolls: &'a HashMap<Pane, f32>,
    logs: &'a [LogLine],
    /// Whether the console pane should stick to the newest line (the user has
    /// not scrolled up).
    console_follow: bool,
    /// The script the Code pane is showing: its project-relative path and the
    /// buffer's current text. `None` when nothing is open.
    open_script: Option<&'a str>,
    script_text: &'a str,
    /// Whether the open script has unsaved edits.
    script_modified: bool,
    /// The buffer's file was deleted or moved away underneath it.
    script_orphaned: bool,
    /// Every diagnostic in the open script, for the Problems pane.
    diagnostics: &'a [comet::Diagnostic],
    /// One tab bar per dock group, in the order the dock is walked. aurora owns
    /// their interaction and animation state; the shell just hands them over.
    tab_bars: &'a [TabBar],
    theme: &'a EditorTheme,
}

/// How a dock node is sized within its parent split: it grows to fill, or takes
/// a fixed extent along the parent split's axis.
#[derive(Clone, Copy)]
enum Sizing {
    Grow,
    Fixed(SplitDir, f32),
}

/// Apply a [`Sizing`] to a node's root style (a fixed width for a Row parent, a
/// fixed height for a Column parent, or grow).
fn apply_sizing(style: Style, sizing: Sizing) -> Style {
    match sizing {
        Sizing::Grow => style.grow(1.0),
        Sizing::Fixed(SplitDir::Row, size) => style.width(size),
        Sizing::Fixed(SplitDir::Column, size) => style.height(size),
    }
}

/// Recursively build a dock node under `parent`, sized by `sizing`. A split
/// emits its two children with a draggable divider between; a leaf emits a tab
/// group. Returns the node's root widget.
fn build_dock_node(
    ui: &mut Ui,
    parent: WidgetId,
    node: &DockNode,
    sizing: Sizing,
    ctx: &PaneCtx,
    rows: &mut EditorRows,
) -> WidgetId {
    match node {
        DockNode::Leaf { panes, active } => {
            build_group(ui, parent, panes, *active, sizing, ctx, rows)
        }
        DockNode::Split {
            dir,
            fixed,
            size,
            first,
            second,
        } => {
            let sw = ctx.theme.aurora.splitter_width;
            let divider = || Style::new().background(ctx.theme.aurora.panel_border);
            let (row_style, orientation, splitter_style) = match dir {
                SplitDir::Row => (
                    Style::new().row(),
                    Orientation::Vertical,
                    divider().width(sw),
                ),
                SplitDir::Column => (
                    Style::new().column(),
                    Orientation::Horizontal,
                    divider().height(sw),
                ),
            };
            let container = ui.panel(parent, apply_sizing(row_style.clip(), sizing));
            let first_fixed = *fixed == Fixed::First;
            let (first_sizing, second_sizing) = if first_fixed {
                (Sizing::Fixed(*dir, *size), Sizing::Grow)
            } else {
                (Sizing::Grow, Sizing::Fixed(*dir, *size))
            };
            let first_id = build_dock_node(ui, container, first, first_sizing, ctx, rows);
            // sign: +1 when the fixed target sits before the divider (first
            // child), -1 when after (second) - matching aurora's splitter drag.
            let sign = if first_fixed { 1.0 } else { -1.0 };
            let splitter = ui.splitter(container, orientation, sign, splitter_style);
            let second_id = build_dock_node(ui, container, second, second_sizing, ctx, rows);
            let fixed_id = if first_fixed { first_id } else { second_id };
            ui.set_splitter_target(splitter, fixed_id);
            // Post-order: children's splits are recorded before this one, so the
            // list matches DockNode::set_fixed_sizes' post-order walk.
            rows.dock_splits.push((fixed_id, *dir));
            container
        }
    }
}

/// Build a tab group under `parent`: a tab bar (one draggable button per pane)
/// above the active pane's content. Returns the group's root widget.
fn build_group(
    ui: &mut Ui,
    parent: WidgetId,
    panes: &[Pane],
    active: usize,
    sizing: Sizing,
    ctx: &PaneCtx,
    rows: &mut EditorRows,
) -> WidgetId {
    let theme = ctx.theme;
    let group = ui.panel(
        parent,
        apply_sizing(
            Style::new()
                .column()
                .clip()
                .background(theme.aurora.panel_bg),
            sizing,
        ),
    );
    // The tab strip is aurora's: it draws the tabs, and owns dragging one along
    // the bar to reorder plus the slide that follows.
    let group_index = rows.dock_tab_bars.len();
    let labels: Vec<String> = panes.iter().map(|p| p.title().to_string()).collect();
    let bar_rows = ctx
        .tab_bars
        .get(group_index)
        .cloned()
        .unwrap_or_default()
        .build(ui, group, &labels, active, &theme.aurora);
    for (&pane, &tab) in panes.iter().zip(&bar_rows.tabs) {
        rows.dock_tabs.push((tab, pane));
    }
    rows.dock_tab_bars.push(bar_rows);
    // The active pane's content fills the rest of the group.
    let content = ui.panel(group, Style::new().column().grow(1.0).clip());
    let pane = panes[active];
    rows.dock_groups.push((content, pane));
    build_pane(ui, content, pane, ctx, rows);
    group
}

/// Build the content of a single pane into `parent`, restoring its saved scroll.
fn build_pane(ui: &mut Ui, parent: WidgetId, pane: Pane, ctx: &PaneCtx, rows: &mut EditorRows) {
    let scroll = match pane {
        Pane::SceneTree => Some(build_scene_tree(
            ui,
            parent,
            ctx.scene,
            ctx.selected,
            ctx.primary,
            ctx.tree,
            ctx.filter,
            ctx.renaming,
            ctx.icons,
            ctx.theme,
            rows,
        )),
        Pane::Inspector => Some(build_inspector(
            ui,
            parent,
            ctx.scene,
            ctx.primary,
            ctx.icons,
            ctx.inspector,
            ctx.explorer,
            ctx.thumbnails,
            ctx.theme,
            rows,
        )),
        Pane::Files => Some(build_file_explorer(ui, parent, ctx, rows)),
        Pane::Viewport => {
            build_viewport_pane(ui, parent, ctx, rows);
            None
        }
        Pane::Code => {
            build_code_pane(ui, parent, ctx, rows);
            None
        }
        Pane::Problems => {
            build_problems_pane(ui, parent, ctx, rows);
            None
        }
        Pane::Console => {
            // The console handles its own scroll: follow the newest line unless
            // the user has scrolled up (then restore where they were).
            let scroll = build_console_pane(ui, parent, ctx.logs, ctx.theme, rows);
            rows.pane_scrolls.push((Pane::Console, scroll));
            if ctx.console_follow {
                ui.set_scroll_offset(scroll, f32::MAX); // layout clamps to bottom
            } else if let Some(&offset) = ctx.scrolls.get(&Pane::Console) {
                ui.set_scroll_offset(scroll, offset);
            }
            None
        }
    };
    if let Some(scroll) = scroll {
        rows.pane_scrolls.push((pane, scroll));
        if let Some(&offset) = ctx.scrolls.get(&pane) {
            ui.set_scroll_offset(scroll, offset);
        }
    }
}

/// The Code pane: a monospace, gutter-lined editor for one script, above a
/// header saying which one.
///
/// The editor itself is one aurora text area with every code capability turned
/// on - the shell's whole job here is to place it and say what file it holds.
fn build_code_pane(ui: &mut Ui, parent: WidgetId, ctx: &PaneCtx, rows: &mut EditorRows) {
    let theme = ctx.theme;
    let panel = ui.panel(
        parent,
        Style::new()
            .column()
            .grow(1.0)
            .background(theme.aurora.panel_bg),
    );

    let header = ui.panel(
        panel,
        Style::new()
            .row()
            .gap(6.0)
            .align_center()
            .padding_x(8.0)
            .padding_y(4.0)
            .border_bottom(theme.aurora.border_width, theme.aurora.panel_border),
    );
    let caption = match ctx.open_script {
        Some(path) if ctx.script_modified => format!("{path} *"),
        Some(path) => path.to_string(),
        None if ctx.script_orphaned => {
            "The file this script was in is gone - Save As to keep it".to_string()
        }
        None => "No script open - double-click a .cmt file".to_string(),
    };
    ui.label(
        header,
        caption,
        Style::new()
            .grow(1.0)
            .ellipsis()
            .foreground(if ctx.open_script.is_some() {
                theme.aurora.heading
            } else {
                theme.aurora.subhead
            }),
    );
    if ctx.open_script.is_some() {
        rows.code_save = Some(
            ui.button(
                header,
                "Save",
                Style::new()
                    .padding_x(8.0)
                    .padding_y(2.0)
                    .corner_radius(theme.aurora.control_radius),
            ),
        );
    }

    // Without a script open there is nothing to edit - an empty editor would
    // invite typing into a buffer with nowhere to go. An orphaned buffer is the
    // exception: its text is still the user's, and hiding it would lose it.
    if ctx.open_script.is_none() && !ctx.script_orphaned {
        return;
    }
    let editor = ui.text_input(
        panel,
        ctx.script_text.to_string(),
        Style::new()
            .grow(1.0)
            .multiline()
            .monospace()
            .gutter()
            .auto_pairs()
            .no_wrap()
            .padding(6.0)
            .background(theme.aurora.panel_bg)
            .foreground(theme.aurora.heading),
    );
    rows.code_editor = Some(editor);
}

/// The console pane: recent log lines, newest at the bottom, colored by level.
/// Every problem in the open script, one clickable row each.
///
/// The Console's shape, because it is the same thing: a list of lines, coloured
/// by how bad they are, that you scan rather than read. Stepping with F8 shows
/// one at a time; this is the only view that says how many there are.
fn build_problems_pane(
    ui: &mut Ui,
    parent: WidgetId,
    ctx: &PaneCtx,
    rows: &mut EditorRows,
) -> WidgetId {
    let panel = ui.panel(
        parent,
        Style::new()
            .column()
            .grow(1.0)
            .padding(6.0)
            .gap(1.0)
            .scroll(),
    );
    if ctx.open_script.is_none() && !ctx.script_orphaned {
        ui.label(
            panel,
            "No script open.",
            Style::new().foreground(ctx.theme.aurora.subhead),
        );
        return panel;
    }
    if ctx.diagnostics.is_empty() {
        ui.label(
            panel,
            "No problems.",
            Style::new().foreground(ctx.theme.aurora.subhead),
        );
        return panel;
    }
    for diagnostic in ctx.diagnostics {
        let at = (diagnostic.span.start as usize).min(ctx.script_text.len());
        let line = ctx.script_text[..at].matches('\n').count() + 1;
        let color = match diagnostic.severity {
            comet::Severity::Error => ctx.theme.code_error,
            comet::Severity::Warning => ctx.theme.code_warning,
        };
        // A button rather than a label: clicking a row should take you there,
        // and a panel emits no Clicked.
        let row = ui.button(
            panel,
            "",
            Style::new()
                .row()
                .gap(6.0)
                .padding(2.0)
                .background(aurora::Color::TRANSPARENT),
        );
        ui.label(
            row,
            format!("{line}"),
            Style::new().width(40.0).foreground(color),
        );
        ui.label(
            row,
            diagnostic.message.clone(),
            Style::new()
                .grow(1.0)
                .foreground(ctx.theme.aurora.heading)
                .ellipsis(),
        );
        rows.problem_rows.push((row, diagnostic.span));
    }
    panel
}

fn build_console_pane(
    ui: &mut Ui,
    parent: WidgetId,
    logs: &[LogLine],
    theme: &EditorTheme,
    rows: &mut EditorRows,
) -> WidgetId {
    let panel = ui.panel(
        parent,
        Style::new()
            .column()
            .grow(1.0)
            .padding(6.0)
            .gap(1.0)
            .scroll(),
    );
    rows.console_panel = Some(panel);
    if logs.is_empty() {
        // An empty pane reads as "is it broken?"; name the state instead.
        ui.label(
            panel,
            "No output yet.",
            Style::new().foreground(theme.aurora.subhead),
        );
        return panel;
    }
    // The ring keeps more lines than are worth building a widget for each rebuild
    // (see console::LOG_CAP). Render only the newest CONSOLE_TAIL and, when older
    // lines exist, say how many rather than dropping them silently.
    let start = logs.len().saturating_sub(CONSOLE_TAIL);
    if start > 0 {
        let plural = if start == 1 { "" } else { "s" };
        ui.label(
            panel,
            format!("{start} earlier line{plural} not shown"),
            Style::new().foreground(theme.aurora.subhead),
        );
    }
    for line in &logs[start..] {
        let color = match line.level {
            tracing::Level::ERROR => theme.axis_x,
            tracing::Level::WARN => theme.console_warn,
            tracing::Level::INFO => theme.aurora.heading,
            _ => theme.aurora.subhead,
        };
        // Just the last path segment of the target keeps lines readable.
        let target = line.target.rsplit("::").next().unwrap_or(&line.target);
        let text = format!("{:>5} {}: {}", line.level, target, line.message);
        ui.label(panel, text, Style::new().foreground(color).ellipsis());
    }
    panel
}

/// The viewport pane: the gizmo-mode toolbar pinned above the scene render
/// target (which travels with the pane wherever it docks).
fn build_viewport_pane(ui: &mut Ui, parent: WidgetId, ctx: &PaneCtx, rows: &mut EditorRows) {
    let col = ui.panel(parent, Style::new().column().grow(1.0).clip());
    build_toolbar(
        ui,
        col,
        ctx.gizmo_mode,
        ctx.show_grid,
        ctx.show_axes,
        ctx.snap_enabled,
        ctx.icons,
        ctx.theme,
        rows,
    );
    // A drop target so a PNG dragged from the file explorer can land here.
    let viewport = ui.image(
        col,
        ctx.viewport_handle,
        Style::new().grow(1.0).drop_target().accepts(DRAG_IMAGE),
    );
    rows.viewport = Some(viewport);
}

/// The status bar along the bottom: live readouts the app refreshes in place
/// each frame (set_label), so no shell rebuild is ever needed for them.
fn build_status_bar(ui: &mut Ui, parent: WidgetId, theme: &EditorTheme, rows: &mut EditorRows) {
    let bar = ui.panel(
        parent,
        Style::new()
            .row()
            .gap(18.0)
            .padding(4.0)
            .align_center()
            .background(theme.aurora.panel_bg),
    );
    let readout = |ui: &mut Ui, text: &str| {
        ui.label(bar, text, Style::new().foreground(theme.aurora.subhead))
    };
    rows.status_cursor = Some(readout(ui, "x -, y -"));
    // The Code pane's two readouts: where the caret is, and what the compiler
    // says about the line it is on. The message is the whole point - a squiggle
    // with no text anywhere is a red line and no information.
    rows.status_caret = Some(readout(ui, ""));
    rows.status_diagnostic = Some(ui.label(
        bar,
        "",
        Style::new().ellipsis().grow(1.0).foreground(theme.axis_x),
    ));
    rows.status_zoom = Some(readout(ui, "zoom 100%"));
    rows.status_selected = Some(readout(ui, "nothing selected"));
    rows.status_fps = Some(readout(ui, "- fps"));
    // Brighter than the neutral readouts so unsaved state stands out.
    rows.status_modified = Some(ui.label(bar, "", Style::new().foreground(theme.aurora.heading)));
}

/// The toolbar directly above the viewport: the editor's actions on the left,
/// the gizmo-mode switches on the right (the active mode highlighted). Each
/// button shows an icon when `icons` is available (falling back to its text
/// caption, e.g. in headless tests). A natural home for more tool switches.
#[allow(clippy::too_many_arguments)]
fn build_toolbar(
    ui: &mut Ui,
    parent: WidgetId,
    mode: GizmoMode,
    show_grid: bool,
    show_axes: bool,
    snap_enabled: bool,
    icons: Option<&Icons>,
    theme: &EditorTheme,
    rows: &mut EditorRows,
) {
    let bar = ui.panel(
        parent,
        Style::new()
            .row()
            .gap(4.0)
            .padding(5.0)
            .align_center()
            .background(theme.aurora.panel_bg)
            .border_bottom(theme.aurora.border_width, theme.aurora.panel_border),
    );
    let icon = |id: Icon| icons.map(|i| i.get(id));

    // What the pointer is doing: select, then the three transforms. First
    // because it is what a hand reaches for most, and the active one is
    // highlighted so the current mode reads at a glance.
    for m in GizmoMode::ALL {
        let button = toolbar_button(ui, bar, m.label(), icon(mode_icon(m)), m == mode, theme);
        rows.mode_buttons.push((button, m));
    }

    toolbar_separator(ui, bar, theme);

    // What the viewport shows, and whether a drag snaps. Highlighted when on,
    // the same way the mode buttons are - these are states rather than actions.
    rows.grid = Some(toolbar_button(
        ui,
        bar,
        "Grid",
        icon(Icon::Grid),
        show_grid,
        theme,
    ));
    rows.axes = Some(toolbar_button(
        ui,
        bar,
        "Axes",
        icon(Icon::Axes),
        show_axes,
        theme,
    ));
    rows.snap = Some(toolbar_button(
        ui,
        bar,
        "Snap",
        icon(Icon::Snap),
        snap_enabled,
        theme,
    ));

    // Everything above is about the viewport; settings is about the editor, so
    // it sits alone at the far end rather than in the run.
    ui.panel(bar, Style::new().grow(1.0));
    rows.settings_button = Some(toolbar_button(
        ui,
        bar,
        "Settings",
        icon(Icon::Settings),
        false,
        theme,
    ));
}

/// A thin vertical rule between groups of toolbar buttons, so "what the pointer
/// does" and "what the viewport shows" read as two runs rather than one row of
/// seven.
fn toolbar_separator(ui: &mut Ui, bar: WidgetId, theme: &EditorTheme) {
    ui.panel(
        bar,
        Style::new()
            .size(1.0, TOOLBAR_ICON)
            .margin_x(3.0)
            .background(theme.aurora.panel_border),
    );
}

/// A toolbar button: an icon inside a fixed square when an icon is given, else
/// a text-captioned button (the headless-test / no-icon fallback). An `active`
/// button (a toggled view/snap, the current gizmo mode) takes the `mode_active`
/// background and tints its icon with `icon_active`; otherwise it sits on the
/// bar and its icon passes through white.
fn toolbar_button(
    ui: &mut Ui,
    bar: WidgetId,
    caption: &str,
    icon: Option<ImageHandle>,
    active: bool,
    theme: &EditorTheme,
) -> WidgetId {
    let background = if active {
        theme.mode_active
    } else {
        theme.aurora.panel_bg
    };
    match icon {
        Some(handle) => {
            let button = ui.button(
                bar,
                "",
                Style::new()
                    .padding(5.0)
                    .background(background)
                    .corner_radius(theme.aurora.control_radius),
            );
            let tint = if active {
                theme.aurora.icon_active
            } else {
                Color::WHITE
            };
            ui.image(
                button,
                handle,
                Style::new()
                    .size(TOOLBAR_ICON, TOOLBAR_ICON)
                    .foreground(tint),
            );
            button
        }
        None => ui.button(
            bar,
            caption,
            Style::new()
                .padding(6.0)
                .background(background)
                .foreground(theme.aurora.heading)
                .corner_radius(theme.aurora.control_radius),
        ),
    }
}

/// The icon for a gizmo mode.
fn mode_icon(mode: GizmoMode) -> Icon {
    match mode {
        GizmoMode::Select => Icon::Select,
        GizmoMode::Move => Icon::Move,
        GizmoMode::Rotate => Icon::Rotate,
        GizmoMode::Scale => Icon::Scale,
    }
}

/// A right-click context menu on Aurora's popup layer: a floating column of
/// action buttons anchored at the click.
fn build_context_menu(ui: &mut Ui, menu: &ContextMenu, theme: &EditorTheme, rows: &mut EditorRows) {
    let popup = ui.popup(
        menu.anchor,
        Style::new()
            .column()
            .gap(2.0)
            .padding(4.0)
            .background(theme.aurora.menu_bg)
            .corner_radius(theme.aurora.card_radius)
            .border(theme.aurora.border_width, theme.aurora.card_border)
            .clip(),
    );
    for (label, action) in &menu.items {
        let item = ui.button(
            popup,
            label.clone(),
            Style::new()
                .width(150.0)
                .padding(6.0)
                .background(theme.aurora.menu_bg)
                .foreground(theme.aurora.heading)
                .corner_radius(theme.aurora.control_radius),
        );
        rows.menu_items.push((item, action.clone()));
    }
    rows.menu_popup = Some(popup);
}

/// Build the floating copy of a tab being dragged out of its bar. It is placed
/// at the origin and moved to the pointer with a draw-time offset each frame, so
/// following the cursor never costs a rebuild.
pub fn build_carried_tab(ui: &mut Ui, title: &str, theme: &EditorTheme) -> WidgetId {
    let card = ui.popup(
        Vec2::ZERO,
        Style::new()
            .padding_x(12.0)
            .padding_y(6.0)
            .background(theme.aurora.panel_bg)
            .foreground(theme.aurora.heading)
            .corner_radius(theme.aurora.tab_radius)
            .border(theme.aurora.border_width, theme.aurora.tab_border)
            .hit_transparent(),
    );
    ui.label(
        card,
        title.to_string(),
        Style::new().foreground(theme.aurora.heading),
    );
    card
}

/// A modal card's width.
const MODAL_W: f32 = 420.0;

/// Build a modal dialog on the popup layer: a full-window scrim backdrop (which,
/// being opaque to hit-testing, blocks all input to the shell beneath it) with a
/// centered card holding a title bar, the body, and footer buttons. Call this
/// LAST (after the shell and any color picker) so it is the topmost popup.
pub fn build_modal(
    ui: &mut Ui,
    modal: &Modal,
    icons: Option<&Icons>,
    thumbnails: &Thumbnails,
    theme: &EditorTheme,
    rows: &mut EditorRows,
) {
    // The backdrop fills the window and dims it. Its column + cross-axis center
    // centers the card horizontally; grow spacers above and below center it
    // vertically (aurora has no justify-content builder).
    let backdrop = ui.popup(
        Vec2::ZERO,
        Style::new()
            .fill()
            .background(theme.aurora.scrim)
            .column()
            .align_center(),
    );
    ui.panel(backdrop, Style::new().grow(1.0));
    let card = ui.panel(
        backdrop,
        Style::new()
            .column()
            .gap(12.0)
            .width(MODAL_W)
            .padding(16.0)
            .background(theme.aurora.card_bg)
            .corner_radius(theme.aurora.card_radius)
            .border(theme.aurora.border_width, theme.aurora.card_border),
    );
    rows.modal_card = Some(card);

    // Title bar: the title, and an optional close button at the top-right.
    let bar = ui.panel(card, Style::new().row().align_center().gap(6.0));
    ui.label(
        bar,
        modal.title.clone(),
        Style::new()
            .grow(1.0)
            .ellipsis()
            .foreground(theme.aurora.heading),
    );
    if modal.closable {
        let close = ui.button(bar, "", Style::new().icon_button().padding(3.0));
        match icons {
            Some(icons) => {
                ui.image(close, icons.get(Icon::Close), Style::new().size(16.0, 16.0));
            }
            None => {
                ui.label(close, "x", Style::new().foreground(theme.aurora.heading));
            }
        }
        rows.modal_close = Some(close);
    }

    match &modal.body {
        ModalBody::Message(text) => {
            ui.label(
                card,
                text.clone(),
                Style::new()
                    .width(MODAL_W - 32.0)
                    .foreground(theme.aurora.subhead),
            );
        }
        ModalBody::Confirm(confirm) => {
            ui.label(
                card,
                confirm.message.clone(),
                Style::new()
                    .width(MODAL_W - 32.0)
                    .foreground(theme.aurora.subhead),
            );
        }
        ModalBody::Settings(draft) => build_settings_body(ui, card, draft, theme, rows),
        ModalBody::AssetChooser(chooser) => {
            build_asset_chooser_body(ui, card, chooser, thumbnails, icons, theme, rows);
        }
    }

    // Footer: action buttons pushed to the right; the last one is the accented
    // default (OK / Save).
    let footer = ui.panel(card, Style::new().row().gap(8.0).align_center());
    ui.panel(footer, Style::new().grow(1.0));
    let buttons: &[(&str, ModalAction)] = match &modal.body {
        ModalBody::Message(_) => &[("OK", ModalAction::Close)],
        // Save last, so the accented default is the answer that cannot lose
        // work; Cancel first, so the destructive one is not next to it.
        ModalBody::Confirm(_) => &[
            ("Cancel", ModalAction::Close),
            ("Discard", ModalAction::DiscardAndProceed),
            ("Save", ModalAction::SaveThenProceed),
        ],
        ModalBody::Settings(_) => &[
            ("Cancel", ModalAction::Close),
            ("Save", ModalAction::SaveSettings),
        ],
        // The chooser commits by clicking an image; the footer just cancels.
        ModalBody::AssetChooser(_) => &[("Cancel", ModalAction::Close)],
    };
    for (i, &(label, action)) in buttons.iter().enumerate() {
        let primary = i == buttons.len() - 1;
        let background = if primary {
            theme.mode_active
        } else {
            theme.aurora.menu_bg
        };
        let btn = ui.button(
            footer,
            label,
            Style::new()
                .padding_x(14.0)
                .padding_y(6.0)
                .background(background)
                .foreground(theme.aurora.heading)
                .corner_radius(theme.aurora.control_radius),
        );
        rows.modal_buttons.push((btn, action));
    }

    ui.panel(backdrop, Style::new().grow(1.0));
}

/// The settings form body: a checkbox per boolean toggle, then a labelled
/// numeric input per snap increment.
fn build_settings_body(
    ui: &mut Ui,
    parent: WidgetId,
    draft: &SettingsDraft,
    theme: &EditorTheme,
    rows: &mut EditorRows,
) {
    let col = ui.panel(parent, Style::new().column().gap(8.0));
    for (field, label) in [
        (SettingField::ShowGrid, "Show grid"),
        (SettingField::ShowAxes, "Show world axes"),
        (SettingField::TidyOnSave, "Tidy scripts on save"),
        (SettingField::SnapEnabled, "Enable snapping"),
    ] {
        let cb = ui.checkbox(
            col,
            draft.checked(field),
            label,
            Style::new().foreground(theme.aurora.heading),
        );
        rows.modal_checks.push((cb, field));
    }
    for (field, label) in [
        (SettingField::MoveStep, "Move step (units)"),
        (SettingField::RotateStep, "Rotate step (deg)"),
        (SettingField::ScaleStep, "Scale step"),
    ] {
        let row = ui.panel(col, Style::new().row().align_center().gap(8.0));
        ui.label(
            row,
            label,
            Style::new().width(140.0).foreground(theme.aurora.subhead),
        );
        let value = draft.number(field).unwrap_or(0.0);
        let input = ui.numeric_input(
            row,
            format!("{value}"),
            Style::new()
                .grow(1.0)
                .padding(3.0)
                .corner_radius(theme.aurora.control_radius)
                .border(theme.aurora.border_width, theme.aurora.card_border),
        );
        rows.modal_inputs.push((input, field));
    }
}

/// The asset chooser body: a scrollable grid of the project's images. Each cell
/// is a button that selects that image (its project-relative path is recorded in
/// `asset_choices`).
const CHOOSER_H: f32 = 320.0;
const CHOOSER_CELL: f32 = 92.0;
const CHOOSER_THUMB: f32 = 72.0;

fn build_asset_chooser_body(
    ui: &mut Ui,
    card: WidgetId,
    chooser: &AssetChooser,
    thumbnails: &Thumbnails,
    icons: Option<&Icons>,
    theme: &EditorTheme,
    rows: &mut EditorRows,
) {
    let images = &chooser.assets;
    if images.is_empty() {
        ui.label(
            card,
            chooser.kind.empty_message().to_string(),
            Style::new()
                .width(MODAL_W - 32.0)
                .foreground(theme.aurora.subhead),
        );
        return;
    }
    let fallback_icon = match chooser.kind {
        AssetKind::Image => Icon::FileImage,
        AssetKind::Script => Icon::FileScript,
    };
    let grid_w = MODAL_W - 32.0;
    let grid = ui.panel(
        card,
        Style::new()
            .column()
            .gap(FILE_GRID_GAP)
            .width(grid_w)
            .height(CHOOSER_H)
            .scroll(),
    );
    // Fit as many cells as the (fixed) modal width allows rather than hardcoding
    // a count, so the grid stays correct if the modal or cell size changes.
    let cols = columns_for(grid_w, CHOOSER_CELL, FILE_GRID_GAP, 0.0, 8);
    for chunk in images.chunks(cols) {
        let row = ui.panel(grid, Style::new().row().gap(FILE_GRID_GAP));
        for asset in chunk {
            let cell = ui.button(
                row,
                "",
                Style::new()
                    .column()
                    .align_center()
                    .gap(3.0)
                    .padding(4.0)
                    .width(CHOOSER_CELL)
                    .flat()
                    .corner_radius(theme.aurora.control_radius),
            );
            if let Some(handle) = thumbnails.get(&asset.abs) {
                ui.image(
                    cell,
                    handle,
                    Style::new().size(CHOOSER_THUMB, CHOOSER_THUMB),
                );
            } else if let Some(icons) = icons {
                ui.image(
                    cell,
                    icons.get(fallback_icon),
                    Style::new().size(CHOOSER_THUMB, CHOOSER_THUMB),
                );
            } else {
                ui.panel(cell, Style::new().size(CHOOSER_THUMB, CHOOSER_THUMB));
            }
            ui.label(
                cell,
                asset.name.clone(),
                Style::new()
                    .width(CHOOSER_CELL - 8.0)
                    .ellipsis()
                    .text_center()
                    .foreground(theme.aurora.heading),
            );
            rows.asset_choices.push((cell, asset.rel.clone()));
        }
    }
}

/// The scene-tree pane's content: one clickable row per node, indented by
/// depth, above a search box. Fills `parent` (a dock group's content area); the
/// group frames it and the tab shows its name.
#[allow(clippy::too_many_arguments)]
fn build_scene_tree(
    ui: &mut Ui,
    parent: WidgetId,
    scene: &Scene,
    selected: &HashSet<NodeId>,
    primary: Option<NodeId>,
    tree: &TreeView,
    filter: &str,
    renaming: Option<NodeId>,
    icons: Option<&Icons>,
    theme: &EditorTheme,
    rows: &mut EditorRows,
) -> WidgetId {
    let panel = ui.panel(
        parent,
        Style::new()
            .column()
            .grow(1.0)
            .padding(10.0)
            .gap(TREE_ROW_GAP)
            .scroll(),
    );
    // A search box: its live text (read back by the app each frame) filters the
    // tree to nodes whose name matches, keeping their ancestors for context.
    let search = ui.text_input(
        panel,
        filter,
        Style::new()
            .foreground(theme.aurora.heading)
            .padding(5.0)
            .corner_radius(theme.aurora.control_radius)
            .border(theme.aurora.border_width, theme.aurora.field_border)
            .placeholder("Search..."),
    );
    rows.tree_filter = Some(search);

    let query = filter.trim().to_lowercase();
    let ctx = TreeCtx {
        selected,
        primary,
        tree,
        query: &query,
        renaming,
        icons,
        theme,
    };
    // Under a search that matches nothing (not even the root's subtree), say so
    // rather than leaving a lone root row that looks like a mistake.
    if !query.is_empty() && !subtree_matches(scene, scene.root(), &query) {
        ui.label(
            panel,
            "No matching nodes.",
            Style::new().padding(4.0).foreground(theme.aurora.subhead),
        );
    } else {
        add_tree_row(ui, panel, scene, scene.root(), 0, &ctx, rows);
    }
    panel
}

/// Whether `node` or any of its descendants has `query` (already lowercased) in
/// its name. An empty query matches everything. Drives the search filter: a
/// non-matching node is still shown when a descendant matches, so the path to a
/// match stays visible.
pub fn subtree_matches(scene: &Scene, node: NodeId, query: &str) -> bool {
    if query.is_empty() || scene.node(node).name.to_lowercase().contains(query) {
        return true;
    }
    scene
        .children(node)
        .iter()
        .any(|&c| subtree_matches(scene, c, query))
}

/// The nodes the tree shows, top to bottom, honoring collapse and the search
/// filter (`filter` is matched case-insensitively). This is the exact order the
/// tree lays rows out in, so it is what shift-click range selection walks.
pub fn visible_nodes(scene: &Scene, tree: &TreeView, filter: &str) -> Vec<NodeId> {
    fn walk(scene: &Scene, node: NodeId, tree: &TreeView, query: &str, out: &mut Vec<NodeId>) {
        out.push(node);
        let filtering = !query.is_empty();
        let collapsed = !filtering && tree.collapsed.contains(&node);
        if collapsed {
            return;
        }
        for &child in scene.children(node) {
            if filtering && !subtree_matches(scene, child, query) {
                continue;
            }
            walk(scene, child, tree, query, out);
        }
    }
    let query = filter.trim().to_lowercase();
    let mut out = Vec::new();
    walk(scene, scene.root(), tree, &query, &mut out);
    out
}

/// The read-only tree state `add_tree_row` needs, bundled so the recursion
/// passes one reference instead of a long argument list.
struct TreeCtx<'a> {
    selected: &'a HashSet<NodeId>,
    primary: Option<NodeId>,
    tree: &'a TreeView,
    /// The lowercased search query (empty when not filtering).
    query: &'a str,
    /// The node being inline-renamed, if any (its row shows an edit field).
    renaming: Option<NodeId>,
    icons: Option<&'a Icons>,
    theme: &'a EditorTheme,
}

/// The width of a disclosure column / indent step, and the chevron glyph size.
const TREE_DISCLOSURE: f32 = 16.0;
const TREE_CHEVRON: f32 = 14.0;
/// The vertical gap between tree rows; the reparent line straddles it so an
/// "after"/"before" pair on the two sides of a boundary draw as one line.
const TREE_ROW_GAP: f32 = 1.0;
/// The visibility eye is drawn a touch larger than the chevron, in a slightly
/// wider column, so it reads clearly.
const TREE_EYE: f32 = 18.0;

fn add_tree_row(
    ui: &mut Ui,
    parent: WidgetId,
    scene: &Scene,
    node: NodeId,
    depth: u32,
    ctx: &TreeCtx,
    rows: &mut EditorRows,
) {
    let theme = ctx.theme;
    // Any selected node is highlighted; the primary (the one the inspector
    // shows) reads a touch brighter. An Into-drop target tints green over both.
    let background = if ctx.tree.drop_target == Some(DropSpot::Into(node)) {
        theme.row_drop
    } else if ctx.primary == Some(node) {
        theme.aurora.row_selected.lighten(0.12)
    } else if ctx.selected.contains(&node) {
        theme.aurora.row_selected
    } else {
        Color::TRANSPARENT
    };
    // The whole row is one flat, selectable button (a tree row, not a raised
    // button); the disclosure triangle sits on top of it as a child.
    let row = ui.button(
        parent,
        "",
        Style::new()
            .row()
            .gap(4.0)
            .align_center()
            .padding(3.0)
            .flat()
            .background(background),
    );

    // Indent by depth; then a disclosure triangle for a parent, or a spacer.
    if depth > 0 {
        ui.panel(row, Style::new().width(depth as f32 * TREE_DISCLOSURE));
    }
    let has_children = !scene.children(node).is_empty();
    // While a search is active the tree is force-expanded so matches show.
    let filtering = !ctx.query.is_empty();
    let collapsed = !filtering && ctx.tree.collapsed.contains(&node);
    if has_children {
        // A flat button holding the disclosure triangle; clicking it toggles
        // collapse. The chevron image is decoration (present when icons are).
        let toggle = ui.button(
            row,
            "",
            Style::new().flat().padding(1.0).width(TREE_DISCLOSURE),
        );
        if let Some(icons) = ctx.icons {
            let chevron = if collapsed {
                icons.get(Icon::ChevronRight)
            } else {
                icons.get(Icon::ChevronDown)
            };
            ui.image(
                toggle,
                chevron,
                Style::new().size(TREE_CHEVRON, TREE_CHEVRON),
            );
        }
        rows.tree_toggles.push((toggle, node));
    } else {
        ui.panel(row, Style::new().width(TREE_DISCLOSURE));
    }
    // While renaming this node, its name is an inline edit field; otherwise a
    // label (a hidden node's name is muted, so the tree reads its state).
    let visible = scene.node(node).visible;
    if ctx.renaming == Some(node) {
        let field = ui.text_input(
            row,
            scene.node(node).name.clone(),
            Style::new()
                .grow(1.0)
                .padding(1.0)
                .foreground(theme.aurora.heading)
                .corner_radius(theme.aurora.inset_radius)
                .border(theme.aurora.border_width, theme.aurora.focus),
        );
        rows.rename_field = Some((field, node));
    } else {
        ui.label(
            row,
            scene.node(node).name.clone(),
            Style::new().grow(1.0).ellipsis().foreground(if visible {
                theme.aurora.heading
            } else {
                theme.aurora.subhead
            }),
        );
    }
    // An eye toggle at the row's right edge: open when visible, struck through
    // when hidden. Clicking it toggles visibility (it does not select the row,
    // being its own interactive child).
    // An icon button: no background on hover, the eye recolors to the accent.
    let eye = ui.button(
        row,
        "",
        Style::new()
            .icon_button()
            .padding(2.0)
            .width(TREE_EYE + 6.0),
    );
    if let Some(icons) = ctx.icons {
        let glyph = if visible { Icon::Eye } else { Icon::EyeOff };
        ui.image(eye, icons.get(glyph), Style::new().size(TREE_EYE, TREE_EYE));
    }
    rows.eye_toggles.push((eye, node));
    rows.tree_rows.push((row, node));

    if has_children && !collapsed {
        for &child in scene.children(node) {
            // Under a search, skip whole branches with no match inside them.
            if filtering && !subtree_matches(scene, child, ctx.query) {
                continue;
            }
            add_tree_row(ui, parent, scene, child, depth + 1, ctx, rows);
        }
    }
}

/// Overlay a thin insertion line at a Before/After drop spot, on Aurora's popup
/// layer so it takes no layout space (an inline line would shift the rows and
/// make the detected drop spot flicker). Call after layout, then lay out once
/// more so the line popup is positioned. A no-op for an Into spot.
pub fn add_reparent_line(ui: &mut Ui, rows: &EditorRows, spot: DropSpot, color: Color) {
    let (node, after) = match spot {
        DropSpot::Before(n) => (n, false),
        DropSpot::After(n) => (n, true),
        DropSpot::Into(_) => return,
    };
    let Some(&(id, _)) = rows.tree_rows.iter().find(|(_, n)| *n == node) else {
        return;
    };
    let Some(rect) = ui.rect(id) else {
        return;
    };
    // Sit the line on the boundary between rows (half a row-gap outside the
    // edge), so "after this row" and "before the next" land on the *same* line
    // instead of two lines a gap apart.
    let y = if after {
        rect.max().y + TREE_ROW_GAP * 0.5
    } else {
        rect.pos.y - TREE_ROW_GAP * 0.5
    };
    // Hit-transparent so the cursor passes through the line to the row beneath
    // it - otherwise the 2px line steals the hover and the drop spot flickers.
    ui.popup(
        Vec2::new(rect.pos.x, y - 1.0),
        Style::new()
            .size(rect.size.x.max(1.0), 2.0)
            .background(color)
            .hit_transparent(),
    );
}

/// Add a hover tooltip for a file entry near the cursor, post-layout on the
/// popup layer. Hit-transparent so it does not steal the hover from the entry
/// beneath it. Shows the FULL name (wrapping, unlike the truncated grid label)
/// and the type / size / modified details.
pub fn add_file_tooltip(
    ui: &mut Ui,
    cursor: Vec2,
    window: Vec2,
    entry: &Entry,
    theme: &EditorTheme,
) {
    // Anchor below-right of the cursor, nudged back inside the window edges.
    let x = (cursor.x + 14.0).clamp(6.0, (window.x - FILE_TOOLTIP_W - 6.0).max(6.0));
    let y = (cursor.y + 20.0).clamp(6.0, (window.y - 96.0).max(6.0));
    let card = ui.popup(
        Vec2::new(x, y),
        Style::new()
            .column()
            .gap(3.0)
            .padding(8.0)
            .width(FILE_TOOLTIP_W)
            .background(theme.aurora.menu_bg)
            .corner_radius(theme.aurora.card_radius)
            .border(theme.aurora.border_width, theme.aurora.card_border)
            .hit_transparent(),
    );
    ui.label(
        card,
        entry.name.clone(),
        Style::new()
            .width(FILE_TOOLTIP_W - 16.0)
            .foreground(theme.aurora.heading),
    );
    tooltip_detail(ui, card, "Type", entry.type_label(), theme);
    tooltip_detail(ui, card, "Size", entry.size_label(), theme);
    let modified = entry.modified_label();
    if !modified.is_empty() {
        tooltip_detail(ui, card, "Modified", modified, theme);
    }
}

/// One "Label: value" line in the file tooltip. The label is left at its natural
/// width (no fixed width) so a longer one like "Modified:" never wraps - Aurora
/// has no wrap-off control, and a too-narrow box wraps the word.
fn tooltip_detail(ui: &mut Ui, parent: WidgetId, label: &str, value: String, theme: &EditorTheme) {
    let row = ui.panel(parent, Style::new().row().gap(6.0));
    ui.label(
        row,
        format!("{label}:"),
        Style::new().foreground(theme.aurora.subhead),
    );
    ui.label(
        row,
        value,
        Style::new().grow(1.0).foreground(theme.aurora.heading),
    );
}

/// The docked inspector panel: the selected node's transform and reflected
/// component fields as editable rows, grouped into collapsible section cards.
// A handful of small view inputs; named arguments read clearer than a struct.
#[allow(clippy::too_many_arguments)]
fn build_inspector(
    ui: &mut Ui,
    parent: WidgetId,
    scene: &Scene,
    selected: Option<NodeId>,
    icons: Option<&Icons>,
    inspector: &InspectorView,
    explorer: &FileExplorer,
    thumbnails: &Thumbnails,
    theme: &EditorTheme,
    rows: &mut EditorRows,
) -> WidgetId {
    let panel = ui.panel(
        parent,
        Style::new()
            .column()
            .grow(1.0)
            .padding(10.0)
            .gap(8.0)
            .scroll(),
    );

    let Some(node) = selected else {
        ui.label(
            panel,
            "Nothing selected",
            Style::new().foreground(theme.aurora.subhead),
        );
        return panel;
    };
    ui.label(
        panel,
        scene.node(node).name.clone(),
        Style::new().foreground(theme.aurora.heading).ellipsis(),
    );

    // The node's own transform first: it is what viewport drags edit, so its
    // rows double as a live readout while dragging.
    if let Some(card) = add_inspector_section(
        ui,
        panel,
        "Transform",
        node,
        InspectorSection::Transform,
        inspector,
        icons,
        theme,
        rows,
    ) {
        let t = scene.node(node).transform;
        add_vec2_field(
            ui,
            card,
            "position",
            t.translation,
            FieldRef::Position(node),
            theme,
            rows,
        );
        // Rotation is a single scalar, edited in degrees (stored radians); the
        // "deg" unit hint makes that explicit next to the field.
        let input = add_value_row(
            ui,
            card,
            "rotation",
            &Value::F32(t.rotation.to_degrees()),
            true,
            FieldShape {
                unit: Some("deg"),
                ..FieldShape::default()
            },
            theme,
        );
        rows.transform_rows.push((input, node, "rotation"));
        add_vec2_field(
            ui,
            card,
            "scale",
            t.scale,
            FieldRef::Scale(node),
            theme,
            rows,
        );
    }

    for (i, component) in scene.node(node).components.iter().enumerate() {
        let reflect = component.as_reflect();
        // Which kind this component's asset fields hold - asked of the
        // component, so a Script's source neither shows a picture nor offers
        // itself to one being dragged.
        let asset_kind = match reflect.type_name() {
            "Script" => AssetKind::Script,
            _ => AssetKind::Image,
        };
        let Some(card) = add_inspector_section(
            ui,
            panel,
            reflect.type_name(),
            node,
            InspectorSection::Component(i),
            inspector,
            icons,
            theme,
            rows,
        ) else {
            continue;
        };
        // A Script's own fields (its path) and the variables its source
        // exports are two different things, so they read as two runs rather
        // than one list where `source` and `speed` look alike.
        let exported: Vec<String> = match component {
            helios::Component::Script(script) => script
                .exports
                .iter()
                .map(|(name, _)| name.clone())
                .collect(),
            _ => Vec::new(),
        };
        let mut opened_group = false;
        for name in reflect.field_names() {
            let Some(value) = reflect.get(&name) else {
                continue;
            };
            if !opened_group && exported.contains(&name) {
                add_field_group(ui, card, "Exported", theme);
                opened_group = true;
            }
            let field = name.as_str();
            // A Vec2 field gets the two-axis treatment, a Color field a swatch
            // that opens the picker; everything else a plain (numeric where it
            // makes sense) row.
            match value {
                Value::Vec2(v) => {
                    let r = FieldRef::Component {
                        node,
                        index: i,
                        field: field.to_string(),
                    };
                    add_vec2_field(ui, card, field, v, r, theme, rows);
                }
                Value::Color(c) => {
                    let row = ui.panel(card, Style::new().row().gap(6.0).align_center());
                    ui.label(
                        row,
                        field,
                        Style::new().width(70.0).foreground(theme.aurora.heading),
                    );
                    let swatch = ui.button(
                        row,
                        "",
                        Style::new()
                            .width(48.0)
                            .padding(6.0)
                            .background(Color::rgba(c[0], c[1], c[2], c[3]))
                            .corner_radius(theme.aurora.control_radius),
                    );
                    rows.color_swatches.push((
                        swatch,
                        ColorTarget {
                            node,
                            index: i,
                            field: field.to_string(),
                        },
                    ));
                }
                Value::Asset(path) => {
                    add_asset_field(
                        ui, card, field, &path, asset_kind, node, i, icons, explorer, thumbnails,
                        theme, rows,
                    );
                }
                _ => {
                    // What the script asked for, if it is a Script component
                    // and it asked for anything.
                    let hints: &[comet::Hint] = match component {
                        helios::Component::Script(script) => script.hints_for(field),
                        _ => &[],
                    };
                    let readonly = hints.contains(&comet::Hint::Readonly);
                    let range = hints.iter().find_map(|h| match h {
                        comet::Hint::Range(low, high) => Some((low, high)),
                        _ => None,
                    });
                    // A range turns the number field into a slider, which is
                    // the whole reason a tuning pass wants it.
                    if let (Some((low, high)), Value::F32(current)) = (range, &value) {
                        let row = ui.panel(card, Style::new().row().gap(6.0).align_center());
                        ui.label(
                            row,
                            field,
                            Style::new().width(70.0).foreground(theme.aurora.heading),
                        );
                        let slider = ui.slider(
                            row,
                            *current,
                            *low,
                            *high,
                            Style::new().grow(1.0).enabled(!readonly),
                        );
                        rows.field_sliders
                            .push((slider, node, i, field.to_string()));
                        continue;
                    }
                    let numeric = matches!(value, Value::F32(_) | Value::Int(_));
                    let multiline = hints.contains(&comet::Hint::Multiline);
                    let shape = FieldShape {
                        multiline,
                        readonly,
                        unit: None,
                    };
                    let input = add_value_row(ui, card, field, &value, numeric, shape, theme);
                    rows.field_rows.push((input, node, i, field.to_string()));
                }
            }
        }
    }
    panel
}

/// The icon standing in for an asset field with no thumbnail. Read off the path
/// where there is one, so a `.cmt` never shows up as a picture; `kind` covers
/// the empty field, which has no path to read.
fn asset_field_icon(path: &str, kind: AssetKind) -> Icon {
    match classify(Path::new(path)) {
        FileKind::Image => Icon::FileImage,
        FileKind::Script => Icon::FileScript,
        FileKind::Scene => Icon::FileScene,
        FileKind::Other => match kind {
            AssetKind::Image => Icon::FileImage,
            AssetKind::Script => Icon::FileScript,
        },
    }
}

/// A one-field "go to line" box over the code editor.
/// The go-to-symbol palette's query field, over the code editor. The list of
/// matches is an ordinary `ListPopup` built underneath it.
pub fn add_symbol_query(
    ui: &mut Ui,
    anchor: Vec2,
    value: &str,
    theme: &EditorTheme,
) -> Option<WidgetId> {
    let card = ui.popup(
        anchor,
        Style::new()
            .row()
            .gap(6.0)
            .padding(6.0)
            .align_center()
            .background(theme.aurora.menu_bg)
            .corner_radius(theme.aurora.card_radius)
            .border(theme.aurora.border_width, theme.aurora.card_border),
    );
    ui.label(card, "Go to", Style::new().foreground(theme.aurora.subhead));
    Some(
        ui.text_input(
            card,
            value.to_string(),
            Style::new()
                .width(220.0)
                .padding(4.0)
                .corner_radius(theme.aurora.control_radius)
                .placeholder("function or state name"),
        ),
    )
}

/// The signature hint under the caret: the call being typed, with the argument
/// you are on picked out.
///
/// Emphasis by colour rather than by weight - there is one font - and the rest
/// of the signature stays legible rather than being dimmed to nothing, because
/// the parameters you have already passed are exactly what you are checking
/// against.
pub fn add_signature_hint(
    ui: &mut Ui,
    anchor: Vec2,
    help: &comet::service::SignatureHelp,
    theme: &EditorTheme,
) {
    let card = ui.popup(
        anchor,
        Style::new()
            .row()
            .gap(0.0)
            .padding(6.0)
            .align_center()
            .background(theme.aurora.menu_bg)
            .corner_radius(theme.aurora.card_radius)
            .border(theme.aurora.border_width, theme.aurora.card_border),
    );
    // The head is everything up to the parameters - `func min(`.
    let head = help
        .signature
        .find('(')
        .map_or(help.signature.as_str(), |at| &help.signature[..=at]);
    ui.label(
        card,
        head.to_string(),
        Style::new().foreground(theme.code_function),
    );
    for (i, param) in help.params.iter().enumerate() {
        if i > 0 {
            ui.label(card, ", ", Style::new().foreground(theme.aurora.subhead));
        }
        let color = if i == help.active {
            theme.code_type
        } else {
            theme.aurora.subhead
        };
        ui.label(card, param.clone(), Style::new().foreground(color));
    }
    // The tail is the closing paren and any return type.
    let tail = help
        .signature
        .rfind(')')
        .map_or(")", |at| &help.signature[at..]);
    ui.label(
        card,
        tail.to_string(),
        Style::new().foreground(theme.code_function),
    );
    // Past the end: more arguments than the function takes. Saying so beside
    // the signature is more use than clamping the highlight and showing nothing.
    if help.active >= help.params.len() {
        ui.label(
            card,
            format!("  (takes {})", help.params.len()),
            Style::new().foreground(theme.code_error),
        );
    }
}

/// The rename field over the code editor, with the reason the last attempt was
/// refused beside it when there was one.
pub fn add_symbol_rename(
    ui: &mut Ui,
    anchor: Vec2,
    value: &str,
    error: Option<&str>,
    theme: &EditorTheme,
) -> Option<WidgetId> {
    let card = ui.popup(
        anchor,
        Style::new()
            .row()
            .gap(6.0)
            .padding(6.0)
            .align_center()
            .background(theme.aurora.menu_bg)
            .corner_radius(theme.aurora.card_radius)
            .border(theme.aurora.border_width, theme.aurora.card_border),
    );
    ui.label(
        card,
        "Rename to",
        Style::new().foreground(theme.aurora.subhead),
    );
    let field = ui.text_input(
        card,
        value.to_string(),
        Style::new()
            .width(180.0)
            .padding(4.0)
            .corner_radius(theme.aurora.control_radius),
    );
    if let Some(error) = error {
        ui.label(card, error, Style::new().foreground(theme.code_error));
    }
    Some(field)
}

pub fn add_go_to_line(
    ui: &mut Ui,
    anchor: Vec2,
    value: &str,
    theme: &EditorTheme,
) -> Option<WidgetId> {
    let card = ui.popup(
        anchor,
        Style::new()
            .row()
            .gap(6.0)
            .padding(6.0)
            .align_center()
            .background(theme.aurora.menu_bg)
            .corner_radius(theme.aurora.card_radius)
            .border(theme.aurora.border_width, theme.aurora.card_border),
    );
    ui.label(
        card,
        "Go to line",
        Style::new().foreground(theme.aurora.subhead),
    );
    Some(
        ui.text_input(
            card,
            value.to_string(),
            Style::new()
                .width(70.0)
                .padding(4.0)
                .mask(aurora::Mask::Integer)
                .corner_radius(theme.aurora.control_radius),
        ),
    )
}

/// Overlay a diagnostic's message near the pointer, the same way the file
/// details tooltip works: built after layout, positioned by hand, nudged back on
/// screen at the edges.
pub fn add_diagnostic_tooltip(
    ui: &mut Ui,
    cursor: Vec2,
    window: Vec2,
    message: &str,
    theme: &EditorTheme,
) {
    let card = ui.popup(
        cursor + Vec2::new(14.0, 20.0),
        Style::new()
            .column()
            .padding(8.0)
            .width((window.x * 0.45).clamp(220.0, 520.0))
            .background(theme.aurora.menu_bg)
            .corner_radius(theme.aurora.card_radius)
            .border(theme.aurora.border_width, theme.aurora.card_border),
    );
    ui.label(card, message, Style::new().foreground(theme.aurora.heading));
}

/// The thumbnail/icon size in an inspector asset field.
const ASSET_THUMB: f32 = 30.0;

/// An inspector asset (image) field: a preview of the current asset (its
/// thumbnail, or a file icon), its project-relative path (editable, committed
/// like any field), and Browse / Clear buttons. The whole row is a drop target -
/// dragging an explorer image onto it sets the field.
#[allow(clippy::too_many_arguments)]
fn add_asset_field(
    ui: &mut Ui,
    card: WidgetId,
    field: &str,
    path: &str,
    kind: AssetKind,
    node: NodeId,
    component: usize,
    icons: Option<&Icons>,
    explorer: &FileExplorer,
    thumbnails: &Thumbnails,
    theme: &EditorTheme,
    rows: &mut EditorRows,
) {
    let row = ui.panel(
        card,
        Style::new()
            .row()
            .gap(6.0)
            .align_center()
            .drop_target()
            .accepts(match kind {
                AssetKind::Image => DRAG_IMAGE,
                AssetKind::Script => DRAG_SCRIPT,
            }),
    );
    rows.asset_fields
        .push((row, node, component, field.to_string()));

    // The field name, left, so the asset row reads label <-> value like the rest.
    ui.label(
        row,
        field,
        Style::new().width(70.0).foreground(theme.aurora.heading),
    );

    // A preview: the asset's decoded thumbnail if we have it, else a file icon.
    match thumbnails.get(&explorer.resolve_relative(path)) {
        Some(handle) => {
            ui.image(row, handle, Style::new().size(ASSET_THUMB, ASSET_THUMB));
        }
        None => match icons {
            Some(icons) => {
                ui.image(
                    row,
                    icons.get(asset_field_icon(path, kind)),
                    Style::new().size(ASSET_THUMB, ASSET_THUMB),
                );
            }
            None => {
                ui.panel(row, Style::new().size(ASSET_THUMB, ASSET_THUMB));
            }
        },
    }

    // The editable path (commits through the same field_rows path as any field).
    let input = ui.text_input(
        row,
        path.to_string(),
        Style::new()
            .grow(1.0)
            .padding(4.0)
            .foreground(theme.aurora.heading)
            .corner_radius(theme.aurora.control_radius),
    );
    rows.field_rows
        .push((input, node, component, field.to_string()));

    // Browse (opens the image chooser) and Clear (empties the field).
    let browse = asset_icon_button(ui, row, icons, Icon::Load, "...", theme);
    rows.asset_browse
        .push((browse, node, component, field.to_string()));
    let clear = asset_icon_button(ui, row, icons, Icon::Close, "x", theme);
    rows.asset_clear
        .push((clear, node, component, field.to_string()));
}

/// A small icon button for an asset field (falls back to a text caption when
/// there is no icon set, as in headless tests).
fn asset_icon_button(
    ui: &mut Ui,
    parent: WidgetId,
    icons: Option<&Icons>,
    icon: Icon,
    caption: &str,
    theme: &EditorTheme,
) -> WidgetId {
    match icons {
        Some(icons) => {
            let button = ui.button(parent, "", Style::new().icon_button().padding(4.0));
            ui.image(button, icons.get(icon), Style::new().size(16.0, 16.0));
            button
        }
        None => ui.button(
            parent,
            caption,
            Style::new()
                .padding(4.0)
                .background(theme.aurora.menu_bg)
                .foreground(theme.aurora.heading)
                .corner_radius(theme.aurora.control_radius),
        ),
    }
}

/// Add a collapsible inspector section as a "card" (its own lighter background,
/// separating it from its neighbors) with a clickable header - a disclosure
/// chevron plus the title. Records the header in `section_toggles`. Returns the
/// card to add the section's fields into when expanded, or `None` when collapsed
/// (so the caller skips building them).
#[allow(clippy::too_many_arguments)]
fn add_inspector_section(
    ui: &mut Ui,
    panel: WidgetId,
    title: &str,
    node: NodeId,
    section: InspectorSection,
    inspector: &InspectorView,
    icons: Option<&Icons>,
    theme: &EditorTheme,
    rows: &mut EditorRows,
) -> Option<WidgetId> {
    let collapsed = inspector.collapsed.contains(&(node, section));
    let card = ui.panel(
        panel,
        Style::new()
            .column()
            .background(theme.aurora.card_bg)
            .corner_radius(theme.aurora.component_radius)
            .border(theme.aurora.border_width, theme.aurora.card_border),
    );
    // A distinct darker header bar spanning the card, holding the section title
    // (and a disclosure chevron). Clicking anywhere on the bar toggles the
    // section - it is the button, so its whole width is the hit area. The header
    // carries the card's outline over its own top and sides (so the single
    // rounded border reads as continuous around the whole component); expanded,
    // it has no bottom edge, so there is no divider between header and body -
    // collapsed, it rounds and closes all four corners as the whole card.
    let header_style = Style::new()
        .row()
        .gap(6.0)
        .align_center()
        .padding_x(8.0)
        .padding_y(6.0)
        // The remove button sits inside this bar, and it does something else -
        // aiming at it should not leave the bar looking like it is about to be
        // clicked.
        .hover_self_only()
        .background(theme.aurora.header_bg)
        .border(theme.aurora.border_width, theme.aurora.card_border);
    let header_style = if collapsed {
        header_style.corner_radius(theme.aurora.component_radius)
    } else {
        header_style
            .round_top(theme.aurora.component_radius)
            .border_sides([true, true, false, true])
    };
    let header = ui.button(card, "", header_style);
    if let Some(icons) = icons {
        let chevron = if collapsed {
            Icon::ChevronRight
        } else {
            Icon::ChevronDown
        };
        ui.image(
            header,
            icons.get(chevron),
            Style::new().size(TREE_CHEVRON, TREE_CHEVRON),
        );
    }
    ui.label(
        header,
        title,
        Style::new().grow(1.0).foreground(theme.aurora.heading),
    );
    rows.section_toggles.push((header, section));
    // A component can be taken off again. Only a component - the node's
    // transform is not one, and there is nothing to remove.
    if let InspectorSection::Component(index) = section {
        // "x" when there is no icon set, the same fallback the toolbar uses, so
        // a headless layout still has the button to find.
        let caption = if icons.is_some() { "" } else { "x" };
        let remove = ui.button(
            header,
            caption,
            Style::new()
                .size(TREE_CHEVRON + 6.0, TREE_CHEVRON + 6.0)
                .align_center()
                .padding(3.0)
                .corner_radius(theme.aurora.control_radius),
        );
        if let Some(icons) = icons {
            ui.image(
                remove,
                icons.get(Icon::Close),
                Style::new().size(TREE_CHEVRON, TREE_CHEVRON),
            );
        }
        rows.component_removes.push((remove, node, index));
    }
    // The fields live in a transparent, padded body below the header (the card
    // has no padding, so the header bar reaches the edges; the body shows the
    // card fill through it, so the card's rounded border wraps it with no seam).
    (!collapsed).then(|| ui.panel(card, Style::new().column().gap(6.0).padding(8.0)))
}

/// A small heading over a run of rows inside a component, with space above it so
/// the run reads as its own group. Not collapsible: it is a label, not a
/// control, and a component with two rows should not grow two disclosure
/// triangles.
fn add_field_group(ui: &mut Ui, card: WidgetId, title: &str, theme: &EditorTheme) {
    let row = ui.panel(card, Style::new().row().align_center().margin_top(6.0));
    ui.label(
        row,
        title,
        Style::new().grow(1.0).foreground(theme.aurora.subhead),
    );
}

/// One labeled, editable inspector row; returns the text input's id. `numeric`
/// restricts typing to numbers.
/// How a script asked for a field to be shown, beyond its type.
#[derive(Debug, Clone, Copy, Default)]
pub struct FieldShape<'a> {
    pub multiline: bool,
    pub readonly: bool,
    /// A unit shown after the field, muted, so it reads as an annotation rather
    /// than part of the value.
    pub unit: Option<&'a str>,
}

fn add_value_row(
    ui: &mut Ui,
    panel: WidgetId,
    label: &str,
    value: &Value,
    numeric: bool,
    shape: FieldShape<'_>,
    theme: &EditorTheme,
) -> WidgetId {
    let row = ui.panel(panel, Style::new().row().gap(6.0).align_center());
    ui.label(
        row,
        label,
        Style::new().width(70.0).foreground(theme.aurora.heading),
    );
    let style = Style::new()
        .grow(1.0)
        .padding(4.0)
        .foreground(theme.aurora.heading)
        .corner_radius(theme.aurora.control_radius)
        .multiline_if(shape.multiline)
        .enabled(!shape.readonly);
    let input = if numeric {
        ui.numeric_input(row, value_to_text(value), style)
    } else {
        ui.text_input(row, value_to_text(value), style)
    };
    // An optional unit hint after the field (e.g. rotation in degrees), muted so
    // it reads as an annotation rather than part of the value.
    if let Some(unit) = shape.unit {
        ui.label(row, unit, Style::new().foreground(theme.aurora.subhead));
    }
    input
}

/// A Vec2 field as two rows: a colored, draggable axis label (X red, Y green)
/// beside a numeric input, matching the engine's axis palette. Dragging a
/// label scrubs that component; typing a value commits it.
fn add_vec2_field(
    ui: &mut Ui,
    panel: WidgetId,
    name: &str,
    v: Vec2,
    field: FieldRef,
    theme: &EditorTheme,
    rows: &mut EditorRows,
) {
    // One row: the field name, then a colored X/Y scrub label + numeric input
    // per axis - the same label <-> input shape as the scalar rows, just with
    // two value cells sharing the space.
    let row = ui.panel(panel, Style::new().row().gap(6.0).align_center());
    ui.label(
        row,
        name,
        Style::new().width(70.0).foreground(theme.aurora.heading),
    );
    for (axis, caption, color) in [(Axis::X, "X", theme.axis_x), (Axis::Y, "Y", theme.axis_y)] {
        let label = ui.button(
            row,
            caption,
            Style::new()
                .width(18.0)
                .padding_y(4.0)
                .background(color)
                .foreground(Color::WHITE)
                .text_center()
                .corner_radius(theme.aurora.control_radius),
        );
        rows.scrub_labels.push((label, field.clone(), axis));
        let input = ui.numeric_input(
            row,
            value_to_text(&Value::F32(axis_of(v, axis))),
            Style::new()
                .grow(1.0)
                .padding(4.0)
                .foreground(theme.aurora.heading)
                .corner_radius(theme.aurora.control_radius),
        );
        rows.vec_inputs.push((input, field.clone(), axis));
    }
}

/// The file-explorer pane's content: the project's PNG and scene files, filling
/// `parent` (a dock group's content area).
/// The folder icon in a tree row.
const FILE_TREE_ICON: f32 = 17.0;
/// The icon/thumbnail size in a list-view row.
const FILE_LIST_ICON: f32 = 22.0;
/// A grid cell's width, its thumbnail size, and the gap between cells.
const FILE_GRID_CELL: f32 = 84.0;
const FILE_GRID_THUMB: f32 = 64.0;
const FILE_GRID_GAP: f32 = 6.0;
/// List-row detail column widths (type / size / modified).
const FILE_COL_TYPE: f32 = 44.0;
const FILE_COL_SIZE: f32 = 62.0;
const FILE_COL_MODIFIED: f32 = 100.0;
/// The hover tooltip's width.
const FILE_TOOLTIP_W: f32 = 240.0;

/// The number of `cell`-wide columns (with `gap` between them and `pad` on each
/// side) that fit in a container `width` px wide - at least 1, at most `max`.
/// N cells span N*cell + (N-1)*gap inside the padded area.
fn columns_for(width: f32, cell: f32, gap: f32, pad: f32, max: usize) -> usize {
    let inner = (width - pad * 2.0).max(0.0);
    let cols = ((inner + gap) / (cell + gap)).floor();
    (cols as usize).clamp(1, max)
}

/// The number of grid columns that fit in a contents pane `width` px wide (at
/// least one). `main` reads the pane's laid-out width back each frame and feeds
/// it here so the preview grid reflows with the pane.
pub fn grid_cols(width: f32) -> usize {
    // The contents panel pads by 6 on each side.
    columns_for(width, FILE_GRID_CELL, FILE_GRID_GAP, 6.0, 8)
}

/// The file-explorer pane: a bar (breadcrumbs + view toggles + refresh) over a
/// folder tree (left) and the selected folder's contents (right, as a list or a
/// thumbnail grid). Returns the contents scroll panel, so the pane machinery
/// persists its scroll like every other pane.
fn build_file_explorer(
    ui: &mut Ui,
    parent: WidgetId,
    ctx: &PaneCtx,
    rows: &mut EditorRows,
) -> WidgetId {
    let theme = ctx.theme;
    let ex = ctx.explorer;
    let pane = ui.panel(parent, Style::new().column().grow(1.0));
    rows.file_pane = Some(pane);

    // The bar: breadcrumbs on the left, view toggles + refresh on the right.
    let bar = ui.panel(
        pane,
        Style::new()
            .row()
            .align_center()
            .gap(4.0)
            .padding(5.0)
            .background(theme.aurora.panel_bg)
            .border_bottom(theme.aurora.border_width, theme.aurora.panel_border),
    );
    let crumbs = ui.panel(
        bar,
        Style::new().row().align_center().gap(1.0).grow(1.0).clip(),
    );
    let trail = ex.breadcrumbs();
    let last = trail.len().saturating_sub(1);
    for (i, (name, path)) in trail.into_iter().enumerate() {
        let fg = if i == last {
            theme.aurora.heading
        } else {
            theme.aurora.subhead
        };
        let crumb = ui.button(
            crumbs,
            name,
            Style::new()
                .flat()
                .padding_x(3.0)
                .padding_y(1.0)
                .foreground(fg),
        );
        rows.file_crumbs.push((crumb, path));
        if i != last {
            ui.label(
                crumbs,
                "/".to_string(),
                Style::new().foreground(theme.aurora.subhead),
            );
        }
    }
    let icon = |id: Icon| ctx.icons.map(|i| i.get(id));
    rows.file_view_list = Some(toolbar_button(
        ui,
        bar,
        "List",
        icon(Icon::ViewList),
        ex.view() == FileView::List,
        theme,
    ));
    rows.file_view_grid = Some(toolbar_button(
        ui,
        bar,
        "Grid",
        icon(Icon::ViewGrid),
        ex.view() == FileView::Grid,
        theme,
    ));
    rows.file_refresh = Some(toolbar_button(
        ui,
        bar,
        "Refresh",
        icon(Icon::Refresh),
        false,
        theme,
    ));

    // The body: folder tree | divider | contents. It clips so the two scroll
    // panes inside stay pane-sized instead of ballooning to their content.
    let body = ui.panel(pane, Style::new().row().grow(1.0).clip());
    let tree = ui.panel(
        body,
        Style::new()
            .column()
            .width(ex.tree_width())
            .gap(TREE_ROW_GAP)
            .padding(4.0)
            .scroll()
            .background(theme.aurora.root_bg),
    );
    for row in ex.tree_rows() {
        build_folder_row(ui, tree, &row, ctx, rows);
    }
    // Restore the tree's scroll (kept in the model, not pane_scrolls) so it does
    // not snap to the top on rebuilds; the app captures it back each frame.
    ui.set_scroll_offset(tree, ex.tree_scroll());
    rows.file_tree_scroll = Some(tree);
    // A draggable splitter (in place of a static divider) resizes the tree; the
    // tree sits before it, so dragging right widens it. The app captures the new
    // width back into the explorer each frame so it survives rebuilds.
    let splitter = ui.splitter(
        body,
        Orientation::Vertical,
        1.0,
        Style::new()
            .width(theme.aurora.splitter_width)
            .background(theme.aurora.panel_border),
    );
    ui.set_splitter_target(splitter, tree);
    let contents = ui.panel(
        body,
        Style::new()
            .column()
            .grow(1.0)
            .padding(6.0)
            .gap(3.0)
            .scroll(),
    );
    build_contents(ui, contents, ctx, rows);
    rows.file_contents = Some(contents);
    contents
}

/// One folder row in the tree: indent, a disclosure arrow (if it has
/// sub-folders), a folder icon, and the name. The whole row selects the folder.
fn build_folder_row(
    ui: &mut Ui,
    parent: WidgetId,
    row: &TreeRow,
    ctx: &PaneCtx,
    rows: &mut EditorRows,
) {
    let theme = ctx.theme;
    let background = if row.selected {
        theme.aurora.row_selected
    } else {
        Color::TRANSPARENT
    };
    let r = ui.button(
        parent,
        "",
        Style::new()
            .row()
            .gap(4.0)
            .align_center()
            .padding(3.0)
            .flat()
            .background(background),
    );
    if row.depth > 0 {
        ui.panel(r, Style::new().width(row.depth as f32 * TREE_DISCLOSURE));
    }
    if row.has_children {
        let toggle = ui.button(
            r,
            "",
            Style::new().flat().padding(1.0).width(TREE_DISCLOSURE),
        );
        if let Some(icons) = ctx.icons {
            let chevron = if row.expanded {
                Icon::ChevronDown
            } else {
                Icon::ChevronRight
            };
            ui.image(
                toggle,
                icons.get(chevron),
                Style::new().size(TREE_CHEVRON, TREE_CHEVRON),
            );
        }
        rows.file_tree_toggles.push((toggle, row.path.clone()));
    } else {
        ui.panel(r, Style::new().width(TREE_DISCLOSURE));
    }
    if let Some(icons) = ctx.icons {
        let folder = if row.expanded {
            Icon::FolderOpen
        } else {
            Icon::Folder
        };
        ui.image(
            r,
            icons.get(folder),
            Style::new().size(FILE_TREE_ICON, FILE_TREE_ICON),
        );
    }
    ui.label(
        r,
        row.name.clone(),
        Style::new()
            .grow(1.0)
            .ellipsis()
            .foreground(theme.aurora.heading),
    );
    rows.file_tree_rows.push((r, row.path.clone()));
}

/// The contents of the selected folder: an empty-state label, a list, or a grid.
fn build_contents(ui: &mut Ui, contents: WidgetId, ctx: &PaneCtx, rows: &mut EditorRows) {
    let items = ctx.explorer.contents();
    if items.is_empty() {
        ui.label(
            contents,
            "This folder is empty".to_string(),
            Style::new()
                .padding(4.0)
                .foreground(ctx.theme.aurora.subhead),
        );
        return;
    }
    match ctx.explorer.view() {
        FileView::List => {
            for entry in items {
                build_list_entry(ui, contents, entry, ctx, rows);
            }
        }
        FileView::Grid => {
            for chunk in items.chunks(ctx.explorer_cols.max(1)) {
                let row = ui.panel(contents, Style::new().row().gap(FILE_GRID_GAP));
                for entry in chunk {
                    build_grid_cell(ui, row, entry, ctx, rows);
                }
            }
        }
    }
}

/// A list-view row: a small icon/thumbnail, the name, and a muted type tag.
fn build_list_entry(
    ui: &mut Ui,
    parent: WidgetId,
    entry: &Entry,
    ctx: &PaneCtx,
    rows: &mut EditorRows,
) {
    let theme = ctx.theme;
    let background = if ctx.explorer.is_selected(&entry.path) {
        theme.aurora.row_selected
    } else {
        Color::TRANSPARENT
    };
    let draggable = entry_draggable(entry);
    let mut style = Style::new()
        .row()
        .gap(6.0)
        .align_center()
        .padding(3.0)
        .flat()
        .background(background);
    if draggable {
        style = style.draggable().drag_kind(drag_kind(&entry.path));
    }
    let r = ui.button(parent, "", style);
    place_entry_icon(ui, r, entry, ctx, FILE_LIST_ICON);
    let label = Style::new()
        .grow(1.0)
        .ellipsis()
        .foreground(theme.aurora.heading);
    let field = Style::new().grow(1.0).padding(1.0);
    build_entry_name(ui, r, entry, ctx, rows, field, label);
    // Detail columns (muted, fixed-width so they line up down the list).
    meta_col(ui, r, entry.type_label(), FILE_COL_TYPE, theme);
    meta_col(ui, r, entry.size_label(), FILE_COL_SIZE, theme);
    meta_col(ui, r, entry.modified_label(), FILE_COL_MODIFIED, theme);
    record_entry(entry, r, ctx, rows, draggable);
}

/// A fixed-width, muted detail column in a list row.
fn meta_col(ui: &mut Ui, parent: WidgetId, text: String, width: f32, theme: &EditorTheme) {
    ui.label(
        parent,
        text,
        Style::new()
            .width(width)
            .ellipsis()
            .foreground(theme.aurora.subhead),
    );
}

/// A grid cell: a large thumbnail/icon over a centered, ellipsized name.
fn build_grid_cell(
    ui: &mut Ui,
    parent: WidgetId,
    entry: &Entry,
    ctx: &PaneCtx,
    rows: &mut EditorRows,
) {
    let theme = ctx.theme;
    let background = if ctx.explorer.is_selected(&entry.path) {
        theme.aurora.row_selected
    } else {
        Color::TRANSPARENT
    };
    let draggable = entry_draggable(entry);
    let mut style = Style::new()
        .column()
        .align_center()
        .gap(4.0)
        .padding(4.0)
        .width(FILE_GRID_CELL)
        .flat()
        .corner_radius(theme.aurora.control_radius)
        .background(background);
    if draggable {
        style = style.draggable().drag_kind(drag_kind(&entry.path));
    }
    let cell = ui.button(parent, "", style);
    place_entry_icon(ui, cell, entry, ctx, FILE_GRID_THUMB);
    let label = Style::new()
        .width(FILE_GRID_CELL - 8.0)
        .ellipsis()
        .text_center()
        .foreground(theme.aurora.heading);
    let field = Style::new().width(FILE_GRID_CELL - 8.0).padding(1.0);
    build_entry_name(ui, cell, entry, ctx, rows, field, label);
    record_entry(entry, cell, ctx, rows, draggable);
}

/// Render a contents entry's name: an inline rename field when this entry is
/// being renamed, otherwise a plain (styled) label.
fn build_entry_name(
    ui: &mut Ui,
    parent: WidgetId,
    entry: &Entry,
    ctx: &PaneCtx,
    rows: &mut EditorRows,
    field_style: Style,
    label_style: Style,
) {
    if ctx.explorer.renaming() == Some(entry.path.as_path()) {
        let field = ui.text_input(
            parent,
            entry.name.clone(),
            field_style
                .foreground(ctx.theme.aurora.heading)
                .corner_radius(ctx.theme.aurora.inset_radius)
                .border(ctx.theme.aurora.border_width, ctx.theme.aurora.focus),
        );
        rows.file_rename_field = Some((field, entry.path.clone()));
    } else {
        ui.label(parent, entry.name.clone(), label_style);
    }
}

/// Place an entry's visual: a ready thumbnail wins, else the type icon, else
/// (no icon set, as in headless tests) a same-size spacer to keep the layout.
fn place_entry_icon(ui: &mut Ui, parent: WidgetId, entry: &Entry, ctx: &PaneCtx, size: f32) {
    if let Some(handle) = ctx.thumbnails.get(&entry.path) {
        ui.image(parent, handle, Style::new().size(size, size));
    } else if let Some(icons) = ctx.icons {
        ui.image(
            parent,
            icons.get(entry_icon(entry)),
            Style::new().size(size, size),
        );
    } else {
        ui.panel(parent, Style::new().width(size).height(size));
    }
}

/// The type icon for an entry (before any thumbnail is available).
fn entry_icon(entry: &Entry) -> Icon {
    if entry.is_dir() {
        return Icon::Folder;
    }
    match classify(&entry.path) {
        FileKind::Image => Icon::FileImage,
        FileKind::Scene => Icon::FileScene,
        FileKind::Script => Icon::FileScript,
        FileKind::Other => Icon::FileGeneric,
    }
}

/// What a dragged file carries, so a drop target can turn it down before the
/// release rather than after (aurora's `Style::drag_kind` / `Style::accepts`).
/// Offering a target that would refuse the drop promises something a release
/// does not deliver.
pub const DRAG_IMAGE: u32 = 1 << 0;
pub const DRAG_SCRIPT: u32 = 1 << 1;

/// The kind a dragged file carries.
fn drag_kind(path: &Path) -> u32 {
    match classify(path) {
        FileKind::Script => DRAG_SCRIPT,
        _ => DRAG_IMAGE,
    }
}

/// Whether an entry can be dragged out of the explorer at all - which means
/// whether dropping it anywhere would do something. A decodable image (the same
/// gate the sprite loader uses) spawns or retextures a sprite; a `.cmt` gives a
/// node a script. Anything else stays put rather than dragging to no effect.
fn entry_draggable(entry: &Entry) -> bool {
    !entry.is_dir() && (can_thumbnail(&entry.path) || classify(&entry.path) == FileKind::Script)
}

/// Record an entry's widget for event routing: every entry for click handling,
/// and draggable images additionally in `file_rows` with their spawn payload.
fn record_entry(
    entry: &Entry,
    widget: WidgetId,
    ctx: &PaneCtx,
    rows: &mut EditorRows,
    draggable: bool,
) {
    rows.file_entries
        .push((widget, entry.path.clone(), entry.is_dir()));
    if draggable && let Some(rel) = ctx.explorer.project_relative(&entry.path) {
        rows.file_rows.push((widget, rel));
    }
}

/// Render a reflected [`Value`] as editable text.
pub fn value_to_text(value: &Value) -> String {
    match value {
        Value::F32(x) => x.to_string(),
        Value::Int(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Str(s) => s.clone(),
        Value::Vec2(v) => format!("{}, {}", v.x, v.y),
        Value::Color(c) => format!("{}, {}, {}, {}", c[0], c[1], c[2], c[3]),
        Value::Asset(s) => s.clone(),
    }
}

/// Parse `text` into a [`Value`] shaped like `like` (its variant carries the
/// field's type, since text alone does not). `None` on anything that does not
/// parse, so a bad edit leaves the field's prior value untouched.
pub fn parse_value(text: &str, like: &Value) -> Option<Value> {
    match like {
        Value::F32(_) => text.trim().parse().ok().map(Value::F32),
        // A whole number only: `3.5` typed into an int field is refused rather
        // than truncated, so the value shown is always the value that runs.
        Value::Int(_) => text.trim().parse().ok().map(Value::Int),
        Value::Bool(_) => text.trim().parse().ok().map(Value::Bool),
        Value::Str(_) => Some(Value::Str(text.to_string())),
        Value::Asset(_) => Some(Value::Asset(text.to_string())),
        Value::Vec2(_) => {
            let mut parts = text.split(',').map(|p| p.trim().parse::<f32>());
            let x = parts.next()?.ok()?;
            let y = parts.next()?.ok()?;
            Some(Value::Vec2(Vec2::new(x, y)))
        }
        Value::Color(_) => {
            let nums: Vec<f32> = text
                .split(',')
                .filter_map(|p| p.trim().parse::<f32>().ok())
                .collect();
            (nums.len() == 4).then(|| Value::Color([nums[0], nums[1], nums[2], nums[3]]))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helios::{Component, Node, SpriteComponent, Transform};
    use std::path::Path;

    /// A bare file entry at `path`, for the classification tests.
    fn file_entry(path: &str) -> Entry {
        Entry {
            path: path.into(),
            name: path.rsplit('/').next().unwrap_or(path).to_string(),
            kind: crate::explorer::EntryKind::File,
            size: 0,
            modified: None,
            children: Vec::new(),
        }
    }

    #[test]
    fn an_asset_field_shows_the_icon_for_what_it_holds() {
        // The bug this pins: every asset field used to show the image icon, so a
        // script component looked like it held a picture.
        assert_eq!(
            asset_field_icon("scripts/bounce.cmt", AssetKind::Script),
            Icon::FileScript
        );
        assert_eq!(
            asset_field_icon("assets/hero.png", AssetKind::Image),
            Icon::FileImage
        );
        // Empty field: nothing to read off the path, so the component decides.
        assert_eq!(asset_field_icon("", AssetKind::Script), Icon::FileScript);
        assert_eq!(asset_field_icon("", AssetKind::Image), Icon::FileImage);
    }

    #[test]
    fn a_dragged_file_is_tagged_with_what_it_is() {
        assert_eq!(drag_kind(Path::new("a/bounce.cmt")), DRAG_SCRIPT);
        assert_eq!(drag_kind(Path::new("a/hero.png")), DRAG_IMAGE);
        assert_eq!(
            DRAG_SCRIPT & DRAG_IMAGE,
            0,
            "the kinds must not overlap, or every target would take everything"
        );
    }

    #[test]
    fn a_file_is_draggable_when_dropping_it_would_do_something() {
        // Images spawn or retexture a sprite; scripts give a node something to
        // run. Anything else would drag to no effect, so it does not drag.
        assert!(entry_draggable(&file_entry("/p/assets/hero.png")));
        assert!(entry_draggable(&file_entry("/p/scripts/bounce.cmt")));
        assert!(!entry_draggable(&file_entry("/p/notes.txt")));
        assert!(!entry_draggable(&file_entry("/p/scenes/main.ron")));

        let mut folder = file_entry("/p/assets");
        folder.kind = crate::explorer::EntryKind::Dir;
        assert!(!entry_draggable(&folder));
    }

    /// A file explorer over a path that does not exist, so its scan is empty and
    /// instant - the shell tests exercise the dock/inspector, not the file pane,
    /// and must not recursively walk the real temp dir.
    fn test_explorer() -> FileExplorer {
        FileExplorer::new(Path::new("/orbit-nonexistent-test-root"))
    }

    /// Build a modal on a bare Ui and lay it out (a fresh root + one popup).
    /// The inspector for `node`, laid out headless.
    use aurora::InputEvent;

    fn laid_out_inspector(scene: &Scene, node: NodeId) -> EditorRows {
        laid_out_inspector_ui(scene, node).1
    }

    /// The same, keeping the Ui so a test can look at where things landed.
    fn laid_out_inspector_ui(scene: &Scene, node: NodeId) -> (Ui, EditorRows) {
        let theme = EditorTheme::default();
        let mut ui = Ui::new();
        ui.set_theme(theme.aurora);
        let root = ui.root_panel(Style::new().fill());
        let mut rows = EditorRows::default();
        build_inspector(
            &mut ui,
            root,
            scene,
            Some(node),
            None,
            &InspectorView::default(),
            &FileExplorer::new(Path::new("/orbit-nonexistent-test-root")),
            &Thumbnails::new(),
            &theme,
            &mut rows,
        );
        ui.layout(Vec2::new(400.0, 700.0)).unwrap();
        (ui, rows)
    }

    fn laid_out_modal(modal: &Modal) -> EditorRows {
        let theme = EditorTheme::default();
        let mut ui = Ui::new();
        ui.set_theme(theme.aurora);
        ui.root_panel(Style::new().fill());
        let mut rows = EditorRows::default();
        let thumbnails = Thumbnails::new();
        build_modal(&mut ui, modal, None, &thumbnails, &theme, &mut rows);
        ui.layout(Vec2::new(1000.0, 700.0)).unwrap();
        rows
    }

    #[test]
    fn losing_the_scene_always_offers_the_same_three_answers() {
        // Reloading the project replaces the scene and resets the undo stack,
        // which is exactly what closing the window does - so it gets the same
        // question, with the same way out. ctrl+O used to just do it.
        for pending in [
            crate::modal::Pending::Quit,
            crate::modal::Pending::ReloadProject,
        ] {
            let rows = laid_out_modal(&Modal::confirm("Unsaved changes.", pending.clone()));
            assert_eq!(
                rows.modal_buttons.len(),
                3,
                "save, discard, and a way to change your mind"
            );
            assert!(
                rows.modal_close.is_some(),
                "and Escape has something to hit"
            );
        }
    }

    #[test]
    fn a_message_modal_lays_out_with_a_card_close_and_one_button() {
        let err = std::io::Error::new(std::io::ErrorKind::AlreadyExists, "x");
        let rows = laid_out_modal(&Modal::error("Rename failed", &err));
        assert!(rows.modal_card.is_some(), "the card is recorded");
        assert!(rows.modal_close.is_some(), "closable -> a close button");
        assert_eq!(rows.modal_buttons.len(), 1, "just OK");
        assert!(rows.modal_checks.is_empty() && rows.modal_inputs.is_empty());
    }

    #[test]
    fn a_settings_modal_lays_out_with_four_checks_three_inputs_and_two_buttons() {
        let draft = SettingsDraft::new(true, true, true, crate::settings::SnapSettings::default());
        let rows = laid_out_modal(&Modal::settings(draft));
        assert_eq!(rows.modal_checks.len(), 4, "grid / axes / tidy / snap");
        assert_eq!(rows.modal_inputs.len(), 3, "move / rotate / scale step");
        assert_eq!(rows.modal_buttons.len(), 2, "Cancel + Save");
    }

    #[test]
    fn an_asset_chooser_modal_lays_out_a_grid_of_choices() {
        let scene = Scene::new("root");
        let target = crate::modal::AssetTarget {
            node: scene.root(),
            component: 0,
            field: "texture".to_string(),
        };
        let images = vec![
            crate::explorer::AssetRef {
                abs: "/p/a.png".into(),
                rel: "a.png".into(),
                name: "a.png".into(),
            },
            crate::explorer::AssetRef {
                abs: "/p/b.png".into(),
                rel: "b.png".into(),
                name: "b.png".into(),
            },
        ];
        let rows = laid_out_modal(&Modal::asset_chooser(target, images, AssetKind::Image));
        assert_eq!(rows.asset_choices.len(), 2, "one cell per image");
        assert_eq!(
            rows.asset_choices[0].1, "a.png",
            "cell records its rel path"
        );
        assert!(rows.modal_close.is_some());
        assert_eq!(rows.modal_buttons.len(), 1, "just Cancel");
    }

    /// Build the shell the way the tests did before docking: a single optional
    /// selection, the default dock layout, no scroll, no search, no rename.
    /// Keeps the test call sites terse while exercising the real builder. The
    /// unused `_sizes` argument keeps existing call sites compiling unchanged.
    #[allow(clippy::too_many_arguments)]
    fn build_editor_ui_test(
        scene: &Scene,
        selected: Option<NodeId>,
        viewport_handle: ImageHandle,
        _project_dir: &Path,
        _sizes: (),
        menu: Option<&ContextMenu>,
        gizmo_mode: GizmoMode,
        icons: Option<&Icons>,
        tree: &TreeView,
        inspector: &InspectorView,
        theme: &EditorTheme,
    ) -> (Ui, EditorRows) {
        let set: HashSet<NodeId> = selected.into_iter().collect();
        let explorer = test_explorer();
        let thumbnails = Thumbnails::new();
        build_editor_ui(
            scene,
            &set,
            selected,
            viewport_handle,
            &explorer,
            &thumbnails,
            3,
            &DockNode::default_layout(),
            &HashMap::new(),
            menu,
            gizmo_mode,
            true,
            true,
            false,
            icons,
            tree,
            "",
            None,
            inspector,
            &[],
            true,
            None,
            "",
            false,
            false,
            &[],
            &[],
            theme,
        )
    }

    fn demo_scene() -> (Scene, ImageHandle) {
        let mut scene = Scene::new("Root");
        let root = scene.root();
        let mut sprite = Node::new("Sprite");
        sprite.transform = Transform::from_translation(Vec2::new(10.0, 10.0));
        sprite
            .components
            .push(Component::Sprite(SpriteComponent::default()));
        scene.add_child(root, sprite);

        // A backend mints handles; a bare SlotMap stands in headlessly.
        let mut registry: slotmap::SlotMap<ImageHandle, ()> = slotmap::SlotMap::with_key();
        let handle = registry.insert(());
        (scene, handle)
    }

    #[test]
    fn live_inspector_updates_do_not_relayout_the_shell() {
        // The viewport-drag fps regression: updating the inspector's transform
        // inputs each frame (a field buried deep under the dock) re-measured
        // thousands of nodes per frame. A single-line field's box is
        // text-independent, so these updates must now cost zero re-measures.
        let (scene, handle) = demo_scene();
        let sprite = scene.children(scene.root())[0];
        let explorer = test_explorer();
        let thumbnails = Thumbnails::new();
        let dock = DockNode::default_layout();
        let mut set = HashSet::new();
        set.insert(sprite);
        let (mut ui, rows) = build_editor_ui(
            &scene,
            &set,
            Some(sprite),
            handle,
            &explorer,
            &thumbnails,
            3,
            &dock,
            &HashMap::new(),
            None,
            GizmoMode::Select,
            true,
            true,
            false,
            None,
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
            &EditorTheme::default(),
        );
        ui.layout(Vec2::new(1200.0, 800.0)).unwrap();
        ui.layout(Vec2::new(1200.0, 800.0)).unwrap();
        assert_eq!(ui.last_measure_count(), 0, "cached when nothing changed");

        // Some inspector fields actually exist to drive.
        assert!(!rows.vec_inputs.is_empty());
        for &(w, ..) in &rows.vec_inputs {
            ui.set_text_input(w, "123.4");
        }
        for &(w, ..) in &rows.transform_rows {
            ui.set_text_input(w, "45.6");
        }
        ui.layout(Vec2::new(1200.0, 800.0)).unwrap();
        assert_eq!(
            ui.last_measure_count(),
            0,
            "live inspector updates must not re-measure the tree"
        );
    }

    #[test]
    fn visible_nodes_honors_collapse_and_the_search_filter() {
        // Root -> [Alpha -> [Beta], Gamma]
        let mut scene = Scene::new("Root");
        let root = scene.root();
        let alpha = scene.add_child(root, Node::new("Alpha"));
        let beta = scene.add_child(alpha, Node::new("Beta"));
        let gamma = scene.add_child(root, Node::new("Gamma"));

        // Fully expanded, no filter: every node, in depth-first order.
        let all = visible_nodes(&scene, &TreeView::default(), "");
        assert_eq!(all, vec![root, alpha, beta, gamma]);

        // Collapsing Alpha hides Beta but not Gamma.
        let mut collapsed = HashSet::new();
        collapsed.insert(alpha);
        let view = TreeView {
            collapsed,
            drop_target: None,
        };
        assert_eq!(visible_nodes(&scene, &view, ""), vec![root, alpha, gamma]);

        // A search for "beta" keeps Beta and its ancestors (Root, Alpha) for
        // context, drops the non-matching Gamma, and ignores the collapse.
        assert_eq!(
            visible_nodes(&scene, &view, "beta"),
            vec![root, alpha, beta]
        );
        // Case-insensitive. A match shows its ancestors for context but not its
        // non-matching descendants, so "alpha" shows Root and Alpha, not Beta.
        assert_eq!(
            visible_nodes(&scene, &TreeView::default(), "ALPHA"),
            vec![root, alpha]
        );
    }

    #[test]
    fn resolve_drop_reorders_and_reparents_correctly() {
        use helios::History;
        let mut scene = Scene::new("Root");
        let root = scene.root();
        let a = scene.add_child(root, Node::new("A"));
        let b = scene.add_child(root, Node::new("B"));
        let c = scene.add_child(root, Node::new("C"));
        let names = |s: &Scene, p| {
            s.children(p)
                .iter()
                .map(|&n| s.node(n).name.clone())
                .collect::<Vec<_>>()
        };

        // Move A to after B: [A, B, C] -> [B, A, C].
        let (p, i) = resolve_drop(&scene, a, DropSpot::After(b)).unwrap();
        let mut hist = History::new();
        hist.reparent(&mut scene, a, p, i);
        assert_eq!(names(&scene, root), ["B", "A", "C"]);

        // Move C before B: -> [B, C, A] -> ... now list is [B, A, C]; C before B.
        let (p, i) = resolve_drop(&scene, c, DropSpot::Before(b)).unwrap();
        hist.reparent(&mut scene, c, p, i);
        assert_eq!(names(&scene, root), ["C", "B", "A"]);

        // Into makes a child; invalid drops (self, own descendant) are None.
        assert_eq!(resolve_drop(&scene, a, DropSpot::Into(b)), Some((b, 0)));
        assert_eq!(resolve_drop(&scene, a, DropSpot::Before(a)), None);
        hist.reparent(&mut scene, a, b, 0); // A is now a child of B
        assert_eq!(
            resolve_drop(&scene, b, DropSpot::Into(a)),
            None,
            "no cycles"
        );
    }

    #[test]
    fn collapsing_a_node_hides_its_children_in_the_tree() {
        // root -> parent -> child
        let mut scene = Scene::new("Root");
        let root = scene.root();
        let parent = scene.add_child(root, Node::new("Parent"));
        let child = scene.add_child(parent, Node::new("Child"));
        let mut reg: slotmap::SlotMap<ImageHandle, ()> = slotmap::SlotMap::with_key();
        let handle = reg.insert(());
        let dir = std::env::temp_dir();

        let build = |tree: &TreeView| {
            build_editor_ui_test(
                &scene,
                None,
                handle,
                &dir,
                (),
                None,
                GizmoMode::Select,
                None,
                tree,
                &InspectorView::default(),
                &EditorTheme::default(),
            )
            .1
        };
        // Expanded: rows for root, parent, and child.
        let rows = build(&TreeView::default());
        let nodes: Vec<NodeId> = rows.tree_rows.iter().map(|(_, n)| *n).collect();
        assert!(nodes.contains(&child));

        // Collapse the parent: the child's row is gone, the parent's stays.
        let mut collapsed = HashSet::new();
        collapsed.insert(parent);
        let rows = build(&TreeView {
            collapsed,
            drop_target: None,
        });
        let nodes: Vec<NodeId> = rows.tree_rows.iter().map(|(_, n)| *n).collect();
        assert!(nodes.contains(&parent));
        assert!(
            !nodes.contains(&child),
            "collapsed parent's child is hidden"
        );
        // The parent still has a disclosure toggle to expand it again.
        assert!(rows.tree_toggles.iter().any(|(_, n)| *n == parent));
    }

    #[test]
    fn the_editor_theme_colors_the_shell() {
        let (scene, handle) = demo_scene();
        let dir = std::env::temp_dir();
        let build = |theme: &EditorTheme| {
            let (mut ui, _) = build_editor_ui_test(
                &scene,
                None,
                handle,
                &dir,
                (),
                None,
                GizmoMode::Select,
                None,
                &TreeView::default(),
                &InspectorView::default(),
                theme,
            );
            ui.layout(Vec2::new(1000.0, 700.0)).unwrap();
            ui.draw_list().commands
        };
        let has_fill = |cmds: &[aurora::DrawCommand], want: Color| {
            cmds.iter()
                .any(|c| matches!(c, aurora::DrawCommand::FillRect { color, .. } if *color == want))
        };
        // Each theme paints its own root background; the other's does not appear.
        let dark = build(&EditorTheme::dark());
        assert!(has_fill(&dark, EditorTheme::dark().aurora.root_bg));
        assert!(!has_fill(&dark, EditorTheme::light().aurora.root_bg));

        let light = build(&EditorTheme::light());
        assert!(has_fill(&light, EditorTheme::light().aurora.root_bg));
    }

    #[test]
    fn every_tree_row_has_an_eye_toggle() {
        let mut scene = Scene::new("Root");
        let root = scene.root();
        let a = scene.add_child(root, Node::new("A"));
        let mut reg: slotmap::SlotMap<ImageHandle, ()> = slotmap::SlotMap::with_key();
        let handle = reg.insert(());
        let dir = std::env::temp_dir();
        let (_, rows) = build_editor_ui_test(
            &scene,
            None,
            handle,
            &dir,
            (),
            None,
            GizmoMode::Select,
            None,
            &TreeView::default(),
            &InspectorView::default(),
            &EditorTheme::default(),
        );
        // One eye toggle per row (root and A), each mapped to its node.
        assert_eq!(rows.eye_toggles.len(), rows.tree_rows.len());
        assert!(rows.eye_toggles.iter().any(|(_, n)| *n == root));
        assert!(rows.eye_toggles.iter().any(|(_, n)| *n == a));
    }

    #[test]
    fn collapsing_an_inspector_section_hides_its_fields() {
        let (scene, handle) = demo_scene();
        let sprite = scene.children(scene.root())[0];
        let dir = std::env::temp_dir();
        let build = |inspector: &InspectorView| {
            build_editor_ui_test(
                &scene,
                Some(sprite),
                handle,
                &dir,
                (),
                None,
                GizmoMode::Select,
                None,
                &TreeView::default(),
                inspector,
                &EditorTheme::default(),
            )
            .1
        };

        // Expanded: the Transform section has its rotation row, the Sprite its
        // fields, and both sections offer a collapse toggle.
        let rows = build(&InspectorView::default());
        assert!(
            rows.transform_rows
                .iter()
                .any(|(_, n, f)| *n == sprite && *f == "rotation")
        );
        assert!(!rows.field_rows.is_empty(), "sprite fields present");
        assert!(
            rows.section_toggles
                .iter()
                .any(|(_, s)| *s == InspectorSection::Transform)
        );
        assert!(
            rows.section_toggles
                .iter()
                .any(|(_, s)| *s == InspectorSection::Component(0))
        );

        // Collapse the Transform section (for this node): its rows vanish, the
        // component's stay.
        let mut collapsed = HashSet::new();
        collapsed.insert((sprite, InspectorSection::Transform));
        let rows = build(&InspectorView { collapsed });
        assert!(
            rows.transform_rows.is_empty(),
            "collapsed transform builds no field rows"
        );
        assert!(
            !rows.field_rows.is_empty(),
            "the component is still expanded"
        );
        // Its header toggle remains, to expand it again.
        assert!(
            rows.section_toggles
                .iter()
                .any(|(_, s)| *s == InspectorSection::Transform)
        );
    }

    #[test]
    fn hovering_a_tree_row_resolves_to_its_row_widget() {
        // The reparent drag looks up the row via the interactive widget under
        // the cursor (hovered), not a raw hit-test - which would return the
        // row's label child and miss. This pins that.
        let (scene, handle) = demo_scene();
        let sprite = scene.children(scene.root())[0];
        let dir = std::env::temp_dir();
        let (mut ui, rows) = build_editor_ui_test(
            &scene,
            None,
            handle,
            &dir,
            (),
            None,
            GizmoMode::Select,
            None,
            &TreeView::default(),
            &InspectorView::default(),
            &EditorTheme::default(),
        );
        ui.layout(Vec2::new(1200.0, 800.0)).unwrap();

        let (row_id, _) = *rows
            .tree_rows
            .iter()
            .find(|(_, n)| *n == sprite)
            .expect("the sprite has a row");
        let rect = ui.rect(row_id).unwrap();
        // Move the cursor over the row's name (the middle of the row - right of
        // the disclosure column, left of the eye toggle at the far edge).
        ui.handle_input(aurora::InputEvent::PointerMoved(
            rect.pos + Vec2::new(rect.size.x * 0.5, rect.size.y * 0.5),
        ));
        assert_eq!(
            ui.hovered(),
            Some(row_id),
            "the interactive widget under a row is the row itself"
        );
    }

    #[test]
    fn a_selected_node_gets_vec2_axis_inputs_and_scrub_labels() {
        let (scene, handle) = demo_scene();
        let node = scene.children(scene.root())[0];
        let dir = std::env::temp_dir();
        let (_ui, rows) = build_editor_ui_test(
            &scene,
            Some(node),
            handle,
            &dir,
            (),
            None,
            GizmoMode::Select,
            None,
            &TreeView::default(),
            &InspectorView::default(),
            &EditorTheme::default(),
        );

        // Three Vec2 fields (position, scale, sprite size) -> two axes each.
        assert_eq!(rows.vec_inputs.len(), 6);
        assert_eq!(rows.scrub_labels.len(), 6);
        // Position's x/y inputs reference the node's translation axes.
        let axes: Vec<Axis> = rows
            .vec_inputs
            .iter()
            .filter(|(_, f, _)| *f == FieldRef::Position(node))
            .map(|(_, _, a)| *a)
            .collect();
        assert_eq!(axes, vec![Axis::X, Axis::Y]);
    }

    #[test]
    fn vec2_field_helpers_round_trip() {
        let (mut scene, _) = demo_scene();
        let node = scene.children(scene.root())[0];
        let field = FieldRef::Position(node);
        assert_eq!(field_vec2(&scene, &field), Vec2::new(10.0, 10.0));

        // Scrub the X component and read it back; Y is untouched.
        let moved = with_axis(field_vec2(&scene, &field), Axis::X, 42.0);
        set_field_vec2(&mut scene, &field, moved);
        assert_eq!(field_vec2(&scene, &field), Vec2::new(42.0, 10.0));
        assert_eq!(axis_of(field_vec2(&scene, &field), Axis::Y), 10.0);
    }

    /// The widget of the tab group currently showing `pane` (its content area).
    fn pane_group(rows: &EditorRows, pane: Pane) -> WidgetId {
        rows.dock_groups
            .iter()
            .find(|(_, p)| *p == pane)
            .map(|(w, _)| *w)
            .expect("pane should have a group")
    }

    /// Build the shell with a given dock layout and scroll offsets (the paths
    /// the docking tests exercise), otherwise the default editor state.
    fn build_with_dock(
        scene: &Scene,
        handle: ImageHandle,
        _dir: &Path,
        dock: &DockNode,
        scrolls: &HashMap<Pane, f32>,
    ) -> (Ui, EditorRows) {
        let explorer = test_explorer();
        let thumbnails = Thumbnails::new();
        build_editor_ui(
            scene,
            &HashSet::new(),
            None,
            handle,
            &explorer,
            &thumbnails,
            3,
            dock,
            scrolls,
            None,
            GizmoMode::Select,
            true,
            true,
            false,
            None,
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
            &EditorTheme::default(),
        )
    }

    #[test]
    fn the_console_pane_renders_its_log_lines() {
        let (scene, handle) = demo_scene();
        let dock = DockNode::Leaf {
            panes: vec![Pane::Console],
            active: 0,
        };
        let logs = vec![
            LogLine {
                level: tracing::Level::INFO,
                target: "atlas".into(),
                message: "started".into(),
            },
            LogLine {
                level: tracing::Level::WARN,
                target: "atlas::wgpu".into(),
                message: "careful".into(),
            },
        ];
        let explorer = test_explorer();
        let thumbnails = Thumbnails::new();
        let (mut ui, rows) = build_editor_ui(
            &scene,
            &HashSet::new(),
            None,
            handle,
            &explorer,
            &thumbnails,
            3,
            &dock,
            &HashMap::new(),
            None,
            GizmoMode::Select,
            true,
            true,
            false,
            None,
            &TreeView::default(),
            "",
            None,
            &InspectorView::default(),
            &logs,
            true,
            None,
            "",
            false,
            false,
            &[],
            &[],
            &EditorTheme::default(),
        );
        assert!(rows.console_panel.is_some());
        assert!(rows.pane_scrolls.iter().any(|(p, _)| *p == Pane::Console));
        ui.layout(Vec2::new(600.0, 400.0)).unwrap();
        let texts = ui
            .draw_list()
            .commands
            .iter()
            .filter(|c| matches!(c, aurora::DrawCommand::Text { .. }))
            .count();
        assert!(texts >= 2, "expected a label per log line, drew {texts}");
    }

    #[test]
    fn a_panel_resize_reshapes_no_text() {
        // The reported fps drop: resizing a panel re-shaped ~2600 unchanged text
        // buffers per drag frame (taffy re-measures the stretched subtree). With
        // the shape cache it must re-shape nothing - no text changed.
        let (scene, handle) = demo_scene();
        let sprite = scene.children(scene.root())[0];
        let explorer = test_explorer();
        let thumbnails = Thumbnails::new();
        let dock = DockNode::default_layout();
        let mut set = HashSet::new();
        set.insert(sprite);
        let (mut ui, rows) = build_editor_ui(
            &scene,
            &set,
            Some(sprite),
            handle,
            &explorer,
            &thumbnails,
            3,
            &dock,
            &HashMap::new(),
            None,
            GizmoMode::Select,
            true,
            true,
            false,
            None,
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
            &EditorTheme::default(),
        );
        ui.layout(Vec2::new(1200.0, 800.0)).unwrap();
        ui.layout(Vec2::new(1200.0, 800.0)).unwrap();
        assert_eq!(ui.last_measure_count(), 0, "idle re-shapes nothing");

        let tree = pane_group(&rows, Pane::SceneTree);
        let w = ui.rect(tree).unwrap().size.x;
        ui.set_width(tree, w + 20.0);
        ui.layout(Vec2::new(1200.0, 800.0)).unwrap();
        assert_eq!(
            ui.last_measure_count(),
            0,
            "resizing a panel must not re-shape any text"
        );
    }

    #[test]
    fn a_slow_narrowing_drag_does_not_reshape_text_every_frame() {
        // The regression this guards: a real splitter drag moves a pane a pixel
        // at a time, and NARROWING is the direction that makes ellipsis labels
        // re-fit. Keying a truncation on the exact box width missed on every
        // frame, re-shaping several times per label per frame - which the old
        // single-widening-step test could not see (widening is exactly the case
        // the shape cache absorbs). Drive an actual drag and bound the total.
        let (scene, handle) = demo_scene();
        let dir = std::env::temp_dir();
        let dock = DockNode::default_layout();
        let (mut ui, rows) = build_with_dock(&scene, handle, &dir, &dock, &HashMap::new());
        ui.layout(Vec2::new(1200.0, 800.0)).unwrap();

        let tree = pane_group(&rows, Pane::SceneTree);
        let start = ui.rect(tree).unwrap().size.x;
        // 60 one-pixel narrowing steps, as dragging the splitter left would.
        let mut total = 0u32;
        for step in 1..=60 {
            ui.set_width(tree, start - step as f32);
            ui.layout(Vec2::new(1200.0, 800.0)).unwrap();
            total += ui.last_measure_count();
        }
        // Text only re-fits when the box crosses one more character's width, so
        // a 60px drag may re-shape a label a handful of times - not every frame.
        // Before the fix this was in the hundreds.
        assert!(
            total < 120,
            "a 60-frame narrowing drag re-shaped text {total} times; \
             it should re-fit only when crossing a character boundary"
        );
    }

    #[test]
    fn a_cold_rebuild_shapes_each_text_once_not_the_whole_tree_repeatedly() {
        // taffy probes each text leaf several times per layout; the shape cache
        // must collapse those to one shape per distinct string, so even a full
        // cold rebuild stays cheap (it was thousands of re-shapes before).
        let (scene, handle) = demo_scene();
        let dir = std::env::temp_dir();
        let dock = DockNode::default_layout();
        let (mut ui, _rows) = build_with_dock(&scene, handle, &dir, &dock, &HashMap::new());
        ui.layout(Vec2::new(1200.0, 800.0)).unwrap();
        assert!(
            ui.last_measure_count() < 200,
            "a cold rebuild should shape each text about once, got {}",
            ui.last_measure_count()
        );
    }

    #[test]
    fn dock_split_sizes_survive_a_shell_rebuild() {
        // The regression: drag a splitter, then trigger a rebuild (as a
        // selection change does) - the dragged size must carry over, not snap
        // back to the default.
        let (scene, handle) = demo_scene();
        let dir = std::env::temp_dir(); // no project files needed for this test
        let mut dock = DockNode::default_layout();
        let scrolls = HashMap::new();

        let (mut ui, rows) = build_with_dock(&scene, handle, &dir, &dock, &scrolls);
        ui.layout(Vec2::new(1200.0, 800.0)).unwrap();

        // Drag the tree pane's splitter 50px right, as a user would.
        let tree = pane_group(&rows, Pane::SceneTree);
        let before = ui.rect(tree).unwrap().size.x;
        let bar = Vec2::new(ui.rect(tree).unwrap().max().x + 2.0, 100.0);
        ui.handle_input(aurora::InputEvent::PointerMoved(bar));
        ui.handle_input(aurora::InputEvent::PointerPressed);
        ui.handle_input(aurora::InputEvent::PointerMoved(bar + Vec2::new(50.0, 0.0)));
        ui.handle_input(aurora::InputEvent::PointerReleased);
        ui.layout(Vec2::new(1200.0, 800.0)).unwrap();
        assert_eq!(ui.rect(tree).unwrap().size.x, before + 50.0);

        // Capture the dragged size back into the dock, then rebuild (what a
        // tree-row click triggers): the width carries over.
        dock.set_fixed_sizes(&capture_dock_sizes(&ui, &rows));
        let (mut ui2, rows2) = build_with_dock(&scene, handle, &dir, &dock, &scrolls);
        ui2.layout(Vec2::new(1200.0, 800.0)).unwrap();
        assert_eq!(
            ui2.rect(pane_group(&rows2, Pane::SceneTree))
                .unwrap()
                .size
                .x,
            before + 50.0,
            "the dragged width survives the rebuild"
        );
    }

    #[test]
    fn the_toolbar_records_mode_buttons_and_highlights_the_active_one() {
        let (scene, handle) = demo_scene();
        let dir = std::env::temp_dir();
        let (mut ui, rows) = build_editor_ui_test(
            &scene,
            None,
            handle,
            &dir,
            (),
            None,
            GizmoMode::Rotate,
            None,
            &TreeView::default(),
            &InspectorView::default(),
            &EditorTheme::default(),
        );
        ui.layout(Vec2::new(1200.0, 800.0)).unwrap();

        // One button per mode, in order.
        let modes: Vec<GizmoMode> = rows.mode_buttons.iter().map(|(_, m)| *m).collect();
        assert_eq!(modes, GizmoMode::ALL.to_vec());
        // The active mode's button carries the highlight background; the others
        // use the plain panel background.
        for (id, m) in &rows.mode_buttons {
            let aurora::WidgetKind::Button(_) = ui.kind(*id) else {
                panic!("mode entries are buttons");
            };
            let theme = EditorTheme::default();
            let bg = ui.background(*id);
            if *m == GizmoMode::Rotate {
                assert_eq!(bg, theme.mode_active, "the active mode is highlighted");
            } else {
                assert_eq!(
                    bg, theme.aurora.panel_bg,
                    "inactive modes blend into the toolbar"
                );
            }
        }
    }

    #[test]
    fn a_context_menu_builds_a_popup_of_action_items() {
        let (scene, handle) = demo_scene();
        let dir = std::env::temp_dir();
        let menu = ContextMenu {
            anchor: Vec2::new(300.0, 200.0),
            items: vec![
                ("Duplicate".to_string(), MenuAction::DuplicateSelection),
                ("Delete".to_string(), MenuAction::DeleteSelection),
            ],
        };
        let (mut ui, rows) = build_editor_ui_test(
            &scene,
            None,
            handle,
            &dir,
            (),
            Some(&menu),
            GizmoMode::Select,
            None,
            &TreeView::default(),
            &InspectorView::default(),
            &EditorTheme::default(),
        );
        ui.layout(Vec2::new(1200.0, 800.0)).unwrap();

        // Two item rows were recorded, and the menu popup sits at the anchor.
        assert_eq!(rows.menu_items.len(), 2);
        let popup = rows.menu_popup.unwrap();
        assert_eq!(ui.rect(popup).unwrap().pos, Vec2::new(300.0, 200.0));
        // A click on the first item lands on the popup (drawn above the tree),
        // not on whatever panel is behind it.
        let item = rows.menu_items[0].0;
        assert!(ui.is_within(item, popup));
        let item_rect = ui.rect(item).unwrap();
        assert_eq!(
            ui.hit_test(item_rect.pos + item_rect.size * 0.5),
            Some(item)
        );
    }

    #[test]
    fn a_selected_sprite_shows_a_color_swatch_and_the_picker_builds() {
        let (scene, handle) = demo_scene();
        let node = scene.children(scene.root())[0];
        let dir = std::env::temp_dir();
        let (_ui, rows) = build_editor_ui_test(
            &scene,
            Some(node),
            handle,
            &dir,
            (),
            None,
            GizmoMode::Select,
            None,
            &TreeView::default(),
            &InspectorView::default(),
            &EditorTheme::default(),
        );
        // The sprite's tint is a Color field, so the inspector has one swatch.
        assert_eq!(rows.color_swatches.len(), 1);
        assert_eq!(rows.color_swatches[0].1.field, "tint");
    }

    #[test]
    fn scroll_positions_survive_a_shell_rebuild() {
        // Same class of bug as the panel sizes: scroll offsets live in the
        // widget tree, so a rebuild must capture and restore them.
        let (mut scene, handle) = demo_scene();
        // Enough nodes that the tree panel genuinely overflows and can scroll.
        let root_node = scene.root();
        for i in 0..60 {
            scene.add_child(root_node, helios::Node::new(format!("n{i}")));
        }
        let dir = std::env::temp_dir();
        let dock = DockNode::default_layout();

        let (mut ui, rows) = build_with_dock(&scene, handle, &dir, &dock, &HashMap::new());
        ui.layout(Vec2::new(1200.0, 800.0)).unwrap();

        // Wheel-scroll the tree pane down. Its scroll panel is inside the group.
        let (_, scroll) = *rows
            .pane_scrolls
            .iter()
            .find(|(p, _)| *p == Pane::SceneTree)
            .unwrap();
        let inside = ui.rect(scroll).unwrap().pos + Vec2::new(20.0, 20.0);
        ui.handle_input(aurora::InputEvent::PointerMoved(inside));
        ui.handle_input(aurora::InputEvent::Scroll(90.0));
        ui.layout(Vec2::new(1200.0, 800.0)).unwrap();
        assert_eq!(ui.scroll_offset(scroll), 90.0);

        // Rebuild with captured state: the offset carries over.
        let captured = capture_dock_scrolls(&ui, &rows);
        assert_eq!(captured.get(&Pane::SceneTree), Some(&90.0));
        let (mut ui2, rows2) = build_with_dock(&scene, handle, &dir, &dock, &captured);
        ui2.layout(Vec2::new(1200.0, 800.0)).unwrap();
        let (_, scroll2) = *rows2
            .pane_scrolls
            .iter()
            .find(|(p, _)| *p == Pane::SceneTree)
            .unwrap();
        assert_eq!(ui2.scroll_offset(scroll2), 90.0);
    }

    #[test]
    fn a_panel_drag_reflows_the_viewport_widget() {
        // The viewport shares the row with the panels, so growing a panel must
        // narrow the viewport's rect by the same amount - that rect is what
        // the app sizes the scene render target to each frame (1:1 pixels, no
        // stretching), so it has to track panel drags.
        let (scene, handle) = demo_scene();
        let dir = std::env::temp_dir();

        let dock = DockNode::default_layout();
        let (mut ui, rows) = build_with_dock(&scene, handle, &dir, &dock, &HashMap::new());
        ui.layout(Vec2::new(1200.0, 800.0)).unwrap();
        let viewport = rows.viewport.unwrap();
        let before = ui.rect(viewport).unwrap().size;

        // Drag the tree splitter 60px right.
        let bar = Vec2::new(
            ui.rect(pane_group(&rows, Pane::SceneTree)).unwrap().max().x + 2.0,
            100.0,
        );
        ui.handle_input(aurora::InputEvent::PointerMoved(bar));
        ui.handle_input(aurora::InputEvent::PointerPressed);
        ui.handle_input(aurora::InputEvent::PointerMoved(bar + Vec2::new(60.0, 0.0)));
        ui.handle_input(aurora::InputEvent::PointerReleased);
        ui.layout(Vec2::new(1200.0, 800.0)).unwrap();

        let after = ui.rect(viewport).unwrap().size;
        assert_eq!(after.x, before.x - 60.0, "the viewport narrows by the drag");
        assert_eq!(after.y, before.y, "its height is untouched");
    }

    #[test]
    fn parse_value_accepts_typed_text_and_rejects_junk() {
        assert_eq!(parse_value("3.5", &Value::F32(0.0)), Some(Value::F32(3.5)));
        assert_eq!(parse_value("junk", &Value::F32(0.0)), None);
        assert_eq!(
            parse_value("1, 2", &Value::Vec2(Vec2::ZERO)),
            Some(Value::Vec2(Vec2::new(1.0, 2.0)))
        );
        assert_eq!(parse_value("1", &Value::Vec2(Vec2::ZERO)), None);
        assert_eq!(
            parse_value("0.1, 0.2, 0.3, 1", &Value::Color([0.0; 4])),
            Some(Value::Color([0.1, 0.2, 0.3, 1.0]))
        );
        assert_eq!(
            parse_value("hero.png", &Value::Asset(String::new())),
            Some(Value::Asset("hero.png".into()))
        );
    }

    #[test]
    fn value_to_text_round_trips_through_parse_value() {
        // What the inspector shows for a value must parse back to that same
        // value, so an untouched field commits unchanged (float formatting is
        // shortest-round-trip, so the numeric cases are exact).
        let cases = [
            Value::F32(3.5),
            Value::Bool(true),
            Value::Str("hello".into()),
            Value::Vec2(Vec2::new(1.5, -2.0)),
            Value::Color([0.1, 0.2, 0.3, 1.0]),
            Value::Asset("art/hero.png".into()),
        ];
        for v in &cases {
            let text = value_to_text(v);
            assert_eq!(
                parse_value(&text, v).as_ref(),
                Some(v),
                "round-trip of {v:?} via {text:?}"
            );
        }
    }

    #[test]
    fn field_color_reads_back_what_set_field_color_wrote() {
        let mut scene = Scene::new("root");
        let root = scene.root();
        let mut node = Node::new("sprite");
        node.components.push(Component::Sprite(SpriteComponent {
            texture: String::new(),
            tint: [1.0, 1.0, 1.0, 1.0],
            size: Vec2::splat(10.0),
        }));
        let id = scene.add_child(root, node);

        let tint = ColorTarget {
            node: id,
            index: 0,
            field: "tint".to_string(),
        };
        set_field_color(&mut scene, &tint, [0.2, 0.4, 0.6, 0.8]);
        assert_eq!(field_color(&scene, &tint), [0.2, 0.4, 0.6, 0.8]);

        // A non-color field falls back to opaque black rather than panicking.
        let size = ColorTarget {
            node: id,
            index: 0,
            field: "size".to_string(),
        };
        assert_eq!(field_color(&scene, &size), [0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn grid_columns_fit_the_width_and_clamp() {
        // At least one column at any width, clamped to the file grid's max of 8.
        assert_eq!(grid_cols(0.0), 1);
        assert_eq!(grid_cols(10_000.0), 8);
        // 6 cells span 6*84 + 5*6 = 534 inside the 6px-per-side padding.
        assert_eq!(grid_cols(534.0 + 12.0), 6);

        // The general helper: cells of `cell` with `gap` between, `pad` per side.
        assert_eq!(columns_for(100.0, 20.0, 0.0, 0.0, 8), 5);
        assert_eq!(columns_for(0.0, 20.0, 5.0, 0.0, 8), 1);
        assert_eq!(columns_for(10_000.0, 20.0, 0.0, 0.0, 3), 3);
    }

    #[test]
    fn a_component_header_offers_a_way_to_take_it_off() {
        // The transform is not a component and has nothing to remove; a Sprite
        // and a Script each get an "x".
        let mut scene = Scene::new("root");
        let node = scene.add_child(scene.root(), Node::new("Thing"));
        scene
            .node_mut(node)
            .components
            .push(Component::Sprite(SpriteComponent::default()));
        scene
            .node_mut(node)
            .components
            .push(Component::Script(helios::ScriptComponent::default()));

        let rows = laid_out_inspector(&scene, node);
        let removed: Vec<usize> = rows
            .component_removes
            .iter()
            .map(|(_, _, index)| *index)
            .collect();
        assert_eq!(removed, vec![0, 1], "one per component, in order");
    }

    #[test]
    fn a_scripts_exported_values_are_shown_as_their_own_group() {
        // `source` and `speed` are different kinds of thing - one is which file
        // runs, the other is what it was tuned to - so they do not read as one
        // undifferentiated list.
        let mut scene = Scene::new("root");
        let node = scene.add_child(scene.root(), Node::new("Thing"));
        scene
            .node_mut(node)
            .components
            .push(Component::Script(helios::ScriptComponent {
                source: "s.cmt".into(),
                exports: vec![("speed".into(), Value::F32(1.0))],
                hints: Vec::new(),
            }));
        let rows = laid_out_inspector(&scene, node);
        assert!(
            rows.field_rows.iter().any(|(_, _, _, f)| f == "speed"),
            "the exported value has a row of its own"
        );
        assert!(
            rows.asset_fields.iter().any(|(_, _, _, f)| f == "source"),
            "and the path is still the asset field it was"
        );
    }

    #[test]
    fn a_ranged_export_gets_a_slider_that_can_actually_be_grabbed() {
        // A slider has no intrinsic height - it takes one from its style - and a
        // row that gives it none lays it out zero tall. It still draws, because
        // the thumb has a floor, so the miss is invisible: the widget is simply
        // never under the cursor and no press ever reaches it.
        let mut scene = Scene::new("root");
        let node = scene.add_child(scene.root(), Node::new("Thing"));
        scene
            .node_mut(node)
            .components
            .push(Component::Script(helios::ScriptComponent {
                source: "s.cmt".into(),
                exports: vec![("speed".into(), Value::F32(1.0))],
                hints: vec![("speed".into(), vec![comet::Hint::Range(0.0, 10.0)])],
            }));
        let (mut ui, rows) = laid_out_inspector_ui(&scene, node);
        let (slider, ..) = *rows
            .field_sliders
            .first()
            .expect("a ranged export is a slider");
        let rect = ui.rect(slider).expect("laid out");
        assert!(rect.size.y >= 14.0, "tall enough to hit: {rect:?}");

        // And grabbing it moves it: press in the middle of the track, drag right.
        let mid = Vec2::new(
            rect.pos.x + rect.size.x * 0.5,
            rect.pos.y + rect.size.y * 0.5,
        );
        ui.handle_input(InputEvent::PointerMoved(mid));
        ui.handle_input(InputEvent::PointerPressed);
        ui.handle_input(InputEvent::PointerMoved(Vec2::new(
            rect.pos.x + rect.size.x - 1.0,
            mid.y,
        )));
        assert!(
            ui.slider_value(slider) > 8.0,
            "dragged to the right end, got {}",
            ui.slider_value(slider)
        );
    }
}
