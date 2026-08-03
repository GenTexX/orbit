//! atlas Code pane state: the open script, its text and history, and the popups over it.
//!
//! Grouped out of `State` for a reason narrower than tidiness. `State` is where
//! everything in the editor meets, and Milestone 5 adds a running/editing mode,
//! a scene snapshot, a script host and a play/stop toolbar to it. The Code pane
//! is the largest piece of it that has nothing to do with any of that: its
//! couplings outward are the dirty flag, the theme, the project directory and
//! the modal used to report a failed save, and nothing else in the editor reads
//! these fields.
//!
//! The names are unchanged from when they were `State`'s, so the move is one
//! the compiler checked in full rather than one that had to be reviewed by eye.

use crate::SymbolRename;
use crate::TextEncoding;
use glam::Vec2;

/// Everything the Code pane owns.
pub struct CodePane {
    /// The script the Code pane shows: its project-relative path, its current
    /// buffer, and whether that buffer differs from the file on disk. One script
    /// at a time is milestone 4's cut, so this is single-valued editor state
    /// rather than anything the dock knows about.
    pub open_script: Option<String>,
    pub script_text: String,
    pub script_modified: bool,
    /// The open script's diagnostics from the last service run, for the status
    /// bar. The squiggles themselves are already on the Ui.
    pub script_diagnostics: Vec<comet::Diagnostic>,
    /// The open script's syntax colors from the last service run.
    pub script_spans: Vec<aurora::TextSpan>,
    /// Everything the language service knows about the current buffer, computed
    /// once per edit rather than once per question.
    pub analysis: comet::service::Analysis,
    /// How the open script was encoded on disk, so a save writes it back the
    /// same way.
    pub script_encoding: TextEncoding,
    /// The open script's file was deleted or moved out from under the buffer.
    /// The text stays; the header says so, and Save is unavailable until it is
    /// opened somewhere real again.
    pub script_orphaned: bool,
    /// Focus the find bar's query field after the next rebuild, and select what
    /// is in it. Consumed there, like `refocus_filter`.
    pub focus_find: bool,
    /// The diagnostic message currently shown as a tooltip, so the shell only
    /// rebuilds when what is under the pointer changes.
    pub diagnostic_tooltip: Option<String>,
    /// Where the pointer was when it last moved, and when it stopped - a
    /// tooltip belongs to a resting pointer, not a passing one.
    pub tooltip_cursor: Vec2,
    pub tooltip_rested: std::time::Instant,
    /// The go-to-line box's contents while it is open.
    pub go_to_line: Option<String>,
    /// The go-to-symbol palette: what has been typed into it, and the list of
    /// matching declarations. Open only over the Code pane.
    pub symbol_palette: Option<(String, aurora::list::ListPopup)>,
    /// Where the caret and the view were, per script, so reopening a file this
    /// session puts you back where you left rather than at the top.
    pub script_positions: std::collections::HashMap<String, (usize, f32)>,
    /// The call the caret is inside, and where to draw the hint. Refreshed on
    /// every edit and caret move, like the completion list.
    pub signature: Option<(comet::service::SignatureHelp, Vec2)>,
    /// An in-progress symbol rename: where in the buffer, the name being typed,
    /// and why the last attempt was refused.
    pub symbol_rename: Option<SymbolRename>,
    /// Set when a script is opened: focus the editor after the next rebuild.
    /// A file that appears with no caret swallows the first keystroke.
    pub focus_editor: bool,
    /// Set by ctrl+space: offer the list wherever the caret is, rather than only
    /// where a name is being typed. Cleared as soon as it has been used.
    pub force_completions: bool,
    /// When the caret's blink phase last flipped, and what the caret was doing
    /// then - so it holds solid while you type instead of blinking under you.
    pub caret_phase: std::time::Instant,
    pub caret_visible: bool,
    pub caret_was: Option<usize>,
    /// Text undo for the Code pane: snapshots of the buffer and where the caret
    /// was, newest last.
    ///
    /// Here rather than in aurora because a rebuild swaps the whole Ui, and an
    /// undo history that vanished whenever a node got selected would be worse
    /// than none. atlas owns the document; aurora owns the widget showing it.
    pub script_undo: Vec<(String, usize)>,
    pub script_redo: Vec<(String, usize)>,
    /// Where the caret was last time the buffer was read, so an undo step
    /// records the position the edit started from rather than where it ended.
    pub script_caret: usize,
    /// Whether the last edit joined the open undo step. A run only continues a
    /// run: without this, the first letter after a space would fold into the
    /// space's step and one undo would swallow both.
    pub script_coalescing: bool,
    /// The autocomplete popup over the Code pane, while one is open, and the
    /// byte range of the word it is completing.
    ///
    /// The range is held rather than re-read on accept, because pressing a row
    /// moves focus to that row - a button is not a text input - and the caret is
    /// a property of the focused widget, so by the time the click arrives there
    /// is no caret to ask about.
    pub completions: Option<(aurora::list::ListPopup, std::ops::Range<usize>)>,
    /// The find bar over the Code pane, while one is open.
    pub find: Option<aurora::find::FindBar>,
}

// --- text helpers ---
//
// Pure: text and a caret in, text and a caret out. They were free
// functions in main.rs among four hundred lines of unrelated ones; here
// they sit beside the state they serve and can be read as a set.

/// Whether an edit from `before` to `after` should join the previous undo step
/// rather than start a new one: a run of ordinary typing, so undo goes back a
/// word at a time instead of a letter at a time. Whitespace ends a run, which is
/// what makes "a word" the unit.
pub fn coalesces(before: &str, after: &str) -> bool {
    if after.len() != before.len() + 1 || !after.starts_with(before) {
        return false;
    }
    after
        .as_bytes()
        .last()
        .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
}

/// Comment or uncomment a block of lines.
///
/// Uncomments only when every non-blank line is already commented, so a block
/// where half is commented gets the rest commented rather than each line
/// flipped and the same mess left inverted. The `//` goes at the shallowest
/// indentation in the block, so the code keeps its shape, and uncommenting
/// takes back the single space it added - a round trip is exact.
pub fn toggle_comment_block(block: &str) -> String {
    let lines: Vec<&str> = block.split('\n').collect();
    let content: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|l| !l.trim().is_empty())
        .collect();
    let all_commented =
        !content.is_empty() && content.iter().all(|l| l.trim_start().starts_with("//"));
    let indent = content
        .iter()
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);

    lines
        .iter()
        .map(|line| {
            if line.trim().is_empty() {
                return (*line).to_string();
            }
            if all_commented {
                let at = line.find("//").expect("every content line has one");
                let rest = &line[at + 2..];
                let rest = rest.strip_prefix(' ').unwrap_or(rest);
                format!("{}{rest}", &line[..at])
            } else {
                format!("{}// {}", &line[..indent], &line[indent..])
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The diagnostic covering `offset`, narrowest first.
///
/// Narrowest wins because a wide diagnostic - "this function must return f32",
/// spanning a whole body - would otherwise hide the precise one inside it, and
/// the precise one is the one that says what to change. A zero-width span (an
/// unclosed brace, reported at the end of the file) is found at its own offset.
pub fn narrowest_at(all: &[comet::Diagnostic], offset: usize) -> Option<&comet::Diagnostic> {
    all.iter()
        .filter(|d| {
            let (lo, hi) = (d.span.start as usize, d.span.end as usize);
            offset >= lo && (offset < hi || (lo == hi && offset == lo))
        })
        .min_by_key(|d| d.span.end - d.span.start)
}

/// Collapse `block` onto one line: trailing whitespace goes, the next line's
/// indent goes, and the two are joined by a single space - or by nothing when
/// one side is empty, so a blank line does not become a stray space.
pub fn join_block(block: &str) -> String {
    let mut out = String::with_capacity(block.len());
    for line in block.split('\n') {
        let piece = line.trim();
        if piece.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(piece);
    }
    // The first line keeps its indentation: joining is about the seam, not
    // about moving the block to the margin.
    let indent: String = block
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();
    format!("{indent}{out}")
}

/// The swap ctrl+t should make at `caret`: the range to replace and what to put
/// there, or `None` when there is nothing to swap.
///
/// Characters, not bytes, so a multi-byte character is never split in half.
pub fn transposition(text: &str, caret: usize) -> Option<(std::ops::Range<usize>, String)> {
    let caret = caret.min(text.len());
    if !text.is_char_boundary(caret) {
        return None;
    }
    let line_start = text[..caret].rfind('\n').map_or(0, |at| at + 1);
    let line_end = text[caret..].find('\n').map_or(text.len(), |at| caret + at);
    let before = text[line_start..caret].chars().next_back()?;
    // At the end of a line there is nothing after the caret to swap with, so
    // take the two behind it - which is where the typo you just spotted is.
    let after = text[caret..line_end].chars().next();
    match after {
        Some(after) => {
            let start = caret - before.len_utf8();
            let end = caret + after.len_utf8();
            Some((start..end, format!("{after}{before}")))
        }
        None => {
            let earlier = text[line_start..caret - before.len_utf8()]
                .chars()
                .next_back()?;
            let start = caret - before.len_utf8() - earlier.len_utf8();
            Some((start..caret, format!("{before}{earlier}")))
        }
    }
}

/// The word the caret is in or next to: where it starts, and its text.
///
/// Unlike [`word_before`], this reaches forward as well, so the caret sitting at
/// the start of a name still finds that name.
pub fn word_before_or_around(text: &str, caret: usize) -> (usize, &str) {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let caret = caret.min(text.len());
    if !text.is_char_boundary(caret) {
        return (caret, "");
    }
    let start = text[..caret]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_word(*c))
        .last()
        .map_or(caret, |(i, _)| i);
    let end = text[caret..]
        .char_indices()
        .take_while(|(_, c)| is_word(*c))
        .last()
        .map_or(caret, |(i, c)| caret + i + c.len_utf8());
    (start, &text[start..end])
}

/// Strip trailing spaces and tabs from every line, and end the text with
/// exactly one newline. Returns the result and where `caret` ended up.
///
/// The caret's own line is spared when it holds nothing but whitespace: that is
/// a fresh auto-indent being typed into, and yanking it away on save would move
/// the caret to the margin under someone's hands.
///
/// The caret is kept on the same character. It has to be: the tidy runs through
/// the buffer, so the widget is rewritten under a caret that is still live.
pub fn tidy_for_save(text: &str, caret: usize) -> (String, usize) {
    let caret = caret.min(text.len());
    let mut out = String::with_capacity(text.len());
    let mut removed_before_caret = 0usize;
    let mut at = 0usize;
    for line in text.split_inclusive('\n') {
        let body = line.strip_suffix('\n').unwrap_or(line);
        let blank = !body.is_empty() && body.bytes().all(|b| b == b' ' || b == b'\t');
        let caret_here = caret >= at && caret <= at + body.len();
        let trimmed = if caret_here && blank {
            body
        } else {
            body.trim_end_matches([' ', '\t'])
        };
        let cut_from = at + trimmed.len();
        if caret >= at + body.len() {
            removed_before_caret += body.len() - trimmed.len();
        } else if caret > cut_from {
            removed_before_caret += caret - cut_from;
        }
        out.push_str(trimmed);
        if line.len() != body.len() {
            out.push('\n');
        }
        at += line.len();
    }
    // One trailing newline, not none and not four. Files this editor writes get
    // diffed and concatenated, and a missing final newline is a spurious
    // one-line diff forever.
    while out.ends_with("\n\n") {
        out.pop();
    }
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    let caret = caret.saturating_sub(removed_before_caret).min(out.len());
    (out, caret)
}

/// Where a completion popup should be anchored for a caret at `caret`, or
/// `None` if none should be open there.
///
/// One predicate, so "should there be a popup" has exactly one answer rather
/// than being decided in pieces at each place that might close one. A popup is
/// wanted while a name is being typed, and right after a `.` where the name has
/// not been started yet - and nowhere else, which is what takes it down when
/// the caret is clicked into whitespace eleven lines away.
pub fn completion_anchor(text: &str, caret: usize) -> Option<usize> {
    let (start, prefix) = word_before(text, caret);
    if !prefix.is_empty() {
        return Some(start);
    }
    // Right after a dot: field completions are the whole point of asking there.
    let after_dot = start > 0 && text.as_bytes()[start - 1] == b'.';
    after_dot.then_some(start)
}

/// The identifier the caret sits in or just after: where it starts, and its
/// text. An empty name means the caret is not in one.
///
/// comet's, not a second copy: this decides what a completion replaces, so the
/// language it is completing should be the one that says where a name begins.
/// The copy that used to live here stepped one *byte* past the character it
/// found, so a single non-ASCII character before the caret - an em dash pasted
/// into a comment - sliced mid-character and took the editor down. This runs on
/// every keystroke in the Code pane.
pub fn word_before(text: &str, caret: usize) -> (usize, &str) {
    comet::service::word_before(text, caret)
}
