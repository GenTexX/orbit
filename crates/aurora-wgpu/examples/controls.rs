//! controls - an interactive aurora-wgpu demo: a button and a checkbox wired to
//! real pointer input. Click the button to bump a counter; toggle the checkbox
//! to show an accent box. Run with: `cargo run -p aurora-wgpu --example controls`.

use std::sync::Arc;

use aurora::{Color, Event, InputEvent, Style, Ui, WidgetId};
use aurora_wgpu::Renderer;
use glam::Vec2;
use winit::{
    application::ApplicationHandler,
    event::{ElementState, MouseButton, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
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
}

/// Handles to the widgets the app reacts to after building the tree.
struct Ids {
    button: WidgetId,
    clicks_label: WidgetId,
    checkbox: WidgetId,
    accent: WidgetId,
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
            _ => {}
        }
    }
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
        "Aurora - interactive controls",
        Style::new().foreground(Color::rgb(0.95, 0.95, 0.98)),
    );

    // A button next to a live counter label.
    let button_row = ui.panel(root, Style::new().row().gap(10.0));
    let button = ui.button(
        button_row,
        "Click me",
        Style::new().padding(8.0).foreground(Color::WHITE),
    );
    let clicks_label = ui.label(button_row, "Clicks: 0", Style::new().padding(8.0));

    // A checkbox next to its caption.
    let check_row = ui.panel(root, Style::new().row().gap(8.0).padding(4.0));
    let checkbox = ui.checkbox(check_row, false, Style::new());
    ui.label(check_row, "Show accent box", Style::new().padding(2.0));

    // The box the checkbox reveals (transparent until toggled on).
    let accent = ui.panel(root, Style::new().size(220.0, 44.0));

    (
        ui,
        Ids {
            button,
            clicks_label,
            checkbox,
            accent,
        },
    )
}
