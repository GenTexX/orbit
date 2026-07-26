//! atlas editor UI: builds the docked shell (scene-tree, viewport, inspector,
//! file explorer) from the current scene, selection, and project directory
//! (M3 step 6 - "the editor looks like an editor").

use std::collections::HashSet;
use std::path::Path;

use aurora::{Color, ImageHandle, Orientation, Style, Theme, Ui, WidgetId};
use glam::Vec2;
use helios::{NodeId, Scene, Value};

use crate::color;
use crate::icons::{Icon, Icons};
use crate::viewport::GizmoMode;

/// Font size for a panel's title (larger than the default UI text).
const PANEL_TITLE: f32 = 17.0;
/// Corner radius for section cards, and for controls (buttons and fields).
const CARD_RADIUS: f32 = 6.0;
const CONTROL_RADIUS: f32 = 4.0;

/// The editor's full color palette: the aurora widget [`Theme`] plus atlas's own
/// surface, text, and accent colors. Swapped as a unit so the whole editor
/// recolors at once (see [`build_editor_ui`]). Loaded from the user's settings
/// file and editable there (see the `settings` module).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EditorTheme {
    /// The palette aurora's built-in widgets draw with.
    pub aurora: Theme,
    /// Docked-panel background, and the darker window/root background behind it.
    pub panel_bg: Color,
    pub root_bg: Color,
    /// Primary and secondary text.
    pub heading: Color,
    pub subhead: Color,
    /// Selected tree-row fill, and the reparent drop-line/into highlight.
    pub row_selected: Color,
    pub row_drop: Color,
    /// Menu / popup surface.
    pub menu_bg: Color,
    /// Inspector card fill and its 1px outline.
    pub card_bg: Color,
    pub card_border: Color,
    /// The active gizmo-mode toolbar button.
    pub mode_active: Color,
    /// Engine axis palette (X red, Y green) for the inspector's Vec2 labels.
    pub axis_x: Color,
    pub axis_y: Color,
}

impl EditorTheme {
    /// The default dark editor theme.
    pub fn dark() -> Self {
        Self {
            aurora: Theme::dark(),
            panel_bg: Color::rgb(0.13, 0.14, 0.18),
            root_bg: Color::rgb(0.08, 0.08, 0.10),
            heading: Color::rgb(0.96, 0.97, 1.0),
            subhead: Color::rgb(0.55, 0.60, 0.72),
            row_selected: Color::rgb(0.20, 0.28, 0.42),
            row_drop: Color::rgb(0.18, 0.40, 0.30),
            menu_bg: Color::rgb(0.16, 0.17, 0.22),
            card_bg: Color::rgb(0.17, 0.18, 0.23),
            card_border: Color::rgb(0.24, 0.26, 0.32),
            mode_active: Color::rgb(0.24, 0.40, 0.62),
            axis_x: Color::rgb(0.90, 0.32, 0.32),
            axis_y: Color::rgb(0.35, 0.70, 0.38),
        }
    }

    /// A bundled light editor theme - a preset a user can copy into their
    /// settings file (the editor loads whatever theme that file holds).
    #[allow(dead_code)]
    pub fn light() -> Self {
        Self {
            aurora: Theme::light(),
            panel_bg: Color::rgb(0.90, 0.91, 0.94),
            root_bg: Color::rgb(0.82, 0.83, 0.86),
            heading: Color::rgb(0.10, 0.12, 0.16),
            subhead: Color::rgb(0.38, 0.42, 0.50),
            row_selected: Color::rgb(0.72, 0.80, 0.94),
            row_drop: Color::rgb(0.55, 0.82, 0.66),
            menu_bg: Color::rgb(0.95, 0.96, 0.98),
            card_bg: Color::rgb(0.85, 0.86, 0.90),
            card_border: Color::rgb(0.72, 0.74, 0.80),
            mode_active: Color::rgb(0.62, 0.76, 0.96),
            axis_x: Color::rgb(0.85, 0.30, 0.30),
            axis_y: Color::rgb(0.28, 0.62, 0.34),
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
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FieldRef {
    /// The node's transform translation.
    Position(NodeId),
    /// The node's transform scale.
    Scale(NodeId),
    /// A `Vec2` field of one of the node's components.
    Component {
        node: NodeId,
        index: usize,
        field: &'static str,
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
pub fn field_vec2(scene: &Scene, r: FieldRef) -> Vec2 {
    match r {
        FieldRef::Position(n) => scene.node(n).transform.translation,
        FieldRef::Scale(n) => scene.node(n).transform.scale,
        FieldRef::Component { node, index, field } => {
            match scene.node(node).components[index].as_reflect().get(field) {
                Some(Value::Vec2(v)) => v,
                _ => Vec2::ZERO,
            }
        }
    }
}

/// Write a `Vec2` field directly (no undo step) - used for live drag-scrub;
/// the caller commits the final value through the history on release.
pub fn set_field_vec2(scene: &mut Scene, r: FieldRef, v: Vec2) {
    match r {
        FieldRef::Position(n) => scene.node_mut(n).transform.translation = v,
        FieldRef::Scale(n) => scene.node_mut(n).transform.scale = v,
        FieldRef::Component { node, index, field } => {
            scene.node_mut(node).components[index]
                .as_reflect_mut()
                .set(field, Value::Vec2(v));
        }
    }
}

/// A `Color`-valued component field the color picker edits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorTarget {
    pub node: NodeId,
    pub index: usize,
    pub field: &'static str,
}

/// The current value of a color field (opaque black if it is not a `Color`).
pub fn field_color(scene: &Scene, t: ColorTarget) -> [f32; 4] {
    match scene.node(t.node).components[t.index]
        .as_reflect()
        .get(t.field)
    {
        Some(Value::Color(c)) => c,
        _ => [0.0, 0.0, 0.0, 1.0],
    }
}

/// Write a color field directly (no undo step) - the picker commits the final
/// value through the history when it closes.
pub fn set_field_color(scene: &mut Scene, t: ColorTarget, rgba: [f32; 4]) {
    scene.node_mut(t.node).components[t.index]
        .as_reflect_mut()
        .set(t.field, Value::Color(rgba));
}

/// What the app hands [`build_color_picker`] to draw the open picker: where it
/// floats, the current color (for the hex field and swatch), and the handles
/// of its three gradient images (which the app keeps updated).
pub struct ColorPickerView {
    pub anchor: Vec2,
    pub rgba: [f32; 4],
    pub sv: ImageHandle,
    pub hue: ImageHandle,
    pub alpha: ImageHandle,
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
    /// A clicked eye icon toggles this node's visibility.
    pub eye_toggles: Vec<(WidgetId, NodeId)>,
    /// A clicked inspector section header collapses/expands that section.
    pub section_toggles: Vec<(WidgetId, InspectorSection)>,
    /// A submitted field row commits its current text to
    /// `(node, component index, field name)`.
    pub field_rows: Vec<(WidgetId, NodeId, usize, &'static str)>,
    /// A submitted transform row commits to the node's Transform; the field is
    /// `"position"`, `"rotation"`, or `"scale"`.
    pub transform_rows: Vec<(WidgetId, NodeId, &'static str)>,
    /// The three resizable panels, for reading their current (possibly
    /// splitter-dragged) sizes back before a rebuild.
    pub tree_panel: Option<WidgetId>,
    pub inspector_panel: Option<WidgetId>,
    pub files_panel: Option<WidgetId>,
    /// The viewport image widget; the app sizes the scene render target to
    /// this widget's laid-out rect each frame, so the scene draws 1:1 instead
    /// of stretching a window-sized texture into a smaller panel.
    pub viewport: Option<WidgetId>,
    /// PNG rows in the file explorer, draggable into the viewport to spawn a
    /// sprite: `(row widget, project-relative path)`.
    pub file_rows: Vec<(WidgetId, String)>,
    /// The toolbar actions.
    pub add_sprite: Option<WidgetId>,
    pub save: Option<WidgetId>,
    pub load: Option<WidgetId>,
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
    pub status_zoom: Option<WidgetId>,
    pub status_selected: Option<WidgetId>,
    pub status_fps: Option<WidgetId>,
    /// The open context-menu popup (for dismiss-on-click-outside), and its
    /// clickable item rows mapped to the action each performs.
    pub menu_popup: Option<WidgetId>,
    pub menu_items: Vec<(WidgetId, MenuAction)>,
    /// Inspector color swatches; clicking one opens the picker for that field.
    pub color_swatches: Vec<(WidgetId, ColorTarget)>,
    /// The open color picker: its popup (for dismiss), the three draggable
    /// gradient regions, and the hex input.
    pub picker_popup: Option<WidgetId>,
    pub picker_sv: Option<WidgetId>,
    pub picker_hue: Option<WidgetId>,
    pub picker_alpha: Option<WidgetId>,
    pub picker_hex: Option<WidgetId>,
}

/// An action offered by a right-click context menu (built on Aurora popups).
#[derive(Debug, Clone, Copy)]
pub enum MenuAction {
    /// Delete this node (and, since delete detaches, its subtree with it).
    DeleteNode(NodeId),
    /// Add a childless copy of this node beside it.
    DuplicateNode(NodeId),
    /// Spawn a new sprite at this world position (viewport menu).
    AddSpriteAt(Vec2),
}

/// An open right-click context menu: where it is anchored and what it offers.
#[derive(Debug, Clone)]
pub struct ContextMenu {
    pub anchor: Vec2,
    pub items: Vec<(String, MenuAction)>,
}

/// The dock's panel sizes. Splitter drags mutate the live widget tree, so the
/// app captures them here before each shell rebuild ([`capture_panel_sizes`])
/// and feeds them back in - otherwise a rebuild (selection change, edit)
/// would silently snap every panel back to its default.
#[derive(Debug, Clone, Copy)]
pub struct PanelSizes {
    pub tree_width: f32,
    pub inspector_width: f32,
    pub files_height: f32,
    /// Scroll offsets, preserved across rebuilds like the sizes (same lesson:
    /// widget-tree state dies with the tree unless captured and fed back).
    pub tree_scroll: f32,
    pub inspector_scroll: f32,
    pub files_scroll: f32,
}

impl Default for PanelSizes {
    fn default() -> Self {
        Self {
            tree_width: 220.0,
            inspector_width: 260.0,
            files_height: 140.0,
            tree_scroll: 0.0,
            inspector_scroll: 0.0,
            files_scroll: 0.0,
        }
    }
}

/// Read the current panel sizes out of a laid-out `Ui`, falling back to
/// `prior` for any panel that has no rect yet (e.g. before the first layout).
pub fn capture_panel_sizes(ui: &Ui, rows: &EditorRows, prior: PanelSizes) -> PanelSizes {
    let size_of = |panel: Option<WidgetId>| panel.and_then(|id| ui.rect(id)).map(|r| r.size);
    let scroll_of = |panel: Option<WidgetId>| panel.map(|id| ui.scroll_offset(id));
    PanelSizes {
        tree_width: size_of(rows.tree_panel).map_or(prior.tree_width, |s| s.x),
        inspector_width: size_of(rows.inspector_panel).map_or(prior.inspector_width, |s| s.x),
        files_height: size_of(rows.files_panel).map_or(prior.files_height, |s| s.y),
        tree_scroll: scroll_of(rows.tree_panel).unwrap_or(prior.tree_scroll),
        inspector_scroll: scroll_of(rows.inspector_panel).unwrap_or(prior.inspector_scroll),
        files_scroll: scroll_of(rows.files_panel).unwrap_or(prior.files_scroll),
    }
}

/// Build the whole editor shell for one frame: docked, resizable panels
/// (scene-tree, viewport, inspector, file explorer) around the viewport.
/// Rebuild whenever the scene or selection changes; the tree is cheap enough
/// that a fresh rebuild beats diffing (M2's inspector already proved this
/// pattern's frame cost is negligible). `sizes` carries the panel sizes across
/// rebuilds so splitter drags persist.
// The editor view is described by several small pieces of state; a params
// struct would not read more clearly than these named arguments.
#[allow(clippy::too_many_arguments)]
pub fn build_editor_ui(
    scene: &Scene,
    selected: Option<NodeId>,
    viewport_handle: ImageHandle,
    project_dir: &Path,
    sizes: PanelSizes,
    menu: Option<&ContextMenu>,
    gizmo_mode: GizmoMode,
    icons: Option<&Icons>,
    tree: &TreeView,
    inspector: &InspectorView,
    theme: &EditorTheme,
) -> (Ui, EditorRows) {
    let mut ui = Ui::new();
    ui.set_theme(theme.aurora);
    let mut rows = EditorRows::default();

    // The three-way dock fills the window (no full-width strip above it - the
    // toolbar sits over the viewport, in the center column).
    let root = ui.root_panel(Style::new().fill().column().background(theme.root_bg));
    // `main` clips (Overflow::Hidden): this is what lets the scroll panels
    // inside it stay window-sized instead of ballooning the row to their tall
    // content - see aurora's nested-row scroll test.
    let main = ui.panel(root, Style::new().row().grow(1.0).clip());

    let tree_panel = build_scene_tree(
        &mut ui,
        main,
        scene,
        selected,
        sizes.tree_width,
        tree,
        icons,
        theme,
        &mut rows,
    );
    let splitter_1 = ui.splitter(main, Orientation::Vertical, 1.0, Style::new().width(4.0));
    ui.set_splitter_target(splitter_1, tree_panel);

    // Center: the toolbar directly above the viewport, then a resizable
    // file-explorer strip below (Godot's viewport-toolbar layout).
    let center = ui.panel(main, Style::new().column().grow(1.0));
    build_toolbar(&mut ui, center, gizmo_mode, icons, theme, &mut rows);
    // A drop target so a PNG dragged from the file explorer can land here.
    let viewport = ui.image(
        center,
        viewport_handle,
        Style::new().grow(1.0).drop_target(),
    );
    let splitter_2 = ui.splitter(
        center,
        Orientation::Horizontal,
        -1.0,
        Style::new().height(4.0),
    );
    let files_panel = build_file_explorer(
        &mut ui,
        center,
        project_dir,
        sizes.files_height,
        theme,
        &mut rows,
    );
    ui.set_splitter_target(splitter_2, files_panel);

    let splitter_3 = ui.splitter(main, Orientation::Vertical, -1.0, Style::new().width(4.0));
    let inspector_panel = build_inspector(
        &mut ui,
        main,
        scene,
        selected,
        sizes.inspector_width,
        icons,
        inspector,
        theme,
        &mut rows,
    );
    ui.set_splitter_target(splitter_3, inspector_panel);

    build_status_bar(&mut ui, root, theme, &mut rows);

    // A right-click context menu floats above everything on Aurora's popup
    // layer, so it is built last and dismissed by the app on a click outside.
    if let Some(menu) = menu {
        build_context_menu(&mut ui, menu, theme, &mut rows);
    }

    // Restore the scroll positions captured from the previous shell.
    ui.set_scroll_offset(tree_panel, sizes.tree_scroll);
    ui.set_scroll_offset(inspector_panel, sizes.inspector_scroll);
    ui.set_scroll_offset(files_panel, sizes.files_scroll);

    rows.tree_panel = Some(tree_panel);
    rows.inspector_panel = Some(inspector_panel);
    rows.files_panel = Some(files_panel);
    rows.viewport = Some(viewport);
    (ui, rows)
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
            .background(theme.panel_bg),
    );
    let readout =
        |ui: &mut Ui, text: &str| ui.label(bar, text, Style::new().foreground(theme.subhead));
    rows.status_cursor = Some(readout(ui, "x -, y -"));
    rows.status_zoom = Some(readout(ui, "zoom 100%"));
    rows.status_selected = Some(readout(ui, "nothing selected"));
    rows.status_fps = Some(readout(ui, "- fps"));
}

/// The toolbar directly above the viewport: the editor's actions on the left,
/// the gizmo-mode switches on the right (the active mode highlighted). Each
/// button shows an icon when `icons` is available (falling back to its text
/// caption, e.g. in headless tests). A natural home for more tool switches.
fn build_toolbar(
    ui: &mut Ui,
    parent: WidgetId,
    mode: GizmoMode,
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
            .background(theme.panel_bg),
    );
    let icon = |id: Icon| icons.map(|i| i.get(id));
    rows.add_sprite = Some(toolbar_button(
        ui,
        bar,
        "Add Sprite",
        icon(Icon::Add),
        theme.panel_bg,
        theme,
    ));
    rows.save = Some(toolbar_button(
        ui,
        bar,
        "Save",
        icon(Icon::Save),
        theme.panel_bg,
        theme,
    ));
    rows.load = Some(toolbar_button(
        ui,
        bar,
        "Load",
        icon(Icon::Load),
        theme.panel_bg,
        theme,
    ));

    // A spacer pushes the mode switches to the right edge of the bar.
    ui.panel(bar, Style::new().grow(1.0));
    for m in GizmoMode::ALL {
        let background = if m == mode {
            theme.mode_active
        } else {
            theme.panel_bg
        };
        let button = toolbar_button(ui, bar, m.label(), icon(mode_icon(m)), background, theme);
        rows.mode_buttons.push((button, m));
    }
}

/// A toolbar button: an icon inside a fixed square when an icon is given, else
/// a text-captioned button (the headless-test / no-icon fallback).
fn toolbar_button(
    ui: &mut Ui,
    bar: WidgetId,
    caption: &str,
    icon: Option<ImageHandle>,
    background: Color,
    theme: &EditorTheme,
) -> WidgetId {
    match icon {
        Some(handle) => {
            let button = ui.button(
                bar,
                "",
                Style::new()
                    .padding(5.0)
                    .background(background)
                    .corner_radius(CONTROL_RADIUS),
            );
            ui.image(button, handle, Style::new().size(18.0, 18.0));
            button
        }
        None => ui.button(
            bar,
            caption,
            Style::new()
                .padding(6.0)
                .background(background)
                .foreground(theme.heading)
                .corner_radius(CONTROL_RADIUS),
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
            .background(theme.menu_bg)
            .corner_radius(CARD_RADIUS)
            .border(1.0, theme.card_border)
            .clip(),
    );
    for (label, action) in &menu.items {
        let item = ui.button(
            popup,
            label.clone(),
            Style::new()
                .width(150.0)
                .padding(6.0)
                .background(theme.menu_bg)
                .foreground(theme.heading)
                .corner_radius(CONTROL_RADIUS),
        );
        rows.menu_items.push((item, *action));
    }
    rows.menu_popup = Some(popup);
}

/// The size of the color picker's SV square, and its bar dimensions (px).
const PICKER_SV: f32 = 150.0;
const PICKER_HUE_W: f32 = 18.0;
const PICKER_ALPHA_H: f32 = 16.0;

/// Build the open color picker on Aurora's popup layer: an SV square + hue bar
/// (as gradient images the app keeps updated), an alpha bar, and a hex field
/// with a preview swatch. The regions are recorded for the app's drag routing.
pub fn build_color_picker(
    ui: &mut Ui,
    view: &ColorPickerView,
    theme: &EditorTheme,
    rows: &mut EditorRows,
) {
    let popup = ui.popup(
        view.anchor,
        Style::new()
            .column()
            .gap(6.0)
            .padding(8.0)
            .background(theme.menu_bg)
            .corner_radius(CARD_RADIUS)
            .border(1.0, theme.card_border)
            .clip(),
    );
    let top = ui.panel(popup, Style::new().row().gap(6.0));
    let sv = ui.image(top, view.sv, Style::new().size(PICKER_SV, PICKER_SV));
    let hue = ui.image(top, view.hue, Style::new().size(PICKER_HUE_W, PICKER_SV));
    let alpha = ui.image(
        popup,
        view.alpha,
        Style::new().size(PICKER_SV, PICKER_ALPHA_H),
    );

    let bottom = ui.panel(popup, Style::new().row().gap(6.0).align_center());
    let hex = ui.text_input(
        bottom,
        color::to_hex(view.rgba),
        Style::new()
            .grow(1.0)
            .padding(4.0)
            .corner_radius(CONTROL_RADIUS)
            .placeholder("#RRGGBBAA"),
    );
    let [r, g, b, a] = view.rgba;
    ui.panel(
        bottom,
        Style::new()
            .size(28.0, 24.0)
            .background(Color::rgba(r, g, b, a))
            .corner_radius(CONTROL_RADIUS),
    );

    rows.picker_popup = Some(popup);
    rows.picker_sv = Some(sv);
    rows.picker_hue = Some(hue);
    rows.picker_alpha = Some(alpha);
    rows.picker_hex = Some(hex);
}

/// The docked scene-tree panel: one clickable row per node, indented by depth.
#[allow(clippy::too_many_arguments)]
fn build_scene_tree(
    ui: &mut Ui,
    parent: WidgetId,
    scene: &Scene,
    selected: Option<NodeId>,
    width: f32,
    tree: &TreeView,
    icons: Option<&Icons>,
    theme: &EditorTheme,
    rows: &mut EditorRows,
) -> WidgetId {
    let panel = ui.panel(
        parent,
        Style::new()
            .column()
            .width(width)
            .padding(10.0)
            .gap(TREE_ROW_GAP)
            .scroll()
            .background(theme.panel_bg),
    );
    ui.label(
        panel,
        "Scene",
        Style::new()
            .foreground(theme.heading)
            .font_size(PANEL_TITLE),
    );
    add_tree_row(
        ui,
        panel,
        scene,
        scene.root(),
        0,
        selected,
        tree,
        icons,
        theme,
        rows,
    );
    panel
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

#[allow(clippy::too_many_arguments)]
fn add_tree_row(
    ui: &mut Ui,
    parent: WidgetId,
    scene: &Scene,
    node: NodeId,
    depth: u32,
    selected: Option<NodeId>,
    tree: &TreeView,
    icons: Option<&Icons>,
    theme: &EditorTheme,
    rows: &mut EditorRows,
) {
    // Selected wins the highlight; an Into-drop target tints green.
    let background = if selected == Some(node) {
        theme.row_selected
    } else if tree.drop_target == Some(DropSpot::Into(node)) {
        theme.row_drop
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
    let collapsed = tree.collapsed.contains(&node);
    if has_children {
        // A flat button holding the disclosure triangle; clicking it toggles
        // collapse. The chevron image is decoration (present when icons are).
        let toggle = ui.button(
            row,
            "",
            Style::new().flat().padding(1.0).width(TREE_DISCLOSURE),
        );
        if let Some(icons) = icons {
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
    // A hidden node's name is muted, so the tree reads its state at a glance.
    let visible = scene.node(node).visible;
    ui.label(
        row,
        scene.node(node).name.clone(),
        Style::new().grow(1.0).foreground(if visible {
            theme.heading
        } else {
            theme.subhead
        }),
    );
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
    if let Some(icons) = icons {
        let glyph = if visible { Icon::Eye } else { Icon::EyeOff };
        ui.image(eye, icons.get(glyph), Style::new().size(TREE_EYE, TREE_EYE));
    }
    rows.eye_toggles.push((eye, node));
    rows.tree_rows.push((row, node));

    if has_children && !collapsed {
        for &child in scene.children(node) {
            add_tree_row(
                ui,
                parent,
                scene,
                child,
                depth + 1,
                selected,
                tree,
                icons,
                theme,
                rows,
            );
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

/// The docked inspector panel: the selected node's transform and reflected
/// component fields as editable rows, grouped into collapsible section cards.
// A handful of small view inputs; named arguments read clearer than a struct.
#[allow(clippy::too_many_arguments)]
fn build_inspector(
    ui: &mut Ui,
    parent: WidgetId,
    scene: &Scene,
    selected: Option<NodeId>,
    width: f32,
    icons: Option<&Icons>,
    inspector: &InspectorView,
    theme: &EditorTheme,
    rows: &mut EditorRows,
) -> WidgetId {
    let panel = ui.panel(
        parent,
        Style::new()
            .column()
            .width(width)
            .padding(10.0)
            .gap(8.0)
            .scroll()
            .background(theme.panel_bg),
    );
    ui.label(
        panel,
        "Inspector",
        Style::new()
            .foreground(theme.heading)
            .font_size(PANEL_TITLE),
    );

    let Some(node) = selected else {
        ui.label(
            panel,
            "Nothing selected",
            Style::new().foreground(theme.subhead),
        );
        return panel;
    };
    ui.label(
        panel,
        scene.node(node).name.clone(),
        Style::new().foreground(theme.heading),
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
        // Rotation is a single scalar, edited in degrees (stored radians).
        let input = add_value_row(
            ui,
            card,
            "rotation",
            &Value::F32(t.rotation.to_degrees()),
            true,
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
        for &field in reflect.field_names() {
            let Some(value) = reflect.get(field) else {
                continue;
            };
            // A Vec2 field gets the two-axis treatment, a Color field a swatch
            // that opens the picker; everything else a plain (numeric where it
            // makes sense) row.
            match value {
                Value::Vec2(v) => {
                    let r = FieldRef::Component {
                        node,
                        index: i,
                        field,
                    };
                    add_vec2_field(ui, card, field, v, r, theme, rows);
                }
                Value::Color(c) => {
                    let row = ui.panel(card, Style::new().row().gap(6.0).align_center());
                    ui.label(
                        row,
                        field,
                        Style::new().width(70.0).foreground(theme.heading),
                    );
                    let swatch = ui.button(
                        row,
                        "",
                        Style::new()
                            .width(48.0)
                            .padding(6.0)
                            .background(Color::rgba(c[0], c[1], c[2], c[3]))
                            .corner_radius(CONTROL_RADIUS),
                    );
                    rows.color_swatches.push((
                        swatch,
                        ColorTarget {
                            node,
                            index: i,
                            field,
                        },
                    ));
                }
                _ => {
                    let numeric = matches!(value, Value::F32(_));
                    let input = add_value_row(ui, card, field, &value, numeric, theme);
                    rows.field_rows.push((input, node, i, field));
                }
            }
        }
    }
    panel
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
            .gap(6.0)
            .padding(8.0)
            .background(theme.card_bg)
            .corner_radius(CARD_RADIUS)
            .border(1.0, theme.card_border),
    );
    // One flat header row: the chevron is decoration inside it (not its own
    // button, so it has no separate hover); clicking anywhere on the header
    // toggles the section.
    let header = ui.button(card, "", Style::new().row().gap(6.0).align_center().flat());
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
        Style::new().grow(1.0).foreground(theme.heading),
    );
    rows.section_toggles.push((header, section));
    (!collapsed).then_some(card)
}

/// One labeled, editable inspector row; returns the text input's id. `numeric`
/// restricts typing to numbers.
fn add_value_row(
    ui: &mut Ui,
    panel: WidgetId,
    label: &str,
    value: &Value,
    numeric: bool,
    theme: &EditorTheme,
) -> WidgetId {
    let row = ui.panel(panel, Style::new().row().gap(6.0).align_center());
    ui.label(
        row,
        label,
        Style::new().width(70.0).foreground(theme.heading),
    );
    let style = Style::new()
        .grow(1.0)
        .padding(4.0)
        .foreground(theme.heading)
        .corner_radius(CONTROL_RADIUS);
    if numeric {
        ui.numeric_input(row, value_to_text(value), style)
    } else {
        ui.text_input(row, value_to_text(value), style)
    }
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
    ui.label(panel, name, Style::new().foreground(theme.subhead));
    for (axis, caption, color) in [(Axis::X, "X", theme.axis_x), (Axis::Y, "Y", theme.axis_y)] {
        let row = ui.panel(panel, Style::new().row().gap(4.0).align_center());
        let label = ui.button(
            row,
            caption,
            Style::new()
                .width(24.0)
                .padding(4.0)
                .background(color)
                .foreground(Color::WHITE)
                .text_center()
                .corner_radius(CONTROL_RADIUS),
        );
        rows.scrub_labels.push((label, field, axis));
        let input = ui.numeric_input(
            row,
            value_to_text(&Value::F32(axis_of(v, axis))),
            Style::new()
                .grow(1.0)
                .padding(4.0)
                .foreground(theme.heading)
                .corner_radius(CONTROL_RADIUS),
        );
        rows.vec_inputs.push((input, field, axis));
    }
}

/// The docked file explorer: the project's PNG and scene files.
fn build_file_explorer(
    ui: &mut Ui,
    parent: WidgetId,
    project_dir: &Path,
    height: f32,
    theme: &EditorTheme,
    rows: &mut EditorRows,
) -> WidgetId {
    let panel = ui.panel(
        parent,
        Style::new()
            .column()
            .height(height)
            .padding(10.0)
            .gap(4.0)
            .scroll()
            .background(theme.panel_bg),
    );
    ui.label(
        panel,
        "Files",
        Style::new()
            .foreground(theme.heading)
            .font_size(PANEL_TITLE),
    );
    for name in list_project_files(project_dir) {
        if name.ends_with(".png") {
            // A PNG row is a draggable button: drag it onto the viewport (a
            // drop target) to spawn a sprite there. Aurora runs the drag and
            // emits Event::Dropped; a plain click does nothing yet.
            let row = ui.button(
                panel,
                name.clone(),
                Style::new()
                    .padding(2.0)
                    .foreground(theme.heading)
                    .draggable(),
            );
            rows.file_rows.push((row, name));
        } else {
            ui.label(panel, name, Style::new().foreground(theme.subhead));
        }
    }
    panel
}

/// The project's `.png` and `.ron` files under `assets/` and `scenes/`, sorted.
fn list_project_files(dir: &Path) -> Vec<String> {
    let mut names = Vec::new();
    for sub in ["assets", "scenes"] {
        let Ok(entries) = std::fs::read_dir(dir.join(sub)) else {
            continue;
        };
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str()
                && (name.ends_with(".png") || name.ends_with(".ron"))
            {
                names.push(format!("{sub}/{name}"));
            }
        }
    }
    names.sort();
    names
}

/// Render a reflected [`Value`] as editable text.
fn value_to_text(value: &Value) -> String {
    match value {
        Value::F32(x) => x.to_string(),
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
            build_editor_ui(
                &scene,
                None,
                handle,
                &dir,
                PanelSizes::default(),
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
            let (mut ui, _) = build_editor_ui(
                &scene,
                None,
                handle,
                &dir,
                PanelSizes::default(),
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
        assert!(has_fill(&dark, EditorTheme::dark().root_bg));
        assert!(!has_fill(&dark, EditorTheme::light().root_bg));

        let light = build(&EditorTheme::light());
        assert!(has_fill(&light, EditorTheme::light().root_bg));
    }

    #[test]
    fn every_tree_row_has_an_eye_toggle() {
        let mut scene = Scene::new("Root");
        let root = scene.root();
        let a = scene.add_child(root, Node::new("A"));
        let mut reg: slotmap::SlotMap<ImageHandle, ()> = slotmap::SlotMap::with_key();
        let handle = reg.insert(());
        let dir = std::env::temp_dir();
        let (_, rows) = build_editor_ui(
            &scene,
            None,
            handle,
            &dir,
            PanelSizes::default(),
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
            build_editor_ui(
                &scene,
                Some(sprite),
                handle,
                &dir,
                PanelSizes::default(),
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
        let (mut ui, rows) = build_editor_ui(
            &scene,
            None,
            handle,
            &dir,
            PanelSizes::default(),
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
        let (_ui, rows) = build_editor_ui(
            &scene,
            Some(node),
            handle,
            &dir,
            PanelSizes::default(),
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
        assert_eq!(field_vec2(&scene, field), Vec2::new(10.0, 10.0));

        // Scrub the X component and read it back; Y is untouched.
        let moved = with_axis(field_vec2(&scene, field), Axis::X, 42.0);
        set_field_vec2(&mut scene, field, moved);
        assert_eq!(field_vec2(&scene, field), Vec2::new(42.0, 10.0));
        assert_eq!(axis_of(field_vec2(&scene, field), Axis::Y), 10.0);
    }

    #[test]
    fn panel_sizes_survive_a_shell_rebuild() {
        // The regression: drag a splitter, then trigger a rebuild (as a
        // selection change does) - the dragged size must carry over, not snap
        // back to the default.
        let (scene, handle) = demo_scene();
        let dir = std::env::temp_dir(); // no project files needed for this test

        let sizes = PanelSizes::default();
        let (mut ui, rows) = build_editor_ui(
            &scene,
            None,
            handle,
            &dir,
            sizes,
            None,
            GizmoMode::Select,
            None,
            &TreeView::default(),
            &InspectorView::default(),
            &EditorTheme::default(),
        );
        ui.layout(Vec2::new(1200.0, 800.0)).unwrap();

        // Drag the tree panel's splitter 50px right, as a user would.
        let tree = rows.tree_panel.unwrap();
        assert_eq!(ui.rect(tree).unwrap().size.x, sizes.tree_width);
        let bar = {
            // The splitter sits just right of the tree panel.
            let r = ui.rect(tree).unwrap();
            Vec2::new(r.max().x + 2.0, 100.0)
        };
        ui.handle_input(aurora::InputEvent::PointerMoved(bar));
        ui.handle_input(aurora::InputEvent::PointerPressed);
        ui.handle_input(aurora::InputEvent::PointerMoved(bar + Vec2::new(50.0, 0.0)));
        ui.handle_input(aurora::InputEvent::PointerReleased);
        ui.layout(Vec2::new(1200.0, 800.0)).unwrap();
        assert_eq!(ui.rect(tree).unwrap().size.x, sizes.tree_width + 50.0);

        // Rebuild the shell (what a tree-row click triggers) with captured sizes.
        let captured = capture_panel_sizes(&ui, &rows, sizes);
        assert_eq!(captured.tree_width, sizes.tree_width + 50.0);
        let (mut ui2, rows2) = build_editor_ui(
            &scene,
            None,
            handle,
            &dir,
            captured,
            None,
            GizmoMode::Select,
            None,
            &TreeView::default(),
            &InspectorView::default(),
            &EditorTheme::default(),
        );
        ui2.layout(Vec2::new(1200.0, 800.0)).unwrap();
        assert_eq!(
            ui2.rect(rows2.tree_panel.unwrap()).unwrap().size.x,
            sizes.tree_width + 50.0,
            "the dragged width survives the rebuild"
        );
    }

    #[test]
    fn the_toolbar_records_mode_buttons_and_highlights_the_active_one() {
        let (scene, handle) = demo_scene();
        let dir = std::env::temp_dir();
        let (mut ui, rows) = build_editor_ui(
            &scene,
            None,
            handle,
            &dir,
            PanelSizes::default(),
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
                assert_eq!(bg, theme.panel_bg, "inactive modes are not");
            }
        }
    }

    #[test]
    fn a_context_menu_builds_a_popup_of_action_items() {
        let (scene, handle) = demo_scene();
        let dir = std::env::temp_dir();
        let node = scene.children(scene.root())[0];
        let menu = ContextMenu {
            anchor: Vec2::new(300.0, 200.0),
            items: vec![
                ("Duplicate".to_string(), MenuAction::DuplicateNode(node)),
                ("Delete".to_string(), MenuAction::DeleteNode(node)),
            ],
        };
        let (mut ui, rows) = build_editor_ui(
            &scene,
            None,
            handle,
            &dir,
            PanelSizes::default(),
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
        let (_ui, rows) = build_editor_ui(
            &scene,
            Some(node),
            handle,
            &dir,
            PanelSizes::default(),
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

        // The picker popup builds its three gradient regions and hex input,
        // and hit-tests above the tree behind it.
        let mut ui = Ui::new();
        ui.root_panel(Style::new().fill());
        let mut rows = EditorRows::default();
        let view = ColorPickerView {
            anchor: Vec2::new(200.0, 150.0),
            rgba: [1.0, 0.5, 0.2, 1.0],
            sv: handle,
            hue: handle,
            alpha: handle,
        };
        build_color_picker(&mut ui, &view, &EditorTheme::default(), &mut rows);
        ui.layout(Vec2::new(1200.0, 800.0)).unwrap();

        let popup = rows.picker_popup.unwrap();
        assert!(ui.is_within(rows.picker_sv.unwrap(), popup));
        assert!(ui.is_within(rows.picker_hue.unwrap(), popup));
        assert!(ui.is_within(rows.picker_alpha.unwrap(), popup));
        assert!(ui.is_within(rows.picker_hex.unwrap(), popup));
        // A press on the SV square hits it (not something behind the popup).
        let sv = ui.rect(rows.picker_sv.unwrap()).unwrap();
        assert_eq!(ui.hit_test(sv.pos + sv.size * 0.5), rows.picker_sv);
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

        let sizes = PanelSizes::default();
        let (mut ui, rows) = build_editor_ui(
            &scene,
            None,
            handle,
            &dir,
            sizes,
            None,
            GizmoMode::Select,
            None,
            &TreeView::default(),
            &InspectorView::default(),
            &EditorTheme::default(),
        );
        ui.layout(Vec2::new(1200.0, 800.0)).unwrap();

        // Wheel-scroll the tree panel down.
        let tree = rows.tree_panel.unwrap();
        let inside = ui.rect(tree).unwrap().pos + Vec2::new(20.0, 60.0);
        ui.handle_input(aurora::InputEvent::PointerMoved(inside));
        ui.handle_input(aurora::InputEvent::Scroll(90.0));
        ui.layout(Vec2::new(1200.0, 800.0)).unwrap();
        assert_eq!(ui.scroll_offset(tree), 90.0);

        // Rebuild with captured state: the offset carries over.
        let captured = capture_panel_sizes(&ui, &rows, sizes);
        assert_eq!(captured.tree_scroll, 90.0);
        let (mut ui2, rows2) = build_editor_ui(
            &scene,
            None,
            handle,
            &dir,
            captured,
            None,
            GizmoMode::Select,
            None,
            &TreeView::default(),
            &InspectorView::default(),
            &EditorTheme::default(),
        );
        ui2.layout(Vec2::new(1200.0, 800.0)).unwrap();
        assert_eq!(ui2.scroll_offset(rows2.tree_panel.unwrap()), 90.0);
    }

    #[test]
    fn a_panel_drag_reflows_the_viewport_widget() {
        // The viewport shares the row with the panels, so growing a panel must
        // narrow the viewport's rect by the same amount - that rect is what
        // the app sizes the scene render target to each frame (1:1 pixels, no
        // stretching), so it has to track panel drags.
        let (scene, handle) = demo_scene();
        let dir = std::env::temp_dir();

        let (mut ui, rows) = build_editor_ui(
            &scene,
            None,
            handle,
            &dir,
            PanelSizes::default(),
            None,
            GizmoMode::Select,
            None,
            &TreeView::default(),
            &InspectorView::default(),
            &EditorTheme::default(),
        );
        ui.layout(Vec2::new(1200.0, 800.0)).unwrap();
        let viewport = rows.viewport.unwrap();
        let before = ui.rect(viewport).unwrap().size;

        // Drag the tree splitter 60px right.
        let bar = Vec2::new(
            ui.rect(rows.tree_panel.unwrap()).unwrap().max().x + 2.0,
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
}
