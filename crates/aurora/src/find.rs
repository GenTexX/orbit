//! aurora find bar: search and replace over a text area.
//!
//! A small floating bar with a query field, a replacement field, and the
//! buttons to step through matches and rewrite them. Aurora owns it for the
//! same reason it owns the list popup: every app with a text area of any size
//! grows one, and the parts worth getting right - keeping the match list
//! correct as the text changes underneath, revealing a match without fighting
//! the field's own scrolling, replacing without invalidating the offsets of the
//! matches still to come - are the same every time.
//!
//! It reuses two mechanisms rather than inventing any:
//!
//! - matches are drawn as [`Decoration`]s, the same byte-range marks a squiggle
//!   uses, so they wrap and scroll like the text they mark;
//! - a replacement selects the match and calls [`Ui::insert_str`], the same
//!   splice a paste goes through, so masks, caret placement, and re-shaping all
//!   behave exactly as they do when a person types.

use std::ops::Range;

use glam::Vec2;

use crate::input::Key;
use crate::style::Style;
use crate::theme::Theme;
use crate::ui::Ui;
use crate::widget::{Decoration, DecorationStyle, WidgetId};

/// The widgets one [`build`](FindBar::build) produced.
#[derive(Debug, Clone, Default)]
pub struct FindRows {
    pub card: Option<WidgetId>,
    pub query: Option<WidgetId>,
    pub replacement: Option<WidgetId>,
    pub previous: Option<WidgetId>,
    pub next: Option<WidgetId>,
    pub replace: Option<WidgetId>,
    pub replace_all: Option<WidgetId>,
    pub match_case: Option<WidgetId>,
    pub whole_word: Option<WidgetId>,
    pub close: Option<WidgetId>,
}

/// What a key did to an open find bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindKey {
    /// The current match moved; reveal it.
    Stepped,
    /// The bar has no use for this key - the field underneath should get it.
    Ignored,
}

/// A find-and-replace bar over one text area.
#[derive(Debug, Clone, Default)]
pub struct FindBar {
    query: String,
    replacement: String,
    match_case: bool,
    whole_word: bool,
    /// Byte ranges of every match, in order.
    matches: Vec<Range<usize>>,
    /// Which match is current.
    current: usize,
    /// Whether the last step ran off an end and came back round. Cleared by
    /// anything that recomputes the matches, because a fresh search has not
    /// wrapped yet.
    wrapped: bool,
    anchor: Vec2,
}

impl FindBar {
    /// A bar anchored with its top-left at `anchor`.
    pub fn new(anchor: Vec2) -> Self {
        Self {
            anchor,
            ..Self::default()
        }
    }

    pub fn anchor(&self) -> Vec2 {
        self.anchor
    }

    pub fn set_anchor(&mut self, anchor: Vec2) {
        self.anchor = anchor;
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn replacement(&self) -> &str {
        &self.replacement
    }

    pub fn set_replacement(&mut self, replacement: impl Into<String>) {
        self.replacement = replacement.into();
    }

    pub fn match_case(&self) -> bool {
        self.match_case
    }

    pub fn whole_word(&self) -> bool {
        self.whole_word
    }

    pub fn set_whole_word(&mut self, whole_word: bool, text: &str) {
        self.whole_word = whole_word;
        self.refresh(text);
    }

    /// Search `text` for `query`, and stand on the first match at or after
    /// `from` - the caret.
    ///
    /// Searching from the top instead sends you to the start of the file for a
    /// word that occurs six lines below where you are looking, and every search
    /// begins with pressing Enter until you get back. `from` makes the first
    /// result the one nearest to hand.
    pub fn set_query(&mut self, query: impl Into<String>, text: &str, from: usize) {
        self.query = query.into();
        self.matches = find_all(text, &self.query, self.match_case, self.whole_word);
        self.current = self
            .matches
            .iter()
            .position(|m| m.start >= from)
            .unwrap_or(0);
        self.wrapped = false;
    }

    pub fn set_match_case(&mut self, match_case: bool, text: &str) {
        self.match_case = match_case;
        self.refresh(text);
    }

    /// Recompute the matches against `text` - after an edit, or after the query
    /// changed.
    ///
    /// You stay on the same *ordinal*: still "match 3 of 12", clamped if there
    /// are now fewer. Holding the byte offset instead would look right until the
    /// edit was above the match, at which point every offset shifts and the
    /// nearest one is a different match.
    pub fn refresh(&mut self, text: &str) {
        self.matches = find_all(text, &self.query, self.match_case, self.whole_word);
        self.current = self.current.min(self.matches.len().saturating_sub(1));
        self.wrapped = false;
    }

    /// How many matches there are.
    pub fn len(&self) -> usize {
        self.matches.len()
    }

    pub fn is_empty(&self) -> bool {
        self.matches.is_empty()
    }

    /// Which match is current, 1-based - the "3 of 12" readout.
    pub fn position(&self) -> Option<usize> {
        (!self.matches.is_empty()).then_some(self.current + 1)
    }

    /// The current match's byte range.
    pub fn current(&self) -> Option<Range<usize>> {
        self.matches.get(self.current).cloned()
    }

    /// Step to the next match, wrapping - and remember that it wrapped, so the
    /// readout can say so.
    pub fn find_next(&mut self) -> Option<Range<usize>> {
        if self.matches.is_empty() {
            return None;
        }
        let next = self.current + 1;
        self.wrapped = next >= self.matches.len();
        self.current = next % self.matches.len();
        self.current()
    }

    /// Step to the previous match, wrapping.
    pub fn find_previous(&mut self) -> Option<Range<usize>> {
        if self.matches.is_empty() {
            return None;
        }
        self.wrapped = self.current == 0;
        self.current = (self.current + self.matches.len() - 1) % self.matches.len();
        self.current()
    }

    /// Offer a key to the bar while one of its fields has focus.
    ///
    /// Enter steps to the next match and shift+Enter to the previous, without
    /// letting focus leave the bar. The query field is single-line, so aurora's
    /// own Enter would step focus into the replacement field and the app's
    /// submit handler would then select a match - putting the caret in the
    /// source file, where the next keystroke would be typed into the code.
    pub fn key(&mut self, key: Key, shift: bool) -> FindKey {
        match key {
            Key::Enter if shift => {
                self.find_previous();
                FindKey::Stepped
            }
            Key::Enter => {
                self.find_next();
                FindKey::Stepped
            }
            _ => FindKey::Ignored,
        }
    }

    /// The readout beside the query field: which match you are on, or that the
    /// query found none. An empty box is otherwise indistinguishable from a
    /// search that matched nothing.
    pub fn status(&self) -> String {
        match self.position() {
            // Coming back round to the top looks identical to not having moved
            // when there is one match, and to having lost your place when there
            // are many. Saying so costs a word.
            Some(at) if self.wrapped => format!("{at} of {} (wrapped)", self.matches.len()),
            Some(at) => format!("{at} of {}", self.matches.len()),
            None if self.query.is_empty() => String::new(),
            None => "no matches".to_string(),
        }
    }

    /// Every match as a decoration: all of them faintly, the current one solidly
    /// enough to pick out of the rest.
    pub fn decorations(&self, theme: &Theme) -> Vec<Decoration> {
        self.matches
            .iter()
            .enumerate()
            .map(|(index, range)| Decoration {
                range: range.clone(),
                color: if index == self.current {
                    theme.find_match
                } else {
                    theme.find_match.fade(0.35)
                },
                style: DecorationStyle::Highlight,
            })
            .collect()
    }

    /// Select the current match in `id`, which also scrolls it into view.
    pub fn reveal(&self, ui: &mut Ui, id: WidgetId) {
        if let Some(range) = self.current() {
            ui.select_range(id, range.start, range.end);
        }
    }

    /// Replace the current match and move to the next one. Returns whether
    /// anything was replaced.
    pub fn replace_current(&mut self, ui: &mut Ui, id: WidgetId) -> bool {
        let Some(range) = self.current() else {
            return false;
        };
        ui.select_range(id, range.start, range.end);
        ui.insert_str(&self.replacement);
        let text = ui.text_of(id).unwrap_or_default().to_string();
        self.refresh(&text);
        true
    }

    /// Replace every match, returning how many. Works back to front, so each
    /// replacement leaves the offsets of the ones still to come untouched.
    pub fn replace_all(&mut self, ui: &mut Ui, id: WidgetId) -> usize {
        let ranges = self.matches.clone();
        for range in ranges.iter().rev() {
            ui.select_range(id, range.start, range.end);
            ui.insert_str(&self.replacement);
        }
        let text = ui.text_of(id).unwrap_or_default().to_string();
        self.refresh(&text);
        ranges.len()
    }

    /// Emit the bar.
    pub fn build(&self, ui: &mut Ui, theme: &Theme) -> FindRows {
        let card = ui.popup(
            self.anchor,
            Style::new()
                .column()
                .gap(4.0)
                .padding(6.0)
                .background(theme.menu_bg)
                .corner_radius(theme.card_radius)
                .border(theme.border_width, theme.card_border),
        );

        let top = ui.panel(card, Style::new().row().gap(4.0).align_center());
        let query = ui.text_input(
            top,
            self.query.clone(),
            Style::new()
                .width(160.0)
                .padding(4.0)
                .corner_radius(theme.control_radius)
                .placeholder("Find"),
        );
        ui.label(
            top,
            self.status(),
            Style::new().width(70.0).foreground(theme.subhead),
        );
        let previous = ui.button(top, "<", small(theme));
        let next = ui.button(top, ">", small(theme));
        let match_case = ui.checkbox(top, self.match_case, "Aa", Style::new());
        let whole_word = ui.checkbox(top, self.whole_word, "W", Style::new());
        let close = ui.button(top, "x", small(theme));

        let bottom = ui.panel(card, Style::new().row().gap(4.0).align_center());
        let replacement = ui.text_input(
            bottom,
            self.replacement.clone(),
            Style::new()
                .width(160.0)
                .padding(4.0)
                .corner_radius(theme.control_radius)
                .placeholder("Replace"),
        );
        let replace = ui.button(bottom, "Replace", small(theme));
        let replace_all = ui.button(bottom, "All", small(theme));

        FindRows {
            card: Some(card),
            query: Some(query),
            replacement: Some(replacement),
            previous: Some(previous),
            next: Some(next),
            replace: Some(replace),
            replace_all: Some(replace_all),
            match_case: Some(match_case),
            whole_word: Some(whole_word),
            close: Some(close),
        }
    }
}

fn small(theme: &Theme) -> Style {
    Style::new()
        .padding_x(8.0)
        .padding_y(3.0)
        .corner_radius(theme.control_radius)
}

/// Every non-overlapping occurrence of `needle` in `haystack`, in order.
///
/// Case-insensitive matching folds ASCII only. That keeps every match the same
/// byte length as the query, so the offsets it reports are exact - lowercasing
/// the whole text first would be simpler and would silently shift them, because
/// case mapping is not length-preserving in general.
fn find_all(haystack: &str, needle: &str, match_case: bool, whole_word: bool) -> Vec<Range<usize>> {
    if needle.is_empty() {
        return Vec::new();
    }
    let mut found = Vec::new();
    let (bytes, needle_bytes) = (haystack.as_bytes(), needle.as_bytes());
    let mut at = 0;
    while at + needle_bytes.len() <= bytes.len() {
        if !haystack.is_char_boundary(at) {
            at += 1;
            continue;
        }
        let window = &bytes[at..at + needle_bytes.len()];
        let hit = if match_case {
            window == needle_bytes
        } else {
            window.eq_ignore_ascii_case(needle_bytes)
        };
        let end = at + needle_bytes.len();
        // Whole-word means neither neighbour continues the word: searching for
        // `pos` should not stop on every `position`.
        let bounded = !whole_word
            || (!haystack[..at].chars().next_back().is_some_and(is_word)
                && !haystack[end..].chars().next().is_some_and(is_word));
        if hit && bounded {
            found.push(at..end);
            at = end;
        } else {
            at += 1;
        }
    }
    found
}

/// What counts as part of a word for whole-word matching: the characters an
/// identifier is made of.
fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::InputEvent;

    const TEXT: &str = "let pos = 1.0;\nlet post = pos + pos;";

    fn bar(query: &str) -> FindBar {
        let mut bar = FindBar::new(Vec2::ZERO);
        bar.set_query(query, TEXT, 0);
        bar
    }

    /// A Ui with one text area holding `text`, laid out.
    fn area(text: &str) -> (Ui, WidgetId) {
        let mut ui = Ui::new();
        let root = ui.root_panel(Style::new().size(600.0, 400.0));
        let field = ui.text_input(
            root,
            text.to_string(),
            Style::new().size(500.0, 300.0).multiline().monospace(),
        );
        ui.layout(Vec2::new(600.0, 400.0)).unwrap();
        (ui, field)
    }

    #[test]
    fn every_occurrence_is_found_in_order() {
        let bar = bar("pos");
        assert_eq!(bar.len(), 4, "pos, post, and two more");
        assert_eq!(bar.current(), Some(4..7));
        assert_eq!(bar.position(), Some(1));
    }

    #[test]
    fn an_empty_query_matches_nothing_rather_than_everything() {
        let bar = bar("");
        assert!(bar.is_empty());
        assert_eq!(bar.position(), None);
    }

    #[test]
    fn matching_is_case_insensitive_until_it_is_not() {
        let mut bar = FindBar::new(Vec2::ZERO);
        bar.set_query("LET", TEXT, 0);
        assert_eq!(bar.len(), 2);
        bar.set_match_case(true, TEXT);
        assert_eq!(bar.len(), 0, "no `LET` in the text");
        bar.set_query("let", TEXT, 0);
        assert_eq!(bar.len(), 2);
    }

    #[test]
    fn matches_do_not_overlap() {
        let mut bar = FindBar::new(Vec2::ZERO);
        bar.set_query("aa", "aaaa", 0);
        assert_eq!(bar.len(), 2, "aaaa holds two `aa`, not three");
    }

    #[test]
    fn case_folding_never_shifts_the_offsets_it_reports() {
        // Lowercasing the whole text before searching would be simpler and would
        // move every offset after a character whose case mapping changes length.
        let text = "AAA \u{130} pos";
        let mut bar = FindBar::new(Vec2::ZERO);
        bar.set_query("POS", text, 0);
        let range = bar.current().expect("found");
        assert_eq!(&text[range], "pos", "the range still slices the real text");
    }

    #[test]
    fn enter_steps_matches_and_keeps_focus_in_the_bar() {
        // The defect: the query field is single-line, so Enter stepped focus
        // into the replacement field and the app's submit handler then selected
        // a match - putting the caret in the source, where the next keystroke
        // was typed into the code.
        let mut bar = bar("pos");
        assert_eq!(bar.key(Key::Enter, false), FindKey::Stepped);
        assert_eq!(bar.position(), Some(2));
        assert_eq!(bar.key(Key::Enter, true), FindKey::Stepped);
        assert_eq!(bar.position(), Some(1), "shift steps back");

        // Anything else is the field's, so typing still works.
        assert_eq!(bar.key(Key::Left, false), FindKey::Ignored);
        assert_eq!(bar.key(Key::Backspace, false), FindKey::Ignored);
    }

    #[test]
    fn stepping_wraps_at_both_ends() {
        let mut bar = bar("pos");
        assert_eq!(bar.position(), Some(1));
        bar.find_next();
        assert_eq!(bar.position(), Some(2));
        bar.find_previous();
        bar.find_previous();
        assert_eq!(bar.position(), Some(4), "wrapped back to the last");
        bar.find_next();
        assert_eq!(bar.position(), Some(1), "and round again");
    }

    #[test]
    fn stepping_an_empty_result_does_nothing() {
        let mut bar = bar("zzz");
        assert_eq!(bar.find_next(), None);
        assert_eq!(bar.find_previous(), None);
    }

    #[test]
    fn an_edit_leaves_you_on_the_same_match_by_ordinal() {
        let mut bar = bar("pos");
        bar.find_next();
        bar.find_next();
        assert_eq!(bar.position(), Some(3));
        let held = bar.current().unwrap();

        // A line inserted above shifts every offset; the ordinal does not move,
        // and the match it names is the same piece of text.
        let edited = format!("// a new line\n{TEXT}");
        bar.refresh(&edited);
        assert_eq!(bar.position(), Some(3), "still the third match");
        let now = bar.current().unwrap();
        assert_eq!(now.start, held.start + 14, "shifted by the inserted line");
    }

    #[test]
    fn losing_matches_clamps_rather_than_dangling() {
        let mut bar = bar("pos");
        bar.find_previous();
        assert_eq!(bar.position(), Some(4), "on the last one");
        bar.refresh("let pos = 1.0;");
        assert_eq!(bar.position(), Some(1), "only one left, and we are on it");
        bar.refresh("nothing here");
        assert_eq!(bar.position(), None);
        assert_eq!(bar.current(), None);
    }

    // --- decorations ---

    #[test]
    fn every_match_is_decorated_and_the_current_one_stands_out() {
        let bar = bar("pos");
        let decorations = bar.decorations(&Theme::dark());
        assert_eq!(decorations.len(), 4);
        assert!(
            decorations
                .iter()
                .all(|d| d.style == DecorationStyle::Highlight)
        );
        let current = &decorations[0];
        assert!(
            decorations[1..].iter().all(|d| d.color != current.color),
            "the current match must be picked out from the others"
        );
    }

    // --- replacing, through the same splice a paste uses ---

    #[test]
    fn replacing_the_current_match_rewrites_only_that_one() {
        let (mut ui, field) = area(TEXT);
        let mut bar = bar("pos");
        bar.set_replacement("here");
        assert!(bar.replace_current(&mut ui, field));
        assert_eq!(
            ui.text_of(field).unwrap(),
            "let here = 1.0;\nlet post = pos + pos;"
        );
        assert_eq!(bar.len(), 3, "and the match list caught up");
    }

    #[test]
    fn replace_all_rewrites_every_match_back_to_front() {
        // Front to back, each replacement of a different length would move every
        // later match's offsets and the next splice would land in the wrong
        // place. This is that test.
        let (mut ui, field) = area(TEXT);
        let mut bar = bar("pos");
        bar.set_replacement("x");
        assert_eq!(bar.replace_all(&mut ui, field), 4);
        assert_eq!(ui.text_of(field).unwrap(), "let x = 1.0;\nlet xt = x + x;");
        assert!(bar.is_empty(), "nothing left to find");
    }

    #[test]
    fn replacing_with_something_longer_also_lands_in_the_right_places() {
        let (mut ui, field) = area("a a a");
        let mut bar = FindBar::new(Vec2::ZERO);
        bar.set_query("a", "a a a", 0);
        bar.set_replacement("long");
        bar.replace_all(&mut ui, field);
        assert_eq!(ui.text_of(field).unwrap(), "long long long");
    }

    #[test]
    fn replacing_nothing_replaces_nothing() {
        let (mut ui, field) = area(TEXT);
        let mut bar = bar("zzz");
        bar.set_replacement("x");
        assert!(!bar.replace_current(&mut ui, field));
        assert_eq!(bar.replace_all(&mut ui, field), 0);
        assert_eq!(ui.text_of(field).unwrap(), TEXT);
    }

    #[test]
    fn revealing_a_match_selects_it_in_the_field() {
        let (mut ui, field) = area(TEXT);
        let bar = bar("post");
        bar.reveal(&mut ui, field);
        assert_eq!(ui.selected_text().as_deref(), Some("post"));
    }

    #[test]
    fn a_replacement_goes_through_the_same_path_a_paste_does() {
        // Which means it behaves the same way: the caret lands after the
        // inserted text, with no selection left over.
        let (mut ui, field) = area("aaa");
        let mut bar = FindBar::new(Vec2::ZERO);
        bar.set_query("aaa", "aaa", 0);
        bar.set_replacement("bb");
        bar.replace_current(&mut ui, field);
        assert_eq!(ui.caret_offset(), Some(2));
        assert_eq!(ui.selected_text(), None);
    }

    // --- what build emits ---

    #[test]
    fn build_emits_the_bar_where_it_was_anchored() {
        let mut ui = Ui::new();
        ui.root_panel(Style::new().size(600.0, 400.0));
        let mut bar = bar("pos");
        bar.set_anchor(Vec2::new(40.0, 30.0));
        let rows = bar.build(&mut ui, &Theme::dark());
        ui.layout(Vec2::new(600.0, 400.0)).unwrap();
        assert_eq!(
            ui.rect(rows.card.unwrap()).unwrap().pos,
            Vec2::new(40.0, 30.0)
        );
        for widget in [rows.query, rows.replacement, rows.next, rows.replace_all] {
            assert!(widget.is_some());
        }
    }

    #[test]
    fn the_bar_says_which_match_you_are_on_and_when_there_are_none() {
        let mut found = bar("pos");
        assert_eq!(found.status(), "1 of 4");
        found.find_next();
        assert_eq!(found.status(), "2 of 4");
        assert_eq!(bar("zzz").status(), "no matches");
        assert_eq!(
            bar("").status(),
            "",
            "an untyped query reports nothing rather than no matches"
        );
    }

    #[test]
    fn typing_in_the_query_field_is_ordinary_text_input() {
        // The bar's fields are plain text inputs, so an app wires them to the
        // bar the way it wires any other field - no special input path.
        let mut ui = Ui::new();
        ui.root_panel(Style::new().size(600.0, 400.0));
        let rows = FindBar::new(Vec2::ZERO).build(&mut ui, &Theme::dark());
        ui.layout(Vec2::new(600.0, 400.0)).unwrap();
        let query = rows.query.unwrap();
        ui.focus(query);
        for c in "pos".chars() {
            ui.handle_input(InputEvent::Text(c));
        }
        assert_eq!(ui.text_of(query), Some("pos"));
    }

    #[test]
    fn whole_word_skips_the_matches_inside_longer_words() {
        let text = "pos position reposition pos_x pos";
        let mut bar = FindBar::default();
        bar.set_query("pos", text, 0);
        assert_eq!(bar.len(), 5, "plainly, every substring");

        bar.set_whole_word(true, text);
        let ranges: Vec<Range<usize>> = (0..bar.len())
            .map(|i| {
                bar.current = i;
                bar.current().unwrap()
            })
            .collect();
        assert_eq!(ranges, vec![0..3, 30..33], "the two standing alone");
    }

    #[test]
    fn a_search_starts_from_the_caret_rather_than_the_top_of_the_file() {
        let text = "a\na\na\na";
        let mut bar = FindBar::default();
        bar.set_query("a", text, 4);
        assert_eq!(bar.current(), Some(4..5), "the first one at or after");
        assert_eq!(
            bar.position(),
            Some(3),
            "and the readout counts it as third"
        );

        // Past the last match, it comes back to the first rather than to none.
        bar.set_query("a", text, 100);
        assert_eq!(bar.current(), Some(0..1));
    }

    #[test]
    fn stepping_past_the_end_says_that_it_wrapped() {
        let text = "a a";
        let mut bar = FindBar::default();
        bar.set_query("a", text, 0);
        assert_eq!(bar.status(), "1 of 2");
        bar.find_next();
        assert_eq!(bar.status(), "2 of 2");
        bar.find_next();
        assert_eq!(bar.status(), "1 of 2 (wrapped)");

        // Backwards off the top says so too, and a fresh query forgets it.
        bar.find_previous();
        assert_eq!(bar.status(), "2 of 2 (wrapped)");
        bar.set_query("a", text, 0);
        assert_eq!(bar.status(), "1 of 2");
    }

    #[test]
    fn matches_do_not_borrow_the_selection_colour() {
        // The current match is also selected, so if the two agreed there would
        // be no way to tell it from the others.
        let text = "a a";
        let mut bar = FindBar::default();
        bar.set_query("a", text, 0);
        let theme = Theme::dark();
        let colors: Vec<_> = bar
            .decorations(&theme)
            .into_iter()
            .map(|d| d.color)
            .collect();
        assert_eq!(colors.len(), 2);
        assert_ne!(colors[0], theme.selection);
        assert_ne!(colors[0], colors[1], "current versus the rest");
    }
}
