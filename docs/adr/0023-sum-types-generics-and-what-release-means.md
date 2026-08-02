# Sum types, generics, and what "release" means once a value's layout depends on its tag

Comet has user-defined enums that carry payloads, exhaustive `match` as an expression, generic type declarations monomorphized at check time, and `Option<T>` as an ordinary enum the language always has in scope. Releasing a value stops being decidable from its static type.

## Enums are stack values

A value is a tag followed by the widest variant's payload. No allocation, no pointer, no indirection - the same treatment `Vec2` gets. Every payload slot is an `i32`, with an `f32` stored reinterpreted, because a uniform slot type is what lets one layout serve every variant whichever is live, and reinterpreting is one instruction that changes no bits.

The consequence is that a type containing itself has no finite layout. `enum L<T> { Nil, Cons(T, L<T>) }` is reported rather than boxed silently: boxing it would make one type allocate for reasons invisible in its declaration.

## `match` is an expression, and exhaustive with no wildcard

An expression, so it can be a `let`'s value or a whole function body rather than needing a mutable variable assigned in every arm - the pattern the tail-expression rule already exists to avoid. Every arm therefore has the same type.

Exhaustive, with **no `_`**. A compiler that tells a learner they forgot a case is the point of the construct, and a wildcard is how that goes quiet. The cost is real for a large enum and was accepted; the message names the variants with no arm.

A `match` parks its result in locals rather than using the `if` block's result. A result wider than one slot needs a function type in the type section, and that section is built long before any instruction is emitted.

## Generics are monomorphized at check time

`Option<f32>` and `Option<Vec2>` are different types with different widths. The checker does the substitution and `TypedScript::enums` holds only concrete types, so **codegen never learns that a type parameter exists**. That is the cheapest possible version of the decision: no runtime type information, no boxing, and a `Vec2` inside an `Option` is still two f32 slots rather than a pointer.

Only enum declarations are generic. Generic functions would mean monomorphizing bodies, which is a much larger piece and is not needed by `Option` or by the `Array<T>` that follows.

## Bidirectional checking, kept to the smallest surface that works

`Option::Some(1.0)` infers `T` from what it carries. `Option::None` carries nothing, so there is nothing to infer from, and the alternatives were both bad: `Option<f32>::None` puts syntax in the most common position, and guessing is worse than either.

So an expected type is threaded to the sites that know one - an annotated `let`, an assignment, a call argument, a `return`, a function's tail expression, and a `match` arm - and exactly two things read it: constructing a generic variant, and a `match` whose arms build one. Everything else ignores it.

This is deliberately not a full expected-type discipline. That would be a second pass, against a checker whose headline property is being one (ADR 0007). Inference from a payload also only matches a bare parameter - `Some(T)` yes, `Some(Wrapper<T>)` no - which covers every generic the language has. When neither says, the error names the parameter and shows the annotation that would fix it.

## `Option` is declared, not known

It comes from a prelude parsed by the same parser as a script's own source. Decision 9 says it gets no special treatment in the checker or codegen, and the only way to be sure is for it to take the same path - so there is a test that parses the prelude and asserts it is one ordinary generic enum. If that test ever needs changing, `Option` has stopped being ordinary.

## Release consults the tag

This is the invariant sum types change, and the reason this ADR exists rather than being two smaller ones. Before, what to release was a property of a value's static type: a `String` released, everything else did not. With payload-carrying variants, dropping a value means reading its tag first, because which payload is live - and whether there is one at all - depends on it.

Three places need it, and each was verified by disabling it and watching tests fail:

- **Reading** a local or global that owns something makes the stack copy a second owner.
- **Binding** a payload in a `match` makes a second owner of it.
- **A `match` consumed its subject**, and nothing freed it. Released after the arms have run, so it frees only what no arm kept.

The dispatch is emitted only for enums that can hold a `String`. Whether one can is a property of the whole graph - an enum's payload may be another enum - so it settles by repetition, and it settles **after** function bodies are checked, because a generic used only inside one does not exist until then.

ADR 0007's known limitation is now reachable from ordinary user code: an enum variant holding a reference can form a cycle, and cycles leak until weak references or a collector exist. It was theoretical before this.

## Consequences

- Only payload-free enums can be `@export`ed, defaulting to their first variant (ADR 0022's rule, with the enum answer taken here). The tag is one number; a payload is not.
- `Type::name()` is not enough for a diagnostic - an enum is an index and only the checker holds the table - so the checker names types itself. Without that, a mismatch read "expected `enum`, found `enum`".
- The layout spine changed shape: `val_types` is no longer `&'static`, because an enum's width depends on the script. Every later container type inherits that.
