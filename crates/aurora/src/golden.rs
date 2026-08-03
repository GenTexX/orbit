//! A deterministic, GPU-free description of what a `Ui` would draw.
//!
//! aurora and the apps built on it have no visual regression net at all: theming
//! and layout changes can only be checked by eye, and several recent ones could
//! not be verified headlessly. Pixels are the obvious answer and the wrong one -
//! they need a device, they differ between drivers, and a diff of two images
//! tells you nothing about what changed.
//!
//! A draw list does not have those problems. It is already what aurora produces
//! (ADR 0015), it already derives `PartialEq`, and rendered as text it is
//! diffable in the way a test failure needs: a line per command, coordinates
//! rounded to whole pixels so a layout that is stable to the eye is stable to
//! the test.
//!
//! This is the snapshot half. What compares a snapshot against a committed file
//! belongs to whoever is testing, because only they know where to put it.

use crate::draw::{DrawCommand, DrawList};

/// Render a draw list as one line per command, rounded to whole pixels.
///
/// Rounded on purpose: a layout that moves by a hundredth of a pixel when a
/// font metric changes is not a regression anybody wants a red test for, and a
/// snapshot that churns is a snapshot people stop reading.
pub fn describe(list: &DrawList) -> String {
    let mut out = String::new();
    for command in &list.commands {
        out.push_str(&describe_command(command));
        out.push('\n');
    }
    out
}

fn describe_command(command: &DrawCommand) -> String {
    match command {
        DrawCommand::FillRect { rect, color } => {
            format!("fill      {} {}", rect_of(rect), rgba(color))
        }
        DrawCommand::RoundedRect {
            rect,
            color,
            radius,
            border_width,
            border_color,
            ..
        } => format!(
            "rounded   {} {} r{:.0},{:.0},{:.0},{:.0} b{:.0} {}",
            rect_of(rect),
            rgba(color),
            radius[0],
            radius[1],
            radius[2],
            radius[3],
            border_width,
            rgba(border_color)
        ),
        // Glyph positions are font-metric detail that churns; how many there
        // are, and where the run starts, is what a layout test is about.
        DrawCommand::Text { glyphs, color } => {
            let at = glyphs.first().map(|g| (g.x, g.y)).unwrap_or((0.0, 0.0));
            format!(
                "text      {:>5.0},{:>5.0} {:>3} glyphs {}",
                at.0,
                at.1,
                glyphs.len(),
                rgba(color)
            )
        }
        DrawCommand::Image { rect, .. } => format!("image     {}", rect_of(rect)),
        DrawCommand::PushClip { rect } => format!("clip      {}", rect_of(rect)),
        DrawCommand::PopClip => "unclip".to_string(),
    }
}

fn rect_of(rect: &crate::Rect) -> String {
    format!(
        "{:>5.0},{:>5.0} {:>5.0}x{:<5.0}",
        rect.pos.x, rect.pos.y, rect.size.x, rect.size.y
    )
}

fn rgba(color: &crate::Color) -> String {
    format!(
        "#{:02x}{:02x}{:02x}{:02x}",
        (color.r * 255.0).round() as u8,
        (color.g * 255.0).round() as u8,
        (color.b * 255.0).round() as u8,
        (color.a * 255.0).round() as u8
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Color, Style, Ui};
    use glam::Vec2;

    #[test]
    fn a_layout_describes_itself_the_same_way_twice() {
        // The property the whole idea rests on: build the same tree twice and
        // get the same text, so a diff means a change rather than noise.
        let build = || {
            let mut ui = Ui::new();
            let root = ui.root_panel(Style::new().column().background(Color::rgb(0.1, 0.1, 0.1)));
            ui.panel(
                root,
                Style::new()
                    .size(80.0, 20.0)
                    .background(Color::rgb(1.0, 0.0, 0.0)),
            );
            ui.label(root, "hello", Style::new());
            ui.layout(Vec2::new(200.0, 100.0)).expect("it lays out");
            describe(&ui.draw_list())
        };
        assert_eq!(build(), build());
    }

    #[test]
    fn the_description_says_what_moved() {
        // And it has to notice one, or it is a snapshot of nothing.
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().column());
        let box_ = ui.panel(root, Style::new().size(80.0, 20.0).background(Color::WHITE));
        ui.layout(Vec2::new(200.0, 100.0)).expect("it lays out");
        let before = describe(&ui.draw_list());

        ui.set_width(box_, 120.0);
        ui.layout(Vec2::new(200.0, 100.0))
            .expect("it lays out again");
        let after = describe(&ui.draw_list());
        assert_ne!(before, after, "a resize shows up:\n{before}\n{after}");
        assert!(after.contains("120"), "{after}");
    }

    #[test]
    fn a_colour_change_shows_up_without_a_gpu() {
        // The case that could only be checked by eye: theming.
        let describe_with = |color| {
            let mut ui = Ui::new();
            let root = ui.root_panel(Style::new().column());
            ui.panel(root, Style::new().size(10.0, 10.0).background(color));
            ui.layout(Vec2::new(50.0, 50.0)).expect("it lays out");
            describe(&ui.draw_list())
        };
        assert_ne!(
            describe_with(Color::rgb(1.0, 0.0, 0.0)),
            describe_with(Color::rgb(0.0, 1.0, 0.0))
        );
    }
}
