//! The scripting reference, generated from the tables it describes.
//!
//! Orbit's entire user-facing surface is about forty rows of `const` tables:
//! the builtins with their signatures, the host schema, the lifecycle hooks,
//! the input bindings. None of it was written anywhere a user could read, and
//! the tables had already drifted apart from each other - `get` was a builtin
//! the checker knew, handled specially, offered in its did-you-mean list and
//! taught by name in a demo script, and the completion table had never heard of
//! it.
//!
//! Writing the reference by hand would add a fifth copy of the same lists.
//! Generating it makes drift structurally impossible instead: the test below
//! regenerates the page and compares it against what is committed, so a table
//! that gains a row and a manual that does not is a red test rather than a
//! documentation bug nobody notices for a milestone.

use std::fmt::Write;

/// Where the generated page lives, relative to the workspace root.
pub const PATH: &str = "docs/manual/reference.md";

/// Render the reference from the tables that define it.
pub fn reference() -> String {
    let mut out = String::new();
    out.push_str(
        "# Scripting reference\n\n\
         Generated from the tables in the source - see `crates/atlas/src/manual.rs`.\n\
         Editing this file by hand will be undone by the next test run.\n\n",
    );

    out.push_str("## What the engine calls\n\n");
    out.push_str(
        "A script is a `.cmt` file attached to a node through a Script component. \
         The engine looks for these three functions by name; a script needs none \
         of them, and a misspelled one is a warning rather than silence.\n\n",
    );
    for written in comet::lifecycle_hooks() {
        let _ = writeln!(out, "- `{written}`");
    }

    out.push_str("\n## What a script can reach\n\n");
    out.push_str(
        "The host surface, which the engine supplies rather than the language \
         defining (ADR 0020). Read-only properties are the engine telling the \
         script something; assigning to one is a compile error rather than a \
         silent no-op.\n\n\
         | Property | Type | |\n|---|---|---|\n",
    );
    for (object, field, ty, access) in helios::script_properties() {
        let access = match access {
            comet::Access::ReadWrite => "",
            comet::Access::ReadOnly => "read-only",
        };
        let _ = writeln!(
            out,
            "| `{object}.{field}` | `{}` | {access} |",
            ty.ty().name()
        );
    }

    out.push_str("\n## Builtin functions\n\n");
    for (_, signature) in comet::service::BUILTINS {
        let _ = writeln!(out, "- `{signature}`");
    }

    out.push_str("\n## Keys a game reads\n\n");
    out.push_str(
        "Held-key state, not events: `input.left` is true for as long as the key \
         is down. Physical positions rather than letters, so the shape under \
         three fingers is the same on every keyboard layout.\n\n",
    );
    for (action, keys) in crate::keys::bindings() {
        let _ = writeln!(out, "- `input.{action}` - {keys}");
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn committed() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(std::path::Path::parent)
            .expect("atlas sits two levels under the workspace root")
            .join(PATH)
    }

    #[test]
    fn the_committed_reference_matches_the_tables_it_describes() {
        // Regenerate rather than assert on the contents: a test that only
        // checked a row exists would need updating as often as the page does,
        // and would still miss a row that was removed. Set ORBIT_WRITE_MANUAL=1
        // to accept a change.
        let generated = reference();
        let path = committed();
        if std::env::var("ORBIT_WRITE_MANUAL").is_ok() {
            std::fs::write(&path, &generated).expect("writing the reference");
            return;
        }
        let found = std::fs::read_to_string(&path).unwrap_or_default();
        assert_eq!(
            found, generated,
            "\n{} is out of date. Run:\n  ORBIT_WRITE_MANUAL=1 cargo test -p atlas manual\n",
            PATH
        );
    }

    #[test]
    fn the_reference_covers_every_table_it_claims_to() {
        let text = reference();
        assert!(text.contains("func update(dt: f32)"), "the hooks");
        assert!(text.contains("`transform.position`"), "the schema");
        assert!(text.contains("read-only"), "and which properties are");
        assert!(text.contains("func random() -> f32"), "the builtins");
        assert!(text.contains("`input.jump`"), "and the bindings");
    }
}
