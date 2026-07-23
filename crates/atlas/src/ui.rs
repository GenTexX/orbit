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
    /// Status bar readouts, updated in place every frame via `set_label`.
    pub status_cursor: Option<WidgetId>,
    pub status_zoom: Option<WidgetId>,
    pub status_selected: Option<WidgetId>,
    pub status_fps: Option<WidgetId>,
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
pub fn build_editor_ui(
    scene: &Scene,
    selected: Option<NodeId>,
    viewport_handle: ImageHandle,
    project_dir: &Path,
    sizes: PanelSizes,
) -> (Ui, EditorRows) {
    let mut ui = Ui::new();
    let mut rows = EditorRows::default();

    // A toolbar strip across the top, then the three-way dock below it.
    let root = ui.root_panel(Style::new().fill().column().background(ROOT_BG));
    build_toolbar(&mut ui, root, &mut rows);
    // `main` clips (Overflow::Hidden): this is what lets the scroll panels
    // inside it stay window-sized instead of ballooning the row to their tall
    // content - see aurora's nested-row scroll test.
    let main = ui.panel(root, Style::new().row().grow(1.0).clip());

    let tree_panel = build_scene_tree(&mut ui, main, scene, selected, sizes.tree_width, &mut rows);
    let splitter_1 = ui.splitter(main, Orientation::Vertical, 1.0, Style::new().width(4.0));
    ui.set_splitter_target(splitter_1, tree_panel);

    // Center: the viewport on top, a resizable file-explorer strip below.
    let center = ui.panel(main, Style::new().column().grow(1.0));
    let viewport = ui.image(center, viewport_handle, Style::new().grow(1.0));
    let splitter_2 = ui.splitter(
        center,
        Orientation::Horizontal,
        -1.0,
        Style::new().height(4.0),
    );
    let files_panel =
        build_file_explorer(&mut ui, center, project_dir, sizes.files_height, &mut rows);
    ui.set_splitter_target(splitter_2, files_panel);

    let splitter_3 = ui.splitter(main, Orientation::Vertical, -1.0, Style::new().width(4.0));
    let inspector_panel = build_inspector(
        &mut ui,
        main,
        scene,
        selected,
        sizes.inspector_width,
        &mut rows,
    );
    ui.set_splitter_target(splitter_3, inspector_panel);

    build_status_bar(&mut ui, root, &mut rows);

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
fn build_status_bar(ui: &mut Ui, parent: WidgetId, rows: &mut EditorRows) {
    let bar = ui.panel(
        parent,
        Style::new()
            .row()
            .gap(18.0)
            .padding(4.0)
            .align_center()
            .background(PANEL_BG),
    );
    let readout = |ui: &mut Ui, text: &str| ui.label(bar, text, Style::new().foreground(SUBHEAD));
    rows.status_cursor = Some(readout(ui, "x -, y -"));
    rows.status_zoom = Some(readout(ui, "zoom 100%"));
    rows.status_selected = Some(readout(ui, "nothing selected"));
    rows.status_fps = Some(readout(ui, "- fps"));
}

/// The toolbar: the editor's actions, and later home to the gizmo-mode
/// switches from the ideatank.
fn build_toolbar(ui: &mut Ui, parent: WidgetId, rows: &mut EditorRows) {
    let bar = ui.panel(
        parent,
        Style::new()
            .row()
            .gap(6.0)
            .padding(6.0)
            .align_center()
            .background(PANEL_BG),
    );
    let button = |ui: &mut Ui, caption: &str| {
        ui.button(bar, caption, Style::new().padding(6.0).foreground(HEADING))
    };
    rows.add_sprite = Some(button(ui, "Add Sprite"));
    rows.save = Some(button(ui, "Save"));
    rows.load = Some(button(ui, "Load"));
}

/// The docked scene-tree panel: one clickable row per node, indented by depth.
fn build_scene_tree(
    ui: &mut Ui,
    parent: WidgetId,
    scene: &Scene,
    selected: Option<NodeId>,
    width: f32,
    rows: &mut EditorRows,
) -> WidgetId {
    let panel = ui.panel(
        parent,
        Style::new()
            .column()
            .width(width)
            .padding(10.0)
            .gap(4.0)
            .scroll()
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
    width: f32,
    rows: &mut EditorRows,
) -> WidgetId {
    let panel = ui.panel(
        parent,
        Style::new()
            .column()
            .width(width)
            .padding(10.0)
            .gap(6.0)
            .scroll()
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

    // The node's own transform first: it is what viewport drags edit, so its
    // rows double as a live readout while dragging.
    ui.label(panel, "Transform", Style::new().foreground(SUBHEAD));
    let t = scene.node(node).transform;
    let transform_fields = [
        ("position", Value::Vec2(t.translation)),
        // Stored radians, edited as degrees (the commit path converts back).
        ("rotation", Value::F32(t.rotation.to_degrees())),
        ("scale", Value::Vec2(t.scale)),
    ];
    for (field, value) in transform_fields {
        let input = add_value_row(ui, panel, field, &value);
        rows.transform_rows.push((input, node, field));
    }

    for (i, component) in scene.node(node).components.iter().enumerate() {
        let reflect = component.as_reflect();
        ui.label(panel, reflect.type_name(), Style::new().foreground(SUBHEAD));
        for &field in reflect.field_names() {
            let Some(value) = reflect.get(field) else {
                continue;
            };
            let input = add_value_row(ui, panel, field, &value);
            rows.field_rows.push((input, node, i, field));
        }
    }
    panel
}

/// One labeled, editable inspector row; returns the text input's id.
fn add_value_row(ui: &mut Ui, panel: WidgetId, label: &str, value: &Value) -> WidgetId {
    let row = ui.panel(panel, Style::new().row().gap(6.0).align_center());
    ui.label(row, label, Style::new().width(70.0));
    ui.text_input(
        row,
        value_to_text(value),
        Style::new().grow(1.0).padding(4.0),
    )
}

/// The docked file explorer: the project's PNG and scene files.
fn build_file_explorer(
    ui: &mut Ui,
    parent: WidgetId,
    project_dir: &Path,
    height: f32,
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
            .background(PANEL_BG),
    );
    ui.label(panel, "Files", Style::new().foreground(HEADING));
    for name in list_project_files(project_dir) {
        if name.ends_with(".png") {
            // A PNG row is a button so it reads as grabbable; press it and
            // drag into the viewport to spawn a sprite there (main.rs routes
            // the press-drag-release; a plain click does nothing yet).
            let row = ui.button(
                panel,
                name.clone(),
                Style::new().padding(2.0).foreground(HEADING),
            );
            rows.file_rows.push((row, name));
        } else {
            ui.label(panel, name, Style::new().foreground(SUBHEAD));
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
    fn panel_sizes_survive_a_shell_rebuild() {
        // The regression: drag a splitter, then trigger a rebuild (as a
        // selection change does) - the dragged size must carry over, not snap
        // back to the default.
        let (scene, handle) = demo_scene();
        let dir = std::env::temp_dir(); // no project files needed for this test

        let sizes = PanelSizes::default();
        let (mut ui, rows) = build_editor_ui(&scene, None, handle, &dir, sizes);
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
        let (mut ui2, rows2) = build_editor_ui(&scene, None, handle, &dir, captured);
        ui2.layout(Vec2::new(1200.0, 800.0)).unwrap();
        assert_eq!(
            ui2.rect(rows2.tree_panel.unwrap()).unwrap().size.x,
            sizes.tree_width + 50.0,
            "the dragged width survives the rebuild"
        );
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
        let (mut ui, rows) = build_editor_ui(&scene, None, handle, &dir, sizes);
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
        let (mut ui2, rows2) = build_editor_ui(&scene, None, handle, &dir, captured);
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

        let (mut ui, rows) = build_editor_ui(&scene, None, handle, &dir, PanelSizes::default());
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
