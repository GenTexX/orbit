//! aurora ui: the widget arena, the taffy layout tree (with text measurement), and draw-list generation.

use cosmic_text::{Buffer, FontSystem, Shaping};
use glam::Vec2;
use slotmap::{SecondaryMap, SlotMap};
use taffy::{AvailableSpace, Size, TaffyTree};

use crate::draw::{DrawCommand, DrawList, Glyph};
use crate::error::AuroraError;
use crate::rect::Rect;
use crate::style::Style;
use crate::text;
use crate::widget::{Widget, WidgetId, WidgetKind};

/// A retained Aurora UI: owns the widget arena, the taffy layout tree, the font
/// system, and the shaped text buffers; caches each widget's absolute rectangle
/// after [`layout`](Self::layout).
///
/// Build a tree with [`root_panel`](Self::root_panel) and the typed child
/// constructors, run [`layout`](Self::layout), then produce a frame's
/// [`draw_list`](Self::draw_list). A backend renders that draw list, borrowing
/// [`font_system_mut`](Self::font_system_mut) to rasterize glyphs.
pub struct Ui {
    widgets: SlotMap<WidgetId, Widget>,
    taffy: TaffyTree<WidgetId>,
    root: Option<WidgetId>,
    rects: SecondaryMap<WidgetId, Rect>,
    font_system: FontSystem,
    /// One shaped text buffer per text-bearing widget, produced during layout.
    buffers: SecondaryMap<WidgetId, Buffer>,
}

impl Ui {
    /// Create an empty UI (with the bundled font loaded).
    pub fn new() -> Self {
        Self {
            widgets: SlotMap::with_key(),
            taffy: TaffyTree::new(),
            root: None,
            rects: SecondaryMap::new(),
            font_system: text::make_font_system(),
            buffers: SecondaryMap::new(),
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

    /// The font system, for a backend to rasterize the glyphs in the draw list.
    pub fn font_system_mut(&mut self) -> &mut FontSystem {
        &mut self.font_system
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
            foreground: style.foreground,
            clip: style.clip,
        });
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

    /// Run layout for `available` space (in pixels). Text-bearing leaves are
    /// shaped and measured through a taffy measure function; then the tree is
    /// walked to turn taffy's parent-relative positions into absolute rects.
    pub fn layout(&mut self, available: Vec2) -> Result<(), AuroraError> {
        let Some(root) = self.root else {
            return Ok(());
        };
        let root_node = self.widgets[root].taffy;

        // Split the borrows so the measure closure can shape text (widgets +
        // buffers + font_system) while taffy computes layout.
        let widgets = &self.widgets;
        let buffers = &mut self.buffers;
        let font_system = &mut self.font_system;
        self.taffy.compute_layout_with_measure(
            root_node,
            Size {
                width: AvailableSpace::Definite(available.x),
                height: AvailableSpace::Definite(available.y),
            },
            |known, avail, _node, ctx, _style| {
                measure_widget(known, avail, ctx, widgets, buffers, font_system)
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

        let children = self.widgets[id].children.clone();
        for child in children {
            self.accumulate(child, pos);
        }
    }

    /// Produce this frame's draw list by walking the laid-out tree. Requires a
    /// prior [`layout`](Self::layout).
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
        if let WidgetKind::Label(_) = &widget.kind {
            self.emit_text(id, rect, widget.foreground, list);
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

    /// Emit a text command for a text-bearing widget from its shaped buffer,
    /// positioned at `rect`'s top-left.
    fn emit_text(&self, id: WidgetId, rect: Rect, color: crate::Color, list: &mut DrawList) {
        let Some(buffer) = self.buffers.get(id) else {
            return;
        };
        let mut glyphs = Vec::new();
        for run in buffer.layout_runs() {
            for glyph in run.glyphs {
                // Bake the widget's origin and the run's baseline into the pen
                // position; the backend adds each glyph's bitmap bearing.
                let physical = glyph.physical((rect.pos.x, rect.pos.y + run.line_y), 1.0);
                glyphs.push(Glyph {
                    cache_key: physical.cache_key,
                    x: physical.x as f32,
                    y: physical.y as f32,
                });
            }
        }
        if !glyphs.is_empty() {
            list.commands.push(DrawCommand::Text { glyphs, color });
        }
    }
}

impl Default for Ui {
    fn default() -> Self {
        Self::new()
    }
}

/// taffy measure function: shape a text-bearing leaf and return its size. Called
/// only for leaf nodes without a definite style size.
fn measure_widget(
    known: Size<Option<f32>>,
    available: Size<AvailableSpace>,
    ctx: Option<&mut WidgetId>,
    widgets: &SlotMap<WidgetId, Widget>,
    buffers: &mut SecondaryMap<WidgetId, Buffer>,
    font_system: &mut FontSystem,
) -> Size<f32> {
    let Some(&mut id) = ctx else {
        return Size::ZERO;
    };
    let content = match &widgets[id].kind {
        WidgetKind::Label(t) | WidgetKind::Button(t) | WidgetKind::TextInput(t) => t.as_str(),
        WidgetKind::Panel | WidgetKind::Checkbox(_) => return Size::ZERO,
    };

    // Wrap to a known width if taffy gives one, else the available width.
    let wrap_width = known.width.or(match available.width {
        AvailableSpace::Definite(w) => Some(w),
        AvailableSpace::MinContent | AvailableSpace::MaxContent => None,
    });

    if !buffers.contains_key(id) {
        buffers.insert(id, Buffer::new(font_system, text::metrics()));
    }
    let buffer = &mut buffers[id];
    {
        let mut borrowed = buffer.borrow_with(font_system);
        borrowed.set_size(wrap_width, None);
        borrowed.set_text(content, &text::default_attrs(), Shaping::Advanced, None);
        borrowed.shape_until_scroll(false);
    }

    let mut width = 0.0f32;
    let mut lines = 0u32;
    for run in buffer.layout_runs() {
        width = width.max(run.line_w);
        lines += 1;
    }
    Size {
        width: width.ceil(),
        height: lines.max(1) as f32 * text::LINE_HEIGHT,
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
        assert_eq!(ui.rect(b).unwrap().pos, Vec2::new(0.0, 30.0));
    }

    #[test]
    fn nested_positions_accumulate() {
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().padding(10.0).size(200.0, 200.0));
        let child = ui.panel(root, Style::new().padding(8.0).size(200.0, 200.0));
        let grandchild = ui.panel(child, Style::new().size(20.0, 20.0));
        ui.layout(Vec2::new(400.0, 400.0)).unwrap();

        assert_eq!(ui.rect(child).unwrap().pos, Vec2::new(10.0, 10.0));
        assert_eq!(ui.rect(grandchild).unwrap().pos, Vec2::new(18.0, 18.0));
    }

    #[test]
    fn a_label_is_measured_and_emits_a_text_command() {
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().column());
        let label = ui.label(root, "Hello", Style::new());
        ui.layout(Vec2::new(400.0, 400.0)).unwrap();

        // The bundled font gives the label a real, non-zero size.
        let r = ui.rect(label).unwrap();
        assert!(
            r.size.x > 0.0,
            "label width should be measured, got {}",
            r.size.x
        );
        assert!(
            r.size.y > 0.0,
            "label height should be measured, got {}",
            r.size.y
        );

        // And it emits a text command with glyphs.
        let has_text = ui
            .draw_list()
            .commands
            .iter()
            .any(|c| matches!(c, DrawCommand::Text { glyphs, .. } if !glyphs.is_empty()));
        assert!(has_text, "label should emit a non-empty Text command");
    }

    #[test]
    fn draw_list_emits_fills_and_clips_in_tree_order() {
        let red = Color::rgb(1.0, 0.0, 0.0);
        let blue = Color::rgb(0.0, 0.0, 1.0);

        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().size(100.0, 100.0).background(red).clip());
        ui.panel(root, Style::new().size(50.0, 50.0).background(blue));
        ui.layout(Vec2::new(100.0, 100.0)).unwrap();

        let cmds = ui.draw_list().commands;
        assert_eq!(cmds.len(), 4, "{cmds:?}");
        assert!(matches!(cmds[0], DrawCommand::FillRect { color, .. } if color == red));
        assert!(matches!(cmds[1], DrawCommand::PushClip { .. }));
        assert!(matches!(cmds[2], DrawCommand::FillRect { color, .. } if color == blue));
        assert!(matches!(cmds[3], DrawCommand::PopClip));
    }
}
