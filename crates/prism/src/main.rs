//! prism: a live-theming tool for the atlas editor.
//!
//! It edits the theme document in the shared `settings.ron` - a palette of
//! variables and the tokens that reference them. Every edit is written straight
//! back to the file, and a running atlas hot-reloads it, so colors change live.
//!
//! Variables are edited with RGBA sliders (drag to recolor); scalar variables
//! and token literals use numeric fields. A token can be bound to a variable
//! (type its name) or hold a literal, toggled per token.
//!
//! Run atlas once first so the file exists, then `cargo run -p prism`.

mod model;

use std::sync::Arc;
use std::time::Instant;

use aurora::{Color, Event, InputEvent, Key, Style, Theme, Ui, WidgetId};
use aurora_wgpu::Renderer;
use glam::Vec2;
use model::{Bind, ThemeDoc, Value};
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

/// Widget handles the app maps events back to after building the tree.
#[derive(Default)]
struct Rows {
    /// A color channel slider: (widget, which value, channel 0..=3).
    channels: Vec<(WidgetId, Target, usize)>,
    /// A live swatch to recolor as its value changes: (widget, which value).
    swatches: Vec<(WidgetId, Target)>,
    /// A scalar numeric field: (widget, which value).
    scalars: Vec<(WidgetId, Target)>,
    /// A token's variable-name field: (widget, token name).
    var_names: Vec<(WidgetId, String)>,
    /// A token's Var/Lit toggle button: (widget, token name).
    toggles: Vec<(WidgetId, String)>,
    /// A remove-variable button: (widget, variable name).
    removes: Vec<(WidgetId, String)>,
    /// The new-variable name field, and the two "add" buttons.
    new_name: Option<WidgetId>,
    add_color: Option<WidgetId>,
    add_scalar: Option<WidgetId>,
    /// The scroll panel (its offset is preserved across rebuilds).
    scroll: Option<WidgetId>,
}

struct State {
    window: Arc<Window>,
    renderer: Renderer,
    ui: Ui,
    rows: Rows,
    /// The theme being edited.
    doc: ThemeDoc,
    modifiers: ModifiersState,
    /// Set when the doc changed; a throttled save flushes it to disk.
    save_pending: bool,
    last_save: Instant,
    /// Set when the tree must be rebuilt (structure changed).
    dirty: bool,
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
        let renderer = Renderer::new(window.clone(), (size.width, size.height)).expect("renderer");
        let doc = model::read().unwrap_or_default().theme;
        let (ui, rows) = build_ui(&doc);
        self.state = Some(State {
            window,
            renderer,
            ui,
            rows,
            doc,
            modifiers: ModifiersState::default(),
            save_pending: false,
            last_save: Instant::now(),
            dirty: false,
        });
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
                state.ui.handle_input(InputEvent::PointerMoved(Vec2::new(
                    position.x as f32,
                    position.y as f32,
                )));
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
                let ev = match btn {
                    ElementState::Pressed => InputEvent::PointerPressed,
                    ElementState::Released => InputEvent::PointerReleased,
                };
                state.ui.handle_input(ev);
                // A release flushes any pending save immediately (the drag ended).
                if btn == ElementState::Released {
                    state.flush_save();
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
                state.react();
                if state.dirty {
                    let scroll = state
                        .rows
                        .scroll
                        .map(|s| state.ui.scroll_offset(s))
                        .unwrap_or(0.0);
                    let (ui, rows) = build_ui(&state.doc);
                    state.ui = ui;
                    state.rows = rows;
                    if let Some(s) = state.rows.scroll {
                        state.ui.set_scroll_offset(s, scroll);
                    }
                    state.dirty = false;
                }
                // Throttled save so a slider drag does not thrash the disk.
                if state.save_pending && state.last_save.elapsed().as_millis() >= 120 {
                    state.flush_save();
                }
                let size = state.window.inner_size();
                state
                    .ui
                    .layout(Vec2::new(size.width as f32, size.height as f32))
                    .unwrap();
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
    /// Drain this frame's events and apply them to the doc.
    fn react(&mut self) {
        for event in self.ui.drain_events() {
            match event {
                Event::SliderChanged { id, value } => self.slider_changed(id, value),
                Event::Submitted(id) => self.submitted(id),
                Event::Clicked(id) => self.clicked(id),
                _ => {}
            }
        }
    }

    /// A color-channel slider moved: update the value in place (no rebuild) and
    /// recolor its swatch live.
    fn slider_changed(&mut self, id: WidgetId, value: f32) {
        let Some((_, target, ch)) = self.rows.channels.iter().find(|(w, ..)| *w == id).cloned()
        else {
            return;
        };
        if let Some(Value::Color(r, g, b, a)) = self.value_mut(&target) {
            let mut c = [*r, *g, *b, *a];
            c[ch] = value;
            (*r, *g, *b, *a) = (c[0], c[1], c[2], c[3]);
            let color = Color::rgba(c[0], c[1], c[2], c[3]);
            if let Some((sw, _)) = self.rows.swatches.iter().find(|(_, t)| *t == target) {
                self.ui.set_background(*sw, color);
            }
            self.save_pending = true;
        }
    }

    /// A numeric/text field committed.
    fn submitted(&mut self, id: WidgetId) {
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
        // A token's variable-name field: rebind it (rebuild for the swatch).
        if let Some((_, token)) = self.rows.var_names.iter().find(|(w, _)| *w == id).cloned() {
            let name = text_of(&self.ui, id).trim().to_string();
            self.doc.tokens.insert(token, Bind::Var(name));
            self.save_pending = true;
            self.dirty = true;
        }
    }

    fn clicked(&mut self, id: WidgetId) {
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
        // Toggle a token between a variable reference and a literal.
        if let Some((_, token)) = self.rows.toggles.iter().find(|(w, _)| *w == id).cloned() {
            let next = match self.doc.tokens.get(&token) {
                Some(Bind::Var(_)) | None => {
                    // Become a literal of whatever it currently resolves to.
                    let v = self
                        .doc
                        .resolved(&token)
                        .unwrap_or(Value::Color(0.5, 0.5, 0.5, 1.0));
                    Bind::Lit(v)
                }
                Some(Bind::Lit(_)) => {
                    // Reference the first variable (or an empty name to fill in).
                    let first = self
                        .doc
                        .variables
                        .keys()
                        .next()
                        .cloned()
                        .unwrap_or_default();
                    Bind::Var(first)
                }
            };
            self.doc.tokens.insert(token, next);
            self.save_pending = true;
            self.dirty = true;
        }
    }

    /// A mutable handle to the value a target points at (a variable's value, or a
    /// token's literal).
    fn value_mut(&mut self, target: &Target) -> Option<&mut Value> {
        match target {
            Target::Var(name) => self.doc.variables.get_mut(name),
            Target::Token(name) => match self.doc.tokens.get_mut(name) {
                Some(Bind::Lit(v)) => Some(v),
                _ => None,
            },
        }
    }

    /// Write the theme back to the shared settings file, re-reading first so
    /// atlas's non-theme settings (grid/snap) are preserved.
    fn flush_save(&mut self) {
        if !self.save_pending {
            return;
        }
        let mut settings = model::read().unwrap_or_default();
        settings.theme = self.doc.clone();
        match model::save(&settings) {
            Ok(()) => tracing::debug!("theme saved"),
            Err(err) => tracing::warn!("could not save theme: {err}"),
        }
        self.save_pending = false;
        self.last_save = Instant::now();
    }
}

/// A text input's current contents (empty for a non-input widget).
fn text_of(ui: &Ui, id: WidgetId) -> String {
    match ui.kind(id) {
        aurora::WidgetKind::TextInput(s) => s.clone(),
        _ => String::new(),
    }
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

    section_header(&mut ui, scroll, "Variables");
    build_add_variable(&mut ui, scroll, &mut rows);
    for (name, value) in &doc.variables {
        build_variable(&mut ui, scroll, name, *value, &mut rows);
    }

    section_header(&mut ui, scroll, "Tokens");
    for (name, bind) in &doc.tokens {
        build_token(&mut ui, scroll, name, bind, doc, &mut rows);
    }

    (ui, rows)
}

fn section_header(ui: &mut Ui, parent: WidgetId, title: &str) {
    ui.label(
        parent,
        title.to_string(),
        Style::new()
            .font_size(15.0)
            .foreground(ACCENT)
            .margin_top(8.0),
    );
}

/// The "add variable" row: a name field and Color/Scalar buttons.
fn build_add_variable(ui: &mut Ui, parent: WidgetId, rows: &mut Rows) {
    let row = ui.panel(parent, Style::new().row().gap(6.0).align_center());
    ui.label(row, "New:", Style::new().foreground(DIM).width(40.0));
    let name = ui.text_input(
        row,
        String::new(),
        Style::new()
            .grow(1.0)
            .padding(4.0)
            .background(FIELD)
            .foreground(TEXT)
            .corner_radius(4.0)
            .placeholder("variable name"),
    );
    rows.new_name = Some(name);
    rows.add_color = Some(ui.button(row, "+ Color", small_button()));
    rows.add_scalar = Some(ui.button(row, "+ Scalar", small_button()));
}

/// A variable: its name, an editor (RGBA sliders + swatch, or a scalar field),
/// and a remove button.
fn build_variable(ui: &mut Ui, parent: WidgetId, name: &str, value: Value, rows: &mut Rows) {
    let card = ui.panel(
        parent,
        Style::new()
            .column()
            .gap(5.0)
            .padding(8.0)
            .background(CARD)
            .corner_radius(5.0),
    );
    let head = ui.panel(card, Style::new().row().align_center().gap(6.0));
    ui.label(
        head,
        name.to_string(),
        Style::new().grow(1.0).foreground(TEXT),
    );
    let remove = ui.button(head, "x", small_button());
    rows.removes.push((remove, name.to_string()));

    let target = Target::Var(name.to_string());
    match value {
        Value::Color(r, g, b, a) => {
            let body = ui.panel(card, Style::new().row().gap(8.0).align_center());
            let sw = ui.panel(
                body,
                Style::new()
                    .size(40.0, 40.0)
                    .background(Color::rgba(r, g, b, a))
                    .corner_radius(4.0),
            );
            rows.swatches.push((sw, target.clone()));
            let cols = ui.panel(body, Style::new().column().grow(1.0).gap(3.0));
            for (ch, (chan_name, v)) in [("R", r), ("G", g), ("B", b), ("A", a)]
                .into_iter()
                .enumerate()
            {
                let line = ui.panel(cols, Style::new().row().align_center().gap(6.0));
                ui.label(line, chan_name, Style::new().width(14.0).foreground(DIM));
                let slider = ui.slider(line, v, 0.0, 1.0, Style::new().grow(1.0).height(16.0));
                rows.channels.push((slider, target.clone(), ch));
            }
        }
        Value::Scalar(s) => {
            let body = ui.panel(card, Style::new().row().align_center().gap(6.0));
            ui.label(body, "Value", Style::new().width(48.0).foreground(DIM));
            let field = ui.numeric_input(body, format!("{s}"), value_field());
            rows.scalars.push((field, target));
        }
    }
}

/// A token: its name, a swatch of its resolved value, a Var/Lit toggle, and the
/// bind editor (a variable-name field, or a literal editor).
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
    ui.label(
        row,
        name.to_string(),
        Style::new().width(130.0).foreground(TEXT),
    );

    // A swatch of the resolved value (grey for a scalar or unresolved).
    let resolved = doc.resolved(name);
    let swatch_color = match resolved {
        Some(Value::Color(r, g, b, a)) => Color::rgba(r, g, b, a),
        _ => Color::rgb(0.25, 0.26, 0.30),
    };
    ui.panel(
        row,
        Style::new()
            .size(20.0, 20.0)
            .background(swatch_color)
            .corner_radius(3.0),
    );

    let is_var = matches!(bind, Bind::Var(_));
    let toggle = ui.button(row, if is_var { "var" } else { "lit" }, small_button());
    rows.toggles.push((toggle, name.to_string()));

    match bind {
        Bind::Var(var) => {
            let field = ui.text_input(
                row,
                var.clone(),
                value_field().grow(1.0).placeholder("variable name"),
            );
            rows.var_names.push((field, name.to_string()));
        }
        Bind::Lit(Value::Scalar(s)) => {
            let field = ui.numeric_input(row, format!("{s}"), value_field().grow(1.0));
            rows.scalars.push((field, Target::Token(name.to_string())));
        }
        Bind::Lit(Value::Color(r, g, b, a)) => {
            // A compact literal color editor: one RGBA slider stack inline.
            let target = Target::Token(name.to_string());
            let sw = ui.panel(
                row,
                Style::new()
                    .size(20.0, 20.0)
                    .background(Color::rgba(*r, *g, *b, *a))
                    .corner_radius(3.0),
            );
            rows.swatches.push((sw, target.clone()));
            let cols = ui.panel(row, Style::new().column().grow(1.0).gap(2.0));
            for (ch, v) in [*r, *g, *b, *a].into_iter().enumerate() {
                let slider = ui.slider(cols, v, 0.0, 1.0, Style::new().grow(1.0).height(12.0));
                rows.channels.push((slider, target.clone(), ch));
            }
        }
    }
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
