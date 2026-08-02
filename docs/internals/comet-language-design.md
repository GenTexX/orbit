# Comet language design decisions

A working record of decisions taken about the Comet language after Milestone 4
shipped, ahead of implementing any of them. It exists because the decisions
below are the kind ADR 0006 and ADR 0007 were: hard to reverse, surprising
without context, and the result of a real trade-off. Several of them will be
promoted to numbered ADRs once the set is complete - which one, and how many,
is itself still open (see "Still to decide").

**Status: incomplete.** Fourteen decisions are settled and two questions are
open. One of those - default values for a bare export - is part of decision 14
rather than independent of it, so decision 14 should not be built before it is
answered. A further set, listed under "Still to decide", has not been put yet.
Nothing here is implemented.

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

Note: this raised a question decision 7 does not settle by itself - `Option<T>`
is generic, and nothing else in the language is. Settled as decision 10.

### 10. The language gets real generics

Chosen over blessing `Option` as the one parameterized type, and over
monomorphizing per concrete use at check time without a surface concept.

Why: a one-off exemption for `Option` is the same asymmetry decision 7 rejected,
one level down. Real generics also make `Array<T>`, and any future container, an
ordinary declaration rather than another compiler special case.

Note: this is a decision about the **surface**. How generics are compiled stays
open, and monomorphizing at emit remains available - a generic surface does not
oblige a generic representation. What it does oblige is a type-parameter concept
in the checker, which is the first thing in the pipeline not resolvable by a
single pass over the tree, against a design whose headline property is compile
speed (ADR 0007). Keeping the checking cheap is the risk to watch.

### 11. `@export` marks a variable as inspector-editable

`@export let speed = 120.0;`. One new lexer token, and the `@` prefix
generalizes to annotations that take arguments - `@range(0, 100)`,
`@tooltip("...")` - so the language does not end up with one syntax for flags
and another for parameterized ones.

Rejected: a plain `export` keyword (burns a reserved word and still needs a
second mechanism for the parameterized annotations), and a doc-comment
convention (no grammar change, but a typo would silently do nothing instead of
producing a diagnostic, which is the wrong failure mode for a language meant to
be learned from).

### 12. The stored value wins, and an exported variable should not carry one

Godot's model: once a variable is exported, the inspector is the source of
truth. The initializer is the default used when the component is first created
or the field is explicitly reverted. This is what makes per-instance tuning work
at all - two nodes running one script at different speeds.

Beyond the mechanism, the intended idiom is that **an exported variable does not
carry a meaningful value in the script**. If the inspector owns the value, a
number in the source is a second answer to the same question, and the one the
reader sees first is the one that loses. The mechanism above defines what
happens; this says what a well-written script looks like.

Note: how far to push that is open, and recorded below. It ranges from
documenting it, to a warning on an exported variable with a non-trivial
initializer, to letting an exported `let` omit its initializer entirely - which
today is not expressible, since a state declaration's initializer is not
optional.

### 13. `start()` and `on_destroy()`, but not `fixed_update` yet

`start()` runs once before the first update; `on_destroy()` runs when the node
or script goes away. Both are one more exported-name lookup in the host.

`start()` also clarifies something currently subtle: a top-level `let` is doing
double duty as init code, and the demo script needs a comment to explain it.
`on_destroy()` is cheap now and awkward to retrofit once instances have real
lifetimes.

`fixed_update(dt)` was deliberately not taken. It needs an accumulator in the
runtime and a decided ordering against `update`, which is M5 runtime design
rather than language design, and it should be settled there with the game loop
in front of us.

### 14. An exported variable carrying an initializer is a warning

`@export let speed = 120.0;` warns. Decision 12's idiom becomes something the
compiler says rather than something the documentation asks for, which is the
only version of it that a reader encounters at the moment it matters.

**This forces a grammar change, and the two must ship together.** A state
declaration's initializer is not optional today - `StateDecl.init` is an `Expr`,
while `ty` is the `Option` - so on its own this warning would fire on every
exported variable with no way to act on it, and a warning nobody can silence is
worse than no warning at all. `@export let speed: f32;` therefore has to become
writable, which inverts the current optionality: a state declaration needs a
type or an initializer, and an exported one should carry only the type.

Note: the warning is on *any* initializer, not only a non-default one, because
"is this value the type's default" is a question about the value rather than
about the code, and the answer changes what the compiler says about identical
source. Simpler to state and simpler to act on.

Two things this needs that are not yet settled, recorded below rather than
guessed: what each type's default is, and what happens for a type that has no
natural default.

## Open

- **Default values for a bare `@export let x: T;`** (from decision 14). Most
  types answer easily - `f32` is `0.0`, `int` is `0`, `bool` is `false`, `Vec2`
  is the origin, `String` is empty, `Option<T>` is `None`. **A user-defined enum
  has no natural default**, which is the real question: either its first variant
  is it (cheap, and an arbitrary rule the author may not have thought about),
  or an exported enum keeps its initializer and is exempt from decision 14's
  warning (honest, but it makes the warning conditional on type), or enums are
  simply not exportable in v1 (smallest, and it defers the question to when
  someone wants it).
- **Arrays: reference or value semantics.** Deliberately deferred to when arrays
  are actually scheduled, rather than decided ahead of the work. The tension is
  recorded so it is not rediscovered: reference semantics match ADR 0006's "GC
  reference semantics for objects, value semantics for small structs" and are
  what Godot and Python do, at the cost of the aliasing surprise that is the
  classic Godot gotcha; value semantics would be consistent with `Vec2` and
  surprise nobody, but copy on every pass unless the compiler grows escape
  analysis, which is the kind of optimization ADR 0007 rules out.

Not in question: a script defining `func update()` with the wrong signature
currently gets silence, because the host looks up exactly one hardcoded name at
`f32 -> ()`. That needs a diagnostic regardless of anything above.

## Still to decide

Not yet put, and roughly in the order they will start to matter:

- **Which annotations exist in v1.** `@range`, `@step`, `@tooltip`, `@color`,
  `@asset`, `@multiline`, `@readonly`. The argument for doing several at once is
  that atlas already owns every widget each one would map to - the slider, the
  drag-scrub numeric field, the colour picker, the asset field with its drop
  target, the text area. This is wiring rather than new UI.
- **Arrays and structs - when to schedule them.** The Milestone 4 plan deferred
  them to "a fast-follow once the single-refcounted-type case is proven", and
  `String` proves it, so this is due rather than new. Array semantics are
  deliberately deferred with them (see "Open"); the parallel question for
  structs is whether they are value or reference types, and reference structs
  are the other place ADR 0007's cycle leak becomes reachable. Generics
  (decision 10) means `Array<T>` is now an ordinary declaration rather than a
  builtin, which makes this cheaper than it was when the plan deferred it.
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
- The checker grows a type-parameter concept (decision 10), the first thing in
  the pipeline not resolvable by a single pass over the tree. How generics are
  compiled is a separate, still-open choice: monomorphizing at emit keeps
  codegen close to what it is today.
- The host looks up exactly one hardcoded exported name at one hardcoded
  signature (`UPDATE`, `f32 -> ()`), so decision 13 touches helios as well as
  comet, and the wrong-signature diagnostic has to know all three names.
- `ScriptComponent` grows from a single `source: String` into source plus stored
  exported values (decisions 11 and 12), which is what makes decision 3's
  dynamic `Reflect` load-bearing rather than merely tidy.
- `StateDecl`'s optionality inverts (decision 14): `init` becomes optional and
  a declaration must carry a type or an initializer. Type inference for state
  therefore stops being unconditional, since there is nothing to infer from
  when only the type is written - which is the point, but it is a checker
  change and not only a parser one.
- Decision 14 makes comet emit a warning about a declaration that is otherwise
  perfectly legal, which is a first: every existing warning is about code that
  does nothing (an unused local), not about a style the project prefers. Worth
  being deliberate about, because it sets the precedent for whether comet's
  diagnostics police idiom at all.
