# Plan - completing the Comet language (iterations 4.1 - 4.9)

## Context

Milestone 4 shipped a language that runs. The
[Comet language design record](../comet-language-design.md) then took fourteen
decisions about what it should become, left two questions open, and listed a
further set that has not been put yet. None of it is implemented.

This plan sequences all of it, and it runs **before Milestone 5**. That is a
deliberate choice with a cost worth naming: M5 is the killer feature, and this
delays it. What it buys is that M5's hot-reload field migration gets built once,
against a settled language, rather than against a `Script` component that has no
exported fields yet.

Numbered as an iteration phase on Milestone 4, following the convention M3 set
with its 3.1 - 3.6 reports.

## The shape of the whole thing

The order below is not the order the decisions were taken in. It follows three
constraints, and everything else is preference:

1. **Decision 1 gets more expensive the longer other work piles onto `pos`.**
   The record says so explicitly. It goes early, before its cost grows.
2. **`int` must precede containers**, because `arr[1.5]` being illegal is half
   the argument for the type - and it should precede everything else that writes
   numeric code, which is all of it.
3. **`@export` must precede user-defined enums.** This is the one piece of
   sequencing that changes what has to be decided rather than only when. See
   4.4.

Everything else is ordered cheapest-and-most-felt first, so the demo scripts
each iteration is proven with get better to read as the plan progresses rather
than at the end.

---

## 4.1 - Vectors and lifecycle (DONE)

**Builds.** Decision 6: `+`, `-`, unary `-`, `Vec2 * f32`, `f32 * Vec2`,
`Vec2 / f32`. Deliberately not `dot`, `length`, `normalize`, `distance`, or
componentwise `Vec2 * Vec2` - the last rejected on teaching grounds. Decision
13: `start()` and `on_destroy()`.

**Why here.** No dependencies in either direction, and decision 6 is the change
`bounce.cmt` is visibly contorted around - it is written one axis at a time
because `pos + vel` is a type error. Doing it first means every later
iteration's example scripts are written in vectors.

**Decide first.** Nothing.

**Risk.** Near zero. Decision 6 is pure checker plus codegen, introduces no new
type and needs nothing from the host. Decision 13 is two rows in the `HOOKS`
table added in `5bcb5ea` plus one lookup each in helios - and the
wrong-signature diagnostic then covers all three names, which the record's
consequences list predicted it would have to.

**ADR.** None. Neither is surprising enough to need one.

**Proven by.** `bounce.cmt` rewritten as `pos += vel * dt` with a `start()` that
seeds the velocity. Execution tests that a Vec2 operation emits both lanes
rather than reading one twice - the same class of test that caught operand
swaps in M4.

**Done.** Six execution tests for the arithmetic, four for the hooks, all
verified to fail by exactly the wrong value when the lane pairing is crossed.
Two things worth carrying forward:

- Compound assignment now routes through `binary()` rather than hardcoding f32
  on both sides, so `pos += vel * dt` works exactly when `pos + vel * dt` does
  and there is one definition of each operator instead of two. It gave
  `s += "text"` for free, which is right and was not planned.
- The frame machinery built for `%` and string concatenation covered Vec2
  arithmetic with one change - a third f32 per frame, because `a + b` on two
  vectors parks three values. Nesting was correct on the first try because of
  it, and there is a test that proves it.

---

## 4.2 - The host surface generalizes (DONE)

**Builds.** Decision 1: `Place` becomes a resolved path, and
`TypedExprKind::Pos`, `Place::Pos` and `Place::PosField` disappear into it.
Decision 3: `Reflect::field_names` stops returning `&'static [&'static str]` so
a component's field list can be owned. Decision 2: `compile(source, &schema)`,
with helios generating the schema from the `Reflect` contract.

**Why here.** Constraint 1. 4.1 piles more onto `pos`, and everything after this
wants to reach a component field.

**Decide first.** Whether `pos` survives as sugar for `transform.position`. The
record flags this as an assumption rather than a decision, and it governs
whether every existing script and both demo files keep working untouched.

**Risk.** The highest of the early iterations, and the widest. Three crates.
`Reflect` touches every impl in helios and **revises ADR 0016 rather than
extending it** - the ADR's actual claim is that inspector, serializer and
hot-reload all walk one contract with no component hard-coded, and a special
case for `ScriptComponent` would make that sentence false. `compile`'s signature
change has exactly one caller, so that part's blast radius is known.

**ADR.** New: the path-based host surface and the host schema. ADR 0016 revised
in place or superseded - itself a decision, and the first instance of a question
the record lists under "still to decide".

**Proven by.** A script reading `transform.rotation` and writing a `Sprite`
field. comet's tests run against a fake schema, consistent with how the crate
already runs against a fake host.

**Done** as ADR 0020, with two deliberate changes to the plan:

- **`pos` is gone rather than kept as sugar.** Philip's call, and the more
  honest one - sugar would have left a second way to say the same thing forever.
  It broke every script in the repo: both demo files, four fixtures and around
  forty test sources. That was the cheapest this will ever be, which is the
  argument for having done it second rather than eighth.
- **Decision 3 moved to 4.4, and components are not in the schema yet.**
  Dynamic `Reflect` fields have no consumer until `ScriptComponent` carries
  exported values, so building it here would have been building ahead of its
  user. Component properties were left out because "what does a script see when
  the node has no Sprite" is a runtime-semantics question worth answering
  deliberately; the mechanism does not care, and adding them later is a data
  change. ADR 0016 is therefore untouched.

The shape that made it work: properties are **numbered**, and codegen passes the
number to a fixed set of accessors. The import list stays fixed however large
the schema grows, which is the property the old fixed-import design had and the
one a function-per-property design would have lost.

---

## 4.3 - The numeric model (DONE)

**Builds.** Decisions 4 and 5: an `int` type, two literal kinds, int and float
variants of every arithmetic and comparison operation, implicit widening from
`int` to `f32`, and explicit `int(x)` to narrow.

**Why here.** Constraint 2, and the general form of it: this is the largest
mechanical surface on the list, and every iteration after it writes numeric
code. Retrofitting `int` after enums, generics and arrays means revisiting all
three.

**Decide first.** Whether `for i in a..b` becomes an int loop. It should - the
counted loop is the case the type was argued for - but it changes the type of
`i` in scripts that already exist.

**Risk.** High surface area, low conceptual risk. Nothing here is subtle; there
is simply a lot of it, and each operation needs its codegen and its test. The
widening is one-directional, so there is no precision-loss surprise to design
around.

**ADR.** The numeric model, including why context-inferred literal types were
rejected as too expensive for a checker whose headline property is speed.

**Proven by.** Every script in the repo compiling unchanged - which is the point
of the implicit widening, and is why decision 5 exists at all.

**Done** as ADR 0021. Three things worth carrying forward:

- **One coercion point.** `Checker::coerce` is the only place widening happens
  and every expected-type site routes through it. Scattering it would have meant
  missing one, and the failure is silent: the checker permits it, codegen emits
  an i32 where an f32 belongs, and the module fails to validate with no line
  number. That happened once during the work - the function tail was the site I
  missed - and it surfaced as `type mismatch: expected f32, found i32` with no
  source location, exactly as predicted.
- **The widening is a node, not a permission.** A rule the checker allows but
  does not record is one codegen cannot act on.
- **`str` of an int is its own host call.** Widening first would print a rounded
  number past 2^24, silently, in the one function used to look at a value. The
  test fails with `16777216` if that path is taken.

Left undone deliberately: the maths builtins stay f32-only, so `abs(-5)` widens
and returns an f32. Doubling them is a wider surface than this iteration should
carry, and containers are what will make `min`/`max` on ints actually hurt.

---

## 4.4 - Exported variables (DONE)

**Builds.** Decision 11: an `@` token and an annotation grammar that generalizes
to arguments, then `@export`. Decision 14's grammar change: `StateDecl.init`
becomes optional and a declaration must carry a type or an initializer -
inverting today's optionality. Decision 14's warning. Decision 12:
`ScriptComponent` grows from `source: String` to source plus stored values, the
inspector edits them, and the stored value wins.

**Why here - and this is the load-bearing bit of the whole plan.** The record's
open question is what a bare `@export let x: T;` defaults to, and it is listed
as blocking decision 14. But the only type with no natural default is a
**user-defined enum**, and those do not exist until 4.5. Ship `@export` before
them and every type in the language has an obvious answer: `f32` is `0.0`, `int`
is `0`, `bool` is `false`, `Vec2` is the origin, `String` is empty. Decision 14
ships unblocked, and the enum case is answered in 4.5 with the enum work in
front of us instead of guessed at now.

**Decide first.** Only confirm the default table above. The hard part of the
question is deferred by construction rather than by choice.

**Risk.** Medium, spread across comet, helios and atlas. Decision 3 from 4.2 is
what makes the dynamic field list load-bearing here rather than merely tidy.
Note that decision 14 makes comet warn about a declaration that is otherwise
perfectly legal - the first time a diagnostic polices idiom rather than
correctness, which sets a precedent worth being deliberate about.

**ADR.** `@export` and the inspector-owns-the-value model.

**Proven by.** Two nodes running one script at different speeds. That is the
thing decision 12 exists for, and it is exactly what M5's hot-reload promise is
measured against.

**Done** as ADR 0022, including decision 3, which came here from 4.2 as planned.

The sequencing bet paid: with no user-defined enums yet, every type had an
obvious default, so decision 14 shipped without the open question ever needing
an answer. It is now the enum iteration's to take, with that work in front of
it.

The cost landed somewhere the plan did not name. Decision 3 was scoped as "touches
every `Reflect` impl", and it does - but the expensive part was the *inspector*,
which assumed `'static` field names all the way down: `FieldRef`, `ColorTarget`,
`AssetTarget` and four row vectors all held `&'static str` and were `Copy`. That
assumption is precisely what a dynamic field list breaks, and unpicking it was
most of the iteration. Worth knowing before 4.7 adds more annotations to the
same plumbing.

One thing deliberately not built: reading exported values back out of a running
module. The write half has a user now; the read half does not until Play exists,
so it was deleted rather than left dead - the same call made about decision 3 in
4.2.

---

## 4.5 - Sum types and generics (DONE)

**Builds.** Decisions 7, 8, 9, 10: user-defined enums carrying payloads,
exhaustive `match`, `Option<T>` as an ordinary declaration with no compiler
special-casing, and real generics.

**Why here.** The largest and riskiest block, and nothing before it depends on
it. It wants `int` already present so the tag representation is designed once.

**Decide first.**
- **How generics are compiled.** The record settles the surface and explicitly
  leaves this open. Monomorphizing at emit keeps codegen close to what it is
  today.
- **The exported-enum question deferred from 4.4**: first variant as the default
  (cheap, and an arbitrary rule the author may not have thought about), exempt
  from decision 14's warning (honest, but makes the warning conditional on
  type), or enums simply not exportable in v1 (smallest).

**Risk.** The highest on the list, and the only iteration that changes an
invariant rather than adding to one:

- **Release stops being decidable from a value's static type.** Dropping a value
  means reading its tag first to know which payload, if any, to release. This is
  the first thing in comet whose layout is not fixed by its type.
- **ADR 0007's known limitation becomes reachable.** Cycles leak until weak refs
  exist; once an enum variant can hold a reference that is reachable from
  ordinary user code rather than theoretical.
- **The checker grows type parameters.** Less dramatic than the record implies:
  it calls this "the first thing not resolvable by a single pass over the tree",
  but `check.rs` already runs a declaration pre-pass collecting every signature
  before any body is checked, which is exactly where type parameters would be
  collected. The shape exists; the work is instantiation and call-site
  inference, not a new pass.

**ADR.** Sum types, `Option` and generics. Likely revises ADR 0007's memory
section.

**Proven by.** Execution tests that a variant holding a `String` frees it and
one holding an `int` does not - the tag-consulting release path is the thing
most likely to be quietly wrong. Exhaustiveness reported on a missing arm.

**Split into two.** 4.5a is enums with payloads and exhaustive `match`; 4.5b is
generics and `Option<T>`. Same decisions, same order - but the first half is
complete on its own (state machines are what decision 7 was argued for), and the
layout work it introduces is what the second builds on. One commit this size
would have been an unverifiable lump.

**4.5a done.** Decisions taken along the way:

- **An enum is a stack value, not an allocation**: a tag plus the widest
  variant's payload. Every payload slot is an `i32`, with an `f32` stored
  reinterpreted - one free instruction each way - because a uniform slot type is
  what lets one layout serve every variant.
- **`match` parks its result in locals** rather than using the `if` block's
  result. A result wider than one slot needs a function type in the type
  section, which is built long before any instruction is emitted; locals have no
  such limit.
- **`val_types` stopped being `&'static`.** An enum's width depends on the
  script, so the layout spine now threads the enum table through codegen. That
  was the bulk of the work and 4.5b inherits it.
- **The predicted refcount problem was faced in 4.5b**, and it took three
  places, not one - reading, binding, and the `match` subject itself.

Exported enums default to their first variant, per Philip's call, and only
payload-free enums are exportable: the tag is one number and a payload is not.
The inspector shows it as a number, which is provisional - a dropdown is 4.7's.

**4.5b done**, as ADR 0023. Generics are monomorphized at *check* time rather
than at emit: the checker substitutes and `TypedScript::enums` holds only
concrete types, so codegen never learns that a type parameter exists. Same
output shape as the decision asked for, and zero codegen change.

Bidirectional checking landed on Philip's call, kept to the smallest surface
that makes `Option::None` work: an expected type threaded to the six sites that
know one, read by exactly two things. Inference from a payload matches only a
bare parameter, which covers every generic the language has.

Two bugs worth recording from this half. The `holds_str` fixpoint was silently
lost in a rewrite and had to move to *after* function bodies anyway, because a
generic used only inside one does not exist until then. And the refcount tests
were wrong twice over: they put string *literals* in payloads - immortal, never
freed - so they passed with the whole fix disabled, and the leak oracle was too
coarse to see a 24-byte leak inside a 64KB page.

A bug worth recording: `collect_literals` had a `_ => {}` arm, so a string
inside a `match` was never interned and emission panicked with "every literal
was interned before emission". Found by the demo script, not by a test. The
wildcard is gone - every node kind is listed, so the next one that can hold a
string fails to compile instead.

---

## 4.6 - Containers: arrays and structs (DONE)

**Builds.** `Array<T>` as an ordinary generic declaration rather than a builtin,
indexing, and user-defined structs.

**Decide first - both of these are genuinely open.**
- **Array semantics.** Reference matches ADR 0006's "GC reference semantics for
  objects, value semantics for small structs" and is what Godot and Python do,
  at the cost of the aliasing surprise that is the classic Godot gotcha. Value
  is consistent with `Vec2` and surprises nobody, but copies on every pass
  unless the compiler grows escape analysis, which ADR 0007 rules out.
- **Whether structs are value or reference types.** Reference structs are the
  other place the cycle leak becomes reachable.

The record deliberately deferred the first of these to when arrays are actually
scheduled, rather than deciding ahead of the work. This is that point.

**Why here.** Needs `int` for honest indices (4.3) and generics for `Array<T>`
to be a declaration rather than a compiler special case (4.5). The M4 plan
deferred containers to "a fast-follow once the single-refcounted-type case is
proven", and `String` proved it - so this is due rather than new.

**Risk.** High. Two open semantic decisions, and the second place the memory
model gets harder.

**ADR.** Container semantics.

**Decided**, and both the way the plan leaned: arrays are **reference** types,
growable, with an explicit `copy` for when you meant one; structs are **value**
types. Indexing past the end traps, and `get` returns an `Option` - which is the
thing decision 8 said arrays would introduce.

**Split into two.** 4.6a is value structs, 4.6b is reference arrays. Structs
reuse the enum layout spine and need no new runtime machinery, so they are the
cheaper half and the foundation: the general field path 4.6a builds is what an
array's element access will want.

**4.6a done.** The shape of it:

- **A struct is exactly its fields**, laid out one after another with no header.
  Nesting is offsets, not pointers, so `o.inner.hp` reads one slot.
- **Places generalized from a Vec2 axis to an offset.** `Place::LocalField(slot,
  Axis)` became `Place::LocalAt { slot, offset, ty }`, and one rule now covers a
  Vec2 component, a struct field, and any nesting of the two. `TypedExprKind::
  Field` did the same.
- **Two bugs worth recording.** The frame gained an f32 park region for picking
  a field out of a value on the stack, and my edit to the declared-locals list
  silently did not apply - the region was addressed but never declared, so every
  struct read failed validation with "expected i32, found f32". And `place()`
  first keyed on the offset coming out zero rather than on whether there was a
  path at all: `o.inner.hp` sits at offset 0 and is still a *part* of `o`, so
  assigning to it stored one slot into a value two slots wide.

Generic structs are not built. The template machinery from 4.5 would carry them,
but nothing needs one yet - `Array<T>` is a builtin, not a user struct - and it
is the same "build ahead of the user" call made twice before.

**4.6b done**, as ADR 0024. An array is two blocks - a handle and its elements -
which is what makes growth invisible to an alias. `a[i]` traps, `get(a, i)`
gives an `Option<T>`, and `copy` is the way out of the aliasing.

**One thing is deliberately refused rather than built**: an array cannot hold
anything that owns a reference. Releasing an array frees its storage but does
not walk its elements, so `Array<String>` would leak. It is a diagnostic rather
than a leak, and the walk - a per-element-type drop, the same tag-consulting
problem 4.5b solved one level further out - is the follow-up.

Three bugs, all found by writing the demo or by disabling a fix:

- `comet_array_at` computed `data + index * width * 4` where `data` is the block
  pointer, so element zero sat on the allocator's own size field and writing it
  corrupted the free list. It read as a leak, not as a failure.
- `pack`/`unpack` handled `f32` and `Vec2` by name, so an array of structs stored
  raw floats into i32 slots. They walk the layout now.
- Frame depth has to count what a *place* needs, not only an expression, and
  `collect_literals` has to walk places too - a string inside `a[f("x")] = v`
  was never interned. Both are the same omission in two passes, and the second
  is the third time a wildcard or a missed arm has caused exactly this.

---

## 4.7 - The annotation set

**Builds.** Whichever of `@range`, `@step`, `@tooltip`, `@color`, `@asset`,
`@multiline`, `@readonly` make v1.

**Decide first.** Which ones exist. The argument for several at once is that
atlas already owns every widget each maps to - the slider, the drag-scrub
numeric field, the colour picker, the asset field with its drop target, the text
area - so this is wiring rather than new UI.

**Why here.** The `@` machinery lands in 4.4. This could fold into 4.4 if you
want the inspector complete in one pass; it is kept separate because it is the
one block that is pure breadth and can be cut entirely without leaving anything
half-built.

**Risk.** Low, and mostly atlas.

---

## 4.8 - The small remainder (DONE)

**Builds.** Tuples and multiple return values (WASM multi-value makes this
nearly free and it removes the need for out-parameters). `const` declarations,
distinct from mutable state. The warning set: unreachable code after `return`, a
function never called, shadowing.

**Decide first.** Whether required parameter and return annotations stay -
currently mandatory, defensible for single-pass checking, but an accident of
implementation rather than a written decision. And which warnings are worth
emitting.

**Risk.** Low, each independently shippable.

**ADR.** Only if the annotations question goes the other way.

**Done.** Philip's answers: annotations stay mandatory, and the warnings are
unreachable-after-return and never-called - not shadowing, which the language now
does legitimately in match arms and loop bodies and which would teach people to
ignore warnings.

- **`const`** is state nothing can assign to. It needs a value, since there is
  otherwise nothing to hold constant, and it cannot be `@export`ed - the
  inspector would own a value the script says never changes.
- **Required annotations stay**, recorded as a decision rather than left as an
  accident of implementation: they keep the checker one pass, they make a
  signature readable without reading the body, and being told the types is the
  point in a language meant to be learned from.
- **Tuples and multiple return values are deliberately not built.** Structs
  arrived in 4.6a and cover the need - `func f() -> Pair` returns two things
  without an out-parameter - so tuples would be sugar for a problem the language
  no longer has. The plan listed them when structs did not exist yet.

Adding the never-called warning made six existing test fixtures noisy, because
they used `func f()` for things that were about something else. That is the
warning working, and the fixtures now say `update`.

---

## 4.9 - The teaching surface (optional)

**Builds.** A panel showing the typed IR, or the emitted WASM, beside the
source.

**Decide first.** Whether to do it at all, and which of the two.

**Why it is on the list.** The pipeline is single-pass and the IR is already a
clean tree, so this is unusually cheap for how much it would back the word
"educational" - and the ideatank notes that nothing in the codebase yet serves
that claim. Cut it without consequence if it does not earn its place.

**Risk.** Low, pure atlas.

---

## Decisions to take, and when

Collected so none is discovered mid-iteration:

| Before | Question |
|---|---|
| 4.2 | Does `pos` survive as sugar for `transform.position`? |
| 4.2 | Is ADR 0016 revised in place or superseded? |
| 4.3 | Does `for i in a..b` become an int loop? |
| 4.4 | Confirm the default-value table for a bare `@export`. |
| 4.5 | How are generics compiled - monomorphize at emit, or otherwise? |
| 4.5 | Exported user-defined enums: first variant, exempt, or not exportable? |
| 4.6 | Arrays: reference or value semantics? |
| 4.6 | Structs: value or reference types? |
| 4.7 | Which annotations exist in v1? |
| 4.8 | Do required parameter and return annotations stay? |

## Proposed ADRs

The record lists "which of these become numbered ADRs" as still to decide. This
plan's answer, following the established pattern that 0006 and 0007 were both
written before the code they governed:

- **0020** - the path-based host surface and the host schema (4.2)
- **0016 revised** - dynamic reflected fields (4.2)
- **0021** - the numeric model (4.3)
- **0022** - `@export` and the inspector-owns-the-value model (4.4)
- **0023** - sum types, `Option` and generics (4.5); revises 0007's memory section
- **0024** - container semantics (4.6)

4.1, 4.7, 4.8 and 4.9 need none.

## Testing strategy

Unchanged from M4, because it worked: **the compiler is proven by execution, not
by structure.** The checker proves a script is consistent and wasmparser proves
the bytes are a well-formed module, but neither can tell whether a Vec2
subtraction emitted both lanes, whether a tag-consulting release freed the right
payload, or whether an `int` comparison used the signed opcode. Tests on a real
wasmtime engine against a fake host answer those, and several should be written
to fail if an operand were swapped.

Two additions for this phase:

- **Every iteration ends with every existing script still compiling.** That is
  the concrete meaning of decision 5's implicit widening and of `pos` surviving
  as sugar, and it is cheap to check.
- **When a test protects an invariant, break the fix and watch it fail** by
  exactly the wrong value before keeping it. Several M4-era tests turned out to
  assert nothing until this was done.

## What M5 inherits

Stated so the delay is accounted for. On finishing 4.4, M5's headline -
"hot-reloads it live while preserving reflected field values" - becomes a
sentence with content for scripts: the reflected fields are the exported
variables, the stored values are what survives a reload, and the migration path
walks the same `Reflect` contract as the inspector and the serializer. On
finishing 4.2, input has somewhere to live that is not another magic identifier.
Everything from 4.5 onward is language depth that M5 does not need, and is where
this plan could be cut short if Play starts to matter more than completeness.
