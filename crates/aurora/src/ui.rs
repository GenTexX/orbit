//! aurora ui: the widget arena, the taffy layout tree (with text measurement), and draw-list generation.

use cosmic_text::{Buffer, FontSystem, Shaping};
use glam::Vec2;
use slotmap::{SecondaryMap, SlotMap};
use taffy::{AvailableSpace, Size, TaffyTree};

use crate::color::Color;
use crate::draw::{DrawCommand, DrawList, Glyph};
use crate::error::AuroraError;
use crate::event::Event;
use crate::input::InputEvent;
use crate::rect::Rect;
use crate::style::Style;
use crate::widget::{Widget, WidgetId, WidgetKind};
use crate::{text, theme};

/// The visual state of an interactive widget, derived from the pointer each
/// frame. Drives the built-in hover and pressed shading.
#[derive(Clone, Copy, PartialEq)]
enum Interaction {
    Idle,
    Hovered,
    Pressed,
}

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
    /// Absolute origin of each widget's content box (its rect inset by padding
    /// and border), where its text is drawn.
    content_origin: SecondaryMap<WidgetId, Vec2>,
    font_system: FontSystem,
    /// One shaped text buffer per text-bearing widget, produced during layout.
    buffers: SecondaryMap<WidgetId, Buffer>,
    /// Last known pointer position, or `None` when the pointer is off-surface.
    cursor: Option<Vec2>,
    /// The interactive widget under the pointer (a click bubbles to it).
    hovered: Option<WidgetId>,
    /// The interactive widget a press landed on, armed until release.
    pressed: Option<WidgetId>,
    /// Semantic events accumulated since the last [`drain_events`](Self::drain_events).
    events: Vec<Event>,
}

impl Ui {
    /// Create an empty UI (with the bundled font loaded).
    pub fn new() -> Self {
        Self {
            widgets: SlotMap::with_key(),
            taffy: TaffyTree::new(),
            root: None,
            rects: SecondaryMap::new(),
            content_origin: SecondaryMap::new(),
            font_system: text::make_font_system(),
            buffers: SecondaryMap::new(),
            cursor: None,
            hovered: None,
            pressed: None,
            events: Vec::new(),
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

    /// Replace a widget's background color. Takes effect on the next draw.
    pub fn set_background(&mut self, id: WidgetId, color: Color) {
        self.widgets[id].background = color;
    }

    /// Replace a label's text. It is re-shaped and re-measured on the next
    /// [`layout`](Self::layout); a no-op on non-label widgets.
    pub fn set_label(&mut self, id: WidgetId, text: impl Into<String>) {
        if let WidgetKind::Label(s) = &mut self.widgets[id].kind {
            *s = text.into();
            // taffy caches layout per node; without marking this one dirty it
            // would serve the old measurement and never re-run the measure
            // function that re-shapes the text buffer.
            let node = self.widgets[id].taffy;
            self.taffy.mark_dirty(node).expect("taffy mark_dirty");
        }
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
        self.content_origin.clear();
        self.accumulate(root, Vec2::ZERO);
        // Layout may have moved widgets under a stationary pointer, so refresh
        // what is hovered from the last known cursor position.
        self.update_hover();
        Ok(())
    }

    /// Walk the subtree at `id`, adding each parent's absolute origin to taffy's
    /// parent-relative location to get an absolute rect (and the content-box
    /// origin where text is drawn).
    fn accumulate(&mut self, id: WidgetId, origin: Vec2) {
        let node = self.widgets[id].taffy;
        let layout = self.taffy.layout(node).expect("taffy layout");
        let pos = origin + Vec2::new(layout.location.x, layout.location.y);
        let size = Vec2::new(layout.size.width, layout.size.height);
        self.rects.insert(id, Rect::new(pos, size));

        let inset = Vec2::new(
            layout.padding.left + layout.border.left,
            layout.padding.top + layout.border.top,
        );
        self.content_origin.insert(id, pos + inset);

        let children = self.widgets[id].children.clone();
        for child in children {
            self.accumulate(child, pos);
        }
    }

    /// Feed one pointer event into the UI, updating hover/press state and
    /// possibly queuing a semantic [`Event`]. Uses the most recent
    /// [`layout`](Self::layout) for hit-testing.
    pub fn handle_input(&mut self, event: InputEvent) {
        match event {
            InputEvent::PointerMoved(p) => {
                self.cursor = Some(p);
                self.update_hover();
            }
            InputEvent::PointerLeft => {
                self.cursor = None;
                self.hovered = None;
            }
            InputEvent::PointerPressed => {
                // Arm the widget under the pointer; a release over the same one
                // is a click.
                self.pressed = self.hovered;
            }
            InputEvent::PointerReleased => {
                // `take` always disarms; activation happens only on a release
                // over the same widget the press landed on.
                if let Some(id) = self.pressed.take()
                    && self.hovered == Some(id)
                {
                    self.activate(id);
                }
            }
        }
    }

    /// Take the events queued since the last call. Call once per frame.
    pub fn drain_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    /// The interactive widget currently under the pointer, if any. Exposed mostly
    /// for tests and debugging; rendering reads it internally.
    pub fn hovered(&self) -> Option<WidgetId> {
        self.hovered
    }

    /// Recompute [`hovered`](Self::hovered) from the current cursor: hit-test the
    /// topmost widget, then bubble to its nearest interactive ancestor.
    fn update_hover(&mut self) {
        self.hovered = self
            .cursor
            .and_then(|p| self.hit_test(p))
            .and_then(|id| self.interactive_ancestor(id));
    }

    /// The topmost widget whose rectangle contains `point`, respecting clips.
    /// "Topmost" is the last widget in draw order (deepest, latest sibling).
    pub fn hit_test(&self, point: Vec2) -> Option<WidgetId> {
        let root = self.root?;
        self.hit_test_node(root, point, None)
    }

    fn hit_test_node(&self, id: WidgetId, point: Vec2, clip: Option<Rect>) -> Option<WidgetId> {
        let rect = self.rect(id)?;
        // A widget clips its subtree to the intersection of the inherited clip
        // and its own rectangle.
        let child_clip = if self.widgets[id].clip {
            Some(clip.map_or(rect, |c| c.intersect(rect)))
        } else {
            clip
        };
        // Children draw over their parent and later siblings over earlier ones,
        // so scan children in order and keep the last (topmost) hit.
        let mut hit = None;
        for &child in &self.widgets[id].children {
            if let Some(h) = self.hit_test_node(child, point, child_clip) {
                hit = Some(h);
            }
        }
        if hit.is_some() {
            return hit;
        }
        // Otherwise the widget itself, if the point is within it and its clip.
        let visible = clip.is_none_or(|c| c.contains(point));
        (visible && rect.contains(point)).then_some(id)
    }

    /// Walk up from `id` (inclusive) to the nearest interactive widget.
    fn interactive_ancestor(&self, id: WidgetId) -> Option<WidgetId> {
        let mut cur = Some(id);
        while let Some(w) = cur {
            if self.widgets[w].kind.is_interactive() {
                return Some(w);
            }
            cur = self.widgets[w].parent;
        }
        None
    }

    /// Apply a click to an interactive widget: fire its event and update any
    /// state it owns (a checkbox toggles).
    fn activate(&mut self, id: WidgetId) {
        let event = match &mut self.widgets[id].kind {
            WidgetKind::Button(_) => Some(Event::Clicked(id)),
            WidgetKind::Checkbox(checked) => {
                *checked = !*checked;
                Some(Event::Toggled {
                    id,
                    checked: *checked,
                })
            }
            _ => None,
        };
        if let Some(event) = event {
            self.events.push(event);
        }
    }

    /// The interaction state of `id` for shading: pressed only while the pointer
    /// is both armed on it and still over it.
    fn interaction(&self, id: WidgetId) -> Interaction {
        if self.pressed == Some(id) && self.hovered == Some(id) {
            Interaction::Pressed
        } else if self.hovered == Some(id) {
            Interaction::Hovered
        } else {
            Interaction::Idle
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

        // The widget's own visuals, by kind.
        match &widget.kind {
            WidgetKind::Panel => self.fill_background(rect, widget.background, list),
            WidgetKind::Label(_) | WidgetKind::TextInput(_) => {
                // A text input renders like a label for now; focus, caret, and
                // field styling arrive in step 5.
                self.fill_background(rect, widget.background, list);
                self.emit_text(id, widget.foreground, list);
            }
            WidgetKind::Button(_) => self.emit_button(id, rect, widget, list),
            WidgetKind::Checkbox(checked) => self.emit_checkbox(id, rect, *checked, list),
        }

        // Children, optionally clipped to this widget's rectangle.
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

    /// Emit a solid background fill, skipping fully transparent ones.
    fn fill_background(&self, rect: Rect, color: Color, list: &mut DrawList) {
        if color.a > 0.0 {
            list.commands.push(DrawCommand::FillRect { rect, color });
        }
    }

    /// A button: a state-shaded fill (its style background, or the theme default)
    /// under its caption.
    fn emit_button(&self, id: WidgetId, rect: Rect, widget: &Widget, list: &mut DrawList) {
        let base = if widget.background.a > 0.0 {
            widget.background
        } else {
            theme::BUTTON
        };
        let color = match self.interaction(id) {
            Interaction::Idle => base,
            Interaction::Hovered => base.lighten(0.08),
            Interaction::Pressed => base.darken(0.12),
        };
        list.commands.push(DrawCommand::FillRect { rect, color });
        self.emit_text(id, widget.foreground, list);
    }

    /// A checkbox: a state-shaded box, with an inset accent square when checked.
    fn emit_checkbox(&self, id: WidgetId, rect: Rect, checked: bool, list: &mut DrawList) {
        let box_color = match self.interaction(id) {
            Interaction::Idle => theme::CHECKBOX_BOX,
            Interaction::Hovered => theme::CHECKBOX_BOX.lighten(0.12),
            Interaction::Pressed => theme::CHECKBOX_BOX.darken(0.10),
        };
        list.commands.push(DrawCommand::FillRect {
            rect,
            color: box_color,
        });
        if checked {
            let inset = rect.size * 0.28;
            let inner = Rect::new(rect.pos + inset, (rect.size - inset * 2.0).max(Vec2::ZERO));
            list.commands.push(DrawCommand::FillRect {
                rect: inner,
                color: theme::CHECKBOX_MARK,
            });
        }
    }

    /// Emit a text command for a text-bearing widget from its shaped buffer,
    /// positioned at the widget's content-box origin.
    fn emit_text(&self, id: WidgetId, color: Color, list: &mut DrawList) {
        let Some(buffer) = self.buffers.get(id) else {
            return;
        };
        let origin = self.content_origin.get(id).copied().unwrap_or(Vec2::ZERO);
        let mut glyphs = Vec::new();
        for run in buffer.layout_runs() {
            for glyph in run.glyphs {
                // Bake the content origin and the run's baseline into the pen
                // position; the backend adds each glyph's bitmap bearing.
                let physical = glyph.physical((origin.x, origin.y + run.line_y), 1.0);
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
        // A checkbox has a fixed intrinsic size and no text to shape.
        WidgetKind::Checkbox(_) => {
            return Size {
                width: theme::CHECKBOX_SIZE,
                height: theme::CHECKBOX_SIZE,
            };
        }
        WidgetKind::Panel => return Size::ZERO,
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
    use crate::event::Event;
    use crate::input::InputEvent;
    use crate::style::Style;
    use crate::widget::WidgetKind;
    use glam::Vec2;

    /// Move the pointer to `p`, then press and release: a click at `p`.
    fn click_at(ui: &mut Ui, p: Vec2) {
        ui.handle_input(InputEvent::PointerMoved(p));
        ui.handle_input(InputEvent::PointerPressed);
        ui.handle_input(InputEvent::PointerReleased);
    }

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
    fn set_label_reshapes_on_the_next_layout() {
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().column());
        let label = ui.label(root, "x", Style::new());
        ui.layout(Vec2::new(400.0, 400.0)).unwrap();
        let narrow = ui.rect(label).unwrap().size.x;

        // Change to clearly wider text and lay out again at the SAME size. Without
        // marking the node dirty, taffy would serve the cached (narrow) measure.
        ui.set_label(label, "wide wide wide wide");
        ui.layout(Vec2::new(400.0, 400.0)).unwrap();
        let wide = ui.rect(label).unwrap().size.x;

        assert!(
            wide > narrow,
            "label should re-measure wider: {narrow} -> {wide}"
        );
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

    #[test]
    fn clicking_a_button_emits_clicked() {
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().size(200.0, 100.0));
        let button = ui.button(root, "Go", Style::new().size(80.0, 30.0));
        ui.layout(Vec2::new(200.0, 100.0)).unwrap();

        click_at(&mut ui, Vec2::new(10.0, 10.0));
        assert_eq!(ui.drain_events(), vec![Event::Clicked(button)]);
        // Events are consumed by draining.
        assert!(ui.drain_events().is_empty());
    }

    #[test]
    fn clicking_a_checkbox_toggles_it_and_emits() {
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().size(100.0, 100.0));
        let cb = ui.checkbox(root, false, Style::new());
        ui.layout(Vec2::new(100.0, 100.0)).unwrap();

        // The checkbox has an intrinsic 18x18 box at the origin.
        click_at(&mut ui, Vec2::new(9.0, 9.0));
        assert_eq!(
            ui.drain_events(),
            vec![Event::Toggled {
                id: cb,
                checked: true
            }]
        );
        assert!(matches!(ui.kind(cb), WidgetKind::Checkbox(true)));

        click_at(&mut ui, Vec2::new(9.0, 9.0));
        assert_eq!(
            ui.drain_events(),
            vec![Event::Toggled {
                id: cb,
                checked: false
            }]
        );
        assert!(matches!(ui.kind(cb), WidgetKind::Checkbox(false)));
    }

    #[test]
    fn pressing_then_releasing_off_the_widget_is_not_a_click() {
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().size(200.0, 100.0));
        ui.button(root, "Go", Style::new().size(80.0, 30.0));
        ui.layout(Vec2::new(200.0, 100.0)).unwrap();

        ui.handle_input(InputEvent::PointerMoved(Vec2::new(10.0, 10.0))); // over the button
        ui.handle_input(InputEvent::PointerPressed); // arm it
        ui.handle_input(InputEvent::PointerMoved(Vec2::new(150.0, 90.0))); // drag away
        ui.handle_input(InputEvent::PointerReleased); // release outside
        assert!(ui.drain_events().is_empty());
    }

    #[test]
    fn hit_test_is_topmost_and_a_click_bubbles_to_the_interactive_ancestor() {
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().size(200.0, 100.0));
        // A button with a non-interactive label child laid over it.
        let button = ui.button(root, "", Style::new().size(80.0, 40.0).padding(4.0));
        let label = ui.label(button, "hi", Style::new().size(40.0, 20.0));
        ui.layout(Vec2::new(200.0, 100.0)).unwrap();

        // The label (a child) is the topmost widget under the point.
        let p = Vec2::new(10.0, 10.0);
        assert_eq!(ui.hit_test(p), Some(label));
        assert_eq!(ui.hovered(), None); // not updated until we route input

        // A click there bubbles past the non-interactive label to the button.
        click_at(&mut ui, p);
        assert_eq!(ui.hovered(), Some(button));
        assert_eq!(ui.drain_events(), vec![Event::Clicked(button)]);
    }

    #[test]
    fn a_clip_hides_children_from_hit_testing() {
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().size(200.0, 200.0).clip());
        // A button wider than the clipped root; its right half overflows.
        let button = ui.button(root, "Go", Style::new().size(300.0, 30.0));
        ui.layout(Vec2::new(400.0, 400.0)).unwrap();

        // A point in the overflowed (clipped-away) region hits nothing...
        assert_eq!(ui.hit_test(Vec2::new(260.0, 10.0)), None);
        // ...while the visible part still hits the button.
        assert_eq!(ui.hit_test(Vec2::new(150.0, 10.0)), Some(button));
    }
}
