//! aurora list popup: a floating list of choices, driven by the keyboard.
//!
//! An autocomplete list, a command palette, a combo box's options - all the same
//! thing: a set of strings, one of them highlighted, narrowed as you type, and
//! chosen with Enter. Aurora owns it because every app that grows a text field
//! eventually wants one, and because the fiddly parts - keeping the highlight on
//! the same item across a re-filter, keeping it visible without a scrollbar, not
//! stealing the keys the field still needs - are the same fiddly parts every
//! time.
//!
//! Like the color picker and the tab bar, this is composed from ordinary
//! widgets rather than being a widget kind of its own (ADR 0014): it holds the
//! state, [`build`](ListPopup::build) emits the widgets, and the app routes
//! input back in.
//!
//! It does not own the text being typed. In an autocomplete the caret stays in
//! the editor, so the app feeds the prefix in with [`set_filter`] and the keys
//! in with [`key`], and gets back what to do.
//!
//! [`set_filter`]: ListPopup::set_filter
//! [`key`]: ListPopup::key

use glam::Vec2;

use crate::input::Key;
use crate::style::Style;
use crate::theme::Theme;
use crate::ui::Ui;
use crate::widget::WidgetId;

/// The most rows drawn at once. More than this and the list windows around the
/// highlight instead of growing - a popup taller than the screen is not a list,
/// and windowing keeps the highlight visible without needing a scrollbar the
/// keyboard would then have to drive.
const MAX_VISIBLE: usize = 8;

/// The popup's width. Fixed rather than fitted, so the list does not jump about
/// as the filter narrows it and the longest detail changes.
const CARD_WIDTH: f32 = 380.0;

/// The widgets one [`build`](ListPopup::build) produced, for routing clicks.
#[derive(Debug, Clone, Default)]
pub struct ListRows {
    /// The popup card, for dismiss-on-press-outside tests.
    pub card: Option<WidgetId>,
    /// Each drawn row and the item index it stands for.
    pub rows: Vec<(WidgetId, usize)>,
}

/// What a key did to an open list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListKey {
    /// The highlight moved; redraw.
    Moved,
    /// An item was chosen, by its index into the full item list.
    Accepted(usize),
    /// The list has no use for this key - the app should handle it (in an
    /// autocomplete, that means the field still gets to type it).
    Ignored,
}

/// One row: what would be inserted, and a dimmer note about what it is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListItem {
    pub label: String,
    /// A short note shown after the label in a quieter color - a type, a
    /// signature, a sentence. Empty for none.
    pub detail: String,
}

impl ListItem {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            detail: String::new(),
        }
    }

    pub fn with_detail(label: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            detail: detail.into(),
        }
    }
}

/// A floating list of choices.
#[derive(Debug, Clone, Default)]
pub struct ListPopup {
    items: Vec<ListItem>,
    filter: String,
    /// Indices into `items` that match `filter`, best first.
    matches: Vec<usize>,
    /// Which of `matches` is highlighted.
    highlighted: usize,
    anchor: Vec2,
}

impl ListPopup {
    /// A list of `items`, anchored with its top-left at `anchor` (screen
    /// coordinates - see [`Ui::caret_rect`] for the caret-relative case).
    pub fn new(items: Vec<ListItem>, anchor: Vec2) -> Self {
        let mut list = Self {
            items,
            anchor,
            ..Self::default()
        };
        list.refilter();
        list
    }

    /// Replace the items, keeping the filter. The highlight stays on the same
    /// item if it is still there.
    pub fn set_items(&mut self, items: Vec<ListItem>) {
        let held = self.highlighted().map(|i| self.items[i].label.clone());
        self.items = items;
        self.refilter();
        self.restore(held.as_deref());
    }

    /// Narrow the list to items matching `filter`, case-insensitively.
    ///
    /// Items starting with the filter come first, then items merely containing
    /// it - typing `po` should offer `pos` before `compose`, but should not hide
    /// `compose` either.
    pub fn set_filter(&mut self, filter: &str) {
        if self.filter == filter {
            return;
        }
        let held = self.highlighted().map(|i| self.items[i].label.clone());
        self.filter = filter.to_string();
        self.refilter();
        self.restore(held.as_deref());
    }

    pub fn filter(&self) -> &str {
        &self.filter
    }

    pub fn anchor(&self) -> Vec2 {
        self.anchor
    }

    /// Move the popup - to follow the caret as it moves along a line.
    pub fn set_anchor(&mut self, anchor: Vec2) {
        self.anchor = anchor;
    }

    /// How many items currently match.
    pub fn len(&self) -> usize {
        self.matches.len()
    }

    /// Whether nothing matches. An autocomplete should close rather than show an
    /// empty box.
    pub fn is_empty(&self) -> bool {
        self.matches.is_empty()
    }

    /// The highlighted item's index into the full item list.
    pub fn highlighted(&self) -> Option<usize> {
        self.matches.get(self.highlighted).copied()
    }

    /// An item's text by its index - what [`ListKey::Accepted`] and
    /// [`press`](Self::press) name. Without this, being told which item was
    /// chosen would not be enough to insert it.
    pub fn item(&self, index: usize) -> Option<&str> {
        self.items.get(index).map(|i| i.label.as_str())
    }

    /// The highlighted item's text.
    pub fn highlighted_text(&self) -> Option<&str> {
        Some(self.items[self.highlighted()?].label.as_str())
    }

    /// Offer a key to the list. Anything it does not use comes back
    /// [`Ignored`](ListKey::Ignored), so the field underneath still gets it.
    ///
    /// Dismissal is not here: Escape is not one of aurora's editing keys, so an
    /// app handles it where it handles Escape for its modals, and simply stops
    /// calling [`build`](Self::build).
    pub fn key(&mut self, key: Key) -> ListKey {
        if self.matches.is_empty() {
            return ListKey::Ignored;
        }
        match key {
            // Wrapping: a list this short is faster to cycle than to stop at.
            Key::Down => {
                self.highlighted = (self.highlighted + 1) % self.matches.len();
                ListKey::Moved
            }
            Key::Up => {
                self.highlighted = (self.highlighted + self.matches.len() - 1) % self.matches.len();
                ListKey::Moved
            }
            Key::Enter | Key::Tab => match self.highlighted() {
                Some(index) => ListKey::Accepted(index),
                None => ListKey::Ignored,
            },
            _ => ListKey::Ignored,
        }
    }

    /// The item a press landed on, if it landed on a row.
    pub fn press(&self, ui: &Ui, rows: &ListRows, cursor: Vec2) -> Option<usize> {
        rows.rows
            .iter()
            .find(|(widget, _)| ui.rect(*widget).is_some_and(|r| r.contains(cursor)))
            .map(|(_, index)| *index)
    }

    /// Emit the popup: one row per visible match, the highlighted one marked.
    pub fn build(&self, ui: &mut Ui, theme: &Theme) -> ListRows {
        if self.matches.is_empty() {
            return ListRows::default();
        }
        // Wide enough that a signature fits without the list jumping in width
        // as you type, and capped so it never covers the code it is over.
        let card = ui.popup(
            self.anchor,
            Style::new()
                .column()
                .padding(4.0)
                .gap(1.0)
                .width(CARD_WIDTH)
                .background(theme.menu_bg)
                .corner_radius(theme.card_radius)
                .border(theme.border_width, theme.card_border)
                .clip(),
        );
        let mut rows = Vec::new();
        for slot in self.window() {
            let index = self.matches[slot];
            let selected = slot == self.highlighted;
            let item = &self.items[index];
            // A row is a panel, not a button, so the label and its detail can be
            // coloured differently - jamming them into one string made the note
            // about a name as loud as the name.
            let mut style = Style::new()
                .row()
                .gap(10.0)
                .align_center()
                .padding_x(8.0)
                .padding_y(3.0)
                .flat()
                .corner_radius(theme.control_radius);
            if selected {
                style = style.background(theme.row_selected);
            }
            // A button rather than a panel: a panel emits no Clicked, so
            // clicking a row did nothing at all - and took focus off the editor
            // on the way, which is what left the popup stranded. An empty
            // caption makes it a container whose children draw the text, the
            // same shape the file explorer's rows use.
            let row = ui.button(card, "", style);
            ui.label(row, &item.label, Style::new().foreground(theme.heading));
            if !item.detail.is_empty() {
                // A real column rather than a spacer plus a label. Given the
                // rest of the width by a grow(1.0) spacer, an ellipsizing label
                // gets none of it and truncates to just the dots.
                ui.label(
                    row,
                    &item.detail,
                    Style::new().grow(1.0).ellipsis().foreground(theme.subhead),
                );
            }
            rows.push((row, index));
        }
        ListRows {
            card: Some(card),
            rows,
        }
    }

    /// Which slice of `matches` to draw: the highlight stays inside it, so
    /// arrowing past the bottom scrolls the window rather than losing the
    /// selection off the edge.
    fn window(&self) -> std::ops::Range<usize> {
        let len = self.matches.len();
        if len <= MAX_VISIBLE {
            return 0..len;
        }
        let start = self
            .highlighted
            .saturating_sub(MAX_VISIBLE - 1)
            .min(len - MAX_VISIBLE);
        let start = start.min(self.highlighted);
        start..start + MAX_VISIBLE
    }

    fn refilter(&mut self) {
        let filter = self.filter.to_lowercase();
        let mut prefix = Vec::new();
        let mut contains = Vec::new();
        for (index, item) in self.items.iter().enumerate() {
            if filter.is_empty() {
                prefix.push(index);
                continue;
            }
            let lower = item.label.to_lowercase();
            if lower.starts_with(&filter) {
                prefix.push(index);
            } else if lower.contains(&filter) {
                contains.push(index);
            }
        }
        prefix.append(&mut contains);
        self.matches = prefix;
        self.highlighted = 0;
    }

    /// Put the highlight back on `held` if it survived the re-filter. Typing one
    /// more character should not silently move the selection to something else.
    fn restore(&mut self, held: Option<&str>) {
        let Some(held) = held else {
            return;
        };
        if let Some(slot) = self
            .matches
            .iter()
            .position(|&index| self.items[index].label == held)
        {
            self.highlighted = slot;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Color, draw::DrawCommand};

    fn items() -> Vec<ListItem> {
        ["pos", "print", "compose", "clamp"]
            .iter()
            .map(|s| ListItem::new(*s))
            .collect()
    }

    fn list() -> ListPopup {
        ListPopup::new(items(), Vec2::new(100.0, 200.0))
    }

    fn shown(list: &ListPopup) -> Vec<&str> {
        list.matches
            .iter()
            .map(|&i| list.items[i].label.as_str())
            .collect()
    }

    #[test]
    fn an_accepted_index_can_be_read_back_as_text() {
        let mut list = list();
        list.key(Key::Down);
        let ListKey::Accepted(index) = list.key(Key::Enter) else {
            panic!("enter accepts");
        };
        assert_eq!(list.item(index), Some("print"));
        assert_eq!(list.item(999), None);
    }

    #[test]
    fn an_unfiltered_list_offers_everything_in_order() {
        let list = list();
        assert_eq!(shown(&list), ["pos", "print", "compose", "clamp"]);
        assert_eq!(list.highlighted_text(), Some("pos"));
    }

    #[test]
    fn a_filter_puts_prefix_matches_first_without_hiding_the_rest() {
        // Typing "po" should offer `pos` before `compose` - but `compose` is
        // still a reasonable thing to have meant, so it stays.
        let mut list = list();
        list.set_filter("po");
        assert_eq!(shown(&list), ["pos", "compose"]);
    }

    #[test]
    fn filtering_is_case_insensitive() {
        let mut list = list();
        list.set_filter("PR");
        assert_eq!(shown(&list), ["print"]);
    }

    #[test]
    fn a_filter_that_matches_nothing_leaves_an_empty_list() {
        let mut list = list();
        list.set_filter("zzz");
        assert!(list.is_empty());
        assert_eq!(list.highlighted(), None);
    }

    #[test]
    fn typing_one_more_character_keeps_the_highlight_on_the_same_item() {
        // The annoyance this prevents: arrowing down to `compose`, typing the
        // next letter, and finding the selection has jumped back to the top.
        let mut list = list();
        list.set_filter("o");
        assert_eq!(list.key(Key::Down), ListKey::Moved);
        let held = list.highlighted_text().unwrap().to_string();
        list.set_filter("os");
        assert_eq!(list.highlighted_text(), Some(held.as_str()));
    }

    #[test]
    fn the_highlight_resets_when_what_it_was_on_no_longer_matches() {
        let mut list = list();
        list.set_filter("com");
        assert_eq!(list.highlighted_text(), Some("compose"));
        list.set_filter("cl");
        assert_eq!(list.highlighted_text(), Some("clamp"));
    }

    #[test]
    fn arrows_move_the_highlight_and_wrap_at_both_ends() {
        let mut list = list();
        assert_eq!(list.highlighted_text(), Some("pos"));
        list.key(Key::Down);
        assert_eq!(list.highlighted_text(), Some("print"));
        list.key(Key::Up);
        assert_eq!(list.highlighted_text(), Some("pos"));
        list.key(Key::Up);
        assert_eq!(list.highlighted_text(), Some("clamp"), "wrapped to the end");
        list.key(Key::Down);
        assert_eq!(list.highlighted_text(), Some("pos"), "and back round");
    }

    #[test]
    fn enter_and_tab_accept_the_highlight() {
        let mut list = list();
        list.key(Key::Down);
        assert_eq!(list.key(Key::Enter), ListKey::Accepted(1));
        assert_eq!(list.key(Key::Tab), ListKey::Accepted(1), "tab accepts too");
    }

    #[test]
    fn a_key_the_list_has_no_use_for_comes_back_ignored() {
        // Which is what lets the field underneath keep typing while the list is
        // open - the list only takes the keys it actually drives.
        let mut list = list();
        assert_eq!(list.key(Key::Left), ListKey::Ignored);
        assert_eq!(list.key(Key::Backspace), ListKey::Ignored);
        assert_eq!(list.key(Key::Home), ListKey::Ignored);
    }

    #[test]
    fn an_empty_list_accepts_nothing_and_swallows_nothing() {
        // With nothing to choose, Enter must reach the field rather than being
        // eaten by a popup that has no answer.
        let mut list = list();
        list.set_filter("zzz");
        assert_eq!(list.key(Key::Enter), ListKey::Ignored);
        assert_eq!(list.key(Key::Down), ListKey::Ignored);
    }

    // --- windowing ---

    fn long_list(n: usize) -> ListPopup {
        let items = (0..n)
            .map(|i| ListItem::new(format!("item{i:02}")))
            .collect();
        ListPopup::new(items, Vec2::ZERO)
    }

    #[test]
    fn a_short_list_draws_every_item() {
        let list = long_list(3);
        assert_eq!(list.window(), 0..3);
    }

    #[test]
    fn a_long_list_windows_and_the_window_follows_the_highlight() {
        let mut list = long_list(20);
        assert_eq!(list.window(), 0..MAX_VISIBLE, "starts at the top");

        // Arrowing down inside the window does not move it.
        for _ in 0..MAX_VISIBLE - 1 {
            list.key(Key::Down);
        }
        assert_eq!(list.window(), 0..MAX_VISIBLE);

        // Arrowing past the bottom scrolls it, keeping the highlight in view.
        list.key(Key::Down);
        let window = list.window();
        assert_eq!(window, 1..MAX_VISIBLE + 1);
        assert!(window.contains(&list.highlighted));

        // Wrapping to the end shows the last window, not the first.
        list.key(Key::Up);
        list.key(Key::Up);
        while list.highlighted != 0 {
            list.key(Key::Up);
        }
        list.key(Key::Up);
        let window = list.window();
        assert_eq!(window, 20 - MAX_VISIBLE..20, "wrapped to the last page");
        assert!(window.contains(&list.highlighted));
    }

    // --- what build actually emits ---

    /// Build `list` into a fresh Ui and lay it out.
    fn built(list: &ListPopup) -> (Ui, ListRows) {
        let mut ui = Ui::new();
        ui.root_panel(Style::new().size(600.0, 400.0));
        let rows = list.build(&mut ui, &Theme::dark());
        ui.layout(Vec2::new(600.0, 400.0)).unwrap();
        (ui, rows)
    }

    #[test]
    fn a_detail_is_drawn_beside_its_label_and_more_quietly() {
        // Jamming the two into one string made the note about a name as loud as
        // the name, and made the name harder to pick out of the list.
        let mut list = ListPopup::new(vec![ListItem::with_detail("pos", "Vec2")], Vec2::ZERO);
        list.set_filter("");
        let (ui, _) = built(&list);
        let theme = Theme::dark();
        let colors: Vec<Color> = ui
            .draw_list()
            .commands
            .iter()
            .filter_map(|c| match c {
                DrawCommand::Text { color, .. } => Some(*color),
                _ => None,
            })
            .collect();
        assert!(
            colors.contains(&theme.heading),
            "the name, in full strength"
        );
        assert!(colors.contains(&theme.subhead), "the detail, dimmed");
    }

    #[test]
    fn build_emits_one_row_per_match_anchored_where_it_was_told() {
        let mut list = list();
        list.set_filter("po");
        let (ui, rows) = built(&list);
        assert_eq!(rows.rows.len(), 2);
        let card = ui.rect(rows.card.unwrap()).unwrap();
        assert_eq!(card.pos, Vec2::new(100.0, 200.0), "anchored at the caret");
    }

    #[test]
    fn a_long_list_draws_only_its_window() {
        let list = long_list(50);
        let (_, rows) = built(&list);
        assert_eq!(rows.rows.len(), MAX_VISIBLE);
        // And the rows report their real indices, not their positions in the
        // window - accepting one has to insert the right completion.
        assert_eq!(rows.rows[0].1, 0);
        assert_eq!(rows.rows[MAX_VISIBLE - 1].1, MAX_VISIBLE - 1);
    }

    #[test]
    fn a_scrolled_window_reports_the_indices_it_is_actually_showing() {
        let mut list = long_list(50);
        for _ in 0..MAX_VISIBLE {
            list.key(Key::Down);
        }
        let (_, rows) = built(&list);
        assert_eq!(rows.rows[0].1, 1, "the window slid down by one");
        assert_eq!(rows.rows.last().unwrap().1, MAX_VISIBLE);
    }

    #[test]
    fn an_empty_list_emits_nothing_at_all() {
        let mut list = list();
        list.set_filter("zzz");
        let (_, rows) = built(&list);
        assert!(rows.card.is_none(), "no empty box floating over the text");
        assert!(rows.rows.is_empty());
    }

    #[test]
    fn a_row_is_interactive_so_clicking_one_reports_it() {
        // The bug this pins: a redesign made rows plain panels, which emit no
        // Clicked at all - so clicking a completion did nothing, and the press
        // took focus off the editor on the way.
        let mut list = list();
        list.set_filter("");
        let (mut ui, rows) = built(&list);
        let (row, index) = rows.rows[1];
        let at = {
            let r = ui.rect(row).unwrap();
            r.pos + r.size * 0.5
        };
        ui.handle_input(crate::input::InputEvent::PointerMoved(at));
        ui.handle_input(crate::input::InputEvent::PointerPressed);
        ui.handle_input(crate::input::InputEvent::PointerReleased);
        assert!(
            ui.drain_events().contains(&crate::Event::Clicked(row)),
            "a row must be clickable"
        );
        assert_eq!(list.item(index), Some("print"));
    }

    #[test]
    fn a_press_on_a_row_names_the_item_it_landed_on() {
        let mut list = list();
        list.set_filter("");
        let (ui, rows) = built(&list);
        let second = ui.rect(rows.rows[1].0).unwrap();
        assert_eq!(
            list.press(&ui, &rows, second.pos + second.size * 0.5),
            Some(1)
        );
        // Well outside the card entirely.
        assert_eq!(list.press(&ui, &rows, Vec2::new(590.0, 390.0)), None);
    }

    #[test]
    fn setting_items_keeps_the_highlight_on_the_same_one() {
        let mut list = list();
        list.key(Key::Down);
        assert_eq!(list.highlighted_text(), Some("print"));
        // A re-computed completion set that still contains `print`.
        list.set_items(
            ["clamp", "print", "pos"]
                .iter()
                .map(|s| ListItem::new(*s))
                .collect(),
        );
        assert_eq!(list.highlighted_text(), Some("print"));
    }
}
