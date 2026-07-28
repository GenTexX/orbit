//! aurora tab bar: a strip of tabs you can click to select and drag along the
//! bar to reorder, with the displaced tabs sliding into their new places.
//!
//! The bar does not own the tabs. You keep the list (and its order); the bar
//! previews a rearrangement while you drag and hands back the [`Reorder`] to
//! apply when you let go. That keeps a single source of truth and lets the tabs
//! be anything - dock panes, open files, scenes.
//!
//! **A drag never rebuilds anything.** The rearrangement is purely a set of
//! draw-time offsets ([`Style::translate`](crate::Style)), which layout and
//! hit-testing ignore. That matters for more than tidiness: rebuilding a tree
//! mid-drag drops the retained pointer state the drag is riding on, so a tab
//! that had been nudged along its bar could no longer be pulled out and dropped
//! somewhere else. Previewing instead of committing keeps both gestures alive at
//! once - rearrange while the pointer is over the strip, tear the tab out when
//! it leaves.
//!
//! Drive the animation by calling [`TabBar::tick`] once a frame with the elapsed
//! time; while it returns `true` there is still motion to draw, so an app that
//! only redraws on demand should schedule another frame.

use glam::Vec2;

use crate::style::Style;
use crate::theme::Theme;
use crate::ui::Ui;
use crate::widget::WidgetId;

/// How fast a tab eases toward where it should be. Higher is snappier; this is
/// the rate of an exponential decay, so it is frame-rate independent.
const EASE_RATE: f32 = 16.0;

/// Closer than this to its destination, a tab has arrived.
const SETTLED_PX: f32 = 0.4;

/// What a live tab drag currently means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabDrag {
    /// The pointer is still over the strip: the tabs are being rearranged, and
    /// [`TabBar::release`] will report the move.
    Reordering,
    /// The pointer has left the strip - the tab is being pulled out. The bar
    /// stops rearranging and leaves the gesture to the app, which decides what
    /// a torn-out tab means (docking it elsewhere, floating it, nothing).
    TornOut,
}

/// A tab that moved from index `from` to index `to`. Apply it to your own list,
/// then rebuild.
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
    /// Which tab is held, as an index into the app's real order.
    index: usize,
    /// Where in the tab the pointer grabbed it, so it does not jump on press.
    grab_dx: f32,
}

/// A tab strip with drag-to-reorder and slide-into-place animation.
///
/// Keep one beside the list it presents; it holds only interaction and
/// animation state, never the tabs themselves.
#[derive(Debug, Clone, Default)]
pub struct TabBar {
    drag: Option<Drag>,
    /// Each tab's current draw-time offset, in px, indexed as the app's list is.
    offsets: Vec<f32>,
    /// The order the tabs are being *shown* in, as indices into the real order.
    /// Only a drag rearranges this; it is empty otherwise.
    preview: Vec<usize>,
    /// A gap held open for a tab being dragged in from somewhere else: which
    /// shown slot it would land in, and how wide it is.
    insertion: Option<(usize, f32)>,
    /// A tab that has just arrived from elsewhere, and the x it arrived from, so
    /// it slides into place instead of appearing there.
    arriving: Option<(usize, f32)>,
}

impl TabBar {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether a tab drag is in progress.
    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// The widget of the tab being dragged, if any - so an app can work out
    /// which of its own items the gesture is carrying.
    pub fn held_tab(&self, rows: &TabBarRows) -> Option<WidgetId> {
        rows.tabs.get(self.drag?.index).copied()
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
        self.preview = (0..rows.tabs.len()).collect();
        Some(index)
    }

    /// Advance a live drag to `cursor`, rearranging how the tabs are *shown*.
    ///
    /// Reports whether the gesture is still a reorder or has become a tear-out;
    /// `None` when no drag is in progress.
    ///
    /// Nothing is committed and nothing is rebuilt; call
    /// [`release`](Self::release) for the move to apply. A tab swaps with the
    /// neighbour whose middle the pointer has passed, so the exchange lands
    /// under the pointer rather than behind it.
    pub fn drag_to(&mut self, ui: &Ui, rows: &TabBarRows, cursor: Vec2) -> Option<TabDrag> {
        let drag = self.drag?;
        let n = rows.tabs.len();
        if self.preview.len() != n {
            self.preview = (0..n).collect();
        }
        // Off the strip this is no longer a reorder but a tab being pulled out,
        // so put the shown order back and leave the gesture to the app.
        let on_bar = rows
            .bar
            .and_then(|b| ui.rect(b))
            .is_some_and(|r| r.contains(cursor));
        if !on_bar {
            self.preview = (0..n).collect();
            return Some(TabDrag::TornOut);
        }
        // The held tab can vanish from this bar - an app that lifts a torn-out
        // tab out of the layout rebuilds without it - so its index may no longer
        // exist. That is a tear-out, not a reorder.
        let Some(held_width) = rows
            .tabs
            .get(drag.index)
            .and_then(|&id| ui.rect(id))
            .map(|r| r.size.x)
        else {
            return Some(TabDrag::TornOut);
        };
        let widths: Vec<f32> = rows
            .tabs
            .iter()
            .map(|&id| ui.rect(id).map(|r| r.size.x).unwrap_or(0.0))
            .collect();
        let Some(left) = self.strip_left(ui, rows) else {
            return Some(TabDrag::Reordering);
        };
        let held_center = cursor.x - drag.grab_dx + held_width * 0.5;

        // Walk the held tab past every neighbour whose middle it has crossed (a
        // fast drag can pass more than one between frames).
        while let Some(slot) = self.preview.iter().position(|&i| i == drag.index) {
            let toward_start = held_center < self.slot_center(&widths, left, slot);
            let next = if toward_start {
                slot.checked_sub(1)
            } else {
                Some(slot + 1).filter(|&s| s < n)
            };
            let Some(next) = next else { break };
            let neighbour = self.slot_center(&widths, left, next);
            let crossed = if toward_start {
                held_center < neighbour
            } else {
                held_center > neighbour
            };
            if !crossed {
                break;
            }
            self.preview.swap(slot, next);
        }
        Some(TabDrag::Reordering)
    }

    /// Which slot a tab dragged in from elsewhere would land in, given the
    /// pointer's x - the index to insert it at.
    pub fn drop_slot(&self, ui: &Ui, rows: &TabBarRows, cursor: Vec2) -> usize {
        rows.tabs
            .iter()
            .filter_map(|&id| ui.rect(id))
            .filter(|r| cursor.x > r.pos.x + r.size.x * 0.5)
            .count()
    }

    /// Start the tab at `slot` from `from_x` and let it slide to where layout
    /// put it - for a tab that has just arrived from another bar, so it settles
    /// from where it was dropped rather than appearing in place.
    ///
    /// Call after the list has been rebuilt with the new tab in it.
    pub fn slide_in(&mut self, slot: usize, from_x: f32) {
        self.arriving = Some((slot, from_x));
    }

    /// Hold a gap open at `slot` for an incoming tab `width` px wide, so the
    /// strip shows where it would land. `None` closes it again.
    pub fn set_insertion(&mut self, insertion: Option<(usize, f32)>) {
        self.insertion = insertion;
    }

    /// Where an incoming tab should be drawn: the x of the gap being held for
    /// it, if any. Pair with [`set_insertion`](Self::set_insertion).
    pub fn insertion_x(&self, ui: &Ui, rows: &TabBarRows) -> Option<f32> {
        let (slot, _) = self.insertion?;
        let widths: Vec<f32> = rows
            .tabs
            .iter()
            .map(|&id| ui.rect(id).map(|r| r.size.x).unwrap_or(0.0))
            .collect();
        let left = self.strip_left(ui, rows).unwrap_or_else(|| {
            rows.bar
                .and_then(|b| ui.rect(b))
                .map(|r| r.pos.x + 6.0)
                .unwrap_or(0.0)
        });
        // With no preview order the tabs are shown as laid out.
        let order: Vec<usize> = if self.preview.len() == widths.len() {
            self.preview.clone()
        } else {
            (0..widths.len()).collect()
        };
        Some(left + order.iter().take(slot).map(|&i| widths[i]).sum::<f32>())
    }

    /// Abandon the drag without reporting a move, and let the tabs settle back.
    ///
    /// Call this when the dragged tab leaves the bar's list entirely - an app
    /// that lifts a torn-out tab out of the layout must, because the bar
    /// identifies the held tab by index and those indices shift the moment the
    /// list changes. Without it the bar would carry on dragging whatever tab
    /// slid into that index.
    pub fn cancel(&mut self) {
        self.drag = None;
        self.preview.clear();
    }

    /// End the drag. Returns the move to apply to your list when the tab ended
    /// up somewhere new, or `None` when it did not move (or was pulled out of
    /// the bar, which is the app's to handle).
    ///
    /// Applying it and rebuilding lands every tab exactly where it is already
    /// being drawn, so the commit is invisible.
    pub fn release(&mut self) -> Option<Reorder> {
        let drag = self.drag.take()?;
        let slot = self.preview.iter().position(|&i| i == drag.index);
        self.preview.clear();
        let slot = slot?;
        if slot == drag.index {
            return None;
        }
        // The tabs already look reordered, so start the new layout with no
        // offsets rather than sliding them a second time.
        self.offsets.clear();
        Some(Reorder {
            from: drag.index,
            to: slot,
        })
    }

    /// The x the leftmost tab starts at; previewed slots lay out flush from here.
    fn strip_left(&self, ui: &Ui, rows: &TabBarRows) -> Option<f32> {
        rows.tabs
            .iter()
            .filter_map(|&id| ui.rect(id))
            .map(|r| r.pos.x)
            .fold(None, |acc: Option<f32>, x| {
                Some(acc.map_or(x, |a| a.min(x)))
            })
    }

    /// The x a previewed slot starts at, laying the shown order out flush.
    fn slot_start(&self, widths: &[f32], left: f32, slot: usize) -> f32 {
        left + self
            .preview
            .iter()
            .take(slot)
            .map(|&i| widths.get(i).copied().unwrap_or(0.0))
            .sum::<f32>()
    }

    /// The center x of a previewed slot.
    fn slot_center(&self, widths: &[f32], left: f32, slot: usize) -> f32 {
        let w = self
            .preview
            .get(slot)
            .and_then(|&i| widths.get(i))
            .copied()
            .unwrap_or(0.0);
        self.slot_start(widths, left, slot) + w * 0.5
    }

    /// Advance the animation by `dt` seconds and write each tab's offset into
    /// the tree. Returns whether anything is still moving, so an app that
    /// redraws on demand knows to schedule another frame.
    ///
    /// Call once per frame after layout, with the rows from the latest build.
    /// A tab being dragged along the bar is pinned to the pointer so it tracks
    /// the cursor exactly; the tabs it displaces slide out of its way.
    pub fn tick(&mut self, dt: f32, ui: &mut Ui, rows: &TabBarRows, cursor: Vec2) -> bool {
        let n = rows.tabs.len();
        self.offsets.resize(n, 0.0);
        let actual: Vec<f32> = rows
            .tabs
            .iter()
            .map(|&id| ui.rect(id).map(|r| r.pos.x).unwrap_or(0.0))
            .collect();
        // Where each tab wants to be: its previewed slot while a drag is
        // rearranging them, otherwise exactly where layout already put it.
        let mut wanted = vec![0.0f32; n];
        if self.preview.len() == n {
            let widths: Vec<f32> = rows
                .tabs
                .iter()
                .map(|&id| ui.rect(id).map(|r| r.size.x).unwrap_or(0.0))
                .collect();
            if let Some(left) = self.strip_left(ui, rows) {
                for slot in 0..n {
                    let i = self.preview[slot];
                    wanted[i] = self.slot_start(&widths, left, slot) - actual[i];
                }
            }
        }

        // A tab that just arrived starts from where it was dropped.
        if let Some((slot, from_x)) = self.arriving.take()
            && let Some(&x) = actual.get(slot)
        {
            self.offsets[slot] = from_x - x;
        }

        // A gap for an incoming tab pushes everything at or after it aside.
        if let Some((slot, width)) = self.insertion
            && self.preview.len() != n
        {
            for (i, want) in wanted.iter_mut().enumerate() {
                if i >= slot {
                    *want += width;
                }
            }
        }

        let decay = (-EASE_RATE * dt.max(0.0)).exp();
        let mut moving = false;
        for (offset, want) in self.offsets.iter_mut().zip(&wanted) {
            *offset = want + (*offset - want) * decay;
            if (*offset - want).abs() < SETTLED_PX {
                *offset = *want;
            } else {
                moving = true;
            }
        }
        // The held tab is pinned to the pointer rather than eased: it should
        // track the cursor exactly, while the tabs it displaces slide.
        if let Some(drag) = self.drag
            && self.preview.len() == n
            && let Some(&x) = actual.get(drag.index)
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

    /// Build a bar of three tabs and lay it out, returning everything needed to
    /// drive it.
    fn bar_of_three() -> (Ui, TabBar, TabBarRows) {
        let labels: Vec<String> = ["Alpha", "Beta", "Gamma"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().fill().column());
        let bar = TabBar::new();
        let rows = bar.build(&mut ui, root, &labels, 0, &Theme::dark());
        ui.layout(Vec2::new(800.0, 600.0)).unwrap();
        (ui, bar, rows)
    }

    #[test]
    fn dragging_a_tab_past_its_neighbour_reorders_it_on_release() {
        let (ui, mut bar, rows) = bar_of_three();
        let first = ui.rect(rows.tabs[0]).unwrap();
        let second = ui.rect(rows.tabs[1]).unwrap();

        let grab = first.pos + first.size * 0.5;
        assert_eq!(bar.press(&ui, &rows, grab), Some(0));
        // A small nudge is not yet a swap.
        bar.drag_to(&ui, &rows, grab + Vec2::new(4.0, 0.0));
        assert_eq!(bar.preview, vec![0, 1, 2]);
        // Dragging past the middle of the second tab shows them swapped...
        bar.drag_to(&ui, &rows, Vec2::new(second.pos.x + second.size.x, grab.y));
        assert_eq!(bar.preview, vec![1, 0, 2], "shown order rearranged");
        // ...and letting go asks the app to make it real.
        assert_eq!(bar.release(), Some(Reorder { from: 0, to: 1 }));
        assert!(!bar.is_dragging());
    }

    #[test]
    fn dragging_off_the_bar_reorders_nothing_so_it_can_become_a_tear_out() {
        // The regression this guards: a tab nudged along its bar could no longer
        // be pulled out and docked elsewhere, because committing the reorder
        // mid-drag rebuilt the tree and killed the pointer drag it rode on.
        let (ui, mut bar, rows) = bar_of_three();
        let first = ui.rect(rows.tabs[0]).unwrap();
        let bar_rect = ui.rect(rows.bar.unwrap()).unwrap();
        let grab = first.pos + first.size * 0.5;
        assert_eq!(bar.press(&ui, &rows, grab), Some(0));

        // Sideways past a neighbour first, then away from the strip entirely -
        // the order a user actually does it in.
        bar.drag_to(&ui, &rows, Vec2::new(bar_rect.max().x - 1.0, grab.y));
        assert_ne!(bar.preview, vec![0, 1, 2], "rearranged while over the bar");
        let torn_out = Vec2::new(bar_rect.max().x - 1.0, bar_rect.max().y + 200.0);
        bar.drag_to(&ui, &rows, torn_out);
        assert_eq!(bar.preview, vec![0, 1, 2], "shown order restored");
        assert_eq!(bar.release(), None, "no reorder to apply; the app docks it");
    }

    #[test]
    fn the_held_tab_vanishing_from_the_bar_is_a_tear_out_not_a_panic() {
        // The crash this guards: an app that lifts a torn-out tab out of the
        // layout rebuilds the bar without it, so the held index no longer
        // exists - indexing the remaining tabs with it panicked.
        let (ui, mut bar, rows) = bar_of_three();
        let last = ui.rect(rows.tabs[2]).unwrap();
        assert_eq!(bar.press(&ui, &rows, last.pos + last.size * 0.5), Some(2));

        // Rebuild with that tab gone, as lifting it out of the dock does.
        let fewer: Vec<String> = ["Alpha", "Beta"].iter().map(|s| s.to_string()).collect();
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().fill().column());
        let rows = bar.build(&mut ui, root, &fewer, 0, &Theme::dark());
        ui.layout(Vec2::new(800.0, 600.0)).unwrap();
        let bar_rect = ui.rect(rows.bar.unwrap()).unwrap();

        let over_bar = bar_rect.pos + bar_rect.size * 0.5;
        assert_eq!(bar.drag_to(&ui, &rows, over_bar), Some(TabDrag::TornOut));
        assert_eq!(bar.release(), None, "nothing to reorder; the app holds it");
    }

    #[test]
    fn lifting_the_held_tab_out_must_not_leave_the_bar_dragging_its_neighbour() {
        // The bug this guards: tearing out the first of two tabs left the drag
        // pointing at index 0, which was now the *other* tab - so the neighbour
        // followed the pointer, and dropping reordered the wrong tab.
        let labels: Vec<String> = ["Files", "Console"].iter().map(|s| s.to_string()).collect();
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().fill().column());
        let mut bar = TabBar::new();
        let rows = bar.build(&mut ui, root, &labels, 0, &Theme::dark());
        ui.layout(Vec2::new(800.0, 600.0)).unwrap();
        let files = ui.rect(rows.tabs[0]).unwrap();
        assert_eq!(bar.press(&ui, &rows, files.pos + files.size * 0.5), Some(0));

        // The app lifts Files out and rebuilds with Console alone, telling the
        // bar the gesture has left it.
        bar.cancel();
        let rest: Vec<String> = vec!["Console".to_string()];
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().fill().column());
        let rows = bar.build(&mut ui, root, &rest, 0, &Theme::dark());
        ui.layout(Vec2::new(800.0, 600.0)).unwrap();

        let bar_rect = ui.rect(rows.bar.unwrap()).unwrap();
        let over = bar_rect.pos + bar_rect.size * 0.5;
        assert_eq!(
            bar.drag_to(&ui, &rows, over),
            None,
            "no drag left in this bar"
        );
        bar.tick(0.016, &mut ui, &rows, over);
        assert_eq!(bar.offsets, vec![0.0], "Console stays put");
    }

    #[test]
    fn a_press_off_the_tabs_starts_nothing() {
        let (ui, mut bar, rows) = bar_of_three();
        assert_eq!(bar.press(&ui, &rows, Vec2::new(700.0, 400.0)), None);
        assert!(!bar.is_dragging());
    }

    #[test]
    fn the_held_tab_tracks_the_pointer_and_the_displaced_one_slides() {
        let (mut ui, mut bar, rows) = bar_of_three();
        let first = ui.rect(rows.tabs[0]).unwrap();
        let second = ui.rect(rows.tabs[1]).unwrap();
        let grab = first.pos + first.size * 0.5;
        bar.press(&ui, &rows, grab);
        let held_at = Vec2::new(second.pos.x + second.size.x, grab.y);
        bar.drag_to(&ui, &rows, held_at);

        // While held, the tab sits exactly under the pointer (not eased toward
        // it), so the bar always reports motion.
        assert!(bar.tick(0.016, &mut ui, &rows, held_at));
        let pinned = held_at.x - (grab.x - first.pos.x) - first.pos.x;
        assert!(
            (bar.offsets[0] - pinned).abs() < 0.01,
            "the held tab is pinned to the pointer"
        );

        // Letting go commits the swap, and the tabs then settle exactly.
        assert_eq!(bar.release(), Some(Reorder { from: 0, to: 1 }));
        for _ in 0..400 {
            if !bar.tick(0.016, &mut ui, &rows, held_at) {
                break;
            }
        }
        assert!(!bar.tick(0.016, &mut ui, &rows, held_at), "settled");
        assert!(bar.offsets.iter().all(|&o| o == 0.0));
    }
}
