//! atlas dock: the editor shell's dockable-panel layout, as pure data.
//!
//! A [`DockNode`] tree describes the arrangement: a [`DockNode::Split`] carves
//! space in two along an axis, and a [`DockNode::Leaf`] holds one or more
//! [`Pane`]s shown as tabs (one active at a time). `build_editor_ui` walks the
//! tree to emit the shell; dragging a tab onto another pane mutates it through
//! [`DockNode::move_pane`]. Headless and unit-tested - no widgets here.

use serde::{Deserialize, Serialize};

/// A dockable panel, identified by which editor view it shows. Each pane
/// appears exactly once across the whole tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Pane {
    SceneTree,
    Viewport,
    Files,
    Inspector,
    Console,
}

impl Pane {
    /// Every pane, so a loaded layout can be checked for completeness.
    pub const ALL: [Pane; 5] = [
        Pane::SceneTree,
        Pane::Viewport,
        Pane::Files,
        Pane::Inspector,
        Pane::Console,
    ];

    /// The pane's tab caption.
    pub fn title(self) -> &'static str {
        match self {
            Pane::SceneTree => "Scene",
            Pane::Viewport => "Viewport",
            Pane::Files => "Files",
            Pane::Inspector => "Inspector",
            Pane::Console => "Console",
        }
    }
}

/// Which way a [`DockNode::Split`] divides its space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SplitDir {
    /// Children sit side by side (`first` left, `second` right).
    Row,
    /// Children stack (`first` on top, `second` below).
    Column,
}

/// Which child of a split holds the fixed size; the other grows to fill. Splits
/// keep one fixed-size child and one flexible one (as the shell's splitters
/// always have), so a window resize flexes the right panel, not the tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Fixed {
    First,
    Second,
}

/// Where a dragged pane lands relative to the pane (group) under the cursor:
/// merged as a tab, or splitting off one of its four sides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropZone {
    Tab,
    Left,
    Right,
    Top,
    Bottom,
}

/// Default fixed size (px) for a side split (a Left/Right drop) and a stacked
/// split (a Top/Bottom drop), and the floor a splitter drag clamps to.
const SIDE_DEFAULT: f32 = 260.0;
const STACK_DEFAULT: f32 = 160.0;
pub const MIN_SIZE: f32 = 80.0;

/// The dock layout tree (see the module docs).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DockNode {
    /// A tab group: one or more panes, `active` shown (the rest as tabs).
    Leaf { panes: Vec<Pane>, active: usize },
    /// A division of space into `first` and `second` along `dir`, with `fixed`
    /// naming the fixed-size child and `size` its extent in px.
    Split {
        dir: SplitDir,
        fixed: Fixed,
        size: f32,
        first: Box<DockNode>,
        second: Box<DockNode>,
    },
}

impl DockNode {
    /// A single-pane leaf.
    pub fn leaf(pane: Pane) -> DockNode {
        DockNode::Leaf {
            panes: vec![pane],
            active: 0,
        }
    }

    /// The editor's starting layout: the scene tree on the left, the inspector
    /// on the right, and the viewport above the file explorer (with the console
    /// tabbed behind it) in the center.
    pub fn default_layout() -> DockNode {
        DockNode::Split {
            dir: SplitDir::Row,
            fixed: Fixed::First,
            size: 220.0,
            first: Box::new(DockNode::leaf(Pane::SceneTree)),
            second: Box::new(DockNode::Split {
                dir: SplitDir::Row,
                fixed: Fixed::Second,
                size: SIDE_DEFAULT,
                first: Box::new(DockNode::Split {
                    dir: SplitDir::Column,
                    fixed: Fixed::Second,
                    size: STACK_DEFAULT,
                    first: Box::new(DockNode::leaf(Pane::Viewport)),
                    second: Box::new(DockNode::Leaf {
                        panes: vec![Pane::Files, Pane::Console],
                        active: 0,
                    }),
                }),
                second: Box::new(DockNode::leaf(Pane::Inspector)),
            }),
        }
    }

    /// Whether the tree holds every pane exactly once - the invariant a valid
    /// layout must keep. A loaded (possibly stale or hand-edited) layout that
    /// fails this is discarded for [`default_layout`](Self::default_layout), so a
    /// pane can never go missing or appear twice.
    pub fn is_complete(&self) -> bool {
        let mut seen = self.flat_panes();
        seen.sort_by_key(|p| p.title());
        let mut all = Pane::ALL.to_vec();
        all.sort_by_key(|p| p.title());
        seen == all
    }

    /// Every pane in the tree, in order (may contain duplicates if malformed).
    fn flat_panes(&self) -> Vec<Pane> {
        let mut out = Vec::new();
        fn walk(node: &DockNode, out: &mut Vec<Pane>) {
            match node {
                DockNode::Leaf { panes, .. } => out.extend_from_slice(panes),
                DockNode::Split { first, second, .. } => {
                    walk(first, out);
                    walk(second, out);
                }
            }
        }
        walk(self, &mut out);
        out
    }

    /// Make the tab holding `pane` show it (a tab click). A no-op if not found.
    pub fn activate(&mut self, pane: Pane) {
        match self {
            DockNode::Leaf { panes, active } => {
                if let Some(i) = panes.iter().position(|&p| p == pane) {
                    *active = i;
                }
            }
            DockNode::Split { first, second, .. } => {
                first.activate(pane);
                second.activate(pane);
            }
        }
    }

    /// Move `pane` to land at `zone` relative to the group holding `target`
    /// (a tab drag-and-drop). Removing `pane` first may collapse the split it
    /// left behind; the pane is then re-inserted beside `target`. A no-op if
    /// `pane == target`, or if dropping a lone pane back as a tab onto its own
    /// group (nothing would change).
    pub fn move_pane(&mut self, pane: Pane, target: Pane, zone: DropZone) {
        if pane == target {
            return;
        }
        // Dropping a pane as a tab onto the group it already lives in alone is a
        // no-op (it would leave and immediately rejoin the same group).
        if zone == DropZone::Tab && self.group_of(pane) == self.group_of(target) {
            return;
        }
        let root = std::mem::replace(self, DockNode::leaf(pane));
        let stripped = root
            .remove_pane(pane)
            .expect("removing one of several panes leaves a tree");
        *self = stripped.insert_beside(target, pane, zone);
    }

    /// A stable identifier for the leaf holding `pane`: its sorted set of panes.
    /// Two panes share a group iff their `group_of` match. `None` if not found.
    fn group_of(&self, pane: Pane) -> Option<Vec<Pane>> {
        match self {
            DockNode::Leaf { panes, .. } => panes.contains(&pane).then(|| {
                let mut key = panes.clone();
                key.sort_by_key(|p| p.title());
                key
            }),
            DockNode::Split { first, second, .. } => {
                first.group_of(pane).or_else(|| second.group_of(pane))
            }
        }
    }

    /// Remove `pane`, returning the pruned tree (or `None` if the tree becomes
    /// empty). A split that loses one child collapses to the survivor.
    fn remove_pane(self, pane: Pane) -> Option<DockNode> {
        match self {
            DockNode::Leaf { mut panes, active } => {
                panes.retain(|&p| p != pane);
                if panes.is_empty() {
                    None
                } else {
                    Some(DockNode::Leaf {
                        active: active.min(panes.len() - 1),
                        panes,
                    })
                }
            }
            DockNode::Split {
                dir,
                fixed,
                size,
                first,
                second,
            } => match (first.remove_pane(pane), second.remove_pane(pane)) {
                (Some(a), Some(b)) => Some(DockNode::Split {
                    dir,
                    fixed,
                    size,
                    first: Box::new(a),
                    second: Box::new(b),
                }),
                (Some(only), None) | (None, Some(only)) => Some(only),
                (None, None) => None,
            },
        }
    }

    /// Insert `pane` beside the group holding `target` at `zone` (Tab merges
    /// into that group; a side wraps it in a new split with `pane` fixed-size).
    fn insert_beside(self, target: Pane, pane: Pane, zone: DropZone) -> DockNode {
        match self {
            DockNode::Leaf { mut panes, active } => {
                if !panes.contains(&target) {
                    return DockNode::Leaf { panes, active };
                }
                match zone {
                    DropZone::Tab => {
                        panes.push(pane);
                        DockNode::Leaf {
                            active: panes.len() - 1,
                            panes,
                        }
                    }
                    _ => {
                        let existing = DockNode::Leaf { panes, active };
                        split_around(existing, DockNode::leaf(pane), zone)
                    }
                }
            }
            DockNode::Split {
                dir,
                fixed,
                size,
                first,
                second,
            } => DockNode::Split {
                dir,
                fixed,
                size,
                first: Box::new(first.insert_beside(target, pane, zone)),
                second: Box::new(second.insert_beside(target, pane, zone)),
            },
        }
    }

    /// Write splitter sizes back into the tree from `sizes`, given in the same
    /// depth-first post-order the shell builder emits them (a split is recorded
    /// only after its children, since building a split needs its children's
    /// widgets first), so a splitter drag survives the next rebuild. Clamped to
    /// [`MIN_SIZE`].
    pub fn set_fixed_sizes(&mut self, sizes: &[f32]) {
        let mut i = 0;
        self.set_fixed_sizes_from(sizes, &mut i);
    }

    fn set_fixed_sizes_from(&mut self, sizes: &[f32], i: &mut usize) {
        if let DockNode::Split {
            size,
            first,
            second,
            ..
        } = self
        {
            first.set_fixed_sizes_from(sizes, i);
            second.set_fixed_sizes_from(sizes, i);
            if let Some(&s) = sizes.get(*i) {
                *size = s.max(MIN_SIZE);
            }
            *i += 1;
        }
    }
}

/// Wrap `existing` in a new split with `dropped` on the `zone` side, `dropped`
/// taking the fixed size (the just-docked panel gets a sensible default extent,
/// the existing area keeps filling the rest).
fn split_around(existing: DockNode, dropped: DockNode, zone: DropZone) -> DockNode {
    let (dir, size) = match zone {
        DropZone::Left | DropZone::Right => (SplitDir::Row, SIDE_DEFAULT),
        DropZone::Top | DropZone::Bottom => (SplitDir::Column, STACK_DEFAULT),
        DropZone::Tab => unreachable!("Tab is handled before split_around"),
    };
    let (fixed, first, second) = match zone {
        DropZone::Left | DropZone::Top => (Fixed::First, dropped, existing),
        DropZone::Right | DropZone::Bottom => (Fixed::Second, existing, dropped),
        DropZone::Tab => unreachable!("Tab is handled before split_around"),
    };
    DockNode::Split {
        dir,
        fixed,
        size,
        first: Box::new(first),
        second: Box::new(second),
    }
}

/// Test-only traversal helpers (the shell builder walks the tree directly).
#[cfg(test)]
impl DockNode {
    /// Whether `pane` is anywhere in the tree.
    fn contains(&self, pane: Pane) -> bool {
        match self {
            DockNode::Leaf { panes, .. } => panes.contains(&pane),
            DockNode::Split { first, second, .. } => first.contains(pane) || second.contains(pane),
        }
    }

    /// The panes flattened in left-to-right, top-to-bottom order.
    fn panes(&self) -> Vec<Pane> {
        match self {
            DockNode::Leaf { panes, .. } => panes.clone(),
            DockNode::Split { first, second, .. } => {
                let mut out = first.panes();
                out.extend(second.panes());
                out
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_layout_round_trips_through_ron_and_is_complete() {
        let dock = DockNode::default_layout();
        assert!(dock.is_complete());
        let text = ron::ser::to_string(&dock).unwrap();
        let back: DockNode = ron::from_str(&text).unwrap();
        assert_eq!(back, dock);
        // A layout missing a pane, or with a duplicate, is not complete.
        assert!(!DockNode::leaf(Pane::Viewport).is_complete());
        let dup = DockNode::Leaf {
            panes: vec![Pane::Viewport, Pane::Viewport],
            active: 0,
        };
        assert!(!dup.is_complete());
    }

    #[test]
    fn the_default_layout_holds_every_pane_once() {
        let dock = DockNode::default_layout();
        let mut panes = dock.panes();
        panes.sort_by_key(|p| p.title());
        assert_eq!(
            panes,
            vec![
                Pane::Console,
                Pane::Files,
                Pane::Inspector,
                Pane::SceneTree,
                Pane::Viewport
            ]
        );
    }

    #[test]
    fn activating_a_pane_selects_its_tab() {
        // A leaf with three tabs; activate the last.
        let mut dock = DockNode::Leaf {
            panes: vec![Pane::Inspector, Pane::Files, Pane::Viewport],
            active: 0,
        };
        dock.activate(Pane::Viewport);
        let DockNode::Leaf { active, .. } = dock else {
            unreachable!()
        };
        assert_eq!(active, 2);
    }

    #[test]
    fn dropping_a_pane_as_a_tab_merges_it_into_the_target_group() {
        let mut dock = DockNode::default_layout();
        // Drag the Files pane onto the Inspector as a tab.
        dock.move_pane(Pane::Files, Pane::Inspector, DropZone::Tab);

        // Every pane still present exactly once.
        let mut panes = dock.panes();
        panes.sort_by_key(|p| p.title());
        assert_eq!(
            panes,
            vec![
                Pane::Console,
                Pane::Files,
                Pane::Inspector,
                Pane::SceneTree,
                Pane::Viewport
            ]
        );
        // Inspector and Files now share a group, with Files active (just added).
        assert_eq!(dock.group_of(Pane::Files), dock.group_of(Pane::Inspector));
    }

    #[test]
    fn a_side_drop_splits_and_removing_collapses_the_leftover() {
        // Start with two panes tabbed together, plus a third beside them.
        let mut dock = DockNode::Split {
            dir: SplitDir::Row,
            fixed: Fixed::First,
            size: 200.0,
            first: Box::new(DockNode::Leaf {
                panes: vec![Pane::SceneTree, Pane::Files],
                active: 0,
            }),
            second: Box::new(DockNode::leaf(Pane::Inspector)),
        };
        // Drag Files out to the right of the Inspector: it leaves the tab group
        // (which collapses to just SceneTree) and forms a new split.
        dock.move_pane(Pane::Files, Pane::Inspector, DropZone::Right);

        assert!(dock.contains(Pane::Files));
        assert_ne!(dock.group_of(Pane::Files), dock.group_of(Pane::Inspector));
        // SceneTree is now alone in its (collapsed) group.
        assert_eq!(dock.group_of(Pane::SceneTree), Some(vec![Pane::SceneTree]));
        assert_eq!(dock.panes().len(), 3);
    }

    #[test]
    fn moving_a_pane_onto_itself_or_its_own_tab_group_is_a_no_op() {
        let mut dock = DockNode::default_layout();
        let before = dock.clone();
        dock.move_pane(Pane::Files, Pane::Files, DropZone::Right);
        assert_eq!(dock, before, "dropping a pane on itself changes nothing");
    }

    #[test]
    fn set_fixed_sizes_writes_back_in_depth_first_order_and_clamps() {
        let mut dock = DockNode::default_layout();
        // Post-order (children before parent): the inner Column, then the inner
        // Row, then the outer Row - matching the shell builder's emit order.
        dock.set_fixed_sizes(&[5.0, 280.0, 300.0]);
        let DockNode::Split { size, second, .. } = &dock else {
            unreachable!()
        };
        assert_eq!(*size, 300.0, "outer Row is recorded last");
        let DockNode::Split { size, first, .. } = second.as_ref() else {
            unreachable!()
        };
        assert_eq!(*size, 280.0, "inner Row is recorded second");
        let DockNode::Split { size, .. } = first.as_ref() else {
            unreachable!()
        };
        assert_eq!(
            *size, MIN_SIZE,
            "inner Column first; 5px clamps to the floor"
        );
    }
}
