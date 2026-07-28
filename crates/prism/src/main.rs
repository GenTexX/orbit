//! prism: a live-theming tool for the atlas editor.
//!
//! It edits the theme document in the shared `settings.ron` - a palette of
//! variables and the tokens that reference them. Every edit is written straight
//! back to the file, and a running atlas hot-reloads it, so colors change live.
//!
//! Click a color swatch to open a color picker (SV square + hue + alpha + hex);
//! scalar variables and token literals use numeric fields; a token can be bound
//! to a variable (type its name) or hold a literal, toggled per token.
//!
//! Run atlas once first so the file exists, then `cargo run -p prism`.

use std::sync::Arc;
use std::time::Instant;

use aurora::picker::{ColorPicker, PickerRows, Press};
use aurora::{Color, Event, ImageHandle, InputEvent, Key, Rect, Style, Theme, Ui, WidgetId};
use aurora_wgpu::Renderer;
use glam::Vec2;
use spectrum::settings;
use spectrum::theme::{self, Bind, Kind, ThemeDoc, Value};
use winit::{
    application::ApplicationHandler,
    event::{ElementState, MouseButton, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{Key as WinitKey, ModifiersState, NamedKey},
    window::{Window, WindowId},
};

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let event_loop = EventLoop::new().unwrap();
    event_loop.set_control_flow(ControlFlow::Wait);
    event_loop.run_app(&mut App::default()).unwrap();
}

#[derive(Default)]
struct App {
    state: Option<State>,
}

/// Which theme value a control edits.
#[derive(Debug, Clone, PartialEq)]
enum Target {
    /// A variable's value, by name.
    Var(String),
    /// A token's literal value, by token name.
    Token(String),
}

/// An open color picker, and which theme value it edits. The picker itself -
/// its gradients, drag routing, and widgets - is aurora's.
struct Picker {
    target: Target,
    picker: ColorPicker,
}

/// Widget handles the app maps events back to after building the tree.
#[derive(Default)]
struct Rows {
    /// A color swatch button (click to open the picker) and its live fill.
    swatches: Vec<(WidgetId, Target)>,
    /// A scalar numeric field.
    scalars: Vec<(WidgetId, Target)>,
    /// A token's variable-name field: (widget, token name).
    var_names: Vec<(WidgetId, String)>,
    /// A token's Var/Lit toggle button: (widget, token name).
    toggles: Vec<(WidgetId, String)>,
    /// A remove-variable button: (widget, variable name).
    removes: Vec<(WidgetId, String)>,
    new_name: Option<WidgetId>,
    add_color: Option<WidgetId>,
    add_scalar: Option<WidgetId>,
    scroll: Option<WidgetId>,
    /// The bottom help line, and each token row's description (shown on hover).
    help: Option<WidgetId>,
    token_rows: Vec<(WidgetId, &'static str)>,
    /// The open picker's widgets (aurora fills these in when it builds it).
    pk: PickerRows,
    /// The picker's Done button.
    pk_done: Option<WidgetId>,
}

struct State {
    window: Arc<Window>,
    renderer: Renderer,
    ui: Ui,
    rows: Rows,
    doc: ThemeDoc,
    picker: Option<Picker>,
    /// The picker's gradient images, registered once and refilled in place.
    picker_images: [ImageHandle; 3],
    cursor: Vec2,
    modifiers: ModifiersState,
    save_pending: bool,
    last_save: Instant,
    /// Set when the tree must be rebuilt (structure changed / picker opened).
    dirty: bool,
    /// Set by a pointer move; the move is applied once per frame rather than per
    /// event. A mouse reports at 125-1000 Hz while the window presents at ~60, so
    /// doing the picker's regeneration per event did the same work many times
    /// over between two presented frames - and once that outpaced the report
    /// rate the event queue grew without bound and the picker lagged the cursor.
    pointer_moved: bool,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        let window = Arc::new(
            event_loop
                .create_window(Window::default_attributes().with_title("prism - atlas theming"))
                .unwrap(),
        );
        let size = window.inner_size();
        let mut renderer =
            Renderer::new(window.clone(), (size.width, size.height)).expect("renderer");
        // Register the picker's three gradients once; aurora refills them in
        // place as the color changes.
        let picker_images = ColorPicker::initial_bitmaps()
            .map(|b| renderer.register_image_rgba_unmipped(&b.rgba, b.width, b.height));
        let doc = settings::read().unwrap_or_default().theme;
        let mut state = State {
            window,
            renderer,
            ui: Ui::new(),
            rows: Rows::default(),
            doc,
            picker: None,
            picker_images,
            cursor: Vec2::ZERO,
            modifiers: ModifiersState::default(),
            save_pending: false,
            last_save: Instant::now(),
            dirty: false,
            pointer_moved: false,
        };
        state.rebuild();
        self.state = Some(state);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(state) = self.state.as_mut() else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                state.renderer.resize((size.width, size.height));
                state.window.request_redraw();
            }
            WindowEvent::CursorMoved { position, .. } => {
                // Record the position and let the redraw apply it: see
                // `pointer_moved` / `flush_pointer`.
                state.cursor = Vec2::new(position.x as f32, position.y as f32);
                state.pointer_moved = true;
                state.window.request_redraw();
            }
            WindowEvent::CursorLeft { .. } => {
                state.ui.handle_input(InputEvent::PointerLeft);
                state.window.request_redraw();
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let dy = match delta {
                    winit::event::MouseScrollDelta::LineDelta(_, y) => y * 48.0,
                    winit::event::MouseScrollDelta::PixelDelta(p) => p.y as f32,
                };
                state.ui.handle_input(InputEvent::Scroll(-dy));
                state.window.request_redraw();
            }
            WindowEvent::MouseInput {
                state: btn,
                button: MouseButton::Left,
                ..
            } => {
                state.flush_pointer();
                match btn {
                    ElementState::Pressed => {
                        state.ui.handle_input(InputEvent::PointerPressed);
                        // A press on a gradient starts (and applies) a drag.
                        state.press_picker();
                    }
                    ElementState::Released => {
                        if let Some(p) = state.picker.as_mut() {
                            p.picker.end_drag();
                        }
                        state.ui.handle_input(InputEvent::PointerReleased);
                        state.flush_save();
                    }
                }
                state.window.request_redraw();
            }
            WindowEvent::ModifiersChanged(m) => {
                state.modifiers = m.state();
                state.ui.set_shift(m.state().shift_key());
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                for ev in translate_key(&event, state.modifiers.control_key()) {
                    state.ui.handle_input(ev);
                }
                state.window.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                state.flush_pointer();
                state.react();
                if state.dirty {
                    let scroll = state
                        .rows
                        .scroll
                        .map(|s| state.ui.scroll_offset(s))
                        .unwrap_or(0.0);
                    state.rebuild();
                    if let Some(s) = state.rows.scroll {
                        state.ui.set_scroll_offset(s, scroll);
                    }
                    // The fresh Ui has no cursor, so nothing reads as hovered and
                    // the next click would not land; seed it so hover/click work
                    // without needing a mouse nudge first.
                    state.ui.set_cursor(state.cursor);
                    state.dirty = false;
                }
                if state.save_pending && state.last_save.elapsed().as_millis() >= 120 {
                    state.flush_save();
                }
                let size = state.window.inner_size();
                state
                    .ui
                    .layout(Vec2::new(size.width as f32, size.height as f32))
                    .unwrap();
                // With rects laid out, refresh the help line from the hovered row.
                state.update_help();
                let list = state.ui.draw_list();
                let clear = Color::rgb(0.06, 0.065, 0.08);
                if let Err(err) = state
                    .renderer
                    .render(&list, state.ui.font_system_mut(), clear)
                {
                    tracing::error!("render error: {err}");
                }
            }
            _ => {}
        }
    }
}

impl State {
    /// Rebuild the widget tree from the doc, adding the picker popup on top when
    /// one is open.
    fn rebuild(&mut self) {
        let (mut ui, mut rows) = build_ui(&self.doc);
        if let Some(open) = &self.picker {
            rows.pk = open.picker.build(&mut ui, &Theme::dark());
            // aurora hands back the popup's card, so an app can add its own
            // controls inside the picker - here, a Done button.
            if let Some(card) = rows.pk.card {
                rows.pk_done = Some(ui.button(
                    card,
                    "Done".to_string(),
                    Style::new().padding(4.0).foreground(Color::WHITE),
                ));
            }
        }
        self.ui = ui;
        self.rows = rows;
    }

    /// Show the description of the token row under the cursor in the help line.
    fn update_help(&mut self) {
        let Some(help) = self.rows.help else { return };
        let doc = self
            .rows
            .token_rows
            .iter()
            .find(|(row, _)| self.ui.rect(*row).is_some_and(|r| within(r, self.cursor)))
            .map(|(_, doc)| *doc)
            .unwrap_or("Hover a token to see what it affects.");
        self.ui.set_label(help, doc);
    }

    fn react(&mut self) {
        for event in self.ui.drain_events() {
            match event {
                Event::Submitted(id) => self.submitted(id),
                Event::Clicked(id) => self.clicked(id),
                _ => {}
            }
        }
    }

    fn submitted(&mut self, id: WidgetId) {
        // The picker's hex field.
        if Some(id) == self.rows.pk.hex {
            if let Some(rgba) = aurora::from_hex(&text_of(&self.ui, id)) {
                if let Some(open) = self.picker.as_mut() {
                    open.picker.set_rgba(rgba);
                }
                self.commit_color();
            }
            return;
        }
        // A scalar value field.
        if let Some((_, target)) = self.rows.scalars.iter().find(|(w, _)| *w == id).cloned() {
            if let Ok(v) = text_of(&self.ui, id).trim().parse::<f32>() {
                if let Some(slot) = self.value_mut(&target) {
                    *slot = Value::Scalar(v);
                }
                self.save_pending = true;
            }
            return;
        }
        // A token's variable-name field: rebind it.
        if let Some((_, token)) = self.rows.var_names.iter().find(|(w, _)| *w == id).cloned() {
            let name = text_of(&self.ui, id).trim().to_string();
            self.doc.tokens.insert(token, Bind::Var(name));
            self.save_pending = true;
            self.dirty = true;
        }
    }

    fn clicked(&mut self, id: WidgetId) {
        // Close the picker.
        if Some(id) == self.rows.pk_done {
            self.picker = None;
            self.dirty = true;
            return;
        }
        // Open the picker on a color swatch.
        if let Some((sw, target)) = self.rows.swatches.iter().find(|(w, _)| *w == id).cloned() {
            let rgba = match self.target_color(&target) {
                Some(c) => c,
                None => return,
            };
            let anchor = self
                .ui
                .rect(sw)
                .map(|r| r.pos + Vec2::new(0.0, r.size.y + 4.0))
                .unwrap_or(self.cursor);
            self.picker = Some(Picker {
                target,
                picker: ColorPicker::new(self.picker_images, anchor, rgba),
            });
            self.upload_picker_images();
            self.dirty = true;
            return;
        }
        // Add a variable using the typed name.
        if Some(id) == self.rows.add_color || Some(id) == self.rows.add_scalar {
            let name = self
                .rows
                .new_name
                .map(|w| text_of(&self.ui, w))
                .unwrap_or_default();
            let name = name.trim();
            if !name.is_empty() {
                let value = if Some(id) == self.rows.add_color {
                    Value::Color(0.5, 0.5, 0.5, 1.0)
                } else {
                    Value::Scalar(4.0)
                };
                self.doc.variables.insert(name.to_string(), value);
                self.save_pending = true;
                self.dirty = true;
            }
            return;
        }
        // Remove a variable.
        if let Some((_, name)) = self.rows.removes.iter().find(|(w, _)| *w == id).cloned() {
            self.doc.variables.remove(&name);
            self.save_pending = true;
            self.dirty = true;
            return;
        }
        // Toggle a token between a variable reference and a literal. The literal
        // keeps the token's declared KIND (a scalar token never becomes a color),
        // even when it was referencing a variable of the wrong kind.
        if let Some((_, token)) = self.rows.toggles.iter().find(|(w, _)| *w == id).cloned() {
            let next = match self.doc.tokens.get(&token) {
                Some(Bind::Lit(_)) => Bind::Var(
                    self.doc
                        .variables
                        .keys()
                        .next()
                        .cloned()
                        .unwrap_or_default(),
                ),
                Some(Bind::Var(_)) | None => {
                    Bind::Lit(literal_for(&token, self.doc.resolved(&token)))
                }
            };
            self.doc.tokens.insert(token, next);
            self.save_pending = true;
            self.dirty = true;
        }
    }

    /// Route a press while the picker is open: aurora reports whether it landed
    /// on a gradient (starting a drag), elsewhere inside the popup, or outside
    /// it - and an outside press dismisses the picker, except on a swatch, which
    /// switches to that target on release.
    fn press_picker(&mut self) {
        let (ui, rows, cursor) = (&self.ui, self.rows.pk, self.cursor);
        let Some(open) = self.picker.as_mut() else {
            return;
        };
        match open.picker.press(ui, &rows, cursor) {
            Press::Grabbed(_) => self.commit_color(),
            Press::Inside => {}
            Press::Outside => {
                let hit = self.ui.hit_test(self.cursor);
                let on_swatch = self.rows.swatches.iter().any(|(w, _)| Some(*w) == hit);
                if !on_swatch {
                    self.picker = None;
                    self.dirty = true;
                }
            }
        }
    }

    /// Apply a pending pointer move, if any: hand it to aurora and advance a
    /// live picker drag. Called once per frame (and before a button event, so a
    /// click acts on the position it was made at), never once per motion event.
    fn flush_pointer(&mut self) {
        if !self.pointer_moved {
            return;
        }
        self.pointer_moved = false;
        self.ui.handle_input(InputEvent::PointerMoved(self.cursor));
        if self.picker.as_ref().is_some_and(|p| p.picker.is_dragging()) {
            self.drag_picker();
        }
    }

    /// Apply the active gradient drag from the cursor.
    fn drag_picker(&mut self) {
        let (ui, rows, cursor) = (&self.ui, self.rows.pk, self.cursor);
        let changed = self
            .picker
            .as_mut()
            .is_some_and(|open| open.picker.drag_to(ui, &rows, cursor));
        if changed {
            self.commit_color();
        }
    }

    /// Write the picker's color to the doc, recolor its swatch, and refresh the
    /// gradient images - all in place, no rebuild.
    fn commit_color(&mut self) {
        let Some((target, rgba)) = self
            .picker
            .as_ref()
            .map(|p| (p.target.clone(), p.picker.rgba()))
        else {
            return;
        };
        if let Some(slot) = self.value_mut(&target) {
            *slot = Value::Color(rgba[0], rgba[1], rgba[2], rgba[3]);
        }
        if let Some((sw, _)) = self.rows.swatches.iter().find(|(_, t)| *t == target) {
            self.ui
                .set_background(*sw, Color::rgba(rgba[0], rgba[1], rgba[2], rgba[3]));
        }
        self.upload_picker_images();
        self.save_pending = true;
    }

    /// Upload whatever gradients aurora says changed since the last upload.
    fn upload_picker_images(&mut self) {
        let Some(open) = self.picker.as_mut() else {
            return;
        };
        for (handle, bitmap) in open.picker.dirty_bitmaps() {
            self.renderer
                .update_image_rgba(handle, &bitmap.rgba, bitmap.width, bitmap.height);
        }
    }

    /// The current RGBA of a target that holds a color.
    fn target_color(&self, target: &Target) -> Option<[f32; 4]> {
        let v = match target {
            Target::Var(name) => self.doc.variables.get(name).copied(),
            Target::Token(name) => match self.doc.tokens.get(name) {
                Some(Bind::Lit(v)) => Some(*v),
                _ => None,
            },
        };
        match v {
            Some(Value::Color(r, g, b, a)) => Some([r, g, b, a]),
            _ => None,
        }
    }

    fn value_mut(&mut self, target: &Target) -> Option<&mut Value> {
        match target {
            Target::Var(name) => self.doc.variables.get_mut(name),
            Target::Token(name) => match self.doc.tokens.get_mut(name) {
                Some(Bind::Lit(v)) => Some(v),
                _ => None,
            },
        }
    }

    /// Write the theme back, re-reading first so atlas's non-theme settings
    /// (grid/snap) are preserved.
    fn flush_save(&mut self) {
        if !self.save_pending {
            return;
        }
        let mut file = settings::read().unwrap_or_default();
        file.theme = self.doc.clone();
        if let Err(err) = settings::save(&file) {
            tracing::warn!("could not save theme: {err}");
        }
        self.save_pending = false;
        self.last_save = Instant::now();
    }
}

/// Whether `rect` contains `p`.
fn within(rect: Rect, p: Vec2) -> bool {
    p.x >= rect.pos.x
        && p.y >= rect.pos.y
        && p.x <= rect.pos.x + rect.size.x
        && p.y <= rect.pos.y + rect.size.y
}

fn text_of(ui: &Ui, id: WidgetId) -> String {
    match ui.kind(id) {
        aurora::WidgetKind::TextInput(s) => s.clone(),
        _ => String::new(),
    }
}

/// A literal for `token` of its declared kind, keeping `resolved` when it already
/// matches (so a scalar token's literal is always a scalar, never a color).
fn literal_for(token: &str, resolved: Option<Value>) -> Value {
    match (theme::kind_of(token), resolved) {
        (Kind::Scalar, Some(v @ Value::Scalar(_))) => v,
        (Kind::Color, Some(v @ Value::Color(..))) => v,
        (Kind::Scalar, _) => Value::Scalar(4.0),
        (Kind::Color, _) => Value::Color(0.5, 0.5, 0.5, 1.0),
    }
}

/// The display group a token belongs to (unknown tokens go under "Other").
fn group_of(token: &str) -> &'static str {
    theme::spec(token).map(|s| s.group).unwrap_or("Other")
}

fn translate_key(event: &winit::event::KeyEvent, ctrl: bool) -> Vec<InputEvent> {
    let named = match &event.logical_key {
        WinitKey::Named(NamedKey::Backspace) => Some(Key::Backspace),
        WinitKey::Named(NamedKey::Delete) => Some(Key::Delete),
        WinitKey::Named(NamedKey::ArrowLeft) if ctrl => Some(Key::WordLeft),
        WinitKey::Named(NamedKey::ArrowRight) if ctrl => Some(Key::WordRight),
        WinitKey::Named(NamedKey::ArrowLeft) => Some(Key::Left),
        WinitKey::Named(NamedKey::ArrowRight) => Some(Key::Right),
        WinitKey::Named(NamedKey::Home) => Some(Key::Home),
        WinitKey::Named(NamedKey::End) => Some(Key::End),
        WinitKey::Named(NamedKey::Enter) => Some(Key::Enter),
        WinitKey::Named(NamedKey::Tab) => Some(Key::Tab),
        _ => None,
    };
    if let Some(key) = named {
        return vec![InputEvent::Key(key)];
    }
    event
        .text
        .iter()
        .flat_map(|t| t.chars())
        .filter(|c| !c.is_control())
        .map(InputEvent::Text)
        .collect()
}

// --- UI ---

const SURFACE: Color = Color::rgb(0.10, 0.11, 0.13);
const CARD: Color = Color::rgb(0.14, 0.15, 0.18);
const FIELD: Color = Color::rgb(0.07, 0.075, 0.09);
const TEXT: Color = Color::rgb(0.92, 0.94, 0.98);
const DIM: Color = Color::rgb(0.55, 0.60, 0.70);
const ACCENT: Color = Color::rgb(0.30, 0.55, 0.90);

fn build_ui(doc: &ThemeDoc) -> (Ui, Rows) {
    let mut ui = Ui::new();
    ui.set_theme(Theme::dark());
    let mut rows = Rows::default();
    let root = ui.root_panel(Style::new().fill().column().background(SURFACE));
    let scroll = ui.panel(
        root,
        Style::new()
            .column()
            .grow(1.0)
            .padding(12.0)
            .gap(10.0)
            .scroll(),
    );
    rows.scroll = Some(scroll);

    ui.label(
        scroll,
        "Prism - live theming",
        Style::new().font_size(20.0).foreground(TEXT),
    );
    ui.label(
        scroll,
        "Edits save to settings.ron; a running atlas reloads them live.",
        Style::new().foreground(DIM),
    );

    section(&mut ui, scroll, "Variables");
    build_add_variable(&mut ui, scroll, &mut rows);
    for (name, value) in &doc.variables {
        build_variable(&mut ui, scroll, name, *value, &mut rows);
    }

    section(&mut ui, scroll, "Tokens");
    // Group tokens by their registry group (in a fixed order), with any unknown
    // token under "Other".
    let groups = theme::GROUPS
        .iter()
        .copied()
        .chain(std::iter::once("Other"));
    for group in groups {
        let mut members: Vec<(&String, &Bind)> = doc
            .tokens
            .iter()
            .filter(|(name, _)| group_of(name) == group)
            .collect();
        if members.is_empty() {
            continue;
        }
        members.sort_by_key(|(name, _)| name.as_str());
        group_header(&mut ui, scroll, group);
        for (name, bind) in members {
            build_token(&mut ui, scroll, name, bind, doc, &mut rows);
        }
    }

    // A fixed help line at the bottom shows the hovered token's description.
    let help = ui.label(
        root,
        "Hover a token to see what it affects.".to_string(),
        Style::new()
            .padding_x(10.0)
            .padding_y(5.0)
            .background(CARD)
            .foreground(DIM)
            .ellipsis(),
    );
    rows.help = Some(help);

    (ui, rows)
}

/// A small group sub-header inside the token list.
fn group_header(ui: &mut Ui, parent: WidgetId, title: &str) {
    ui.label(
        parent,
        title.to_string(),
        Style::new()
            .font_size(13.0)
            .foreground(TEXT)
            .margin_top(6.0),
    );
}

fn section(ui: &mut Ui, parent: WidgetId, title: &str) {
    ui.label(
        parent,
        title.to_string(),
        Style::new()
            .font_size(15.0)
            .foreground(ACCENT)
            .margin_top(8.0),
    );
}

fn build_add_variable(ui: &mut Ui, parent: WidgetId, rows: &mut Rows) {
    let row = ui.panel(parent, Style::new().row().gap(6.0).align_center());
    ui.label(row, "New:", Style::new().foreground(DIM).width(40.0));
    let name = ui.text_input(
        row,
        String::new(),
        value_field().grow(1.0).placeholder("variable name"),
    );
    rows.new_name = Some(name);
    rows.add_color = Some(ui.button(row, "+ Color", small_button()));
    rows.add_scalar = Some(ui.button(row, "+ Scalar", small_button()));
}

fn build_variable(ui: &mut Ui, parent: WidgetId, name: &str, value: Value, rows: &mut Rows) {
    let card = ui.panel(
        parent,
        Style::new()
            .row()
            .align_center()
            .gap(8.0)
            .padding(8.0)
            .background(CARD)
            .corner_radius(5.0),
    );
    ui.label(
        card,
        name.to_string(),
        Style::new().width(120.0).foreground(TEXT),
    );
    match value {
        Value::Color(r, g, b, a) => {
            let sw = swatch_button(ui, card, Color::rgba(r, g, b, a), 26.0);
            rows.swatches.push((sw, Target::Var(name.to_string())));
            ui.label(
                card,
                aurora::to_hex([r, g, b, a]),
                Style::new().grow(1.0).foreground(DIM),
            );
        }
        Value::Scalar(s) => {
            let field = ui.numeric_input(card, format!("{s}"), value_field().grow(1.0));
            rows.scalars.push((field, Target::Var(name.to_string())));
        }
    }
    let remove = ui.button(card, "x", small_button());
    rows.removes.push((remove, name.to_string()));
}

fn build_token(
    ui: &mut Ui,
    parent: WidgetId,
    name: &str,
    bind: &Bind,
    doc: &ThemeDoc,
    rows: &mut Rows,
) {
    let row = ui.panel(
        parent,
        Style::new()
            .row()
            .align_center()
            .gap(6.0)
            .padding(5.0)
            .background(CARD)
            .corner_radius(4.0),
    );
    if let Some(spec) = theme::spec(name) {
        rows.token_rows.push((row, spec.doc));
    }
    ui.label(
        row,
        name.to_string(),
        Style::new().width(130.0).foreground(TEXT),
    );

    let is_var = matches!(bind, Bind::Var(_));
    let is_color = theme::kind_of(name) == Kind::Color;
    // A leading preview keyed on the token's KIND (not the bind), so a scalar
    // token never shows a color swatch: color tokens get a swatch (a button when
    // it is an editable literal), scalar tokens get their resolved number.
    match bind {
        Bind::Lit(Value::Color(r, g, b, a)) => {
            let sw = swatch_button(ui, row, Color::rgba(*r, *g, *b, *a), 20.0);
            rows.swatches.push((sw, Target::Token(name.to_string())));
        }
        _ if is_color => {
            let c = match doc.resolved(name) {
                Some(Value::Color(r, g, b, a)) => Color::rgba(r, g, b, a),
                _ => Color::rgb(0.25, 0.26, 0.30),
            };
            ui.panel(
                row,
                Style::new()
                    .size(20.0, 20.0)
                    .background(c)
                    .corner_radius(3.0),
            );
        }
        _ => {
            // Scalar token: show the resolved number instead of a swatch.
            let n = match doc.resolved(name) {
                Some(Value::Scalar(s)) => format!("{s}"),
                _ => "-".to_string(),
            };
            ui.label(row, n, Style::new().width(20.0).foreground(DIM));
        }
    }

    let toggle = ui.button(row, if is_var { "var" } else { "lit" }, small_button());
    rows.toggles.push((toggle, name.to_string()));

    match bind {
        Bind::Var(var) => {
            let field = ui.text_input(
                row,
                var.clone(),
                value_field().grow(1.0).placeholder("variable"),
            );
            rows.var_names.push((field, name.to_string()));
        }
        Bind::Lit(Value::Scalar(s)) => {
            let field = ui.numeric_input(row, format!("{s}"), value_field().grow(1.0));
            rows.scalars.push((field, Target::Token(name.to_string())));
        }
        Bind::Lit(Value::Color(r, g, b, a)) => {
            ui.label(
                row,
                aurora::to_hex([*r, *g, *b, *a]),
                Style::new().grow(1.0).foreground(DIM),
            );
        }
    }
}

fn swatch_button(ui: &mut Ui, parent: WidgetId, color: Color, size: f32) -> WidgetId {
    ui.button(
        parent,
        "",
        Style::new()
            .size(size, size)
            .background(color)
            .corner_radius(4.0)
            .border(1.0, Color::rgb(0.28, 0.30, 0.36)),
    )
}

fn small_button() -> Style {
    Style::new()
        .padding_x(8.0)
        .padding_y(3.0)
        .background(Color::rgb(0.20, 0.22, 0.27))
        .foreground(TEXT)
        .corner_radius(4.0)
}

fn value_field() -> Style {
    Style::new()
        .padding(4.0)
        .background(FIELD)
        .foreground(TEXT)
        .corner_radius(4.0)
}
