# Comet language design decisions

A working record of decisions taken about the Comet language after Milestone 4
shipped, ahead of implementing any of them. It exists because the decisions
below are the kind ADR 0006 and ADR 0007 were: hard to reverse, surprising
without context, and the result of a real trade-off. Several of them will be
promoted to numbered ADRs once the set is complete - which one, and how many,
is itself still open (see "Still to decide").

**Status: incomplete.** Nine decisions are settled. Four questions are open, and
a further set has not been put yet. Nothing here is implemented. The generics
question under "Open" gates decisions 7 and 9, so those two should not be built
before it is answered.

## Where the language actually stands

Recorded here because every decision below is relative to it, and the roadmap
describes the milestone rather than the surface.

Types are `f32`, `bool`, `Vec2` (a value type), `String` (the one refcounted
heap type), plus `Unit` and an `Error` poison type. Declarations are a top-level
`let` (persistent state, lowered to a WASM global) and `func f(p: T) -> R`.
Statements are `let`, assignment with the compound forms, `if`/`else if`/`else`,
`while`, `for i in a..b`, `return`, and a bare expression. Expressions are
literals, identifiers, `.x`/`.y` field access, calls, unary `-` and `!`, the
binary arithmetic, comparison and short-circuiting logical operators, and a
block's tail expression as its value. The builtins are `print`, `str`, `vec2`,
`abs`, `sqrt`, `floor`, `ceil`, `min`, `max`, `sin`, `cos`, `atan2`, `pow`.

Two things are worth stating plainly because they drove several decisions:

- **The host surface is `pos`, and nothing else.** A script can read and write
  its own node's local translation. It cannot see rotation, scale, its own
  components, any other node, input, or time.
- **`Vec2` has no arithmetic.** `pos + vel`, `pos * dt` and `-v` are all type
  errors today; only construction, field access and equality exist. This is why
  `bounce.cmt` is written one axis at a time.

## Decided

### 1. The host surface is path-based, not magic identifiers

`pos` is a magic name with dedicated variants all the way down the pipeline -
`TypedExprKind::Pos`, `Place::Pos`, `Place::PosField(Axis)`. Every new
host-exposed property would cost another set of special cases in the checker,
the typed IR, and codegen.

Instead `Place` generalizes from a variant per property to a resolved path, so
`transform.position.x`, `transform.rotation` and a component's fields are all
one mechanism. Adding a property stops costing new IR variants.

Note: this is the enabling change for input and for component access, so it is
worth doing before either. `pos` is expected to survive as sugar for
`transform.position` so existing scripts and the demo keep working - but that is
an assumption, not yet a decision.

### 2. comet compiles against a host schema, supplied by the engine

`compile(source, &schema)` rather than a closed `Host` enum inside comet. The
schema describes the available surface - component types, their field names and
types - and helios generates it from the `Reflect` contract it already has.

Why: comet stops accumulating engine knowledge and never names `Sprite`; adding
a component makes it scriptable without touching the compiler; and the language
service gets real component-field completions for free. It makes reflection a
fourth consumer of one contract, alongside the inspector, the serializer, and
ADR 0008's hot-reload migration - which is the property ADR 0016 exists to
claim.

Note: this changes `comet::compile`'s signature, which is helios's only entry
point into the compiler. comet's tests already run against a fake host, so a
fake schema is consistent with how the crate is already tested.

### 3. `Reflect` grows dynamic fields

`Reflect::field_names` returns `&'static [&'static str]`, which cannot describe
a component whose fields depend on a source file. Rather than special-casing
`ScriptComponent` in the inspector and the serializer, the contract itself
changes so a field list can be owned rather than static.

Why: it keeps ADR 0016's actual claim true - that the inspector, the serializer
and hot-reload all walk one contract and none of them hard-codes any
component's fields. A special case would make that sentence false and would
leave hot-reload migration needing its own path.

Note: this touches every existing `Reflect` impl, and it revises ADR 0016 rather
than merely extending it.

### 4. An `int` type is added

Chosen over keeping the language f32-only, and chosen now rather than deferring
until arrays force it. Honest indices and loop counters, no fencepost weirdness,
and `arr[1.5]` stops being legal - which matters more in a language whose stated
purpose is teaching.

Note: the cost is a doubled numeric surface - two literal kinds, conversion
rules, and int/float variants of every arithmetic and comparison operation.
This is the largest single addition among the decisions here.

### 5. `int` widens to `f32` implicitly; narrowing stays explicit

`5` is an `int`, `5.0` is an `f32`, and an `int` widens silently wherever an
`f32` is wanted. Going the other way needs `int(x)`.

Why: one-directional, so there is no precision-loss surprise, and existing
scripts keep compiling. This matters concretely - the Milestone 4 plan states
that "bare integer literals (200, not 200.0) are valid f32 literals", and every
script in the repo, including `bounce.cmt` and the two demo scripts, relies on
it. Strict conversion would have made `pos.x += 1` an error and required editing
them all.

Note: this supersedes that line in the Milestone 4 plan, which should be marked
rather than silently contradicted. Full context-inferred literal types were
rejected as the most expensive option for a checker that is deliberately
single-pass, and compile speed is the pipeline's headline property (ADR 0007).

### 6. `Vec2` gets the additive core plus scalar multiplication

`+`, `-`, unary `-`, `Vec2 * f32`, `f32 * Vec2`, and `Vec2 / f32`. Exactly
enough to make `pos += vel * dt` compile.

Deliberately excluded for now: `dot`, `length`, `normalize`, `distance`, and
componentwise `Vec2 * Vec2`. The geometry builtins are a small further step and
can follow. Componentwise multiplication was rejected on teaching grounds -
`a * b` on two vectors reads as a dot product to enough people that it is a
hazard in a language meant to be learned from.

Note: this is the smallest and most-felt item on the list. It is pure checker
plus codegen, introduces no new type, and needs nothing from the host.

### 7. User-defined enums carry payloads, with exhaustive `match`

`enum State { Idle, Walking, Falling }`, and also
`enum Hit { Miss, Wall(f32), Node(Node) }`. Real sum types.

This revises an earlier answer of payload-free enums, taken back once it turned
out not to compose with decision 8. `Option` carries a payload by definition, so
a payload-free rule would have meant special-casing `Option` in the compiler
while forbidding users the same shape - an asymmetry a learner notices and
cannot act on.

Why: state machines can carry the data their states are about, `Option` needs no
special casing, and `match` still gets the exhaustiveness checking that was the
point of the original answer. A compiler that tells a learner they forgot a case
is a teaching win rather than only an ergonomic one.

Note: this is the largest single language change on the list. The cost is a
tagged representation plus per-variant refcount rules against ADR 0007's
allocator - a variant holding a `String` means release has to consult the tag to
know what to drop. It is the first construct in the language whose layout depends
on which variant is live, and the first place where the "known limitation" in
ADR 0007 (cycles leak until weak refs exist) becomes reachable from ordinary
user code rather than only from a future reference struct.

### 8. `Option` and exhaustive matching land now, not later

Rather than deferring absence until node handles exist. Arrays going out of
bounds, schema lookups, and a future `find("Player")` all introduce it, and the
decision was taken to settle it before anything depends on it.

Why not the alternative: a null value plus a runtime trap is cheap and familiar,
but it puts a whole class of error back into runtime after ADR 0006 chose static
types specifically to catch things early in our own editor.

### 9. `Option` is an ordinary enum, not a compiler builtin

Follows from decision 7. With payloads available to users there is nothing left
for a builtin to provide, so `Option` is declared the way any other sum type is
and gets no special treatment in the checker or codegen.

Rejected on the way: a compiler-known `Option<T>` alongside payload-free user
enums (the asymmetry described in decision 7), and a nullable type suffix -
`Node?`, `int?` - which would have avoided both payloads and generics, but only
covers "a T or nothing" and would have to be replaced the first time a second
payload-carrying shape appeared.

Note: this raises a question decision 7 does not settle by itself, recorded
under "Open" below - `Option<T>` is generic, and nothing else in the language is.

## Open - asked, not yet answered

- **Does the language get generics?** Created by decisions 7 and 9 together.
  `Option<T>` is parameterized over its payload type, and no other Comet
  construct is parameterized over anything. Three ways out, none obviously
  right: user-facing generics on enums and functions (the general answer, and
  the most checker work in a pipeline whose headline property is compile speed);
  `Option` alone is parameterized, as the one blessed generic, which is a
  smaller version of exactly the asymmetry decision 7 rejected; or monomorphize
  per concrete use at check time, which keeps codegen simple but needs a rule
  for what happens when the payload type is itself an error type. This should be
  settled before any of decision 7 is built, because it decides whether the
  checker grows a type-parameter concept at all.
- **Export syntax.** `@export let speed = 120.0;` (one new lexer token, and the
  `@` prefix generalizes to `@range(0, 100)` and `@tooltip("...")`, matching the
  Godot spelling this audience likely arrives with), a plain `export` keyword
  (no new token class, but annotations with arguments then need a second
  mechanism), or a doc-comment convention (no grammar change, but semantics move
  into comments the lexer discards and a typo silently does nothing).
- **Initializer versus stored value.** The script says `let speed = 120.0`, the
  inspector has stored `200`. Either the stored value wins and the initializer
  is the default on creation and revert (Godot's model, and what makes
  per-instance tuning work), or the initializer always wins and inspector edits
  touch only the live instance, or the stored value is re-seeded whenever the
  initializer changes.
- **Lifecycle hooks beyond `update`.** Candidates are `start()` before the first
  update (nearly free, and it clarifies that a top-level `let` is currently
  doing double duty as init code), `on_destroy()` (cheap now, awkward to
  retrofit once instances have lifetimes), and `fixed_update(dt)` (needs an
  accumulator and an ordering rule, so more M5 runtime design than language
  design).

Independent of that last one: a script defining `func update()` with the wrong
signature currently gets silence, because the host looks up exactly one hardcoded
name at `f32 -> ()`. A diagnostic for that is not in question.

## Still to decide

Not yet put, and roughly in the order they will start to matter:

- **Which annotations exist in v1.** `@range`, `@step`, `@tooltip`, `@color`,
  `@asset`, `@multiline`, `@readonly`. The argument for doing several at once is
  that atlas already owns every widget each one would map to - the slider, the
  drag-scrub numeric field, the colour picker, the asset field with its drop
  target, the text area. This is wiring rather than new UI.
- **Arrays and structs.** The Milestone 4 plan deferred them to "a fast-follow
  once the single-refcounted-type case is proven", and `String` proves it, so
  this is due rather than new. Open within it: whether structs are value or
  reference types, and that reference structs will immediately expose the cycle
  leak ADR 0007 names as a known limitation.
- **Tuples and multiple return values.** WASM multi-value makes this nearly free
  and it removes the need for out-parameters.
- **`const` declarations**, distinct from mutable state.
- **Whether required parameter and return annotations stay.** Currently
  mandatory, which is defensible for single-pass checking and clarity, but it is
  an accident of implementation rather than a written decision.
- **Which warnings the language emits.** Unused locals already work. Candidates:
  unreachable code after `return`, a function never called, shadowing, and the
  wrong-signature `update` above.
- **The teaching surface.** Whether to build a panel showing the emitted WASM or
  the typed IR beside the source. The pipeline is single-pass and the IR is
  already a clean tree, so it is unusually cheap for how much it would
  differentiate an engine that calls itself educational - and the ideatank notes
  that nothing in the codebase yet serves that claim.
- **Which of these become numbered ADRs**, and whether ADR 0016 is revised in
  place or superseded by a new one. ADRs 0006 and 0007 were both written before
  the code they governed, so ADR-before-implementation is the established
  pattern here.

## Consequences already implied

Collected so they are not rediscovered during implementation:

- `comet::compile` changes signature (decision 2), and helios is its only
  caller.
- The closed `Host` enum in `tir.rs` stops being the definition of the host
  surface (decision 2).
- ADR 0016 is revised, not merely extended (decision 3).
- The Milestone 4 plan's statement that bare integer literals are f32 literals
  is superseded (decision 5).
- `Place::Pos` and `Place::PosField` disappear into a general path (decision 1),
  which is the one change here that gets more expensive the longer other work
  piles onto `pos`.
- Release stops being decidable from a value's static type alone (decision 7):
  with payload-carrying variants, dropping a value means reading its tag first
  to know which payload, if any, to release. This is the first thing in comet
  whose layout is not fixed by its type.
- ADR 0007's known limitation - reference cycles leak until weak refs or a cycle
  collector exist - becomes reachable from ordinary user code once an enum
  variant can hold a reference (decision 7), rather than staying theoretical
  until reference structs arrive.
- The checker may grow a type-parameter concept (the open generics question),
  which would be the first thing in the pipeline that is not resolvable in a
  single pass over the tree.
