//! atlas shortcuts: every keystroke the editor answers, in one table.
//!
//! They were spread over four dispatch functions and a thousand lines, each
//! arm's condition explaining where it applies and nothing anywhere saying what
//! the set was. Adding one meant grepping for a letter to see whether it was
//! free, and there was no way at all for a person using the editor to find out
//! what it could do.
//!
//! This table is the answer to both. F1 shows it, the generated manual prints
//! it, and a test walks the source to check that nothing is dispatched that is
//! not listed here.
//!
//! It is a **register, not a dispatcher**. The handlers still decide what a key
//! does, because their conditions are real - `ctrl+m` is "jump to the matching
//! bracket" with the editor focused and nothing otherwise, and a table that
//! could express that would be a worse copy of the code. What this guarantees
//! is that the set is written down, and that it stays that way.

/// Where a shortcut applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Anywhere in the editor.
    Editor,
    /// With the Code pane focused.
    Code,
    /// With the scene tree or viewport focused (not a text field).
    Scene,
    /// With the file explorer focused.
    Files,
}

impl Scope {
    pub fn title(self) -> &'static str {
        match self {
            Scope::Editor => "Anywhere",
            Scope::Code => "Code pane",
            Scope::Scene => "Scene",
            Scope::Files => "Files pane",
        }
    }
}

/// One keystroke and what it does.
pub struct Shortcut {
    pub keys: &'static str,
    pub what: &'static str,
    pub scope: Scope,
}

const fn s(keys: &'static str, what: &'static str, scope: Scope) -> Shortcut {
    Shortcut { keys, what, scope }
}

/// Every shortcut the editor answers.
///
/// Grouped by scope and roughly by how often a key gets pressed, because this
/// is read by somebody looking for one thing.
pub const SHORTCUTS: &[Shortcut] = &[
    // --- anywhere ---
    s("F1", "This list", Scope::Editor),
    s("F5", "Play, or stop", Scope::Editor),
    s("shift+F5", "Restart the game", Scope::Editor),
    s("F11", "Maximize the pane under the pointer", Scope::Editor),
    s(
        "ctrl+S",
        "Save (the script, with the Code pane focused)",
        Scope::Editor,
    ),
    s("ctrl+O", "Reload the project from disk", Scope::Editor),
    s("F2", "Rename the selected node or file", Scope::Editor),
    s("Delete", "Delete the selection", Scope::Editor),
    // --- the scene ---
    s("Q / W / E / R", "Select, Move, Rotate, Scale", Scope::Scene),
    s("ctrl+shift+N", "Add a sprite node", Scope::Scene),
    s("ctrl+shift+M", "Add a script node", Scope::Scene),
    s("ctrl+shift+K", "Add a camera node", Scope::Scene),
    s("ctrl+D", "Duplicate the selection", Scope::Scene),
    s(
        "ctrl+C / ctrl+X / ctrl+V",
        "Copy, cut, paste nodes",
        Scope::Scene,
    ),
    s("ctrl+A", "Select every node", Scope::Scene),
    s("ctrl+Z / ctrl+Y", "Undo, redo", Scope::Scene),
    // --- the code pane ---
    s("ctrl+F", "Find (and replace)", Scope::Code),
    s("F3 / shift+F3", "Next, previous match", Scope::Code),
    s("F8 / shift+F8", "Next, previous problem", Scope::Code),
    s(
        "F12",
        "Go to what declares the name at the caret",
        Scope::Code,
    ),
    s(
        "ctrl+click",
        "The same, on the name under the pointer",
        Scope::Code,
    ),
    s("F2", "Rename the name at the caret", Scope::Code),
    s("ctrl+G", "Go to line", Scope::Code),
    s("ctrl+shift+O", "Go to a declaration by name", Scope::Code),
    s("ctrl+M", "Jump to the matching bracket", Scope::Code),
    s("ctrl+space", "Ask for completions", Scope::Code),
    s(
        "ctrl+/",
        "Comment or uncomment the selected lines",
        Scope::Code,
    ),
    s("ctrl+D", "Duplicate the selected lines", Scope::Code),
    s("ctrl+K", "Delete the selected lines", Scope::Code),
    s("alt+up / alt+down", "Move the selected lines", Scope::Code),
    s(
        "ctrl+up / ctrl+down",
        "Previous, next declaration",
        Scope::Code,
    ),
    s("ctrl+shift+J", "Join the selected lines", Scope::Code),
    s(
        "ctrl+T",
        "Transpose the characters around the caret",
        Scope::Code,
    ),
    s(
        "ctrl+shift+U / L / Y",
        "Upper, lower, title case",
        Scope::Code,
    ),
    s(
        "ctrl+shift+S",
        "Sort the selected lines (alt: drop duplicates)",
        Scope::Code,
    ),
    s(
        "ctrl+Z / ctrl+Y",
        "Undo, redo - the script's own history",
        Scope::Code,
    ),
    // --- the files pane ---
    s(
        "ctrl+C / ctrl+X / ctrl+V",
        "Copy, cut, paste files",
        Scope::Files,
    ),
    s("ctrl+A", "Select every file", Scope::Files),
];

/// The table as text, for the F1 view.
pub fn as_text() -> String {
    let mut out = String::new();
    for scope in [Scope::Editor, Scope::Scene, Scope::Code, Scope::Files] {
        out.push_str(scope.title());
        out.push('\n');
        for shortcut in SHORTCUTS.iter().filter(|s| s.scope == scope) {
            out.push_str(&format!("   {:<26}{}\n", shortcut.keys, shortcut.what));
        }
        out.push('\n');
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Keys the dispatchers use that are not shortcuts: text editing, dialog
    /// answers, and modifiers read as part of a drag.
    const NOT_SHORTCUTS: &[&str] = &["Enter", "Escape", "Tab", "Backspace"];

    /// Every key the table names, as the dispatcher spells it: the letter of a
    /// ctrl chord (whatever else is held with it), and the function keys.
    fn listed_keys() -> std::collections::HashSet<String> {
        let mut out = std::collections::HashSet::new();
        for shortcut in SHORTCUTS {
            for token in shortcut.keys.split([' ', '/']) {
                let token = token.trim().to_ascii_lowercase();
                if token.is_empty() {
                    continue;
                }
                // The last segment of a chord is the key; the rest are
                // modifiers, and which modifiers a row lists is not what this
                // is checking.
                let key = token.rsplit('+').next().unwrap_or(&token).to_string();
                out.insert(key);
            }
        }
        out
    }

    #[test]
    fn every_dispatched_key_is_in_the_table() {
        // The failure this exists to catch: a shortcut added to one of the four
        // dispatch functions and written down nowhere, so nobody using the
        // editor can find it and the next person to add one picks the same
        // letter.
        let source = include_str!("main.rs");
        let listed = listed_keys();

        let mut missing = Vec::new();
        // Function keys, which are always shortcuts.
        for n in 1..=12 {
            let key = format!("f{n}");
            if source.contains(&format!("NamedKey::F{n} ")) && !listed.contains(&key) {
                missing.push(key);
            }
        }
        // ctrl chords, which are dispatched by their letter.
        for line in source.lines() {
            let Some(rest) = line.split("eq_ignore_ascii_case(\"").nth(1) else {
                continue;
            };
            let Some(letter) = rest.split('"').next() else {
                continue;
            };
            if NOT_SHORTCUTS.contains(&letter) {
                continue;
            }
            let letter = letter.to_ascii_lowercase();
            if !listed.contains(&letter) {
                missing.push(letter);
            }
        }
        missing.sort();
        missing.dedup();
        assert!(
            missing.is_empty(),
            "dispatched but not in the shortcut table: {missing:?}"
        );
    }

    #[test]
    fn the_table_reads_as_a_table() {
        let text = as_text();
        for scope in [Scope::Editor, Scope::Scene, Scope::Code, Scope::Files] {
            assert!(text.contains(scope.title()), "{} is missing", scope.title());
        }
        // No stray blank block at the end, and every row is indented under its
        // heading - this text goes straight into a dialog.
        assert!(!text.ends_with('\n'));
        assert!(text.contains("   F5"));
    }
}
