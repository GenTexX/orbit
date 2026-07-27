//! atlas project-local editor state: the per-project view (dock layout and
//! sizes, camera, active tool, pane scroll, collapsed tree nodes), persisted as
//! RON at `<project>/.orbit/editor.ron`.
//!
//! This is machine-managed view state, distinct from the user-global settings
//! (`~/.config/orbit/settings.ron`, theme + grid/snap): it travels with a
//! project, not the user. Unlike settings it is NOT written on first sight -
//! only on exit and on a debounced change - so opening a project read-only
//! leaves no trace.
//!
//! Collapsed tree nodes are stored as child-index PATHS from the scene root
//! (not `NodeId`s, which are slot-map keys meaningless across a reload): each
//! path is resolved back to a node when loading, and dropped if it no longer
//! resolves.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use helios::{NodeId, Scene};
use serde::{Deserialize, Serialize};

use crate::dock::{DockNode, Pane};
use crate::viewport::{EditorCamera, GizmoMode};

/// The `.orbit` subdirectory of a project (holds machine-managed editor state).
fn state_dir(project_dir: &Path) -> PathBuf {
    project_dir.join(".orbit")
}

/// The editor-state file path (`<project>/.orbit/editor.ron`).
fn state_path(project_dir: &Path) -> PathBuf {
    state_dir(project_dir).join("editor.ron")
}

/// A serializable mirror of [`EditorCamera`] (its own fields are runtime-only).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CameraState {
    pub pan: [f32; 2],
    pub zoom: f32,
}

impl CameraState {
    pub fn from_camera(c: &EditorCamera) -> Self {
        Self {
            pan: [c.pan.x, c.pan.y],
            zoom: c.zoom,
        }
    }

    pub fn to_camera(self) -> EditorCamera {
        EditorCamera {
            pan: glam::Vec2::new(self.pan[0], self.pan[1]),
            zoom: self.zoom,
        }
    }
}

impl Default for CameraState {
    fn default() -> Self {
        Self::from_camera(&EditorCamera::default())
    }
}

/// The per-project editor view, persisted across restarts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorState {
    /// The dock layout (which panes go where, their tab order, and split sizes).
    pub dock: DockNode,
    /// The viewport pan/zoom.
    pub camera: CameraState,
    /// The active transform tool.
    pub gizmo_mode: GizmoMode,
    /// Per-pane scroll offsets.
    pub pane_scrolls: Vec<(Pane, f32)>,
    /// Collapsed scene-tree nodes, as child-index paths from the root.
    pub collapsed: Vec<Vec<usize>>,
}

impl Default for EditorState {
    fn default() -> Self {
        Self {
            dock: DockNode::default_layout(),
            camera: CameraState::default(),
            gizmo_mode: GizmoMode::default(),
            pane_scrolls: Vec::new(),
            collapsed: Vec::new(),
        }
    }
}

/// Load a project's editor state, or `None` if the file is missing or invalid
/// (an invalid file logs a warning and is left as-is). A loaded layout whose
/// dock is not a complete pane set is repaired to the default layout.
pub fn load(project_dir: &Path) -> Option<EditorState> {
    let path = state_path(project_dir);
    let text = std::fs::read_to_string(&path).ok()?;
    match ron::from_str::<EditorState>(&text) {
        Ok(mut state) => {
            if !state.dock.is_complete() {
                tracing::warn!(
                    "editor layout in {} was incomplete; using the default",
                    path.display()
                );
                state.dock = DockNode::default_layout();
            }
            Some(state)
        }
        Err(err) => {
            tracing::warn!("editor state at {} is invalid: {err}", path.display());
            None
        }
    }
}

/// Encode collapsed node ids as child-index paths from the scene root, so they
/// survive a reload (a `NodeId` is a slot-map key that does not). A node no
/// longer in the tree is silently dropped.
pub fn encode_collapsed(scene: &Scene, collapsed: &HashSet<NodeId>) -> Vec<Vec<usize>> {
    let mut paths: Vec<Vec<usize>> = collapsed
        .iter()
        .filter_map(|&node| node_path(scene, node))
        .collect();
    // Sorted so the saved file is deterministic (nice diffs, stable tests).
    paths.sort();
    paths
}

/// The child-index path from the root down to `node` (empty for the root), or
/// `None` if `node` is detached from the tree.
fn node_path(scene: &Scene, node: NodeId) -> Option<Vec<usize>> {
    let mut path = Vec::new();
    let mut cur = node;
    while let Some(parent) = scene.parent(cur) {
        let idx = scene.children(parent).iter().position(|&c| c == cur)?;
        path.push(idx);
        cur = parent;
    }
    path.reverse();
    Some(path)
}

/// Resolve child-index paths back to node ids against `scene`, dropping any that
/// no longer point at a node (the tree changed since the paths were saved).
pub fn decode_collapsed(scene: &Scene, paths: &[Vec<usize>]) -> HashSet<NodeId> {
    paths
        .iter()
        .filter_map(|path| resolve_path(scene, path))
        .collect()
}

fn resolve_path(scene: &Scene, path: &[usize]) -> Option<NodeId> {
    let mut node = scene.root();
    for &idx in path {
        node = *scene.children(node).get(idx)?;
    }
    Some(node)
}

/// Write a project's editor state (creating the `.orbit` directory).
pub fn save(project_dir: &Path, state: &EditorState) -> std::io::Result<()> {
    let dir = state_dir(project_dir);
    std::fs::create_dir_all(&dir)?;
    let pretty = ron::ser::PrettyConfig::default();
    let text = ron::ser::to_string_pretty(state, pretty)
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    std::fs::write(dir.join("editor.ron"), text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_state_round_trips_through_ron() {
        let state = EditorState {
            dock: DockNode::default_layout(),
            camera: CameraState {
                pan: [12.0, -30.0],
                zoom: 2.5,
            },
            gizmo_mode: GizmoMode::Rotate,
            pane_scrolls: vec![(Pane::SceneTree, 40.0), (Pane::Console, 100.0)],
            collapsed: vec![vec![0], vec![1, 2]],
        };
        let text = ron::ser::to_string_pretty(&state, ron::ser::PrettyConfig::default()).unwrap();
        let back: EditorState = ron::from_str(&text).unwrap();
        assert_eq!(back, state);
    }

    #[test]
    fn an_incomplete_saved_layout_is_repaired_on_load() {
        let dir = tempfile::tempdir().unwrap();
        // A layout missing panes (only the scene tree).
        let bad = EditorState {
            dock: DockNode::leaf(Pane::SceneTree),
            ..EditorState::default()
        };
        save(dir.path(), &bad).unwrap();
        let loaded = load(dir.path()).unwrap();
        assert!(loaded.dock.is_complete(), "load repairs an incomplete dock");
    }

    #[test]
    fn collapsed_paths_survive_a_reload_and_drop_stale_ones() {
        use helios::Node;
        // Build root -> [a -> [a0, a1], b]; collapse a and b.
        let mut scene = Scene::new("root");
        let root = scene.root();
        let a = scene.add_child(root, Node::new("a"));
        let a0 = scene.add_child(a, Node::new("a0"));
        scene.add_child(a, Node::new("a1"));
        let b = scene.add_child(root, Node::new("b"));
        let collapsed: HashSet<NodeId> = [a, b, a0].into_iter().collect();

        let paths = encode_collapsed(&scene, &collapsed);
        // A structurally identical scene with fresh NodeIds (as after a reload).
        let mut reloaded = Scene::new("root");
        let r2 = reloaded.root();
        let a2 = reloaded.add_child(r2, Node::new("a"));
        let a0_2 = reloaded.add_child(a2, Node::new("a0"));
        reloaded.add_child(a2, Node::new("a1"));
        let b2 = reloaded.add_child(r2, Node::new("b"));
        let back = decode_collapsed(&reloaded, &paths);
        assert_eq!(back, [a2, b2, a0_2].into_iter().collect());

        // A path that no longer resolves (a scene with fewer nodes) is dropped.
        let mut smaller = Scene::new("root");
        let rs = smaller.root();
        smaller.add_child(rs, Node::new("only")); // no children, no second child
        let survivors = decode_collapsed(&smaller, &paths);
        // Only the depth-1 index-0 path ("a") resolves to the lone child.
        assert_eq!(survivors.len(), 1);
    }

    #[test]
    fn save_then_load_round_trips_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load(dir.path()).is_none(), "nothing to load yet");
        let state = EditorState {
            gizmo_mode: GizmoMode::Scale,
            camera: CameraState {
                pan: [1.0, 2.0],
                zoom: 3.0,
            },
            ..EditorState::default()
        };
        save(dir.path(), &state).unwrap();
        let loaded = load(dir.path()).unwrap();
        assert_eq!(loaded.gizmo_mode, GizmoMode::Scale);
        assert_eq!(loaded.camera.zoom, 3.0);
    }
}
