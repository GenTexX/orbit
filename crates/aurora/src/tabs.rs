//! aurora tab bar: a strip of tabs you can click to select and drag along the
//! bar to reorder, with the displaced tabs sliding into their new places.
//!
//! The bar does not own the tabs. You keep the list (and its order); the bar
//! reports a [`Reorder`] when a drag says two tabs should swap, you apply it to
//! your own list and rebuild, and the bar animates the difference. That keeps a
//! single source of truth and lets the tabs be anything - dock panes, open
//! files, scenes.
//!
//! The animation is a draw-time offset ([`Style::translate`](crate::Style)), so
//! nothing about layout is fighting it: layout puts every tab where it finally
//! belongs the moment the order changes, and the bar just slides each one from
//! where it used to be. Drive it by calling [`TabBar::tick`] once a frame with
//! the elapsed time; while it returns `true` there is still motion to draw, so
//! an app that only redraws on demand should schedule another frame.

use glam::Vec2;

use crate::style::Style;
use crate::theme::Theme;
use crate::ui::Ui;
use crate::widget::WidgetId;

/// How fast a displaced tab eases into its new position. Higher is snappier;
/// this is the rate of an exponential decay, so it is frame-rate independent.
const EASE_RATE: f32 = 16.0;

/// Below this many pixels an animation is finished and snaps to zero.
const SETTLED_PX: f32 = 0.4;

/// A request to move the tab at `from` to index `to`, as a drag crossed a
/// neighbour. Apply it to your own list, then rebuild.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reorder {
    pub from: usize,
    pub to: usize,
}

/// The widgets a [`TabBar::build`] produced, for routing input back to it.
#[derive(Debug, Clone, Default)]
pub struct TabBarRows {
    /// The strip itself.
    pub bar: Option<WidgetId>,
    /// One button per tab, in the order they were passed in.
    pub tabs: Vec<WidgetId>,
}

/// A live reorder drag.
#[derive(Debug, Clone, Copy)]
struct Drag {
    /// The index of the tab being dragged, in the *current* order.
    index: usize,
    /// Where in the tab the pointer grabbed it, so it does not jump on press.
    grab_dx: f32,
}

/// A tab strip with drag-to-reorder and slide-into-place animation.
///
/// Keep one of these beside the list it presents; it holds only interaction and
/// animation state, never the tabs themselves.
#[derive(Debug, Clone, Default)]
pub struct TabBar {
    drag: Option<Drag>,
    /// Each tab's remaining draw-time offset, in px, indexed as the tabs are.
    offsets: Vec<f32>,
    /// Each tab's x from the last frame, so a reorder can be animated from
    /// where the tabs actually were.
    last_x: Vec<f32>,
    /// Set when a reorder was emitted, so the next tick animates the change
    /// rather than treating the new layout as a jump to ignore.
    reordered: bool,
}

impl TabBar {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether a reorder drag is in progress.
    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Build the strip under `parent`: one tab per label, the `active` one
    /// filled with the panel color so it merges into the content below.
    ///
    /// Tabs sit flush on a darker strip, inset from the top and sides so they do
    /// not jam the panel's corner. The strip carries a bottom border that the
    /// active tab punches through, because its fill is drawn over that segment -
    /// which is what makes the active tab read as connected to its content.
    pub fn build(
        &self,
        ui: &mut Ui,
        parent: WidgetId,
        labels: &[String],
        active: usize,
        theme: &Theme,
    ) -> TabBarRows {
        let bar = ui.panel(
            parent,
            Style::new()
                .row()
                .padding_top(5.0)
                .padding_x(6.0)
                .background(theme.bar_bg)
                .border_bottom(theme.border_width, theme.panel_border),
        );
        let mut tabs = Vec::with_capacity(labels.len());
        for (i, label) in labels.iter().enumerate() {
            let selected = i == active;
            let base = Style::new()
                .padding_x(12.0)
                .padding_y(6.0)
                .round_top(theme.tab_radius)
                .foreground(if selected {
                    theme.heading
                } else {
                    theme.subhead
                })
                .translate(Vec2::new(self.offset(i), 0.0));
            // The active tab takes the content's background and an outline open
            // at the bottom, so it merges into the pane; an inactive tab is flat.
            let style = if selected {
                base.background(theme.panel_bg)
                    .border(theme.border_width, theme.tab_border)
                    .border_sides([true, true, false, true])
            } else {
                base.flat()
            };
            tabs.push(ui.button(bar, label.clone(), style));
        }
        TabBarRows {
            bar: Some(bar),
            tabs,
        }
    }

    /// This tab's current animated offset, or 0 once it has settled.
    fn offset(&self, i: usize) -> f32 {
        self.offsets.get(i).copied().unwrap_or(0.0)
    }

    /// Start a reorder drag if `cursor` is on a tab. Returns the tab's index,
    /// which is also the one a plain click would select.
    pub fn press(&mut self, ui: &Ui, rows: &TabBarRows, cursor: Vec2) -> Option<usize> {
        let (index, rect) = rows.tabs.iter().enumerate().find_map(|(i, &id)| {
            let r = ui.rect(id)?;
            r.contains(cursor).then_some((i, r))
        })?;
        self.drag = Some(Drag {
            index,
            grab_dx: cursor.x - rect.pos.x,
        });
        Some(index)
    }

    /// Advance a live drag to `cursor`. Returns a [`Reorder`] when the dragged
    /// tab has moved far enough to change places with a neighbour - apply it to
    /// your list and rebuild; the animation is handled from there.
    ///
    /// The swap triggers when the dragged tab's leading edge passes the middle
    /// of the neighbour it is heading toward, which is what makes the exchange
    /// feel like it happens under the pointer rather than after it.
    pub fn drag_to(&mut self, ui: &Ui, rows: &TabBarRows, cursor: Vec2) -> Option<Reorder> {
        let drag = self.drag?;
        let held = *rows.tabs.get(drag.index)?;
        let held_rect = ui.rect(held)?;
        // Where the tab is being drawn right now (its laid-out slot plus how far
        // the pointer has pulled it).
        let visual_x = cursor.x - drag.grab_dx;
        let center = visual_x + held_rect.size.x * 0.5;

        let neighbour = if visual_x < held_rect.pos.x {
            drag.index.checked_sub(1)
        } else {
            Some(drag.index + 1).filter(|&i| i < rows.tabs.len())
        }?;
        let n_rect = ui.rect(rows.tabs[neighbour])?;
        let crossed = if neighbour < drag.index {
            center < n_rect.pos.x + n_rect.size.x * 0.5
        } else {
            center > n_rect.pos.x + n_rect.size.x * 0.5
        };
        if !crossed {
            return None;
        }
        // The dragged tab keeps following the pointer from its new slot.
        self.drag = Some(Drag {
            index: neighbour,
            ..drag
        });
        self.reordered = true;
        Some(Reorder {
            from: drag.index,
            to: neighbour,
        })
    }

    /// End a reorder drag; the tab eases from under the pointer into its slot.
    pub fn release(&mut self) {
        self.drag = None;
    }

    /// Advance the animation by `dt` seconds and write each tab's offset into
    /// the tree. Returns whether anything is still moving, so an app that
    /// redraws on demand knows to schedule another frame.
    ///
    /// Call this once per frame after layout, with the rows from the most recent
    /// build.
    pub fn tick(&mut self, dt: f32, ui: &mut Ui, rows: &TabBarRows, cursor: Vec2) -> bool {
        let xs: Vec<f32> = rows
            .tabs
            .iter()
            .map(|&id| ui.rect(id).map(|r| r.pos.x).unwrap_or(0.0))
            .collect();
        self.offsets.resize(xs.len(), 0.0);

        // A reorder just moved the tabs in layout. Seed each one's offset with
        // where it used to be, so it starts from there and slides.
        if self.reordered && self.last_x.len() == xs.len() {
            for (i, x) in xs.iter().enumerate() {
                self.offsets[i] += self.last_x[i] - x;
            }
        }
        self.reordered = false;
        self.last_x = xs.clone();

        // Ease every tab toward its settled position.
        let decay = (-EASE_RATE * dt.max(0.0)).exp();
        let mut moving = false;
        for offset in &mut self.offsets {
            *offset *= decay;
            if offset.abs() < SETTLED_PX {
                *offset = 0.0;
            } else {
                moving = true;
            }
        }

        // The held tab does not ease - it stays exactly under the pointer, or
        // the drag would feel like it is lagging behind the cursor.
        if let Some(drag) = self.drag
            && let Some(&x) = xs.get(drag.index)
        {
            self.offsets[drag.index] = cursor.x - drag.grab_dx - x;
            moving = true;
        }

        for (i, &id) in rows.tabs.iter().enumerate() {
            ui.set_translate(id, Vec2::new(self.offsets[i], 0.0));
        }
        moving
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::Style;

    /// Build a bar of three tabs and lay it out, returning everything needed to
    /// drive it.
    fn bar_of_three() -> (Ui, TabBar, TabBarRows, Vec<String>) {
        let labels: Vec<String> = ["Alpha", "Beta", "Gamma"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().fill().column());
        let bar = TabBar::new();
        let rows = bar.build(&mut ui, root, &labels, 0, &Theme::dark());
        ui.layout(Vec2::new(800.0, 600.0)).unwrap();
        (ui, bar, rows, labels)
    }

    #[test]
    fn dragging_a_tab_past_its_neighbour_asks_for_a_swap() {
        let (mut ui, mut bar, rows, _) = bar_of_three();
        let first = ui.rect(rows.tabs[0]).unwrap();
        let second = ui.rect(rows.tabs[1]).unwrap();

        // Grab the first tab in its middle.
        let grab = first.pos + first.size * 0.5;
        assert_eq!(bar.press(&ui, &rows, grab), Some(0));
        // Nudging it a little is not yet a swap.
        assert_eq!(bar.drag_to(&ui, &rows, grab + Vec2::new(4.0, 0.0)), None);
        // Dragging past the middle of the second tab is.
        let past = Vec2::new(second.pos.x + second.size.x, grab.y);
        assert_eq!(
            bar.drag_to(&ui, &rows, past),
            Some(Reorder { from: 0, to: 1 })
        );
        // The drag now follows the tab into its new slot, so continuing to drag
        // does not immediately swap back.
        assert!(bar.is_dragging());
        bar.release();
        assert!(!bar.is_dragging());
        let _ = &mut ui;
    }

    #[test]
    fn a_press_off_the_tabs_starts_nothing() {
        let (ui, mut bar, rows, _) = bar_of_three();
        assert_eq!(bar.press(&ui, &rows, Vec2::new(700.0, 400.0)), None);
        assert!(!bar.is_dragging());
    }

    #[test]
    fn a_reordered_tab_slides_from_where_it_was_and_settles() {
        let labels: Vec<String> = ["Alpha", "Beta"].iter().map(|s| s.to_string()).collect();
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().fill().column());
        let mut bar = TabBar::new();
        let rows = bar.build(&mut ui, root, &labels, 0, &Theme::dark());
        ui.layout(Vec2::new(800.0, 600.0)).unwrap();
        // Settle, so the bar knows where the tabs are.
        bar.tick(0.016, &mut ui, &rows, Vec2::ZERO);

        // Swap them, as applying a Reorder would, and rebuild.
        let swapped: Vec<String> = vec![labels[1].clone(), labels[0].clone()];
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().fill().column());
        let rows = bar.build(&mut ui, root, &swapped, 0, &Theme::dark());
        ui.layout(Vec2::new(800.0, 600.0)).unwrap();
        bar.reordered = true;

        // The first tick after the swap leaves them visually where they were, so
        // the change is a slide rather than a jump.
        assert!(
            bar.tick(0.016, &mut ui, &rows, Vec2::ZERO),
            "still animating right after a reorder"
        );
        assert!(
            bar.offsets.iter().any(|o| o.abs() > SETTLED_PX),
            "a displaced tab starts offset from its new slot"
        );

        // Given enough time it settles exactly, with no offset left over.
        for _ in 0..200 {
            if !bar.tick(0.016, &mut ui, &rows, Vec2::ZERO) {
                break;
            }
        }
        assert!(!bar.tick(0.016, &mut ui, &rows, Vec2::ZERO), "settled");
        assert!(bar.offsets.iter().all(|&o| o == 0.0));
    }
}
