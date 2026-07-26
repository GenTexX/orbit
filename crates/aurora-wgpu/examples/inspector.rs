//! inspector - the Milestone 2 target demo: a mock editor inspector.
//!
//! A docked panel of labeled, editable property rows plus a checkbox and a
//! button, plus a viewport placeholder - laid out with taffy, fully interactive
//! (click fields and type, toggle the checkbox, press Reset), redrawing
//! continuously so it can report its frame rate and blink the caret.
//!
//! Run with: `cargo run -p aurora-wgpu --example inspector`.
//! Profile with: `ORBIT_PROFILE=1 cargo run -p aurora-wgpu --example inspector`
//! then connect `puffin_viewer --url 127.0.0.1:8585`.

use std::sync::Arc;
use std::time::Instant;

use aurora::{Color, Event, InputEvent, Key, Style, Ui, WidgetId};
use aurora_wgpu::Renderer;
use glam::Vec2;
use winit::{
    application::ApplicationHandler,
    event::{ElementState, MouseButton, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{Key as WinitKey, NamedKey},
    window::{Window, WindowId},
};

const BACKDROP: Color = Color::rgb(0.06, 0.06, 0.08);
const PANEL: Color = Color::rgb(0.13, 0.14, 0.18);
const HEADING: Color = Color::rgb(0.96, 0.97, 1.0);
const SUBHEAD: Color = Color::rgb(0.55, 0.60, 0.72);
const SCENE_LIT: Color = Color::rgb(0.16, 0.18, 0.23);
const SCENE_DIM: Color = Color::rgb(0.09, 0.10, 0.12);

/// The caret is solid for this long after any edit, then blinks at this period.
const BLINK_MS: u128 = 530;

fn main() {
    let _profiler = init_profiling();
    let event_loop = EventLoop::new().unwrap();
    // Redraw continuously: the demo reports its frame rate and blinks the caret.
    event_loop.set_control_flow(ControlFlow::Poll);
    event_loop.run_app(&mut App::default()).unwrap();
}

/// Start the puffin profiler server when ORBIT_PROFILE is set (matching photon's
/// sandbox). Off otherwise, so every `profiling::scope!` stays a cheap no-op.
fn init_profiling() -> Option<puffin_http::Server> {
    std::env::var_os("ORBIT_PROFILE")?;
    puffin::set_scopes_on(true);
    match puffin_http::Server::new("0.0.0.0:8585") {
        Ok(server) => {
            eprintln!("puffin profiler on; connect with: puffin_viewer --url 127.0.0.1:8585");
            Some(server)
        }
        Err(err) => {
            eprintln!("failed to start puffin server: {err}");
            None
        }
    }
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
    /// Reset to now on each edit; the caret's blink phase is measured from it.
    blink_since: Instant,
    frames: u32,
    fps_since: Instant,
}

/// Handles the app reacts to after building the tree.
struct Ids {
    /// Editable rows, each with the default value the Reset button restores.
    fields: Vec<(WidgetId, &'static str)>,
    visible: WidgetId,
    reset: WidgetId,
    viewport: WidgetId,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        let window = Arc::new(
            event_loop
                .create_window(Window::default_attributes().with_title("aurora - inspector"))
                .unwrap(),
        );
        let size = window.inner_size();
        let renderer = Renderer::new(window.clone(), (size.width, size.height)).expect("renderer");
        let (ui, ids) = build_ui();
        let now = Instant::now();
        self.state = Some(State {
            window,
            renderer,
            ui,
            ids,
            blink_since: now,
            frames: 0,
            fps_since: now,
        });
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(state) = self.state.as_mut() else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => state.renderer.resize((size.width, size.height)),
            WindowEvent::CursorMoved { position, .. } => {
                let p = Vec2::new(position.x as f32, position.y as f32);
                state.ui.handle_input(InputEvent::PointerMoved(p));
            }
            WindowEvent::CursorLeft { .. } => state.ui.handle_input(InputEvent::PointerLeft),
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
                state.blink_since = Instant::now(); // solid caret on interaction
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                for ev in translate_key(&event) {
                    state.ui.handle_input(ev);
                }
                state.blink_since = Instant::now();
            }
            WindowEvent::RedrawRequested => state.draw(),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(state) = &self.state {
            state.window.request_redraw();
        }
    }
}

impl State {
    fn draw(&mut self) {
        // Blink: solid for the first half-period after an edit, then alternate.
        let caret_on = (self.blink_since.elapsed().as_millis() / BLINK_MS).is_multiple_of(2);
        self.ui.set_caret_on(caret_on);

        self.react();

        let size = self.window.inner_size();
        self.ui
            .layout(Vec2::new(size.width as f32, size.height as f32))
            .unwrap();
        let list = self.ui.draw_list();
        if let Err(err) = self
            .renderer
            .render(&list, self.ui.font_system_mut(), BACKDROP)
        {
            eprintln!("render error: {err}");
        }
        self.report_fps();
        // Mark the frame boundary for the profiler (a no-op when profiling off).
        profiling::finish_frame!();
    }

    /// Drain this frame's events and update the UI from them.
    fn react(&mut self) {
        for event in self.ui.drain_events() {
            match event {
                Event::Clicked(id) if id == self.ids.reset => {
                    for (field, default) in self.ids.fields.clone() {
                        self.ui.set_text_input(field, default);
                    }
                }
                Event::Toggled { id, checked } if id == self.ids.visible => {
                    let color = if checked { SCENE_LIT } else { SCENE_DIM };
                    self.ui.set_background(self.ids.viewport, color);
                }
                _ => {}
            }
        }
    }

    fn report_fps(&mut self) {
        self.frames += 1;
        let elapsed = self.fps_since.elapsed().as_secs_f32();
        if elapsed >= 1.0 {
            let fps = self.frames as f32 / elapsed;
            eprintln!("{} fps ({:.2} ms/frame)", fps as u32, 1000.0 / fps);
            self.frames = 0;
            self.fps_since = Instant::now();
        }
    }
}

/// Map a winit key press to Aurora input: a named editing key, or the typed
/// text as a run of character events.
fn translate_key(event: &winit::event::KeyEvent) -> Vec<InputEvent> {
    let named = match &event.logical_key {
        WinitKey::Named(NamedKey::Backspace) => Some(Key::Backspace),
        WinitKey::Named(NamedKey::Delete) => Some(Key::Delete),
        WinitKey::Named(NamedKey::ArrowLeft) => Some(Key::Left),
        WinitKey::Named(NamedKey::ArrowRight) => Some(Key::Right),
        WinitKey::Named(NamedKey::Home) => Some(Key::Home),
        WinitKey::Named(NamedKey::End) => Some(Key::End),
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

/// Build the inspector panel beside a viewport placeholder.
fn build_ui() -> (Ui, Ids) {
    let mut ui = Ui::new();
    let root = ui.root_panel(Style::new().fill().row().background(BACKDROP));

    // The docked inspector: a fixed-width column on the left, stretched to the
    // full window height by the root row.
    let inspector = ui.panel(
        root,
        Style::new()
            .column()
            .width(300.0)
            .padding(14.0)
            .gap(9.0)
            .background(PANEL),
    );
    ui.label(inspector, "Inspector", Style::new().foreground(HEADING));

    ui.label(
        inspector,
        "TRANSFORM",
        Style::new().foreground(SUBHEAD).size(120.0, 18.0),
    );
    let mut fields = Vec::new();
    fields.push((field_row(&mut ui, inspector, "Position X", "0.0"), "0.0"));
    fields.push((field_row(&mut ui, inspector, "Position Y", "0.0"), "0.0"));
    fields.push((field_row(&mut ui, inspector, "Rotation", "0"), "0"));

    ui.label(
        inspector,
        "NODE",
        Style::new().foreground(SUBHEAD).size(120.0, 18.0),
    );
    fields.push((field_row(&mut ui, inspector, "Name", "Player"), "Player"));

    // A checkbox (with a clickable caption) and a reset button.
    let visible_row = ui.panel(inspector, Style::new().row().gap(8.0).align_center());
    let visible = ui.checkbox(visible_row, true, "Visible", Style::new());

    let reset = ui.button(
        inspector,
        "Reset",
        Style::new().padding(8.0).foreground(Color::WHITE),
    );

    // The viewport placeholder fills the rest of the window.
    let viewport = ui.panel(
        root,
        Style::new().grow(1.0).padding(12.0).background(SCENE_LIT),
    );
    ui.label(viewport, "Scene viewport", Style::new().foreground(SUBHEAD));

    (
        ui,
        Ids {
            fields,
            visible,
            reset,
            viewport,
        },
    )
}

/// A property row: a fixed-width label column and an editable field that fills
/// the rest of the row. Returns the field's id.
fn field_row(ui: &mut Ui, parent: WidgetId, label: &str, value: &str) -> WidgetId {
    let row = ui.panel(parent, Style::new().row().gap(10.0).align_center());
    ui.label(row, label, Style::new().size(96.0, 20.0));
    ui.text_input(row, value, Style::new().grow(1.0).padding(6.0))
}
