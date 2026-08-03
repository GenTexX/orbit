# Hot reload preserves reflected fields, resets heap and coroutines

When a recompiled script module hot-loads into a running game, each Script component's reflected fields (the same set the inspector shows) are serialized, the new module is instantiated, and fields matching by name and compatible type are reapplied; everything else - heap object graphs, in-flight coroutines - restarts cold. The reflection system built for the inspector does double duty as the migration mechanism. Full heap migration was rejected as a tar pit (layout diffing, identity mapping, failure semantics); cold restart was rejected because it reduces the language's headline hot-reload feature to a restart button.

## Amendment (2026-08-03, Milestone 5)

Two things the original decision did not say, both settled while building the
reload path.

**A reload does not re-run `start`.** A script whose `start` places its node -
which is what `start` is for - would teleport it back to its opening position on
every save, and the person watching would conclude that saving resets the game.
`helios::Begin` names the two cases at the one call site that differs.

**Plain top-level state carries across; heap-backed state does not.** The
original wording said reflected fields are reapplied and "everything else - heap
object graphs, in-flight coroutines - restarts cold". Taken literally that left a
gap: state a `start` had set was neither preserved nor re-initialized, because
the initializer re-ran and `start` did not. So a script that set its velocity in
`start` simply stopped when the file was saved.

The rule is now narrower and complete. A reload builds a whole new linear memory,
so anything held as a pointer into the old heap would arrive dangling - and a
wasm `i32` alone cannot tell an `int` from a pointer. comet therefore decides
what is carryable and exports only those globals (scalars: f32, int, bool, Vec2),
and the host copies whatever it finds exported and is right by construction. A
`String`, an `Array`, and any struct or enum that could contain one restart cold,
which is what this ADR always said about heap object graphs.

The component still wins for anything it owns (ADR 0022): those names are skipped,
because the inspector's value was written in a moment earlier and carrying the
module's copy over would quietly reverse that rule.
