//! atlas editor UI: builds the docked shell (scene-tree, viewport, inspector,
//! file explorer) from the current scene, selection, and project directory
//! (M3 step 6 - "the editor looks like an editor").

use std::path::Path;

use aurora::{Color, ImageHandle, Orientation, Style, Ui, WidgetId};
use glam::Vec2;
use helios::{NodeId, Scene, Value};

const PANEL_BG: Color = Color::rgb(0.13, 0.14, 0.18);
const ROOT_BG: Color = Color::rgb(0.08, 0.08, 0.10);
const HEADING: Color = Color::rgb(0.96, 0.97, 1.0);
const SUBHEAD: Color = Color::rgb(0.55, 0.60, 0.72);
const ROW_SELECTED: Color = Color::rgb(0.20, 0.28, 0.42);

/// Maps widget handles back to the domain data they represent, so the app can
/// react to Aurora events without Aurora knowing about `Scene`/`NodeId`.
#[derive(Default)]
pub struct EditorRows {
    /// A clicked tree row selects this node.
    pub tree_rows: Vec<(WidgetId, NodeId)>,
    /// A submitted field row commits its current text to
    /// `(node, component index, field name)`.
    pub field_rows: Vec<(WidgetId, NodeId, usize, &'static str)>,
}

/// Build the whole editor shell for one frame: docked, resizable panels
/// (scene-tree, viewport, inspector, file explorer) around the viewport.
/// Rebuild whenever the scene or selection changes; the tree is cheap enough
/// that a fresh rebuild beats diffing (M2's inspector already proved this
/// pattern's frame cost is negligible).
pub fn build_editor_ui(
    scene: &Scene,
    selected: Option<NodeId>,
    viewport_handle: ImageHandle,
    project_dir: &Path,
) -> (Ui, EditorRows) {
    let mut ui = Ui::new();
    let mut rows = EditorRows::default();

    let root = ui.root_panel(Style::new().fill().row().background(ROOT_BG));

    let tree_panel = build_scene_tree(&mut ui, root, scene, selected, &mut rows);
    let splitter_1 = ui.splitter(root, Orientation::Vertical, 1.0, Style::new().width(4.0));
    ui.set_splitter_target(splitter_1, tree_panel);

    // Center: the viewport on top, a resizable file-explorer strip below.
    let center = ui.panel(root, Style::new().column().grow(1.0));
    ui.image(center, viewport_handle, Style::new().grow(1.0));
    let splitter_2 = ui.splitter(
        center,
        Orientation::Horizontal,
        -1.0,
        Style::new().height(4.0),
    );
    let files_panel = build_file_explorer(&mut ui, center, project_dir);
    ui.set_splitter_target(splitter_2, files_panel);

    let splitter_3 = ui.splitter(root, Orientation::Vertical, -1.0, Style::new().width(4.0));
    let inspector_panel = build_inspector(&mut ui, root, scene, selected, &mut rows);
    ui.set_splitter_target(splitter_3, inspector_panel);

    (ui, rows)
}

/// The docked scene-tree panel: one clickable row per node, indented by depth.
fn build_scene_tree(
    ui: &mut Ui,
    parent: WidgetId,
    scene: &Scene,
    selected: Option<NodeId>,
    rows: &mut EditorRows,
) -> WidgetId {
    let panel = ui.panel(
        parent,
        Style::new()
            .column()
            .width(220.0)
            .padding(10.0)
            .gap(4.0)
            .background(PANEL_BG),
    );
    ui.label(panel, "Scene", Style::new().foreground(HEADING));
    add_tree_row(ui, panel, scene, scene.root(), 0, selected, rows);
    panel
}

fn add_tree_row(
    ui: &mut Ui,
    parent: WidgetId,
    scene: &Scene,
    node: NodeId,
    depth: u32,
    selected: Option<NodeId>,
    rows: &mut EditorRows,
) {
    let row = ui.panel(parent, Style::new().row().gap(4.0));
    if depth > 0 {
        // A blank fixed-width spacer stands in for per-side padding, which
        // Style does not expose yet - the smallest thing that indents.
        ui.panel(row, Style::new().width(depth as f32 * 14.0));
    }
    let highlight = if selected == Some(node) {
        ROW_SELECTED
    } else {
        Color::TRANSPARENT
    };
    let button = ui.button(
        row,
        scene.node(node).name.clone(),
        Style::new()
            .grow(1.0)
            .padding(4.0)
            .background(highlight)
            .foreground(HEADING),
    );
    rows.tree_rows.push((button, node));

    for &child in scene.children(node) {
        add_tree_row(ui, parent, scene, child, depth + 1, selected, rows);
    }
}

/// The docked inspector panel: the selected node's reflected component fields
/// as editable rows (label + text input), one per field.
fn build_inspector(
    ui: &mut Ui,
    parent: WidgetId,
    scene: &Scene,
    selected: Option<NodeId>,
    rows: &mut EditorRows,
) -> WidgetId {
    let panel = ui.panel(
        parent,
        Style::new()
            .column()
            .width(260.0)
            .padding(10.0)
            .gap(6.0)
            .background(PANEL_BG),
    );
    ui.label(panel, "Inspector", Style::new().foreground(HEADING));

    let Some(node) = selected else {
        ui.label(panel, "Nothing selected", Style::new().foreground(SUBHEAD));
        return panel;
    };
    ui.label(
        panel,
        scene.node(node).name.clone(),
        Style::new().foreground(HEADING),
    );

    for (i, component) in scene.node(node).components.iter().enumerate() {
        let reflect = component.as_reflect();
        ui.label(panel, reflect.type_name(), Style::new().foreground(SUBHEAD));
        for &field in reflect.field_names() {
            let Some(value) = reflect.get(field) else {
                continue;
            };
            let field_row = ui.panel(panel, Style::new().row().gap(6.0).align_center());
            ui.label(field_row, field, Style::new().width(70.0));
            let input = ui.text_input(
                field_row,
                value_to_text(&value),
                Style::new().grow(1.0).padding(4.0),
            );
            rows.field_rows.push((input, node, i, field));
        }
    }
    panel
}

/// The docked file explorer: the project's PNG and scene files.
fn build_file_explorer(ui: &mut Ui, parent: WidgetId, project_dir: &Path) -> WidgetId {
    let panel = ui.panel(
        parent,
        Style::new()
            .column()
            .height(140.0)
            .padding(10.0)
            .gap(4.0)
            .background(PANEL_BG),
    );
    ui.label(panel, "Files", Style::new().foreground(HEADING));
    for name in list_project_files(project_dir) {
        ui.label(panel, name, Style::new().foreground(SUBHEAD));
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
