//! aurora ui: the widget arena, the taffy layout tree, the builder API, layout, and draw-list generation.

use glam::Vec2;
use slotmap::{SecondaryMap, SlotMap};
use taffy::{AvailableSpace, Size, TaffyTree};

use crate::draw::{DrawCommand, DrawList};
use crate::error::AuroraError;
use crate::rect::Rect;
use crate::style::Style;
use crate::widget::{Widget, WidgetId, WidgetKind};

/// A retained Aurora UI: owns the widget arena and the taffy layout tree, and
/// caches each widget's absolute rectangle after [`layout`](Self::layout).
///
/// Build a tree with [`root_panel`](Self::root_panel) plus the typed child
/// constructors, run [`layout`](Self::layout), then either read rectangles with
/// [`rect`](Self::rect) or produce a frame's [`draw_list`](Self::draw_list). The
/// widget tree and the taffy tree stay in lockstep: every widget owns one taffy
/// node.
pub struct Ui {
    widgets: SlotMap<WidgetId, Widget>,
    taffy: TaffyTree<WidgetId>,
    root: Option<WidgetId>,
    rects: SecondaryMap<WidgetId, Rect>,
}

impl Ui {
    /// Create an empty UI with no root.
    pub fn new() -> Self {
        Self {
            widgets: SlotMap::with_key(),
            taffy: TaffyTree::new(),
            root: None,
            rects: SecondaryMap::new(),
        }
    }

    /// Create the root widget, a panel with the given style. Replaces any
    /// existing root.
    pub fn root_panel(&mut self, style: Style) -> WidgetId {
        let id = self.add(WidgetKind::Panel, style, None);
        self.root = Some(id);
        id
    }

    /// The current root widget, if one has been set.
    pub fn root(&self) -> Option<WidgetId> {
        self.root
    }

    /// Add a child panel (a container) under `parent`.
    pub fn panel(&mut self, parent: WidgetId, style: Style) -> WidgetId {
        self.add(WidgetKind::Panel, style, Some(parent))
    }

    /// Add a static text label under `parent`.
    pub fn label(&mut self, parent: WidgetId, text: impl Into<String>, style: Style) -> WidgetId {
        self.add(WidgetKind::Label(text.into()), style, Some(parent))
    }

    /// Add a clickable button under `parent`.
    pub fn button(&mut self, parent: WidgetId, text: impl Into<String>, style: Style) -> WidgetId {
        self.add(WidgetKind::Button(text.into()), style, Some(parent))
    }

    /// Add a checkbox under `parent`, initially `checked` or not.
    pub fn checkbox(&mut self, parent: WidgetId, checked: bool, style: Style) -> WidgetId {
        self.add(WidgetKind::Checkbox(checked), style, Some(parent))
    }

    /// Add a single-line text input under `parent`, holding initial `text`.
    pub fn text_input(
        &mut self,
        parent: WidgetId,
        text: impl Into<String>,
        style: Style,
    ) -> WidgetId {
        self.add(WidgetKind::TextInput(text.into()), style, Some(parent))
    }

    /// The kind (and per-widget data) of a widget.
    pub fn kind(&self, id: WidgetId) -> &WidgetKind {
        &self.widgets[id].kind
    }

    /// The parent of a widget, or `None` for the root. Event routing walks up
    /// these links to bubble (Milestone 2, step 4).
    pub fn parent(&self, id: WidgetId) -> Option<WidgetId> {
        self.widgets[id].parent
    }

    /// The widget's absolute rectangle from the last [`layout`](Self::layout), or
    /// `None` if it has not been laid out yet.
    pub fn rect(&self, id: WidgetId) -> Option<Rect> {
        self.rects.get(id).copied()
    }

    /// Insert a widget, create its taffy node, and link it under `parent`. taffy
    /// calls only fail on internal misuse, so they are treated as bugs.
    fn add(&mut self, kind: WidgetKind, style: Style, parent: Option<WidgetId>) -> WidgetId {
        let node = self.taffy.new_leaf(style.layout).expect("taffy new_leaf");
        let id = self.widgets.insert(Widget {
            kind,
            taffy: node,
            parent,
            children: Vec::new(),
            background: style.background,
            clip: style.clip,
        });
        // Let taffy map its node back to our widget (needed once text widgets
        // measure themselves via a taffy measure function).
        self.taffy
            .set_node_context(node, Some(id))
            .expect("taffy set_node_context");
        if let Some(parent) = parent {
            self.widgets[parent].children.push(id);
            let parent_node = self.widgets[parent].taffy;
            self.taffy
                .add_child(parent_node, node)
                .expect("taffy add_child");
        }
        id
    }

    /// Run layout for `available` space (in pixels) and cache every widget's
    /// absolute rectangle. taffy reports each node's position relative to its
    /// parent; this pass walks the tree to turn those into absolute rects.
    pub fn layout(&mut self, available: Vec2) -> Result<(), AuroraError> {
        let Some(root) = self.root else {
            return Ok(());
        };
        let root_node = self.widgets[root].taffy;
        self.taffy.compute_layout(
            root_node,
            Size {
                width: AvailableSpace::Definite(available.x),
                height: AvailableSpace::Definite(available.y),
            },
        )?;
        self.rects.clear();
        self.accumulate(root, Vec2::ZERO);
        Ok(())
    }

    /// Walk the subtree at `id`, adding each parent's absolute origin to taffy's
    /// parent-relative location to get an absolute rect.
    fn accumulate(&mut self, id: WidgetId, origin: Vec2) {
        let node = self.widgets[id].taffy;
        let layout = self.taffy.layout(node).expect("taffy layout");
        let pos = origin + Vec2::new(layout.location.x, layout.location.y);
        let size = Vec2::new(layout.size.width, layout.size.height);
        self.rects.insert(id, Rect::new(pos, size));

        // Clone the child list so the recursive borrow of `self` is not blocked
        // by an outstanding borrow of the arena.
        let children = self.widgets[id].children.clone();
        for child in children {
            self.accumulate(child, pos);
        }
    }

    /// Produce this frame's draw list by walking the laid-out tree: each widget
    /// emits a background fill (if any), then a clip around its children (if it
    /// clips), then its children, in tree order. Requires a prior
    /// [`layout`](Self::layout).
    pub fn draw_list(&self) -> DrawList {
        let mut list = DrawList::default();
        if let Some(root) = self.root {
            self.emit(root, &mut list);
        }
        list
    }

    fn emit(&self, id: WidgetId, list: &mut DrawList) {
        let Some(rect) = self.rect(id) else {
            return;
        };
        let widget = &self.widgets[id];

        if widget.background.a > 0.0 {
            list.commands.push(DrawCommand::FillRect {
                rect,
                color: widget.background,
            });
        }
        if widget.clip {
            list.commands.push(DrawCommand::PushClip { rect });
        }
        for &child in &widget.children {
            self.emit(child, list);
        }
        if widget.clip {
            list.commands.push(DrawCommand::PopClip);
        }
    }
}

impl Default for Ui {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::Ui;
    use crate::color::Color;
    use crate::draw::DrawCommand;
    use crate::style::Style;
    use glam::Vec2;

    #[test]
    fn root_takes_its_style_size_at_the_origin() {
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().size(200.0, 100.0));
        ui.layout(Vec2::new(800.0, 600.0)).unwrap();

        let r = ui.rect(root).unwrap();
        assert_eq!(r.pos, Vec2::ZERO);
        assert_eq!(r.size, Vec2::new(200.0, 100.0));
    }

    #[test]
    fn a_column_stacks_children_vertically() {
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().column().size(100.0, 100.0));
        let a = ui.panel(root, Style::new().size(100.0, 30.0));
        let b = ui.panel(root, Style::new().size(100.0, 40.0));
        ui.layout(Vec2::new(200.0, 200.0)).unwrap();

        assert_eq!(ui.rect(a).unwrap().pos, Vec2::new(0.0, 0.0));
        assert_eq!(ui.rect(a).unwrap().size, Vec2::new(100.0, 30.0));
        // b stacks directly below a.
        assert_eq!(ui.rect(b).unwrap().pos, Vec2::new(0.0, 30.0));
        assert_eq!(ui.rect(b).unwrap().size, Vec2::new(100.0, 40.0));
    }

    #[test]
    fn nested_positions_accumulate() {
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().padding(10.0).size(200.0, 200.0));
        let child = ui.panel(root, Style::new().padding(8.0).size(200.0, 200.0));
        let grandchild = ui.panel(child, Style::new().size(20.0, 20.0));
        ui.layout(Vec2::new(400.0, 400.0)).unwrap();

        // child at (10, 10) from root padding; grandchild a further (8, 8) in.
        assert_eq!(ui.rect(child).unwrap().pos, Vec2::new(10.0, 10.0));
        assert_eq!(ui.rect(grandchild).unwrap().pos, Vec2::new(18.0, 18.0));
    }

    #[test]
    fn draw_list_emits_fills_and_clips_in_tree_order() {
        let red = Color::rgb(1.0, 0.0, 0.0);
        let blue = Color::rgb(0.0, 0.0, 1.0);

        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().size(100.0, 100.0).background(red).clip());
        ui.panel(root, Style::new().size(50.0, 50.0).background(blue));
        ui.layout(Vec2::new(100.0, 100.0)).unwrap();

        // root fill, then a clip around the child, the child fill, then pop.
        let cmds = ui.draw_list().commands;
        assert_eq!(cmds.len(), 4, "{cmds:?}");
        assert!(matches!(cmds[0], DrawCommand::FillRect { color, .. } if color == red));
        assert!(matches!(cmds[1], DrawCommand::PushClip { .. }));
        assert!(matches!(cmds[2], DrawCommand::FillRect { color, .. } if color == blue));
        assert!(matches!(cmds[3], DrawCommand::PopClip));
    }

    #[test]
    fn transparent_widgets_emit_no_fill() {
        let mut ui = Ui::new();
        // No background -> transparent -> no FillRect for the root.
        ui.root_panel(Style::new().size(100.0, 100.0));
        ui.layout(Vec2::new(100.0, 100.0)).unwrap();
        assert!(ui.draw_list().commands.is_empty());
    }
}
