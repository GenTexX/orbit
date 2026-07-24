//! aurora ui: the widget arena, the taffy layout tree (with text measurement), and draw-list generation.

use cosmic_text::{Buffer, FontSystem, Shaping};
use glam::Vec2;
use slotmap::{SecondaryMap, SlotMap};
use taffy::prelude::length;
use taffy::{AvailableSpace, Size, TaffyTree};

use crate::color::Color;
use crate::draw::{DrawCommand, DrawList, Glyph};
use crate::error::AuroraError;
use crate::event::Event;
use crate::input::{CursorHint, InputEvent, Key};
use crate::rect::Rect;
use crate::style::Style;
use crate::widget::{Orientation, Widget, WidgetId, WidgetKind};
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
    /// Vertical scroll offset per scroll container, in pixels (0 = top).
    scroll_offsets: SecondaryMap<WidgetId, f32>,
    /// Content height per scroll container from the last layout, for clamping
    /// offsets and sizing the scrollbar thumb.
    scroll_content: SecondaryMap<WidgetId, f32>,
    /// Root-level floating widgets (menus, dropdowns, tooltips): drawn above
    /// the normal tree and hit-tested before it, in insertion order (last on
    /// top). See [`popup`](Self::popup).
    popups: Vec<WidgetId>,
    /// Last known pointer position, or `None` when the pointer is off-surface.
    cursor: Option<Vec2>,
    /// The interactive widget under the pointer (a click bubbles to it).
    hovered: Option<WidgetId>,
    /// The interactive widget a press landed on, armed until release.
    pressed: Option<WidgetId>,
    /// The focused text input, which receives keyboard input.
    focused: Option<WidgetId>,
    /// Caret position in the focused input, as a byte offset into its text.
    caret: usize,
    /// The fixed end of a selection (the caret is the moving end), or `None`
    /// for no selection. The selected range is `min..max` of the two.
    selection_anchor: Option<usize>,
    /// Whether shift is currently held (set by the app), so movement keys
    /// extend the selection rather than collapse it.
    shift: bool,
    /// Whether the caret is in its visible blink phase (driven by the app).
    caret_on: bool,
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
            scroll_offsets: SecondaryMap::new(),
            scroll_content: SecondaryMap::new(),
            popups: Vec::new(),
            cursor: None,
            hovered: None,
            pressed: None,
            focused: None,
            caret: 0,
            selection_anchor: None,
            shift: false,
            caret_on: true,
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

    /// Add a text input that accepts only numeric text (a leading `-`, digits,
    /// and a single `.`). Keystrokes and pastes that would break that are
    /// rejected - the primitive the inspector's numeric fields build on.
    pub fn numeric_input(
        &mut self,
        parent: WidgetId,
        text: impl Into<String>,
        style: Style,
    ) -> WidgetId {
        let id = self.add(WidgetKind::TextInput(text.into()), style, Some(parent));
        self.widgets[id].numeric = true;
        id
    }

    /// Add a horizontal slider under `parent`, holding `value` clamped to
    /// `[min, max]`. Dragging the track sets the value and emits
    /// [`Event::SliderChanged`].
    pub fn slider(
        &mut self,
        parent: WidgetId,
        value: f32,
        min: f32,
        max: f32,
        style: Style,
    ) -> WidgetId {
        let value = value.clamp(min.min(max), min.max(max));
        self.add(WidgetKind::Slider { value, min, max }, style, Some(parent))
    }

    /// A slider's current value (0.0 for non-sliders).
    pub fn slider_value(&self, id: WidgetId) -> f32 {
        match self.widgets[id].kind {
            WidgetKind::Slider { value, .. } => value,
            _ => 0.0,
        }
    }

    /// Add a widget under `parent` that draws a backend-registered texture
    /// (`handle`) filling its rectangle - e.g. the engine's viewport.
    pub fn image(
        &mut self,
        parent: WidgetId,
        handle: crate::draw::ImageHandle,
        style: Style,
    ) -> WidgetId {
        self.add(WidgetKind::Image(handle), style, Some(parent))
    }

    /// Add a floating container anchored at `anchor` (absolute UI-space
    /// pixels), on top of everything else: a menu, dropdown, or tooltip. It is
    /// laid out out of flow (taffy `Position::Absolute`), sized to its content
    /// or its style, drawn after the normal tree, and hit-tested before it. If
    /// it would spill off the bottom or right edge, layout nudges it back on
    /// screen. Build its contents as children like any panel.
    pub fn popup(&mut self, anchor: Vec2, style: Style) -> WidgetId {
        // A popup parents to the root but is positioned absolutely, so it
        // ignores the root's flex flow and sits where anchored.
        let root = self.root.expect("popup requires a root panel");
        let mut style = style;
        style.layout.position = taffy::Position::Absolute;
        style.layout.inset = taffy::Rect {
            left: length(anchor.x),
            top: length(anchor.y),
            right: taffy::style_helpers::auto(),
            bottom: taffy::style_helpers::auto(),
        };
        let id = self.add(WidgetKind::Panel, style, Some(root));
        self.popups.push(id);
        id
    }

    /// Whether `id` is `ancestor` or a descendant of it (walking parent links).
    /// Apps use this to tell whether a click landed inside an open popup.
    pub fn is_within(&self, id: WidgetId, ancestor: WidgetId) -> bool {
        let mut cur = Some(id);
        while let Some(w) = cur {
            if w == ancestor {
                return true;
            }
            cur = self.widgets[w].parent;
        }
        false
    }

    /// Add a draggable divider under `parent` that will resize a target widget
    /// along `orientation`'s axis - the foundation resizable panels are built
    /// from. The target is wired up afterward via
    /// [`set_splitter_target`](Self::set_splitter_target): a splitter placed
    /// before the sibling it resizes (the common case - see `sign`) cannot
    /// take that sibling's handle yet, since it does not exist until built.
    /// `sign` is `1.0` if the target will sit before this splitter in layout
    /// order (dragging right/down grows it) or `-1.0` if it will sit after
    /// (dragging right/down shrinks it) - see [`WidgetKind::Splitter`].
    /// `style` sets the bar's own thickness (`width` for a vertical splitter,
    /// `height` for a horizontal one) and appearance.
    pub fn splitter(
        &mut self,
        parent: WidgetId,
        orientation: Orientation,
        sign: f32,
        style: Style,
    ) -> WidgetId {
        self.add(
            WidgetKind::Splitter {
                orientation,
                target: None,
                sign,
            },
            style,
            Some(parent),
        )
    }

    /// Set the widget a splitter resizes when dragged. A no-op if `splitter`
    /// is not a splitter.
    pub fn set_splitter_target(&mut self, splitter: WidgetId, target: WidgetId) {
        if let WidgetKind::Splitter { target: t, .. } = &mut self.widgets[splitter].kind {
            *t = Some(target);
        }
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

    /// A widget's background color (mostly for tests and introspection).
    pub fn background(&self, id: WidgetId) -> Color {
        self.widgets[id].background
    }

    /// Replace a label's text. It is re-shaped and re-measured on the next
    /// [`layout`](Self::layout); a no-op on non-label widgets.
    pub fn set_label(&mut self, id: WidgetId, text: impl Into<String>) {
        if let WidgetKind::Label(s) = &mut self.widgets[id].kind {
            *s = text.into();
            self.invalidate_text(id);
        }
    }

    /// Replace a text input's contents (e.g. an editor populating a field). It
    /// is re-shaped on the next [`layout`](Self::layout); the caret is clamped
    /// if this field is focused. A no-op on non-input widgets.
    pub fn set_text_input(&mut self, id: WidgetId, text: impl Into<String>) {
        if let WidgetKind::TextInput(s) = &mut self.widgets[id].kind {
            *s = text.into();
            let len = s.len();
            if self.focused == Some(id) {
                self.caret = self.caret.min(len);
            }
            self.invalidate_text(id);
        }
    }

    /// Set whether the caret is in its visible blink phase. Apps that want a
    /// blinking caret drive this from a timer; the default is steady-on.
    pub fn set_caret_on(&mut self, on: bool) {
        self.caret_on = on;
    }

    /// Tell Aurora whether shift is held (the app tracks OS modifiers). While
    /// set, movement keys extend the selection instead of collapsing it.
    pub fn set_shift(&mut self, shift: bool) {
        self.shift = shift;
    }

    /// The focused input's selected text, or `None` if nothing is selected
    /// (for the app to put on the OS clipboard on copy/cut).
    pub fn selected_text(&self) -> Option<String> {
        let id = self.focused?;
        let (lo, hi) = self.selection_bounds(id);
        if lo == hi {
            return None;
        }
        match &self.widgets[id].kind {
            WidgetKind::TextInput(s) => Some(s[lo..hi].to_string()),
            _ => None,
        }
    }

    /// Select the whole focused input (ctrl+a, driven by the app).
    pub fn select_all(&mut self) {
        if let Some(id) = self.focused {
            self.selection_anchor = Some(0);
            self.caret = self.text_len(id);
        }
    }

    /// Delete the focused input's selection, if any; returns whether it did.
    /// The app uses this for cut (after reading [`selected_text`](Self::selected_text)).
    pub fn delete_selection(&mut self) -> bool {
        let Some(id) = self.focused else {
            return false;
        };
        let (lo, hi) = self.selection_bounds(id);
        if lo == hi {
            return false;
        }
        if let WidgetKind::TextInput(s) = &mut self.widgets[id].kind {
            s.replace_range(lo..hi, "");
        }
        self.caret = lo;
        self.selection_anchor = None;
        self.invalidate_text(id);
        true
    }

    /// Insert `text` into the focused input at the caret, replacing any
    /// selection (control characters are dropped). The app uses this for paste.
    pub fn insert_str(&mut self, text: &str) {
        let Some(id) = self.focused else {
            return;
        };
        if !matches!(self.widgets[id].kind, WidgetKind::TextInput(_)) {
            return;
        }
        let cleaned: String = text.chars().filter(|c| !c.is_control()).collect();
        let (lo, hi) = self.selection_bounds(id);
        // A numeric field rejects an edit that would make its text non-numeric.
        if self.widgets[id].numeric
            && let WidgetKind::TextInput(s) = &self.widgets[id].kind
        {
            let mut candidate = s.clone();
            candidate.replace_range(lo..hi, &cleaned);
            if !is_numeric_text(&candidate) {
                return;
            }
        }
        if let WidgetKind::TextInput(s) = &mut self.widgets[id].kind {
            s.replace_range(lo..hi, &cleaned);
        }
        self.caret = lo + cleaned.len();
        self.selection_anchor = None;
        self.invalidate_text(id);
    }

    /// The selection's byte range in `id`'s text (`caret..caret` if none),
    /// clamped to the current length.
    fn selection_bounds(&self, id: WidgetId) -> (usize, usize) {
        let caret = self.caret.min(self.text_len(id));
        match self.selection_anchor {
            Some(a) => {
                let a = a.min(self.text_len(id));
                (a.min(caret), a.max(caret))
            }
            None => (caret, caret),
        }
    }

    /// A scroll container's current vertical offset (0 = scrolled to the top).
    pub fn scroll_offset(&self, id: WidgetId) -> f32 {
        self.scroll_offsets.get(id).copied().unwrap_or(0.0)
    }

    /// Set a scroll container's vertical offset (e.g. restoring editor state
    /// after a rebuild). Clamped against the content during the next layout.
    pub fn set_scroll_offset(&mut self, id: WidgetId, offset: f32) {
        self.scroll_offsets.insert(id, offset.max(0.0));
    }

    /// Fix a widget's width in pixels at runtime (e.g. a splitter resizing its
    /// target), pinning the minimum too - the same contract as
    /// [`Style::width`](crate::Style::width). Reads the node's current taffy
    /// style back, mutates just the width, and writes it - other layout
    /// properties (padding, flex, height) are preserved.
    pub fn set_width(&mut self, id: WidgetId, width: f32) {
        let node = self.widgets[id].taffy;
        let mut style = self.taffy.style(node).expect("taffy style").clone();
        style.size.width = length(width);
        style.min_size.width = length(width);
        self.taffy.set_style(node, style).expect("taffy set_style");
    }

    /// Fix a widget's height in pixels at runtime; see
    /// [`set_width`](Self::set_width).
    pub fn set_height(&mut self, id: WidgetId, height: f32) {
        let node = self.widgets[id].taffy;
        let mut style = self.taffy.style(node).expect("taffy style").clone();
        style.size.height = length(height);
        style.min_size.height = length(height);
        self.taffy.set_style(node, style).expect("taffy set_style");
    }

    /// Mark a text widget's layout node dirty after its text changed. taffy
    /// caches layout per node; without this it serves the old measurement and
    /// never re-runs the measure function that re-shapes the text buffer.
    fn invalidate_text(&mut self, id: WidgetId) {
        let node = self.widgets[id].taffy;
        self.taffy.mark_dirty(node).expect("taffy mark_dirty");
    }

    /// Insert a widget, create its taffy node, and link it under `parent`. taffy
    /// calls only fail on internal misuse, so they are treated as bugs.
    fn add(&mut self, kind: WidgetKind, style: Style, parent: Option<WidgetId>) -> WidgetId {
        let mut layout = style.layout;
        // Children of a scroll container keep their natural size: flex would
        // otherwise shrink them to fit and nothing would ever overflow.
        if let Some(p) = parent
            && self.widgets[p].scroll
        {
            layout.flex_shrink = 0.0;
        }
        let node = self.taffy.new_leaf(layout).expect("taffy new_leaf");
        let id = self.widgets.insert(Widget {
            kind,
            taffy: node,
            parent,
            children: Vec::new(),
            background: style.background,
            foreground: style.foreground,
            clip: style.clip || style.scroll,
            scroll: style.scroll,
            numeric: false,
            flat: style.flat,
            hit_transparent: style.hit_transparent,
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
        profiling::scope!("layout");
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
        // Nudge any popup that overhangs the right or bottom edge back on
        // screen (a menu opened near a corner would otherwise spill off).
        for &popup in &self.popups.clone() {
            if let Some(rect) = self.rect(popup) {
                let over = (rect.max() - available).max(Vec2::ZERO);
                let under = rect.pos.min(Vec2::ZERO);
                let shift = under - over;
                if shift != Vec2::ZERO {
                    self.shift_subtree(popup, shift);
                }
            }
        }
        // Layout may have moved widgets under a stationary pointer, so refresh
        // what is hovered from the last known cursor position.
        self.update_hover();
        Ok(())
    }

    /// Translate a whole subtree's cached rects and text origins by `delta`
    /// (used to nudge an off-screen popup back into view after layout).
    fn shift_subtree(&mut self, id: WidgetId, delta: Vec2) {
        if let Some(rect) = self.rects.get_mut(id) {
            rect.pos += delta;
        }
        if let Some(origin) = self.content_origin.get_mut(id) {
            *origin += delta;
        }
        for child in self.widgets[id].children.clone() {
            self.shift_subtree(child, delta);
        }
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

        // A scroll container shifts its children up by its offset; everything
        // downstream (rects, text origins, hit-testing, drawing) follows from
        // the shifted origins automatically. The offset is clamped against the
        // content height measured from taffy's (unshifted) child layouts.
        let mut child_origin = pos;
        if self.widgets[id].scroll {
            let pad_bottom = layout.padding.bottom + layout.border.bottom;
            let mut content_bottom = 0.0f32;
            for &child in &children {
                let cl = self
                    .taffy
                    .layout(self.widgets[child].taffy)
                    .expect("taffy layout");
                content_bottom = content_bottom.max(cl.location.y + cl.size.height);
            }
            let content = content_bottom + pad_bottom;
            self.scroll_content.insert(id, content);
            let max_offset = (content - size.y).max(0.0);
            let offset = self.scroll_offset(id).clamp(0.0, max_offset);
            self.scroll_offsets.insert(id, offset);
            child_origin.y -= offset;
        }

        for child in children {
            self.accumulate(child, child_origin);
        }
    }

    /// The nearest scroll container at or above `id` in the tree.
    fn scroll_ancestor(&self, id: WidgetId) -> Option<WidgetId> {
        let mut cur = Some(id);
        while let Some(w) = cur {
            if self.widgets[w].scroll {
                return Some(w);
            }
            cur = self.widgets[w].parent;
        }
        None
    }

    /// Apply a wheel delta to the scroll container under the cursor.
    fn scroll_under_cursor(&mut self, delta: f32) {
        let Some(target) = self
            .cursor
            .and_then(|p| self.hit_test(p))
            .and_then(|id| self.scroll_ancestor(id))
        else {
            return;
        };
        let content = self.scroll_content.get(target).copied().unwrap_or(0.0);
        let height = self.rect(target).map_or(0.0, |r| r.size.y);
        let max_offset = (content - height).max(0.0);
        let offset = (self.scroll_offset(target) + delta).clamp(0.0, max_offset);
        self.scroll_offsets.insert(target, offset);
    }

    /// Feed one pointer event into the UI, updating hover/press state and
    /// possibly queuing a semantic [`Event`]. Uses the most recent
    /// [`layout`](Self::layout) for hit-testing.
    pub fn handle_input(&mut self, event: InputEvent) {
        match event {
            InputEvent::PointerMoved(p) => {
                let delta = self.cursor.map(|c| p - c).unwrap_or(Vec2::ZERO);
                self.cursor = Some(p);
                self.drag_splitter(delta);
                if let Some(id) = self.pressed {
                    self.drag_slider(id);
                }
                // Dragging within the focused input sweeps a selection: move
                // the caret (the anchor set on press stays put).
                if self.pressed == self.focused
                    && let Some(id) = self.focused
                    && matches!(self.widgets[id].kind, WidgetKind::TextInput(_))
                {
                    self.caret = self.caret_at_x(id, self.cursor);
                }
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
                self.focus_from_press();
                // A press on a slider jumps it to the cursor immediately.
                if let Some(id) = self.pressed {
                    self.drag_slider(id);
                }
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
            InputEvent::Scroll(delta) => self.scroll_under_cursor(delta),
            InputEvent::Text(ch) => self.insert_char(ch),
            InputEvent::Key(key) => self.edit_key(key),
        }
    }

    /// On a press: focus the text input under the pointer (placing the caret at
    /// the click), or clear focus when the press lands anywhere else. A field
    /// losing focus submits its current text first - clicking away commits an
    /// edit rather than silently keeping the old value.
    fn focus_from_press(&mut self) {
        let previous = self.focused;
        match self.hovered {
            Some(id) if matches!(self.widgets[id].kind, WidgetKind::TextInput(_)) => {
                self.focused = Some(id);
                self.caret = self.caret_at_x(id, self.cursor);
                // Anchor a selection at the press point; a drag before release
                // extends it, a plain click leaves anchor == caret (no
                // selection).
                self.selection_anchor = Some(self.caret);
            }
            _ => {
                self.focused = None;
                self.selection_anchor = None;
            }
        }
        if let Some(old) = previous
            && self.focused != previous
        {
            self.events.push(Event::Submitted(old));
        }
    }

    /// If `id` is a slider, set its value from the cursor's x within the track
    /// and emit `SliderChanged` (a no-op for other widgets or an off-surface
    /// cursor). Called on press and on drag.
    fn drag_slider(&mut self, id: WidgetId) {
        let WidgetKind::Slider { value, min, max } = self.widgets[id].kind else {
            return;
        };
        let (Some(cursor), Some(rect)) = (self.cursor, self.rect(id)) else {
            return;
        };
        let t = if rect.size.x > 0.0 {
            ((cursor.x - rect.pos.x) / rect.size.x).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let new = min + t * (max - min);
        if new != value {
            self.widgets[id].kind = WidgetKind::Slider {
                value: new,
                min,
                max,
            };
            self.events.push(Event::SliderChanged { id, value: new });
        }
    }

    /// If a splitter is currently pressed, resize its target by `delta` along
    /// the splitter's axis (direction per `sign`), floored at a minimum size.
    fn drag_splitter(&mut self, delta: Vec2) {
        let Some(id) = self.pressed else {
            return;
        };
        let WidgetKind::Splitter {
            orientation,
            target,
            sign,
        } = self.widgets[id].kind
        else {
            return;
        };
        let Some(target) = target else {
            return;
        };
        let Some(rect) = self.rect(target) else {
            return;
        };
        match orientation {
            Orientation::Vertical => {
                let width = (rect.size.x + delta.x * sign).max(theme::SPLITTER_MIN_TARGET);
                self.set_width(target, width);
            }
            Orientation::Horizontal => {
                let height = (rect.size.y + delta.y * sign).max(theme::SPLITTER_MIN_TARGET);
                self.set_height(target, height);
            }
        }
    }

    /// The focused text input, if any.
    pub fn focused(&self) -> Option<WidgetId> {
        self.focused
    }

    /// Insert a typed character at the caret in the focused input.
    fn insert_char(&mut self, ch: char) {
        if ch.is_control() {
            return;
        }
        // Typing over a selection replaces it; insert_str handles both cases.
        let mut buf = [0u8; 4];
        self.insert_str(ch.encode_utf8(&mut buf));
    }

    /// Apply a named editing key to the focused input.
    fn edit_key(&mut self, key: Key) {
        let Some(id) = self.focused else {
            return;
        };
        // Enter commits rather than edits, and needs no text borrow.
        if key == Key::Enter {
            self.events.push(Event::Submitted(id));
            return;
        }
        // Tab steps focus to the next (or previous, with shift) input.
        if key == Key::Tab {
            self.focus_step(if self.shift { -1 } else { 1 });
            return;
        }
        // Backspace/Delete over a selection remove it as a unit.
        if matches!(key, Key::Backspace | Key::Delete) && self.delete_selection() {
            return;
        }
        let shift = self.shift;
        let WidgetKind::TextInput(s) = &mut self.widgets[id].kind else {
            return;
        };
        let caret = self.caret.min(s.len());
        // A shift+movement extends a selection: anchor the fixed end here if
        // none yet. A plain movement collapses any selection.
        if matches!(key, Key::Left | Key::Right | Key::Home | Key::End) {
            if shift {
                self.selection_anchor.get_or_insert(caret);
            } else {
                self.selection_anchor = None;
            }
        }
        let mut edited = false;
        match key {
            Key::Left => self.caret = prev_boundary(s, caret),
            Key::Right => self.caret = next_boundary(s, caret),
            Key::Home => self.caret = 0,
            Key::End => self.caret = s.len(),
            Key::Backspace if caret > 0 => {
                let prev = prev_boundary(s, caret);
                s.replace_range(prev..caret, "");
                self.caret = prev;
                edited = true;
            }
            Key::Delete if caret < s.len() => {
                let next = next_boundary(s, caret);
                s.replace_range(caret..next, "");
                self.caret = caret;
                edited = true;
            }
            // Backspace at the start / Delete at the end: nothing to remove.
            Key::Backspace | Key::Delete => self.caret = caret,
            Key::Enter | Key::Tab => unreachable!("handled above"),
        }
        if edited {
            self.invalidate_text(id);
        }
    }

    /// The caret byte offset nearest to a cursor position (for click-to-place).
    /// Falls back to the end of the text when off-field or not yet shaped.
    fn caret_at_x(&self, id: WidgetId, cursor: Option<Vec2>) -> usize {
        let text_len = self.text_len(id);
        let (Some(cursor), Some(buffer)) = (cursor, self.buffers.get(id)) else {
            return text_len;
        };
        let origin = self.content_origin.get(id).copied().unwrap_or(Vec2::ZERO);
        let local_x = cursor.x - origin.x;
        for run in buffer.layout_runs() {
            for g in run.glyphs {
                // Snap to whichever side of the glyph's midpoint the click is on.
                if local_x < g.x + g.w * 0.5 {
                    return g.start;
                }
            }
        }
        text_len
    }

    /// The x offset (within the content box) of the caret, from the shaped text.
    fn caret_x_offset(&self, id: WidgetId) -> f32 {
        self.x_offset_at(id, self.caret)
    }

    /// The x offset (within the content box) of byte offset `byte` in `id`'s
    /// shaped text - the left edge of the glyph starting at or after `byte`,
    /// or the end of the text.
    fn x_offset_at(&self, id: WidgetId, byte: usize) -> f32 {
        let Some(buffer) = self.buffers.get(id) else {
            return 0.0;
        };
        let mut x = 0.0;
        for run in buffer.layout_runs() {
            for g in run.glyphs {
                if g.start >= byte {
                    return g.x;
                }
                x = g.x + g.w;
            }
        }
        x
    }

    /// The focusable inputs in tree (pre-order) order - the tab ring.
    fn focus_ring(&self) -> Vec<WidgetId> {
        let mut ring = Vec::new();
        if let Some(root) = self.root {
            self.collect_focusable(root, &mut ring);
        }
        ring
    }

    fn collect_focusable(&self, id: WidgetId, ring: &mut Vec<WidgetId>) {
        if matches!(self.widgets[id].kind, WidgetKind::TextInput(_)) {
            ring.push(id);
        }
        for &child in &self.widgets[id].children {
            self.collect_focusable(child, ring);
        }
    }

    /// Step focus by `dir` (+1 next, -1 previous) around the tab ring, wrapping
    /// at the ends. The field being left is submitted first (so its edit
    /// commits, exactly as clicking away does), and the newly focused field is
    /// selected whole (the tab-in convention), ready to be overtyped.
    fn focus_step(&mut self, dir: i32) {
        let ring = self.focus_ring();
        if ring.is_empty() {
            return;
        }
        let len = ring.len() as i32;
        let previous = self.focused;
        let next = match previous.and_then(|f| ring.iter().position(|&w| w == f)) {
            Some(i) => (i as i32 + dir).rem_euclid(len) as usize,
            None if dir > 0 => 0,
            None => (len - 1) as usize,
        };
        let id = ring[next];
        if let Some(old) = previous
            && old != id
        {
            self.events.push(Event::Submitted(old));
        }
        self.focused = Some(id);
        self.caret = self.text_len(id);
        self.selection_anchor = Some(0);
    }

    /// The byte length of a text input's contents (0 for other kinds).
    fn text_len(&self, id: WidgetId) -> usize {
        match &self.widgets[id].kind {
            WidgetKind::TextInput(s) => s.len(),
            _ => 0,
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

    /// What the mouse cursor should look like right now: resize arrows over
    /// (or while dragging) a splitter, a text beam over a text input, the
    /// default arrow otherwise. The app maps this to its windowing system.
    pub fn cursor_hint(&self) -> CursorHint {
        // A pressed splitter keeps its resize cursor even when the pointer
        // outruns the bar mid-drag.
        let target = self.pressed.or(self.hovered);
        match target.map(|id| &self.widgets[id].kind) {
            Some(WidgetKind::Splitter {
                orientation: Orientation::Vertical,
                ..
            }) => CursorHint::ResizeHorizontal,
            Some(WidgetKind::Splitter {
                orientation: Orientation::Horizontal,
                ..
            }) => CursorHint::ResizeVertical,
            Some(WidgetKind::TextInput(_)) => CursorHint::Text,
            _ => CursorHint::Default,
        }
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
    ///
    /// Splitters get priority with an inflated grab area: the bar is a few
    /// pixels thin, so its hit zone extends a few pixels beyond the visual on
    /// the drag axis - stealing that sliver from the neighboring panels is
    /// exactly what makes it grabbable.
    pub fn hit_test(&self, point: Vec2) -> Option<WidgetId> {
        // Popups float above everything; the last one added is topmost.
        for &popup in self.popups.iter().rev() {
            if let Some(hit) = self.hit_test_node(popup, point, None) {
                return Some(hit);
            }
        }
        for (id, widget) in &self.widgets {
            if let WidgetKind::Splitter { orientation, .. } = widget.kind
                && let Some(rect) = self.rect(id)
                && inflate_splitter(rect, orientation).contains(point)
            {
                return Some(id);
            }
            // A slider's thumb overhangs each end by half its height; extend
            // the grab area so the overhanging part stays clickable.
            if let WidgetKind::Slider { .. } = widget.kind
                && let Some(rect) = self.rect(id)
            {
                let half = rect.size.y * 0.5;
                let grab = Rect::new(
                    rect.pos - Vec2::new(half, 0.0),
                    rect.size + Vec2::new(half * 2.0, 0.0),
                );
                if grab.contains(point) {
                    return Some(id);
                }
            }
        }
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
        // A hit-transparent widget (a decorative overlay) never claims the hit
        // itself - the cursor passes through to whatever is behind it.
        if self.widgets[id].hit_transparent {
            return None;
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
        profiling::scope!("draw_list");
        let mut list = DrawList::default();
        if let Some(root) = self.root {
            self.emit(root, &mut list);
        }
        // Popups draw last, so they sit above the whole normal tree.
        for &popup in &self.popups {
            self.emit(popup, &mut list);
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
            WidgetKind::Label(_) => {
                self.fill_background(rect, widget.background, list);
                self.emit_text(id, widget.foreground, list);
            }
            WidgetKind::Button(_) => self.emit_button(id, rect, widget, list),
            WidgetKind::Checkbox(checked) => self.emit_checkbox(id, rect, *checked, list),
            WidgetKind::TextInput(_) => self.emit_text_input(id, rect, widget, list),
            WidgetKind::Image(handle) => list.commands.push(DrawCommand::Image {
                rect,
                handle: *handle,
            }),
            WidgetKind::Slider { value, min, max } => {
                self.emit_slider(id, rect, *value, *min, *max, list)
            }
            WidgetKind::Splitter { .. } => self.emit_splitter(id, rect, widget, list),
        }

        // Children, optionally clipped to this widget's rectangle. Popup
        // subtrees are skipped here and drawn on top by `draw_list`.
        if widget.clip {
            list.commands.push(DrawCommand::PushClip { rect });
        }
        for &child in &widget.children {
            if self.popups.contains(&child) {
                continue;
            }
            self.emit(child, list);
        }
        if widget.clip {
            list.commands.push(DrawCommand::PopClip);
        }

        // A scrollbar thumb over scrolling content: its length mirrors the
        // visible fraction, its position the scrolled fraction.
        if widget.scroll
            && let Some(&content) = self.scroll_content.get(id)
            && content > rect.size.y
        {
            let track = rect.size.y - 2.0 * theme::SCROLLBAR_INSET;
            let thumb_h = (track * rect.size.y / content).max(theme::SCROLLBAR_MIN_THUMB);
            let max_offset = content - rect.size.y;
            let t = (self.scroll_offset(id) / max_offset).clamp(0.0, 1.0);
            let thumb_y = rect.pos.y + theme::SCROLLBAR_INSET + t * (track - thumb_h);
            list.commands.push(DrawCommand::FillRect {
                rect: Rect::new(
                    Vec2::new(
                        rect.max().x - theme::SCROLLBAR_INSET - theme::SCROLLBAR_WIDTH,
                        thumb_y,
                    ),
                    Vec2::new(theme::SCROLLBAR_WIDTH, thumb_h),
                ),
                color: theme::SCROLLBAR_THUMB,
            });
        }
    }

    /// A slider: a track, a filled portion up to the value, and a thumb over
    /// it. The thumb hover/press-shades like a button.
    fn emit_slider(
        &self,
        id: WidgetId,
        rect: Rect,
        value: f32,
        min: f32,
        max: f32,
        list: &mut DrawList,
    ) {
        // A thin track centered vertically.
        let track_h = 4.0;
        let track = Rect::new(
            Vec2::new(rect.pos.x, rect.pos.y + (rect.size.y - track_h) * 0.5),
            Vec2::new(rect.size.x, track_h),
        );
        list.commands.push(DrawCommand::FillRect {
            rect: track,
            color: theme::SLIDER_TRACK,
        });
        let t = if max > min {
            ((value - min) / (max - min)).clamp(0.0, 1.0)
        } else {
            0.0
        };
        // The filled portion.
        list.commands.push(DrawCommand::FillRect {
            rect: Rect::new(track.pos, Vec2::new(track.size.x * t, track_h)),
            color: theme::SLIDER_FILL,
        });
        // The thumb: a small square centered on the value position.
        let half = rect.size.y * 0.5;
        let cx = rect.pos.x + track.size.x * t;
        let base = theme::SLIDER_FILL;
        let color = match self.interaction(id) {
            Interaction::Idle => base,
            Interaction::Hovered => base.lighten(0.10),
            Interaction::Pressed => base.darken(0.10),
        };
        list.commands.push(DrawCommand::FillRect {
            rect: Rect::new(
                Vec2::new(cx - half, rect.pos.y),
                Vec2::new(half * 2.0, rect.size.y),
            ),
            color,
        });
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
        if widget.flat {
            // A flat button (list/tree row): its own background when idle
            // (transparent draws nothing). A row with a background (selected)
            // keeps it and just brightens on hover; a plain row gets a subtle
            // overlay, so nothing reads as a raised button.
            let bg = widget.background;
            let color = match self.interaction(id) {
                Interaction::Idle => bg,
                Interaction::Hovered if bg.a > 0.0 => bg.lighten(0.06),
                Interaction::Hovered => theme::ROW_HOVER,
                Interaction::Pressed if bg.a > 0.0 => bg.darken(0.06),
                Interaction::Pressed => theme::ROW_PRESSED,
            };
            self.fill_background(rect, color, list);
            self.emit_text(id, widget.foreground, list);
            return;
        }
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

    /// A splitter: a state-shaded bar (its style background, or the theme
    /// default), so it reads as grabbable without needing a resize cursor.
    fn emit_splitter(&self, id: WidgetId, rect: Rect, widget: &Widget, list: &mut DrawList) {
        let base = if widget.background.a > 0.0 {
            widget.background
        } else {
            theme::SPLITTER
        };
        let color = match self.interaction(id) {
            Interaction::Idle => base,
            Interaction::Hovered => base.lighten(0.10),
            Interaction::Pressed => base.lighten(0.20),
        };
        list.commands.push(DrawCommand::FillRect { rect, color });
    }

    /// A text input: a field fill (with an accent frame when focused), its text
    /// clipped to the field, and a caret when focused.
    fn emit_text_input(&self, id: WidgetId, rect: Rect, widget: &Widget, list: &mut DrawList) {
        let focused = self.focused == Some(id);
        let fill = if widget.background.a > 0.0 {
            widget.background
        } else {
            theme::FIELD
        };
        if focused {
            // An accent frame: an outer accent rect with the fill inset inside it.
            let border = 1.5;
            list.commands.push(DrawCommand::FillRect {
                rect,
                color: theme::FOCUS,
            });
            let inner = Rect::new(
                rect.pos + Vec2::splat(border),
                (rect.size - Vec2::splat(2.0 * border)).max(Vec2::ZERO),
            );
            list.commands.push(DrawCommand::FillRect {
                rect: inner,
                color: fill,
            });
        } else {
            list.commands
                .push(DrawCommand::FillRect { rect, color: fill });
        }

        // Clip the text, selection, and caret so long content stays in-field.
        list.commands.push(DrawCommand::PushClip { rect });
        // The selection highlight, behind the text.
        if focused {
            let (lo, hi) = self.selection_bounds(id);
            if lo != hi {
                let origin = self.content_origin.get(id).copied().unwrap_or(rect.pos);
                let x0 = origin.x + self.x_offset_at(id, lo);
                let x1 = origin.x + self.x_offset_at(id, hi);
                list.commands.push(DrawCommand::FillRect {
                    rect: Rect::new(
                        Vec2::new(x0, rect.pos.y + 3.0),
                        Vec2::new((x1 - x0).max(0.0), (rect.size.y - 6.0).max(0.0)),
                    ),
                    color: theme::SELECTION,
                });
            }
        }
        self.emit_text(id, widget.foreground, list);
        if focused && self.caret_on {
            let origin = self.content_origin.get(id).copied().unwrap_or(rect.pos);
            let x = origin.x + self.caret_x_offset(id);
            let caret = Rect::new(
                Vec2::new(x, rect.pos.y + 3.0),
                Vec2::new(1.5, (rect.size.y - 6.0).max(0.0)),
            );
            list.commands.push(DrawCommand::FillRect {
                rect: caret,
                color: theme::CARET,
            });
        }
        list.commands.push(DrawCommand::PopClip);
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

/// A splitter's grab rectangle: its visual rect expanded along the drag axis
/// (the thin one) by [`theme::SPLITTER_GRAB_EXTRA`] per side.
fn inflate_splitter(rect: Rect, orientation: Orientation) -> Rect {
    let extra = theme::SPLITTER_GRAB_EXTRA;
    match orientation {
        Orientation::Vertical => Rect::new(
            rect.pos - Vec2::new(extra, 0.0),
            rect.size + Vec2::new(extra * 2.0, 0.0),
        ),
        Orientation::Horizontal => Rect::new(
            rect.pos - Vec2::new(0.0, extra),
            rect.size + Vec2::new(0.0, extra * 2.0),
        ),
    }
}

/// Whether `s` is valid as-you-type numeric text: an optional leading `-`,
/// digits, and at most one `.`. Intermediate states like ``, `-`, `1.`, `.5`
/// are allowed so a value can be typed; a full parse happens on commit.
fn is_numeric_text(s: &str) -> bool {
    let body = s.strip_prefix('-').unwrap_or(s);
    let mut dots = 0;
    for c in body.chars() {
        match c {
            '.' => {
                dots += 1;
                if dots > 1 {
                    return false;
                }
            }
            _ if c.is_ascii_digit() => {}
            _ => return false,
        }
    }
    true
}

/// The byte offset of the character boundary just before `i` (or 0). Keeps the
/// caret on `char` boundaries so `String` edits never split a UTF-8 sequence.
fn prev_boundary(s: &str, i: usize) -> usize {
    s[..i].chars().next_back().map_or(0, |c| i - c.len_utf8())
}

/// The byte offset of the character boundary just after `i` (or `i` at the end).
fn next_boundary(s: &str, i: usize) -> usize {
    s[i..].chars().next().map_or(i, |c| i + c.len_utf8())
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
        // Panels, images, sliders, and splitters have no intrinsic content
        // size; they take their size from style (fixed size, grow, or fill).
        WidgetKind::Panel
        | WidgetKind::Image(_)
        | WidgetKind::Slider { .. }
        | WidgetKind::Splitter { .. } => {
            return Size::ZERO;
        }
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
    use crate::input::{InputEvent, Key};
    use crate::rect::Rect;
    use crate::style::Style;
    use crate::widget::{Orientation, WidgetKind};
    use glam::Vec2;

    /// Move the pointer to `p`, then press and release: a click at `p`.
    fn click_at(ui: &mut Ui, p: Vec2) {
        ui.handle_input(InputEvent::PointerMoved(p));
        ui.handle_input(InputEvent::PointerPressed);
        ui.handle_input(InputEvent::PointerReleased);
    }

    /// The contents of a text input (panics on other kinds).
    fn text_of(ui: &Ui, id: crate::WidgetId) -> &str {
        match ui.kind(id) {
            WidgetKind::TextInput(s) => s,
            other => panic!("not a text input: {other:?}"),
        }
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
    fn width_fixes_one_axis_and_leaves_the_other_to_stretch() {
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().row().size(400.0, 300.0));
        let side = ui.panel(root, Style::new().width(100.0));
        ui.layout(Vec2::new(400.0, 300.0)).unwrap();

        let r = ui.rect(side).unwrap();
        assert_eq!(r.size.x, 100.0, "width is fixed");
        assert_eq!(r.size.y, 300.0, "auto height stretches to the row");
    }

    #[test]
    fn a_fixed_panel_and_a_growing_sibling_dock_side_by_side() {
        // The inspector layout: a fixed-width side panel next to a viewport that
        // fills the rest, both full height.
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().row().size(400.0, 300.0));
        let side = ui.panel(root, Style::new().width(100.0));
        let main = ui.panel(root, Style::new().grow(1.0));
        ui.layout(Vec2::new(400.0, 300.0)).unwrap();

        assert_eq!(ui.rect(side).unwrap().size, Vec2::new(100.0, 300.0));
        let m = ui.rect(main).unwrap();
        assert_eq!(m.pos, Vec2::new(100.0, 0.0));
        assert_eq!(m.size, Vec2::new(300.0, 300.0));
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

    /// A field whose contents can be edited, focused by a click.
    fn ui_with_field(text: &str) -> (Ui, crate::WidgetId) {
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().size(300.0, 100.0));
        let field = ui.text_input(root, text, Style::new().size(200.0, 24.0));
        ui.layout(Vec2::new(300.0, 100.0)).unwrap();
        (ui, field)
    }

    #[test]
    fn clicking_a_text_input_focuses_it() {
        let (mut ui, field) = ui_with_field("hi");
        assert_eq!(ui.focused(), None);
        click_at(&mut ui, Vec2::new(20.0, 12.0));
        assert_eq!(ui.focused(), Some(field));
    }

    #[test]
    fn typing_into_a_focused_input_inserts_at_the_caret() {
        let (mut ui, field) = ui_with_field("ab");
        click_at(&mut ui, Vec2::new(180.0, 12.0)); // click past the text: caret at end

        ui.handle_input(InputEvent::Text('c'));
        assert_eq!(text_of(&ui, field), "abc");
        assert_eq!(ui.caret, 3);
    }

    #[test]
    fn dragging_a_slider_sets_its_value_and_emits() {
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().size(200.0, 40.0));
        let s = ui.slider(root, 0.0, 0.0, 100.0, Style::new().size(100.0, 20.0));
        ui.layout(Vec2::new(200.0, 40.0)).unwrap();

        // The slider spans x 0..100; a press at x=25 sets the value to 25.
        ui.handle_input(InputEvent::PointerMoved(Vec2::new(25.0, 10.0)));
        ui.handle_input(InputEvent::PointerPressed);
        assert!((ui.slider_value(s) - 25.0).abs() < 1e-3);
        assert_eq!(
            ui.drain_events(),
            vec![Event::SliderChanged { id: s, value: 25.0 }]
        );

        // Dragging updates it; past the end clamps to the max.
        ui.handle_input(InputEvent::PointerMoved(Vec2::new(90.0, 10.0)));
        assert!((ui.slider_value(s) - 90.0).abs() < 1e-3);
        ui.handle_input(InputEvent::PointerMoved(Vec2::new(500.0, 10.0)));
        assert!((ui.slider_value(s) - 100.0).abs() < 1e-3);
        ui.handle_input(InputEvent::PointerReleased);
    }

    #[test]
    fn a_sliders_overhanging_thumb_is_still_clickable() {
        // The thumb overhangs each end by half the slider height; that region
        // is outside the widget's rect but must still hit the slider (the
        // annoyance Philip reported).
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().size(200.0, 40.0));
        let s = ui.slider(root, 100.0, 0.0, 100.0, Style::new().size(100.0, 20.0));
        ui.layout(Vec2::new(200.0, 40.0)).unwrap();

        // 5px past the right edge (within the 10px overhang) still hits it.
        assert_eq!(ui.hit_test(Vec2::new(105.0, 10.0)), Some(s));
        // Well past the overhang, it does not.
        assert_ne!(ui.hit_test(Vec2::new(130.0, 10.0)), Some(s));
    }

    #[test]
    fn a_numeric_input_rejects_non_numeric_edits() {
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().size(200.0, 60.0));
        let field = ui.numeric_input(root, "", Style::new().size(180.0, 24.0));
        ui.layout(Vec2::new(200.0, 60.0)).unwrap();
        click_at(&mut ui, Vec2::new(10.0, 12.0));

        for ch in "-12.5".chars() {
            ui.handle_input(InputEvent::Text(ch));
        }
        assert_eq!(text_of(&ui, field), "-12.5");
        // A letter and a second dot are both rejected.
        ui.handle_input(InputEvent::Text('a'));
        ui.handle_input(InputEvent::Text('.'));
        assert_eq!(text_of(&ui, field), "-12.5");
        // Pasting a non-numeric string is rejected wholesale.
        ui.insert_str("abc");
        assert_eq!(text_of(&ui, field), "-12.5");
    }

    #[test]
    fn tab_moves_focus_around_the_ring_and_selects_the_field() {
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().column().size(200.0, 200.0));
        let a = ui.text_input(root, "aaa", Style::new().size(200.0, 24.0));
        let b = ui.text_input(root, "bbb", Style::new().size(200.0, 24.0));
        let c = ui.text_input(root, "ccc", Style::new().size(200.0, 24.0));
        ui.layout(Vec2::new(200.0, 200.0)).unwrap();

        click_at(&mut ui, Vec2::new(10.0, 12.0)); // focus the first
        assert_eq!(ui.focused(), Some(a));
        ui.drain_events();

        ui.handle_input(InputEvent::Key(Key::Tab));
        assert_eq!(ui.focused(), Some(b));
        // Leaving `a` submits it (so its edit commits, like clicking away).
        assert_eq!(ui.drain_events(), vec![Event::Submitted(a)]);
        // Tab-in selects the whole field, ready to overtype.
        assert_eq!(ui.selected_text().as_deref(), Some("bbb"));

        ui.handle_input(InputEvent::Key(Key::Tab));
        assert_eq!(ui.focused(), Some(c));
        // Wraps around to the first.
        ui.handle_input(InputEvent::Key(Key::Tab));
        assert_eq!(ui.focused(), Some(a));

        // Shift+Tab steps backwards (wrapping to the last).
        ui.set_shift(true);
        ui.handle_input(InputEvent::Key(Key::Tab));
        assert_eq!(ui.focused(), Some(c));
    }

    #[test]
    fn shift_arrows_extend_a_selection_and_typing_replaces_it() {
        let (mut ui, field) = ui_with_field("hello");
        click_at(&mut ui, Vec2::new(180.0, 12.0)); // caret at end (5)
        assert_eq!(ui.selected_text(), None);

        // Shift+Left twice selects the last two chars ("lo").
        ui.set_shift(true);
        ui.handle_input(InputEvent::Key(Key::Left));
        ui.handle_input(InputEvent::Key(Key::Left));
        assert_eq!(ui.selected_text().as_deref(), Some("lo"));

        // Typing replaces the selection.
        ui.set_shift(false);
        ui.handle_input(InputEvent::Text('p'));
        assert_eq!(text_of(&ui, field), "help");
        assert_eq!(ui.selected_text(), None);
    }

    #[test]
    fn select_all_then_delete_clears_the_field() {
        let (mut ui, field) = ui_with_field("abcde");
        click_at(&mut ui, Vec2::new(20.0, 12.0));
        ui.select_all();
        assert_eq!(ui.selected_text().as_deref(), Some("abcde"));
        ui.handle_input(InputEvent::Key(Key::Backspace));
        assert_eq!(text_of(&ui, field), "");
    }

    #[test]
    fn a_mouse_drag_sweeps_a_selection() {
        let (mut ui, _field) = ui_with_field("hello");
        // Press near the start, drag to the right, release: a selection spans
        // the swept text.
        ui.handle_input(InputEvent::PointerMoved(Vec2::new(2.0, 12.0)));
        ui.handle_input(InputEvent::PointerPressed);
        ui.handle_input(InputEvent::PointerMoved(Vec2::new(180.0, 12.0)));
        assert_eq!(ui.selected_text().as_deref(), Some("hello"));
        ui.handle_input(InputEvent::PointerReleased);
        assert_eq!(ui.selected_text().as_deref(), Some("hello"));
    }

    #[test]
    fn insert_str_pastes_over_a_selection() {
        let (mut ui, field) = ui_with_field("hello");
        click_at(&mut ui, Vec2::new(20.0, 12.0));
        ui.select_all();
        ui.insert_str("world");
        assert_eq!(text_of(&ui, field), "world");
        // Control characters in pasted text are dropped.
        ui.insert_str("\n!");
        assert_eq!(text_of(&ui, field), "world!");
    }

    #[test]
    fn typing_is_ignored_without_focus() {
        let (mut ui, field) = ui_with_field("ab");
        ui.handle_input(InputEvent::Text('z'));
        assert_eq!(text_of(&ui, field), "ab");
    }

    #[test]
    fn backspace_deletes_before_the_caret_delete_after() {
        let (mut ui, field) = ui_with_field("abc");
        click_at(&mut ui, Vec2::new(180.0, 12.0)); // caret at end (3)

        ui.handle_input(InputEvent::Key(Key::Backspace));
        assert_eq!(text_of(&ui, field), "ab");
        assert_eq!(ui.caret, 2);

        ui.handle_input(InputEvent::Key(Key::Home));
        ui.handle_input(InputEvent::Key(Key::Delete));
        assert_eq!(text_of(&ui, field), "b");
        assert_eq!(ui.caret, 0);
    }

    #[test]
    fn home_then_typing_inserts_at_the_start() {
        let (mut ui, field) = ui_with_field("bc");
        click_at(&mut ui, Vec2::new(180.0, 12.0)); // caret at end

        ui.handle_input(InputEvent::Key(Key::Home));
        ui.handle_input(InputEvent::Text('a'));
        assert_eq!(text_of(&ui, field), "abc");
        assert_eq!(ui.caret, 1);
    }

    #[test]
    fn click_position_places_the_caret() {
        let (mut ui, _field) = ui_with_field("abcdef");
        // Far right of the text: caret at the end.
        click_at(&mut ui, Vec2::new(199.0, 12.0));
        assert_eq!(ui.caret, "abcdef".len());
        // Far left: caret at the start.
        click_at(&mut ui, Vec2::new(0.5, 12.0));
        assert_eq!(ui.caret, 0);
    }

    #[test]
    fn clicking_a_button_clears_field_focus() {
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().row().gap(10.0).size(300.0, 100.0));
        let field = ui.text_input(root, "hi", Style::new().size(100.0, 24.0));
        let button = ui.button(root, "B", Style::new().size(40.0, 24.0));
        ui.layout(Vec2::new(300.0, 100.0)).unwrap();

        click_at(&mut ui, Vec2::new(20.0, 12.0)); // focus the field
        assert_eq!(ui.focused(), Some(field));

        // The button sits at x = 100 (field) + 10 (gap) = 110. Clicking it
        // blurs the field, which submits it (commit-on-focus-loss) before the
        // button's own click fires.
        click_at(&mut ui, Vec2::new(120.0, 12.0));
        assert_eq!(ui.focused(), None);
        assert_eq!(
            ui.drain_events(),
            vec![Event::Submitted(field), Event::Clicked(button)]
        );
    }

    #[test]
    fn an_image_widget_fills_its_rect_and_emits_its_handle() {
        // A backend mints ImageHandle values via its own registry; a bare
        // SlotMap stands in for that here since Ui never constructs one itself.
        let mut registry: slotmap::SlotMap<crate::draw::ImageHandle, ()> =
            slotmap::SlotMap::with_key();
        let handle = registry.insert(());

        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().size(200.0, 100.0));
        let viewport = ui.image(root, handle, Style::new().size(200.0, 100.0));
        ui.layout(Vec2::new(200.0, 100.0)).unwrap();

        assert_eq!(
            ui.rect(viewport),
            Some(Rect::new(Vec2::ZERO, Vec2::new(200.0, 100.0)))
        );
        let cmds = ui.draw_list().commands;
        assert_eq!(
            cmds,
            vec![DrawCommand::Image {
                rect: ui.rect(viewport).unwrap(),
                handle,
            }]
        );
    }

    #[test]
    fn dragging_a_splitter_resizes_its_target_and_clamps_at_the_minimum() {
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().row().size(300.0, 100.0));
        let left = ui.panel(root, Style::new().width(100.0));
        // sign = 1.0: left sits BEFORE this splitter, so dragging right grows it.
        let splitter = ui.splitter(root, Orientation::Vertical, 1.0, Style::new().width(6.0));
        ui.set_splitter_target(splitter, left);
        ui.layout(Vec2::new(300.0, 100.0)).unwrap();
        assert_eq!(ui.rect(left).unwrap().size.x, 100.0);
        let bar = ui.rect(splitter).unwrap(); // sits right after `left`, at x=100

        // Press on the bar and drag it 30px right: left should grow to 130.
        ui.handle_input(InputEvent::PointerMoved(bar.pos + Vec2::new(2.0, 2.0)));
        ui.handle_input(InputEvent::PointerPressed);
        ui.handle_input(InputEvent::PointerMoved(bar.pos + Vec2::new(32.0, 2.0)));
        ui.handle_input(InputEvent::PointerReleased);
        ui.layout(Vec2::new(300.0, 100.0)).unwrap();
        assert_eq!(ui.rect(left).unwrap().size.x, 130.0);

        // Dragging far left clamps at the theme minimum rather than going
        // negative or to zero.
        ui.handle_input(InputEvent::PointerMoved(ui.rect(splitter).unwrap().pos));
        ui.handle_input(InputEvent::PointerPressed);
        ui.handle_input(InputEvent::PointerMoved(Vec2::new(-500.0, 2.0)));
        ui.handle_input(InputEvent::PointerReleased);
        ui.layout(Vec2::new(300.0, 100.0)).unwrap();
        assert_eq!(
            ui.rect(left).unwrap().size.x,
            crate::theme::SPLITTER_MIN_TARGET
        );
    }

    #[test]
    fn a_negative_sign_splitter_shrinks_its_target_when_dragged_toward_it() {
        // Mirrors a right-docked panel: the splitter sits BEFORE the panel it
        // resizes, so dragging right (toward the panel) shrinks it.
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().row().size(300.0, 100.0));
        let splitter = ui.splitter(root, Orientation::Vertical, -1.0, Style::new().width(6.0));
        let right = ui.panel(root, Style::new().width(150.0));
        ui.set_splitter_target(splitter, right);
        ui.layout(Vec2::new(300.0, 100.0)).unwrap();
        let bar = ui.rect(splitter).unwrap();

        ui.handle_input(InputEvent::PointerMoved(bar.pos + Vec2::new(2.0, 2.0)));
        ui.handle_input(InputEvent::PointerPressed);
        ui.handle_input(InputEvent::PointerMoved(bar.pos + Vec2::new(22.0, 2.0)));
        ui.handle_input(InputEvent::PointerReleased);
        ui.layout(Vec2::new(300.0, 100.0)).unwrap();
        assert_eq!(ui.rect(right).unwrap().size.x, 130.0);
    }

    #[test]
    fn a_hit_transparent_popup_lets_the_cursor_through() {
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().fill());
        let under = ui.button(root, "under", Style::new().fill());
        // A hit-transparent overlay covering the button (e.g. an insertion line).
        ui.popup(
            Vec2::new(0.0, 0.0),
            Style::new().size(400.0, 300.0).hit_transparent(),
        );
        ui.layout(Vec2::new(400.0, 300.0)).unwrap();
        // The cursor passes through the overlay to the button beneath it.
        assert_eq!(ui.hit_test(Vec2::new(200.0, 150.0)), Some(under));
    }

    #[test]
    fn a_popup_draws_on_top_hit_tests_first_and_clamps_on_screen() {
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().fill());
        // A full-window button underneath.
        let under = ui.button(root, "under", Style::new().fill());
        // A popup covering part of it.
        let menu = ui.popup(Vec2::new(100.0, 100.0), Style::new().size(80.0, 60.0));
        let item = ui.button(menu, "item", Style::new().fill());
        ui.layout(Vec2::new(400.0, 300.0)).unwrap();

        // A point inside the popup hits the popup's item, not the button below.
        assert_eq!(ui.hit_test(Vec2::new(120.0, 120.0)), Some(item));
        // A point outside the popup falls through to the underlying button.
        assert_eq!(ui.hit_test(Vec2::new(300.0, 250.0)), Some(under));

        // The popup draws after (on top of) the underlying button's fill.
        let cmds = ui.draw_list().commands;
        let is_menu_fill = |c: &DrawCommand| {
            matches!(c, DrawCommand::FillRect { rect, .. }
                if rect.pos == Vec2::new(100.0, 100.0))
        };
        let is_under_fill = |c: &DrawCommand| matches!(c, DrawCommand::FillRect { rect, .. } if rect.size == Vec2::new(400.0, 300.0));
        let menu_at = cmds.iter().position(is_menu_fill).unwrap();
        let under_at = cmds.iter().position(is_under_fill).unwrap();
        assert!(menu_at > under_at, "popup must draw after the tree");
    }

    #[test]
    fn a_popup_near_the_edge_is_nudged_back_on_screen() {
        let mut ui = Ui::new();
        ui.root_panel(Style::new().fill());
        // Anchored so an 80x60 popup would spill past the 400x300 window.
        let menu = ui.popup(Vec2::new(390.0, 290.0), Style::new().size(80.0, 60.0));
        ui.layout(Vec2::new(400.0, 300.0)).unwrap();

        let rect = ui.rect(menu).unwrap();
        // Pushed fully inside: right edge at 400, bottom at 300.
        assert_eq!(rect.max(), Vec2::new(400.0, 300.0));
        assert!(ui.is_within(menu, menu));
    }

    #[test]
    fn a_scroll_container_shifts_clamps_and_marks_its_content() {
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().size(200.0, 400.0));
        // A 100px-tall scroll container holding 300px of content.
        let scroller = ui.panel(root, Style::new().column().size(200.0, 100.0).scroll());
        let first = ui.panel(scroller, Style::new().size(200.0, 150.0));
        let second = ui.panel(scroller, Style::new().size(200.0, 150.0));
        ui.layout(Vec2::new(200.0, 400.0)).unwrap();

        // Children keep their natural 150px height (no flex squashing).
        assert_eq!(ui.rect(first).unwrap().size.y, 150.0);
        assert_eq!(ui.rect(first).unwrap().pos.y, 0.0);
        assert_eq!(ui.rect(second).unwrap().pos.y, 150.0);

        // Wheel down 60px over the container: children shift up by 60.
        ui.handle_input(InputEvent::PointerMoved(Vec2::new(100.0, 50.0)));
        ui.handle_input(InputEvent::Scroll(60.0));
        ui.layout(Vec2::new(200.0, 400.0)).unwrap();
        assert_eq!(ui.scroll_offset(scroller), 60.0);
        assert_eq!(ui.rect(first).unwrap().pos.y, -60.0);

        // Scrolling far past the end clamps at content - height = 200.
        ui.handle_input(InputEvent::Scroll(10_000.0));
        ui.layout(Vec2::new(200.0, 400.0)).unwrap();
        assert_eq!(ui.scroll_offset(scroller), 200.0);
        // And back up past the start clamps at zero.
        ui.handle_input(InputEvent::Scroll(-10_000.0));
        ui.layout(Vec2::new(200.0, 400.0)).unwrap();
        assert_eq!(ui.scroll_offset(scroller), 0.0);

        // The overflow draws a scrollbar thumb (a fill on the right edge).
        ui.handle_input(InputEvent::Scroll(50.0));
        ui.layout(Vec2::new(200.0, 400.0)).unwrap();
        let thumbs = ui
            .draw_list()
            .commands
            .iter()
            .filter(|c| {
                matches!(c, DrawCommand::FillRect { rect, .. }
                    if rect.pos.x > 190.0 && rect.size.x < 10.0)
            })
            .count();
        assert_eq!(thumbs, 1);
    }

    #[test]
    fn a_scroll_containers_height_comes_from_its_parent_not_its_content() {
        // The atlas bug this pins: an auto-height scroll container inside a
        // fixed parent must stretch to the parent, not balloon to its
        // unshrinkable content (min-height auto would use min-content without
        // Overflow::Hidden).
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().column().size(200.0, 300.0));
        let scroller = ui.panel(root, Style::new().column().grow(1.0).scroll());
        ui.panel(scroller, Style::new().size(200.0, 500.0));
        ui.layout(Vec2::new(200.0, 300.0)).unwrap();

        assert_eq!(ui.rect(scroller).unwrap().size.y, 300.0);
    }

    #[test]
    fn a_scroll_container_in_a_nested_row_stays_parent_sized_too() {
        // The atlas dock shape: root column (fixed) > main row (grow) > a
        // fixed-width, auto-height scroll panel. Its height must come from
        // the row, not from its tall content (the cross-axis variant of the
        // ballooning bug).
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().column().size(800.0, 600.0));
        ui.panel(root, Style::new().height(40.0)); // a toolbar strip
        // `main` clips: that is what stops the tall content's size
        // contribution from propagating up through the auto-height row (the
        // CSS nested-flexbox min-height:0 problem, solved by overflow:hidden).
        let main = ui.panel(root, Style::new().row().grow(1.0).clip());
        let scroller = ui.panel(main, Style::new().column().width(220.0).scroll());
        ui.panel(scroller, Style::new().size(200.0, 2000.0)); // tall content
        ui.panel(main, Style::new().grow(1.0)); // the viewport area

        ui.layout(Vec2::new(800.0, 600.0)).unwrap();
        assert_eq!(ui.rect(main).unwrap().size.y, 560.0);
        assert_eq!(ui.rect(scroller).unwrap().size.y, 560.0);
    }

    #[test]
    fn scrolled_away_content_is_not_hittable_and_state_is_restorable() {
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().size(200.0, 400.0));
        let scroller = ui.panel(root, Style::new().column().size(200.0, 100.0).scroll());
        let button = ui.button(scroller, "top", Style::new().size(200.0, 40.0));
        ui.panel(scroller, Style::new().size(200.0, 260.0));
        ui.layout(Vec2::new(200.0, 400.0)).unwrap();

        // The button is hittable at the top...
        assert_eq!(ui.hit_test(Vec2::new(100.0, 20.0)), Some(button));
        // ...then scrolls out of view: the same point now hits the filler
        // panel that scrolled up into its place, and clicking there does not
        // reach the button (the clip hides it from hit-testing).
        ui.handle_input(InputEvent::PointerMoved(Vec2::new(100.0, 50.0)));
        ui.handle_input(InputEvent::Scroll(120.0));
        ui.layout(Vec2::new(200.0, 400.0)).unwrap();
        assert_ne!(ui.hit_test(Vec2::new(100.0, 20.0)), Some(button));

        // A fresh Ui (a shell rebuild) restores the offset via the accessor.
        let mut ui2 = Ui::new();
        let root2 = ui2.root_panel(Style::new().size(200.0, 400.0));
        let scroller2 = ui2.panel(root2, Style::new().column().size(200.0, 100.0).scroll());
        ui2.button(scroller2, "top", Style::new().size(200.0, 40.0));
        ui2.panel(scroller2, Style::new().size(200.0, 260.0));
        ui2.set_scroll_offset(scroller2, ui.scroll_offset(scroller));
        ui2.layout(Vec2::new(200.0, 400.0)).unwrap();
        assert_eq!(ui2.scroll_offset(scroller2), 120.0);
    }

    #[test]
    fn a_splitter_grabs_beyond_its_thin_bar() {
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().row().size(300.0, 100.0));
        let left = ui.panel(root, Style::new().width(100.0));
        let splitter = ui.splitter(root, Orientation::Vertical, 1.0, Style::new().width(4.0));
        ui.set_splitter_target(splitter, left);
        ui.panel(root, Style::new().grow(1.0)); // a later sibling beside the bar
        ui.layout(Vec2::new(300.0, 100.0)).unwrap();

        // The bar spans x 100..104; 3px past its right edge still grabs it,
        // even though that point lies inside the later-drawn sibling panel.
        assert_eq!(ui.hit_test(Vec2::new(107.0, 50.0)), Some(splitter));
        assert_eq!(ui.hit_test(Vec2::new(97.0, 50.0)), Some(splitter));
        // Well clear of the grab zone, the neighbors win again.
        assert_ne!(ui.hit_test(Vec2::new(90.0, 50.0)), Some(splitter));
    }

    #[test]
    fn cursor_hints_follow_the_hover_target() {
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().row().size(300.0, 100.0));
        let left = ui.panel(root, Style::new().width(100.0));
        let splitter = ui.splitter(root, Orientation::Vertical, 1.0, Style::new().width(4.0));
        ui.set_splitter_target(splitter, left);
        let field = ui.text_input(root, "hi", Style::new().size(80.0, 24.0));
        ui.layout(Vec2::new(300.0, 100.0)).unwrap();

        assert_eq!(ui.cursor_hint(), crate::CursorHint::Default);
        ui.handle_input(InputEvent::PointerMoved(Vec2::new(102.0, 50.0)));
        assert_eq!(ui.cursor_hint(), crate::CursorHint::ResizeHorizontal);
        ui.handle_input(InputEvent::PointerMoved(Vec2::new(140.0, 10.0)));
        assert_eq!(ui.cursor_hint(), crate::CursorHint::Text);
        let _ = field;
    }

    #[test]
    fn losing_focus_submits_the_field() {
        let (mut ui, field) = ui_with_field("typed");
        click_at(&mut ui, Vec2::new(20.0, 12.0)); // focus
        ui.drain_events();

        // Press on empty space: the field blurs and submits.
        ui.handle_input(InputEvent::PointerMoved(Vec2::new(250.0, 90.0)));
        ui.handle_input(InputEvent::PointerPressed);
        ui.handle_input(InputEvent::PointerReleased);
        assert_eq!(ui.focused(), None);
        assert_eq!(ui.drain_events(), vec![Event::Submitted(field)]);
    }

    #[test]
    fn enter_submits_without_changing_the_fields_text() {
        let (mut ui, field) = ui_with_field("hello");
        click_at(&mut ui, Vec2::new(20.0, 12.0)); // focus the field
        assert_eq!(ui.focused(), Some(field));

        ui.handle_input(InputEvent::Key(Key::Enter));
        assert_eq!(ui.drain_events(), vec![Event::Submitted(field)]);
        assert_eq!(text_of(&ui, field), "hello"); // unchanged - Enter commits, not edits
    }
}
