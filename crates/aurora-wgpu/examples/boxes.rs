//! boxes - a minimal aurora-wgpu demo: a panel of colored, taffy-laid-out boxes.
//! Run with: `cargo run -p aurora-wgpu --example boxes`.

use std::sync::Arc;

use aurora::{Color, Style, Ui};
use aurora_wgpu::Renderer;
use glam::Vec2;
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    window::{Window, WindowId},
};

fn main() {
    let event_loop = EventLoop::new().unwrap();
    // A static UI redraws on demand, not every frame.
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
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        let window = Arc::new(
            event_loop
                .create_window(Window::default_attributes().with_title("aurora - boxes"))
                .unwrap(),
        );
        let size = window.inner_size();
        let renderer = Renderer::new(window.clone(), (size.width, size.height)).expect("renderer");
        let state = State {
            window,
            renderer,
            ui: build_ui(),
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
            WindowEvent::RedrawRequested => {
                let size = state.window.inner_size();
                state
                    .ui
                    .layout(Vec2::new(size.width as f32, size.height as f32))
                    .unwrap();
                let list = state.ui.draw_list();
                if let Err(err) = state.renderer.render(&list, Color::rgb(0.08, 0.08, 0.10)) {
                    eprintln!("render error: {err}");
                }
            }
            _ => {}
        }
    }
}

/// A root panel filling the window, holding a vertical column of colored boxes.
fn build_ui() -> Ui {
    let mut ui = Ui::new();
    let root = ui.root_panel(
        Style::new()
            .fill()
            .column()
            .padding(16.0)
            .gap(8.0)
            .clip()
            .background(Color::rgb(0.15, 0.16, 0.20)),
    );
    let colors = [
        Color::rgb(0.85, 0.30, 0.30),
        Color::rgb(0.30, 0.75, 0.40),
        Color::rgb(0.30, 0.55, 0.90),
        Color::rgb(0.85, 0.75, 0.30),
    ];
    for (i, color) in colors.into_iter().enumerate() {
        ui.panel(
            root,
            Style::new()
                .size(240.0, 40.0 + i as f32 * 16.0)
                .background(color),
        );
    }
    ui
}
