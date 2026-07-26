//! controls - an interactive aurora-wgpu demo showcasing every input widget:
//! a button, a checkbox, a plain text field (with a placeholder), masked fields
//! (integer/decimal/hex), a disabled field and button, a multi-line text area,
//! and a slider - all wired to real pointer and keyboard input. Click the button
//! to bump a counter; toggle the checkbox to show an accent box; click a field
//! and type; Tab moves between fields; shift+arrows extend a selection and
//! ctrl+a/c/x/v select-all/copy/cut/paste; in the text area Enter adds a newline
//! and Up/Down move by line.
//! Run with: `cargo run -p aurora-wgpu --example controls`.

use std::sync::Arc;

use aurora::{Color, Event, InputEvent, Key, Mask, Style, Ui, WidgetId};
use aurora_wgpu::Renderer;
use glam::Vec2;
use winit::{
    application::ApplicationHandler,
    event::{ElementState, MouseButton, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{Key as WinitKey, ModifiersState, NamedKey},
    window::{Window, WindowId},
};

fn main() {
    let event_loop = EventLoop::new().unwrap();
    // Redraw on demand (after input), not continuously.
    event_loop.set_control_flow(ControlFlow::Wait);
    event_loop.run_app(&mut App::default()).unwrap();
}

#[derive(Default)]
struct App {
    state: Option<State>,
}

struct State {
    window: Arc<Window>,
    renderer: Renderer,
    ui: Ui,
    ids: Ids,
    clicks: u32,
    /// The current keyboard modifiers (for shift-selection and ctrl shortcuts).
    modifiers: ModifiersState,
    /// The OS clipboard for ctrl+c/x/v, or `None` if it could not be opened.
    clipboard: Option<arboard::Clipboard>,
}

/// Handles to the widgets the app reacts to after building the tree.
struct Ids {
    button: WidgetId,
    clicks_label: WidgetId,
    checkbox: WidgetId,
    accent: WidgetId,
    slider: WidgetId,
    slider_label: WidgetId,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        let window = Arc::new(
            event_loop
                .create_window(Window::default_attributes().with_title("aurora - controls"))
                .unwrap(),
        );
        let size = window.inner_size();
        let renderer = Renderer::new(window.clone(), (size.width, size.height)).expect("renderer");
        let (ui, ids) = build_ui();
        let state = State {
            window,
            renderer,
            ui,
            ids,
            clicks: 0,
            modifiers: ModifiersState::default(),
            clipboard: arboard::Clipboard::new().ok(),
        };
        state.window.request_redraw();
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
            // Translate winit pointer events into Aurora's own input vocabulary.
            WindowEvent::CursorMoved { position, .. } => {
                let p = Vec2::new(position.x as f32, position.y as f32);
                state.ui.handle_input(InputEvent::PointerMoved(p));
                state.window.request_redraw();
            }
            WindowEvent::CursorLeft { .. } => {
                state.ui.handle_input(InputEvent::PointerLeft);
                state.window.request_redraw();
            }
            WindowEvent::MouseInput {
                state: btn_state,
                button: MouseButton::Left,
                ..
            } => {
                let ev = match btn_state {
                    ElementState::Pressed => InputEvent::PointerPressed,
                    ElementState::Released => InputEvent::PointerReleased,
                };
                state.ui.handle_input(ev);
                state.window.request_redraw();
            }
            // Track modifiers so shift extends a selection and ctrl arms the
            // clipboard shortcuts.
            WindowEvent::ModifiersChanged(m) => {
                state.modifiers = m.state();
                state.ui.set_shift(m.state().shift_key());
            }
            // Translate winit key presses into Aurora text/key events. Ctrl+a/c/
            // x/v are intercepted as clipboard shortcuts first.
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                if !clipboard_shortcut(state, &event) {
                    for ev in translate_key(&event, state.modifiers.control_key()) {
                        state.ui.handle_input(ev);
                    }
                }
                state.window.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                react(state);
                let size = state.window.inner_size();
                state
                    .ui
                    .layout(Vec2::new(size.width as f32, size.height as f32))
                    .unwrap();
                let list = state.ui.draw_list();
                let clear = Color::rgb(0.08, 0.08, 0.10);
                if let Err(err) = state
                    .renderer
                    .render(&list, state.ui.font_system_mut(), clear)
                {
                    eprintln!("render error: {err}");
                }
            }
            _ => {}
        }
    }
}

/// Drain this frame's semantic events and update the UI from them.
fn react(state: &mut State) {
    for event in state.ui.drain_events() {
        match event {
            Event::Clicked(id) if id == state.ids.button => {
                state.clicks += 1;
                state
                    .ui
                    .set_label(state.ids.clicks_label, format!("Clicks: {}", state.clicks));
            }
            Event::Toggled { id, checked } if id == state.ids.checkbox => {
                let color = if checked {
                    Color::rgb(0.30, 0.55, 0.90)
                } else {
                    Color::TRANSPARENT
                };
                state.ui.set_background(state.ids.accent, color);
            }
            Event::SliderChanged { id, value } if id == state.ids.slider => {
                state
                    .ui
                    .set_label(state.ids.slider_label, format!("{value:.0}"));
            }
            _ => {}
        }
    }
}

/// Handle ctrl+a/c/x/v on the focused field, backed by the OS clipboard.
/// Returns whether the key was a (handled) clipboard shortcut.
fn clipboard_shortcut(state: &mut State, event: &winit::event::KeyEvent) -> bool {
    if !state.modifiers.control_key() {
        return false;
    }
    let WinitKey::Character(c) = &event.logical_key else {
        return false;
    };
    match c.as_str() {
        "a" | "A" => state.ui.select_all(),
        "c" | "C" => {
            if let (Some(text), Some(cb)) = (state.ui.selected_text(), state.clipboard.as_mut()) {
                let _ = cb.set_text(text);
            }
        }
        "x" | "X" => {
            if let (Some(text), Some(cb)) = (state.ui.selected_text(), state.clipboard.as_mut()) {
                let _ = cb.set_text(text);
                state.ui.delete_selection();
            }
        }
        "v" | "V" => {
            if let Some(text) = state.clipboard.as_mut().and_then(|cb| cb.get_text().ok()) {
                state.ui.insert_str(&text);
            }
        }
        _ => return false,
    }
    true
}

/// Map a winit key press to Aurora input: a named editing key, or the typed
/// text as a run of character events. `ctrl` upgrades the arrows to word motion.
fn translate_key(event: &winit::event::KeyEvent, ctrl: bool) -> Vec<InputEvent> {
    let named = match &event.logical_key {
        WinitKey::Named(NamedKey::Backspace) => Some(Key::Backspace),
        WinitKey::Named(NamedKey::Delete) => Some(Key::Delete),
        WinitKey::Named(NamedKey::ArrowLeft) if ctrl => Some(Key::WordLeft),
        WinitKey::Named(NamedKey::ArrowRight) if ctrl => Some(Key::WordRight),
        WinitKey::Named(NamedKey::ArrowLeft) => Some(Key::Left),
        WinitKey::Named(NamedKey::ArrowRight) => Some(Key::Right),
        WinitKey::Named(NamedKey::ArrowUp) => Some(Key::Up),
        WinitKey::Named(NamedKey::ArrowDown) => Some(Key::Down),
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

/// Build the control panel and return the handles the app reacts to.
fn build_ui() -> (Ui, Ids) {
    let mut ui = Ui::new();
    let root = ui.root_panel(
        Style::new()
            .fill()
            .column()
            .padding(16.0)
            .gap(12.0)
            .background(Color::rgb(0.13, 0.14, 0.18)),
    );

    ui.label(
        root,
        "Aurora - every input widget",
        Style::new()
            .foreground(Color::rgb(0.95, 0.95, 0.98))
            .font_size(20.0),
    );

    // A button next to a live counter label. Rounded, with a subtle border, to
    // show off corner_radius()/border().
    let button_row = ui.panel(root, Style::new().row().gap(10.0));
    let button = ui.button(
        button_row,
        "Click me",
        Style::new()
            .padding(8.0)
            .foreground(Color::WHITE)
            .corner_radius(6.0)
            .border(1.0, Color::rgb(0.45, 0.55, 0.75)),
    );
    let clicks_label = ui.label(button_row, "Clicks: 0", Style::new().padding(8.0));

    // A checkbox next to its caption.
    let check_row = ui.panel(root, Style::new().row().gap(8.0).padding(4.0));
    let checkbox = ui.checkbox(check_row, false, Style::new());
    ui.label(check_row, "Show accent box", Style::new().padding(2.0));

    // An editable text field next to its caption. Click it and type. A
    // placeholder shows while it is empty; Tab moves between the fields.
    let field_row = ui.panel(root, Style::new().row().gap(8.0).align_center());
    ui.label(field_row, "Name:", Style::new().padding(6.0).width(64.0));
    ui.text_input(
        field_row,
        "",
        Style::new()
            .size(240.0, 28.0)
            .padding(6.0)
            .corner_radius(4.0)
            .placeholder("type a name..."),
    );

    // Masked inputs: each rejects any keystroke that would break its mask. The
    // fields are rounded (corner_radius) to show the rounded-rect rendering.
    let field = |px: f32| Style::new().size(px, 28.0).padding(6.0).corner_radius(4.0);
    for (caption, mask, hint) in [
        ("Integer:", Mask::Integer, "-1234"),
        ("Decimal:", Mask::Decimal, "-3.14"),
        ("Hex:", Mask::Hex, "1a2b3c"),
    ] {
        let row = ui.panel(root, Style::new().row().gap(8.0).align_center());
        ui.label(row, caption, Style::new().padding(6.0).width(64.0));
        ui.text_input(row, "", field(160.0).mask(mask).placeholder(hint));
    }

    // A disabled field and button: dimmed and inert (no hover, focus, or click).
    let disabled_row = ui.panel(root, Style::new().row().gap(8.0).align_center());
    ui.label(
        disabled_row,
        "Disabled:",
        Style::new().padding(6.0).width(64.0),
    );
    ui.text_input(disabled_row, "read only", field(160.0).disabled());
    ui.button(
        disabled_row,
        "Save",
        Style::new()
            .padding(8.0)
            .disabled()
            .foreground(Color::WHITE),
    );

    // A multi-line text area: Enter inserts a newline, Up/Down move by line,
    // Home/End act on the line, and a selection highlights each line. It grows
    // as lines are added.
    ui.label(
        root,
        "Notes (text area):",
        Style::new().foreground(Color::rgb(0.7, 0.72, 0.8)),
    );
    ui.text_area(
        root,
        "A multi-line text area.\nEnter adds a newline.\nUp/Down move by line.",
        Style::new().width(320.0).padding(6.0),
    );

    // A slider with a live value readout.
    let slider_row = ui.panel(root, Style::new().row().gap(8.0).align_center());
    let slider = ui.slider(slider_row, 50.0, 0.0, 100.0, Style::new().size(220.0, 24.0));
    let slider_label = ui.label(slider_row, "50", Style::new().padding(6.0));

    // The box the checkbox reveals (transparent until toggled on).
    let accent = ui.panel(
        root,
        Style::new()
            .size(220.0, 44.0)
            .corner_radius(10.0)
            .border(2.0, Color::rgb(0.30, 0.55, 0.90)),
    );

    (
        ui,
        Ids {
            button,
            clicks_label,
            checkbox,
            accent,
            slider,
            slider_label,
        },
    )
}
