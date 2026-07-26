//! aurora ui: the widget arena, the taffy layout tree (with text measurement), and draw-list generation.

use cosmic_text::{Buffer, Cursor, FontSystem, Shaping};
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
use crate::theme::Theme;
use crate::widget::{Mask, Orientation, Widget, WidgetId, WidgetKind};
use crate::{text, theme};

/// The visual state of an interactive widget, derived from the pointer each
/// frame. Drives the built-in hover and pressed shading.
#[derive(Clone, Copy, PartialEq)]
enum Interaction {
    Idle,
    Hovered,
    Pressed,
}

/// An in-progress drag from a draggable widget. Armed on press over the source,
/// it goes `active` once the pointer moves past [`DRAG_THRESHOLD`] from the
/// press point (so a plain click is never a drag).
#[derive(Clone, Copy)]
struct DragState {
    source: WidgetId,
    start: Vec2,
    active: bool,
}

/// How far the pointer must move from the press point before an armed drag
/// becomes live, in pixels.
const DRAG_THRESHOLD: f32 = 4.0;

/// Opacity of the drag "ghost" (the faded copy of the dragged widget).
const DRAG_GHOST_ALPHA: f32 = 0.6;

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
    /// A shaped buffer for each placeholder-bearing text input, produced after
    /// layout and drawn (dimmed) only while the input's own text is empty.
    placeholder_buffers: SecondaryMap<WidgetId, Buffer>,
    /// A shaped check-glyph buffer per checkbox (its main buffer holds the label
    /// caption instead), produced during measure and drawn when checked.
    check_buffers: SecondaryMap<WidgetId, Buffer>,
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
    /// An armed or live drag from a draggable widget (first-class drag-and-drop).
    drag: Option<DragState>,
    /// The focused text input, which receives keyboard input.
    focused: Option<WidgetId>,
    /// Caret position in the focused input, as a byte offset into its text.
    caret: usize,
    /// Horizontal scroll (px) of the focused single-line field, so its caret
    /// stays visible as text runs past the field's width. Recomputed each layout;
    /// 0 for multiline fields (which wrap) and reset on focus change.
    focused_hscroll: f32,
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
    /// The color palette the built-in widgets draw with (swap with `set_theme`).
    theme: Theme,
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
            placeholder_buffers: SecondaryMap::new(),
            check_buffers: SecondaryMap::new(),
            scroll_offsets: SecondaryMap::new(),
            scroll_content: SecondaryMap::new(),
            popups: Vec::new(),
            cursor: None,
            hovered: None,
            pressed: None,
            drag: None,
            focused: None,
            caret: 0,
            focused_hscroll: 0.0,
            selection_anchor: None,
            shift: false,
            caret_on: true,
            events: Vec::new(),
            theme: Theme::default(),
        }
    }

    /// Replace the color theme the built-in widgets draw with. Takes effect on
    /// the next [`draw_list`](Self::draw_list); per-widget [`Style`](crate::Style)
    /// colors still override it locally.
    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
    }

    /// The current theme.
    pub fn theme(&self) -> Theme {
        self.theme
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

    /// Add a checkbox under `parent`, initially `checked` or not, with a caption
    /// `label` beside the box (pass `""` for a bare box). Clicking anywhere on
    /// the widget - box or caption - toggles it.
    pub fn checkbox(
        &mut self,
        parent: WidgetId,
        checked: bool,
        label: impl Into<String>,
        style: Style,
    ) -> WidgetId {
        self.add(
            WidgetKind::Checkbox {
                checked,
                label: label.into(),
            },
            style,
            Some(parent),
        )
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

    /// Add a multi-line text input (a text area): Enter inserts a newline and
    /// Up/Down move the caret between lines. A convenience for `text_input` with
    /// `Style::multiline()`.
    pub fn text_area(
        &mut self,
        parent: WidgetId,
        text: impl Into<String>,
        style: Style,
    ) -> WidgetId {
        self.add(
            WidgetKind::TextInput(text.into()),
            style.multiline(),
            Some(parent),
        )
    }

    /// Add a text input that accepts only numeric text (a leading `-`, digits,
    /// and a single `.`). Keystrokes and pastes that would break that are
    /// rejected - the primitive the inspector's numeric fields build on. A
    /// convenience for `text_input` with `Style::mask(Mask::Decimal)`.
    pub fn numeric_input(
        &mut self,
        parent: WidgetId,
        text: impl Into<String>,
        style: Style,
    ) -> WidgetId {
        let id = self.add(WidgetKind::TextInput(text.into()), style, Some(parent));
        // An explicit style mask wins; otherwise default this field to Decimal.
        if self.widgets[id].mask == Mask::None {
            self.widgets[id].mask = Mask::Decimal;
        }
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

    /// Seed the pointer position without synthesizing a full move. An app that
    /// rebuilds its UI (discarding the old `Ui` and its retained pointer state)
    /// calls this on the fresh `Ui` before [`layout`](Self::layout), so the
    /// end-of-layout hover refresh knows where the cursor is - otherwise hover
    /// (and click-again-without-moving) is dead until the next real move.
    pub fn set_cursor(&mut self, pos: Vec2) {
        self.cursor = Some(pos);
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
        // A masked field rejects an edit that would make its whole text stop
        // matching the mask.
        if self.widgets[id].mask != Mask::None
            && let WidgetKind::TextInput(s) = &self.widgets[id].kind
        {
            let mut candidate = s.clone();
            candidate.replace_range(lo..hi, &cleaned);
            if !self.widgets[id].mask.accepts(&candidate) {
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
            mask: style.mask,
            placeholder: style.placeholder.unwrap_or_default(),
            font_size: style.font_size.unwrap_or(text::FONT_SIZE),
            multiline: style.multiline,
            text_center: style.text_center,
            icon_button: style.icon_button,
            corner_radius: style.corner_radius,
            border_width: style.border_width,
            border_color: style.border_color,
            draggable: style.draggable,
            drop_target: style.drop_target,
            flat: style.flat,
            hit_transparent: style.hit_transparent,
            disabled: style.disabled,
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
        let check_buffers = &mut self.check_buffers;
        let font_system = &mut self.font_system;
        self.taffy.compute_layout_with_measure(
            root_node,
            Size {
                width: AvailableSpace::Definite(available.x),
                height: AvailableSpace::Definite(available.y),
            },
            |known, avail, _node, ctx, _style| {
                measure_widget(
                    known,
                    avail,
                    ctx,
                    widgets,
                    buffers,
                    check_buffers,
                    font_system,
                )
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
        // Shape placeholder text now that fields have their widths (needed to
        // wrap), so emit can draw it while a field is empty.
        self.shape_placeholders();
        // Now the focused field's width and caret x are known, so keep its caret
        // in view (scrolling long single-line content horizontally).
        self.update_hscroll();
        // Layout may have moved widgets under a stationary pointer, so refresh
        // what is hovered from the last known cursor position.
        self.update_hover();
        Ok(())
    }

    /// Shape the placeholder string of each placeholder-bearing text input into
    /// its own buffer, wrapped to the field's content width. Runs after layout,
    /// when those widths are known; the result is drawn only while the field is
    /// empty (see [`emit_text_input`](Self::emit_text_input)).
    fn shape_placeholders(&mut self) {
        // Collect the work first (text + field width), then split the buffer and
        // font-system borrows to shape - the same borrow dance as layout().
        let jobs: Vec<(WidgetId, String, f32, f32)> = self
            .widgets
            .iter()
            .filter(|(_, w)| {
                matches!(w.kind, WidgetKind::TextInput(_)) && !w.placeholder.is_empty()
            })
            .map(|(id, w)| {
                (
                    id,
                    w.placeholder.clone(),
                    self.rect(id).map_or(0.0, |r| r.size.x),
                    w.font_size,
                )
            })
            .collect();
        let buffers = &mut self.placeholder_buffers;
        let font_system = &mut self.font_system;
        for (id, text, width, font_size) in jobs {
            let metrics = text::metrics_for(font_size);
            if !buffers.contains_key(id) {
                buffers.insert(id, Buffer::new(font_system, metrics));
            }
            let mut borrowed = buffers[id].borrow_with(font_system);
            borrowed.set_metrics(metrics);
            borrowed.set_size(Some(width.max(1.0)), None);
            borrowed.set_text(&text, &text::default_attrs(), Shaping::Advanced, None);
            borrowed.shape_until_scroll(false);
        }
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
                self.update_drag(p);
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
                // Arm a drag if the press landed on (or within) a draggable.
                self.drag = self
                    .cursor
                    .and_then(|p| self.draggable_at(p))
                    .map(|source| DragState {
                        source,
                        start: self.cursor.unwrap_or(Vec2::ZERO),
                        active: false,
                    });
                self.focus_from_press();
                // A press on a slider jumps it to the cursor immediately.
                if let Some(id) = self.pressed {
                    self.drag_slider(id);
                }
            }
            InputEvent::PointerReleased => {
                // A live drag resolves to a drop (over a target, or a cancel);
                // it also suppressed the click by clearing `pressed` when it went
                // live, so the activation below only fires for a plain click.
                if let Some(drag) = self.drag.take()
                    && drag.active
                {
                    let target = self.cursor.and_then(|p| self.drop_target_at(p));
                    self.events.push(Event::Dropped {
                        source: drag.source,
                        target,
                    });
                }
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
                // A field starts unscrolled; reset before mapping the click so
                // caret_at_x reads the right (zero) scroll for a new field.
                if previous != Some(id) {
                    self.focused_hscroll = 0.0;
                }
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
        let multiline = self.widgets[id].multiline;
        // Enter inserts a newline in a text area, otherwise it commits the edit.
        if key == Key::Enter {
            if multiline {
                self.insert_newline();
            } else {
                self.events.push(Event::Submitted(id));
            }
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
        // A shift+movement extends a selection (anchoring the fixed end here if
        // none yet); a plain movement collapses it.
        if matches!(
            key,
            Key::Left
                | Key::Right
                | Key::WordLeft
                | Key::WordRight
                | Key::Up
                | Key::Down
                | Key::Home
                | Key::End
        ) {
            let caret = self.caret.min(self.text_len(id));
            if self.shift {
                self.selection_anchor.get_or_insert(caret);
            } else {
                self.selection_anchor = None;
            }
        }
        // Vertical and line-scoped moves are 2D (text areas only); Up/Down are a
        // no-op on a single-line field.
        match key {
            Key::Up if multiline => {
                self.caret = self.caret_vertical(id, -1);
                return;
            }
            Key::Down if multiline => {
                self.caret = self.caret_vertical(id, 1);
                return;
            }
            Key::Up | Key::Down => return,
            Key::Home if multiline => {
                self.caret = self.line_start(id, self.caret);
                return;
            }
            Key::End if multiline => {
                self.caret = self.line_end(id, self.caret);
                return;
            }
            _ => {}
        }
        let WidgetKind::TextInput(s) = &mut self.widgets[id].kind else {
            return;
        };
        let caret = self.caret.min(s.len());
        let mut edited = false;
        match key {
            Key::Left => self.caret = prev_boundary(s, caret),
            Key::Right => self.caret = next_boundary(s, caret),
            Key::WordLeft => self.caret = prev_word_boundary(s, caret),
            Key::WordRight => self.caret = next_word_boundary(s, caret),
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
            // Up/Down/Home/End multiline paths returned above; single-line
            // Up/Down returned; the rest are unreachable here.
            Key::Up | Key::Down | Key::Enter | Key::Tab => unreachable!("handled above"),
        }
        if edited {
            self.invalidate_text(id);
        }
    }

    /// Insert a newline at the caret of the focused text area (replacing any
    /// selection). Bypasses the control-character filter that `insert_str` and
    /// `insert_char` apply, which is what keeps a single-line field single-line.
    fn insert_newline(&mut self) {
        let Some(id) = self.focused else {
            return;
        };
        let (lo, hi) = self.selection_bounds(id);
        if let WidgetKind::TextInput(s) = &mut self.widgets[id].kind {
            s.replace_range(lo..hi, "\n");
        } else {
            return;
        }
        self.caret = lo + 1;
        self.selection_anchor = None;
        self.invalidate_text(id);
    }

    /// Move the caret one visual line up (`dir < 0`) or down: find its current
    /// pixel position, then hit-test the same x on the adjacent line's middle.
    fn caret_vertical(&self, id: WidgetId, dir: i32) -> usize {
        let WidgetKind::TextInput(s) = &self.widgets[id].kind else {
            return self.caret;
        };
        let Some(buffer) = self.buffers.get(id) else {
            return self.caret;
        };
        let caret = self.caret.min(s.len());
        let cursor = byte_to_cursor(s, caret);
        let Some((x, top)) = buffer.cursor_position(&cursor) else {
            return caret;
        };
        let line_h = text::metrics_for(self.widgets[id].font_size).line_height;
        let target_y = top + dir as f32 * line_h + line_h * 0.5;
        buffer
            .hit(x, target_y.max(0.0))
            .map_or(caret, |c| cursor_to_byte(s, c))
    }

    /// The byte offset of the start of the text line (by hard newline)
    /// containing `byte`.
    fn line_start(&self, id: WidgetId, byte: usize) -> usize {
        let WidgetKind::TextInput(s) = &self.widgets[id].kind else {
            return byte;
        };
        let b = byte.min(s.len());
        s[..b].rfind('\n').map_or(0, |i| i + 1)
    }

    /// The byte offset of the end of the text line (by hard newline) containing
    /// `byte` - just before the next newline, or the end of the text.
    fn line_end(&self, id: WidgetId, byte: usize) -> usize {
        let WidgetKind::TextInput(s) = &self.widgets[id].kind else {
            return byte;
        };
        let b = byte.min(s.len());
        b + s[b..].find('\n').unwrap_or(s.len() - b)
    }

    /// The caret byte offset nearest to a cursor position (for click-to-place).
    /// Falls back to the end of the text when off-field or not yet shaped. A
    /// text area hit-tests in 2D (line and column); a single-line field uses x.
    fn caret_at_x(&self, id: WidgetId, cursor: Option<Vec2>) -> usize {
        let text_len = self.text_len(id);
        let (Some(cursor), Some(buffer)) = (cursor, self.buffers.get(id)) else {
            return text_len;
        };
        let origin = self.content_origin.get(id).copied().unwrap_or(Vec2::ZERO);
        // Undo the field's horizontal scroll so the click maps to text coords.
        let local = cursor - origin + Vec2::new(self.hscroll_of(id), 0.0);
        if self.widgets[id].multiline {
            if let WidgetKind::TextInput(s) = &self.widgets[id].kind
                && let Some(c) = buffer.hit(local.x.max(0.0), local.y.max(0.0))
            {
                return cursor_to_byte(s, c);
            }
            return text_len;
        }
        for run in buffer.layout_runs() {
            for g in run.glyphs {
                // Snap to whichever side of the glyph's midpoint the click is on.
                if local.x < g.x + g.w * 0.5 {
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

    /// The focused single-line field's horizontal scroll: 0 for it, so ignore.
    /// Recompute [`focused_hscroll`](Self::focused_hscroll) so the caret stays in
    /// view. Called after layout, when the field's width and shaped text (hence
    /// caret x) are known. Multiline fields wrap, so they never scroll.
    fn update_hscroll(&mut self) {
        let Some(id) = self.focused else {
            self.focused_hscroll = 0.0;
            return;
        };
        if !matches!(self.widgets[id].kind, WidgetKind::TextInput(_)) || self.widgets[id].multiline
        {
            self.focused_hscroll = 0.0;
            return;
        }
        let (Some(rect), Some(origin)) = (self.rect(id), self.content_origin.get(id).copied())
        else {
            return;
        };
        let left_pad = origin.x - rect.pos.x;
        let content_w = (rect.size.x - 2.0 * left_pad).max(1.0);
        let caret_x = self.caret_x_offset(id);
        let total_w = self
            .buffers
            .get(id)
            .map(|b| b.layout_runs().map(|r| r.line_w).fold(0.0, f32::max))
            .unwrap_or(0.0);
        // Keep a little context past the caret. Scroll from the last value so the
        // text only shifts when the caret would leave the visible window.
        let margin = 6.0;
        let mut h = self.focused_hscroll;
        if caret_x - h < 0.0 {
            h = caret_x;
        } else if caret_x - h > content_w - margin {
            h = caret_x - content_w + margin;
        }
        // Never scroll past the end (so a short field shows from the start).
        let max_scroll = (total_w - content_w + margin).max(0.0);
        self.focused_hscroll = h.clamp(0.0, max_scroll);
    }

    /// The focused single-line field's current horizontal scroll (0 for other
    /// or multiline fields), applied to its text, selection, caret, and clicks.
    fn hscroll_of(&self, id: WidgetId) -> f32 {
        if self.focused == Some(id) && !self.widgets[id].multiline {
            self.focused_hscroll
        } else {
            0.0
        }
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
        if matches!(self.widgets[id].kind, WidgetKind::TextInput(_)) && !self.widgets[id].disabled {
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
        // The new field starts unscrolled; layout re-derives its scroll to keep
        // the (end-of-text) caret in view.
        self.focused_hscroll = 0.0;
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

    /// The interactive widget under `point`: a fresh hit-test bubbled to its
    /// nearest interactive ancestor. Unlike [`hovered`](Self::hovered) this reads
    /// nothing retained, so it stays correct immediately after a rebuild (which
    /// resets the cached hover) - what a drag that rebuilds every frame needs.
    pub fn interactive_at(&self, point: Vec2) -> Option<WidgetId> {
        self.hit_test(point)
            .and_then(|id| self.interactive_ancestor(id))
    }

    /// Whether a first-class drag is currently live (past the start threshold).
    pub fn is_dragging(&self) -> bool {
        self.drag.is_some_and(|d| d.active)
    }

    /// The draggable widget a live drag started on, if any (for drag feedback).
    pub fn drag_source(&self) -> Option<WidgetId> {
        self.drag.filter(|d| d.active).map(|d| d.source)
    }

    /// The drop-target widget under `point`: a hit-test bubbled to the nearest
    /// ancestor marked [`drop_target`](crate::Style::drop_target). Exposed so an
    /// app can highlight the target it is hovering during a drag.
    pub fn drop_target_at(&self, point: Vec2) -> Option<WidgetId> {
        let mut cur = self.hit_test(point);
        while let Some(w) = cur {
            if self.widgets[w].drop_target {
                return Some(w);
            }
            cur = self.widgets[w].parent;
        }
        None
    }

    /// The draggable widget under `point`: a hit-test bubbled to the nearest
    /// ancestor marked [`draggable`](crate::Style::draggable).
    fn draggable_at(&self, point: Vec2) -> Option<WidgetId> {
        let mut cur = self.hit_test(point);
        while let Some(w) = cur {
            if self.widgets[w].draggable {
                return Some(w);
            }
            cur = self.widgets[w].parent;
        }
        None
    }

    /// Advance an armed drag: once the pointer moves past the threshold from the
    /// press point, the drag goes live - it emits `DragStarted` and cancels the
    /// pending click (clearing `pressed`) so a release drops rather than clicks.
    fn update_drag(&mut self, p: Vec2) {
        if let Some(drag) = &mut self.drag
            && !drag.active
            && (p - drag.start).length() > DRAG_THRESHOLD
        {
            drag.active = true;
            let source = drag.source;
            self.pressed = None;
            self.events.push(Event::DragStarted { source });
        }
    }

    /// What the mouse cursor should look like right now: resize arrows over
    /// (or while dragging) a splitter, a text beam over a text input, the
    /// default arrow otherwise. The app maps this to its windowing system.
    pub fn cursor_hint(&self) -> CursorHint {
        // A live first-class drag shows the grabbing hand, over everything else.
        if self.is_dragging() {
            return CursorHint::Grabbing;
        }
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
            // A disabled splitter/slider is inert; skip its extended grab zone so
            // it neither drags nor steals the hit from anything behind it.
            if widget.disabled {
                continue;
            }
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
            // A disabled widget is inert: it is never the interactive target, so
            // hover/press/focus skip past it (to a non-disabled ancestor, if any).
            if self.widgets[w].kind.is_interactive() && !self.widgets[w].disabled {
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
            WidgetKind::Checkbox { checked, .. } => {
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
    /// is both armed on it and still over it. A widget also counts as hovered
    /// when the pointer is over one of its interactive children (e.g. a tree row
    /// stays highlighted while the cursor is on its eye toggle), so moving onto a
    /// nested control does not flicker the parent's highlight off.
    fn interaction(&self, id: WidgetId) -> Interaction {
        let hovered_here = self.hovered.is_some_and(|h| self.is_within(h, id));
        if self.pressed == Some(id) && hovered_here {
            Interaction::Pressed
        } else if hovered_here {
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
        // Drag feedback (drop-target highlight + drag ghost) sits above even the
        // popups, following the cursor.
        self.emit_drag_feedback(&mut list);
        list
    }

    /// While a first-class drag is live, draw two cues on top of everything: a
    /// highlight on the drop target under the cursor, and a faded "ghost" of the
    /// dragged widget following the cursor.
    fn emit_drag_feedback(&self, list: &mut DrawList) {
        let (Some(drag), Some(cursor)) = (self.drag.filter(|d| d.active), self.cursor) else {
            return;
        };
        // Highlight the drop target under the cursor (a translucent accent fill
        // plus an accent border), so it reads as "drop here".
        if let Some(target) = self.drop_target_at(cursor)
            && let Some(rect) = self.rect(target)
        {
            list.commands.push(DrawCommand::RoundedRect {
                rect,
                color: self.theme.focus.fade(0.20),
                radius: self.widgets[target].corner_radius,
                border_width: 2.0,
                border_color: self.theme.focus,
            });
        }
        // The ghost: re-emit the source's subtree, then translate it by the drag
        // delta (so it sits under the cursor as when grabbed) and fade it.
        if self.rect(drag.source).is_some() {
            let delta = cursor - drag.start;
            let from = list.commands.len();
            self.emit(drag.source, list);
            for cmd in &mut list.commands[from..] {
                translate_and_fade(cmd, delta, DRAG_GHOST_ALPHA);
            }
        }
    }

    fn emit(&self, id: WidgetId, list: &mut DrawList) {
        let Some(rect) = self.rect(id) else {
            return;
        };
        let widget = &self.widgets[id];
        // A disabled widget fades its whole subtree: remember where this
        // widget's commands start, then scale their alpha down at the end.
        let dim_from = widget.disabled.then_some(list.commands.len());

        // The widget's own visuals, by kind.
        match &widget.kind {
            WidgetKind::Panel => self.fill_widget_bg(id, rect, widget.background, list),
            WidgetKind::Label(_) => {
                self.fill_widget_bg(id, rect, widget.background, list);
                self.emit_text(id, widget.foreground, list);
            }
            WidgetKind::Button(_) => self.emit_button(id, rect, widget, list),
            WidgetKind::Checkbox { checked, .. } => {
                self.emit_checkbox(id, rect, *checked, widget, list)
            }
            WidgetKind::TextInput(_) => self.emit_text_input(id, rect, widget, list),
            WidgetKind::Image(handle) => list.commands.push(DrawCommand::Image {
                rect,
                handle: *handle,
                tint: self.image_tint(id, widget),
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
                color: self.theme.scrollbar_thumb,
            });
        }

        // Fade the fills and text this disabled widget (and its subtree) emitted.
        if let Some(from) = dim_from {
            for cmd in &mut list.commands[from..] {
                match cmd {
                    DrawCommand::FillRect { color, .. } | DrawCommand::Text { color, .. } => {
                        *color = color.fade(self.theme.disabled_fade);
                    }
                    DrawCommand::RoundedRect {
                        color,
                        border_color,
                        ..
                    } => {
                        *color = color.fade(self.theme.disabled_fade);
                        *border_color = border_color.fade(self.theme.disabled_fade);
                    }
                    _ => {}
                }
            }
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
            color: self.theme.slider_track,
        });
        let t = if max > min {
            ((value - min) / (max - min)).clamp(0.0, 1.0)
        } else {
            0.0
        };
        // The filled portion.
        list.commands.push(DrawCommand::FillRect {
            rect: Rect::new(track.pos, Vec2::new(track.size.x * t, track_h)),
            color: self.theme.slider_fill,
        });
        // The thumb: a small square centered on the value position.
        let half = rect.size.y * 0.5;
        let cx = rect.pos.x + track.size.x * t;
        let base = self.theme.slider_fill;
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

    /// Emit a widget's background fill styled by its corner radius and border
    /// (a `RoundedRect` when either is set, else a plain fill). Skips a fully
    /// transparent fill that also has no border.
    fn fill_widget_bg(&self, id: WidgetId, rect: Rect, color: Color, list: &mut DrawList) {
        let w = &self.widgets[id];
        let has_border = w.border_width > 0.0 && w.border_color.a > 0.0;
        if w.corner_radius > 0.0 || has_border {
            if color.a > 0.0 || has_border {
                list.commands.push(DrawCommand::RoundedRect {
                    rect,
                    color,
                    radius: w.corner_radius,
                    border_width: if has_border { w.border_width } else { 0.0 },
                    border_color: w.border_color,
                });
            }
        } else {
            self.fill_background(rect, color, list);
        }
    }

    /// The tint for an image: its foreground (white by default = passthrough),
    /// but the theme accent when it sits inside an icon-button that is hovered or
    /// pressed - so the icon recolors instead of the button drawing a background.
    fn image_tint(&self, id: WidgetId, widget: &Widget) -> Color {
        if let Some(button) = self.interactive_ancestor(id)
            && self.widgets[button].icon_button
            && self.interaction(button) != Interaction::Idle
        {
            return self.theme.icon_hover;
        }
        widget.foreground
    }

    /// A button: a state-shaded fill (its style background, or the theme default)
    /// under its caption.
    fn emit_button(&self, id: WidgetId, rect: Rect, widget: &Widget, list: &mut DrawList) {
        // An icon button draws no fill at all; its child icon shows hover/press
        // by recoloring (see image_tint). Any caption still draws.
        if widget.icon_button {
            self.emit_text(id, widget.foreground, list);
            return;
        }
        if widget.flat {
            // A flat button (list/tree row): its own background when idle
            // (transparent draws nothing). A row with a background (selected)
            // keeps it and just brightens on hover; a plain row gets a subtle
            // overlay, so nothing reads as a raised button.
            let bg = widget.background;
            let color = match self.interaction(id) {
                Interaction::Idle => bg,
                Interaction::Hovered if bg.a > 0.0 => bg.lighten(0.06),
                Interaction::Hovered => self.theme.row_hover,
                Interaction::Pressed if bg.a > 0.0 => bg.darken(0.06),
                Interaction::Pressed => self.theme.row_pressed,
            };
            self.fill_widget_bg(id, rect, color, list);
            self.emit_text(id, widget.foreground, list);
            return;
        }
        let base = if widget.background.a > 0.0 {
            widget.background
        } else {
            self.theme.button
        };
        let color = match self.interaction(id) {
            Interaction::Idle => base,
            Interaction::Hovered => base.lighten(0.08),
            Interaction::Pressed => base.darken(0.12),
        };
        self.fill_widget_bg(id, rect, color, list);
        self.emit_text(id, widget.foreground, list);
    }

    /// A checkbox: a state-shaded box, with an inset accent square when checked.
    fn emit_checkbox(
        &self,
        id: WidgetId,
        rect: Rect,
        checked: bool,
        widget: &Widget,
        list: &mut DrawList,
    ) {
        let box_color = match self.interaction(id) {
            Interaction::Idle => self.theme.checkbox_box,
            Interaction::Hovered => self.theme.checkbox_box.lighten(0.12),
            Interaction::Pressed => self.theme.checkbox_box.darken(0.10),
        };
        // A fixed, slightly-rounded square box at the left, vertically centered
        // in the widget's rect (which may be taller when it stretches in a row) -
        // so the box always reads square, not stretched.
        let size = theme::CHECKBOX_SIZE;
        let box_rect = Rect::new(
            Vec2::new(rect.pos.x, rect.pos.y + (rect.size.y - size) * 0.5),
            Vec2::splat(size),
        );
        list.commands.push(DrawCommand::RoundedRect {
            rect: box_rect,
            color: box_color,
            radius: theme::CHECKBOX_RADIUS,
            border_width: 0.0,
            border_color: Color::TRANSPARENT,
        });
        // The check glyph, centered in the box (from its own buffer).
        if checked
            && let Some(buffer) = self.check_buffers.get(id)
            && let Some(run) = buffer.layout_runs().next()
        {
            let line_h = text::checkmark_metrics(size).line_height;
            let origin = Vec2::new(
                box_rect.pos.x + (size - run.line_w) * 0.5,
                box_rect.pos.y + (size - line_h) * 0.5,
            );
            self.emit_buffer(buffer, origin, self.theme.checkbox_mark, None, list);
        }
        // The caption, to the right of the box, vertically centered.
        if let Some(buffer) = self.buffers.get(id) {
            let line_h = text::metrics_for(widget.font_size).line_height;
            let origin = Vec2::new(
                box_rect.max().x + theme::CHECKBOX_GAP,
                rect.pos.y + (rect.size.y - line_h) * 0.5,
            );
            self.emit_buffer(buffer, origin, widget.foreground, None, list);
        }
    }

    /// A splitter: a state-shaded bar (its style background, or the theme
    /// default), so it reads as grabbable without needing a resize cursor.
    fn emit_splitter(&self, id: WidgetId, rect: Rect, widget: &Widget, list: &mut DrawList) {
        let base = if widget.background.a > 0.0 {
            widget.background
        } else {
            self.theme.splitter
        };
        let color = match self.interaction(id) {
            Interaction::Idle => base,
            Interaction::Hovered => base.lighten(0.10),
            Interaction::Pressed => base.lighten(0.20),
        };
        list.commands.push(DrawCommand::FillRect { rect, color });
    }

    /// A text input: a field fill (with an accent border when focused), its text
    /// clipped to the field, and a caret when focused.
    fn emit_text_input(&self, id: WidgetId, rect: Rect, widget: &Widget, list: &mut DrawList) {
        let focused = self.focused == Some(id);
        let fill = if widget.background.a > 0.0 {
            widget.background
        } else {
            self.theme.field
        };
        // The border is the focus accent when focused, else the field's own
        // style border. A rounded or bordered field draws as one RoundedRect.
        let (border_w, border_c) = if focused {
            (widget.border_width.max(1.5), self.theme.focus)
        } else {
            (widget.border_width, widget.border_color)
        };
        let has_border = border_w > 0.0 && border_c.a > 0.0;
        if widget.corner_radius > 0.0 || has_border {
            list.commands.push(DrawCommand::RoundedRect {
                rect,
                color: fill,
                radius: widget.corner_radius,
                border_width: if has_border { border_w } else { 0.0 },
                border_color: border_c,
            });
        } else {
            list.commands
                .push(DrawCommand::FillRect { rect, color: fill });
        }

        // Clip the text, selection, and caret so long content stays in-field.
        list.commands.push(DrawCommand::PushClip { rect });
        // The content-box origin, shifted left by the field's horizontal scroll
        // so a long single-line field keeps its caret in view.
        let base = self.content_origin.get(id).copied().unwrap_or(rect.pos);
        let origin = base - Vec2::new(self.hscroll_of(id), 0.0);
        // While the field is empty, show its placeholder (dimmed) in place of
        // the text - it does not take focus, a caret, or a selection.
        let empty = matches!(&widget.kind, WidgetKind::TextInput(s) if s.is_empty());
        if empty && let Some(buffer) = self.placeholder_buffers.get(id) {
            self.emit_buffer(buffer, base, self.theme.placeholder, None, list);
        }
        // The selection highlight, behind the text.
        if focused {
            let (lo, hi) = self.selection_bounds(id);
            if lo != hi {
                if widget.multiline {
                    self.emit_multiline_selection(id, lo, hi, origin, list);
                } else {
                    let x0 = origin.x + self.x_offset_at(id, lo);
                    let x1 = origin.x + self.x_offset_at(id, hi);
                    list.commands.push(DrawCommand::FillRect {
                        rect: Rect::new(
                            Vec2::new(x0, rect.pos.y + 3.0),
                            Vec2::new((x1 - x0).max(0.0), (rect.size.y - 6.0).max(0.0)),
                        ),
                        color: self.theme.selection,
                    });
                }
            }
        }
        if let Some(buffer) = self.buffers.get(id) {
            self.emit_buffer(buffer, origin, widget.foreground, None, list);
        }
        if focused && self.caret_on {
            let caret = if widget.multiline {
                self.multiline_caret(id).map(|(cx, top)| {
                    let line_h = text::metrics_for(widget.font_size).line_height;
                    Rect::new(origin + Vec2::new(cx, top), Vec2::new(1.5, line_h))
                })
            } else {
                let x = origin.x + self.caret_x_offset(id);
                Some(Rect::new(
                    Vec2::new(x, rect.pos.y + 3.0),
                    Vec2::new(1.5, (rect.size.y - 6.0).max(0.0)),
                ))
            };
            if let Some(caret) = caret {
                list.commands.push(DrawCommand::FillRect {
                    rect: caret,
                    color: self.theme.caret,
                });
            }
        }
        list.commands.push(DrawCommand::PopClip);
    }

    /// Emit the selection highlight for a text area: one rectangle per visual
    /// line, using cosmic-text's per-run highlight spans (which handle wrapping
    /// and line breaks). `origin` is the field's content-box top-left.
    fn emit_multiline_selection(
        &self,
        id: WidgetId,
        lo: usize,
        hi: usize,
        origin: Vec2,
        list: &mut DrawList,
    ) {
        let WidgetKind::TextInput(s) = &self.widgets[id].kind else {
            return;
        };
        let Some(buffer) = self.buffers.get(id) else {
            return;
        };
        let (start, end) = (byte_to_cursor(s, lo), byte_to_cursor(s, hi));
        for run in buffer.layout_runs() {
            // highlight() only tests "between the cursors"; for a run OUTSIDE the
            // selected line range that wrongly reports the whole line selected,
            // so skip those runs (else selecting one line highlights every line
            // below it to the end of the text).
            if run.line_i < start.line || run.line_i > end.line {
                continue;
            }
            for (x, w) in run.highlight(start, end) {
                list.commands.push(DrawCommand::FillRect {
                    rect: Rect::new(
                        origin + Vec2::new(x, run.line_top),
                        Vec2::new(w.max(0.0), run.line_height),
                    ),
                    color: self.theme.selection,
                });
            }
        }
    }

    /// The text-area caret's (x, top) offset within the content box, from the
    /// shaped buffer - `None` if the field is not shaped yet.
    fn multiline_caret(&self, id: WidgetId) -> Option<(f32, f32)> {
        let WidgetKind::TextInput(s) = &self.widgets[id].kind else {
            return None;
        };
        let buffer = self.buffers.get(id)?;
        let cursor = byte_to_cursor(s, self.caret.min(s.len()));
        buffer.cursor_position(&cursor)
    }

    /// Emit a text command for a text-bearing widget from its shaped buffer,
    /// positioned at the widget's content-box origin (centered horizontally if
    /// the widget asks for it).
    fn emit_text(&self, id: WidgetId, color: Color, list: &mut DrawList) {
        let Some(buffer) = self.buffers.get(id) else {
            return;
        };
        let origin = self.content_origin.get(id).copied().unwrap_or(Vec2::ZERO);
        // For centering, the content-box width: the field width minus the left
        // inset on both sides (padding is symmetric for the labels that use it).
        let center_width = if self.widgets[id].text_center {
            self.rect(id).map(|rect| {
                let pad_left = origin.x - rect.pos.x;
                (rect.size.x - 2.0 * pad_left).max(0.0)
            })
        } else {
            None
        };
        self.emit_buffer(buffer, origin, color, center_width, list);
    }

    /// Emit a `Text` command for a shaped `buffer` drawn from `origin` (its
    /// content-box top-left), in `color`. Shared by the main text and the
    /// placeholder. When `center_width` is set, each line is centered within
    /// that width (its own run width subtracted).
    fn emit_buffer(
        &self,
        buffer: &Buffer,
        origin: Vec2,
        color: Color,
        center_width: Option<f32>,
        list: &mut DrawList,
    ) {
        let mut glyphs = Vec::new();
        for run in buffer.layout_runs() {
            // Center this line by shifting its pen x by half the slack.
            let x0 = origin.x + center_width.map_or(0.0, |w| ((w - run.line_w) * 0.5).max(0.0));
            for glyph in run.glyphs {
                // Bake the content origin and the run's baseline into the pen
                // position; the backend adds each glyph's bitmap bearing.
                let physical = glyph.physical((x0, origin.y + run.line_y), 1.0);
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

/// A whole-string byte offset as a cosmic-text [`Cursor`] (buffer line index +
/// byte within that line). Text areas store the caret as a flat byte offset;
/// cosmic-text positions and hit-tests in (line, index) - these two convert.
fn byte_to_cursor(text: &str, byte: usize) -> Cursor {
    let byte = byte.min(text.len());
    let line = text[..byte].bytes().filter(|&b| b == b'\n').count();
    let line_start = text[..byte].rfind('\n').map_or(0, |i| i + 1);
    Cursor::new(line, byte - line_start)
}

/// A cosmic-text [`Cursor`] back to a whole-string byte offset (the inverse of
/// [`byte_to_cursor`]).
fn cursor_to_byte(text: &str, cursor: Cursor) -> usize {
    let mut start = 0;
    for _ in 0..cursor.line {
        match text[start..].find('\n') {
            Some(i) => start += i + 1,
            None => return text.len(),
        }
    }
    (start + cursor.index).min(text.len())
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

/// Offset a draw command's geometry by `delta` and fade its colors by `fade`
/// (used to turn a re-emitted widget subtree into a drag ghost).
fn translate_and_fade(cmd: &mut DrawCommand, delta: Vec2, fade: f32) {
    match cmd {
        DrawCommand::FillRect { rect, color } => {
            rect.pos += delta;
            *color = color.fade(fade);
        }
        DrawCommand::RoundedRect {
            rect,
            color,
            border_color,
            ..
        } => {
            rect.pos += delta;
            *color = color.fade(fade);
            *border_color = border_color.fade(fade);
        }
        DrawCommand::Text { glyphs, color } => {
            for g in glyphs.iter_mut() {
                g.x += delta.x;
                g.y += delta.y;
            }
            *color = color.fade(fade);
        }
        DrawCommand::Image { rect, tint, .. } => {
            rect.pos += delta;
            *tint = tint.fade(fade);
        }
        DrawCommand::PushClip { rect } => rect.pos += delta,
        DrawCommand::PopClip => {}
    }
}

/// The previous word boundary at or before `i`: skip any whitespace to the left,
/// then the run of word characters, landing at the word's start (ctrl+left).
fn prev_word_boundary(s: &str, i: usize) -> usize {
    let is_ws = |j: usize| s[..j].chars().next_back().is_some_and(char::is_whitespace);
    let mut j = i;
    while j > 0 && is_ws(j) {
        j = prev_boundary(s, j);
    }
    while j > 0 && !is_ws(j) {
        j = prev_boundary(s, j);
    }
    j
}

/// The next word boundary at or after `i`: skip the run of word characters, then
/// the whitespace after it, landing at the next word's start (ctrl+right).
fn next_word_boundary(s: &str, i: usize) -> usize {
    let is_ws = |j: usize| s[j..].chars().next().is_some_and(char::is_whitespace);
    let mut j = i;
    while j < s.len() && !is_ws(j) {
        j = next_boundary(s, j);
    }
    while j < s.len() && is_ws(j) {
        j = next_boundary(s, j);
    }
    j
}

/// taffy measure function: shape a text-bearing leaf and return its size. Called
/// only for leaf nodes without a definite style size.
fn measure_widget(
    known: Size<Option<f32>>,
    available: Size<AvailableSpace>,
    ctx: Option<&mut WidgetId>,
    widgets: &SlotMap<WidgetId, Widget>,
    buffers: &mut SecondaryMap<WidgetId, Buffer>,
    check_buffers: &mut SecondaryMap<WidgetId, Buffer>,
    font_system: &mut FontSystem,
) -> Size<f32> {
    let Some(&mut id) = ctx else {
        return Size::ZERO;
    };
    let content = match &widgets[id].kind {
        WidgetKind::Label(t) | WidgetKind::Button(t) | WidgetKind::TextInput(t) => t.as_str(),
        // A checkbox is a fixed-size box plus an optional caption. Shape the
        // check glyph into check_buffers (drawn when checked) and the caption
        // into the main buffer; measure to box + gap + caption width.
        WidgetKind::Checkbox { label, .. } => {
            let box_metrics = text::checkmark_metrics(theme::CHECKBOX_SIZE);
            if !check_buffers.contains_key(id) {
                check_buffers.insert(id, Buffer::new(font_system, box_metrics));
            }
            {
                let mut borrowed = check_buffers[id].borrow_with(font_system);
                borrowed.set_metrics(box_metrics);
                borrowed.set_size(None, None);
                borrowed.set_text(
                    text::CHECK_MARK,
                    &text::default_attrs(),
                    Shaping::Advanced,
                    None,
                );
                borrowed.shape_until_scroll(false);
            }
            let mut label_w = 0.0f32;
            let mut line_h = theme::CHECKBOX_SIZE;
            if !label.is_empty() {
                let metrics = text::metrics_for(widgets[id].font_size);
                line_h = line_h.max(metrics.line_height);
                if !buffers.contains_key(id) {
                    buffers.insert(id, Buffer::new(font_system, metrics));
                }
                let mut borrowed = buffers[id].borrow_with(font_system);
                borrowed.set_metrics(metrics);
                borrowed.set_size(None, None);
                borrowed.set_text(label, &text::default_attrs(), Shaping::Advanced, None);
                borrowed.shape_until_scroll(false);
                label_w = borrowed.layout_runs().map(|r| r.line_w).fold(0.0, f32::max);
            }
            let width = theme::CHECKBOX_SIZE
                + if label_w > 0.0 {
                    theme::CHECKBOX_GAP + label_w.ceil()
                } else {
                    0.0
                };
            return Size {
                width,
                height: line_h,
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

    // A single-line text input never wraps and never reports its text width: it
    // stays one line and scrolls horizontally instead (see focused_hscroll), so
    // long text neither folds onto a hidden second line nor grows the field out
    // of its container. Labels, buttons, and text areas measure to their content.
    let single_line =
        matches!(widgets[id].kind, WidgetKind::TextInput(_)) && !widgets[id].multiline;

    // Wrap to a known width if taffy gives one, else the available width.
    let wrap_width = if single_line {
        None
    } else {
        known.width.or(match available.width {
            AvailableSpace::Definite(w) => Some(w),
            AvailableSpace::MinContent | AvailableSpace::MaxContent => None,
        })
    };

    // Shape at this widget's font size, so headings and captions measure (and
    // later draw) larger or smaller than the UI default.
    let metrics = text::metrics_for(widgets[id].font_size);
    if !buffers.contains_key(id) {
        buffers.insert(id, Buffer::new(font_system, metrics));
    }
    let buffer = &mut buffers[id];
    {
        let mut borrowed = buffer.borrow_with(font_system);
        borrowed.set_metrics(metrics);
        borrowed.set_size(wrap_width, None);
        borrowed.set_text(content, &text::default_attrs(), Shaping::Advanced, None);
        borrowed.shape_until_scroll(false);
    }

    if single_line {
        // Intrinsic width 0: the style's width or grow drives the field's size;
        // its text is not part of that, so typing never resizes it.
        return Size {
            width: 0.0,
            height: metrics.line_height,
        };
    }

    let mut width = 0.0f32;
    let mut lines = 0u32;
    for run in buffer.layout_runs() {
        width = width.max(run.line_w);
        lines += 1;
    }
    Size {
        width: width.ceil(),
        height: lines.max(1) as f32 * metrics.line_height,
    }
}

#[cfg(test)]
mod tests {
    use super::Ui;
    use crate::color::Color;
    use crate::draw::DrawCommand;
    use crate::event::Event;
    use crate::input::{CursorHint, InputEvent, Key};
    use crate::rect::Rect;
    use crate::style::Style;
    use crate::theme::Theme;
    use crate::widget::{Mask, Orientation, WidgetKind};
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
    fn clicking_a_text_area_line_places_the_caret_on_that_line() {
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().size(300.0, 200.0).padding(10.0));
        let area = ui.text_area(root, "aa\nbb\ncc", Style::new().width(200.0));
        ui.layout(Vec2::new(300.0, 200.0)).unwrap();

        // The content box top-left (no field padding here; the root pads 10).
        let rect = ui.rect(area).unwrap();
        // Click near the start of line 1 ("bb"): x just inside, y in the second
        // 20px line band.
        let click = Vec2::new(rect.pos.x + 2.0, rect.pos.y + 20.0 + 10.0);
        click_at(&mut ui, click);
        // Select the two chars to the right; if the caret really landed at the
        // start of line 1, that selects "bb" (not "" as an end-of-text caret would).
        ui.set_shift(true);
        ui.handle_input(InputEvent::Key(Key::Right));
        ui.handle_input(InputEvent::Key(Key::Right));
        assert_eq!(ui.selected_text().as_deref(), Some("bb"));
    }

    #[test]
    fn dragging_across_text_area_lines_selects_the_span() {
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().column().size(360.0, 200.0).padding(16.0));
        // A caption above it, like the controls example (so the field is not at
        // the origin - exercises the content-origin offset).
        ui.label(root, "Notes:", Style::new());
        let area = ui.text_area(
            root,
            "aaaa\nbbbb\ncccc",
            Style::new().width(320.0).padding(6.0),
        );
        ui.layout(Vec2::new(360.0, 200.0)).unwrap();

        let rect = ui.rect(area).unwrap();
        let content = rect.pos + Vec2::splat(6.0);
        // Press mid-line-0, drag to mid-line-1, release.
        ui.handle_input(InputEvent::PointerMoved(content + Vec2::new(10.0, 10.0)));
        ui.handle_input(InputEvent::PointerPressed);
        ui.handle_input(InputEvent::PointerMoved(content + Vec2::new(14.0, 30.0)));
        ui.handle_input(InputEvent::PointerReleased);

        let sel = ui.selected_text().unwrap_or_default();
        // The selection should span from inside line 0 across the newline into
        // line 1 - it must contain the break and not be empty/degenerate.
        assert!(
            sel.contains('\n') && sel.len() >= 3,
            "expected a cross-line selection, got {sel:?}"
        );
    }

    #[test]
    fn text_area_selection_highlight_sits_on_the_selected_line() {
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().column().size(360.0, 200.0).padding(16.0));
        ui.label(root, "Notes:", Style::new());
        let area = ui.text_area(
            root,
            "aaaaaaaa\nbbbbbbbb\ncccccccc",
            Style::new().width(320.0).padding(6.0),
        );
        ui.layout(Vec2::new(360.0, 200.0)).unwrap();

        let rect = ui.rect(area).unwrap();
        let content = rect.pos + Vec2::splat(6.0);
        // Drag across the first half of line 0.
        ui.handle_input(InputEvent::PointerMoved(content + Vec2::new(2.0, 10.0)));
        ui.handle_input(InputEvent::PointerPressed);
        ui.handle_input(InputEvent::PointerMoved(content + Vec2::new(40.0, 10.0)));
        ui.handle_input(InputEvent::PointerReleased);
        ui.layout(Vec2::new(360.0, 200.0)).unwrap();

        // The one selection highlight must sit on line 0 (top band) and start at
        // its left edge - not at the end of the text.
        let sel: Vec<Rect> = ui
            .draw_list()
            .commands
            .iter()
            .filter_map(|c| match c {
                DrawCommand::FillRect { rect, color } if *color == Theme::dark().selection => {
                    Some(*rect)
                }
                _ => None,
            })
            .collect();
        assert_eq!(sel.len(), 1, "one line selected -> one highlight: {sel:?}");
        let h = sel[0];
        assert!(
            (h.pos.y - content.y).abs() < 5.0,
            "highlight should be on line 0 (y~{}), got {h:?}",
            content.y
        );
        assert!(
            (h.pos.x - content.x).abs() < 3.0,
            "highlight should start at the line's left (x~{}), got {h:?}",
            content.x
        );
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
    fn byte_and_cursor_round_trip_across_lines() {
        let text = "ab\ncde\n\nfg";
        for byte in 0..=text.len() {
            if !text.is_char_boundary(byte) {
                continue;
            }
            let cur = super::byte_to_cursor(text, byte);
            assert_eq!(super::cursor_to_byte(text, cur), byte, "byte {byte}");
        }
        // Spot-check the mapping itself: the 'd' at index 4 is line 1, column 1.
        assert_eq!(
            super::byte_to_cursor(text, 4),
            cosmic_text::Cursor::new(1, 1)
        );
    }

    #[test]
    fn a_text_area_inserts_newlines_on_enter() {
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().size(300.0, 200.0));
        let area = ui.text_area(root, "", Style::new().size(280.0, 120.0));
        ui.layout(Vec2::new(300.0, 200.0)).unwrap();
        click_at(&mut ui, Vec2::new(20.0, 20.0));

        for ch in "ab".chars() {
            ui.handle_input(InputEvent::Text(ch));
        }
        ui.handle_input(InputEvent::Key(Key::Enter));
        for ch in "cd".chars() {
            ui.handle_input(InputEvent::Text(ch));
        }
        assert_eq!(text_of(&ui, area), "ab\ncd");
        // Enter inserted a newline; it did not submit.
        assert!(
            !ui.drain_events()
                .iter()
                .any(|e| matches!(e, Event::Submitted(_)))
        );
    }

    #[test]
    fn text_area_up_then_home_lands_on_the_line_above() {
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().size(300.0, 200.0));
        let area = ui.text_area(root, "", Style::new().size(280.0, 120.0));
        ui.layout(Vec2::new(300.0, 200.0)).unwrap();
        click_at(&mut ui, Vec2::new(20.0, 20.0));

        // Build "aa\nbb" with the caret at the end (line 2).
        for ch in "aa".chars() {
            ui.handle_input(InputEvent::Text(ch));
        }
        ui.handle_input(InputEvent::Key(Key::Enter));
        for ch in "bb".chars() {
            ui.handle_input(InputEvent::Text(ch));
        }
        // Re-shape after editing (the app lays out every frame; Up/Home read the
        // shaped buffer to position the caret in 2D).
        ui.layout(Vec2::new(300.0, 200.0)).unwrap();
        // Up moves onto line 1; multiline Home snaps to its start; typing there
        // proves the caret really crossed the line break.
        ui.handle_input(InputEvent::Key(Key::Up));
        ui.handle_input(InputEvent::Key(Key::Home));
        ui.handle_input(InputEvent::Text('X'));
        assert_eq!(text_of(&ui, area), "Xaa\nbb");
    }

    #[test]
    fn a_text_area_selection_highlights_each_line() {
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().size(300.0, 200.0));
        let _area = ui.text_area(root, "ab\ncd", Style::new().size(280.0, 120.0));
        ui.layout(Vec2::new(300.0, 200.0)).unwrap();
        click_at(&mut ui, Vec2::new(20.0, 20.0));
        ui.select_all();
        ui.layout(Vec2::new(300.0, 200.0)).unwrap();

        // Two selected lines -> at least two selection-colored highlight rects.
        let highlights = ui
            .draw_list()
            .commands
            .iter()
            .filter(|c| matches!(c, DrawCommand::FillRect { color, .. } if *color == Theme::dark().selection))
            .count();
        assert!(
            highlights >= 2,
            "expected a highlight per line, got {highlights}"
        );
    }

    #[test]
    fn a_drag_from_a_source_onto_a_target_drops() {
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().row().size(400.0, 100.0));
        let source = ui.button(root, "drag me", Style::new().size(120.0, 100.0).draggable());
        let target = ui.panel(root, Style::new().size(120.0, 100.0).drop_target());
        ui.layout(Vec2::new(400.0, 100.0)).unwrap();

        // Press on the source, move past the threshold (drag goes live), move
        // over the target, release.
        ui.handle_input(InputEvent::PointerMoved(Vec2::new(60.0, 50.0)));
        ui.handle_input(InputEvent::PointerPressed);
        ui.handle_input(InputEvent::PointerMoved(Vec2::new(200.0, 50.0)));
        assert!(ui.is_dragging());
        assert_eq!(ui.drag_source(), Some(source));
        ui.handle_input(InputEvent::PointerReleased);

        let events = ui.drain_events();
        assert!(events.contains(&Event::DragStarted { source }));
        assert!(events.contains(&Event::Dropped {
            source,
            target: Some(target)
        }));
        // A drop is not a click: the source button never fired Clicked.
        assert!(!events.iter().any(|e| matches!(e, Event::Clicked(_))));
    }

    #[test]
    fn a_plain_click_on_a_draggable_is_not_a_drag() {
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().size(200.0, 100.0));
        let source = ui.button(root, "x", Style::new().size(120.0, 60.0).draggable());
        ui.layout(Vec2::new(200.0, 100.0)).unwrap();

        // Press and release without moving past the threshold: a click, no drop.
        ui.handle_input(InputEvent::PointerMoved(Vec2::new(30.0, 20.0)));
        ui.handle_input(InputEvent::PointerPressed);
        ui.handle_input(InputEvent::PointerReleased);

        let events = ui.drain_events();
        assert_eq!(events, vec![Event::Clicked(source)]);
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
    fn per_side_padding_insets_the_child() {
        let mut ui = Ui::new();
        let root = ui.root_panel(
            Style::new()
                .size(200.0, 200.0)
                .padding_left(10.0)
                .padding_top(6.0),
        );
        let child = ui.panel(root, Style::new().fill());
        ui.layout(Vec2::new(200.0, 200.0)).unwrap();
        // The child starts at the padded top-left corner.
        assert_eq!(ui.rect(child).unwrap().pos, Vec2::new(10.0, 6.0));
    }

    #[test]
    fn per_side_margin_offsets_the_box() {
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().size(200.0, 200.0).column());
        let child = ui.panel(
            root,
            Style::new()
                .size(20.0, 20.0)
                .margin_left(15.0)
                .margin_top(4.0),
        );
        ui.layout(Vec2::new(200.0, 200.0)).unwrap();
        // Margin pushes the box in from the container's edges (no padding here).
        assert_eq!(ui.rect(child).unwrap().pos, Vec2::new(15.0, 4.0));
    }

    #[test]
    fn text_center_offsets_a_short_caption_from_the_left() {
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().size(200.0, 100.0));
        let button = ui.button(root, "X", Style::new().size(80.0, 24.0).text_center());
        ui.layout(Vec2::new(200.0, 100.0)).unwrap();

        let rect = ui.rect(button).unwrap();
        let gx = ui
            .draw_list()
            .commands
            .iter()
            .find_map(|c| match c {
                DrawCommand::Text { glyphs, .. } if !glyphs.is_empty() => Some(glyphs[0].x),
                _ => None,
            })
            .unwrap();
        // A single narrow "X" centered in an 80px button starts well right of the
        // left edge, not at ~0 as a left-aligned caption would.
        assert!(
            gx > rect.pos.x + 20.0,
            "centered glyph x {gx} not centered in {rect:?}"
        );
    }

    #[test]
    fn a_larger_font_size_measures_bigger() {
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().size(400.0, 200.0).column());
        // Same text, default vs a big font size: the big one is taller (in a
        // column the cross axis stretches both to full width, so height is the
        // unambiguous signal).
        let small = ui.label(root, "Heading", Style::new());
        let big = ui.label(root, "Heading", Style::new().font_size(40.0));
        ui.layout(Vec2::new(400.0, 200.0)).unwrap();

        let s = ui.rect(small).unwrap().size;
        let b = ui.rect(big).unwrap().size;
        assert!(b.y > s.y, "small={s:?} big={b:?}");
    }

    #[test]
    fn a_checked_checkbox_draws_the_check_glyph() {
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().size(60.0, 60.0).row());
        let on = ui.checkbox(root, true, "", Style::new());
        let off = ui.checkbox(root, false, "", Style::new());
        ui.layout(Vec2::new(60.0, 60.0)).unwrap();

        // The checked box emits a check-colored Text glyph run (the box itself
        // is a RoundedRect), so a Text command is unambiguously the mark.
        let cmds = ui.draw_list().commands;
        let mark = |c: &DrawCommand| {
            matches!(c, DrawCommand::Text { color, glyphs }
                if *color == Theme::dark().checkbox_mark && !glyphs.is_empty())
        };
        assert!(cmds.iter().any(mark), "checked box should draw the glyph");
        // Sanity: exactly one mark (only the checked box), and both boxes exist.
        assert_eq!(cmds.iter().filter(|c| mark(c)).count(), 1);
        assert!(ui.rect(on).unwrap().size.x > 0.0 && ui.rect(off).unwrap().size.x > 0.0);
    }

    #[test]
    fn clicking_a_checkbox_caption_toggles_it() {
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().size(240.0, 40.0));
        let cb = ui.checkbox(root, false, "Show grid", Style::new());
        ui.layout(Vec2::new(240.0, 40.0)).unwrap();

        // The widget is wider than the box (box + gap + caption); click on the
        // caption area, well to the right of the box, and it still toggles.
        let rect = ui.rect(cb).unwrap();
        assert!(rect.size.x > 40.0, "the caption should widen the checkbox");
        click_at(
            &mut ui,
            Vec2::new(rect.max().x - 6.0, rect.pos.y + rect.size.y * 0.5),
        );
        assert_eq!(
            ui.drain_events(),
            vec![Event::Toggled {
                id: cb,
                checked: true
            }]
        );
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
    fn a_disabled_button_takes_no_hover_or_click() {
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().size(200.0, 100.0));
        let _button = ui.button(root, "Go", Style::new().size(80.0, 30.0).disabled());
        ui.layout(Vec2::new(200.0, 100.0)).unwrap();

        // Hover resolves to nothing interactive, and a full click fires no event.
        ui.handle_input(InputEvent::PointerMoved(Vec2::new(10.0, 10.0)));
        assert_eq!(ui.hovered(), None);
        click_at(&mut ui, Vec2::new(10.0, 10.0));
        assert!(ui.drain_events().is_empty());
    }

    #[test]
    fn a_disabled_text_input_is_skipped_by_tab_focus() {
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().size(200.0, 100.0).column());
        let a = ui.text_input(root, "a", Style::new().size(80.0, 24.0));
        let _off = ui.text_input(root, "b", Style::new().size(80.0, 24.0).disabled());
        let c = ui.text_input(root, "c", Style::new().size(80.0, 24.0));
        ui.layout(Vec2::new(200.0, 100.0)).unwrap();

        // Tabbing from the first field lands on the third, past the disabled one.
        ui.focus_step(1); // focus first
        assert_eq!(ui.focused(), Some(a));
        ui.focus_step(1); // skip disabled -> third
        assert_eq!(ui.focused(), Some(c));
    }

    #[test]
    fn a_disabled_widget_fades_its_fill() {
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().size(200.0, 100.0));
        // An opaque-backgrounded button so its fill alpha is unambiguous.
        ui.button(
            root,
            "Go",
            Style::new()
                .size(80.0, 30.0)
                .background(Color::rgb(0.3, 0.3, 0.3))
                .disabled(),
        );
        ui.layout(Vec2::new(200.0, 100.0)).unwrap();

        let faded = ui.draw_list().commands.iter().any(|c| {
            matches!(c, DrawCommand::FillRect { color, .. }
                if (color.a - Theme::dark().disabled_fade).abs() < 1e-6)
        });
        assert!(faded, "disabled button fill should be faded");
    }

    #[test]
    fn clicking_a_checkbox_toggles_it_and_emits() {
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().size(100.0, 100.0));
        let cb = ui.checkbox(root, false, "Enabled", Style::new());
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
        assert!(matches!(
            ui.kind(cb),
            WidgetKind::Checkbox { checked: true, .. }
        ));

        click_at(&mut ui, Vec2::new(9.0, 9.0));
        assert_eq!(
            ui.drain_events(),
            vec![Event::Toggled {
                id: cb,
                checked: false
            }]
        );
        assert!(matches!(
            ui.kind(cb),
            WidgetKind::Checkbox { checked: false, .. }
        ));
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
    fn mask_accepts_matches_each_kind() {
        // Signed masks accept the empty string and a lone '-' so a value can be
        // typed sign-first; the digit-only masks do not take a sign.
        assert!(Mask::Integer.accepts(""));
        assert!(Mask::Integer.accepts("-42"));
        assert!(!Mask::Integer.accepts("4.2"));
        assert!(Mask::Decimal.accepts("-3.14"));
        assert!(!Mask::Decimal.accepts("1.2.3"));
        assert!(Mask::Digits.accepts("2048"));
        assert!(!Mask::Digits.accepts("-5"));
        assert!(Mask::Hex.accepts("1aF"));
        assert!(!Mask::Hex.accepts("xyz"));
        assert!(Mask::None.accepts("anything at all"));
    }

    #[test]
    fn a_style_masked_input_rejects_out_of_mask_edits() {
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().size(200.0, 60.0));
        let field = ui.text_input(root, "", Style::new().size(180.0, 24.0).mask(Mask::Hex));
        ui.layout(Vec2::new(200.0, 60.0)).unwrap();
        click_at(&mut ui, Vec2::new(10.0, 12.0));
        for ch in "1a".chars() {
            ui.handle_input(InputEvent::Text(ch));
        }
        // A non-hex letter is rejected; a hex one is accepted.
        ui.handle_input(InputEvent::Text('g'));
        ui.handle_input(InputEvent::Text('F'));
        assert_eq!(text_of(&ui, field), "1aF");
    }

    #[test]
    fn a_placeholder_shows_only_while_the_field_is_empty() {
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().size(200.0, 60.0));
        let field = ui.text_input(root, "", Style::new().size(180.0, 24.0).placeholder("Name"));
        ui.layout(Vec2::new(200.0, 60.0)).unwrap();
        // Empty: the placeholder is drawn (a Text command in the dimmed color).
        let shows_placeholder = |ui: &Ui| {
            ui.draw_list().commands.iter().any(|c| {
                matches!(c, DrawCommand::Text { color, .. } if *color == Theme::dark().placeholder)
            })
        };
        assert!(shows_placeholder(&ui));

        // Once it has text, the placeholder is gone.
        ui.set_text_input(field, "abc");
        ui.layout(Vec2::new(200.0, 60.0)).unwrap();
        assert!(!shows_placeholder(&ui));
    }

    #[test]
    fn ctrl_word_navigation_moves_and_selects_by_word() {
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().size(320.0, 60.0));
        let field = ui.text_input(root, "hello world foo", Style::new().size(300.0, 24.0));
        ui.layout(Vec2::new(320.0, 60.0)).unwrap();
        click_at(&mut ui, Vec2::new(10.0, 12.0));
        ui.handle_input(InputEvent::Key(Key::End));

        // Shift+WordLeft from the end selects the last word, then the next.
        ui.set_shift(true);
        ui.handle_input(InputEvent::Key(Key::WordLeft));
        assert_eq!(ui.selected_text().as_deref(), Some("foo"));
        ui.handle_input(InputEvent::Key(Key::WordLeft));
        assert_eq!(ui.selected_text().as_deref(), Some("world foo"));

        // Plain WordRight (no shift) collapses the selection and moves by word.
        ui.set_shift(false);
        ui.handle_input(InputEvent::Key(Key::WordRight));
        assert_eq!(ui.selected_text(), None);
        // From the start of "world", WordRight lands at the start of "foo"; typing
        // there proves the caret position.
        ui.handle_input(InputEvent::Text('!'));
        assert_eq!(text_of(&ui, field), "hello world !foo");
    }

    #[test]
    fn a_live_drag_shows_the_grab_cursor_ghost_and_drop_highlight() {
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().size(400.0, 200.0).row());
        let src_fill = Color::rgb(0.2, 0.2, 0.2);
        let source = ui.button(
            root,
            "file.png",
            Style::new()
                .size(100.0, 24.0)
                .background(src_fill)
                .draggable(),
        );
        let target = ui.panel(root, Style::new().size(200.0, 200.0).drop_target());
        ui.layout(Vec2::new(400.0, 200.0)).unwrap();

        // Press on the source, then move well past the threshold onto the target.
        ui.handle_input(InputEvent::PointerMoved(Vec2::new(20.0, 12.0)));
        ui.handle_input(InputEvent::PointerPressed);
        ui.handle_input(InputEvent::PointerMoved(Vec2::new(250.0, 100.0)));

        assert!(ui.is_dragging());
        assert_eq!(ui.cursor_hint(), CursorHint::Grabbing);

        let cmds = ui.draw_list().commands;
        // Drop-target highlight: a bordered RoundedRect over the target's rect.
        let target_rect = ui.rect(target).unwrap();
        assert!(
            cmds.iter().any(|c| matches!(c,
                DrawCommand::RoundedRect { rect, border_width, .. }
                    if *border_width > 0.0 && *rect == target_rect)),
            "the drop target should be highlighted"
        );
        // Ghost: a faded copy of the source fill, offset from the original.
        let ghost = Color {
            a: src_fill.a * super::DRAG_GHOST_ALPHA,
            ..src_fill
        };
        let src_pos = ui.rect(source).unwrap().pos;
        assert!(
            cmds.iter().any(|c| matches!(c,
                DrawCommand::FillRect { rect, color }
                    if *color == ghost && rect.pos != src_pos)),
            "a faded, offset ghost of the source should be drawn"
        );
    }

    #[test]
    fn set_theme_recolors_the_built_in_widgets() {
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().size(120.0, 40.0));
        // A button with no style background fills with the theme's button color.
        ui.button(root, "Go", Style::new().size(80.0, 24.0));
        ui.layout(Vec2::new(120.0, 40.0)).unwrap();

        let fills_with = |ui: &Ui, want: Color| {
            ui.draw_list()
                .commands
                .iter()
                .any(|c| matches!(c, DrawCommand::FillRect { color, .. } if *color == want))
        };
        // Default (dark) theme paints the dark button fill.
        assert!(fills_with(&ui, Theme::dark().button));

        // Switching the theme repaints it with the new palette's button color.
        ui.set_theme(Theme::light());
        assert_ne!(Theme::dark().button, Theme::light().button);
        assert!(fills_with(&ui, Theme::light().button));
    }

    #[test]
    fn a_rounded_bordered_style_emits_a_rounded_rect() {
        let mut ui = Ui::new();
        ui.root_panel(
            Style::new()
                .size(80.0, 40.0)
                .background(Color::rgb(0.2, 0.2, 0.2))
                .corner_radius(8.0)
                .border(2.0, Color::WHITE),
        );
        ui.layout(Vec2::new(80.0, 40.0)).unwrap();
        assert!(
            ui.draw_list().commands.iter().any(|c| matches!(c,
                DrawCommand::RoundedRect { radius, border_width, .. }
                    if (*radius - 8.0).abs() < 1e-6 && (*border_width - 2.0).abs() < 1e-6)),
            "a rounded, bordered background should emit a RoundedRect"
        );

        // A plain background still takes the fast FillRect path (unchanged).
        let mut plain = Ui::new();
        plain.root_panel(
            Style::new()
                .size(40.0, 40.0)
                .background(Color::rgb(0.1, 0.1, 0.1)),
        );
        plain.layout(Vec2::new(40.0, 40.0)).unwrap();
        assert!(
            plain
                .draw_list()
                .commands
                .iter()
                .all(|c| !matches!(c, DrawCommand::RoundedRect { .. })),
            "a plain background should not emit a RoundedRect"
        );
    }

    #[test]
    fn a_single_line_field_does_not_grow_with_its_text() {
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().row().size(200.0, 40.0));
        // A grow field with very long text: it must fill the row, not stretch to
        // the text's width (which would push it out of the container).
        let field = ui.text_input(root, "x".repeat(300), Style::new().grow(1.0).height(24.0));
        ui.layout(Vec2::new(200.0, 40.0)).unwrap();
        let w = ui.rect(field).unwrap().size.x;
        assert!(
            w <= 200.0,
            "single-line field grew to {w}px; its text should not size it"
        );
    }

    #[test]
    fn a_long_single_line_field_scrolls_to_keep_the_caret_visible() {
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().size(140.0, 40.0));
        let field = ui.text_input(root, "", Style::new().size(100.0, 24.0).padding(4.0));
        ui.layout(Vec2::new(140.0, 40.0)).unwrap();
        click_at(&mut ui, Vec2::new(10.0, 12.0)); // focus

        // Type well past the field's width.
        for _ in 0..40 {
            ui.handle_input(InputEvent::Text('W'));
        }
        ui.layout(Vec2::new(140.0, 40.0)).unwrap();

        let rect = ui.rect(field).unwrap();
        let caret_x = ui
            .draw_list()
            .commands
            .iter()
            .find_map(|c| match c {
                DrawCommand::FillRect { rect: r, color } if *color == Theme::dark().caret => {
                    Some(r.pos.x)
                }
                _ => None,
            })
            .expect("a caret is drawn in a focused field");
        // The caret stays inside the field (the text scrolled), near the right.
        assert!(
            caret_x >= rect.pos.x && caret_x <= rect.max().x,
            "caret x {caret_x} escaped the field {rect:?}"
        );
        assert!(
            caret_x > rect.pos.x + rect.size.x * 0.5,
            "the field should have scrolled the caret toward the right edge"
        );
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
    fn an_icon_button_recolors_its_icon_on_hover() {
        let mut registry: slotmap::SlotMap<crate::draw::ImageHandle, ()> =
            slotmap::SlotMap::with_key();
        let handle = registry.insert(());

        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().size(100.0, 100.0));
        let button = ui.button(root, "", Style::new().size(40.0, 40.0).icon_button());
        ui.image(button, handle, Style::new().size(24.0, 24.0));
        ui.layout(Vec2::new(100.0, 100.0)).unwrap();

        let image_tint = |ui: &Ui| {
            ui.draw_list().commands.iter().find_map(|c| match c {
                DrawCommand::Image { tint, .. } => Some(*tint),
                _ => None,
            })
        };
        // Idle: the icon is untinted (white passthrough) and the button draws
        // no fill (only the image command, no FillRect).
        assert_eq!(image_tint(&ui), Some(Color::WHITE));
        assert!(
            !ui.draw_list()
                .commands
                .iter()
                .any(|c| matches!(c, DrawCommand::FillRect { .. })),
            "an icon button draws no background"
        );

        // Hovering the button recolors the icon to the accent (no background).
        ui.handle_input(InputEvent::PointerMoved(Vec2::new(20.0, 20.0)));
        assert_eq!(image_tint(&ui), Some(Theme::dark().icon_hover));
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
                tint: Color::WHITE,
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
        // interactive_at reads nothing retained (no PointerMoved needed) and
        // bubbles a hit on the label child up to the button - so a drag that
        // rebuilds every frame can still resolve the row under the cursor.
        assert_eq!(ui.interactive_at(Vec2::new(200.0, 150.0)), Some(under));
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
