# Scenes serialize to RON, driven by the same reflection the inspector uses

Scene files (`*.ron`) are stored as RON (Rusty Object Notation). Serialization walks the node tree and each component's `Reflect` fields (ADR 0016) to emit RON, and reads RON back through the same field handles, so what is saved is exactly what is reflected and edited - one contract for the inspector, persistence, and hot-reload. The `ron` crate provides the syntax layer (parse and print); the field walk is ours. The project manifest stays `orbit.toml` (ADR 0009).

We rejected JSON: no comments, verbose, and the `Component` enum variants serialize as awkward tagged objects that diff poorly. We rejected TOML for scenes: fine for the flat manifest, a poor fit for a deeply nested node tree. We rejected a bespoke text format: RON already is the ergonomic, enum-friendly, serde-compatible text format for Rust data, so rolling our own buys nothing.

This gives human-readable, diff- and merge-friendly scene files - the whole point of ADR 0009 - with the on-disk shape following the reflected model automatically. Accepted cost: RON is a Rust-ecosystem format, not a universal interchange one, which is fine because a scene is Orbit's own authoring artifact, not an export format.
