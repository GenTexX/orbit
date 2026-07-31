# Code editor backlog

Everything the Code pane does not do yet, from a survey run on 2026-07-31, the
day after milestone 4's editor first worked. Ten passes over the code - one per
lens, each reading the real source before calling anything missing - then a
merge and a completeness critique. 229 entries.

This is a companion to [ideas.md](ideas.md), not part of it: the code editor is
one pane, and putting 229 entries about it in the general idea inbox would
drown everything else. Same rules apply - an entry is a captured thought, not a
promise; when one ships, delete it.

**Status, 2026-08-01.** All twenty defects are fixed, and the twenty entries
picked as most important are implemented and tested. They are struck through
below rather than deleted, because the reasoning in each is what a later reader
needs to know why the editor behaves the way it does; the next sweep should
delete them. Everything not marked DONE is still open - about 190 entries.

**How to read it.** The first section is defects, not features: behaviours that
lose text, discard state, or draw nothing where something should be. Three of
them were reproduced with a test before this document was written. Everything
after that is missing capability, grouped by what a person would recognize.

**Sizes** are rough: S is under about a hundred lines in one place, M a few
hundred across two crates, L a new subsystem or a change to how text is stored.

**A caution about ordering.** The survey's own ranking optimized for "most felt
per hour" as a text editor. That is the right question for a text editor and the
wrong one for Orbit right now: this is an educational game engine, and the pane
exists so a person can write ten lines of Comet and watch a sprite move. An
entry that makes the language legible to a beginner beats one that makes the
editor competitive with a mature IDE. The five sections near the end - running
a script, debugging one, teaching, one-handed use, and testing the editor
itself - are the load-bearing ones for that, and every one of them came from
the completeness critique rather than from the ten lenses. A pure text-editor
survey does not think to ask them.

## Defects: the editor loses work or lies about it

These are not missing features, they are behaviours that destroy text, discard
state, or draw nothing where something should be. Several fire on reflex
keystrokes. They should be cleared before anything is built on top of them.

Three were reproduced with a throwaway test before this document existed,
rather than taken on trust:

- Tab with the whole of `aaa\nbbb\nccc` selected left behind exactly `"    "`.
  **Fixed on the spot** (2026-07-31) rather than filed: it was a regression
  introduced the same day by making Tab indent, and it deletes a file on a
  reflex key. Tab and shift+Tab now indent and outdent whole lines. The entry
  is kept below for the record.
- `insert_str("one\ntwo\nthree")` produced `"onetwothree"`.
- A zero-width diagnostic span produced zero draw commands.

- **~~Tab over a multi-line selection deletes it~~** DONE (S, aurora) - FIXED 2026-07-31
  Tab with a selection spanning lines indents each touched line instead of
  replacing the selection with four spaces.
  Why: edit_key's Tab branch calls insert_str(INDENT), which does
  s.replace_range(lo..hi, " "). Selecting a function body and pressing Tab
  silently destroys it. Data loss on a reflex key.
  Note: The full block-indent behaviour is a separate entry; this defect is
  fixed the moment Tab-with-a-multiline-selection stops going through
  insert_str.

- **~~Paste drops every newline~~** DONE (S, aurora) - REPRODUCED
  ctrl+v of multi-line text into a multiline field inserts the newlines; CRLF
  normalises to LF. A single-line field still strips them.
  Why: Ui::insert_str filters with !c.is_control() and '\n'.is_control() is
  true, so three pasted lines weld into one. Also makes a newline-containing
  find/replace impossible, since replace_current goes through the same splice.
  Note: Gate on widget.multiline; the filter is load-bearing for single-line
  fields and has a test.

- **~~A gutter press arms a text sweep from a stale anchor~~** DONE (S, aurora)
  A press in the gutter neither moves the caret nor extends any selection,
  before or after the pointer moves.
  Why: handle_input sets self.pressed = self.hovered before focus_from_press,
  and focus_from_press returns early for a gutter press without clearing it.
  The next PointerMoved sees pressed == focused and drags the caret, sweeping
  from whatever anchor an earlier click left. Press a line number, twitch the
  mouse, and a chunk of the file is selected.

- **~~shift+Tab throws focus out of the file~~** DONE (S, aurora)
  shift+Tab with a text area focused outdents; it never moves focus. In a
  single-line field it still steps focus backwards.
  Why: edit_key guards indent with `multiline && !self.shift`, so shift+Tab
  falls through to focus_step(-1) and jumps to some other widget in the shell.
  Reaching for the standard outdent key ejects you from the file.
  Note: The test tab_indents_a_text_area_and_still_steps_focus_elsewhere pins
  today's behaviour as intended, so this is a decision to revisit
  deliberately. Escape or ctrl+Tab can carry the focus-escape duty.

- **~~ctrl+f leaves focus in the source file~~** DONE (S, atlas)
  ctrl+f opens the bar and focuses the query field with its text selected.
  ctrl+f while it is open re-focuses and re-selects rather than closing.
  Why: toggle_find never calls ui.focus, and the post-rebuild block restores
  focus to the code editor whenever it had it - which is exactly when ctrl+f
  was pressed. The most-used shortcut in any editor currently types your query
  into the buffer.
  Note: Needs a focus_find flag consumed after the bar is built, and the
  code_caret restore must not fire when it is set.

- **~~Enter in the find query moves focus into the buffer~~** DONE (M, aurora+atlas)
  Enter steps to the next match and keeps focus in the query field;
  shift+Enter steps back; Enter in the replacement field replaces and
  advances. Neither leaves the bar.
  Why: The query field is single-line, so Key::Enter runs focus_step(1) into
  the replacement field, and the app's Submitted handler then calls reveal,
  whose select_range steals focus to the editor. Type a query, press Enter,
  and your next keystroke lands in the source.
  Note: Wants a FindBar::key(Key, shift) -> outcome mirroring ListPopup::key,
  routed from handle_code_key before ui.handle_input.

- **~~A plain arrow with a selection steps past the wrong edge~~** DONE (S, aurora)
  With a selection, Left/Up collapse to the low edge without moving further
  and Right/Down collapse to the high edge; Home/End collapse then apply the
  line move.
  Why: edit_key clears selection_anchor and then steps the caret, so selecting
  a word and pressing Left lands one character before whichever end happened
  to be moving - which depends on drag direction. It silently loses your
  place.

- **~~ctrl+a scrolls the view to the bottom of the file~~** DONE (S, aurora)
  Select-all sets anchor 0 and caret text_len without scrolling; the visible
  rows stay put.
  Why: select_all moves the caret to text_len and the next layout's
  update_vscroll yanks the view to the end. ctrl+a then ctrl+c loses your
  reading position.
  Note: A do-not-reveal flag consumed by the next update_vscroll, or reveal
  only when a movement key moved the caret.

- **~~Wheel-scrolling away from the caret is undone next frame~~** DONE (S, aurora)
  Wheel-scrolling the focused editor to read elsewhere stays where you put it;
  the view only chases the caret when the caret actually moved.
  Why: update_vscroll runs at the end of every layout for the focused field
  and forces the caret line back into view, and layout runs every frame.
  Glancing at another function while writing one is undone before it is drawn.
  It also fights find: after a reveal, any manual scroll snaps back.
  Note: Remember the caret offset the scroll was last reconciled against and
  only re-follow when it changes. The existing test never calls layout after
  the wheel, which is why it passes.

- **~~The selection is destroyed by any shell rebuild~~** DONE (S, aurora+atlas)
  Anchor and caret are both captured before the Ui is swapped and restored
  after, so a rebuild triggered by anything else leaves the selection exactly
  as it was.
  Why: atlas restores only the caret with focus_caret, and Ui::focus_caret
  explicitly sets selection_anchor = None. A console line arriving mid-
  selection clears it, and ctrl+c then quietly copies nothing. toggle_find
  reads the selection to pre-fill the query and then sets dirty, so the
  selection it was searching for is gone by the next frame.
  Note: Needs a selection_range() reader and a focus_selection(id, anchor,
  caret) setter; Ui has caret_offset but no anchor reader.

- **~~The editor's scroll position is destroyed by any shell rebuild~~** DONE (S, aurora+atlas)
  After a rebuild the editor shows the same lines it showed before, whether or
  not the caret is on screen.
  Why: text_vscroll is a SecondaryMap on the Ui and a rebuild makes a new one;
  capture_dock_scrolls only captures registered scroll containers, which a
  text area is not. Wheel-scrolling does not focus the pane, so an unfocused
  scrolled editor snaps to line 1 when a log line arrives or a thumbnail
  decodes.
  Note: Restore after focus_caret, or update_vscroll overwrites it - which
  argues for a restore-scroll-without-chasing-the-caret flag rather than a
  plain setter.

- **~~A zero-width diagnostic draws no squiggle~~** DONE (S, aurora) - REPRODUCED
  A diagnostic whose span is empty still gets a visible mark: at least one
  character cell wide, at that offset or under the last character before it.
  Why: comet's lexer pushes Eof with span (pos, pos) and parser::expect
  reports at peek_span, so every `expected }` in a half-typed file is zero-
  length. LayoutRun::highlight yields nothing for an empty range, so
  range_rects returns empty and emit_decoration returns early on size.x <= 0.
  The commonest error state while typing renders nothing at all.

- **~~A blank line inside a selection or a decoration draws nothing~~** DONE (S, aurora)
  A selection or decoration crossing an empty line fills it with a stub about
  one space wide, so a multi-line selection reads as continuous.
  Why: range_rects gets its rects from LayoutRun::highlight, which iterates
  glyphs; an empty line has none. Selecting a block with a blank line in it
  looks like two separate selections, and a find match or squiggle can never
  land on an empty line.

- **~~Closing the window discards every unsaved edit~~** DONE (M, atlas)
  A Save / Discard / Cancel modal appears before any action that would drop a
  dirty buffer: closing the window, closing a tab, opening another file into
  the same slot, reloading the project. Cancel aborts entirely; several dirty
  documents list themselves and offer Save All.
  Why: WindowEvent::CloseRequested calls event_loop.exit() unconditionally.
  script_modified is tracked and nothing reads it before destroying the
  buffer.
  Note: winit's exit must be deferred: CloseRequested sets a pending-quit flag
  and returns, and the modal's answer calls exit(). modal.rs has no three-
  button confirm body yet, and nothing there is async, so the pending action
  rides on State.

- **~~Opening another script discards the current buffer~~** DONE (S, atlas)
  Double-clicking a second .cmt with unsaved edits prompts before replacing
  the buffer.
  Why: open_script_file never consults script_modified; it reads the new file
  and overwrites script_text, script_undo and script_redo. The scene-tree and
  explorer paths have the same hole for a dirty scene.

- **~~Renaming, moving or deleting the open file leaves the save path stale~~** DONE (S, atlas)
  Renaming or moving the open file updates the document's path in place;
  deleting it marks the document orphaned, the header says the file is gone,
  and ctrl+s becomes Save As.
  Why: commit_file_rename, file_delete and the cut-paste move path all rescan
  the explorer and never touch open_script. Rename player.cmt with it open and
  the next ctrl+s recreates the file you just renamed away, with your edits in
  it.
  Note: open_script holds a project-relative String while file ops work in
  absolute paths, so the comparison goes through explorer.project_relative; a
  folder rename above the file has to re-prefix it, which is why the check
  belongs in one place.

- **~~Save truncates the file before writing it~~** DONE (S, atlas)
  A save writes a sibling temp file in the same directory, flushes, then
  renames over the target. A failure anywhere leaves the original untouched
  and reports it.
  Why: save_script calls std::fs::write, which truncates then writes. A crash
  or a full disk leaves a zero-length .cmt where a working script was - the
  one failure mode where the editor destroys work rather than failing to
  preserve it. Project::save has the same shape.
  Note: The temp file must be in the target's own directory; file_ops.rs
  already owns atlas's filesystem vocabulary including unique_path.

- **~~A CRLF file mis-places the caret and grows mixed endings~~** DONE (M, atlas)
  The dominant ending is detected on open, the buffer always holds LF, and
  save writes the original ending back. Mixed endings normalise to whichever
  dominates, and the header says so.
  Why: open_script_file does read_to_string and keeps every CR; insert_newline
  inserts a bare '\n'; save writes verbatim. line_end finds the '\n', so End
  puts the caret between the CR and the LF - past the end of cosmic-text's
  line text, so cursor_position returns None, the caret stops drawing, and
  typing inserts inside the line terminator.
  Note: Normalising at the open/save boundary keeps aurora LF-only by
  contract; worth stating that in a doc comment on byte_to_cursor.

- **~~A UTF-8 BOM produces a diagnostic on an invisible character~~** DONE (S, atlas)
  A BOM is stripped on open, remembered, and written back on save. A file that
  is not valid UTF-8 is refused with a sentence naming the problem rather than
  a raw io::Error.
  Why: read_to_string neither strips a BOM nor explains itself, so comet's
  lexer sees 0xEF, falls through lex_punct, and squiggles line 1 column 1 on
  something the user cannot see or delete.
  Note: byte_to_cursor's boundary snapping is why this is quiet rather than a
  panic. modal::friendly_error already exists for the wording half.

- **~~Completion items are de-duplicated adjacently only~~** DONE (S, comet)
  A name appearing in two scopes is offered once.
  Why: names_in_scope does names.dedup() and completions_at does
  items.dedup_by(...), both adjacent-only, so a parameter sharing a name with
  state declared before another state name shows twice in the popup.
  Note: One-line fix: sort then dedup, or a HashSet on (label, kind).

## Caret and selection semantics

What the caret does when you move it, and what a selection is worth. Most of
these are single-function changes inside aurora that the user feels on every
keystroke.

- **~~The caret remembers a goal column across vertical moves~~** DONE (S, aurora)
  The first Up/Down records a goal x in pixels within the content box; every
  subsequent Up/Down hit-tests that x, so passing through a short line and
  continuing lands back on the original column. Any horizontal move, click,
  edit or Home/End clears it. PageUp/PageDown and shift+Up/Down keep it.
  Why: caret_vertical re-derives x from the caret's current position on every
  press, so one short line permanently truncates the column: Down through a
  blank line and the caret is stuck at column 0 for the rest of the file. It
  is fatal once there are several carets - three carets stepped past one short
  line collapse onto one offset and typing triples there.
  Note: cosmic-text 0.19's Buffer::cursor_motion(font_system, cursor,
  cursor_x_opt, Motion::Up/Down) threads exactly this and would also hand over
  SoftHome, PageUp/PageDown and BufferStart/End; adopting it needs a &mut
  FontSystem in edit_key.

- **~~Smart Home: first non-whitespace, then column 0~~** DONE (S, aurora)
  Home moves to the line's first non-whitespace character; again from there
  goes to column 0; from column 0 back to the first non-whitespace. shift+Home
  extends over the same two stops.
  Why: Home goes to line_start, i.e. always column 0, so on an indented line -
  which is every line in a func body - the caret lands in the whitespace and
  typing inserts code to the left of the indent.
  Note: cosmic-text's Motion::SoftHome is the first stop; the toggle needs one
  bit of state or just a compare of caret against line_start and first_non_ws.

- **Home/End act on the wrapped visual row before the logical line** (M, aurora)
  On a soft-wrapped line Home goes to the start of the visual row the caret is
  on and End to its end; pressing again moves to the logical line start/end.
  End on a row ending in a soft wrap lands before the wrap.
  Why: The code pane wraps, but line_start/line_end only know hard newlines,
  so End on a long wrapped line jumps several rows down and scrolls the view
  when you were trying to reach the end of what you can see.
  Note: Needs a run-lookup-by-caret helper (the LayoutRun whose line_i and
  byte span contain the caret) - the same helper wrap affinity and
  PageUp/PageDown want.

- **~~ctrl+Home / ctrl+End jump to the ends of the document~~** DONE (S, aurora+atlas)
  ctrl+Home puts the caret at 0 and scrolls to the top, ctrl+End at text_len
  and scrolls to the bottom; shift+ctrl+ them selects to that end. Up on the
  first visual line also moves to offset 0.
  Why: translate_key maps Home/End identically with and without ctrl (only the
  arrows are ctrl-upgraded), and in a multiline field Home is line-scoped - so
  there is no keyboard route to the top of a file at all. Up at the top is a
  silent no-op because caret_vertical clamps target_y with .max(0.0) and re-
  hits the same row.

- **~~PageUp / PageDown~~** DONE (M, aurora+atlas)
  Move the caret by the number of visual rows that fit in the field's height
  minus one row of overlap, and scroll by the same amount so the caret keeps
  its on-screen row. They respect the goal column; shift+ extends; ctrl+ stays
  free for tab switching.
  Why: NamedKey::PageUp/PageDown are not translated at all and Key has no
  variant for them, so the keys are dead. In a file longer than the pane the
  only keyboard navigation is one line at a time.
  Note: cosmic-text derives the page from Buffer::size().1, which aurora
  leaves as None, so the page height has to come from text_area_extent's
  view_h.

- **Word motion stops at punctuation, and alt+arrow moves sub-word** (M, aurora (classifier) + atlas (alt+arrow mapping))
  ctrl+arrow classifies into whitespace / word chars (alphanumeric plus
  underscore) / punctuation and stops at every transition between the two non-
  whitespace classes, so ctrl+right through `self.pos.x = fn(a);` stops at
  self, ., pos, ., x, =, fn, (, a, ), ;. alt+arrow stops at camelCase humps
  and underscores, so worldToLocal is three stops and max_speed is two.
  Why: prev_word_boundary/next_word_boundary split on whitespace only, so
  ctrl+right skips a whole member expression and there is no way to land on an
  operator or a field name. Editing a member access - the commonest thing in a
  comet script - needs character arrows.
  Note: The same classifier should back double-click, ctrl+shift+arrow and
  ctrl+Backspace so all four agree. atlas::word_before has a third definition
  for completion prefixes - worth reconciling.

- **~~Double-click selects the word under the pointer~~** DONE (M, aurora)
  Two presses on the same field within the double-click window and within a
  few pixels select the word containing (or immediately before) the hit
  offset: anchor at word start, caret at word end. Inside a whitespace run it
  selects the run; past the end of a line it just places the caret; on
  punctuation it selects the punctuation run.
  Why: Selecting an identifier is the most common selection in any editor and
  the gateway to find-from-selection, rename and select-next-occurrence. Today
  the only route is a precise drag.
  Note: InputEvent::PointerPressed carries no click count, so aurora needs its
  own Instant-based counter. atlas has the pattern to copy in click_tree_row
  and click_file_entry.

- **~~Triple-click selects the whole line~~** DONE (S, aurora)
  A third press in the window selects the logical line from its start through
  and including the trailing newline, so a follow-up cut removes the line
  whole. On the last line it selects to the end of the text.
  Why: Line selection by mouse is how a line gets moved or deleted, and
  including the newline is what stops a cut-then-paste gluing two lines
  together.

- **Dragging after a double or triple click extends by word or line** (M, aurora)
  The press that selected a word or line remembers the granularity and the
  original range; dragging snaps so the selection covers whole words (or
  lines) and never shrinks below the initial range in either direction.
  Release keeps the snapped range; the next plain press resets granularity.
  Why: Without it, double-click-and-drag - how a phrase or a block gets
  selected - degrades to character granularity the instant the pointer moves,
  cutting the word you just selected in half.
  Note: Needs the anchor to become a range (anchor_lo/anchor_hi), which
  shift+click and multi-selection also want.

- ~~**Shift+click extends the selection instead of restarting it**~~ DONE (S, aurora)
  A press with shift on the focused field leaves the anchor where it is (or
  drops one at the caret if there is none) and moves only the caret, extending
  or shrinking. shift+drag keeps extending. Without shift the anchor resets,
  as today.
  Why: focus_from_press sets selection_anchor = Some(caret) unconditionally,
  so shift+click destroys the selection you were extending. Selecting a block
  taller than the pane needs one uninterrupted drag, which is impossible
  without drag autoscroll.
  Note: focus_from_press already has self.shift available.

- **Dragging past the edge of the field autoscrolls** (M, aurora+atlas)
  While a selection drag is live and the pointer is above or below the text
  rect, the field scrolls toward it at a rate proportional to the overshoot
  (roughly 1 to 20 lines/second) with the caret following, continuously while
  the pointer is held still. It stops at the document ends and on release.
  Same for a single-line field dragged past its sides.
  Why: The selection only advances on a PointerMoved event, so holding the
  mouse below the pane does nothing. Selecting more than one screenful is
  currently impossible.
  Note: Needs a per-frame tick, not a pointer event: atlas coalesces moves in
  flush_pointer and requests a redraw every frame, so the tick has a home.
  caret_at_x's local.y.max(0.0) makes an upward overshoot jump straight to
  line 0 rather than one line at a time.

- ~~**Escape collapses the selection to the caret**~~ DONE (S, aurora+atlas)
  With the editor focused and no popup open, Escape collapses the selection
  (keeping the caret at the moving end) and leaves focus alone. A completion
  list or find bar still gets Escape first; with nothing open and no selection
  it drops editor focus rather than clearing the scene selection.
  Why: Escape does nothing in the editor today: handle_code_key returns false
  with no popup open, handle_editor_key bails because a field is focused, and
  translate_key filters the control character. A stray selection can only be
  cleared by clicking.

- ~~**An unfocused field keeps showing its selection and current line**~~ DONE (S, aurora)
  When focus moves to another widget, the editor keeps drawing its selection
  in a desaturated inactive colour and still marks the caret's line (fainter);
  only the caret itself disappears. Clicking back restores the active colours
  without changing the range.
  Why: emit_text_input draws the selection only `if focused` and emit_gutter
  computes caret_line only when focused, so clicking into the find bar makes
  the match you were about to replace vanish - the exact case where you need
  to keep seeing it.
  Note: Cheap: the same FillRects in different Colors. Inverting the text
  under a selection would NOT be cheap - DrawCommand::Text is one colour per
  run, so it would mean splicing the colour spans.

- **~~The caret blinks, and holds solid while typing~~** DONE (S, atlas (timer and reset points) + aurora (a caret-moved hook or a last-movement instant))
  Solid for about 500ms after any caret move or edit, then on/off at roughly
  530ms. Solid while a drag-selection is live, hidden when the window loses
  focus, phase reset on every keystroke.
  Why: atlas never calls Ui::set_caret_on, so the editor's caret is a
  permanently solid 1.5px bar - hard to find on a busy coloured line and no
  signal that the pane has focus.
  Note: aurora-wgpu/examples/inspector.rs already has the recipe at
  BLINK_MS=530. atlas redraws every frame, so no new wake-up source is needed
  - but gate it on editor focus if the deferred idle-redraw work lands.

- **The caret picks the right visual row at a soft-wrap boundary** (M, aurora)
  Clicking at the start of a wrapped continuation row draws the caret there,
  not at the end of the row above. End on the row above, or Right off its end,
  draws at the end of the upper row. Same byte offset, two reachable places.
  Why: The caret is a bare byte offset and cursor_to_byte throws cosmic-text's
  Affinity away, so byte_to_cursor always rebuilds Affinity::Before and
  Buffer::cursor_position takes the first run matching the line - always the
  upper row. On a wrapped line the caret draws a whole row from where you
  clicked.
  Note: Storing (byte, affinity) rather than byte is a change to how the caret
  is stored, so it is worth doing at the same time as the caret-set rework
  rather than twice. LayoutRun::cursor_glyph does not consult affinity, so the
  disambiguation is in choosing which run to ask.

- **~~The app can read the selection, and the status bar shows Ln/Col~~** DONE (S, aurora (accessors) + atlas (the readout))
  Ui exposes the focused field's selection as byte offsets and the caret as
  (line, column) counted in characters with tabs expanded to the indent width.
  The status bar reads `Ln 12, Col 5` while the editor is focused and `Ln 12,
  Col 5 (37 selected, 3 lines)` with a selection, and clears when focus
  leaves.
  Why: selected_text returns a String and selection_bounds is private, so
  atlas cannot ask what is selected in offsets - which blocks block indent,
  comment toggle, search-in-selection and any readout. The status bar shows
  world cursor, zoom, scene selection, fps and modified, and nothing about the
  file being edited, so a diagnostic quoted by line number cannot be located.
  Note: update_status_bar sets labels in place with no rebuild, which matters
  because caret movement must not dirty the shell.

- **Gutter click selects the line; gutter drag selects a range of lines** (M, aurora (line-to-range mapping and the drag) + atlas (the handler); the app keeps GutterClicked for breakpoints)
  A press on a line number selects that whole logical line including its
  trailing newline and puts the caret at its start; dragging down extends
  whole lines, dragging above the start line extends upward; shift+click
  extends from the existing anchor. ctrl+l does it from the keyboard and
  repeats swallow the next line.
  Why: Event::GutterClicked is emitted with the line and nothing in atlas
  matches on it - grep the workspace and it appears only in aurora and its
  tests. The gutter looks interactive, tracks the caret with its band, and
  does nothing when clicked.
  Note: Depends on the gutter-press defect above. Aurora only emits on the
  press, so a drag needs it to keep reporting while the button is held.

- ~~**Opening a script focuses the editor, with the caret where you left it**~~ DONE (S, atlas)
  Double-clicking a .cmt opens the pane, focuses the text area and places the
  caret at offset 0, or at the caret and scroll it had if the file was open
  before this session. Switching dock tabs and back restores the same caret;
  closing and reopening within a session does not reset it.
  Why: open_script_file sets the text and activates the pane but never focuses
  the field, so the file appears with no caret and the first keystroke goes
  nowhere. There is no per-file caret memory either.
  Note: A fresh Ui has no text_vscroll entry, so update_vscroll re-derives
  scroll from the caret and parks the caret line at the BOTTOM of the view;
  restoring scroll alongside the caret fixes that jump too.

## Multiple carets and column editing

One structural change (a caret set) followed by a long tail of small ones.
This is the group that changes what the editor is for - renaming a field on
eight lines, adding a trailing comma to a list - and it should be designed
whole before any of it is attempted.

- **A caret set replaces the single flat caret** (L, aurora)
  Ui stores an ordered, non-overlapping Vec<Caret> (each with offset, anchor:
  Option<usize>, and a goal x) instead of caret: usize plus selection_anchor:
  Option<usize>, with one designated primary (the last-added). Carets stay
  sorted by offset. With exactly one caret every existing behaviour is bit-
  identical to today.
  Why: ui.rs:218 and :225 hold one usize and one Option<usize> on the Ui
  itself, not even per widget. Every other entry in this group is blocked on
  it: without it, "add a caret" has nowhere to put the caret.
  Note: The blast radius is enumerable: selection_bounds, selected_text,
  select_all, focus_caret, caret_offset, delete_selection, select_range,
  insert_str, edit_key, insert_newline, caret_vertical, caret_at_x,
  caret_x_offset, update_hscroll, update_vscroll, multiline_caret, caret_rect,
  emit_text_input, emit_gutter. Keeping caret/selection_anchor as accessors
  returning the primary keeps atlas compiling on day one.

- **Aurora learns that alt and ctrl are held** (S, aurora + one line in atlas)
  Ui::set_alt(bool) and Ui::set_ctrl(bool) beside set_shift, driven from
  atlas's ModifiersChanged arm and consulted by focus_from_press and the
  PointerMoved drag branch.
  Why: InputEvent carries no modifier state and PointerPressed has no payload;
  atlas pushes only shift. Aurora physically cannot tell an alt+click from a
  click, so every mouse-driven entry below is blocked - as is ctrl+click go-
  to-definition.
  Note: Mirror set_shift exactly. Do not put modifiers on InputEvent variants
  - that churns every existing call site for no gain.

- **Alt+click adds a caret; alt+click on an existing caret removes it** (M, aurora)
  Alt+click inside the focused text area appends a caret at the clicked offset
  and makes it primary, leaving the others and their selections alone.
  Alt+click landing on an existing caret (same offset or within a couple of
  pixels) removes that one instead. Alt+click on a different widget still
  moves focus normally. Alt+drag from a fresh alt+click sweeps a selection for
  the new caret only.
  Why: focus_from_press unconditionally sets the caret and anchors a
  selection, so every click destroys whatever was there. This is the primary
  way anyone creates a second caret.

- **ctrl+alt+up / ctrl+alt+down adds a caret on the line above or below** (M, aurora (a public Ui::add_caret_vertical(dir)) + atlas routing)
  Adds one caret directly above the topmost (up) or below the bottommost
  (down) at the same goal column and makes it primary. Repeating extends the
  column; the opposite direction retracts the last-added caret rather than
  adding on the far side. A no-op at the first or last line.
  Why: This is the keyboard half and the only one usable without a mouse. It
  cannot be expressed today: Key has 12 variants, none carry modifiers, and
  ctrl+arrow is already claimed by WordLeft/WordRight.
  Note: Prefer a public Ui method over new Key variants; handle_shortcut-style
  routing is where every other chord already lives.

- **Every edit applies at every caret, back to front** (M, aurora)
  insert_char, insert_str, insert_newline, Backspace, Delete, word-wise
  deletion and Tab-indent splice at each caret, running from the highest
  offset down so earlier splices do not invalidate later offsets. Afterwards
  each caret shifts by the total length delta below it. The whole pass is one
  invalidate_text.
  Why: insert_str and edit_key each splice one range and assign self.caret.
  Without this a caret set is decorative: you can create carets but only one
  types.
  Note: find.rs's replace_all already establishes the back-to-front idiom;
  reuse its shape. The mask check in insert_str must test the candidate after
  ALL carets' edits, not per caret.

- **Carets merge when they collide** (S, aurora)
  After every edit and every movement, carets on the same offset collapse into
  one, and carets whose selections overlap or touch merge into the union
  keeping the surviving one's direction. The primary survives any merge it
  takes part in.
  Why: Without it, ctrl+alt+down through a short line, or Backspace with two
  adjacent carets, leaves two carets on one byte and every later keystroke
  inserts twice there. This is the commonest way naive multi-cursor
  implementations corrupt a buffer.

- **Motion keys resolve per caret against their own line** (M, aurora)
  Home/End move each caret to its own line's start/end; Left/Right/word motion
  move each independently and then merge; shift+motion extends each caret's
  own selection; select_all collapses to one caret spanning the buffer.
  Why: Home/End route through line_start/line_end for the single caret and
  select_all sets one anchor. A column of carets where Home sends them all to
  one line's start is worse than no multi-cursor.

- **Escape collapses to one caret** (S, aurora (Ui::collapse_carets() -> bool) + atlas)
  With more than one caret Escape drops all but the primary, clears its
  selection, and consumes the key. With one caret it keeps its other meaning.
  In atlas the order is completion popup, find bar, collapse carets, then the
  existing scene-selection clear.
  Why: There has to be a way out, and the Escape slot in the code pane is
  currently free.

- **Each caret draws its own bar and its own selection** (S, aurora)
  emit_text_input draws one caret rect per caret and one selection highlight
  per caret with an anchor, all blinking in phase off caret_on. The primary is
  drawn identically to the rest.
  Why: emit_text_input emits exactly one selection and one caret keyed off
  self.caret. Carets you cannot see are carets you type into by accident.
  Note: N extra FillRects per frame and no re-shape; range_rects already does
  the per-line work, so this is a loop around an existing call.

- **A primary caret that the view, the popups and the readout follow** (S, aurora)
  One caret is primary: update_vscroll reveals it, caret_rect returns it (so
  the completion popup and any signature hint anchor there), the Ln/Col
  readout describes it, and it survives an Escape collapse. The primary is the
  most recently added caret. Typing at N carets does not scroll if the primary
  is already visible; adding a caret off-screen scrolls to it because it
  becomes primary.
  Why: Without a designated primary the view fights itself (every caret wants
  revealing) and the popup has no defensible anchor. "Keep all carets visible"
  is impossible once they span more than a screen, and scrolling to each in
  turn jitters every keystroke.
  Note: Add caret_rects(id) for drawing while caret_rect(id) keeps returning
  the primary.

- **Copy and cut with N selections** (S, aurora)
  selected_text with several non-empty selections returns them joined by
  newlines in document order; cut removes all of them as one edit. With N
  carets and no selections, copy takes each caret's whole line.
  Why: selected_text slices exactly one range and atlas uses it for both copy
  and cut, so with N carets copy would silently take only the primary's text -
  which looks like it worked.

- **Paste N lines into N carets, and column paste** (M, aurora (Ui::insert_multi) + atlas passes the clipboard string)
  If the clipboard has exactly as many lines as there are carets, line i goes
  to caret i in document order with each caret's selection replaced; otherwise
  the whole text goes to every caret. Text copied from a rectangular selection
  pastes as a rectangle: each clipboard line onto a successive buffer line at
  the caret's column, padding short lines with spaces, appending past the end.
  Copy/cut with N selections records the count so a round trip is lossless.
  Why: This is the payoff of multi-cursor - copy a column of names, paste them
  each on their own line. Without the count rule the common case pastes the
  whole block N times and produces N-squared garbage.
  Note: arboard carries only text, so the "this was a block/line-wise copy"
  flag is atlas-side state, invalidated when the OS clipboard no longer
  matches what atlas last wrote.

- **One undo step per multi-caret edit, restoring the caret set** (M, atlas (it owns the document history deliberately) + a caret-set getter/setter on aurora)
  A single keystroke applied at N carets is one undo step; undo restores the
  buffer and all N carets with their selections so you can undo, adjust and
  retype without rebuilding the set. A run of multi-caret typing coalesces
  word-granularly the way single-caret typing does.
  Why: script_undo stores (String, usize) - buffer plus one caret - so undoing
  drops you to one caret somewhere arbitrary. Worse, coalesces only joins when
  after.len() == before.len()+1 && after.starts_with(before), i.e. one byte
  appended at the very end of the buffer, so a 3-caret keystroke never
  coalesces and a 6-letter word becomes 6 undo steps.
  Note: coalesces already fails to coalesce ANY mid-file single-caret typing
  for the same reason - the fix is the same: compare edits, not whole-string
  prefixes.

- **Carets survive the shell rebuild** (S, atlas + Ui::carets()/set_carets())
  The whole caret set, with selections and the primary index, is captured
  before atlas swaps the Ui and restored after, exactly as the single caret is
  today.
  Why: atlas captures one Option<usize> and restores it with focus_caret,
  which sets selection_anchor = None and one caret. A caret set that
  evaporated the first time a log line arrived would be unusable, and the
  failure would look random.

- **Add a caret at the next occurrence of the selection (ctrl+d)** (M, aurora (the caret plumbing) + atlas (the shortcut, beside ctrl+f))
  With a selection, ctrl+d finds the next occurrence of exactly that text
  after the last caret, adds a caret there with it selected, and scrolls it
  into view, wrapping at the end. With no selection it first selects the word
  under the caret and only adds a caret on a second press. ctrl+shift+L adds a
  caret at every occurrence at once; ctrl+k ctrl+d skips the current one.
  Nothing else matching does nothing rather than beeping.
  Why: This is what people actually use multi-cursor for - rename a local,
  retype a repeated field - and the only gesture that does not need the
  occurrences to line up in a column. comet has no rename, so this is how a
  rename gets done.
  Note: The search itself is find.rs's find_all, which already returns exact
  byte ranges. Respect how the word came in: from a word selection match whole
  words, from an arbitrary drag match the substring.

- **Alt+drag makes a rectangular (column) selection** (L, aurora)
  Alt+drag sweeps a rectangle in screen space; on release there is one caret
  per crossed visual row, each selecting the rectangle's x-span on that row. A
  zero-width drag straight down yields N bare carets. Dragging back up past
  the origin shrinks the set rather than inverting it. shift+alt+arrow grows
  the block by a row.
  Why: The mouse gesture for column editing and the only ergonomic way to put
  carets on twenty lines. Today the PointerMoved branch only ever moves the
  single caret, producing a linear selection.
  Note: Buildable from primitives that exist: buffer.hit(x0, line_y) and
  buffer.hit(x1, line_y) per line, which is what caret_at_x already does for
  one point. Monospace makes column-from-x a divide in the code pane.
  Realistically needs no-wrap mode first - a column across folded lines is not
  a column.

- **The short-line rule for column selections** (S, aurora)
  A rectangular selection whose x-span reaches past a short line's end puts
  that line's caret at the line end with an empty selection (clamp) rather
  than skipping the line or padding it with spaces; typing then inserts at the
  line end. A caret is still created for a completely empty line inside the
  rectangle.
  Why: This is the fork that decides whether column editing is usable.
  Clamping is what VS Code does and what people expect; padding silently
  rewrites lines you never touched; skipping short lines - the easiest
  implementation - drops carets out of the middle of the column for no visible
  reason.

- **Insert a caret at the end of every selected line** (S, aurora)
  With a selection spanning several lines, shift+alt+I replaces it with one
  caret at the end of each selected line.
  Why: The fastest route from "I selected a block" to "I have a column of
  carets", and the one multi-caret gesture that is correct with soft wrap on
  because it is defined on logical lines rather than pixel columns - so it can
  ship before the no-wrap work.
  Note: line_start/line_end already walk hard newlines and are exactly the
  primitives needed.

- **Alt+click in the gutter adds a caret on that line** (S, aurora)
  Alt+click on a line number adds a caret at the start of that line's text,
  after leading indentation, without disturbing the others. A plain gutter
  click keeps emitting GutterClicked.
  Why: The gutter is a wide, easy target and the natural way to build a
  sparse, non-contiguous caret set - the case ctrl+alt+down cannot reach.
  Note: focus_from_press already intercepts gutter presses and reports the
  line; the branch is one arm away.

- **The gutter marks every caret's line** (S, aurora)
  The current-line band is drawn for every caret's line; a line holding
  several carets gets one band, not several overlapping fades.
  Why: emit_gutter computes a single caret_line from self.caret, so a 20-line
  column of carets would band exactly one line - which reads as "the other 19
  are not real".
  Note: The band colour is theme.selection.fade(GUTTER_CURRENT_FADE); drawing
  it twice on one line doubles the alpha, so dedupe lines before emitting.

- **Autocomplete does something defensible with N carets** (M, atlas)
  With more than one caret, either suppress the popup outright or replace each
  caret's own prefix word on accept. Pick one and make it explicit;
  suppressing is the honest cheap answer.
  Why: refresh_completions does word_before on a single caret and
  accept_completion does select_range + insert_str - a single splice in one
  place. With N carets it would fire on the primary's prefix and insert in
  exactly one of N spots, leaving the others' half-typed words behind. That is
  a data-loss-shaped surprise, not a missing feature.

- **The editor says how many carets are live** (S, atlas)
  The status bar shows the caret count whenever there is more than one, e.g.
  "3 carets", beside the Ln/Col readout.
  Why: Carets sit off-screen and multi-caret typing edits places you cannot
  see. Without a count, the first sign that you still have four carets is four
  copies of what you just typed.

## Indentation and whitespace

Comet is a brace language whose every statement lives at least one level deep,
and the editor currently contributes exactly four spaces on Tab. This group is
what makes typing a block feel finished rather than half-done.

- **~~Enter keeps the current line's leading whitespace~~** DONE (S, aurora)
  Enter inserts a newline followed by a copy of the leading whitespace of the
  line the caret was on, leaving the caret after it. Enter inside the leading
  whitespace copies only what is left of the caret; on a whitespace-only line
  it copies that whitespace; with a selection it deletes the selection first
  and indents from the line the selection started on.
  Why: insert_newline inserts a bare "\n" and puts the caret at column 0, so
  every line in a func body is re-indented by hand, four spaces at a time. A
  two-level-deep if body costs two Tab presses per line.
  Note: insert_newline sets self.caret = lo + 1 assuming one inserted byte;
  that becomes lo + 1 + indent.len(). Leading-whitespace copy changes
  behaviour for every text area, so it likely wants a per-widget Style flag
  (Style::auto_indent()).

- **~~An opening brace at end of line adds a level~~** DONE (S, aurora)
  When the line being left ends - ignoring trailing whitespace and a trailing
  line comment - with '{', the newline is followed by that line's leading
  whitespace plus one indent unit. Only the last non-whitespace character is
  inspected.
  Why: Without it, every func and if body starts flush with its own header, so
  the file only looks indented if the user does the work. The bundled fixtures
  already use four spaces per level.
  Note: A '{' inside a string or comment must not count, so `let s = "{";`
  needs the veto hook from the brackets group; a purely textual last-char test
  is wrong.

- **~~Enter between a brace pair opens a block~~** DONE (S, aurora)
  With the caret directly between '{' and '}', Enter produces three lines: the
  '{' line unchanged, a blank line one level deeper with the caret on it, and
  a line holding '}' at the opener's own indentation.
  Why: This is the payoff move - type '{', press Enter, be inside a properly
  formed block - and the one people notice missing most. It is also what makes
  auto-close worth having.
  Note: Needs to insert text containing two newlines and place the caret in
  the middle; insert_newline hardcodes one "\n" and insert_str strips control
  characters, so this wants a shared private replace_range(lo, hi, text,
  caret_at).

- ~~**Typing a closing brace dedents its own line**~~ DONE (S, aurora)
  Typing '}' when everything left of the caret on that line is whitespace
  removes one indent unit before inserting, so the brace lines up with the
  line that opened the block. With no indentation, or with non-whitespace to
  the left, '}' is just a character.
  Why: Auto-indent alone leaves every closing brace one level too deep, which
  then costs four Backspaces per block.
  Note: insert_char currently just forwards to insert_str; this would be its
  first character-specific behaviour. Same string/comment caveat. Dedenting by
  exactly one unit is the honest cheap version; matching the opener's real
  column needs the bracket index.

- **~~Tab and shift+Tab indent or outdent a whole multi-line selection~~** DONE (M, aurora (the Tab branch) + atlas (restore the selection across the rebuild the first edit triggers))
  With a selection spanning more than one line, Tab adds one indent unit to
  every touched line and shift+Tab removes up to one from each (fewer where
  there is less). Blank lines are skipped by indent and untouched by outdent.
  The selection is re-expanded to cover the same lines including the inserted
  characters, so a second Tab indents the same block again. A selection ending
  at column 0 does not drag the following line in.
  Why: See the defect entry - today this deletes the block. It is also how
  anyone fixes indentation after a paste.
  Note: Must compute the new bounds itself since anchor and caret are plain
  byte offsets that both shift. ctrl+] / ctrl+[ should map to the same two
  commands so it works without a selection.

- **Tab indents to the next tab stop** (S, aurora)
  Tab with no selection inserts as many spaces as reach the next multiple of
  the indent width from the caret's current display column: column 2 goes to
  4, column 4 to 8. A literal tab already on the line counts as its expanded
  width.
  Why: insert_str(INDENT) always inserts four spaces, so Tab on a line sitting
  at column 2 produces column 6 - a depth no other line has. Indentation
  drifts off the grid and never comes back.
  Note: Column measured from the line start, not the byte offset, so a UTF-8
  identifier earlier on the line does not skew it.

- ~~**Backspace deletes a whole indent level**~~ DONE (S, aurora)
  Backspace when everything between line start and caret is whitespace deletes
  back to the previous tab stop (up to one indent unit) in one press: column 6
  lands on column 4, not column 0. Elsewhere, and with a selection, it keeps
  today's behaviour.
  Why: Backspace removes exactly one grapheme, so undoing one level of
  indentation is four presses and two levels is eight.

- **An abandoned auto-indent is dropped** (S, aurora)
  Enter on a line whose entire content is whitespace removes that whitespace
  before inserting the newline, so three Enters in a row leave two genuinely
  empty lines rather than two lines of trailing spaces. Same when focus leaves
  the editor.
  Why: Auto-indent manufactures trailing whitespace as a side effect of its
  own helpfulness; without this the feature makes files dirtier than they are
  today, and save-time trimming only hides it behind a spurious modified flag.
  Note: The stateless rule (trim any whitespace-only line on Enter regardless
  of who put it there) is behaviourally almost identical to tracking
  authorship and needs no state.

- **Indent width and tabs-vs-spaces are configurable** (M, aurora (the widget reads it) + atlas (owns and persists the value))
  A setting chooses the indent width (default 4) and whether an indent is
  spaces or a real tab. Tab, shift+Tab, Backspace-over-indent, auto-indent and
  every bulk re-indent read it. Changing it does not rewrite the open buffer.
  Why: INDENT is a private const in ui.rs and nothing can reach it, so any
  project indenting with 2 spaces or tabs is stuck.
  spectrum::settings::Settings carries theme, show_grid, show_axes and snap
  and has no editor section at all.
  Note: Choosing tabs also needs insert_str's control-character filter to stop
  eating '\t', or a tab can never be typed or pasted. Best as a Style/Ui-level
  setting rather than a global const, so a form field and a code pane can
  differ.

- **The configured indent width reaches the shaped buffer** (S, aurora)
  Aurora calls Buffer::set_tab_width with the configured width when it shapes
  a text input, so a literal tab occupies the same columns as a spaces-indent
  of the same depth.
  Why: cosmic-text defaults tab_width to 8 and aurora never overrides it, so a
  tab-indented file displays at 8 columns while Tab types 4 spaces. A mixed
  file lines up in no editor but this one - and rulers, indent guides and any
  column arithmetic inherit the disagreement.
  Note: cosmic-text already flags TAB_WIDTH-dirty lines, so it costs nothing
  on files with no tabs. ShapeCacheEntry keys on content/font/face/width and
  needs the tab width folded in or it serves a stale shape.

- **An opened file's own indentation is detected and adopted** (S, atlas (open_script_file))
  On open, leading whitespace is scanned: if most indented lines start with a
  tab the buffer uses tabs, otherwise the commonest non-zero indent step
  becomes the width. A file with no indented lines keeps the default. What was
  detected shows unobtrusively next to the file name.
  Why: Otherwise opening a 2-space file and pressing Tab once puts 4 spaces in
  it and the file becomes mixed. Orbit will read scripts written by other
  tools.
  Note: Store per open buffer, not globally, so it does not leak into the next
  file.

- ~~**Trailing whitespace is stripped on save**~~ DONE (on save only, no on-demand command yet) (S, atlas)
  An opt-in setting (default on) removes trailing spaces and tabs from every
  line on write, skipping the caret's own line when it is currently
  whitespace-only so a fresh auto-indent is not yanked away. A command does
  the same over the selection, or the whole buffer with none. The caret and
  selection stay on the same character and the change goes through the buffer
  path so undo reverses it.
  Why: save_script writes script_text verbatim, and the project's own demo
  file already has a line of four spaces and nothing else. Auto-indent will
  manufacture far more.
  Note: Must write the trimmed text back into the widget as well as to disk,
  or the buffer and the file disagree and the modified flag comes straight
  back.

- ~~**A final newline is ensured on save**~~ DONE (S, atlas)
  An opt-in setting (default on) appends a single '\n' when the buffer does
  not end in one and collapses two or more trailing blank lines to one. The
  caret does not move.
  Why: demo_project/test.cmt ends with '}' and no newline. Files this editor
  writes will be diffed and concatenated; a missing final newline is a
  spurious one-line diff forever.
  Note: Same write-back requirement; do it in the same pass as the trailing-
  whitespace strip so save costs one buffer rewrite.

- **Trailing whitespace is visible** (S, atlas (apply_script_marks))
  Spaces and tabs at the end of a line get a faint warning-tinted background,
  on every line except the one the caret is on so typing a trailing space does
  not flash.
  Why: Whitespace you cannot see is whitespace you cannot fix, and this is the
  cheapest win in the group: the mechanism is already wired.
  Note: Zero new vocabulary - a Decoration with DecorationStyle::Highlight
  resolving through range_rects, one FillRect per offending line, merged into
  the single decoration vector apply_script_marks already rebuilds each frame.

- **Indent guides** (M, aurora (emit_text_input))
  A faint 1px vertical rule at each indent stop a line's leading whitespace
  passes through, spanning the run of lines at least that deep, with the guide
  at the caret's own depth brighter. Off by default on non-code fields.
  Why: With brace-delimited, soft-wrapped code and no folding, depth is the
  main thing the eye needs and raw whitespace is the only cue. It is the
  standard way a mis-indented line becomes obvious at a glance.
  Note: One FillRect per guide segment; needs the mono advance width, so it
  should be mono-only. Computed per logical line from the text, not per glyph.
  Wrapped continuation rows show the guide at the wrong depth until hanging
  indent or no-wrap lands.

- **Whitespace can be rendered visibly** (M, aurora)
  A toggle draws a faint middle dot per space and a faint arrow per tab,
  either everywhere or only inside selections. Off by default.
  Why: The only way to answer "is this line tabs or spaces?" without deleting
  characters and watching how far the caret jumps - which matters exactly when
  a file is mixed.
  Note: Awkward in this vocabulary: DrawCommand::Text is one glyph run in one
  colour, so marks cannot interleave with the code's own coloured runs. Either
  a parallel shaped buffer of dots and arrows at the same positions (mono, so
  advances line up) drawn as its own Text command, or a 2x2 FillRect per
  space, which needs no font work. Affordable for one editor pane, not for
  every text input.

- **Reindent by brace depth, over the selection or the whole file** (M, atlas (it needs comet::service::highlight to know which braces are real))
  Rewrites each line's leading whitespace to depth times the indent unit,
  where depth is the running brace nesting from the start of the file and a
  line beginning with '}' draws at depth minus one. Nothing but leading
  whitespace changes; braces in strings and comments do not count. One undo
  step, selection restored over the same lines.
  Why: The pragmatic 80 percent of format-document, and it works on a file
  that does not parse - which is precisely when indentation is worst.
  Note: Applies via the script_history pattern (set_text_input +
  focus_caret/select_range), so no new aurora API.

- **Convert a file's indentation between tabs and spaces** (S, atlas)
  Two commands rewrite only leading whitespace: to tabs (each full indent-
  width run of spaces becomes one tab, a partial remainder stays spaces) or to
  spaces (each tab expands to the next stop). Whitespace after the first non-
  whitespace character is never touched.
  Why: Detection adopts what a file uses; sometimes the answer is that the
  file is wrong. Mixed tabs and spaces render differently in every tool and
  are the classic source of code that looks correctly indented and is not.
  Note: Leading-whitespace-only is load-bearing: a naive global replace
  corrupts every '\t' inside a string literal.

## Brackets, quotes and pairs

One shared answer to "what pairs with this bracket" unlocks six visible
features; auto-close is a separate mechanism that needs a language veto to be
trustworthy. Comet is entirely braces and parenthesised conditions, so this
group carries more weight here than in a typical editor.

- **~~Comet answers what pairs with this bracket~~** DONE (S, comet)
  A comet::service call over lex_with_comments walks
  LBrace/RBrace/LParen/RParen with a stack and returns per bracket token: its
  byte span, its partner's span or none, its nesting depth, and whether it is
  a stray closer or a never-closed opener. Brackets inside Str and Comment
  tokens are invisible. Never fails on a half-typed file - an opener with no
  closer comes back unmatched, not as a parse error.
  Why: Six entries below need the same answer. Without it each grows its own
  brace scan and the first '}' in a string makes them all disagree with the
  syntax colours - the exact failure highlight() was built to avoid.
  Note: Cache in atlas beside script_spans and invalidate in analyze_script,
  not in apply_script_marks, which runs every frame. There are no [ ] in comet
  at all, so the table is two pairs.

- **~~The matching bracket lights up when the caret touches one~~** DONE (S, comet+atlas)
  Look at the byte before the caret first, then the byte at it, so both `}|`
  and `|{` match. On a hit, add a Highlight on both single-byte ranges in a
  colour distinct from the selection. Recomputed when the caret moves, not
  only when the text changes, and cleared the moment the caret is on neither
  end.
  Why: In a func containing a while containing an if, nothing tells you which
  '}' closes which. This is the one bracket affordance used every minute and
  it costs two decorations and no re-shape.
  Note: Must go into the vector apply_script_marks builds; a second
  set_decorations owner erases the first. The new part is the trigger -
  sync_script_buffer returns early when the text is unchanged, so nothing in
  atlas currently reacts to a caret-only move.

- **The enclosing pair lights up when the caret is inside a block** (S, atlas)
  When the caret is on no bracket, mark the innermost enclosing pair more
  faintly. Moving from `{ | }` into a nested `{ { | } }` moves the mark
  inward. Nothing is marked at the top level.
  Why: The caret is almost never on a bracket, so without this the highlight
  only appears in the moment you type or step past one, and "which block am I
  in" - the question you actually have - goes unanswered.
  Note: A fade of the match colour rather than a new theme entry; Color::fade
  is already used for the gutter band.

- **~~An unmatched bracket is squiggled where it is~~** DONE (M, comet+atlas)
  An opener with no closer gets an error on the opener ("this { is never
  closed"); a closer with no opener gets one on the closer. When the caret
  sits on an unmatched bracket, the match highlight uses the error colour so
  it reads as "there is nothing to show you".
  Why: Deleting a '}' mid-file today squiggles the last character of the file
  with "expected } to close this block" - the error is at the far end from the
  mistake and everything after re-parses wrong meanwhile.
  Note: The parser's own expect(RBrace) also fires, so diagnostics() must
  suppress one or the same mistake double-squiggles at both ends. Suppress the
  lexer-level one when the parser already named the same block - the parser's
  is the one that drives recovery.

- **~~A pair closed by the wrong bracket marks both ends~~** DONE (S, comet)
  A closer that does not match the kind on top of the stack is reported on
  both the closer and the opener it collided with ("( closed by }"), and both
  draw in the error colour when the caret touches either.
  Why: The commonest brace typo after a plain missing one, and the case where
  a naive matcher actively lies - it will draw a match between a ( and a } and
  send a jump to the wrong place.

- **~~Jump to the matching bracket, and select to it~~** DONE (S, atlas)
  ctrl+m with the caret on or just after a bracket moves the caret past its
  partner, so twice returns you home. Inside a block and on no bracket, it
  jumps to the enclosing opener, and again to that block's closer. With shift
  held it selects to just past the partner instead - inside a block that is
  the whole block including both braces. Silent no-op with no partner.
  Why: Reading a 200-line script means constantly asking where a block ends,
  and scrolling to find out loses your place. "Select this function body" and
  "select these call arguments" are the two selections a code editor is asked
  for most and today both are a careful drag across a scrolling region.
  Note: handle_shortcut reads WinitKey::Character under ctrl, so no new Key
  variant. focus_caret plus update_vscroll reveals the target; select_range
  focuses and reveals in one call.

- **Expand and shrink the selection by scope** (M, comet (a containing-node chain for a byte range) + atlas (the shrink stack))
  alt+up grows the selection to the smallest syntactic node strictly
  containing it: caret, identifier, expression, statement, block interior,
  block with braces, function, file. alt+down retraces exactly, using a stack
  pushed on the way out, so four expansions and four contractions land on the
  original caret. Any manual selection change or edit clears the stack.
  Why: The fastest way to select an argument, then the call, then the
  statement, without aiming a mouse at a 7px character - and the keyboard
  route to selecting a block to fold, cut or indent.
  Note: Better off the AST than off brackets: Expr::span and the per-Stmt
  spans give real steps a brace walk cannot see. On a file that does not
  parse, fall back to the bracket index. atlas must hold the stack - aurora
  keeps one anchor and no history.

- **~~Typing an opener or a quote auto-closes it~~** DONE (M, aurora (pair table, per-widget flag, pending state) + atlas (the language veto))
  Typing (, { or " in the code editor inserts the pair and leaves the caret
  between the halves. Only a typed character does this: paste, find-and-
  replace, completion accept and insert_str in general insert exactly what
  they were given. Off by default per widget, so inspector fields and the find
  query never grow a stray ).
  Why: The most-felt typing affordance in any code editor, and comet is brace-
  heavy - every function body, every if, every while, every call.
  Note: The hook must sit in insert_char, not insert_str - that boundary is
  what makes "only typing pairs" fall out for free. Needs a
  Style::auto_pairs() alongside multiline()/monospace()/gutter().

- **~~No auto-closing inside a string or a comment~~** DONE (S, comet (a context_at(source, offset) on the same lex_with_comments pass) + atlas hands aurora a predicate so aurora stays language-free)
  Typing ( inside `// note (see below` inserts one (. Typing " inside an open
  string closes that string rather than pairing. Typing { inside "a {b}"
  inserts one {. The editor asks comet whether the offset is code, string or
  comment and skips the pair everywhere but code.
  Why: Prose in comments is full of unbalanced parens, and a comment that
  grows a ) every time you type ( is worse than no auto-close. This is the
  rule that decides whether the feature is trusted enough to leave on.
  Note: Same argument service.rs makes for highlight living in comet. The
  lexer closes an unterminated string at EOF, so the answer is well-defined
  mid-typing, which is the only time it is asked.

- **~~Auto-close only when the next character is a boundary~~** DONE (S, aurora)
  The pair is inserted only at end of line, or when the next character is
  whitespace, ), }, comma, semicolon or dot. Typing ( immediately before
  `speed` gives `(speed`. For a quote the rule is tighter: no pair when the
  character before the caret is a word character, so " after `let s` pairs but
  retyping the closing quote of "abc does not open a new pair.
  Why: Without it, editing an existing line is where auto-close turns from
  help into damage: wrapping a call in parens produces `()foo(x)` every time.

- **Auto-close is skipped when it would unbalance a closer already there** (S, comet+atlas)
  Typing ( at the start of `|foo)` inserts only (, because the line already
  has an unmatched ) the new opener will pair with. Same for {. Checked
  against the bracket index, not by counting characters on the line.
  Why: This is exactly the wrap-an-existing-expression move; without the check
  you get `(|)foo)` and delete a paren every time. It is the difference
  between auto-close being safe on existing code and safe only on a blank
  line.
  Note: Uses the same veto channel as the string/comment rule, so no new
  plumbing.

- **~~Typing the closer steps over one that was auto-inserted~~** DONE (S, aurora)
  After `print(` auto-closed to `print(|)`, typing ) moves past the existing )
  and inserts nothing. A small stack of pending closers records each auto-
  inserted offset; offsets shift with edits before them, and an entry drops
  when the caret leaves its range, when the text there stops being that
  closer, or on focus change. Typing ) where no pending closer sits inserts a
  real one.
  Why: Everyone types the closer out of habit for the first month, and without
  this the file fills with )). This is the half that makes the other half
  survivable.
  Note: The cheap alternative - step over whenever the next char equals the
  typed closer - is wrong in the case that matters: typing ) at `foo(|)bar()`
  when you genuinely meant to add one.

- **~~Backspace between an empty auto-pair deletes both halves~~** DONE (S, aurora)
  With the caret at `(|)` where the ) is a tracked auto-insertion, one
  Backspace leaves nothing. If anything was typed between them, or the closer
  was not auto-inserted, Backspace removes only the opener - a { deleted from
  the top of a 40-line block never takes that block's } with it.
  Why: Typing ( and thinking better of it should cost one keystroke. The
  negative half matters more: a Backspace that silently removes a brace 40
  lines away is a data-loss bug.

- **~~Typing a quote or bracket with a selection surrounds it~~** DONE (S, aurora)
  With `pos.x + 1.0` selected, typing ( yields `(pos.x + 1.0)` with the
  selection preserved over the original text so a second ( nests another pair.
  " does the same. A closing ) or } with a selection still replaces it like
  any character.
  Why: Wrapping an expression to fix precedence is a two-second operation
  everywhere else and a retype here - and today typing ( over a selection
  destroys it, the worst outcome for the exact keystroke people reach for.
  Note: insert_str replaces lo..hi unconditionally, so this needs its own
  path: insert the closer at hi first, then the opener at lo, then set
  anchor/caret to lo+1..hi+1. The other order invalidates hi.

- **An auto-pair and its type-over fold into the surrounding undo step** (S, atlas)
  Typing `print("hi")` then one ctrl+z goes back to before `print`, not
  through four states where parens appear and disappear on their own. An auto-
  inserted closer never gets its own undo entry, nor does a type-over that
  consumes one.
  Why: Undo is how you recover from an auto-pair doing the wrong thing, so it
  must not have made the history stranger. coalesces joins only a single
  appended word character, so ( plus its auto ) is a two-character append that
  breaks the run and chops the history.
  Note: The clean fix is for aurora to say what the last edit was (typed,
  paired, stepped-over, spliced) rather than atlas guessing from before/after
  strings.

- **Brackets are coloured by nesting depth** (M, comet (depth) + atlas (palette and persistence))
  Each bracket takes a colour from a small cycling palette keyed on depth, so
  a pair's two halves always agree and a nested pair differs from its parent.
  Unmatched and mismatched brackets take the error colour, making a missing
  brace visible as a colour shift down the rest of the file. Toggleable.
  Why: In `if (a && (b || c)) {` the eye has no other way to pair the parens,
  and free diagnosis: the moment the colours stop alternating as expected, a
  brace is missing above.
  Note: analyze_script returns None for all punctuation today, so bracket
  spans are purely additive and stay disjoint, which is what color_at
  requires. Cost is a few extra Text runs per line since a colour change
  breaks the batch. The palette needs N entries in theme.rs's settings-
  persisted colour table.

- **The gutter marks the extent of the block the caret is in** (M, aurora (draws the gutter, so it needs an API to mark a line range) + atlas (computes it))
  A thin vertical band in the gutter strip from the enclosing opener's line to
  its closer's line, under the numbers. Moving into a nested block shrinks it.
  Gone at the top level and while the enclosing brace is unmatched.
  Why: The current-line band says where you are; nothing says how far the
  thing you are inside extends. On a screenful of a long function that is the
  difference between knowing you are in the while and scrolling up to check.
  Note: emit_gutter already knows each run's line_i and pixel top, so this is
  one FillRect per contiguous run range - a 2-3px column, since DrawCommand
  has no line primitive. Shape the API like set_decorations rather than
  inferring more state inside aurora.

- ~~**Accepting a function completion brings its call parens**~~ DONE (S, atlas)
  Accepting a completion whose kind is Function inserts `name()` with the
  caret between the parens. Variable, field, type and keyword completions
  insert their label unchanged.
  Why: Every function completion is followed within a keystroke by (. The
  service already classifies which ones those are and the popup already knows
  the accepted item's kind; not using either is a small daily tax.
  Note: accept_completion takes only the label because refresh_completions
  flattens items to labels; both that and completion_row/ListPopup need to
  carry CompletionKind through. Going via insert_str means the inserted ) is
  not a tracked auto-insertion, so either register it with the pending stack
  or accept the double.

- **Typing an opening paren accepts the highlighted completion first** (S, atlas)
  With the popup open on `pri`, typing ( accepts `print`, inserts the call
  parens and closes the popup, giving `print(|)`. Any other non-word character
  (space, ;, .) dismisses the popup without accepting and leaves what was
  typed.
  Why: The fastest path from three letters to a call and what the muscle
  memory does. Today typing ( leaves `pri(` and a stale popup, because only
  Up/Down/Enter/Tab/Escape are routed to the list.
  Note: handle_code_key returns early unless the key is a named one, so it
  needs a character branch first - and it must run before the auto-pair hook,
  or the ( pairs against the half-typed word.

## Editing commands and the keymap

The edits bound to a key rather than typed. Almost all of them are a few lines
of string work once two primitives exist, and together they are the difference
between a text box and an editor. They also want one table instead of a fourth
if-ladder.

- **~~A newline-safe text mutation API on Ui~~** DONE (S, aurora)
  Ui::replace_range(id, range, text) replaces an arbitrary byte range with
  arbitrary text, newlines included, then places the caret and optionally a
  selection at a caller-given offset. Control characters other than '\n' are
  still filtered and a single-line field still rejects newlines.
  Why: Every command below is "replace these bytes with those bytes". Today
  the only public mutators are insert_str (which strips '\n'),
  delete_selection, and set_text_input (whole buffer), so a command that
  inserts a line can only rewrite the entire file - re-shaping everything and
  throwing away the caret.
  Note: insert_str is also the paste and find-replace path, so the filter
  becomes multiline-aware rather than disappearing. Keep the Mask::accepts
  check on the candidate.

- **~~Duplicate line or selection (ctrl+shift+d)~~** DONE (S, atlas)
  With no selection, copy the caret's whole line and insert it directly below
  with the caret at the same column on the copy. With a selection, insert an
  exact copy immediately after it and select the copy so repeats stack. Works
  on a last line with no trailing newline.
  Why: The commonest structural edit while scripting - copy a let or a
  component setup and change one number. Today it is select-line, ctrl+c, End,
  Enter, ctrl+v, and the paste eats the newline.
  Note: One Ui mutation so sync_script_buffer records one undo step.

- **~~Move line or selection up and down (alt+up / alt+down)~~** DONE (S, atlas)
  alt+up swaps the caret's line with the one above and keeps the caret on the
  moved text at the same column; alt+down swaps downward. With a selection the
  whole block of touched lines moves as a unit and stays selected. At the
  first or last line it is a no-op, not a wrap.
  Why: Reordering statements is constant, and the alternative is cut-and-
  paste, which currently loses the newline. Keeping the caret on the moved
  line is what makes hold-to-repeat usable.
  Note: Routing is the fiddly part: alt+Arrow falls through handle_mode_key
  (which bails on alt) into translate_key, which maps it to plain Key::Up/Down
  and just moves the caret. Intercept before translate_key; self.modifiers
  already carries alt_key().

- **~~Delete line (ctrl+shift+k)~~** DONE (S, atlas)
  Delete the caret's whole line including its newline; with a selection, every
  line it touches. The caret lands at the same clamped column on the line that
  moved up. Deleting the last line removes the preceding newline instead,
  leaving no blank line.
  Why: The counterpart to duplicate and the fastest way to clear dead code.
  Home, shift+Down, Delete is three keys and gets the trailing newline wrong
  half the time.

- ~~**Join lines (ctrl+shift+j)**~~ DONE (S, atlas)
  Replace the newline at the end of the caret's line with a single space,
  collapsing the next line's leading indentation, caret at the join. With a
  selection spanning N lines, join all of them. If the next line is empty,
  just remove the newline with no stray space.
  Why: The inverse of Enter and the fix for a line that got split during
  editing; manually it is End, Delete, then deleting indentation one space at
  a time.
  Note: Strip trailing whitespace on the first line before inserting the
  space, or joining an indented block leaves double spaces.

- **~~Toggle line comment (ctrl+slash)~~** DONE (M, atlas)
  With no selection, prefix the caret's line with `// ` at its first non-
  whitespace column, or strip an existing one there. With a selection, if
  every non-blank touched line is already commented, uncomment all; otherwise
  comment all, inserting at the shallowest indentation of the block so the
  comment column lines up. The selection stays over the same lines.
  Why: Commenting out a block to bisect a script is the most-used editor
  command that does not exist here - and comet has no debugger, so commenting
  out lines IS the debugging technique.
  Note: comet has line comments only, so `//` is the only correct prefix.
  Blank lines inside a selection are skipped when commenting but must not veto
  the all-commented test.

- **Toggle block comment (ctrl+shift+slash)** (M, comet (lexer + highlight) then atlas)
  Wrap the selection in /* */ or unwrap it when already exactly wrapped, so a
  mid-line range can be commented. With no selection, act on the line. Nested
  block comments either nest or are refused - pick one and make the lexer
  agree.
  Why: Commenting out the middle of an expression is not something line
  comments can do, and it is how you bisect a single long line.
  Note: Blocked on the language: the lexer recognises // only, so /* lexes as
  divide-then-star, the checker reports errors through the commented region,
  and the highlighter colours it as code. Until then the honest fallback is to
  comment each line with // and say so.

- ~~**Transpose characters (ctrl+t)**~~ DONE (S, atlas)
  Swap the character before the caret with the one after and advance one
  character. At end of line, swap the two before the caret. At line start, a
  no-op. Operates on chars, not bytes.
  Why: The standard fix for the commonest typo class, one keystroke instead of
  backspace-retype.
  Note: Use prev_boundary/next_boundary or char_indices so a multi-byte
  character is never split.

- ~~**Change the case of the selection (upper / lower / title)**~~ DONE (S, atlas)
  ctrl+shift+u uppercases the selection, ctrl+shift+l lowercases, a third
  binding title-cases (first letter of each word up, rest down). With no
  selection, act on the word under the caret. The selection survives so the
  three can be tried in sequence.
  Why: Renaming a constant to SCREAMING_CASE or fixing a stray CapsLock line
  is a retype today, and it is the cheapest command once the range API exists.
  Note: to_uppercase can change byte length, so recompute the new selection
  end from the replacement's length - the same trap find.rs's case-folded
  search documents.

- ~~**Sort selected lines, and drop duplicates**~~ DONE (S, atlas)
  Sort the touched lines in place, ascending by byte order, stably, keeping
  each line's indentation; a second binding reverses and another removes
  adjacent duplicates. The selection still covers the same lines. Fewer than
  two lines is a no-op.
  Why: Sorting a block of let declarations or a list of asset names has no
  other route in a GUI editor, and it is the command that most wants "expand
  the selection to whole lines" as a shared verb.
  Note: The last selected line may lack a trailing newline; normalise before
  sorting and restore after or the final line joins.

- ~~**Insert line below or above without splitting (ctrl+enter / ctrl+shift+enter)**~~ DONE (S, atlas)
  ctrl+enter opens a new line below the caret's line and moves there
  regardless of where in the line the caret was; ctrl+shift+enter does the
  same above. Both copy the current line's leading whitespace.
  Why: The caret is almost never at end of line when you decide to add one
  after it, so today it is End then Enter, and Enter drops to column 0 so you
  retype the indent.

- ~~**Delete by word (ctrl+backspace / ctrl+delete)**~~ DONE (S, aurora (two Key variants plus edit_key arms) + atlas (translate_key))
  ctrl+backspace deletes back to the previous word boundary, skipping trailing
  whitespace first then the word; ctrl+delete deletes forward. Over a
  selection they behave like plain Backspace/Delete. A run of whitespace
  deletes as one unit.
  Why: ctrl+left/right motion already exists, so deleting by word is the
  missing half of a pairing users assume. Today ctrl+backspace deletes one
  character because translate_key maps Backspace with no regard for ctrl - the
  key looks bound and silently does the wrong thing.
  Note: prev_word_boundary/next_word_boundary already exist. Adding the
  variants forces edit_key's unreachable!() arm to be revisited.

- ~~**Delete to end of line, and to start of line**~~ DONE (S, aurora (Key variants, so it works in any text area) or atlas)
  ctrl+k deletes from the caret to the end of its line leaving the newline;
  again on an empty remainder removes the newline and pulls the next line up.
  ctrl+u deletes back to the first non-whitespace character, and again to
  column 0.
  Why: Clearing the tail of a line you are rewriting is shift+End then Delete
  today, a two-hand reach; line-start deletion is how you re-indent a line
  from scratch.
  Note: line_start/line_end are exactly the offsets these need.

- ~~**Cut and copy with no selection act on the whole line**~~ DONE (M, atlas)
  ctrl+c with a collapsed caret copies the caret's entire line including its
  newline; ctrl+x cuts it, closing the gap and leaving the caret at the same
  column on the following line. A clipboard entry captured this way pastes as
  a whole line above the caret's line, wherever the caret sits.
  Why: ctrl+c with no selection does nothing at all today (selected_text
  returns None), so the reflex reads as a broken keyboard. The line-wise paste
  half is what saves the time - otherwise you paste a line into the middle of
  another line.
  Note: arboard carries only text, so the line-wise flag is atlas-side state,
  invalidated when the OS clipboard stops matching what atlas last wrote.

- **A clipboard ring (ctrl+shift+v cycles recent cuts)** (M, atlas)
  Every in-app copy/cut pushes onto a bounded ring (16 entries, newest first,
  duplicates of the head collapsed). ctrl+v pastes the head; ctrl+shift+v
  immediately after a paste replaces what was just pasted with the next older
  entry, repeating cycles further back. Any other edit ends the cycle.
  Why: Rearranging a script means cutting several fragments and re-inserting
  them; with one slot each cut destroys the last. The replace-the-last-paste
  interaction makes it discoverable without a picker.
  Note: Reconcile the head with the OS clipboard on window focus. A picker
  could reuse aurora::list::ListPopup, which already does filtering, keyboard
  selection and click-to-accept.

- **Drag selected text to move it** (M, aurora (it owns selection and the caret) + atlas (cursor hint))
  Pressing inside an existing selection and dragging past a small threshold
  picks the text up rather than sweeping a new selection: a drop caret follows
  the pointer, releasing moves the text there (adjusting for a source range
  before the drop point), ctrl at release copies instead. Escape or releasing
  inside the source cancels. A press with no drag collapses the caret there,
  as now.
  Why: Direct manipulation is how a mouse-first user reorders an argument;
  today every press inside the selection destroys it, so a mis-aimed click
  after a careful selection loses it entirely.
  Note: PointerMoved unconditionally sweeps when pressed == focused, so the
  press handler must record whether it landed inside selection_bounds and
  defer. The drop caret is one FillRect at an offset caret_rect can already
  compute; a translucent ghost of the dragged text would need a text-range
  equivalent of emit_drag_feedback.

- **A right-click context menu over the code pane** (M, atlas)
  Right-click opens Cut / Copy / Paste / Duplicate Line / Toggle Comment /
  Find (and Go to Definition later), acting on the selection if there is one
  and the caret's line if not. Right-clicking inside a selection keeps it;
  outside one moves the caret first, mirroring the tree-row rule in
  open_context_menu.
  Why: Every other surface in atlas has one - open_context_menu falls through
  to close_menu() for the code pane, so right-clicking the editor visibly does
  nothing. It is also the only discoverability route for commands nobody has
  memorised.
  Note: MenuAction is a flat enum of scene/file actions; this is where it
  starts wanting a nested or generic form. The menu should name commands from
  the table, not duplicate their bodies.

- **One command table and a rebindable keymap** (M, atlas)
  An EditCommand enum with a single apply(&mut self, cmd) that reads text plus
  selection, computes one replacement, applies it, sets caret and selection,
  and marks one undo step; a default (chord -> command) table consulted before
  translate_key; user overrides in the settings file; commands unit-testable
  headlessly on (text, selection) pairs with no window. An unknown or
  conflicting binding is reported rather than ignored.
  Why: handle_shortcut is a 100-line ladder of eq_ignore_ascii_case with three
  focus contexts interleaved, and
  handle_editor_key/handle_mode_key/handle_code_key each own a slice of the
  keyboard. Adding fifteen commands to that guarantees a conflict nobody
  notices - alt+arrow already reaches translate_key by falling through two
  handlers that decline it for different reasons. Nobody can rebind anything.
  Note: The hard part is precedence, not the table: a completion popup owning
  Enter and the arrows, a modal owning Escape, a focused field owning ctrl+A
  are context filters, not bindings. It should also fix undo granularity - a
  command-shaped undo entry carries the selection it started from, which
  script_undo's (String, usize) cannot. docs/internals/ideas.md already flags
  an action map as a prerequisite for gameplay input; land on one table, not
  two.

## Find, replace and in-file navigation

The bar works and its search is correct; what is missing is everything around
it - focus, keyboard stepping, scope, and the ability to search more than one
file. Several entries here are also outright focus bugs, listed in the defects
group.

- **~~Find as you type~~** DONE (S, atlas (a sync_find_query each frame, like sync_tree_filter))
  Every keystroke in the query field re-searches: the count updates, all
  matches re-highlight, and the view previews the first match at or after the
  caret. No Enter needed. Deleting back to empty clears the highlights and
  leaves the caret where it started.
  Why: The query reaches the FindBar only on Submitted (Enter or a focus
  change), so until you press Enter the bar shows an empty count and no
  highlights while the field visibly holds text. Worse, the widget text and
  bar.query are out of sync, so any unrelated rebuild re-emits the field from
  bar.query and erases what you typed.
  Note: Copy the refocus_filter/filter_caret dance or the rebuild loses the
  caret. find_all is an O(n*m) byte scan re-run over the whole buffer per
  keystroke - fine at script size, worth remembering before a 5k-line file.

- **~~F3 and ctrl+g step matches with the bar closed~~** DONE (M, atlas)
  F3 / ctrl+g goes to the next match of the last query, shift+F3 /
  ctrl+shift+g the previous, whether or not the bar is open. Escape keeps the
  query alive for these; they wrap and announce the wrap.
  Why: Stepping matches needs the mouse on the < / > buttons today, and
  clicking either moves focus into the editor via reveal, so you cannot click
  twice without re-aiming. The point of a find is to walk occurrences while
  reading code with the bar out of the way.
  Note: handle_code_key reads winit keys directly, so NamedKey::F3 needs no
  change to aurora's Key enum. Requires the query and match list to outlive
  the bar's visibility - today `self.find = None` on close throws the whole
  search away.

- **The bar remembers its query, replacement and toggles between openings** (S, atlas)
  Closing and reopening restores the previous query (still pre-empted by a
  selection), the replacement text, and the match-case / whole-word / regex
  toggles. Only changing project resets them.
  Why: toggle_find builds a fresh FindBar::new every time, so match_case falls
  back to false and the replacement is gone. Escape to look at the code and
  ctrl+f to resume means retyping both fields and re-arming the toggles -
  which is how a Replace All ends up running case-insensitively by accident.
  Note: Prerequisite for F3-with-the-bar-closed; do it once and both land.

- **Escape puts the caret back where the search started** (S, aurora (a public text-scroll setter) + atlas (the origin))
  Opening the bar records the caret offset and the editor's scroll. Escape
  closes the bar, clears the highlights, and returns both with no selection.
  Accepting a match (clicking in the editor, or a dedicated commit) keeps the
  new position instead, so Escape is a cancel and not an undo of your
  navigation.
  Why: Escape just drops the bar today, leaving the caret on whatever match
  was last revealed with that match still selected - so a search done to check
  something has silently moved you, and the next keystroke overwrites the
  selected match.

- ~~**Searching starts at the caret, not at the top of the file**~~ DONE (S, aurora (a FindBar::seek_from(byte)))
  Opening the bar, or changing the query, selects the first match at or after
  the caret rather than match #1. Clicking elsewhere in the editor while the
  bar is open re-bases the next find. Wrapping past the end returns to the
  first match.
  Why: refresh keeps the ordinal, which is right for surviving edits but wrong
  for starting a search: with the caret on line 200 you type a query and get
  thrown to line 3, then click > a dozen times to walk back.
  Note: A separate entry point, not a change to refresh - the
  an_edit_leaves_you_on_the_same_match_by_ordinal test is the contract to
  preserve.

- ~~**Say when the search wrapped**~~ DONE (S, aurora (return whether it wrapped) + atlas (show it))
  Stepping past the last match to the first (or before the first to the last)
  shows "wrapped to the top" / "wrapped to the bottom" beside the count for a
  couple of seconds, or on the status bar.
  Why: find_next is a silent modulo, so with four matches in a long file the
  view jumps from the bottom to the top and nothing distinguishes that from a
  mis-click - you lose your place and cannot tell whether you have seen every
  occurrence.
  Note: update_status_bar sets labels with no rebuild, which is the cheap
  place for transient search feedback.

- ~~**Whole-word matching**~~ DONE (S, aurora)
  A third toggle beside Aa restricts matches to ones whose neighbouring
  characters are not alphanumeric or underscore, so `pos` in `let pos = 1.0;
  let post = pos` finds two, not four. Toggling re-searches and keeps you on
  the nearest surviving match.
  Why: Auditing an identifier is the commonest reason to search code and
  substring hits swamp the result - the crate's own doc example shows pos
  matching inside post. Without it, Replace All on any short identifier is
  unusable.
  Note: Boundary test on chars, not bytes, using the same word rule as
  prev_word_boundary so it agrees with the editor's word motion.

- **Regex search** (L, aurora)
  A .* toggle switches the query to a regular expression: classes, anchors,
  alternation, quantifiers, and \n / \t escapes so a multi-line pattern fits a
  single-line field. An invalid pattern tints the field and says "bad pattern"
  rather than silently matching nothing. A zero-width match advances by one
  character.
  Why: Every non-trivial code edit is a pattern, not a literal - `fn \w+\(`,
  trailing whitespace, a call with any argument - and it is the gate on
  capture-group replacement, the only way to do a mechanical rewrite without
  visiting every site.
  Note: No regex dependency in the workspace, so this is a new dep or a small
  backtracking engine. Two invariants change: matches stop being the same
  length as the query (the case-folding comment assumes they are), and the
  empty-needle early return needs a zero-width guard instead.

- **Replace with capture groups** (S, aurora)
  With regex on, $1 / $0 in the replacement expand to the match's groups and
  $$ is a literal dollar. Replace All expands per match, so back-to-front
  splicing still applies.
  Why: Without it regex only finds; the rewrite is still typed at every site.
  Swapping `set_x(a, b)` to `set_x(b, a)` across a script is the canonical
  case.
  Note: replace_all snapshots the ranges and splices one string for all of
  them; it needs the per-match captures captured in the same snapshot, since
  after the first splice the text no longer matches.

- **Preserve the case of what was replaced** (S, aurora)
  An AB toggle: replacing foo with bar rewrites foo->bar, Foo->Bar, FOO->BAR.
  Mixed-case matches fitting no pattern take the replacement verbatim. Active
  only while match-case is off.
  Why: Renaming an identifier that appears as a type, a variable and a
  constant currently needs three searches, and getting it wrong produces code
  that still compiles and reads wrong. It is what makes a case-insensitive
  Replace All safe to press.
  Note: Keep the mapping ASCII-only for the same reason find_all folds ASCII
  in place, or the replacement length stops being predictable.

- **Search within the selection** (M, aurora (a scope: Option<Range<usize>> honoured by refresh and replace_all) + atlas (the toggle))
  Opened over a selection spanning more than one line, the bar defaults to in-
  selection: matches outside are not listed, counted or highlighted, and
  Replace All only touches the range. A toggle turns it off. Edits inside grow
  or shrink the range; an edit deleting it turns the scope off rather than
  leaving it stale.
  Why: Replace All over a whole file is the most dangerous button in the bar
  and scoping it to a selected block is the standard safety valve. Today
  toggle_find does the opposite of what a multi-line selection implies - it
  copies the whole block in as the literal query, which matches once, itself.
  Note: The scope must be kept live by the same call that keeps matches live
  (sync_script_buffer -> find.refresh), or one edit above the block shifts the
  protected region.

- **Search history** (M, atlas)
  Up/Down in the query field walks the last ~20 distinct queries, most recent
  first; the same for the replacement. Committing a search moves it to the
  front. The list survives closing the bar and reopening the project.
  Why: Alternating between two searches - the definition and its call sites -
  is the normal rhythm of reading code, and today each switch means retyping.
  The bar is thrown away on close, so there is nothing to retype from.
  Note: ListPopup already draws exactly this dropdown with
  Up/Down/Enter/click. Persist in EditorState, which already carries per-
  project view state.

- **Say how many were replaced, and show a dead query as dead** (S, atlas (count) + aurora (tint))
  Replace All reports "12 replaced" on the status bar. A query with no matches
  tints the query field's border in the warning colour as well as reading "no
  matches", clearing as soon as a keystroke matches.
  Why: replace_all returns the count and find_action throws it away, and after
  the sweep the readout flips to "no matches" - literally true and identical
  to a Replace All that did nothing. There is no way to tell a typo from a
  successful whole-file rewrite.
  Note: emit_text_input already picks the field border colour per state, so
  the tint is a colour choice, not a new DrawCommand.

- **Reveal a match with context, and never under the find bar** (M, aurora (a reveal taking a line margin and an optional obstruction rect))
  Jumping to a match leaves two or three lines of context above and below and
  never places it behind the bar's own rectangle. A match already comfortably
  on screen does not scroll the view at all.
  Why: update_vscroll does the minimum possible scroll, so a match found
  downwards lands flush against the bottom edge with no following context, and
  the bar is anchored over the top-right - so a match revealed upwards can
  land behind it and appear not to have moved.
  Note: The obstruction rect is knowable - the bar is a popup whose laid-out
  rect atlas has. Alternatively dodge it: move the bar to the bottom-right
  when the current match is in the top band.

- ~~**Give find matches their own colour, distinct from the selection**~~ DONE (S, aurora (two theme tokens read by FindBar::decorations))
  The selection keeps the blue accent; find matches take a separate hue - a
  faint amber wash for the others, stronger amber or an outline for the
  current one - so a revealed match and a hand-made selection never look
  alike.
  Why: theme.selection is rgba(0.30,0.55,0.90,0.40) and FindBar::decorations
  uses theme.focus, rgb(0.30,0.55,0.90), faded to 0.45 and 0.18 - the same hue
  at nearly the same alpha. And reveal calls select_range on the current
  match, so it is painted twice, a highlight and a selection of the same blue
  stacked, while the others are a washed-out version. Nothing separates "I
  selected this" from "the search found this".

- ~~**Highlight every occurrence of the word under the caret**~~ DONE (M, atlas (computed in apply_script_marks) + aurora (a distinct theme colour))
  After a short pause with the caret inside an identifier, every other whole-
  word occurrence in the file gets a dim highlight - `pos` must not light up
  inside `post` - with the occurrence under the caret unmarked or marked
  differently. Clears on any edit, on moving out of the word, and while a
  selection is active. ctrl+d jumps to the next one.
  Why: comet has no rename and no go-to-definition, so seeing where a local is
  used is a find-bar round trip that also steals the caret and the selection.
  It is also how you notice a typo'd second spelling.
  Note: word_before yields only the prefix, so the word under the caret needs
  the suffix side too. Must merge into apply_script_marks, ordered behind find
  matches and squiggles, since a second set_decorations owner erases the rest.
  Needs a debounce so it does not rescan on every keystroke; changing
  decorations provably does not re-shape.

- **Find in all project files** (L, atlas)
  ctrl+shift+f searches every .cmt with the same toggles and lists results
  grouped by file: path, line number, and the matching line with the match
  emphasised. Clicking a result opens that file and selects the range; Up/Down
  walk results without leaving the list; a header reads "37 in 6 files".
  Why: The editor opens one file at a time and the bar searches only the
  widget's buffer, so with behaviour spread over several scripts, finding
  which one calls a function means opening each and searching it.
  Note: explorer.all_scripts() already enumerates every .cmt with absolute and
  project-relative paths. Two things to design: where results live (a new
  Pane, or a takeover of the console), and that the open buffer may be dirty -
  search script_text for the open file or results will not match what the user
  sees. Clicking a result must guard the unsaved-edits case.

- **Replace across files** (L, atlas)
  Replace All in a project-wide search rewrites every listed match, writing
  unopened files straight to disk and routing the open file through the normal
  buffer path so it stays undoable. Reports "37 replaced in 6 files" and
  refuses to run silently on a dirty buffer.
  Why: The payoff of project-wide search: renaming a function three scripts
  call is otherwise six file-opens and six Replace Alls with no way to tell
  whether one was missed.
  Note: The existing replace path is Ui-bound, so unopened files need a
  headless String rewrite sharing find_all. Undo is the hard part: script_undo
  is one buffer's history, so this needs either a multi-file undo record or an
  explicit confirmation modal before it runs.

- **~~Go to line, and line:column~~** DONE (S, atlas)
  A shortcut opens a small field over the editor; `120` puts the caret at the
  first non-space character of line 120 and reveals it with context, `120:8`
  puts it at column 8, a number past the end clamps to the last line, Escape
  cancels and leaves the caret untouched.
  Why: Compiler messages, panics and the console all speak in line numbers,
  and there is no way to act on one except counting against the gutter.
  Note: aurora keeps line_starts / byte_to_cursor private, so either atlas
  duplicates the newline scan or aurora grows a small public line API - worth
  the latter since the Ln/Col readout and the diagnostic jump want it too.
  Style::mask(Mask::Digits) covers the plain form; `120:8` needs Mask::None.

- **Match ticks and problem marks beside the text** (M, aurora)
  A narrow strip down the right edge shows one small mark per match at its
  proportional position in the file, the current one brighter, plus marks for
  error and warning lines. Clicking a mark jumps to it. Empty and zero-width
  with nothing to show.
  Why: The count says "3 of 12" but not where the other eleven are, so judging
  whether a Replace All is safe means stepping through all twelve. It is also
  the only cheap way to see that a file's errors are all in one function.
  Note: Cheap under the vocabulary - each tick is one FillRect - but there is
  no surface to hang it on: the editor is a bare text area with no scrollbar,
  so the strip and its hit-testing are new. Fold into the scrollbar entry if
  that lands first; the app supplies line numbers, since aurora must not learn
  what a diagnostic is.

## Structure: folding, symbols and navigation

A script is one flat file with no map. Two small comet queries (symbols and
fold ranges) feed an outline, a picker, breadcrumbs, a sticky header and
function jumps; real folding - hiding lines - is a much larger change to how
text is projected.

- **comet::service::fold_ranges(source)** (S, comet)
  Returns every foldable region as (head_line, last_line, kind) where kind is
  Function, Block, StateRun (consecutive top-level lets) or Comment
  (consecutive // lines). Regions under two lines are not offered; regions
  nest and come back outermost-first in source order; head_line is the line
  the opening brace is on so folding leaves `func update() {` visible.
  Why: Every other folding entry needs one authoritative answer to what can
  fold here; without it each caller re-walks the AST and they drift.
  Note: Block.span is built as start.to(end), so a block whose closing brace
  is missing still spans to where the parser gave up - fold_ranges must decide
  whether to offer that. Line numbers are not in the AST, so it builds its own
  line index.

- ~~**comet::service::symbols(source)**~~ DONE (S, comet)
  One entry per top-level let and per func in source order: name, kind (State
  | Function), name_span (what a jump selects), full span (what a fold
  covers), and for a function its parameter names and types plus its return
  type, formatted for display. An unnamed item (the parser inserts an empty
  name on a typo) is skipped rather than listed blank.
  Why: The outline pane, go-to-symbol, breadcrumbs and sticky headers are four
  consumers of one list; four separate AST walks in atlas would each have
  their own idea of what counts as a symbol.
  Note: A projection of Script.state/Script.functions, which already carry
  name_span and span. TIR is the wrong source - locals there collapse to slot
  indices.

- **Fold state that outlives the shell rebuild** (S, atlas)
  atlas holds the folded regions for the open script keyed by head line,
  beside script_text. An edit above a fold shifts it by the newline delta; an
  edit inside a folded region unfolds it; opening a different script clears
  the set.
  Why: The whole Ui is discarded on every dirty frame, so state parked on the
  widget dies the first time the modified mark appears - the same reason
  script_undo, script_spans and script_diagnostics live on State.
  Note: Remapping is easy here because sync_script_buffer already sees the
  whole before/after text at the one point the buffer changes.

- **Fold arrows in the gutter, with their own click** (M, aurora (draws and hit-tests) + atlas (supplies marks, reacts))
  Ui::set_fold_marks(id, Vec<(line, FoldMark)>) where FoldMark is Open or
  Closed. emit_gutter draws the mark left of the number on that logical line's
  first visual row; Open draws only while the pointer is over the field,
  Closed always. A press in the mark's column emits Event::FoldToggled { id,
  line } and leaves the caret alone; the number column keeps emitting
  GutterClicked.
  Why: Folding with no visible affordance is a keyboard-only feature nobody
  discovers, and a folded region with no arrow is indistinguishable from a
  short function.
  Note: Three obstacles: DrawCommand has no paths, so the arrow is a glyph or
  an Image (atlas already rasterizes Icon::ChevronRight/Down for the scene
  tree, so aurora would accept two ImageHandles); gutter_width and
  content_origin are private, so an app cannot place its own widgets in the
  strip; and GutterClicked carries only a line, never a column, so an arrow
  zone and a number zone cannot be told apart today.

- **Fold commands: toggle at the caret, fold all, unfold all, fold to level N** (S, atlas)
  One binding folds the innermost region containing the caret and unfolds it
  when pressed again on the same caret. Fold-all folds every function body and
  leaves the caret on the head line of whichever fold swallowed it. Unfold-all
  clears the set. Fold-to-level-N folds every region N deep or deeper.
  Why: The gutter arrow handles one fold; "show me only the function
  signatures" is the reason folding exists and is a keyboard command
  everywhere it exists.
  Note: handle_shortcut reads the winit KeyEvent directly, so bracket and
  function keys are reachable without widening aurora's Key enum.

- **Folding by indentation, for the file that does not parse** (M, comet (beside fold_ranges, so callers get one merged answer))
  Where a region produced parse diagnostics, fold ranges come from indentation
  instead: a line whose next non-blank line is more indented opens a region
  ending at the last line indented deeper; blank lines do not close a region;
  indentation is measured on logical lines.
  Why: The editor re-parses on every keystroke and the file is malformed most
  of the time it is looked at. Fold arrows that blink out of existence mid-
  word are worse than no arrows.
  Note: Should treat the configured indent unit as the unit and decide a tab's
  width rather than assuming spaces.

- **Hide folded lines from the rendered text** (L, aurora - the doc-to-view mapping must live where the String, the shaped buffer and every byte-range API live)
  A closed region's interior lines occupy no rows; the caret cannot be placed
  inside them by click or arrow; Down from the head line lands after the
  region; a selection dragged across a closed fold takes the hidden text with
  it; Backspace on the head line deletes the region as a unit; and the gutter
  still shows the real numbers, so a fold jumps from 12 to 40.
  Why: This is the only part of folding that saves screen space. Everything
  else in this group is navigation around it.
  Note: The flat-String problem concretely: the widget's text IS the document
  and every editor API is a byte offset into it (set_text_spans,
  set_decorations, select_range, caret_offset, insert_str, range_rects, plus
  atlas's spans, diagnostics, find matches, undo snapshots and completions).
  Letting atlas project instead is worse - sync_script_buffer reads the
  widget's text back as the document and would delete every folded line from
  the file on the next keystroke. Also: shape_gutters builds the numbers as
  1..=lines and indexes by run.line_i, so a projected buffer renumbers the
  visible lines and needs Ui::set_gutter_numbers(id, Vec<usize>).
  caret_vertical comes out fold-correct for free once hidden lines are absent
  from the buffer.

- **A placeholder that says what a closed fold is hiding** (M, aurora)
  A closed region draws its head line normally and then, after its last glyph,
  a dimmed run reading `... 12 lines`. Clicking it unfolds; hovering shows the
  hidden text in a tooltip.
  Why: Without it a closed fold looks like a function whose body was deleted,
  and there is no target to click except the gutter arrow.
  Note: A per-line trailing run, which the pipeline has no concept of -
  emit_buffer_spans only emits glyphs that came out of the widget's own shaped
  String. Needs a small side buffer per placeholder, shaped the way
  shape_gutters shapes numbers. A Decoration cannot stand in:
  Highlight/Underline/Squiggle all draw over existing glyphs, never past them.

- **A jump into a folded region unfolds it first** (S, atlas (one reveal helper every navigation command routes through))
  Any reveal landing inside a closed fold - find match, go-to-line, go-to-
  definition, a diagnostic step, a stack frame - opens the enclosing folds
  before selecting, and leaves them open.
  Why: Otherwise select_range puts the caret somewhere invisible and the view
  appears not to have moved, which reads as a broken jump.
  Note: Two direct callers to fold into one already exist: FindBar::reveal and
  accept_completion's select_range.

- **Fold state persists across editor sessions** (S, atlas)
  Each recently-opened script's folded regions are written into the editor
  state file and restored on reopen, keyed by project-relative path. A script
  whose line count changed on disk drops its folds rather than restoring them
  onto the wrong lines.
  Why: Folding a 400-line script down to its signatures is real work; losing
  it every restart teaches people not to bother.
  Note: editor_state.rs already persists paths and encoded collapse sets, so
  this is one more field plus encode/decode.

- **An outline pane listing the script's symbols** (M, atlas)
  A dock pane showing the open script's state declarations and functions in
  source order with signatures; clicking one selects its name span and reveals
  it; the entry containing the caret is highlighted and follows the caret;
  empty with a hint when no script is open.
  Why: One flat 400-line file with no map is the state today - the only way to
  find `update` is to scroll or search for it.
  Note: Pane is a closed enum that the default layout, the tab titles and the
  serialized editor state all enumerate, so a new variant touches all three.
  Following the caret needs a caret move to trigger a refresh, which today it
  deliberately does not.

- ~~**Go to symbol (a filtered symbol palette)**~~ DONE (S, atlas)
  A shortcut opens a popup of the script's symbols; typing filters; Up/Down
  move; Enter or click selects that symbol's name span and reveals it; Escape
  closes and puts the caret back where it started.
  Why: The fastest way to move around a file, and nearly free: the popup, the
  filtering, the key handling and the reveal all exist for completions.
  Note: The genuinely new part is remembering the pre-jump caret so Escape can
  restore it - nothing does that today.

- **Breadcrumbs over the editor** (M, atlas)
  The Code pane header shows the path to the caret - file, then enclosing
  function, then the enclosing block (`bounce.cmt > update > while`). Each
  segment is clickable and selects that construct's span. The trail updates as
  the caret moves, not only as the text changes.
  Why: It answers "where am I" continuously, which is what makes a long file
  navigable, and it is the natural place to hang a symbol dropdown later.
  Note: The hard part is the refresh: sync_script_buffer deliberately does not
  dirty the shell per keystroke because rebuilding while someone types is how
  a caret gets lost. So breadcrumbs must update in place via set_label like
  the status readouts, i.e. a fixed set of segment widgets whose text changes,
  not a rebuilt row of varying length.

- **A sticky header pinning the enclosing function while scrolled** (M, atlas (decides the line, builds the strip as a popup) + aurora (must say which line is at the top))
  When the top visible line is inside a function whose func line has scrolled
  off, that declaration is drawn as a one-line strip across the top of the
  editor in the editor's own colours and syntax colouring, over the text.
  Clicking it scrolls to the declaration. It disappears the moment the real
  line is visible and never covers the caret's line.
  Why: A script's update body is the part people scroll through, and 40 lines
  in there is no way to tell which function you are in.
  Note: Style::hit_transparent exists for exactly the not-a-click-target part.
  The header's syntax colouring means re-slicing script_spans to its own byte
  range onto a separate label - spans are absolute offsets into the widget's
  text, so they cannot be reused unshifted.

- ~~**Jump to the previous or next function**~~ DONE (S, atlas)
  One shortcut moves the caret to the func line of the previous top-level
  function, another to the next; at the ends it stops rather than wrapping.
  The line is revealed with context above it, not pinned to the very top.
  Why: The commonest structural move in a script that is a handful of
  functions, and the one that needs no popup or pane to exist first.
  Note: Must be intercepted before the text area sees the keys - edit_key
  handles Up/Down itself for multiline fields and Key carries no modifiers.
  handle_code_key already claims arrows for the completion popup, which is the
  precedent.

- **Navigation history: jump back and jump forward** (M, atlas)
  Every non-local caret move - go-to-definition, go-to-symbol, go-to-line, a
  diagnostic step, a find match - pushes where it came from. Back returns
  there, forward re-does it. Typing and arrow keys do not push. Opening a
  different script pushes a boundary so back can return to the previous file.
  Why: Go-to-definition is only usable if you can get back; without it,
  following one call means finding your way home by hand, which is why people
  stop following calls.
  Note: One script is open at a time and open_script_file replaces the buffer
  wholesale, so a cross-file entry stores the path and re-opens. Keep it
  distinct from script_undo/redo, or ctrl+z would undo a jump.

## Language service depth

comet computes far more than the editor shows. The single biggest hole is that
a Diagnostic's message string is produced, cached, and rendered nowhere.
Beyond that: hover, signature help, rename, references, warnings, and quick
fixes - most of which read a TypedScript that already exists.

- **~~One analysis per edit instead of three whole-file pipeline runs~~** DONE (S, comet (the Analysis type) + atlas (hold one, invalidate on edit))
  A single Analysis value computed once per buffer revision carries the token
  stream, the AST, the TypedScript, the diagnostics and the highlight spans;
  diagnostics, highlight, completions, hover and signature help all read it.
  One keystroke lexes the file once and parses it once.
  Why: Today one keystroke runs three full passes: analyze_script calls
  highlight (one lex) and diagnostics (a second lex, a parse, a check), then
  refresh_completions calls completions_at (a third lex, a second parse).
  Every feature below adds another pass unless this lands first.
  Note: Deliberately not incremental reparsing - it is caching, changes no
  behaviour, and is a few dozen lines. It shrinks the number true incremental
  would have to beat by roughly 3x before that question is worth asking, and
  ORBIT_FRAME_LOG exists to do the measuring.

- **~~Show a diagnostic's message, not just its squiggle~~** DONE (M, atlas (dwell timer and popup) + aurora (a public byte-offset-at-point))
  Resting the pointer over a squiggle for about 500ms pops a small card near
  the cursor with the message, tinted by severity; moving off, typing or
  clicking dismisses it; overlapping diagnostics stack in one card in source
  order. Independently, when the caret is inside a diagnostic's span the
  status bar shows that message.
  Why: The single biggest hole in the editor. script_diagnostics is cached and
  turned into squiggles, and the message String is rendered nowhere in atlas -
  the field's own comment claims it is "for the status bar" and
  build_status_bar has five fixed readouts, none of them this. A red wiggle
  that will not say what is wrong is a puzzle, not a diagnostic.
  Note: The caret-inside-a-span half needs no new aurora API and is about 30
  lines. atlas has the whole pattern to copy in HOVER_DELAY / tooltip_target /
  update_file_tooltip / add_file_tooltip, including the freeze-while-the-
  button-is-held rule a rebuild-driven popup needs. caret_at_x already does
  the point-to-offset mapping but is private and returns text_len as its off-
  the-field answer, which a hover must not treat as real.

- **~~Error and warning counts, and stepping between diagnostics~~** DONE (S, atlas)
  The Code pane header shows `2 errors, 1 warning` in the error/warning
  colours, or `no problems`. F8 / shift+F8 (and clicking the count) move the
  caret to the next / previous diagnostic span and reveal it, wrapping at the
  ends and saying so; with none, both are no-ops rather than jumping to the
  top.
  Why: An error 400 lines above the viewport is completely invisible, and
  finding the second error in a file means scrolling looking for another
  squiggle.
  Note: script_diagnostics is already sorted by (span.start, span.end), so
  stepping is a partition_point.

- **A problems list** (S, atlas)
  A list - its own pane or a tab of the Console - shows every diagnostic
  across open documents as `12: cannot find spede in this scope`, sorted,
  staying live as you type; clicking a row opens that file at that span.
  Why: The counterpart to stepping, and the only view that shows how many
  problems there are at once.
  Note: Reuses the Console pane's list-of-lines shape and its per-level
  colouring; line numbers come from counting newlines to span.start.

- **Report a codegen failure, not just parse and check** (S, atlas (comet only if compile needs a diagnostics-returning variant))
  Saving (or an explicit build) runs a real compile and writes a codegen
  failure to the Console with its message.
  Why: comet::service::diagnostics runs parse and check only, and atlas never
  calls comet::compile at all - only helios does, at instantiation. A failure
  in the stage that decides whether the script can actually run has no path to
  the screen.

- **~~Hover shows a symbol's type~~** DONE (M, comet (service::hover(source, offset)) + atlas (the card))
  Hovering an identifier shows `dt: f32`, `pos: Vec2` (and says it is script
  state), or for a function `func clamp(value: f32, lo: f32, hi: f32) -> f32`.
  Punctuation, whitespace and literals show nothing; a name that did not
  resolve shows nothing rather than <error>.
  Why: The whole inference story is `let` taking its type from its
  initializer, so an unannotated local's type is invisible - and that is
  exactly the case where field completion also refuses to answer. Today the
  only way to learn a type is to provoke a type error.
  Note: Cheaper than it looks: check() returns a TypedScript whose every
  TypedExpr carries {ty, span}, so an expression hover is a smallest-
  enclosing-span walk. The gap is binders - TypedStmt::Let carries a slot and
  no span, and ast::Param has no name_span at all (parser.rs discards it), so
  hovering a parameter's declaration is blocked until that field is added.

- **Signature help while typing a call's arguments** (M, comet (which call, which argument index) + atlas (the popup))
  After `clamp(` a hint at the caret reads `func clamp(value: f32, lo: f32,
  hi: f32) -> f32` with the current parameter emphasised; it advances on each
  comma, survives nesting (`clamp(min(a, ` shows min's), and closes on the
  matching ), on Escape, and when the caret leaves the call. It shows for
  print too.
  Why: The checker reports wrong arity after the fact; while typing, nothing
  says what the next argument should be - in a language with mandatory
  parameter annotations, which has exactly that information.
  Note: A half-typed call does not parse into Expr::Call, so this scans the
  token stream back for the innermost unmatched ( and the identifier before
  it, then looks the name up in the checker's signature table, which check.rs
  builds up front. There are no overloads (by_name maps one name to one
  index), which removes the fiddliest part.

- **~~Completion detail, kind and documentation~~** DONE (M, aurora (ListPopup rows take a colour/tag) + comet (a detail string) + atlas (map kinds onto the code_* palette))
  Each row shows the label, a small kind-coloured marker on the left, and a
  dimmed right-aligned detail: `f32` for a local, `(f32, f32, f32) -> f32` for
  a function, `keyword`, `type`, `field`. The typed prefix is emphasised in
  the label. The highlighted row's doc comment shows in a panel beside or
  below. A list longer than the window shows that there is more above or
  below.
  Why: CompletionItem carries a kind and atlas discards it with .map(|item|
  item.label), and ListPopup emits one plain button per row - so the keyword
  `while` and a local named `w` look identical, even though the pane already
  has a distinct colour for each class.
  Note: ListPopup is Vec<String> throughout and restore() compares labels, so
  if rows become a struct the label must stay the identity or the highlight-
  survives-a-refilter behaviour breaks. Prefix emphasis can be a TextSpan on
  the caption and costs no re-shape; emphasis by weight is not expressible -
  see the per-span styling ceiling.

- **Completion sorting by relevance** (S, comet (rank) + aurora (respect a supplied rank))
  Locals and parameters first, then script state, then this script's
  functions, then builtins, then types, then keywords. Within a tier: exact
  prefix, then case-insensitive prefix, then substring, ties broken by nearest
  declaration. Typing `p` in a script with a `pitch` local offers pitch above
  print.
  Why: completions_at appends in one fixed order and ListPopup::refilter only
  splits prefix matches from contains matches, so relevance never enters into
  it.
  Note: refilter re-orders unconditionally today, so a supplied rank would be
  discarded; it must become a stable tie-break rather than the whole ordering.

- ~~**Suppress completions inside comments and string literals**~~ DONE (S, comet (the context test belongs with the lexer) + a one-line guard in atlas)
  Typing a word inside `// bounces along x` or inside "hello world" opens no
  popup. The same word outside them still completes.
  Why: refresh_completions asks only whether the prefix is non-empty and
  completions_at never looks at the token kind, so writing an English comment
  fires autocomplete on every word - the most-hit annoyance in the editor,
  since comments are how a teaching engine's example scripts are written.
  Note: highlight() already returns Comment and Str spans over the whole file,
  so the test is span containment over a list recomputed every keystroke
  anyway.

- **Field completions on an inferred receiver and on non-identifier receivers** (M, comet)
  `let here = pos; here.` offers x and y. So do `foo(pos.`, `clamp(a, b).`
  when it returns Vec2, and `(pos).`. An f32 or String receiver still offers
  nothing rather than guessing.
  Why: receiver_type reads only declared annotations - its doc admits an
  annotated binding completes and an inferred one does not - and
  field_receiver accepts only a bare identifier left of the dot. Since `let
  here = pos;` is how anyone would write it, the common case is the one that
  fails.
  Note: Read the checked TypedScript instead of the AST: the receiver's
  TypedExpr already carries its Type. `pos.` parses as `pos` plus a parse
  error, so the sub-expression survives; what is needed is a stable
  "expression whose span ends just before this offset" lookup, the same walk
  hover needs.

- **Completions ranked and hinted at an argument position** (S, comet)
  At `clamp(|` the popup ranks in-scope names whose type matches the expected
  parameter first and shows the expected type as its header. A bool local is
  not first where an f32 goes, and the keyword tier is dropped at an
  expression position.
  Why: An argument position is currently indistinguishable from a statement
  position - you are offered `while` and `String` where only an f32 expression
  can go.
  Note: Once signature help exists this is a re-sort of an already-computed
  list against one expected Type. Mostly free.

- **Rename a symbol across the file** (M, comet (resolve every occurrence) + atlas (drive the edit))
  F2 on a local, parameter, state name or function name opens a small inline
  field; confirming rewrites the declaration and every reference as one undo
  step; a name colliding with an existing name in the same scope is refused
  with a message rather than silently shadowing; Escape leaves the buffer
  untouched.
  Why: In a language where a missed site produces `cannot find X in this
  scope` at every one, renaming by hand is the classic reason a bad name
  stays.
  Note: Blocked on a declaration map - the checker resolves name to slot and
  keeps no span back-map - and on ast::Param gaining a name_span, without
  which parameters cannot be renamed. On the atlas side it is one select_range
  + insert_str per site applied back to front, and since script_undo snapshots
  the whole buffer it is naturally one step; clear the coalesces run flag or
  the next typed character folds into the rename.

- **Highlight every occurrence of the resolved symbol under the caret** (S, comet (resolution) + atlas (decorations))
  The resolution-based upgrade of the textual occurrence highlight: a shadowed
  inner binding tints only its own occurrences, and a local `x` does not tint
  a field `pos.x`.
  Why: It is rename's prerequisite and the cheapest use of the same
  resolution; in a language that allows shadowing it is the only way to see
  which binding you are looking at.
  Note: No new draw vocabulary - Highlight plus range_rects plus one more
  extend() on the vector apply_script_marks rebuilds each frame. The Field
  expr carries its own field_span, which is what makes the distinction
  possible.

- **Go to definition** (M, comet (definition_at(source, offset) -> Option<Span>) + atlas)
  ctrl+click and F12 on a name select and reveal what declares it: the
  enclosing function's parameter, the nearest let in an enclosing block
  declared above the caret, a top-level state declaration, or a function
  declaration for a call. A builtin like print or a type like Vec2 says so in
  the status bar rather than doing nothing; a name with no declaration says so
  rather than jumping somewhere plausible. ctrl+hover underlines the name to
  advertise the click.
  Why: Reading someone's update means constantly asking where a name came
  from, and today the answer is ctrl+f and hope.
  Note: The existing scope walk is not reusable as-is: visit_lets is
  documented as deliberately loose - it offers a local after its block has
  ended - which is a defensible bias for completions and a wrong answer for a
  jump, so definition_at needs real block scoping innermost-first. TIR cannot
  help; locals there are slot indices. The ctrl+hover underline is
  DecorationStyle::Underline, which exists and is used by nothing.

- **Find references** (M, comet (references_at) + atlas)
  A shortcut on a name lists every use, the declaration marked as such, each
  entry revealing its span; while the list is open every reference is drawn as
  a Highlight so they are visible in the text. Closing clears the highlights.
  Why: The other half of go-to-definition and what you want before renaming a
  state variable by hand.
  Note: Must be resolution-based, not text-matching. The highlights merge
  inside apply_script_marks - a second set_decorations caller erases the
  squiggles.

- ~~**Did-you-mean suggestions in name diagnostics**~~ DONE (S, comet)
  `cannot find spede in this scope` becomes `... - did you mean speed?`. Same
  at the three other sites: unknown function, unknown type (Vecc2 -> Vec2),
  and `Vec2 has no field z` -> x/y. Suggest only within edit distance 2 (1 for
  names under four characters), and nothing rather than something absurd.
  Why: The candidate set is already in hand at every site -
  scopes/locals/globals, by_name plus host_fn, Type::from_name's four names,
  axis() - and a name typo is exactly the error whose message tells you
  nothing you did not know.
  Note: About 25 lines of Levenshtein plus four call-site edits. No plumbing,
  and it is what makes the spelling quick fix possible.

- ~~**Unused-variable and unreachable-code warnings**~~ DONE (unreachable code not yet) (M, comet)
  A let (local, parameter or state) that is never read gets a warning squiggle
  on its name; a name starting with _ is exempt. A statement after a return in
  the same block gets "this code is never reached", spanning to the end of the
  block. Warnings never block compilation.
  Why: The whole warning path is built and dead: Severity::Warning exists and
  atlas already maps it to theme.code_warning as a squiggle, and comet has no
  Diagnostic::warning constructor and emits zero warnings - a colour in the
  theme document that paints nothing.
  Note: Usage is a walk of the TypedScript counting reads per local and global
  slot, which the checker already resolves. Unreachable code falls out of the
  shape of the existing always_returns(). One design call: script state may
  legitimately be write-only from the script's side, so warn on never-read AND
  never-written.

- **Quick fixes for common diagnostics** (M, comet (offer the edits) + atlas (menu and apply))
  With the caret on a diagnostic, a shortcut offers the fixes that apply:
  insert the missing ;, annotate a let with the type its initializer produced,
  add the missing return, replace a misspelled name with the suggestion,
  remove or underscore-prefix an unused let, drop an else after a branch that
  always returns. Choosing one edits as a single undo step and re-runs
  analysis.
  Why: The commonest diagnostics here are mechanical - `expected ';' after an
  expression statement` is what a half-typed line produces constantly - and
  each has one obvious repair.
  Note: The load-bearing part is a schema change: Diagnostic is {span,
  severity, message} with no code and no payload, so matching on message text
  would break the first time a message is reworded. Add code: DiagnosticCode
  (or an optional fix: Vec<TextEdit>) first - every construction site is
  Diagnostic::error, so it is a mechanical edit across lexer.rs, parser.rs and
  check.rs.

- **Semantic (type-aware) highlighting** (M, comet (a richer resolved TokenClass set) + atlas (more theme colours))
  An identifier is coloured by what it resolved to - parameter, local, script
  state, function, unresolved - rather than by its neighbours. `notAFunction(`
  is not coloured as a function; `x: Vecc2` is not coloured as a type; speed
  reads as state at every occurrence including its declaration. Where the file
  does not resolve, the current lexical classification is used unchanged.
  Why: highlight()'s doc is candid that names are told apart by their
  neighbours, which is right for half-typed states and wrong for finished
  code: a call to a function that does not exist looks exactly like a real
  call, the opposite of what the colour should say.
  Note: Genuinely cheap in aurora's model: spans are consulted when the draw
  list is built, never when text is shaped, so more classes are near-free at
  draw time. Keep the lexical pass as the fallback layer rather than replacing
  it.

- **Doc comments, and somewhere for them to surface** (M, comet + atlas (rendering))
  A run of /// lines immediately above a func or a top-level let attaches to
  it, and the text appears in hover, in signature help, and beside the
  highlighted completion item. Plain // stays an ordinary comment and attaches
  to nothing.
  Why: Nothing attaches a comment to a declaration. For an engine whose
  differentiator is teaching, a script that cannot document its own functions
  - and a print builtin whose completion cannot say what it does - is a real
  absence.
  Note: The parser consumes lex(), which strips comments, so either it moves
  to lex_with_comments and skips them explicitly (risking the error recovery
  block() and recover_to_top_level depend on) or a side table maps each
  declaration's start offset to the comment block above it, built from the
  token stream the highlighter already produces. The side table is smaller and
  cannot break recovery. The builtins themselves need no syntax, just static
  strings beside BUILTINS.

- **A code formatter** (L, comet (the printer belongs beside the parser) + atlas (the command))
  A shortcut (and an opt-in format-on-save) rewrites the buffer to canonical
  style: one statement per line, one indent unit per brace level, single
  spaces around binary operators, none before ; or , and one after, blank-line
  runs collapsed to one, trailing whitespace gone, one final newline, braces
  on the header's line. Comments stay on the line they were attached to. The
  caret stays on the token it was on. A file that does not lex cleanly is left
  completely untouched and says so.
  Why: Tab-indent is the only formatting help that exists, so every script's
  shape is hand-maintained, and nothing can tidy an already-messy file. It is
  also what makes a house style enforceable.
  Note: Pretty-printing from the AST would delete every comment - ast.rs has
  no comment nodes and lex() strips them before the parser sees them - and
  would drop whatever recovery could not represent. The honest v1 is a token-
  stream reformatter over lex_with_comments: keeps comments, needs no tree,
  survives a file that does not parse. The same token pass gives
  comment/uncomment-the-selection almost for free.

- **Snippets and templates** (M, comet (what to insert) + atlas (tabstop state))
  Accepting `func` from the completion list inserts `func name(dt: f32) {\n
  \n}` with `name` selected; Tab jumps to the body then out. Same for if,
  while, and a full update skeleton offered in an empty file. Escape abandons
  the tabstops and leaves the text as ordinary text.
  Why: Every script in the repo starts with the same func update(dt: f32)
  typed by hand, and a first-time user staring at an empty .cmt has nothing
  telling them the shape of a script.
  Note: CompletionItem has a label only - no insert_text, no placeholder
  positions - and accept_completion does one select_range + insert_str, so
  both ends need extending. The multi-line insert alone is about 80 percent of
  the value; without tabstops it is still a working template with the caret at
  the end.

- **Inlay hints for inferred types** (L, aurora (the hard part) + comet (the text))
  `let step = speed * dt;` draws a dimmed `: f32` after step that is not part
  of the buffer: selecting the line does not select it, ctrl+c does not copy
  it, the caret steps over it, find never matches it. Toggleable, and absent
  where the type is written.
  Why: Local inference is the entire inference story, so an unannotated let is
  where the type is least visible - the same binding whose fields completion
  currently refuses to offer.
  Note: The one item aurora's design actively fights: spans only recolour
  glyphs that exist and decorations only draw over ranges; nothing inserts
  glyphs the buffer does not contain. Doing it properly means shaping a buffer
  that differs from the widget's String and mapping every byte offset in atlas
  across the difference. The cheap alternative worth taking instead: show the
  caret line's inferred type in the status bar, or in the gutter.

## The view: wrap, scrollbar, rulers, fonts

What the pane looks like and how you move around it. Wrap-versus-scroll is the
load-bearing decision here - several other entries in this backlog reason
differently depending on which way it goes.

- **~~Turn word wrap off and scroll code horizontally~~** DONE (L, aurora (the flag, the set_wrap call, generalizing hscroll to multiline) + atlas (the toggle in build_code_pane))
  Style::no_wrap() on a text area sets cosmic-text's Wrap::None so a long line
  stays one visual row and runs past the right edge; the field scrolls
  horizontally to keep the primary caret in view, shift+wheel pans, and the
  gutter stays pinned at the left while the text slides under it. The current-
  line band spans the visible width, not the content width. A per-editor
  toggle chooses.
  Why: measure_widget hands a wrap width to any multiline field and hscroll_of
  hard-returns 0 for multiline, so a 120-column comet line always wraps mid-
  expression (at WordOrGlyph, so the break lands inside an expression). The
  gutter numbers stop matching rows, indentation structure is destroyed on
  continuation rows, and there is no way to see a long line as one line. A
  column selection across wrapped lines is also meaningless, so column editing
  on a wrapping editor is dishonest.
  Note: The horizontal machinery is written and merely disabled for multiline.
  text_area_extent computes content height from layout_runs().count(), which
  stops matching logical lines once wrap is off, and the shape cache's
  single_line_w/wrap_bits reasoning needs a third case. emit_gutter takes the
  already-scrolled origin and would slide the numbers off the left edge, so it
  needs the unscrolled base. caret_at_x and range_rects follow for free.

- **Wrapped lines hang at their own indentation, and continuation rows are marked** (L, aurora)
  While wrap is on, a continuation row begins at the logical line's
  indentation (optionally plus one unit) rather than column 0, and carries a
  faint marker in the gutter strip so it is never mistaken for a new
  statement.
  Why: A long expression inside a nested block currently continues at the far-
  left margin, level with top-level declarations, so the block structure
  indentation exists to convey is broken exactly on the hardest lines to read
  - and a blank gutter cell at column 0 is what an unindented new line looks
  like.
  Note: cosmic-text 0.19 has no hanging indent or per-line left inset, so this
  is not a parameter to set. Either give the pane horizontal scrolling instead
  (the honest option, and the entry above) or offset continuation rows at emit
  time, which fixes the drawing and leaves hit-testing, caret_at_x,
  range_rects and caret_rect on unoffset coordinates. emit_gutter already
  detects a continuation (last_line == run.line_i), so the marker half is
  cheap and can ship alone.

- **~~A scrollbar on the code editor~~** DONE (M, aurora)
  A thumb on the right edge sized to the visible fraction and positioned by
  the scrolled fraction; dragging scrolls, clicking the track pages toward the
  click, and it appears only while the content overflows - the same behaviour
  scroll panels already have.
  Why: scrollbar_metrics returns None unless widget.scroll is set, and a text
  area scrolls through the separate text_vscroll path, so it never gets one.
  There is no indication that a file continues below the pane, no sense of how
  long it is or where you are, and no way to drag through it.
  Note: The drawing and dragging exist for containers (scrollbar_thumb_at,
  scrollbar_track_at, scroll_to_thumb_top); the work is making them read
  text_area_extent and text_vscroll instead of scroll_content. Nothing new is
  needed from the draw vocabulary.

- **~~Diagnostic and search marks on the scrollbar track~~** DONE (M, aurora (an overview-marks API drawn in the track) + atlas (feeds it from apply_script_marks))
  A 2px mark per diagnostic at its line's fraction of the document in the
  error or warning colour, and a fainter mark per find match. Clicking a mark
  scrolls to it.
  Why: An error twenty lines below the viewport is unreachable except by
  scrolling and watching for red. The overview strip is the standard answer
  and the data is computed on every edit.
  Note: The app supplies line numbers; aurora must not learn what a diagnostic
  is.

- **Diagnostic marks in the gutter** (M, aurora (Ui::set_gutter_marks(id, Vec<(line, Color)>) consumed by emit_gutter) + atlas)
  A line carrying an error or warning gets a small coloured mark in the gutter
  beside its number and its number tinted to match; clicking the mark selects
  that diagnostic's span.
  Why: The gutter draws only numbers, so a diagnostic's presence rests on a
  squiggle that is easy to miss under a coloured token or off to the right of
  a long line.
  Note: emit_gutter already knows each run's line_i, line_top and line_height,
  so a mark is one FillRect in a strip it owns. This and breakpoints are what
  Event::GutterClicked was built for.

- **A column ruler at 80 and 100** (S, aurora (Style::rulers(&[80, 100]) plus one FillRect each))
  A 1px vertical line at configurable columns drawn behind the text across the
  field's height in a colour barely above the background, following the
  horizontal scroll when wrap is off.
  Why: comet has no formatter and the pane has no other width cue, so lines
  grow until they wrap and the wrap is the first sign anything is wrong.
  Note: Only solid rects exist - no dashed or dotted line is expressible - so
  it is a faint continuous line. Needs the mono advance width.

- **A minimap beside the code** (L, atlas rasterizes the strip into RGBA and pushes it via register_image_rgba / update_image_rgba as an Image widget; aurora supplies the scroll setter a click needs)
  A narrow strip on the right showing the whole file at one or two pixel rows
  per line, coloured from the same syntax spans the text uses, with a
  translucent rectangle over the visible range. Clicking or dragging scrolls
  there; diagnostic and find-match rows show as coloured ticks.
  Why: With no scrollbar and no overview, a 400-line script gives no sense of
  where you are, how long it is, or where the dense parts sit.
  Note: Thousands of tiny Text or FillRect commands is not viable against this
  command set; the CPU-rasterized-image route is exactly what the colour
  picker gradients and file thumbnails already do, and update_image_rgba means
  no re-registration per edit.

- **Code font size and line height as real settings** (M, aurora (a line-height factor travelling with the widget the way font_size does) + atlas (the setting))
  The Code pane's font size comes from a setting (roughly 11 to 24px) with a
  separate line-height multiplier (1.0 to 1.8) applying to the code face only.
  Both take effect live, and the gutter, current-line band, caret, squiggles,
  scrolling and click-to-caret all follow because they read the same metrics.
  Why: The pane draws at the 15px UI default with line height hard-wired to
  font_size * 20/15 in text::metrics_for; build_code_pane sets .monospace()
  and no font_size, and Settings carries only theme/grid/axes/snap. Code
  cannot be made bigger or looser except by editing the source.
  Note: Everything already reading metrics_for(font_size).line_height -
  caret_rect, text_area_extent, update_vscroll, emit_gutter - picks it up for
  free. ShapeCacheEntry keys on font_bits only, so the factor must join that
  key or a size change silently reuses a stale shape.

- **~~Expose the code viewport and the mono cell metrics~~** DONE (S, aurora)
  A small public surface on Ui for a text area: get and set its vertical
  scroll, its content height and visible height, the range of visual rows on
  screen, first_visible_line, the byte offset under an arbitrary point, and
  the advance width of one character in the widget's face and size.
  Why: None of it is reachable - text_vscroll, text_area_extent, vscroll_of
  and caret_at_x are private and there is no advance-width accessor anywhere -
  yet the ruler, indent guides, whitespace markers, minimap, scrollbar,
  overview marks, hover tooltip, sticky header, scroll restoration and draw
  culling each need one or more. Without it every one of those reimplements a
  buffer walk aurora already does, app-side and worse.
  Note: scroll_offset / set_scroll_offset / max_scroll_offset exist for
  containers and read a different map, so the naming must make the two
  obviously distinct rather than overloading them.

- **Decide the per-span styling ceiling** (L, aurora)
  Either write down that a TextSpan will only ever carry a Color - so italic
  comments, bold keywords and a bold matched completion prefix are permanently
  out - or add a per-face second shaped buffer whose runs merge with the main
  one at draw time, so a span can pick a face without re-shaping on every
  recolour.
  Why: TextSpan carries only range and color, and its doc explains this is
  deliberate: cosmic-text can carry attributes per glyph but that would re-
  shape whenever a colour changed, which is what an editor does constantly.
  The constraint is real but implicit, and three features in this backlog
  quietly assume it away. There is also no italic face bundled at all - only
  DejaVu Sans Regular/Bold/ExtraLight and Mono Regular/Bold.
  Note: DrawCommand::Text is one run in one colour, so the draw list imposes
  no limit; the cost is entirely in shaping. Two buffers plus a run merge is
  the standard way out, at the price of a second shape cache and interleaving
  glyph order correctly.

## Documents, disk and text plumbing

One file at a time, no guard on unsaved work, no notion that the file might
change underneath you, and no IME. This group is mostly about not losing
things.

- **Hold more than one open document** (L, atlas)
  A list of documents, each carrying path, buffer, modified flag, undo/redo
  stacks, caret, selection, scroll offset, cached spans and cached
  diagnostics. Switching the active document swaps all of it atomically;
  closing drops only its state; opening a file already open activates it
  rather than re-reading it. With zero documents the pane shows the current
  empty state.
  Why: open_script/script_text/script_modified/script_undo/script_redo/script_
  caret/script_coalescing are seven parallel singletons on State, so opening a
  second file silently overwrites the first, undo history included. You cannot
  look at the script you are calling from while writing the one you are
  calling.
  Note: sync_script_buffer, analyze_script, apply_script_marks,
  refresh_completions, script_history, save_script and toggle_find all read
  those fields directly - wide but mechanical. script_undo is deliberately in
  atlas rather than aurora; per-document undo keeps that property.

- **A tab strip inside the Code pane** (M, atlas)
  One tab per open document showing its file name, a dot when modified, a
  close affordance on hover and on middle-click. Clicking activates; dragging
  along the strip reorders; ctrl+tab cycles; closing the last returns the pane
  to the empty state.
  Why: Without it the multi-document state has no way to be driven. The dock
  tab is captioned by Pane::title, a &'static str that always reads "Code", so
  with the pane behind another tab nothing on screen names the file or shows
  it is unsaved.
  Note: aurora::TabBar already owns tab drawing, along-the-bar dragging and
  the settle animation - reuse, not new machinery. Two files with the same
  base name need disambiguation by parent directory; Style::ellipsis handles
  overflow. Pane::title should become a per-pane dynamic caption so the dock
  tab can read "player.cmt *".

- **Reopen the last files on launch** (S, atlas)
  The open documents and which was active are written into the per-project
  editor state and restored next launch: files re-read from disk, the active
  one shown, caret and scroll restored, and a path that no longer resolves
  dropped with a log line rather than an error dialog.
  Why: EditorState persists the dock, camera, tool, pane scrolls, collapsed
  nodes and explorer selection but has no field for the open script, so every
  launch starts on "No script open" even after a session spent in one file.
  Note: EditorState is #[serde(default)] so an older editor.ron loads
  unchanged. Restore after the explorer scan so project_relative resolves, and
  without marking the buffer modified.

- **Notice the open file changed on disk** (M, atlas)
  Stat the open file's mtime and length a few times a second. Changed and the
  buffer clean: reload silently, keeping caret and scroll. Changed and the
  buffer dirty: a bar above the editor says so and offers Reload (discard my
  edits) or Keep mine. Saving over a file whose mtime moved since it was
  opened warns first.
  Why: Nothing re-reads the file after open. Edit the script elsewhere, or let
  git switch branches under you, and the pane shows stale text - then ctrl+s
  writes that stale text back over the newer file with no warning.
  Note: poll_settings is the exact precedent - it compares a stored SystemTime
  a few times a second and is cheap. The reload-when-clean path must not push
  an undo step. The banner is a strip in build_code_pane, not a modal - a
  modal blocks the whole shell for something the user may want to ignore.

- **Auto-save, and recover unsaved buffers after a crash** (M, atlas)
  An optional setting writes each dirty document to its file a few seconds
  after the last keystroke and on focus loss, off by default. Independently
  and always on, dirty buffers mirror to <project>/.orbit/drafts/ on the same
  debounce; on the next launch a draft newer than its file offers "restore
  unsaved changes".
  Why: The only path to disk is an explicit ctrl+s. A panic, a GPU device loss
  or a kill loses every edit since the last manual save, and exiting() flushes
  editor state but not the buffer.
  Note: maybe_save_editor_state is the debounce pattern to copy. Auto-save
  must not fire while the on-disk check says the file moved, or a conflict
  becomes a silent overwrite. Drafts need a stable name derived from the
  project-relative path, cleared on a successful save.

- **A read-only mode for the text widget** (M, aurora (flag and guards) + atlas (writability and caption))
  A text input can be marked read-only: it still takes focus, shows a caret,
  hit-tests clicks, allows drag and shift+arrow selection, ctrl+A and ctrl+C,
  and scrolls - but every mutating path is refused, including insert_str from
  the app. It draws in normal colours, not dimmed. atlas turns it on when the
  file is not writable and says so in the header.
  Why: Style::disabled() fades the whole subtree and makes it inert to all
  input - you cannot even select text to copy - so the only two states an
  atlas editor has are fully editable and unusable. Opening a file from a
  read-only checkout has no honest representation; you edit freely and
  discover the truth when ctrl+s fails.
  Note: Guards land in edit_key, insert_char, insert_str, insert_newline and
  delete_selection; edit_key mixes movement and mutation in one match, so the
  guard is per-arm rather than an early return. This also unlocks opening any
  text file (a .ron scene, a README) read-only, which click_file_entry
  currently refuses.

- **Create a new .cmt file from the editor** (S, atlas)
  "New Script" in the explorer's context menu and in the Code pane's empty
  state creates a uniquely-named .cmt in the current folder seeded with a stub
  - the functions comet expects, with a comment - selects it, starts an inline
  rename on the name portion, and opens it.
  Why: MenuAction has NewFolder and nothing else creational, and file_ops has
  create_folder but no create_file, so the only way to get a script into a
  project is to make the file outside the editor. The editor can attach a
  script to a node but cannot produce one - a strange asymmetry to hit in the
  first session.
  Note: unique_path already inserts a suffix before the extension, and
  start_file_rename / focus_file_rename exist. The stub's content should come
  from comet, not a string literal in atlas, so it cannot drift from what the
  checker requires.

- **Two editors side by side** (M, atlas (dock.rs's Pane identity and completeness invariant, plus the pane builder))
  The Code pane can be split, or a second Code pane opened, so two documents -
  or two views of one document with independent carets and scroll but a shared
  buffer - are visible at once. Dragging an editor tab out creates the second
  pane through the existing dock drop zones.
  Why: DockNode can split anything, but Pane is a fixed six-variant enum with
  Pane::ALL and an is_complete() demanding exactly one of each, so a layout
  with two Code panes is rejected as malformed and replaced by the default.
  The docking system that would make this trivial actively forbids it.
  Comparing a caller and a callee, or the top and bottom of one long file, is
  not possible.
  Note: The clean shape is Pane::Code(usize) keyed by an editor-group id, with
  is_complete relaxed to "every non-editor pane exactly once, editor panes any
  number including zero". That invariant protects a stale editor.ron from
  losing a pane, so weaken it carefully. Two views of one buffer also needs
  per-view state split out of the document, which is a good idea anyway.

- **IME and dead-key input** (L, aurora (events and pre-edit rendering) + atlas (the winit wiring))
  A dead key (' then e) or a compose sequence inserts one character. A CJK
  input method shows its candidate window at the caret, pre-edit text appears
  underlined at the caret without entering the buffer or the undo history, and
  committing inserts the final string as one undo step. Escape during pre-edit
  cancels.
  Why: window_event never matches WindowEvent::Ime and nothing calls
  window.set_ime_allowed(true), so winit never delivers IME events at all and
  translate_key forwards only event.text. On Linux and Windows a dead-key
  sequence inserts nothing or the wrong thing, and any input method beyond a
  plain Latin keyboard cannot type into the editor. That is every user whose
  language needs accents.
  Note: Adds InputEvent::ImePreedit { text, cursor_range } and
  ImeCommit(String) rather than Key variants, keeping the Key enum small and
  windowing-agnostic. The pre-edit string must be shaped and drawn without
  living in WidgetKind::TextInput, or every candidate keystroke would push an
  undo step and re-run the whole comet pipeline. Ui::caret_rect is already
  public precisely for anchoring at the caret, which is what
  set_ime_cursor_area needs.

- **Font fallback for characters the bundled faces cannot draw** (M, aurora)
  A character with no glyph in the bundled faces falls back to another bundled
  or system face rather than drawing a box, and where no fallback exists the
  editor says the file contains characters it cannot display rather than
  silently showing boxes you can select and edit.
  Why: make_font_system loads five DejaVu files into a fresh fontdb and
  nothing else, deliberately (determinism, no system scan). Latin and Cyrillic
  are covered; CJK, Hebrew, Arabic, Devanagari and emoji are not - so a
  comment or string literal in Japanese is a row of identical boxes with a
  working caret. It also makes the IME entry in this same group unbuildable:
  composing CJK into a buffer that cannot render CJK is a feature that cannot
  work, and the two entries have to be sequenced together.

## Performance and scale

Nothing here is felt at 200 lines and all of it is felt at 5,000. Worth
measuring with the existing ORBIT_FRAME_LOG profiler and
Ui::last_measure_count before deciding how far to take any of it.

- **Draw only the visible lines** (M, aurora)
  A scrolled text area emits glyphs, gutter numbers and decoration rects only
  for the visual rows intersecting its rectangle, plus a line of overscan each
  way. Scrolling a 10k-line file produces the same number of draw commands as
  scrolling a 30-line one.
  Why: emit_buffer_spans walks every layout run with no visibility test and
  pushes every glyph into DrawCommand::Text; emit_gutter walks every run;
  range_rects walks every run again per decoration. At 10k lines of ~40
  columns that is ~400k Glyph structs, each carrying a CacheKey, allocated and
  copied every frame for maybe 40 visible rows, then clipped away on the GPU.
  Note: The clip rect is known at the call site, so the bound is run.line_top
  + run.line_height < top || run.line_top > bottom in the widget's scrolled
  space. range_rects is shared by selection and decorations, so culling it
  once fixes both. measure_widget calls shape_until_scroll(false), so culling
  the draw is the cheap half and lazy shaping is the expensive half. Pair with
  a size guard at open time - a multi-megabyte file should be refused or
  opened read-only rather than freezing while cosmic-text shapes it.

- **Stop re-shaping the whole file on every shell rebuild** (L, aurora)
  The Code pane's shaped buffer survives a rebuild, so a rebuild triggered by
  something unrelated to the editor costs a widget-tree rebuild but no re-
  shape. Editing still re-shapes, once.
  Why: A rebuild constructs a brand-new Ui, so shape_cache and buffers start
  empty and measure_widget shapes the entire text from scratch. Anything that
  sets dirty does this: a console line, a thumbnail decoding, selecting a
  node, a tooltip appearing - and save_script itself sets dirty. At 10k lines
  that is a full shape of the document on an unrelated event, a visible multi-
  frame stall.
  Note: This touches the load-bearing "a rebuild swaps the whole Ui"
  assumption that has already bitten three times for retained interaction
  state. The cheap version is a content+face-keyed shape cache outliving the
  Ui swap, since ShapeCacheEntry already keys on exactly that; the honest
  version is to stop rebuilding the shell for reasons the code pane does not
  care about.

- **Cache the gutter's shaped line numbers** (S, aurora)
  shape_gutters re-shapes a field's line-number buffer only when the line
  count, font size or face changed. An unchanged file costs a field compare
  per gutter field per layout.
  Why: layout() calls shape_gutters unconditionally at its top, layout runs
  every frame from draw, and shape_gutters rebuilds "1\n2\n...\n10000" with a
  fresh allocation per line and re-shapes all of it - about 48KB built and
  shaped 60 times a second while nothing changes. It also feeds gutter_width,
  which measure_widget reads, so it cannot simply move later.
  Note: Key on (line_count, font_bits, face) - the numbers derive from the
  count alone, so the text never needs comparing. Its own small map, since
  gutter buffers live in gutter_numbers rather than buffers.

- **Bound the per-keystroke analysis work** (M, atlas (debounce and clone); comet only if measurement names the pipeline itself)
  Typing one character does not re-lex, re-parse and re-check the whole
  document on that frame. Analysis runs on a short debounce (or off the frame
  thread) with the last result on screen meanwhile, and the span vector
  reaches the Ui without a deep clone per frame.
  Why: sync_script_buffer clones the whole buffer then calls analyze_script,
  which runs highlight and diagnostics (parse plus check) over the whole text;
  apply_script_marks then clones the whole span Vec onto the Ui every frame.
  comet's own module doc calls the whole-text pipeline a deliberate v1 bet for
  a few-hundred-line script and invites re-measuring.
  Note: set_text_spans taking an Arc<[TextSpan]> kills the per-frame clone
  without touching the analysis. Careful: debouncing must not let squiggles
  lag far enough behind that their byte ranges point into a buffer that has
  moved on.

- **An undo history that is not a copy of the file per step** (M, atlas)
  The document's history stores edits (a range plus the replaced text) rather
  than whole-buffer snapshots, so memory and per-keystroke cost are
  proportional to what was typed. Undo still restores caret and selection.
  Why: script_undo is Vec<(String, usize)> capped at MAX_TEXT_UNDO = 200, and
  sync_script_buffer clones the entire buffer to make each snapshot after
  already comparing the whole widget text against the whole stored text on
  every frame. On a 5k-line script that is a full-file compare per frame, two
  full-file allocations per keystroke, and up to 200 resident copies of the
  file. The backlog treats undo only as a granularity problem.

- **A size and content guard at open time** (S, atlas)
  A file over a threshold opens read-only with syntax colouring and analysis
  off, and says why; a file that is not valid UTF-8 or that looks binary is
  refused with a sentence; a file with one pathologically long line is not
  shaped as a single run.
  Why: open_script_file is read_to_string straight into the widget, which
  shapes the whole buffer synchronously in the next layout with no ceiling -
  the failure mode is a frozen window, not a slow one, and it is reachable by
  double-clicking the wrong file in the explorer. The backlog carries this
  only as a parenthetical note under draw culling.

- ~~**Bound the parser's recursion**~~ DONE (S, comet)
  Parsing input nested past a fixed depth stops with a diagnostic ("expression
  nested too deeply") instead of recursing further, and the editor keeps
  working.
  Why: parser.rs is hand-written recursive descent - expr -> unary -> primary
  -> LParen -> expr - with no depth counter, and it runs on every keystroke
  over whatever the buffer currently holds, including a paste. A stack
  overflow inside the analysis pass aborts the process and takes the unsaved
  buffer with it. Every other crash class in this backlog loses a selection;
  this one loses the file.

## The comet language itself

Editor features can only be as deep as the language they describe. Several of
these are why completions and hover have little to be deep about, and one of
them (math builtins) is the highest value-per-line item anywhere in this
backlog.

- **Math builtins and a modulo operator** (S, comet (Host enum, host_signature, codegen imports) and helios (bind_host) for the ones wasm lacks)
  sqrt, abs, min, max, floor, sin, cos and a % operator available to any
  script, appearing in completions with their signatures.
  Why: BUILTINS is literally &["print"] and Host has one variant; codegen's
  import list is get_position_x, get_position_y, set_position, print. The
  completion popup's "functions the engine provides" tier is one item, the
  clamp fixture hand-writes clamping in eight lines, and nothing in the
  language can compute a distance or wrap an angle. lex_punct has no %, so
  modulo does not exist either.
  Note: sqrt/abs/floor/min/max are single wasm f32 instructions needing no
  host import - a new Intrinsic lowering straight to an opcode. Only sin/cos
  need bind_host. This makes every other completion feature look better
  immediately.

- ~~**Strings can be built, not only printed as literals** (M, comet (check + codegen))~~ DONE
  `print("x is " + pos.x)` works: + on String concatenates, and an f32 becomes
  a String (an interpolation form, or a str(x) builtin). Two Strings can be
  compared.
  Why: check::binary requires both Add operands to be F32 and host_signature
  says print takes exactly (Str) -> Unit, so the only thing a script can print
  is a string constant. The engine's single debugging tool cannot report a
  single value, which makes the Console nearly pointless for scripts and
  String a type you can declare and never usefully build.
  Note: ADR 0007 already put refcounting and a linear-memory allocator in
  place for String, so concatenation is a host call or an emitted helper
  rather than new infrastructure. Number formatting is the fiddlier half and
  is probably a host import next to print.
  Done: `+` on two Strings lowers to BinaryOp::ConcatStr, which allocates
  through comet_alloc and memory.copy's both operands in - the first thing a
  script can write that reaches the allocator at all. `str(f32)` is a host
  import, as the note guessed, and it calls back into the module to allocate
  so what it returns is an ordinary refcounted block. Joining a String to a
  number is still an error, but one that names the fix. Comparison of two
  Strings is not done.

- **Vec2 can be constructed and mutated** (M, comet (check + codegen))
  `let target = Vec2(100.0, 0.0);` builds one and `target.x = 5.0;` assigns
  into a local Vec2.
  Why: Both halves are missing and they compound. There is no Vec2 literal and
  no constructor - Vec2 resolves through Type::from_name for annotations, but
  a call hits `cannot find function Vec2` - so `pos` is the only Vec2 value a
  program can have. And check::place refuses field assignment on anything but
  pos. Meanwhile Vec2 is offered by completions as a type and is legal as a ->
  Vec2 return that only `return pos;` can satisfy.
  Note: A constructor is small - a host-shaped builtin where print lives.
  Field assignment on a local is the real work: tir::Place has
  Local/Global/Pos/PosField and no LocalField, so codegen needs a read-modify-
  write it does not emit. Do them together; a Vec2 you can make but not mutate
  is barely better.

- **Arrays and user-defined structs** (L, comet, all layers)
  A script can declare `let enemies: [f32]` or a `struct Enemy { pos: Vec2,
  hp: f32 }` and index or field it.
  Why: Total: the lexer has no [ ] and no struct keyword, tir::Type is a
  closed six-variant enum, and codegen has exactly one heap type. Without a
  collection there is nothing for a for loop to iterate and no way to write a
  game object with more than two floats of state, which caps what any example
  script can teach.
  Note: The M4 plan defers this to a fast-follow once the single-refcounted-
  type case is proven on String, and that sequencing looks right. Flagged
  because until it lands, completions and hover have little to be deep about.

- ~~**A for loop** (M, comet)~~ DONE
  `for i in 0.0..10.0 { }` (or over a collection, once one exists) parses,
  checks and lowers to the same wasm loop while already emits.
  Why: while is the only loop, so every iteration is a hand-written counter
  with a manual increment - exactly the beginner mistake (forget the
  increment, infinite loop inside a per-frame update) an educational engine
  should not force.
  Note: A range-based for desugars to the existing While in the AST-to-TIR
  step, so codegen needs nothing. The cost is a design call: no integer type
  means an f32 loop counter, which is a real semantics decision about
  fencepost behaviour, not a syntax one.
  Done: desugars in the checker, as the note guessed - codegen never learns
  that `for` exists, and the loop provably behaves as the `while` a reader
  would have written. Half-open, f32 counter, upper bound hoisted into a local
  so it is evaluated once. The counter and the bound live in a scope of their
  own, so the variable is gone after the loop.

- **Block comments in the lexer** was done with the maths builtins.

- ~~**Block comments in the lexer** (S, comet)~~ DONE
  /* */ lexes as a comment, is classified as one by highlight, and is skipped
  by the parser and the checker. Nesting either works or is explicitly
  refused.
  Why: The lexer recognises // only, so /* lexes as divide-then-star: the
  checker reports errors right through the commented-out region and the
  highlighter colours it as code. It also blocks the block-comment toggle
  command.

- **Multi-file scripts and import resolution** (L, comet + atlas + helios)
  A script names another file's functions after an import; completions, hover
  and go-to-definition follow across files; a diagnostic in a file that is not
  open is still reported; renaming a function updates its callers elsewhere.
  Why: Nothing exists at any layer: no import token, ast::Script is state plus
  functions with no imports, completions_at's doc states resolution is single-
  file, atlas holds one open_script, and helios::instantiate_file compiles one
  file to one module.
  Note: A new subsystem rather than a feature: a module graph and cycle
  detection in comet, cross-module linking in codegen (every script function
  is exported by name into a single module, which is why is_reserved_name
  exists), a workspace-wide analysis cache, and a tab set in atlas. The M4
  plan defers it explicitly and that call looks right - but the editor will
  feel single-file until it lands.

## Running the script you are editing

- **Run the open script and watch it move the node** (M, atlas (+ a one-shot entry point in helios if stepping needs one))
  A Run control in the Code pane (and a shortcut) compiles the buffer - not
  the file on disk - instantiates it on a node whose Script component points
  at this file, and either steps it one frame or runs it until Stop, with the
  viewport showing the node move. A compile failure lists its diagnostics and
  refuses to start; a node with no script, or a script no node references,
  says which is missing rather than doing nothing.
  Why: helios::ScriptHost, instantiate, instantiate_file and
  ScriptInstance::update exist, are tested, and are called from nowhere in
  atlas - grep the workspace for ScriptHost and the only hits are in helios
  itself; atlas's `instantiate` hits are scene-node instantiation in
  actions.rs. So the editor can attach a script to a node, colour it, squiggle
  it, complete it and save it, and has no way to find out whether it does
  anything at all. M5 owns Play, the game loop and hot reload; running one
  script on one node for one frame is already built and merely unwired, and it
  is the gate on every debugging entry below.

- **~~print output reaches the Console, tagged with its script~~** DONE (S, atlas)
  Everything a running instance printed since the last frame is drained into
  the Console prefixed with the node and file it came from, filterable to
  script output only, and rate-limited so a script printing every frame does
  not bury the log. A print with the script paused or stopped still shows what
  it emitted before it stopped.
  Why: ScriptInstance::take_printed exists, caps at MAX_PRINTED = 512 and has
  tests, and nothing in atlas calls it. atlas's Console is a tracing mirror -
  engine log lines - so a script's own output currently accumulates in a Vec
  that is dropped. print is the only debugging tool comet has (no debugger, no
  watch, no formatter), so an educational engine whose print goes nowhere has
  no debugging story at all.

- **~~A runtime error names a line, not a wasm trap~~** DONE (M, comet (a function name section plus a statement->line table) + helios (carry it on the error) + atlas (draw and route it))
  A trap or host error while a script runs reports the comet function and line
  it happened in, squiggles that line in the Code pane in the error colour
  until the next edit, and its Console entry jumps there when clicked. An
  error that cannot be mapped still names the script and the node rather than
  only the trap kind.
  Why: ScriptError::Runtime is a formatted wasmtime string and comet's codegen
  emits no name section, no producers section and no line table (grep
  name_section/debug_info in crates/comet: nothing), so the best a trap can
  say is `wasm trap: unreachable` against an anonymous function index. Compile
  errors carry precise spans and runtime errors carry nothing, which is
  exactly backwards: the runtime ones are the errors a learner cannot reason
  about from the text.

- **A runaway script is stopped instead of freezing the editor** (M, helios (engine config, budget, the stopped state) + atlas (report, restart))
  Each update call runs under a fuel or epoch budget; exceeding it stops that
  instance, reports "this script ran too long and was stopped" against the
  line it was executing, and leaves the editor responsive. The instance stays
  stopped until the file is edited or it is explicitly restarted, so one bad
  frame does not produce an error per frame forever.
  Why: `while true { }` inside update is the single commonest beginner mistake
  in a per-frame language, and while is comet's only loop with a hand-written
  counter, so forgetting the increment is one keystroke away.
  ScriptInstance::update calls a TypedFunc on a Store built from
  Engine::default() with neither epoch_interruption nor fuel configured, so
  the call never returns: the event loop stops pumping and the editor becomes
  a hung window with your unsaved buffer inside it. This is the one bug in
  this whole backlog that costs the user the application, and it is reachable
  in the first five minutes.

- **The node/script round trip, in both directions** (M, atlas)
  The Code pane header names the nodes whose Script component points at the
  open file and selects one when clicked, or says "no node runs this script".
  In the inspector, the Script row opens its file in the Code pane on double-
  click, and Add Script on a node with no file offers to create one (seeded
  stub, inline rename) rather than leaving an empty asset field.
  Why: open_script_file has exactly one caller - a double-click in the file
  explorer - so the only route from a node to its code is remembering the path
  and hunting for it in the tree, and there is no route at all from a script
  back to what runs it. add_script_action attaches a ScriptComponent whose
  source is empty and offers no way to fill it except picking a file that must
  already exist. This round trip is the whole difference between a game-engine
  script editor and a text editor, and it is absent in both directions.

- **Decide now whether run and reload read the buffer or the file** (S, atlas)
  Running uses the buffer and says so when it differs from disk (or saves
  first, explicitly). A reload triggered by the file changing on disk never
  silently replaces text you are typing; the two paths agree on one
  authoritative copy and the header states which.
  Why: save_script writes the buffer to disk and nothing ever reads it back.
  When M5's hot reload lands it will watch the file while the buffer holds
  newer text, and whichever wins silently is a data-loss bug in the feature
  the whole architecture was shaped around. Written down now this is a
  sentence in a doc comment; discovered after the watcher exists it is a
  rewrite of both paths.

- **Project-wide script health on load** (S, atlas)
  Opening a project compiles every .cmt it contains (or at least every one a
  node references) once, and reports the failures in the problems list: which
  file, which line, which node it would have run on. A script referenced by a
  node but missing from disk is reported the same way.
  Why: Nothing checks a script until you open it in the Code pane. A scene can
  reference six scripts, three of which no longer compile after a rename or a
  language change, and the editor's only signal is that the sprites do not
  move once Play exists. The compile is already millisecond-scale by design -
  this is the cheapest use of that property.

## Debugging a running script

- **Breakpoints in the gutter** (L, comet (a per-statement hook or an instrumented build) + helios (pause and inspect a Store) + atlas (the UI))
  Clicking the gutter's mark column toggles a breakpoint on that line; a
  paused script bands the stopped line, and the pane shows the values of the
  locals, parameters and script state in scope. Breakpoints persist per file
  in the editor state and shift with edits above them. A breakpoint on a line
  with no code snaps to the next executable line.
  Why: Event::GutterClicked exists, is emitted, and is matched by nobody - the
  backlog itself twice says the app keeps GutterClicked "for breakpoints" and
  then never opens a breakpoint entry. Without a debugger and without print
  routed anywhere, comet's debugging technique is commenting lines out, which
  is a statement about the engine, not about the editor.

- **Step, step over, continue, and advance exactly one frame** (L, helios (the stepping protocol) + atlas (the controls))
  While paused: step one statement, step over a call, continue to the next
  breakpoint, and step exactly one frame of update with the viewport redrawing
  between. The gutter marks the line about to execute and the view follows it.
  Why: Stepping is how a beginner learns that a program is a sequence and that
  update runs sixty times a second - the two ideas an educational engine
  exists to teach. The ideatank already lists step-a-frame under the teaching
  surface; the code pane is where it has to be driven from, and nothing in
  this backlog acknowledges that.

- **Live values: hover shows what a name is now, and a watch list keeps them** (M, helios (expose the store's state) + atlas (the card and the list); comet for arbitrary expressions)
  While a script is running or paused, hovering an identifier shows its
  current value alongside its type, and a small watch list keeps chosen
  expressions visible across frames. Script state shows live values even
  without a debugger, since the host already copies them each frame.
  Why: The hover entry in this backlog is static types only. In a live engine
  the question is almost never "what type is pos" - the annotation says so -
  but "what is pos right now and why is it 4000". helios already hands the
  node's position in and reads it back out around every update, so the value
  side of hover for state costs no new runtime machinery.

- **A script that can never run says so** (S, comet (the warning) or atlas (the contract is helios's, not the language's) - pick one owner deliberately)
  A file with no update function, or an update whose signature is not (dt:
  f32), gets a warning: "nothing here runs - the engine calls update(dt: f32)
  once a frame". The same when a node's Script component names a file that
  does not exist, or one that does not compile.
  Why: helios looks up the export named "update" and, finding nothing, does
  nothing. The checker is content, the file is squiggle-free, the node sits
  still, and every signal in the editor says the script is fine. This is the
  archetypal first-five-minutes failure of a teaching tool: correct code that
  silently does nothing, with no diagnostic anywhere that names the engine's
  contract.

## Teaching, help and the first five minutes

- **A keyboard reference generated from the keymap** (S, atlas)
  A shortcut and a Help affordance open a searchable list of every editor
  command with its binding, grouped by context (code pane, explorer, viewport,
  scene tree), generated from the command table rather than hand-maintained.
  It says which context each binding applies in, because several are context-
  dependent.
  Why: ctrl+s, ctrl+f, ctrl+z/y, ctrl+d, F2, Delete and the gizmo keys are
  discoverable only by reading main.rs. atlas has no menu bar, no Help entry,
  and docs/manual contains one file (index.md). A backlog that adds twenty
  more chords without this makes the problem worse, and the command-table
  foundation makes the list nearly free.

- **An in-editor comet reference** (M, atlas (the pane) + comet (supplies the strings, so it cannot drift from the checker))
  A pane or popup listing the language as it actually is: the keywords, the
  four types, the builtins with signatures and one-line descriptions, the
  shape of a script, and what the engine calls and when. Reachable from the
  Code pane's empty state, from a diagnostic, and from the completion popup.
  Why: BUILTINS is literally &["print"], the type list is four names, and none
  of it is written anywhere a learner can see - docs/manual has one file and
  the fixtures that would serve as examples live in
  crates/comet/tests/fixtures where no user reaches them. The completion popup
  is the only surface naming any of it, and it only appears after you type a
  prefix of something you already knew existed.

- **Diagnostics that can explain themselves** (M, comet (the text) + atlas (the surface))
  A diagnostic can carry a longer explanation - the rule, a wrong example and
  a right one - shown on demand from the hover card, the problems list, or a
  link in the message. Keyed by DiagnosticCode, never by matching the message
  text.
  Why: This is where the educational claim is actually cashed. "expected `;`
  after an expression statement" tells a learner what the parser wanted, not
  what they did or why the rule exists. The backlog's did-you-mean entry is
  the sharpest tenth of this; the other nine tenths is that Diagnostic is
  {span, severity, message} and nothing can say more than one sentence. It
  rides on the same DiagnosticCode change quick fixes need.

- **Starter content a first launch can actually see** (S, atlas (+ the stub text owned by comet))
  A new or empty project offers two or three annotated example scripts (move,
  bounce, follow) that open as an editable copy, and the bundled demo project
  ships with one attached to a node so the first launch shows something moving
  and something to read.
  Why: crates/atlas/demo_project has one test.cmt and the good examples
  (bounce, ticker, clamp) are test fixtures. The first-run experience is an
  empty viewport, an empty Code pane, and the sentence "No script open -
  double-click a .cmt file" pointing at a project that may contain none, with
  no way to create one from the editor.

## One hand on the mouse

- **Editor commands reachable without the keyboard** (S, atlas)
  The Code pane header carries save, undo, redo, find, comment-toggle and run,
  enabled or greyed to reflect what is possible right now; the right-click
  menu names the same commands. Undo and redo show whether they will act on
  the script or on the scene.
  Why: Every editor command in atlas is a ctrl chord with no visible
  affordance - the toolbar holds scene actions only (add sprite, gizmo modes,
  add script). A mouse-driven user cannot save the file they are editing.
  Worse, handle_shortcut routes ctrl+z by focus, so after clicking the
  viewport ctrl+z undoes a scene edit while the user believes it undid their
  typing; a button that can be disabled and labelled is what makes that
  visible.

- **ctrl+wheel changes the code font size** (S, atlas (+ the aurora font-size plumbing that entry already needs))
  ctrl+wheel over the Code pane steps the code font size within sane bounds
  and persists it; the viewport keeps its own ctrl+wheel zoom, and the gutter,
  caret, squiggles and click-to-caret all follow.
  Why: It is the standard gesture everywhere and the one accessibility
  affordance a mouse user reaches for first. The settings entry in this
  backlog covers the value but not the gesture, and a settings dialog is not
  what someone squinting at 15px does.

## Testing the editor itself

- **A headless atlas harness** (L, atlas)
  The editor's document and command core can be constructed with no window,
  GPU or event loop, driven by a scripted sequence of key and pointer events,
  and asserted against buffer text, caret, selection, undo depth, diagnostics
  and the modified flag. Every behavioural claim in this backlog becomes a
  test.
  Why: crates/atlas/src/main.rs is 4680 lines and contains two #[test]
  attributes. sync_script_buffer, coalesces, script_history, open_script_file,
  save_script, analyze_script, apply_script_marks, refresh_completions and
  accept_completion all live there and are verifiable only by hand. aurora has
  116 tests in ui.rs precisely because a Ui can be built headlessly; the app
  cannot, and the fix is the extraction, not the harness.

- **Golden-image tests for the code pane's layers** (M, aurora (a headless draw-list-to-image path) + CI)
  A committed reference image per composited state: syntax colours, a squiggle
  under coloured tokens, a selection crossing a blank line, the current-line
  band under a selection, find highlights under a selection, a decoration and
  a selection on the same range, the completion popup over the last line.
  Compared pixel-wise in CI.
  Why: The pane stacks at least six drawing layers (gutter band, selection,
  find highlights, decorations, spans, caret) whose ordering and alpha are the
  entire bug surface, and every one of them is a colour decision no unit test
  can observe. photon pixel-tests its output; aurora and atlas have no visual
  net at all, which the project's own quality notes already say - and this
  backlog adds a dozen more layers to that stack.

- **Invariant and round-trip tests over arbitrary edits** (S, aurora + atlas)
  A property test applies random edit sequences and asserts after each one:
  the caret sits on a char boundary and within text_len, every span and
  decoration range is in bounds and not inverted, undo restores the exact
  previous bytes, and open -> edit -> save -> reopen round-trips the file byte
  for byte including BOM, line endings and the final newline.
  Why: Debounced analysis - an entry already in this backlog - means spans
  computed against revision N get drawn against revision N+1, and a span whose
  end exceeds text_len is a panic in the middle of someone's typing. Nothing
  in the backlog catches the risk it creates, and the encoding entries assert
  round-trip behaviour that has no test to pin it.

## Foundations

The structural changes that unblock many entries at once. Doing any one of
these makes a dozen entries above small; skipping them makes each of those
entries carry its own version of the same work, and they drift.

- **A caret set on Ui, replacing caret: usize + selection_anchor: Option<usize>**
  An ordered, non-overlapping Vec<Caret> (offset, anchor, goal x) with a
  designated primary, sorted by offset at all times; with one caret every
  current behaviour is bit-identical. Keep caret/selection_anchor as accessors
  returning the primary so atlas compiles on day one.
  Unblocks: Every entry in the multi-caret group: alt+click, ctrl+alt+up/down,
  edits at every caret, merging, per-caret drawing, ctrl+d select-next-
  occurrence, alt+drag column selection, N-line paste, N-selection copy, per-
  caret motion, the caret-count readout, carets surviving a rebuild. It is
  also the right moment to change how the caret is stored for wrap affinity
  (byte plus affinity) and to add the goal column, rather than touching the
  same code three times.

- **A newline-safe mutation API: Ui::replace_range plus a multiline-aware control filter**
  Replace an arbitrary byte range with arbitrary text including newlines, then
  place the caret and optionally a selection at a caller-given offset. Single-
  line fields keep rejecting newlines; other control characters stay filtered;
  the Mask check still runs on the candidate.
  Unblocks: Multi-line paste (a live data-corruption bug today), find/replace
  with a newline in the replacement, block indent and outdent,
  duplicate/move/delete/join line, comment toggle, sort lines, transpose, case
  change, insert-line-below, auto-indent on Enter, Enter-between-braces,
  surround-selection, snippets, reindent, and the formatter's buffer rewrite.
  Nearly every entry in the commands group is one line of string work plus
  this call.

- **A public text-area viewport surface on Ui**
  Get/set vertical scroll, content height and visible height, the on-screen
  row range, first_visible_line, byte-offset-at-point, and the mono advance
  width. Named so it is obviously distinct from the scroll-container family.
  Unblocks: Scroll restore across a shell rebuild, the scrollbar, overview
  marks, the sticky header, hover-a-squiggle and hover-a-symbol (both need
  point-to-offset), the minimap, column rulers, indent guides, whitespace
  markers, draw culling, Escape-restores-the-find-origin, and go-to-line's
  reveal-with-context. Ten entries each currently reimplement a buffer walk
  aurora already does, app-side and worse.

- **Public selection accessors and focus_selection**
  selection_range() -> Option<Range<usize>> on the focused field, an anchor
  reader, a focus_selection(id, anchor, caret) setter, and a caret-to-(line,
  column) helper.
  Unblocks: The selection surviving a shell rebuild (a live defect), the
  Ln/Col and selected-count status readouts, block indent/outdent, comment
  toggle, sort, case change, search-in-selection, and every other command that
  must expand a selection to whole lines and then put it back.

- **An alt and ctrl modifier channel on Ui**
  Ui::set_alt(bool) and set_ctrl(bool) mirroring set_shift exactly, fed from
  atlas's ModifiersChanged arm and consulted by focus_from_press and the
  PointerMoved drag branch. Do not put modifiers on InputEvent variants.
  Unblocks: Alt+click add/remove a caret, alt+click in the gutter, alt+drag
  rectangular selection, ctrl+click go-to-definition and its ctrl+hover
  underline, and the routing for alt+arrow line moves. Aurora physically
  cannot tell an alt+click from a click today, so every mouse-driven multi-
  caret entry is blocked on this one small change.

- **One comet::Analysis per buffer revision**
  A single value carrying the token stream, AST, TypedScript, diagnostics and
  highlight spans, computed once per revision and read by diagnostics,
  highlight, completions, hover, signature help and everything after.
  Unblocks: Hover, signature help, semantic highlighting, rename, references,
  symbols, fold ranges, the bracket index, field completions on an inferred
  receiver - all of which would each otherwise add a fourth, fifth and sixth
  full pipeline pass per keystroke on top of today's three. It also shrinks by
  roughly 3x the number true incremental reparsing would have to beat before
  that question is worth asking.

- **A comet bracket and pair index over lex_with_comments**
  Per bracket token: its span, its partner or none, its depth, whether it is a
  stray closer or a never-closed opener, and a context_at(source, offset)
  answering code / string / comment. Never fails on a half-typed file.
  Unblocks: Matching-bracket highlight, enclosing-pair highlight, the
  unmatched-brace diagnostic on the opener, the mismatched-pair diagnostic,
  jump-to-match and select-to-match, rainbow brackets, the gutter block-extent
  band, the auto-close veto inside strings and comments, the balance-aware
  auto-close skip, and the correct brace test for auto-indent's opening-brace
  rule. Nine entries, one scan.

- **comet::service::symbols and fold_ranges**
  Two small projections of the existing spanned AST: one symbol per top-level
  let and func with name_span and full span, and every foldable region as
  (head_line, last_line, kind), plus an indentation fallback where the file
  did not parse.
  Unblocks: The outline pane, go-to-symbol, breadcrumbs, the sticky header,
  jump to previous/next function, and all of the folding entries. Without them
  each consumer re-walks the AST and they drift on what counts as a symbol or
  a region.

- **An EditCommand table and keymap in atlas**
  One enum of named commands with a single apply() that computes one
  replacement and one undo step, a (chord -> command) table consulted before
  translate_key, user overrides in the settings file, and headless tests over
  (text, selection) pairs.
  Unblocks: All twenty-odd entries in the commands group, the right-click
  context menu (which should name commands, not duplicate bodies),
  configurable keybindings, and command-shaped undo entries that carry the
  selection they started from. It also stops a fourth handler being added to
  the three that already each own a slice of the keyboard with implicit
  precedence.

- **Multiple open documents in atlas**
  A Documents structure replacing the seven parallel singletons on State, each
  entry carrying path, buffer, modified flag, undo/redo, caret, selection,
  scroll, spans and diagnostics, swapped atomically.
  Unblocks: The Code pane tab strip, reopening the last files on launch, two
  editors side by side, clicking a project-wide search result without
  destroying the current buffer, per-file caret memory, and the disk-change
  watcher (which needs a per-document mtime). It is also the prerequisite that
  makes the unsaved-work guard cover more than one case.

- **Style::no_wrap plus multiline horizontal scrolling**
  Set cosmic-text's Wrap::None from a Style flag and lift the multiline bail-
  out in update_hscroll/hscroll_of, with the gutter emitted from the
  unscrolled origin.
  Unblocks: Honest column selection (a column across wrapped rows is not a
  column), indent guides and column rulers that mean something on every row,
  the minimap's line-to-row mapping, a simpler visible-row cull, and the
  wrapped-continuation-indent problem, which cosmic-text 0.19 cannot solve
  directly. It also decides how several other entries reason about "line 40"
  on screen.

- **A code (or fix payload) field on comet::Diagnostic**
  Add code: DiagnosticCode - or an optional fix: Vec<TextEdit> - beside span,
  severity and message. Every construction site is Diagnostic::error, so it is
  a mechanical edit across lexer.rs, parser.rs and check.rs.
  Unblocks: Quick fixes (which would otherwise match on message text and break
  the first time a message is reworded), the unmatched-brace suppression rule
  that has to tell two diagnostics about the same block apart, and any future
  filtering or grouping in the problems list.

## Corrections the critique caught

Kept because they are the kind of mistake this list will keep making: a key
bound in three places, an entry whose stated reason is wrong, a fix that would
break something else in Orbit specifically.

- **Opening another script discards the current buffer**
  The stated why is wrong, and the truth is worse. open_script_file does not
  overwrite script_undo and script_redo - it never touches them (grep:
  script_undo/script_redo appear only in the State literal, script_history and
  sync_script_buffer). So opening a second .cmt inherits the first file's
  history: one ctrl+z replaces the new buffer with the previous file's entire
  text, at the previous file's caret, and ctrl+s then writes it to the new
  path. Clearing the history, caret and coalescing flag on open is the fix
  that must land whether or not the confirm prompt does, and the entry as
  written would ship the prompt and leave the corruption.

- **ctrl+f leaves focus in the source file**
  Understated: handle_shortcut fires ctrl+f whenever
  self.rows.code_editor.is_some(), with no focus test at all, so ctrl+f while
  renaming a node, typing in an inspector field or filtering the tree also
  opens the find bar over the code pane. The fix is a focus-scoped binding,
  not only a focus_find flag - and this is precisely the precedence problem
  the command-table foundation exists to solve, so the two should land
  together rather than adding one more condition to the ladder.

- **Highlight every occurrence of the word under the caret (ctrl+d) / Add a caret at the next occurrence of the selection (ctrl+d) / Duplicate line or selection (ctrl+shift+d)**
  ctrl+d is triple-booked, and one of the three already exists: main.rs binds
  ctrl+d to duplicate_selection on the scene whenever no field is focused. Two
  more backlog entries claim it for the code pane. Likewise ctrl+shift+L is
  claimed twice (add a caret at every occurrence; lowercase the selection),
  ctrl+k twice (delete to end of line; the ctrl+k ctrl+d chord prefix), and
  ctrl+g by find-next while go-to-line conventionally wants it. Chords must be
  assigned in one table before these entries are written, or the conflicts are
  discovered one bug report at a time.

- **shift+Tab throws focus out of the file**
  The note hands the focus-escape duty to "Escape or ctrl+Tab", but Escape is
  already claimed four times over elsewhere in the same backlog (dismiss the
  completion popup, close the find bar, collapse carets, collapse the
  selection) and ctrl+Tab is claimed by the Code pane tab strip for cycling
  documents. Taking shift+Tab away without reserving a replacement leaves a
  keyboard-only user with no exit from the text area at all - a focus trap,
  which is strictly worse than the bug being fixed. Name the escape key
  explicitly and reserve it in the keymap.

- **Escape collapses the selection to the caret / Escape collapses to one caret**
  The two entries state different orderings for the same key and neither
  matches the code. handle_editor_key's Escape arm returns false as soon as
  any field is focused, so scene-selection-clear is reachable today only with
  nothing focused - it is not competing with the editor at all. The proposal
  that Escape "drops editor focus" when there is nothing to collapse then
  makes clearing a scene selection take two Escapes, and removes the caret,
  which is the only indication the pane is live. Pick one order, write it down
  once, and prefer Escape stopping at the editor.

- **Closing the window discards every unsaved edit**
  Scoped too narrowly. The same CloseRequested arm also discards unsaved scene
  edits, which are tracked separately (history.revision() vs saved_revision,
  surfaced by is_modified()). A modal that guards script buffers only would
  ship a prompt that appears for a typo and not for an hour of scene work. One
  deferred-quit modal has to enumerate both kinds of dirty state; and the
  drafts/auto-save entry should land first so the Discard branch is
  survivable.

- **Save truncates the file before writing it**
  The stated behaviour needs one more clause in Orbit: the temp sibling must
  be a dotfile or live under .orbit/, because the explorer rescans the project
  directory and hides only names starting with '.' - a plain player.cmt.tmp
  appears in the file tree, is picked up by explorer.all_scripts() as a
  script, can be opened, thumbnailed and dragged onto a node, and is left
  visible behind any crash. Also state the rename-over-existing behaviour
  explicitly rather than relying on the Unix default.

- **Reindent by brace depth, over the selection or the whole file**
  Names comet::service::highlight as the source of which braces are real,
  while the brackets group establishes a bracket-and-pair index over
  lex_with_comments as the single authority for exactly that question. Two
  consumers with two brace scans is the drift this backlog elsewhere warns
  about; reindent should read the index.

- **top_twelve (the whole list)**
  Defensible as a text-editor triage and wrong for this milestone. M4's done-
  condition is "you write a .cmt, it compiles in milliseconds, runs on
  wasmtime, and moves a node - and the editor shows live error squiggles as
  you type", and the open tasks are E2 (live wiring) and E3 (the M4 gate
  demo). Not one of the twelve touches running a script, routing print, or
  mapping a runtime error to a line, and the list contains nothing that would
  prevent an infinite loop hanging the editor during that demo. At least the
  runaway-script guard and print-to-Console belong in the top twelve; both are
  small, and both are the difference between a code editor and a game-engine
  code editor.

