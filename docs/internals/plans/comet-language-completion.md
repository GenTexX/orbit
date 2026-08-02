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

## 4.1 - Vectors and lifecycle

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

---

## 4.2 - The host surface generalizes

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

---

## 4.3 - The numeric model

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

---

## 4.4 - Exported variables

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

---

## 4.5 - Sum types and generics

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

---

## 4.6 - Containers: arrays and structs

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

## 4.8 - The small remainder

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
