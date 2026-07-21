# Conventions

## Comments

Every file opens with a one-line purpose comment in the same format - in Rust, a doc comment:

```rust
//! {crate or module} - {one line: what this is and its key responsibility}.
```

Beyond that, comments appear only where code cannot explain itself: invariants, non-obvious constraints, and links to the ADR that forced a strange-looking choice.

## Testing

TDD applies to the deterministic cores: the Comet compiler (golden tests - source in, expected diagnostics/WASM out), scene serialization round-trips, Aurora layout and event logic (headless), and engine math. The renderer is exempt: it gets golden-image comparison tests once output stabilizes.

## Profiling

`tracing` spans and the `puffin` frame profiler are wired in from the first triangle and stay in. Performance-relevant changes are compared against the benchmarks page before merging.

## Commits and ADRs

Any decision that is hard to reverse, surprising without context, and the result of a real trade-off gets an ADR in `docs/adr/`, numbered sequentially. The glossary in `CONTEXT.md` is the naming authority - code uses glossary terms (Node, Component, Scene, Instance, Project, Game Package).
